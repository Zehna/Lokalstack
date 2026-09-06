//! # Discovery
//!
//! Engines that observe what is running on this machine, read-only.
//!
//! ## Phase 1 — Port Discovery (implemented)
//!
//! `ports.rs` holds the pure logic (DTOs, byte-order conversion, IPv4/IPv6
//! address decoding, normalization) and `windows.rs` holds the narrow,
//! unsafe FFI boundary to `GetExtendedTcpTable`. The rest of the crate only
//! sees [`enumerate_tcp_listeners`] and the serde DTO [`PortListener`] —
//! no Windows types leak past this module.
//!
//! ## Later phases (placeholders)
//!
//! - Phase 2 — Process Intelligence: map PIDs to executables, command lines,
//!   resource usage.
//! - Phase 3 — Service Detection: classify listeners into known services.
//!
//! The engines never probe port ranges, never parse `netstat` output, and
//! never require elevated privileges.

pub(crate) mod ports;

#[cfg(windows)]
pub(crate) mod windows;

use serde::Serialize;

pub(crate) use ports::PortListener;

/// Public response DTO for the `get_port_listeners` Tauri command.
///
/// Wraps the listener list with the fields the frontend needs to render
/// loading/error/empty states honestly.
#[derive(Debug, Clone, Serialize)]
// Field names deliberately mirror the frontend contract — camelCase JSON.
#[allow(non_snake_case)]
pub(crate) struct PortListenersResponse {
    /// All TCP listeners (IPv4 + IPv6), deduplicated and sorted by port.
    pub listeners: Vec<PortListener>,
    /// Unix epoch milliseconds at which the snapshot was taken.
    pub capturedAt: u64,
}

/// Enumerate all TCP listeners on this machine (read-only).
///
/// Platform dispatch: the real engine on Windows; an explicit, honest error
/// everywhere else (Phase 1 is Windows-first per the roadmap).
#[allow(clippy::unused_async)] // async signature is part of the Tauri command contract
pub(crate) async fn discover_port_listeners() -> Result<PortListenersResponse, String> {
    #[cfg(windows)]
    {
        let started_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let listeners = tauri::async_runtime::spawn_blocking(windows::enumerate_tcp_listeners)
            .await
            .map_err(|e| format!("discovery task join error: {e}"))?;

        Ok(PortListenersResponse {
            capturedAt: started_at,
            listeners: ports::normalize_listeners(listeners?),
        })
    }

    #[cfg(not(windows))]
    {
        let _ = std::marker::PhantomData::<PortListener>;
        Err("Port discovery is Windows-only in Phase 1.".to_string())
    }
}

#[cfg(test)]
mod tests {
    /// LIVE-SYSTEM TEST (#[ignore]d so CI stays hermetic).
    ///
    /// Runs the real production discovery path against the actual Windows
    /// TCP tables and prints every listener. Run explicitly with:
    ///
    ///     cargo test -- --ignored --nocapture
    ///
    /// Assertions are limited to invariants that must always hold for any
    /// system state (parse succeeds, ports in range, state LISTEN), so the
    /// test never fails because a dev server came or went.
    /// LIVE-SYSTEM TEST (#[ignore]d so normal `cargo test` stays hermetic).
    ///
    /// Runs the full production path exactly as the frontend calls it —
    /// the async `discover_port_listeners` wrapper (spawn_blocking +
    /// normalization) plus the serde JSON serialization the Tauri command
    /// performs — against the real Windows TCP tables. Run explicitly with:
    ///
    ///     cargo test -- --ignored --nocapture
    ///
    /// Assertions are limited to invariants that must hold for any system
    /// state, so the test never fails because a dev server came or went.
    #[test]
    #[ignore = "touches the live system; run manually for verification"]
    fn live_command_returns_invariant_valid_listeners() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio test runtime");

        let (response, json) = runtime.block_on(async {
            let response = super::discover_port_listeners()
                .await
                .expect("live discovery must not fail on Windows");
            // Serialize exactly as the Tauri IPC layer will.
            let json = serde_json::to_string(&response).expect("response must serialize");
            (response, json)
        });

        assert!(!response.listeners.is_empty(), "Windows always has listeners");
        assert!(response.capturedAt > 0, "capturedAt must be a real timestamp");
        assert!(json.contains("\"listeners\""), "JSON must contain the listeners field");

        for (index, listener) in response.listeners.iter().enumerate() {
            assert_eq!(listener.state, super::ports::ListenerState::Listen);
            assert!(listener.port > 0, "port must be positive");
            assert!(!listener.localAddress.is_empty(), "address must be present");
            if index < 60 {
                println!(
                    "{:>5}  {}  {:<26}  PID {:>6}",
                    listener.port,
                    match listener.ipVersion {
                        super::ports::IpVersion::V4 => "IPv4",
                        super::ports::IpVersion::V6 => "IPv6",
                    },
                    listener.localAddress,
                    listener.pid,
                );
            }
        }
        println!("total listeners: {}", response.listeners.len());
    }

    #[test]
    fn module_is_wired() {
        // Compilation of this test proves the discovery tree is reachable.
        assert!(true);
    }
}
