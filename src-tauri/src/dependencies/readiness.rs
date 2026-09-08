//! Pure readiness evaluation: managed service states + dependency states +
//! port conflicts → workspace readiness with structured root-cause issues.
//!
//! # Status precedence (documented, spec §21)
//!
//! 1. `Error` — a dependency cycle, a `StartFailed`, or a `StopTimeout`.
//! 2. `Conflict` — a required service cannot launch because its requested
//!    port is occupied by a `Blocking` verdict.
//! 3. `Blocked` — a required dependency is `Unavailable`/`Conflicted`.
//! 4. `Starting` — any managed service is currently starting/stopping.
//! 5. `Stopped` — no managed service is running (and nothing is wrong).
//! 6. `Partial` — some managed services run, others do not.
//! 7. `Ready` — everything required runs, every required dependency is
//!    available, no blocking conflict.
//!
//! # Honesty rules
//!
//! - An open TCP port means *listening*, never *healthy* (spec §31): the
//!   only evidence in Phase 7 is the listener table, so dependency states
//!   claim availability, not health.
//! - External dependencies are never "stopped" merely because LocalStack
//!   does not manage them; unavailable is the honest word.
//! - `LISTENING ≠ HEALTHY` is visible in the issue text.

use serde::Serialize;

use crate::workspace::rules::ManagedState;

/// Runtime state of one dependency (spec §19). `Available` claims only that
/// the target endpoint is *listening* — health checks are a later phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DependencyState {
    Available,
    Unavailable,
    /// Target service exists but is currently starting.
    Starting,
    /// Reserved for real health evidence (unused in Phase 7; never emitted).
    #[allow(dead_code)]
    Unhealthy,
    /// The target could not be checked at all.
    Unknown,
    /// The port is held by a different project (see the conflict engine).
    Conflicted,
}

/// Severity of one readiness issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IssueSeverity {
    Info,
    Warning,
    Error,
}

/// A structured root-cause issue (spec §22). Stable identities ride in the
/// codes + ids so the history layer can emit transitions, not poll spam.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ReadinessIssue {
    /// Machine-readable code, e.g. `PORT_OCCUPIED`, `DEPENDENCY_UNAVAILABLE`.
    pub code: String,
    pub severity: IssueSeverity,
    pub title: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sourceServiceId: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencyId: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// One workspace service as seen by the readiness engine.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ServiceNode {
    pub id: String,
    pub name: String,
    pub expected_port: Option<u16>,
    /// Latest managed state, when the service was ever launched.
    pub managed: Option<ManagedState>,
    /// Conflict verdict for the expected port (from the conflict engine).
    pub conflict_kind: Option<crate::conflicts::rules::ConflictKind>,
    pub conflict_severity: Option<crate::conflicts::rules::ConflictSeverity>,
}

/// One evaluated dependency edge.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DependencyNode {
    pub id: String,
    /// Workspace service that declares the dependency.
    pub source_service_id: String,
    /// Human-facing target label (`PostgreSQL :5432`, service name…).
    pub target_label: String,
    pub required: bool,
    pub state: DependencyState,
}

/// Derived workspace readiness (status + root causes).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct WorkspaceReadiness {
    /// `ready | partial | blocked | conflict | starting | stopped | error`
    pub status: String,
    pub issues: Vec<ReadinessIssue>,
}

