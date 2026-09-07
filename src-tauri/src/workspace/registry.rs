//! Workspace registries — the trust boundary for the managed lifecycle.
//!
//! Two registries, deliberately separate from Phase 5's external
//! control-target registry:
//!
//! - [`LaunchSpecRegistry`] — backend-derived, trusted launch
//!   specifications behind **opaque launch ids**. The frontend references a
//!   spec only by id; it can never submit a program, args, cwd, or env.
//! - [`ManagedProcessRegistry`] — processes LocalStack itself launched
//!   (`ManagedProcess`), keyed by managed id, bounded, and cleaned when
//!   processes exit. Nothing here applies to externally discovered
//!   processes.
//!
//! Neither registry persists across app restarts (documented limitation):
//! after LocalStack exits, its former managed processes become external.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use crate::control::registry::{blake3_256, boot_key};
use crate::workspace::rules::{LaunchSpec, LogRing, ManagedState};

/// Bounded capacity of the launch-spec registry.
pub(crate) const LAUNCH_SPEC_CAPACITY: usize = 256;

/// Bounded capacity of the managed-process registry.
pub(crate) const MANAGED_CAPACITY: usize = 256;

/// Opaque, unpredictable hex id from hashed parts.
fn opaque_id(parts: &[&[u8]]) -> String {
    let mut input = Vec::new();
    for part in parts {
        input.extend_from_slice(&(part.len() as u32).to_le_bytes());
        input.extend_from_slice(part);
    }
    let mut key = boot_key();
    input.extend_from_slice(&key);
    key.fill(0); // do not leave the boot key lying in scratch buffers
    let hash = blake3_256(&input);
    let mut hex = String::with_capacity(64);
    for byte in hash {
        hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex digit"));
        hex.push(char::from_digit(u32::from(byte & 0xF), 16).expect("hex digit"));
    }
    hex
}

/// Concatenated UTF-8 bytes of the launch args (hash input helper).
fn launch_args_bytes(args: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for arg in args {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0); // separator
    }
    bytes
}

/// Opaque managed-process id, unique per launch (startedAt distinguishes
/// successive launches of the same service).
pub(crate) fn opaque_managed_id(workspace_id: &str, service_id: &str, started_at: u64) -> String {
    opaque_id(&[
        b"managed",
        workspace_id.as_bytes(),
        service_id.as_bytes(),
        &started_at.to_le_bytes(),
    ])
}

// ---------------------------------------------------------------------------
// Launch-spec registry
// ---------------------------------------------------------------------------

/// A trusted launch specification as stored server-side.
#[derive(Debug, Clone)]
pub(crate) struct TrustedLaunchSpec {
    pub id: String,
    pub workspace_id: String,
    pub service_id: String,
    pub spec: LaunchSpec,
    /// Expected port (evidence-based only), used for readiness and the
    /// port-conflict preflight.
    pub expected_port: Option<u16>,
    /// Project root the spec belongs to (revalidated at launch time).
    pub project_root: String,
    /// When the spec was derived (specs are re-derived on refresh; stale
    /// ones are removed).
    pub issued: Instant,
}

/// Registry of backend-derived launch specs, addressed by opaque ids.
#[derive(Default)]
pub(crate) struct LaunchSpecRegistry {
    map: Mutex<HashMap<String, Arc<TrustedLaunchSpec>>>,
}

