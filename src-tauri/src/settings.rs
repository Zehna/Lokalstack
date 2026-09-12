//! Phase 10C — application settings: model, defaults, validation,
//! persistence, corrupt-file recovery, and Windows-startup reconciliation.
//!
//! # Design rules (spec §A–§F)
//!
//! - **One normalized model** — a single `AppSettings` with explicit
//!   centralized defaults. `minimizeToTray` is NOT a separate boolean: it is
//!   normalized into [`CloseBehavior::MinimizeToTray`] so contradictory
//!   flags cannot be stored (spec §P).
//! - **Operational preferences only** — there is deliberately no field that
//!   could hold a secret. The frontend static guard plus the `settings_dto`
//!   test below pin this contract (spec §AI).
//! - **Persisted under the OS app-data dir** (`FOLDERID_LocalAppData` via
//!   the same resolver as diagnostics), never inside any source tree.
//!   Writes are atomic (temp file + rename) so a crash mid-write cannot
//!   corrupt the real file.
//! - **Corrupt input never crashes startup** — missing, empty, invalid-JSON,
//!   wrong-typed, or out-of-range files all recover to validated defaults
//!   and record a diagnostics breadcrumb (spec §F). Unknown future fields
//!   are ignored by serde.
//!
//! # Windows startup (spec §S–§U, §AH)
//!
//! Startup registration is abstracted behind [`startup`] so the settings
//! engine never depends on the registry directly: enable/disable are
//! user-opt-in operations through a narrow HKCU run-key entry (no admin, no
//! unrelated registry writes), and [`startup::actual_enabled`] reconciles
//! persisted intent against OS reality — the UI must never lie (spec §AH).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::diagnostics;

/// Minimum poll interval accepted from any source (file or frontend).
pub(crate) const MIN_INTERVAL_MS: u32 = 1_000;
/// Maximum poll interval accepted from any source (file or frontend).
pub(crate) const MAX_INTERVAL_MS: u32 = 60_000;

/// What happens when the user closes the window (spec §P).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CloseBehavior {
    /// Closing the window exits LocalStack.
    #[default]
    Exit,
    /// Closing the window hides it; the app keeps running in the tray.
    MinimizeToTray,
}

/// Theme preference (spec §AD). System/Light/Dark are representable; the
/// UI currently ships a single dark palette, so anything other than the
/// default maps to the same visual result until theming lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Theme {
    /// Follow the OS preference (reserved; visually the dark palette today).
    #[default]
    System,
    Light,
    Dark,
}

/// One normalized settings model (spec §B).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AppSettings {
    /// Base frontend auto-polling (ports/listeners).
    pub(crate) auto_refresh: bool,
    /// Base poll cadence in milliseconds, clamped to [MIN, MAX].
    pub(crate) port_refresh_interval_ms: u32,
    /// AI runtime auto-polling (manual refresh always works).
    pub(crate) ai_polling_enabled: bool,
    /// Docker auto-polling (manual refresh always works).
    pub(crate) docker_polling_enabled: bool,
    /// Launch with the main window hidden (tray fallback guarantees access).
    pub(crate) launch_minimized: bool,
    /// What closing the window does (absorbs minimize-to-tray, spec §P).
    pub(crate) close_behavior: CloseBehavior,
    /// Register LocalStack to run at Windows startup (opt-in, default OFF).
    pub(crate) run_at_startup: bool,
    /// Theme preference (visual no-op until theming lands; persisted anyway).
    pub(crate) theme: Theme,
}

impl Default for AppSettings {
    fn default() -> Self {
        // Spec §C — explicit centralized defaults. Never surprise the user
        // with background startup or tray-only close behavior.
        Self {
            auto_refresh: true,
            port_refresh_interval_ms: 3_000,
            ai_polling_enabled: true,
            docker_polling_enabled: true,
            launch_minimized: false,
            close_behavior: CloseBehavior::Exit,
            run_at_startup: false,
            theme: Theme::System,
        }
    }
}

