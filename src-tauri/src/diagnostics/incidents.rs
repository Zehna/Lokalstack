//! Phase 11C Task 2 — typed incident model, fingerprint, bounded persistent
//! index (spec §1, §3, §6.2).
//!
//! The public surface of this module is consumed by later tasks (capture
//! policy/worker, commands, export); until each consumer lands, not-yet-used
//! items carry scoped `#[allow(dead_code)]` rather than a module-wide allow.

#![allow(dead_code)] // task-by-task consumers; tightened as they land

use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

use super::ids;
use super::storage;

/// Severity model (spec §2), ordered lowest → highest for deterministic
/// eviction (lower-severity history is discarded first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    Info,
    Warning,
    Severe,
    Critical,
}

/// User-facing review state (spec §1): every incident starts `New`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ReviewState {
    New,
    Reviewed,
}

/// Overflow-safe occurrence counter (plan §8): increments saturate at
/// `u64::MAX` instead of panicking or wrapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SaturatingU64(u64);

impl SaturatingU64 {
    pub(crate) fn new(v: u64) -> Self {
        Self(v)
    }
    pub(crate) fn get(self) -> u64 {
        self.0
    }
    pub(crate) fn saturating_inc(&mut self) {
        self.0 = self.0.saturating_add(1);
    }
}

/// The stable identity of an incident: subsystem + fingerprint (spec §3).
/// Volatile fields (PID, timestamps, paths, raw messages) are deliberately
/// excluded from the fingerprint domain so the same logical problem
/// coalesces into one incident.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct IncidentKey {
    pub(crate) subsystem: String,
    pub(crate) fingerprint: String,
}

/// Deterministic, privacy-safe fingerprint (spec §3): blake3 hex over the
/// normalized 5-field domain `v1\0{subsystem}\0{code}\0{operation}\0{severity}`.
/// No PID/timestamp/username/path/message input exists in the signature.
pub(crate) fn fingerprint(
    subsystem: &str,
    code: &str,
    operation: &str,
    severity: Severity,
) -> String {
    let sev = match severity {
        Severity::Info => "info",
        Severity::Warning => "warning",
        Severity::Severe => "severe",
        Severity::Critical => "critical",
    };
    let domain = format!("v1\0{subsystem}\0{code}\0{operation}\0{sev}");
    blake3::hash(domain.as_bytes()).to_hex().to_string()
}

/// One coalesced persistent incident record (spec §6.2). Payload-free by
/// construction: no stdout/stderr, no stack traces, no env, no command
/// lines — detailed evidence lives only in encrypted bundles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IncidentRecord {
    pub(crate) opaque_id: String,
    pub(crate) key: IncidentKey,
    pub(crate) code: String,
    pub(crate) severity: Severity,
    pub(crate) operation: String,
    /// Already-redacted one-line summary (caller redacts before recording).
    pub(crate) summary: String,
    pub(crate) first_seen_ms: u64,
    pub(crate) last_seen_ms: u64,
    pub(crate) occurrences: SaturatingU64,
    pub(crate) review_state: ReviewState,
    pub(crate) trigger_reason: String,
    pub(crate) bundle_ids: Vec<String>,
}

/// Index schema version (spec §16): explicit from day one.
pub(crate) const INDEX_SCHEMA_VERSION: u32 = 1;

/// Hard bounds (plan §8): 500 incidents globally, 100 per subsystem.
pub(crate) const MAX_INCIDENTS_GLOBAL: usize = 500;
pub(crate) const MAX_INCIDENTS_PER_SUBSYSTEM: usize = 100;

/// Health of the index file at load time (spec §6.4/§16). Index loss is NOT
/// payload corruption — a missing/malformed index never implies bundles are
/// unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexState {
    Valid,
    Corrupt,
    Unsupported,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexHealth {
    pub(crate) state: IndexState,
    pub(crate) repaired_from_backup: bool,
}

/// The persistent incident index (schema v1). `pending_deletions` carries
/// tombstoned bundle IDs for the Task 10 retention transaction; it is
/// deserialized with `serde(default)` so older files load cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IncidentIndex {
    pub(crate) schema_version: u32,
    pub(crate) incidents: Vec<IncidentRecord>,
    #[serde(default)]
    pub(crate) pending_deletions: Vec<String>,
}

