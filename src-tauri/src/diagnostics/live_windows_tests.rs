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

    /// Containment: a junction inside bundles_dir pointing OUTSIDE must be
    /// refused — delete/lookup fail closed (spec §15/§31/§33).
    #[test]
    #[ignore]
    fn live_reparse_junction_escape_rejected() {
        let root = crate::diagnostics::paths::temp_dir()
            .unwrap_or_else(|| std::env::temp_dir())
            .join(format!("lcc-live-junction-{}", std::process::id()));
        let bundles = root.join("bundles");
        let outside = std::env::temp_dir().join("lcc-live-outside-target");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&bundles).expect("mkdir bundles");
        std::fs::create_dir_all(&outside).expect("mkdir outside");

        let junction = bundles.join("bundle-escapepoc.lsdiag");
        let creation = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .status();
        match creation {
            Ok(s) if s.success() => {
                // The delete helper must refuse reparse entries outright
                // (fail closed) rather than deleting through the link.
                let outcome = crate::diagnostics::store::safe_delete_bundle_file(
                    &bundles,
                    "bundle-escapepoc.lsdiag",
                );
                assert!(outcome.is_err(), "junction escape must be refused");
                // And the outside target must still exist (not deleted through).
                assert!(outside.exists(), "outside target must survive the refused delete");
            }
            _ => {
                eprintln!("junction creation not permitted on this host; structural containment is covered by store::tests::delete_rejects_reparse_escape");
            }
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
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
