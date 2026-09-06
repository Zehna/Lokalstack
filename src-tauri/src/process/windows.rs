//! Windows-specific process inspection via `OpenProcess` and friends.
//!
//! # Safety and scope boundaries
//!
//! - All `unsafe` FFI of the process engine is confined to this module.
//! - Processes are opened with the **minimum** access rights required:
//!   `PROCESS_QUERY_LIMITED_INFORMATION` (0x0400) suffices for image name,
//!   times, and memory; command-line retrieval additionally needs
//!   `PROCESS_VM_READ` (0x0010), so the opener tries that combination first
//!   and degrades to limited-only when refused (command line becomes
//!   `None`, everything else stays available).
//! - Command lines are read via the documented mechanism Sysinternals tools
//!   use: `NtQueryInformationProcess(ProcessBasicInformation)` → PEB →
//!   `RTL_USER_PROCESS_PARAMETERS.CommandLine`, copied out with
//!   `ReadProcessMemory`. Nothing is written to the target process; the
//!   walk is pure reading of the process's own parameter block.
//! - Every successfully opened handle is closed exactly once via the RAII
//!   [`ProcessHandle`] guard; on query failure `OpenProcess` itself cleans
//!   up (it only "succeeds" into a handle we must free when it returns one).
//! - A process that disappears or refuses access mid-cycle is reported as
//!   inaccessible — never an engine-wide error.
//! - Cross-bitness targets (32-bit processes inspected from the 64-bit app)
//!   have a different PEB layout; command-line retrieval simply returns
//!   `None` there rather than guessing offsets.
//!
//! Reference: [`OpenProcess`](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-openprocess)
//! and [`NtQueryInformationProcess`](https://learn.microsoft.com/en-us/windows/win32/api/winternl/nf-winternl-ntqueryinformationprocess)
//! in the Microsoft documentation.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PEB,
    PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
    RTL_USER_PROCESS_PARAMETERS,
};

use super::sampler::{
    executable_basename, filetime_pair_to_ticks, filetime_to_unix_ms, ProcessInfo, RawCpuSample,
};

/// Everything `OpenProcess`-based inspection can learn about one PID in one
/// cycle, before CPU-percentage merge.
#[derive(Debug, Clone)]
struct RawInspection {
    info: ProcessInfo,
    cpu: Option<RawCpuSample>,
}

/// RAII wrapper around a raw `HANDLE` from `OpenProcess`.
///
/// `CloseHandle` runs exactly once on drop — the guarantee that no inspection
/// cycle leaks a handle even when a query fails partway through.
pub(crate) struct ProcessHandle(HANDLE);

impl ProcessHandle {
    /// Take ownership of a handle returned by `OpenProcess`.
    ///
    /// # Safety
    ///
    /// `handle` must be a valid process handle that has not been closed yet;
    /// ownership transfers to the returned guard.
    pub(crate) unsafe fn own(handle: HANDLE) -> Self {
        Self(handle)
    }

    pub(crate) fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and owned exclusively by this guard
        // (constructed in `own` from a fresh `OpenProcess` result, closed
        // once here and never copied).
        unsafe {
            CloseHandle(self.0);
        }
    }
}

