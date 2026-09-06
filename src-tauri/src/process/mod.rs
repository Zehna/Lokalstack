//! # Process Intelligence (Phase 2)
//!
//! Resolves listener PIDs into real Windows process metadata: image name,
//! executable path, start time, working-set memory, and delta-sampled CPU
//! percentage. Read-only — no process control of any kind lives here.
//!
//! ## Structure
//!
//! - [`sampler`] — pure, OS-independent logic: DTOs, FILETIME conversion,
//!   basename extraction, CPU-percentage math, cache merge. Fully unit-tested.
//! - [`windows`] — the narrow unsafe FFI boundary (`OpenProcess` with
//!   `PROCESS_QUERY_LIMITED_INFORMATION`, `QueryFullProcessImageNameW`,
//!   `GetProcessTimes`, `GetProcessMemoryInfo`, plus a process-snapshot name
//!   lookup for access-denied PIDs). Every handle is RAII-guarded.
//!
//! ## Sampling architecture
//!
//! Port discovery produces listeners → [`run_discovery_cycle`] derives the
//! **unique** PID set → each PID is opened **once per cycle** (five listeners
//! sharing a PID cost one inspection, not five) → the previous cycle's cache
//! merges delta CPU percentages → inaccessible PIDs get a best-effort display
//! name from the toolhelp snapshot.
//!
//! ## Cross-cycle state
//!
//! [`SampleCache`] (per-PID previous CPU sample + creation-time identity)
//! lives in [`ProcessEngineState`], managed by Tauri. The cache key includes
//! the process creation time, so a reused PID cannot inherit a stale CPU
//! baseline. Dead PIDs are dropped from the cache every cycle — it can never
//! grow beyond the current listener set.

pub(crate) mod sampler;

#[cfg(windows)]
pub(crate) mod windows;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub(crate) use sampler::ProcessInfo;

/// Tauri-managed cross-cycle state for the process engine.
///
/// The mutex serializes refresh cycles (the frontend store already refuses
/// overlapping requests — this is defense in depth) and holds the previous
/// cycle's CPU baseline keyed by PID with creation-time identity. Wrapped in
/// an `Arc` so the blocking task can own it for the duration of a cycle.
#[derive(Default)]
pub(crate) struct ProcessEngineState {
    pub(crate) cache: Arc<Mutex<sampler::SampleCache>>,
}

/// The result of one full discovery cycle.
pub(crate) struct DiscoveryCycle {
    /// The serialized response payload (listeners + process intelligence).
    pub response: crate::discovery::PortListenersResponse,
    /// Raw per-PID CPU samples taken during this cycle. These become the
    /// next cycle's baseline; they deliberately do not cross the Tauri
    /// boundary (the frontend contract stays minimal).
    pub raw_samples: HashMap<u32, sampler::RawCpuSample>,
}

/// One full discovery cycle: TCP listener enumeration + unique-PID process
/// inspection + CPU delta merge against the previous cycle, then project
/// resolution (Phase 4) against the project cache.
///
/// Every failure mode of an individual process (gone, access denied) is
/// absorbed into that process's [`ProcessInfo::accessible`] — only a failure
/// of the *listener tables themselves* errors the whole cycle, matching the
/// Phase 1 contract.
pub(crate) fn run_discovery_cycle(
    previous_cache: &sampler::SampleCache,
    project_cache: &mut crate::project::ProjectCache,
    bypass_project_cache: bool,
) -> Result<DiscoveryCycle, String> {
    #[cfg(windows)]
    {
        use std::collections::BTreeSet;
        use std::time::Instant;

        let started = Instant::now();
        let wall_ms = windows::now_unix_ms();

        let listeners = crate::discovery::windows::enumerate_tcp_listeners()?;
        let listeners = crate::discovery::ports::normalize_listeners(listeners);

        // Unique PIDs, ordered — each is inspected exactly once per cycle.
        let unique_pids: Vec<u32> = listeners
            .iter()
            .map(|l| l.pid)
            .collect::<BTreeSet<u32>>()
            .into_iter()
            .collect();

        let (processes, raw_samples) = windows::sample_processes(&unique_pids);
        let (mut processes, _merged) = sampler::merge_cpu_percentages(
            processes,
            &raw_samples,
            previous_cache,
            sampler::logical_core_count(),
        );
        // Restore display names for processes Windows refused to open.
        windows::apply_snapshot_names(&mut processes);

        // Phase 3: classify each PID into a developer-facing service
        // identity from evidence (executable, path, command line). Pure
        // derivation — no additional Windows calls.
        let services = crate::intelligence::classify_processes(&processes, &listeners);

        let response = crate::discovery::PortListenersResponse {
            capturedAt: wall_ms,
            durationMs: started.elapsed().as_millis() as u64,
            listeners,
            processes,
            services,
            projects: Vec::new(),
            projectLinks: Vec::new(),
        };

        // Phase 4: resolve projects from process evidence, content-addressed
        // cached so warm cycles do zero filesystem work.
        let (projects, project_links, _stats) = crate::project::resolve_projects(
            &response.processes,
            project_cache,
            bypass_project_cache,
        );
        let response = crate::discovery::PortListenersResponse {
            projects,
            projectLinks: project_links,
            ..response
        };

        Ok(DiscoveryCycle {
            response,
            raw_samples,
        })
    }

    #[cfg(not(windows))]
    {
        let _ = (previous_cache, project_cache, bypass_project_cache);
        Err("Port and process discovery are Windows-only in Phase 2.".to_string())
    }
}

