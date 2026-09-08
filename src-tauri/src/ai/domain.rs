//! Pure AI-runtime domain model, endpoint trust policy, and HTTP bounds.
//!
//! Everything here is OS-independent and unit-testable without any server:
//! the endpoint policy is the security core (loopback only, redirects
//! rejected, no credentials in URLs, http scheme only) and the limits keep a
//! malformed local runtime from allocating unbounded memory (spec §6, §9).
//!
//! # Privacy boundary (spec §42)
//!
//! Model names and runtime metadata are acceptable observability data.
//! Prompts, chat history, conversation databases, cookies, tokens, and user
//! accounts are never inspected — and no inference request is ever sent
//! (spec §45), so none of that data can even be generated.

use serde::Serialize;

// ---------------------------------------------------------------------------
// Limits (spec §9) — chosen values, documented here and in the docs.
// ---------------------------------------------------------------------------

/// Connect timeout for one AI probe.
pub(crate) const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);
/// Total request timeout for one AI probe (must stay far below anything that
/// could stall the interactive UI; discovery itself never waits on AI).
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Maximum accepted response body for metadata endpoints (JSON models lists
/// with hundreds of entries fit far below this).
pub(crate) const MAX_BODY_BYTES: usize = 2 * 1024 * 1024; // 2 MB
/// Maximum models per runtime kept in a snapshot (inventory list bound).
pub(crate) const MAX_MODELS: usize = 500;
/// Maximum loaded-model entries per snapshot.
pub(crate) const MAX_LOADED_MODELS: usize = 64;
/// Maximum simultaneous adapter probes (spec §28).
pub(crate) const MAX_CONCURRENT_PROBES: usize = 4;

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Runtime health — adapter-evidence-based, distinct from process lifecycle
/// (a process can be RUNNING while its runtime is LOADING) and distinct from
/// TCP listening (a listener proves only socket availability; spec §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AiHealth {
    /// Adapter-specific readiness evidence succeeded.
    Ready,
    /// The runtime reports itself as starting/loading a model.
    Loading,
    /// The runtime reports busy/degraded but functional endpoints.
    Busy,
    /// The runtime answered unexpectedly (payload mismatch).
    Degraded,
    /// The runtime could not be reached at all.
    Unavailable,
    /// Not yet probed / not enough evidence.
    Unknown,
}

/// One structured probe error (spec §46) — no "unknown error" collapse.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum AiProbeError {
    /// TCP connect failed / connection refused.
    ConnectFailed { detail: String },
    /// The request exceeded the timeout.
    Timeout,
    /// HTTP status was an error or an unsupported code path.
    HttpStatus { status: u16 },
    /// The body was not the JSON the adapter contract expects.
    MalformedJson { detail: String },
    /// The response exceeded the size bound.
    TooLarge { limit: usize },
    /// The endpoint URL failed the trust policy (never even requested).
    PolicyRejected { reason: String },
}

