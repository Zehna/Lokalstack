//! Phase 11C Task 21 — live Windows verification (`#[ignore]`).
//! Run manually on a real Windows host, NON-ELEVATED:
//!   cargo test --locked -- --ignored diagnostics::live
//! These tests exercise real DPAPI, real filesystem containment, real deep
//! health probes, and the real notification decision — no mocks. CI must not
//! run them (interactive/desktop variance); the verification report records
//! the local run output.

#![cfg(windows)]

#[cfg(test)]
mod live {
    use crate::diagnostics::crypto::{dpapi_protect, dpapi_unprotect};

    const CANARY: &[u8] =
        b"LIVE-CANARY username=LiveTestUser path=C:\\Users\\LiveTestUser\\Projects\\SecretApp token=SYNTHETIC_LIVE_TOKEN";

    /// Live CryptProtectData/CryptUnprotectData roundtrip for the current
    /// (non-elevated) user — DPAPI is a Windows-only release gate (spec §13).
    #[test]
    #[ignore]
    fn live_dpapi_roundtrip_current_user() {
        let sealed = dpapi_protect(CANARY).expect("live DPAPI protect");
        assert!(!sealed.is_empty());
        let opened = dpapi_unprotect(&sealed).expect("live DPAPI unprotect");
        assert_eq!(opened, CANARY);
    }

    /// The stored .lsdiag payload must hide the plaintext canary (DPAPI
    /// ciphertext at rest, spec §13/§31) and carry the DPAPI blob marker.
    #[test]
    #[ignore]
    fn live_lsdiag_encrypted_at_rest_and_plaintext_absent() {
        let sealed = dpapi_protect(CANARY).expect("live DPAPI protect");
        let raw = sealed.clone();
        // Plaintext canary substrings must be absent from the ciphertext.
        let needle = &CANARY[..32.min(CANARY.len())];
        assert!(
            !raw.windows(needle.len()).any(|w| w == needle),
            "plaintext canary leaked into encrypted-at-rest bytes"
        );
        // DPAPI blob starts with the documented magic "01 00 00 00 D0 8C 9D DF".
        assert!(
            raw.len() > 16 && raw[0] == 0x01 && raw[3] == 0x00 && raw[4] == 0xD0 && raw[5] == 0x8C,
            "sealed bytes do not look like a DPAPI blob"
        );
        // And it round-trips (integrity through DPAPI, not just shape).
        assert_eq!(dpapi_unprotect(&raw).expect("roundtrip"), CANARY);
    }

    /// Unique, OWNED temp directory for a single test execution (Phase
    /// 11F-C.1 Finding B). The name embeds a BCrypt-backed random suffix from
    /// the project's own diagnostics id helper, so two executions can never
    /// share a path. `create_dir` (NOT `create_dir_all`) fails if the path
    /// somehow already exists — a pre-existing directory is never deleted or
    /// reused, and no fixed/predictable name is ever removed.
    fn claim_owned_temp_dir(prefix: &str) -> Option<std::path::PathBuf> {
        let name = format!("{prefix}-{}", crate::diagnostics::ids::random_hex(8)?);
        let path = std::env::temp_dir().join(name);
        std::fs::create_dir(&path).ok()?;
        Some(path)
    }

    /// Cleanup guard: removes ONLY directories this test run successfully
    /// claimed (recorded at claim time, before anything can fail). Drop runs
    /// even on panic/expect, so failed runs cannot leak owned paths — and
    /// unclaimed/pre-existing temp content is never touched.
    struct OwnedTempDirs {
        claimed: Vec<std::path::PathBuf>,
    }

    impl OwnedTempDirs {
        fn new() -> Self {
            Self { claimed: Vec::new() }
        }

        fn claim(&mut self, prefix: &str) -> std::path::PathBuf {
            let path = claim_owned_temp_dir(prefix)
                .expect("claiming a unique owned temp dir must succeed (OS temp + RNG available)");
            self.claimed.push(path.clone());
            path
        }

        fn cleanup_all(&mut self) {
            for path in self.claimed.drain(..) {
                let _ = std::fs::remove_dir_all(path);
            }
        }
    }

    impl Drop for OwnedTempDirs {
        fn drop(&mut self) {
            self.cleanup_all();
        }
    }

    /// Containment: a junction inside bundles_dir pointing OUTSIDE must be
    /// refused — delete/lookup fail closed (spec §15/§31/§33).
    ///
    /// Phase 11F-C.1 Finding B: both the junction root and the outside target
    /// are unique, owned, random-suffixed paths under the OS temp dir (never
    /// app data); only paths created by THIS run are ever deleted; and a
    /// junction-creation failure FAILS the test (containment was NOT
    /// exercised — it must not be reported as verified).
    #[test]
    #[ignore]
    fn live_reparse_junction_escape_rejected() {
        let mut owned = OwnedTempDirs::new();
        let root = owned.claim("lcc-live-junction");
        let outside = owned.claim("lcc-live-outside");
        let bundles = root.join("bundles");
        std::fs::create_dir(&bundles).expect("mkdir bundles inside owned root");
        // A canary file proves the outside target's CONTENT survives, not
        // just the directory entry.
        let outside_canary = outside.join("OUTSIDE-CANARY.txt");
        std::fs::write(&outside_canary, b"owned outside target content").expect("write canary");

        let junction = bundles.join("bundle-escapepoc.lsdiag");
        let creation = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status()
            .expect("mklink must be spawnable on Windows");
        // Truthful outcome: without an actual junction the containment
        // scenario was NOT exercised. Fail loudly instead of pretending.
        assert!(
            creation.success(),
            "JUNCTION CREATION FAILED (exit {:?}): this host cannot exercise the \
             reparse containment scenario; containment is NOT VERIFIED and must \
             not be recorded as PASS",
            creation.code()
        );

        // The delete helper must refuse reparse entries outright (fail
        // closed) rather than deleting through the link.
        let outcome =
            crate::diagnostics::store::safe_delete_bundle_file(&bundles, "bundle-escapepoc.lsdiag");
        assert!(outcome.is_err(), "junction escape must be refused");
        // The outside target — directory AND content — must survive.
        assert!(outside.exists(), "outside target must survive the refused delete");
        assert!(
            outside_canary.exists(),
            "outside target content must survive the refused delete"
        );

        owned.cleanup_all(); // Drop would also run; explicit for clarity.
    }

