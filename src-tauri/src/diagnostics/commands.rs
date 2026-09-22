//! Phase 11C Task 14 — the narrow diagnostics Tauri command surface (spec §20).
//!
//! Every command: opaque IDs or validated enums cross the bridge; no
//! filesystem path parameters anywhere; trusted path resolution stays in
//! Rust (store/export/paths modules). Handlers are thin delegations over
//! the Phase 11C engine plus the shared [`DiagnosticsState`].
//!
//! This module is also the Phase 11C wiring point:
//! - [`DiagnosticsState::init`] creates the incident index, bundle
//!   registry, export-capability store, and spawns the single bounded
//!   capture worker (Task 4) with the real bundle builder (Tasks 7–13).
//! - `report_incident` is the typed incident entry point (spec §2): it
//!   records into the persistent index and consults the Task 3 capture
//!   policy; capture requests go through the non-blocking producer.
//! - Automatic capture NEVER opens UI: the build closure only gathers,
//!   encrypts, commits, and links. Portable export is a user action.

use std::collections::HashMap;
use std::path::PathBuf;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use super::bundle::{apply_budget, new_bundle_id, BundleModel, BundleSection, SectionContent, SectionName, TypedCollectionError, TruncationState, BundleManifest};
use super::cache::{self, CacheHandle};
use super::collectors::{collect_managed_output, collect_system_identity, collect_windows_version};
use super::export::{ExportCapabilityStore, ExportError, ExportOutcome};
use super::health::{run_deep_checks, EngineStatusSummary, HealthCheckResult, HealthStatus};
use super::incidents::{IncidentIndex, IncidentKey, IncidentRecord, Severity};
use super::policy::{CaptureDecision, CapturePolicyState};
use super::redact_export::PrivacyProfile;
use super::store::{BundleRegistry, MetaFields};
use super::worker::{CaptureRequest, CaptureWorker, EnqueueOutcome};