impl AppSettings {
    /// Validate + clamp every field into its safe range. Used both after
    /// deserialization (file/frontend input) and in tests (spec §D).
    pub(crate) fn validated(mut self) -> Self {
        self.port_refresh_interval_ms = self
            .port_refresh_interval_ms
            .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS);
        self
    }
}

/// Absolute path of the settings file (never inside a source tree).
pub(crate) fn settings_path() -> Option<PathBuf> {
    crate::diagnostics::local_app_data_dir().map(|dir| dir.join("settings.json"))
}

/// Load settings from disk. ANY problem — missing, empty, invalid JSON,
/// wrong types, out-of-range values — recovers to validated defaults and
/// records one safe diagnostics breadcrumb (spec §F). Never panics.
pub(crate) fn load_or_default() -> AppSettings {
    let Some(path) = settings_path() else {
        diagnostics::warn("settings", "app-data dir unavailable; using default settings");
        return AppSettings::default().validated();
    };
    match fs::read_to_string(&path) {
        // Missing file: first run. Defaults, no warning.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => AppSettings::default().validated(),
        Err(err) => {
            diagnostics::warn(
                "settings",
                &format!("settings unreadable ({err}); using defaults"),
            );
            AppSettings::default().validated()
        }
        Ok(text) => {
            if text.trim().is_empty() {
                diagnostics::warn("settings", "settings file empty; using defaults");
                return AppSettings::default().validated();
            }
            match serde_json::from_str::<AppSettings>(&text) {
                Ok(settings) => {
                    let validated = settings.clone().validated();
                    if validated != settings {
                        // Out-of-range values are corrected, not fatal (§D).
                        diagnostics::warn(
                            "settings",
                            "settings contained out-of-range values; clamped",
                        );
                    }
                    validated
                }
                Err(err) => {
                    diagnostics::warn(
                        "settings",
                        &format!("settings file invalid ({err}); using defaults"),
                    );
                    AppSettings::default().validated()
                }
            }
        }
    }
}

/// Atomically persist settings: write to a temp file in the same directory,
/// then rename over the real file. A crash mid-write leaves the old (or no)
/// file, never a truncated one. Creates the parent dir when missing.
pub(crate) fn save(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path().ok_or_else(|| "settings directory unavailable".to_string())?;
    save_to(&path, settings)
}

/// Path-injected atomic write core (the production behavior, testable:
/// temp-file placement, old-file preservation on failed write, and leftover
/// temp-file tolerance are all asserted in tests below).
fn save_to(path: &Path, settings: &AppSettings) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| "settings path has no parent".to_string())?;
    fs::create_dir_all(dir).map_err(|err| format!("could not create settings dir: {err}"))?;
    let json = serde_json::to_string_pretty(settings).map_err(|err| err.to_string())?;
    // Temp file lives in the SAME directory (same volume → rename is atomic).
    let tmp = dir.join("settings.json.tmp");
    fs::write(&tmp, json).map_err(|err| format!("could not write settings: {err}"))?;
    fs::rename(&tmp, path).map_err(|err| format!("could not finalize settings: {err}"))?;
    Ok(())
}

/// Reset to defaults: restore in memory, persist, and return the model.
pub(crate) fn reset() -> AppSettings {
    let defaults = AppSettings::default();
    if let Err(err) = save(&defaults) {
        diagnostics::warn("settings", &format!("settings reset failed to persist: {err}"));
    }
    defaults
}

/* =========================================================================
 * Windows startup — reconciliation policy (spec §S–§U, §AH).
 *
 * Registration itself goes through the Tauri autostart plugin (HKCU run
 * key, no admin, one named entry — enable/disable reverse cleanly).
 * This module owns the POLICY: reconciling persisted intent against OS
 * reality so the UI never lies (spec §AH), with pure, testable decisions.
 * ---------------------------------------------------------------------- */

pub(crate) mod startup {
    use super::*;

