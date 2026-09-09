//! Pure Docker domain model, typed errors, and response bounds (spec §5–7,
//! §47–49, §66–67).
//!
//! Everything here is OS-independent and fixture-testable. The privacy
//! rules are structural: the DTOs below are the *only* shapes that cross to
//! React, and they contain no field for environment variables, raw inspect
//! payloads, Docker auth, or arbitrary labels (spec §47, §48, §58).

use serde::Serialize;

// ---------------------------------------------------------------------------
// Bounds (spec §67)
// ---------------------------------------------------------------------------

/// Maximum containers returned in one snapshot (an extreme host truncates
/// and reports truncation rather than allocating unbounded memory).
pub(crate) const MAX_CONTAINERS: usize = 500;
/// Maximum published-port mappings per container.
pub(crate) const MAX_PORTS_PER_CONTAINER: usize = 64;
/// Maximum mounts *considered* per container (internal evidence only).
pub(crate) const MAX_MOUNTS_PER_CONTAINER: usize = 32;
/// Maximum networks listed per container.
pub(crate) const MAX_NETWORKS_PER_CONTAINER: usize = 16;
/// Maximum accepted response body from the Engine API (a container list on
/// a large host fits far below this).
pub(crate) const MAX_BODY_BYTES: usize = 8 * 1024 * 1024; // 8 MB
/// Connect/read timeout for one Engine request.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);
/// Maximum simultaneous Engine requests (spec §27).
pub(crate) const MAX_CONCURRENT_REQUESTS: usize = 4;
/// Backoff base when the engine is unavailable (spec §29) — doubles up to
/// the cap, reset on success.
pub(crate) const UNAVAILABLE_BACKOFF_INITIAL: std::time::Duration = std::time::Duration::from_secs(5);
pub(crate) const UNAVAILABLE_BACKOFF_MAX: std::time::Duration = std::time::Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Typed errors (spec §66)
// ---------------------------------------------------------------------------

/// Structured Docker error model (spec §66).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DockerFailure {
    /// Docker Engine is not currently available (not an app error, spec §29).
    DockerUnavailable { detail: String },
    /// The pipe exists but access was denied (informational only, spec §30).
    AccessDenied { detail: String },
    /// The request exceeded the timeout.
    Timeout,
    /// The daemon rejected the API version / endpoint for this daemon.
    /// Not produced by the current fixed-endpoint adapter, but part of the
    /// typed error contract (spec §66) for future API-version handling.
    #[allow(dead_code)]
    ApiUnsupported { detail: String },
    /// The body was not the JSON the adapter contract expects.
    MalformedResponse { detail: String },
    /// The response exceeded the size bound.
    ResponseTooLarge { limit: usize },
    /// Any other engine-side error (HTTP 5xx, broken pipe…).
    EngineError { detail: String },
}

impl DockerFailure {
    /// Short honest label for the UI (no raw pipe internals, spec §66).
    /// Consumed by the frontend error surface via the serialized snapshot
    /// (`error.label`) and asserted in the domain tests.
    #[allow(dead_code)]
    pub(crate) fn label(&self) -> String {
        match self {
            DockerFailure::DockerUnavailable { .. } => {
                "Docker Engine is not currently available.".to_string()
            }
            DockerFailure::AccessDenied { .. } => {
                "Docker Engine detected but access was denied.".to_string()
            }
            DockerFailure::Timeout => "Docker Engine did not respond in time.".to_string(),
            DockerFailure::ApiUnsupported { detail } => {
                format!("Docker API not supported by this engine: {detail}")
            }
            DockerFailure::MalformedResponse { .. } => {
                "Docker Engine returned an unexpected payload.".to_string()
            }
            DockerFailure::ResponseTooLarge { .. } => {
                "Docker Engine response exceeded the size limit.".to_string()
            }
            DockerFailure::EngineError { .. } => "Docker Engine error.".to_string(),
        }
    }

    /// Machine-readable code for history/dependency surfaces.
    #[allow(dead_code)]
    pub(crate) fn code(&self) -> &'static str {
        match self {
            DockerFailure::DockerUnavailable { .. } => "DOCKER_UNAVAILABLE",
            DockerFailure::AccessDenied { .. } => "ACCESS_DENIED",
            DockerFailure::Timeout => "TIMEOUT",
            DockerFailure::ApiUnsupported { .. } => "API_UNSUPPORTED",
            DockerFailure::MalformedResponse { .. } => "MALFORMED_RESPONSE",
            DockerFailure::ResponseTooLarge { .. } => "RESPONSE_TOO_LARGE",
            DockerFailure::EngineError { .. } => "ENGINE_ERROR",
        }
    }
}

// ---------------------------------------------------------------------------
// Container lifecycle + health (spec §6–7)
// ---------------------------------------------------------------------------

