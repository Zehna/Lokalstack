//! Phase 10B (spec §AC) — static security guards.
//!
//! These tests scan the production source tree for forbidden control
//! patterns. They are cheap, deterministic, and pin the trust boundary
//! against silent regressions: any accidental reintroduction of an
//! arbitrary-control primitive fails the suite.
//!
//! Excluded from scanning: this file (it quotes the patterns), and
//! `tests/` directories (test doubles may legitimately quote payloads).

use std::fs;
use std::path::Path;

/// Patterns that must never appear in production Rust source.
const FORBIDDEN_RUST: &[(&str, &str)] = &[
    // A console event to group id 0 broadcasts to every attached process —
    // forbidden since Phase 5 (managed stop targets only a known group).
    (
        "GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0)",
        "console event to group id 0 broadcasts to all attached processes",
    ),
    (
        "GenerateConsoleCtrlEvent(0)",
        "console event with an implicit group id",
    ),
    // No generic kill-by-pid escape hatch may exist as a Tauri command.
    (
        "fn kill_pid",
        "generic kill-by-pid command",
    ),
    // No generic shell execution primitives.
    (
        "fn shell_exec",
        "generic shell execution command",
    ),
    (
        "fn execute_command",
        "generic command execution command",
    ),
    // No generic HTTP/file/endpoint primitives that would break the
    // loopback-only AI policy and the Docker read-only allowlist.
    (
        "fn http_get",
        "generic arbitrary-URL HTTP command",
    ),
    (
        "fn probe_endpoint",
        "generic arbitrary-endpoint probe command",
    ),
    (
        "fn docker_request",
        "generic arbitrary Docker request command",
    ),
    // Docker lifecycle/exec must stay out of the native boundary.
    (
        "fn docker_exec",
        "Docker exec primitive",
    ),
    (
        "fn docker_stop_container",
        "Docker lifecycle primitive",
    ),
    (
        "fn docker_kill_container",
        "Docker lifecycle primitive",
    ),
];

/// Tauri command names that would expose raw-PID control to the frontend.
const FORBIDDEN_COMMAND_NAMES: &[&str] = &[
    "kill_process",
    "kill_by_pid",
    "stop_pid",
    "terminate_pid",
];

fn src_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn collect_rs_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }
}

/// Strip `//` line comments, `/* */` block comments, and string/char
/// literals so documentation and regression-guard test strings don't trip
/// pattern scans. Only executable code remains: a genuinely forbidden call
/// is code, never a string.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let bytes: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut in_block = false;
    while i < bytes.len() {
        if in_block {
            if i + 1 < bytes.len() && bytes[i] == '*' && bytes[i + 1] == '/' {
                in_block = false;
                out.push(' ');
                i += 2;
            } else {
                if bytes[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == '/' && bytes[i + 1] == '/' {
            while i < bytes.len() && bytes[i] != '\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == '/' && bytes[i + 1] == '*' {
            in_block = true;
            out.push(' ');
            i += 2;
        } else if bytes[i] == '"' || bytes[i] == '\'' {
            // Skip the literal (and its escapes) without emitting contents.
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() && bytes[i] != quote {
                if bytes[i] == '\\' && i + 1 < bytes.len() {
                    i += 1;
                }
                i += 1;
            }
            i += 1; // closing quote
            out.push(' ');
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    out
}

fn production_sources() -> Vec<(std::path::PathBuf, String)> {
    let mut files = Vec::new();
    collect_rs_files(&src_root(), &mut files);
    let self_file = Path::new(file!()).canonicalize().ok();
    let mut contents = Vec::new();
    for path in files {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if self_file.as_ref().is_some_and(|me| *me == canonical) {
            continue; // this file legitimately quotes the forbidden patterns
        }
        if let Ok(text) = fs::read_to_string(&path) {
            contents.push((path, strip_comments(&text)));
        }
    }
    contents
}

#[test]
fn no_forbidden_control_primitives_in_production_source() {
    let sources = production_sources();
    assert!(
        sources.len() > 10,
        "sanity: the source scan should cover the whole backend, found {}",
        sources.len()
    );
    for (path, text) in &sources {
        for (pattern, reason) in FORBIDDEN_RUST {
            assert!(
                !text.contains(pattern),
                "{} contains forbidden pattern {:?} — {}",
                path.display(),
                pattern,
                reason
            );
        }
    }
}

#[test]
fn no_raw_pid_tauri_commands_exist() {
    let sources = production_sources();
    for (path, text) in &sources {
        for name in FORBIDDEN_COMMAND_NAMES {
            assert!(
                !text.contains(&format!("fn {name}")),
                "{} defines a raw-PID Tauri command `{name}` — control must go through the opaque target registry only",
                path.display()
            );
        }
        // Every control-path Tauri command must take an opaque id, never a pid.
        if text.contains("#[tauri::command]") {
            for line in text.lines() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("fn end_process")
                    || trimmed.starts_with("fn stop_managed_service")
                    || trimmed.starts_with("fn restart_managed_service")
                {
                    let sig = trimmed.to_string();
                    assert!(
                        !sig.contains("pid: u32") && !sig.contains("pid: i32") && !sig.contains("pid:"),
                        "{}: control command `{}` accepts a raw pid — must accept opaque target/managed ids only",
                        path.display(),
                        trimmed
                    );
                }
            }
        }
    }
}

#[test]
fn managed_graceful_stop_never_targets_group_zero() {
    // The managed stop path must validate the stored process group before
    // any console event: group id 0 is a broadcast, not a target.
    let sources = production_sources();
    for (path, text) in &sources {
        if text.contains("GenerateConsoleCtrlEvent") {
            let has_guard = text.contains("group_id == 0")
                || text.contains("process_group == 0")
                || text.contains("group_id != 0")
                || text.contains("assert_ne!(group_id, 0)")
                || text.contains("!= 0");
            assert!(
                has_guard,
                "{} sends console control events without a group-0 guard",
                path.display()
            );
        }
    }
}
