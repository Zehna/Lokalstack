//! Docker Engine API JSON → normalized models (spec §5–23).
//!
//! Pure parsing over fixture payloads — every state/health/port/compose
//! combination is unit-tested without a daemon (spec §53–55). Only the
//! canonical Compose labels are extracted (spec §48); `Config.Env` is never
//! even *read* into an intermediate structure (spec §47, §58).

use serde_json::Value;

use super::domain::{
    self, ContainerHealth, ContainerPort, ContainerState, DockerComposeIdentity, DockerContainer,
    ContainerStats, MAX_CONTAINERS, MAX_MOUNTS_PER_CONTAINER, MAX_NETWORKS_PER_CONTAINER,
    MAX_PORTS_PER_CONTAINER,
};

/// Parse `GET /containers/json?all=true` into normalized containers.
/// This list endpoint carries ports/state/labels — enough for the UI without
/// per-container inspect calls (spec §8).
pub(crate) fn parse_containers_list(body: &str) -> Result<Vec<DockerContainer>, String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("containers/json parse: {e}"))?;
    let Some(list) = value.as_array() else {
        return Err("containers/json: expected array".to_string());
    };
    let mut containers = Vec::with_capacity(list.len().min(MAX_CONTAINERS));
    for raw in list.iter().take(MAX_CONTAINERS) {
        containers.push(container_from_list_entry(raw));
    }
    Ok(containers)
}

fn container_from_list_entry(raw: &Value) -> DockerContainer {
    let id = raw
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    // Names come as ["/name"]; take the first, strip the leading slash.
    let name = raw
        .get("Names")
        .and_then(Value::as_array)
        .and_then(|names| names.first())
        .and_then(Value::as_str)
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_default();
    let image = raw
        .get("Image")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let image_id = raw
        .get("ImageID")
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = raw
        .get("State")
        .and_then(Value::as_str)
        .map(domain::parse_container_state)
        .unwrap_or(ContainerState::Unknown);
    let status = raw
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let created = raw
        .get("Created")
        .and_then(Value::as_i64)
        .and_then(|s| u64::try_from(s).ok())
        .map(|secs| secs.saturating_mul(1000));
    let ports = parse_port_list(raw.get("Ports"));
    let labels = raw.get("Labels").and_then(Value::as_object);
    let compose = labels.map(compose_from_labels);
    // Health from the list payload's Status string is ambiguous; the
    // authoritative source is inspect. The list gives `Health` only in some
    // versions — treat absence as None-configured only when inspect
    // confirms; here default to None and let enrich_health correct it.
    let health = health_from_status_string(&status);
    let _ = MAX_PORTS_PER_CONTAINER;

    DockerContainer {
        shortId: id.chars().take(12).collect(),
        id,
        name,
        image,
        imageId: image_id,
        imageDigest: None,
        state,
        status,
        health,
        createdAt: created,
        ports,
        compose,
        networks: Vec::new(),
        mounts: Vec::new(),
        stats: None,
    }
}

/// Best-effort health read from the list `Status` string (e.g.
/// "Up 2 hours (healthy)"). Authoritative values come from inspect.
fn health_from_status_string(status: &str) -> ContainerHealth {
    let lower = status.to_ascii_lowercase();
    if lower.contains("(healthy)") {
        ContainerHealth::Healthy
    } else if lower.contains("(unhealthy)") {
        ContainerHealth::Unhealthy
    } else if lower.contains("(health: starting)") || lower.contains("(starting)") {
        ContainerHealth::Starting
    } else {
        ContainerHealth::None
    }
}

/// Parse the `Ports` array of the list payload (spec §11, §14, §54).
fn parse_port_list(raw: Option<&Value>) -> Vec<ContainerPort> {
    let Some(entries) = raw.and_then(Value::as_array) else {
        return Vec::new();
    };
    entries
        .iter()
        .take(MAX_PORTS_PER_CONTAINER)
        .map(|p| ContainerPort {
            protocol: p
                .get("Type")
                .and_then(Value::as_str)
                .unwrap_or("tcp")
                .to_string(),
            containerPort: p.get("PrivatePort").and_then(Value::as_u64).unwrap_or(0) as u16,
            hostIp: p
                .get("IP")
                .and_then(Value::as_str)
                .map(str::to_string),
            hostPort: p.get("PublicPort").and_then(Value::as_u64).map(|v| v as u16),
            published: p.get("PublicPort").is_some(),
        })
        .collect()
}

