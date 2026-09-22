//! Phase 11C Task 7 — typed bundle model + logical budget (spec §8, §14).
//!
//! The bundle model is the structural privacy substrate: exports transform
//! THIS typed model, never free text alone. Budget enforcement is exact and
//! never silent — truncation state is recorded in the manifest.

#![allow(dead_code)] // Consumed by capture/recovery/export tasks (Tasks 8-13).
use std::collections::BTreeMap;

/// Bundle schema version (bumped only on breaking shape changes).
pub(crate) const BUNDLE_SCHEMA_VERSION: u32 = 1;
/// Exact logical bundle budget: 50 MiB.
pub(crate) const BUNDLE_LOGICAL_BUDGET_BYTES: u64 = 52_428_800;

/// Logical bundle sections (spec §8). Entry names are fixed and derived from
/// the section name only — never from bundle content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) enum SectionName {
    Manifest,
    Summary,
    Health,
    Incidents,
    System,
    Inventory,
    Projects,
    Workspaces,
    Docker,
    AiRuntimes,
    DiagnosticsLog,
    ManagedOutput,
    CrashEmergency,
}

impl SectionName {
    /// Fixed entry name (content-free by construction).
    pub(crate) fn entry_name(&self) -> &'static str {
        match self {
            SectionName::Manifest => "manifest.json",
            SectionName::Summary => "summary.txt",
            SectionName::Health => "health.json",
            SectionName::Incidents => "incidents.json",
            SectionName::System => "system.json",
            SectionName::Inventory => "inventory.json",
            SectionName::Projects => "projects.json",
            SectionName::Workspaces => "workspaces.json",
            SectionName::Docker => "docker.json",
            SectionName::AiRuntimes => "ai-runtimes.json",
            SectionName::DiagnosticsLog => "diagnostics.log",
            SectionName::ManagedOutput => "managed-output",
            SectionName::CrashEmergency => "crash/emergency.json",
        }
    }

    /// Retention/trim priority rank (0 = core, never dropped; higher = trim
    /// first). Managed output trims before losing core diagnostic evidence.
    fn trim_rank(&self) -> u8 {
        match self {
            SectionName::Manifest | SectionName::Incidents => 0,
            SectionName::Summary => 1,
            SectionName::CrashEmergency => 2,
            SectionName::Health => 3,
            SectionName::DiagnosticsLog => 4,
            SectionName::System => 5,
            SectionName::Inventory => 6,
            SectionName::Projects => 7,
            SectionName::Workspaces => 8,
            SectionName::Docker => 9,
            SectionName::AiRuntimes => 10,
            SectionName::ManagedOutput => 11,
        }
    }

    fn json_key(&self) -> &'static str {
        self.entry_name()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct ManagedServiceOutput {
    pub(crate) service_id: String,
    pub(crate) lines: Vec<String>,
    pub(crate) truncated_lines: bool,
    pub(crate) bytes: u64,
    pub(crate) truncated_bytes: bool,
}

#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) enum SectionContent {
    Json(serde_json::Value),
    Text(String),
    ManagedOutput(Vec<ManagedServiceOutput>),
}

#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BundleSection {
    pub(crate) name: SectionName,
    pub(crate) content: SectionContent,
}

/// Typed collector failure. Kept in the manifest; detail is redacted at
/// collection time. Partial bundles are valid — only packaging/encryption/
/// durable-write failures fail final persistence (spec §8).
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct TypedCollectionError {
    pub(crate) section: SectionName,
    pub(crate) code: String,
    pub(crate) detail_redacted: String,
}

/// Never-silent truncation record (spec §14).
#[derive(Debug, Clone, Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct TruncationState {
    pub(crate) truncated: bool,
    pub(crate) truncated_sections: Vec<String>,
}

#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BundleManifest {
    pub(crate) schema_version: u32,
    pub(crate) bundle_id: String,
    pub(crate) app_version: String,
    pub(crate) created_at_ms: u64,
    pub(crate) trigger: String,
    pub(crate) subsystem: String,
    pub(crate) severity: String,
    pub(crate) fingerprint: String,
    pub(crate) collection_errors: Vec<TypedCollectionError>,
    pub(crate) truncation: TruncationState,
    pub(crate) section_sizes: BTreeMap<String, u64>,
    pub(crate) redaction_applied: bool,
    pub(crate) privacy_profile_note: String,
    pub(crate) blake3_digest_note: String,
}

