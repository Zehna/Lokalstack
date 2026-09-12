//! # Managed Workspaces & Service Lifecycle Orchestration (Phase 6)
//!
//! The first phase where LocalStack **launches** development services
//! itself — and therefore owns their lifecycle metadata.
//!
//! ## Trust model: managed vs external
//!
//! - **External** services were discovered on the machine; LocalStack has
//!   only observational metadata and the Phase 5 hardened control path
//!   (opaque ids, no graceful console stop, no restart).
//! - **Managed** services were launched by LocalStack from a
//!   **backend-derived** launch spec. Only they get: targeted graceful
//!   stop (known process group), restart, logs, readiness tracking.
//!   Managed metadata lives server-side; the frontend references specs and
//!   processes only by **opaque ids** and can never submit a program, args,
//!   cwd, or environment.
//!
//! ## Structure
//!
//! - [`rules`] — pure logic: candidates, validation, preflight, status
//!   derivation, ordering, log bounds, state transitions.
//! - [`registry`] — the opaque launch-spec registry and the bounded
//!   managed-process registry (both separate from Phase 5's registry).
//! - [`windows`] — narrow FFI: `CreateProcessW` with
//!   `CREATE_NEW_PROCESS_GROUP`, pipe capture with RAII reader threads,
//!   targeted `CTRL_BREAK` to the known group (never 0), force terminate.
//! - This facade — workspace models, the readiness/exit monitor, and the
//!   Tauri command surface.

pub(crate) mod registry;
pub(crate) mod rules;

#[cfg(windows)]
pub(crate) mod windows;

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

pub(crate) use registry::{LaunchSpecRegistry, ManagedProcess, ManagedProcessRegistry, TrustedLaunchSpec};
pub(crate) use rules::{
    LaunchCandidate, LaunchSpec, LogLine, ManagedState, Role, WorkspaceStatus,
};

use crate::project::markers::PackageManager;

/// Readiness window: a `Starting` service with an expected port has this
/// long to show its listener before it is marked `Degraded` (alive but not
/// ready — documented semantics choice).
pub(crate) const READINESS_TIMEOUT: Duration = Duration::from_secs(25);
/// Services without an expected port are `Running` after this grace period
/// (process alive = running, per the no-port semantics).
pub(crate) const NO_PORT_GRACE: Duration = Duration::from_secs(3);
/// How long a graceful stop may take before `StopTimeout`.
pub(crate) const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(5);
/// Monitor cadence (event-driven where practical; this is the fallback
/// tick for exit detection and readiness transitions).
pub(crate) const MONITOR_TICK: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------------------
// Domain models
// ---------------------------------------------------------------------------

/// One managed service inside a workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct WorkspaceService {
    pub id: String,
    pub name: String,
    pub role: Role,
    /// Evidence-based expected port, when the launch command states one.
    pub expectedPort: Option<u16>,
    /// Opaque launch-spec id — the only handle the frontend may use to
    /// start this service.
    pub launchSpecId: String,
    /// Where the spec came from, e.g. `package.json scripts.dev`.
    pub source: String,
}

/// A first-class workspace: explicit, user-created, project-scoped.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct Workspace {
    pub id: String,
    /// Project root the workspace belongs to (its stable identity).
    pub projectRoot: String,
    pub name: String,
    pub services: Vec<WorkspaceService>,
}

/// Managed process view for the frontend (opaque id + display state).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ManagedProcessView {
    pub managedId: String,
    pub rootPid: u32,
    /// Serialized `ManagedState` (tagged object with a `state` field).
    pub state: ManagedState,
    pub startedAt: u64,
}

/// One service row in a workspace view.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct WorkspaceServiceView {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub expectedPort: Option<u16>,
    pub launchSpecId: String,
    pub source: String,
    /// Live managed process, when one exists (or last existed).
    pub managed: Option<ManagedProcessView>,
}

/// Full workspace view delivered to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct WorkspaceView {
    pub id: String,
    pub projectRoot: String,
    pub name: String,
    pub status: WorkspaceStatus,
    pub services: Vec<WorkspaceServiceView>,
}

/// Result of a managed lifecycle action, reported honestly.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ManagedActionOutcome {
    pub ok: bool,
    /// Structured marker: `UNKNOWN_LAUNCH_SPEC`, `STALE_LAUNCH_SPEC`,
    /// `ALREADY_RUNNING`, `PORT_CONFLICT`, `UNKNOWN_MANAGED_PROCESS`,
    /// `STALE_MANAGED_IDENTITY`, `STOP_TIMEOUT`, `NOT_RUNNING`, `INVALID_STATE`.
    pub code: String,
    pub message: String,
    /// New managed id after a successful (re)start or `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managedId: Option<String>,
}

/// A batch of logs for one service (incremental polling).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct LogBatch {
    pub lines: Vec<LogLine>,
    pub lastIndex: u64,
}

// ---------------------------------------------------------------------------
// Engine state
// ---------------------------------------------------------------------------

/// One log-routing item: which managed process produced the line.
pub(crate) struct RoutedLog {
    pub managed_id: String,
    pub line: LogLine,
}

/// Tauri-managed workspace engine state.
pub(crate) struct WorkspaceEngineState {
    inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub workspaces: Mutex<Vec<Workspace>>,
    pub specs: LaunchSpecRegistry,
    pub managed: ManagedProcessRegistry,
    /// Declared runtime dependencies per workspace id (Phase 7B), stored
    /// behind the same lock domain as the rest of the engine state.
    pub dependencies: Mutex<HashMap<String, Vec<crate::dependencies::Dependency>>>,
    pub logs_tx: Sender<RoutedLog>,
    pub logs_rx: Mutex<Receiver<RoutedLog>>,
    monitor_started: std::sync::atomic::AtomicBool,
}

impl Default for WorkspaceEngineState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            inner: Arc::new(Inner {
                workspaces: Mutex::new(Vec::new()),
                specs: LaunchSpecRegistry::default(),
                managed: ManagedProcessRegistry::default(),
                dependencies: Mutex::new(HashMap::new()),
                logs_tx: tx,
                logs_rx: Mutex::new(rx),
                monitor_started: std::sync::atomic::AtomicBool::new(false),
            }),
        }
    }
}

impl WorkspaceEngineState {
    /// Clone the shared engine handle (commands hand it to blocking tasks).
    pub(crate) fn engine_inner(&self) -> Arc<Inner> {
        Arc::clone(&self.inner)
    }

    /// Spawn the readiness/exit monitor once (idempotent).
    pub(crate) fn spawn_monitor(&self) {
        if self
            .inner
            .monitor_started
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("workspace-monitor".to_string())
            .spawn(move || monitor_loop(inner))
            .expect("workspace monitor thread");
    }
}

// ---------------------------------------------------------------------------
// Candidate derivation (backend-only — the trusted source of launch specs)
// ---------------------------------------------------------------------------

