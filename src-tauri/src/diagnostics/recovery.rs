//! Phase 11C Task 9 — startup crash-recovery pipeline (spec §7).
//! RED tests first.

//! Phase 11C Task 9 — startup crash-recovery pipeline (spec §7).
//!
//! Sequence: detect emergency marker → validate size/schema → re-redact
//! (defense-in-depth) → enrich (omission-safe) → build bundle → write via
//! injected closure (encrypt+persist+index) → ONLY THEN delete the marker →
//! banner. Bounded to 3 attempts; on the 3rd failure the marker moves into
//! `failed/` and retries stop. Startup is never blocked.

#![allow(dead_code)] // Consumed by commands/UI (Tasks 14/17/18).

use serde::Deserialize;
use std::path::PathBuf;

/// Exact emergency-record byte cap (mirrors emergency.rs).
const EMERGENCY_RECORD_MAX_BYTES: usize = 524_288;
/// Maximum recovery attempts before escalation to `failed/`.
const MAX_RECOVERY_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum RecoveryBanner {
    #[default]
    None,
    /// Previous crash was finalized into a support bundle.
    Recovered,
    /// Recovery failed 3 times; marker escalated to `failed/`.
    RecoveryFailed,
    /// Marker was present but invalid (size/schema); discarded safely.
    InvalidMarker,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RecoveryOutcome {
    pub(crate) recovered_bundle_id: Option<String>,
    pub(crate) attempts_used: u8,
    pub(crate) attempts_recorded: u8,
    pub(crate) moved_to_failed: bool,
    pub(crate) banner: RecoveryBanner,
}

/// Injectable dependencies (task 10 supplies the real write_bundle closure
/// with index registration; task 18 supplies the real banner notifier).
pub(crate) struct RecoveryDeps {
    pub(crate) record_bytes: Option<Vec<u8>>,
    pub(crate) record_exists: bool,
    pub(crate) app_version: String,
    pub(crate) attempt_state_path: PathBuf,
    pub(crate) marker_path: PathBuf,
    pub(crate) failed_dir: PathBuf,
    /// Encrypt + persist + index the payload; returns the opaque bundle ID.
    pub(crate) write_bundle: Box<dyn Fn(&[u8], &str) -> Result<String, String>>,
    /// Optional enrichment (Task 5 snapshot); errors/panics are omitted.
    pub(crate) enrich: Option<Box<dyn Fn() -> Result<serde_json::Value, String>>>,
    pub(crate) set_banner: Box<dyn Fn(RecoveryOutcome)>,
}

#[derive(Deserialize)]
struct EmergencySchemaGate {
    schema_version: u32,
}

/// Run the recovery pipeline. Never blocks startup; every failure is typed.
pub(crate) fn finalize_pending(deps: RecoveryDeps) -> RecoveryOutcome {
    if !deps.record_exists {
        return RecoveryOutcome::default();
    }
    let bytes = match deps.record_bytes {
        Some(ref b) => b.clone(),
        None => return RecoveryOutcome::default(),
    };

    // Validate size + schema (strict gate BEFORE any work).
    let valid = bytes.len() <= EMERGENCY_RECORD_MAX_BYTES
        && serde_json::from_slice::<EmergencySchemaGate>(&bytes)
            .map(|g| g.schema_version == 1)
            .unwrap_or(false);
    if !valid {
        let moved = move_marker_to_failed(&deps, "invalid-marker");
        let outcome = RecoveryOutcome {
            recovered_bundle_id: None,
            attempts_used: 0,
            attempts_recorded: read_attempts(&deps.attempt_state_path),
            moved_to_failed: moved,
            banner: RecoveryBanner::InvalidMarker,
        };
        clear_attempts(&deps.attempt_state_path);
        (deps.set_banner)(outcome.clone());
        return outcome;
    }

    // Re-redact defensively (every string passes the redactor again).
    let mut record: serde_json::Value =
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    re_redact(&mut record);

    // Enrich — omission-safe: failure or panic yields no enrichment section.
    let enriched = deps.enrich.as_ref().and_then(|f| {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
            .ok()
            .and_then(|r| r.ok())
    });

    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "crash-recovery",
        "app_version": deps.app_version,
        "emergency": record,
        "enriched": enriched,
    });
    let payload_bytes = serde_json::to_vec(&payload).unwrap_or_default();
    if payload_bytes.is_empty() {
        let _ = move_marker_to_failed(&deps, "payload-serialization-failed");
        let outcome = RecoveryOutcome {
            banner: RecoveryBanner::RecoveryFailed,
            moved_to_failed: true,
            ..Default::default()
        };
        (deps.set_banner)(outcome.clone());
        return outcome;
    }

    let attempts_before = read_attempts(&deps.attempt_state_path);
    match (deps.write_bundle)(&payload_bytes, "crash-recovery") {
        Ok(bundle_id) => {
            // Durable success FIRST — only now may the marker be deleted.
            let _ = std::fs::remove_file(&deps.marker_path);
            clear_attempts(&deps.attempt_state_path);
            let outcome = RecoveryOutcome {
                recovered_bundle_id: Some(bundle_id),
                attempts_used: attempts_before,
                attempts_recorded: 0,
                moved_to_failed: false,
                banner: RecoveryBanner::Recovered,
            };
            (deps.set_banner)(outcome.clone());
            outcome
        }
        Err(_) => {
            let attempts = attempts_before + 1;
            let _ = std::fs::write(&deps.attempt_state_path, attempts.to_string());
            if attempts >= MAX_RECOVERY_ATTEMPTS {
                let moved = move_marker_to_failed(&deps, "max-attempts-exceeded");
                clear_attempts(&deps.attempt_state_path);
                let outcome = RecoveryOutcome {
                    recovered_bundle_id: None,
                    attempts_used: attempts,
                    attempts_recorded: attempts,
                    moved_to_failed: moved,
                    banner: RecoveryBanner::RecoveryFailed,
                };
                (deps.set_banner)(outcome.clone());
                return outcome;
            }
            // Retryable failure: keep the marker, stay silent this startup.
            RecoveryOutcome {
                recovered_bundle_id: None,
                attempts_used: attempts,
                attempts_recorded: attempts,
                moved_to_failed: false,
                banner: RecoveryBanner::None,
            }
        }
    }
}

