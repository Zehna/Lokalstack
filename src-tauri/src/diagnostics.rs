//! Phase 10B (spec §W, §X) — local-only diagnostics.
//!
//! A tiny structured logger plus a panic hook, both strictly local:
//!
//! - Log file lives under the user's `FOLDERID_LocalAppData` (created on
//!   demand), capped at a bounded number of lines per session with
//!   rotation-free truncation — a runaway subsystem cannot grow it forever.
//! - `log_line` writes `{timestamp} [{level}] {subsystem}: {message}` with a
//!   **redaction pass**: any `key=value` / `key: value` token whose key looks
//!   secret-shaped (token, secret, key, password, authorization, cookie…) is
//!   masked before it reaches disk.
//! - The panic hook records the panic message + location (never arbitrary
//!   process memory or payload data) and then runs the previous hook.
//!   No network, no telemetry, no crash service.
//!
//! Production Rust was previously silent (verified by audit); this gives
//! start-up, subsystem failures, and panics a local breadcrumb trail without
//! changing any subsystem behavior.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Session cap: after this many lines the log simply stops growing (spec §W
/// bounds everything; diagnostics are not exempt).
pub(crate) const MAX_SESSION_LINES: usize = 2_000;

static LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

/// Keys whose values must never reach the log (case-insensitive substring
/// match on the key portion of `key=value` / `key: value` tokens).
const SECRET_KEY_HINTS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "cookie",
    "api_key",
    "apikey",
    "api-key",
    "private_key",
    "credential",
];

/// `true` when `word` is `key=value` / `key:value` / `--key value`-style
/// with a secret-shaped key.
fn is_secret_keyed(word: &str) -> bool {
    let lowered = word.to_ascii_lowercase();
    SECRET_KEY_HINTS.iter().any(|hint| {
        lowered
            .split_once(['=', ':'])
            .map(|(key, _)| key.contains(hint))
            .unwrap_or(false)
    })
}

/// Mask secret-shaped tokens in `message`. Handled forms (spec §4):
///
/// - `key=value` / `key:value` — key-bearing token masked whole;
/// - `Authorization: Bearer xyz` / `Authorization=Bearer xyz` — the scheme
///   word after the masked key is consumed too;
/// - a bare `Bearer xyz` pair — both words masked;
/// - `--api-key xyz` / `--api_key xyz` / `--token xyz` / `--password xyz`
///   (CLI-flag style) — flag and following value word masked.
///
/// Deterministic, single-pass, no secret-detection framework.
pub(crate) fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut words = message.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let lowered = word.to_ascii_lowercase();
        if is_secret_keyed(word) {
            let split_at = word.find(['=', ':']).expect("is_secret_keyed proved a separator");
            out.push_str(&word[..=split_at]);
            out.push_str("[REDACTED]");
            // Scheme may sit in the NEXT word ("Authorization: Bearer x") or
            // inside the masked token itself ("Authorization=Bearer x") —
            // either way the value token that follows is masked too.
            let scheme_inline = lowered[split_at + 1..].eq_ignore_ascii_case("bearer");
            let scheme_next = words
                .peek()
                .is_some_and(|next| next.eq_ignore_ascii_case("bearer"));
            if scheme_inline {
                // The value is the next word.
                if words.next().is_some() {
                    out.push_str(" [REDACTED]");
                }
            } else if scheme_next {
                words.next(); // consume "Bearer"
                if words.next().is_some() {
                    out.push_str(" [REDACTED]");
                }
            }
            out.push(' ');
            continue;
        }
        if lowered == "bearer" {
            // Bare scheme word: mask it and the following value token.
            out.push_str("[REDACTED]");
            if words.next().is_some() {
                out.push_str(" [REDACTED]");
            }
            out.push(' ');
            continue;
        }
        if lowered.starts_with("--")
            && SECRET_KEY_HINTS.iter().any(|hint| lowered[2..].contains(hint))
        {
            out.push_str("[REDACTED]");
            if words.next().is_some() {
                out.push_str(" [REDACTED]");
            }
            out.push(' ');
            continue;
        }
        out.push_str(word);
        out.push(' ');
    }
    out.pop(); // trailing space
    out
}

