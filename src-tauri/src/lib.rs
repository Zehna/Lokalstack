//! Rust backend for LocalStack Control Center.
//!
//! Phase 5 (hardened) adds safe local service control on top of the
//! read-only engines: `end_process` acts only on **opaque control-target
//! ids** issued by the backend's in-memory registry, with identity
//! revalidation and policy recomputation at action time. The pipeline lives
//! in `src/control/` (registry + rules + FFI) on top of `src/process/`
//! (inspection), `src/discovery/` (TCP tables), `src/intelligence/`
//! (service identity), and `src/project/` (project identity). The remaining
//! placeholder modules (`health`, `conflicts`, `workspace`, `ai`) await
//! later phases.

mod ai;
mod conflicts;
mod control;
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
        .invoke_handler(tauri::generate_handler![
            greet,
            process::get_port_listeners,
            process::end_process,
            process::open_service_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
