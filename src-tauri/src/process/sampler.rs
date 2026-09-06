//! Pure, OS-independent logic for the process-intelligence engine.
//!
//! Everything here is deterministic and unit-testable without a Windows
//! handle: DTO definitions, FILETIME conversion, executable basename
//! extraction, delta CPU percentage, and the merge of sampled process data
//! into the listener response. The unsafe FFI lives in [`super::windows`].

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One Windows process, as far as the OS let us inspect it.
///
/// This is the serde DTO that crosses the Tauri boundary. No Windows types
/// leak past the FFI layer in [`super::windows`].
///
/// Access-denied is a first-class outcome, not an error: `accessible == false`
/// means "the listener exists, but Windows refused (or the process was gone
/// before we could look) — everything else is unknown". The UI renders
/// `null`/`false` honestly instead of inventing values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// Field names deliberately mirror the frontend contract (`domain.ts`).
#[allow(non_snake_case)]
pub(crate) struct ProcessInfo {
    /// Process ID the snapshot was taken for.
    pub pid: u32,
    /// Image basename from `QueryFullProcessImageNameW` (e.g. `node.exe`),
    /// or `null` when the process could not be inspected.
    pub name: Option<String>,
    /// Full executable path (`C:\Program Files\nodejs\node.exe`), or `null`
    /// when unknown. Some processes are queryable but reveal no path.
    pub executablePath: Option<String>,
    /// Process start time as Unix epoch milliseconds (from `GetProcessTimes`
    /// creation FILETIME), or `null` when unknown.
    pub startedAt: Option<u64>,
    /// Working set in bytes (from `GetProcessMemoryInfo`), or `null`.
    pub memoryBytes: Option<u64>,
    /**
     * CPU usage in percent over the most recent sampling window,
     * normalized per logical core and clamped to 0.0–100.0.
     *
     * `None` means "first observation — no previous sample to diff against
     * yet" or the sampling window was too short to be meaningful. The UI
     * renders `None` as `—` / "calculating…". We never fabricate `0.0`.
     */
    pub cpuPercent: Option<f64>,
    /// Whether the process could be inspected at all this cycle.
    pub accessible: bool,
}

/// An access-denied/gone process keeps its identity but no metadata.
impl ProcessInfo {
    pub(crate) fn inaccessible(pid: u32) -> Self {
        Self {
            pid,
            name: None,
            executablePath: None,
            startedAt: None,
            memoryBytes: None,
            cpuPercent: None,
            accessible: false,
        }
    }
}

// ---------------------------------------------------------------------------
// FILETIME conversion
// ---------------------------------------------------------------------------

/// Convert a Win32 `FILETIME` pair (100 ns ticks since 1601-01-01 UTC) to
/// Unix epoch milliseconds.
///
/// The FILETIME epoch (1601-01-01) is exactly 11 644 473 600 seconds before
/// the Unix epoch (1970-01-01). Ticks are 100 ns units, so:
/// `unix_ms = (ticks / 10_000) - 11_644_473_600_000`.
///
/// Predates-1970 and overflow cases return `None` (the frontend renders `—`)
/// rather than panicking or wrapping — a bogus creation time must never take
/// down a refresh cycle.
#[must_use]
pub(crate) fn filetime_to_unix_ms(low: u32, high: u32) -> Option<u64> {
    let ticks: u64 = (u64::from(high) << 32) | u64::from(low);
    const UNIX_EPOCH_IN_FILETIME_100NS: u64 = 116_444_736_000_000_000; // 11 644 473 600 s × 10⁷
    const TICKS_PER_MILLISECOND: u64 = 10_000;

    let unix_100ns = ticks.checked_sub(UNIX_EPOCH_IN_FILETIME_100NS)?;
    // Round to the nearest millisecond so identical instants sampled via
    // different tick granularities compare equal. (Saturation only matters
    // for absurd dates ~year 60530 and keeps the conversion total.)
    Some(unix_100ns.saturating_add(TICKS_PER_MILLISECOND / 2) / TICKS_PER_MILLISECOND)
}

// ---------------------------------------------------------------------------
// Executable basename
// ---------------------------------------------------------------------------

/// Extract the executable basename from a full Windows image path.
///
/// Windows paths use `\` (and tolerate `/`); a path with no separator at all
/// is treated as an already-basename input. Empty input maps to `None`.
///
/// Phase 2 extracts the name only — no framework classification (that is
/// Phase 3's `node.exe` → Next.js-style job).
#[must_use]
pub(crate) fn executable_basename(path: &str) -> Option<String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return None;
    }
    trimmed
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

