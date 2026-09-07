//! Windows-native process launching and managed lifecycle FFI.
//!
//! # Safety and scope boundaries
//!
//! - All `unsafe` FFI of the workspace engine is confined to this module.
//! - Managed children are launched with `CreateProcessW` and the flags
//!   `CREATE_NEW_PROCESS_GROUP` (+ `CREATE_NO_WINDOW` in release builds,
//!   where LocalStack itself has no console to inherit):
//!   - the **process-group id equals the root PID** and is stored in the
//!     managed registry — it is the ONLY target a console event is ever
//!     sent to (never 0, never a guessed id);
//! - stdout/stderr are captured through `CreatePipe` + `STARTF_USESTDHANDLES`;
//!   dedicated reader threads own the read ends and close them at EOF.
//!   Nothing leaks: every handle is owned exactly once.
//! - **Graceful stop is targeted**: `AttachConsole(rootPid)` (with
//!   LocalStack ignoring CTRL_C/CTRL_BREAK for itself while attached),
//!   `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, known_group_id)` where
//!   `known_group_id == rootPid` from our own launch, then `FreeConsole`.
//!   Group id 0 (console-wide broadcast) is refused structurally.
//! - The child's process handle from `CreateProcessW` is closed inside the
//!   launch call itself. All later monitoring re-opens the PID with
//!   minimal rights (`PROCESS_SYNCHRONIZE` /
//!   `PROCESS_QUERY_LIMITED_INFORMATION`) and verifies the stored creation
//!   time, so the registry never stores native handles (nothing to leak,
//!   no Send/Sync hazards) and a reused PID cannot impersonate a dead one.
//! - `TerminateProcess` is the force path only, gated by the caller's
//!   authorization chain (identity revalidation happened before calling).
//! - No elevation, no tree kills, no environment reading or logging.

use std::ffi::OsStr;
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::sync::mpsc::Sender;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_TIMEOUT,
    FILETIME, ERROR_ACCESS_DENIED,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler, CTRL_BREAK_EVENT,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess,
    WaitForSingleObject, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW,
    CREATE_UNICODE_ENVIRONMENT, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, STARTF_USESTDHANDLES, STARTUPINFOW,
};

use super::rules::{LaunchSpec, LogLine, ProgramKind};

/// RAII handle guard — closes on drop.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: valid handle owned exclusively by this guard.
            unsafe { CloseHandle(self.0) };
        }
    }
}

impl HandleGuard {
    fn as_raw(&self) -> HANDLE {
        self.0
    }
}

/// Encode a Rust string as a NUL-terminated UTF-16 buffer.
fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// Quote one argument for a Windows command line.
///
/// Bare when it contains no spaces/quotes/tabs; otherwise wrapped in `"`
/// with the backslash-doubling and `\"` escaping rules. Only used to
/// assemble command lines from structured args — never raw strings.
fn quote_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| matches!(c, ' ' | '"' | '\t')) {
        return arg.to_string();
    }
    let mut quoted = String::with_capacity(arg.len() + 3);
    quoted.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                backslashes = 0;
                quoted.push('"');
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes));
    quoted.push('"');
    quoted
}

/// Command line for a real executable: quoted program + quoted args.
fn exe_command_line(program: &str, args: &[String]) -> String {
    let mut line = String::from("\"");
    line.push_str(program);
    line.push('"');
    for arg in args {
        line.push(' ');
        line.push_str(&quote_arg(arg));
    }
    line
}

/// Assemble the `cmd.exe /d /s /c` command line for a batch launcher
/// (`npm.cmd` & friends cannot be executed directly by `CreateProcessW`):
/// `cmd.exe /d /s /c "<batch-path>" <arg> <arg> …`. `/d` disables AutoRun
/// (no user-profile interference), `/s` fixes quote-stripping semantics.
/// Every argument is individually quoted by [`quote_arg`].
pub(crate) fn batch_command_line(batch_path: &str, args: &[String]) -> String {
    exe_command_line(batch_path, args)
}

/// Result of one successful launch.
pub(crate) struct LaunchedProcess {
    pub root_pid: u32,
}

