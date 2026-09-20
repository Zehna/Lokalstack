//! Phase 11C Task 5 — diagnostics snapshot cache + active-view context
//! (spec §5, §6 panic snapshot, plan Task 5).
//!
//! All reads are **non-blocking** (`try_lock`; busy/poisoned → `None`/empty)
//! so the panic hook and the capture worker can never stall on a contended
//! cache. Bounds are enforced at publish time (plan §8 exact constants):
//! listeners 100, services 100, projects 50, workspaces 50, log tail 50.
//!
//! Everything here is consumed by the panic hook (Task 6), the bundle
//! builders (Tasks 7/9/10) and the command surface (Task 14); the module
//! allow covers the wiring gap until those tasks land.
#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

/// Exact bounds (plan §8). Consumed at publish time inside this module and
/// by the publisher wiring in later tasks.
pub(crate) const MAX_CACHED_LISTENERS: usize = 100;
pub(crate) const MAX_CACHED_SERVICES: usize = 100;
pub(crate) const MAX_CACHED_PROJECTS: usize = 50;
pub(crate) const MAX_CACHED_WORKSPACES: usize = 50;
pub(crate) const MAX_LOG_TAIL_LINES: usize = 50;

/// Active-view discriminator range mirror. Positional per
/// `src/app/navigation.ts` `NAV_ITEMS` order; Task 17 wires the real mapping
/// when the navigation store becomes canonical. A value outside `0..NAV_LEN`
/// is rejected (`set_active_view` returns `false`).
pub(crate) const NAV_LEN: u8 = 10;

/// Listener summary: stable display facts only (no local address strings in
/// the shared cache; bundle sections that need them collect them explicitly
/// through their own profile-aware path).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ListenerSummary {
    pub port: u16,
    pub ip_version: u8,
    pub pid: u32,
    pub state: String,
}

/// Managed-service summary (managed processes only — never external ones).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ServiceSummary {
    pub managed_id: String,
    pub workspace_id: String,
    pub service_id: String,
    pub state: String,
}

/// Project summary: id/name/kind only — no paths in the shared cache.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ProjectSummary {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// Workspace summary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct WorkspaceSummary {
    pub id: String,
    pub name: String,
    pub service_count: usize,
}

/// Subsystem status summary (populated by later tasks' owners).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SubsystemStatusSummary {
    pub subsystem: String,
    pub state: String,
}

/// One whole-app cached snapshot (panic-snapshot + bundle source).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct AppSnapshot {
    pub listeners: Vec<ListenerSummary>,
    pub services: Vec<ServiceSummary>,
    pub projects: Vec<ProjectSummary>,
    pub workspaces: Vec<WorkspaceSummary>,
    pub subsystem_status: Vec<SubsystemStatusSummary>,
    pub captured_at_ms: u64,
}

/// Shared cache handle. Every read is `try_lock`-based; every write enforces
/// the exact bounds. A poisoned mutex reads as `None`/empty — never blocks,
/// never panics.
#[derive(Default)]
pub(crate) struct CacheHandle {
    snapshot: Mutex<Option<std::sync::Arc<AppSnapshot>>>,
    active_view: Mutex<Option<u8>>,
    log_tail: Mutex<VecDeque<String>>,
}

impl CacheHandle {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Publish a new snapshot, enforcing bounds by keeping the LAST N items
    /// of each over-long section (newest data is the most useful).
    pub(crate) fn publish(&self, mut s: AppSnapshot) {
        truncate_tail(&mut s.listeners, MAX_CACHED_LISTENERS);
        truncate_tail(&mut s.services, MAX_CACHED_SERVICES);
        truncate_tail(&mut s.projects, MAX_CACHED_PROJECTS);
        truncate_tail(&mut s.workspaces, MAX_CACHED_WORKSPACES);
        truncate_tail(&mut s.subsystem_status, MAX_CACHED_SERVICES);
        if let Ok(mut slot) = self.snapshot.lock() {
            *slot = Some(std::sync::Arc::new(s));
        }
    }

    /// Non-blocking snapshot read: busy/poisoned → `None`.
    pub(crate) fn try_snapshot(&self) -> Option<std::sync::Arc<AppSnapshot>> {
        self.snapshot.try_lock().ok()?.clone()
    }

    /// Record the active frontend view (validated `0..NAV_LEN`). Returns
    /// whether the value was accepted.
    pub(crate) fn set_active_view(&self, v: u8) -> bool {
        if v >= NAV_LEN {
            return false;
        }
        match self.active_view.lock() {
            Ok(mut slot) => {
                *slot = Some(v);
                true
            }
            Err(_) => false,
        }
    }

    /// Non-blocking active-view read: busy/poisoned → `None`.
    pub(crate) fn try_active_view(&self) -> Option<u8> {
        let guard = self.active_view.try_lock().ok()?;
        *guard
    }

    /// Publish the recent diagnostic-log tail; capped at
    /// [`MAX_LOG_TAIL_LINES`], newest last.
    pub(crate) fn publish_log_tail(&self, lines: Vec<String>) {
        if let Ok(mut tail) = self.log_tail.lock() {
            tail.clear();
            let skip = lines.len().saturating_sub(MAX_LOG_TAIL_LINES);
            tail.extend(lines.into_iter().skip(skip));
        }
    }