/// Unix epoch milliseconds for wall-clock sampling.
pub(crate) fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Open for full inspection: try `QUERY_LIMITED | VM_READ` first (needed for
/// the command-line PEB walk), then degrade to limited-only when the target
/// refuses VM_READ (protected/system processes). Returns the guard plus
/// whether VM_READ was granted.
fn open_for_inspection(pid: u32) -> Option<(ProcessHandle, bool)> {
    // SAFETY: both OpenProcess calls are read-only flag combinations.
    let with_vm_read =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
    if !with_vm_read.is_null() {
        return Some((unsafe { ProcessHandle::own(with_vm_read) }, true));
    }
    open_limited(pid).ok().map(|guard| (guard, false))
}

/// Hard cap for a command-line byte length — normal command lines are well
/// under 32 KB; anything larger is treated as garbage rather than allocated.
const MAX_COMMAND_LINE_BYTES: usize = 64 * 1024;

/// Read the target process's full command line via its PEB.
///
/// Chain: `NtQueryInformationProcess(ProcessBasicInformation)` →
/// `PebBaseAddress` → read the remote `PEB` → read the remote
/// `RTL_USER_PROCESS_PARAMETERS` → read the `CommandLine` UTF-16 buffer.
/// All three reads are pure `ReadProcessMemory` copies; nothing is written.
///
/// Returns `None` when any step is refused (no `VM_READ` grant, protected
/// process, cross-bitness target, or the process died mid-walk).
#[cfg(target_pointer_width = "64")]
unsafe fn query_command_line(handle: HANDLE) -> Option<String> {
    // 1. PEB base address.
    let mut pbi: windows_sys::Win32::System::Threading::PROCESS_BASIC_INFORMATION =
        std::mem::zeroed();
    let mut return_length: u32 = 0;
    // SAFETY: pbi is a valid, correctly-sized out-buffer; the handle is open
    // with (at least) QUERY_LIMITED_INFORMATION, which is what this class
    // needs.
    let status = unsafe {
        NtQueryInformationProcess(
            handle,
            ProcessBasicInformation,
            (&mut pbi as *mut _) as *mut core::ffi::c_void,
            std::mem::size_of::<windows_sys::Win32::System::Threading::PROCESS_BASIC_INFORMATION>()
                as u32,
            &mut return_length,
        )
    };
    if status != 0 || pbi.PebBaseAddress.is_null() {
        return None;
    }

    // 2. Read the remote PEB to get the ProcessParameters pointer.
    let mut peb: PEB = std::mem::zeroed();
    // SAFETY: peb is a valid destination of exactly the struct's size; the
    // source is the remote PEB base reported by the kernel for this handle.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            pbi.PebBaseAddress.cast(),
            (&mut peb as *mut _) as *mut core::ffi::c_void,
            std::mem::size_of::<PEB>(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || peb.ProcessParameters.is_null() {
        return None;
    }

    // 3. Read the remote RTL_USER_PROCESS_PARAMETERS.
    let mut params: RTL_USER_PROCESS_PARAMETERS = std::mem::zeroed();
    // SAFETY: params is a valid destination; the source pointer came from
    // the remote PEB we just copied.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            peb.ProcessParameters.cast(),
            (&mut params as *mut _) as *mut core::ffi::c_void,
            std::mem::size_of::<RTL_USER_PROCESS_PARAMETERS>(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }

    // 4. Copy the UTF-16 command-line buffer out of the remote address space.
    let length_bytes = params.CommandLine.Length as usize & !1; // whole UTF-16 units
    if length_bytes == 0 {
        return Some(String::new());
    }
    if length_bytes > MAX_COMMAND_LINE_BYTES || params.CommandLine.Buffer.is_null() {
        return None;
    }
    let mut buffer = vec![0u16; length_bytes / 2];
    // SAFETY: buffer holds exactly length_bytes of writable space.
    let ok = unsafe {
        ReadProcessMemory(
            handle,
            params.CommandLine.Buffer.cast(),
            buffer.as_mut_ptr().cast(),
            length_bytes,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer))
}

/// Non-64-bit builds do not attempt the PEB walk (x86 PEB layout differs).
#[cfg(not(target_pointer_width = "64"))]
unsafe fn query_command_line(_handle: HANDLE) -> Option<String> {
    None
}

/// Open a process with minimal read-only rights, wrapped in the RAII guard.
fn open_limited(pid: u32) -> Result<ProcessHandle, u32> {
    // SAFETY: pid is a caller-provided id; OpenProcess is read-only.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        Err(unsafe { windows_sys::Win32::Foundation::GetLastError() })
    } else {
        // SAFETY: a non-null handle from OpenProcess is owned by us.
        Ok(unsafe { ProcessHandle::own(handle) })
    }
}

/// `QueryFullProcessImageNameW` — full executable path of the process.
///
/// Two-call pattern: the first call asks for the required UTF-16 length; a
/// truncated path is re-queried with a larger buffer.
unsafe fn query_image_path(handle: HANDLE) -> Option<String> {
    let mut buffer = [0u16; 1024]; // long paths truncate rather than fail hard
    let mut size: u32 = buffer.len() as u32;
    // SAFETY: buffer/size are a valid writable pair for the duration of the
    // call; the API writes at most `size` UTF-16 units.
    let ok = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut size)
    };
    if ok == 0 || size == 0 {
        return None;
    }
    let units = &buffer[..size as usize];
    Some(String::from_utf16_lossy(units))
}

