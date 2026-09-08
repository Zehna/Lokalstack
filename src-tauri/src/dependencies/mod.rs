//! # Dependency & Readiness Engine (Phase 7B)
//!
//! Runtime dependency intelligence for workspaces: who needs what, whether
//! it is available, what blocks readiness, and in which order managed
//! services may start.
//!
//! ## Trust and evidence
//!
//! - Dependencies come only from **explicit sources**: workspace services
//!   referencing each other (validated server-side ids) or a **user-confirmed
//!   mapping** to a detected external service (the port is the evaluated
//!   fact; the display name is an advisory label).
//! - Nothing is inferred from port conventions ("5432 exists ⇒ backend
//!   depends on PostgreSQL" is never assumed).
//! - Evaluation is pure over the current listener table: **`Available`
//!   means listening, never healthy** (spec §31) — no HTTP health engine
//!   exists yet and none is faked.
//! - External dependencies are observed, never started, stopped, or restarted.
//!
//! ## Structure
//!
//! - [`graph`] — pure cycle detection + topological start order.
//! - [`readiness`] — pure status precedence + structured root-cause issues.
//! - This facade — storage (bounded, in-memory), evaluation assembly, and
//!   the Tauri command surface.
pub(crate) mod graph;
pub(crate) mod readiness;

use serde::{Deserialize, Serialize};

pub(crate) use graph::DependencyCycle;
pub(crate) use readiness::{DependencyState, ReadinessIssue};

use crate::conflicts::rules::{ConflictKind, ConflictSeverity};
use crate::conflicts::{classify_port, build_context, RequestedBy};
use crate::workspace::rules::{ManagedState, Role};
use crate::workspace::WorkspaceEngineState;

/// Bounded dependency count per workspace (defense against unbounded growth).
pub(crate) const MAX_DEPENDENCIES_PER_WORKSPACE: usize = 64;

// ---------------------------------------------------------------------------
// Domain model
// ---------------------------------------------------------------------------

/// What a dependency points at. Deserialized from the frontend as a tagged
/// choice among backend-validated shapes — never an arbitrary command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(non_snake_case)]
pub(crate) enum DependencyTarget {
    /// Another workspace service (validated backend-issued id).
    Service {
        service_id: String,
    },
    /// An explicitly detected external service the user confirmed. The
    /// evaluated fact is the local `port`; `name` is an advisory label.
    ExternalService { name: String, port: u16 },
    /// A local TCP endpoint (localhost only).
    TcpPort { port: u16 },
    /// A local HTTP-ish endpoint — evaluated by TCP listening only in
    /// Phase 7 (honest: LISTENING ≠ HEALTHY).
    HttpEndpoint { host: String, port: u16 },
}

impl DependencyTarget {
    /// The local port this target is evaluated against, when port-based.
    pub(crate) fn port(&self) -> Option<u16> {
        match self {
            DependencyTarget::Service { .. } => None,
            DependencyTarget::ExternalService { port, .. }
            | DependencyTarget::TcpPort { port }
            | DependencyTarget::HttpEndpoint { port, .. } => Some(*port),
        }
    }

    /// Human-facing label (advisory; never authority).
    pub(crate) fn label(&self) -> String {
        match self {
            DependencyTarget::Service { service_id } => format!("service {service_id}"),
            DependencyTarget::ExternalService { name, port } => format!("{name} :{port}"),
            DependencyTarget::TcpPort { port } => format!("localhost :{port}"),
            DependencyTarget::HttpEndpoint { host, port } => format!("{host}:{port}"),
        }
    }
}

/// One declared runtime dependency (spec §13).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct Dependency {
    /// Opaque id (deterministic over the target fields + per-boot key).
    pub id: String,
    pub workspaceId: String,
    /// Workspace service that depends on the target.
    pub sourceServiceId: String,
    pub target: DependencyTarget,
    /// Required dependencies block readiness; optional ones warn.
    pub required: bool,
}