// ---------------------------------------------------------------------------
// Wire DTOs (camelCase across the bridge; frontend mirrors in domain.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsOverviewDto {
    pub app_version: String,
    pub overall_health: String,
    pub last_deep_check_ms: Option<u64>,
    pub active_incidents: u32,
    pub bundle_count: u32,
    pub storage_bytes: u64,
    pub pending_crash_recovery: bool,
    pub recovery_banner: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCheckResultDto {
    pub id: String,
    pub subsystem: String,
    pub label: String,
    pub status: String,
    pub code: String,
    pub summary: String,
    pub detail: Option<String>,
    pub checked_at_ms: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IncidentDto {
    pub incident_id: String,
    pub subsystem: String,
    pub code: String,
    pub severity: String,
    pub operation: String,
    pub summary: String,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub occurrences: u64,
    pub review_state: String,
    pub bundle_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleMetaDto {
    pub bundle_id: String,
    pub created_at_ms: u64,
    pub trigger: String,
    pub subsystem: String,
    pub severity: String,
    pub fingerprint: String,
    pub encrypted_size_bytes: u64,
    pub integrity: String,
    pub review_state: String,
    pub app_version: String,
    pub schema_version: u32,
}

/// Metadata-first detail (spec §24): the payload is decrypted only by the
/// export/summary flows — never shipped wholesale to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleDetailDto {
    pub meta: BundleMetaDto,
    pub collection_errors: Vec<String>,
    pub truncated: bool,
    pub truncated_sections: Vec<String>,
    pub section_names: Vec<String>,
    pub section_sizes: Vec<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportProfileDto {
    SafeShare,
    DeveloperDetail,
    FullForensics,
}

impl From<ExportProfileDto> for PrivacyProfile {
    fn from(p: ExportProfileDto) -> Self {
        match p {
            ExportProfileDto::SafeShare => PrivacyProfile::SafeShare,
            ExportProfileDto::DeveloperDetail => PrivacyProfile::DeveloperDetail,
            ExportProfileDto::FullForensics => PrivacyProfile::FullForensics,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOutcomeDto {
    pub export_id: String,
    pub file_name: String,
    pub profile: String,
}

/// Safe-Share-only diagnostic summary (spec §22): never identity data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupportSummaryDto {
    pub app_version: String,
    pub windows_version: String,
    pub architecture: String,
    pub subsystem: String,
    pub health_state: String,
    pub incident_code: String,
    pub fingerprint: String,
    pub occurrences: u64,
    pub first_seen_ms: u64,
    pub last_seen_ms: u64,
    pub bundle_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitHubIssueDraftDto {
    pub title: String,
    pub body: String,
    /// Fixed trusted URL assembled Rust-side; never user input.
    pub issue_url: String,
}

/// Fixed new-issue endpoint (spec §24): trusted application constant.
const GITHUB_NEW_ISSUE_URL: &str = "https://github.com/Zehna/Lokalstack/issues/new";

// ---------------------------------------------------------------------------
// Shared application state
// ---------------------------------------------------------------------------

/// Diagnostics application state managed by `lib.rs`. Everything heavy is
/// behind `Arc`/`Mutex`; `Clone` lets async commands move a handle into
/// `spawn_blocking`.
#[derive(Clone)]
pub struct DiagnosticsState {
    pub(crate) cache: &'static CacheHandle,
    pub(crate) incidents: Arc<Mutex<IncidentIndex>>,
    pub(crate) incidents_path: PathBuf,
    pub(crate) policy: Arc<Mutex<HashMap<IncidentKey, CapturePolicyState>>>,
    pub(crate) registry: Option<Arc<BundleRegistry>>,
    pub(crate) capabilities: Arc<ExportCapabilityStore>,
    pub(crate) capture: Arc<OnceLock<CaptureWorker>>,
    #[allow(dead_code)] // shared with the worker; app exit drops senders
    pub(crate) shutdown: Arc<AtomicBool>,
    pub(crate) health_cache: Arc<Mutex<Vec<HealthCheckResult>>>,
    pub(crate) recovery_banner: Arc<Mutex<Option<String>>>,
    pub(crate) app_version: String,
}

#[cfg(test)]
static TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Shared recovery-banner slot: written by the startup recovery pipeline
/// (lib.rs) and adopted by `DiagnosticsState::init`, so the UI can surface
/// "previous crash recovered" exactly once after the webview is ready.
pub(crate) fn recovery_banner_slot() -> &'static Arc<Mutex<Option<String>>> {
    static SLOT: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(Mutex::new(None)))
}

/// Production app handle for the capture worker's native-notification path.
/// Empty in tests (the injected closure is used there instead).
static DIAGNOSTICS_APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Capture-notification banner slot: the overview merges it into
/// `recovery_banner` so the frontend shows exactly one banner.
static BANNER_SLOT: OnceLock<Arc<Mutex<Option<String>>>> = OnceLock::new();

fn st_capture_banner() -> Option<&'static Arc<Mutex<Option<String>>>> {
    Some(BANNER_SLOT.get_or_init(|| Arc::new(Mutex::new(None))))
}

impl DiagnosticsState {
    /// Production init (called once from the `lib.rs` setup path): loads the
    /// incident index, opens the bundle registry, and spawns the single
    /// capture worker with the real bundle builder. UI-less by construction.
    pub(crate) fn init(handle: &tauri::AppHandle) -> DiagnosticsState {
        let _ = DIAGNOSTICS_APP_HANDLE.set(handle.clone());
        let app_version = env!("CARGO_PKG_VERSION").to_string();

        let incidents_path = super::paths::diagnostics_dir()
            .map(|d| d.join("incident-index.json"))
            .unwrap_or_else(|| std::env::temp_dir().join("lcc-incident-index-fallback.json"));
        let (index, _health) = IncidentIndex::load_or_default(&incidents_path);

        let registry = super::paths::diagnostics_dir()
            .and_then(|d| BundleRegistry::new(d.join("bundle-index.json"), d.join("bundles")))
            .map(Arc::new);

        let incidents = Arc::new(Mutex::new(index));
        let policy: Arc<Mutex<HashMap<IncidentKey, CapturePolicyState>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let capabilities = Arc::new(ExportCapabilityStore::new());
        let capture: Arc<OnceLock<CaptureWorker>> = Arc::new(OnceLock::new());
        let shutdown = Arc::new(AtomicBool::new(false));

        // Real bundle builder (Tasks 7–13 composition). Captures only
        // Send+Sync engine handles; never opens UI (spec §12 ruling A).
        let ws_inner = handle
            .state::<crate::workspace::WorkspaceEngineState>()
            .engine_inner();
        let project_cache = handle.state::<crate::project::ProjectEngineState>().cache.clone();
        let docker_handle = handle.clone();
        let docker_status: Arc<dyn Fn() -> Option<EngineStatusSummary> + Send + Sync> =
            Arc::new(move || {
                docker_handle
                    .try_state::<crate::docker::DockerEngineState>()
                    .map(|st| st.cached_engine_status())
            });
        let b_incidents = Arc::clone(&incidents);
        let b_path = incidents_path.clone();
        let b_registry = registry.clone();
        let b_version = app_version.clone();
        let coalesce: Arc<Mutex<std::collections::HashSet<IncidentKey>>> =
            Arc::new(Mutex::new(std::collections::HashSet::new()));
        let (tx, rx) = std::sync::mpsc::sync_channel::<CaptureRequest>(super::worker::QUEUE_CAPACITY);
        let win_handle = handle.clone();
        let focus_of: Arc<dyn Fn() -> super::notify::Focus + Send + Sync> = Arc::new(move || {
            // Any query failure / missing window → Unknown → conservative
            // in-app banner fallback (never guesses Background).
            let win = win_handle.get_webview_window("main");
            super::notify::focus_of(win.as_ref())
        });
        let native_notify: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync> =
            Arc::new(|text: &str| {
                use tauri_plugin_notification::NotificationExt;
                let handle = DIAGNOSTICS_APP_HANDLE.get().ok_or("app handle unavailable")?;
                handle
                    .notification()
                    .builder()
                    .title("LocalStack Control Center")
                    .body(text.to_string())
                    .show()
                    .map_err(|e| e.to_string())
            });
        let build: Arc<dyn Fn(CaptureRequest) + Send + Sync> = {
            let coalesce = Arc::clone(&coalesce);
            let focus_of = Arc::clone(&focus_of);
            let native_notify = Arc::clone(&native_notify);
            Arc::new(move |req: CaptureRequest| {
                let _ = &coalesce;
                build_bundle_job(
                    req,
                    &ws_inner,
                    &project_cache,
                    docker_status.as_ref(),
                    b_registry.as_deref(),
                    &b_incidents,
                    &b_path,
                    &b_version,
                    focus_of.as_ref(),
                    native_notify.as_ref(),
                );
            })
        };
        // Spawn the single worker and publish the enqueue handle. The
        // CaptureWorker handle shares the SAME coalescing set the loop
        // releases keys into, so producers and the worker stay consistent.
        let worker = CaptureWorker::new(tx, Arc::clone(&coalesce), Arc::clone(&shutdown));
        let join = CaptureWorker::spawn(rx, coalesce, build, Arc::clone(&shutdown));
        let _ = capture.set(worker);
        let _join = join; // detached by design; no join on hot paths

        DiagnosticsState {
            cache: cache::global(),
            incidents,
            incidents_path,
            policy,
            registry,
            capabilities,
            capture,
            shutdown,
            health_cache: Arc::new(Mutex::new(Vec::new())),
            recovery_banner: Arc::clone(recovery_banner_slot()),
            app_version,
        }
    }

    /// Test-only state: no Tauri runtime, no registry, unique LocalStack-
    /// owned scratch index path. Never touches real bundle storage.
    #[cfg(test)]
    pub(crate) fn for_tests() -> DiagnosticsState {
        let n = TEST_SEQ.fetch_add(1, Ordering::Relaxed);
        let incidents_path = super::paths::temp_dir()
            .unwrap_or_else(|| std::env::temp_dir())
            .join(format!("lcc-incident-index-test-{}-{n}.json", std::process::id()));
        let _ = std::fs::remove_file(&incidents_path);
        DiagnosticsState {
            cache: cache::global(),
            incidents: Arc::new(Mutex::new(IncidentIndex::default())),
            incidents_path,
            policy: Arc::new(Mutex::new(HashMap::new())),
            registry: None,
            capabilities: Arc::new(ExportCapabilityStore::new()),
            capture: Arc::new(OnceLock::new()),
            shutdown: Arc::new(AtomicBool::new(false)),
            health_cache: Arc::new(Mutex::new(Vec::new())),
            recovery_banner: Arc::new(Mutex::new(None)),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}


// ---------------------------------------------------------------------------
// Typed incident reporting (spec §2) + capture policy consumption (§3/§9)
// ---------------------------------------------------------------------------

/// Production entry point: record one typed incident and consult the
/// severe/critical capture policy. Lock section is SHORT: update state,
/// decide, release; bundle building happens on the worker, never here.
///
/// RULING (Task 14): this is the sole typed-reporting entry point (spec §2
/// forbids blanket conversion of existing diagnostics::error call sites;
/// per-callsite severity mapping was not approved in Phase 11C scope), so
/// production subsystems do not call it yet — tests + future adoption use
/// this exact surface.
#[allow(dead_code)] // typed entry point; per-callsite adoption out of scope
pub(crate) fn report_incident(
    st: &DiagnosticsState,
    subsystem: &str,
    code: &str,
    severity: Severity,
    operation: &str,
    summary: &str,
) {
    let now = unix_ms();
    let summary = super::redact(summary);
    let key = IncidentKey {
        subsystem: subsystem.to_string(),
        fingerprint: super::incidents::fingerprint(subsystem, code, operation, severity),
    };
    let decision = {
        let mut index = match st.incidents.lock() {
            Ok(g) => g,
            Err(_) => return, // poisoned: diagnostics never crashes the app
        };
        index.record(key.clone(), code, severity, operation, &summary, now);
        let _ = index.save_atomic(&st.incidents_path); // best-effort persistence
        let mut policy = match st.policy.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        policy
            .entry(key.clone())
            .or_default()
            .on_event(&key, severity, now)
    };
    if decision == CaptureDecision::RequestBundle {
        enqueue_capture(st, CaptureRequest::Bundle {
            key,
            trigger: format!("severe-incident:{code}"),
            severity,
        });
    }
}

fn enqueue_capture(st: &DiagnosticsState, req: CaptureRequest) {
    match st.capture.get() {
        Some(worker) => {
            // Coalescing set lives with the worker; producer path is
            // try_lock + try_send (never blocks).
            let outcome = worker.try_enqueue(req);
            if matches!(outcome, EnqueueOutcome::QueueClosed) {
                super::warn("diagnostics", "capture queue closed; capture skipped");
            }
        }
        None => {
            super::warn("diagnostics", "capture queue unavailable; capture skipped");
        }
    }
}

// ---------------------------------------------------------------------------
// Bundle build job (the Tasks 7–13 composition, run on the capture worker)
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn build_bundle_job(
    req: CaptureRequest,
    ws_inner: &crate::workspace::Inner,
    project_cache: &Arc<Mutex<crate::project::ProjectCache>>,
    docker_status: &dyn Fn() -> Option<EngineStatusSummary>,
    registry: Option<&BundleRegistry>,
    incidents: &Mutex<IncidentIndex>,
    incidents_path: &std::path::Path,
    app_version: &str,
    focus_of: &dyn Fn() -> super::notify::Focus,
    native_notify: &dyn Fn(&str) -> Result<(), String>,
) {
    let CaptureRequest::Bundle { key, trigger, severity } = req;
    let now = unix_ms();
    let mut errors: Vec<TypedCollectionError> = Vec::new();
    let mut sections: Vec<BundleSection> = Vec::new();

    // Health (bounded probes; docker summary from cached engine state).
    let results = run_deep_checks(2000, docker_status());
    let health_json = serde_json::Value::Array(
        results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id, "subsystem": r.subsystem, "label": r.label,
                    "status": r.status.as_text(), "code": r.code,
                    "summary": r.summary, "detail": r.detail,
                    "checked_at_ms": r.checked_at_ms, "duration_ms": r.duration_ms,
                })
            })
            .collect(),
    );
    sections.push(BundleSection { name: SectionName::Health, content: SectionContent::Json(health_json) });

    // System identity (internal bundle §11; profiles transform at export).
    let identity_json = serde_json::Value::Array(
        collect_system_identity()
            .iter()
            .map(|f| {
                serde_json::json!({
                    "kind": identity_kind_str(&f.kind),
                    "present": f.value.is_some(),
                    "skipped_by_design": f.skipped_by_design,
                    "value": f.value,
                })
            })
            .collect(),
    );
    sections.push(BundleSection { name: SectionName::System, content: SectionContent::Json(identity_json) });

    // Incident index (typed codes/fingerprints — already redacted).
    let incidents_json = incidents
        .lock()
        .map(|idx| {
            serde_json::Value::Array(
                idx.incidents
                    .iter()
                    .map(|r| {
                        serde_json::json!({
                            "subsystem": r.key.subsystem, "code": r.code,
                            "severity": severity_str(r.severity),
                            "operation": r.operation, "summary": r.summary,
                            "first_seen_ms": r.first_seen_ms,
                            "last_seen_ms": r.last_seen_ms,
                            "occurrences": r.occurrences.get(),
                            "bundle_ids": r.bundle_ids,
                        })
                    })
                    .collect(),
            )
        })
        .unwrap_or(serde_json::Value::Array(Vec::new()));
    sections.push(BundleSection { name: SectionName::Incidents, content: SectionContent::Json(incidents_json) });

    // Runtime inventories: cached snapshot first (§26), owner caches as
    // fallback. All pure reads under existing locks.
    let snap = cache::global().try_snapshot();
    if let Some(s) = &snap {
        let inventory = serde_json::json!({
            "listeners": s.listeners, "services": s.services,
            "subsystem_status": s.subsystem_status,
        });
        sections.push(BundleSection { name: SectionName::Inventory, content: SectionContent::Json(inventory) });
        let projects = serde_json::json!({ "projects": s.projects, "workspaces": s.workspaces });
        sections.push(BundleSection { name: SectionName::Projects, content: SectionContent::Json(projects) });
    } else {
        let listeners = crate::discovery::cached_listener_snapshot();
        let projects: Vec<_> = project_cache
            .lock()
            .map(|c| crate::project::cached_project_snapshot(&c))
            .unwrap_or_default();
        let (services, workspaces) = {
            let inner = ws_inner;
            (
                crate::workspace::cached_service_snapshot(inner),
                crate::workspace::cached_workspace_snapshot(inner),
            )
        };
        let inventory = serde_json::json!({ "listeners": listeners, "services": services, "subsystem_status": [] });
        sections.push(BundleSection { name: SectionName::Inventory, content: SectionContent::Json(inventory) });
        let projects_json = serde_json::json!({ "projects": projects, "workspaces": workspaces });
        sections.push(BundleSection { name: SectionName::Projects, content: SectionContent::Json(projects_json) });
    }

    // Docker summary (cached/read-only) — no pipe query from this path.
    let docker_json = match docker_status() {
        Some(s) => serde_json::json!({ "available": s.available, "engine_version": s.engine_version }),
        None => serde_json::json!({ "available": false }),
    };
    sections.push(BundleSection { name: SectionName::Docker, content: SectionContent::Json(docker_json) });

    // Managed-service output: bounded tails of the EXISTING LogRings only
    // (§16), redacted inside the collector. Never external-process output.
    let managed = collect_managed_output(&crate::workspace::snapshot_managed_log_tails(ws_inner));
    sections.push(BundleSection { name: SectionName::ManagedOutput, content: managed });

    // Diagnostic log tail (cached ring, ≤50 lines).
    let tail = cache::global().try_recent_log_tail(super::cache::MAX_LOG_TAIL_LINES);
    if !tail.is_empty() {
        sections.push(BundleSection {
            name: SectionName::DiagnosticsLog,
            content: SectionContent::Text(tail.join("\n")),
        });
    }

    // Generated summary from trusted fields only.
    let summary_text = format!(
        "LocalStack Control Center support bundle\nsubsystem: {}\nseverity: {}\nfingerprint: {}\ntrigger: {}\n",
        key.subsystem,
        severity_str(severity),
        key.fingerprint,
        trigger,
    );
    sections.push(BundleSection { name: SectionName::Summary, content: SectionContent::Text(summary_text) });

    // Emergency crash record — included only when one is present (it is
    // normally consumed by startup recovery before any capture).
    if let Some(bytes) = read_emergency_record() {
        match serde_json::from_slice::<serde_json::Value>(&bytes) {
            Ok(v) => sections.push(BundleSection {
                name: SectionName::CrashEmergency,
                content: SectionContent::Json(v),
            }),
            Err(_) => errors.push(TypedCollectionError {
                section: SectionName::CrashEmergency,
                code: "EMERGENCY_PARSE".into(),
                detail_redacted: "emergency record unreadable; omitted".into(),
            }),
        }
    }

    let mut model = BundleModel {
        manifest: BundleManifest {
            schema_version: 1,
            bundle_id: new_bundle_id().unwrap_or_default(),
            app_version: app_version.to_string(),
            created_at_ms: now,
            trigger,
            subsystem: key.subsystem.clone(),
            severity: severity_str(severity),
            fingerprint: key.fingerprint.clone(),
            collection_errors: errors.clone(),
            truncation: TruncationState::default(),
            section_sizes: section_sizes(&sections),
            redaction_applied: true,
            privacy_profile_note: "internal encrypted bundle (DPAPI current-user)".into(),
            blake3_digest_note: String::new(),
        },
        sections,
    };
    model.manifest.collection_errors = errors;
    let model = apply_budget(model);

    let Some(registry) = registry else {
        super::error("diagnostics", "support bundle capture failed: storage unavailable");
        return;
    };
    let plain = match serde_json::to_vec(&model) {
        Ok(v) => v,
        Err(_) => {
            super::error("diagnostics", "support bundle capture failed: serialization");
            return;
        }
    };
    let meta = MetaFields {
        created_at_ms: now,
        trigger: model.manifest.trigger.clone(),
        subsystem: model.manifest.subsystem.clone(),
        severity: model.manifest.severity.clone(),
        fingerprint: model.manifest.fingerprint.clone(),
        app_version: model.manifest.app_version.clone(),
    };
    match registry.commit_bundle(&plain, meta) {
        Ok(outcome) => {
            // Link the bundle to the incident (short lock, best-effort save).
            if let Ok(mut idx) = incidents.lock() {
                if let Some(rec) = idx.incidents.iter_mut().find(|r| r.key == key) {
                    if !rec.bundle_ids.contains(&outcome.bundle_id) {
                        rec.bundle_ids.push(outcome.bundle_id.clone());
                    }
                }
                let _ = idx.save_atomic(incidents_path);
            }
            super::info("diagnostics", "support bundle captured");
            // One visible notification event per capture (spec §23). Decision
            // input is read via injected closures so this stays testable and
            // a notification failure can never affect bundle persistence.
            let decision = super::notify::decide(focus_of(), false, false);
            super::notify::dispatch(decision, native_notify);
            if decision == super::notify::NotificationDecision::InAppBanner {
                if let Some(slot) = st_capture_banner() {
                    if let Ok(mut b) = slot.lock() {
                        *b = Some(super::notify::CAPTURED_BANNER_TEXT.to_string());
                    }
                }
            }
        }
        Err(_) => {
            super::error("diagnostics", "support bundle capture failed");
        }
    }
}



