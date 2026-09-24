//! Phase 11C Task 13 — export pipeline: privacy-profiled ZIP assembly, native
//! save, one-shot reveal capability, GitHub Safe Share summary (spec §12, §21,
//! §24). Pure parts tested; dialog/opener injected behind closures.

#![allow(dead_code)] // Consumed by Task 14 (commands.rs) wiring.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::diagnostics::bundle::{BundleModel, SectionContent};
use crate::diagnostics::ids::random_hex;
use crate::diagnostics::redact_export::{entry_name, transform, IdentityField as TxIdentityField, IdentityKind as TxIdentityKind, PrivacyProfile};
use crate::diagnostics::store::{BundleIntegrityState, BundleRegistry};

/// TTL for an export reveal capability (plan-pinned: 10 minutes).
const EXPORT_TTL_MS: u64 = 10 * 60 * 1_000;
/// Max in-memory export capabilities (plan-pinned: 8, FIFO eviction).
const MAX_EXPORT_CAPABILITIES: usize = 8;

/// Export failure modes. `FullForensicsUnconfirmed` is a USER-INTENT gate in
/// the official UI flow — the backend cannot cryptographically prove a human
/// clicked a modal; it validates bundle ID + profile + trusted flow only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    UnknownBundle,
    UnknownExport,
    CorruptSource,
    TooLarge,
    SaveCancelled,
    SaveFailed(String),
    FullForensicsUnconfirmed,
}

/// A registered, bound-to-one-destination reveal capability. In-memory only:
/// a process restart invalidates every ID.
#[derive(Debug, Clone)]
pub struct ExportCapability {
    pub export_id: String,
    pub destination: PathBuf,
    pub created_at_ms: u64,
}

/// Registry entry: a capability plus its exclusive-reveal state. `in_flight`
/// reserves the capability so AT MOST ONE reveal attempt can own it at a
/// time (Phase 11F-C.1 Finding A); the reservation is taken and released
/// under the registry lock while the OS opener itself runs OUTSIDE it.
#[derive(Debug, Clone)]
struct CapabilityEntry {
    cap: ExportCapability,
    in_flight: bool,
}

/// Bounded, expiring, one-shot-after-successful-reveal capability registry.
#[derive(Default)]
pub struct ExportCapabilityStore {
    inner: Mutex<VecDeque<CapabilityEntry>>,
    /// Test seam: unix-ms override for TTL/eviction determinism.
    pub now_ms_override: Mutex<Option<u64>>,
}

impl ExportCapabilityStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn now(&self) -> u64 {
        if let Ok(g) = self.now_ms_override.lock() {
            if let Some(v) = *g {
                return v;
            }
        }
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Register a capability bound to a TRUSTED destination (backend-owned:
    /// only the trusted native save flow calls this; no frontend path exists).
    fn register(&self, destination: PathBuf) -> Option<ExportCapability> {
        let mut q = self.inner.lock().ok()?;
        while q.len() >= MAX_EXPORT_CAPABILITIES {
            q.pop_front(); // FIFO eviction of the oldest
        }
        let cap = ExportCapability {
            export_id: random_hex(12)?,
            destination,
            created_at_ms: self.now(),
        };
        q.push_back(CapabilityEntry { cap: cap.clone(), in_flight: false });
        Some(cap)
    }

    /// One-shot reveal with EXCLUSIVE ownership (Phase 11F-C.1 Finding A):
    ///
    /// 1. under the lock: locate, reject unknown/expired, and atomically
    ///    reserve the capability (`in_flight = true`);
    /// 2. lock RELEASED — the OS opener never runs under the registry lock;
    /// 3. on success the capability is consumed permanently; on failure the
    ///    reservation is released and the capability stays retryable until
    ///    TTL/eviction. A second simultaneous reveal of the same ID fails
    ///    WITHOUT invoking its opener. `created_at_ms` is never touched by
    ///    reveal, so TTL is never renewed; max-8 FIFO semantics unchanged.
    fn reveal_with_opener(
        &self,
        export_id: &str,
        opener: &dyn Fn(&Path) -> Result<(), String>,
    ) -> Result<(), ExportError> {
        // Phase 1 (under lock): validate + atomically reserve.
        let destination = {
            let mut q = self.inner.lock().map_err(|_| ExportError::UnknownExport)?;
            let now = self.now();
            let idx = q.iter().position(|e| e.cap.export_id == export_id);
            let Some(idx) = idx else {
                return Err(ExportError::UnknownExport);
            };
            if now.saturating_sub(q[idx].cap.created_at_ms) > EXPORT_TTL_MS {
                // Expired entries are evicted on touch (even mid-reservation:
                // an expired capability can never be revealed again).
                q.remove(idx);
                return Err(ExportError::UnknownExport);
            }
            if q[idx].in_flight {
                // Exactly-one-owner invariant: the concurrent second reveal
                // fails without invoking its opener.
                return Err(ExportError::UnknownExport);
            }
            q[idx].in_flight = true;
            q[idx].cap.destination.clone()
        }; // lock released BEFORE the (slow, external) opener runs.

        // Reveal the parent folder of the trusted destination.
        let parent = destination
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| destination.clone());
        let opened = opener(&parent);

        // Phase 2 (under lock): consume on success / release reservation on
        // failure. Poisoned-lock failures here degrade conservatively: the
        // reservation may linger, but every subsequent reveal attempt of the
        // same ID fails closed and TTL/eviction still reaps the entry.
        match opened {
            Ok(()) => {
                // ONE-SHOT: remove immediately on success.
                if let Ok(mut q) = self.inner.lock() {
                    q.retain(|e| e.cap.export_id != export_id);
                }
                Ok(())
            }
            Err(_) => {
                // Opener failure: release the reservation so a later retry
                // (while unexpired) succeeds. TTL is NOT renewed.
                if let Ok(mut q) = self.inner.lock() {
                    if let Some(e) = q.iter_mut().find(|e| e.cap.export_id == export_id) {
                        e.in_flight = false;
                    }
                }
                Err(ExportError::UnknownExport) // retained for retry
            }
        }
    }
}

