//! Pure, OS-independent logic for the port-discovery engine.
//!
//! Everything in this file is deterministic and unit-testable without a
//! Windows handle: DTO definitions, network-byte-order conversion, IPv6
//! address decoding and list normalization/sorting. The unsafe FFI that calls
//! [`GetExtendedTcpTable`](windows_sys::Win32::NetworkManagement::IpHelper::GetExtendedTcpTable)
//! lives in [`super::windows`].

use serde::{Deserialize, Serialize};

/// `MIB_TCP_STATE_LISTEN` from `tcpmib.h` — the only state a row in the
/// listener table can have. Used as a defensive filter when parsing rows.
pub(crate) const MIB_TCP_STATE_LISTEN: u32 = 2;

/// Network transport protocol of a listener. Only TCP is discovered in
/// Phase 1; UDP has no listen state, so a dedicated enum member would be
/// dishonest. When a future phase adds UDP, extend this enum explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Protocol {
    Tcp,
}

/// IP protocol version of the socket's binding — serialized as the JSON
/// number `4` or `6`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum IpVersion {
    /// IPv4 — serializes as `4`.
    #[serde(rename = "4")]
    V4,
    /// IPv6 — serializes as `6`.
    #[serde(rename = "6")]
    V6,
}

/// Listener lifecycle state as reported by the OS table. Windows only ever
/// reports `LISTEN` in the listener tables we query, but the field exists so
/// the serialized shape is stable for the UI contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum ListenerState {
    Listen,
}

/// One listening TCP socket on this machine, exactly as Windows reports it.
///
/// This is the serde DTO that the `get_port_listeners` Tauri command returns
/// to the frontend. No Windows FFI types leak across the command boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
// Field names deliberately mirror the frontend contract (`domain.ts`), which
// uses camelCase — this struct *is* the JSON shape.
#[allow(non_snake_case)]
pub(crate) struct PortListener {
    /// Transport protocol — always `"tcp"` in Phase 1.
    pub protocol: Protocol,
    /// IP version of the binding: `4` or `6`.
    pub ipVersion: IpVersion,
    /// Bind address as a string: `127.0.0.1`, `0.0.0.0`, `::1`, `::`, ... .
    /// Loopback and wildcard bindings are kept; see [`normalize_listeners`].
    pub localAddress: String,
    /// Local TCP port in host byte order.
    pub port: u16,
    /// PID of the socket owner (may be 0 for system-owned sockets).
    pub pid: u32,
    /// Lifecycle state — always `"LISTEN"` in Phase 1.
    pub state: ListenerState,
}

/// Convert a TCP port from network byte order to host byte order.
///
/// The tables returned by `GetExtendedTcpTable` store `dwLocalPort` (and
/// `dwRemotePort`) in network byte order in the low 16 bits of the `DWORD`;
/// the high 16 bits are padding and must be masked off before converting.
#[must_use]
pub(crate) fn port_from_network_order(dword: u32) -> u16 {
    u16::from_be((dword & 0xFFFF) as u16)
}

/// Format an IPv4 address dword (as `dwLocalAddr`, bytes in network order)
/// in dotted-quad notation, e.g. `127.0.0.1`.
#[must_use]
pub(crate) fn ipv4_address_to_string(bytes: [u8; 4]) -> String {
    std::net::Ipv4Addr::from(bytes).to_string()
}

/// Decode a `byte[16]` address field from an IPv6 table row.
///
/// The raw bytes arrive in **in network order**; that order is preserved for
/// the textual representation, so e.g. `::1` renders as `::1` and a v4-mapped
/// address renders in its canonical dotted-quad form.
#[must_use]
pub(crate) fn ipv6_address_to_string(bytes: [u8; 16]) -> String {
    // RFC 4291 §2.5.5.1/2: ::w.x.y.z (IPv4-compatible) and ::ffff:w.x.y.z
    // (IPv4-mapped) have a conventional compressed form — use it when the
    // prefix matches so `::ffff:127.0.0.1` doesn't render as the full
    // 8-group form. Rust's `std::net::Ipv6Addr` implements exactly this.
    std::net::Ipv6Addr::from(bytes).to_string()
}

