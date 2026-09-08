//! # Port Conflict Engine (Phase 7A)
//!
//! Explains *who owns a requested port* and *whether a workspace service
//! can bind it* — evidence-based, read-only, advisory.
//!
//! ## Structure
//!
//! - [`rules`] — pure bind semantics + conflict classification (unit-tested).
//! - [`free_port`] — pure bounded Free Port Finder (unit-tested).
//! - This facade — resolves owners from a live discovery snapshot and
//!   exposes the two Tauri commands.
//!
//! ## No automatic mutation
//!
//! The engine never edits configuration, never changes ports, never stops
//! the conflicting owner. Resolution options are *derived from real
//! capabilities* and are executed only through the existing hardened
//! control paths (Phase 5 external / Phase 6 managed), always after an
//! explicit user confirmation.

pub(crate) mod free_port;
pub(crate) mod rules;

use serde::Serialize;

use crate::discovery::PortListener;
use rules::{ConflictKind, ConflictSeverity, ListenerVerdict, OwnerLifecycle, PortOwner};

#[cfg(test)]
use crate::conflicts::free_port::CandidateStatus;

/// Who is asking for the port (spec §1).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct RequestedBy {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspaceId: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspaceName: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serviceId: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serviceName: Option<String>,
    /// The workspace's project root (its stable project identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectId: Option<String>,
}

/// Resolution options, derived from actual capabilities (spec §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionOption {
    /// A browser-friendly localhost URL exists for the owner.
    OpenExisting,
    /// Owner details are available for display.
    ShowOwner,
    /// The owner is a LocalStack-managed service and may be stopped through
    /// the existing managed lifecycle (explicit confirmation in the UI).
    StopManagedService,
    /// Advisory free-port alternatives can be listed.
    FindFreePort,
}

/// The full conflict report for one requested port (spec §1).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PortConflictReport {
    pub requestedPort: u16,
    pub requestedBy: RequestedBy,
    /// Classified outcome (snake_case string for the frontend).
    pub kind: ConflictKind,
    pub severity: ConflictSeverity,
    /// Human-facing root-cause sentence, evidence-based.
    pub message: String,
    /// Resolved owner, when one could be identified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<PortOwner>,
    /// The listeners observed on the requested port.
    pub listeners: Vec<ListenerSummary>,
    /// Options the UI may offer, all backed by real capabilities.
    pub resolutions: Vec<ResolutionOption>,
}

/// One observed listener on the requested port.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ListenerSummary {
    pub ipVersion: u8,
    pub address: String,
    pub pid: u32,
}

/// Everything the engine needs to resolve owner metadata for a snapshot.
/// Implemented by the Tauri-managed state bundle; pure code above stays
/// independent of Tauri.
pub(crate) struct ResolutionContext<'a> {
    /// Process names for the owner PIDs (sampled once per PID).
    pub process_names: std::collections::HashMap<u32, Option<String>>,
    /// Service identity display names per PID (from the intelligence layer).
    pub service_names: std::collections::HashMap<u32, String>,
    /// Project id per PID.
    pub project_by_pid: std::collections::HashMap<u32, String>,
    /// Unique projects (id → name).
    pub project_names: std::collections::HashMap<String, String>,
    /// Managed registry lookups: pid → (workspace name, service name).
    pub managed_by_pid: std::collections::HashMap<u32, (Option<String>, String)>,
    pub _marker: std::marker::PhantomData<&'a ()>,
}

impl ResolutionContext<'_> {
    #[cfg(any(not(windows), test))]
    pub(crate) fn empty() -> Self {
        Self {
            process_names: std::collections::HashMap::new(),
            service_names: std::collections::HashMap::new(),
            project_by_pid: std::collections::HashMap::new(),
            project_names: std::collections::HashMap::new(),
            managed_by_pid: std::collections::HashMap::new(),
            _marker: std::marker::PhantomData,
        }
    }
}