impl LaunchSpecRegistry {
    /// Insert (or replace) a spec; the id is deterministic over
    /// (workspace, service, program, args, cwd) + the per-boot key, so a
    /// refresh re-derives the same id for an unchanged spec but ids never
    /// survive an app restart.
    pub(crate) fn put(&self, mut trusted: TrustedLaunchSpec) -> String {
        let args_hash_input = launch_args_bytes(&trusted.spec.args);
        let id = opaque_id(&[
            b"launch",
            trusted.workspace_id.as_bytes(),
            trusted.service_id.as_bytes(),
            trusted.spec.program.as_bytes(),
            &args_hash_input,
            trusted.spec.cwd.as_bytes(),
        ]);
        trusted.id = id.clone();
        let mut map = self.lock();
        if map.len() >= LAUNCH_SPEC_CAPACITY && !map.contains_key(&id) {
            // Drop the oldest entry to stay bounded.
            if let Some(oldest) = map
                .values()
                .min_by_key(|s| s.issued)
                .cloned()
                .map(|s| s.id.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(id.clone(), Arc::new(trusted));
        id
    }

    /// Resolve an opaque id to its trusted spec.
    pub(crate) fn get(&self, launch_spec_id: &str) -> Option<Arc<TrustedLaunchSpec>> {
        self.lock().get(launch_spec_id).cloned()
    }

    /// Remove every spec belonging to a workspace (workspace deletion).
    pub(crate) fn remove_workspace(&self, workspace_id: &str) {
        self.lock().retain(|_, s| s.workspace_id != workspace_id);
    }

    /// Drop every spec (refresh-time wholesale replacement).
    #[cfg(test)]
    pub(crate) fn replace_all(&self, specs: impl IntoIterator<Item = TrustedLaunchSpec>) {
        let mut map = self.lock();
        map.clear();
        for spec in specs {
            let args_hash_input = launch_args_bytes(&spec.spec.args);
            let id = opaque_id(&[
                b"launch",
                spec.workspace_id.as_bytes(),
                spec.service_id.as_bytes(),
                spec.spec.program.as_bytes(),
                &args_hash_input,
                spec.spec.cwd.as_bytes(),
            ]);
            let mut spec = spec;
            spec.id = id.clone();
            map.insert(id, Arc::new(spec));
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Arc<TrustedLaunchSpec>>> {
        match self.map.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

// ---------------------------------------------------------------------------
// Managed-process registry
// ---------------------------------------------------------------------------

/// A process LocalStack itself launched, with full lifecycle metadata.
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ManagedProcess {
    /// Opaque managed-process id (frontend-visible).
    pub managedId: String,
    pub workspaceId: String,
    pub serviceId: String,
    /// Root PID of the launched process.
    pub rootPid: u32,
    /// Creation time of the root process (Unix ms) — identity for
    /// revalidation, exactly like the Phase 5 external path.
    pub creationTime: Option<u64>,
    /// Known Windows process-group id (== root PID, by construction of
    /// `CREATE_NEW_PROCESS_GROUP`). NEVER 0 — group 0 is a broadcast.
    pub processGroupId: u32,
    /// The trusted spec id this process was launched from.
    pub launchSpecId: String,
    /// Unix ms when LocalStack started it.
    pub startedAt: u64,
    pub state: ManagedState,
}

/// One registry entry: the managed process plus its output capture.
pub(crate) struct ManagedEntry {
    pub process: ManagedProcess,
    /// Bounded stdout+stderr capture (see `rules::LogRing`).
    pub logs: LogRing,
    /// When the state last changed (for timeouts: readiness, stop).
    pub state_since: Instant,
}

impl ManagedEntry {
    /// Lines a reader thread has appended since the caller last looked.
    pub(crate) fn drain_new_logs(&mut self, after_index: u64) -> Vec<crate::workspace::rules::LogLine> {
        self.logs.since(after_index)
    }
}

/// Registry of LocalStack-managed processes. Bounded; entries are removed
/// when their process exits and the terminal state has been observed.
#[derive(Default)]
pub(crate) struct ManagedProcessRegistry {
    map: Mutex<HashMap<String, ManagedEntry>>,
}

impl ManagedProcessRegistry {
    /// Record a freshly launched process.
    pub(crate) fn insert(&self, entry: ManagedEntry) -> Result<String, String> {
        let mut map = self.lock();
        if map.len() >= MANAGED_CAPACITY {
            return Err("Managed-process registry is full.".to_string());
        }
        let id = entry.process.managedId.clone();
        map.insert(id.clone(), entry);
        Ok(id)
    }

    /// Look up one managed process (immutable view).
    pub(crate) fn get(&self, managed_id: &str) -> Option<ManagedProcess> {
        self.lock().get(managed_id).map(|e| e.process.clone())
    }

    /// Mutate one entry in place (state transitions, log reads).
    pub(crate) fn with_entry<R>(
        &self,
        managed_id: &str,
        f: impl FnOnce(&mut ManagedEntry) -> R,
    ) -> Option<R> {
        let mut map = self.lock();
        map.get_mut(managed_id).map(f)
    }

    /// Remove a managed process (user removed the service / cleanup).
    pub(crate) fn remove(&self, managed_id: &str) -> Option<ManagedProcess> {
        self.lock().remove(managed_id).map(|e| e.process)
    }

    /// Iterate over every managed process (snapshot view for the monitor).
    pub(crate) fn for_each(&self, f: &mut dyn FnMut(&ManagedProcess)) {
        self.lock().values().for_each(|e| f(&e.process));
    }

    /// The most recent managed process for one workspace service (any
    /// state), for view assembly.
    pub(crate) fn latest_for_service(&self, workspace_id: &str, service_id: &str) -> Option<ManagedProcess> {
        self.lock()
            .values()
            .filter(|e| e.process.workspaceId == workspace_id && e.process.serviceId == service_id)
            .max_by_key(|e| e.process.startedAt)
            .map(|e| e.process.clone())
    }

    /// Entries whose root process no longer exists — the monitor removes
    /// them after capturing the exit code (zombie prevention).
    pub(crate) fn exited_ids(&self, alive: &dyn Fn(u32) -> bool) -> Vec<String> {
        self.lock()
            .values()
            .filter(|e| e.process.state.alive() && !alive(e.process.rootPid))
            .map(|e| e.process.managedId.clone())
            .collect()
    }

    /// Every managed process of one workspace.
    pub(crate) fn workspace_processes(&self, workspace_id: &str) -> Vec<ManagedProcess> {
        let mut processes: Vec<ManagedProcess> = self
            .lock()
            .values()
            .filter(|e| e.process.workspaceId == workspace_id)
            .map(|e| e.process.clone())
            .collect();
        processes.sort_by(|a, b| a.startedAt.cmp(&b.startedAt));
        processes
    }

    /// All managed root PIDs (duplicate-launch checks).
    pub(crate) fn managed_pids(&self) -> Vec<u32> {
        self.lock().values().map(|e| e.process.rootPid).collect()
    }

    /// Whether a workspace service already has a live managed process.
    pub(crate) fn has_running_service(&self, workspace_id: &str, service_id: &str) -> bool {
        self.lock().values().any(|e| {
            e.process.workspaceId == workspace_id
                && e.process.serviceId == service_id
                && e.process.state.alive()
        })
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lock().len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, ManagedEntry>> {
        match self.map.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: &str, args: &[&str], cwd: &str) -> LaunchSpec {
        LaunchSpec {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: cwd.to_string(),
            kind: crate::workspace::rules::ProgramKind::Exe,
        }
    }

    fn trusted(workspace: &str, service: &str, program: &str) -> TrustedLaunchSpec {
        TrustedLaunchSpec {
            id: String::new(),
            workspace_id: workspace.to_string(),
            service_id: service.to_string(),
            spec: spec(program, &["run", "dev"], r"D:\Projects\app"),
            expected_port: None,
            project_root: r"D:\Projects\app".to_string(),
            issued: Instant::now(),
        }
    }

    // --- launch-spec registry -------------------------------------------------

    #[test]
    fn spec_ids_are_opaque_and_deterministic() {
        let registry = LaunchSpecRegistry::default();
        let a = registry.put(trusted("ws1", "svc1", "npm.cmd"));
        let b = registry.put(trusted("ws1", "svc1", "npm.cmd"));
        assert_eq!(a, b, "unchanged spec → same id on refresh");
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn different_specs_get_different_ids() {
        let registry = LaunchSpecRegistry::default();
        let a = registry.put(trusted("ws1", "svc1", "npm.cmd"));
        let b = registry.put(trusted("ws1", "svc1", "pnpm.cmd"));
        assert_ne!(a, b);
    }

    #[test]
    fn ids_do_not_survive_replace_all() {
        let registry = LaunchSpecRegistry::default();
        let old = registry.put(trusted("ws1", "svc1", "npm.cmd"));
        registry.replace_all([trusted("ws1", "svc1", "pnpm.cmd")]);
        assert!(registry.get(&old).is_none(), "stale spec ids must refuse");
    }

    #[test]
    fn spec_registry_is_bounded() {
        let registry = LaunchSpecRegistry::default();
        for i in 0..(LAUNCH_SPEC_CAPACITY + 50) {
            registry.put(trusted("ws", &format!("svc{i}"), "npm.cmd"));
        }
        assert!(registry.len() <= LAUNCH_SPEC_CAPACITY);
    }

    #[test]
    fn remove_workspace_drops_its_specs() {
        let registry = LaunchSpecRegistry::default();
        registry.put(trusted("ws1", "svc1", "npm.cmd"));
        registry.put(trusted("ws2", "svc2", "npm.cmd"));
        registry.remove_workspace("ws1");
        assert_eq!(registry.len(), 1);
    }

    // --- managed-process registry ------------------------------------------------

    fn entry(managed_id: &str, workspace: &str, service: &str, pid: u32, state: ManagedState) -> ManagedEntry {
        ManagedEntry {
            process: ManagedProcess {
                managedId: managed_id.to_string(),
                workspaceId: workspace.to_string(),
                serviceId: service.to_string(),
                rootPid: pid,
                creationTime: Some(1_700_000_000_000),
                processGroupId: pid, // group id == root pid by construction
                launchSpecId: "spec".to_string(),
                startedAt: 1_700_000_000_000,
                state,
            },
            logs: LogRing::new(8),
            state_since: Instant::now(),
        }
    }

    #[test]
    fn managed_registry_is_bounded() {
        let registry = ManagedProcessRegistry::default();
        for i in 0..(MANAGED_CAPACITY + 10) {
            let result = registry.insert(entry(
                &format!("m{i}"),
                "ws",
                "svc",
                u32::try_from(i).unwrap_or(1),
                ManagedState::Running,
            ));
            // Insert refuses beyond capacity rather than silently growing.
            if i >= MANAGED_CAPACITY {
                assert!(result.is_err());
            }
        }
        assert!(registry.len() <= MANAGED_CAPACITY);
    }

    #[test]
    fn has_running_service_tracks_liveness() {
        let registry = ManagedProcessRegistry::default();
        registry
            .insert(entry("m1", "ws1", "svc1", 100, ManagedState::Running))
            .expect("insert");
        assert!(registry.has_running_service("ws1", "svc1"));
        registry
            .with_entry("m1", |e| e.process.state = ManagedState::Stopped)
            .expect("entry");
        assert!(!registry.has_running_service("ws1", "svc1"));
    }

    #[test]
    fn workspace_processes_are_scoped_to_their_workspace() {
        let registry = ManagedProcessRegistry::default();
        registry.insert(entry("a", "ws1", "s1", 10, ManagedState::Running)).expect("insert");
        registry.insert(entry("b", "ws2", "s2", 20, ManagedState::Running)).expect("insert");
        assert_eq!(registry.workspace_processes("ws1").len(), 1);
        assert_eq!(registry.workspace_processes("ws2").len(), 1);
    }

    #[test]
    fn exited_ids_finds_alive_state_with_dead_process() {
        let registry = ManagedProcessRegistry::default();
        registry.insert(entry("m1", "ws", "s", 100, ManagedState::Running)).expect("insert");
        registry.insert(entry("m2", "ws", "s", 200, ManagedState::Stopped)).expect("insert");
        let exited = registry.exited_ids(&|pid| pid != 100);
        assert_eq!(exited, vec!["m1".to_string()]);
    }

    #[test]
    fn managed_pids_lists_root_pids() {
        let registry = ManagedProcessRegistry::default();
        registry.insert(entry("a", "ws", "s", 10, ManagedState::Running)).expect("insert");
        registry.insert(entry("b", "ws", "s2", 20, ManagedState::Starting)).expect("insert");
        let mut pids = registry.managed_pids();
        pids.sort_unstable();
        assert_eq!(pids, vec![10, 20]);
    }

    #[test]
    fn remove_drops_the_entry() {
        let registry = ManagedProcessRegistry::default();
        registry.insert(entry("m1", "ws", "s", 1, ManagedState::Running)).expect("insert");
        assert!(registry.remove("m1").is_some());
        assert!(registry.is_empty());
    }
}
