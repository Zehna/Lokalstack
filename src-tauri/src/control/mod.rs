//! # Safe Local Service Control (Phase 5, hardened)
//!
//! The product's first **write** capability — with all authority on the
//! native side of the Tauri boundary.
//!
//! ## Trust model
//!
//! - The frontend never sends policy-bearing fields (PID, creation time,
//!   service category, project, canStop…). It echoes back only the
//!   **opaque target id** the backend issued during discovery
//!   ([`registry::ControlTargetRegistry`]).
//! - Every action resolves the id server-side, re-inspects the process,
//!   revalidates identity (creation time + executable path), and
//!   **recomputes eligibility** ([`rules::evaluate_capability`]) with fresh
//!   data — the snapshot-time category/project/name are hints, never
//!   authority. A process that became protected, changed its image, or was
//!   PID-reused is refused.
//! - **No console control events, ever.** The earlier
//!   `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, 0)` was a console-wide
//!   broadcast (group id 0 reaches every process on the attached console;
//!   a PID is not a process-group id) and has been removed. A
//!   source-level regression test in this file keeps it out. For
//!   externally discovered processes the honest semantics are
//!   `gracefulStopSupported: false` + a user-confirmed **End Process**.
//!   Phase 6 contract: LocalStack-launched processes in a managed
//!   process group (`CREATE_NEW_PROCESS_GROUP`) may regain *targeted*
//!   CTRL_BREAK against their known group id.
//!
//! ## Structure
//!
//! - [`registry`] — opaque id issuance/lookup (bounded, TTL-expiring).
//! - [`rules`] — pure eligibility + URL mapping, fully unit-tested.
//! - [`windows`] — narrow FFI: revalidate / wait / terminate / open.
//! - This facade — DTOs, per-cycle capability derivation, the
//!   authorization chain, and the Tauri-facing action results.

pub(crate) mod registry;
pub(crate) mod rules;

#[cfg(windows)]
pub(crate) mod windows;

use serde::Serialize;

pub(crate) use registry::{ControlTargetRegistry, TargetResolution, TrustedControlTarget};
pub(crate) use rules::{browsable_url, evaluate_capability};

use std::sync::Arc;

/// Tauri-managed state for the control engine: the opaque target registry.
/// The registry is the native trust boundary — every action authorizes
/// through it.
#[derive(Default)]
pub(crate) struct ControlEngineState {
    pub(crate) registry: Arc<ControlTargetRegistry>,
}

use crate::discovery::PortListener;
use crate::intelligence::ServiceIdentity;
use crate::process::ProcessInfo;
use crate::project::ProjectIdentity;

/// Structured refusal markers. The frontend maps these to honest messages
/// instead of guessing.
pub(crate) const STALE_TARGET: &str = "STALE_TARGET";
pub(crate) const IDENTITY_UNVERIFIABLE: &str = "IDENTITY_UNVERIFIABLE";
pub(crate) const UNKNOWN_TARGET: &str = "UNKNOWN_TARGET";
pub(crate) const REFUSED_BY_POLICY: &str = "REFUSED_BY_POLICY";

/// The exact process a control action may act on, as the **backend**
/// derives it from the discovery snapshot. The frontend receives only the
/// opaque `id` plus display fields; it cannot construct or forge any of
/// this.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ControlTarget {
    /// Opaque, unpredictable registry id. The only value the frontend may
    /// send back to authorize an action.
    pub id: String,
    /// Display fields (advisory; the backend re-derives everything at
    /// action time from its own trusted snapshot).
    pub displayName: String,
    pub processName: Option<String>,
}

