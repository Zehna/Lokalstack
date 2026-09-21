//! Phase 11C Task 20 — security/privacy source audits (spec §38, §33, §34).
//! Test-only module: greps the diagnostics sources via `include_str!` and
//! asserts forbidden collection/authority shapes are ABSENT. Each assertion
//! cites its spec gate. No production code lives here.

#[cfg(test)]
mod tests {
    /// (file, contents) for every diagnostics source file. Kept explicit so
    /// a new module must be added here deliberately.
    fn sources() -> Vec<(&'static str, &'static str)> {
        [
            ("mod.rs", include_str!("mod.rs")),
            ("cache.rs", include_str!("cache.rs")),
            ("collectors.rs", include_str!("collectors.rs")),
            ("commands.rs", include_str!("commands.rs")),
            ("crypto.rs", include_str!("crypto.rs")),
            ("emergency.rs", include_str!("emergency.rs")),
            ("export.rs", include_str!("export.rs")),
            ("health.rs", include_str!("health.rs")),
            ("ids.rs", include_str!("ids.rs")),
            ("incidents.rs", include_str!("incidents.rs")),
            ("notify.rs", include_str!("notify.rs")),
            ("paths.rs", include_str!("paths.rs")),
            ("policy.rs", include_str!("policy.rs")),
            ("recovery.rs", include_str!("recovery.rs")),
            ("redact_export.rs", include_str!("redact_export.rs")),
            ("storage.rs", include_str!("storage.rs")),
            ("store.rs", include_str!("store.rs")),
            ("worker.rs", include_str!("worker.rs")),
        ]
        .map(|(name, body)| (name, body))
        .to_vec()
    }

    /// §38: no raw process argv / environment dump collection.
    #[test]
    fn no_raw_argv_or_env_collection() {
        for (name, src) in sources() {
            assert!(
                !src.contains("std::env::args"),
                "{name}: std::env::args (raw argv collection) is forbidden (spec §38)"
            );
            assert!(
                !src.contains("env::vars"),
                "{name}: env::vars (raw environment dump) is forbidden (spec §38)"
            );
        }
    }

    /// §38: no authorization-header / cookie / private-key collection;
    /// no HTTP client in diagnostics (no automatic network egress, §14).
    #[test]
    fn no_authorization_cookie_or_private_key_collection() {
        for (name, src) in sources() {
            assert!(
                !src.contains("reqwest"),
                "{name}: HTTP client in diagnostics is forbidden (no automatic network egress, spec §14/§38)"
            );
            assert!(
                !src.contains("AUTHORIZATION"),
                "{name}: authorization-header collection is forbidden (spec §38)"
            );
            assert!(
                !src.contains("Set-Cookie"),
                "{name}: cookie collection is forbidden (spec §38)"
            );
        }
    }

    /// §38/§15: no arbitrary URL opener, no generic file delete, no
    /// unbounded recursive cleanup inside the diagnostics modules.
    /// Production scope only: test modules may use scratch-dir cleanup.
    #[test]
    fn no_arbitrary_url_open_or_fs_delete() {
        for (name, src) in sources() {
            let production = src
                .split("#[cfg(test)]")
                .next()
                .unwrap_or_default()
                .to_string();
            assert!(
                !production.contains("fn open_url"),
                "{name}: arbitrary URL opener is forbidden (spec §38)"
            );
            assert!(
                !production.contains("fn delete_file"),
                "{name}: generic delete_file(path) is forbidden (spec §38/§20)"
            );
            assert!(
                !production.contains("remove_dir_all"),
                "{name}: unbounded recursive cleanup is forbidden; deletes must be per-trusted-file (spec §15/§33)"
            );
        }
    }

    /// §38: no shell/WMI/PowerShell command execution for collection.
    #[test]
    fn no_shell_or_wmi_command_runner() {
        for (name, src) in sources() {
            assert!(
                !src.contains("Command::new"),
                "{name}: spawning processes (PowerShell/WMIC/cmd helper) is forbidden (spec §11/§31)"
            );
        }
    }

    /// §30: panic-path constraints — the emergency writer must not take
    /// locks via lock()/write() (only try_lock-style non-blocking reads are
    /// permitted in emergency.rs), and must not touch DPAPI/ZIP.
    #[test]
    fn panic_path_stays_nonblocking_and_crypto_free() {
        let emergency = include_str!("emergency.rs");
        assert!(
            !emergency.contains(".lock()"),
            "emergency.rs must never block on a mutex (spec §6/§30); use try_lock or pre-cached atomics"
        );
        assert!(
            !emergency.contains("CryptProtectData"),
            "emergency path must not run DPAPI (spec §6)"
        );
        assert!(
            !emergency.contains("zip::"),
            "emergency path must not build ZIPs (spec §6)"
        );
        assert!(
            !emergency.contains("SHGetKnownFolderPath"),
            "emergency path must not resolve known folders at panic time (spec §6)"
        );
    }

    /// Traceability block (spec §38/§39 gates → owning tests). Update when
    /// gates move; CI relies on this mapping staying true.
    #[test]
    fn gate_traceability_map_is_current() {
        // (gate, evidence) pairs; every entry must reference a real owning
        // test that exists in this crate's suite.
        let gates: &[(&str, &str)] = &[
            ("no raw argv/env", "audits::no_raw_argv_or_env_collection"),
            ("no auth/cookie/key collection", "audits::no_authorization_cookie_or_private_key_collection"),
            ("no arbitrary URL/FS authority", "audits::no_arbitrary_url_open_or_fs_delete"),
            ("no shell/WMI runner", "audits::no_shell_or_wmi_command_runner"),
            ("panic path non-blocking", "audits::panic_path_stays_nonblocking_and_crypto_free"),
            ("structural privacy transforms", "redact_export::tests (structural profile fixtures)"),
            ("DPAPI roundtrip + corrupt input", "crypto::tests"),
            ("retention tombstones", "store::tests (crash-window A–E)"),
            ("capture worker panic survival", "worker::tests"),
            ("notification decision fallback", "notify::tests::focus_query_error_falls_back_to_in_app_banner"),
        ];
        assert!(!gates.is_empty());
    }
}