impl AiProbeError {
    /// Short, honest, UI-facing label.
    pub(crate) fn label(&self) -> String {
        match self {
            AiProbeError::ConnectFailed { .. } => "connection refused or unreachable".to_string(),
            AiProbeError::Timeout => "timed out".to_string(),
            AiProbeError::HttpStatus { status } => format!("HTTP {status}"),
            AiProbeError::MalformedJson { .. } => "unexpected payload".to_string(),
            AiProbeError::TooLarge { .. } => "response too large".to_string(),
            AiProbeError::PolicyRejected { reason } => format!("endpoint rejected: {reason}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Models / resources / capabilities
// ---------------------------------------------------------------------------

/// One model as observed from a runtime (spec §30). Identity is the only
/// required field; everything else stays absent when the provider does not
/// expose it — never cross-fabricated.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct AiModelInfo {
    /// Provider-specific model identifier (e.g. `qwen2.5:7b-instruct-q4_K_M`).
    pub id: String,
    /// Human-facing name when the provider separates it from the id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub displayName: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameterSize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sizeBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiedAt: Option<u64>,
    /// Currently resident in memory/VRAM.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded: Option<bool>,
    /// VRAM/RAM used by the loaded instance (reported, never estimated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vramBytes: Option<u64>,
    /// Provider status string (`loaded`, `downloading`…) when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// `expiresAt` for loaded Ollama models (keep-alive deadline).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiresAt: Option<u64>,
}

/// Device/resource observations (spec §31) — only what the runtime reported.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct AiResourceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vramTotalBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vramFreeBytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modelVramBytes: Option<u64>,
    /// Queue summary (ComfyUI): running / pending counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queueRunning: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queuePending: Option<u32>,
}

/// Explicit capability flags (spec §32) so the UI never renders broken
/// elements for a runtime that does not expose a feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct AiRuntimeCapabilities {
    pub models: bool,
    pub loadedModels: bool,
    pub version: bool,
    pub health: bool,
    pub metrics: bool,
    pub gpuStats: bool,
    pub queue: bool,
}

impl Default for AiRuntimeCapabilities {
    fn default() -> Self {
        Self {
            models: false,
            loadedModels: false,
            version: false,
            health: false,
            metrics: false,
            gpuStats: false,
            queue: false,
        }
    }
}

/// llama.cpp-style runtime properties from `/props` (spec §17) — a
/// conservative normalized subset, never a raw config blob.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct AiLlamaCppProps {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modelPath: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contextSize: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slotsTotal: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slotsIdle: Option<u32>,
}

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

/// Normalized, provider-agnostic runtime snapshot (spec §4). Raw provider
/// payloads stay in Rust; React sees only this shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct AiRuntimeSnapshot {
    /// Stable runtime id (hash of pid+creation+service kind+endpoint).
    pub runtimeId: String,
    pub pid: u32,
    /// Service kind from Phase 3 (`ollama`, `llama_cpp`, `comfy_ui`, `gradio`,
    /// `open_web_ui`, `generic_ai`).
    pub serviceKind: String,
    /// Human-facing provider name (Phase 3 display name).
    pub displayName: String,
    /// Loopback base URL the adapter is allowed to probe.
    pub endpoint: String,
    pub health: AiHealth,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub capabilities: AiRuntimeCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub props: Option<AiLlamaCppProps>,
    pub models: Vec<AiModelInfo>,
    pub loadedModels: Vec<AiModelInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<AiResourceInfo>,
    /// Unix ms when this snapshot was captured.
    pub capturedAt: u64,
    /// Wall-clock probe duration in ms (diagnostic).
    pub latencyMs: u64,
    /// Structured error when health is Unavailable/Degraded (spec §46).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AiProbeError>,
    /// Concise honest label for the UI (derived from `error`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errorLabel: Option<String>,
}

// ---------------------------------------------------------------------------
// Endpoint trust policy (spec §6) — the security core.
// ---------------------------------------------------------------------------

/// Failure mode of the endpoint trust check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EndpointPolicyError {
    /// Not `http` (https to loopback is unusual and unsupported this phase).
    Scheme,
    /// Host could not be parsed.
    Unparsable,
    /// Host is not a loopback address/name.
    NotLoopback,
    /// URL carries credentials — never acceptable.
    Credentials,
}

impl EndpointPolicyError {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            EndpointPolicyError::Scheme => "only plain http to loopback is allowed",
            EndpointPolicyError::Unparsable => "host could not be parsed",
            EndpointPolicyError::NotLoopback => "only loopback endpoints are inspected",
            EndpointPolicyError::Credentials => "URLs with credentials are rejected",
        }
    }
}

