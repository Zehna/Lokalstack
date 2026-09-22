//! Phase 11C Task 6 — panic emergency-record writer (spec §6 panic/crash).
//!
//! Contract: the panic hook performs ZERO new OS work — no dir discovery, no
//! registry, no Docker/AI, no IPC, no ZIP/DPAPI, and never waits on any
//! contended lock (all cache reads are try_lock; busy/poisoned → omit). The
//! emergency destination is prepared ONCE during normal runtime; when it was
//! not prepared, capture degrades to the Phase 10B redacted breadcrumb via a
//! try_lock-based sink (content kept verbatim, never blocking).

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;

use super::cache::CacheHandle;
use super::incidents::{fingerprint, Severity};
use super::paths;

/// Exact emergency-record byte cap (spec §8 bounds table).
pub(crate) const EMERGENCY_RECORD_MAX_BYTES: usize = 524_288; // 512 KiB
/// Max recent diagnostic log lines carried into the emergency record.
const MAX_RECENT_LOG_LINES: usize = 50;

const TRUNCATION_MARKER: &[u8] = b"...[TRUNCATED]";

/// Emergency marker prepared ONCE during normal startup. Holds an open,
/// exclusively-owned handle to the marker file so the panic path needs no
/// directory work at all.
pub(crate) struct PreparedEmergency {
    /// Marker location; consumed by the startup crash-recovery path (Task 9)
    /// and by tests. The hook itself only uses the open handle.
    #[allow(dead_code)]
    pub(crate) file_path: PathBuf,
    file: Mutex<File>,
}

/// Versioned emergency crash record (schema v1). Every section is optional
/// evidence; a smaller valid record is always preferred over a deadlock.
#[derive(Debug, Serialize)]
pub(crate) struct EmergencyRecordV1 {
    pub(crate) schema_version: u32,
    pub(crate) timestamp_ms: u64,
    pub(crate) app_version: String,
    pub(crate) panic_summary_redacted: String,
    pub(crate) source_location: String,
    pub(crate) active_view: Option<u8>,
    pub(crate) fingerprint: String,
    pub(crate) recent_log_lines: Vec<String>,
    pub(crate) cached_listeners: Vec<super::cache::ListenerSummary>,
    pub(crate) cached_services: Vec<super::cache::ServiceSummary>,
    pub(crate) cached_projects: Vec<super::cache::ProjectSummary>,
    pub(crate) cached_workspaces: Vec<super::cache::WorkspaceSummary>,
    pub(crate) subsystem_status_summary: Vec<super::cache::SubsystemStatusSummary>,
}

/// Resolve + create `emergency/` and open the marker file handle exactly once,
/// during NORMAL runtime. Called from the startup path (never from the hook).
pub(crate) fn prepare_once() -> Result<PreparedEmergency, String> {
    let dir = paths::emergency_dir().ok_or_else(|| "emergency dir unavailable".to_string())?;
    let file_path = dir.join("emergency.json");
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&file_path)
        .map_err(|e| format!("emergency marker open failed: {e}"))?;
    Ok(PreparedEmergency {
        file_path,
        file: Mutex::new(file),
    })
}