/// Derive the next cycle's CPU baseline from this cycle's raw samples.
///
/// Keyed by PID, carrying the creation-time identity — a reused PID gets a
/// fresh identity and thus no stale percentage (see `sampler::ProcessIdentity`).
pub(crate) fn build_next_cache(cycle: &DiscoveryCycle) -> sampler::SampleCache {
    cycle
        .raw_samples
        .iter()
        .map(|(&pid, raw)| {
            (
                pid,
                sampler::PreviousSample {
                    identity: sampler::ProcessIdentity {
                        pid,
                        creation_ticks: raw.creation_ticks,
                    },
                    cpu_sample: sampler::CpuSample {
                        cpu_ticks: raw.cpu_ticks,
                        wall_ms: raw.wall_ms,
                    },
                },
            )
        })
        .collect()
}

/// Read-only Tauri command: full snapshot of TCP listeners, their owning
/// processes, service identities, and resolved projects in one payload, so a
/// refresh cycle samples every PID exactly once and resolves each project
/// once. Frontend contract: `{ listeners, processes, services, projects,
/// projectLinks, capturedAt, durationMs }`; rejects with a human-readable
/// error string. `bypassProjectCache` (manual refresh) re-reads project
/// metadata from the filesystem.
#[tauri::command]
pub(crate) async fn get_port_listeners(
    state: tauri::State<'_, ProcessEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
    bypass_project_cache: Option<bool>,
) -> Result<crate::discovery::PortListenersResponse, String> {
    let cache = Arc::clone(&state.cache);
    let project_cache = Arc::clone(&projects.cache);
    tauri::async_runtime::spawn_blocking(move || {
        let bypass = bypass_project_cache.unwrap_or(false);
        let mut previous = cache
            .lock()
            .map_err(|_| "process engine cache lock poisoned".to_string())?;
        let mut project_cache = project_cache
            .lock()
            .map_err(|_| "project engine cache lock poisoned".to_string())?;
        let cycle = run_discovery_cycle(&previous, &mut project_cache, bypass)?;
        *previous = build_next_cache(&cycle);
        Ok(cycle.response)
    })
    .await
    .map_err(|e| format!("discovery task join error: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_is_wired() {
        assert!(true);
    }

    #[test]
    fn next_cache_is_keyed_by_pid_with_raw_identity() {
        let mut raw_samples = HashMap::new();
        raw_samples.insert(
            42u32,
            sampler::RawCpuSample {
                cpu_ticks: 123_456,
                creation_ticks: 999,
                wall_ms: 5_000,
            },
        );
        let cycle = DiscoveryCycle {
            response: crate::discovery::PortListenersResponse {
                capturedAt: 5_000,
                durationMs: 1,
                listeners: Vec::new(),
                processes: Vec::new(),
                services: Vec::new(),
                projects: Vec::new(),
                projectLinks: Vec::new(),
            },
            raw_samples,
        };

        let cache = build_next_cache(&cycle);
        let entry = cache.get(&42).expect("PID 42 must be cached");
        assert_eq!(entry.identity.pid, 42);
        assert_eq!(entry.identity.creation_ticks, 999);
        assert_eq!(entry.cpu_sample.cpu_ticks, 123_456);
        assert_eq!(entry.cpu_sample.wall_ms, 5_000);
    }
}
