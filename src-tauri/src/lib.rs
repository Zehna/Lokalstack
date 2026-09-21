//! Rust backend for LocalStack Control Center.
//!
//! Phase 6 adds managed workspaces: LocalStack launches development
//! services itself from **backend-derived launch specs** (referenced by
//! opaque ids), tracks them in a managed-process registry with known
//! Windows process groups, captures bounded logs, and provides targeted
//! graceful stop / restart for managed services only. External processes
//! keep the Phase 5 hardened control path. The pipeline lives in
//! `src/workspace/` on top of `src/project/` (manifests),
//! `src/discovery/` (port preflight), and `src/control/` (registry
//! primitives + external control). Phase 7 adds `src/conflicts/` (port
//! conflict engine + free-port finder) and `src/dependencies/`
//! (dependency graph, readiness, root causes). Phase 8 adds `src/ai/`
//! (runtime observability); Phase 9 `src/docker/`. Phase 10C adds
//! `src/settings.rs` + `src/app_commands.rs` (settings, tray, single
//! instance, startup, close behavior).

mod ai;
mod app_commands;
mod conflicts;
mod control;
mod diagnostics;
mod docker;
mod dependencies;
mod discovery;
mod health;
mod intelligence;
#[cfg(test)]
mod perf_baseline;
mod process;
mod settings;
mod project;
#[cfg(test)]
mod resilience_stress;
#[cfg(test)]
mod safety_guards;
mod workspace;

use tauri::{
    Manager,
    RunEvent,
    WindowEvent,
};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_single_instance::init as single_instance_init;

/// Show, restore (if minimized), and focus the main window. Used by both
/// the tray "Open" item and the second-instance handler — never creates a
/// second window (spec §M, §W).
fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Order matters: un-minimize first so show+focus land visibly.
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Canonical tray identity for Windows accessibility (Phase 11B2): Phase 11A
/// live QA found the notification-area icon exposed no useful accessible
/// name. On Windows the tray tooltip is what UIA/screen readers announce as
/// the icon's name. Must stay in lockstep with the product identity — a
/// regression test asserts equality with the configured package name.
const TRAY_TOOLTIP_LABEL: &str = "LocalStack Control Center";