/// Launch a managed process with a dedicated process group.
///
/// Returns the root PID; the process-group id equals it by
/// `CREATE_NEW_PROCESS_GROUP` semantics. The child's handles are closed
/// here — callers monitor by PID + creation time afterwards.
pub(crate) fn launch_managed(
    spec: &LaunchSpec,
    log_tx: Option<&Sender<LogLine>>,
) -> Result<LaunchedProcess, String> {
    let batch = spec.kind == ProgramKind::Batch;

    // `CreateProcessW`'s lpApplicationName does NOT search PATH — a bare
    // `node.exe` fails with ERROR_FILE_NOT_FOUND. Resolve the program the
    // same way `validate_spec` did so both gates agree on the executable.
    let path_entries: Vec<String> = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let resolved = super::rules::resolve_program(&spec.program, &path_entries, &|p| {
        std::path::Path::new(p).exists()
    })
    .ok_or_else(|| format!("Launch program could not be resolved: {}", spec.program))?;
    let program_path = resolved.path;

    // Application + command line. Batch launchers run via
    // `cmd.exe /d /s /c` (the one unavoidable shell hop, scoped to a
    // command line assembled from structured parts).
    let application: Vec<u16> = if batch {
        wide(r"C:\Windows\System32\cmd.exe")
    } else {
        wide(&program_path)
    };
    let command_line = if batch {
        let mut line = String::from("\"");
        line.push_str(r"C:\Windows\System32\cmd.exe");
        line.push_str("\" /d /s /c ");
        line.push_str(&batch_command_line(&program_path, &spec.args));
        line
    } else {
        exe_command_line(&program_path, &spec.args)
    };

    // Pipes for stdout/stderr when output capture is requested.
    let mut stdout_read: HANDLE = std::ptr::null_mut();
    let mut stdout_write: HANDLE = std::ptr::null_mut();
    let mut stderr_read: HANDLE = std::ptr::null_mut();
    let mut stderr_write: HANDLE = std::ptr::null_mut();
    let piped = log_tx.is_some();

    if piped {
        let mut security = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>()).unwrap_or(0),
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        // SAFETY: valid attribute pointer + out-handles.
        let ok = unsafe { CreatePipe(&mut stdout_read, &mut stdout_write, &mut security, 0) };
        if ok == 0 {
            return Err("CreatePipe(stdout) failed.".to_string());
        }
        // SAFETY: valid attribute pointer + out-handles.
        let ok = unsafe { CreatePipe(&mut stderr_read, &mut stderr_write, &mut security, 0) };
        if ok == 0 {
            unsafe { CloseHandle(stdout_read) };
            unsafe { CloseHandle(stdout_write) };
            return Err("CreatePipe(stderr) failed.".to_string());
        }
        // The parent's read ends must NOT leak into the child.
        // SAFETY: valid handles, single-flag mask.
        unsafe {
            SetHandleInformation(stdout_read, HANDLE_FLAG_INHERIT, 0);
            SetHandleInformation(stderr_read, HANDLE_FLAG_INHERIT, 0);
        }
    }

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = u32::try_from(std::mem::size_of::<STARTUPINFOW>()).unwrap_or(0);
    if piped {
        startup.dwFlags = STARTF_USESTDHANDLES;
        startup.hStdInput = std::ptr::null_mut();
        startup.hStdOutput = stdout_write;
        startup.hStdError = stderr_write;
    }

    let mut flags = CREATE_NEW_PROCESS_GROUP | CREATE_UNICODE_ENVIRONMENT;
    if cfg!(not(debug_assertions)) {
        // Release builds have no console (`windows_subsystem = "windows"`):
        // give the child its own hidden console so console-event delivery
        // later is well-defined and scoped to the child.
        flags |= CREATE_NO_WINDOW;
    }

    let mut command_wide = wide(&command_line);
    let cwd_wide = wide(&spec.cwd);
    let mut process_info = unsafe { std::mem::zeroed::<windows_sys::Win32::System::Threading::PROCESS_INFORMATION>() };

    // SAFETY: every pointer is a valid, NUL-terminated structure; the
    // command line is a mutable wide buffer as CreateProcessW requires;
    // the environment is inherited (Phase 6 rule: no env manipulation).
    let ok = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_wide.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            i32::from(piped), // inherit handles only when pipes are handed over
            flags,
            std::ptr::null_mut(),
            cwd_wide.as_ptr(),
            &mut startup,
            &mut process_info,
        )
    };

    // Close the parent's write ends immediately so reader threads see EOF
    // when the child exits.
    if piped {
        unsafe { CloseHandle(stdout_write) };
        unsafe { CloseHandle(stderr_write) };
    }

    if ok == 0 {
        if piped {
            unsafe { CloseHandle(stdout_read) };
            unsafe { CloseHandle(stderr_read) };
        }
        // SAFETY: plain error fetch.
        let err = unsafe { GetLastError() };
        return Err(format!("CreateProcessW failed (Win32 error {err})."));
    }

    let root_pid = process_info.dwProcessId;
    // SAFETY: both handles returned by CreateProcessW are owned exactly
    // once; the thread handle is never needed and the process handle is
    // released here (monitoring re-opens by PID with minimal rights).
    unsafe {
        CloseHandle(process_info.hThread);
        CloseHandle(process_info.hProcess);
    }

    if let Some(tx) = log_tx {
        spawn_reader(stdout_read, "stdout", tx.clone());
        spawn_reader(stderr_read, "stderr", tx.clone());
    }

    Ok(LaunchedProcess { root_pid })
}