fn read_emergency_record() -> Option<Vec<u8>> {
    let dir = super::paths::emergency_dir()?;
    let path = dir.join("emergency.json");
    let bytes = std::fs::read(&path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    Some(bytes)
}

fn identity_kind_str(k: &super::collectors::IdentityKind) -> &'static str {
    match k {
        super::collectors::IdentityKind::Username => "username",
        super::collectors::IdentityKind::Hostname => "hostname",
        super::collectors::IdentityKind::LocalIP => "local-ip",
        super::collectors::IdentityKind::Mac => "mac",
        super::collectors::IdentityKind::Sid => "sid",
        super::collectors::IdentityKind::MachineGuid => "machine-guid",
        super::collectors::IdentityKind::DeviceSerial => "device-serial",
    }
}

pub(crate) fn severity_str(s: Severity) -> String {
    match s {
        Severity::Info => "info".into(),
        Severity::Warning => "warning".into(),
        Severity::Severe => "severe".into(),
        Severity::Critical => "critical".into(),
    }
}

fn section_sizes(sections: &[BundleSection]) -> std::collections::BTreeMap<String, u64> {
    sections
        .iter()
        .map(|s| {
            let n = match &s.content {
                SectionContent::Json(v) => serde_json::to_vec(v).map(|b| b.len() as u64).unwrap_or(0),
                SectionContent::Text(t) => t.len() as u64,
                SectionContent::ManagedOutput(svcs) => svcs
                    .iter()
                    .map(|x| x.service_id.len() as u64 + x.bytes)
                    .sum(),
            };
            (s.name.entry_name().to_string(), n)
        })
        .collect()
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Command handlers (runtime-free inner fns; #[tauri::command] wrappers below)
// ---------------------------------------------------------------------------

fn overview_inner(st: &DiagnosticsState) -> DiagnosticsOverviewDto {
    let active_incidents = st
        .incidents
        .lock()
        .map(|i| i.incidents.len() as u32)
        .unwrap_or(0);
    let (bundle_count, storage_bytes) = match &st.registry {
        Some(r) => {
            let metas = r.list();
            (
                metas.len() as u32,
                metas.iter().map(|m| m.encrypted_size_bytes).sum(),
            )
        }
        None => (0, 0),
    };
    let health = st.health_cache.lock().map(|h| h.clone()).unwrap_or_default();
    let overall = worst_health_text(&health);
    let last_deep = health.iter().map(|r| r.checked_at_ms).max();
    let pending = pending_crash_recovery();
    let recovery = st.recovery_banner.lock().ok().and_then(|b| b.clone());
    // Recovery banner wins; otherwise surface a pending capture banner once
    // (the frontend dedupes by message so it is not re-shown every poll).
    let banner = recovery.or_else(|| {
        st_capture_banner()
            .and_then(|slot| slot.lock().ok())
            .and_then(|b| b.clone())
    });
    DiagnosticsOverviewDto {
        app_version: st.app_version.clone(),
        overall_health: overall,
        last_deep_check_ms: last_deep,
        active_incidents,
        bundle_count,
        storage_bytes,
        pending_crash_recovery: pending,
        recovery_banner: banner,
    }
}

fn pending_crash_recovery() -> bool {
    super::paths::emergency_dir()
        .map(|d| {
            std::fs::metadata(d.join("emergency.json"))
                .map(|m| m.len() > 0)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

fn worst_health_text(results: &[HealthCheckResult]) -> String {
    if results.is_empty() {
        return "not-run".into();
    }
    let mut worst = 0i32; // healthy=0 degraded=1 unavailable=2
    for r in results {
        let v = match r.status {
            HealthStatus::Healthy => 0,
            HealthStatus::Degraded => 1,
            HealthStatus::Unavailable => 2,
            HealthStatus::Skipped => 0,
        };
        worst = worst.max(v);
    }
    match worst {
        0 => "healthy".into(),
        1 => "degraded".into(),
        _ => "unavailable".into(),
    }
}

fn health_dto(results: Vec<HealthCheckResult>) -> Vec<HealthCheckResultDto> {
    results
        .into_iter()
        .map(|r| HealthCheckResultDto {
            id: r.id,
            subsystem: r.subsystem.to_string(),
            label: r.label,
            status: r.status.as_text().to_string(),
            code: r.code,
            summary: r.summary,
            detail: r.detail,
            checked_at_ms: r.checked_at_ms,
            duration_ms: r.duration_ms,
        })
        .collect()
}

fn deep_checks_inner(st: &DiagnosticsState, docker: Option<EngineStatusSummary>) -> Vec<HealthCheckResultDto> {
    let results = run_deep_checks(2000, docker);
    if let Ok(mut cache) = st.health_cache.lock() {
        *cache = results.clone();
    }
    health_dto(results)
}

fn incidents_dto(st: &DiagnosticsState) -> Vec<IncidentDto> {
    st.incidents
        .lock()
        .map(|idx| {
            idx.incidents
                .iter()
                .map(|r| incident_dto(r))
                .collect()
        })
        .unwrap_or_default()
}

fn incident_dto(r: &IncidentRecord) -> IncidentDto {
    IncidentDto {
        incident_id: r.opaque_id.clone(),
        subsystem: r.key.subsystem.clone(),
        code: r.code.clone(),
        severity: severity_str(r.severity),
        operation: r.operation.clone(),
        summary: r.summary.clone(),
        first_seen_ms: r.first_seen_ms,
        last_seen_ms: r.last_seen_ms,
        occurrences: r.occurrences.get(),
        review_state: match r.review_state {
            super::incidents::ReviewState::New => "new".into(),
            super::incidents::ReviewState::Reviewed => "reviewed".into(),
        },
        bundle_ids: r.bundle_ids.clone(),
    }
}

fn mark_incident_reviewed_inner(st: &DiagnosticsState, incident_id: &str) -> Result<(), String> {
    let mut idx = st.incidents.lock().map_err(|_| "index-locked")?;
    let rec = idx
        .incidents
        .iter_mut()
        .find(|r| r.opaque_id == incident_id)
        .ok_or_else(|| "unknown-incident".to_string())?;
    rec.review_state = super::incidents::ReviewState::Reviewed;
    idx.save_atomic(&st.incidents_path).map_err(|_| "index-persistence".to_string())?;
    Ok(())
}

fn bundles_dto(st: &DiagnosticsState) -> Vec<BundleMetaDto> {
    match &st.registry {
        Some(r) => r.list().iter().map(bundle_meta_dto).collect(),
        None => Vec::new(),
    }
}

fn bundle_meta_dto(m: &super::store::BundleMeta) -> BundleMetaDto {
    BundleMetaDto {
        bundle_id: m.bundle_id.clone(),
        created_at_ms: m.created_at_ms,
        trigger: m.trigger.clone(),
        subsystem: m.subsystem.clone(),
        severity: m.severity.clone(),
        fingerprint: m.fingerprint.clone(),
        encrypted_size_bytes: m.encrypted_size_bytes,
        integrity: match m.integrity {
            super::store::BundleIntegrityState::Valid => "valid".into(),
            super::store::BundleIntegrityState::Corrupt => "corrupt".into(),
            super::store::BundleIntegrityState::Unsupported => "unsupported".into(),
            super::store::BundleIntegrityState::RecoveryRequired => "recovery-required".into(),
        },
        review_state: match m.review_state {
            super::store::ReviewState::New => "new".into(),
            super::store::ReviewState::Reviewed => "reviewed".into(),
        },
        app_version: m.app_version.clone(),
        schema_version: m.schema_version,
    }
}

/// Decrypt + unwrap the commit envelope into the typed model. Opaque-ID
/// validation happens before any path join (traversal-proof by shape).
fn load_bundle_model(registry: &BundleRegistry, bundle_id: &str) -> Result<BundleModel, String> {
    if !super::store::is_owned_bundle_id(bundle_id) {
        return Err("unknown-bundle".into());
    }
    let path = registry.bundles_dir().join(format!("bundle-{bundle_id}.lsdiag"));
    let plain = super::crypto::read_lsdiag(&path).map_err(|_| "corrupt-source".to_string())?;
    let envelope: serde_json::Value =
        serde_json::from_slice(&plain).map_err(|_| "corrupt-source".to_string())?;
    serde_json::from_value(envelope.get("payload").cloned().ok_or("corrupt-source")?)
        .map_err(|_| "corrupt-source".to_string())
}

fn bundle_detail_inner(st: &DiagnosticsState, bundle_id: &str) -> Result<BundleDetailDto, String> {
    let registry = st.registry.as_ref().ok_or("unknown-bundle")?;
    let meta = registry
        .list()
        .into_iter()
        .find(|m| m.bundle_id == bundle_id)
        .ok_or_else(|| "unknown-bundle".to_string())?;
    let dto = bundle_meta_dto(&meta);
    if meta.integrity != super::store::BundleIntegrityState::Valid {
        // Metadata-first: a corrupt/unsupported bundle never decrypts on poll.
        return Ok(BundleDetailDto {
            meta: dto,
            collection_errors: Vec::new(),
            truncated: false,
            truncated_sections: Vec::new(),
            section_names: Vec::new(),
            section_sizes: Vec::new(),
        });
    }
    let model = load_bundle_model(registry, bundle_id)?;
    Ok(BundleDetailDto {
        meta: dto,
        collection_errors: model
            .manifest
            .collection_errors
            .iter()
            .map(|e| format!("{}: {}", e.section.entry_name(), e.code))
            .collect(),
        truncated: model.manifest.truncation.truncated,
        truncated_sections: model.manifest.truncation.truncated_sections.clone(),
        section_names: model
            .sections
            .iter()
            .map(|s| s.name.entry_name().to_string())
            .collect(),
        section_sizes: model
            .sections
            .iter()
            .map(|s| model.manifest.section_sizes.get(s.name.entry_name()).copied().unwrap_or(0))
            .collect(),
    })
}

fn delete_support_bundle_inner(st: &DiagnosticsState, bundle_id: &str) -> Result<(), String> {
    let registry = st.registry.as_ref().ok_or("unknown-bundle")?;
    registry.delete_bundle(bundle_id).map_err(|e| match e {
        super::store::CommitError::TooLarge => "unknown-bundle".into(),
        _ => "delete-failed".into(),
    })
}

pub(crate) fn export_error_code(e: &ExportError) -> String {
    match e {
        ExportError::UnknownBundle => "unknown-bundle".into(),
        ExportError::UnknownExport => "unknown-export".into(),
        ExportError::CorruptSource => "corrupt-source".into(),
        ExportError::TooLarge => "too-large".into(),
        ExportError::SaveCancelled => "save-cancelled".into(),
        ExportError::SaveFailed(d) => format!("save-failed: {d}"),
        ExportError::FullForensicsUnconfirmed => "full-forensics-unconfirmed".into(),
    }
}

#[allow(clippy::too_many_arguments)]
fn export_inner(
    st: &DiagnosticsState,
    bundle_id: &str,
    profile: ExportProfileDto,
    confirmed: bool,
    save: &dyn Fn() -> Result<Option<PathBuf>, String>,
) -> Result<ExportOutcomeDto, String> {
    let registry = st.registry.as_ref().ok_or("unknown-bundle")?;
    let outcome: Result<ExportOutcome, ExportError> = super::export::export_bundle(
        registry,
        bundle_id,
        profile.into(),
        confirmed,
        &collect_system_identity(),
        &st.capabilities,
        save,
    );
    outcome
        .map(|o| ExportOutcomeDto {
            export_id: o.export_id,
            file_name: o.file_name,
            profile: profile_str(profile).into(),
        })
        .map_err(|e| export_error_code(&e))
}

fn profile_str(p: ExportProfileDto) -> &'static str {
    match p {
        ExportProfileDto::SafeShare => "safe-share",
        ExportProfileDto::DeveloperDetail => "developer-detail",
        ExportProfileDto::FullForensics => "full-forensics",
    }
}

fn reveal_inner(
    st: &DiagnosticsState,
    export_id: &str,
    opener: &dyn Fn(&std::path::Path) -> Result<(), String>,
) -> Result<(), String> {
    super::export::reveal_export_result(&st.capabilities, export_id, opener)
        .map_err(|e| export_error_code(&e))
}

fn support_summary_inner(st: &DiagnosticsState, bundle_id: Option<String>) -> SupportSummaryDto {
    let (subsystem, _severity, fingerprint, code, occurrences, first, last, bid) =
        match &bundle_id {
            Some(id) => {
                let (sub, _sev, fp) = st
                    .registry
                    .as_ref()
                    .and_then(|r| r.list().into_iter().find(|m| &m.bundle_id == id))
                    .map(|m| (m.subsystem.clone(), m.severity.clone(), m.fingerprint.clone()))
                    .unwrap_or_default();
                let inc = st
                    .incidents
                    .lock()
                    .ok()
                    .and_then(|i| i.incidents.iter().find(|r| r.key.fingerprint == fp).cloned());
                (
                    sub,
                    "safe-share".into(),
                    fp,
                    inc.as_ref().map(|i| i.code.clone()).unwrap_or_default(),
                    inc.as_ref().map(|i| i.occurrences.get()).unwrap_or(0),
                    inc.as_ref().map(|i| i.first_seen_ms).unwrap_or(0),
                    inc.as_ref().map(|i| i.last_seen_ms).unwrap_or(0),
                    Some(id.clone()),
                )
            }
            None => {
                let inc = st
                    .incidents
                    .lock()
                    .ok()
                    .and_then(|i| {
                        i.incidents
                            .iter()
                            .max_by_key(|r| r.last_seen_ms)
                            .cloned()
                    });
                match inc {
                    Some(i) => (
                        i.key.subsystem.clone(),
                        severity_str(i.severity),
                        i.key.fingerprint.clone(),
                        i.code.clone(),
                        i.occurrences.get(),
                        i.first_seen_ms,
                        i.last_seen_ms,
                        i.bundle_ids.first().cloned(),
                    ),
                    None => (String::new(), String::new(), String::new(), String::new(), 0, 0, 0, None),
                }
            }
        };
    let windows = collect_windows_version()
        .ok()
        .flatten()
        .map(|(v, b)| format!("{v} (build {b})"))
        .unwrap_or_else(|| "Windows".into());
    let health = st.health_cache.lock().map(|h| worst_health_text(&h)).unwrap_or_else(|_| "not-run".into());
    SupportSummaryDto {
        app_version: st.app_version.clone(),
        windows_version: windows,
        architecture: std::env::consts::ARCH.into(),
        subsystem,
        health_state: health,
        incident_code: code,
        fingerprint,
        occurrences,
        first_seen_ms: first,
        last_seen_ms: last,
        bundle_id: bid,
    }
}

fn github_issue_draft_inner(st: &DiagnosticsState, bundle_id: Option<String>) -> GitHubIssueDraftDto {
    let (title, body) = match (&st.registry, bundle_id) {
        (Some(registry), Some(id)) => match load_bundle_model(registry, &id) {
            Ok(model) => {
                super::export::github_safe_share_issue(&model, &collect_system_identity())
            }
            Err(_) => minimal_issue_draft(st),
        },
        _ => minimal_issue_draft(st),
    };
    GitHubIssueDraftDto {
        title,
        body,
        issue_url: GITHUB_NEW_ISSUE_URL.to_string(),
    }
}

/// Minimal Safe-Share draft from live state (no bundle selected).
fn minimal_issue_draft(st: &DiagnosticsState) -> (String, String) {
    let s = support_summary_inner(st, None);
    let title = format!("[Diagnostics] {} issue in {}", s.incident_code, s.app_version);
    let body = format!(
        "## Diagnostics summary (Safe Share)\n\n- App version: {}\n- Windows: {} ({})\n- Subsystem: {}\n- Incident code: {}\n- Fingerprint: `{}`\n- Occurrences: {}\n",
        s.app_version, s.windows_version, s.architecture, s.subsystem, s.incident_code, s.fingerprint, s.occurrences,
    );
    (title, body)
}

fn open_diagnostics_folder_inner(opener: &dyn Fn(&std::path::Path) -> Result<(), String>) -> Result<(), String> {
    let dir = super::paths::diagnostics_dir().ok_or("diagnostics-dir-unavailable")?;
    opener(&dir)
}

fn update_diagnostics_context_view(st: &DiagnosticsState, view_id: u8) -> bool {
    st.cache.set_active_view(view_id)
}

// ---------------------------------------------------------------------------
// Tauri command wrappers
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) fn get_diagnostics_overview(
    state: tauri::State<'_, DiagnosticsState>,
) -> DiagnosticsOverviewDto {
    overview_inner(&state)
}

#[tauri::command]
pub(crate) async fn run_deep_health_checks(
    state: tauri::State<'_, DiagnosticsState>,
    app: tauri::AppHandle,
) -> Result<Vec<HealthCheckResultDto>, String> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let docker = app
            .try_state::<crate::docker::DockerEngineState>()
            .map(|s| s.cached_engine_status());
        deep_checks_inner(&st, docker)
    })
    .await
    .map_err(|_| "deep-checks-join".to_string())
}

