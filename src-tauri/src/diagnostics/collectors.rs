//! Phase 11C Task 11 — managed LogRing snapshot collector (spec §16).
//!
//! REUSES the bounded output the managed-process reader threads already
//! capture (workspace LogRings). No second stdout/stderr pipeline and NO
//! external-process output scraping: the collector is a pure function over
//! the already-captured per-managed-service tails.
//! RED tests first.
// Health/identity collectors are wired into the Tauri command surface by
// Task 14 (commands.rs); until then the dead-code allow keeps the
// warning budget clean without weakening any test.
#![allow(dead_code)]


#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::bundle::SectionContent;

    fn service(id: &str, n_lines: usize, line_len: usize) -> (String, Vec<String>) {
        (
            id.to_string(),
            (0..n_lines).map(|i| format!("line-{i}-{}", "x".repeat(line_len))).collect(),
        )
    }

    #[test]
    fn per_service_line_cap_100() {
        let services = vec![service("svc-a", 150, 10)];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        assert_eq!(v.len(), 1);
        let s = &v[0];
        assert_eq!(s.lines.len(), MANAGED_LINES_PER_SERVICE, "kept = last 100");
        assert!(s.truncated_lines, "flagged");
        assert!(s.lines[0].starts_with("line-50-"), "oldest dropped");
        assert!(s.lines[99].starts_with("line-149-"), "newest kept");
    }

    #[test]
    fn per_service_byte_cap_256kib() {
        // 128 KiB total lines but one giant line → byte cap enforced with an
        // explicit marker line.
        let services = vec![service("svc-b", 4, 80 * 1024)];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let s = &v[0];
        assert!(
            s.bytes <= MANAGED_BYTES_PER_SERVICE,
            "byte cap enforced: {}",
            s.bytes
        );
        assert!(s.truncated_bytes, "flagged");
        assert!(
            s.lines
                .iter()
                .any(|l| l.contains("[TRUNCATED: original output exceeded diagnostic limit]")),
            "explicit truncation marker present"
        );
    }

    #[test]
    fn combined_cap_5_mib_across_services() {
        // 25 services x ~256 KiB = ~6.4 MiB > 5 MiB → deterministic trims.
        let services: Vec<(String, Vec<String>)> = (0..25)
            .map(|i| service(&format!("svc-{i:02}"), 4, 64 * 1024))
            .collect();
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let total: u64 = v.iter().map(|s| s.bytes).sum();
        assert!(
            total <= MANAGED_TOTAL_BYTES,
            "combined cap enforced: {total}"
        );
        // Every retained service keeps its per-service integrity flags; all
        // 25 services are still represented (trimmed, not dropped).
        assert_eq!(v.len(), 25);
    }

    #[test]
    fn redaction_pass_applies_to_every_line() {
        let services = vec![(
            "svc-c".into(),
            vec![
                "ok line".to_string(),
                "leak token=supersecret123".to_string(),
            ],
        )];
        let out = collect_managed_output(&services);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        let joined = v[0].lines.join("\n");
        assert!(!joined.contains("supersecret123"), "secret redacted");
        assert!(joined.contains("token=[REDACTED]"));
    }

    #[test]
    fn empty_input_yields_empty_section() {
        let out = collect_managed_output(&[]);
        let SectionContent::ManagedOutput(v) = &out else {
            panic!("wrong variant");
        };
        assert!(v.is_empty());
    }
}

// -- implementation -----------------------------------------------------------

use crate::diagnostics::bundle::{ManagedServiceOutput, SectionContent};
use crate::diagnostics::redact;

/// Exact managed-output bounds (spec §16 / plan §8).
pub(crate) const MANAGED_LINES_PER_SERVICE: usize = 100;
pub(crate) const MANAGED_BYTES_PER_SERVICE: u64 = 262_144; // 256 KiB
pub(crate) const MANAGED_TOTAL_BYTES: u64 = 5_242_880; // 5 MiB

const TRUNCATION_MARKER: &str = "[TRUNCATED: original output exceeded diagnostic limit]";