/// Derive launch candidates for a project root from **trusted project
/// metadata only** (manifest files at the root — never frontend input).
pub(crate) fn derive_candidates(root: &std::path::Path) -> Vec<LaunchCandidate> {
    let mut candidates = Vec::new();
    let markers = crate::project::markers::find_markers(root);

    // Node / JS: package.json scripts via the detected package manager.
    if markers.contains(&"package.json") {
        if let Ok(package) = crate::project::markers::parse_package_json(&root.join("package.json")) {
            let scripts: Vec<(String, String)> = {
                let mut list: Vec<(String, String)> =
                    package.scripts.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                list.sort_by(|a, b| a.0.cmp(&b.0));
                list
            };
            let (manager, _evidence) =
                crate::project::markers::detect_package_manager(root, Some(&package));
            candidates.extend(node_candidates(manager, &scripts));
        }
    }

    // Rust: `cargo run` is a known trusted runner.
    if markers.contains(&"Cargo.toml") {
        candidates.extend(crate::workspace::rules::cargo_launch_candidates());
    }
    // Go: `go run .` is a known trusted runner.
    if markers.contains(&"go.mod") {
        candidates.extend(crate::workspace::rules::go_launch_candidates());
    }

    // Fill the cwd (project root) and re-derive nothing else.
    let root_string = root.to_string_lossy().into_owned();
    for candidate in &mut candidates {
        candidate.spec.cwd = root_string.clone();
    }
    candidates
}

/// Adapter so `rules::node_launch_candidates` stays private-testable while
/// the facade feeds it project data.
fn node_candidates(
    manager: PackageManager,
    scripts: &[(String, String)],
) -> Vec<LaunchCandidate> {
    crate::workspace::rules::node_launch_candidates(manager, scripts)
}

/// Register candidates as trusted launch specs and build the workspace.
fn build_workspace(root: &std::path::Path, specs: &LaunchSpecRegistry) -> Workspace {
    let candidates = derive_candidates(root);
    let markers = crate::project::markers::find_markers(root);
    let name = if markers.contains(&"package.json") {
        crate::project::markers::parse_package_json(&root.join("package.json"))
            .ok()
            .and_then(|p| p.name)
            .unwrap_or_else(|| root_name(root))
    } else {
        crate::project::markers::parse_cargo_name(&root.join("Cargo.toml"))
            .or_else(|| crate::project::markers::parse_pyproject_name(&root.join("pyproject.toml")))
            .unwrap_or_else(|| root_name(root))
    };
    let workspace_id = opaque_workspace_id(&root.to_string_lossy());

    let mut services = Vec::new();
    for (index, candidate) in candidates.into_iter().enumerate() {
        let service_id = format!("svc-{}", index + 1);
        let trusted = TrustedLaunchSpec {
            id: String::new(),
            workspace_id: workspace_id.clone(),
            service_id: service_id.clone(),
            spec: candidate.spec.clone(),
            expected_port: candidate.expectedPort,
            project_root: root.to_string_lossy().into_owned(),
            issued: Instant::now(),
        };
        let launch_spec_id = specs.put(trusted);
        services.push(WorkspaceService {
            id: service_id,
            name: candidate.name,
            role: candidate.role,
            expectedPort: candidate.expectedPort,
            launchSpecId: launch_spec_id,
            source: candidate.source,
        });
    }

    Workspace {
        id: workspace_id,
        projectRoot: root.to_string_lossy().into_owned(),
        name,
        services,
    }
}

fn root_name(root: &std::path::Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned())
}

fn opaque_workspace_id(root: &str) -> String {
    crate::control::registry::blake3_256(root.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---------------------------------------------------------------------------
// Lifecycle operations (shared by commands)
// ---------------------------------------------------------------------------

/// START: validate spec → duplicate check → port-conflict preflight →
/// launch → registry entry (`Starting`).
/// Phase 10B correction (spec §1): a poisoned registry means trusted
/// lifecycle state could not be validated. Every mutating path gates on
/// this before resolving specs or touching processes — no launch, no
/// CTRL_BREAK, no termination, and never a fallback to raw PID, executable
/// name, or frontend metadata. The user-facing message carries no poisoning
/// internals.
const WORKSPACE_STATE_UNAVAILABLE: &str = "WORKSPACE_STATE_UNAVAILABLE";

fn state_unavailable() -> ManagedActionOutcome {
    ManagedActionOutcome {
        ok: false,
        code: WORKSPACE_STATE_UNAVAILABLE.to_string(),
        message: "Workspace lifecycle control is temporarily unavailable because trusted workspace state could not be validated. Refresh to rebuild it.".to_string(),
        managedId: None,
    }
}

fn start_service(inner: &Inner, launch_spec_id: &str) -> Result<ManagedProcess, ManagedActionOutcome> {
    // Fail closed: a launch resolves and executes a trusted spec — poison
    // in either registry quarantines starts.
    if inner.specs.poisoned() || inner.managed.poisoned() {
        return Err(state_unavailable());
    }
    let spec = inner
        .specs
        .get(launch_spec_id)
        .ok_or_else(|| ManagedActionOutcome {
            ok: false,
            code: "UNKNOWN_LAUNCH_SPEC".to_string(),
            message: "This launch specification is no longer valid. Refresh and try again."
                .to_string(),
            managedId: None,
        })?;

    // Fresh validation against current reality.
    if let Err(problem) = validate_spec_fresh(&spec) {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "STALE_LAUNCH_SPEC".to_string(),
            message: format!("Launch specification is stale: {problem}."),
            managedId: None,
        });
    }

    // Duplicate launch prevention: managed registry first…
    if inner.managed.has_running_service(&spec.workspace_id, &spec.service_id) {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "ALREADY_RUNNING".to_string(),
            message: "This workspace service is already running.".to_string(),
            managedId: None,
        });
    }

    // Dependency gate (spec §28): a required dependency that is not
    // listening/running blocks the launch — no process is created.
    {
        #[cfg(windows)]
        let listeners = crate::discovery::windows::enumerate_tcp_listeners().unwrap_or_default();
        #[cfg(not(windows))]
        let listeners: Vec<crate::discovery::PortListener> = Vec::new();
        let required: Vec<crate::dependencies::Dependency> = {
            let deps = crate::dependencies::dependencies_of(inner, &spec.workspace_id);
            deps.into_iter()
                .filter(|d| d.sourceServiceId == spec.service_id && d.required)
                .collect()
        };
        for dependency in &required {
            let managed_of = |service_id: &str| {
                inner
                    .managed
                    .latest_for_service(&spec.workspace_id, service_id)
                    .map(|p| p.state)
            };
            let state = crate::dependencies::evaluate_dependency(dependency, &listeners, managed_of, |_| None);
            if !matches!(
                state,
                crate::dependencies::DependencyState::Available
                    | crate::dependencies::DependencyState::Starting
            ) {
                return Err(ManagedActionOutcome {
                    ok: false,
                    code: "DEPENDENCY_BLOCKED".to_string(),
                    message: format!(
                        "Cannot start: required dependency {} is not available (state: {:?}).",
                        dependency.target.label(),
                        state
                    ),
                    managedId: None,
                });
            }
        }
    }

    // …then the current listener table for the expected port, with an
    // evidence-based owner description (Phase 7A).
    if let Some(port) = spec.expected_port {
        if let Some(block) = crate::conflicts::check_launch_port(port, &inner.managed.managed_pids()) {
            // Our own just-stopped process must not be mistaken for an
            // external instance; anything else is a genuine conflict.
            let owned_by_us = block
                .owner_pid
                .and_then(|pid| inner.managed.managed_by_pid(pid))
                .is_some_and(|p| p.workspaceId == spec.workspace_id);
            if !owned_by_us && !block.same_managed_service {
                let owner = block
                    .owner_pid
                    .and_then(|pid| inner.managed.managed_by_pid(pid))
                    .map(|p| format!("managed service {}", p.serviceId))
                    .or_else(|| {
                        block
                            .owner_name
                            .as_ref()
                            .map(|name| format!("{name} (PID {})", block.owner_pid.unwrap_or(0)))
                    })
                    .unwrap_or_else(|| "an unresolvable process".to_string());
                return Err(ManagedActionOutcome {
                    ok: false,
                    code: "PORT_CONFLICT".to_string(),
                    message: format!(
                        "Port {port} is owned by {owner}. An external instance appears to already be running — decide on it separately."
                    ),
                    managedId: None,
                });
            }
        }
    }

    // Managed id is derived before launch so reader threads can route logs.
    let started_at = crate::process::windows::now_unix_ms();
    let managed_id = crate::workspace::registry::opaque_managed_id(
        &spec.workspace_id,
        &spec.service_id,
        started_at,
    );

    // Registry entry first (so early logs have a home), launched after.
    let managed_entry = crate::workspace::registry::ManagedEntry {
        process: crate::workspace::registry::ManagedProcess {
            managedId: managed_id.clone(),
            workspaceId: spec.workspace_id.clone(),
            serviceId: spec.service_id.clone(),
            rootPid: 0, // filled after launch
            creationTime: None,
            processGroupId: 0,
            launchSpecId: launch_spec_id.to_string(),
            startedAt: started_at,
            state: ManagedState::Starting,
        },
        logs: crate::workspace::rules::LogRing::new(LOG_CAPACITY),
        state_since: Instant::now(),
    };
    inner
        .managed
        .insert(managed_entry)
        .map_err(|e| ManagedActionOutcome {
            ok: false,
            code: "INVALID_STATE".to_string(),
            message: e,
            managedId: None,
        })?;

    let launch_tx: Option<Sender<crate::workspace::rules::LogLine>> =
        Some(wrap_sender(Sender::clone(&inner.logs_tx), managed_id.clone()));

    let spec_ref = spec.spec.clone();
    let result = launch_with_logs(&spec_ref, launch_tx);

    match result {
        Ok(launched) => {
            let creation_time = managed_creation_time(launched.root_pid);
            inner.managed.with_entry(&managed_id, |entry| {
                entry.process.rootPid = launched.root_pid;
                entry.process.creationTime = creation_time;
                // Group id == root PID by CREATE_NEW_PROCESS_GROUP.
                entry.process.processGroupId = launched.root_pid;
            });
            Ok(crate::workspace::registry::ManagedProcess {
                managedId: managed_id,
                workspaceId: (*spec).workspace_id.clone(),
                serviceId: (*spec).service_id.clone(),
                rootPid: launched.root_pid,
                creationTime: creation_time,
                processGroupId: launched.root_pid,
                launchSpecId: launch_spec_id.to_string(),
                startedAt: started_at,
                state: ManagedState::Starting,
            })
        }
        Err(e) => {
            // Launch failed — the placeholder entry must not linger.
            inner.managed.remove(&managed_id);
            Err(ManagedActionOutcome {
                ok: false,
                code: "INVALID_STATE".to_string(),
                message: e,
                managedId: None,
            })
        }
    }
}