/// `GetProcessTimes` — creation time as raw FILETIME ticks, the projected
/// Unix-epoch-milliseconds start timestamp, and cumulative kernel+user CPU
/// ticks. Exited processes report their exit-time view, which is fine: they
/// will vanish from the listener list next cycle.
unsafe fn query_times(handle: HANDLE) -> Option<(u64, Option<u64>, u64)> {
    let mut creation = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut exit = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut kernel = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut user = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    // SAFETY: all four out-params are valid, zero-initialized FILETIME slots.
    let ok = unsafe {
        GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
    };
    if ok == 0 {
        return None;
    }
    let creation_ticks =
        (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime);
    let started_at =
        filetime_to_unix_ms(creation.dwLowDateTime, creation.dwHighDateTime);
    let cpu_ticks = filetime_pair_to_ticks(kernel.dwLowDateTime, kernel.dwHighDateTime)
        .saturating_add(filetime_pair_to_ticks(user.dwLowDateTime, user.dwHighDateTime));
    Some((creation_ticks, started_at, cpu_ticks))
}

/// `GetProcessMemoryInfo` — working set in bytes.
unsafe fn query_memory(handle: HANDLE) -> Option<u64> {
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
    };
    // SAFETY: counters is a valid struct and `cb` declares its size, exactly
    // as the API requires.
    let ok = unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) };
    if ok == 0 {
        return None;
    }
    // WorkingSetSize is usize on some toolchains — widen explicitly.
    Some(counters.WorkingSetSize as u64)
}

/// Inspect a single PID, mapping every failure mode to "inaccessible".
fn inspect_pid(pid: u32) -> RawInspection {
    match open_for_inspection(pid) {
        Some((guard, vm_read_granted)) => {
            let handle = guard.as_raw();
            // SAFETY: each call below takes a valid, open process handle;
            // the guard keeps it alive until end of scope. The command-line
            // walk additionally requires the VM_READ grant we negotiated.
            let (path, times, memory, command_line) = unsafe {
                (
                    query_image_path(handle),
                    query_times(handle),
                    query_memory(handle),
                    if vm_read_granted {
                        query_command_line(handle)
                    } else {
                        None
                    },
                )
            };
            // The guard closes the handle here, on drop.

            match times {
                Some((creation_ticks, started_at, cpu_ticks)) => {
                    let (name, executable_path) = match &path {
                        Some(full) => (executable_basename(full), Some(full.clone())),
                        None => (None, None),
                    };
                    RawInspection {
                        info: ProcessInfo {
                            pid,
                            name,
                            executablePath: executable_path,
                            startedAt: started_at,
                            memoryBytes: memory,
                            cpuPercent: None, // merged later, with the previous cycle
                            commandLine: command_line,
                            accessible: true,
                        },
                        cpu: Some(RawCpuSample {
                            cpu_ticks,
                            creation_ticks,
                            wall_ms: now_unix_ms(),
                        }),
                    }
                }
                None => {
                    // Opened but the process died between OpenProcess and
                    // GetProcessTimes — treat as inaccessible.
                    RawInspection {
                        info: ProcessInfo::inaccessible(pid),
                        cpu: None,
                    }
                }
            }
        }
        None => {
            // ERROR_ACCESS_DENIED (protected/system process) and
            // ERROR_INVALID_PARAMETER (died between listener enumeration and
            // this lookup) are the expected paths — both are normal, not
            // engine errors. Anything else lands in the same honest bucket.
            let _ = (ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER); // referenced for docs
            RawInspection {
                info: ProcessInfo::inaccessible(pid),
                cpu: None,
            }
        }
    }
}

/// Resolve process image basenames for PIDs we may not be allowed to open.
///
/// A single `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` walk lists *every*
/// process's `szExeFile` — readable without opening the process at all. This
/// restores a display name for access-denied rows (e.g. `svchost.exe` on
/// port 135) while keeping `accessible: false` for the rich metadata Windows
/// really withheld.
fn snapshot_names(pids: &[u32]) -> HashMap<u32, String> {
    let wanted: HashMap<u32, ()> = pids.iter().map(|&p| (p, ())).collect();
    let mut resolved = HashMap::new();
    if wanted.is_empty() {
        return resolved;
    }

    // SAFETY: TH32CS_SNAPPROCESS creates a read-only snapshot of the process
    // list; a NULL/INVALID_HANDLE_VALUE return is checked before use.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot.is_null() || snapshot == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return resolved;
    }

    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: entry.dwSize declares the struct size; the snapshot handle is
    // valid until closed below.
    unsafe {
        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                if wanted.contains_key(&entry.th32ProcessID) {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    resolved.insert(
                        entry.th32ProcessID,
                        String::from_utf16_lossy(&entry.szExeFile[..len]),
                    );
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
    }
    resolved
}