/// Validate an endpoint URL against the local trust policy.
///
/// Allowed: `http://localhost`, `http://127.0.0.1`, `http://[::1]` — with an
/// explicit port. Everything else (LAN/private/public IPs, other schemes,
/// credentials) is rejected *before* any request is made.
pub(crate) fn validate_endpoint(url: &str) -> Result<(), EndpointPolicyError> {
    let parsed = reqwest::Url::parse(url).map_err(|_| EndpointPolicyError::Unparsable)?;
    if parsed.scheme() != "http" {
        return Err(EndpointPolicyError::Scheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EndpointPolicyError::Credentials);
    }
    let host = parsed.host_str().ok_or(EndpointPolicyError::Unparsable)?;
    let is_loopback = match host {
        "localhost" => true,
        _ => {
            // Strip IPv6 brackets via the typed host enum.
            match parsed.host() {
                Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
                Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
                _ => false,
            }
        }
    };
    if !is_loopback {
        return Err(EndpointPolicyError::NotLoopback);
    }
    Ok(())
}

/// Map a listener row to the loopback base URL the adapter may probe
/// (spec §6: local bind aliases derived from the listener). Wildcards map to
/// `localhost` so the client connects over loopback, never to the wildcard.
pub(crate) fn listener_base_url(ip_version: crate::discovery::ports::IpVersion, address: &str, port: u16) -> Option<String> {
    let host = match (ip_version, address) {
        (crate::discovery::ports::IpVersion::V4, "0.0.0.0") => "localhost",
        (crate::discovery::ports::IpVersion::V4, "127.0.0.1") => "127.0.0.1",
        (crate::discovery::ports::IpVersion::V6, "::") => "localhost",
        (crate::discovery::ports::IpVersion::V6, "::1") => "[::1]",
        _ => return None,
    };
    Some(format!("http://{host}:{port}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_endpoints_are_accepted() {
        assert_eq!(validate_endpoint("http://127.0.0.1:11434"), Ok(()));
        assert_eq!(validate_endpoint("http://localhost:8080"), Ok(()));
        assert_eq!(validate_endpoint("http://[::1]:7860"), Ok(()));
    }

    #[test]
    fn non_loopback_hosts_are_rejected() {
        assert_eq!(
            validate_endpoint("http://192.168.1.10:11434"),
            Err(EndpointPolicyError::NotLoopback)
        );
        assert_eq!(
            validate_endpoint("http://10.0.0.5:8080"),
            Err(EndpointPolicyError::NotLoopback)
        );
        assert_eq!(
            validate_endpoint("http://8.8.8.8:80"),
            Err(EndpointPolicyError::NotLoopback)
        );
        assert_eq!(
            validate_endpoint("http://example.com:80"),
            Err(EndpointPolicyError::NotLoopback)
        );
    }

    #[test]
    fn unexpected_scheme_and_credentials_are_rejected() {
        assert_eq!(
            validate_endpoint("https://127.0.0.1:11434"),
            Err(EndpointPolicyError::Scheme)
        );
        assert_eq!(
            validate_endpoint("http://user:pass@127.0.0.1:11434"),
            Err(EndpointPolicyError::Credentials)
        );
        assert_eq!(
            validate_endpoint("file://127.0.0.1/x"),
            Err(EndpointPolicyError::Scheme)
        );
        assert!(validate_endpoint("http://").is_err());
    }

    #[test]
    fn listener_addresses_map_to_safe_client_urls() {
        use crate::discovery::ports::IpVersion;
        assert_eq!(
            listener_base_url(IpVersion::V4, "0.0.0.0", 11434).as_deref(),
            Some("http://localhost:11434")
        );
        assert_eq!(
            listener_base_url(IpVersion::V4, "127.0.0.1", 8080).as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            listener_base_url(IpVersion::V6, "::", 3000).as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            listener_base_url(IpVersion::V6, "::1", 7860).as_deref(),
            Some("http://[::1]:7860")
        );
        // A specific non-loopback bind is not probe-safe in this phase.
        assert_eq!(listener_base_url(IpVersion::V4, "192.168.1.7", 9000), None);
    }
}
