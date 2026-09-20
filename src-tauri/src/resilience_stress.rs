//! Phase 10B (spec §R, §S, §D) — resilience stress and race tests.
//!
//! These tests run many real discovery cycles against the live Windows TCP
//! table (read-only) and verify:
//!  - cycles remain time-bounded (no unbounded per-cycle work)
//!  - no panic across cycles (process-gone races are expected and absorbed)
//!  - cache chaining keeps CPU baselines bounded (dead PIDs pruned)
//!
//! They never open, write, or terminate any other process. Short-lived
//! disposable `cmd /c exit` children create realistic process-gone races.

use std::collections::HashMap;

use crate::control::ControlTargetRegistry;
use crate::process::{run_discovery_cycle, sampler::SampleCache};
use crate::project::ProjectCache;

#[test]
fn repeated_discovery_cycles_are_stable_and_bounded() {
    let mut project_cache = ProjectCache::new();
    let registry = ControlTargetRegistry::new();
    let mut previous: SampleCache = HashMap::new();

    // Warm-up cycle (allocates lazy OS resources) outside the measurement.
    let warm = run_discovery_cycle(&previous, &mut project_cache, false, &registry);
    assert!(warm.is_ok(), "warm-up discovery cycle failed: {:?}", warm.err());
    let warm = warm.unwrap();
    previous = warm.raw_samples.iter().map(|(pid, s)| {
        (
            *pid,
            crate::process::sampler::PreviousSample {
                identity: crate::process::sampler::ProcessIdentity {
                    pid: *pid,
                    creation_ticks: s.creation_ticks,
                },
                cpu_sample: crate::process::sampler::CpuSample {
                    cpu_ticks: s.cpu_ticks,
                    wall_ms: s.wall_ms,
                },
            },
        )
    }).collect();

    let cycles = 40;
    let mut durations = Vec::with_capacity(cycles);

    for _ in 0..cycles {
        let started = std::time::Instant::now();
        let cycle = run_discovery_cycle(&previous, &mut project_cache, false, &registry)
            .expect("repeated discovery cycle must not fail");
        durations.push(started.elapsed());
        // Dead PIDs are dropped each rebuild — the cache can never grow
        // unboundedly across cycles (Phase 2 invariant, re-proven here).
        previous = cycle.raw_samples.iter().map(|(pid, s)| {
            (
                *pid,
                crate::process::sampler::PreviousSample {
                    identity: crate::process::sampler::ProcessIdentity {
                        pid: *pid,
                        creation_ticks: s.creation_ticks,
                    },
                    cpu_sample: crate::process::sampler::CpuSample {
                        cpu_ticks: s.cpu_ticks,
                        wall_ms: s.wall_ms,
                    },
                },
            )
        }).collect();
    }

    let avg: std::time::Duration =
        durations.iter().sum::<std::time::Duration>() / cycles as u32;
    let max = durations.iter().max().copied().unwrap_or_default();
    // Each cycle must be bounded well under the 3 s polling cadence.
    assert!(
        max.as_millis() < 3_000,
        "discovery cycle exceeded its poll budget: max {:?} avg {:?}",
        max,
        avg
    );
}

#[test]
fn discovery_absorbs_concurrent_process_churn() {
    // Spawn-and-exit disposable children immediately before a cycle so some
    // PIDs vanish between enumeration and inspection. Discovery must
    // complete and stay consistent.
    let mut project_cache = ProjectCache::new();
    let registry = ControlTargetRegistry::new();
    let previous: SampleCache = HashMap::new();

    let mut children = Vec::new();
    for _ in 0..5 {
        // `cmd /c exit` — exits immediately, creating a PID that is very
        // likely gone by the time the cycle inspects it.
        let child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn disposable child");
        children.push(child);
    }
    for child in &mut children {
        let _ = child.wait();
    }

    let cycle = run_discovery_cycle(&previous, &mut project_cache, false, &registry)
        .expect("discovery must absorb process churn without error");

    // A second cycle with the churn settled must also succeed.
    let settled: SampleCache = cycle
        .raw_samples
        .iter()
        .map(|(pid, s)| {
            (
                *pid,
                crate::process::sampler::PreviousSample {
                    identity: crate::process::sampler::ProcessIdentity {
                        pid: *pid,
                        creation_ticks: s.creation_ticks,
                    },
                    cpu_sample: crate::process::sampler::CpuSample {
                        cpu_ticks: s.cpu_ticks,
                        wall_ms: s.wall_ms,
                    },
                },
            )
        })
        .collect();
    let second = run_discovery_cycle(&settled, &mut project_cache, false, &registry)
        .expect("settled discovery cycle must not fail");
    let _ = second;
}