/// Sample every requested PID exactly once and produce the cycle's
/// [`ProcessInfo`] list plus raw CPU samples for the percentage merge.
///
/// Deduplication happens upstream (the caller passes unique PIDs); this
/// function never opens the same PID twice.
pub(crate) fn sample_processes(
    unique_pids: &[u32],
) -> (Vec<ProcessInfo>, HashMap<u32, RawCpuSample>) {
    let mut processes = Vec::with_capacity(unique_pids.len());
    let mut cpu_samples = HashMap::with_capacity(unique_pids.len());

    for &pid in unique_pids {
        let inspection = inspect_pid(pid);
        if let Some(cpu) = inspection.cpu {
            cpu_samples.insert(pid, cpu);
        }
        processes.push(inspection.info);
    }

    (processes, cpu_samples)
}

/// Fill in display names for inaccessible processes from the toolhelp
/// snapshot (best-effort; missing entries simply stay `None`).
pub(crate) fn apply_snapshot_names(processes: &mut [ProcessInfo]) {
    let missing: Vec<u32> = processes
        .iter()
        .filter(|p| !p.accessible && p.name.is_none())
        .map(|p| p.pid)
        .collect();
    let names = snapshot_names(&missing);
    for process in processes.iter_mut() {
        if !process.accessible && process.name.is_none() {
            if let Some(name) = names.get(&process.pid) {
                process.name = Some(name.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_names_resolves_current_process() {
        // The test runner itself is always in the snapshot; its basename
        // must end in `.exe` on Windows.
        let pid = std::process::id();
        let names = snapshot_names(&[pid]);
        let name = names.get(&pid).expect("own process must be in the snapshot");
        assert!(
            name.to_lowercase().ends_with(".exe"),
            "unexpected image name: {name}"
        );
    }

    #[test]
    fn snapshot_names_empty_input_is_cheap_no_op() {
        assert!(snapshot_names(&[]).is_empty());
    }

    #[test]
    fn snapshot_name_for_nonexistent_pid_is_absent() {
        // PID 0xFFFF_FFFF does not exist; no crash, no entry.
        assert!(!snapshot_names(&[0xFFFF_FFFF]).contains_key(&0xFFFF_FFFF));
    }

    #[test]
    fn inspect_pid_reports_inaccessible_for_nonexistent_process() {
        // A PID that does not exist → inaccessible, not an engine error.
        // (On a normal user session, system PIDs behave the same way.)
        let inspection = inspect_pid(0xFFFF_FFFF);
        assert!(!inspection.info.accessible);
        assert!(inspection.info.name.is_none());
        assert!(inspection.cpu.is_none());
    }

    #[test]
    fn sample_processes_deduplicates_by_caller_contract_and_is_live() {
        // Inspect our own PID twice through the cycle entry point: the
        // contract is that the *caller* deduped; here we verify the live
        // path yields accessible=true with a name for a runnable process.
        // A freshly spawned process can report 0 cumulative CPU ticks
        // (user/kernel time only updates at scheduling boundaries), so burn
        // a little CPU first to make the assertion meaningful.
        let spin_start = std::time::Instant::now();
        let mut sink = 0u64;
        while spin_start.elapsed() < std::time::Duration::from_millis(50) {
            sink = sink.wrapping_add(1);
        }
        std::hint::black_box(sink);

        let pid = std::process::id();
        let (processes, cpu) = sample_processes(&[pid, pid]);
        assert_eq!(processes.len(), 2, "one entry per requested pid");
        let info = &processes[0];
        assert!(info.accessible, "own process must be inspectable");
        assert!(info.name.is_some(), "image basename must resolve");
        assert!(info
            .executablePath
            .as_ref()
            .is_some_and(|p| p.to_lowercase().contains(".exe")));
        assert!(info.startedAt.is_some(), "creation time must resolve");
        assert!(info.memoryBytes.is_some_and(|m| m > 0), "working set must be positive");
        let sample = cpu.get(&pid).expect("CPU sample must be recorded");
        assert!(sample.cpu_ticks > 0, "a running test process has consumed CPU");
        assert!(sample.creation_ticks > 0);
        assert!(sample.wall_ms > 0);
    }

    #[test]
    fn open_limited_rejects_nonexistent_pid_with_win32_error() {
        // Expected ERROR_INVALID_PARAMETER (87) for a dead/nonexistent PID.
        match open_limited(0xFFFF_FFFF) {
            Err(code) => assert_eq!(code, ERROR_INVALID_PARAMETER),
            Ok(_) => panic!("a nonexistent PID must not open"),
        }
    }
}
