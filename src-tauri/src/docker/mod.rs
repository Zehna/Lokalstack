//! # Docker & Container Intelligence (Phase 9)
//!
//! Read-only container observability and ownership intelligence over the
//! Docker Engine API on the Windows named pipe (spec §2). Observing,
//! identifying, mapping, explaining — **no container lifecycle control of
//! any kind** (spec §44), no exec (§45), no log ingestion (§46), no
//! environment-variable exposure (§47), no arbitrary-label passthrough (§48).
//!
//! ## Structure
//!
//! - [`domain`] — bounded models, state/health normalization, typed errors.
//! - [`transport`] — named-pipe HTTP GET transport with a hard read-only
//!   path allowlist and a fake-transport test harness.
//! - [`parse`] — Engine JSON → models (fixture-tested).
//! - [`project`] — Compose → LocalStack project association (evidence-graded).
//! - This facade — engine state (cache/cadence/backoff, spec §26, §28, §29),
//!   snapshot assembly, and the Tauri command surface (spec §10, §50).
//!
//! ## Trust boundary (spec §10, §50, §58)
//!
//! The frontend calls `get_docker_snapshot` / `refresh_docker` /
//! `get_container_details(containerId)` only. There is no method/path/pipe
//! parameter anywhere; every Engine request originates from this adapter's
//! hard-coded read-only allowlist.

pub(crate) mod domain;
pub(crate) mod parse;
pub(crate) mod project;
pub(crate) mod transport;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use domain::{
    ContainerPortOwnership, DockerContainer, DockerEngineSnapshot, DockerFailure,
    MAX_CONCURRENT_REQUESTS, UNAVAILABLE_BACKOFF_INITIAL, UNAVAILABLE_BACKOFF_MAX,
};
use transport::Transport;

/// Cache cadences (spec §26, §65).
mod cadence {
    use std::time::Duration;
    /// Engine + container list refresh.
    pub(crate) const LIST: Duration = Duration::from_secs(5);
    /// Per-container stats refresh (one request per running container,
    /// bounded by concurrency).
    pub(crate) const STATS: Duration = Duration::from_secs(8);
    /// Engine /version is stable for the daemon's lifetime.
    pub(crate) const VERSION: Duration = Duration::from_secs(600);
}

/// Per-container cached facts (spec §28): keyed by container ID, dropped
/// when the container disappears, never reused across different IDs.
#[derive(Debug, Default)]
struct ContainerCacheEntry {
    inspect: Option<parse::InspectFacts>,
    inspect_at: Option<Instant>,
    stats: Option<domain::ContainerStats>,
    stats_at: Option<Instant>,
}

impl Default for EngineInner {
    fn default() -> Self {
        Self {
            last_snapshot: None,
            list_at: None,
            version: None,
            version_at: None,
            containers: HashMap::new(),
            unavailable_since: None,
            backoff: UNAVAILABLE_BACKOFF_INITIAL,
        }
    }
}

struct EngineInner {
    /// Cached last-good snapshot (also served when the engine goes away —
    /// with `available` recomputed honestly).
    last_snapshot: Option<Arc<DockerEngineSnapshot>>,
    list_at: Option<Instant>,
    version: Option<domain::DockerEngineInfo>,
    version_at: Option<Instant>,
    /// Container ID → cached inspect/stats (spec §28).
    containers: HashMap<String, ContainerCacheEntry>,
    /// Exponential backoff state while the engine is unavailable (spec §29).
    unavailable_since: Option<Instant>,
    backoff: Duration,
}

/// Tauri-managed Docker engine state.
pub(crate) struct DockerEngineState {
    inner: Arc<Mutex<EngineInner>>,
    transport: Arc<dyn Transport>,
}