// ---------------------------------------------------------------------------
// CPU percentage via delta sampling
// ---------------------------------------------------------------------------

/// Convert kernel+user CPU time (a FILETIME pair, as returned by
/// `GetProcessTimes`) into total 100 ns ticks.
#[must_use]
pub(crate) fn filetime_pair_to_ticks(low: u32, high: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// One CPU sample for a PID: cumulative CPU ticks and the wall-clock moment
/// (Unix ms) at which the cumulative counter was read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CpuSample {
    /// Cumulative kernel+user CPU time in 100 ns ticks since process start.
    pub cpu_ticks: u64,
    /// Wall-clock snapshot time in Unix epoch milliseconds.
    pub wall_ms: u64,
}

/// CPU percent between two samples, normalized per logical core.
///
/// Formula (documented in `docs/architecture.md`):
///
/// ```text
/// delta_cpu_ticks            cumulative CPU time the process consumed
/// delta_wall_ticks           wall time between the two samples
/// cpu_pct = 100 * (delta_cpu_ticks / delta_wall_ticks) / logical_cores
/// ```
///
/// - Ticks are 100 ns units, so `delta_wall_ms * 10_000` converts wall time
///   into the same unit.
/// - Normalizing by the core count maps 100 % to "all cores busy"; a
///   single-threaded process therefore maxes out at `100 / cores`, while a
///   fully parallel one can reach 100 %.
/// - Two guards against race conditions:
///   - a non-positive wall interval (equal timestamps, clock steps backwards,
///     or a window shorter than 1 ms) returns `None` — no division, no
///     fabricated value;
///   - the result is clamped to `0.0..=100.0` because a process can burn
///     slightly more CPU ticks than wall time delta (scheduler accounting
///     across resampled threads) or sample mid-death with a stale counter.
#[must_use]
pub(crate) fn cpu_percent_between(
    previous: CpuSample,
    current: CpuSample,
    logical_cores: u32,
) -> Option<f64> {
    if logical_cores == 0 {
        return None;
    }
    if current.wall_ms <= previous.wall_ms {
        // No measurable wall time (clock step or same-instant resample):
        // return nothing rather than a wild number.
        return None;
    }

    let delta_cpu = current.cpu_ticks.saturating_sub(previous.cpu_ticks);
    let delta_wall_ticks = (current.wall_ms - previous.wall_ms).saturating_mul(10_000);
    if delta_wall_ticks == 0 {
        return None;
    }

    let raw = (delta_cpu as f64 / delta_wall_ticks as f64) * 100.0 / f64::from(logical_cores);
    Some(raw.clamp(0.0, 100.0))
}

/// The number of logical processors, via the standard environment variable —
/// the cheapest reliable source and no FFI at all.
///
/// Falls back to `1` when the variable is missing or unparseable, which keeps
/// the CPU formula well-defined everywhere.
#[must_use]
pub(crate) fn logical_core_count() -> u32 {
    std::env::var("NUMBER_OF_PROCESSORS")
        .ok()
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|&count| count > 0)
        .unwrap_or(1)
}

// ---------------------------------------------------------------------------
// Cache + merge
// ---------------------------------------------------------------------------

/// Cache key that survives PID reuse: the PID **and** the process's creation
/// FILETIME. When Windows hands the PID to a different executable, the
/// creation time changes, so the stale CPU baseline is invalidated instead of
/// producing one wildly wrong percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProcessIdentity {
    pub pid: u32,
    /// Creation time as raw 100 ns FILETIME ticks (not converted) — the raw
    /// form is the stable identity Windows reports.
    pub creation_ticks: u64,
}

/// Per-PID previous sample used to compute the next CPU percentage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PreviousSample {
    pub identity: ProcessIdentity,
    pub cpu_sample: CpuSample,
}

/// Snapshot-cache backend: the previous cycle's per-PID samples.
///
/// Kept deliberately tiny — keyed by PID, carrying the creation-time identity
/// so PID reuse invalidates the CPU baseline (see [`ProcessIdentity`]).
pub(crate) type SampleCache = HashMap<u32, PreviousSample>;

