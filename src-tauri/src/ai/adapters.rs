//! AI runtime adapters: per-provider read-only inspection.
//!
//! # Architecture (spec §3)
//!
//! Each adapter is a trait implementor: it knows the *approved endpoint
//! paths* for its provider and turns raw JSON payloads into the normalized
//! [`AiRuntimeSnapshot`]. Parsing is **pure** (JSON string in → model out)
//! and fully fixture-tested without any server; the HTTP layer is a thin,
//! shared client with hard timeouts and size bounds.
//!
//! # Provider selection (spec §25)
//!
//! The adapter is chosen from the Phase 3 `ServiceKind` — never by probing
//! URLs against unknown ports. A Gradio/Open WebUI/unknown AI service gets
//! the conservative generic adapter: HTTP reachability only.

use serde_json::Value;

use super::domain::{
    AiHealth, AiLlamaCppProps, AiModelInfo, AiProbeError, AiResourceInfo, AiRuntimeCapabilities,
    MAX_LOADED_MODELS, MAX_MODELS,
};

/// Which runtime an adapter serves (mirrors `intelligence::rules::ServiceKind`
/// for the AI subset, plus the generic fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiRuntimeKind {
    Ollama,
    LlamaCpp,
    ComfyUi,
    Gradio,
    OpenWebUi,
    Generic,
}

impl AiRuntimeKind {
    /// Map from the Phase 3 classifier's kind name.
    pub(crate) fn from_service_kind(kind: &str) -> Self {
        match kind {
            "ollama" => Self::Ollama,
            "llama_cpp" => Self::LlamaCpp,
            "comfy_ui" => Self::ComfyUi,
            "gradio" => Self::Gradio,
            "open_web_ui" => Self::OpenWebUi,
            _ => Self::Generic,
        }
    }
}

/// The outcome of one full adapter inspection.
#[derive(Debug, Clone)]
pub(crate) struct Inspection {
    pub health: AiHealth,
    pub version: Option<String>,
    pub capabilities: AiRuntimeCapabilities,
    pub props: Option<AiLlamaCppProps>,
    pub models: Vec<AiModelInfo>,
    pub loaded_models: Vec<AiModelInfo>,
    pub resources: Option<AiResourceInfo>,
    /// Structured error when the primary probe failed.
    pub error: Option<AiProbeError>,
}