/// Build the owner descriptor for one PID from the resolution context.
pub(crate) fn owner_of(pid: u32, context: &ResolutionContext<'_>) -> PortOwner {
    let (managed_workspace, managed_service) =
        context.managed_by_pid.get(&pid).cloned().unwrap_or((None, String::new()));
    let lifecycle = if context.managed_by_pid.contains_key(&pid) {
        OwnerLifecycle::Managed
    } else {
        OwnerLifecycle::External
    };
    let project_id = context.project_by_pid.get(&pid).cloned();
    let project_name = project_id
        .as_ref()
        .and_then(|id| context.project_names.get(id).cloned());
    PortOwner {
        pid,
        processName: context.process_names.get(&pid).cloned().flatten(),
        serviceDisplayName: context.service_names.get(&pid).cloned(),
        projectId: project_id,
        projectName: project_name,
        lifecycle,
        managedWorkspace: managed_workspace,
        managedService: if managed_service.is_empty() { None } else { Some(managed_service) },
    }
}

/// Windows-reserved PIDs that can never be user services (System Idle /
/// System kernel sockets).
fn is_reserved_pid(pid: u32) -> bool {
    pid == 0 || pid == 4
}

/// Classify the conflict for one requested port against observed listeners.
///
/// Pure over the inputs: the listeners must be the *current* table (the
/// caller enumerates fresh) and `same_managed_service_pid` identifies the
/// requesting managed service's own root PID, if it has one already.
pub(crate) fn classify_port(
    requested_port: u16,
    requested_by: RequestedBy,
    listeners: &[PortListener],
    same_managed_service_pid: Option<u32>,
    context: &ResolutionContext<'_>,
) -> PortConflictReport {
    let on_port: Vec<&PortListener> =
        listeners.iter().filter(|l| l.port == requested_port).collect();

    let requester_is_v6 = None; // bind family is unknown before launch
    let mut verdicts: Vec<ListenerVerdict> = Vec::with_capacity(on_port.len());
    let mut owners: Vec<Option<PortOwner>> = Vec::with_capacity(on_port.len());

    for listener in &on_port {
        let owner_is_same = same_managed_service_pid.is_some_and(|pid| pid == listener.pid);
        let verdict = rules::evaluate_listener(
            requester_is_v6,
            listener,
            owner_is_same,
            is_reserved_pid(listener.pid),
        );
        verdicts.push(verdict);
        owners.push(if is_reserved_pid(listener.pid) {
            None
        } else {
            Some(owner_of(listener.pid, context))
        });
    }

    let worst = rules::aggregate(&verdicts);
    let (kind, severity, owner, message) = match worst {
        None => (
            ConflictKind::NoConflict,
            ConflictSeverity::Info,
            None,
            format!("Port {requested_port} is free."),
        ),
        Some(verdict) => {
            // Prefer the owner of the worst-severity listener.
            let index = verdicts
                .iter()
                .position(|v| v.severity == verdict.severity && v.kind == verdict.kind)
                .unwrap_or(0);
            let owner = owners[index].clone();
            let kind = refine_kind(verdict.kind, owner.as_ref(), &requested_by);
            let message = describe(kind, severity_of(&verdict), requested_port, owner.as_ref(), &requested_by);
            (kind, verdict.severity, owner, message)
        }
    };

    // Resolution options from real capabilities (spec §11): no destructive
    // automatic action is ever offered for external owners.
    let mut resolutions = Vec::new();
    if let Some(owner) = &owner {
        if matches!(kind, ConflictKind::OtherProject | ConflictKind::SameProjectExternal | ConflictKind::SameProjectManaged | ConflictKind::UnknownOwner) {
            resolutions.push(ResolutionOption::ShowOwner);
            if owner.lifecycle == OwnerLifecycle::Managed {
                resolutions.push(ResolutionOption::StopManagedService);
            }
        }
        // Open-existing requires a localhost URL; the UI verifies its own
        // snapshot controls for the PID. Advisory offer only.
        resolutions.push(ResolutionOption::OpenExisting);
    }
    if matches!(severity, ConflictSeverity::Blocking | ConflictSeverity::Potential) {
        resolutions.push(ResolutionOption::FindFreePort);
    }

    PortConflictReport {
        requestedPort: requested_port,
        requestedBy: requested_by,
        kind,
        severity,
        message,
        owner,
        listeners: on_port
            .iter()
            .map(|l| ListenerSummary {
                ipVersion: match l.ipVersion {
                    crate::discovery::ports::IpVersion::V4 => 4,
                    crate::discovery::ports::IpVersion::V6 => 6,
                },
                address: l.localAddress.clone(),
                pid: l.pid,
            })
            .collect(),
        resolutions,
    }
}