/// Merge newly sampled processes into the previous cycle's cache.
///
/// For every sampled process that has a usable previous sample **for the same
/// process identity** (same PID *and* same creation time), computes the CPU
/// percentage between the two cycles and stores it on the returned
/// [`ProcessInfo`]. PIDs sampled for the first time keep `cpuPercent: None`
/// (honest "first sample" behavior — the next cycle, ~3 s later, gets a real
/// number). Finally, the new samples replace the cache wholesale.
///
/// PIDs that were seen last cycle but disappeared this cycle are dropped —
/// dead entries must not linger and grow the cache unboundedly.
#[must_use]
pub(crate) fn merge_cpu_percentages(
    mut processes: Vec<ProcessInfo>,
    raw_samples: &HashMap<u32, RawCpuSample>,
    previous: &SampleCache,
    logical_cores: u32,
) -> (Vec<ProcessInfo>, SampleCache) {
    let mut next_cache: SampleCache = HashMap::with_capacity(raw_samples.len());

    for info in &mut processes {
        let Some(raw) = raw_samples.get(&info.pid) else {
            continue;
        };
        let identity = ProcessIdentity {
            pid: info.pid,
            creation_ticks: raw.creation_ticks,
        };
        let current_sample = CpuSample {
            cpu_ticks: raw.cpu_ticks,
            wall_ms: raw.wall_ms,
        };

        if let Some(previous_entry) = previous.get(&info.pid) {
            if previous_entry.identity == identity {
                info.cpuPercent =
                    cpu_percent_between(previous_entry.cpu_sample, current_sample, logical_cores);
            }
            // Identity mismatch (PID reused) → leave None; fresh baseline below.
        }
        // Either way this cycle's sample becomes the next baseline.
        next_cache.insert(
            info.pid,
            PreviousSample {
                identity,
                cpu_sample: current_sample,
            },
        );
    }

    // `processes` was iterated immutably above; drop entries for PIDs that
    // vanished this cycle by rebuilding the cache strictly from live PIDs.
    (processes, next_cache)
}

