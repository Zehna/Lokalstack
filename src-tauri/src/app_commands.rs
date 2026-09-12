//! Phase 10C — narrow app-level commands: settings read/save/reset and the
//! Windows startup toggle. This module is the ONLY place the frontend can
//! touch settings or startup registration; there is no generic filesystem
//! writer, no arbitrary registry access, and no secret can pass through —
//! the DTO is a fixed struct of operational preferences (guarded by tests
//! in `settings.rs` and the static safety guards).

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::settings::{self, startup, AppSettings};

/// Serialized settings view. Field names match the existing camelCase DTO
/// convention used across the Tauri boundary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsView {
    auto_refresh: bool,
    port_refresh_interval_ms: u32,
    ai_polling_enabled: bool,
    docker_polling_enabled: bool,
    launch_minimized: bool,
    close_behavior: &'static str,
    run_at_startup: bool,
    /// Startup registration as the OS reports it right now (spec §U, §AH):
    /// the UI reflects reality, not just persisted intent.
    startup_registered: bool,
    theme: &'static str,
}

impl SettingsView {
    /// Build the view from settings plus the actual OS registration state
    /// (the UI reflects reality, spec §U/§AH).
    fn build(s: &AppSettings, startup_registered: bool) -> Self {
        Self {
            auto_refresh: s.auto_refresh,
            port_refresh_interval_ms: s.port_refresh_interval_ms,
            ai_polling_enabled: s.ai_polling_enabled,
            docker_polling_enabled: s.docker_polling_enabled,
            launch_minimized: s.launch_minimized,
            close_behavior: match s.close_behavior {
                settings::CloseBehavior::Exit => "exit",
                settings::CloseBehavior::MinimizeToTray => "minimize_to_tray",
            },
            run_at_startup: s.run_at_startup,
            startup_registered,
            theme: match s.theme {
                settings::Theme::System => "system",
                settings::Theme::Light => "light",
                settings::Theme::Dark => "dark",
            },
        }
    }
}

/// The persisted close-behavior enum from a frontend string.
impl AppSettings {
    fn set_close_behavior(&mut self, value: &str) {
        self.close_behavior = match value {
            "minimize_to_tray" => settings::CloseBehavior::MinimizeToTray,
            _ => settings::CloseBehavior::Exit,
        };
    }

    fn set_theme(&mut self, value: &str) {
        self.theme = match value {
            "light" => settings::Theme::Light,
            "dark" => settings::Theme::Dark,
            _ => settings::Theme::System,
        };
    }
}

/// Global app settings state — loaded once at startup, mutated only through
/// the commands below. A `Mutex` (not `RwLock`) is fine: settings mutations
/// are rare, short, and never hold the lock during I/O beyond the atomic
/// settings-file write itself.
pub(crate) struct SettingsState {
    pub(crate) settings: std::sync::Mutex<AppSettings>,
}

impl SettingsState {
    pub(crate) fn load() -> Self {
        Self {
            settings: std::sync::Mutex::new(settings::load_or_default()),
        }
    }
}

/// Read the current settings + actual startup-registration state.
#[tauri::command]
pub(crate) fn get_app_settings(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
) -> SettingsView {
    let guard = state.settings.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    SettingsView::build(&guard, startup::is_registered_from(&app))
}

/// In-flight settings payload from the frontend. Deliberately opaque: the
/// backend validates and clamps every field — the frontend can never store
/// an out-of-range interval or invent a new close behavior.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct SettingsPatch {
    auto_refresh: bool,
    port_refresh_interval_ms: u32,
    ai_polling_enabled: bool,
    docker_polling_enabled: bool,
    launch_minimized: bool,
    close_behavior: String,
    theme: String,
}

impl Default for SettingsPatch {
    fn default() -> Self {
        Self {
            auto_refresh: true,
            port_refresh_interval_ms: settings::MIN_INTERVAL_MS,
            ai_polling_enabled: true,
            docker_polling_enabled: true,
            launch_minimized: false,
            close_behavior: "exit".to_string(),
            theme: "system".to_string(),
        }
    }
}

/// Outcome of a save: the validated settings (clamped where needed) plus a
/// note when a value had to be corrected, so the UI can say "Invalid value
/// corrected" instead of a raw error (spec §AC).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveSettingsResult {
    settings: SettingsView,
    /// Human-safe note when a field was clamped/corrected; null otherwise.
    corrected_note: Option<String>,
}

impl SaveSettingsResult {
    fn build(settings: SettingsView, corrected_note: Option<String>) -> Self {
        Self {
            settings,
            corrected_note,
        }
    }
}