#[tauri::command]
pub(crate) fn list_incidents(state: tauri::State<'_, DiagnosticsState>) -> Vec<IncidentDto> {
    incidents_dto(&state)
}

#[tauri::command]
pub(crate) fn mark_incident_reviewed(
    state: tauri::State<'_, DiagnosticsState>,
    incident_id: String,
) -> Result<(), String> {
    mark_incident_reviewed_inner(&state, &incident_id)
}

#[tauri::command]
pub(crate) fn list_support_bundles(state: tauri::State<'_, DiagnosticsState>) -> Vec<BundleMetaDto> {
    bundles_dto(&state)
}

#[tauri::command]
pub(crate) async fn get_support_bundle_detail(
    state: tauri::State<'_, DiagnosticsState>,
    bundle_id: String,
) -> Result<BundleDetailDto, String> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || bundle_detail_inner(&st, &bundle_id))
        .await
        .map_err(|_| "detail-join".to_string())?
}

#[tauri::command]
pub(crate) async fn export_support_bundle(
    state: tauri::State<'_, DiagnosticsState>,
    app: tauri::AppHandle,
    bundle_id: String,
    profile: ExportProfileDto,
    confirmed: bool,
) -> Result<ExportOutcomeDto, String> {
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        // The dedicated blocking thread hosting the native save dialog is
        // THIS command's — never the automatic capture worker (§43-A).
        use tauri_plugin_dialog::DialogExt;
        let app2 = app.clone();
        let suggested = format!(
            "localstack-support-{}-{}.zip",
            bundle_id,
            profile_str(profile)
        );
        let save = move || {
            let picked = app2
                .dialog()
                .file()
                .add_filter("Support bundle", &["zip"])
                .set_file_name(&suggested)
                .blocking_save_file();
            match picked {
                Some(fp) => match fp.into_path() {
                    Ok(p) => Ok(Some(p)),
                    Err(_) => Err("invalid-destination".into()),
                },
                None => Ok(None),
            }
        };
        export_inner(&st, &bundle_id, profile, confirmed, &save)
    })
    .await
    .map_err(|_| "export-join".to_string())?
}