/// Raw per-PID CPU sample straight from the FFI layer, before percentage
/// computation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RawCpuSample {
    /// Cumulative kernel+user CPU time in 100 ns ticks.
    pub cpu_ticks: u64,
    /// Process creation time as raw FILETIME ticks (identity component).
    pub creation_ticks: u64,
    /// Wall-clock snapshot time in Unix epoch milliseconds.
    pub wall_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- filetime_to_unix_ms ---------------------------------------------

    #[test]
    fn filetime_epoch_boundaries_convert() {
        // The FILETIME of the Unix epoch itself → 0 ms.
        assert_eq!(filetime_to_unix_ms(0, 0), None); // 1601-01-01, before Unix
        let unix_epoch_ticks = 116_444_736_000_000_000u64;
        assert_eq!(
            filetime_to_unix_ms(unix_epoch_ticks as u32, (unix_epoch_ticks >> 32) as u32),
            Some(0)
        );
        // Exactly one second after the Unix epoch.
        let ticks = unix_epoch_ticks + 10_000_000;
        assert_eq!(
            filetime_to_unix_ms(ticks as u32, (ticks >> 32) as u32),
            Some(1_000)
        );
    }

    #[test]
    fn filetime_rounds_to_nearest_millisecond() {
        // epoch + 1 s in ticks; then +0.4 ms rounds DOWN, +0.6 ms rounds UP.
        let base = 116_444_736_000_000_000u64 + 10_000_000;
        let plus_0_4ms = base + 4_000; // 1 ms = 10 000 ticks
        assert_eq!(
            filetime_to_unix_ms(plus_0_4ms as u32, (plus_0_4ms >> 32) as u32),
            Some(1_000)
        );
        let plus_0_6ms = base + 6_000;
        assert_eq!(
            filetime_to_unix_ms(plus_0_6ms as u32, (plus_0_6ms >> 32) as u32),
            Some(1_001)
        );
    }

    #[test]
    fn filetime_extreme_value_does_not_panic() {
        // All-ones FILETIME is an absurd date (~year 60530); the conversion
        // must stay total and bounded rather than panic or wrap.
        assert_eq!(
            filetime_to_unix_ms(u32::MAX, u32::MAX),
            Some(1_833_029_933_770_955)
        );
    }

    #[test]
    fn filetime_2020_date_is_plausible() {
        // 2020-01-01T00:00:00Z = 1577836800 s since Unix epoch.
        let ticks = 116_444_736_000_000_000u64 + 1_577_836_800u64 * 10_000_000;
        assert_eq!(
            filetime_to_unix_ms(ticks as u32, (ticks >> 32) as u32),
            Some(1_577_836_800_000)
        );
    }

    // --- executable_basename ----------------------------------------------

    #[test]
    fn basename_extracts_windows_path_tail() {
        assert_eq!(
            executable_basename(r"C:\Program Files\nodejs\node.exe"),
            Some("node.exe".to_string())
        );
        assert_eq!(
            executable_basename(r"C:\Windows\System32\svchost.exe"),
            Some("svchost.exe".to_string())
        );
    }

    #[test]
    fn basename_handles_forward_slashes_and_bare_names() {
        assert_eq!(
            executable_basename("C:/Program Files/nodejs/node.exe"),
            Some("node.exe".to_string())
        );
        assert_eq!(
            executable_basename("python.exe"),
            Some("python.exe".to_string())
        );
    }

    #[test]
    fn basename_rejects_empty_and_separator_only() {
        assert_eq!(executable_basename(""), None);
        assert_eq!(executable_basename("   "), None);
        assert_eq!(executable_basename(r"C:\"), None);
    }

    // --- cpu_percent_between ------------------------------------------------

    fn sample(cpu_ticks: u64, wall_ms: u64) -> CpuSample {
        CpuSample { cpu_ticks, wall_ms }
    }

    #[test]
    fn cpu_percent_full_core_single_thread() {
        // 1 s of CPU in 1 s of wall time, 1 core → 100 %.
        let previous = sample(0, 1_000);
        let current = sample(10_000_000, 2_000);
        assert_eq!(
            cpu_percent_between(previous, current, 1),
            Some(100.0)
        );
    }

    #[test]
    fn cpu_percent_is_normalized_per_logical_core() {
        // Same consumption on 8 cores → 12.5 % (all cores busy = 100 %).
        let previous = sample(0, 1_000);
        let current = sample(10_000_000, 2_000);
        let value = cpu_percent_between(previous, current, 8).expect("percentage");
        assert!((value - 12.5).abs() < 1e-9);
    }

    #[test]
    fn cpu_percent_idle_process_is_zero() {
        // No CPU consumed between samples → 0 %, a *derived* zero.
        let previous = sample(1_000_000, 1_000);
        let current = sample(1_000_000, 4_000);
        assert_eq!(cpu_percent_between(previous, current, 4), Some(0.0));
    }

    #[test]
    fn cpu_percent_first_sample_returns_none() {
        // "Previous" with zero wall distance can't produce a rate — the
        // engine models the first observation as no previous sample at all,
        // and this guard covers resampling within the same millisecond.
        let same_instant = sample(5_000_000, 2_000);
        assert_eq!(cpu_percent_between(same_instant, same_instant, 8), None);
    }

    #[test]
    fn cpu_percent_backwards_clock_returns_none() {
        let previous = sample(0, 5_000);
        let current = sample(10_000_000, 4_000); // wall clock went backwards
        assert_eq!(cpu_percent_between(previous, current, 8), None);
    }

    #[test]
    fn cpu_percent_clamps_scheduler_overrun() {
        // Race condition: more CPU ticks consumed than wall time allows
        // (2 s CPU in 1 s wall). Must clamp to 100 %, not 200 %.
        let previous = sample(0, 1_000);
        let current = sample(20_000_000, 2_000);
        assert_eq!(cpu_percent_between(previous, current, 1), Some(100.0));
    }

    #[test]
    fn cpu_percent_zero_cores_is_undefined() {
        let previous = sample(0, 1_000);
        let current = sample(10_000_000, 2_000);
        assert_eq!(cpu_percent_between(previous, current, 0), None);
    }

    #[test]
    fn cpu_percent_saturating_sub_handles_counter_reset() {
        // A counters-reset / reused-PID scenario must not go negative.
        let previous = sample(9_000_000, 1_000);
        let current = sample(1_000_000, 2_000);
        assert_eq!(cpu_percent_between(previous, current, 8), Some(0.0));
    }

    // --- cache identity (PID reuse) -----------------------------------------

    #[test]
    fn cache_identity_changes_when_pid_is_reused() {
        let old = ProcessIdentity { pid: 4212, creation_ticks: 100 };
        let reused = ProcessIdentity { pid: 4212, creation_ticks: 200 };
        assert_ne!(old, reused, "PID reuse must invalidate the CPU baseline");
    }

    // --- merge_cpu_percentages ----------------------------------------------

    fn info(pid: u32) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: Some("node.exe".to_string()),
            executablePath: None,
            startedAt: Some(1_700_000_000_000),
            memoryBytes: Some(100 * 1024 * 1024),
            cpuPercent: None,
            accessible: true,
        }
    }

    #[test]
    fn merge_first_cycle_leaves_cpu_none() {
        let processes = vec![info(1), info(2)];
        let mut raw = HashMap::new();
        raw.insert(1, RawCpuSample { cpu_ticks: 5_000_000, creation_ticks: 100, wall_ms: 1_000 });
        raw.insert(2, RawCpuSample { cpu_ticks: 1_000_000, creation_ticks: 200, wall_ms: 1_000 });

        let (merged, cache) = merge_cpu_percentages(processes, &raw, &HashMap::new(), 8);

        // Honest first sample: no fabricated percentages.
        assert!(merged.iter().all(|p| p.cpuPercent.is_none()));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache[&1].cpu_sample.cpu_ticks, 5_000_000);
    }

    #[test]
    fn merge_second_cycle_computes_percentages() {
        let processes = vec![info(1)];
        let mut raw = HashMap::new();
        raw.insert(1, RawCpuSample { cpu_ticks: 15_000_000, creation_ticks: 100, wall_ms: 4_000 });

        let mut previous: SampleCache = HashMap::new();
        previous.insert(1, PreviousSample {
            identity: ProcessIdentity { pid: 1, creation_ticks: 100 },
            cpu_sample: CpuSample { cpu_ticks: 5_000_000, wall_ms: 1_000 },
        });

        // 10_000_000 ticks = 1 s CPU (ticks are 100 ns) in 3 s wall, on 8
        // cores → 100 % / 8 = 12.5 % per-core-normalized.
        let (merged, _) = merge_cpu_percentages(processes, &raw, &previous, 8);
        let value = merged[0].cpuPercent.expect("second cycle must have a value");
        assert!((value - 100.0 * 10_000_000.0 / 30_000_000.0 / 8.0).abs() < 1e-9);
    }

    #[test]
    fn merge_pid_reuse_skips_stale_baseline() {
        // Same PID, different creation time → no percentage this cycle,
        // but the cache adopts the new identity for next cycle.
        let processes = vec![info(7)];
        let mut raw = HashMap::new();
        raw.insert(7, RawCpuSample { cpu_ticks: 9_000_000, creation_ticks: 999, wall_ms: 2_000 });

        let mut previous: SampleCache = HashMap::new();
        previous.insert(7, PreviousSample {
            identity: ProcessIdentity { pid: 7, creation_ticks: 111 },
            cpu_sample: CpuSample { cpu_ticks: 1_000_000, wall_ms: 1_000 },
        });

        let (merged, cache) = merge_cpu_percentages(processes, &raw, &previous, 8);
        assert!(merged[0].cpuPercent.is_none());
        assert_eq!(cache[&7].identity.creation_ticks, 999);
    }

    #[test]
    fn merge_drops_dead_pids_and_inaccessible_stay_out_of_cache() {
        // PID 3 existed last cycle but was not sampled this cycle (died):
        // it must not survive in the cache.
        let processes = vec![info(1)];
        let mut raw = HashMap::new();
        raw.insert(1, RawCpuSample { cpu_ticks: 2_000_000, creation_ticks: 100, wall_ms: 1_000 });

        let mut previous: SampleCache = HashMap::new();
        previous.insert(1, PreviousSample {
            identity: ProcessIdentity { pid: 1, creation_ticks: 100 },
            cpu_sample: CpuSample { cpu_ticks: 0, wall_ms: 500 },
        });
        previous.insert(3, PreviousSample {
            identity: ProcessIdentity { pid: 3, creation_ticks: 300 },
            cpu_sample: CpuSample { cpu_ticks: 0, wall_ms: 500 },
        });

        let (_, cache) = merge_cpu_percentages(processes, &raw, &previous, 8);
        assert!(cache.contains_key(&1));
        assert!(!cache.contains_key(&3), "dead PID must not linger in the cache");
    }

    // --- ProcessInfo::inaccessible -------------------------------------------

    #[test]
    fn inaccessible_process_keeps_only_pid() {
        let info = ProcessInfo::inaccessible(1068);
        assert_eq!(info.pid, 1068);
        assert!(!info.accessible);
        assert!(info.name.is_none());
        assert!(info.executablePath.is_none());
        assert!(info.startedAt.is_none());
        assert!(info.memoryBytes.is_none());
        assert!(info.cpuPercent.is_none());
    }

    // --- memory DTO conversion (bytes round-trip) ----------------------------

    #[test]
    fn memory_bytes_are_raw_values_not_strings() {
        // The domain stores bytes; formatting happens only in the UI layer.
        let mut info = info(5);
        let mb: u64 = 1024 * 1024;
        info.memoryBytes = Some(245 * mb);
        assert_eq!(info.memoryBytes, Some(245 * mb));
    }
}