/// Compose identity from canonical labels only (spec §15–16, §48). No other
/// label crosses into the model.
fn compose_from_labels(labels: &serde_json::Map<String, Value>) -> DockerComposeIdentity {
    let get = |key: &str| {
        labels
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    DockerComposeIdentity {
        projectName: get("com.docker.compose.project"),
        serviceName: get("com.docker.compose.service"),
        containerNumber: get("com.docker.compose.container-number"),
        workingDir: get("com.docker.compose.project.working_dir"),
        configFiles: get("com.docker.compose.project.config_files"),
    }
}

/// Structured facts extracted from one inspect payload — the cacheable
/// subset the facade stores per container (spec §28).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct InspectFacts {
    pub state: Option<domain::ContainerState>,
    pub health: ContainerHealth,
    pub networks: Vec<domain::ContainerNetwork>,
    pub mounts: Vec<(String, String)>,
}

/// Parse an inspect payload into the cacheable facts.
pub(crate) fn parse_inspect(body: &str) -> InspectFacts {
    let mut facts = InspectFacts {
        state: None,
        health: ContainerHealth::None,
        networks: Vec::new(),
        mounts: Vec::new(),
    };
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return facts;
    };
    if let Some(state) = value.get("State").and_then(Value::as_object) {
        if let Some(status) = state.get("Status").and_then(Value::as_str) {
            facts.state = Some(domain::parse_container_state(status));
        }
        let has_health = state.contains_key("Health");
        let health_status = state
            .get("Health")
            .and_then(|h| h.get("Status"))
            .and_then(Value::as_str);
        facts.health = domain::parse_container_health(health_status, has_health);
    }
    if let Some(net) = value
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for (name, details) in net.iter().take(MAX_NETWORKS_PER_CONTAINER) {
            facts.networks.push(domain::ContainerNetwork {
                name: name.clone(),
                ipAddress: details.get("IPAddress").and_then(Value::as_str).map(str::to_string),
                gateway: details.get("Gateway").and_then(Value::as_str).map(str::to_string),
            });
        }
    }
    if let Some(list) = value.get("Mounts").and_then(Value::as_array) {
        for mount in list.iter().take(MAX_MOUNTS_PER_CONTAINER) {
            if mount.get("Type").and_then(Value::as_str) != Some("bind") {
                continue;
            }
            let Some(source) = mount.get("Source").and_then(Value::as_str) else {
                continue;
            };
            facts.mounts.push((
                source.to_string(),
                mount.get("Destination").and_then(Value::as_str).unwrap_or("").to_string(),
            ));
        }
    }
    facts
}

/// Apply cached inspect facts to a container.
pub(crate) fn apply_inspect_facts(container: &mut DockerContainer, facts: &InspectFacts) {
    if let Some(state) = facts.state {
        container.state = state;
    }
    container.health = facts.health;
    container.networks = facts.networks.clone();
    container.mounts = facts.mounts.clone();
}

/// Engine `/version` → useful identity fields only (spec §49).
pub(crate) fn parse_version(body: &str) -> domain::DockerEngineInfo {
    let mut info = domain::DockerEngineInfo::default();
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return info;
    };
    info.version = value.get("Version").and_then(Value::as_str).map(str::to_string);
    info.apiVersion = value.get("ApiVersion").and_then(Value::as_str).map(str::to_string);
    info.os = value.get("Os").and_then(Value::as_str).map(str::to_string);
    info.arch = value.get("Arch").and_then(Value::as_str).map(str::to_string);
    info
}