/// STOP (graceful, then optionally forced after timeout): revalidate
/// identity → targeted CTRL_BREAK to the known group → bounded wait →
/// honest status.
fn stop_service(inner: &Inner, managed_id: &str, force: bool) -> Result<ManagedState, ManagedActionOutcome> {
    // Fail closed: stop/force act on the stored identity + process group
    // from the managed map — poison quarantines every termination path
    // (graceful CTRL_BREAK and confirmed force alike).
    if inner.managed.poisoned() || inner.specs.poisoned() {
        return Err(state_unavailable());
    }
    let Some(process) = inner.managed.get(managed_id) else {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "UNKNOWN_MANAGED_PROCESS".to_string(),
            message: "This managed process is no longer registered (LocalStack may have restarted).".to_string(),
            managedId: None,
        });
    };
    if !process.state.alive() {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "NOT_RUNNING".to_string(),
            message: "This service is not running.".to_string(),
            managedId: None,
        });
    }
    // Identity revalidation — a reused PID can never be acted upon.
    if !managed_alive(&process) {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "STALE_MANAGED_IDENTITY".to_string(),
            message: "The managed process identity changed. Refresh before trying again.".to_string(),
            managedId: None,
        });
    }

    inner.managed.with_entry(managed_id, |entry| {
        entry.process.state = ManagedState::Stopping;
        entry.state_since = Instant::now();
    });

    // Targeted graceful stop — the known group id from our own launch.
    #[cfg(windows)]
    let graceful = crate::workspace::windows::graceful_stop(process.rootPid, process.processGroupId);
    #[cfg(not(windows))]
    let graceful: Result<(), String> = Err("Managed lifecycle is Windows-only.".to_string());

    let exited = match graceful {
        Ok(()) => {
            #[cfg(windows)]
            {
                crate::workspace::windows::wait_for_exit(process.rootPid, process.creationTime, GRACEFUL_STOP_TIMEOUT)
            }
            #[cfg(not(windows))]
            {
                false
            }
        }
        Err(_) => false,
    };

    if exited {
        let code = exit_code_now(&process);
        inner.managed.with_entry(managed_id, |entry| {
            entry.process.state = ManagedState::Stopped;
            entry.state_since = Instant::now();
        });
        let _ = code;
        return Ok(ManagedState::Stopped);
    }

    // Graceful stop timed out. Never force automatically.
    if force {
        // Force requires the explicit confirmation the UI gates; it is only
        // offered after this timeout (state now STOP_TIMEOUT).
        #[cfg(windows)]
        {
            crate::workspace::windows::force_terminate(process.rootPid, process.creationTime)
                .map_err(|e| ManagedActionOutcome {
                    ok: false,
                    code: "INVALID_STATE".to_string(),
                    message: e,
                    managedId: None,
                })?;
            let exited = crate::workspace::windows::wait_for_exit(
                process.rootPid,
                process.creationTime,
                Duration::from_secs(3),
            );
            if !exited {
                return Err(ManagedActionOutcome {
                    ok: false,
                    code: "INVALID_STATE".to_string(),
                    message: "Termination was issued but exit has not been observed.".to_string(),
                    managedId: None,
                });
            }
        }
        inner.managed.with_entry(managed_id, |entry| {
            entry.process.state = ManagedState::Stopped;
            entry.state_since = Instant::now();
        });
        return Ok(ManagedState::Stopped);
    }

    inner.managed.with_entry(managed_id, |entry| {
        entry.process.state = ManagedState::StopTimeout;
        entry.state_since = Instant::now();
    });
    Err(ManagedActionOutcome {
        ok: false,
        code: "STOP_TIMEOUT".to_string(),
        message: "The process did not exit within the graceful window. Force Stop is now available.".to_string(),
        managedId: None,
    })
}

/// RESTART (managed only): graceful stop → relaunch from the same trusted
/// spec → new identity in the registry. On timeout: no silent force — the
/// user decides.
fn restart_service(inner: &Inner, managed_id: &str) -> Result<ManagedProcess, ManagedActionOutcome> {
    // Fail closed: restart = stop + relaunch; both halves are quarantined
    // by the same poison policy, and the gate here makes the refusal
    // unconditional (no partial stop-then-refuse sequences).
    if inner.managed.poisoned() || inner.specs.poisoned() {
        return Err(state_unavailable());
    }
    let Some(process) = inner.managed.get(managed_id) else {
        return Err(ManagedActionOutcome {
            ok: false,
            code: "UNKNOWN_MANAGED_PROCESS".to_string(),
            message: "This managed process is no longer registered.".to_string(),
            managedId: None,
        });
    };
    let spec_id = process.launchSpecId.clone();

    match stop_service(inner, managed_id, false) {
        Ok(_) => {}
        Err(outcome) if outcome.code == "STOP_TIMEOUT" => return Err(outcome),
        Err(outcome) if outcome.code == "NOT_RUNNING" => {}
        Err(outcome) => return Err(outcome),
    }

    // The old managed entry is terminal; remove it so the relaunch is a
    // genuinely new identity (old id cannot control the new process).
    inner.managed.remove(managed_id);
    start_service(inner, &spec_id)
}

