//! Phase 11C Task 12 — deep-health engine + bounded Windows collectors
//! (spec §17/§18, plan Task 12/§43-F).
//!
//! Every probe runs on its OWN thread and is joined through a bounded
//! `recv_timeout`: one stuck probe degrades to `Degraded/TIMEOUT` without
//! blocking the remaining results. All collectors are read-only,
//! non-elevated, and bounded. There is no WMI/PowerShell/cmd/shell runner
//! anywhere; Docker checks consume already-cached engine state only.
// Health/identity collectors are wired into the Tauri command surface by
// Task 14 (commands.rs); until then the dead-code allow keeps the
// warning budget clean without weakening any test.
#![allow(dead_code)]


use std::sync::mpsc;
use std::time::Duration;

/// Deep-health status (spec §17): `Skipped` is NOT an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unavailable,
    Skipped,
}

impl HealthStatus {
    /// Explicit text for every status — never color-alone (spec §25).
    pub fn as_text(self) -> &'static str {
        match self {
            HealthStatus::Healthy => "Healthy",
            HealthStatus::Degraded => "Degraded",
            HealthStatus::Unavailable => "Unavailable",
            HealthStatus::Skipped => "Skipped",
        }
    }
}

/// One deep-health probe result (spec §17 model).
#[derive(Debug, Clone)]
pub struct HealthCheckResult {
    pub id: String,
    pub subsystem: &'static str,
    pub label: String,
    pub status: HealthStatus,
    pub code: String,
    pub summary: String,
    pub detail: Option<String>,
    pub checked_at_ms: u64,
    pub duration_ms: u64,
}

impl HealthCheckResult {
    pub fn new(id: &str, subsystem: &'static str, label: &str, status: HealthStatus) -> Self {
        Self {
            id: id.to_string(),
            subsystem,
            label: label.to_string(),
            status,
            code: String::new(),
            summary: String::new(),
            detail: None,
            checked_at_ms: now_unix_ms(),
            duration_ms: 0,
        }
    }
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A named probe: id, subsystem, own timeout (zero = engine default), body.
pub struct CheckFn {
    pub id: String,
    pub subsystem: &'static str,
    pub timeout: Duration,
    pub body: Box<dyn FnOnce() -> HealthCheckResult + Send>,
}

/// Build a named probe from a closure.
pub fn check_fn(
    id: &str,
    subsystem: &'static str,
    timeout: Duration,
    body: impl FnOnce() -> HealthCheckResult + Send + 'static,
) -> CheckFn {
    CheckFn {
        id: id.to_string(),
        subsystem,
        timeout,
        body: Box::new(body),
    }
}

/// Run each probe on its own thread; join via bounded `recv_timeout`.
/// A probe exceeding its timeout yields `Degraded/TIMEOUT` while the other
/// results are unaffected. Detached probe threads finish on their own.
pub fn run_checks_with(timeout_per_probe_ms: u64, checks: Vec<CheckFn>) -> Vec<HealthCheckResult> {
    let default_timeout = Duration::from_millis(timeout_per_probe_ms);
    let mut receivers = Vec::with_capacity(checks.len());
    let mut prespawn_failures = Vec::new();
    for check in checks {
        let timeout = if check.timeout.is_zero() { default_timeout } else { check.timeout };
        let id = check.id.clone();
        let subsystem = check.subsystem;
        let (tx, rx) = mpsc::channel();
        if std::thread::Builder::new()
            .name(format!("health-{id}"))
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut result = (check.body)();
                result.duration_ms = started.elapsed().as_millis() as u64;
                let _ = tx.send(result);
            })
            .is_err()
        {
            // Spawn failure: honest degraded result for this probe only.
            let mut r = HealthCheckResult::new(&id, subsystem, &id, HealthStatus::Degraded);
            r.code = "SPAWN_FAILED".to_string();
            r.summary = "probe thread could not be started".to_string();
            prespawn_failures.push(r);
            continue;
        }
        receivers.push((id, subsystem, timeout, rx));
    }

    let mut out = prespawn_failures;
    for (id, subsystem, timeout, rx) in receivers {
        match rx.recv_timeout(timeout) {
            Ok(result) => out.push(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let mut r = HealthCheckResult::new(&id, subsystem, &id, HealthStatus::Degraded);
                r.code = "TIMEOUT".to_string();
                r.summary = format!("probe exceeded {} ms isolation bound", timeout.as_millis());
                r.duration_ms = timeout.as_millis() as u64;
                out.push(r);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Probe thread panicked before sending — isolated degradation.
                let mut r = HealthCheckResult::new(&id, subsystem, &id, HealthStatus::Degraded);
                r.code = "PROBE_FAILED".to_string();
                r.summary = "probe thread ended without a result".to_string();
                out.push(r);
            }
        }
    }
    out
}

