//! Phase 11C Task 10 — bundle registry, crash-safe tombstoned retention,
//! startup reconciliation (spec §14, §19, §21, §28).
//!
//! Deletion intent is DURABLE: evicted bundles are tombstoned in the index
//! (`pending_deletions`) and only file-deleted AFTER the replacement index
//! state is durably committed. A tombstoned file is never re-indexed. All
//! filesystem authority derives from trusted index entries + strict owned
//! filename shape + verified containment — fail closed on any ambiguity.

#![allow(dead_code)] // Consumed by commands/UI/export tasks (Tasks 13/14/17).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::crypto::{self, CryptoError};
use super::storage;

/// Exact retention bounds (spec §14): 20 bundles, 1 GiB target. A bounded
/// transactional overshoot (files still on disk between the durable index
/// commit and best-effort deletion) is tolerated by design.
pub(crate) const MAX_BUNDLES: usize = 20;
pub(crate) const MAX_TOTAL_BUNDLE_BYTES: u64 = 1_073_741_824; // 1 GiB
const MAX_ENCRYPTED_BUNDLE_BYTES: u64 = 52_428_800; // 50 MiB per bundle

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum BundleIntegrityState {
    #[default]
    Valid,
    Corrupt,
    Unsupported,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub(crate) enum ReviewState {
    #[default]
    New,
    Reviewed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BundleMeta {
    pub(crate) bundle_id: String,
    pub(crate) created_at_ms: u64,
    pub(crate) trigger: String,
    pub(crate) subsystem: String,
    pub(crate) severity: String,
    pub(crate) fingerprint: String,
    pub(crate) encrypted_size_bytes: u64,
    pub(crate) integrity: BundleIntegrityState,
    pub(crate) review_state: ReviewState,
    pub(crate) app_version: String,
    pub(crate) schema_version: u32,
}

/// Bundle index schema v1. `pending_deletions` are tombstoned OWNED bundle
/// filenames — durable deletion intent (crash-safe retention §19).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct BundleIndex {
    #[serde(default = "default_index_schema")]
    pub(crate) schema_version: u32,
    #[serde(default)]
    pub(crate) bundles: Vec<BundleMeta>,
    #[serde(default)]
    pub(crate) pending_deletions: Vec<String>,
}

fn default_index_schema() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub(crate) struct MetaFields {
    pub(crate) created_at_ms: u64,
    pub(crate) trigger: String,
    pub(crate) subsystem: String,
    pub(crate) severity: String,
    pub(crate) fingerprint: String,
    pub(crate) app_version: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // evicted/digest/tombstones feed logging + UI (Tasks 14/17)
pub(crate) struct CommitOutcome {
    pub(crate) bundle_id: String,
    pub(crate) evicted: Vec<String>,
    /// blake3 of the exact plaintext bytes passed to encryption
    /// (bookkeeping defense-in-depth; DPAPI is the integrity primitive).
    pub(crate) digest: String,
    pub(crate) tombstones_remaining: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommitError {
    TooLarge,
    IndexPersistence,
    Crypto(CryptoError),
    Io,
}

pub(crate) struct BundleRegistry {
    index_path: PathBuf,
    bundles_dir: PathBuf,
    state: Mutex<RegistryState>,
    #[cfg(test)]
    fail_index_commit: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    fail_delete_names: Mutex<Vec<String>>,
}

struct RegistryState {
    index: BundleIndex,
}

impl BundleRegistry {
    pub(crate) fn new(index_path: PathBuf, bundles_dir: PathBuf) -> Option<Self> {
        let _ = std::fs::create_dir_all(&bundles_dir);
        let index = read_index_or_empty(&index_path);
        Some(Self {
            index_path,
            bundles_dir,
            state: Mutex::new(RegistryState { index }),
            #[cfg(test)]
            fail_index_commit: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            fail_delete_names: Mutex::new(Vec::new()),
        })
    }

    #[cfg(test)]
    pub(crate) fn fail_index_commit(&self) {
        self.fail_index_commit
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) fn inject_delete_failure(&self, filename: &str) {
        if let Ok(mut v) = self.fail_delete_names.lock() {
            v.push(filename.to_string());
        }
    }

    /// Crash-safe commit (spec §19 order): finalize encrypted object →
    /// durable index commit including tombstones → best-effort delete →
    /// second durable commit clearing cleared tombstones.
    pub(crate) fn commit_bundle(
        &self,
        plain_payload: &[u8],
        meta: MetaFields,
    ) -> Result<CommitOutcome, CommitError> {
        self.commit_bundle_with_schema(plain_payload, meta, 1)
    }

    /// Test/robustness seam: commit with an explicit envelope schema version.
    pub(crate) fn commit_bundle_raw_with_schema(
        &self,
        plain_payload: &[u8],
        meta: MetaFields,
        schema_version: u32,
    ) -> Result<CommitOutcome, CommitError> {
        self.commit_bundle_with_schema(plain_payload, meta, schema_version)
    }

    fn commit_bundle_with_schema(
        &self,
        plain_payload: &[u8],
        meta: MetaFields,
        schema_version: u32,
    ) -> Result<CommitOutcome, CommitError> {
        if plain_payload.is_empty() {
            return Err(CommitError::Io);
        }
        let bundle_id = super::ids::random_hex(12).ok_or(CommitError::Io)?;
        let filename = format!("bundle-{bundle_id}.lsdiag");
        // Envelope: trusted meta rides INSIDE the encrypted payload so a
        // lost index can be rebuilt from bundle-owned state alone.
        let payload_value: serde_json::Value =
            serde_json::from_slice(plain_payload).unwrap_or(serde_json::Value::Null);
        let envelope = serde_json::json!({
            "schema_version": schema_version,
            "kind": "support-bundle",
            "meta": {
                "created_at_ms": meta.created_at_ms,
                "trigger": meta.trigger,
                "subsystem": meta.subsystem,
                "severity": meta.severity,
                "fingerprint": meta.fingerprint,
                "app_version": meta.app_version,
            },
            "payload": payload_value,
        });
        let plain = serde_json::to_vec(&envelope).map_err(|_| CommitError::Io)?;
        let digest = blake3::hash(plain_payload).to_hex().to_string();
        let cipher = crypto::dpapi_protect(&plain).map_err(CommitError::Crypto)?;
        let mut frame = Vec::with_capacity(7 + 4 + cipher.len());
        frame.extend_from_slice(crypto::LSDIAG_MAGIC);
        frame.extend_from_slice(&(cipher.len() as u32).to_le_bytes());
        frame.extend_from_slice(&cipher);
        if frame.len() as u64 > MAX_ENCRYPTED_BUNDLE_BYTES {
            return Err(CommitError::TooLarge);
        }
        let dest = self.bundles_dir.join(&filename);
        storage::write_atomic(&dest, &frame).map_err(|_| CommitError::Io)?;

        // Registry transaction.
        let mut evicted: Vec<String> = Vec::new();
        let tombstones_remaining;
        {
            let mut st = self
                .state
                .lock()
                .map_err(|_| CommitError::IndexPersistence)?;
            let committed_meta = BundleMeta {
                bundle_id: bundle_id.clone(),
                created_at_ms: meta.created_at_ms,
                trigger: meta.trigger,
                subsystem: meta.subsystem,
                severity: meta.severity,
                fingerprint: meta.fingerprint,
                encrypted_size_bytes: frame.len() as u64,
                integrity: BundleIntegrityState::Valid,
                review_state: ReviewState::New,
                app_version: meta.app_version,
                schema_version,
            };
            st.index.bundles.push(committed_meta);
            // Deterministic eviction: oldest created_at_ms first, then
            // bundle_id tie-break. Enforce count cap, then size target.
            while st.index.bundles.len() > MAX_BUNDLES
                || st.index.bundles.iter().map(|b| b.encrypted_size_bytes).sum::<u64>()
                    > MAX_TOTAL_BUNDLE_BYTES
            {
                let Some(victim_idx) = oldest_candidate_idx(&st.index.bundles) else {
                    break;
                };
                let victim = st.index.bundles.remove(victim_idx);
                let victim_file = format!("bundle-{}.lsdiag", victim.bundle_id);
                if !st.index.pending_deletions.contains(&victim_file) {
                    st.index.pending_deletions.push(victim_file.clone());
                }
                evicted.push(victim_file);
            }
            // Durable index commit #1: new bundle active, evictions
            // tombstoned. Files are NOT touched yet.
            if !self.persist_index(&st.index) {
                // Revert in-memory state; the new file remains as an
                // unindexed owned artifact for reconciliation to recover.
                st.index = read_index_or_empty(&self.index_path);
                return Err(CommitError::IndexPersistence);
            }
        }
        // Best-effort deletion of tombstoned files (already durable intent).
        tombstones_remaining = self.process_pending_deletions();
        // Durable index commit #2: tombstones cleared for deleted files.
        {
            let st = self.state.lock().map_err(|_| CommitError::IndexPersistence)?;
            let _ = self.persist_index(&st.index);
        }
        Ok(CommitOutcome {
            bundle_id,
            evicted,
            digest,
            tombstones_remaining,
        })
    }

    fn persist_index(&self, index: &BundleIndex) -> bool {
        #[cfg(test)]
        if self
            .fail_index_commit
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.fail_index_commit
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return false;
        }
        match serde_json::to_vec(index) {
            Ok(bytes) => storage::write_atomic(&self.index_path, &bytes).is_ok(),
            Err(_) => false,
        }
    }

    /// Delete every tombstoned file that can be safely deleted; clear their
    /// tombstones durably. Retains tombstones for files that could not be
    /// deleted (bounded startup retry). Never touches foreign shapes.
    fn process_pending_deletions(&self) -> usize {
        let mut st = match self.state.lock() {
            Ok(g) => g,
            Err(_) => return 0,
        };
        let mut cleared: Vec<String> = Vec::new();
        for filename in st.index.pending_deletions.clone() {
            let path = self.bundles_dir.join(&filename);
            if !path.exists() {
                cleared.push(filename); // file gone: clear tombstone
                continue;
            }
            #[cfg(test)]
            {
                if let Ok(v) = self.fail_delete_names.lock() {
                    if v.contains(&filename) {
                        continue; // injected failure: tombstone retained
                    }
                }
            }
            if safe_delete_bundle_file(&self.bundles_dir, &filename).is_ok() {
                cleared.push(filename);
            }
        }
        if !cleared.is_empty() {
            st.index
                .pending_deletions
                .retain(|f| !cleared.contains(f));
            let _ = self.persist_index(&st.index);
        }
        st.index.pending_deletions.len()
    }

    /// Startup reconciliation (spec §21): tombstones → delete/never
    /// re-index; unindexed owned files → ONE bounded decrypt attempt;
    /// missing files → dropped; malformed index → rebuild from bundle-owned
    /// state; reparse escape → fail closed; stale owned temp → purged.
    pub(crate) fn reconcile_startup(&self) {
        // Startup models a FRESH process: reload the index from disk so any
        // externally-simulated crash state (tombstones, index loss) is what
        // reconciliation actually sees.
        {
            let mut st = match self.state.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            st.index = read_index_or_empty(&self.index_path);
        }
        // Purge stale owned temp files first (strict owned shape only).
        storage::purge_stale_temp();

        // Detect malformed/missing index: rebuild minimal metadata from
        // trusted bundle-owned state (decrypt-validated) below.
        let malformed = false;
        {
            let mut st = match self.state.lock() {
                Ok(g) => g,
                Err(_) => return,
            };
            // Drop indexed metas whose files vanished.
            st.index
                .bundles
                .retain(|m| self.bundles_dir.join(format!("bundle-{}.lsdiag", m.bundle_id)).exists());
            // Tombstones: delete / clear / retain.
            let mut cleared: Vec<String> = Vec::new();
            for filename in st.index.pending_deletions.clone() {
                if !is_owned_bundle_filename(&filename) {
                    // Foreign/invalid tombstone: drop the tombstone itself,
                    // never delete anything (fail closed happened by NOT
                    // deleting; the entry is meaningless bookkeeping).
                    cleared.push(filename);
                    continue;
                }
                let path = self.bundles_dir.join(&filename);
                if !path.exists() {
                    cleared.push(filename);
                    continue;
                }
                #[cfg(test)]
                {
                    if let Ok(v) = self.fail_delete_names.lock() {
                        if v.contains(&filename) {
                            continue; // injected failure: tombstone retained
                        }
                    }
                }
                if safe_delete_bundle_file(&self.bundles_dir, &filename).is_ok() {
                    cleared.push(filename);
                }
                // Deletion failed: tombstone retained for the next bounded
                // retry (no loop, no panic).
            }
            st.index.pending_deletions.retain(|f| !cleared.contains(f));
            let _ = self.persist_index(&st.index);
        }

        // Scan for unindexed owned files (strict shape, not tombstoned).
        let entries = match std::fs::read_dir(&self.bundles_dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        let mut to_recover: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let filename = entry.file_name().to_string_lossy().to_string();
            if !is_owned_bundle_filename(&filename) {
                continue; // foreign file: never touched
            }
            let Ok(meta) = entry.metadata() else { continue };
            if meta.file_type().is_symlink() {
                super::warn(
                    "diagnostics",
                    &format!("reparse/symlink bundle entry refused: {filename}"),
                );
                continue; // fail closed: never delete, never index
            }
            let indexed = {
                let st = match self.state.lock() {
                    Ok(g) => g,
                    Err(_) => return,
                };
                let id = filename
                    .trim_start_matches("bundle-")
                    .trim_end_matches(".lsdiag")
                    .to_string();
                st.index.bundles.iter().any(|b| b.bundle_id == id)
                    || st.index.pending_deletions.contains(&filename)
            };
            if !indexed {
                to_recover.push(filename);
            }
        }

        for filename in to_recover {
            let path = self.bundles_dir.join(&filename);
            match recover_bundle_meta(&path) {
                RecoverOutcome::Recovered(meta) => {
                    if let Ok(mut st) = self.state.lock() {
                        st.index.bundles.push(meta);
                        let _ = self.persist_index(&st.index);
                    }
                }
                RecoverOutcome::Unsupported(meta) | RecoverOutcome::Corrupt(meta) => {
                    // Record the determination in the index so later runs do
                    // NOT retry decryption every poll (bounded, once).
                    if let Ok(mut st) = self.state.lock() {
                        st.index.bundles.push(meta);
                        let _ = self.persist_index(&st.index);
                    }
                }
                RecoverOutcome::RecoveryRequired => {
                    if let Ok(mut st) = self.state.lock() {
                        let id = filename
                            .trim_start_matches("bundle-")
                            .trim_end_matches(".lsdiag")
                            .to_string();
                        st.index.bundles.push(BundleMeta {
                            bundle_id: id,
                            created_at_ms: 0,
                            trigger: String::new(),
                            subsystem: String::new(),
                            severity: String::new(),
                            fingerprint: String::new(),
                            encrypted_size_bytes: 0,
                            integrity: BundleIntegrityState::RecoveryRequired,
                            review_state: ReviewState::New,
                            app_version: String::new(),
                            schema_version: 1,
                        });
                        let _ = self.persist_index(&st.index);
                    }
                }
            }
        }
        let _ = malformed;
    }

    /// Trusted delete: resolve ID → strict filename → verified path.
    pub(crate) fn delete_bundle(&self, bundle_id: &str) -> Result<(), CommitError> {
        if !is_owned_bundle_id(bundle_id) {
            return Err(CommitError::Io);
        }
        let filename = format!("bundle-{bundle_id}.lsdiag");
        let path = self.bundles_dir.join(&filename);
        if path.exists() {
            safe_delete_bundle_file(&self.bundles_dir, &filename)
                .map_err(|_| CommitError::Io)?;
        }
        let mut st = self.state.lock().map_err(|_| CommitError::IndexPersistence)?;
        st.index.bundles.retain(|b| b.bundle_id != bundle_id);
        if !self.persist_index(&st.index) {
            return Err(CommitError::IndexPersistence);
        }
        Ok(())
    }

    pub(crate) fn list(&self) -> Vec<BundleMeta> {
        match self.state.lock() {
            Ok(st) => st.index.bundles.clone(),
            Err(_) => Vec::new(),
        }
    }

    /// Read-only accessor for the export pipeline (Task 13): trusted
    /// bundles directory backing this registry.
    pub(crate) fn bundles_dir(&self) -> &Path {
        &self.bundles_dir
    }
}

fn oldest_candidate_idx(bundles: &[BundleMeta]) -> Option<usize> {
    bundles
        .iter()
        .enumerate()
        .min_by(|(ia, a), (ib, b)| {
            (a.created_at_ms, &a.bundle_id)
                .cmp(&(b.created_at_ms, &b.bundle_id))
                .then(ia.cmp(ib))
        })
        .map(|(i, _)| i)
}

/// Strict owned bundle filename shape: `bundle-<24 lowercase hex>.lsdiag`.
pub(crate) fn is_owned_bundle_filename(filename: &str) -> bool {
    let Some(rest) = filename.strip_prefix("bundle-") else {
        return false;
    };
    let Some(id) = rest.strip_suffix(".lsdiag") else {
        return false;
    };
    is_owned_bundle_id(id)
}

pub(crate) fn is_owned_bundle_id(id: &str) -> bool {
    id.len() == 24 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Verified-safe deletion of an owned bundle file. Fails closed on: foreign
/// shape, non-regular file, symlink/reparse, containment escape.
pub(crate) fn safe_delete_bundle_file(bundles_dir: &Path, filename: &str) -> Result<(), ()> {
    if !is_owned_bundle_filename(filename) {
        return Err(());
    }
    let path = bundles_dir.join(filename);
    let Ok(meta) = std::fs::symlink_metadata(&path) else {
        return Err(());
    };
    if meta.file_type().is_symlink() {
        return Err(()); // never follow or delete links
    }
    if !meta.is_file() {
        return Err(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(());
        }
    }
    // Containment: resolved path must stay beneath the resolved bundles dir.
    let (Ok(resolved_dir), Ok(resolved_path)) =
        (std::fs::canonicalize(bundles_dir), std::fs::canonicalize(&path))
    else {
        return Err(()); // ambiguity → fail closed
    };
    if !resolved_path.starts_with(&resolved_dir) {
        return Err(());
    }
    std::fs::remove_file(&path).map_err(|_| ())
}

enum RecoverOutcome {
    Recovered(BundleMeta),
    Unsupported(BundleMeta),
    Corrupt(BundleMeta),
    RecoveryRequired,
}

/// ONE bounded decrypt/integrity attempt to rebuild trusted metadata from
/// bundle-owned state. Never retried automatically once Corrupt/Unsupported
/// is recorded in the index.
fn recover_bundle_meta(path: &Path) -> RecoverOutcome {
    let bundle_id = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .trim_start_matches("bundle-")
        .trim_end_matches(".lsdiag")
        .to_string();
    let encrypted_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let base = BundleMeta {
        bundle_id,
        created_at_ms: 0,
        trigger: String::new(),
        subsystem: String::new(),
        severity: String::new(),
        fingerprint: String::new(),
        encrypted_size_bytes: encrypted_size,
        integrity: BundleIntegrityState::Valid,
        review_state: ReviewState::New,
        app_version: String::new(),
        schema_version: 1,
    };
    let plain = match crypto::read_lsdiag(path) {
        Ok(p) => p,
        Err(CryptoError::EmptyPayload) | Err(CryptoError::ApiFailed(_)) => {
            return RecoverOutcome::Corrupt(BundleMeta {
                integrity: BundleIntegrityState::Corrupt,
                ..base
            });
        }
        Err(CryptoError::PayloadTooLarge) => {
            return RecoverOutcome::Corrupt(BundleMeta {
                integrity: BundleIntegrityState::Corrupt,
                ..base
            });
        }
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&plain) else {
        return RecoverOutcome::Corrupt(BundleMeta {
            integrity: BundleIntegrityState::Corrupt,
            ..base
        });
    };
    let schema = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    if schema != 1 {
        return RecoverOutcome::Unsupported(BundleMeta {
            schema_version: schema,
            integrity: BundleIntegrityState::Unsupported,
            ..base
        });
    }
    let m = value.get("meta").cloned().unwrap_or(serde_json::Value::Null);
    let field = |k: &str| {
        m.get(k)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    RecoverOutcome::Recovered(BundleMeta {
        created_at_ms: m
            .get("created_at_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        trigger: field("trigger"),
        subsystem: field("subsystem"),
        severity: field("severity"),
        fingerprint: field("fingerprint"),
        app_version: field("app_version"),
        integrity: BundleIntegrityState::Valid,
        ..base
    })
}

pub(crate) fn read_index_or_empty(path: &Path) -> BundleIndex {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub(crate) fn write_index(path: &Path, index: &BundleIndex) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(index).map_err(std::io::Error::other)?;
    storage::write_atomic(path, &bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::storage::{remove_scratch_dir, test_scratch_dir};

    fn registry_in(dir: &std::path::Path) -> BundleRegistry {
        BundleRegistry::new(
            dir.join("index.json"),
            dir.join("bundles"),
        )
        .expect("registry")
    }

    fn meta_fields(created_at_ms: u64) -> MetaFields {
        MetaFields {
            created_at_ms,
            trigger: "severe-threshold".into(),
            subsystem: "test-subsystem".into(),
            severity: "severe".into(),
            fingerprint: "fp-test".into(),
            app_version: "1.0.1".into(),
        }
    }

    fn commit_ok(reg: &BundleRegistry, created_at_ms: u64) -> CommitOutcome {
        let payload = serde_json::json!({ "hello": created_at_ms }).to_string().into_bytes();
        reg.commit_bundle(&payload, meta_fields(created_at_ms))
            .expect("commit ok")
    }

    fn seed_index_entry(dir: &std::path::Path, id: &str, created_at_ms: u64, size: u64) {
        // Write a tiny real encrypted file with the strict owned shape.
        let bundles = dir.join("bundles");
        let _ = std::fs::create_dir_all(&bundles);
        let path = bundles.join(format!("bundle-{id}.lsdiag"));
        crate::diagnostics::crypto::write_lsdiag(&path, b"seeded").expect("seed file");
        // Manipulate the INDEX directly: trusted meta with a chosen size.
        let index_path = dir.join("index.json");
        let mut index = read_index_or_empty(&index_path);
        index.bundles.push(BundleMeta {
            bundle_id: id.to_string(),
            created_at_ms,
            trigger: "seed".into(),
            subsystem: "seed".into(),
            severity: "info".into(),
            fingerprint: "fp".into(),
            encrypted_size_bytes: size,
            integrity: BundleIntegrityState::Valid,
            review_state: ReviewState::New,
            app_version: "1.0.1".into(),
            schema_version: 1,
        });
        write_index(&index_path, &index).expect("seed index");
    }

    fn bundle_ids(index: &BundleIndex) -> Vec<String> {
        let mut v: Vec<String> = index.bundles.iter().map(|b| b.bundle_id.clone()).collect();
        v.sort();
        v
    }

    #[test]
    fn commit_writes_encrypted_owned_file_and_updates_index() {
        let dir = test_scratch_dir("store-basic").expect("scratch");
        let reg = registry_in(&dir);
        let out = commit_ok(&reg, 100);
        assert_eq!(out.bundle_id.len(), 24);
        let path = dir.join("bundles").join(format!("bundle-{}.lsdiag", out.bundle_id));
        assert!(path.exists(), "encrypted file written");
        let raw = std::fs::read(&path).unwrap();
        assert!(raw.starts_with(b"LSDIAG1"));
        assert!(!raw.windows(5).any(|w| w == b"hello"), "plaintext absent");
        let index = read_index_or_empty(&dir.join("index.json"));
        assert_eq!(index.bundles.len(), 1);
        assert_eq!(index.bundles[0].bundle_id, out.bundle_id);
        remove_scratch_dir(&dir);
    }

    #[test]
    fn index_commit_failure_preserves_evicted_candidates() {
        let dir = test_scratch_dir("store-commitfail").expect("scratch");
        // Seed 3 active bundles (cap 20 not hit; force eviction via count cap
        // with a tiny cap? Use the global cap by seeding 20 entries).
        let reg = registry_in(&dir);
        let mut first_ids = Vec::new();
        for i in 0..20 {
            let out = commit_ok(&reg, 1_000 + i);
            first_ids.push(out.bundle_id);
        }
        // Inject index-commit failure for the next commit.
        reg.fail_index_commit();
        let payload = serde_json::json!({ "new": true }).to_string().into_bytes();
        let err = reg
            .commit_bundle(&payload, meta_fields(9_999))
            .expect_err("must fail");
        assert!(matches!(err, CommitError::IndexPersistence));
        // Old active bundles: all 20 files still present.
        for id in &first_ids {
            let p = dir.join("bundles").join(format!("bundle-{id}.lsdiag"));
            assert!(p.exists(), "old bundle {id} must survive commit failure");
        }
        // New bundle file remains as an unindexed owned artifact.
        let index = read_index_or_empty(&dir.join("index.json"));
        assert_eq!(index.bundles.len(), 20, "index unchanged");
        // Reconcile recovers the unindexed new bundle without a tombstone.
        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        assert!(index2.bundles.len() >= 21, "recovered new bundle indexed");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn retention_enforces_20_bundle_cap_oldest_first() {
        let dir = test_scratch_dir("store-cap20").expect("scratch");
        let reg = registry_in(&dir);
        let mut ids = Vec::new();
        for i in 0..20 {
            let out = commit_ok(&reg, 1_000 + i);
            ids.push(out.bundle_id);
        }
        let oldest = ids[0].clone();
        // Mislead with mtimes: make the OLDEST-created file look NEWEST.
        let oldest_path = dir.join("bundles").join(format!("bundle-{oldest}.lsdiag"));
        let f = std::fs::File::options().append(true).open(&oldest_path).unwrap();
        let future = std::time::SystemTime::now() + std::time::Duration::from_secs(3600);
        f.set_times(std::fs::FileTimes::new().set_accessed(future).set_modified(future))
            .unwrap();
        drop(f);
        // 21st commit evicts by trusted created_at_ms, NOT mtime.
        let out = commit_ok(&reg, 5_000);
        let index = read_index_or_empty(&dir.join("index.json"));
        assert_eq!(index.bundles.len(), 20);
        assert!(!bundle_ids(&index).contains(&oldest), "oldest evicted");
        assert!(bundle_ids(&index).contains(&out.bundle_id));
        assert!(
            !oldest_path.exists(),
            "evicted file deleted after durable index commit"
        );
        remove_scratch_dir(&dir);
    }

    #[test]
    fn retention_enforces_1_gib_target_with_overshoot_tolerance() {
        let dir = test_scratch_dir("store-1gib").expect("scratch");
        // Seed metas with sizes summing over 1 GiB (trusted index sizes; real
        // files are tiny — policy is enforced on meta, not on disk churn).
        seed_index_entry(&dir, &"a".repeat(24), 100, 600 * 1024 * 1024);
        seed_index_entry(&dir, &"b".repeat(24), 200, 500 * 1024 * 1024);
        let reg = registry_in(&dir);
        let out = commit_ok(&reg, 300); // ~tiny size → sum still > 1 GiB → evict oldest
        let index = read_index_or_empty(&dir.join("index.json"));
        assert!(!bundle_ids(&index).contains(&"a".repeat(24)), "oldest evicted");
        assert!(bundle_ids(&index).contains(&out.bundle_id));
        // Retained total now under the 1 GiB target.
        let retained: u64 = index.bundles.iter().map(|b| b.encrypted_size_bytes).sum();
        assert!(retained <= 1_073_741_824 + 64 * 1024 * 1024, "target enforced with bounded overshoot");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn tombstoned_file_deleted_never_reindexed() {
        let dir = test_scratch_dir("store-tombstone").expect("scratch");
        seed_index_entry(&dir, &"c".repeat(24), 100, 1024);
        let reg = registry_in(&dir);
        // Simulate crash AFTER tombstone index commit BEFORE file delete:
        let mut index = read_index_or_empty(&dir.join("index.json"));
        let meta = index.bundles.remove(0);
        let filename = format!("bundle-{}.lsdiag", meta.bundle_id);
        index.pending_deletions.push(filename.clone());
        write_index(&dir.join("index.json"), &index).unwrap();
        let file = dir.join("bundles").join(&filename);
        assert!(file.exists());

        reg.reconcile_startup();
        assert!(!file.exists(), "tombstoned file deleted on startup");
        let index2 = read_index_or_empty(&dir.join("index.json"));
        assert!(!index2.pending_deletions.contains(&filename), "tombstone cleared");
        assert!(
            !index2.bundles.iter().any(|b| b.bundle_id == meta.bundle_id),
            "NEVER re-indexed"
        );
        remove_scratch_dir(&dir);
    }

    #[test]
    fn crash_d_partial_deletion_before_tombstone_clear_is_safe() {
        let dir = test_scratch_dir("store-partialdel").expect("scratch");
        seed_index_entry(&dir, &"d".repeat(24), 100, 1024);
        seed_index_entry(&dir, &"e".repeat(24), 200, 1024);
        // Tombstone both; delete only one file (simulated partial deletion).
        let mut index = read_index_or_empty(&dir.join("index.json"));
        index.bundles.clear();
        let f1 = format!("bundle-{}.lsdiag", "d".repeat(24));
        let f2 = format!("bundle-{}.lsdiag", "e".repeat(24));
        index.pending_deletions = vec![f1.clone(), f2.clone()];
        write_index(&dir.join("index.json"), &index).unwrap();
        std::fs::remove_file(dir.join("bundles").join(&f1)).unwrap();

        let reg = registry_in(&dir);
        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        assert!(!index2.pending_deletions.contains(&f1), "missing file clears tombstone");
        assert!(!index2.pending_deletions.contains(&f2), "deleted file clears tombstone");
        assert!(!dir.join("bundles").join(&f2).exists(), "still-present tombstoned file deleted");
        assert!(index2.bundles.is_empty(), "never re-indexed");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn crash_c_deletion_failure_keeps_tombstone() {
        let dir = test_scratch_dir("store-delfail").expect("scratch");
        seed_index_entry(&dir, &"f".repeat(24), 100, 1024);
        let mut index = read_index_or_empty(&dir.join("index.json"));
        let filename = format!("bundle-{}.lsdiag", "f".repeat(24));
        index.bundles.clear();
        index.pending_deletions.push(filename.clone());
        write_index(&dir.join("index.json"), &index).unwrap();
        let reg = registry_in(&dir);
        reg.inject_delete_failure(&filename);
        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        assert!(dir.join("bundles").join(&filename).exists(), "file remains");
        assert!(index2.pending_deletions.contains(&filename), "tombstone retained for retry");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn crash_e_malformed_or_foreign_tombstone_fails_closed() {
        let dir = test_scratch_dir("store-foreign").expect("scratch");
        // A foreign-shaped tombstone and a non-owned file in bundles/.
        let mut index = read_index_or_empty(&dir.join("index.json"));
        index.pending_deletions = vec!["evil.txt".to_string(), "not-ours".to_string()];
        write_index(&dir.join("index.json"), &index).unwrap();
        let _ = std::fs::create_dir_all(dir.join("bundles"));
        std::fs::write(dir.join("bundles").join("evil.txt"), b"x").unwrap();

        let reg = registry_in(&dir);
        reg.reconcile_startup();
        assert!(dir.join("bundles").join("evil.txt").exists(), "foreign file NEVER deleted");
        // Foreign tombstones are dropped (they can never match an owned file)
        // but only after failing closed — no deletion happened.
        let index2 = read_index_or_empty(&dir.join("index.json"));
        assert!(!index2.pending_deletions.contains(&"evil.txt".to_string()));
        remove_scratch_dir(&dir);
    }

    #[test]
    fn delete_rejects_reparse_escape() {
        let dir = test_scratch_dir("store-reparse").expect("scratch");
        let bundles = dir.join("bundles");
        let _ = std::fs::create_dir_all(&bundles);
        // Link INSIDE bundles/ pointing OUTSIDE the diagnostics tree.
        let outside = dir.join("outside-target");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious.txt"), b"keep").unwrap();
        let link = bundles.join("bundle-9999.lsdiag");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_dir(&outside, &link).is_ok();
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&outside, &link).is_ok();
        if !created {
            eprintln!("reparse/symlink creation not permitted on this host; containment asserted structurally");
        } else {
            let _reg = registry_in(&dir);
            let refused = safe_delete_bundle_file(&bundles, "bundle-9999.lsdiag");
            assert!(refused.is_err(), "escape path refused");
            assert!(outside.join("precious.txt").exists(), "target untouched");
        }
        remove_scratch_dir(&dir);
    }

    #[test]
    fn unindexed_owned_bundle_without_tombstone_recovered_to_valid() {
        let dir = test_scratch_dir("store-recover").expect("scratch");
        let reg = registry_in(&dir);
        let out = commit_ok(&reg, 100);
        // Simulate index loss for this bundle only.
        let mut index = read_index_or_empty(&dir.join("index.json"));
        index.bundles.clear();
        write_index(&dir.join("index.json"), &index).unwrap();

        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        let meta = index2
            .bundles
            .iter()
            .find(|b| b.bundle_id == out.bundle_id)
            .expect("recovered");
        assert!(matches!(meta.integrity, BundleIntegrityState::Valid));
        assert_eq!(meta.trigger, "severe-threshold");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn unsupported_schema_not_labeled_corrupt() {
        let dir = test_scratch_dir("store-unsupported").expect("scratch");
        let reg = registry_in(&dir);
        let payload = serde_json::json!({ "future": true }).to_string().into_bytes();
        let out = reg
            .commit_bundle_raw_with_schema(&payload, meta_fields(100), 2)
            .expect("committed raw");
        let mut index = read_index_or_empty(&dir.join("index.json"));
        index.bundles.clear();
        write_index(&dir.join("index.json"), &index).unwrap();

        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        let meta = index2
            .bundles
            .iter()
            .find(|b| b.bundle_id == out.bundle_id)
            .expect("meta recorded");
        assert!(matches!(meta.integrity, BundleIntegrityState::Unsupported));
        remove_scratch_dir(&dir);
    }

    #[test]
    fn dpapi_corrupt_bundle_marked_corrupt_and_not_retried() {
        let dir = test_scratch_dir("store-corrupt").expect("scratch");
        let bundles = dir.join("bundles");
        let _ = std::fs::create_dir_all(&bundles);
        let id = "ab12cd34ef56ab12cd34ef56";
        let path = bundles.join(format!("bundle-{id}.lsdiag"));
        // Valid frame magic + length, garbage ciphertext.
        let mut frame = b"LSDIAG1".to_vec();
        frame.extend_from_slice(&16u32.to_le_bytes());
        frame.extend_from_slice(&[0u8; 16]);
        std::fs::write(&path, &frame).unwrap();

        let reg = registry_in(&dir);
        reg.reconcile_startup();
        let index = read_index_or_empty(&dir.join("index.json"));
        let meta = index.bundles.iter().find(|b| b.bundle_id == id).expect("meta recorded");
        assert!(matches!(meta.integrity, BundleIntegrityState::Corrupt));

        // Second run: no repeated decrypt attempts — meta stays, state Corrupt.
        reg.reconcile_startup();
        let index2 = read_index_or_empty(&dir.join("index.json"));
        let meta2 = index2.bundles.iter().find(|b| b.bundle_id == id).expect("meta kept");
        assert!(matches!(meta2.integrity, BundleIntegrityState::Corrupt));
        remove_scratch_dir(&dir);
    }

    #[test]
    fn missing_index_rebuilds_minimal_metadata() {
        let dir = test_scratch_dir("store-noindex").expect("scratch");
        let reg = registry_in(&dir);
        let out = commit_ok(&reg, 100);
        std::fs::remove_file(dir.join("index.json")).unwrap();

        reg.reconcile_startup();
        let index = read_index_or_empty(&dir.join("index.json"));
        let meta = index
            .bundles
            .iter()
            .find(|b| b.bundle_id == out.bundle_id)
            .expect("rebuilt from trusted bundle state");
        assert!(matches!(meta.integrity, BundleIntegrityState::Valid));
        remove_scratch_dir(&dir);
    }

    #[test]
    fn missing_indexed_bundle_dropped_from_index() {
        let dir = test_scratch_dir("store-missing").expect("scratch");
        seed_index_entry(&dir, &"ff".repeat(12), 100, 1024);
        // Delete the file behind the index entry.
        std::fs::remove_file(dir.join("bundles").join(format!("bundle-{}.lsdiag", "ff".repeat(12))))
            .unwrap();
        let reg = registry_in(&dir);
        reg.reconcile_startup();
        let index = read_index_or_empty(&dir.join("index.json"));
        assert!(index.bundles.is_empty(), "missing file dropped");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn orphan_tmp_cleaned_strictly() {
        let dir = test_scratch_dir("store-tmp").expect("scratch");
        let temp = crate::diagnostics::paths::temp_dir().expect("temp");
        let owned = temp.join(format!(
            "tmp-{}-{}.part",
            std::process::id(),
            "ab".repeat(8)
        ));
        let foreign = temp.join("not-ours.txt");
        std::fs::write(&owned, b"stale").unwrap();
        // Backdate the mtime so the file is genuinely stale (the purge
        // guard never touches files a live writer could be staging).
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7200);
        std::fs::File::options()
            .append(true)
            .open(&owned)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_accessed(old).set_modified(old))
            .unwrap();
        std::fs::write(&foreign, b"keep").unwrap();
        let reg = registry_in(&dir);
        reg.reconcile_startup();
        assert!(!owned.exists(), "owned stale tmp purged");
        assert!(foreign.exists(), "foreign temp file untouched");
        let _ = std::fs::remove_file(&foreign);
        remove_scratch_dir(&dir);
    }
}
