//! Phase 11C Task 7 — structural privacy transforms for export profiles
//! (spec §12, §34).
//!
//! Exports operate over the TYPED bundle model: every structured JSON field,
//! array/map, text section, managed-output line, and manifest string is
//! rewritten through the identity substitution list + the Phase 10B secret
//! redactor. Free-text redaction alone is never trusted (structural first,
//! redactor as defense-in-depth). Entry names derive from section names only.

#![allow(dead_code)] // Consumed by export/GitHub-workflow tasks (Tasks 13/19).
use crate::diagnostics::bundle::{
    BundleModel, BundleSection, ManagedServiceOutput, SectionContent, SectionName,
};
use crate::diagnostics::redact;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrivacyProfile {
    /// Default GitHub support workflow / Copy Diagnostic Summary.
    SafeShare,
    /// Full path/project/process diagnostic detail; unique machine IDs removed.
    DeveloperDetail,
    /// Approved local identity retained; secret invariant still enforced.
    FullForensics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IdentityKind {
    Username,
    Hostname,
    Sid,
    MachineGuid,
    MacAddress,
    LocalIp,
    DeviceSerial,
    UserHomePath,
    ProjectName,
    ProjectPath,
    ExecutablePath,
}

#[derive(Debug, Clone)]
pub(crate) struct IdentityField {
    pub(crate) kind: IdentityKind,
    pub(crate) value: String,
}

impl IdentityKind {
    /// Safe Share generalization token for this identity kind.
    fn token(&self) -> &'static str {
        match self {
            IdentityKind::Username => "<USERNAME>",
            IdentityKind::Hostname => "<HOSTNAME>",
            IdentityKind::Sid => "<SID>",
            IdentityKind::MachineGuid => "<MACHINE_GUID>",
            IdentityKind::MacAddress => "<MAC>",
            IdentityKind::LocalIp => "<LOCAL_IP>",
            IdentityKind::DeviceSerial => "<DEVICE_SERIAL>",
            IdentityKind::UserHomePath => "<USER_HOME>",
            IdentityKind::ProjectName => "<PROJECT>",
            IdentityKind::ProjectPath => "<PROJECT>",
            IdentityKind::ExecutablePath => "<EXE_PATH>",
        }
    }

    /// Developer Detail removes unique machine identity only; local
    /// path/project/process detail is the point of this profile (spec §12-B).
    fn stripped_in_developer_detail(&self) -> bool {
        matches!(
            self,
            IdentityKind::Username
                | IdentityKind::Hostname
                | IdentityKind::Sid
                | IdentityKind::MachineGuid
                | IdentityKind::MacAddress
                | IdentityKind::LocalIp
                | IdentityKind::DeviceSerial
        )
    }
}

