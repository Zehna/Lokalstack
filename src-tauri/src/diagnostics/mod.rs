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

pub(crate) mod bundle;
pub(crate) mod cache;
pub(crate) mod crypto;
pub(crate) mod ids;
pub(crate) mod emergency;
pub(crate) mod incidents;
pub(crate) mod redact_export;
pub(crate) mod paths;
pub(crate) mod recovery;
pub(crate) mod policy;
pub(crate) mod storage;
pub(crate) mod store;
pub(crate) mod worker;

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

/// Session cap: after this many lines the log simply stops growing (spec §W
/// bounds everything; diagnostics are not exempt).
pub(crate) const MAX_SESSION_LINES: usize = 2_000;

/// Cross-session cap (Phase 10D §S): the file is append-mode, so without a
/// size ceiling it would grow across many sessions. On open, a file larger
/// than this is truncated — old sessions' lines are pruned wholesale, the
/// current session starts fresh and stays far below the cap
/// (2_000 lines × ~120 B ≈ 240 KB worst case).
const MAX_FILE_BYTES: u64 = 256 * 1024;

static LINES_WRITTEN: AtomicUsize = AtomicUsize::new(0);
static LOG_FILE: OnceLock<Option<Mutex<File>>> = OnceLock::new();

/// Canonical `Path`-typed accessor over [`paths::local_app_data_dir`].
/// The `OnceLock` in `paths` memoizes the resolved directory for the
/// program lifetime, so re-borrowing it as `&'static Path` is sound.
/// (Consumed by the snapshot/cache tasks onward.)
#[allow(dead_code)]
pub(crate) fn app_data_path() -> Option<&'static std::path::Path> {
    paths::LOCAL_APP_DATA.get().and_then(|p| p.as_deref())
}

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

/// Mask PEM private-key blocks (`-----BEGIN ... PRIVATE KEY-----` through
/// `-----END ... PRIVATE KEY-----`) before word-level redaction. An
/// unterminated BEGIN block is redacted to the end of the text (fail-safe).
fn strip_pem_private_key_blocks(message: &str) -> String {
    const BEGIN: &str = "-----BEGIN";
    const END: &str = "-----END";
    if !message.contains(BEGIN) {
        return message.to_string();
    }
    let mut out = message.to_string();
    let mut search_from = 0usize;
    while let Some(rel) = out[search_from..].find(BEGIN) {
        let start = search_from + rel;
        // Confirm the header is a private-key block.
        let header_end = (out[start..].find('\n')).unwrap_or(out.len() - start) + start;
        if !out[start..header_end.min(out.len())].contains("PRIVATE KEY") {
            // Not private-key material (e.g. a public certificate): leave it
            // and keep scanning after this marker.
            search_from = start + BEGIN.len();
            continue;
        }
        let Some(end_rel) = out[start..].find(END) else {
            // Unterminated private-key block: redact to end of text.
            out.replace_range(start.., "[REDACTED-PRIVATE-KEY]");
            break;
        };
        let end = start + end_rel;
        let end_line_stop = out[end..]
            .find('\n')
            .map(|i| end + i)
            .unwrap_or(out.len());
        out.replace_range(start..end_line_stop, "[REDACTED-PRIVATE-KEY]");
        search_from = start;
    }
    out
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
    let message = strip_pem_private_key_blocks(message);
    let mut out = String::with_capacity(message.len());
    let mut words = message.split_whitespace().peekable();
    while let Some(word) = words.next() {
        let lowered = word.to_ascii_lowercase();
        if is_secret_keyed(word) {
            let split_at = word.find(['=', ':']).expect("is_secret_keyed proved a separator");
            out.push_str(&word[..=split_at]);
            out.push_str("[REDACTED]");
            // Header form ("Cookie: session=x"): the header VALUE that
            // follows is masked too. If that value is a scheme ("Bearer"),
            // the credential after the scheme is masked as well.
            if word.ends_with(':') {
                if let Some(next) = words.next() {
                    let was_bearer = next.eq_ignore_ascii_case("bearer");
                    out.push_str(" [REDACTED]");
                    if was_bearer && words.next().is_some() {
                        out.push_str(" [REDACTED]");
                    }
                }
                out.push(' ');
                continue;
            }
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

/// Resolve (once) the log file inside the shared app-data directory.
/// Failure to resolve or open is silently absorbed: diagnostics must never
/// become the reason the app fails.
fn log_file() -> &'static Option<Mutex<File>> {
    LOG_FILE.get_or_init(|| {
        local_app_data_dir().and_then(|dir| open_bounded_log(&dir).map(Mutex::new))
    })
}

/// Try-lock-based error breadcrumb for the panic path: identical line format
/// to `error()`, but never waits on the LOG_FILE mutex — busy/poisoned drops
/// the line instead of blocking inside a panic hook.
pub(crate) fn try_log_error_breadcrumb(message: &str) {
    if LINES_WRITTEN.load(Ordering::Relaxed) >= MAX_SESSION_LINES {
        return;
    }
    let Some(file_mutex) = log_file() else {
        return;
    };
    let Ok(mut file) = file_mutex.try_lock() else {
        return; // busy: drop rather than block inside the panic path
    };
    let line = format!(
        "{} [error] panic: {}\n",
        now_unix_ms(),
        redact(message)
    );
    if file.write_all(line.as_bytes()).is_ok() {
        LINES_WRITTEN.fetch_add(1, Ordering::Relaxed);
    }
}

/// Test seam: acquire the normal logger's file mutex so tests can prove the
/// emergency writer never depends on it.
#[cfg(test)]
pub(crate) fn log_file_lock_for_test() -> Option<std::sync::MutexGuard<'static, File>> {
    log_file().as_ref().map(|m| match m.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    })
}