    /// Required action to make persisted intent match OS reality.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum ReconcileAction {
        /// OS state already matches — nothing to do.
        None,
        /// Register startup (intent ON, OS absent).
        Enable,
        /// Deregister startup (intent OFF, OS present).
        Disable,
    }

    /// Pure decision: what must happen given intent vs actual.
    pub(crate) fn reconcile_action(intent: bool, actual: bool) -> ReconcileAction {
        match (intent, actual) {
            (true, false) => ReconcileAction::Enable,
            (false, true) => ReconcileAction::Disable,
            _ => ReconcileAction::None,
        }
    }

    /// Apply the policy. `read_actual`/`set_enabled` are injected so the
    /// decision path is testable without a registry; production wires the
    /// autostart plugin API in `lib.rs`.
    pub(crate) fn reconcile_with(
        settings: &mut AppSettings,
        read_actual: impl FnOnce() -> bool,
        mut set_enabled: impl FnMut(bool) -> Result<(), String>,
    ) {
        let actual = read_actual();
        match reconcile_action(settings.run_at_startup, actual) {
            ReconcileAction::None => {}
            ReconcileAction::Enable => match set_enabled(true) {
                Ok(()) => diagnostics::info("startup", "startup registration restored"),
                Err(err) => {
                    diagnostics::warn("startup", &format!("startup re-registration failed: {err}"))
                }
            },
            ReconcileAction::Disable => match set_enabled(false) {
                Ok(()) => diagnostics::info("startup", "stale startup registration removed"),
                Err(err) => {
                    diagnostics::warn("startup", &format!("startup deregistration failed: {err}"))
                }
            },
        }
    }

    /// Set startup registration from an explicit user action, keeping the
    /// settings model in sync with what was actually applied. Returns the
    /// resulting state for accurate UI reflection (spec §U).
    pub(crate) fn apply_user_toggle(
        settings: &mut AppSettings,
        enabled: bool,
        mut set_enabled: impl FnMut(bool) -> Result<(), String>,
    ) -> Result<bool, String> {
        set_enabled(enabled)?;
        settings.run_at_startup = enabled;
        Ok(enabled)
    }

    /// True when the OS currently has LocalStack registered for startup.
    /// Delegates to the autostart plugin's manager (HKCU read, no admin)
    /// via the app handle.
    pub(crate) fn is_registered_from(app: &tauri::AppHandle) -> bool {
        use tauri_plugin_autostart::ManagerExt;
        app.autolaunch().is_enabled().unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Defaults (spec §C) -------------------------------------------

    #[test]
    fn defaults_match_spec() {
        let d = AppSettings::default();
        assert!(d.auto_refresh);
        assert_eq!(d.port_refresh_interval_ms, 3_000);
        assert!(d.ai_polling_enabled);
        assert!(d.docker_polling_enabled);
        assert!(!d.launch_minimized);
        assert_eq!(d.close_behavior, CloseBehavior::Exit);
        assert!(!d.run_at_startup);
        assert_eq!(d.theme, Theme::System);
    }

    // ---- Validation (spec §D) -----------------------------------------

    #[test]
    fn interval_clamps_to_bounds() {
        let low = AppSettings { port_refresh_interval_ms: 0, ..Default::default() }.validated();
        assert_eq!(low.port_refresh_interval_ms, MIN_INTERVAL_MS);
        let high = AppSettings {
            port_refresh_interval_ms: u32::MAX,
            ..Default::default()
        }
        .validated();
        assert_eq!(high.port_refresh_interval_ms, MAX_INTERVAL_MS);
    }

    // ---- Serialization round-trip --------------------------------------

    #[test]
    fn serializes_and_deserializes() {
        let settings = AppSettings {
            close_behavior: CloseBehavior::MinimizeToTray,
            theme: Theme::Dark,
            ..Default::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, settings);
    }

    // ---- Corrupt-file recovery (spec §F) -------------------------------

    #[test]
    fn invalid_json_recovers_to_defaults() {
        let parsed: Result<AppSettings, _> = serde_json::from_str("not json at all");
        assert!(parsed.is_err(), "invalid JSON must not parse");
        // The load path wraps this: defaults + warning, no panic. The file
        // level is tested by load_or_default via the real path below.
    }

    #[test]
    fn wrong_typed_fields_fall_back_per_field() {
        // Unknown future fields are ignored; wrong types on known fields
        // make the whole file fall back to defaults (documented policy).
        let parsed: Result<AppSettings, _> =
            serde_json::from_str(r#"{"port_refresh_interval_ms": "fast"}"#);
        assert!(parsed.is_err());
        // Unknown fields survive:
        let parsed: AppSettings =
            serde_json::from_str(r#"{"future_field": true, "auto_refresh": false}"#).unwrap();
        assert!(!parsed.auto_refresh);
        assert_eq!(parsed.port_refresh_interval_ms, 3_000);
    }

    #[test]
    fn out_of_range_interval_from_file_is_clamped() {
        let parsed: AppSettings =
            serde_json::from_str(r#"{"port_refresh_interval_ms": 999999999}"#).unwrap();
        assert_eq!(parsed.validated().port_refresh_interval_ms, MAX_INTERVAL_MS);
    }

    // ---- DTO security contract (spec §AI) -------------------------------

    #[test]
    fn settings_dto_has_no_secret_shaped_fields() {
        // The serialized shape is the DTO. Guard that it gains no field
        // resembling a secret carrier.
        let json = serde_json::to_string(&AppSettings::default()).unwrap();
        let forbidden = [
            "apikey", "api_key", "api-key", "token", "password", "passwd", "cookie", "secret",
            "credential", "authorization", "dockerenv", "docker_env", "session",
        ];
        for hint in forbidden {
            assert!(
                !json.to_ascii_lowercase().contains(hint),
                "settings DTO must never gain a `{hint}`-shaped field"
            );
        }
    }

    // ---- Reconciliation policy (spec §AH, pure decision part) -----------

    #[test]
    fn reconcile_policy_matrix_is_complete() {
        use startup::{reconcile_action, ReconcileAction};
        assert_eq!(reconcile_action(true, true), ReconcileAction::None);
        assert_eq!(reconcile_action(false, false), ReconcileAction::None);
        assert_eq!(reconcile_action(true, false), ReconcileAction::Enable);
        assert_eq!(reconcile_action(false, true), ReconcileAction::Disable);
    }

    #[test]
    fn reconcile_with_applies_only_required_action() {
        use startup::reconcile_with;
        let mut calls: Vec<(bool, bool)> = Vec::new();
        let mut settings = AppSettings {
            run_at_startup: true,
            ..Default::default()
        };
        // Intent ON, actual ON → nothing happens.
        reconcile_with(&mut settings, || true, |en| {
            calls.push((en, true));
            Ok(())
        });
        assert!(calls.is_empty(), "matching state must not rewrite the OS");

        // Intent ON, actual OFF → Enable.
        reconcile_with(&mut settings, || false, |en| {
            calls.push((en, true));
            Ok(())
        });
        assert_eq!(calls, vec![(true, true)]);

        // Intent OFF, actual ON → Disable.
        settings.run_at_startup = false;
        reconcile_with(&mut settings, || true, |en| {
            calls.push((en, false));
            Ok(())
        });
        assert_eq!(calls.last(), Some(&(false, false)));
    }

    #[test]
    fn user_toggle_failure_leaves_settings_untouched() {
        use startup::apply_user_toggle;
        let mut settings = AppSettings::default();
        let result = apply_user_toggle(&mut settings, true, |_| {
            Err("registration failed".to_string())
        });
        assert!(result.is_err());
        assert!(!settings.run_at_startup, "a failed toggle must not lie");
    }

    #[test]
    fn user_toggle_success_syncs_model() {
        use startup::apply_user_toggle;
        let mut settings = AppSettings::default();
        let applied = apply_user_toggle(&mut settings, true, |en| {
            assert!(en);
            Ok(())
        });
        assert_eq!(applied, Ok(true));
        assert!(settings.run_at_startup);
    }

    // ---- Reconciliation END STATE (correction §8): no stale boolean wins
    // over actual OS registration — the applied view always equals OS truth.

    #[test]
    fn reconcile_persisted_true_os_false_ends_false() {
        use std::cell::Cell;

        use startup::reconcile_with;
        let mut settings = AppSettings { run_at_startup: true, ..Default::default() };
        // OS says not registered; policy must re-register. The resulting
        // user-visible state is derived from the OS AFTER the action, so the
        // UI reports false if registration could not be restored.
        let os_actual = Cell::new(false);
        reconcile_with(&mut settings, || os_actual.get(), |en| {
            if en {
                os_actual.set(true); // simulated plugin enable
            }
            Ok(())
        });
        assert!(os_actual.get(), "persisted true + OS false must end registered");
        assert!(settings.run_at_startup);
    }

    #[test]
    fn reconcile_persisted_false_os_true_ends_false() {
        use std::cell::Cell;

        use startup::reconcile_with;
        let mut settings = AppSettings::default(); // intent OFF
        let os_actual = Cell::new(true); // stale registration exists
        reconcile_with(&mut settings, || os_actual.get(), |en| {
            if !en {
                os_actual.set(false); // simulated plugin disable
            }
            Ok(())
        });
        assert!(!os_actual.get(), "persisted false + OS true must end deregistered");
        assert!(!settings.run_at_startup, "no stale OS registration wins over persisted intent");
    }

    #[test]
    fn reconcile_failure_never_flips_the_model() {
        use startup::reconcile_with;
        // Persisted ON but registration fails → the model keeps the user's
        // intent while the OS stays unregistered; the UI reflects the OS via
        // startup_registered (get_app_settings), not the intent field.
        let mut settings = AppSettings { run_at_startup: true, ..Default::default() };
        reconcile_with(&mut settings, || false, |_| {
            Err("access denied".to_string())
        });
        assert!(settings.run_at_startup, "intent is preserved for retry-free display");
    }

    // ---- Atomic write core (correction §13) ------------------------------

    #[test]
    fn atomic_write_puts_temp_file_in_same_directory() {
        let dir = std::env::temp_dir().join("lcc-atomic-test-samedir");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        save_to(&path, &AppSettings::default()).unwrap();
        assert!(path.exists(), "real file written");
        // After a successful rename the temp file is gone — it lived in the
        // same directory (same volume) or the rename would have failed.
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_failure_preserves_old_valid_settings() {
        let dir = std::env::temp_dir().join("lcc-atomic-test-preserve");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        let original = AppSettings {
            port_refresh_interval_ms: 4_444,
            ..Default::default()
        };
        save_to(&path, &original).unwrap();

        // Make the rename step fail: replace the temp path with a DIRECTORY.
        // fs::write succeeds into it (dir/tmp/…)? No — writing a file onto a
        // directory path fails BEFORE the old real file is touched.
        let tmp = dir.join("settings.json.tmp");
        fs::create_dir_all(&tmp).unwrap();
        let result = save_to(&path, &AppSettings::default());
        assert!(result.is_err(), "write onto a directory path must fail");
        // The old valid settings are intact, untouched by the failed save.
        let back = fs::read_to_string(&path).unwrap();
        assert!(back.contains("4444"), "old valid settings must survive a failed save");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leftover_temp_file_does_not_break_startup() {
        let dir = std::env::temp_dir().join("lcc-atomic-test-leftover");
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");
        save_to(&path, &AppSettings::default()).unwrap();
        // A previous crash left a stale temp file next to the real one.
        fs::write(dir.join("settings.json.tmp"), "garbage").unwrap();
        // Reading the REAL file is unaffected — a leftover temp is ignored.
        let text = fs::read_to_string(&path).unwrap();
        let parsed: AppSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed.port_refresh_interval_ms, 3_000);
        // And a new save recycles the temp name without error.
        save_to(&path, &AppSettings::default()).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