/// Spawn a reader thread that owns `read_end`, forwarding lines tagged by
/// stream. The thread closes the handle when the pipe reaches EOF (child
/// exit) — the `File` owns the handle, so there is no double close.
fn spawn_reader(read_end: HANDLE, stream: &'static str, tx: Sender<LogLine>) {
    use std::os::windows::io::FromRawHandle;
    // HANDLE is a raw pointer (not Send); move it as an integer and
    // reconstruct inside the thread.
    let handle_value = read_end as isize;
    std::thread::spawn(move || {
        let read_end = handle_value as HANDLE;
        // SAFETY: the raw handle is exclusively owned from here on (the
        // parent never touches it again); the File closes it exactly once
        // on drop.
        let mut file = unsafe { std::fs::File::from_raw_handle(read_end) };
        let mut buf: Vec<u8> = Vec::with_capacity(4096);
        let mut chunk = [0u8; 4096];
        loop {
            match file.read(&mut chunk) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
                        let mut line: Vec<u8> = buf.drain(..=pos).collect();
                        line.pop(); // '\n'
                        if line.last() == Some(&b'\r') {
                            line.pop();
                        }
                        let text = String::from_utf8_lossy(&line).into_owned();
                        let _ = tx.send(LogLine { at: now_unix_ms(), stream, line: text });
                    }
                    // Bound the partial-line buffer against pathological
                    // output (no newline for megabytes).
                    if buf.len() > 64 * 1024 {
                        let text = String::from_utf8_lossy(&buf).into_owned();
                        buf.clear();
                        let _ = tx.send(LogLine { at: now_unix_ms(), stream, line: text });
                    }
                }
                Err(_) => break,
            }
        }
        // `file` drops here → handle closed exactly once.
    });
}

/// Unix ms right now (log timestamps and registry times).
pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// PID-based monitoring (no retained handles)
// ---------------------------------------------------------------------------