impl Inspection {
    pub(crate) fn empty() -> Self {
        Self {
            health: AiHealth::Unknown,
            version: None,
            capabilities: AiRuntimeCapabilities::default(),
            props: None,
            models: Vec::new(),
            loaded_models: Vec::new(),
            resources: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

/// Ollama adapter (spec §10–14): `/api/version`, `/api/tags`, `/api/ps`.
///
/// All three are read-only. Health semantics: a successful `/api/version` or
/// `/api/tags` proves the API is serving → `Ready`; connect failure →
/// `Unavailable`; an unexpected payload on the primary endpoint → `Degraded`.
pub(crate) mod ollama {
    use super::*;

    /// Parse `GET /api/version` → version string.
    pub(crate) fn parse_version(body: &str) -> Result<Option<String>, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        Ok(value.get("version").and_then(Value::as_str).map(str::to_string))
    }

    /// Parse `GET /api/tags` → installed model inventory.
    pub(crate) fn parse_tags(body: &str) -> Result<Vec<AiModelInfo>, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        let Some(models) = value.get("models").and_then(Value::as_array) else {
            return Err(AiProbeError::MalformedJson {
                detail: "missing models array".to_string(),
            });
        };
        Ok(models
            .iter()
            .take(MAX_MODELS)
            .map(|m| AiModelInfo {
                id: m
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                displayName: None,
                family: m
                    .pointer("/details/family")
                    .or_else(|| m.get("family"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                // `size`/`parameter size`/`quantization level` live in
                // details for many versions; also accept flat fields.
                parameterSize: m
                    .pointer("/details/parameter_size")
                    .or_else(|| m.get("parameterSize"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                quantization: m
                    .pointer("/details/quantization_level")
                    .or_else(|| m.get("quantization"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sizeBytes: m
                    .get("size")
                    .and_then(Value::as_u64)
                    .or_else(|| m.get("sizeBytes").and_then(Value::as_u64)),
                modifiedAt: None,
                loaded: None,
                vramBytes: None,
                status: None,
                expiresAt: None,
            })
            .collect())
    }

    /// Parse `GET /api/ps` → currently loaded models with VRAM.
    pub(crate) fn parse_ps(body: &str) -> Result<Vec<AiModelInfo>, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        let Some(models) = value.get("models").and_then(Value::as_array) else {
            return Err(AiProbeError::MalformedJson {
                detail: "missing models array".to_string(),
            });
        };
        Ok(models
            .iter()
            .take(MAX_LOADED_MODELS)
            .map(|m| AiModelInfo {
                id: m
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                displayName: None,
                family: None,
                parameterSize: m
                    .pointer("/details/parameter_size")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                quantization: m
                    .pointer("/details/quantization_level")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                sizeBytes: m.get("size").and_then(Value::as_u64),
                modifiedAt: None,
                loaded: Some(true),
                // Reported VRAM usage only — never estimated from file size.
                vramBytes: m.get("size_vram").and_then(Value::as_u64),
                status: None,
                expiresAt: m.get("expires_at").and_then(Value::as_str).and_then(|s| {
                    // RFC3339 → epoch ms is only best-effort here; keep the
                    // raw string out of the domain by parsing with a tiny
                    // manual ISO-8601 fallback. If it fails, leave None.
                    chrono_like_parse_epoch_ms(s)
                }),
            })
            .collect())
    }

    /// Minimal RFC3339 → epoch-ms parser for `expires_at` (UTC only, no
    /// external time crate). Returns None for anything it cannot parse.
    fn chrono_like_parse_epoch_ms(s: &str) -> Option<u64> {
        // Shape: 2024-01-15T10:30:00.123456789Z (fractional part optional).
        let bytes = s.as_bytes();
        if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || (bytes[10] != b'T' && bytes[10] != b't') {
            return None;
        }
        let year: i64 = s.get(0..4)?.parse().ok()?;
        let month: i64 = s.get(5..7)?.parse().ok()?;
        let day: i64 = s.get(8..10)?.parse().ok()?;
        let hour: i64 = s.get(11..13)?.parse().ok()?;
        let minute: i64 = s.get(14..16)?.parse().ok()?;
        let second: i64 = s.get(17..19)?.parse().ok()?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        // Days since Unix epoch (proleptic Gregorian, civil algorithm).
        let days = days_from_civil(year, month, day);
        let secs = days * 86_400 + hour * 3_600 + minute * 60 + second;
        // Fractional seconds: collect up to 3 digits of ms precision.
        let mut millis: u64 = 0;
        let mut index = 19usize;
        if bytes.len() > index && bytes[index] == b'.' {
            index += 1;
            let mut digits = 0usize;
            let mut value: u64 = 0;
            while index < bytes.len() && bytes[index].is_ascii_digit() && digits < 3 {
                value = value * 10 + u64::from(bytes[index] - b'0');
                index += 1;
                digits += 1;
            }
            if digits == 0 {
                return None;
            }
            millis = match digits {
                1 => value * 100,
                2 => value * 10,
                _ => value,
            };
            // Skip remaining sub-millisecond digits (nanosecond precision —
            // below our resolution, and they precede the designator).
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        // Trailing designator: Z / z / end — non-UTC offsets are not guessed.
        let tail = &bytes[index..];
        if !(tail.is_empty() || tail == b"Z" || tail == b"z") {
            return None;
        }
        u64::try_from(secs).ok().map(|s| s * 1000 + millis)
    }

    /// Howard Hinnant's civil-from-days inverse (pure, tested below).
    fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn version_parsing() {
            assert_eq!(
                parse_version(r#"{"version":"0.5.4"}"#).expect("parse"),
                Some("0.5.4".to_string())
            );
            // Missing field → None, not an error (spec §11).
            assert_eq!(parse_version(r#"{}"#).expect("parse"), None);
            assert!(parse_version("not json").is_err());
        }

        #[test]
        fn tags_parses_models_with_optional_fields() {
            let body = r#"{"models":[
                {"name":"qwen2.5:7b-instruct-q4_K_M","size":4683087569,"details":{"parameter_size":"7.6B","quantization_level":"Q4_K_M","family":"qwen2"}},
                {"name":"tiny:1b","size":123}
            ]}"#;
            let models = parse_tags(body).expect("parse");
            assert_eq!(models.len(), 2);
            assert_eq!(models[0].id, "qwen2.5:7b-instruct-q4_K_M");
            assert_eq!(models[0].parameterSize.as_deref(), Some("7.6B"));
            assert_eq!(models[0].quantization.as_deref(), Some("Q4_K_M"));
            assert_eq!(models[0].family.as_deref(), Some("qwen2"));
            assert_eq!(models[0].sizeBytes, Some(4_683_087_569));
            // Second model has no details — optional fields stay None.
            assert_eq!(models[1].parameterSize, None);
            assert_eq!(models[1].quantization, None);
            assert_eq!(models[1].sizeBytes, Some(123));
        }

        #[test]
        fn tags_empty_list_and_malformed() {
            assert_eq!(parse_tags(r#"{"models":[]}"#).expect("parse").len(), 0);
            assert!(parse_tags(r#"{"nope":1}"#).is_err());
            assert!(parse_tags("[1,2,3]").is_err());
        }

        #[test]
        fn ps_parses_loaded_models_with_vram() {
            let body = r#"{"models":[
                {"name":"qwen2.5:7b","size":4683087569,"size_vram":4831838208,
                 "expires_at":"2024-01-15T10:30:00.500000000Z",
                 "details":{"parameter_size":"7.6B"}}
            ]}"#;
            let loaded = parse_ps(body).expect("parse");
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].loaded, Some(true));
            assert_eq!(loaded[0].vramBytes, Some(4_831_838_208));
            assert_eq!(loaded[0].expiresAt, Some(1_705_314_600_500));
        }

        #[test]
        fn expires_at_unknown_shapes_stay_none() {
            let body = r#"{"models":[{"name":"m","expires_at":"not-a-date"}]}"#;
            let loaded = parse_ps(body).expect("parse");
            assert_eq!(loaded[0].expiresAt, None);
            // Non-UTC offsets are not guessed.
            let body = r#"{"models":[{"name":"m","expires_at":"2024-01-15T10:30:00+02:00"}]}"#;
            let loaded = parse_ps(body).expect("parse");
            assert_eq!(loaded[0].expiresAt, None);
        }

        #[test]
        fn days_from_civil_matches_known_dates() {
            assert_eq!(days_from_civil(1970, 1, 1), 0);
            assert_eq!(days_from_civil(2024, 1, 15), 19_737);
        }
    }
}

// ---------------------------------------------------------------------------
// llama.cpp
// ---------------------------------------------------------------------------

/// llama.cpp adapter (spec §15–19): `/health`, `/props`, `/models`,
/// `/metrics` (optional). Read-only only; `/models` is queried in a way that
/// never triggers model autoload (plain GET, no generation params).
pub(crate) mod llamacpp {
    use super::*;

    /// Parse `GET /health`. Known shapes:
    /// - `{"status":"ok"}` → Ready
    /// - `{"status":"loading model"}` (or `error` payload with message) → Loading
    /// - `{"error":…}` → Degraded
    pub(crate) fn parse_health(body: &str) -> Result<AiHealth, AiProbeError> {
        // The endpoint may answer with a bare string ("ok" / "loading") or JSON.
        let trimmed = body.trim();
        let lower_bare = trimmed.to_ascii_lowercase();
        if lower_bare == "ok" {
            return Ok(AiHealth::Ready);
        }
        if lower_bare.contains("loading") {
            return Ok(AiHealth::Loading);
        }
        let value: Value = serde_json::from_str(trimmed)
            .map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        if let Some(status) = value.get("status").and_then(Value::as_str) {
            let lower = status.to_ascii_lowercase();
            if lower == "ok" {
                return Ok(AiHealth::Ready);
            }
            if lower.contains("loading") {
                return Ok(AiHealth::Loading);
            }
            if lower.contains("busy") || lower.contains("no slot") {
                return Ok(AiHealth::Busy);
            }
            return Ok(AiHealth::Degraded);
        }
        if value.get("error").is_some() {
            return Ok(AiHealth::Degraded);
        }
        Ok(AiHealth::Degraded)
    }

    /// Parse `GET /props` → conservative normalized subset (spec §17).
    pub(crate) fn parse_props(body: &str) -> Result<AiLlamaCppProps, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        // Shapes vary by version: model path at /model_path or /default_generation_settings/…
        let model_path = value
            .get("model_path")
            .and_then(Value::as_str)
            .map(str::to_string);
        let context = value
            .pointer("/default_generation_settings/n_ctx")
            .or_else(|| value.get("n_ctx"))
            .and_then(Value::as_u64);
        let total = value
            .pointer("/slots_total")
            .or_else(|| value.get("total_slots"))
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
        let idle = value
            .pointer("/slots_idle")
            .or_else(|| value.get("idle_slots"))
            .and_then(Value::as_u64)
            .map(|v| u32::try_from(v).unwrap_or(u32::MAX));
        Ok(AiLlamaCppProps {
            modelPath: model_path,
            contextSize: context,
            slotsTotal: total,
            slotsIdle: idle,
        })
    }

    /// Parse `GET /v1/models` (OpenAI-compatible) or `GET /models`.
    /// Returns the model ids; no autoload is triggered by a plain GET.
    pub(crate) fn parse_models(body: &str) -> Result<Vec<AiModelInfo>, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        let Some(list) = value.get("data").and_then(Value::as_array) else {
            return Err(AiProbeError::MalformedJson {
                detail: "missing data array".to_string(),
            });
        };
        Ok(list
            .iter()
            .take(MAX_MODELS)
            .map(|m| AiModelInfo {
                id: m
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                displayName: None,
                family: None,
                parameterSize: None,
                quantization: None,
                sizeBytes: None,
                modifiedAt: None,
                loaded: Some(true), // llama.cpp serves the loaded model
                vramBytes: None,
                status: Some("loaded".to_string()),
                expiresAt: None,
            })
            .collect())
    }

    /// Parse selected safe `GET /metrics` values (spec §19). Returns the
    /// slots-idle gauge when present; anything unparseable is None — metrics
    /// are optional and never a failure.
    pub(crate) fn parse_metrics_slots_idle(body: &str) -> Option<u32> {
        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("llamacpp:slot_count") {
                // Only the plain gauge without labels: `llamacpp:slot_count 3`.
                // Labeled variants (`llamacpp:slot_count{...}`) are skipped:
                // their first token after trim starts with '{'.
                let value = rest.trim().split_whitespace().next()?;
                if let Ok(v) = value.parse::<u32>() {
                    return Some(v);
                }
            }
        }
        None
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn health_ready_shapes() {
            assert_eq!(parse_health(r#"{"status":"ok"}"#).expect("parse"), AiHealth::Ready);
            assert_eq!(parse_health("ok").expect("parse"), AiHealth::Ready);
            assert_eq!(parse_health(r#"{"status":"OK"}"#).expect("parse"), AiHealth::Ready);
        }

        #[test]
        fn health_loading_is_distinguished_from_ready() {
            assert_eq!(
                parse_health(r#"{"status":"loading model"}"#).expect("parse"),
                AiHealth::Loading
            );
            assert_eq!(parse_health("loading model").expect("parse"), AiHealth::Loading);
        }

        #[test]
        fn health_busy_and_error_and_unexpected() {
            assert_eq!(parse_health(r#"{"status":"no slot free"}"#).expect("parse"), AiHealth::Busy);
            assert_eq!(parse_health(r#"{"error":"model failed"}"#).expect("parse"), AiHealth::Degraded);
            assert_eq!(parse_health(r#"{"weird":1}"#).expect("parse"), AiHealth::Degraded);
            assert!(parse_health("<html>").is_err());
        }

        #[test]
        fn props_parses_known_fields() {
            let body = r#"{"model_path":"D:\\models\\qwen2.5-7b-q4.gguf","default_generation_settings":{"n_ctx":8192},"slots_total":4,"slots_idle":3}"#;
            let props = parse_props(body).expect("parse");
            assert_eq!(props.modelPath.as_deref(), Some("D:\\models\\qwen2.5-7b-q4.gguf"));
            assert_eq!(props.contextSize, Some(8192));
            assert_eq!(props.slotsTotal, Some(4));
            assert_eq!(props.slotsIdle, Some(3));
        }

        #[test]
        fn props_missing_fields_stay_none() {
            let props = parse_props(r#"{}"#).expect("parse");
            assert_eq!(props.contextSize, None);
            assert_eq!(props.slotsTotal, None);
        }

        #[test]
        fn models_parses_v1_shape() {
            let body = r#"{"object":"list","data":[{"id":"qwen2.5-7b-q4_K_M","object":"model"}]}"#;
            let models = parse_models(body).expect("parse");
            assert_eq!(models.len(), 1);
            assert_eq!(models[0].id, "qwen2.5-7b-q4_K_M");
            assert_eq!(models[0].loaded, Some(true));
        }

        #[test]
        fn metrics_optional_gauge() {
            assert_eq!(parse_metrics_slots_idle("llamacpp:slot_count 3\n"), Some(3));
            assert_eq!(parse_metrics_slots_idle("# HELP x\nnothing here\n"), None);
        }
    }
}

// ---------------------------------------------------------------------------
// ComfyUI
// ---------------------------------------------------------------------------

/// ComfyUI adapter (spec §20–21): read-only `/system_stats` and `/queue`.
/// Never submits, cancels, or clears anything; workflow contents are not
/// copied into LocalStack (only counts).
pub(crate) mod comfyui {
    use super::*;

    /// Parse `GET /system_stats` → device + VRAM facts.
    pub(crate) fn parse_system_stats(body: &str) -> Result<AiResourceInfo, AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        let mut info = AiResourceInfo::default();
        if let Some(devices) = value.get("devices").and_then(Value::as_array) {
            if let Some(device) = devices.first() {
                info.device = device
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                info.vramTotalBytes = device
                    .pointer("/vram_total")
                    .and_then(Value::as_u64);
                info.vramFreeBytes = device
                    .pointer("/vram_free")
                    .and_then(Value::as_u64);
            }
        }
        Ok(info)
    }

    /// Parse `GET /queue` → running/pending counts only (no workflow data).
    pub(crate) fn parse_queue(body: &str) -> Result<(u32, u32), AiProbeError> {
        let value: Value =
            serde_json::from_str(body).map_err(|e| AiProbeError::MalformedJson { detail: e.to_string() })?;
        let count = |key: &str| -> u32 {
            value
                .get(key)
                .and_then(Value::as_array)
                .map(|a| u32::try_from(a.len()).unwrap_or(u32::MAX))
                .unwrap_or(0)
        };
        Ok((count("queue_running"), count("queue_pending")))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn system_stats_parses_device_and_vram() {
            let body = r#"{"devices":[{"name":"NVIDIA GeForce RTX 4070","vram_total":12884901888,"vram_free":10737418240}]}"#;
            let info = parse_system_stats(body).expect("parse");
            assert_eq!(info.device.as_deref(), Some("NVIDIA GeForce RTX 4070"));
            assert_eq!(info.vramTotalBytes, Some(12_884_901_888));
            assert_eq!(info.vramFreeBytes, Some(10_737_418_240));
        }

        #[test]
        fn system_stats_missing_fields_stay_none() {
            let info = parse_system_stats(r#"{"devices":[]}"#).expect("parse");
            assert_eq!(info.device, None);
            let info = parse_system_stats(r#"{}"#).expect("parse");
            assert_eq!(info.device, None);
            assert!(parse_system_stats("<html>").is_err());
        }

        #[test]
        fn queue_counts_only() {
            let body = r#"{"queue_running":[{}],"queue_pending":[{},{},{}]}"#;
            assert_eq!(parse_queue(body).expect("parse"), (1, 3));
            assert_eq!(parse_queue(r#"{"queue_running":[],"queue_pending":[]}"#).expect("parse"), (0, 0));
            assert_eq!(parse_queue(r#"{}"#).expect("parse"), (0, 0));
            assert!(parse_queue("nope").is_err());
        }
    }
}

// ---------------------------------------------------------------------------
// Generic / conservative adapters
// ---------------------------------------------------------------------------

/// Gradio / Open WebUI / unknown AI web services (spec §22–24): conservative
/// HTTP reachability only. No page scraping, no auth, no account data.
pub(crate) mod generic {
    use super::*;

    /// The generic adapter's inspection outcome from a successful HTTP GET.
    pub(crate) fn inspection_from_reachable(kind: AiRuntimeKind) -> Inspection {
        let mut inspection = Inspection::empty();
        inspection.health = AiHealth::Ready;
        inspection.capabilities = AiRuntimeCapabilities {
            health: true,
            ..AiRuntimeCapabilities::default()
        };
        let _ = kind;
        inspection
    }
}

#[cfg(test)]
mod adapter_selection_tests {
    use super::*;

    #[test]
    fn service_kind_maps_to_runtime_kind() {
        assert_eq!(AiRuntimeKind::from_service_kind("ollama"), AiRuntimeKind::Ollama);
        assert_eq!(AiRuntimeKind::from_service_kind("llama_cpp"), AiRuntimeKind::LlamaCpp);
        assert_eq!(AiRuntimeKind::from_service_kind("comfy_ui"), AiRuntimeKind::ComfyUi);
        assert_eq!(AiRuntimeKind::from_service_kind("gradio"), AiRuntimeKind::Gradio);
        assert_eq!(AiRuntimeKind::from_service_kind("open_web_ui"), AiRuntimeKind::OpenWebUi);
        // Anything else — including non-AI kinds — degrades to generic.
        assert_eq!(AiRuntimeKind::from_service_kind("node_js"), AiRuntimeKind::Generic);
        assert_eq!(AiRuntimeKind::from_service_kind("unknown"), AiRuntimeKind::Generic);
    }
}
