//! # Service & Framework Intelligence (Phase 3)
//!
//! Classifies inspected processes into developer-facing identities
//! (PostgreSQL, Vite, Flask, Ollama, …) from **evidence** — executable
//! names, executable paths, and (new in Phase 3) process command lines.
//!
//! - [`rules`] holds the pure, deterministic, fully unit-tested detector.
//! - This facade attaches the per-PID identity to the discovery response.
//!
//! ## Product principle
//!
//! Never claim a framework or service identity without sufficient evidence.
//! "Node.js" beats an incorrect "Next.js"; port numbers are *never* strong
//! evidence. Every identity carries explicit [`rules::Confidence`] and the
//! [`rules::Evidence`] that produced it.

pub(crate) mod rules;

use serde::Serialize;

pub(crate) use rules::{detect_service, ProcessEvidence, ServiceIdentity};

/// A [`ServiceIdentity`] attached to the PID it belongs to.
///
/// Identity is fundamentally PID-based: multiple listener rows of one
/// process share one identity (the frontend keys it `serviceByPid`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)] // field names mirror the frontend contract
pub(crate) struct PidServiceIdentity {
    /// PID the identity belongs to (same PID space as `processes`).
    pub pid: u32,
    /// The classified identity.
    #[serde(flatten)]
    pub identity: ServiceIdentity,
}

/// Classify every process of a discovery cycle.
///
/// Pure derivation over already-collected data: no extra Windows calls, so
/// cycle cost stays at the Phase 2 scale. Ports are passed per PID as *weak*
/// evidence only (the rules may inspect them but never grant confidence).
pub(crate) fn classify_processes(
    processes: &[crate::process::ProcessInfo],
    listeners: &[crate::discovery::PortListener],
) -> Vec<PidServiceIdentity> {
    // Group listener ports by PID — one lookup per PID, shared by all of the
    // PID's listener rows.
    let mut ports_by_pid: std::collections::HashMap<u32, Vec<u16>> =
        std::collections::HashMap::with_capacity(processes.len());
    for listener in listeners {
        ports_by_pid
            .entry(listener.pid)
            .or_default()
            .push(listener.port);
    }

    processes
        .iter()
        .map(|process| {
            let evidence = ProcessEvidence::from_process(
                process,
                ports_by_pid.remove(&process.pid).unwrap_or_default(),
            );
            PidServiceIdentity {
                pid: process.pid,
                identity: detect_service(&evidence),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessInfo;

    fn process(pid: u32, name: &str, command_line: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: Some(name.to_string()),
            executablePath: Some(format!(r"C:\Program Files\{name}")),
            startedAt: Some(1_700_000_000_000),
            memoryBytes: Some(10_000_000),
            cpuPercent: None,
            commandLine: command_line.map(str::to_string),
            accessible: true,
        }
    }

    fn listener(port: u16, pid: u32) -> crate::discovery::PortListener {
        crate::discovery::PortListener {
            protocol: crate::discovery::ports::Protocol::Tcp,
            ipVersion: crate::discovery::ports::IpVersion::V4,
            localAddress: "127.0.0.1".to_string(),
            port,
            pid,
            state: crate::discovery::ports::ListenerState::Listen,
        }
    }

    #[test]
    fn classification_covers_every_process_exactly_once() {
        let processes = vec![process(1, "postgres.exe", None), process(2, "node.exe", None)];
        let listeners = vec![
            listener(5432, 1),
            listener(5433, 1), // same PID, second port — one identity
            listener(3000, 2),
        ];
        let services = classify_processes(&processes, &listeners);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].pid, 1);
        assert_eq!(services[1].pid, 2);
    }

    #[test]
    fn multiple_ports_of_one_pid_share_one_identity() {
        let processes = vec![process(7, "node.exe", Some("node .bin/vite"))];
        let listeners = vec![listener(1420, 7), listener(1421, 7)];
        let services = classify_processes(&processes, &listeners);
        assert_eq!(services[0].identity.kind, rules::ServiceKind::Vite);
        // Both ports were visible as evidence input, identity still singular.
        assert_eq!(services.len(), 1);
    }

    #[test]
    fn inaccessible_process_gets_unknown_identity() {
        let processes = vec![ProcessInfo::inaccessible(1068)];
        let listeners = vec![listener(135, 1068)];
        let services = classify_processes(&processes, &listeners);
        assert_eq!(services[0].identity.kind, rules::ServiceKind::Unknown);
        assert_eq!(services[0].identity.displayName, "Unavailable");
    }

    #[test]
    fn serialized_shape_is_frontend_ready() {
        let processes = vec![process(9, "postgres.exe", None)];
        let listeners = vec![listener(5432, 9)];
        let services = classify_processes(&processes, &listeners);
        let json = serde_json::to_string(&services).expect("serialize");
        assert!(json.contains("\"pid\":9"));
        assert!(json.contains("\"displayName\":\"PostgreSQL\""));
        assert!(json.contains("\"confidence\":\"exact\""));
        assert!(json.contains("\"category\":\"database\""));
    }
}