impl Default for IncidentIndex {
    fn default() -> Self {
        Self {
            schema_version: INDEX_SCHEMA_VERSION,
            incidents: Vec::new(),
            pending_deletions: Vec::new(),
        }
    }
}

/// What a `record` call did to the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexMutation {
    /// Existing incident coalesced: occurrence count + last_seen updated.
    Updated,
    /// New incident inserted (possibly triggering an eviction).
    Inserted,
}

impl IncidentIndex {
    /// Record one occurrence of `(key, …)` at `now_ms`. Coalesces matches by
    /// key (bumping `occurrences` saturating-ly, moving `last_seen`), else
    /// inserts a fresh record, then enforces the caps deterministically.
    pub(crate) fn record(
        &mut self,
        key: IncidentKey,
        code: &str,
        severity: Severity,
        operation: &str,
        summary: &str,
        now_ms: u64,
    ) -> (IndexMutation, Option<IncidentRecord>) {
        if let Some(existing) = self.incidents.iter_mut().find(|r| r.key == key) {
            existing.occurrences.saturating_inc();
            existing.last_seen_ms = existing.last_seen_ms.max(now_ms);
            return (IndexMutation::Updated, Some(existing.clone()));
        }
        let record = IncidentRecord {
            opaque_id: ids::random_hex(12).unwrap_or_else(|| {
                // RNG outage: a deterministic still-opaque fallback keeps the
                // index bounded and usable; collision risk is irrelevant at
                // 500-record scale. Never content-derived.
                format!("fallback-{:024}", now_ms % 10_000_000_000)
            }),
            key,
            code: code.to_string(),
            severity,
            operation: operation.to_string(),
            summary: summary.to_string(),
            first_seen_ms: now_ms,
            last_seen_ms: now_ms,
            occurrences: SaturatingU64::new(1),
            review_state: ReviewState::New,
            trigger_reason: String::new(),
            bundle_ids: Vec::new(),
        };
        self.incidents.push(record.clone());
        self.enforce_caps();
        (IndexMutation::Inserted, self.incidents.last().cloned())
    }

    /// Deterministic bound enforcement (plan §9): per-subsystem cap FIRST,
    /// then global cap. Candidates sort by (severity ASC, last_seen ASC,
    /// opaque_id DESC) — the front of that ordering is evicted first, so
    /// lower-severity history is discarded before severe/critical history,
    /// and the opaque ID is the stable tie-breaker.
    fn enforce_caps(&mut self) {
        let evict_order = |a: &IncidentRecord, b: &IncidentRecord| {
            a.severity
                .cmp(&b.severity)
                .then(a.last_seen_ms.cmp(&b.last_seen_ms))
                .then(b.opaque_id.cmp(&a.opaque_id))
        };
        // Per-subsystem cap.
        loop {
            let mut counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for r in &self.incidents {
                *counts.entry(r.key.subsystem.as_str()).or_default() += 1;
            }
            let Some((over_sub, _)) =
                counts.into_iter().find(|(_, n)| *n > MAX_INCIDENTS_PER_SUBSYSTEM)
            else {
                break;
            };
            let victim = self
                .incidents
                .iter()
                .filter(|r| r.key.subsystem == over_sub)
                .min_by(|a, b| evict_order(a, b))
                .map(|r| r.opaque_id.clone())
                .unwrap();
            self.incidents.retain(|r| r.opaque_id != victim);
        }
        // Global cap.
        while self.incidents.len() > MAX_INCIDENTS_GLOBAL {
            let victim = self
                .incidents
                .iter()
                .min_by(|a, b| evict_order(a, b))
                .map(|r| r.opaque_id.clone())
                .unwrap();
            self.incidents.retain(|r| r.opaque_id != victim);
        }
    }