/// Deduplicate + deterministically sort listeners for stable UI rendering.
///
/// Windows can legitimately report the same (address, port, pid) tuple more
/// than once — duplicated rows are dropped here, but rows that share only the
/// **port** are kept: the same port may be bound on both `0.0.0.0` and
/// `::` (dual-stack), or on two different specific addresses, and those are
/// genuinely different sockets the user needs to see.
///
/// Order: port ascending, then address, then PID — stable across polls so the
/// table doesn't shuffle between refreshes.
#[must_use]
pub(crate) fn normalize_listeners(mut listeners: Vec<PortListener>) -> Vec<PortListener> {
    listeners.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then_with(|| a.localAddress.cmp(&b.localAddress))
            .then_with(|| b.pid.cmp(&a.pid))
        // Deterministic total order for identical (port, address) rows that
            // differ only by PID: higher PID wins the earlier slot.
    });

    listeners.dedup(); // removes *consecutive* duplicates only — sort above makes that correct
    listeners
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- port_from_network_order -----------------------------------------

    #[test]
    fn port_conversion_known_values() {
        // 3000 = 0x0BB8 → on the wire 0xB8, 0x0B.
        assert_eq!(port_from_network_order(0x0000_B80B), 3000);
        // 1420 = 0x058C → 0x8C, 0x05.
        assert_eq!(port_from_network_order(0x0000_8C05), 1420);
        // 65535 = 0xFFFF — symmetric byte pattern, sanity check.
        assert_eq!(port_from_network_order(0x0000_FFFF), 65535);
        // 8080 = 0x1F90 → 0x90, 0x1F.
        assert_eq!(port_from_network_order(0x0000_901F), 8080);
    }

    #[test]
    fn port_conversion_masks_upper_dword_bits() {
        // The upper 16 bits of the DWORD are padding and must be ignored:
        // same low word as 3000 (0x0BB8 → bytes B8 0B), garbage above.
        assert_eq!(port_from_network_order(0xABCD_B80B), 3000);
        assert_eq!(port_from_network_order(0xFFFF_B80B), 3000);
    }

    #[test]
    fn port_conversion_is_involution_on_host_order_values() {
        // Round-trip: host value → network bytes → back to host value.
        for port in [80u16, 3000, 1420, 5432, 8080, 65535] {
            let network = u16::to_be(port);
            let dword = network as u32; // high bits zero, as the OS writes it
            assert_eq!(port_from_network_order(dword), port);
        }
    }

    // --- ipv4_address_to_string ------------------------------------------

    #[test]
    fn ipv4_addresses_render_dotted_quad() {
        assert_eq!(ipv4_address_to_string([127, 0, 0, 1]), "127.0.0.1");
        assert_eq!(ipv4_address_to_string([0, 0, 0, 0]), "0.0.0.0");
        assert_eq!(ipv4_address_to_string([192, 168, 1, 5]), "192.168.1.5");
    }

    // --- ipv6_address_to_string ------------------------------------------

    #[test]
    fn ipv6_loopback_renders_compressed() {
        let mut bytes = [0u8; 16];
        bytes[15] = 1;
        assert_eq!(ipv6_address_to_string(bytes), "::1");
    }

    #[test]
    fn ipv6_unspecified_renders_double_colon() {
        assert_eq!(ipv6_address_to_string([0u8; 16]), "::");
    }

    #[test]
    fn ipv6_global_address_preserves_network_byte_order() {
        // 2001:db8::1 — network-order bytes as the OS hands them over.
        let mut bytes = [0u8; 16];
        bytes[0] = 0x20;
        bytes[1] = 0x01;
        bytes[2] = 0x0d;
        bytes[3] = 0xb8;
        bytes[15] = 1;
        assert_eq!(ipv6_address_to_string(bytes), "2001:db8::1");
    }

    #[test]
    fn ipv6_v4mapped_renders_dotted_quad() {
        // ::ffff:192.168.1.5 — network-order bytes as the OS hands them over.
        let mut bytes = [0u8; 16];
        bytes[10] = 0xff;
        bytes[11] = 0xff;
        bytes[12] = 192;
        bytes[13] = 168;
        bytes[14] = 1;
        bytes[15] = 5;
        assert_eq!(ipv6_address_to_string(bytes), "::ffff:192.168.1.5");
    }

    // --- normalize_listeners ---------------------------------------------

    fn listener(port: u16, address: &str, pid: u32, version: IpVersion) -> PortListener {
        PortListener {
            protocol: Protocol::Tcp,
            ipVersion: version,
            localAddress: address.to_string(),
            port,
            pid,
            state: ListenerState::Listen,
        }
    }

    #[test]
    fn normalization_sorts_by_port_then_address() {
        let listeners = vec![
            listener(8080, "127.0.0.1", 100, IpVersion::V4),
            listener(3000, "127.0.0.1", 200, IpVersion::V4),
            listener(3000, "0.0.0.0", 300, IpVersion::V4),
            listener(1420, "::1", 400, IpVersion::V6),
        ];

        let normalized = normalize_listeners(listeners);
        let ports: Vec<u16> = normalized.iter().map(|l| l.port).collect();
        assert_eq!(ports, vec![1420, 3000, 3000, 8080]);
        // Same port, different addresses: 0.0.0.0 sorts before 127.0.0.1.
        assert_eq!(normalized[1].localAddress, "0.0.0.0");
        assert_eq!(normalized[2].localAddress, "127.0.0.1");
        // IPv4/IPv6 mix is preserved, not collapsed.
        assert_eq!(normalized[0].ipVersion, IpVersion::V6);
    }

    #[test]
    fn normalization_keeps_dual_stack_duplicates() {
        // Same port on 0.0.0.0 and :: is TWO different sockets — both stay.
        let listeners = vec![
            listener(5432, "0.0.0.0", 100, IpVersion::V4),
            listener(5432, "::", 100, IpVersion::V6),
        ];
        let normalized = normalize_listeners(listeners);
        assert_eq!(normalized.len(), 2);
    }

    #[test]
    fn normalization_drops_exact_duplicates_only() {
        let listeners = vec![
            listener(3000, "127.0.0.1", 42, IpVersion::V4),
            listener(3000, "127.0.0.1", 42, IpVersion::V4),
            listener(3000, "127.0.0.1", 42, IpVersion::V4),
        ];
        let normalized = normalize_listeners(listeners);
        assert_eq!(normalized.len(), 1);
    }

    #[test]
    fn normalization_is_stable_across_input_orders() {
        let a = vec![
            listener(8080, "127.0.0.1", 100, IpVersion::V4),
            listener(3000, "0.0.0.0", 200, IpVersion::V4),
            listener(1420, "::1", 300, IpVersion::V6),
        ];
        let b: Vec<PortListener> = a.iter().rev().cloned().collect();

        assert_eq!(normalize_listeners(a), normalize_listeners(b));
    }
}