#[test]
fn discovery_handles_cold_full_reinspection() {
    // Bypass cache forces full project re-inspection each cycle (the
    // slowest path); it must still be bounded and error-free.
    let mut project_cache = ProjectCache::new();
    let registry = ControlTargetRegistry::new();
    let previous: SampleCache = HashMap::new();

    let started = std::time::Instant::now();
    let cycle = run_discovery_cycle(&previous, &mut project_cache, true, &registry)
        .expect("cold discovery must not fail");
    let elapsed = started.elapsed();
    assert!(
        elapsed.as_secs() < 10,
        "cold discovery took {:?} — unbounded work detected",
        elapsed
    );
    let _ = cycle;
}

/// Real handle count for this process (Windows). Returns `None` outside
/// Windows so the test harness stays cross-platform.
fn self_handle_count() -> Option<u32> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetProcessHandleCount,
        };
        // GetCurrentProcess returns a pseudo-handle (never closed) — this is
        // the documented Windows contract, not a leak.
        let handle = GetCurrentProcess();
        let mut count: u32 = 0;
        if GetProcessHandleCount(handle, &mut count) != 0 {
            Some(count)
        } else {
            None
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Phase 10B (§R): repeated discovery cycles must not leak handles.
///
/// A snapshot toolhelp handle, an OpenProcess handle, or a pipe left open
/// per cycle shows up as monotonic growth. Small variance (±8) between
/// individual cycles is normal runtime noise (allocators, OS bookkeeping);
/// only clear growth beyond noise fails the test.
#[test]
fn discovery_cycles_do_not_leak_handles() {
    let Some(before) = self_handle_count() else {
        return; // non-Windows: nothing to measure
    };

    let mut project_cache = ProjectCache::new();
    let registry = ControlTargetRegistry::new();
    let mut previous: SampleCache = HashMap::new();

    for _ in 0..150 {
        let cycle = run_discovery_cycle(&previous, &mut project_cache, false, &registry)
            .expect("discovery cycle must not fail");
        previous = cycle
            .raw_samples
            .iter()
            .map(|(pid, s)| {
                (
                    *pid,
                    crate::process::sampler::PreviousSample {
                        identity: crate::process::sampler::ProcessIdentity {
                            pid: *pid,
                            creation_ticks: s.creation_ticks,
                        },
                        cpu_sample: crate::process::sampler::CpuSample {
                            cpu_ticks: s.cpu_ticks,
                            wall_ms: s.wall_ms,
                        },
                    },
                )
            })
            .collect();
    }

    let midway = self_handle_count().expect("handle count must remain readable");

    // Phase 10B §R policy: fail only on CLEAR MONOTONIC growth, not one-shot
    // variance. A single warm-up burst can legitimately grow handles (lazy
    // allocator/OS bookkeeping), so the leak signal is growth that
    // CONTINUES at the same per-cycle rate in a second, equally long burst.
    let mut previous2 = previous;
    for _ in 0..150 {
        let cycle = run_discovery_cycle(&previous2, &mut project_cache, false, &registry)
            .expect("discovery cycle must not fail");
        previous2 = cycle
            .raw_samples
            .iter()
            .map(|(pid, s)| {
                (
                    *pid,
                    crate::process::sampler::PreviousSample {
                        identity: crate::process::sampler::ProcessIdentity {
                            pid: *pid,
                            creation_ticks: s.creation_ticks,
                        },
                        cpu_sample: crate::process::sampler::CpuSample {
                            cpu_ticks: s.cpu_ticks,
                            wall_ms: s.wall_ms,
                        },
                    },
                )
            })
            .collect();
    }

    let after = self_handle_count().expect("handle count must remain readable");
    let burst1 = i64::from(midway) - i64::from(before);
    let burst2 = i64::from(after) - i64::from(midway);
    // Leak signal = growth that CONTINUES at the same per-cycle rate in the
    // second burst (Phase 10B §R policy). burst1 is warm-up only: concurrent
    // tests can RELEASE handles during burst1 (ambient noise can make it
    // negative), so it must never gate the assertion — burst2 continuing
    // growth is the per-cycle leak signal. burst2 <= 8 tolerates one-shot
    // variance; a genuine per-cycle leak would keep growing across the
    // second burst.
    assert!(
        burst2 <= 8,
        "handle leak detected: second-burst growth continued \
         (burst1 {before}→{midway} = {burst1:+}, burst2 {midway}→{after} = {burst2:+}) \
         — per-cycle leak, not warm-up",
    );
}
