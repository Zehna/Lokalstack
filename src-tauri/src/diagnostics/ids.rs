//! Shared opaque-ID generator (Phase 11C, spec §3 + §43-H).
//!
//! One mechanism for every diagnostics opaque ID: incident IDs, bundle IDs,
//! export-capability IDs, and temp-file suffixes all draw from here.
//!
//! Windows-first choice (per plan §43-H): `BCryptGenRandom` with
//! `BCRYPT_USE_SYSTEM_PREFERRED_RNG` through the already-required
//! `windows-sys` `Win32_Security_Cryptography` feature — no new randomness
//! crate, no time-seeded fallback. IDs are:
//!
//! - **collision-resistant** (64+ bits drawn per ID),
//! - **unpredictable** (system CSPRNG, so guessing a capability/ID is not an
//!   authority path),
//! - **not derived from content** (never from project names, paths, errors,
//!   or secrets).
//!
//! Deterministic test seam: `hex_from_source` lets tests inject bytes so ID
//! formatting is proven without depending on RNG behavior.
//!
//! Consumers arrive task-by-task (incidents now, bundles/capabilities/temp
//! suffixes later); until then the live generator carries a scoped
//! `#[allow(dead_code)]`.

/// Draw `bytes` cryptographically-random bytes via BCryptGenRandom and
/// return them lowercase-hex-encoded. Returns `None` when the OS RNG fails —
/// callers must degrade (never fall back to a weaker source).
#[allow(dead_code)] // consumed from the capture-worker task onward
pub(crate) fn random_hex(bytes: usize) -> Option<String> {
    hex_from_source(bytes, || {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Security::Cryptography::{
                BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            };
            let mut buf = vec![0u8; bytes];
            // BCRYPT_USE_SYSTEM_PREFERRED_RNG allows hAlgorithm = NULL.
            let status = unsafe {
                BCryptGenRandom(
                    std::ptr::null_mut(),
                    buf.as_mut_ptr(),
                    buf.len() as u32,
                    BCRYPT_USE_SYSTEM_PREFERRED_RNG,
                )
            };
            if status != 0 {
                return Err(());
            }
            Ok(buf)
        }
        #[cfg(not(windows))]
        {
            // Non-Windows hosts have no diagnostics subsystem (spec: Windows-first);
            // the code must still compile for `cargo check` on other targets.
            let _ = bytes;
            Err(())
        }
    })
}

/// Deterministic seam: format `bytes` drawn from `source` as lowercase hex.
/// `None` when the source fails (mirrors `random_hex` failure semantics).
#[allow(dead_code)]
pub(crate) fn hex_from_source(
    bytes: usize,
    source: impl FnOnce() -> Result<Vec<u8>, ()>,
) -> Option<String> {
    let buf = source().ok()?;
    if buf.len() != bytes {
        return None;
    }
    Some(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_hex_is_opaque_and_not_content_derived() {
        // On Windows the live generator must produce well-formed opaque IDs.
        let a = match random_hex(12) {
            Some(v) => v,
            None => return, // non-Windows host or RNG outage: live path unprovable here
        };
        assert_eq!(a.len(), 24, "12 bytes = 24 hex chars");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, random_hex(12).unwrap(), "distinct draws");
        assert_ne!(a, "deadbeef");
    }

    #[test]
    fn hex_seam_formats_injected_bytes_deterministically() {
        let id = hex_from_source(4, || Ok(vec![0xDE, 0xAD, 0xBE, 0xEF])).unwrap();
        assert_eq!(id, "deadbeef");

        // Source failure must propagate as None — never a fallback value.
        assert!(hex_from_source(4, || Err(())).is_none());

        // Length mismatch is rejected, never silently truncated.
        assert!(hex_from_source(4, || Ok(vec![1, 2, 3])).is_none());
    }
}