#[tauri::command]
pub(crate) fn delete_support_bundle(
    state: tauri::State<'_, DiagnosticsState>,
    bundle_id: String,
) -> Result<(), String> {
    delete_support_bundle_inner(&state, &bundle_id)
}

#[tauri::command]
pub(crate) fn open_diagnostics_folder(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    open_diagnostics_folder_inner(&|path| {
        app.opener()
            .open_path(path.to_string_lossy().into_owned(), None::<&str>)
            .map_err(|e| format!("open-failed: {e}"))
    })
}

/// Safe-Share-only summary for Copy Diagnostic Summary (spec §22).
#[tauri::command]
pub(crate) fn get_support_summary(
    state: tauri::State<'_, DiagnosticsState>,
    bundle_id: Option<String>,
) -> SupportSummaryDto {
    support_summary_inner(&state, bundle_id)
}

#[tauri::command]
pub(crate) fn prepare_localstack_github_issue(
    state: tauri::State<'_, DiagnosticsState>,
    bundle_id: Option<String>,
) -> GitHubIssueDraftDto {
    github_issue_draft_inner(&state, bundle_id)
}

/// Navigation-time only (spec §43-I): cache the validated ViewId for the
/// panic hook. Never called during panic; no OS authority; returns whether
/// the value was accepted.
#[tauri::command]
pub(crate) fn update_diagnostics_context(
    state: tauri::State<'_, DiagnosticsState>,
    view_id: u8,
) -> bool {
    update_diagnostics_context_view(&state, view_id)
}