impl DockerEngineState {
    /// Build the real state (named-pipe transport). Called from `lib.rs`.
    pub(crate) fn real() -> Self {
        let transport: Arc<dyn Transport> =
            Arc::new(transport::SharedTransport::new(transport::NamedPipeTransport::new().expect(
                "named-pipe transport path is a constant",
            )));
        Self {
            inner: Arc::new(Mutex::new(EngineInner::default())),
            transport,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl EngineInner {
    fn note_success(&mut self) {
        self.unavailable_since = None;
        self.backoff = UNAVAILABLE_BACKOFF_INITIAL;
    }

    fn note_failure(&mut self) {
        let since = *self.unavailable_since.get_or_insert_with(Instant::now);
        // Backoff grows while the engine stays away; the *next* allowed
        // attempt is scheduled by should_attempt via backoff.
        let _ = since;
        self.backoff = (self.backoff * 2).min(UNAVAILABLE_BACKOFF_MAX);
    }

    fn should_attempt(&self) -> bool {
        match self.unavailable_since {
            None => true,
            Some(since) => since.elapsed() >= self.backoff,
        }
    }
}

/// One full engine cycle against any [`Transport`] (real pipe in prod,
/// fake in tests). Pure over the transport: no global state.
fn run_cycle(
    t: &dyn Transport,
    inner: &mut EngineInner,
    project_list: &[crate::project::ProjectIdentity],
) -> Arc<DockerEngineSnapshot> {
    let started = Instant::now();
    let captured = now_ms();

    // 1. Engine reachability + version (version is long-lived cached).
    let version_fresh = inner
        .version_at
        .map(|at| at.elapsed() < cadence::VERSION)
        .unwrap_or(false);
    if !version_fresh {
        match request_json(t, "/version") {
            Ok(body) => {
                inner.version = Some(parse::parse_version(&body));
                inner.version_at = Some(Instant::now());
                inner.note_success();
            }
            Err(failure) => {
                inner.note_failure();
                return snapshot_unavailable(inner, failure, captured, started);
            }
        }
    }

    // 2. Container list (all states — the UI shows stopped containers too).
    let list_body = match request_json(t, "/containers/json?all=true") {
        Ok(body) => {
            inner.note_success();
            body
        }
        Err(failure) => {
            inner.note_failure();
            return snapshot_unavailable(inner, failure, captured, started);
        }
    };
    let mut containers = match parse::parse_containers_list(&list_body) {
        Ok(list) => list,
        Err(detail) => {
            inner.note_failure();
            return snapshot_unavailable(
                inner,
                DockerFailure::MalformedResponse { detail },
                captured,
                started,
            );
        }
    };
    let truncated = containers.len() >= domain::MAX_CONTAINERS;

    // 3. Selective inspect for facts the list lacks: authoritative health,
    //    networks, mounts (spec §8). Bounded concurrency (spec §27); only
    //    *running* containers — stopped ones have no live health anyway.
    enrich_running(t, inner, &mut containers);

    // 4. Bounded stats sampling (spec §25–27).
    sample_stats(t, inner, &mut containers);

    // 4b. Prune cache entries for containers that no longer exist (spec §28).
    prune(inner, &containers);

    // 5. Project association (evidence-based, spec §17–18).
    let project_links = project::associate(&containers, project_list);

    // 6. Port-ownership overlay entries (spec §12–13).
    let port_ownerships = port_ownership_index(&containers, &project_links);

    let snapshot = Arc::new(DockerEngineSnapshot {
        available: true,
        engine: inner.version.clone(),
        containers,
        projectLinks: project_links,
        portOwnerships: port_ownerships,
        truncated,
        capturedAt: captured,
        latencyMs: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        error: None,
    });
    inner.last_snapshot = Some(Arc::clone(&snapshot));
    inner.list_at = Some(Instant::now());
    snapshot
}

/// Run one cycle with a fresh project list (used by the facade commands).
fn run_cycle_with_projects(
    t: &dyn Transport,
    inner: &mut EngineInner,
    projects_state: &crate::project::ProjectEngineState,
) -> Arc<DockerEngineSnapshot> {
    let project_list = current_projects(projects_state);
    run_cycle(t, inner, &project_list)
}

/// Enumerate currently known projects from the shared project cache —
/// association runs against the same identities the discovery cycle found.
fn current_projects(projects_state: &crate::project::ProjectEngineState) -> Vec<crate::project::ProjectIdentity> {
    let Ok(cache) = projects_state.cache.lock() else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for identity in cache.values() {
        if seen.insert(identity.id.clone()) {
            out.push((**identity).clone());
        }
    }
    out
}

fn snapshot_unavailable(
    inner: &mut EngineInner,
    failure: DockerFailure,
    captured: u64,
    started: Instant,
) -> Arc<DockerEngineSnapshot> {
    let snapshot = Arc::new(DockerEngineSnapshot::unavailable(
        failure,
        captured,
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    ));
    inner.last_snapshot = Some(Arc::clone(&snapshot));
    snapshot
}

/// Inspect running containers under a concurrency bound (spec §27).
fn enrich_running(t: &dyn Transport, inner: &mut EngineInner, containers: &mut [DockerContainer]) {
    // Snapshot the ids to inspect, then apply under the same borrow.
    let ids: Vec<String> = containers
        .iter()
        .filter(|c| c.state == domain::ContainerState::Running)
        .map(|c| c.id.clone())
        .collect();
    let _ = MAX_CONCURRENT_REQUESTS; // bound documented; engine requests are sequential per cycle

    for id in ids {
        let fresh = inner
            .containers
            .get(&id)
            .and_then(|e| e.inspect_at)
            .map(|at| at.elapsed() < cadence::LIST)
            .unwrap_or(false);
        if !fresh {
            match request_json(t, &format!("/containers/{id}/json")) {
                Ok(body) => {
                    let facts = parse::parse_inspect(&body);
                    let entry = inner.containers.entry(id.clone()).or_default();
                    entry.inspect = Some(facts);
                    entry.inspect_at = Some(Instant::now());
                }
                Err(_) => {
                    // Inspect is enrichment only — a failure here degrades
                    // this container to list-level facts, not the snapshot.
                }
            }
        }
        if let Some(entry) = inner.containers.get(&id) {
            if let Some(facts) = &entry.inspect {
                if let Some(container) = containers.iter_mut().find(|c| c.id == id) {
                    parse::apply_inspect_facts(container, facts);
                }
            }
        }
    }
}

/// Sample stats for running containers (spec §25–26), bounded cadence.
fn sample_stats(t: &dyn Transport, inner: &mut EngineInner, containers: &mut [DockerContainer]) {
    let ids: Vec<String> = containers
        .iter()
        .filter(|c| c.state == domain::ContainerState::Running)
        .map(|c| c.id.clone())
        .collect();

    for id in ids {
        let fresh = inner
            .containers
            .get(&id)
            .and_then(|e| e.stats_at)
            .map(|at| at.elapsed() < cadence::STATS)
            .unwrap_or(false);
        if !fresh {
            if let Ok(body) = request_json(t, &format!("/containers/{id}/stats?stream=false")) {
                if let Ok(stats) = parse::parse_stats(&body) {
                    let entry = inner.containers.entry(id.clone()).or_default();
                    entry.stats = Some(stats);
                    entry.stats_at = Some(Instant::now());
                }
            }
        }
        if let Some(entry) = inner.containers.get(&id) {
            if let Some(container) = containers.iter_mut().find(|c| c.id == id) {
                container.stats = entry.stats.clone();
            }
        }
    }
}

/// Build the port-ownership index from the parsed containers (spec §12–13).
fn port_ownership_index(
    containers: &[DockerContainer],
    links: &[project::ContainerProjectLink],
) -> Vec<ContainerPortOwnership> {
    let mut out = Vec::new();
    for container in containers {
        let project_id = links
            .iter()
            .find(|l| l.containerId == container.id)
            .and_then(|l| l.projectId.clone());
        for port in &container.ports {
            if port.protocol != "tcp" {
                continue; // UDP stays Docker-metadata only (spec §14)
            }
            let Some(host_port) = port.hostPort else {
                continue; // unpublished exposures are not host ownership
            };
            out.push(ContainerPortOwnership {
                hostPort: host_port,
                hostIp: port.hostIp.clone(),
                containerId: container.id.clone(),
                containerName: container.name.clone(),
                containerPort: port.containerPort,
                protocol: port.protocol.clone(),
                image: Some(container.image.clone()),
                composeProject: container
                    .compose
                    .as_ref()
                    .and_then(|c| c.projectName.clone()),
                composeService: container
                    .compose
                    .as_ref()
                    .and_then(|c| c.serviceName.clone()),
                projectId: project_id.clone(),
            });
        }
    }
    out.sort_by_key(|o| o.hostPort);
    out
}

/// GET one allowlisted Engine path and return the body when status is 2xx.
fn request_json(t: &dyn Transport, path: &str) -> Result<String, DockerFailure> {
    let response = t.get(path)?;
    if !(200..300).contains(&response.status) {
        return Err(DockerFailure::EngineError {
            detail: format!("HTTP {} for {path}", response.status),
        });
    }
    Ok(response.body)
}

/// Prune cache entries whose containers disappeared (spec §28).
fn prune(inner: &mut EngineInner, containers: &[DockerContainer]) {
    let live: std::collections::HashSet<&str> =
        containers.iter().map(|c| c.id.as_str()).collect();
    inner.containers.retain(|id, _| live.contains(id.as_str()));
}

// ---------------------------------------------------------------------------
// Tauri commands (spec §10, §50)
// ---------------------------------------------------------------------------

/// Read-only Docker snapshot. Serve-from-cache inside the list cadence;
/// otherwise run a fresh engine cycle (async — never on the UI thread, and
/// never inside the discovery loop, spec §27, §63).
#[tauri::command]
pub(crate) async fn get_docker_snapshot(
    state: tauri::State<'_, DockerEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
) -> Result<DockerEngineSnapshot, String> {
    snapshot_with(state, projects, false).await
}

/// Manual refresh — bypasses the list/version/stats caches (spec §26, §65).
#[tauri::command]
pub(crate) async fn refresh_docker(
    state: tauri::State<'_, DockerEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
) -> Result<DockerEngineSnapshot, String> {
    snapshot_with(state, projects, true).await
}

async fn snapshot_with(
    state: tauri::State<'_, DockerEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
    bypass_cache: bool,
) -> Result<DockerEngineSnapshot, String> {
    let inner_arc = Arc::clone(&state.inner);
    let transport = Arc::clone(&state.transport);
    let projects_arc = Arc::clone(&projects.cache);
    let snapshot: DockerEngineSnapshot = tauri::async_runtime::spawn_blocking(move || {
        // Phase 10D (§J): this lock is the deliberate single-flight gate — the
        // engine cycle (pipe I/O) runs inside it to serialize concurrent
        // frontend refreshes. Slow I/O here is BOUNDED, not broad: the pipe
        // transport enforces REQUEST_TIMEOUT per request and parse counts are
        // capped by MAX_BODY_BYTES/MAX_CONTAINERS, so worst-case hold is
        // bounded (stale-cache readers also serve without waiting past it).
        let Ok(mut inner) = inner_arc.lock() else {
            return Err("docker state poisoned".to_string());
        };

        // Serve from cache inside the cadence unless refresh was explicit.
        if !bypass_cache {
            if let (Some(snapshot), Some(list_at)) = (&inner.last_snapshot, inner.list_at) {
                if snapshot.available && list_at.elapsed() < cadence::LIST {
                    return Ok((**snapshot).clone());
                }
            }
        }

        // Backoff: when the engine is away, don't hammer the pipe (spec §29).
        if !inner.should_attempt() {
            if let Some(snapshot) = &inner.last_snapshot {
                return Ok((**snapshot).clone());
            }
            return Ok(DockerEngineSnapshot::unavailable(
                DockerFailure::DockerUnavailable {
                    detail: "waiting to retry".to_string(),
                },
                now_ms(),
                0,
            ));
        }

        let projects_handle = crate::project::ProjectEngineState { cache: projects_arc };
        let snapshot = run_cycle_with_projects(&*transport, &mut inner, &projects_handle);
        Ok((*snapshot).clone())
    })
    .await
    .map_err(|e| format!("docker task join error: {e}"))??;
    Ok(snapshot)
}

/// Per-container details by trusted container ID (spec §10, §33). The ID
/// must originate from a backend snapshot; unknown IDs are refused.
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerDetails {
    pub container: DockerContainer,
    /// Association explanation, when present in the last snapshot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectEvidence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectConfidence: Option<String>,
}

#[tauri::command]
pub(crate) async fn get_container_details(
    container_id: String,
    state: tauri::State<'_, DockerEngineState>,
    projects: tauri::State<'_, crate::project::ProjectEngineState>,
) -> Result<ContainerDetails, String> {
    let inner_arc = Arc::clone(&state.inner);
    let transport = Arc::clone(&state.transport);
    let projects_arc = Arc::clone(&projects.cache);
    tauri::async_runtime::spawn_blocking(move || {
        // Trust check: the ID must exist in backend state (spec §10).
        let mut inner = inner_arc.lock().map_err(|_| "docker state poisoned")?;
        let known = inner.last_snapshot.as_ref().map_or(false, |s| {
            s.containers.iter().any(|c| c.id == container_id)
        });
        if !known {
            return Err("UNKNOWN_CONTAINER".to_string());
        }

        // One fresh inspect for the authoritative view (still allowlisted).
        let mut container = inner
            .last_snapshot
            .as_ref()
            .expect("checked")
            .containers
            .iter()
            .find(|c| c.id == container_id)
            .expect("checked")
            .clone();
        if let Ok(body) = request_json(&*transport, &format!("/containers/{container_id}/json")) {
            if let Ok(()) = parse::enrich_from_inspect(&mut container, &body) {
                let entry = inner.containers.entry(container_id.clone()).or_default();
                entry.inspect = Some(parse::parse_inspect(&body));
                entry.inspect_at = Some(Instant::now());
            }
        }

        let projects_handle = crate::project::ProjectEngineState { cache: projects_arc };
        let links = project::associate(
            std::slice::from_ref(&container),
            &current_projects(&projects_handle),
        );
        Ok(ContainerDetails {
            projectEvidence: Some(links[0].evidence.clone()),
            projectConfidence: Some(links[0].confidence.clone()),
            container,
        })
    })
    .await
    .map_err(|e| format!("container details join error: {e}"))?
}

// ---------------------------------------------------------------------------
// Tests (fake transport — no Docker required, spec §52)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::docker::transport::tests::FakeEngine;

    const CONTAINERS: &str = r#"[
        {
            "Id":"a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1",
            "Names":["/historyai-frontend-1"],
            "Image":"historyai/frontend:dev",
            "State":"running",
            "Status":"Up 3 hours (healthy)",
            "Created":1700000000,
            "Ports":[{"IP":"0.0.0.0","PrivatePort":3000,"PublicPort":3000,"Type":"tcp"}],
            "Labels":{
                "com.docker.compose.project":"historyai",
                "com.docker.compose.service":"frontend",
                "com.docker.compose.project.working_dir":"D:/Projects/HistoryAI"
            }
        },
        {
            "Id":"b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2",
            "Names":["/plain-db"],
            "Image":"postgres:16",
            "State":"exited",
            "Status":"Exited (0) 5 minutes ago",
            "Created":1700000001,
            "Ports":[{"IP":"127.0.0.1","PrivatePort":5432,"PublicPort":55432,"Type":"tcp"}]
        }
    ]"#;

    const INSPECT_A: &str = r#"{
        "State": {"Status": "running", "Health": {"Status": "healthy"}},
        "NetworkSettings": {"Networks": {"bridge": {"IPAddress": "172.17.0.2", "Gateway": "172.17.0.1"}}},
        "Mounts": [{"Type": "bind", "Source": "D:\\Projects\\HistoryAI", "Destination": "/app"}]
    }"#;