    /// Non-blocking tail read: busy/poisoned → empty.
    pub(crate) fn try_recent_log_tail(&self, max_lines: usize) -> Vec<String> {
        let Ok(tail) = self.log_tail.try_lock() else {
            return Vec::new();
        };
        let skip = tail.len().saturating_sub(max_lines);
        tail.iter().skip(skip).cloned().collect()
    }

    // -- test seams -------------------------------------------------------

    #[cfg(test)]
    pub(crate) fn snapshot_lock_for_test(&self) -> std::sync::MutexGuard<'_, Option<std::sync::Arc<AppSnapshot>>> {
        match self.snapshot.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    #[cfg(test)]
    pub(crate) fn log_tail_lock_for_test(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        match self.log_tail.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }
}

fn truncate_tail<T>(v: &mut Vec<T>, max: usize) {
    if v.len() > max {
        let drop_count = v.len() - max;
        v.drain(0..drop_count);
    }
}

/// Process-global cache handle (published during normal runtime by the
/// polling/owner wiring in later tasks; read by panic hook + capture worker).
pub(crate) fn global() -> &'static CacheHandle {
    static CACHE: OnceLock<CacheHandle> = OnceLock::new();
    CACHE.get_or_init(CacheHandle::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn listener(port: u16) -> ListenerSummary {
        ListenerSummary { port, ip_version: 4, pid: 1, state: "LISTEN".into() }
    }

    fn service(i: usize) -> ServiceSummary {
        ServiceSummary {
            managed_id: format!("m{i}"),
            workspace_id: format!("w{}", i % 3),
            service_id: format!("s{i}"),
            state: "running".into(),
        }
    }

    fn project(i: usize) -> ProjectSummary {
        ProjectSummary { id: format!("p{i}"), name: format!("proj{i}"), kind: "node".into() }
    }

    fn workspace(i: usize) -> WorkspaceSummary {
        WorkspaceSummary { id: format!("w{i}"), name: format!("ws{i}"), service_count: 1 }
    }

    fn snapshot_with(
        listeners: Vec<ListenerSummary>,
        services: Vec<ServiceSummary>,
        projects: Vec<ProjectSummary>,
        workspaces: Vec<WorkspaceSummary>,
    ) -> AppSnapshot {
        AppSnapshot {
            listeners,
            services,
            projects,
            workspaces,
            subsystem_status: Vec::new(),
            captured_at_ms: 0,
        }
    }

    #[test]
    fn publish_enforces_exact_bounds() {
        let cache = CacheHandle::new();
        let listeners: Vec<_> = (0..120u16).map(listener).collect();
        let services: Vec<_> = (0..120).map(service).collect();
        let projects: Vec<_> = (0..60).map(project).collect();
        let workspaces: Vec<_> = (0..60).map(workspace).collect();
        cache.publish(snapshot_with(listeners, services, projects, workspaces));
        let snap = cache.try_snapshot().expect("snapshot present");
        assert_eq!(snap.listeners.len(), 100, "listeners capped at 100");
        assert_eq!(snap.services.len(), 100, "services capped at 100");
        assert_eq!(snap.projects.len(), 50, "projects capped at 50");
        assert_eq!(snap.workspaces.len(), 50, "workspaces capped at 50");
    }

    #[test]
    fn busy_mutex_reads_return_none_not_block() {
        let cache = CacheHandle::new();
        cache.publish(snapshot_with(vec![listener(80)], vec![], vec![], vec![]));
        // Hold the lock manually, then verify the read path gives up fast.
        let guard = cache.snapshot_lock_for_test();
        let start = std::time::Instant::now();
        let got = cache.try_snapshot();
        assert!(got.is_none(), "busy mutex must read as None");
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "try_snapshot must not block, took {:?}",
            start.elapsed()
        );
        drop(guard);
        assert!(cache.try_snapshot().is_some(), "readable again after release");
    }

    #[test]
    fn active_view_validated() {
        let cache = CacheHandle::new();
        assert!(!cache.set_active_view(NAV_LEN), "out-of-range rejected");
        assert!(!cache.set_active_view(255), "out-of-range rejected");
        assert!(cache.set_active_view(0), "in-range accepted");
        assert_eq!(cache.try_active_view(), Some(0));
    }

    #[test]
    fn log_tail_capped_at_50() {
        let cache = CacheHandle::new();
        let lines: Vec<String> = (1..=60).map(|i| format!("line-{i}")).collect();
        cache.publish_log_tail(lines);
        let tail = cache.try_recent_log_tail(50);
        assert_eq!(tail.len(), 50, "tail capped at 50 lines");
        assert_eq!(tail[0], "line-11", "oldest kept line is #11");
        assert_eq!(tail[49], "line-60", "newest line last");
    }

    #[test]
    fn busy_log_tail_returns_empty_not_block() {
        let cache = CacheHandle::new();
        cache.publish_log_tail(vec!["a".into(), "b".into()]);
        let _guard = cache.log_tail_lock_for_test();
        let start = std::time::Instant::now();
        assert!(cache.try_recent_log_tail(50).is_empty(), "busy tail reads empty");
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
