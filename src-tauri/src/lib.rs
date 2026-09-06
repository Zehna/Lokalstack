//! Rust backend for LocalStack Control Center.
//!
//! Phase 2 adds process intelligence on top of Phase 1's port discovery:
//! the `get_port_listeners` Tauri command now returns TCP listeners **and**
//! their owning processes' metadata (name, executable path, start time,
//! working-set memory, delta-sampled CPU percentage). The pipeline lives in
//! `src/process/` (sampling + FFI) on top of `src/discovery/` (TCP tables).
//! Later engines (health, control, conflicts, workspaces, AI) remain
//! placeholders in their feature modules.

mod ai;
mod conflicts;
mod control;
mod discovery;
mod health;
mod process;
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
        .invoke_handler(tauri::generate_handler![
            greet,
            process::get_port_listeners
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
