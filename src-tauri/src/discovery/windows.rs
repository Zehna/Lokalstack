//! Windows-specific TCP listener enumeration via `GetExtendedTcpTable`.
//!
//! # Safety and scope boundaries
//!
//! - All `unsafe` FFI in this crate is confined to this module.
//! - The module does exactly three things: size the output buffer, call
//!   [`GetExtendedTcpTable`], and copy raw table rows into [`PortListener`]
//!   DTOs. There is no port probing, no `netstat` parsing, and no other OS
//!   surface here — the module is entirely read-only.
//! - IPv4 and IPv6 live in separate OS tables (`AF_INET` / `AF_INET6`), so
//!   both are queried and merged.
//!
//! Reference: [`GetExtendedTcpTable`](https://learn.microsoft.com/en-us/windows/win32/api/iphlpapi/nf-iphlpapi-getextendedtcptable)
//! in the Microsoft Win32 documentation — including the documented two-call
//! pattern (probe for size, then query).

use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_MODULE, MIB_TCP6TABLE_OWNER_MODULE,
    MIB_TCPROW_OWNER_MODULE, MIB_TCPTABLE_OWNER_MODULE, TCP_TABLE_OWNER_MODULE_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

use super::ports::{
    ipv4_address_to_string, ipv6_address_to_string, port_from_network_order, IpVersion,
    ListenerState, PortListener, Protocol,
};

/// Offset of the first row inside a raw table buffer.
///
/// The wire format IS the C struct: `dwNumEntries` (4 bytes) followed by the
/// row array, which is 8-byte-aligned because rows contain an `i64` — so
/// there are 4 bytes of padding after the header. Deriving the offset from
/// the real struct (instead of assuming 4) is what keeps this correct.
const V4_ENTRIES_OFFSET: usize = std::mem::offset_of!(MIB_TCPTABLE_OWNER_MODULE, table);
const V6_ENTRIES_OFFSET: usize = std::mem::offset_of!(MIB_TCP6TABLE_OWNER_MODULE, table);

/// Read the TCP listener table for one address family.
///
/// Uses the documented two-call pattern: the first call passes a null buffer
/// and fails with `ERROR_INSUFFICIENT_BUFFER` while writing the required
/// size into `size`; the second call fills a buffer of exactly that size.
fn query_tcp_table(family: u16) -> Result<Vec<PortListener>, String> {
    let mut size: u32 = 0;

    // Call 1 — sizing probe. NULL buffer + 0 size ⇒ ERROR_INSUFFICIENT_BUFFER
    // with the required byte count written to `size`.
    // SAFETY: `size` is a valid out-param; the null buffer is the documented
    // way to request the size.
    let probe = unsafe {
        GetExtendedTcpTable(
            std::ptr::null_mut(),
            &mut size,
            0, // bOrder: no — we sort deterministically ourselves
            family.into(),
            TCP_TABLE_OWNER_MODULE_LISTENER,
            0,
        )
    };
    if probe != ERROR_INSUFFICIENT_BUFFER {
        return Err(format!(
            "GetExtendedTcpTable (family {family}) sizing probe returned {probe}, expected ERROR_INSUFFICIENT_BUFFER (122)"
        ));
    }
    if size == 0 {
        return Err(format!(
            "GetExtendedTcpTable (family {family}) reported zero required buffer size"
        ));
    }

    // Zero-initialized so any padding the API doesn't write is defined.
    let mut buffer: Vec<u8> = vec![0u8; size as usize];
    let mut returned_size: u32 = size;

    // Call 2 — the real query into the sized buffer.
    // SAFETY: `buffer` is alive and `size` bytes long for the duration of the
    // call; the API writes at most `size` bytes and reports how many in
    // `returned_size`.
    let result = unsafe {
        GetExtendedTcpTable(
            buffer.as_mut_ptr().cast(),
            &mut returned_size,
            0,
            family.into(),
            TCP_TABLE_OWNER_MODULE_LISTENER,
            0,
        )
    };
    if result != NO_ERROR {
        return Err(format!(
            "GetExtendedTcpTable (family {family}) failed with error {result}"
        ));
    }
    if (returned_size as usize) > buffer.len() {
        return Err(format!(
            "GetExtendedTcpTable (family {family}) reported {returned_size} bytes, buffer holds {}",
            buffer.len()
        ));
    }

    let valid = &buffer[..returned_size as usize];
    match family {
        AF_INET => parse_v4_table(valid),
        AF_INET6 => parse_v6_table(valid),
        other => Err(format!("unsupported address family {other}")),
    }
}