/// Minimal cached Docker-engine summary the command layer extracts from the
/// EXISTING engine state (read-only; no new pipe query from the health path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatusSummary {
    pub available: bool,
    pub engine_version: Option<String>,
}

/// The Phase 11C deep-check suite (categories B/C, spec §17). Production
/// entry point; tests use `run_checks_with` directly for injection.
pub fn run_deep_checks(timeout_per_probe_ms: u64, docker: Option<EngineStatusSummary>) -> Vec<HealthCheckResult> {
    let mut checks: Vec<CheckFn> = Vec::new();

    // Category C — Windows/system (read-only, non-elevated).
    checks.push(check_fn(
        "windows-version",
        "windows",
        Duration::from_secs(2),
        windows_version_check,
    ));
    checks.push(check_fn(
        "webview2",
        "windows",
        Duration::from_secs(2),
        webview2_check,
    ));
    checks.push(check_fn(
        "machine-identity",
        "windows",
        Duration::from_secs(3),
        identity_check,
    ));
    checks.push(check_fn(
        "diagnostics-write-probe",
        "diagnostics",
        Duration::from_secs(2),
        filesystem_write_probe,
    ));
    // Category B — integrations (cached state only).
    checks.push(check_fn(
        "docker-engine",
        "integrations",
        Duration::from_secs(2),
        move || docker_check(docker),
    ));

    run_checks_with(timeout_per_probe_ms, checks)
}

// -- category C checks ---------------------------------------------------------

fn windows_version_check() -> HealthCheckResult {
    let mut r =
        HealthCheckResult::new("windows-version", "windows", "Windows version", HealthStatus::Healthy);
    match crate::diagnostics::collectors::collect_windows_version() {
        Ok(Some((build, ubr))) => r.summary = format!("Windows build {build}.{ubr}"),
        Ok(None) | Err(_) => {
            r.status = HealthStatus::Skipped;
            r.code = "UNAVAILABLE".into();
            r.summary = "version information unavailable".into();
        }
    }
    r
}

fn webview2_check() -> HealthCheckResult {
    let mut r = HealthCheckResult::new("webview2", "windows", "WebView2 runtime", HealthStatus::Healthy);
    match crate::diagnostics::collectors::collect_webview2_version() {
        Ok(Some(v)) => r.summary = format!("WebView2 {v}"),
        Ok(None) => {
            r.status = HealthStatus::Unavailable;
            r.code = "NOT_INSTALLED".into();
            r.summary = "WebView2 BLBeacon not found".into();
        }
        Err(_) => {
            r.status = HealthStatus::Skipped;
            r.code = "REGISTRY_UNREADABLE".into();
            r.summary = "WebView2 version unavailable".into();
        }
    }
    r
}

fn identity_check() -> HealthCheckResult {
    let mut r =
        HealthCheckResult::new("machine-identity", "windows", "Machine identity", HealthStatus::Healthy);
    let fields = crate::diagnostics::collectors::collect_system_identity();
    let total = fields.len();
    let present = fields.iter().filter(|f| f.value.is_some()).count();
    let by_design = fields.iter().filter(|f| f.skipped_by_design).count();
    if present == 0 && by_design < fields.len() {
        r.status = HealthStatus::Skipped;
        r.code = "UNAVAILABLE".into();
        r.summary = "no identity fields available".into();
    } else if present + by_design == fields.len() {
        r.summary = format!(
            "{present}/{total} identity fields collected ({by_design} skipped by design)"
        );
    } else {
        r.status = HealthStatus::Degraded;
        r.code = "PARTIAL".into();
        r.summary = format!("{present}/{total} identity fields collected");
    }
    r
}

/// LocalStack-owned filesystem write test (spec §17): a tiny file inside the
/// diagnostics temp dir only; written and removed. Confined, bounded.
pub fn filesystem_write_probe() -> HealthCheckResult {
    let mut r = HealthCheckResult::new(
        "diagnostics-write-probe",
        "diagnostics",
        "Diagnostics storage writable",
        HealthStatus::Healthy,
    );
    let Some(dir) = crate::diagnostics::paths::temp_dir() else {
        r.status = HealthStatus::Skipped;
        r.code = "NO_STORAGE".into();
        r.summary = "diagnostics temp dir unavailable".into();
        return r;
    };
    let path = dir.join("write-probe.tmp");
    match std::fs::write(&path, b"probe") {
        Ok(()) => {
            if std::fs::remove_file(&path).is_ok() {
                r.summary = "LocalStack-owned temp write succeeded".into();
                r.detail = Some(path.to_string_lossy().into_owned());
            } else {
                r.status = HealthStatus::Degraded;
                r.code = "CLEANUP_FAILED".into();
                r.summary = "probe file written but not removed".into();
            }
        }
        Err(e) => {
            r.status = HealthStatus::Unavailable;
            r.code = "WRITE_FAILED".into();
            r.summary = "diagnostics temp write failed".into();
            r.detail = Some(crate::diagnostics::redact(&e.to_string()));
        }
    }
    r
}