/// Deterministic bounded tail of the ALREADY-CAPTURED managed output
/// (workspace LogRings). Redaction applies to every line. Bounds: 100 lines
/// and 256 KiB per service; 5 MiB combined — lowest-priority services are
/// trimmed first (largest byte count, id tie-break). No external-process
/// scraping is possible: the input is only the managed registry snapshot.
pub(crate) fn collect_managed_output(
    services: &[(String, Vec<String>)],
) -> SectionContent {
    // Per-service: redact EVERY line first (defense-in-depth: the pure
    // bounds function also redacts so no caller can forget), keep the LAST
    // 100 lines, then trim to ≤ 256 KiB from the tail with an explicit
    // marker.
    let mut out: Vec<ManagedServiceOutput> = services
        .iter()
        .map(|(id, lines)| {
            let lines: Vec<String> = lines.iter().map(|l| redact(l)).collect();
            let truncated_lines = lines.len() > MANAGED_LINES_PER_SERVICE;
            let kept: Vec<String> = if truncated_lines {
                lines[lines.len() - MANAGED_LINES_PER_SERVICE..].to_vec()
            } else {
                lines.clone()
            };
            let mut truncated_bytes = false;
            let mut kept = keep_last_bytes(kept, MANAGED_BYTES_PER_SERVICE, &mut truncated_bytes);
            if truncated_bytes {
                kept.insert(0, TRUNCATION_MARKER.to_string());
            }
            let bytes: u64 = kept.iter().map(|l| l.len() as u64 + 1).sum();
            ManagedServiceOutput {
                service_id: id.clone(),
                lines: kept,
                truncated_lines,
                bytes,
                truncated_bytes,
            }
        })
        .collect();

    // Combined budget: trim lowest-priority services first — largest byte
    // count, then service id (deterministic). Each pass either halves the
    // largest service or reduces it to the marker line, so the total strictly
    // decreases and the loop converges.
    let mut total: u64 = out.iter().map(|s| s.bytes).sum();
    while total > MANAGED_TOTAL_BYTES {
        let Some(idx) = out
            .iter()
            .enumerate()
            .max_by(|(ia, a), (ib, b)| {
                b.bytes.cmp(&a.bytes).then(b.service_id.cmp(&a.service_id)).then(ib.cmp(ia))
            })
            .map(|(i, _)| i)
        else {
            break;
        };
        let len = out[idx].lines.len();
        if len > 1 {
            // Halve the service's lines, keep newest tail, re-measure.
            let svc = &mut out[idx];
            let keep_from = svc.lines.len() - svc.lines.len() / 2;
            svc.lines.drain(0..keep_from);
            svc.truncated_lines = true;
            svc.lines.insert(0, TRUNCATION_MARKER.to_string());
            svc.bytes = svc.lines.iter().map(|l| l.len() as u64 + 1).sum();
            svc.truncated_bytes = true;
        } else {
            // Already minimal: marker line only.
            let svc = &mut out[idx];
            svc.lines.clear();
            svc.lines.push(TRUNCATION_MARKER.to_string());
            svc.bytes = TRUNCATION_MARKER.len() as u64 + 1;
            svc.truncated_bytes = true;
        }
        total = out.iter().map(|s| s.bytes).sum();
    }

    SectionContent::ManagedOutput(out)
}

/// Keep the newest lines fitting `cap` bytes; set `truncated` when anything
/// was dropped. Deterministic: drop from the front (oldest first).
fn keep_last_bytes(lines: Vec<String>, cap: u64, truncated: &mut bool) -> Vec<String> {
    let mut total: u64 = lines.iter().map(|l| l.len() as u64 + 1).sum();
    let mut start = 0usize;
    while total > cap && start < lines.len() {
        total -= lines[start].len() as u64 + 1;
        start += 1;
        *truncated = true;
    }
    if start == 0 {
        lines
    } else if start >= lines.len() {
        // Everything exceeded the cap: keep nothing but flag truncation.
        Vec::new()
    } else {
        lines[start..].to_vec()
    }
}

/// Live wiring: snapshot tails from the EXISTING workspace LogRings via the
/// owner accessor. Pure read; redaction + bounds are applied by
/// `collect_managed_output`.
#[allow(dead_code)] // consumed by bundle assembly (Tasks 13/19)
pub(crate) fn collect_managed_output_from(
    tails: &[(String, Vec<String>)],
) -> SectionContent {
    collect_managed_output(tails)
}

// -- Task 12: machine identity + version collectors (§43-F) --------------------

/// Identity field kinds mirrored from the Task 7 structural-privacy schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityKind {
    Username,
    Hostname,
    LocalIP,
    Mac,
    Sid,
    MachineGuid,
    DeviceSerial,
}

/// One collected identity field. `value: None` = omitted (Skipped);
/// `skipped_by_design` marks the explicit Phase 11C device-serial decision.
#[derive(Debug, Clone)]
pub struct IdentityField {
    pub kind: IdentityKind,
    pub value: Option<String>,
    pub skipped_by_design: bool,
}

impl IdentityField {
    fn present(kind: IdentityKind, value: String) -> Self {
        Self { kind, value: Some(value), skipped_by_design: false }
    }
    fn omitted(kind: IdentityKind) -> Self {
        Self { kind, value: None, skipped_by_design: false }
    }
}