fn read_attempts(path: &std::path::Path) -> u8 {
    std::fs::read(path)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.trim().parse::<u8>().ok())
        .unwrap_or(0)
}

fn clear_attempts(path: &std::path::Path) {
    let _ = std::fs::remove_file(path);
}

/// Move the marker into `failed/` with a typed record. Best-effort: a
/// failure here must never crash startup.
fn move_marker_to_failed(deps: &RecoveryDeps, reason: &str) -> bool {
    let stamp = super::ids::random_hex(6).unwrap_or_else(|| "000000000000".into());
    let dest = deps.failed_dir.join(format!("emergency-{stamp}.json"));
    let _ = std::fs::create_dir_all(&deps.failed_dir);
    let moved = std::fs::rename(&deps.marker_path, &dest)
        .or_else(|_| {
            // Fallback: copy + delete (rename can fail across some contexts).
            std::fs::copy(&deps.marker_path, &dest).and_then(|_| {
                std::fs::remove_file(&deps.marker_path)
            })
        })
        .is_ok();
    if moved {
        let record = serde_json::json!({
            "schema_version": 1,
            "kind": "failed-recovery",
            "reason": reason,
            "app_version": deps.app_version,
        });
        let _ = std::fs::write(
            deps.failed_dir.join(format!("failed-recovery-{stamp}.json")),
            record.to_string(),
        );
    }
    moved
}

/// Defense-in-depth: re-run the Phase 10B redactor over every string in the
/// parsed record before it enters any persisted bundle.
fn re_redact(v: &mut serde_json::Value) {
    match v {
        serde_json::Value::String(s) => {
            *s = super::redact(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                re_redact(item);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, val) in map.iter_mut() {
                re_redact(val);
            }
        }
        _ => {}
    }
}