/// Refine the generic per-listener kind with the resolved owner metadata.
fn refine_kind(
    base: ConflictKind,
    owner: Option<&PortOwner>,
    requested_by: &RequestedBy,
) -> ConflictKind {
    use ConflictKind::*;
    match base {
        AlreadyRunning | ReservedOrUnverifiable | DualStackEquivalent | NoConflict => base,
        _ => {
            let Some(owner) = owner else {
                return UnknownOwner;
            };
            if owner.lifecycle == OwnerLifecycle::Managed {
                if requested_by.workspaceId.is_some()
                    && owner.managedWorkspace.as_deref() == requested_by.workspaceId.as_deref()
                {
                    SameProjectManaged
                } else {
                    OtherProject
                }
            } else if owner.projectId.is_some()
                && requested_by.projectId.is_some()
                && owner.projectId == requested_by.projectId
            {
                SameProjectExternal
            } else if owner.projectId.is_some() {
                OtherProject
            } else {
                UnknownOwner
            }
        }
    }
}

/// Evidence-based human-facing explanation (the product principle: no bare
/// "port in use").
fn severity_label(severity: ConflictSeverity) -> &'static str {
    match severity {
        ConflictSeverity::Blocking => "",
        ConflictSeverity::Potential => " may still collide with (bind semantics cannot be verified)",
        ConflictSeverity::Info => "",
    }
}

fn describe(
    kind: ConflictKind,
    severity: ConflictSeverity,
    port: u16,
    owner: Option<&PortOwner>,
    requested_by: &RequestedBy,
) -> String {
    let requester = requested_by
        .serviceName
        .clone()
        .unwrap_or_else(|| "The requested service".to_string());
    match kind {
        ConflictKind::NoConflict => format!("Port {port} is free."),
        ConflictKind::AlreadyRunning => {
            format!("{requester} is already running on port {port}.")
        }
        ConflictKind::SameProjectExternal => format!(
            "Port {port} is held by an external (not LocalStack-managed) process of the same project: {}. Decide on that instance separately.",
            owner_label(owner)
        ),
        ConflictKind::SameProjectManaged => format!(
            "Port {port} is held by the managed service {} of the same workspace.",
            owner_label(owner)
        ),
        ConflictKind::OtherProject => format!(
            "Port {port} is owned by {}{}.",
            owner_label(owner),
            severity_label(severity)
        ),
        ConflictKind::UnknownOwner => format!(
            "Port {port} is occupied, but the owner's identity is unavailable (PID {} may be protected or gone).",
            owner.map(|o| o.pid).unwrap_or(0)
        ),
        ConflictKind::DualStackEquivalent => format!(
            "Port {port} has an IPv6 wildcard listener that{} conflict with an IPv4 bind (dual-stack behavior cannot be verified from the TCP table).",
            if severity == ConflictSeverity::Potential { " may" } else { " does" }
        ),
        ConflictKind::ReservedOrUnverifiable => format!(
            "Port {port} is held by a Windows-reserved socket that cannot be inspected."
        ),
    }
}

fn owner_label(owner: Option<&PortOwner>) -> String {
    let Some(owner) = owner else {
        return "an unresolvable process".to_string();
    };
    let identity = owner
        .serviceDisplayName
        .clone()
        .or_else(|| owner.processName.clone())
        .unwrap_or_else(|| "unknown process".to_string());
    let lifecycle = match owner.lifecycle {
        OwnerLifecycle::Managed => "managed",
        OwnerLifecycle::External => "external",
    };
    match (&owner.projectName, owner.managedService.as_deref()) {
        (Some(project), Some(service)) => format!("{service} ({identity}, {lifecycle}, project {project})"),
        (Some(project), None) => format!("{identity} ({lifecycle}, project {project}, PID {})", owner.pid),
        (None, Some(service)) => format!("{service} ({identity}, {lifecycle})"),
        (None, None) => format!("{identity} ({lifecycle}, PID {})", owner.pid),
    }
}