/// Outcome handed back to the UI: opaque ID + display-only file name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    pub export_id: String,
    pub file_name: String,
    pub profile: PrivacyProfile,
}

/// `README-PRIVACY` text documenting the profile on every export.
fn readme_privacy(profile: PrivacyProfile) -> String {
    match profile {
        PrivacyProfile::SafeShare => {
            "This support bundle uses the Safe Share profile: unique machine identifiers \
             (username, hostname, SID, MachineGuid, MAC, local IPs, device serials) were \
             removed and local paths were generalized. Exported support bundles are NOT \
             encrypted by LocalStack. Review them before sharing."
                .to_string()
        }
        PrivacyProfile::DeveloperDetail => {
            "This support bundle uses the Developer Detail profile: full local project and \
             executable paths are retained; unique machine identifiers were removed. Full \
             paths and project names may reveal local context. Exported support bundles are \
             NOT encrypted by LocalStack. Review them before sharing."
                .to_string()
        }
        PrivacyProfile::FullForensics => {
            "This support bundle uses the Full Forensics profile: approved local identity \
             fields (username, hostname, local IPs, MAC, SID, MachineGuid, full paths) are \
             retained. Hard secret classes are still excluded in every profile. Exported \
             support bundles are NOT encrypted by LocalStack. Review them before sharing."
                .to_string()
        }
    }
}

/// Pure ZIP builder over the ALREADY-TRANSFORMED model. Entry names are fixed
/// section names (never content-derived); manifest + README-PRIVACY included.
pub fn build_zip_bytes(model: &BundleModel, profile: PrivacyProfile) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let opts: zip::write::SimpleFileOptions = Default::default();
        let _ = zip.start_file("manifest.json", opts);
        let _ = zip.write_all(serde_json::to_vec(&model.manifest).unwrap_or_default().as_slice());
        let _ = zip.start_file("README-PRIVACY.txt", opts);
        let _ = zip.write_all(readme_privacy(profile).as_bytes());
        for section in &model.sections {
            let name = entry_name(&section.name);
            match &section.content {
                SectionContent::Json(v) => {
                    let _ = zip.start_file(name, opts);
                    let _ = zip.write_all(serde_json::to_vec(v).unwrap_or_default().as_slice());
                }
                SectionContent::Text(t) => {
                    let _ = zip.start_file(name, opts);
                    let _ = zip.write_all(t.as_bytes());
                }
                SectionContent::ManagedOutput(services) => {
                    for s in services {
                        let entry = format!("managed-output/{}.log", sanitize_entry_component(&s.service_id));
                        let _ = zip.start_file(entry, opts);
                        let _ = zip.write_all(s.lines.join("\n").as_bytes());
                    }
                }
            }
        }
        let _ = zip.finish();
    }
    buf.into_inner()
}

/// Entry-name component sanitizer: opaque-shaped, no path separators.
fn sanitize_entry_component(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(64)
        .collect()
}