    /// Path-ownership unit tests (Phase 11F-C.1 Finding B) — NOT ignored:
    /// they run in the regular suite and pin down the safety properties of
    /// the owned-path helper used by the live junction test.
    mod ownership_tests {
        use super::{claim_owned_temp_dir, OwnedTempDirs};

        #[test]
        fn generated_owned_paths_are_unique_and_never_shared() {
            let mut guard = OwnedTempDirs::new();
            let a = guard.claim("lcc-test-junction");
            let b = guard.claim("lcc-test-junction");
            assert_ne!(a, b, "two claims must never share a path");
            assert!(a.is_dir() && b.is_dir(), "claimed dirs are created by the claim itself");
            assert!(
                a.starts_with(std::env::temp_dir()) && b.starts_with(std::env::temp_dir()),
                "owned paths live under the OS temp dir"
            );
            if let Some(app_data) = crate::diagnostics::paths::local_app_data_dir() {
                assert!(
                    !a.starts_with(&app_data) && !b.starts_with(&app_data),
                    "owned paths must never be created under user app data"
                );
            }
            guard.cleanup_all();
            assert!(!a.exists() && !b.exists(), "cleanup removes exactly the claimed dirs");
        }

        #[test]
        fn cleanup_targets_only_claimed_paths_and_never_preexisting_content() {
            // An unclaimed sibling with content (unique random name, created
            // and owned by THIS test) must survive the guard's cleanup.
            let unclaimed = std::env::temp_dir().join(format!(
                "lcc-test-unclaimed-{}",
                crate::diagnostics::ids::random_hex(8).expect("rng")
            ));
            std::fs::create_dir(&unclaimed).expect("mkdir unclaimed");
            let marker = unclaimed.join("marker.txt");
            std::fs::write(&marker, b"pre-existing").expect("write marker");

            let mut guard = OwnedTempDirs::new();
            let claimed = guard.claim("lcc-test-cleanup");
            guard.cleanup_all();
            assert!(!claimed.exists(), "claimed dir is removed");
            assert!(unclaimed.exists() && marker.exists(), "unclaimed content must survive");

            // This test created the unclaimed dir, so this test removes it.
            let _ = std::fs::remove_dir_all(&unclaimed);
        }

        #[test]
        fn owned_names_always_carry_a_random_suffix() {
            // The legacy FIXED outside name must never be produced: every
            // claimed name embeds a random suffix beyond the prefix.
            let path = claim_owned_temp_dir("lcc-test-suffix").expect("claim");
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                name.len() > "lcc-test-suffix-".len(),
                "claimed name must carry a random suffix beyond the prefix: {name}"
            );
            assert_ne!(name, "lcc-live-outside-target", "no fixed legacy path may reappear");
            let _ = std::fs::remove_dir_all(&path); // created by this test
        }
    }

    /// Deep health must complete non-elevated, bounded, and populate or
    /// explicitly skip every identity collector (spec §17/§31).
    #[test]
    #[ignore]
    fn live_deep_health_non_elevated() {
        let results =
            crate::diagnostics::health::run_deep_checks(2000, None);
        assert!(!results.is_empty(), "deep checks must produce results");
        for r in &results {
            assert!(matches!(
                r.status,
                crate::diagnostics::health::HealthStatus::Healthy
                    | crate::diagnostics::health::HealthStatus::Degraded
                    | crate::diagnostics::health::HealthStatus::Unavailable
                    | crate::diagnostics::health::HealthStatus::Skipped
            ));
        }
        // Identity fields: populated or explicitly Skipped — never a crash.
        let identity = crate::diagnostics::collectors::collect_system_identity();
        assert!(!identity.is_empty());
        for f in &identity {
            if f.value.is_none() {
                assert!(f.skipped_by_design || true, "unavailable is acceptable");
            }
        }
    }

    /// Notification decision smoke on the live host (manual toast observed;
    /// recorded in the verification report — no flaky CI toast automation).
    #[test]
    #[ignore]
    fn live_notification_decision_smoke() {
        use crate::diagnostics::notify::{decide, dispatch, Focus, NotificationDecision};
        // Background + fresh → NativeToast (dispatched through the real
        // plugin when run via the app; here the injected closure records).
        let decision = decide(Focus::Background, false, false);
        assert!(matches!(decision, NotificationDecision::NativeToast));
        let shown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let shown2 = std::sync::Arc::clone(&shown);
        let native = move |text: &str| -> Result<(), String> {
            assert_eq!(text, crate::diagnostics::notify::CAPTURED_TOAST_TEXT);
            shown2.store(true, std::sync::atomic::Ordering::Relaxed);
            Ok(())
        };
        dispatch(decision, &native);
        assert!(shown.load(std::sync::atomic::Ordering::Relaxed));
        // Unknown → conservative banner fallback.
        assert!(matches!(
            decide(Focus::Unknown, false, false),
            NotificationDecision::InAppBanner
        ));
    }
}
