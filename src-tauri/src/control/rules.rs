//! Pure control-eligibility rules and URL mapping.
//!
//! Deterministic and fully unit-tested; no Windows calls. The conservative
//! default: **refuse** unless the process is clearly a user-owned
//! development process.

use crate::discovery::PortListener;
use crate::intelligence::ServiceIdentity;
use crate::intelligence::rules::ServiceCategory;
use crate::process::ProcessInfo;
use crate::project::ProjectIdentity;

/// Windows system processes that must never be controllable, regardless of
/// any other evidence. Matched case-insensitively on the image basename.
const SYSTEM_PROCESS_NAMES: &[&str] = &[
    "system",
    "system idle process",
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "svchost.exe",
    "dwm.exe",
    "fontdrvhost.exe",
    "sihost.exe",
    "taskhostw.exe",
    "explorer.exe",
    "conhost.exe",
    "wudfhost.exe",
    "memory compression",
    "secure system",
];

/// basename (final path segment, both separators) of a Windows path,
/// ASCII-lowercased for matching.
fn basename_lower(path: &str) -> String {
    let trimmed = path.trim();
    let start = trimmed
        .rfind(['\\', '/'])
        .map(|index| index + 1)
        .unwrap_or(0);
    trimmed[start..].to_ascii_lowercase()
}

/// Windows-directory process → refuse. Compares the executable path against
/// the Windows directory (`C:\Windows` typically, read from `SystemRoot`,
/// defaulting to the conventional path) case-insensitively.
fn is_under_windows_directory(executable_path: Option<&str>) -> bool {
    let Some(path) = executable_path else {
        return false;
    };
    let windows_dir = std::env::var("SystemRoot")
        .unwrap_or_else(|_| r"C:\Windows".to_string())
        .trim_end_matches(['\\', '/'])
        .to_ascii_lowercase();
    let lowered = path.to_ascii_lowercase().replace('/', "\\");
    lowered.starts_with(&windows_dir)
}

/// Denylist-only policy check — the exact function the action-time
/// authorization chain recomputes against **fresh** process data before any
/// write primitive runs. Returns the honest refusal reason, or `None` when
/// the process passes every hard refusal rule:
///
/// 0. reserved PIDs (0–4) — always denied;
/// 1. inaccessible process → identity cannot be verified;
/// 2. no creation time → identity cannot be verified;
/// 3. Windows system process (name);
/// 4. executable under the Windows directory;
/// 5. database engines — conservative refuse;
/// 6. Docker/infrastructure processes (backend-derived category hint —
///    the hint can only *tighten* a refusal, never loosen one).
///
/// Deliberately NOT part of this function: the development-evidence
/// requirement (step 7 of [`evaluate_capability`]). At action time the
/// full classification pipeline is not re-run; the backend-stored
/// issuance hint (`TrustedControlTarget::has_dev_evidence`) carries that
/// decision — it was derived by this process, never by the frontend.
pub(crate) fn denylist_refusal(
    process: &ProcessInfo,
    service_category: Option<ServiceCategory>,
) -> Option<String> {
    // 0. Reserved Windows PIDs. PID 0 is the System Idle Process and PID 4
    //    is the System process; nothing user-owned ever lives there. (Unlike
    //    Unix, Windows does not reserve PIDs 1–3.)
    if process.pid == 0 || process.pid == 4 {
        return Some("Windows reserved system process (PID 0 or 4).".to_string());
    }

    // 1–2. Identity must be verifiable before anything else matters.
    if !process.accessible {
        return Some(
            "Process identity cannot be verified (Windows refused inspection).".to_string(),
        );
    }
    if process.startedAt.is_none() {
        return Some(
            "Process identity cannot be verified (no creation time available).".to_string(),
        );
    }

    let name = process
        .name
        .as_deref()
        .map(basename_lower)
        .unwrap_or_default();

    // 3. Hard system list.
    if SYSTEM_PROCESS_NAMES.contains(&name.as_str()) {
        return Some("Windows system process.".to_string());
    }

    // 4. System location.
    if is_under_windows_directory(process.executablePath.as_deref()) {
        return Some("Process runs from the Windows directory.".to_string());
    }

    // 5. Database engines — a user-owned DB server may be managed by a
    //    service controller; LocalStack stays out of its lifecycle.
    const DATABASE_PROCESSES: &[&str] = &[
        "postgres.exe",
        "mysqld.exe",
        "mariadbd.exe",
        "redis-server.exe",
        "mongod.exe",
        "sqlservr.exe",
    ];
    if DATABASE_PROCESSES.contains(&name.as_str()) {
        return Some(
            "Database engine — LocalStack does not stop database services.".to_string(),
        );
    }

    // 6. Docker/infrastructure — container lifecycle belongs to Docker.
    if service_category == Some(ServiceCategory::Infrastructure) {
        return Some("Infrastructure process — manage it through its own tooling.".to_string());
    }

    None
}