/// Save settings: validate + clamp, persist atomically, and return the
/// effective (post-validation) view. Saving never touches startup
/// registration — that is the dedicated toggle command, so a malformed
/// save can never flip a machine-level registration by accident.
#[tauri::command]
pub(crate) fn save_app_settings(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    patch: SettingsPatch,
) -> Result<SaveSettingsResult, String> {
    let mut guard = state
        .settings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let mut updated = AppSettings {
        auto_refresh: patch.auto_refresh,
        port_refresh_interval_ms: patch.port_refresh_interval_ms,
        ai_polling_enabled: patch.ai_polling_enabled,
        docker_polling_enabled: patch.docker_polling_enabled,
        launch_minimized: patch.launch_minimized,
        ..guard.clone()
    };
    updated.set_close_behavior(&patch.close_behavior);
    updated.set_theme(&patch.theme);

    let pre = updated.clone();
    updated = updated.validated();
    let corrected =
        if updated.port_refresh_interval_ms != pre.port_refresh_interval_ms {
            Some(format!(
                "Refresh interval must be between {} and {} ms — the value was adjusted.",
                settings::MIN_INTERVAL_MS,
                settings::MAX_INTERVAL_MS
            ))
        } else {
            None
        };

    settings::save(&updated)?;
    *guard = updated;
    let view = SettingsView::build(&guard, startup::is_registered_from(&app));
    drop(guard);
    crate::diagnostics::info("settings", "settings saved");
    Ok(SaveSettingsResult::build(view, corrected))
}

/// Reset settings to defaults (explicit user action; UI confirms first).
/// Startup registration is intentionally NOT modified by reset — the user
/// toggles it separately, so reset can never silently deregister.
#[tauri::command]
pub(crate) fn reset_app_settings(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
) -> SettingsView {
    let mut guard = state
        .settings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = settings::reset();
    SettingsView::build(&guard, startup::is_registered_from(&app))
}

/// Enable or disable run-at-Windows-startup (explicit user opt-in).
/// Returns the resulting OS-registered state so the UI reflects reality.
#[tauri::command]
pub(crate) fn set_run_at_startup(
    app: AppHandle,
    state: tauri::State<'_, SettingsState>,
    enabled: bool,
) -> Result<SettingsView, String> {
    let mut guard = state
        .settings
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // Bridge to the autostart plugin (HKCU run key, no admin).
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = startup::apply_user_toggle(&mut guard, enabled, |en| {
        if en {
            manager.enable().map_err(|err| err.to_string())
        } else {
            manager.disable().map_err(|err| err.to_string())
        }
    });

    match result {
        Ok(_) => {
            settings::save(&guard).map_err(|err| {
                crate::diagnostics::warn("settings", &format!("startup toggle persist failed: {err}"));
                err
            })?;
            let view = SettingsView::build(&guard, startup::is_registered_from(&app));
            drop(guard);
            crate::diagnostics::info(
                "startup",
                if enabled { "startup registration enabled by user" } else { "startup registration disabled by user" },
            );
            Ok(view)
        }
        Err(err) => {
            // Failure is local + reported (spec §U): no crash, no retry loop.
            crate::diagnostics::warn("startup", &format!("startup toggle failed: {err}"));
            Err(err)
        }
    }
}

/// Actual startup registration state as the OS reports it (reconciliation
/// probe for the UI, spec §AH).
#[tauri::command]
pub(crate) fn get_startup_registered(app: AppHandle) -> bool {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pure logic tests for the patch application (no Tauri state needed).

    #[test]
    fn close_behavior_and_theme_parse_conservatively() {
        let mut s = AppSettings::default();
        s.set_close_behavior("minimize_to_tray");
        assert_eq!(s.close_behavior, settings::CloseBehavior::MinimizeToTray);
        // Unknown values fall back to Exit — never a surprise tray-only mode.
        s.set_close_behavior("garbage");
        assert_eq!(s.close_behavior, settings::CloseBehavior::Exit);

        s.set_theme("dark");
        assert_eq!(s.theme, settings::Theme::Dark);
        s.set_theme("nonsense");
        assert_eq!(s.theme, settings::Theme::System);
    }

    #[test]
    fn settings_patch_defaults_are_safe() {
        let d = SettingsPatch::default();
        assert!(d.auto_refresh);
        assert_eq!(d.port_refresh_interval_ms, settings::MIN_INTERVAL_MS);
        assert_eq!(d.close_behavior, "exit");
        assert_eq!(d.theme, "system");
    }
}