/// Adapter: live collector fields (Option values; omitted = None) → the Task 7
/// transform's identity model. Only PRESENT values are translated; omissions
/// contribute nothing to substitute.
fn adapt_identity(fields: &[crate::diagnostics::collectors::IdentityField]) -> Vec<TxIdentityField> {
    use crate::diagnostics::collectors::IdentityKind as LiveKind;
    fields
        .iter()
        .filter_map(|f| {
            let value = f.value.as_ref()?;
            let kind = match f.kind {
                LiveKind::Username => TxIdentityKind::Username,
                LiveKind::Hostname => TxIdentityKind::Hostname,
                LiveKind::LocalIP => TxIdentityKind::LocalIp,
                LiveKind::Mac => TxIdentityKind::MacAddress,
                LiveKind::Sid => TxIdentityKind::Sid,
                LiveKind::MachineGuid => TxIdentityKind::MachineGuid,
                // By-design skip: no live value exists in Phase 11C.
                LiveKind::DeviceSerial => return None,
            };
            Some(TxIdentityField { kind, value: value.clone() })
        })
        .collect()
}

/// Model → section bytes for a single transform+zip round trip.
pub fn export_model_bytes(model: &BundleModel, profile: PrivacyProfile, identity: &[crate::diagnostics::collectors::IdentityField]) -> Vec<u8> {
    let transformed = transform(model, profile, &adapt_identity(identity));
    build_zip_bytes(&transformed, profile)
}

/// Full export flow. `save` is the injected trusted native save dialog (real
/// impl: tauri-plugin-dialog's blocking save builder on the command's own
/// thread — NEVER the background capture worker).
pub fn export_bundle(
    registry: &BundleRegistry,
    bundle_id: &str,
    profile: PrivacyProfile,
    confirm_full_forensics: bool,
    identity: &[crate::diagnostics::collectors::IdentityField],
    capabilities: &ExportCapabilityStore,
    save: &dyn Fn() -> Result<Option<PathBuf>, String>,
) -> Result<ExportOutcome, ExportError> {
    // User-intent gate for Full Forensics (UI provides fresh per-export confirm).
    if profile == PrivacyProfile::FullForensics && !confirm_full_forensics {
        return Err(ExportError::FullForensicsUnconfirmed);
    }
    if !crate::diagnostics::store::is_owned_bundle_id(bundle_id) {
        return Err(ExportError::UnknownBundle);
    }

    let metas = registry.list();
    let Some(meta) = metas.iter().find(|m| m.bundle_id == bundle_id) else {
        return Err(ExportError::UnknownBundle);
    };
    if meta.integrity != BundleIntegrityState::Valid {
        return Err(ExportError::CorruptSource);
    }

    // Trusted native save first — user picks the destination.
    let Some(dest) = save().map_err(ExportError::SaveFailed)? else {
        return Err(ExportError::SaveCancelled);
    };
    if dest.extension().and_then(|e| e.to_str()) != Some("zip") {
        return Err(ExportError::SaveFailed("destination must be .zip".into()));
    }

    // Decrypt → unwrap the commit envelope → transform → zip.
    let filename = format!("bundle-{bundle_id}.lsdiag");
    let path = registry.bundles_dir().join(&filename);
    let plain = crate::diagnostics::crypto::read_lsdiag(&path).map_err(|_| ExportError::CorruptSource)?;
    let envelope: serde_json::Value =
        serde_json::from_slice(&plain).map_err(|_| ExportError::CorruptSource)?;
    let model: BundleModel = serde_json::from_value(
        envelope.get("payload").cloned().ok_or(ExportError::CorruptSource)?,
    )
    .map_err(|_| ExportError::CorruptSource)?;
    let transformed = transform(&model, profile, &adapt_identity(identity));
    let bytes = build_zip_bytes(&transformed, profile);

    // Durable temp write in LocalStack-owned storage, then rename into place.
    super::storage::write_export_atomic(&dest, &bytes).map_err(|_| ExportError::SaveFailed("durable write failed".into()))?;

    // Register the reveal capability bound to the trusted destination.
    let Some(cap) = capabilities.register(dest.clone()) else {
        return Err(ExportError::SaveFailed("capability registration failed".into()));
    };
    let file_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(ExportOutcome { export_id: cap.export_id, file_name, profile })
}