fn severity_of(verdict: &ListenerVerdict) -> ConflictSeverity {
    verdict.severity
}

// ---------------------------------------------------------------------------
// Launch-preflight helper (shared with the workspace engine)
// ---------------------------------------------------------------------------

/// Why a launch is blocked by the port state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LaunchPortBlock {
    /// The PID that currently owns the port, when resolvable.
    pub owner_pid: Option<u32>,
    /// Owner process name when resolvable.
    pub owner_name: Option<String>,
    /// True when the PID belongs to the same managed service (not a block —
    /// the workspace engine checks ALREADY_RUNNING separately).
    pub same_managed_service: bool,
    /// True when the PID belongs to another LocalStack-managed service.
    pub owned_by_managed: bool,
}

/// Live preflight for one expected port: enumerates the current listener
/// table and resolves the owner's process name. Used by the workspace
/// engine before creating a process. Windows-only; on other platforms it
/// never blocks.
#[cfg(windows)]
pub(crate) fn check_launch_port(port: u16, same_managed_pids: &[u32]) -> Option<LaunchPortBlock> {
    let listeners = crate::discovery::windows::enumerate_tcp_listeners().ok()?;
    let listener = listeners.iter().find(|l| l.port == port)?;
    let same = same_managed_pids.contains(&listener.pid);
    let (owner_name, _) = crate::control::windows::revalidate(listener.pid)
        .unwrap_or((None, None));
    Some(LaunchPortBlock {
        owner_pid: Some(listener.pid),
        owner_name,
        same_managed_service: same,
        owned_by_managed: false, // resolved by the caller via its registry
    })
}

#[cfg(not(windows))]
pub(crate) fn check_launch_port(_port: u16, _same_managed_pids: &[u32]) -> Option<LaunchPortBlock> {
    None
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Read-only: evaluate who owns `port` right now and whether a workspace
/// service could bind it. Owners are resolved from a fresh listener table
/// plus one-shot process/identity/project lookups — no cached ownership is
/// ever trusted (spec §33).
#[tauri::command]
pub(crate) async fn evaluate_port(
    port: u16,
    state: tauri::State<'_, crate::workspace::WorkspaceEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
) -> Result<PortConflictReport, String> {
    let inner = state.engine_inner();
    let projects = std::sync::Arc::clone(&projects.cache);
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let listeners = crate::discovery::windows::enumerate_tcp_listeners()
            .map_err(|e| format!("listener enumeration failed: {e}"))?;
        #[cfg(not(windows))]
        let listeners: Vec<PortListener> = Vec::new();

        let projects_handle = crate::project::ProjectEngineState { cache: projects };
        let context = build_context(&listeners, &inner, &projects_handle)?;
        let report = classify_port(
            port,
            RequestedBy {
                workspaceId: None,
                workspaceName: None,
                serviceId: None,
                serviceName: None,
                projectId: None,
            },
            &listeners,
            None,
            &context,
        );
        Ok(report)
    })
    .await
    .map_err(|e| format!("conflict task join error: {e}"))?
}

/// Read-only: bounded free-port suggestions near `preferred` (spec §8–10).
#[tauri::command]
pub(crate) async fn find_free_ports(
    preferred: u16,
    count: Option<usize>,
) -> Result<Vec<free_port::PortCandidate>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(windows)]
        let listeners = crate::discovery::windows::enumerate_tcp_listeners()
            .map_err(|e| format!("listener enumeration failed: {e}"))?;
        #[cfg(not(windows))]
        let listeners: Vec<PortListener> = Vec::new();

        let occupied = occupied_index(&listeners);
        Ok(free_port::find_free_ports(preferred, count.unwrap_or(free_port::SUGGESTION_COUNT), &occupied))
    })
    .await
    .map_err(|e| format!("free-port task join error: {e}"))?
}

/// Port → owner-summary index from one listener table (used by the Free
/// Port Finder and readiness evaluation).
pub(crate) fn occupied_index(listeners: &[PortListener]) -> std::collections::BTreeMap<u16, Vec<String>> {
    let mut index: std::collections::BTreeMap<u16, Vec<String>> = std::collections::BTreeMap::new();
    for listener in listeners {
        let summary = format!("PID {}", listener.pid);
        index.entry(listener.port).or_default().push(summary);
    }
    index
}