/// Build the tray icon with the small operational menu (spec §L):
/// Open / Refresh / separator / Exit. No destructive items — tray menus
/// never start, stop, or kill anything (spec §AM).
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;

    let open = MenuItem::with_id(app, "open", "Open LocalStack", true, None::<&str>)?;
    let refresh = MenuItem::with_id(app, "refresh", "Refresh", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &refresh, &quit])?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().cloned().ok_or_else(|| {
            tauri::Error::AssetNotFound("default window icon".into())
        })?)
        .tooltip(TRAY_TOOLTIP_LABEL)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            match event.id().as_ref() {
                "open" => show_main_window(app),
                // Read-only operational refresh: reuses the same safe
                // discovery cycle the frontend Refresh button uses. It
                // cannot stop/kill/modify anything (spec §N).
                "refresh" => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        // Same cycle the frontend uses; errors are swallowed
                        // here (the next frontend poll renders the state).
                        let _ = process::get_port_listeners(
                            app.state::<process::ProcessEngineState>(),
                            app.state::<project::ProjectEngineState>(),
                            app.state::<control::ControlEngineState>(),
                            None,
                        )
                        .await;
                    });
                }
                // Exit terminates LocalStack itself only. Managed services,
                // external processes, containers, and AI runtimes are NOT
                // touched (spec §O, §AL, §AM).
                "exit" => {
                    diagnostics::info("tray", "exit requested from tray");
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            // Left-click on the icon (not the menu) also opens the window.
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Pure close-behavior decision (spec §P) — testable without a window.
enum CloseDecision {
    /// Hide the window; app keeps running (minimize-to-tray).
    Hide,
    /// Let the default close proceed (exit).
    Exit,
}

fn close_decision(close_behavior: settings::CloseBehavior) -> CloseDecision {
    match close_behavior {
        settings::CloseBehavior::MinimizeToTray => CloseDecision::Hide,
        settings::CloseBehavior::Exit => CloseDecision::Exit,
    }
}

/// Simple sample command proving the Tauri command boundary works end to end.
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Narrow startup argument (spec §T): `--startup` marks a Windows
    // startup launch so launch-minimized is honored. No other argument is
    // interpreted; there is no generic CLI execution feature.
    let launched_at_startup = std::env::args().any(|arg| arg == "--startup");

    tauri::Builder::default()
        .manage(process::ProcessEngineState::default())
        .manage(project::ProjectEngineState::default())
        .manage(control::ControlEngineState::default())
        .manage(workspace::WorkspaceEngineState::default())
        .manage(ai::registry::AiEngineState::default())
        .manage(docker::DockerEngineState::real())
        .manage(app_commands::SettingsState::load())
        .plugin(single_instance_init(|app, _args, _cwd| {
            // Second instance: signal the first (show + focus) and exit.
            // It must NOT re-adopt managed processes, start workspaces,
            // reset settings, or create another tray icon (spec §W).
            diagnostics::info("single-instance", "second launch detected; focusing first instance");
            show_main_window(app);
        }))
        .plugin(tauri_plugin_dialog::init()) // Rust-side save dialog only (Task 13)
        .plugin(tauri_plugin_opener::init()) // Rust-side folder reveal only (Task 13)
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            // Extra args applied when launched at startup: marks the
            // launch so launch-minimized can be honored (spec §T).
            Some(vec!["--startup"]),
        ))
        .setup(move |app| {
            // Phase 10B (§X) + Phase 11C (§6): local-only panic diagnostics,
            // installed before any subsystem work. No telemetry, no network.
            // The emergency destination is prepared ONCE here (normal runtime);
            // if unavailable, the hook degrades to the redacted breadcrumb.
            {
                let prepared = diagnostics::emergency::prepare_once();
                diagnostics::emergency::install(diagnostics::cache::global(), prepared.ok());
            }
            // Phase 11C (§7): finalize any pending crash record BEFORE the
            // window is shown. Never blocks startup; outcome banner is
            // surfaced to the frontend via the diagnostics commands (Task 14).
            {
                let app_version = env!("CARGO_PKG_VERSION").to_string();
                if let Some(deps) = diagnostics::recovery::production_deps(
                    app_version,
                    Box::new(|outcome| {
                        if let Some(id) = &outcome.recovered_bundle_id {
                            diagnostics::info(
                                "recovery",
                                &format!("crash record finalized into bundle (pending UI notice)"),
                            );
                            let _ = id;
                        }
                    }),
                ) {
                    let outcome = diagnostics::recovery::finalize_pending(deps);
                    if matches!(outcome.banner, diagnostics::recovery::RecoveryBanner::Recovered) {
                        diagnostics::info("recovery", "previous crash recovered into bundle");
                    }
                    // Surface the recovery outcome to the UI via the
                    // diagnostics state (Task 14 command `get_diagnostics_overview`).
                    if let Ok(mut slot) = diagnostics::commands::recovery_banner_slot().lock() {
                        *slot = match outcome.banner {
                            diagnostics::recovery::RecoveryBanner::Recovered => {
                                Some("A previous crash was recovered into a support bundle.".into())
                            }
                            diagnostics::recovery::RecoveryBanner::RecoveryFailed => {
                                Some("Diagnostics recovery failed after repeated attempts.".into())
                            }
                            _ => None,
                        };
                    }
                }
            }
            // Phase 11C Task 14: diagnostics application state (incident
            // index, bundle registry, capture worker, export capabilities).
            // Init AFTER recovery so the capture worker sees post-recovery
            // storage state.
            app.manage(diagnostics::commands::DiagnosticsState::init(app.handle()));
            diagnostics::info("startup", "LocalStack Control Center starting");

            // Phase 10C startup order (spec §AK): settings → reconciliation
            // → visibility → normal work.
            {
                let state = app.state::<app_commands::SettingsState>();
                let mut guard = state
                    .settings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                // Reconcile persisted startup intent against OS reality so
                // the UI never lies (spec §AH). Failures are logged, never
                // fatal, never retried in a loop (spec §U).
                {
                    use tauri_plugin_autostart::ManagerExt;
                    let manager = app.autolaunch();
                    settings::startup::reconcile_with(&mut guard, || {
                        manager.is_enabled().unwrap_or(false)
                    }, |en| {
                        if en {
                            manager.enable().map_err(|err| err.to_string())
                        } else {
                            manager.disable().map_err(|err| err.to_string())
                        }
                    });
                }
                // Tray BEFORE the visibility decision: if launch-minimized
                // is set, the tray is the recovery path and must exist first.
                // If the tray fails, fall back to showing the window — the
                // user must never end up with an invisible app (spec §R, §AR).
                if let Err(err) = build_tray(app.handle()) {
                    diagnostics::error("tray", &format!("tray initialization failed: {err}"));
                    // Fall through: window stays visible (safe default).
                } else {
                    let launch_hidden = guard.launch_minimized || launched_at_startup && guard.launch_minimized;
                    if launch_hidden {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.hide();
                            diagnostics::info("startup", "launched minimized (tray available)");
                        }
                    }
                }
            }

            // Readiness/exit monitor for managed processes (idempotent).
            app.state::<workspace::WorkspaceEngineState>().spawn_monitor();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            process::get_port_listeners,
            process::end_process,
            process::open_service_url,
            workspace::get_workspace_candidates,
            workspace::create_workspace,
            workspace::remove_workspace,
            workspace::list_workspaces,
            workspace::start_managed_service,
            workspace::stop_managed_service,
            workspace::restart_managed_service,
            workspace::start_workspace_services,
            workspace::stop_workspace_services,
            workspace::get_service_logs,
            conflicts::evaluate_port,
            conflicts::find_free_ports,
            dependencies::get_workspaces_readiness,
            docker::get_docker_snapshot,
            docker::refresh_docker,
            docker::get_container_details,
            dependencies::list_dependency_targets,
            dependencies::add_workspace_dependency,
            dependencies::remove_workspace_dependency,
            ai::registry::get_ai_runtimes,
            app_commands::get_app_settings,
            app_commands::save_app_settings,
            app_commands::reset_app_settings,
            app_commands::set_run_at_startup,
            app_commands::get_startup_registered,
            diagnostics::commands::get_diagnostics_overview,
            diagnostics::commands::run_deep_health_checks,
            diagnostics::commands::list_incidents,
            diagnostics::commands::mark_incident_reviewed,
            diagnostics::commands::list_support_bundles,
            diagnostics::commands::get_support_bundle_detail,
            diagnostics::commands::export_support_bundle,
            diagnostics::commands::delete_support_bundle,
            diagnostics::commands::open_diagnostics_folder,
            diagnostics::commands::get_support_summary,
            diagnostics::commands::prepare_localstack_github_issue,
            diagnostics::commands::update_diagnostics_context,
            diagnostics::commands::reveal_export_result
        ])
        .on_window_event(|window, event| {
            // Close behavior (spec §P, §Q): window lifecycle is independent
            // of service lifecycle. Hiding or exiting NEVER stops managed
            // services, external processes, containers, or AI runtimes.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let state = app.state::<app_commands::SettingsState>();
                let guard = state
                    .settings
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                match close_decision(guard.close_behavior) {
                    CloseDecision::Hide => {
                        api.prevent_close();
                        let _ = window.hide();
                        diagnostics::info("window", "close requested; hidden to tray");
                    }
                    CloseDecision::Exit => {
                        diagnostics::info("window", "close requested; exiting");
                        // Default close proceeds; app exits normally.
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // Process-level exit stays dumb by design: no cleanup kills
            // managed/external services on exit (spec §AL, §37/§U Phase 6).
            if let RunEvent::Exit = event {
                diagnostics::info("shutdown", "LocalStack exited");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Close decision (spec §P) --------------------------------------

    #[test]
    fn close_decision_follows_setting() {
        assert!(matches!(
            close_decision(settings::CloseBehavior::Exit),
            CloseDecision::Exit
        ));
        assert!(matches!(
            close_decision(settings::CloseBehavior::MinimizeToTray),
            CloseDecision::Hide
        ));
    }

    // ---- Startup argument (spec §T) -------------------------------------

    #[test]
    fn startup_arg_detection_is_exact() {
        // Only the exact `--startup` flag counts.
        let args = vec!["app.exe".to_string(), "--startup".to_string()];
        assert!(args.iter().any(|a| a == "--startup"));
        let args = vec!["app.exe".to_string(), "--startup-extra".to_string()];
        assert!(!args.iter().any(|a| a == "--startup"));
    }

    // ---- Tray identity (Phase 11B2 accessibility) -----------------------

    // Phase 11A Windows QA found the notification-area icon exposed a null
    // accessible name (no tooltip set). The tray must label itself with the
    // canonical product identity so screen readers/UIA can announce it.
    // The real Windows exposure is verified live (tooltip → accName); this
    // test pins the canonical label that `build_tray` must consume.
    #[test]
    fn tray_tooltip_label_is_the_canonical_product_identity() {
        assert!(!TRAY_TOOLTIP_LABEL.trim().is_empty());
        assert_eq!(TRAY_TOOLTIP_LABEL, "LocalStack Control Center");
        // Stay in lockstep with the configured product identity.
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        assert_eq!(TRAY_TOOLTIP_LABEL, ctx.package_info().name);
    }
}