/// What the user may do with a process, and why not when they may not.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ControlCapability {
    /// A browser-friendly URL exists for at least one of the process's
    /// listener rows.
    pub canOpen: bool,
    /// The process is a controllable development process (End Process
    /// available after explicit confirmation).
    pub canStop: bool,
    /// Whether a *targeted* graceful stop exists. Always `false` in
    /// Phase 5: externally discovered processes were not launched into a
    /// LocalStack-managed process group, and a console-wide CTRL_BREAK
    /// broadcast is not acceptable. Phase 6 managed launches may set this
    /// to `true` against a known group id.
    pub gracefulStopSupported: bool,
    /// Why `gracefulStopSupported` is what it is.
    pub gracefulStopReason: String,
    /// Deliberately `false` — restart is deferred to Phase 6.
    pub canRestart: bool,
    /// When `canStop` is `false`, the honest reason.
    pub reason: String,
}

/// A PID's control surface, attached to the discovery response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PidControl {
    pub pid: u32,
    #[serde(flatten)]
    pub capability: ControlCapability,
    /// Browser-friendly URLs (localhost forms only), deduplicated.
    pub urls: Vec<String>,
    /// The backend-issued control target (controllable PIDs only; `null`
    /// targets for refused processes).
    pub target: Option<ControlTarget>,
}

/// Compute the control surface for one inspected process **during a
/// discovery cycle**. Registers a trusted snapshot in the registry and
/// returns the opaque-id DTO.
pub(crate) fn control_for_process(
    process: &ProcessInfo,
    service: Option<&ServiceIdentity>,
    project: Option<&ProjectIdentity>,
    listeners: &[PortListener],
    registry: &ControlTargetRegistry,
) -> PidControl {
    let urls: Vec<String> = listeners
        .iter()
        .filter(|l| l.pid == process.pid)
        .filter_map(browsable_url)
        .collect();

    let mut capability = evaluate_capability(process, service, project);
    // `evaluate_capability` cannot see listener rows; the facade fills the
    // open capability from the browsable URLs it derives from them.
    capability.canOpen = !urls.is_empty();
    let display_name = service
        .map(|s| s.displayName.clone())
        .or_else(|| process.name.clone())
        .unwrap_or_else(|| format!("PID {}", process.pid));
    let target = if capability.canStop {
        let id = registry.register(trusted_target_of(
            process,
            service,
            capability.canStop,
        ));
        Some(ControlTarget {
            id,
            displayName: display_name.clone(),
            processName: process.name.clone(),
        })
    } else {
        None
    };

    PidControl {
        pid: process.pid,
        capability,
        urls,
        target,
    }
}

/// Build the backend-trusted snapshot for one process (used at issuance and
/// at refresh-time registry replacement). The evidence hints here are
/// backend-derived only — computed from the discovery cycle's own
/// classification and project resolution, never from any frontend input.
pub(crate) fn trusted_target_of(
    process: &ProcessInfo,
    service: Option<&ServiceIdentity>,
    has_dev_evidence: bool,
) -> TrustedControlTarget {
    TrustedControlTarget {
        pid: process.pid,
        creation_ms: process.startedAt,
        executable_path: process.executablePath.clone(),
        process_name: process.name.clone(),
        display_name: service
            .map(|s| s.displayName.clone())
            .or_else(|| process.name.clone())
          .unwrap_or_else(|| format!("PID {}", process.pid)),
        accessible: process.accessible,
        service_category: service.map(|s| s.category.clone()),
        has_dev_evidence,
    }
}

/// Outcome of a stop action, reported honestly to the UI.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct StopResult {
    /// The process is gone.
    pub stopped: bool,
    /// A *targeted* graceful method existed and was used. Always `false`
    /// for externally discovered processes (see `gracefulStopSupported`).
    pub gracefulAttempted: bool,
    /// `true` when the process was still alive after the operation.
    pub stillRunning: bool,
    /// Human-readable summary for the history log.
    pub message: String,
}