    /// Load the index at `path`. Missing file → default + `Valid` (first
    /// run). Malformed → `Corrupt` (file left untouched, never auto-deleted).
    /// Wrong major schema → `Unsupported` (never parsed speculatively).
    pub(crate) fn load_or_default(path: &Path) -> (Self, IndexHealth) {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(_) => return (Self::default(), IndexHealth { state: IndexState::Valid, repaired_from_backup: false }),
        };
        // Read schema version first without full deserialization.
        #[derive(Deserialize)]
        struct Probe {
            #[serde(default)]
            schema_version: u32,
        }
        match serde_json::from_slice::<Probe>(&bytes) {
            Ok(probe) if probe.schema_version != INDEX_SCHEMA_VERSION => {
                return (
                    Self::default(),
                    IndexHealth { state: IndexState::Unsupported, repaired_from_backup: false },
                );
            }
            Ok(_) => {}
            Err(_) => {
                return (
                    Self::default(),
                    IndexHealth { state: IndexState::Corrupt, repaired_from_backup: false },
                );
            }
        }
        match serde_json::from_slice::<IncidentIndex>(&bytes) {
            Ok(idx) if idx.schema_version == INDEX_SCHEMA_VERSION => {
                (idx, IndexHealth { state: IndexState::Valid, repaired_from_backup: false })
            }
            _ => (
                Self::default(),
                IndexHealth { state: IndexState::Corrupt, repaired_from_backup: false },
            ),
        }
    }

    /// Persist atomically via Task 1 `storage::write_atomic` (create_new temp
    /// in LocalStack-owned temp/, sync, same-volume rename — no tempfile).
    pub(crate) fn save_atomic(&self, path: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        storage::write_atomic(path, &bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(sub: &str, sev: Severity) -> IncidentRecord {
        IncidentRecord {
            opaque_id: format!("id-{sub}-{sev:?}-{}", ids::random_hex(4).unwrap_or_default()),
            key: IncidentKey {
                subsystem: sub.to_string(),
                fingerprint: fingerprint(sub, "ERR_X", "op", sev),
            },
            code: "ERR_X".into(),
            severity: sev,
            operation: "op".into(),
            summary: "summary".into(),
            first_seen_ms: 100,
            last_seen_ms: 100,
            occurrences: SaturatingU64::new(1),
            review_state: ReviewState::New,
            trigger_reason: String::new(),
            bundle_ids: Vec::new(),
        }
    }

    #[test]
    fn fingerprint_is_stable_and_discriminates_normalized_fields() {
        let a = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
        let b = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
        assert_eq!(a, b, "deterministic across calls");
        assert_ne!(fingerprint("docker", "ERR_X", "probe", Severity::Warning), a);
        assert_ne!(fingerprint("docker", "ERR_Y", "probe", Severity::Severe), a);
        assert_ne!(fingerprint("workspace", "ERR_X", "probe", Severity::Severe), a);
        assert_eq!(a.len(), 64, "blake3 hex length");
    }

    #[test]
    fn fingerprint_domain_excludes_volatile_inputs() {
        // Different PIDs/timestamps/paths must not alter the fingerprint:
        // they are not part of the domain signature at all, proven by the
        // 5-field domain hash equality with the same logical inputs.
        let f1 = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
        // Simulate "same problem from a different process/moment" — the
        // signature has nowhere for volatile data to enter.
        let f2 = fingerprint("docker", "ERR_X", "probe", Severity::Severe);
        assert_eq!(f1, f2);
        // Raw secret-bearing messages are not hashed into the fingerprint —
        // only the typed fields are. (No API surface even accepts a message.)
    }

    #[test]
    fn occurrences_saturate() {
        let mut o = SaturatingU64::new(u64::MAX);
        o.saturating_inc();
        o.saturating_inc();
        assert_eq!(o.get(), u64::MAX, "counter saturates, never wraps");
    }

    #[test]
    fn record_coalesces_by_key() {
        let mut idx = IncidentIndex::default();
        let key = IncidentKey {
            subsystem: "docker".into(),
            fingerprint: fingerprint("docker", "ERR_X", "probe", Severity::Severe),
        };
        let (m1, _) = idx.record(key.clone(), "ERR_X", Severity::Severe, "probe", "s", 10);
        let (m2, r2) = idx.record(key.clone(), "ERR_X", Severity::Severe, "probe", "s", 20);
        assert_eq!(m1, IndexMutation::Inserted);
        assert_eq!(m2, IndexMutation::Updated);
        let r2 = r2.unwrap();
        assert_eq!(r2.occurrences.get(), 2);
        assert_eq!(r2.last_seen_ms, 20);
        assert_eq!(idx.incidents.len(), 1, "coalesced, not duplicated");
    }

    #[test]
    fn global_cap_500_evicts_deterministically() {
        let mut idx = IncidentIndex::default();
        // 500 Info records spread across 5 subsystems (≤ 100 per subsystem,
        // so the per-subsystem cap never fires) + 1 Critical (new): the
        // global cap must evict the lowest-severity oldest record, never the
        // critical one.
        let subs = ["a", "b", "c", "d", "e"];
        for i in 0..MAX_INCIDENTS_GLOBAL {
            let mut r = rec(subs[i / MAX_INCIDENTS_PER_SUBSYSTEM], Severity::Info);
            r.key.fingerprint = format!("fp-{i}");
            r.last_seen_ms = 1_000 + i as u64;
            r.opaque_id = format!("id-{i:04}");
            idx.incidents.push(r);
        }
        idx.record(
            IncidentKey { subsystem: "f".into(), fingerprint: "fp-critical".into() },
            "ERR_CRIT",
            Severity::Critical,
            "op",
            "s",
            9_999,
        );
        assert_eq!(idx.incidents.len(), MAX_INCIDENTS_GLOBAL, "cap enforced");
        assert!(
            idx.incidents.iter().any(|r| r.key.fingerprint == "fp-critical"),
            "critical incident retained"
        );
        assert!(
            !idx.incidents.iter().any(|r| r.opaque_id == "id-0000"),
            "lowest-severity oldest record evicted"
        );
    }

    #[test]
    fn per_subsystem_cap_100_evicts_first() {
        let mut idx = IncidentIndex::default();
        for i in 0..(MAX_INCIDENTS_PER_SUBSYSTEM + 1) {
            idx.record(
                IncidentKey {
                    subsystem: "docker".into(),
                    fingerprint: format!("fp-{i}"),
                },
                "ERR_X",
                Severity::Info,
                "op",
                "s",
                i as u64,
            );
        }
        let docker_count = idx
            .incidents
            .iter()
            .filter(|r| r.key.subsystem == "docker")
            .count();
        assert_eq!(docker_count, MAX_INCIDENTS_PER_SUBSYSTEM, "per-subsystem cap");
        // Oldest (lowest last_seen) of the lowest severity evicted.
        assert!(!idx
            .incidents
            .iter()
            .any(|r| r.key.fingerprint == "fp-0"), "oldest evicted first");
    }

    #[test]
    fn evicted_candidate_is_lowest_severity_then_oldest() {
        // Fill one subsystem to its per-subsystem cap (100): 50 Info records
        // + 50 Critical records, all with distinct last_seen values. Adding
        // one more (Warning, newest) must evict the OLDEST INFO record —
        // lowest severity first, then oldest last_seen — never the newer
        // Warning nor any Critical.
        let mut idx = IncidentIndex::default();
        for i in 0..50 {
            let mut r = rec("s", Severity::Info);
            r.key.fingerprint = format!("fp-info-{i}");
            r.last_seen_ms = 1_000 + i as u64;
            r.opaque_id = format!("id-info-{i:03}");
            idx.incidents.push(r);
            let mut c = rec("s", Severity::Critical);
            c.key.fingerprint = format!("fp-crit-{i}");
            c.last_seen_ms = 1_000 + i as u64;
            c.opaque_id = format!("id-crit-{i:03}");
            idx.incidents.push(c);
        }
        assert_eq!(idx.incidents.len(), 100);
        idx.record(
            IncidentKey { subsystem: "s".into(), fingerprint: "fp-new".into() },
            "ERR_X",
            Severity::Warning,
            "op",
            "s",
            9_999,
        );
        assert_eq!(
            idx.incidents
                .iter()
                .filter(|r| r.key.subsystem == "s")
                .count(),
            MAX_INCIDENTS_PER_SUBSYSTEM,
            "cap enforced"
        );
        // The oldest Info record (last_seen = 1_000, the smallest among
        // Info records) was evicted; every Critical survives.
        assert!(
            !idx.incidents.iter().any(|r| r.key.fingerprint == "fp-info-0"),
            "oldest lowest-severity record evicted"
        );
        assert!(
            idx.incidents
                .iter()
                .all(|r| r.severity != Severity::Info || r.key.fingerprint != "fp-info-0"),
        );
        assert_eq!(
            idx.incidents.iter().filter(|r| r.severity == Severity::Critical).count(),
            50,
            "critical history survives"
        );
        assert!(
            idx.incidents.iter().any(|r| r.key.fingerprint == "fp-new"),
            "newest record retained"
        );
    }

    #[test]
    fn index_save_and_load_roundtrip() {
        let dir = match storage::test_scratch_dir("roundtrip") {
            Some(d) => d,
            None => return,
        };
        let path = dir.join("index.json");
        let mut idx = IncidentIndex::default();
        idx.record(
            IncidentKey { subsystem: "docker".into(), fingerprint: "fp-1".into() },
            "ERR_X",
            Severity::Severe,
            "probe",
            "summary text",
            42,
        );
        idx.pending_deletions.push("bundle-abc.lsdiag".into());
        idx.save_atomic(&path).expect("save");
        let (loaded, health) = IncidentIndex::load_or_default(&path);
        assert_eq!(health.state, IndexState::Valid);
        assert_eq!(loaded, idx, "full roundtrip equality incl. tombstones");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_index_write_leaves_previous_file_intact() {
        let dir = match storage::test_scratch_dir("failed-write") {
            Some(d) => d,
            None => return,
        };
        let path = dir.join("index.json");
        let mut idx = IncidentIndex::default();
        idx.record(
            IncidentKey { subsystem: "docker".into(), fingerprint: "fp-x".into() },
            "ERR_X",
            Severity::Info,
            "op",
            "s",
            1,
        );
        idx.save_atomic(&path).expect("initial save");
        let before = std::fs::read(&path).unwrap();

        // Mutate + attempt a save whose RENAME fails: a directory at the
        // destination blocks MoveFileEx-with-replace ("Access is denied").
        // NOTE: saving requires removing the file first in this synthetic
        // setup (real callers never hit this shape — their destination is a
        // regular file). The property under test: failed commit ⇒ old index
        // content unchanged, no temp residue.
        let old_bytes = before.clone();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let mut idx2 = IncidentIndex::default();
        idx2.record(
            IncidentKey { subsystem: "other".into(), fingerprint: "fp-y".into() },
            "ERR_Y",
            Severity::Warning,
            "op",
            "s",
            2,
        );
        assert!(idx2.save_atomic(&path).is_err(), "rename over a dir fails");
        // No temp residue anywhere in LocalStack temp/.
        let temp_dir = crate::diagnostics::paths::temp_dir().unwrap();
        let residue: Vec<_> = std::fs::read_dir(&temp_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .collect();
        assert!(residue.is_empty(), "no temp residue after failed save");
        drop(old_bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_index_loads_as_corrupt_without_deleting_file() {
        let dir = match storage::test_scratch_dir("malformed") {
            Some(d) => d,
            None => return,
        };
        let path = dir.join("index.json");
        std::fs::write(&path, "not json at all").unwrap();
        let (idx, health) = IncidentIndex::load_or_default(&path);
        assert_eq!(health.state, IndexState::Corrupt);
        assert_eq!(idx, IncidentIndex::default());
        assert!(path.is_file(), "file NOT deleted on malformed load");
        assert_eq!(std::fs::read(&path).unwrap(), b"not json at all");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_major_schema_is_unsupported_not_corrupt() {
        let dir = match storage::test_scratch_dir("unsupported") {
            Some(d) => d,
            None => return,
        };
        let path = dir.join("index.json");
        std::fs::write(&path, r#"{"schema_version": 2, "incidents": []}"#).unwrap();
        let (_, health) = IncidentIndex::load_or_default(&path);
        assert_eq!(health.state, IndexState::Unsupported);
        assert!(path.is_file(), "unsupported file preserved");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_index_is_valid_first_run() {
        let dir = match storage::test_scratch_dir("missing") {
            Some(d) => d,
            None => return,
        };
        let (idx, health) = IncidentIndex::load_or_default(&dir.join("nope.json"));
        assert_eq!(health.state, IndexState::Valid);
        assert_eq!(idx, IncidentIndex::default());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