/// Enrich a list-derived container with `GET /containers/{id}/json` facts:
/// authoritative health, networks, mounts (spec §8, §19, §23).
pub(crate) fn enrich_from_inspect(container: &mut DockerContainer, body: &str) -> Result<(), String> {
    let value: Value =
        serde_json::from_str(body).map_err(|e| format!("inspect parse: {e}"))?;

    // Authoritative state + health (spec §7).
    if let Some(state) = value.get("State").and_then(Value::as_object) {
        if let Some(status) = state.get("Status").and_then(Value::as_str) {
            container.state = domain::parse_container_state(status);
        }
        let has_health = state.contains_key("Health");
        let health_status = state
            .get("Health")
            .and_then(|h| h.get("Status"))
            .and_then(Value::as_str);
        container.health = domain::parse_container_health(health_status, has_health);
    }

    // Image digest when the daemon already has one (spec §22) — the list
    // endpoint does not carry digests and we do not make registry calls to
    // resolve them, so this stays as parsed (None unless a future endpoint
    // that already returns digests is wired in).
    let _ = &container.imageDigest;

    // Networks (spec §23).
    let mut networks = Vec::new();
    if let Some(net) = value
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for (name, details) in net.iter().take(MAX_NETWORKS_PER_CONTAINER) {
            networks.push(super::domain::ContainerNetwork {
                name: name.clone(),
                ipAddress: details
                    .get("IPAddress")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                gateway: details
                    .get("Gateway")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            });
        }
    }
    container.networks = networks;

    // Mounts as project evidence + details metadata (spec §19–20). Only
    // bind-type mounts carry a host path worth reading; contents are never
    // opened.
    let mut mounts = Vec::new();
    if let Some(list) = value.get("Mounts").and_then(Value::as_array) {
        for mount in list.iter().take(MAX_MOUNTS_PER_CONTAINER) {
            let kind = mount.get("Type").and_then(Value::as_str).unwrap_or("");
            if kind != "bind" {
                continue;
            }
            let Some(source) = mount.get("Source").and_then(Value::as_str) else {
                continue;
            };
            let destination = mount
                .get("Destination")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            mounts.push((source.to_string(), destination));
        }
    }
    container.mounts = mounts;

    Ok(())
}

/// Parse `GET /containers/{id}/stats?stream=false` (spec §25).
pub(crate) fn parse_stats(body: &str) -> Result<ContainerStats, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("stats parse: {e}"))?;
    let mut stats = ContainerStats::default();

    // CPU: (cpu_delta / system_delta) × online_cpus × 100.
    let cpu_delta = value
        .pointer("/cpu_stats/cpu_usage/total_usage")
        .and_then(Value::as_u64)
        .and_then(|total| {
            value
                .pointer("/precpu_stats/cpu_usage/total_usage")
                .and_then(Value::as_u64)
                .map(|prev| total.saturating_sub(prev))
        });
    let system_delta = value
        .pointer("/cpu_stats/system_cpu_usage")
        .and_then(Value::as_u64)
        .and_then(|system| {
            value
                .pointer("/precpu_stats/system_cpu_usage")
                .and_then(Value::as_u64)
                .map(|prev| system.saturating_sub(prev))
        });
    let online_cpus = value
        .pointer("/cpu_stats/online_cpus")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    if let (Some(cpu), Some(system)) = (cpu_delta, system_delta) {
        if system > 0 {
            stats.cpuPercent =
                Some((cpu as f64 / system as f64) * online_cpus as f64 * 100.0);
        }
    }

    // Memory.
    let used = value
        .pointer("/memory_stats/usage")
        .and_then(Value::as_u64)
        .or_else(|| value.pointer("/memory_stats/stats/rss").and_then(Value::as_u64));
    let limit = value
        .pointer("/memory_stats/limit")
        .and_then(Value::as_u64);
    stats.memoryUsedBytes = used;
    stats.memoryLimitBytes = limit;

    // Network totals (first interface only is fine; summed).
    let mut rx = 0u64;
    let mut tx = 0u64;
    if let Some(networks) = value
        .pointer("/networks")
        .and_then(Value::as_object)
    {
        for details in networks.values() {
            rx += details.get("rx_bytes").and_then(Value::as_u64).unwrap_or(0);
            tx += details.get("tx_bytes").and_then(Value::as_u64).unwrap_or(0);
        }
        stats.networkRxBytes = Some(rx);
        stats.networkTxBytes = Some(tx);
    }

    Ok(stats)
}