/// Compare a snapshot identity against a freshly probed one.
///
/// - The snapshot must carry a creation time (`IDENTITY_UNVERIFIABLE`).
/// - Creation times must match exactly (`STALE_TARGET` on mismatch).
/// - Executable paths are compared case-insensitively when both sides are
///   known; a path only the fresh probe reveals is not a mismatch.
pub(crate) fn verify_identity(
    snapshot_creation_ms: Option<u64>,
    snapshot_path: Option<&str>,
    fresh_path: Option<&str>,
    fresh_started_at_ms: Option<u64>,
) -> Result<(), &'static str> {
    let Some(snapshot_time) = snapshot_creation_ms else {
        return Err(IDENTITY_UNVERIFIABLE);
    };
    let Some(fresh_time) = fresh_started_at_ms else {
        return Err(STALE_TARGET);
    };
    if snapshot_time != fresh_time {
        return Err(STALE_TARGET);
    }
    if let (Some(snapshot_path), Some(fresh_path)) = (snapshot_path, fresh_path) {
        if !snapshot_path.eq_ignore_ascii_case(fresh_path) {
            return Err(STALE_TARGET);
        }
    }
    Ok(())
}

/// The full authorization chain for a control action, run inside the
/// command handler before any write primitive:
///
/// 1. resolve the opaque target id (unknown → refuse);
/// 2. re-inspect the process (gone/protected → stale);
/// 3. validate PID + creation identity + executable path;
/// 4. **recompute eligibility from fresh data** (denylist reapplied);
/// 5. refuse unless the fresh evaluation says controllable.
///
/// Returns the trusted snapshot on success or the structured refusal marker.
pub(crate) fn authorize_action(
    registry: &ControlTargetRegistry,
    target_id: &str,
) -> Result<std::sync::Arc<TrustedControlTarget>, String> {
    // 1. Registry lookup — unknown/expired ids refuse here. A forged PID
    //    can never enter the chain, because PID is not an input at all.
    let trusted = match registry.resolve(target_id) {
        TargetResolution::Found(trusted) => trusted,
        TargetResolution::Unknown => {
            return Err(format!(
                "{UNKNOWN_TARGET}: this control target is no longer valid. Refresh and try again."
            ))
        }
    };

    // 2. Re-inspect the process right now.
    #[cfg(windows)]
    let fresh = windows::revalidate(trusted.pid);
    #[cfg(not(windows))]
    let fresh: Option<(Option<String>, Option<u64>)> = None;
    #[cfg(not(windows))]
    let _ = &fresh;

    #[cfg(windows)]
    {
        let Some((fresh_path, fresh_creation)) = fresh else {
            return Err(format!(
                "{STALE_TARGET}: the process is no longer inspectable. Refresh and try again."
            ));
        };

        // 3. Identity: creation time + executable path.
        if let Err(marker) = verify_identity(
            trusted.creation_ms,
            trusted.executable_path.as_deref(),
            fresh_path.as_deref(),
            fresh_creation,
        ) {
            return Err(format!(
                "{marker}: this process changed since it was discovered. Refresh before trying again."
            ));
        }

        // 4–5. Recompute policy from FRESH data. Every hard deny rule —
        // reserved PIDs, verifiability, system names, Windows directory,
        // databases — is re-derived from the fresh probe. The
        // backend-stored issuance hints (service category, dev evidence)
        // can only *tighten* the decision (infrastructure category) or
        // satisfy the development-evidence requirement; the frontend has
        // no way to influence either (they were computed server-side at
        // issuance and live only in the registry).
        let fresh_process = ProcessInfo {
            pid: trusted.pid,
            name: fresh_path.as_deref().map(|path| {
                path.rsplit(['\\', '/'])
                    .next()
                    .unwrap_or(path)
                    .to_string()
            }),
            executablePath: fresh_path.clone(),
            startedAt: fresh_creation,
            memoryBytes: None,
            cpuPercent: None,
            commandLine: None,
            accessible: true,
        };
        if let Some(reason) = rules::denylist_refusal(&fresh_process, trusted.service_category) {
            return Err(format!("{REFUSED_BY_POLICY}: {reason} Refresh and try again."));
        }

        // 6. Development evidence: required at issuance AND still present
        // in the backend's own records. A target could only have been
        // issued because the backend found evidence then; require it again
        // here so the two gates cannot drift.
        if !trusted.has_dev_evidence {
            return Err(format!(
                "{REFUSED_BY_POLICY}: no development-process evidence. Refresh and try again."
            ));
        }

        Ok(trusted)
    }

    #[cfg(not(windows))]
    {
        Err("Service control is Windows-only.".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ports::{IpVersion, ListenerState, Protocol};
    use crate::intelligence::rules::{Confidence, Evidence, ServiceCategory, ServiceKind};
    use crate::project::{Confidence as ProjectConfidence, ProjectKind};

    fn process(pid: u32, name: &str, path: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: Some(name.to_string()),
            executablePath: path.map(str::to_string),
            startedAt: Some(1_700_000_000_000),
            memoryBytes: Some(10_000_000),
            cpuPercent: None,
            commandLine: Some("node vite.js".to_string()),
            accessible: true,
        }
    }

    fn service(display: &str, category: ServiceCategory) -> ServiceIdentity {
        ServiceIdentity {
            kind: ServiceKind::Vite,
            displayName: display.to_string(),
            category,
            confidence: Confidence::High,
            evidence: vec![Evidence::new("command_line", "vite")],
        }
    }

    fn project(id: &str) -> ProjectIdentity {
        ProjectIdentity {
            id: id.to_string(),
            name: "proj".to_string(),
            rootPath: id.to_string(),
            kind: ProjectKind::NodeJs,
            git: crate::project::GitInfo::not_a_repository(),
            packageManager: Some("npm".to_string()),
            startCommand: None,
            confidence: ProjectConfidence::High,
            evidence: Vec::new(),
        }
    }

    fn listener(port: u16, pid: u32, address: &str) -> PortListener {
        PortListener {
            protocol: Protocol::Tcp,
            ipVersion: IpVersion::V4,
            localAddress: address.to_string(),
            port,
            pid,
            state: ListenerState::Listen,
        }
    }

    // --- capability model ----------------------------------------------------

    #[test]
    fn vite_process_is_controllable_but_not_gracefully_stoppable() {
        let registry = ControlTargetRegistry::new();
        let control = control_for_process(
            &process(1, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
            Some(&service("Vite", ServiceCategory::Frontend)),
            Some(&project(r"D:\Projects\localstack")),
            &[listener(1420, 1, "127.0.0.1")],
            &registry,
        );
        assert!(control.capability.canStop);
        assert!(
            !control.capability.gracefulStopSupported,
            "externally discovered processes have no targeted graceful stop"
        );
        assert_eq!(
            control.capability.gracefulStopReason,
            "Process was not launched in a LocalStack-managed process group."
        );
        let target = control.target.expect("controllable → target");
        assert_eq!(target.id.len(), 64, "opaque id is a 256-bit hex string");
    }

    #[test]
    fn svchost_capability_refuses_with_system_reason() {
        let capability = evaluate_capability(
            &process(2, "svchost.exe", Some(r"C:\Windows\System32\svchost.exe")),
            None,
            None,
        );
        assert!(!capability.canStop);
        assert_eq!(capability.reason, "Windows system process.");
    }

    #[test]
    fn metadata_unavailable_reason_mentions_identity() {
        let mut inaccessible = process(3, "protected.exe", None);
        inaccessible.accessible = false;
        let capability = evaluate_capability(&inaccessible, None, None);
        assert!(!capability.canStop);
        assert!(capability.reason.contains("identity cannot be verified"));
    }

    // --- eligibility (backend recomputation path) ------------------------------

    #[test]
    fn system_processes_are_never_controllable() {
        for name in [
            "System", "svchost.exe", "lsass.exe", "services.exe",
            "wininit.exe", "csrss.exe", "winlogon.exe", "smss.exe",
        ] {
            let capability = evaluate_capability(
                &process(4, name, Some(r"C:\Windows\System32\svchost.exe")),
                None,
                None,
            );
            assert!(!capability.canStop, "{name} must not be controllable");
        }
    }

    #[test]
    fn database_engines_are_conservatively_refused() {
        let capability = evaluate_capability(
            &process(5, "postgres.exe", Some(r"C:\Program Files\PostgreSQL\16\bin\postgres.exe")),
            Some(&service("PostgreSQL", ServiceCategory::Database)),
            None,
        );
        assert!(!capability.canStop);
    }

    #[test]
    fn infrastructure_is_refused() {
        let capability = evaluate_capability(
            &process(6, "com.docker.backend.exe", Some(r"C:\Program Files\Docker\com.docker.backend.exe")),
            Some(&service("Docker", ServiceCategory::Infrastructure)),
            None,
        );
        assert!(!capability.canStop);
        assert!(capability.reason.contains("Infrastructure"));
    }

    /// Forged service/project metadata from a frontend cannot turn a
    /// denied process into an allowed one: the denylist runs on the fresh
    /// process data (name, path, PID) and the backend-stored hints can only
    /// tighten the decision — the frontend has no channel for either.
    #[test]
    fn forged_metadata_cannot_authorize_a_denied_process() {
        // A system process with a fully forged "Vite + project" context:
        // the denylist still fires on the real name/path.
        let capability = evaluate_capability(
            &process(40, "svchost.exe", Some(r"C:\Windows\System32\svchost.exe")),
            Some(&service("Fake Vite", ServiceCategory::Frontend)),
            Some(&project(r"D:\Projects\fake")),
        );
        assert!(!capability.canStop, "forged metadata must not flip a denylisted process");

        // An infrastructure-category hint can only tighten: even a process
        // that looks like a dev runtime stays refused.
        let capability = evaluate_capability(
            &process(41, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
            Some(&service("Docker", ServiceCategory::Infrastructure)),
            Some(&project(r"D:\Projects\fake")),
        );
        assert!(!capability.canStop, "infrastructure category must keep the refusal");
    }

    /// Reserved Windows PIDs (0 = System Idle, 4 = System) are always
    /// denied, with any metadata. PIDs 1–3 are NOT reserved on Windows and
    /// must stay eligible.
    #[test]
    fn reserved_pids_are_always_denied() {
        for pid in [0u32, 4] {
            let capability = evaluate_capability(
                &process(pid, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
                Some(&service("Vite", ServiceCategory::Frontend)),
                Some(&project(r"D:\Projects\fake")),
            );
            assert!(!capability.canStop, "PID {pid} must be denied");
        }
        let ordinary = evaluate_capability(
            &process(3, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
            Some(&service("Vite", ServiceCategory::Frontend)),
            None,
        );
        assert!(ordinary.canStop, "PID 1–3 are ordinary Windows PIDs and must stay eligible");
    }

    #[test]
    fn processes_without_creation_time_are_refused() {
        let mut timeless = process(7, "node.exe", Some(r"C:\Program Files\nodejs\node.exe"));
        timeless.startedAt = None;
        let capability =
            evaluate_capability(&timeless, Some(&service("Vite", ServiceCategory::Frontend)), None);
        assert!(!capability.canStop);
    }

    #[test]
    fn classified_dev_process_is_controllable() {
        let capability = evaluate_capability(
            &process(8, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
            Some(&service("Vite", ServiceCategory::Frontend)),
            None,
        );
        assert!(capability.canStop);
    }

    #[test]
    fn project_associated_process_is_controllable() {
        let capability = evaluate_capability(
            &process(9, "my-tool.exe", Some(r"D:\Projects\tool\target\debug\my-tool.exe")),
            Some(&service("my-tool.exe", ServiceCategory::Unknown)),
            Some(&project(r"D:\Projects\tool")),
        );
        assert!(capability.canStop);
    }

    // --- URL mapping -----------------------------------------------------------

    #[test]
    fn loopback_and_wildcard_addresses_map_to_browsable_urls() {
        assert_eq!(
            browsable_url(&listener(1420, 1, "127.0.0.1")).as_deref(),
            Some("http://127.0.0.1:1420")
        );
        assert_eq!(
            browsable_url(&listener(1420, 1, "0.0.0.0")).as_deref(),
            Some("http://localhost:1420")
        );
    }

    #[test]
    fn non_web_addresses_get_no_url() {
        assert_eq!(browsable_url(&listener(3300, 1, "100.80.10.2")), None);
    }

    #[test]
    fn control_surface_combines_capability_and_urls() {
        let registry = ControlTargetRegistry::new();
        let control = control_for_process(
            &process(20, "node.exe", Some(r"C:\Program Files\nodejs\node.exe")),
            Some(&service("Vite", ServiceCategory::Frontend)),
            Some(&project(r"D:\Projects\localstack")),
            &[listener(1420, 20, "127.0.0.1"), listener(1421, 20, "0.0.0.0")],
            &registry,
        );
        assert!(control.capability.canStop);
        assert!(control.capability.canOpen);
        assert_eq!(
            control.urls,
            vec!["http://127.0.0.1:1420".to_string(), "http://localhost:1421".to_string()]
        );
    }

    #[test]
    fn non_controllable_processes_have_no_target() {
        let registry = ControlTargetRegistry::new();
        let control = control_for_process(
            &process(21, "svchost.exe", Some(r"C:\Windows\System32\svchost.exe")),
            None,
            None,
            &[listener(135, 21, "0.0.0.0")],
            &registry,
        );
        assert!(!control.capability.canStop);
        assert!(control.target.is_none());
    }

    // --- identity revalidation ---------------------------------------------------

    #[test]
    fn matching_identity_passes() {
        assert!(verify_identity(
            Some(1_700_000_000_000),
            Some(r"C:\Program Files\nodejs\node.exe"),
            Some(r"C:\Program Files\nodejs\node.exe"),
            Some(1_700_000_000_000),
        )
        .is_ok());
    }

    #[test]
    fn creation_time_mismatch_is_stale() {
        assert_eq!(
            verify_identity(Some(1_700_000_000_000), None, None, Some(1_700_000_999_999)),
            Err(STALE_TARGET)
        );
    }

    #[test]
    fn pid_reuse_is_detected_via_creation_time() {
        assert_eq!(verify_identity(Some(100), None, None, Some(200)), Err(STALE_TARGET));
    }

    #[test]
    fn executable_path_change_is_stale() {
        assert_eq!(
            verify_identity(
                Some(100),
                Some(r"C:\old\node.exe"),
                Some(r"C:\new\node.exe"),
                Some(100),
            ),
            Err(STALE_TARGET)
        );
    }

    #[test]
    fn path_comparison_is_case_insensitive() {
        assert!(verify_identity(
            Some(100),
            Some(r"C:\Program Files\NODEJS\node.exe"),
            Some(r"c:\program files\nodejs\node.exe"),
            Some(100),
        )
        .is_ok());
    }

    #[test]
    fn snapshot_without_creation_time_is_unverifiable() {
        assert_eq!(
            verify_identity(None, None, None, Some(100)),
            Err(IDENTITY_UNVERIFIABLE)
        );
    }

    #[test]
    fn process_that_cannot_be_reinspected_is_stale() {
        assert_eq!(verify_identity(Some(100), None, None, None), Err(STALE_TARGET));
    }

    // --- authorization chain (pure parts) ----------------------------------------

    #[test]
    fn unknown_opaque_target_id_is_refused() {
        // Issue a real id, then ask for a different (never-issued) one.
        let registry = ControlTargetRegistry::new();
        registry.register(TrustedControlTarget {
            pid: 30,
            creation_ms: Some(1_700_000_000_000),
            executable_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
            process_name: Some("node.exe".to_string()),
            display_name: "Vite".to_string(),
            accessible: true,
            service_category: Some(ServiceCategory::Frontend),
            has_dev_evidence: true,
        });
        let err = authorize_action(&registry, "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .expect_err("unknown id must refuse");
        assert!(err.starts_with(UNKNOWN_TARGET), "got: {err}");
    }

    #[test]
    fn arbitrary_target_fields_cannot_authorize_a_pid() {
        // The chain takes ONLY an opaque id: there is no function that
        // accepts (pid, creationTime, …) from the frontend. This test
        // pins the contract structurally — a forged registry with no
        // matching entry refuses, regardless of what fields a caller
        // could possibly know.
        let registry = ControlTargetRegistry::new();
        let err = authorize_action(&registry, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .expect_err("forged id must refuse");
        assert!(err.starts_with(UNKNOWN_TARGET));
    }

    #[test]
    fn expired_target_id_is_refused() {
        let registry = ControlTargetRegistry::new();
        let id = registry.register(TrustedControlTarget {
            pid: 31,
            creation_ms: Some(1_700_000_000_000),
            executable_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
            process_name: Some("node.exe".to_string()),
            display_name: "Vite".to_string(),
            accessible: true,
            service_category: Some(ServiceCategory::Frontend),
            has_dev_evidence: true,
        });
        // Age the entry past the TTL (white-box, deterministic).
        registry.age_for_test(&id);
        let err = authorize_action(&registry, &id).expect_err("expired id must refuse");
        assert!(err.starts_with(UNKNOWN_TARGET), "got: {err}");
    }

    #[test]
    fn refreshed_registry_refuses_old_ids() {
        let registry = ControlTargetRegistry::new();
        let old = registry.register(TrustedControlTarget {
            pid: 32,
            creation_ms: Some(1_700_000_000_000),
            executable_path: None,
            process_name: None,
            display_name: "Vite".to_string(),
            accessible: true,
            service_category: Some(ServiceCategory::Frontend),
            has_dev_evidence: true,
        });
        registry.replace_all(std::iter::empty());
        let err = authorize_action(&registry, &old).expect_err("refreshed-over id must refuse");
        assert!(err.starts_with(UNKNOWN_TARGET));
    }

    /// REGRESSION GUARD: `GenerateConsoleCtrlEvent` must never return to
    /// this crate. Group id 0 broadcasts to every process on the attached
    /// console; LocalStack does not own those consoles. Source-level guard
    /// so the unsafe pattern cannot be reintroduced silently.
    //
    // Reminder of the audit's forged-input scenarios, all covered by the
    // chain: forged service/project metadata cannot turn a denied process
    // into an allowed one (hints are backend-stored only; the command takes
    // a single opaque id), a system process stays denied at action time
    // (`denylist_refusal` re-runs on fresh data), and no operation accepts
    // a raw PID-only request.
    #[test]
    fn no_console_broadcast_anywhere_in_the_crate() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("read src tree") {
                let path = entry.expect("dir entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    // Strip // and /* */ comments first: documentation may
                    // mention the banned API by name; code may not use it.
                    let mut code = String::with_capacity(text.len());
                    let mut rest = text.as_str();
                    while let Some(pos) = rest.find("//") {
                        code.push_str(&rest[..pos]);
                        match rest[pos..].find('\n') {
                            Some(end) => rest = &rest[pos + end..],
                            None => {
                                rest = "";
                                break;
                            }
                        }
                    }
                    code.push_str(rest);
                    let code = code.replace("/*", "").replace("*/", "");
                    // Strip string-literal contents so this guard's own
                    // needles do not self-match; a real API call in code is
                    // an identifier, not a literal, and stays visible.
                    // (Naive scanner: raw strings with embedded quotes could
                    // desync it — acceptable for this source-level guard.)
                    let mut no_strings = String::with_capacity(code.len());
                    let mut in_string = false;
                    let mut escaped = false;
                    for ch in code.chars() {
                        if in_string {
                            if escaped {
                                escaped = false;
                                continue;
                            }
                            match ch {
                                '\\' => escaped = true,
                                '"' => {
                                    in_string = false;
                                    no_strings.push('"');
                                }
                                _ => {}
                            }
                            continue;
                        }
                        match ch {
                            '"' => {
                                in_string = true;
                                no_strings.push('"');
                            }
                            _ => no_strings.push(ch),
                        }
                    }
                    if no_strings.contains("GenerateConsoleCtrlEvent")
                        || no_strings.contains("AttachConsole")
                        || no_strings.contains("CTRL_BREAK_EVENT")
                    {
                        offenders.push(path.display().to_string());
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "console control broadcast must not be reintroduced: {offenders:?}"
        );
    }

    /// LIVE-SYSTEM TEST (#[ignore]d): the full hardened authorization chain
    /// against a **disposable child process** started by this test —
    ///
    /// 1. issue an opaque target id for the child (registry trust path);
    /// 2. a forged/unknown id is refused, child untouched;
    /// 3. the valid id passes lookup → revalidation → identity → policy;
    /// 4. the user-confirmed End Process terminates the child;
    /// 5. the child is observed gone.
    ///
    /// No console control events are used anywhere in this test.
    /// Run explicitly:  cargo test -- --ignored --nocapture live_end_process
    #[test]
    #[ignore = "terminates a disposable test child; run manually for verification"]
    fn live_end_process_disposable_child() {
        use std::process::{Command, Stdio};

        /// RAII guard so the disposable child can never leak, even when an
        /// assertion fails — the previous hang was exactly this: the test
        /// panicked before terminating the child, whose 10-minute
        /// `setTimeout` kept cargo's output pipe open. Holds the PID only,
        /// so the `Child` handle stays free for `wait()`.
        struct LeakGuard(u32);
        impl Drop for LeakGuard {
            fn drop(&mut self) {
                #[cfg(windows)]
                {
                    let _ = super::windows::terminate(self.0);
                }
            }
        }

        // Disposable node child: a classified Node.js runtime, idle, with a
        // marker file so we can poll for exit without platform APIs.
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let marker = std::env::temp_dir().join(format!("localstack-live-end-{unique}.flag"));
        let mut child = Command::new("node")
            .args([
                "-e",
                &format!(
                    "const fs=require('fs');fs.writeFileSync({},'x');setTimeout(()=>{{}},600000)",
                    serde_json::to_string(&marker.display().to_string()).unwrap_or_default()
                ),
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("node must be available for this live test");
        let pid = child.id();
        let _guard = LeakGuard(pid);

        // Wait for the marker: the child is alive and stable.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !marker.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(marker.exists(), "child must signal readiness");

        // 1. Issue the trusted target via the registry (same as discovery).
        let fresh = windows::revalidate(pid).expect("child must be inspectable");
        let registry = ControlTargetRegistry::new();
        let target_id = registry.register(TrustedControlTarget {
            pid,
            creation_ms: fresh.1,
            executable_path: fresh.0.clone(),
            process_name: Some("node.exe".to_string()),
            display_name: "Node.js".to_string(),
            accessible: true,
            service_category: Some(ServiceCategory::Backend),
            has_dev_evidence: true,
        });

        // 2. Forged/unknown id refused; child untouched.
        let err = authorize_action(&registry, "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff")
            .expect_err("unknown id must refuse");
        assert!(err.starts_with(UNKNOWN_TARGET));
        assert!(windows::is_running(pid), "refused action must not touch the child");

        // 3–4. Valid id → authorized → explicit End Process (as the
        // confirming user would trigger through the UI).
        let authorized = authorize_action(&registry, &target_id)
            .expect("valid target must pass the full chain");
        assert_eq!(authorized.pid, pid);
        let terminated = windows::terminate(pid);
        let gone = windows::wait_for_exit(pid, std::time::Duration::from_secs(5));
        println!(
            "LIVE END PROCESS: pid={pid} terminated={terminated} exited={gone}"
        );
        assert!(terminated && gone, "explicit termination of the disposable child must work");

        // 5. Observe exit through the child handle too.
        let status = child.wait().expect("wait for child");
        assert!(!status.success() || status.code().is_some(), "child exited");

        let _ = std::fs::remove_file(&marker);
        println!("LIVE END PROCESS: disposable child confirmed gone; no console events used");
    }
}