/// Evaluate workspace readiness. Pure over the inputs.
pub(crate) fn evaluate(
    services: &[ServiceNode],
    dependencies: &[DependencyNode],
    cycle: Option<&crate::dependencies::graph::DependencyCycle>,
) -> WorkspaceReadiness {
    let mut issues: Vec<ReadinessIssue> = Vec::new();

    // --- issues -----------------------------------------------------------

    if let Some(cycle) = cycle {
        issues.push(ReadinessIssue {
            code: "DEPENDENCY_CYCLE".to_string(),
            severity: IssueSeverity::Error,
            title: "Dependency cycle".to_string(),
            message: format!(
                "The dependency graph contains a cycle ({}). Ordered start is refused until it is fixed.",
                cycle.path.join(" → ")
            ),
            sourceServiceId: None,
            dependencyId: None,
            port: None,
        });
    }

    for service in services {
        match &service.managed {
            Some(ManagedState::StartFailed { .. }) => issues.push(ReadinessIssue {
                code: "START_FAILED".to_string(),
                severity: IssueSeverity::Error,
                title: "Startup failed".to_string(),
                message: format!("{} exited during startup.", service.name),
                sourceServiceId: Some(service.id.clone()),
                dependencyId: None,
                port: None,
            }),
            Some(ManagedState::StopTimeout) => issues.push(ReadinessIssue {
                code: "STOP_TIMEOUT".to_string(),
                severity: IssueSeverity::Error,
                title: "Stop timed out".to_string(),
                message: format!("{} did not exit within the graceful window.", service.name),
                sourceServiceId: Some(service.id.clone()),
                dependencyId: None,
                port: None,
            }),
            Some(ManagedState::Degraded) => issues.push(ReadinessIssue {
                code: "START_TIMEOUT".to_string(),
                severity: IssueSeverity::Warning,
                title: "Readiness timeout".to_string(),
                message: format!(
                    "{} is alive but its expected port never appeared (readiness window elapsed).",
                    service.name
                ),
                sourceServiceId: Some(service.id.clone()),
                dependencyId: None,
                port: service.expected_port,
            }),
            _ => {}
        }

        // Port conflicts on a not-running service block the launch.
        let not_running = !service.managed.as_ref().is_some_and(|s| s.alive());
        if let (true, Some(port), Some(kind), Some(severity)) =
            (not_running, service.expected_port, service.conflict_kind, service.conflict_severity)
        {
            match (kind, severity) {
                (crate::conflicts::rules::ConflictKind::AlreadyRunning, _) => {}
                (crate::conflicts::rules::ConflictKind::NoConflict, _) => {}
                (kind, crate::conflicts::rules::ConflictSeverity::Blocking) => {
                    issues.push(ReadinessIssue {
                        code: "PORT_OCCUPIED".to_string(),
                        severity: IssueSeverity::Error,
                        title: "Port occupied".to_string(),
                        message: format!(
                            "{} cannot start: port {} is {}.",
                            service.name,
                            port,
                            match kind {
                                crate::conflicts::rules::ConflictKind::SameProjectExternal => {
                                    "held by an external process of this same project".to_string()
                                }
                                crate::conflicts::rules::ConflictKind::SameProjectManaged => {
                                    "held by a managed service of this workspace".to_string()
                                }
                                crate::conflicts::rules::ConflictKind::OtherProject => {
                                    "owned by another project".to_string()
                                }
                                crate::conflicts::rules::ConflictKind::UnknownOwner => {
                                    "occupied (owner identity unavailable)".to_string()
                                }
                                crate::conflicts::rules::ConflictKind::ReservedOrUnverifiable => {
                                    "held by a Windows-reserved socket".to_string()
                                }
                                _ => "occupied".to_string(),
                            }
                        ),
                        sourceServiceId: Some(service.id.clone()),
                        dependencyId: None,
                        port: Some(port),
                    });
                }
                (kind, crate::conflicts::rules::ConflictSeverity::Potential) => {
                    issues.push(ReadinessIssue {
                        code: "PORT_RISK".to_string(),
                        severity: IssueSeverity::Warning,
                        title: "Possible port conflict".to_string(),
                        message: format!(
                            "{} may fail to bind port {} ({}; bind semantics cannot be verified).",
                            service.name,
                            port,
                            match kind {
                                crate::conflicts::rules::ConflictKind::DualStackEquivalent => {
                                    "IPv6 wildcard dual-stack"
                                }
                                _ => "bind ambiguity",
                            }
                        ),
                        sourceServiceId: Some(service.id.clone()),
                        dependencyId: None,
                        port: Some(port),
                    });
                }
                _ => {}
            }
        }
    }

    for dependency in dependencies {
        match (dependency.required, dependency.state) {
            (_, DependencyState::Available) => {}
            (_, DependencyState::Starting) => issues.push(ReadinessIssue {
                code: "DEPENDENCY_STARTING".to_string(),
                severity: IssueSeverity::Info,
                title: "Dependency starting".to_string(),
                message: format!(
                    "{} is starting (needed by {}).",
                    dependency.target_label, dependency.source_service_id
                ),
                sourceServiceId: Some(dependency.source_service_id.clone()),
                dependencyId: Some(dependency.id.clone()),
                port: None,
            }),
            (true, DependencyState::Unavailable | DependencyState::Conflicted) => {
                issues.push(ReadinessIssue {
                    code: if dependency.state == DependencyState::Conflicted {
                        "DEPENDENCY_CONFLICTED".to_string()
                    } else {
                        "DEPENDENCY_UNAVAILABLE".to_string()
                    },
                    severity: IssueSeverity::Error,
                    title: "Dependency unavailable".to_string(),
                    message: format!(
                        "{} requires {} — currently not listening. (A listening port is not a health claim; LocalStack does not fake health checks.)",
                        dependency.source_service_id, dependency.target_label
                    ),
                    sourceServiceId: Some(dependency.source_service_id.clone()),
                    dependencyId: Some(dependency.id.clone()),
                    port: None,
                });
            }
            (false, DependencyState::Unavailable | DependencyState::Conflicted) => {
                issues.push(ReadinessIssue {
                    code: "DEPENDENCY_UNAVAILABLE".to_string(),
                    severity: IssueSeverity::Warning,
                    title: "Optional dependency down".to_string(),
                    message: format!(
                        "{}'s optional dependency {} is not listening; the service may run degraded.",
                        dependency.source_service_id, dependency.target_label
                    ),
                    sourceServiceId: Some(dependency.source_service_id.clone()),
                    dependencyId: Some(dependency.id.clone()),
                    port: None,
                });
            }
            (_, DependencyState::Unknown) => issues.push(ReadinessIssue {
                code: "DEPENDENCY_UNKNOWN".to_string(),
                severity: IssueSeverity::Warning,
                title: "Dependency unknown".to_string(),
                message: format!(
                    "The state of {} (needed by {}) could not be checked.",
                    dependency.target_label, dependency.source_service_id
                ),
                sourceServiceId: Some(dependency.source_service_id.clone()),
                dependencyId: Some(dependency.id.clone()),
                port: None,
            }),
            (_, DependencyState::Unhealthy) => {
                // Never emitted in Phase 7 (no health engine yet).
            }
        }
    }

    // --- status (documented precedence) -------------------------------------

    let has_cycle = cycle.is_some() || issues.iter().any(|i| i.code == "DEPENDENCY_CYCLE");
    let lifecycle_error = issues
        .iter()
        .any(|i| matches!(i.code.as_str(), "START_FAILED" | "STOP_TIMEOUT") && i.severity == IssueSeverity::Error);
    let conflict = issues.iter().any(|i| i.code == "PORT_OCCUPIED");
    let blocked = issues.iter().any(|i| {
        i.severity == IssueSeverity::Error
            && matches!(i.code.as_str(), "DEPENDENCY_UNAVAILABLE" | "DEPENDENCY_CONFLICTED")
    });
    let starting = services
        .iter()
        .any(|s| matches!(s.managed, Some(ManagedState::Starting) | Some(ManagedState::Stopping)));
    let any_running = services
        .iter()
        .any(|s| matches!(s.managed, Some(ManagedState::Running | ManagedState::Degraded)));
    let any_service = !services.is_empty();

    let status = if has_cycle || lifecycle_error {
        "error"
    } else if conflict {
        "conflict"
    } else if blocked {
        "blocked"
    } else if starting {
        "starting"    } else if any_running && any_service && services.iter().all(|s| matches!(s.managed, Some(ManagedState::Running))) {
        // Every managed service fully Running — Degraded deliberately fails
        // this check and falls through to `partial`.
        "ready"
    } else if any_running {
        "partial"
    } else {
        "stopped"
    };

    WorkspaceReadiness { status: status.to_string(), issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflicts::rules::ConflictKind;
    use crate::dependencies::graph::DependencyCycle;

    fn service(id: &str, port: Option<u16>, managed: Option<ManagedState>) -> ServiceNode {
        ServiceNode {
            id: id.to_string(),
            name: id.to_string(),
            expected_port: port,
            managed,
            conflict_kind: None,
            conflict_severity: None,
        }
    }

    fn dep(id: &str, source: &str, required: bool, state: DependencyState) -> DependencyNode {
        DependencyNode {
            id: id.to_string(),
            source_service_id: source.to_string(),
            target_label: format!("target-{id}"),
            required,
            state,
        }
    }

    #[test]
    fn everything_running_is_ready() {
        let services = vec![service("a", None, Some(ManagedState::Running))];
        let report = evaluate(&services, &[], None);
        assert_eq!(report.status, "ready");
        assert!(report.issues.is_empty());
    }

    #[test]
    fn required_dependency_missing_blocks() {
        let services = vec![service("a", None, Some(ManagedState::Running))];
        let deps = vec![dep("d1", "a", true, DependencyState::Unavailable)];
        let report = evaluate(&services, &deps, None);
        assert_eq!(report.status, "blocked");
        assert!(report.issues.iter().any(|i| i.code == "DEPENDENCY_UNAVAILABLE"));
    }

    #[test]
    fn optional_dependency_missing_does_not_block() {
        let services = vec![service("a", None, Some(ManagedState::Running))];
        let deps = vec![dep("d1", "a", false, DependencyState::Unavailable)];
        let report = evaluate(&services, &deps, None);
        assert_eq!(report.status, "ready");
        assert!(report.issues.iter().any(|i| i.severity == IssueSeverity::Warning));
    }

    #[test]
    fn occupied_port_of_stopped_service_is_conflict() {
        let mut node = service("a", Some(3000), None);
        node.conflict_kind = Some(ConflictKind::OtherProject);
        node.conflict_severity = Some(crate::conflicts::rules::ConflictSeverity::Blocking);
        let report = evaluate(&[node], &[], None);
        assert_eq!(report.status, "conflict");
        assert!(report.issues.iter().any(|i| i.code == "PORT_OCCUPIED"));
    }

    #[test]
    fn already_running_is_not_reported_as_conflict() {
        let mut node = service("a", Some(1420), Some(ManagedState::Running));
        node.conflict_kind = Some(ConflictKind::AlreadyRunning);
        node.conflict_severity = Some(crate::conflicts::rules::ConflictSeverity::Info);
        let report = evaluate(&[node], &[], None);
        assert_eq!(report.status, "ready");
        assert!(report.issues.is_empty());
    }

    #[test]
    fn mixed_running_states_are_partial() {
        let services = vec![
            service("a", None, Some(ManagedState::Running)),
            service("b", None, Some(ManagedState::Stopped)),
        ];
        assert_eq!(evaluate(&services, &[], None).status, "partial");
    }

    #[test]
    fn nothing_running_is_stopped() {
        let services = vec![
            service("a", None, Some(ManagedState::Stopped)),
            service("b", None, None),
        ];
        assert_eq!(evaluate(&services, &[], None).status, "stopped");
    }

    #[test]
    fn starting_is_starting() {
        let services = vec![service("a", None, Some(ManagedState::Starting))];
        assert_eq!(evaluate(&services, &[], None).status, "starting");
    }

    #[test]
    fn start_failure_is_error_and_outranks_conflict() {
        let mut node = service("a", Some(3000), Some(ManagedState::StartFailed { exit_code: Some(1) }));
        node.conflict_kind = Some(ConflictKind::OtherProject);
        node.conflict_severity = Some(crate::conflicts::rules::ConflictSeverity::Blocking);
        let report = evaluate(&[node], &[], None);
        assert_eq!(report.status, "error");
    }

    #[test]
    fn degraded_service_is_not_ready_but_partial() {
        let services = vec![service("a", Some(3000), Some(ManagedState::Degraded))];
        let report = evaluate(&services, &[], None);
        assert_eq!(report.status, "partial");
        assert!(report.issues.iter().any(|i| i.code == "START_TIMEOUT"));
    }

    #[test]
    fn cycle_is_error_with_the_path_in_the_message() {
        let cycle = DependencyCycle { path: vec!["a".into(), "b".into(), "a".into()] };
        let report = evaluate(&[service("a", None, None)], &[], Some(&cycle));
        assert_eq!(report.status, "error");
        let issue = report
            .issues
            .iter()
            .find(|i| i.code == "DEPENDENCY_CYCLE")
            .expect("cycle issue");
        assert!(issue.message.contains("a → b → a"));
    }

    #[test]
    fn conflicted_dependency_blocks() {
        let services = vec![service("a", None, Some(ManagedState::Running))];
        let deps = vec![dep("d1", "a", true, DependencyState::Conflicted)];
        let report = evaluate(&services, &deps, None);
        assert_eq!(report.status, "blocked");
        assert!(report.issues.iter().any(|i| i.code == "DEPENDENCY_CONFLICTED"));
    }

    #[test]
    fn precedence_error_over_conflict_over_blocked() {
        // Blocked dependency + starting + conflict + lifecycle error → error.
        let mut conflict_node = service("a", Some(3000), Some(ManagedState::StartFailed { exit_code: None }));
        conflict_node.conflict_kind = Some(ConflictKind::OtherProject);
        conflict_node.conflict_severity = Some(crate::conflicts::rules::ConflictSeverity::Blocking);
        let services = vec![
            conflict_node,
            service("b", Some(8000), None),
        ];
        let deps = vec![dep("d1", "b", true, DependencyState::Unavailable)];
        let report = evaluate(&services, &deps, None);
        assert_eq!(report.status, "error");
    }

    #[test]
    fn multiple_issues_are_all_reported() {
        let services = vec![service("a", None, Some(ManagedState::Degraded))];
        let deps = vec![
            dep("d1", "a", true, DependencyState::Unavailable),
            dep("d2", "a", false, DependencyState::Unavailable),
        ];
        let report = evaluate(&services, &deps, None);
        assert!(report.issues.len() >= 3, "{}", report.issues.len());
    }

    #[test]
    fn listening_is_explicitly_not_health() {
        let services = vec![service("a", None, Some(ManagedState::Running))];
        let deps = vec![dep("d1", "a", true, DependencyState::Unavailable)];
        let report = evaluate(&services, &deps, None);
        let issue = report
            .issues
            .iter()
            .find(|i| i.code == "DEPENDENCY_UNAVAILABLE")
            .expect("issue");
        assert!(issue.message.contains("not a health claim"));
    }
}