/// Open a PID with minimal rights, verifying its creation time matches
/// `expected_ms` when both are known — a reused PID can never impersonate
/// the managed process. Runs `f` with the guarded handle.
fn with_process<R>(
    pid: u32,
    rights: u32,
    expected_creation_ms: Option<u64>,
    f: impl FnOnce(HANDLE) -> R,
) -> Option<R> {
    // The creation-time identity check calls GetProcessTimes, which needs
    // PROCESS_QUERY_LIMITED_INFORMATION — a terminate-only handle would
    // make every identity check fail (found by the live test). Always OR
    // in the read-only query right.
    let rights = rights | PROCESS_QUERY_LIMITED_INFORMATION;
    // SAFETY: minimal read rights; the guard closes the handle.
    let handle = unsafe { OpenProcess(rights, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let guard = HandleGuard(handle);
    if let Some(expected) = expected_creation_ms {
        // SAFETY: valid handle + out-params for GetProcessTimes.
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let mut creation = zero;
        let mut exit = zero;
        let mut kernel = zero;
        let mut user = zero;
        let ok = unsafe { GetProcessTimes(guard.as_raw(), &mut creation, &mut exit, &mut kernel, &mut user) };
        if ok == 0 {
            return None;
        }
        let actual = crate::process::sampler::filetime_to_unix_ms(creation.dwLowDateTime, creation.dwHighDateTime);
        if actual != Some(expected) {
            return None; // PID reuse or identity drift — refuse
        }
    }
    Some(f(guard.as_raw()))
}

/// Whether the managed root process is still alive (with creation-time
/// identity verification).
pub(crate) fn is_alive(pid: u32, expected_creation_ms: Option<u64>) -> bool {
    with_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, expected_creation_ms, |_| ()).is_some()
}

/// Whether *any* process currently owns `pid` (no identity check) — used by
/// the monitor's zombie sweep only; lifecycle actions always revalidate the
/// stored creation time instead.
pub(crate) fn pid_exists(pid: u32) -> bool {
    is_alive(pid, None)
}

/// Exit code of an exited process (`None` while still running or when the
/// identity does not match).
pub(crate) fn exit_code(pid: u32, expected_creation_ms: Option<u64>) -> Option<u32> {
    with_process(pid, PROCESS_QUERY_LIMITED_INFORMATION, expected_creation_ms, |handle| {
        let mut code: u32 = 0;
        // SAFETY: valid handle + valid out-pointer.
        let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
        if ok == 0 || code == 259 {
            // 259 = STILL_ACTIVE — never report as an exit code.
            return None;
        }
        Some(code)
    })?
}

/// Wait (bounded) for the managed process to exit. `true` = exited.
///
/// The wait loop runs **inside** `with_process`'s closure: the guarded
/// handle is only valid while that closure runs (the guard closes it on
/// return), so no raw handle ever escapes the guard's lifetime.
pub(crate) fn wait_for_exit(pid: u32, expected_creation_ms: Option<u64>, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    with_process(pid, PROCESS_SYNCHRONIZE, expected_creation_ms, |handle| {
        loop {
            // SAFETY: valid, PROCESS_SYNCHRONIZE-capable handle guarded by
            // the with_process wrapper for this whole loop.
            let state = unsafe { WaitForSingleObject(handle, 100) };
            if state != WAIT_TIMEOUT {
                return true; // signaled (exited) or failed wait → gone
            }
            if started.elapsed() >= timeout {
                return false;
            }
        }
    })
    .unwrap_or(true) // cannot open with matching identity → effectively gone
}

// ---------------------------------------------------------------------------
// Console + termination
// ---------------------------------------------------------------------------

/// Graceful stop of a **managed** process: targeted `CTRL_BREAK_EVENT` to
/// the known process-group id (== root PID) captured at launch. Group id 0
/// is refused structurally — it would be a console-wide broadcast.
///
/// Documented Windows behavior, step by step:
/// 1. `AttachConsole(root_pid)` — attach to the *child's* console. The
///    child got its own console at launch (hidden in release builds), so
///    attaching does not touch any unrelated process. Fails when the child
///    already exited → reported honestly, no event sent.
/// 2. `SetConsoleCtrlHandler(NULL, TRUE)` — LocalStack ignores
///    CTRL_C/CTRL_BREAK for itself while attached, so the event cannot
///    kill LocalStack.
/// 3. `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, group_id)` with the
///    stored group id. Windows delivers to the process group only — the
///    group contains exactly what we launched into it (the root; children
///    that later call `CreateProcess` inherit it only if they pass the
///    group along, which dev servers do not).
/// 4. `SetConsoleCtrlHandler(NULL, FALSE)` + `FreeConsole` — always, on
///    every exit path.
pub(crate) fn graceful_stop(root_pid: u32, group_id: u32) -> Result<(), String> {
    if group_id == 0 {
        return Err("Refusing to send a console event to group id 0 (broadcast).".to_string());
    }
    // SAFETY: documented console sequence; the ignore-handler is restored
    // on every path, and FreeConsole runs only when we actually attached.
    unsafe {
        if AttachConsole(root_pid) != 0 {
            // Attached to the child's own console (release builds: the
            // child got one via CREATE_NO_WINDOW). Send, restore, detach.
            SetConsoleCtrlHandler(None, 1);
            let sent = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, group_id);
            SetConsoleCtrlHandler(None, 0);
            FreeConsole();
            if sent == 0 {
                let err = GetLastError();
                return Err(format!("GenerateConsoleCtrlEvent failed (Win32 error {err})."));
            }
            return Ok(());
        }
        let attach_err = GetLastError();
        if attach_err == ERROR_ACCESS_DENIED {
            // We already share a console with the child (debug/test builds
            // inherit the parent's console). We are attached to the very
            // console that contains the child's group, so the targeted
            // send works without attach/detach — still group-scoped,
            // still never group 0.
            SetConsoleCtrlHandler(None, 1);
            let sent = GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, group_id);
            SetConsoleCtrlHandler(None, 0);
            if sent == 0 {
                let err = GetLastError();
                return Err(format!("GenerateConsoleCtrlEvent failed (Win32 error {err})."));
            }
            return Ok(());
        }
        return Err(format!(
            "The managed process's console is not attachable (Win32 error {attach_err}; it may have already exited)."
        ));
    }
}