/// The typed bundle: manifest + sections. Exports transform this model
/// structurally (redact_export) before serialization.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct BundleModel {
    pub(crate) manifest: BundleManifest,
    pub(crate) sections: Vec<BundleSection>,
}

/// Opaque 24-hex bundle ID from the OS RNG (BCryptGenRandom). Not derived
/// from content — no project/path/error text ever becomes the ID.
pub(crate) fn new_bundle_id() -> Option<String> {
    super::ids::random_hex(12)
}

fn section_size(content: &SectionContent) -> u64 {
    match content {
        SectionContent::Json(v) => serde_json::to_vec(v).map(|b| b.len() as u64).unwrap_or(0),
        SectionContent::Text(t) => t.len() as u64,
        SectionContent::ManagedOutput(services) => services
            .iter()
            .map(|s| {
                s.service_id.len() as u64
                    + s.lines.iter().map(|l| l.len() as u64 + 1).sum::<u64>()
            })
            .sum(),
    }
}

/// Enforce the exact 50 MiB logical budget. Deterministic: measure every
/// section, then trim/drop in reverse priority (managed output first, core
/// manifest+incidents never). Every trim is recorded in the manifest —
/// truncation is NEVER silent (spec §14).
pub(crate) fn apply_budget(mut model: BundleModel) -> BundleModel {
    // Measure.
    let mut sizes: BTreeMap<String, u64> = BTreeMap::new();
    for section in &model.sections {
        sizes.insert(
            section.name.json_key().to_string(),
            section_size(&section.content),
        );
    }
    model.manifest.section_sizes = sizes;

    let total = |sizes: &BTreeMap<String, u64>| sizes.values().sum::<u64>();
    let mut truncated_sections: Vec<String> = Vec::new();

    while total(&model.manifest.section_sizes) > BUNDLE_LOGICAL_BUDGET_BYTES {
        // Lowest-priority present section (highest trim_rank).
        let Some(victim_name) = model
            .sections
            .iter()
            .map(|s| (s.name.trim_rank(), s.name))
            .filter(|(rank, _)| *rank > 0)
            .max_by_key(|(rank, _)| *rank)
            .map(|(_, name)| name)
        else {
            // Only core sections remain and still over budget: never silent.
            model.manifest.truncation.truncated = true;
            truncated_sections.push("core-overflow".to_string());
            break;
        };

        if victim_name == SectionName::ManagedOutput {
            // Trim managed output first: halve lines per service, then drop
            // the whole section if still over budget.
            for section in model.sections.iter_mut() {
                if section.name == SectionName::ManagedOutput {
                    if let SectionContent::ManagedOutput(services) = &mut section.content {
                        for svc in services.iter_mut() {
                            let keep = svc.lines.len() / 2;
                            let removed = svc.lines.len() - keep;
                            if removed > 0 {
                                svc.lines.drain(0..removed); // keep newest tail
                                svc.truncated_lines = true;
                            }
                        }
                    }
                    let key = SectionName::ManagedOutput.json_key().to_string();
                    let measured = section_size(&section.content);
                    model.manifest.section_sizes.insert(key.clone(), measured);
                    if !truncated_sections.contains(&key) {
                        truncated_sections.push(key);
                    }
                }
            }
            if total(&model.manifest.section_sizes) > BUNDLE_LOGICAL_BUDGET_BYTES {
                drop_section(&mut model, victim_name, &mut truncated_sections);
            }
        } else {
            drop_section(&mut model, victim_name, &mut truncated_sections);
        }
    }

    if !truncated_sections.is_empty() {
        model.manifest.truncation.truncated = true;
        model.manifest.truncation.truncated_sections = truncated_sections;
    }
    model
}

fn drop_section(
    model: &mut BundleModel,
    name: SectionName,
    truncated_sections: &mut Vec<String>,
) {
    model.sections.retain(|s| s.name != name);
    model.manifest.section_sizes.remove(name.json_key());
    let key = name.json_key().to_string();
    if !truncated_sections.contains(&key) {
        truncated_sections.push(key);
    }
}