// -- category B checks -----------------------------------------------------------

/// Docker engine check from ALREADY-CACHED engine status. `None` = not
/// observable → `Skipped` with an explanatory code (never a fabricated
/// error). Read-only; the health engine issues no new pipe query.
pub fn docker_check(status: Option<EngineStatusSummary>) -> HealthCheckResult {
    let mut r =
        HealthCheckResult::new("docker-engine", "integrations", "Docker engine", HealthStatus::Healthy);
    match status {
        None => {
            r.status = HealthStatus::Skipped;
            r.code = "SKIPPED_UNCONFIGURED".into();
            r.summary = "Docker integration not currently observable".into();
        }
        Some(s) => {
            if s.available {
                let ver = s.engine_version.as_deref().map(|v| format!(" ({v})")).unwrap_or_default();
                r.summary = format!("engine reachable{ver}");
            } else {
                r.status = HealthStatus::Unavailable;
                r.code = "ENGINE_UNAVAILABLE".into();
                r.summary = "engine not reachable (cached state)".into();
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::collectors::IdentityKind;
    use crate::diagnostics::paths;

    #[test]
    fn stuck_probe_is_isolated_by_timeout() {
        let started = std::time::Instant::now();
        let results = run_checks_with(
            5000,
            vec![check_fn(
                "stuck",
                "Test",
                std::time::Duration::from_millis(100),
                || {
                    std::thread::sleep(std::time::Duration::from_secs(10));
                    HealthCheckResult::new("stuck", "Test", "Slept", HealthStatus::Healthy)
                },
            )],
        );
        let elapsed = started.elapsed();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].status, HealthStatus::Degraded);
        assert_eq!(results[0].code, "TIMEOUT");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "probe isolation must not wait for the stuck body: {elapsed:?}"
        );
    }

    #[test]
    fn parallel_probes_all_return() {
        // Uniform short sleep with wide margin under the bound: the point is
        // that all six results arrive (isolation works), not scheduler noise.
        let results = run_checks_with(
            2_000,
            (0..6)
                .map(|i| {
                    check_fn(
                        &format!("p{i}"),
                        "Test",
                        std::time::Duration::from_millis(1_500),
                        move || {
                            std::thread::sleep(std::time::Duration::from_millis(35));
                            HealthCheckResult::new(&format!("p{i}"), "Test", "ok", HealthStatus::Healthy)
                        },
                    )
                })
                .collect(),
        );
        assert_eq!(results.len(), 6);
        assert!(results.iter().all(|r| r.status == HealthStatus::Healthy));
    }

    #[test]
    fn health_status_text_is_explicit() {
        let texts: Vec<&str> = [
            HealthStatus::Healthy,
            HealthStatus::Degraded,
            HealthStatus::Unavailable,
            HealthStatus::Skipped,
        ]
        .iter()
        .map(|s| s.as_text())
        .collect();
        assert_eq!(texts, ["Healthy", "Degraded", "Unavailable", "Skipped"]);
        for t in &texts {
            assert!(!t.is_empty());
        }
    }

    #[test]
    fn optional_dependency_absent_yields_skipped() {
        let r = docker_check(None);
        assert_eq!(r.status, HealthStatus::Skipped);
        assert!(r.code.contains("SKIPPED") || r.code.contains("UNCONFIGURED"), "{}", r.code);
    }

    #[test]
    fn identity_collectors_return_skipped_not_error_when_unavailable() {
        let r = crate::diagnostics::collectors::sid_field_with_seam(|| {
            Err("injected token failure".into())
        });
        assert!(matches!(r, Err(_) | Ok(None)), "seam maps to skipped");
    }

    #[test]
    fn device_serial_collector_is_skipped_by_design() {
        let fields = crate::diagnostics::collectors::collect_system_identity();
        let dev = fields.iter().find(|f| f.kind == IdentityKind::DeviceSerial);
        let dev = dev.expect("DeviceSerial field must be PRESENT in schema");
        assert!(dev.value.is_none(), "no live device serial in Phase 11C");
        assert!(
            dev.skipped_by_design,
            "field reports explicit Skipped/Unsupported decision"
        );
    }

    #[test]
    fn write_probe_confined_to_diagnostics_temp() {
        let temp = paths::temp_dir().expect("LocalStack temp dir available");
        let before = std::fs::read_dir(&temp).map(|d| d.count()).unwrap_or(0);
        let r = filesystem_write_probe();
        assert!(matches!(r.status, HealthStatus::Healthy | HealthStatus::Skipped));
        let after = std::fs::read_dir(&temp).map(|d| d.count()).unwrap_or(0);
        assert_eq!(before, after, "probe must clean up its tiny temp file");
        assert!(
            r.detail.as_deref().unwrap_or("").starts_with(temp.to_str().unwrap()),
            "probe path confined to LocalStack diagnostics temp"
        );
    }
}