/// Narrow read-only registry string read (`REG_SZ`). None on any failure.
pub(crate) fn registry_read_string(hkey: usize, subkey: &[u16], value: &[u16]) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegGetValueW, RRF_RT_REG_SZ, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    };
    let mut buf = [0u16; 512];
    let mut len = (buf.len() as u32) * 2;
    let base = match hkey {
        0 => HKEY_LOCAL_MACHINE,
        _ => HKEY_CURRENT_USER,
    };
    let rc = unsafe { RegGetValueW(base, subkey.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut core::ffi::c_void, &mut len) };
    if rc == 0 {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    } else {
        None
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// WebView2 runtime version from the documented BLBeacon registry value.
pub(crate) fn collect_webview2_version() -> Result<Option<String>, String> {
    Ok(registry_read_string(
        1,
        &wide(r"Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"),
        &wide("pv"),
    )
    .or_else(|| {
        registry_read_string(1, &wide(r"Software\Microsoft\EdgeWebView\BLBeacon"), &wide("version"))
    })
    .or_else(|| {
        registry_read_string(0, &wide(r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"), &wide("pv"))
    }))
}

/// Windows build + UBR via read-only registry (CurrentVersion).
pub(crate) fn collect_windows_version() -> Result<Option<(String, String)>, String> {
    let build = registry_read_string(0, &wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), &wide("CurrentBuildNumber"));
    let ubr = registry_read_string(0, &wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"), &wide("UBR"));
    match (build, ubr) {
        (Some(b), Some(u)) => Ok(Some((b, u))),
        (Some(b), None) => Ok(Some((b, "0".to_string()))),
        _ => Ok(None),
    }
}

/// Current-user SID via the exact verified chain (§43-F): OpenProcessToken →
/// GetTokenInformation(TokenUser) → ConvertSidToStringSidW → LocalFree.
/// Test seam allows injecting token-info failure.
pub(crate) fn sid_field_with_seam(
    token_info: impl FnOnce() -> Result<Vec<u8>, String>,
) -> Result<Option<String>, String> {
    let buf = token_info()?;
    use windows_sys::Win32::Security::TOKEN_USER;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows_sys::Win32::Foundation::LocalFree;
    if buf.len() < std::mem::size_of::<TOKEN_USER>() {
        return Err("token buffer too small".into());
    }
    let token_user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let mut str_sid: windows_sys::core::PWSTR = std::ptr::null_mut();
    let ok = unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut str_sid) };
    if ok == 0 || str_sid.is_null() {
        return Err("ConvertSidToStringSidW failed".into());
    }
    unsafe {
        let mut len = 0usize;
        while *str_sid.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(str_sid, len));
        LocalFree(str_sid as _);
        Ok(Some(s))
    }
}

fn sid_via_token() -> Result<Option<String>, String> {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    let mut token: HANDLE = std::ptr::null_mut();
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
    if opened == 0 {
        return Err("OpenProcessToken failed".into());
    }
    // One bounded retry: query size, then allocate exactly that.
    let mut needed: u32 = 0;
    let mut scratch = [0u8; 128];
    let mut ret: u32 = 0;
    let got = unsafe { GetTokenInformation(token, TokenUser, scratch.as_mut_ptr() as *mut core::ffi::c_void, scratch.len() as u32, &mut needed) };
    let buf = if got == 0 && needed > 0 && needed <= 4096 {
        let mut v = vec![0u8; needed as usize];
        let rc = unsafe { GetTokenInformation(token, TokenUser, v.as_mut_ptr() as *mut core::ffi::c_void, needed, &mut ret) };
        if rc == 0 { Err("GetTokenInformation retry failed".into()) } else { Ok(v) }
    } else if got != 0 {
        Ok(scratch[..ret as usize].to_vec())
    } else {
        Err("GetTokenInformation failed".into())
    };
    unsafe { CloseHandle(token) };
    sid_field_with_seam(|| buf)
}