/// Build the owner-resolution context for a listener table: sample each
/// owner PID once, classify identities, resolve projects against the shared
/// cache, and mark managed PIDs.
#[cfg(windows)]
pub(crate) fn build_context(
    listeners: &[PortListener],
    inner: &crate::workspace::Inner,
    projects: &crate::project::ProjectEngineState,
) -> Result<ResolutionContext<'static>, String> {
    use std::collections::BTreeSet;

    let unique_pids: Vec<u32> = listeners
        .iter()
        .map(|l| l.pid)
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect();

    // Sample the owner PIDs once (fresh metadata, no cached ownership).
    let (processes, _samples) = crate::process::windows::sample_processes(&unique_pids);
    let mut named = processes;
    crate::process::windows::apply_snapshot_names(&mut named);

    let services = crate::intelligence::classify_processes(&named, listeners);

    let service_names: std::collections::HashMap<u32, String> = services
        .iter()
        .map(|s| (s.pid, s.identity.displayName.clone()))
        .collect();
    let process_names: std::collections::HashMap<u32, Option<String>> =
        named.iter().map(|p| (p.pid, p.name.clone())).collect();

    // Project resolution against the shared cache (warm cycles do no I/O).
    let mut project_cache = projects
        .cache
        .lock()
        .map_err(|_| "project cache lock poisoned".to_string())?;
    let (projects_list, links, _stats) =
        crate::project::resolve_projects(&named, &mut project_cache, false);
    drop(project_cache);
    let project_by_pid: std::collections::HashMap<u32, String> =
        links.into_iter().map(|l| (l.pid, l.projectId)).collect();
    let project_names: std::collections::HashMap<String, String> = projects_list
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();

    // Managed registry: pid → (workspace name, service name).
    let mut managed_by_pid = std::collections::HashMap::new();
    let workspaces = inner
        .workspaces
        .lock()
        .map_err(|_| "workspace list lock poisoned".to_string())?;
    inner.managed.for_each(&mut |process| {
        let workspace_name = workspaces
            .iter()
            .find(|w| w.id == process.workspaceId)
            .map(|w| w.name.clone());
        managed_by_pid.insert(process.rootPid, (workspace_name, process.serviceId.clone()));
    });

    Ok(ResolutionContext {
        process_names,
        service_names,
        project_by_pid,
        project_names,
        managed_by_pid,
        _marker: std::marker::PhantomData,
    })
}