/// Production dependency wiring (paths from Task 1; Task 10 replaces the
/// write_bundle closure with the full registry transaction).
pub(crate) fn production_deps(app_version: String, set_banner: Box<dyn Fn(RecoveryOutcome)>) -> Option<RecoveryDeps> {
    let app_version_for_closure = app_version.clone();
    let marker_path = super::paths::emergency_dir()?.join("emergency.json");
    let bundles_dir = super::paths::diagnostics_dir()?.join("bundles");
    let failed_dir = super::paths::failed_dir()?;
    let attempt_state_path = super::paths::diagnostics_dir()?.join("recovery-attempts");
    let record_exists = marker_path.exists();
    let record_bytes = std::fs::read(&marker_path).ok();
    let enrich: Box<dyn Fn() -> Result<serde_json::Value, String>> = {
        // Task 5 snapshot reads are try_lock/omission-safe by contract.
        Box::new(|| {
            let snap = super::cache::global().try_snapshot();
            match snap {
                Some(s) => serde_json::to_value(&*s).map_err(|e| e.to_string()),
                None => Err("snapshot unavailable".into()),
            }
        })
    };
    let registry = match super::store::BundleRegistry::new(
        super::paths::diagnostics_dir()?.join("bundle-index.json"),
        bundles_dir,
    ) {
        Some(r) => std::sync::Arc::new(r),
        None => return None,
    };
    let write_bundle: Box<dyn Fn(&[u8], &str) -> Result<String, String>> = {
        let registry = registry.clone();
        let app_version = app_version_for_closure;
        Box::new(move |payload, trigger| {
            // Task 10 crash-safe transaction: encrypt + persist + index with
            // tombstoned retention. `trigger` becomes the bundle meta.
            let meta = super::store::MetaFields {
                created_at_ms: super::now_unix_ms(),
                trigger: trigger.to_string(),
                subsystem: "crash-recovery".into(),
                severity: "critical".into(),
                fingerprint: String::new(),
                app_version: app_version.clone(),
            };
            registry
                .commit_bundle(payload, meta)
                .map(|out| out.bundle_id)
                .map_err(|e| format!("commit: {e:?}"))
        })
    };
    let _ = &registry;
    Some(RecoveryDeps {
        record_bytes,
        record_exists,
        app_version,
        attempt_state_path,
        marker_path,
        failed_dir,
        write_bundle,
        enrich: Some(enrich),
        set_banner,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn valid_record_bytes() -> Vec<u8> {
        serde_json::json!({
            "schema_version": 1,
            "timestamp_ms": 1,
            "app_version": "1.0.1",
            "panic_summary_redacted": "synthetic panic",
            "source_location": "src/lib.rs:1:1",
            "active_view": null,
            "fingerprint": "fp",
            "recent_log_lines": [],
            "cached_listeners": [],
            "cached_services": [],
            "cached_projects": [],
            "cached_workspaces": [],
            "subsystem_status_summary": []
        })
        .to_string()
        .into_bytes()
    }

    /// Fresh deps with a REAL marker file on disk so deletion order can be
    /// observed physically.
    fn fresh_deps(dir: &std::path::Path, record: Option<Vec<u8>>) -> (RecoveryDeps, Rc<RefCell<Vec<&'static str>>>, std::path::PathBuf) {
        let marker = dir.join("emergency.json");
        let attempts = dir.join("recovery-attempts");
        let failed = dir.join("failed");
        let _ = std::fs::create_dir_all(&failed);
        if let Some(bytes) = &record {
            std::fs::write(&marker, bytes).expect("marker written");
        }
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(Vec::new()));
        let order_w = order.clone();
        let marker_w = marker.clone();
        let deps = RecoveryDeps {
            record_bytes: record,
            record_exists: marker.exists(),
            app_version: "1.0.1".into(),
            attempt_state_path: attempts.clone(),
            marker_path: marker.clone(),
            failed_dir: failed.clone(),
            write_bundle: Box::new(move |_payload, _trigger| {
                // The marker must STILL exist at bundle-write time: deletion
                // happens only AFTER durable success.
                if !marker_w.exists() {
                    return Err("marker deleted before bundle write".into());
                }
                order_w.borrow_mut().push("write");
                Ok("bundle-abc123".into())
            }),
            enrich: None,
            set_banner: Box::new(|_| {}),
        };
        (deps, order, marker)
    }

    #[test]
    fn successful_recovery_deletes_marker_only_after_bundle_written() {
        let dir = super::super::storage::test_scratch_dir("recovery-success").expect("scratch");
        let (deps, order, marker) = fresh_deps(&dir, Some(valid_record_bytes()));
        let outcome = finalize_pending(deps);
        assert_eq!(outcome.recovered_bundle_id.as_deref(), Some("bundle-abc123"));
        assert_eq!(order.borrow().as_slice(), ["write"], "write happened first");
        assert!(!marker.exists(), "marker deleted AFTER durable bundle write");
        assert!(matches!(outcome.banner, RecoveryBanner::Recovered));
        super::super::storage::remove_scratch_dir(&dir);
    }

    #[test]
    fn failed_write_keeps_marker_for_retry() {
        let dir = super::super::storage::test_scratch_dir("recovery-retry").expect("scratch");
        let (mut deps, _order, marker) = fresh_deps(&dir, Some(valid_record_bytes()));
        deps.write_bundle = Box::new(|_, _| Err("dpapi unavailable".into()));
        let outcome = finalize_pending(deps);
        assert!(outcome.recovered_bundle_id.is_none());
        assert!(marker.exists(), "marker must survive a failed attempt");
        assert_eq!(outcome.attempts_recorded, 1);
        assert!(!outcome.moved_to_failed);
        super::super::storage::remove_scratch_dir(&dir);
    }

    #[test]
    fn three_failed_attempts_move_to_failed_and_stop() {
        let dir = super::super::storage::test_scratch_dir("recovery-3fail").expect("scratch");
        let (mut deps, _order, marker) = fresh_deps(&dir, Some(valid_record_bytes()));
        deps.write_bundle = Box::new(|_, _| Err("persistent failure".into()));
        let o1 = finalize_pending(deps);
        assert_eq!(o1.attempts_recorded, 1);
        let (mut deps, _o2, _m) = fresh_deps(&dir, None);
        deps.write_bundle = Box::new(|_, _| Err("persistent failure".into()));
        deps.record_exists = marker.exists();
        deps.record_bytes = std::fs::read(&marker).ok();
        deps.attempt_state_path = dir.join("recovery-attempts");
        let o2 = finalize_pending(deps);
        assert_eq!(o2.attempts_recorded, 2);
        let (mut deps, _o3, _m) = fresh_deps(&dir, None);
        deps.write_bundle = Box::new(|_, _| Err("persistent failure".into()));
        deps.record_exists = marker.exists();
        deps.record_bytes = std::fs::read(&marker).ok();
        deps.attempt_state_path = dir.join("recovery-attempts");
        let o3 = finalize_pending(deps);
        assert!(o3.moved_to_failed, "3rd failure escalates to failed/");
        assert!(!marker.exists(), "marker moved into failed/");
        assert!(matches!(o3.banner, RecoveryBanner::RecoveryFailed));
        // 4th call: nothing pending — no retry, no banner.
        let (mut deps, _o4, _m) = fresh_deps(&dir, None);
        deps.record_exists = false;
        deps.record_bytes = None;
        deps.attempt_state_path = dir.join("recovery-attempts");
        let o4 = finalize_pending(deps);
        assert!(o4.recovered_bundle_id.is_none());
        assert!(matches!(o4.banner, RecoveryBanner::None));
        super::super::storage::remove_scratch_dir(&dir);
    }

    #[test]
    fn oversized_or_wrong_schema_marker_is_discarded_safely() {
        let dir = super::super::storage::test_scratch_dir("recovery-invalid").expect("scratch");
        // Oversized marker.
        let giant = vec![b'x'; 524_289];
        let (deps, _o, marker) = fresh_deps(&dir, Some(giant));
        let outcome = finalize_pending(deps);
        assert!(outcome.recovered_bundle_id.is_none(), "no bundle for invalid marker");
        assert!(!marker.exists(), "invalid marker moved out of emergency/");
        assert!(outcome.moved_to_failed);
        assert!(matches!(outcome.banner, RecoveryBanner::InvalidMarker));
        super::super::storage::remove_scratch_dir(&dir);

        // Wrong schema version.
        let dir2 = super::super::storage::test_scratch_dir("recovery-schema").expect("scratch");
        let future = serde_json::json!({ "schema_version": 2 }).to_string().into_bytes();
        let (deps, _o, marker2) = fresh_deps(&dir2, Some(future));
        let outcome2 = finalize_pending(deps);
        assert!(outcome2.recovered_bundle_id.is_none());
        assert!(outcome2.moved_to_failed);
        assert!(!marker2.exists());
        super::super::storage::remove_scratch_dir(&dir2);
    }

    #[test]
    fn enrichment_failure_still_recovers_core() {
        let dir = super::super::storage::test_scratch_dir("recovery-enrich").expect("scratch");
        let (mut deps, _order, _marker) = fresh_deps(&dir, Some(valid_record_bytes()));
        deps.enrich = Some(Box::new(|| Err("cache busy".into())));
        let outcome = finalize_pending(deps);
        assert_eq!(outcome.recovered_bundle_id.as_deref(), Some("bundle-abc123"));
        super::super::storage::remove_scratch_dir(&dir);
    }

    #[test]
    fn missing_marker_is_a_noop() {
        let dir = super::super::storage::test_scratch_dir("recovery-noop").expect("scratch");
        let (deps, _order, _marker) = fresh_deps(&dir, None);
        let outcome = finalize_pending(deps);
        assert!(outcome.recovered_bundle_id.is_none());
        assert!(matches!(outcome.banner, RecoveryBanner::None));
        super::super::storage::remove_scratch_dir(&dir);
    }
}
