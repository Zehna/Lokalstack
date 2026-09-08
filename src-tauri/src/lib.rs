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
//! (dependency graph, readiness, root causes). Placeholder modules
//! (`health`, `ai`) await later phases.

mod ai;
mod conflicts;
mod control;
mod dependencies;
mod discovery;
mod health;
mod intelligence;
mod process;
mod project;
mod workspace;

/// Simple sample command proving the Tauri command boundary works end to end.
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(process::ProcessEngineState::default())
        .manage(project::ProjectEngineState::default())
        .manage(control::ControlEngineState::default())
        .manage(workspace::WorkspaceEngineState::default())
        .setup(|app| {
            // Readiness/exit monitor for managed processes (idempotent).
            use tauri::Manager;
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
            dependencies::list_dependency_targets,
            dependencies::add_workspace_dependency,
            dependencies::remove_workspace_dependency
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