// ---------------------------------------------------------------------------
// Tests (fixtures — no daemon, spec §53–55)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const RUNNING: &str = r#"[
        {
            "Id":"a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            "Names":["/historyai-frontend-1"],
            "Image":"historyai/frontend:dev",
            "ImageID":"sha256:aaa111",
            "State":"running",
            "Status":"Up 3 hours (healthy)",
            "Created":1700000000,
            "Ports":[
                {"IP":"0.0.0.0","PrivatePort":3000,"PublicPort":3000,"Type":"tcp"},
                {"PrivatePort":9229,"Type":"tcp"}
            ],
            "Labels":{
                "com.docker.compose.project":"historyai",
                "com.docker.compose.service":"frontend",
                "com.docker.compose.container-number":"1",
                "com.docker.compose.project.working_dir":"D:/Projects/HistoryAI",
                "my-custom-secret-label":"do-not-expose"
            }
        },
        {
            "Id":"b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3",
            "Names":["/plain-postgres"],
            "Image":"postgres:16",
            "State":"exited",
            "Status":"Exited (0) 5 minutes ago",
            "Created":1700000001,
            "Ports":[
                {"IP":"127.0.0.1","PrivatePort":5432,"PublicPort":55432,"Type":"tcp"},
                {"PrivatePort":53,"PublicPort":1053,"Type":"udp"}
            ]
        }
    ]"#;

    #[test]
    fn running_container_parses_fully() {
        let containers = parse_containers_list(RUNNING).expect("parse");
        assert_eq!(containers.len(), 2);
        let c = &containers[0];
        assert_eq!(c.shortId, "a1b2c3d4e5f6");
        assert_eq!(c.name, "historyai-frontend-1");
        assert_eq!(c.image, "historyai/frontend:dev");
        assert_eq!(c.imageId.as_deref(), Some("sha256:aaa111"));
        assert_eq!(c.state, ContainerState::Running);
        assert_eq!(c.health, ContainerHealth::Healthy);
        assert_eq!(c.createdAt, Some(1_700_000_000_000));
        // Ports: container/host distinct, published flag honest (spec §11).
        assert_eq!(c.ports.len(), 2);
        assert_eq!(c.ports[0].containerPort, 3000);
        assert_eq!(c.ports[0].hostPort, Some(3000));
        assert_eq!(c.ports[0].hostIp.as_deref(), Some("0.0.0.0"));
        assert!(c.ports[0].published);
        // Exposed-but-unpublished stays unpublished.
        assert!(!c.ports[1].published);
        assert_eq!(c.ports[1].hostPort, None);
        // Compose identity from canonical labels.
        let compose = c.compose.as_ref().expect("compose");
        assert_eq!(compose.projectName.as_deref(), Some("historyai"));
        assert_eq!(compose.serviceName.as_deref(), Some("frontend"));
        assert_eq!(compose.workingDir.as_deref(), Some("D:/Projects/HistoryAI"));
    }

    #[test]
    fn stopped_container_and_udp_metadata() {
        let containers = parse_containers_list(RUNNING).expect("parse");
        let c = &containers[1];
        assert_eq!(c.state, ContainerState::Exited);
        // Specific host address preserved (spec §54).
        assert_eq!(c.ports[0].hostIp.as_deref(), Some("127.0.0.1"));
        assert_eq!(c.ports[0].hostPort, Some(55432));
        assert_eq!(c.ports[0].containerPort, 5432);
        // UDP is Docker metadata only — the TCP scanner is not pretended to
        // cover it (spec §14).
        assert_eq!(c.ports[1].protocol, "udp");
        assert_eq!(c.ports[1].hostPort, Some(1053));
        // No compose labels → None identity (spec §55).
        assert!(c.compose.is_none());
    }

    #[test]
    fn similar_name_without_labels_is_not_compose() {
        let body = r#"[{"Id":"c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4",
            "Names":["/historyai-frontend-1"],"Image":"x:1","State":"running","Status":"Up"}]"#;
        let containers = parse_containers_list(body).expect("parse");
        assert!(containers[0].compose.is_none(), "name similarity is not evidence (spec §55)");
    }

    #[test]
    fn missing_optional_fields_stay_absent() {
        let body = r#"[{"Id":"d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5"}]"#;
        let containers = parse_containers_list(body).expect("parse");
        let c = &containers[0];
        assert_eq!(c.name, "");
        assert_eq!(c.image, "");
        assert_eq!(c.state, ContainerState::Unknown);
        assert_eq!(c.health, ContainerHealth::None);
        assert!(c.ports.is_empty());
        assert!(c.compose.is_none());
        assert_eq!(c.createdAt, None);
    }

    #[test]
    fn malformed_and_empty_lists() {
        assert!(parse_containers_list("{not json").is_err());
        assert!(parse_containers_list("{\"nope\":1}").is_err());
        assert_eq!(parse_containers_list("[]").expect("empty").len(), 0);
    }

    #[test]
    fn inspect_enriches_health_networks_mounts() {
        let mut container = DockerContainer {
            id: "x".repeat(64),
            shortId: "xxxxxxxxxxxx".to_string(),
            name: "db".to_string(),
            image: "postgres:16".to_string(),
            imageId: None,
            imageDigest: None,
            state: ContainerState::Running,
            status: "Up".to_string(),
            health: ContainerHealth::None,
            createdAt: None,
            ports: Vec::new(),
            compose: None,
            networks: Vec::new(),
            mounts: Vec::new(),
            stats: None,
        };
        let inspect = r#"{
            "State": {"Status": "running", "Health": {"Status": "unhealthy", "FailingStreak": 3}},
            "NetworkSettings": {"Networks": {
                "bridge": {"IPAddress": "172.17.0.2", "Gateway": "172.17.0.1"}
            }},
            "Mounts": [
                {"Type": "bind", "Source": "D:\\Projects\\HistoryAI", "Destination": "/app"},
                {"Type": "volume", "Source": "pgdata", "Destination": "/var/lib/postgresql/data"}
            ]
        }"#;
        enrich_from_inspect(&mut container, inspect).expect("enrich");
        // Health from evidence, NOT from running (spec §7).
        assert_eq!(container.health, ContainerHealth::Unhealthy);
        assert_eq!(container.state, ContainerState::Running);
        assert_eq!(container.networks.len(), 1);
        assert_eq!(container.networks[0].name, "bridge");
        assert_eq!(container.networks[0].ipAddress.as_deref(), Some("172.17.0.2"));
        // Only bind mounts surface; volume mounts skipped.
        assert_eq!(container.mounts.len(), 1);
        assert_eq!(container.mounts[0].0, "D:\\Projects\\HistoryAI");
    }

    #[test]
    fn inspect_no_healthcheck_means_none() {
        let mut container = DockerContainer {
            id: "y".repeat(64),
            shortId: "yyyyyyyyyyyy".to_string(),
            name: "app".to_string(),
            image: "app:1".to_string(),
            imageId: None,
            imageDigest: None,
            state: ContainerState::Running,
            status: "Up".to_string(),
            health: ContainerHealth::Unknown,
            createdAt: None,
            ports: Vec::new(),
            compose: None,
            networks: Vec::new(),
            mounts: Vec::new(),
            stats: None,
        };
        let inspect = r#"{"State": {"Status": "running"}}"#;
        enrich_from_inspect(&mut container, inspect).expect("enrich");
        assert_eq!(container.health, ContainerHealth::None, "no Health key → no healthcheck");
        assert_eq!(container.state, ContainerState::Running);
    }

    #[test]
    fn stats_parses_cpu_memory_network() {
        let body = r#"{
            "cpu_stats": {"cpu_usage": {"total_usage": 200}, "system_cpu_usage": 10000, "online_cpus": 4},
            "precpu_stats": {"cpu_usage": {"total_usage": 100}, "system_cpu_usage": 9000},
            "memory_stats": {"usage": 214748364, "limit": 4294967296},
            "networks": {"eth0": {"rx_bytes": 1000, "tx_bytes": 2000}}
        }"#;
        let stats = parse_stats(body).expect("parse");
        assert_eq!(stats.cpuPercent, Some(40.0)); // 100/1000 × 4 × 100
        assert_eq!(stats.memoryUsedBytes, Some(214_748_364));
        assert_eq!(stats.memoryLimitBytes, Some(4_294_967_296));
        assert_eq!(stats.networkRxBytes, Some(1000));
        assert_eq!(stats.networkTxBytes, Some(2000));
    }

    #[test]
    fn stats_first_sample_and_missing_networks() {
        // precpu zeroed (first sample) → no CPU% fabricated.
        let body = r#"{
            "cpu_stats": {"cpu_usage": {"total_usage": 200}, "system_cpu_usage": 10000, "online_cpus": 4},
            "precpu_stats": {"cpu_usage": {"total_usage": 200}, "system_cpu_usage": 10000},
            "memory_stats": {"usage": 5}
        }"#;
        let stats = parse_stats(body).expect("parse");
        assert_eq!(stats.cpuPercent, None, "zero system delta must not divide");
        assert_eq!(stats.networkRxBytes, None);
    }

    #[test]
    fn env_and_secret_labels_never_parsed() {
        // Even if a payload contained Env and secret labels, the parse
        // functions have no code path that reads them (spec §47–48, §58).
        let body = r#"[{
            "Id": "e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6",
            "Config": {"Env": ["API_KEY=super-secret", "DB_PASSWORD=hunter2"]},
            "Labels": {"custom": "metadata", "com.docker.compose.project": "p"}
        }]"#;
        let containers = parse_containers_list(body).expect("parse");
        let serialized = serde_json::to_string(&containers[0]).expect("serialize");
        assert!(!serialized.contains("super-secret"), "{serialized}");
        assert!(!serialized.contains("hunter2"), "{serialized}");
        assert!(!serialized.contains("API_KEY"), "{serialized}");
        assert!(!serialized.contains("\"custom\""), "arbitrary labels excluded: {serialized}");
        // Only the canonical compose project label survived.
        assert!(containers[0].compose.is_some());
    }
}