#[cfg(not(windows))]
pub(crate) fn build_context(
    _listeners: &[PortListener],
    _inner: &crate::workspace::Inner,
    _projects: &crate::project::ProjectEngineState,
) -> Result<ResolutionContext<'static>, String> {
    Ok(ResolutionContext::empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ports::{IpVersion, ListenerState, Protocol};

    fn listener(v6: bool, address: &str, port: u16, pid: u32) -> PortListener {
        PortListener {
            protocol: Protocol::Tcp,
            ipVersion: if v6 { IpVersion::V6 } else { IpVersion::V4 },
            localAddress: address.to_string(),
            port,
            pid,
            state: ListenerState::Listen,
        }
    }

    fn ctx(pids: &[u32]) -> ResolutionContext<'static> {
        let mut context = ResolutionContext::empty();
        for pid in pids {
            context.process_names.insert(*pid, Some("node.exe".to_string()));
            context.service_names.insert(*pid, "Vite".to_string());
        }
        context
    }

    fn requester() -> RequestedBy {
        RequestedBy {
            workspaceId: Some("ws1".to_string()),
            workspaceName: Some("HistoryAI".to_string()),
            serviceId: Some("svc-1".to_string()),
            serviceName: Some("Frontend".to_string()),
            projectId: Some(r"D:\Projects\historyai".to_string()),
        }
    }

    #[test]
    fn free_port_is_no_conflict() {
        let report = classify_port(3000, requester(), &[], None, &ctx(&[]));
        assert_eq!(report.kind, ConflictKind::NoConflict);
        assert_eq!(report.message, "Port 3000 is free.");
        assert!(report.owner.is_none());
    }

    #[test]
    fn same_managed_instance_is_already_running() {
        let listeners = vec![listener(false, "127.0.0.1", 1420, 8332)];
        let report = classify_port(1420, requester(), &listeners, Some(8332), &ctx(&[8332]));
        assert_eq!(report.kind, ConflictKind::AlreadyRunning);
        assert_eq!(report.severity, ConflictSeverity::Info);
        assert!(report.message.contains("already running"));
    }

    #[test]
    fn same_project_external_is_recognized() {
        let mut context = ctx(&[900]);
        context.project_by_pid.insert(900, r"D:\Projects\historyai".to_string());
        context.project_names.insert(r"D:\Projects\historyai".to_string(), "HistoryAI".to_string());
        let listeners = vec![listener(false, "127.0.0.1", 3000, 900)];
        let report = classify_port(3000, requester(), &listeners, None, &context);
        assert_eq!(report.kind, ConflictKind::SameProjectExternal);
        assert_eq!(report.severity, ConflictSeverity::Blocking);
        assert!(report.message.contains("same project"));
        assert!(report.resolutions.contains(&ResolutionOption::ShowOwner));
        assert!(!report.resolutions.contains(&ResolutionOption::StopManagedService));
    }

    #[test]
    fn other_project_conflict_names_the_owner() {
        let mut context = ctx(&[8332]);
        context.project_by_pid.insert(8332, r"D:\Projects\localstack".to_string());
        context.project_names.insert(r"D:\Projects\localstack".to_string(), "LocalStack".to_string());
        let listeners = vec![listener(false, "127.0.0.1", 3000, 8332)];
        let report = classify_port(3000, requester(), &listeners, None, &context);
        assert_eq!(report.kind, ConflictKind::OtherProject);
        assert!(report.message.contains("LocalStack"), "{}", report.message);
        assert!(report.message.contains("PID 8332"));
    }

    #[test]
    fn managed_owner_gains_the_stop_option() {
        let mut context = ctx(&[500]);
        context
            .managed_by_pid
            .insert(500, (Some("LocalStack".to_string()), "svc-2".to_string()));
        let listeners = vec![listener(false, "0.0.0.0", 3000, 500)];
        let report = classify_port(3000, requester(), &listeners, None, &context);
        assert_eq!(report.owner.as_ref().expect("owner").lifecycle, OwnerLifecycle::Managed);
        assert!(report.resolutions.contains(&ResolutionOption::StopManagedService));
    }

    #[test]
    fn unknown_owner_is_reported_honestly() {
        let listeners = vec![listener(false, "0.0.0.0", 3000, 1234)];
        let report = classify_port(3000, requester(), &listeners, None, &ctx(&[]));
        assert_eq!(report.kind, ConflictKind::UnknownOwner);
        assert!(report.message.contains("unavailable"), "{}", report.message);
    }

    #[test]
    fn reserved_pid_is_unverifiable() {
        let listeners = vec![listener(false, "0.0.0.0", 135, 4)];
        let report = classify_port(135, requester(), &listeners, None, &ctx(&[]));
        assert_eq!(report.kind, ConflictKind::ReservedOrUnverifiable);
        assert!(report.owner.is_none(), "reserved sockets get no fabricated owner");
    }

    #[test]
    fn dual_stack_conflict_stays_potential() {
        let listeners = vec![listener(true, "::", 3000, 700)];
        let report = classify_port(3000, requester(), &listeners, None, &ctx(&[700]));
        assert_eq!(report.kind, ConflictKind::DualStackEquivalent);
        assert_eq!(report.severity, ConflictSeverity::Potential);
    }

    #[test]
    fn missing_project_metadata_does_not_invent_ownership() {
        // Owner has a process name but no project link: honest UnknownOwner,
        // not a guessed project.
        let listeners = vec![listener(false, "127.0.0.1", 3000, 42)];
        let report = classify_port(3000, requester(), &listeners, None, &ctx(&[42]));
        assert_eq!(report.kind, ConflictKind::UnknownOwner);
        let owner = report.owner.expect("owner");
        assert_eq!(owner.projectId, None);
        assert_eq!(owner.processName.as_deref(), Some("node.exe"));
    }

    // -----------------------------------------------------------------------
    // LIVE integration tests (disposable children only) — spec §52.
    // -----------------------------------------------------------------------

    /// Launch a disposable node HTTP server the way the managed engine does
    /// and wait until its OS-assigned port appears in the listener table.
    /// Returns (script path, root pid, creation time, port).
    #[cfg(windows)]
    fn spawn_disposable_server(tag: &str) -> (std::path::PathBuf, u32, u64, u16) {
        use std::sync::mpsc::channel;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let script = std::env::temp_dir().join(format!("localstack-conflict-{tag}-{unique}.js"));
        std::fs::write(
            &script,
            r#"const s = require('http').createServer((req, res) => res.end('ok'));
s.listen(0, '127.0.0.1', () => console.log('conflict-test port ' + s.address().port));"#,
        )
        .expect("write disposable server script");

        let spec = crate::workspace::LaunchSpec {
            program: "node.exe".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            kind: crate::workspace::rules::ProgramKind::Exe,
        };
        let (log_tx, log_rx) = channel::<crate::workspace::rules::LogLine>();
        let launched =
            crate::workspace::launch_with_logs(&spec, Some(log_tx)).expect("managed launch");
        let creation =
            crate::workspace::managed_creation_time(launched.root_pid).expect("creation time");

        // Read the assigned port from the child's own stdout (proves capture).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let port = loop {
            if std::time::Instant::now() > deadline {
                panic!("disposable child never logged its port");
            }
            if let Ok(line) = log_rx.recv_timeout(std::time::Duration::from_millis(200)) {
                if let Some(rest) = line.line.strip_prefix("conflict-test port ") {
                    break rest.trim().parse::<u16>().expect("port line");
                }
            }
        };

        // Wait for the listener to actually appear.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if crate::discovery::windows::enumerate_tcp_listeners()
                .expect("listener table")
                .iter()
                .any(|l| l.port == port && l.pid == launched.root_pid)
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "port {port} never appeared for pid {}",
                launched.root_pid
            );
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        (script, launched.root_pid, creation, port)
    }

    /// RAII guard: force-terminate the child and remove its script even if
    /// an assertion fails. Never touches any other process.
    #[cfg(windows)]
    struct ChildGuard(std::path::PathBuf, u32);
    #[cfg(windows)]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = crate::workspace::windows::force_terminate(self.1, None);
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// SCENARIO A (spec §52): a workspace service expects the port a live
    /// disposable server holds → conflict detected, owner PID identified,
    /// **no process is stopped**, free alternatives are returned; after the
    /// test stops its own child the conflict resolves.
    #[cfg(windows)]
    #[test]
    #[ignore = "launches a disposable child server; run manually for verification"]
    fn live_port_conflict_scenario_a() {
        let (script, pid, creation, port) = spawn_disposable_server("a");
        let _guard = ChildGuard(script, pid);

        // 1. Conflict detected with the real owner PID.
        let listeners =
            crate::discovery::windows::enumerate_tcp_listeners().expect("listener table");
        let occupied = occupied_index(&listeners);
        let requester = RequestedBy {
            workspaceId: Some("ws-test".to_string()),
            workspaceName: Some("ConflictTest".to_string()),
            serviceId: Some("svc-test".to_string()),
            serviceName: Some("TestService".to_string()),
            projectId: None,
        };
        let context = ResolutionContext::empty();
        let report = classify_port(port, requester.clone(), &listeners, None, &context);
        assert_eq!(report.kind, ConflictKind::UnknownOwner, "unindexed pid → honest unknown owner");
        assert_eq!(report.severity, ConflictSeverity::Blocking);
        assert_eq!(report.owner.as_ref().expect("owner").pid, pid, "real PID surfaced");
        println!("SCENARIO A: conflict detected on {port}, owner pid={pid}");

        // 2. Free alternatives exclude the occupied port and the child's own.
        let suggestions = free_port::find_free_ports(port, 5, &occupied);
        assert!(suggestions
            .iter()
            .all(|c| !(c.port == port && c.status == CandidateStatus::Available)));
        assert!(suggestions.iter().any(|c| c.status == CandidateStatus::Available));
        println!("SCENARIO A: {} alternatives returned", suggestions.len());

        // 3. Launch preflight refuses to create a process for this port.
        let block = check_launch_port(port, &[]);
        assert!(block.is_some(), "preflight must block on an occupied port");

        // 4. The child was NOT stopped by any of the above (read-only phase).
        assert!(
            crate::workspace::windows::is_alive(pid, Some(creation)),
            "conflict evaluation must never terminate the owner"
        );

        // 5. Stop the disposable child *through the test itself*, then the
        //    conflict resolves.
        let outcome = crate::workspace::windows::graceful_stop(pid, pid);
        let _ = outcome.map_err(|e| println!("SCENARIO A: graceful stop note: {e}"));
        let exited = crate::workspace::windows::wait_for_exit(pid, Some(creation), std::time::Duration::from_secs(5));
        if !exited {
            crate::workspace::windows::force_terminate(pid, Some(creation)).expect("cleanup");
        }
        // Termination is asynchronous and the dying PID can still open
        // briefly during teardown — wait until gone, don't assert once.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::workspace::windows::is_alive(pid, Some(creation))
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(
            !crate::workspace::windows::is_alive(pid, Some(creation)),
            "child must be gone"
        );
        std::thread::sleep(std::time::Duration::from_millis(300));
        let listeners =
            crate::discovery::windows::enumerate_tcp_listeners().expect("listener table");
        assert!(
            !listeners.iter().any(|l| l.port == port),
            "conflict must resolve once the owner exits"
        );
        println!("SCENARIO A: conflict resolved after owner exit");
    }

    /// SCENARIO B (spec §52): a workspace with a required TCP dependency on
    /// a disposable service port — absent → BLOCKED; running → eligible;
    /// stopped again → BLOCKED. Pure dependency-state transitions over real
    /// process lifecycle.
    #[test]
    #[ignore = "launches a disposable child server; run manually for verification"]
    fn live_dependency_scenario_b() {
        #[cfg(windows)]
        {
            let (script, pid, _creation, port) = spawn_disposable_server("b");
            let _guard = ChildGuard(script, pid);

            // Dependency state derived from the live listener table.
            let state_of = |p: u16| -> crate::dependencies::readiness::DependencyState {
                let listeners = crate::discovery::windows::enumerate_tcp_listeners().expect("table");
                if listeners.iter().any(|l| l.port == p) {
                    crate::dependencies::readiness::DependencyState::Available
                } else {
                    crate::dependencies::readiness::DependencyState::Unavailable
                }
            };

            // While the child runs: dependency AVAILABLE.
            assert_eq!(state_of(port), crate::dependencies::readiness::DependencyState::Available);
            println!("SCENARIO B: dependency available while child runs (port {port})");

            // Stop the child *through the test itself*.
            let outcome = crate::workspace::windows::graceful_stop(pid, pid);
            let _ = outcome.map_err(|e| println!("SCENARIO B: graceful stop note: {e}"));
            let exited = crate::workspace::windows::wait_for_exit(pid, Some(_creation), std::time::Duration::from_secs(5));
            if !exited {
                crate::workspace::windows::force_terminate(pid, Some(_creation)).expect("cleanup");
            }
            std::thread::sleep(std::time::Duration::from_millis(300));

            // After the child exits: UNAVAILABLE → readiness BLOCKED.
            assert_eq!(state_of(port), crate::dependencies::readiness::DependencyState::Unavailable);
            let report = crate::dependencies::readiness::evaluate(
                &[crate::dependencies::readiness::ServiceNode {
                    id: "backend".into(),
                    name: "Backend".into(),
                    expected_port: None,
                    managed: Some(crate::workspace::rules::ManagedState::Stopped),
                    conflict_kind: None,
                    conflict_severity: None,
                }],
                &[crate::dependencies::readiness::DependencyNode {
                    id: "dep-db".into(),
                    source_service_id: "backend".into(),
                    target_label: "TestDependency".into(),
                    required: true,
                    state: state_of(port),
                }],
                None,
            );
            assert_eq!(report.status, "blocked");
            assert!(report.issues.iter().any(|i| i.code == "DEPENDENCY_UNAVAILABLE"));
            println!("SCENARIO B: BLOCKED after dependency exit — transition verified");
        }
        #[cfg(not(windows))]
        {
            println!("SCENARIO B: skipped on non-Windows");
        }
    }
}