/// Force-terminate the managed root process only (never a tree), after the
/// caller revalidated identity and the user explicitly confirmed.
pub(crate) fn force_terminate(root_pid: u32, expected_creation_ms: Option<u64>) -> Result<(), String> {
    with_process(root_pid, PROCESS_TERMINATE, expected_creation_ms, |handle| {
        // SAFETY: valid handle with the terminate right; exit code 1.
        let ok = unsafe { TerminateProcess(handle, 1) };
        if ok == 0 {
            let err = unsafe { GetLastError() };
            Err(format!("TerminateProcess failed (Win32 error {err})."))
        } else {
            Ok(())
        }
    })
    .unwrap_or_else(|| Err("The managed process could not be reopened (it may have exited).".to_string()))
}

// ---------------------------------------------------------------------------
// Tests (pure parts — FFI is exercised by the live ignored test)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_args_stay_bare() {
        assert_eq!(quote_arg("run"), "run");
        assert_eq!(quote_arg("dev"), "dev");
    }

    #[test]
    fn spaced_args_are_quoted() {
        assert_eq!(quote_arg("hello world"), "\"hello world\"");
    }

    #[test]
    fn embedded_quotes_and_backslashes_escape() {
        // Built from char parts so the escaping rules themselves are what
        // the test documents: a backslash right before a quote becomes
        // 2n+1 backslashes (the pair doubles, plus the quote escape).
        let bs = '\\'; // one literal backslash
        let q = '"';
        // Input:  say "hi"   →  Output: "say \"hi\""  (1 backslash/quote)
        let input = format!("say {q}hi{q}");
        let expected = format!("{q}say {bs}{q}hi{bs}{q}{q}");
        assert_eq!(quote_arg(&input), expected);
        // Backslashes not before a quote pass through.
        assert_eq!(quote_arg(&format!("a{bs}b")), format!("a{bs}b"));
        // Input:  a\"b  →  Output: quoted, 3 backslashes, escaped quote.
        let input2 = format!("a{bs}{q}b");
        let expected2 = format!("{q}a{bs}{bs}{bs}{q}b{q}");
        assert_eq!(quote_arg(&input2), expected2);
    }

    #[test]
    fn exe_command_line_quotes_program_and_args() {
        let line = exe_command_line(r"C:\tools\svc.exe", &["--name".into(), "my app".into()]);
        assert_eq!(line, "\"C:\\tools\\svc.exe\" --name \"my app\"");
    }

    #[test]
    fn batch_command_line_passes_args_through() {
        let line = batch_command_line(r"C:\Program Files\nodejs\npm.cmd", &["run".into(), "dev".into()]);
        // `quote_arg` leaves unambiguous args bare — no needless quoting.
        assert_eq!(line, "\"C:\\Program Files\\nodejs\\npm.cmd\" run dev");
    }
}