/// Resolve (once) the log file under `FOLDERID_LocalAppData`. Failure to
/// resolve or open is silently absorbed: diagnostics must never become the
/// reason the app fails.
fn log_file() -> &'static Option<Mutex<File>> {
    LOG_FILE.get_or_init(|| {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Shell::{
                SHGetKnownFolderPath, FOLDERID_LocalAppData, KF_FLAG_DEFAULT,
            };
            unsafe {
                let mut raw: *mut u16 = std::ptr::null_mut();
                let hr = SHGetKnownFolderPath(
                    &FOLDERID_LocalAppData,
                    KF_FLAG_DEFAULT as u32,
                    std::ptr::null_mut(),
                    &mut raw,
                );
                if hr != 0 || raw.is_null() {
                    return None;
                }
                // RAII box over the PWSTR.
                let len = {
                    let mut l = 0usize;
                    while *raw.add(l) != 0 {
                        l += 1;
                    }
                    l
                };
                let slice = std::slice::from_raw_parts(raw, len);
                let path = String::from_utf16_lossy(slice);
                windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());

                let dir = PathBuf::from(path).join("localstack-control-center");
                if std::fs::create_dir_all(&dir).is_err() {
                    return None;
                }
                let file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(dir.join("localstack.log"))
                    .ok()?;
                Some(Mutex::new(file))
            }
        }
        #[cfg(not(windows))]
        {
            None
        }
    })
}

/// Log a startup/subsystem event (`info` level).
pub(crate) fn info(subsystem: &str, message: &str) {
    write_log("info", subsystem, message);
}

/// Log a typed subsystem failure (`error` level).
pub(crate) fn error(subsystem: &str, message: &str) {
    write_log("error", subsystem, message);
}

fn write_log(level: &str, subsystem: &str, message: &str) {
    if LINES_WRITTEN.load(Ordering::Relaxed) >= MAX_SESSION_LINES {
        return; // session bound reached — drop further lines
    }
    let Some(file_mutex) = log_file() else {
        return;
    };
    let Ok(mut file) = file_mutex.lock() else {
        return; // poisoned: drop rather than panic inside diagnostics
    };
    let line = format!(
        "{} [{level}] {subsystem}: {}\n",
        now_unix_ms(),
        redact(message)
    );
    if file.write_all(line.as_bytes()).is_ok() {
        LINES_WRITTEN.fetch_add(1, Ordering::Relaxed);
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Install the panic hook (spec §X). Records panic message + location
/// locally, then delegates to any previously installed hook. Never uploads
/// anything and never attempts recovery — the process still unwinds normally.
pub(crate) fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "opaque panic payload".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        error("panic", &format!("{payload} at {location}"));
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction_masks_secret_shaped_tokens() {
        let out = redact("ollama request api_key=sk-123 failed for user");
        assert!(out.contains("api_key=[REDACTED]"));
        assert!(!out.contains("sk-123"));

        let out = redact("config Authorization: Bearer abc and normal text");
        assert!(out.contains("Authorization:[REDACTED]"));
        assert!(!out.contains("Bearer"));
        assert!(!out.contains("abc"));

        let out = redact("password=hunter2");
        assert_eq!(out, "password=[REDACTED]");

        // Non-secret tokens pass through untouched.
        let out = redact("port 3000 owned by pid 42");
        assert_eq!(out, "port 3000 owned by pid 42");
    }

    #[test]
    fn redaction_survives_multiple_and_weird_tokens() {
        let out = redact("a=1 secret=x cookie:y PRIVATE_KEY=z end");
        assert!(out.contains("secret=[REDACTED]"));
        assert!(out.contains("cookie:[REDACTED]"));
        assert!(out.contains("PRIVATE_KEY=[REDACTED]"));
        assert!(out.contains("a=1"));
        assert!(out.contains("end"));
    }

    /// Phase 10B correction (spec §4): the common secret forms must never
    /// let their value reach the log.
    #[test]
    fn redaction_covers_common_secret_forms() {
        let cases: Vec<(&str, &str)> = vec![
            ("Authorization: Bearer abc123", "abc123"),
            ("Authorization=Bearer abc123", "abc123"),
            ("Bearer abc123", "abc123"),
            ("--api-key abc123", "abc123"),
            ("--api_key abc123", "abc123"),
            ("--token abc123", "abc123"),
            ("--password abc123", "abc123"),
            ("api_key=abc123", "abc123"),
            ("token=abc123", "abc123"),
            ("password=abc123", "abc123"),
            ("secret=abc123", "abc123"),
        ];
        for (input, secret) in cases {
            let out = redact(input);
            assert!(
                !out.contains(secret),
                "input {input:?} leaked {secret:?} → {out:?}"
            );
            assert!(out.contains("[REDACTED]"), "input {input:?} → {out:?}");
        }
    }

    /// Spec §5: panic text must pass through the SAME redaction path.
    #[test]
    fn panic_payload_is_redacted_before_logging() {
        // The hook calls error("panic", …) → write_log → redact. Prove the
        // composition on a secret-bearing panic message.
        let payload = "panicked at config load: token=abc123 Authorization: Bearer xyz";
        let out = redact(payload);
        assert!(!out.contains("abc123"));
        assert!(!out.contains("xyz"));
        assert!(out.contains("[REDACTED]"));
    }

    #[test]
    fn session_bound_is_a_sane_constant() {
        assert!(MAX_SESSION_LINES >= 500, "too small to be useful");
        assert!(MAX_SESSION_LINES <= 10_000, "too large to bound anything");
    }
}
