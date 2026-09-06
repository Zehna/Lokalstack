//! # Discovery
//!
//! Engines that observe what is running on this machine, read-only.
//!
//! ## Phase 1 — Port Discovery (implemented)
//!
//! `ports.rs` holds the pure logic (DTOs, byte-order conversion, IPv4/IPv6
//! address decoding, normalization) and `windows.rs` holds the narrow,
//! unsafe FFI boundary to `GetExtendedTcpTable`.
//!
//! ## Phase 2 — Process Intelligence (implemented in `crate::process`)
//!
//! The `get_port_listeners` Tauri command now lives in `crate::process`,
//! because a refresh cycle is one pipeline: listeners → unique PIDs →
//! process inspection → CPU delta merge. This module keeps the shared
//! response DTO ([`PortListenersResponse`]) and the port-discovery engine.
//!
//! The engines never probe port ranges, never parse `netstat` output, and
//! never require elevated privileges.

pub(crate) mod ports;

#[cfg(windows)]
pub(crate) mod windows;

use serde::Serialize;

pub(crate) use ports::PortListener;

/// Public response DTO for the `get_port_listeners` Tauri command.
///
/// Wraps the listener list with the fields the frontend needs to render
/// loading/error/empty states honestly, plus the Phase 2 process
/// intelligence for the listeners' owning PIDs.
#[derive(Debug, Clone, Serialize)]
// Field names deliberately mirror the frontend contract — camelCase JSON.
#[allow(non_snake_case)]
pub(crate) struct PortListenersResponse {
    /// All TCP listeners (IPv4 + IPv6), deduplicated and sorted by port.
    pub listeners: Vec<PortListener>,
    /// Process metadata for every unique PID in the listener list.
    pub processes: Vec<crate::process::ProcessInfo>,
    /// Service/framework identity per PID (Phase 3 intelligence layer).
    pub services: Vec<crate::intelligence::PidServiceIdentity>,
    /// Unique projects resolved from process evidence (Phase 4).
    pub projects: Vec<crate::project::ProjectIdentity>,
    /// PID → project id (Phase 4). Many PIDs may share one project.
    pub projectLinks: Vec<crate::project::PidProjectLink>,
    /// Control capability + browser URLs per PID (Phase 5).
    pub controls: Vec<crate::control::PidControl>,
    /// Unix epoch milliseconds at which the snapshot was taken.
    pub capturedAt: u64,
    /// Wall-clock duration of the full discovery cycle, in milliseconds.
    pub durationMs: u64,
}

