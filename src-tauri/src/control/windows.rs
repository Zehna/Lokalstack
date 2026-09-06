//! Windows-native control primitives for Phase 5 (hardened).
//!
//! # Safety and scope boundaries
//!
//! - All `unsafe` FFI of the control engine is confined to this module.
//! - **No console control events, ever.** The earlier draft used
//!   `AttachConsole(pid)` + `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0)`.
//!   Windows delivers group id `0` to *every* process sharing the attached
//!   console — a broadcast LocalStack cannot scope to the selected process
//!   (and PID is not a process-group id). That call has been **removed**;
//!   a source-level regression test in `mod.rs` greps this file to keep it
//!   out. For externally discovered processes there is no targeted graceful
//!   signal; the honest capability is `gracefulStopSupported: false` and the
//!   user-confirmed *End Process* termination below. A future
//!   LocalStack-launched process group (`CREATE_NEW_PROCESS_GROUP`, Phase 6)
//!   can restore targeted `CTRL_BREAK` delivery against its *known* group id.
//! - **End Process** targets exactly one PID with `PROCESS_TERMINATE` after
//!   the caller has resolved an opaque registry target, revalidated identity,
//!   and recomputed policy. The handle is RAII-guarded.
//! - **Open** uses `ShellExecuteW` with the `open` verb (default browser).
//! - Every handle is closed exactly once; no elevation, no tree kill.

use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, WAIT_TIMEOUT, HANDLE};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess,
    WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;

use crate::process::sampler::filetime_to_unix_ms;
use crate::process::windows::ProcessHandle;

/// How long the post-termination wait observes the PID before reporting the
/// outcome to the user.
pub(crate) const TERMINATION_WAIT_MS: u64 = 3_000;

/// RAII wrapper for a handle opened with termination rights.
struct TerminableHandle(HANDLE);

impl Drop for TerminableHandle {
    fn drop(&mut self) {
        // SAFETY: valid, exclusively-owned handle from OpenProcess.
        unsafe { CloseHandle(self.0) };
    }
}

/// Reopen the process and re-inspect its identity immediately before an
/// action. Returns `(image path, creation time in Unix ms)` or `None` when
/// the process cannot be inspected (dead, or now protected).
pub(crate) fn revalidate(pid: u32) -> Option<(Option<String>, Option<u64>)> {
    // SAFETY: read-only rights; the handle is guarded and closed on drop.
    let guard = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if guard.is_null() {
        return None;
    }
    let owned = unsafe { ProcessHandle::own(guard) };
    let handle = owned.as_raw();
    // SAFETY: valid open handle for both read-only queries below.
    unsafe {
        let path = query_image_path(handle);
        let creation = query_creation_ms(handle);
        drop(owned);
        Some((path, creation))
    }
}

/// `QueryFullProcessImageNameW` — narrow copy because `process::windows`
/// keeps its helpers private by design.
// SAFETY (callers): handle must be a valid open process handle.
unsafe fn query_image_path(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 1024];
    let mut size: u32 = buffer.len() as u32;
    // SAFETY: buffer/size are a valid writable pair for the call.
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size)
    };
    if ok == 0 || size == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..size as usize]))
}

/// Creation time as Unix ms via `GetProcessTimes`.
// SAFETY (callers): handle must be a valid open process handle.
unsafe fn query_creation_ms(handle: HANDLE) -> Option<u64> {
    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut creation = zero;
    let mut exit = zero;
    let mut kernel = zero;
    let mut user = zero;
    // SAFETY: valid out-params.
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    filetime_to_unix_ms(creation.dwLowDateTime, creation.dwHighDateTime)
}

/// Whether the process still exists (cheap existence probe).
pub(crate) fn is_running(pid: u32) -> bool {
    // SAFETY: read-only rights; the guard closes the handle immediately.
    let guard = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if guard.is_null() {
        return false;
    }
    drop(unsafe { ProcessHandle::own(guard) });
    true
}

/// Wait until the process exits or the timeout elapses.
/// Returns `true` when the process is gone.
pub(crate) fn wait_for_exit(pid: u32, timeout: Duration) -> bool {
    // SAFETY: PROCESS_SYNCHRONIZE is a read-only right; the handle is guarded.
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return true; // cannot even open → already gone
    }
    let _guard = unsafe { ProcessHandle::own(handle) };
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: valid handle with the synchronize right.
        let state = unsafe { WaitForSingleObject(handle, 100) };
        if state != WAIT_TIMEOUT {
            return true; // signaled (exited) or failed → treat as gone
        }
        if Instant::now() >= deadline {
            return false;
        }
    }
}

/// Terminate exactly this process. Returns `true` when `TerminateProcess`
/// succeeded; the caller re-checks actual exit via liveness.
///
/// The caller must already have: resolved an opaque registry target,
/// revalidated the identity, and recomputed eligibility. This function
/// performs none of those checks itself — it is the last step of the chain,
/// not the policy.
pub(crate) fn terminate(pid: u32) -> bool {
    // SAFETY: PROCESS_TERMINATE is the only write capability this module
    // requests, and only after the caller's full authorization chain.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let guarded = TerminableHandle(handle);
    // SAFETY: valid handle with the terminate right; 1 is the exit code.
    let ok = unsafe { TerminateProcess(handle, 1) };
    drop(guarded);
    ok != 0
}

/// `open` verb for `ShellExecuteW`, as a NUL-terminated UTF-16 constant.
const OPEN_VERB: &[u16] = &['o' as u16, 'p' as u16, 'e' as u16, 'n' as u16, 0];

/// Open a URL in the user's default browser.
pub(crate) fn open_url(url: &str) -> Result<(), String> {
    let mut wide: Vec<u16> = url.encode_utf16().collect();
    wide.push(0);
    // SAFETY: all string arguments are valid NUL-terminated wide strings for
    // the duration of the call; ShellExecuteW does not retain them. A return
    // value > 32 indicates success per the documented contract.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            OPEN_VERB.as_ptr(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    if (result as isize) <= 32 {
        return Err(format!("Windows could not open {url} (ShellExecute error)."));
    }
    Ok(())
}