/// Parse a raw IPv4 `MIB_TCPTABLE_OWNER_MODULE` buffer into DTOs.
///
/// The API wrote a `dwNumEntries` header followed by that many fixed-size
/// rows. Only `LISTEN` rows are accepted (the listener table should contain
/// nothing else, but the filter is defensive).
fn parse_v4_table(buffer: &[u8]) -> Result<Vec<PortListener>, String> {
    let header_len = V4_ENTRIES_OFFSET;
    let row_len = std::mem::size_of::<MIB_TCPROW_OWNER_MODULE>();
    if buffer.len() < header_len {
        return Err(format!(
            "IPv4 TCP table buffer too small: {} bytes, need {header_len}",
            buffer.len()
        ));
    }

    let entry_count = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let needed = header_len + entry_count * row_len;
    if buffer.len() < needed {
        return Err(format!(
            "IPv4 TCP table buffer too small for {entry_count} entries: {} bytes, need {needed}",
            buffer.len()
        ));
    }

    let mut listeners = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let offset = header_len + index * row_len;
        let row_bytes = &buffer[offset..offset + row_len];

        let dw_state = u32::from_ne_bytes(row_bytes[0..4].try_into().expect("4 bytes"));
        if dw_state != super::ports::MIB_TCP_STATE_LISTEN {
            continue;
        }
        let dw_local_addr = u32::from_ne_bytes(row_bytes[4..8].try_into().expect("4 bytes"));
        let dw_local_port = u32::from_ne_bytes(row_bytes[8..12].try_into().expect("4 bytes"));
        // Row layout: state(0) localAddr(4) localPort(8) remoteAddr(12)
        // remotePort(16) pid(20) — the assertion at the bottom of this file
        // pins these offsets at compile time.
        let dw_pid = u32::from_ne_bytes(row_bytes[20..24].try_into().expect("4 bytes"));

        listeners.push(PortListener {
            protocol: Protocol::Tcp,
            ipVersion: IpVersion::V4,
            localAddress: ipv4_address_to_string(dw_local_addr.to_ne_bytes()),
            port: port_from_network_order(dw_local_port),
            pid: dw_pid,
            state: ListenerState::Listen,
        });
    }
    Ok(listeners)
}

/// Parse a raw IPv6 `MIB_TCP6TABLE_OWNER_MODULE` buffer into DTOs.
fn parse_v6_table(buffer: &[u8]) -> Result<Vec<PortListener>, String> {
    let header_len = V6_ENTRIES_OFFSET;
    let row_len = std::mem::size_of::<MIB_TCP6ROW_OWNER_MODULE>();
    if buffer.len() < header_len {
        return Err(format!(
            "IPv6 TCP table buffer too small: {} bytes, need {header_len}",
            buffer.len()
        ));
    }

    let entry_count = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let needed = header_len + entry_count * row_len;
    if buffer.len() < needed {
        return Err(format!(
            "IPv6 TCP table buffer too small for {entry_count} entries: {} bytes, need {needed}",
            buffer.len()
        ));
    }

    let mut listeners = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let offset = header_len + index * row_len;
        let row_bytes = &buffer[offset..offset + row_len];

        // ucLocalAddr[16] | dwLocalScopeId(4) | dwLocalPort(4) | ucRemoteAddr[16]
        // | dwRemoteScopeId(4) | dwRemotePort(4) | dwState(4) | dwOwningPid(4)
        let local_addr: [u8; 16] = row_bytes[0..16].try_into().expect("16 bytes");
        let dw_local_port = u32::from_ne_bytes(row_bytes[20..24].try_into().expect("4 bytes"));
        let dw_state = u32::from_ne_bytes(row_bytes[48..52].try_into().expect("4 bytes"));
        let dw_pid = u32::from_ne_bytes(row_bytes[52..56].try_into().expect("4 bytes"));

        if dw_state != super::ports::MIB_TCP_STATE_LISTEN {
            continue;
        }

        listeners.push(PortListener {
            protocol: Protocol::Tcp,
            ipVersion: IpVersion::V6,
            localAddress: ipv6_address_to_string(local_addr),
            port: port_from_network_order(dw_local_port),
            pid: dw_pid,
            state: ListenerState::Listen,
        });
    }
    Ok(listeners)
}