/// Evaluate what the user may do with this process, with an honest reason
/// when the answer is "nothing".
///
/// Priority (strongest refusal first): the [`denylist_refusal`] rules
/// (reserved PIDs, verifiability, system names, Windows directory,
/// databases, infrastructure), then the development-evidence requirement:
/// controllable only with real evidence — a classified service identity
/// (runtime/framework) **or** a confirmed project association.
pub(crate) fn evaluate_capability(
    process: &ProcessInfo,
    service: Option<&ServiceIdentity>,
    project: Option<&ProjectIdentity>,
) -> super::ControlCapability {
    if let Some(reason) = denylist_refusal(process, service.map(|s| s.category.clone())) {
        return refusal(&reason);
    }

    // Development evidence required.
    let classified = matches!(service, Some(s) if s.category != ServiceCategory::Unknown);
    if classified || project.is_some() {
        return super::ControlCapability {
            canOpen: false, // filled in by the facade from listener rows
            canStop: true,
            gracefulStopSupported: false,
            gracefulStopReason: GRACEFUL_UNSUPPORTED_REASON.to_string(),
            canRestart: false, // Phase 5 defers restart — see docs/roadmap.md
            reason: String::new(),
        };
    }

    refusal("No development-process evidence — only classified services or project-associated processes are controllable.")
}

/// Why targeted graceful stop is unavailable for externally discovered
/// processes (the only kind Phase 5 has). The console-wide CTRL_BREAK
/// broadcast is not an acceptable substitute.
pub(crate) const GRACEFUL_UNSUPPORTED_REASON: &str =
    "Process was not launched in a LocalStack-managed process group.";

fn refusal(reason: &str) -> super::ControlCapability {
    super::ControlCapability {
        canOpen: false,
        canStop: false,
        gracefulStopSupported: false,
        gracefulStopReason: GRACEFUL_UNSUPPORTED_REASON.to_string(),
        canRestart: false,
        reason: reason.to_string(),
    }
}

/// Map a listener row to a browser-friendly URL, when its bind address is a
/// localhost form.
///
/// - `127.0.0.1` → `http://127.0.0.1:<port>`
/// - `::1`       → `http://[::1]:<port>`
/// - `0.0.0.0` / `::` (wildcards) → `http://localhost:<port>` — the server
///   listens everywhere, so the friendliest local name is correct.
/// - Any other address (a specific LAN/Tailscale IP) → `None`; the UI
///   decides separately whether to offer anything.
pub(crate) fn browsable_url(listener: &PortListener) -> Option<String> {
    let host = match listener.localAddress.as_str() {
        "0.0.0.0" | "::" => "localhost".to_string(),
        "::1" => "[::1]".to_string(),
        "127.0.0.1" => "127.0.0.1".to_string(),
        _ => return None,
    };
    Some(format!("http://{host}:{}", listener.port))
}