#[cfg(test)]
mod tests {
    /// LIVE-SYSTEM TEST (#[ignore]d so normal `cargo test` stays hermetic).
    ///
    /// Runs the full Phase 2 production path exactly as the frontend calls
    /// it: two full discovery cycles (so CPU percentages are computed on the
    /// second one), including serde JSON serialization, timing, and an
    /// access-denied/restricted-process observation. Run explicitly with:
    ///
    ///     cargo test -- --ignored --nocapture
    ///
    /// Assertions are limited to invariants that must hold for any system
    /// state, so the test never fails because a dev server came or went.
    #[test]
    #[ignore = "touches the live system; run manually for verification"]
    fn live_two_cycle_snapshot_is_invariant_valid() {
        let state = crate::process::ProcessEngineState::default();
        let project_state = crate::project::ProjectEngineState::default();
        let control_state = crate::control::ControlEngineState::default();
        let cache = state.cache.lock().expect("cache lock");
        let mut project_cache = project_state.cache.lock().expect("project cache lock");

        // ---- Cycle 1: establishes the CPU baseline ----------------------
        let cycle1 = crate::process::run_discovery_cycle(&cache, &mut project_cache, false, &control_state.registry)
            .expect("live discovery must not fail on Windows");
        let response = &cycle1.response;

        assert!(!response.listeners.is_empty(), "Windows always has listeners");
        assert!(response.capturedAt > 0 && response.durationMs > 0);

        // Every accessible process is merge-ready; every PID in the
        // listener list must appear exactly once in the process list.
        let unique_pids: std::collections::HashSet<u32> =
            response.listeners.iter().map(|l| l.pid).collect();
        let process_pids: std::collections::HashSet<u32> =
            response.processes.iter().map(|p| p.pid).collect();
        assert_eq!(unique_pids, process_pids, "process list must cover exactly the listener PIDs");

        let accessible: Vec<_> = response
            .processes
            .iter()
            .filter(|p| p.accessible)
            .collect();
        for process in &accessible {
            assert!(process.name.is_some(), "accessible process must have a name");
            assert!(process.startedAt.is_some(), "accessible process must have a start time");
            assert!(process.memoryBytes.is_some_and(|m| m > 0), "working set must be positive");
            assert!(process.cpuPercent.is_none(), "first cycle must not fabricate CPU");
        }
        println!(
            "cycle 1: {} listeners, {} unique PIDs ({} accessible), {} ms",
            response.listeners.len(),
            unique_pids.len(),
            accessible.len(),
            response.durationMs,
        );

        // Explicit port-1420 report: what Windows' own dev-server port shows.
        match response.listeners.iter().find(|l| l.port == 1420) {
            Some(listener) => {
                let process = response
                    .processes
                    .iter()
                    .find(|p| p.pid == listener.pid);
                let service = response
                    .services
                    .iter()
                    .find(|s| s.pid == listener.pid);
                println!(
                    "PORT1420: engine → {} IPv{} {} PID {} name={:?} mem={:?} cpu={:?} accessible={} → service={:?} confidence={:?} evidence={:?} cmdline={}",
                    listener.port,
                    match listener.ipVersion {
                        super::ports::IpVersion::V4 => "4",
                        super::ports::IpVersion::V6 => "6",
                    },
                    listener.localAddress,
                    listener.pid,
                    process.and_then(|p| p.name.as_deref()),
                    process.and_then(|p| p.memoryBytes),
                    process.and_then(|p| p.cpuPercent),
                    process.map(|p| p.accessible).unwrap_or(false),
                    service.map(|s| &s.identity.displayName),
                    service.map(|s| &s.identity.confidence),
                    service.map(|s| &s.identity.evidence),
                    process
                        .and_then(|p| p.commandLine.as_deref())
                        .map(|c| {
                            if c.len() > 160 {
                                format!("{}…", &c[..160])
                            } else {
                                c.to_string()
                            }
                        })
                        .unwrap_or_else(|| "(unreadable)".to_string()),
                );
            }
            None => println!("PORT1420: not listening at engine snapshot time"),
        }

        // Explicit project report for port 1420 (Phase 4).
        match response.listeners.iter().find(|l| l.port == 1420) {
            Some(listener) => {
                match response
                    .projectLinks
                    .iter()
                    .find(|l| l.pid == listener.pid)
                {
                    Some(link) => {
                        let project = response
                            .projects
                            .iter()
                            .find(|p| p.id == link.projectId)
                            .expect("link must reference a project in the response");
                        println!(
                            "PROJECT1420: pid={} → name={:?} root={:?} kind={:?} git(branch={:?}, repo={}) pm={:?} start={:?} confidence={:?} evidence={}",
                            listener.pid,
                            project.name,
                            project.rootPath,
                            project.kind,
                            project.git.branch,
                            project.git.isRepository,
                            project.packageManager,
                            project.startCommand.as_ref().map(|s| &s.command),
                            project.confidence,
                            project.evidence.iter().map(|e| format!("{}={}", e.source, e.value)).collect::<Vec<_>>().join(" | "),
                        );
                    }
                    None => println!("PROJECT1420: pid {} has no project association", listener.pid),
                }
            }
            None => println!("PORT1420: not listening at engine snapshot time"),
        }

        // All resolved projects.
        for project in &response.projects {
            println!(
                "PROJECT: {:<28} root={:<46} kind={:?} git={:?} pm={:?} conf={:?}",
                project.name,
                project.rootPath,
                project.kind,
                project.git.branch,
                project.packageManager,
                project.confidence,
            );
        }
        println!(
            "PROJECTS: {} resolved, {} pid links",
            response.projects.len(),
            response.projectLinks.len(),
        );

        // Classification overview: every process with its identity.
        for service in response.services.iter().take(30) {
            let process = response.processes.iter().find(|p| p.pid == service.pid);
            println!(
                "SERVICE: PID {:>6}  {:<18} → {:<22} ({:?})",
                service.pid,
                process.and_then(|p| p.name.as_deref()).unwrap_or("<inaccessible>"),
                service.identity.displayName,
                service.identity.confidence,
            );
        }
        for p in response.processes.iter().take(60) {
            println!(
                "  PID {:>6}  {:<18}  mem={:<10}  accessible={}",
                p.pid,
                p.name.as_deref().unwrap_or("<unavailable>"),
                p.memoryBytes.map(|b| format!("{} bytes", b)).unwrap_or_else(|| "—".into()),
                p.accessible,
            );
        }

        // Cache adopts cycle 1's raw samples as cycle 2's baseline.
        let next = crate::process::build_next_cache(&cycle1);
        drop(cache);
        *state.cache.lock().unwrap() = next;

        // ---- Cycle 2: computes real delta CPU percentages ----------------
        std::thread::sleep(std::time::Duration::from_millis(1200));
        let cache = state.cache.lock().expect("cache lock");
        let cycle2 = crate::process::run_discovery_cycle(&cache, &mut project_cache, false, &control_state.registry)
            .expect("live discovery must not fail on Windows");
        let response2 = &cycle2.response;

        let accessible2: Vec<_> = response2
            .processes
            .iter()
            .filter(|p| p.accessible && p.cpuPercent.is_some())
            .collect();
        for process in &accessible2 {
            // Clamp contract: 0.0–100.0 (per-core-normalized).
            let cpu = process.cpuPercent.expect("filtered above");
            assert!((0.0..=100.0).contains(&cpu), "CPU must be clamped, got {cpu}");
        }
        println!(
            "cycle 2: {} listeners, {} processes with CPU values (avg {:.2}%), {} ms",
            response2.listeners.len(),
            accessible2.len(),
            if accessible2.is_empty() {
                0.0
            } else {
                accessible2.iter().map(|p| p.cpuPercent.unwrap()).sum::<f64>() / accessible2.len() as f64
            },
            response2.durationMs,
        );

        // Serialize exactly as the Tauri IPC layer will.
        let json = serde_json::to_string(&response2).expect("response must serialize");
        assert!(json.contains("\"processes\""), "JSON must carry process data");
        assert!(json.contains("\"projects\""), "JSON must carry project data");
        assert!(json.contains("\"projectLinks\""), "JSON must carry project links");
        println!("serialized payload: {} bytes", json.len());
    }

    #[test]
    fn module_is_wired() {
        // Compilation of this test proves the discovery tree is reachable.
        assert!(true);
    }
}