/// Open (or create) the session log inside `dir`, enforcing the cross-session
/// size cap: an oversized file from previous sessions is truncated before
/// appending, so total log growth stays bounded forever. A file already
/// within the cap is preserved and appended to.
fn open_bounded_log(dir: &std::path::Path) -> Option<File> {
    let path = dir.join("localstack.log");
    let oversized = std::fs::metadata(&path).map(|m| m.len() > MAX_FILE_BYTES).unwrap_or(false);
    if oversized {
        // Truncate in place — same directory, same file, fresh content.
        File::create(&path).ok()?;
    }
    OpenOptions::new().create(true).append(true).open(&path).ok()
}

/// Log a startup/subsystem event (`info` level).
pub(crate) fn info(subsystem: &str, message: &str) {
    write_log("info", subsystem, message);
}

/// Log a recoverable/degraded condition (`warn` level) — e.g. settings
/// corruption recovery, startup-registration failure.
pub(crate) fn warn(subsystem: &str, message: &str) {
    write_log("warn", subsystem, message);
}

/// Resolve (once) the LocalStack app-data directory under
/// `FOLDERID_LocalAppData`, creating it on demand. Shared by diagnostics
/// (log file) and settings (settings.json) so both live in the same
/// LocalStack-owned location — never inside a source tree (spec §E).
///
/// Phase 11C: implementation moved to [`paths::local_app_data_dir`]; this
/// re-export preserves the exact existing call-site contract.
pub(crate) use paths::local_app_data_dir;

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

    /// Phase 10D (§S): the log file is append-mode, so total growth must be
    /// bounded ACROSS sessions too. An oversized file left by previous
    /// sessions is truncated on open; the original content is proven present
    /// first (the truncation is deliberate pruning, not data loss in a valid
    /// file). A within-cap file is preserved.
    #[test]
    fn oversized_log_file_is_truncated_on_open() {
        let dir = std::env::temp_dir().join(format!("lscc_diag_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("localstack.log");
        std::fs::write(&path, "x".repeat((MAX_FILE_BYTES * 2) as usize)).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            MAX_FILE_BYTES * 2,
            "setup: oversized file present before open"
        );

        let file = open_bounded_log(&dir).expect("log file opens");
        drop(file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            0,
            "oversized pre-existing log must be truncated on open"
        );

        // Within-cap file: preserved and appended to (never truncated).
        std::fs::write(&path, "session-start\n").unwrap();
        let file = open_bounded_log(&dir).expect("second open");
        drop(file);
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            "session-start\n".len() as u64,
            "within-cap file must be preserved"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Phase 10D (§S): worst-case session growth stays far below the file
    /// cap — the two bounds compose.
    #[test]
    fn session_line_bound_composes_with_file_cap() {
        // ~120 B is a generous average line; 2_000 × 120 B = 240 KB < 256 KB.
        assert!(
            (MAX_SESSION_LINES as u64) * 120 < MAX_FILE_BYTES,
            "a full session must fit under the file cap"
        );
    }
}