/// Full machine-identity field set (§43-F). Every field maps failure to an
/// omitted value — never a crash. DeviceSerial is Skipped BY DESIGN in
/// Phase 11C (no narrow repository API; structural schema retained).
pub fn collect_system_identity() -> Vec<IdentityField> {
    let mut out = Vec::new();

    // Username — GetUserNameW (Win32_System_WindowsProgramming).
    out.push(match username() {
        Ok(Some(u)) => IdentityField::present(IdentityKind::Username, u),
        _ => IdentityField::omitted(IdentityKind::Username),
    });

    // Hostname — GetComputerNameW.
    out.push(match hostname() {
        Ok(Some(h)) => IdentityField::present(IdentityKind::Hostname, h),
        _ => IdentityField::omitted(IdentityKind::Hostname),
    });

    // SID — exact §43-F chain.
    out.push(match sid_via_token() {
        Ok(Some(s)) => IdentityField::present(IdentityKind::Sid, s),
        _ => IdentityField::omitted(IdentityKind::Sid),
    });

    // MachineGuid — read-only HKLM registry.
    out.push(match registry_read_string(
        0,
        &wide(r"SOFTWARE\Microsoft\Cryptography"),
        &wide("MachineGuid"),
    ) {
        Some(g) => IdentityField::present(IdentityKind::MachineGuid, g),
        None => IdentityField::omitted(IdentityKind::MachineGuid),
    });

    // Local IP + MAC — GetAdaptersAddresses (IpHelper already enabled).
    let (ips, macs) = network_identity();
    out.push(match ips.first() {
        Some(ip) => IdentityField::present(IdentityKind::LocalIP, ip.clone()),
        None => IdentityField::omitted(IdentityKind::LocalIP),
    });
    out.push(match macs.first() {
        Some(m) => IdentityField::present(IdentityKind::Mac, m.clone()),
        None => IdentityField::omitted(IdentityKind::Mac),
    });

    // Device/disk serial — EXPLICIT Phase 11C Skipped/Unsupported decision.
    out.push(IdentityField {
        kind: IdentityKind::DeviceSerial,
        value: None,
        skipped_by_design: true,
    });

    out
}
fn username() -> Result<Option<String>, String> {
    use windows_sys::Win32::System::WindowsProgramming::GetUserNameW;
    let mut buf = [0u16; 257];
    let mut len = buf.len() as u32;
    let ok = unsafe { GetUserNameW(buf.as_mut_ptr(), &mut len) };
    if ok == 0 || len <= 1 {
        return Err("GetUserNameW failed".into());
    }
    Ok(Some(String::from_utf16_lossy(&buf[..(len - 1) as usize])))
}

fn hostname() -> Result<Option<String>, String> {
    use windows_sys::Win32::System::WindowsProgramming::GetComputerNameW;
    let mut buf = [0u16; 261];
    let mut len = buf.len() as u32;
    let ok = unsafe { GetComputerNameW(buf.as_mut_ptr(), &mut len) };
    if ok == 0 || len == 0 {
        return Err("GetComputerNameW failed".into());
    }
    Ok(Some(String::from_utf16_lossy(&buf[..len as usize])))
}

/// Local/private IPv4 addresses + MACs from GetAdaptersAddresses. Read-only.
fn network_identity() -> (Vec<String>, Vec<String>) {
    use windows_sys::Win32::NetworkManagement::IpHelper::GetAdaptersAddresses;
    use windows_sys::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_ANYCAST;
    use windows_sys::Win32::NetworkManagement::IpHelper::GAA_FLAG_SKIP_MULTICAST;
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    let mut size: u32 = 16 * 1024;
    let mut buf = vec![0u8; size as usize];
    for _ in 0..3 {
        let rc = unsafe {
            GetAdaptersAddresses(
                AF_INET as u32,
                GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST,
                std::ptr::null_mut(),
                buf.as_mut_ptr() as *mut _,
                &mut size,
            )
        };
        if rc == 0 {
            break;
        }
        if rc == ERROR_BUFFER_OVERFLOW && size <= 1024 * 1024 {
            buf = vec![0u8; size as usize];
        } else {
            return (Vec::new(), Vec::new());
        }
    }

    let mut ips = Vec::new();
    let mut macs = Vec::new();
    unsafe {
        let mut adapter = buf.as_ptr()
            as *const windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_ADDRESSES_LH;
        while !adapter.is_null() {
            let a = &*adapter;
            if a.OperStatus == 1 && a.PhysicalAddressLength > 0 {
                let mac = a
                    .PhysicalAddress
                    .iter()
                    .take(a.PhysicalAddressLength as usize)
                    .map(|b| format!("{b:02X}"))
                    .collect::<Vec<_>>()
                    .join(":");
                macs.push(mac);
            }
            // First unicast IPv4 of each live adapter.
            let mut uni = a.FirstUnicastAddress;
            while !uni.is_null() {
                let sa = (*uni).Address.lpSockaddr;
                if !sa.is_null() && (*sa).sa_family == AF_INET {
                    let sin =
                        &*(sa as *const windows_sys::Win32::Networking::WinSock::SOCKADDR_IN);
                    let o = sin.sin_addr.S_un.S_un_b;
                    ips.push(format!("{}.{}.{}.{}", o.s_b1, o.s_b2, o.s_b3, o.s_b4));
                    break;
                }
                uni = (*uni).Next;
            }
            adapter = a.Next;
        }
    }
    (ips, macs)
}

/// Human-readable CPU description via read-only registry
/// (HARDWARE\DESCRIPTION\System\CentralProcessor\0\ProcessorNameString).
pub(crate) fn collect_cpu_description() -> Option<String> {
    registry_read_string(
        0,
        &wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0"),
        &wide("ProcessorNameString"),
    )
}