/// Enumerate all TCP listeners (IPv4 + IPv6) on this machine.
///
/// Queries both OS tables and merges the results. Normalization
/// (dedup + deterministic sort) happens once, over the merged list, in
/// [`super::ports::normalize_listeners`].
///
/// # Errors
///
/// Returns a human-readable error when either table query fails. A failed
/// family is surfaced honestly (the whole command errors) rather than
/// silently returning a partial list.
pub(crate) fn enumerate_tcp_listeners() -> Result<Vec<PortListener>, String> {
    let v4 = query_tcp_table(AF_INET)
        .map_err(|e| format!("IPv4 listener enumeration failed: {e}"))?;
    let v6 = query_tcp_table(AF_INET6)
        .map_err(|e| format!("IPv6 listener enumeration failed: {e}"))?;

    let mut merged = Vec::with_capacity(v4.len() + v6.len());
    merged.extend(v4);
    merged.extend(v6);
    Ok(merged)
}

/// Compile-time layout assertions: if a future `windows-sys` release changes
/// these sizes, the build fails here instead of producing silently wrong
/// offsets at runtime.
const _: () = assert!(std::mem::size_of::<MIB_TCPROW_OWNER_MODULE>() == 160);
const _: () = assert!(std::mem::size_of::<MIB_TCP6ROW_OWNER_MODULE>() == 192);
const _: () = assert!(std::mem::offset_of!(MIB_TCPROW_OWNER_MODULE, dwOwningPid) == 20);
const _: () = assert!(std::mem::offset_of!(MIB_TCP6ROW_OWNER_MODULE, dwState) == 48);
const _: () = assert!(std::mem::offset_of!(MIB_TCP6ROW_OWNER_MODULE, dwOwningPid) == 52);
const _: () = assert!(V4_ENTRIES_OFFSET == 8, "v4 rows start after dwNumEntries + padding");
const _: () = assert!(V6_ENTRIES_OFFSET == 8, "v6 rows start after dwNumEntries + padding");

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a port the way the OS stores it: network byte order in the low
    /// 16 bits of a `u32` dword.
    fn net_port_dword(port: u16) -> u32 {
        u16::to_be(port) as u32
    }

    #[test]
    fn v4_row_encoding_round_trips() {
        // 127.0.0.1:3000 owned by PID 4242 — bytes exactly as the API writes
        // them into the table buffer (native-endian dwords, port network-order).
        // Row size is 160: 6 dwords + 8-byte timestamp + 128-byte module info.
        let listeners = parse_v4_table(&v4_table(&[v4_listen_row()])).expect("parse");
        assert_eq!(listeners.len(), 1);
        let l = &listeners[0];
        assert_eq!(l.localAddress, "127.0.0.1");
        assert_eq!(l.port, 3000);
        assert_eq!(l.pid, 4242);
        assert_eq!(l.ipVersion, IpVersion::V4);
        assert_eq!(l.protocol, Protocol::Tcp);
    }

    #[test]
    fn v4_table_rejects_non_listen_rows() {
        let mut row = v4_listen_row();
        row[0..4].copy_from_slice(&5u32.to_ne_bytes()); // ESTABLISHED, not LISTEN
        let listeners = parse_v4_table(&v4_table(&[row])).expect("parse");
        assert!(listeners.is_empty());
    }

    #[test]
    fn v4_table_rejects_truncated_buffer() {
        // Header promises 1 entry (160 bytes) but only the header arrives.
        let buffer = 1u32.to_ne_bytes();
        assert!(parse_v4_table(&buffer).is_err());
    }

    #[test]
    fn v4_table_empty_is_valid() {
        let listeners = parse_v4_table(&v4_table(&[])).expect("parse");
        assert!(listeners.is_empty());
    }

    #[test]
    fn v6_row_encoding_round_trips() {
        // [::1]:5432 owned by PID 77 — MIB_TCP6ROW_OWNER_MODULE layout.
        // Row size is 192: 16+4+4+16+4+4+4+4 + 8-byte timestamp + 128 module info.
        let listeners = parse_v6_table(&v6_table(&[v6_listen_row()])).expect("parse");
        assert_eq!(listeners.len(), 1);
        let l = &listeners[0];
        assert_eq!(l.localAddress, "::1");
        assert_eq!(l.port, 5432);
        assert_eq!(l.pid, 77);
        assert_eq!(l.ipVersion, IpVersion::V6);
    }

    #[test]
    fn v6_table_rejects_truncated_buffer() {
        // Header promises 1 entry (192 bytes) but only 40 bytes follow.
        let mut buffer = 1u32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(&[0u8; 40]);
        assert!(parse_v6_table(&buffer).is_err());
    }

    #[test]
    fn v6_wildcard_address_round_trips() {
        let mut row = v6_listen_row();
        // ucLocalAddr = :: (all zero) — the dual-stack wildcard binding.
        row[15] = 0;
        let listeners = parse_v6_table(&v6_table(&[row])).expect("parse");
        assert_eq!(listeners[0].localAddress, "::");
    }

    /// Build a full IPv4 table buffer: `dwNumEntries` header + 4 bytes of
    /// 8-alignment padding + rows (exactly what the C struct lays out).
    fn v4_table(rows: &[[u8; 160]]) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(super::V4_ENTRIES_OFFSET + rows.len() * 160);
        buffer.extend_from_slice(&(rows.len() as u32).to_ne_bytes());
        buffer.extend_from_slice(&[0u8; super::V4_ENTRIES_OFFSET - 4]);
        for row in rows {
            buffer.extend_from_slice(row);
        }
        buffer
    }

    /// Build a full IPv6 table buffer: `dwNumEntries` header + padding + rows.
    fn v6_table(rows: &[[u8; 192]]) -> Vec<u8> {
        let mut buffer = Vec::with_capacity(super::V6_ENTRIES_OFFSET + rows.len() * 192);
        buffer.extend_from_slice(&(rows.len() as u32).to_ne_bytes());
        buffer.extend_from_slice(&[0u8; super::V6_ENTRIES_OFFSET - 4]);
        for row in rows {
            buffer.extend_from_slice(row);
        }
        buffer
    }

    /// A LISTEN-state IPv4 row for 127.0.0.1:3000 owned by PID 4242.
    fn v4_listen_row() -> [u8; 160] {
        let mut row = [0u8; 160];
        row[0..4].copy_from_slice(&2u32.to_ne_bytes()); // dwState = LISTEN
        row[4..8].copy_from_slice(&[127, 0, 0, 1]); // dwLocalAddr, network order
        row[8..12].copy_from_slice(&net_port_dword(3000).to_ne_bytes()); // dwLocalPort
        row[20..24].copy_from_slice(&4242u32.to_ne_bytes()); // dwOwningPid
        row
    }

    /// A LISTEN-state IPv6 row for [::1]:5432 owned by PID 77.
    fn v6_listen_row() -> [u8; 192] {
        let mut row = [0u8; 192];
        row[15] = 1; // ucLocalAddr = ::1
        row[20..24].copy_from_slice(&net_port_dword(5432).to_ne_bytes()); // dwLocalPort
        row[48..52].copy_from_slice(&2u32.to_ne_bytes()); // dwState = LISTEN
        row[52..56].copy_from_slice(&77u32.to_ne_bytes()); // dwOwningPid
        row
    }
}