/// Dependency as delivered to the frontend, with its live state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct DependencyView {
    pub id: String,
    pub workspaceId: String,
    pub sourceServiceId: String,
    pub target: DependencyTarget,
    pub targetLabel: String,
    pub required: bool,
    pub state: DependencyState,
}

/// A target option the UI may offer when adding a dependency.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(non_snake_case)]
pub(crate) enum TargetOption {
    Service { serviceId: String, name: String, expectedPort: Option<u16> },
    /// Reserved for a future backend-derived external-candidate flow; the
    /// current UI derives external candidates from its own live snapshot.
    #[allow(dead_code)]
    ExternalServiceHint,
}

/// Readiness + conflicts + dependencies for one workspace.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct WorkspaceReadinessView {
    pub workspaceId: String,
    pub workspaceName: String,
    pub status: String,
    pub issues: Vec<ReadinessIssue>,
    pub dependencies: Vec<DependencyView>,
    /// Conflicts blocking the workspace's stopped services.
    pub conflicts: Vec<crate::conflicts::PortConflictReport>,
    /// Topological start order (when acyclic) for managed services.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startOrder: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Storage (inside the workspace engine state — one lock domain for
// workspace + specs + managed processes + dependencies)
// ---------------------------------------------------------------------------

/// Opaque deterministic id for a dependency edge.
fn opaque_dependency_id(dependency_key: &str) -> String {
    crate::control::registry::blake3_256(dependency_key.as_bytes())
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Insert (or replace) a dependency for a workspace after validation.
/// Returns the stored dependency. Errors name the exact problem.
pub(crate) fn add_dependency(
    inner: &crate::workspace::Inner,
    workspace_id: &str,
    source_service_id: &str,
    target: DependencyTarget,
    required: bool,
) -> Result<Dependency, String> {
    // Validate against the workspace's own services — never frontend ids.
    {
        let workspaces = inner
            .workspaces
            .lock()
            .map_err(|_| "workspace list lock poisoned".to_string())?;
        let workspace = workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .ok_or("Workspace not found.")?;
        if !workspace.services.iter().any(|s| s.id == source_service_id) {
            return Err("Source service does not belong to this workspace.".to_string());
        }
        if let DependencyTarget::Service { service_id } = &target {
            if service_id == source_service_id {
                return Err("A service cannot depend on itself.".to_string());
            }
            if !workspace.services.iter().any(|s| &s.id == service_id) {
                return Err("Target service does not belong to this workspace.".to_string());
            }
        }
        if let DependencyTarget::HttpEndpoint { host, .. } = &target {
            let host = host.trim_matches(['[', ']']).to_lowercase();
            let local = matches!(
                host.as_str(),
                "localhost" | "127.0.0.1" | "::1" | "0:0:0:0:0:0:0:1"
            );
            if !local {
                return Err("HTTP endpoints are limited to localhost in Phase 7.".to_string());
            }
        }
    }

    let dependency_key = format!(
        "{workspace_id}\u{0}{source_service_id}\u{0}{}",
        serde_json::to_string(&target).map_err(|e| e.to_string())?
    );
    let dependency = Dependency {
        id: opaque_dependency_id(&dependency_key),
        workspaceId: workspace_id.to_string(),
        sourceServiceId: source_service_id.to_string(),
        target,
        required,
    };

    let mut map = inner
        .dependencies
        .lock()
        .map_err(|_| "dependency store lock poisoned".to_string())?;
    let list = map.entry(workspace_id.to_string()).or_default();
    if let Some(existing) = list.iter_mut().find(|d| d.id == dependency.id) {
        // Same edge: update requiredness, keep one entry.
        existing.required = required;
        return Ok(existing.clone());
    }
    if list.len() >= MAX_DEPENDENCIES_PER_WORKSPACE {
        return Err("This workspace already has the maximum number of dependencies.".to_string());
    }
    list.push(dependency.clone());
    Ok(dependency)
}

/// Remove a dependency edge.
pub(crate) fn remove_dependency(
    inner: &crate::workspace::Inner,
    workspace_id: &str,
    dependency_id: &str,
) -> Result<(), String> {
    let mut map = inner
        .dependencies
        .lock()
        .map_err(|_| "dependency store lock poisoned".to_string())?;
    let Some(list) = map.get_mut(workspace_id) else {
        return Err("Workspace has no dependencies.".to_string());
    };
    let before = list.len();
    list.retain(|d| d.id != dependency_id);
    if list.len() == before {
        return Err("Unknown dependency.".to_string());
    }
    Ok(())
}

/// The declared dependencies of one workspace (empty when none).
pub(crate) fn dependencies_of(
    inner: &crate::workspace::Inner,
    workspace_id: &str,
) -> Vec<Dependency> {
    inner
        .dependencies
        .lock()
        .ok()
        .and_then(|map| map.get(workspace_id).cloned())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Evaluation assembly
// ---------------------------------------------------------------------------

/// Evaluate one dependency's live state.
///
/// - `Service` targets: the target's managed state (`Running` → available;
///   `Starting` → starting; anything else → unavailable, or `Conflicted`
///   when the target service's own port has a blocking conflict).
/// - Port-based targets: the port is **listening** in the current table →
///   available (never a health claim), else unavailable.
pub(crate) fn evaluate_dependency(
    dependency: &Dependency,
    listeners: &[crate::discovery::PortListener],
    managed_of: impl Fn(&str) -> Option<ManagedState>,
    conflict_of: impl Fn(&str) -> Option<(ConflictKind, ConflictSeverity)>,
) -> DependencyState {
    match &dependency.target {
        DependencyTarget::Service { service_id } => {
            match managed_of(service_id) {
                Some(ManagedState::Running) => DependencyState::Available,
                Some(ManagedState::Starting) => DependencyState::Starting,
                Some(state) if state.alive() => DependencyState::Unknown,
                _ => {
                    if let Some((kind, severity)) = conflict_of(service_id) {
                        if severity >= ConflictSeverity::Blocking
                            && !matches!(
                                kind,
                                ConflictKind::NoConflict | ConflictKind::AlreadyRunning
                            )
                        {
                            return DependencyState::Conflicted;
                        }
                    }
                    DependencyState::Unavailable
                }
            }
        }
        _ => {
            let port = dependency.target.port().unwrap_or(0);
            if listeners.iter().any(|l| l.port == port) {
                DependencyState::Available
            } else {
                DependencyState::Unavailable
            }
        }
    }
}

/// Build readiness views for every workspace in one pass (one listener
/// enumeration, one owner sampling — cheap over existing in-memory state).
pub(crate) fn evaluate_all(
    inner: &crate::workspace::Inner,
    projects: &crate::project::ProjectEngineState,
) -> Result<Vec<WorkspaceReadinessView>, String> {
    #[cfg(windows)]
    let listeners = crate::discovery::windows::enumerate_tcp_listeners()
        .map_err(|e| format!("listener enumeration failed: {e}"))?;
    #[cfg(not(windows))]
    let listeners: Vec<crate::discovery::PortListener> = Vec::new();

    let context = build_context(&listeners, inner, projects)?;

    let workspaces = inner
        .workspaces
        .lock()
        .map_err(|_| "workspace list lock poisoned".to_string())?;

    let mut views = Vec::with_capacity(workspaces.len());
    for workspace in workspaces.iter() {
        // Managed state per service (latest entry wins).
        let managed_of_service = |service_id: &str| -> Option<ManagedState> {
            inner
                .managed
                .latest_for_service(&workspace.id, service_id)
                .map(|p| p.state)
        };

        // Conflict per service expected port (fresh classification).
        let conflicts: Vec<crate::conflicts::PortConflictReport> = workspace
            .services
            .iter()
            .filter_map(|service| {
                let port = service.expectedPort?;
                let requested_by = RequestedBy {
                    workspaceId: Some(workspace.id.clone()),
                    workspaceName: Some(workspace.name.clone()),
                    serviceId: Some(service.id.clone()),
                    serviceName: Some(service.name.clone()),
                    projectId: Some(workspace.projectRoot.clone()),
                };
                let same_managed_pid = managed_of_service(&service.id)
                    .filter(|state| state.alive())
                    .and_then(|_| {
                        inner
                            .managed
                            .latest_for_service(&workspace.id, &service.id)
                            .filter(|p| p.state.alive())
                            .map(|p| p.rootPid)
                    });
                Some(classify_port(
                    port,
                    requested_by,
                    &listeners,
                    same_managed_pid,
                    &context,
                ))
            })
            .collect();

        let conflict_of_service = |service_id: &str| -> Option<(ConflictKind, ConflictSeverity)> {
            conflicts
                .iter()
                .find(|c| c.requestedBy.serviceId.as_deref() == Some(service_id))
                .map(|c| (c.kind, c.severity))
        };

        let deps = dependencies_of(inner, &workspace.id);
        let dependency_nodes: Vec<readiness::DependencyNode> = deps
            .iter()
            .map(|dependency| readiness::DependencyNode {
                id: dependency.id.clone(),
                source_service_id: dependency.sourceServiceId.clone(),
                target_label: dependency.target.label(),
                required: dependency.required,
                state: evaluate_dependency(
                    dependency,
                    &listeners,
                    |service_id| managed_of_service(service_id),
                    |service_id| conflict_of_service(service_id),
                ),
            })
            .collect();

        // Graph: managed services + their inter-service edges.
        let services_with_ranks: Vec<(String, u8)> = workspace
            .services
            .iter()
            .map(|s| (s.id.clone(), role_rank(s.role)))
            .collect();
        let edges: Vec<graph::ServiceEdge> = deps
            .iter()
            .filter_map(|d| match &d.target {
                DependencyTarget::Service { service_id } => Some(graph::ServiceEdge {
                    source: d.sourceServiceId.clone(),
                    target: service_id.clone(),
                }),
                _ => None,
            })
            .collect();
        let cycle_and_order = graph::topological_order(&services_with_ranks, &edges);
        let cycle = cycle_and_order.as_ref().err().cloned();
        let start_order = cycle_and_order.ok();

        let service_nodes: Vec<readiness::ServiceNode> = workspace
            .services
            .iter()
            .map(|service| {
                let conflict = conflicts
                    .iter()
                    .find(|c| c.requestedBy.serviceId.as_deref() == Some(service.id.as_str()));
                readiness::ServiceNode {
                    id: service.id.clone(),
                    name: service.name.clone(),
                    expected_port: service.expectedPort,
                    managed: managed_of_service(&service.id),
                    conflict_kind: conflict.map(|c| c.kind),
                    conflict_severity: conflict.map(|c| c.severity),
                }
            })
            .collect();

        let readiness = readiness::evaluate(&service_nodes, &dependency_nodes, cycle.as_ref());

        let dependency_views: Vec<DependencyView> = deps
            .iter()
            .zip(dependency_nodes.iter())
            .map(|(dependency, node)| DependencyView {
                id: dependency.id.clone(),
                workspaceId: dependency.workspaceId.clone(),
                sourceServiceId: dependency.sourceServiceId.clone(),
                target: dependency.target.clone(),
                targetLabel: dependency.target.label(),
                required: dependency.required,
                state: node.state,
            })
            .collect();

        let blocking_conflicts: Vec<crate::conflicts::PortConflictReport> = conflicts
            .into_iter()
            .filter(|c| {
                matches!(
                    c.severity,
                    ConflictSeverity::Blocking | ConflictSeverity::Potential
                ) && c.kind != ConflictKind::AlreadyRunning
            })
            .collect();

        views.push(WorkspaceReadinessView {
            workspaceId: workspace.id.clone(),
            workspaceName: workspace.name.clone(),
            status: readiness.status,
            issues: readiness.issues,
            dependencies: dependency_views,
            conflicts: blocking_conflicts,
            startOrder: start_order,
        });
    }
    Ok(views)
}

/// Role rank mirroring `workspace::rules::Role::order_rank` (roles are
/// Copy, but readiness inputs carry the plain role value).
fn role_rank(role: Role) -> u8 {
    role.order_rank()
}

/// Topological start order for one workspace's managed services (used by
/// the workspace start command). Falls back to `Err` on cycles.
pub(crate) fn start_order(
    inner: &crate::workspace::Inner,
    workspace_id: &str,
) -> Result<Vec<String>, DependencyCycle> {
    let workspaces = inner
        .workspaces
        .lock()
        .ok()
        .ok_or_else(|| DependencyCycle { path: Vec::new() })?;
    let Some(workspace) = workspaces.iter().find(|w| w.id == workspace_id) else {
        return Ok(Vec::new());
    };
    let deps = dependencies_of(inner, workspace_id);
    let services_with_ranks: Vec<(String, u8)> = workspace
        .services
        .iter()
        .map(|s| (s.id.clone(), s.role.order_rank()))
        .collect();
    let edges: Vec<graph::ServiceEdge> = deps
        .iter()
        .filter_map(|d| match &d.target {
            DependencyTarget::Service { service_id } => Some(graph::ServiceEdge {
                source: d.sourceServiceId.clone(),
                target: service_id.clone(),
            }),
            _ => None,
        })
        .collect();
    graph::topological_order(&services_with_ranks, &edges)
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Readiness + conflicts + dependencies for every workspace (one poll).
#[tauri::command]
pub(crate) async fn get_workspaces_readiness(
    state: tauri::State<'_, WorkspaceEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
) -> Result<Vec<WorkspaceReadinessView>, String> {
    let inner = state.engine_inner();
    let projects = std::sync::Arc::clone(&projects.cache);
    tauri::async_runtime::spawn_blocking(move || {
        let projects_handle = crate::project::ProjectEngineState { cache: projects };
        evaluate_all(&inner, &projects_handle)
    })
    .await
    .map_err(|e| format!("readiness task join error: {e}"))?
}

/// Targets the UI may offer when adding a dependency (workspace services
/// only; external candidates come from the frontend's live snapshot).
#[tauri::command]
pub(crate) async fn list_dependency_targets(
    state: tauri::State<'_, WorkspaceEngineState>,
    workspace_id: String,
) -> Result<Vec<TargetOption>, String> {
    let inner = state.engine_inner();
    tauri::async_runtime::spawn_blocking(move || {
        let workspaces = inner
            .workspaces
            .lock()
            .map_err(|_| "workspace list lock poisoned".to_string())?;
        let workspace = workspaces
            .iter()
            .find(|w| w.id == workspace_id)
            .ok_or("Workspace not found.")?;
        Ok(workspace
            .services
            .iter()
            .map(|s| TargetOption::Service {
                serviceId: s.id.clone(),
                name: s.name.clone(),
                expectedPort: s.expectedPort,
            })
            .collect())
    })
    .await
    .map_err(|e| format!("targets task join error: {e}"))?
}

/// Add a dependency (user-confirmed mapping; validated server-side).
#[tauri::command]
pub(crate) async fn add_workspace_dependency(
    state: tauri::State<'_, WorkspaceEngineState>,
    workspace_id: String,
    source_service_id: String,
    target: DependencyTarget,
    required: Option<bool>,
) -> Result<DependencyView, String> {
    let inner = state.engine_inner();
    tauri::async_runtime::spawn_blocking(move || {
        let dependency = add_dependency(
            &inner,
            &workspace_id,
            &source_service_id,
            target,
            required.unwrap_or(true),
        )?;
        Ok(DependencyView {
            id: dependency.id,
            workspaceId: dependency.workspaceId,
            sourceServiceId: dependency.sourceServiceId,
            targetLabel: dependency.target.label(),
            target: dependency.target,
            required: dependency.required,
            state: DependencyState::Unknown,
        })
    })
    .await
    .map_err(|e| format!("dependency task join error: {e}"))?
}

/// Remove a dependency edge.
#[tauri::command]
pub(crate) async fn remove_workspace_dependency(
    state: tauri::State<'_, WorkspaceEngineState>,
    workspace_id: String,
    dependency_id: String,
) -> Result<(), String> {
    let inner = state.engine_inner();
    tauri::async_runtime::spawn_blocking(move || {
        remove_dependency(&inner, &workspace_id, &dependency_id)
    })
    .await
    .map_err(|e| format!("dependency task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ports::{IpVersion, ListenerState, Protocol};

    fn listener(port: u16) -> crate::discovery::PortListener {
        crate::discovery::PortListener {
            protocol: Protocol::Tcp,
            ipVersion: IpVersion::V4,
            localAddress: "127.0.0.1".to_string(),
            port,
            pid: 1,
            state: ListenerState::Listen,
        }
    }

    fn external(port: u16) -> DependencyTarget {
        DependencyTarget::ExternalService { name: "PostgreSQL".to_string(), port }
    }

    #[test]
    fn port_dependency_is_available_when_listening() {
        let dependency = Dependency {
            id: "d".to_string(),
            workspaceId: "ws".to_string(),
            sourceServiceId: "svc".to_string(),
            target: external(5432),
            required: true,
        };
        let listeners = vec![listener(5432)];
        let state = evaluate_dependency(&dependency, &listeners, |_| None, |_| None);
        assert_eq!(state, DependencyState::Available);
    }

    #[test]
    fn port_dependency_is_unavailable_when_silent() {
        let dependency = Dependency {
            id: "d".to_string(),
            workspaceId: "ws".to_string(),
            sourceServiceId: "svc".to_string(),
            target: external(5432),
            required: true,
        };
        let state = evaluate_dependency(&dependency, &[], |_| None, |_| None);
        assert_eq!(state, DependencyState::Unavailable);
    }

    #[test]
    fn service_dependency_follows_managed_state() {
        let dependency = Dependency {
            id: "d".to_string(),
            workspaceId: "ws".to_string(),
            sourceServiceId: "svc".to_string(),
            target: DependencyTarget::Service { service_id: "svc-2".to_string() },
            required: true,
        };
        let running =
            evaluate_dependency(&dependency, &[], |id| (id == "svc-2").then_some(ManagedState::Running), |_| None);
        assert_eq!(running, DependencyState::Available);

        let starting = evaluate_dependency(&dependency, &[], |id| {
            (id == "svc-2").then_some(ManagedState::Starting)
        }, |_| None);
        assert_eq!(starting, DependencyState::Starting);

        let stopped =
            evaluate_dependency(&dependency, &[], |id| (id == "svc-2").then_some(ManagedState::Stopped), |_| None);
        assert_eq!(stopped, DependencyState::Unavailable);
    }

    #[test]
    fn service_dependency_is_conflicted_when_target_port_is_blocked() {
        let dependency = Dependency {
            id: "d".to_string(),
            workspaceId: "ws".to_string(),
            sourceServiceId: "svc".to_string(),
            target: DependencyTarget::Service { service_id: "svc-2".to_string() },
            required: true,
        };
        let state = evaluate_dependency(
            &dependency,
            &[],
            |id| (id == "svc-2").then_some(ManagedState::Stopped),
            |id| (id == "svc-2").then_some((ConflictKind::OtherProject, ConflictSeverity::Blocking)),
        );
        assert_eq!(state, DependencyState::Conflicted);
    }

    #[test]
    fn dependency_ids_are_deterministic_and_distinct() {
        let a = opaque_dependency_id("ws\u{0}svc\u{0}target-a");
        let b = opaque_dependency_id("ws\u{0}svc\u{0}target-b");
        let a2 = opaque_dependency_id("ws\u{0}svc\u{0}target-a");
        assert_eq!(a, a2);
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