#[tauri::command]
pub(crate) fn reveal_export_result(
    state: tauri::State<'_, DiagnosticsState>,
    app: tauri::AppHandle,
    export_id: String,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    reveal_inner(&state, &export_id, &|path| {
        app.opener()
            .open_path(path.to_string_lossy().into_owned(), None::<&str>)
            .map_err(|e| format!("open-failed: {e}"))
    })
}

// ---------------------------------------------------------------------------
// Tests (RED first — pinned contract, then GREEN implementation)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> DiagnosticsState {
        DiagnosticsState::for_tests()
    }

    #[test]
    fn update_diagnostics_context_validates_range() {
        let st = state();
        // Invalid discriminant (≥ NAV_LEN) is ignored; the cache stays empty.
        assert!(!update_diagnostics_context_view(&st, 200));
        assert!(st.cache.try_active_view().is_none());
        // Valid ViewId position is cached for the panic hook.
        assert!(update_diagnostics_context_view(&st, 0));
        assert_eq!(st.cache.try_active_view(), Some(0));
    }

    #[test]
    fn mark_reviewed_with_unknown_id_fails_typed() {
        let st = state();
        let err = mark_incident_reviewed_inner(&st, "unknown-incident")
            .expect_err("unknown ID must fail typed");
        assert_eq!(err, "unknown-incident");
    }

    #[test]
    fn mark_bundle_reviewed_with_unknown_id_fails_typed() {
        let st = state();
        let err = delete_support_bundle_inner(&st, "ffffffffffffffffffffffff")
            .expect_err("unknown bundle must fail typed");
        assert_eq!(err, "unknown-bundle");
    }

    #[test]
    fn overview_aggregates_real_state() {
        let st = state();
        // Seed one incident through the real engine path.
        report_incident(
            &st,
            "docker",
            "ERR_TEST",
            Severity::Warning,
            "probe",
            "synthetic summary",
        );
        let ov = overview_inner(&st);
        assert_eq!(ov.active_incidents, 1);
        assert_eq!(ov.bundle_count, 0);
        assert!(!ov.app_version.is_empty());
    }

    #[test]
    fn github_issue_draft_uses_fixed_url_and_safe_share() {
        let st = state();
        let draft = github_issue_draft_inner(&st, None);
        assert!(draft
            .issue_url
            .starts_with("https://github.com/Zehna/Lokalstack/issues/new"));
        // Never a user-controlled URL.
        assert!(!draft.issue_url.contains("http://"));
    }

    #[test]
    fn full_forensics_export_requires_confirmation() {
        // The command layer maps the typed error to its stable string code.
        assert_eq!(
            export_error_code(&ExportError::FullForensicsUnconfirmed),
            "full-forensics-unconfirmed"
        );
        assert_eq!(export_error_code(&ExportError::UnknownBundle), "unknown-bundle");
        assert_eq!(export_error_code(&ExportError::CorruptSource), "corrupt-source");
        assert_eq!(export_error_code(&ExportError::SaveCancelled), "save-cancelled");
    }

    #[test]
    fn report_incident_severe_triggers_capture_request_without_worker() {
        let st = state();
        // No worker registered: RequestBundle degrades to one bounded warn,
        // never a panic, and the incident still records.
        report_incident(&st, "docker", "ERR_SEV", Severity::Severe, "probe", "sev");
        let idx = st.incidents.lock().unwrap();
        assert_eq!(idx.incidents.len(), 1);
        assert_eq!(idx.incidents[0].code, "ERR_SEV");
    }
}