/// Docker container lifecycle state — **distinct** from any process
/// lifecycle and from health (spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContainerState {
    Created,
    Running,
    Paused,
    Restarting,
    Removing,
    Exited,
    Dead,
    /// A state string Docker added that this build does not know.
    Unknown,
}

/// Normalize Docker's `State.Status` string (spec §6).
pub(crate) fn parse_container_state(status: &str) -> ContainerState {
    match status {
        "created" => ContainerState::Created,
        "running" => ContainerState::Running,
        "paused" => ContainerState::Paused,
        "restarting" => ContainerState::Restarting,
        "removing" => ContainerState::Removing,
        "exited" => ContainerState::Exited,
        "dead" => ContainerState::Dead,
        _ => ContainerState::Unknown,
    }
}

/// Docker Healthcheck evidence (spec §7) — `none` when no healthcheck is
/// configured; running alone never implies healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContainerHealth {
    Healthy,
    Unhealthy,
    Starting,
    /// No healthcheck configured.
    None,
    /// Health data exists but could not be understood.
    Unknown,
}

/// Normalize Docker's `State.Health.Status` (spec §7).
pub(crate) fn parse_container_health(health: Option<&str>, has_health_key: bool) -> ContainerHealth {
    match health {
        Some("healthy") => ContainerHealth::Healthy,
        Some("unhealthy") => ContainerHealth::Unhealthy,
        Some("starting") => ContainerHealth::Starting,
        Some(_) => ContainerHealth::Unknown,
        None if has_health_key => ContainerHealth::Unknown,
        None => ContainerHealth::None,
    }
}

// ---------------------------------------------------------------------------
// Ports, mounts, networks, compose, stats
// ---------------------------------------------------------------------------

/// One published port mapping (spec §11) — container and host ports stay
/// distinct concepts; UDP metadata is displayed as Docker-only facts (the
/// Phase 1 scanner is TCP, and we do not pretend otherwise, spec §14).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerPort {
    /// `tcp` | `udp`.
    pub protocol: String,
    /// Port inside the container (e.g. 80).
    pub containerPort: u16,
    /// Host bind IP (`0.0.0.0`, `127.0.0.1`, `::`…), when published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostIp: Option<String>,
    /// Host port, when published.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostPort: Option<u16>,
    /// False for exposed-but-unpublished ports.
    pub published: bool,
}

/// Mount metadata used internally for project association (spec §19–20).
/// Source paths are used as *evidence*; the details view may show them, but
/// file contents are never read and `.env`/secret-looking paths are excluded
/// from display.
///
/// The facade stores mounts directly on the container model as
/// `(source, destination)` pairs (bounded by `MAX_MOUNTS_PER_CONTAINER`);
/// this named struct is intentionally not used to avoid duplicating the
/// evidence into a second shape.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MountEvidence {
    /// Host path (e.g. `D:\Projects\HistoryAI`).
    pub source: String,
    /// Container path (e.g. `/app`).
    pub destination: String,
}

/// Compose identity from canonical labels (spec §15–16) — labels are the
/// evidence; container *names* are never sufficient (spec §55).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct DockerComposeIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectName: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serviceName: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub containerNumber: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workingDir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configFiles: Option<String>,
}

/// One Docker network the container is attached to (spec §23) — metadata
/// only; no network discovery/scan is ever performed (spec §24).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerNetwork {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipAddress: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<String>,
}

/// One read-only stats observation (spec §25) — `stats?stream=false` only.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerStats {
    /// CPU utilization 0–100 (relative to one core, daemon-normalized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpuPercent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memoryUsedBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memoryLimitBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networkRxBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub networkTxBytes: Option<u64>,
}

/// Full snapshot delivered by `get_docker_snapshot` (spec §5).
///
/// Extends the engine facts with the evidence-based container → project
/// association (spec §17–18) and Docker → listener port ownership hints
/// (spec §12–13): one entry per published TCP host port, mapped to the
/// container that publishes it. A mapping is *docker metadata* for the
/// UI overlay; host PID evidence always wins for process facts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct DockerContainer {
    /// Full 64-hex container ID (stable identity, spec §28).
    pub id: String,
    /// 12-hex display form.
    pub shortId: String,
    /// Primary human-readable name (leading `/` stripped).
    pub name: String,
    /// Normalized image reference (e.g. `postgres:16`).
    pub image: String,
    /// Image ID (sha256:…) — stored separately from the reference (spec §21).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imageId: Option<String>,
    /// Local digest when the daemon already returned one (no registry calls,
    /// spec §22).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imageDigest: Option<String>,
    pub state: ContainerState,
    /// Human-facing status string (e.g. `Up 3 hours`).
    pub status: String,
    pub health: ContainerHealth,
    /// Unix ms creation timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub createdAt: Option<u64>,
    pub ports: Vec<ContainerPort>,
    /// Compose identity from canonical labels (spec §15–16).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compose: Option<DockerComposeIdentity>,
    pub networks: Vec<ContainerNetwork>,
    /// Selected mount metadata (details view only; evidence internally).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<(String, String)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<ContainerStats>,
}