pub(crate) fn entry_name(name: &SectionName) -> &'static str {
    name.entry_name()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oversize_model() -> BundleModel {
        let mut model = minimal_model();
        // A single ~51 MiB managed-output section: far over the 50 MiB budget.
        let giant_line = "x".repeat(1024);
        model.sections.push(BundleSection {
            name: SectionName::ManagedOutput,
            content: SectionContent::ManagedOutput(vec![ManagedServiceOutput {
                service_id: "svc-giant".into(),
                lines: vec![giant_line; 51 * 1024],
                truncated_lines: false,
                bytes: 0,
                truncated_bytes: false,
            }]),
        });
        model
    }

    fn model_just_under_budget() -> BundleModel {
        let mut model = minimal_model();
        let line = "y".repeat(1024);
        model.sections.push(BundleSection {
            name: SectionName::ManagedOutput,
            content: SectionContent::ManagedOutput(vec![ManagedServiceOutput {
                service_id: "svc-small".into(),
                lines: vec![line; 49 * 1024],
                truncated_lines: false,
                bytes: 0,
                truncated_bytes: false,
            }]),
        });
        model
    }

    fn minimal_model() -> BundleModel {
        BundleModel {
            manifest: BundleManifest {
                schema_version: BUNDLE_SCHEMA_VERSION,
                bundle_id: new_bundle_id().expect("bundle id"),
                app_version: env!("CARGO_PKG_VERSION").to_string(),
                created_at_ms: 0,
                trigger: "severe-threshold".into(),
                subsystem: "test".into(),
                severity: "severe".into(),
                fingerprint: "fp".into(),
                collection_errors: Vec::new(),
                truncation: TruncationState::default(),
                section_sizes: std::collections::BTreeMap::new(),
                redaction_applied: false,
                privacy_profile_note: String::new(),
                blake3_digest_note: String::new(),
            },
            sections: vec![BundleSection {
                name: SectionName::Health,
                content: SectionContent::Json(serde_json::json!({"ok": true})),
            }],
        }
    }

    #[test]
    fn manifest_records_truncation_state_and_never_truncates_silently() {
        let model = apply_budget(oversize_model());
        assert!(model.manifest.truncation.truncated, "must not truncate silently");
        assert!(
            !model.manifest.truncation.truncated_sections.is_empty(),
            "truncated sections must be named"
        );
        // Core manifest data is always preserved.
        assert_eq!(model.manifest.schema_version, BUNDLE_SCHEMA_VERSION);
        assert!(!model.manifest.bundle_id.is_empty());
    }

    #[test]
    fn budget_is_exactly_50_mi_b() {
        assert_eq!(BUNDLE_LOGICAL_BUDGET_BYTES, 52_428_800);
        let under = apply_budget(model_just_under_budget());
        assert!(
            !under.manifest.truncation.truncated,
            "49 MiB model must be untouched"
        );
        let over = apply_budget(oversize_model());
        let total: u64 = over.manifest.section_sizes.values().sum();
        assert!(
            total <= BUNDLE_LOGICAL_BUDGET_BYTES,
            "budget enforced: {total}"
        );
    }

    #[test]
    fn collector_error_is_typed_and_kept_in_manifest() {
        let mut model = minimal_model();
        model.manifest.collection_errors.push(TypedCollectionError {
            section: SectionName::Docker,
            code: "DOCKER_PROBE_FAILED".into(),
            detail_redacted: crate::diagnostics::redact("docker ping failed token=abc123"),
        });
        assert_eq!(model.manifest.collection_errors.len(), 1);
        let err = &model.manifest.collection_errors[0];
        assert!(matches!(err.section, SectionName::Docker));
        assert!(!err.detail_redacted.contains("abc123"), "detail redacted");
    }

    #[test]
    fn bundle_id_is_opaque_24_hex() {
        let id = new_bundle_id().expect("bundle id");
        assert_eq!(id.len(), 24, "24 hex chars");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        // Not derived from content: two calls differ (OS RNG).
        assert_ne!(id, new_bundle_id().expect("second bundle id"));
    }

    #[test]
    fn entry_names_are_fixed_and_content_free() {
        assert_eq!(entry_name(&SectionName::Manifest), "manifest.json");
        assert_eq!(entry_name(&SectionName::Summary), "summary.txt");
        assert_eq!(entry_name(&SectionName::ManagedOutput), "managed-output");
        assert_eq!(entry_name(&SectionName::CrashEmergency), "crash/emergency.json");
    }
}