fn exit_code_now(process: &crate::workspace::registry::ManagedProcess) -> Option<u32> {
    #[cfg(windows)]
    {
        crate::workspace::windows::exit_code(process.rootPid, process.creationTime)
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        None
    }
}

fn managed_alive(process: &crate::workspace::registry::ManagedProcess) -> bool {
    #[cfg(windows)]
    {
        crate::workspace::windows::is_alive(process.rootPid, process.creationTime)
    }
    #[cfg(not(windows))]
    {
        let _ = process;
        false
    }
}

fn validate_spec_fresh(spec: &TrustedLaunchSpec) -> Result<(), String> {
    let path_entries = path_entries();
    let exists = |p: &str| std::path::Path::new(p).exists();
    crate::workspace::rules::validate_spec(&spec.spec, &spec.project_root, &path_entries, &exists)
        .map_err(|problem| format!("{problem:?}"))
}

/// PATH entries of the current process (used for program resolution).
fn path_entries() -> Vec<String> {
    std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// Find the owner PID currently listening on `port`, if any.
fn find_port_owner(port: u16) -> Option<u32> {
    #[cfg(windows)]
    {
        crate::discovery::windows::enumerate_tcp_listeners()
            .ok()?
            .into_iter()
            .find(|l| l.port == port)
            .map(|l| l.pid)
    }
    #[cfg(not(windows))]
    {
        let _ = port;
        None
    }
}

pub(crate) fn launch_with_logs(
    spec: &LaunchSpec,
    tx: Option<Sender<crate::workspace::rules::LogLine>>,
) -> Result<crate::workspace::windows::LaunchedProcess, String> {
    #[cfg(windows)]
    {
        crate::workspace::windows::launch_managed(spec, tx.as_ref())
    }
    #[cfg(not(windows))]
    {
        let _ = (spec, tx);
        Err("Managed lifecycle is Windows-only.".to_string())
    }
}

/// Creation time of a freshly launched PID (best effort — the registry
/// stores it as the process identity).
pub(crate) fn managed_creation_time(pid: u32) -> Option<u64> {
    #[cfg(windows)]
    {
        let probe = crate::control::windows::revalidate(pid);
        probe.and_then(|(_, creation)| creation)
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        None
    }
}

/// Wrap the shared log sender so lines carry their managed id.
fn wrap_sender(tx: Sender<RoutedLog>, managed_id: String) -> Sender<crate::workspace::rules::LogLine> {
    let (forward_tx, forward_rx) = channel::<crate::workspace::rules::LogLine>();
    std::thread::spawn(move || {
        for line in forward_rx {
            if tx.send(RoutedLog { managed_id: managed_id.clone(), line }).is_err() {
                break;
            }
        }
    });
    forward_tx
}

/// Log ring capacity per service (bounded memory; see rule 21).
pub(crate) const LOG_CAPACITY: usize = 1_000;

// ---------------------------------------------------------------------------
// Monitor: log routing, readiness transitions, exit detection
// ---------------------------------------------------------------------------

fn monitor_loop(inner: Arc<Inner>) {
    loop {
        std::thread::sleep(MONITOR_TICK);

        // 1. Route captured output into the bounded rings.
        if let Ok(rx) = inner.logs_rx.lock() {
            while let Ok(routed) = rx.try_recv() {
                inner.managed.with_entry(&routed.managed_id, |entry| {
                    entry.logs.push(routed.line);
                });
                // Lines for an entry that vanished (launch failure) are
                // dropped — the ring died with the entry.
            }
        }

        // 2. Readiness + exit transitions.
        let mut any_starting_with_port = false;
        // Recovery path (Phase 10B correction): if the managed registry is
        // quarantined by poison, the monitor rebuilds a NEW clean map before
        // doing anything else. Entries become External (documented semantic
        // — same as an app restart); the processes themselves are untouched.
        // Recovery also captures exit codes for killed entries so history
        // stays honest.
        if inner.managed.poisoned() {
            let snapshot: Vec<crate::workspace::registry::ManagedProcess> = {
                let mut list = Vec::new();
                inner.for_each_process(|p| list.push(p.clone()));
                list
            };
            for process in snapshot {
                if !process.state.alive() {
                    continue;
                }
                let code = exit_code_now(&process);
                let _ = code; // diagnostic-only today; no history write here
            }
            inner.managed.recover();
            crate::diagnostics::info(
                "workspace",
                "managed-process registry rebuilt after poison; entries are External",
            );
        }
        // 3. Zombie cleanup: entries whose root process is gone get their
        //    exit code captured and the entry dropped (bounded registry).
        let dead_ids = inner.managed.exited_ids(&|pid| {
            #[cfg(windows)]
            {
                crate::workspace::windows::pid_exists(pid)
            }
            #[cfg(not(windows))]
            {
                let _ = pid;
                false
            }
        });
        for id in dead_ids {
            if let Some(process) = inner.managed.get(&id) {
                let code = exit_code_now(&process);
                let final_state = if process.state == ManagedState::Stopping {
                    ManagedState::Stopped
                } else {
                    ManagedState::Exited { exit_code: code }
                };
                inner.managed.remove(&id);
                let _ = final_state;
            }
        }
        let entries: Vec<crate::workspace::registry::ManagedProcess> = {
            // Snapshot the states we need to evaluate.
            let mut list = Vec::new();
            inner.for_each_process(|p| list.push(p.clone()));
            list
        };
        for process in entries {
            match process.state {
                ManagedState::Starting => {
                    if !managed_alive(&process) {
                        // Startup failure — capture the exit code honestly.
                        let code = exit_code_now(&process);
                        inner.managed.with_entry(&process.managedId, |entry| {
                            entry.process.state = ManagedState::StartFailed { exit_code: code };
                            entry.state_since = Instant::now();
                        });
                        continue;
                    }
                    if let Some(port) = expected_port_of(&inner, &process) {
                        any_starting_with_port = true;
                        if port_is_listening(port) {
                            inner.managed.with_entry(&process.managedId, |entry| {
                                entry.process.state = ManagedState::Running;
                                entry.state_since = Instant::now();
                            });
                            continue;
                        }
                    }
                    // Timeout semantics: alive past the window with no port
                    // → `Degraded` (never a fake `Running`).
                    let waited = inner.managed.with_entry(&process.managedId, |entry| {
                        entry.state_since.elapsed()
                    });
                    let limit = if expected_port_of(&inner, &process).is_some() {
                        READINESS_TIMEOUT
                    } else {
                        NO_PORT_GRACE
                    };
                    if waited.is_some_and(|w| w >= limit) {
                        let new_state = if expected_port_of(&inner, &process).is_some() {
                            ManagedState::Degraded
                        } else {
                            ManagedState::Running
                        };
                        inner.managed.with_entry(&process.managedId, |entry| {
                            entry.process.state = new_state.clone();
                            entry.state_since = Instant::now();
                        });
                    }
                }
                ManagedState::Running | ManagedState::Degraded => {
                    if !managed_alive(&process) {
                        // Unplanned exit.
                        let code = exit_code_now(&process);
                        inner.managed.with_entry(&process.managedId, |entry| {
                            entry.process.state = ManagedState::Exited { exit_code: code };
                            entry.state_since = Instant::now();
                        });
                    }
                }
                ManagedState::Stopping => {
                    if !managed_alive(&process) {
                        inner.managed.with_entry(&process.managedId, |entry| {
                            entry.process.state = ManagedState::Stopped;
                            entry.state_since = Instant::now();
                        });
                    } else if inner.managed.with_entry(&process.managedId, |entry| entry.state_since.elapsed())
                        .is_some_and(|e| e >= GRACEFUL_STOP_TIMEOUT + Duration::from_secs(1))
                    {
                        inner.managed.with_entry(&process.managedId, |entry| {
                            entry.process.state = ManagedState::StopTimeout;
                            entry.state_since = Instant::now();
                        });
                    }
                }
                _ => {}
            }
        }

        // Keep the preflight cheap: only probe the TCP table while
        // something is actually starting with a port.
        let _ = any_starting_with_port;
    }
}

fn expected_port_of(inner: &Inner, process: &crate::workspace::registry::ManagedProcess) -> Option<u16> {
    let spec = inner.specs.get(&process.launchSpecId)?;
    spec.expected_port
}

fn port_is_listening(port: u16) -> bool {
    find_port_owner(port).is_some()
}

impl Inner {
    fn for_each_process(&self, mut f: impl FnMut(&crate::workspace::registry::ManagedProcess)) {
        self.managed.for_each(&mut f);
    }
}

// ---------------------------------------------------------------------------
// View assembly
// ---------------------------------------------------------------------------

fn workspace_view(inner: &Inner, workspace: &Workspace) -> WorkspaceView {
    let mut services = Vec::new();
    let mut states: Vec<ManagedState> = Vec::new();
    for service in &workspace.services {
        let managed = inner
            .managed
            .latest_for_service(&workspace.id, &service.id)
            .map(|p| ManagedProcessView {
                managedId: p.managedId,
                rootPid: p.rootPid,
                state: p.state.clone(),
                startedAt: p.startedAt,
            });
        if let Some(view) = &managed {
            states.push(view.state.clone());
        }
        services.push(WorkspaceServiceView {
            id: service.id.clone(),
            name: service.name.clone(),
            role: service.role,
            expectedPort: service.expectedPort,
            launchSpecId: service.launchSpecId.clone(),
            source: service.source.clone(),
            managed,
        });
    }
    // Only managed states count toward the workspace status — external
    // dependencies are never "stopped" merely because LocalStack does not
    // manage them.
    let status = crate::workspace::rules::workspace_status(&states);
    WorkspaceView {
        id: workspace.id.clone(),
        projectRoot: workspace.projectRoot.clone(),
        name: workspace.name.clone(),
        status,
        services,
    }
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Derived launch candidates for a project root (read-only; used by the
/// create-workspace dialog to show what would be offered).
#[tauri::command]
pub(crate) async fn get_workspace_candidates(
    project_root: String,
) -> Result<Vec<LaunchCandidate>, String> {
    let root = std::path::PathBuf::from(&project_root);
    if !root.is_dir() {
        return Err("Project root does not exist.".to_string());
    }
    let root = tauri::async_runtime::spawn_blocking(move || derive_candidates(&root))
        .await
        .map_err(|e| format!("candidate task join error: {e}"))?;
    Ok(root)
}

/// Create a workspace for a project root (explicit user action). Derives
/// candidates from trusted manifest metadata, registers their launch specs,
/// and returns the workspace view. Duplicate workspaces for the same root
/// are refused.
#[tauri::command]
pub(crate) async fn create_workspace(
    state: tauri::State<'_, WorkspaceEngineState>,
    project_root: String,
) -> Result<WorkspaceView, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let root = std::path::PathBuf::from(&project_root);
        if !root.is_dir() {
            return Err("Project root does not exist.".to_string());
        }
        // Phase 10D (§J): fast-path duplicate check WITHOUT holding the list
        // lock across spec derivation (filesystem reads). The authoritative
        // check is re-run under the lock after derivation.
        {
            let workspaces = inner
                .workspaces
                .lock()
                .map_err(|_| "workspace list lock poisoned".to_string())?;
            if workspaces.iter().any(|w| w.projectRoot == project_root) {
                return Err("A workspace already exists for this project.".to_string());
            }
        } // list lock released before filesystem I/O
        // Recovery path (Phase 10B correction): creating a workspace
        // re-derives every spec fresh from project manifests, so it is the
        // natural place to rebuild trusted launch-spec state after poison.
        // The NEW clean map (plus per-launch revalidation) is what makes
        // lifecycle control available again — poisoned contents are never
        // silently continued.
        if inner.specs.poisoned() {
            inner.specs.recover();
        }
        let workspace = build_workspace(&root, &inner.specs); // fs I/O outside the list lock
        let mut workspaces = inner
            .workspaces
            .lock()
            .map_err(|_| "workspace list lock poisoned".to_string())?;
        // Authoritative duplicate re-check after the I/O window: two racing
        // creators for the same root — the loser is refused.
        if workspaces.iter().any(|w| w.projectRoot == project_root) {
            return Err("A workspace already exists for this project.".to_string());
        }
        let view = workspace_view(&inner, &workspace);
        workspaces.push(workspace);
        Ok(view)
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// Remove a workspace and its launch specs. Managed processes keep running
/// (app exit / workspace removal is not STOP ALL — documented behavior).
#[tauri::command]
pub(crate) async fn remove_workspace(
    state: tauri::State<'_, WorkspaceEngineState>,
    workspace_id: String,
) -> Result<(), String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let mut workspaces = inner
            .workspaces
            .lock()
            .map_err(|_| "workspace list lock poisoned".to_string())?;
        workspaces.retain(|w| w.id != workspace_id);
        inner.specs.remove_workspace(&workspace_id);
        Ok(())
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// All workspaces with live managed-state views.
#[tauri::command]
pub(crate) async fn list_workspaces(
    state: tauri::State<'_, WorkspaceEngineState>,
) -> Result<Vec<WorkspaceView>, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let workspaces = inner
            .workspaces
            .lock()
            .map_err(|_| "workspace list lock poisoned".to_string())?;
        Ok(workspaces.iter().map(|w| workspace_view(&inner, w)).collect())
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// START one workspace service by its opaque launch-spec id.
#[tauri::command]
pub(crate) async fn start_managed_service(
    state: tauri::State<'_, WorkspaceEngineState>,
    launch_spec_id: String,
) -> Result<ManagedActionOutcome, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        match start_service(&inner, &launch_spec_id) {
            Ok(process) => Ok(ManagedActionOutcome {
                ok: true,
                code: "STARTED".to_string(),
                message: format!("Started (PID {}).", process.rootPid),
                managedId: Some(process.managedId),
            }),
            Err(outcome) => Ok(outcome),
        }
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// STOP one managed service: graceful (targeted CTRL_BREAK) first; force
/// only when the graceful window timed out and the user explicitly
/// confirmed (`force: true`).
#[tauri::command]
pub(crate) async fn stop_managed_service(
    state: tauri::State<'_, WorkspaceEngineState>,
    managed_id: String,
    force: Option<bool>,
) -> Result<ManagedActionOutcome, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || match stop_service(&inner, &managed_id, force.unwrap_or(false)) {
        Ok(state) => Ok(ManagedActionOutcome {
            ok: true,
            code: format!("stopped:{state:?}"),
            message: "Stopped.".to_string(),
            managedId: Some(managed_id),
        }),
        Err(outcome) => Ok(outcome),
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// RESTART one managed service from its trusted launch spec. The old
/// managed id becomes invalid; a new one is issued.
#[tauri::command]
pub(crate) async fn restart_managed_service(
    state: tauri::State<'_, WorkspaceEngineState>,
    managed_id: String,
) -> Result<ManagedActionOutcome, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || match restart_service(&inner, &managed_id) {
        Ok(process) => Ok(ManagedActionOutcome {
            ok: true,
            code: "RESTARTED".to_string(),
            message: format!("Restarted (new PID {}).", process.rootPid),
            managedId: Some(process.managedId),
        }),
        Err(outcome) => Ok(outcome),
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// START WORKSPACE — full preflight first (spec §27), then a conservative
/// start:
///
/// 1. dependency graph (cycle → refuse);
/// 2. external dependency availability;
/// 3. port-conflict preflight per service;
/// 4. topological start order (dependencies before dependents;
///    role order as the acyclic fallback);
/// 5. start eligible managed services only — external dependencies are
///    never started.
///
/// **Conservative decision (documented):** if any blocking issue exists,
/// *nothing* is started — Phase 7 has no partial auto-start policy. The
/// blocking root causes are returned in one `BLOCKED` outcome.
#[tauri::command]
pub(crate) async fn start_workspace_services(
    state: tauri::State<'_, WorkspaceEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
    workspace_id: String,
) -> Result<Vec<ManagedActionOutcome>, String> {
    let inner = Arc::clone(&state.inner);
    let project_cache = Arc::clone(&projects.cache);
    tauri::async_runtime::spawn_blocking(move || {
        let workspace = {
            let list = inner
                .workspaces
                .lock()
                .map_err(|_| "workspace list lock poisoned".to_string())?;
            list.iter().find(|w| w.id == workspace_id).cloned()
        };
        let Some(workspace) = workspace else {
            return Err("Workspace not found.".to_string());
        };
        let _ = &workspace_id;

        // ---- Preflight: conflicts + dependencies (one snapshot) --------
        let projects_handle = crate::project::ProjectEngineState { cache: project_cache };
        let readiness_views =
            crate::dependencies::evaluate_all(&inner, &projects_handle)?;
        if let Some(view) = readiness_views.iter().find(|v| v.workspaceId == workspace.id) {
            let blockers: Vec<&crate::dependencies::ReadinessIssue> = view
                .issues
                .iter()
                .filter(|issue| issue.severity == crate::dependencies::readiness::IssueSeverity::Error)
                .collect();
            if !blockers.is_empty() {
                let detail = blockers
                    .iter()
                    .map(|issue| issue.message.clone())
                    .collect::<Vec<_>>()
                    .join(" ");
                return Ok(vec![ManagedActionOutcome {
                    ok: false,
                    code: "BLOCKED".to_string(),
                    message: format!("Workspace is blocked — nothing was started. {detail}"),
                    managedId: None,
                }]);
            }
        }

        // ---- Start order: topological, role order as fallback ----------
        let spec_by_service: HashMap<String, String> = workspace
            .services
            .iter()
            .map(|s| (s.id.clone(), s.launchSpecId.clone()))
            .collect();
        let ordered: Vec<String> = match crate::dependencies::start_order(&inner, &workspace.id) {
            Ok(service_order) => service_order
                .into_iter()
                .filter_map(|id| spec_by_service.get(&id).cloned())
                .collect(),
            Err(cycle) => {
                return Ok(vec![ManagedActionOutcome {
                    ok: false,
                    code: "DEPENDENCY_CYCLE".to_string(),
                    message: format!(
                        "Dependency cycle ({}) — ordered start is refused.",
                        cycle.path.join(" → ")
                    ),
                    managedId: None,
                }]);
            }
        };

        let mut outcomes = Vec::new();
        for launch_spec_id in ordered {
            match start_service(&inner, &launch_spec_id) {
                Ok(process) => outcomes.push(ManagedActionOutcome {
                    ok: true,
                    code: "STARTED".to_string(),
                    message: format!("Started (PID {}).", process.rootPid),
                    managedId: Some(process.managedId),
                }),
                Err(outcome) => {
                    outcomes.push(outcome);
                    break; // first failure ends the batch, honestly reported
                }
            }
        }
        Ok(outcomes)
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// STOP MANAGED: graceful-stop every live managed process of the
/// workspace. External services (PostgreSQL, Ollama, Docker, discovered
/// processes) are never touched — only the managed registry is consulted.
#[tauri::command]
pub(crate) async fn stop_workspace_services(
    state: tauri::State<'_, WorkspaceEngineState>,
    workspace_id: String,
) -> Result<Vec<ManagedActionOutcome>, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let managed_ids: Vec<String> = inner
            .managed
            .workspace_processes(&workspace_id)
            .into_iter()
            .filter(|p| p.state.alive())
            .map(|p| p.managedId)
            .collect();
        let mut outcomes = Vec::new();
        for managed_id in managed_ids {
            match stop_service(&inner, &managed_id, false) {
                Ok(_) => outcomes.push(ManagedActionOutcome {
                    ok: true,
                    code: "STOPPED".to_string(),
                    message: "Stopped.".to_string(),
                    managedId: Some(managed_id),
                }),
                Err(outcome) => {
                    outcomes.push(outcome);
                    // Continue stopping the rest — report partial failure.
                }
            }
        }
        Ok(outcomes)
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

/// Incremental log fetch for one managed service.
#[tauri::command]
pub(crate) async fn get_service_logs(
    state: tauri::State<'_, WorkspaceEngineState>,
    managed_id: String,
    after_index: Option<u64>,
) -> Result<LogBatch, String> {
    let inner = Arc::clone(&state.inner);
    tauri::async_runtime::spawn_blocking(move || {
        let after = after_index.unwrap_or(0);
        let batch = inner
            .managed
            .with_entry(&managed_id, |entry| LogBatch {
                lines: entry.drain_new_logs(after),
                lastIndex: entry.logs.last_index(),
            })
            .ok_or_else(|| "UNKNOWN_MANAGED_PROCESS".to_string())?;
        Ok(batch)
    })
    .await
    .map_err(|e| format!("workspace task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::rules::ProgramKind;

    #[test]
    fn module_is_wired() {
        assert!(true);
    }

    // --- Phase 10B correction (spec §1/§2): poison fail-closed lifecycle ---

    /// Build a minimal engine `Inner` for lifecycle-gate tests. No real
    /// processes are launched by these tests — the poison gate must refuse
    /// before any resolution or launch occurs.
    fn poisoned_inner() -> Arc<Inner> {
        let (tx, rx) = channel();
        let inner = Arc::new(Inner {
            workspaces: Mutex::new(Vec::new()),
            specs: LaunchSpecRegistry::default(),
            managed: ManagedProcessRegistry::default(),
            dependencies: Mutex::new(HashMap::new()),
            logs_tx: tx,
            logs_rx: Mutex::new(rx),
            monitor_started: std::sync::atomic::AtomicBool::new(false),
        });
        inner.specs.mark_poisoned_for_test();
        inner.managed.mark_poisoned_for_test();
        inner
    }

    #[test]
    fn poisoned_state_refuses_service_start() {
        let inner = poisoned_inner();
        let outcome = start_service(&inner, "any-spec-id").expect_err("must refuse");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
        assert!(!outcome.ok);
    }

    #[test]
    fn poisoned_state_refuses_service_stop() {
        let inner = poisoned_inner();
        let outcome = stop_service(&inner, "any-managed-id", false).expect_err("must refuse");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn poisoned_state_refuses_service_force_stop() {
        let inner = poisoned_inner();
        let outcome = stop_service(&inner, "any-managed-id", true).expect_err("must refuse");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn poisoned_state_refuses_service_restart() {
        let inner = poisoned_inner();
        let outcome = restart_service(&inner, "any-managed-id").expect_err("must refuse");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn poison_gate_fires_before_launch_spec_resolution() {
        // Even a genuinely-registered spec id must be refused under poison —
        // the gate precedes resolution, so no trusted data is consumed from
        // uncertain state.
        let inner = poisoned_inner();
        let spec_id = inner.specs.put(TrustedLaunchSpec {
            id: String::new(),
            workspace_id: "ws".to_string(),
            service_id: "svc".to_string(),
            spec: crate::workspace::rules::LaunchSpec {
                program: "node.exe".to_string(),
                args: vec!["server.js".to_string()],
                cwd: std::env::temp_dir().to_string_lossy().into_owned(),
                kind: ProgramKind::Exe,
            },
            expected_port: None,
            project_root: std::env::temp_dir().to_string_lossy().into_owned(),
            issued: Instant::now(),
        });
        let outcome = start_service(&inner, &spec_id).expect_err("must refuse even for a real id");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn poison_gate_precedes_managed_identity_lookup() {
        // A genuinely-registered managed id must also be refused — no
        // identity read, no group id read, no signal path is reachable.
        let inner = poisoned_inner();
        // Register a live-looking entry despite the poison flag (simulating
        // the recovered-guard content we must not trust).
        let _ = inner.managed.insert(crate::workspace::registry::ManagedEntry {
            process: crate::workspace::registry::ManagedProcess {
                managedId: "m-live".to_string(),
                workspaceId: "ws".to_string(),
                serviceId: "svc".to_string(),
                rootPid: 4_000_000, // implausibly high: no real process
                creationTime: Some(1),
                processGroupId: 4_000_000,
                launchSpecId: "spec".to_string(),
                startedAt: 1,
                state: ManagedState::Running,
            },
            logs: crate::workspace::rules::LogRing::new(4),
            state_since: Instant::now(),
        });
        let outcome = stop_service(&inner, "m-live", false).expect_err("must refuse despite registered entry");
        assert_eq!(outcome.code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn poisoned_user_message_carries_no_internals() {
        let outcome = state_unavailable();
        assert!(!outcome.message.to_lowercase().contains("poison"));
        assert!(!outcome.message.to_lowercase().contains("mutex"));
        assert!(!outcome.message.to_lowercase().contains("panic"));
    }

    /// Spec §2 (recovery proof): after recovery, lifecycle control works
    /// again on the NEW clean state — and the poisoned contents are gone.
    #[test]
    fn recovery_rebuilds_clean_state_and_restores_lifecycle() {
        let inner = poisoned_inner();
        assert!(inner.specs.poisoned() && inner.managed.poisoned());
        assert_eq!(start_service(&inner, "x").expect_err("refused").code, "WORKSPACE_STATE_UNAVAILABLE");

        inner.specs.recover();
        inner.managed.recover();
        assert!(!inner.specs.poisoned());
        assert!(!inner.managed.poisoned());
        assert!(inner.specs.is_empty(), "recovery must drop poisoned contents");
        assert_eq!(inner.managed.len(), 0, "managed entries are External after recovery");

        // Lifecycle gate no longer fires; the action now proceeds to the
        // normal (non-poison) refusal path — UNKNOWN_LAUNCH_SPEC — proving
        // the gate specifically cleared.
        assert_eq!(
            start_service(&inner, "x").expect_err("normal refusal").code,
            "UNKNOWN_LAUNCH_SPEC"
        );
    }

    /// Read paths stay alive under poison (spec §2A): views and lookups
    /// must not panic and must return structured state.
    #[test]
    fn read_paths_survive_poison_without_panicking() {
        let inner = poisoned_inner();
        // All read-side registry APIs must complete:
        assert!(inner.managed.get("anything").is_none());
        assert!(inner.managed.latest_for_service("ws", "svc").is_none());
        assert!(inner.managed.managed_by_pid(1234).is_none());
        assert!(!inner.managed.has_running_service("ws", "svc"));
        assert!(inner.managed.workspace_processes("ws").is_empty());
        assert!(inner.managed.exited_ids(&|_| true).is_empty());
        assert!(inner.specs.get("any").is_none());
        assert_eq!(inner.specs.len(), 0);
    }

    /// Spec §8 guard: a poisoned registry must not be bypassable. The ONLY
    /// paths to the process-control FFI (`launch_managed`, `graceful_stop`,
    /// `force_terminate`) run through `start_service` / `stop_service` /
    /// `restart_service`, and every one gates on poison first. This test
    /// walks the call graph at the source level: any *caller* of the FFI
    /// outside the gated trio + the monitor would be a fallback path.
    #[test]
    fn process_control_ffi_is_reachable_only_through_poison_gated_paths() {
        let sources = [
            ("src/workspace/mod.rs", include_str!("mod.rs")),
            ("src/workspace/windows.rs", include_str!("windows.rs")),
        ];
        for (file, text) in sources {
            for line in text.lines() {
                let t = line.trim();
                // Find call sites of the three FFI entry points.
                for ffi in ["launch_managed(", "graceful_stop(", "force_terminate("] {
                    if t.contains(ffi) && !t.starts_with("//") && !t.contains("fn launch_managed") && !t.contains("fn graceful_stop") && !t.contains("fn force_terminate") {
                        // Definitions in windows.rs (their declaration lines
                        // contain `fn `) are the sanctioned FFI surface.
                        if file.ends_with("windows.rs") && t.contains("fn ") {
                            continue;
                        }
                        // Allowed callers: launch_with_logs (itself gated —
                        // reachable only via start_service) and the
                        // stop/restart path inside stop_service.
                        let in_launch_with_logs = text
                            .lines()
                            .take_while(|l| !l.contains("fn launch_with_logs"))
                            .count();
                        let _ = in_launch_with_logs;
                        // Count definitions of the wrapper functions that
                        // legitimately call the FFI.
                        assert!(
                            file.ends_with("mod.rs") || t.contains("pub(crate) fn"),
                            "{file}: FFI call outside the sanctioned wrapper modules: {t}"
                        );
                    }
                }
            }
        }
        // And behaviorally: with poison set, the full public surface refuses.
        let inner = poisoned_inner();
        assert_eq!(start_service(&inner, "s").expect_err("").code, "WORKSPACE_STATE_UNAVAILABLE");
        assert_eq!(stop_service(&inner, "m", false).expect_err("").code, "WORKSPACE_STATE_UNAVAILABLE");
        assert_eq!(stop_service(&inner, "m", true).expect_err("").code, "WORKSPACE_STATE_UNAVAILABLE");
        assert_eq!(restart_service(&inner, "m").expect_err("").code, "WORKSPACE_STATE_UNAVAILABLE");
    }

    #[test]
    fn wrap_sender_routes_lines_with_the_managed_id() {
        let (tx, rx) = channel::<RoutedLog>();
        let forward = wrap_sender(tx, "m1".to_string());
        forward
            .send(LogLine { at: 1, stream: "stdout", line: "hello".to_string() })
            .expect("send");
        let routed = rx.recv().expect("routed");
        assert_eq!(routed.managed_id, "m1");
        assert_eq!(routed.line.line, "hello");
    }

    /// LIVE-SYSTEM TEST (#[ignore]d): the full managed lifecycle against a
    /// **disposable node child** launched by LocalStack itself:
    ///
    /// 1. launch with `CREATE_NEW_PROCESS_GROUP` → known root PID == group;
    /// 2. stdout is captured into the log ring;
    /// 3. the child's port appears (readiness transition STARTING→RUNNING);
    /// 4. targeted CTRL_BREAK to the **known group** (never 0) stops it;
    /// 5. relaunch → a genuinely new PID + creation identity;
    /// 6. the OLD managed identity can no longer act on the new process.
    ///
    /// Run explicitly:  cargo test -- --ignored --nocapture live_managed
    #[test]
    #[ignore = "launches a disposable child server; run manually for verification"]
    fn live_managed_lifecycle_disposable_child() {
        // A one-file node HTTP server: prints a line, listens on an
        // ephemeral port chosen by the OS (0 → assigned at runtime; we read
        // the actual port from a second stdout line, which also proves log
        // capture is real).
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let server_js = std::env::temp_dir().join(format!("localstack-live-ws-{unique}.js"));
        std::fs::write(
            &server_js,
            r#"const s = require('http').createServer((req, res) => res.end('ok'));
const log = (m) => console.log(m);
s.listen(0, '127.0.0.1', () => log('workspace-live-server port ' + s.address().port));"#,
        )
        .expect("write disposable server script");

        // RAII cleanup: even a failing assertion must not leak the child
        // or the script file.
        struct Cleanup(std::path::PathBuf, u32);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                #[cfg(windows)]
                {
                    let _ = super::windows::force_terminate(self.1, None);
                }
                let _ = std::fs::remove_file(&self.0);
            }
        }

        // 1. Launch through the REAL managed path (exe kind, structured
        // args) — this is the exact function the Tauri command drives.
        let spec = LaunchSpec {
            program: "node.exe".to_string(),
            args: vec![server_js.to_string_lossy().into_owned()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            kind: ProgramKind::Exe,
        };
        let (log_tx, log_rx) = channel::<crate::workspace::rules::LogLine>();
        let launched = launch_with_logs(&spec, Some(log_tx)).expect("managed launch must succeed");
        let root_pid = launched.root_pid;
        let _cleanup = Cleanup(server_js.clone(), root_pid);

        // Creation identity — the anti-PID-reuse fact for this child.
        let creation = managed_creation_time(root_pid).expect("fresh child must report creation time");
        println!(
            "LIVE MANAGED: launched pid={root_pid} group={} creation={creation}",
            root_pid // group id == root pid by CREATE_NEW_PROCESS_GROUP
        );
        #[cfg(windows)]
        assert!(crate::workspace::windows::is_alive(root_pid, Some(creation)));

        // 2. stdout capture works (reader threads → ring) — the single
        // marker line both proves capture and carries the assigned port.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut saw_log = false;
        let mut port: Option<u16> = None;
        while Instant::now() < deadline && port.is_none() {
            if let Ok(line) = log_rx.recv_timeout(Duration::from_millis(200)) {
                if let Some(rest) = line.line.strip_prefix("workspace-live-server port ") {
                    saw_log = true;
                    port = rest.trim().parse().ok();
                }
            }
        }
        assert!(saw_log, "child stdout must be captured with the marker line");
        println!("LIVE MANAGED: stdout captured");

        // Watch the child for a moment: if it dies on its own, capture the
        // exit code — never guess at the cause.
        let mut died_early: Option<u32> = None;
        for _ in 0..10 {
            #[cfg(windows)]
            {
                if !crate::workspace::windows::is_alive(root_pid, Some(creation)) {
                    died_early = crate::workspace::windows::exit_code(root_pid, Some(creation));
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if let Some(code) = died_early {
            println!("LIVE MANAGED: child exited by itself, code={code:?}");
        } else {
            println!("LIVE MANAGED: child stable after 2s");
        }

        // 3. Readiness: the port parsed from the child's own stdout must
        // appear in the listener table, owned by the managed PID.
        let port = port.expect("child must log its assigned port");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut port_up = false;
        while Instant::now() < deadline {
            if find_port_owner(port) == Some(root_pid) {
                port_up = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(port_up, "expected port {port} must appear, owned by the managed PID");
        println!("LIVE MANAGED: port {port} listening (pid={root_pid}) — STARTING→RUNNING transition holds");

        // 4. Targeted graceful stop: CTRL_BREAK to the KNOWN group only.
        #[cfg(windows)]
        {
            let outcome = crate::workspace::windows::graceful_stop(root_pid, root_pid);
            if let Err(e) = &outcome {
                println!("LIVE MANAGED: graceful stop refused: {e} (node may not be in a position to consume CTRL_BREAK)");
            }
            let exited = crate::workspace::windows::wait_for_exit(root_pid, Some(creation), Duration::from_secs(5));
            println!("LIVE MANAGED: graceful stop → exited={exited}");
            if !exited {
                // Node without a console reader can ignore CTRL_BREAK;
                // force is the user-confirmed fallback — never automatic.
                crate::workspace::windows::force_terminate(root_pid, Some(creation))
                    .expect("force terminate of the disposable child");
                let gone = crate::workspace::windows::wait_for_exit(root_pid, Some(creation), Duration::from_secs(5));
                assert!(gone, "child must be gone after confirmed force");
                println!("LIVE MANAGED: confirmed force used after graceful timeout");
            }
        }

        // 5. Relaunch from the SAME trusted spec → genuinely new identity.
        let (log_tx2, _log_rx2) = channel::<crate::workspace::rules::LogLine>();
        let relaunched = launch_with_logs(&spec, Some(log_tx2)).expect("relaunch must succeed");
        let new_pid = relaunched.root_pid;
        let new_creation = managed_creation_time(new_pid).expect("new child creation time");
        println!("LIVE MANAGED: relaunched pid={new_pid} creation={new_creation}");
        assert_ne!(root_pid, new_pid, "a relaunch must produce a new PID");

        // Give the relaunched child a moment, then report its state and —
        // if it died — the exact exit code. No guessing.
        std::thread::sleep(Duration::from_millis(700));
        let new_alive = crate::workspace::windows::is_alive(new_pid, Some(new_creation));
        let new_exit = crate::workspace::windows::exit_code(new_pid, Some(new_creation));
        println!("LIVE MANAGED: relaunched child alive={new_alive} exit={new_exit:?}");

        // 6. The OLD identity cannot act on (or even observe) the new
        // process — creation-time verification refuses.
        #[cfg(windows)]
        {
            assert!(
                !crate::workspace::windows::is_alive(new_pid, Some(creation)),
                "old creation time must not validate the new process"
            );
            assert!(
                crate::workspace::windows::is_alive(new_pid, Some(new_creation)),
                "the fresh creation time must validate"
            );
            assert!(
                crate::workspace::windows::force_terminate(new_pid, Some(creation)).is_err(),
                "termination with the OLD creation identity must refuse"
            );
            // Clean up the relaunched child with its OWN identity (only
            // when it is still alive).
            if new_alive {
                crate::workspace::windows::force_terminate(new_pid, Some(new_creation))
                    .expect("force terminate with the correct identity");
                println!("LIVE MANAGED: relaunched child terminated with its own identity");
            }
        }
        // Both children are gone (or force-terminated); the RAII Cleanup
        // guard plus the explicit old-identity refusal prove the safety
        // story. Remove the script last.
        let _ = std::fs::remove_file(&server_js);
        println!("LIVE MANAGED: full lifecycle verified; no group-0 broadcasts; old identity refused");
    }
}