/// Build the record from the shared cache using try_lock reads ONLY.
/// Busy/poisoned/missing cache sections are omitted — never waited on.
pub(crate) fn build_record(
    payload: &str,
    location: &str,
    cache: &CacheHandle,
) -> EmergencyRecordV1 {
    let summary = super::redact(payload);
    let fp = fingerprint("panic", "PANIC", location, Severity::Critical);
    let snap = cache.try_snapshot();
    EmergencyRecordV1 {
        schema_version: 1,
        timestamp_ms: super::now_unix_ms(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        panic_summary_redacted: summary,
        source_location: location.to_string(),
        active_view: cache.try_active_view(),
        fingerprint: fp,
        recent_log_lines: cache.try_recent_log_tail(MAX_RECENT_LOG_LINES),
        cached_listeners: snap
            .as_ref()
            .map(|s| s.listeners.clone())
            .unwrap_or_default(),
        cached_services: snap
            .as_ref()
            .map(|s| s.services.clone())
            .unwrap_or_default(),
        cached_projects: snap
            .as_ref()
            .map(|s| s.projects.clone())
            .unwrap_or_default(),
        cached_workspaces: snap
            .as_ref()
            .map(|s| s.workspaces.clone())
            .unwrap_or_default(),
        subsystem_status_summary: snap
            .as_ref()
            .map(|s| s.subsystem_status.clone())
            .unwrap_or_default(),
    }
}

/// Serialize with the hard byte cap. Priority order when oversized: drop
/// cached_* sections first, then the subsystem status summary, then recent log
/// lines; if still over, byte-truncate with an explicit marker so the file is
/// always ≤ `EMERGENCY_RECORD_MAX_BYTES`.
pub(crate) fn render_record(record: &mut EmergencyRecordV1) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    if bytes.len() > EMERGENCY_RECORD_MAX_BYTES {
        record.cached_listeners.clear();
        record.cached_services.clear();
        record.cached_projects.clear();
        record.cached_workspaces.clear();
        bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    }
    if bytes.len() > EMERGENCY_RECORD_MAX_BYTES {
        record.subsystem_status_summary.clear();
        bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    }
    if bytes.len() > EMERGENCY_RECORD_MAX_BYTES {
        record.recent_log_lines.clear();
        bytes = serde_json::to_vec(&record).map_err(|e| e.to_string())?;
    }
    if bytes.len() > EMERGENCY_RECORD_MAX_BYTES {
        let keep = EMERGENCY_RECORD_MAX_BYTES - TRUNCATION_MARKER.len();
        bytes.truncate(keep);
        bytes.extend_from_slice(TRUNCATION_MARKER);
    }
    debug_assert!(bytes.len() <= EMERGENCY_RECORD_MAX_BYTES);
    Ok(bytes)
}

/// The testable core of the panic hook. `breadcrumb` is the injected sink for
/// the Phase 10B redacted-breadcrumb degradation path.
pub(crate) fn capture_record(
    payload: &str,
    location: &str,
    cache: &CacheHandle,
    prepared: Option<&PreparedEmergency>,
    breadcrumb: &mut dyn FnMut(&str),
) {
    let mut record = build_record(payload, location, cache);
    let bytes = match render_record(&mut record) {
        Ok(b) => b,
        Err(_) => {
            // Serialization failed: fall back to the bounded breadcrumb text.
            breadcrumb(&format!("panic: {} at {}", record.panic_summary_redacted, location));
            return;
        }
    };
    match prepared {
        Some(p) => {
            // try_lock only: never wait on the emergency handle. On failure,
            // degrade to the non-blocking breadcrumb.
            match p.file.try_lock() {
                Ok(mut f) => {
                    let _ = write_all_and_fsync(&mut f, &bytes);
                }
                Err(_) => {
                    breadcrumb(&format!(
                        "panic: {} at {}",
                        record.panic_summary_redacted, location
                    ));
                }
            }
        }
        None => {
            breadcrumb(&format!(
                "panic: {} at {}",
                record.panic_summary_redacted, location
            ));
        }
    }
}