/// Docker → listener port-ownership hint (spec §12–13): one published TCP
/// host port and the container that publishes it. The host *listener's* PID
/// facts remain authoritative for process identity; this supplies the
/// container/compose dimension that a host-side PID cannot.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerPortOwnership {
    pub hostPort: u16,
    /// `0.0.0.0` / specific IP / `::` as Docker reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostIp: Option<String>,
    pub containerId: String,
    pub containerName: String,
    pub containerPort: u16,
    pub protocol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Compose project/service when canonical labels exist (spec §15).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composeProject: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub composeService: Option<String>,
    /// Associated LocalStack project id when the evidence supports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectId: Option<String>,
}

/// Engine identity (spec §49) — useful facts only, never the raw `/info`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct DockerEngineInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub apiVersion: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

/// Full snapshot delivered by `get_docker_snapshot` (spec §5).
///
/// Extends the engine facts with the evidence-based container → project
/// association (spec §17–18) and Docker → listener port ownership hints
/// (spec §12–13): one entry per published TCP host port, mapped to the
/// container that publishes it. A mapping is *docker metadata* for the
/// UI overlay; host PID evidence always wins for process facts.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct DockerEngineSnapshot {
    /// False when Docker is absent/stopped — an honest state, not an error
    /// banner (spec §29).
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine: Option<DockerEngineInfo>,
    pub containers: Vec<DockerContainer>,
    /// Evidence-based container → LocalStack project association.
    pub projectLinks: Vec<super::project::ContainerProjectLink>,
    /// Published TCP host ports → owning container (overlay metadata).
    pub portOwnerships: Vec<ContainerPortOwnership>,
    /// True when MAX_CONTAINERS forced truncation (spec §67).
    pub truncated: bool,
    /// Unix ms capture time.
    pub capturedAt: u64,
    /// Wall-clock duration of the engine cycle, ms.
    pub latencyMs: u64,
    /// Structured failure when `available == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<DockerFailure>,
}

impl DockerEngineSnapshot {
    pub(crate) fn unavailable(failure: DockerFailure, now_ms: u64, latency_ms: u64) -> Self {
        Self {
            available: false,
            engine: None,
            containers: Vec::new(),
            projectLinks: Vec::new(),
            portOwnerships: Vec::new(),
            truncated: false,
            capturedAt: now_ms,
            latencyMs: latency_ms,
            error: Some(failure),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_states_normalize() {
        assert_eq!(parse_container_state("running"), ContainerState::Running);
        assert_eq!(parse_container_state("created"), ContainerState::Created);
        assert_eq!(parse_container_state("paused"), ContainerState::Paused);
        assert_eq!(parse_container_state("restarting"), ContainerState::Restarting);
        assert_eq!(parse_container_state("removing"), ContainerState::Removing);
        assert_eq!(parse_container_state("exited"), ContainerState::Exited);
        assert_eq!(parse_container_state("dead"), ContainerState::Dead);
        assert_eq!(parse_container_state("some-new-state"), ContainerState::Unknown);
    }

    #[test]
    fn health_requires_docker_evidence_not_running() {
        // No health key at all → None (no healthcheck configured).
        assert_eq!(parse_container_health(None, false), ContainerHealth::None);
        // Health key present but empty → Unknown, not Healthy.
        assert_eq!(parse_container_health(None, true), ContainerHealth::Unknown);
        // Evidence-based values.
        assert_eq!(parse_container_health(Some("healthy"), true), ContainerHealth::Healthy);
        assert_eq!(parse_container_health(Some("unhealthy"), true), ContainerHealth::Unhealthy);
        assert_eq!(parse_container_health(Some("starting"), true), ContainerHealth::Starting);
        assert_eq!(parse_container_health(Some("weird"), true), ContainerHealth::Unknown);
        // The critical semantic: running state alone never appears here.
        assert_ne!(parse_container_health(None, false), ContainerHealth::Healthy);
    }

    #[test]
    fn error_labels_hide_pipe_internals() {
        let f = DockerFailure::AccessDenied {
            detail: r"\\.\pipe\docker_engine ACL 0x5".to_string(),
        };
        let label = f.label();
        assert!(!label.contains("pipe"), "{label}");
        assert!(!label.contains("0x5"), "{label}");
        assert_eq!(f.code(), "ACCESS_DENIED");
        assert_eq!(
            DockerFailure::DockerUnavailable { detail: "x".to_string() }.code(),
            "DOCKER_UNAVAILABLE"
        );
    }
}