fn substitution_lists(
    profile: PrivacyProfile,
    identity: &[IdentityField],
) -> (Vec<(String, &'static str)>, Vec<String>) {
    let mut subs: Vec<(String, &'static str)> = Vec::new();
    let mut kept: Vec<String> = Vec::new();
    for f in identity {
        let strip = match profile {
            PrivacyProfile::SafeShare => true,
            PrivacyProfile::DeveloperDetail => f.kind.stripped_in_developer_detail(),
            PrivacyProfile::FullForensics => false,
        };
        if strip {
            subs.push((f.value.clone(), f.kind.token()));
        } else {
            kept.push(f.value.clone());
        }
    }
    // Longest values first so "C:\Users\alice" is generalized before the
    // bare username "alice" can tear a longer path apart.
    subs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
    kept.sort_by(|a, b| b.len().cmp(&a.len()));
    (subs, kept)
}

fn scrub_string(s: &str, subs: &[(String, &'static str)], kept: &[String]) -> String {
    let mut out = s.to_string();
    // Protect retained (profile-approved) identity values so a shorter
    // stripped value embedded inside them (username inside a full path)
    // cannot corrupt the approved data. Protect → substitute → restore.
    let mut sentinels: Vec<(String, String)> = Vec::new();
    for (i, k) in kept.iter().enumerate() {
        if !k.is_empty() && out.contains(k.as_str()) {
            let sentinel = format!("\u{0}K{i}\u{0}");
            sentinels.push((sentinel.clone(), k.clone()));
            out = out.replace(k.as_str(), &sentinel);
        }
    }
    for (value, token) in subs {
        if !value.is_empty() && out.contains(value.as_str()) {
            out = out.replace(value.as_str(), token);
        }
    }
    for (sentinel, original) in &sentinels {
        out = out.replace(sentinel.as_str(), original);
    }
    // Defense-in-depth LAST so secret patterns are masked even inside
    // restored approved values (spec §10 hard invariant, every profile).
    redact(&out)
}

fn rewrite_json(v: &serde_json::Value, subs: &[(String, &'static str)], kept: &[String]) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => serde_json::Value::String(scrub_string(s, subs, kept)),
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(|i| rewrite_json(i, subs, kept)).collect())
        }
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, val) in map {
                // Keys are structural (field names), values are content.
                out.insert(k.clone(), rewrite_json(val, subs, kept));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

fn scrub_managed(
    services: &[ManagedServiceOutput],
    subs: &[(String, &'static str)],
    kept: &[String],
) -> Vec<ManagedServiceOutput> {
    services
        .iter()
        .map(|s| ManagedServiceOutput {
            service_id: scrub_string(&s.service_id, subs, kept),
            lines: s.lines.iter().map(|l| scrub_string(l, subs, kept)).collect(),
            truncated_lines: s.truncated_lines,
            bytes: s.bytes,
            truncated_bytes: s.truncated_bytes,
        })
        .collect()
}

/// STRUCTURAL export transform: walks the typed model and rewrites every
/// string surface for the requested profile. Identity values are replaced by
/// the profile's generalizations; the secret redactor runs over all strings
/// in ALL profiles including Full Forensics (spec §12-C).
pub(crate) fn transform(
    model: &BundleModel,
    profile: PrivacyProfile,
    identity: &[IdentityField],
) -> BundleModel {
    let (subs, kept) = substitution_lists(profile, identity);

    let manifest_note = match profile {
        PrivacyProfile::SafeShare => {
            "Safe Share: unique machine identifiers removed; paths generalized."
        }
        PrivacyProfile::DeveloperDetail => {
            "Developer Detail: full local paths retained; unique machine IDs removed."
        }
        PrivacyProfile::FullForensics => {
            "Full Forensics: approved local identity retained; hard secret classes still excluded."
        }
    };

    let mut manifest = model.manifest.clone();
    manifest.redaction_applied = true;
    manifest.privacy_profile_note = manifest_note.to_string();
    manifest.trigger = scrub_string(&manifest.trigger, &subs, &kept);
    manifest.subsystem = scrub_string(&manifest.subsystem, &subs, &kept);
    manifest.severity = scrub_string(&manifest.severity, &subs, &kept);
    for err in &mut manifest.collection_errors {
        err.detail_redacted = redact(&err.detail_redacted);
    }

    let sections = model
        .sections
        .iter()
        .map(|section| BundleSection {
            name: section.name,
            content: match &section.content {
                SectionContent::Json(v) => SectionContent::Json(rewrite_json(v, &subs, &kept)),
                SectionContent::Text(t) => SectionContent::Text(scrub_string(t, &subs, &kept)),
                SectionContent::ManagedOutput(services) => {
                    SectionContent::ManagedOutput(scrub_managed(services, &subs, &kept))
                }
            },
        })
        .collect();

    BundleModel { manifest, sections }
}

/// Deterministic issue-summary derivation used by the GitHub support
/// workflow (Task 19): ALWAYS derived from an already-transformed model.
/// Kept here so the transform test can prove raw derivation is unsafe and
/// transformed derivation is clean.
pub(crate) fn derive_issue_summary(model: &BundleModel) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(format!(
        "LocalStack Control Center {} — {} ({})",
        model.manifest.app_version, model.manifest.trigger, model.manifest.subsystem
    ));
    for section in &model.sections {
        match &section.content {
            SectionContent::Text(t) => parts.push(t.clone()),
            SectionContent::Json(v) => parts.push(v.to_string()),
            SectionContent::ManagedOutput(services) => {
                for s in services {
                    parts.push(s.lines.join("\n"));
                }
            }
        }
    }
    parts.join("\n\n")
}

pub(crate) fn entry_name(name: &SectionName) -> &'static str {
    name.entry_name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::bundle::{BundleManifest, BUNDLE_SCHEMA_VERSION};

    /// COMPLETE synthetic fixture (no real credentials). Every identity
    /// value and every secret shape appears in ALL of: a direct System JSON
    /// field, a nested object (two levels), an array element, a map value,
    /// ManagedOutput (LogRing) lines, and the generated Summary text.
    fn fixture_values() -> Vec<String> {
        vec![
            "alice".into(),                                // Username
            "DESKTOP-XYZ".into(),                          // Hostname
            "S-1-5-21-1004336348-1177248915-682003330-1001".into(), // Sid
            "1234abcd-12ab-34cd-56ef-1234567890ab".into(), // MachineGuid
            "AA-BB-CC-DD-EE-FF".into(),                    // MacAddress
            "192.168.1.50".into(),                         // LocalIp
            "WD-EXAMPLE-SERIAL-77413".into(),              // DeviceSerial (collector Skipped; schema must still handle it)
            "C:\\Users\\alice".into(),                     // UserHomePath
            "SecretApp".into(),                            // ProjectName
            "C:\\Users\\alice\\Projects\\SecretApp".into(), // ProjectPath
            "C:\\tools\\secretapp.exe".into(),             // ExecutablePath
            "token=abc123secret".into(),                   // secret: token
            "password=hunter2".into(),                     // secret: password
            "api_key=sk-live-EXAMPLE-000".into(),          // secret: api key
            "Authorization: Bearer eyJEXAMPLE.TOKEN.999".into(), // secret: auth header
            "Cookie: session=EXAMPLE-5150".into(),         // secret: cookie
            "-----BEGIN PRIVATE KEY-----\nEXAMPLEBLOCK\n-----END PRIVATE KEY-----".into(), // secret: private key
        ]
    }

    fn identity() -> Vec<IdentityField> {
        let kinds = [
            IdentityKind::Username,
            IdentityKind::Hostname,
            IdentityKind::Sid,
            IdentityKind::MachineGuid,
            IdentityKind::MacAddress,
            IdentityKind::LocalIp,
            IdentityKind::DeviceSerial,
            IdentityKind::UserHomePath,
            IdentityKind::ProjectName,
            IdentityKind::ProjectPath,
            IdentityKind::ExecutablePath,
        ];
        let vals = fixture_values();
        kinds
            .into_iter()
            .zip(vals.iter().take(11).cloned())
            .map(|(kind, value)| IdentityField { kind, value })
            .collect()
    }

    fn model_with_sensitive() -> BundleModel {
        let vals = fixture_values();
        let mut direct = serde_json::Map::new();
        for (i, v) in vals.iter().enumerate() {
            direct.insert(format!("field_{i}"), serde_json::Value::String(v.clone()));
        }
        // Two-level nested object.
        let mut nested = serde_json::Map::new();
        for (i, v) in vals.iter().enumerate() {
            let mut inner = serde_json::Map::new();
            inner.insert("deep".into(), serde_json::Value::String(v.clone()));
            nested.insert(format!("nested_{i}"), serde_json::Value::Object(inner));
        }
        // Array + map placements.
        let array = serde_json::Value::Array(
            vals.iter()
                .map(|v| serde_json::Value::String(v.clone()))
                .collect(),
        );
        let mut map = serde_json::Map::new();
        for (i, v) in vals.iter().enumerate() {
            map.insert(format!("key_{i}"), serde_json::Value::String(v.clone()));
        }
        direct.insert("nested".into(), serde_json::Value::Object(nested));
        direct.insert("array".into(), array);
        direct.insert("map".into(), serde_json::Value::Object(map));

        let summary = format!("Diag summary: {}", vals.join(" | "));
        let managed: Vec<ManagedServiceOutput> = vals
            .iter()
            .enumerate()
            .map(|(i, v)| ManagedServiceOutput {
                service_id: format!("svc-{i}"),
                lines: vec![v.clone()],
                truncated_lines: false,
                bytes: v.len() as u64,
                truncated_bytes: false,
            })
            .collect();

        BundleModel {
            manifest: BundleManifest {
                schema_version: BUNDLE_SCHEMA_VERSION,
                bundle_id: "0123456789abcdef01234567".into(),
                app_version: "1.0.1".into(),
                created_at_ms: 0,
                trigger: "severe-threshold".into(),
                subsystem: "system".into(),
                severity: "severe".into(),
                fingerprint: "fp".into(),
                collection_errors: Vec::new(),
                truncation: Default::default(),
                section_sizes: Default::default(),
                redaction_applied: false,
                privacy_profile_note: String::new(),
                blake3_digest_note: String::new(),
            },
            sections: vec![
                BundleSection {
                    name: SectionName::System,
                    content: SectionContent::Json(serde_json::Value::Object(direct)),
                },
                BundleSection {
                    name: SectionName::Summary,
                    content: SectionContent::Text(summary),
                },
                BundleSection {
                    name: SectionName::ManagedOutput,
                    content: SectionContent::ManagedOutput(managed),
                },
            ],
        }
    }

    const IDENTITY_ABSENT_SAFE_SHARE: &[&str] = &[
        "alice",
        "DESKTOP-XYZ",
        "S-1-5-21",
        "1234abcd",
        "AA-BB-CC",
        "192.168.1.50",
        "SecretApp",
        "WD-EXAMPLE-SERIAL",
        "C:\\Users\\alice",
        "secretapp.exe",
    ];
    const SECRET_ABSENT_ALL: &[&str] = &[
        "hunter2",
        "abc123secret",
        "sk-live-EXAMPLE",
        "eyJEXAMPLE",
        "session=EXAMPLE-5150",
        "BEGIN PRIVATE KEY",
        "EXAMPLEBLOCK",
    ];

    fn serialized(out: &BundleModel) -> String {
        serde_json::to_string(out).unwrap()
    }

    #[test]
    fn safe_share_structural_transform_hits_nested_json_arrays_and_maps() {
        let model = model_with_sensitive();
        let out = transform(&model, PrivacyProfile::SafeShare, &identity());
        let s = serialized(&out);
        for absent in IDENTITY_ABSENT_SAFE_SHARE
            .iter()
            .chain(SECRET_ABSENT_ALL.iter())
        {
            assert!(!s.contains(absent), "Safe Share leaked: {absent}");
        }
        for present in [
            "<USER_HOME>", "<PROJECT>", "<LOCAL_IP>", "<DEVICE_SERIAL>", "<SID>",
            "<MACHINE_GUID>", "<MAC>", "<HOSTNAME>", "<USERNAME>", "<EXE_PATH>",
        ] {
            assert!(
                s.contains(present),
                "Safe Share missing generalization: {present}"
            );
        }
    }

    #[test]
    fn developer_detail_keeps_paths_but_strips_unique_ids() {
        let model = model_with_sensitive();
        let out = transform(&model, PrivacyProfile::DeveloperDetail, &identity());
        let s = serialized(&out);
        // Approved path/project detail retained (JSON-escaped backslashes).
        assert!(s.contains("C:\\\\Users\\\\alice\\\\Projects\\\\SecretApp"));
        assert!(s.contains("SecretApp"));
        assert!(s.contains("C:\\\\tools\\\\secretapp.exe"));
        // Unique identity removed. NOTE: the standalone username value is
        // replaced by <USERNAME>; retained full paths keep their embedded
        // username by design — that IS the approved full-path data (spec
        // §12-B), so absence is asserted for the standalone JSON value only.
        assert!(!s.contains("\"alice\""), "standalone username leaked");
        assert!(s.contains("<USERNAME>"));
        for absent in [
            "DESKTOP-XYZ",
            "S-1-5-21",
            "1234abcd",
            "AA-BB-CC",
            "192.168.1.50",
            "WD-EXAMPLE-SERIAL",
        ] {
            assert!(!s.contains(absent), "Developer Detail leaked: {absent}");
        }
        // Secret invariant holds in every profile.
        for absent in SECRET_ABSENT_ALL {
            assert!(!s.contains(absent), "Developer Detail leaked secret: {absent}");
        }
    }

    #[test]
    fn full_forensics_keeps_identity_but_enforces_secret_invariant() {
        let model = model_with_sensitive();
        let out = transform(&model, PrivacyProfile::FullForensics, &identity());
        let s = serialized(&out);
        // Approved identity retained (incl. DeviceSerial fixture).
        for present in [
            "alice",
            "DESKTOP-XYZ",
            "S-1-5-21",
            "1234abcd",
            "AA-BB-CC",
            "192.168.1.50",
            "WD-EXAMPLE-SERIAL",
            "SecretApp",
        ] {
            assert!(s.contains(present), "Full Forensics lost identity: {present}");
        }
        // Secret invariant still enforced.
        for absent in SECRET_ABSENT_ALL {
            assert!(!s.contains(absent), "Full Forensics leaked secret: {absent}");
        }
    }

    #[test]
    fn safe_share_issue_body_derives_only_from_transformed_model() {
        let model = model_with_sensitive();
        // Raw derivation is UNSAFE — prove the fixture is real by showing raw
        // output carries identity.
        let raw = derive_issue_summary(&model);
        assert!(raw.contains("alice"), "fixture sanity: raw contains identity");

        let out = transform(&model, PrivacyProfile::SafeShare, &identity());
        let body = derive_issue_summary(&out);
        for absent in IDENTITY_ABSENT_SAFE_SHARE
            .iter()
            .chain(SECRET_ABSENT_ALL.iter())
        {
            assert!(!body.contains(absent), "issue body leaked: {absent}");
        }
    }

    #[test]
    fn filenames_never_contain_content() {
        for name in [
            SectionName::Manifest,
            SectionName::Summary,
            SectionName::Health,
            SectionName::Incidents,
            SectionName::System,
            SectionName::Inventory,
            SectionName::Projects,
            SectionName::Workspaces,
            SectionName::Docker,
            SectionName::AiRuntimes,
            SectionName::DiagnosticsLog,
            SectionName::ManagedOutput,
            SectionName::CrashEmergency,
        ] {
            let entry = entry_name(&name);
            for content in ["alice", "SecretApp", "DESKTOP-XYZ"] {
                assert!(!entry.contains(content), "entry name carries content");
            }
        }
    }
}