fn write_all_and_fsync(f: &mut File, bytes: &[u8]) -> std::io::Result<()> {
    f.set_len(0)?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// Replace the Phase 10B panic hook. Keeps the old hook's delegation and its
/// redacted-breadcrumb degradation, made strictly non-blocking: the breadcrumb
/// sink uses try_lock on the log file and drops the line if busy.
pub(crate) fn install(cache: &'static CacheHandle, prepared: Option<PreparedEmergency>) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "opaque panic payload".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".to_string());
        let mut breadcrumb = |s: &str| super::try_log_error_breadcrumb(s);
        capture_record(&payload, &location, &cache, prepared.as_ref(), &mut breadcrumb);
        default_hook(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::cache::{
        AppSnapshot, CacheHandle, ListenerSummary, ProjectSummary, ServiceSummary,
        SubsystemStatusSummary, WorkspaceSummary,
    };
    use std::sync::Arc;
    use std::time::Instant;

    fn cache_with_big_snapshot() -> Arc<CacheHandle> {
        let c = Arc::new(CacheHandle::new());
        let big = |n: usize, tag: &str| -> String { tag.repeat(n) };
        let mut snap = AppSnapshot::default();
        for i in 0..100 {
            snap.listeners.push(ListenerSummary {
                port: i as u16,
                ip_version: 4,
                pid: 1,
                state: big(4096, "L"),
            });
            snap.services.push(ServiceSummary {
                managed_id: format!("m{i}"),
                workspace_id: format!("w{i}"),
                service_id: format!("s{i}"),
                state: big(4096, "S"),
            });
            snap.projects.push(ProjectSummary {
                id: format!("p{i}"),
                name: big(4096, "P"),
                kind: "local".into(),
            });
            snap.workspaces.push(WorkspaceSummary {
                id: format!("w{i}"),
                name: big(4096, "W"),
                service_count: 1,
            });
            snap.subsystem_status.push(SubsystemStatusSummary {
                subsystem: format!("sub{i}"),
                state: big(4096, "T"),
            });
        }
        c.publish(snap);
        c.publish_log_tail(
            (0..60)
                .map(|i| format!("log line {i} {}", big(4096, "x")))
                .collect(),
        );
        c
    }

    #[test]
    fn record_serialization_is_capped_at_524_288_bytes() {
        let cache = cache_with_big_snapshot();
        let mut record = build_record("synthetic panic payload", "src/lib.rs:1:1", &cache);
        let bytes = render_record(&mut record).expect("render");
        assert!(
            bytes.len() <= EMERGENCY_RECORD_MAX_BYTES,
            "record exceeded cap: {}",
            bytes.len()
        );
        assert_eq!(record.schema_version, 1);
    }

    #[test]
    fn oversized_record_drops_lowest_priority_sections_first() {
        let cache = cache_with_big_snapshot();
        let mut record = build_record("synthetic panic payload", "src/lib.rs:1:1", &cache);
        let bytes = render_record(&mut record).expect("render");
        assert!(bytes.len() <= EMERGENCY_RECORD_MAX_BYTES);
        // Core panic evidence survives; lowest-priority cached sections were
        // dropped first (cached_* before subsystem_status before log lines).
        assert!(!record.panic_summary_redacted.is_empty());
        assert!(record.cached_listeners.is_empty());
        assert!(record.cached_services.is_empty());
    }

    #[test]
    fn busy_cache_sections_are_omitted() {
        let cache = cache_with_big_snapshot();
        let guard = cache.snapshot_lock_for_test();
        let start = Instant::now();
        let mut record = build_record("synthetic panic payload", "src/lib.rs:1:1", &cache);
        let bytes = render_record(&mut record).expect("render");
        let elapsed = start.elapsed();
        drop(guard);
        // try_lock only: busy snapshot must be OMITTED, never waited on.
        assert!(elapsed.as_millis() < 100, "hook blocked on busy cache");
        assert!(record.cached_listeners.is_empty());
        assert!(!bytes.is_empty());
    }

    #[test]
    fn unprepared_emergency_degrades_to_breadcrumb() {
        let cache = Arc::new(CacheHandle::new());
        let mut sink_outputs: Vec<String> = Vec::new();
        {
            let mut sink = |s: &str| sink_outputs.push(s.to_string());
            capture_record(
                "synthetic panic payload",
                "src/lib.rs:1:1",
                &cache,
                None,
                &mut sink,
            );
        }
        assert_eq!(sink_outputs.len(), 1, "breadcrumb must be emitted once");
        let bc = &sink_outputs[0];
        assert!(bc.contains("synthetic panic payload"));
        assert!(bc.contains("src/lib.rs:1:1"));
    }

    #[test]
    fn prepared_writer_uses_try_lock_only() {
        // Hold the normal logger's LOG_FILE mutex while capturing: the
        // emergency writer must be fully independent of it.
        let _log_guard = crate::diagnostics::log_file_lock_for_test();
        let cache = Arc::new(CacheHandle::new());
        let prepared = prepare_once().expect("prepare emergency destination");
        let path = prepared.file_path.clone();
        let start = Instant::now();
        {
            let mut sink = |_s: &str| {};
            capture_record(
                "prepared-path panic",
                "src/lib.rs:2:2",
                &cache,
                Some(&prepared),
                &mut sink,
            );
        }
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 2000,
            "emergency write must not block on the log mutex"
        );
        let bytes = std::fs::read(&path).expect("marker file readable");
        assert!(!bytes.is_empty());
        assert!(bytes.windows(19).any(|w| w == b"prepared-path panic"));
        drop(prepared);
        // Best-effort cleanup of the synthetic marker.
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn panic_summary_is_redacted() {
        let cache = Arc::new(CacheHandle::new());
        let mut record = build_record(
            "boom: password=hunter2 token=abc123",
            "src/lib.rs:3:3",
            &cache,
        );
        let bytes = render_record(&mut record).expect("render");
        let text = String::from_utf8_lossy(&bytes).to_string();
        assert!(!text.contains("hunter2"), "raw secret in emergency record");
        assert!(!text.contains("abc123"), "raw token in emergency record");
        assert!(text.contains("REDACTED"), "redaction marker expected");
    }
}