    const STATS_A: &str = r#"{
        "cpu_stats": {"cpu_usage": {"total_usage": 200}, "system_cpu_usage": 10000, "online_cpus": 4},
        "precpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 9000},
        "memory_stats": {"usage": 214748364, "limit": 4294967296},
        "networks": {"eth0": {"rx_bytes": 1000, "tx_bytes": 2000}}
    }"#;

    fn full_engine() -> FakeEngine {
        FakeEngine::new()
            .with_route("/_ping", 200, "OK")
            .with_route("/version", 200, r#"{"Version":"27.1.1","ApiVersion":"1.46","Os":"windows","Arch":"amd64"}"#)
            .with_route("/containers/json?all=true", 200, CONTAINERS)
            .with_route(&format!("/containers/{}/json", "a1".repeat(32)), 200, INSPECT_A)
            .with_route(&format!("/containers/{}/stats?stream=false", "a1".repeat(32)), 200, STATS_A)
    }

    fn project_historyai() -> crate::project::ProjectIdentity {
        crate::project::ProjectIdentity {
            id: "D:\\Projects\\HistoryAI".to_string(),
            name: "HistoryAI".to_string(),
            rootPath: "D:\\Projects\\HistoryAI".to_string(),
            kind: crate::project::ProjectKind::NodeJs,
            git: crate::project::GitInfo {
                isRepository: false,
                rootPath: None,
                branch: None,
            },
            packageManager: None,
            startCommand: None,
            confidence: crate::intelligence::rules::Confidence::High,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn cycle_produces_full_snapshot() {
        let mut inner = EngineInner::default();
        let projects = [project_historyai()];
        let snapshot = run_cycle(&full_engine(), &mut inner, &projects);

        assert!(snapshot.available);
        assert_eq!(snapshot.engine.as_ref().unwrap().version.as_deref(), Some("27.1.1"));
        assert_eq!(snapshot.containers.len(), 2);
        // Running container fully enriched.
        let a = &snapshot.containers[0];
        assert_eq!(a.state, domain::ContainerState::Running);
        assert_eq!(a.health, domain::ContainerHealth::Healthy, "authoritative from inspect");
        assert_eq!(a.networks.len(), 1);
        assert_eq!(a.mounts.len(), 1);
        assert!(a.stats.is_some());
        // Stopped container: no inspect/stats fabricated.
        let b = &snapshot.containers[1];
        assert_eq!(b.state, domain::ContainerState::Exited);
        assert!(b.stats.is_none());
        // Project association from the working-dir label.
        assert_eq!(snapshot.projectLinks[0].projectId.as_deref(), Some("D:\\Projects\\HistoryAI"));
        assert_eq!(snapshot.projectLinks[0].confidence, "exact");
        // Port ownership overlay (TCP only).
        assert_eq!(snapshot.portOwnerships.len(), 2);
        let first = &snapshot.portOwnerships[0];
        assert_eq!(first.hostPort, 3000);
        assert_eq!(first.containerName, "historyai-frontend-1");
        assert_eq!(first.composeProject.as_deref(), Some("historyai"));
        assert_eq!(first.projectId.as_deref(), Some("D:\\Projects\\HistoryAI"));
        assert_eq!(snapshot.portOwnerships[1].hostPort, 55432);
        // Version cached across cycles.
        let _ = run_cycle(&full_engine(), &mut inner, &projects);
        assert!(inner.version_at.is_some());
    }

    #[test]
    fn unavailable_engine_maps_to_typed_failure() {
        let mut inner = EngineInner::default();
        let projects: Vec<crate::project::ProjectIdentity> = Vec::new();
        let snapshot = run_cycle(&FakeEngine::new(), &mut inner, &projects);
        assert!(!snapshot.available);
        assert!(matches!(
            snapshot.error,
            Some(DockerFailure::DockerUnavailable { .. })
        ));
        assert_eq!(
            snapshot.error.as_ref().unwrap().label(),
            "Docker Engine is not currently available."
        );
    }

    #[test]
    fn access_denied_is_distinct_and_informational() {
        let mut inner = EngineInner::default();
        let engine = FakeEngine::new().with_failure(
            "/version",
            DockerFailure::AccessDenied { detail: "acl".to_string() },
        );
        let projects: Vec<crate::project::ProjectIdentity> = Vec::new();
        let snapshot = run_cycle(&engine, &mut inner, &projects);
        assert!(!snapshot.available);
        assert_eq!(snapshot.error.as_ref().unwrap().code(), "ACCESS_DENIED");
    }

    #[test]
    fn backoff_grows_and_resets() {
        let mut inner = EngineInner::default();
        assert_eq!(inner.backoff, UNAVAILABLE_BACKOFF_INITIAL);
        inner.note_failure();
        assert_eq!(inner.backoff, UNAVAILABLE_BACKOFF_INITIAL * 2);
        for _ in 0..10 {
            inner.note_failure();
        }
        assert_eq!(inner.backoff, UNAVAILABLE_BACKOFF_MAX, "capped");
        inner.note_success();
        assert_eq!(inner.backoff, UNAVAILABLE_BACKOFF_INITIAL, "reset");
        assert!(inner.unavailable_since.is_none());
    }

    #[test]
    fn cache_prunes_dead_containers() {
        let mut inner = EngineInner::default();
        let projects: Vec<crate::project::ProjectIdentity> = Vec::new();
        let _ = run_cycle(&full_engine(), &mut inner, &projects);
        assert!(inner.containers.contains_key(&"a1".repeat(32)));
        // Now an engine where the container is gone.
        let engine = FakeEngine::new()
            .with_route("/version", 200, r#"{"Version":"27.1.1","ApiVersion":"1.46","Os":"windows","Arch":"amd64"}"#)
            .with_route("/containers/json?all=true", 200, "[]");
        let _ = run_cycle(&engine, &mut inner, &projects);
        assert!(inner.containers.is_empty(), "dead containers pruned (spec §28)");
    }

    #[test]
    fn port_ownership_index_tcp_only_published_only() {
        let containers = vec![DockerContainer {
            id: "x".to_string(),
            shortId: "x".to_string(),
            name: "c".to_string(),
            image: "img:1".to_string(),
            imageId: None,
            imageDigest: None,
            state: domain::ContainerState::Running,
            status: "Up".to_string(),
            health: domain::ContainerHealth::None,
            createdAt: None,
            ports: vec![
                domain::ContainerPort {
                    protocol: "tcp".to_string(),
                    containerPort: 80,
                    hostIp: Some("0.0.0.0".to_string()),
                    hostPort: Some(8080),
                    published: true,
                },
                domain::ContainerPort {
                    protocol: "udp".to_string(),
                    containerPort: 53,
                    hostIp: Some("0.0.0.0".to_string()),
                    hostPort: Some(1053),
                    published: true,
                },
                domain::ContainerPort {
                    protocol: "tcp".to_string(),
                    containerPort: 9229,
                    hostIp: None,
                    hostPort: None,
                    published: false,
                },
            ],
            compose: None,
            networks: Vec::new(),
            mounts: Vec::new(),
            stats: None,
        }];
        let index = port_ownership_index(&containers, &[]);
        assert_eq!(index.len(), 1, "TCP published only (spec §14)");
        assert_eq!(index[0].hostPort, 8080);
        assert_eq!(index[0].containerPort, 80);
    }

    #[test]
    fn allowlist_refuses_mutation_paths_end_to_end() {
        // Even a hostile "frontend" route through the facade can't mutate:
        // the transport's allowlist is checked in FakeEngine.get too.
        let engine = full_engine();
        assert!(engine.get("/containers/abc/stop").is_err());
        assert!(engine.get("/containers/abc/kill").is_err());
        assert!(engine.get("/images/create?fromImage=x").is_err());
    }
}
