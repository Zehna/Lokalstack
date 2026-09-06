//! Rust backend for LocalStack Control Center.
//!
//! Phase 1 adds real, read-only Windows TCP listener discovery:
//! `src/discovery/` owns the engine (`ports.rs` pure logic + `windows.rs`
//! FFI), exposed to the frontend through the `get_port_listeners` Tauri
//! command. Later engines (process intelligence, health, control, conflicts,
//! workspaces, AI) live in the feature modules below and remain placeholders.

mod ai;
mod conflicts;
mod control;
mod discovery;
mod health;
mod workspace;

use discovery::discover_port_listeners;

/// Simple sample command proving the Tauri command boundary works end to end.
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Read-only Tauri command: enumerate every TCP listener currently bound on
/// this machine (IPv4 + IPv6), with owning PID and bind address.
///
/// Frontend contract: resolves to `PortListenersResponse`
/// (`{ listeners: [...], capturedAt: epochMillis }`), rejects with a
/// human-readable error string. Never mutates system state.
#[tauri::command]
async fn get_port_listeners(
) -> Result<discovery::PortListenersResponse, String> {
    discover_port_listeners().await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![greet, get_port_listeners])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