/// Reveal the export result using the OS opener. One-shot semantics enforced
/// by the store; the opener is injected for tests, real impl wraps
/// tauri-plugin-opener's Rust API on the command thread.
pub fn reveal_export_result(
    capabilities: &ExportCapabilityStore,
    export_id: &str,
    opener: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<(), ExportError> {
    capabilities.reveal_with_opener(export_id, opener)
}

/// Safe-Share GitHub issue title/body, produced ONLY from an already
/// Safe-Share-transformed representation (never raw incident/bundle state).
pub fn github_safe_share_issue(model: &BundleModel, identity: &[crate::diagnostics::collectors::IdentityField]) -> (String, String) {
    let safe = transform(model, PrivacyProfile::SafeShare, &adapt_identity(identity));
    let m = &safe.manifest;
    let title: String = format!(
        "[Diagnostics] {} issue in {} (build {})",
        m.severity, m.subsystem, m.app_version
    )
    .chars()
    .take(120)
    .collect();

    let mut body = String::new();
    body.push_str("## Diagnostics summary (Safe Share)\n\n");
    body.push_str(&format!("- App version: {}\n", m.app_version));
    body.push_str(&format!("- Subsystem: {}\n", m.subsystem));
    body.push_str(&format!("- Severity: {}\n", m.severity));
    body.push_str(&format!("- Trigger: {}\n", m.trigger));
    body.push_str(&format!("- Fingerprint: `{}`\n", m.fingerprint));
    for s in &safe.sections {
        match &s.content {
            SectionContent::Text(t) => {
                body.push_str(&format!("\n### {}\n\n```\n{}\n```\n", s.name.entry_name(), t));
            }
            SectionContent::Json(v) => {
                body.push_str(&format!("\n### {}\n\n```json\n{}\n```\n", s.name.entry_name(), v));
            }
            SectionContent::ManagedOutput(_) => {
                body.push_str(&format!("\n### {}\n\n(managed output attached in ZIP)\n", s.name.entry_name()));
            }
        }
        if body.len() > 7_500 {
            break;
        }
    }
    body.push_str("\n---\n\n*Generated by LocalStack Control Center (Safe Share profile). Attach the exported ZIP manually if desired.*\n");
    let body: String = body.chars().take(8_000).collect();
    (title, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::bundle::BundleSection;
    use crate::diagnostics::collectors::IdentityField as LiveField;
    use crate::diagnostics::collectors::IdentityKind as LiveKind;
    use crate::diagnostics::storage::{remove_scratch_dir, test_scratch_dir};
    use crate::diagnostics::store::MetaFields;

    /// COMPLETE synthetic fixture (no real credentials). Covers identity
    /// classes and ALL hard-secret shapes (plan §8/§H).
    fn identity_fixture() -> Vec<LiveField> {
        vec![
            LiveField { kind: LiveKind::Username, value: Some("alice".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::Hostname, value: Some("DESKTOP-XYZ".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::Sid, value: Some("S-1-5-21-1004336348-1170666999-682003330-1001".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::MachineGuid, value: Some("11111111-2222-3333-4444-555555555555".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::LocalIP, value: Some("192.168.1.50".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::Mac, value: Some("AA-BB-CC-DD-EE-FF".into()), skipped_by_design: false },
            LiveField { kind: LiveKind::DeviceSerial, value: None, skipped_by_design: true }, // by design
        ]
    }

    fn fixture_model() -> BundleModel {
        let manifest = crate::diagnostics::bundle::BundleManifest {
            schema_version: 1,
            bundle_id: "0123456789abcdef01234567".into(),
            app_version: "1.0.1".into(),
            created_at_ms: 1_700_000_000_000,
            trigger: "manual".into(),
            subsystem: "test".into(),
            severity: "severe".into(),
            fingerprint: "fp-test-0001".into(),
            collection_errors: Vec::new(),
            truncation: Default::default(),
            section_sizes: Default::default(),
            redaction_applied: true,
            privacy_profile_note: String::new(),
            blake3_digest_note: String::new(),
        };
        // Secrets embedded across structured surfaces: direct field, nested
        // JSON, array, map, managed output line, text summary.
        let system_json = serde_json::json!({
            "user_home": r"C:\Users\alice\Projects\SecretApp",
            "nested": { "api_key": "AKIAIOSFODNN7EXAMPLE", "sid": "S-1-5-21-1004336348-1170666999-682003330-1001" },
            "tags": ["token=ghp_0123456789abcdefghijklmnopqrstuvwxyz", "hostname=DESKTOP-XYZ"],
            "map": { "guid": "11111111-2222-3333-4444-555555555555", "mac": "AA-BB-CC-DD-EE-FF" },
            "ip": "192.168.1.50"
        });
        let summary_text = "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.signature\n\
             Cookie: session=0123456789abcdef\n\
             -----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQEA\n-----END RSA PRIVATE KEY-----\n\
             password=hunter2 at C:\\Users\\alice\\secret.txt";
        let mut model = BundleModel { manifest, sections: Vec::new() };
        model.sections.push(BundleSection {
            name: crate::diagnostics::bundle::SectionName::System,
            content: SectionContent::Json(system_json),
        });
        model.sections.push(BundleSection {
            name: crate::diagnostics::bundle::SectionName::DiagnosticsLog,
            content: SectionContent::Text(summary_text.to_string()),
        });
        model.sections.push(BundleSection {
            name: crate::diagnostics::bundle::SectionName::ManagedOutput,
            content: SectionContent::ManagedOutput(vec![crate::diagnostics::bundle::ManagedServiceOutput {
                service_id: "svc-alice".into(),
                lines: vec!["login failed for alice, api_key=AKIAIOSFODNN7EXAMPLE".into()],
                truncated_lines: false,
                bytes: 64,
                truncated_bytes: false,
            }]),
        });
        model
    }

    const SECRET_MARKERS: &[&str] = &[
        "AKIAIOSFODNN7EXAMPLE",
        "ghp_0123456789abcdefghijklmnopqrstuvwxyz",
        "eyJhbGciOiJIUzI1NiJ9",
        "hunter2",
        "session=0123456789abcdef",
        "MIIEpAIBAAKCAQEA",
    ];
    const IDENTITY_MARKERS_SAFE_SHARE: &[&str] = &[
        "alice", "DESKTOP-XYZ", "S-1-5-21-1004336348", "11111111-2222-3333",
        "192.168.1.50", "AA-BB-CC-DD-EE-FF",
    ];

    fn registry_with_bundle(dir: &std::path::Path) -> (BundleRegistry, String) {
        let reg =
            BundleRegistry::new(dir.join("index.json"), dir.join("bundles")).expect("registry");
        let model = fixture_model();
        let payload = serde_json::to_vec(&model).unwrap();
        let outcome = reg
            .commit_bundle(
                &payload,
                MetaFields {
                    created_at_ms: 1,
                    trigger: "test".into(),
                    subsystem: "test".into(),
                    severity: "severe".into(),
                    fingerprint: "fp".into(),
                    app_version: "1.0.1".into(),
                },
            )
            .expect("commit");
        (reg, outcome.bundle_id)
    }

    #[test]
    fn full_forensics_without_confirmation_is_rejected() {
        let dir = test_scratch_dir("export-ff-confirm").expect("scratch");
        let (reg, id) = registry_with_bundle(&dir);
        let caps = ExportCapabilityStore::new();
        let r = export_bundle(
            &reg, &id, PrivacyProfile::FullForensics, false, &identity_fixture(), &caps,
            &|| Ok(Some(dir.join("out.zip"))),
        );
        assert!(matches!(r, Err(ExportError::FullForensicsUnconfirmed)));
        remove_scratch_dir(&dir);
    }

    #[test]
    fn safe_share_export_contains_no_identity_fixture() {
        let dir = test_scratch_dir("export-safeshare").expect("scratch");
        let (reg, id) = registry_with_bundle(&dir);
        let caps = ExportCapabilityStore::new();
        let dest = dir.join("safe.zip");
        let out = export_bundle(
            &reg, &id, PrivacyProfile::SafeShare, false, &identity_fixture(), &caps,
            &|| Ok(Some(dest.clone())),
        )
        .expect("export ok");
        assert!(!out.export_id.is_empty());
        let bytes = std::fs::read(&dest).expect("zip written");
        // Assertions run over DECOMPRESSED entry contents — scanning raw zip
        // bytes is meaningless under DEFLATE.
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip readable");
        let mut all_text = String::new();
        for i in 0..reader.len() {
            let mut f = reader.by_index(i).expect("entry");
            assert!(
                f.name() == "README-PRIVACY.txt" || f.name().ends_with(".json") || f.name().ends_with(".log") || f.name().ends_with(".txt"),
                "entry names are fixed/sanitized: {}",
                f.name()
            );
            use std::io::Read;
            let mut s = String::new();
            f.read_to_string(&mut s).expect("entry text");
            all_text.push_str(&s);
        }
        assert!(
            reader.file_names().any(|n| n == "README-PRIVACY.txt"),
            "README-PRIVACY present"
        );
        for marker in IDENTITY_MARKERS_SAFE_SHARE {
            assert!(!all_text.contains(marker), "identity leaked: {marker}");
        }
        for marker in SECRET_MARKERS {
            assert!(!all_text.contains(marker), "secret leaked: {marker}");
        }
        remove_scratch_dir(&dir);
    }

    #[test]
    fn full_forensics_export_retains_identity_but_never_secrets() {
        let dir = test_scratch_dir("export-fullforensics").expect("scratch");
        let (reg, id) = registry_with_bundle(&dir);
        let caps = ExportCapabilityStore::new();
        let dest = dir.join("ff.zip");
        export_bundle(
            &reg, &id, PrivacyProfile::FullForensics, true, &identity_fixture(), &caps,
            &|| Ok(Some(dest.clone())),
        )
        .expect("confirmed export ok");
        let bytes = std::fs::read(&dest).expect("zip written");
        let mut reader = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).expect("zip readable");
        let mut all_text = String::new();
        for i in 0..reader.len() {
            let mut f = reader.by_index(i).expect("entry");
            use std::io::Read;
            let mut s = String::new();
            f.read_to_string(&mut s).expect("entry text");
            all_text.push_str(&s);
        }
        // Approved identity retained in Full Forensics.
        for marker in IDENTITY_MARKERS_SAFE_SHARE {
            assert!(
                all_text.contains(marker),
                "Full Forensics must retain approved identity: {marker}"
            );
        }
        // Secret invariant holds in EVERY profile.
        for marker in SECRET_MARKERS {
            assert!(!all_text.contains(marker), "secret leaked in Full Forensics: {marker}");
        }
        remove_scratch_dir(&dir);
    }

    #[test]
    fn corrupt_source_refuses_export() {
        let dir = test_scratch_dir("export-corrupt").expect("scratch");
        let reg =
            BundleRegistry::new(dir.join("index.json"), dir.join("bundles")).expect("registry");
        let payload = serde_json::to_vec(&fixture_model()).unwrap();
        let outcome = reg
            .commit_bundle(&payload, MetaFields {
                created_at_ms: 1, trigger: "t".into(), subsystem: "s".into(),
                severity: "severe".into(), fingerprint: "f".into(), app_version: "1.0.1".into(),
            })
            .unwrap();
        let caps = ExportCapabilityStore::new();
        // Truncate the .lsdiag payload → decrypt/integrity failure.
        let filename = format!("bundle-{}.lsdiag", outcome.bundle_id);
        let p = dir.join("bundles").join(&filename);
        let raw = std::fs::read(&p).unwrap();
        std::fs::write(&p, &raw[..raw.len() / 2]).unwrap();
        let r = export_bundle(
            &reg, &outcome.bundle_id, PrivacyProfile::SafeShare, false, &identity_fixture(), &caps,
            &|| Ok(Some(dir.join("x.zip"))),
        );
        assert!(matches!(r, Err(ExportError::CorruptSource)));
        assert!(!dir.join("x.zip").exists(), "no file written");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn cancelled_save_writes_nothing() {
        let dir = test_scratch_dir("export-cancel").expect("scratch");
        let (reg, id) = registry_with_bundle(&dir);
        let caps = ExportCapabilityStore::new();
        let r = export_bundle(
            &reg, &id, PrivacyProfile::SafeShare, false, &identity_fixture(), &caps,
            &|| Ok(None),
        );
        assert!(matches!(r, Err(ExportError::SaveCancelled)));
        assert_eq!(caps.inner.lock().unwrap().len(), 0, "no capability registered");
        remove_scratch_dir(&dir);
    }

    #[test]
    fn export_capability_registry_is_bounded_and_expiring() {
        let caps = ExportCapabilityStore::new();
        // 9 registrations → oldest evicted (max 8).
        let mut ids = Vec::new();
        for i in 0..9 {
            let cap = caps
                .register(std::path::PathBuf::from(format!("C:\\tmp\\e{i}.zip")))
                .expect("cap");
            ids.push(cap.export_id);
        }
        assert_eq!(caps.inner.lock().unwrap().len(), 8);
        assert!(caps.reveal_with_opener(&ids[0], &|_| Ok(())).is_err(), "oldest evicted");
        // TTL: age a capability past 10 minutes.
        let cap = caps.register(std::path::PathBuf::from("C:\\tmp\\old.zip")).unwrap();
        *caps.now_ms_override.lock().unwrap() = Some(cap.created_at_ms + EXPORT_TTL_MS + 1);
        assert!(matches!(
            caps.reveal_with_opener(&cap.export_id, &|_| Ok(())),
            Err(ExportError::UnknownExport)
        ));
        *caps.now_ms_override.lock().unwrap() = None;
    }

    #[test]
    fn successful_reveal_consumes_capability_one_shot() {
        let caps = ExportCapabilityStore::new();
        let cap = caps.register(std::path::PathBuf::from("C:\\tmp\\one.zip")).unwrap();
        assert!(caps.reveal_with_opener(&cap.export_id, &|_| Ok(())).is_ok());
        assert_eq!(caps.inner.lock().unwrap().len(), 0, "consumed");
        assert!(caps.reveal_with_opener(&cap.export_id, &|_| Ok(())).is_err(), "second reveal fails");
    }

    #[test]
    fn failed_opener_preserves_capability_for_retry() {
        let caps = ExportCapabilityStore::new();
        let cap = caps.register(std::path::PathBuf::from("C:\\tmp\\retry.zip")).unwrap();
        assert!(caps.reveal_with_opener(&cap.export_id, &|_| Err("opener down".into())).is_err());
        assert_eq!(caps.inner.lock().unwrap().len(), 1, "retained for retry");
        assert!(caps.reveal_with_opener(&cap.export_id, &|_| Ok(())).is_ok(), "retry succeeds");
    }

    /// Regression (Phase 11F-C.1 Finding A): two simultaneous reveals of the
    /// SAME export_id must not both pass validation. Exactly one reveal may
    /// own/reserve the capability; the concurrent second attempt fails
    /// WITHOUT invoking its opener. Opener runs OUTSIDE the registry lock.
    ///
    /// Determinism: the loser thread waits on a channel until the winner's
    /// opener has STARTED (which, under the reservation fix, proves the
    /// winner owns the capability), then attempts the same ID. A Barrier
    /// aligns both threads up front; the channel pins the ownership order so
    /// the overlap is real on every run (a barrier alone leaves the order to
    /// the scheduler).
    #[test]
    fn concurrent_reveals_of_same_id_are_exclusively_serialized() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let caps = Arc::new(ExportCapabilityStore::new());
        let cap = caps
            .register(std::path::PathBuf::from("C:\\tmp\\conc.zip"))
            .expect("cap");
        let opener_calls = Arc::new(AtomicUsize::new(0));
        let loser_opener_ran = Arc::new(AtomicBool::new(false));
        let results = Arc::new(std::sync::Mutex::new(Vec::<bool>::new()));
        let barrier = Arc::new(Barrier::new(2));
        let (winner_opened_tx, winner_opened_rx) = std::sync::mpsc::channel::<()>();

        let caps_winner = Arc::clone(&caps);
        let calls_winner = Arc::clone(&opener_calls);
        let results_winner = Arc::clone(&results);
        let barrier_winner = Arc::clone(&barrier);
        let export_id = cap.export_id.clone();
        let winner = std::thread::spawn(move || {
            barrier_winner.wait();
            let opener = move |_: &std::path::Path| -> Result<(), String> {
                calls_winner.fetch_add(1, Ordering::SeqCst);
                // Signal that this reveal OWNS the capability (reservation
                // held / validation passed), then hold the opener open so the
                // loser attempts the same ID while it is in flight.
                let _ = winner_opened_tx.send(());
                std::thread::sleep(std::time::Duration::from_millis(150));
                Ok(())
            };
            let ok = caps_winner.reveal_with_opener(&export_id, &opener).is_ok();
            results_winner.lock().unwrap().push(ok);
        });

        let caps_loser = Arc::clone(&caps);
        let calls_loser = Arc::clone(&opener_calls);
        let loser_flag = Arc::clone(&loser_opener_ran);
        let results_loser = Arc::clone(&results);
        let barrier_loser = Arc::clone(&barrier);
        let export_id_loser = cap.export_id.clone();
        let loser = std::thread::spawn(move || {
            barrier_loser.wait();
            // Overlap point: reveal only once the winner provably entered
            // its opener (owner established).
            winner_opened_rx
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("winner opener must start within 5s");
            let opener = move |_: &std::path::Path| -> Result<(), String> {
                calls_loser.fetch_add(1, Ordering::SeqCst);
                loser_flag.store(true, Ordering::SeqCst);
                Err("loser opener must never run".into())
            };
            let ok = caps_loser.reveal_with_opener(&export_id_loser, &opener).is_ok();
            results_loser.lock().unwrap().push(ok);
        });

        winner.join().expect("winner reveal thread must not panic");
        loser.join().expect("loser reveal thread must not panic");

        let results = results.lock().unwrap().clone();
        assert_eq!(
            results.iter().filter(|ok| **ok).count(),
            1,
            "exactly ONE reveal succeeds: {results:?}"
        );
        assert_eq!(
            results.iter().filter(|ok| !**ok).count(),
            1,
            "exactly ONE reveal fails: {results:?}"
        );
        assert_eq!(
            opener_calls.load(Ordering::SeqCst),
            1,
            "opener must be invoked exactly ONCE across both concurrent reveals"
        );
        assert!(
            !loser_opener_ran.load(Ordering::SeqCst),
            "second simultaneous reveal must fail WITHOUT invoking its opener"
        );
        assert_eq!(
            caps.inner.lock().unwrap().len(),
            0,
            "successful reveal consumes the capability"
        );
        assert!(
            caps.reveal_with_opener(&cap.export_id, &|_| Ok(())).is_err(),
            "consumed capability cannot be revealed again"
        );
    }

    /// Finding A follow-up: a failed reveal must NOT renew/extend TTL. The
    /// capability keeps its ORIGINAL created_at_ms, so once the 10-minute TTL
    /// has passed, a later retry cannot resurrect it — and the expired entry
    /// is evicted on touch.
    #[test]
    fn failed_reveal_does_not_renew_ttl_or_resurrect_expired_capability() {
        let caps = ExportCapabilityStore::new();
        let cap = caps.register(std::path::PathBuf::from("C:\\tmp\\ttl.zip")).unwrap();
        assert!(caps.reveal_with_opener(&cap.export_id, &|_| Err("opener down".into())).is_err());
        // Age the capability past TTL (failed reveal did not touch created_at_ms).
        *caps.now_ms_override.lock().unwrap() = Some(cap.created_at_ms + EXPORT_TTL_MS + 1);
        assert!(
            matches!(
                caps.reveal_with_opener(&cap.export_id, &|_| Ok(())),
                Err(ExportError::UnknownExport)
            ),
            "expired capability must not be resurrected after a failed reveal"
        );
        assert_eq!(caps.inner.lock().unwrap().len(), 0, "expired entry evicted on touch");
        *caps.now_ms_override.lock().unwrap() = None;
    }

    /// Finding A scoping: the exclusive reservation is PER CAPABILITY. Two
    /// simultaneous reveals of DIFFERENT export_ids must both succeed — the
    /// fix must not degenerate into a global single-flight gate.
    #[test]
    fn concurrent_reveals_of_different_ids_do_not_block_each_other() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Barrier};

        let caps = Arc::new(ExportCapabilityStore::new());
        let cap_a = caps.register(std::path::PathBuf::from("C:\\tmp\\a.zip")).unwrap();
        let cap_b = caps.register(std::path::PathBuf::from("C:\\tmp\\b.zip")).unwrap();
        let opener_calls = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));

        let mut handles = Vec::new();
        for cap in [cap_a, cap_b] {
            let caps = Arc::clone(&caps);
            let opener_calls = Arc::clone(&opener_calls);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let opener = move |_: &std::path::Path| -> Result<(), String> {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                };
                assert!(caps.reveal_with_opener(&cap.export_id, &opener).is_ok());
            }));
        }
        for h in handles {
            h.join().expect("reveal threads must not panic");
        }
        assert_eq!(opener_calls.load(Ordering::SeqCst), 2, "both distinct reveals ran their opener");
        assert_eq!(caps.inner.lock().unwrap().len(), 0, "both capabilities consumed");
    }

    #[test]
    fn restart_invalidates_all_export_ids() {
        let caps = ExportCapabilityStore::new();
        let cap = caps.register(std::path::PathBuf::from("C:\\tmp\\r.zip")).unwrap();
        // The restart shape: a FRESH store (capabilities are in-memory only).
        let fresh = ExportCapabilityStore::new();
        assert!(matches!(
            fresh.reveal_with_opener(&cap.export_id, &|_| Ok(())),
            Err(ExportError::UnknownExport)
        ));
    }

    #[test]
    fn frontend_path_never_creates_or_renews_capability() {
        let caps = ExportCapabilityStore::new();
        // register() is private; the only public registration path is
        // export_bundle's trusted save. A path submitted as an ID fails.
        assert!(matches!(
            caps.reveal_with_opener("C:\\Users\\alice\\out.zip", &|_| Ok(())),
            Err(ExportError::UnknownExport)
        ));
    }

    #[test]
    fn reveal_never_accepts_frontend_path() {
        let caps = ExportCapabilityStore::new();
        let _ = caps.register(std::path::PathBuf::from("C:\\tmp\\p.zip")).unwrap();
        // Path-like strings are not IDs; membership fails closed.
        for probe in ["C:\\tmp\\p.zip", "/tmp/p.zip", "..\\..\\p.zip"] {
            assert!(caps.reveal_with_opener(probe, &|_| Ok(())).is_err());
        }
    }

    #[test]
    fn github_safe_share_body_originates_from_transformed_model() {
        let model = fixture_model();
        let (title, body) = github_safe_share_issue(&model, &identity_fixture());
        assert!(title.len() <= 120, "title bounded");
        assert!(body.len() <= 8_000, "body bounded");
        for marker in IDENTITY_MARKERS_SAFE_SHARE {
            assert!(!body.contains(marker), "identity leaked into issue body: {marker}");
            assert!(!title.contains(marker), "identity leaked into issue title: {marker}");
        }
        for marker in SECRET_MARKERS {
            assert!(!body.contains(marker), "secret leaked into issue body: {marker}");
        }
    }
}
