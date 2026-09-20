#![allow(dead_code)] // Consumed by crash-recovery (Task 9) and export pipeline (Task 13).
//! Phase 11C Task 8 — DPAPI encryption + `.lsdiag` frame (spec §13/§18).
//!
//! Windows current-user DPAPI only (CryptProtectData/CryptUnprotectData on
//! crypt32.dll via windows-sys). No custom key, no application-owned key
//! material, no cloud key, no entropy beyond the OS default. A protected
//! bundle can be opened only by the same Windows user context.
//!
//! On-disk frame: `"LSDIAG1"` magic (7 bytes) + u32 LE cipher length + DPAPI
//! blob. Writes go through the LocalStack-owned atomic write helper (Task 1)
//! — never a generic OS temp directory.

use std::path::Path;

/// Exact DPAPI errors. `ApiFailed` carries the Win32 error code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CryptoError {
    /// Empty input would produce a meaningless blob; rejected before any API
    /// call.
    EmptyPayload,
    /// Input length cannot be represented in the Win32 `cbData` u32 field.
    /// Checked TOTALly before any Windows call — crypto.rs is correct
    /// independently of the 50 MiB bundle budget.
    PayloadTooLarge,
    /// The Windows API failed; carries GetLastError().
    ApiFailed(u32),
}

/// TOTAL usize → u32 conversion for `CRYPT_INTEGER_BLOB.cbData`.
pub(crate) fn checked_cb_data(len: usize) -> Result<u32, CryptoError> {
    if len > u32::MAX as usize {
        return Err(CryptoError::PayloadTooLarge);
    }
    Ok(len as u32)
}

/// On-disk frame magic.
pub(crate) const LSDIAG_MAGIC: &[u8; 7] = b"LSDIAG1";

#[cfg(windows)]
pub(crate) fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    if plain.is_empty() {
        return Err(CryptoError::EmptyPayload);
    }
    let cb = checked_cb_data(plain.len())?; // total conversion BEFORE any call
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let inp = CRYPT_INTEGER_BLOB {
            cbData: cb,
            pbData: plain.as_ptr() as *mut u8,
        };
        let ok = CryptProtectData(
            &inp,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            return Err(CryptoError::ApiFailed(
                windows_sys::Win32::Foundation::GetLastError(),
            ));
        }
        let owned = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // Always free the DPAPI-allocated buffer.
        windows_sys::Win32::Foundation::LocalFree(out.pbData as _);
        Ok(owned)
    }
}

#[cfg(windows)]
pub(crate) fn dpapi_unprotect(cipher: &[u8]) -> Result<Vec<u8>, CryptoError> {
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    if cipher.is_empty() {
        return Err(CryptoError::EmptyPayload);
    }
    let cb = checked_cb_data(cipher.len())?; // total conversion BEFORE any call
    unsafe {
        let mut out = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };
        let mut descr: *mut u16 = std::ptr::null_mut();
        let inp = CRYPT_INTEGER_BLOB {
            cbData: cb,
            pbData: cipher.as_ptr() as *mut u8,
        };
        let ok = CryptUnprotectData(
            &inp,
            &mut descr,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        );
        if ok == 0 {
            return Err(CryptoError::ApiFailed(
                windows_sys::Win32::Foundation::GetLastError(),
            ));
        }
        let owned = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        // Free BOTH the output blob and the returned description string.
        windows_sys::Win32::Foundation::LocalFree(out.pbData as _);
        if !descr.is_null() {
            windows_sys::Win32::Foundation::LocalFree(descr as _);
        }
        Ok(owned)
    }
}

/// Serialize + encrypt + atomically persist the `.lsdiag` frame.
pub(crate) fn write_lsdiag(path: &Path, payload_plain: &[u8]) -> Result<(), CryptoError> {
    let cipher = dpapi_protect(payload_plain)?;
    let mut frame = Vec::with_capacity(LSDIAG_MAGIC.len() + 4 + cipher.len());
    frame.extend_from_slice(LSDIAG_MAGIC);
    frame.extend_from_slice(&(cipher.len() as u32).to_le_bytes());
    frame.extend_from_slice(&cipher);
    super::storage::write_atomic(path, &frame).map_err(|_| {
        CryptoError::ApiFailed(0) // durable write failure maps to a typed error
    })
}

/// Read + parse + decrypt the `.lsdiag` frame.
pub(crate) fn read_lsdiag(path: &Path) -> Result<Vec<u8>, CryptoError> {
    let raw = std::fs::read(path).map_err(|_| CryptoError::ApiFailed(0))?;
    if raw.len() < LSDIAG_MAGIC.len() + 4 || &raw[..LSDIAG_MAGIC.len()] != LSDIAG_MAGIC {
        return Err(CryptoError::ApiFailed(0));
    }
    let len_off = LSDIAG_MAGIC.len();
    let cipher_len =
        u32::from_le_bytes([raw[len_off], raw[len_off + 1], raw[len_off + 2], raw[len_off + 3]])
            as usize;
    let cipher_start = len_off + 4;
    if cipher_len == 0 || raw.len() - cipher_start < cipher_len {
        return Err(CryptoError::ApiFailed(0));
    }
    dpapi_unprotect(&raw[cipher_start..cipher_start + cipher_len])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_payload_is_rejected_without_api_call() {
        assert!(matches!(dpapi_protect(&[]), Err(CryptoError::EmptyPayload)));
        assert!(matches!(dpapi_unprotect(&[]), Err(CryptoError::EmptyPayload)));
    }

    #[test]
    fn oversized_length_rejected_before_api_call() {
        // Seam-tested directly: no >4 GiB allocation ever occurs in tests.
        assert!(matches!(
            checked_cb_data(usize::MAX),
            Err(CryptoError::PayloadTooLarge)
        ));
        assert_eq!(checked_cb_data(64), Ok(64u32));
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_roundtrip_recovers_payload() {
        let m = b"phase-11c-dpapi-roundtrip";
        let c = dpapi_protect(m).unwrap();
        assert_ne!(c, m.to_vec());
        assert_eq!(dpapi_unprotect(&c).unwrap(), m.to_vec());
    }

    #[cfg(windows)]
    #[test]
    fn lsdiag_file_roundtrip_and_magic() {
        let dir = super::super::paths::temp_dir().expect("temp dir");
        let path = dir.join(format!("lsdiag-test-{}.tmp", std::process::id()));
        let plain = b"phase-11c-lsdiag-frame-roundtrip";
        write_lsdiag(&path, plain).expect("write_lsdiag");
        let raw = std::fs::read(&path).expect("raw frame readable");
        assert!(raw.starts_with(b"LSDIAG1"), "frame magic");
        assert_eq!(read_lsdiag(&path).unwrap(), plain.to_vec());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(windows)]
    #[test]
    fn corrupt_ciphertext_is_an_error_not_a_panic() {
        let mut garbage = b"LSDIAG1".to_vec();
        garbage.extend_from_slice(&16u32.to_le_bytes());
        garbage.extend_from_slice(
            b"\xde\xad\xbe\xef\xca\xfe\xba\xbe\x01\x02\x03\x04\x05\x06\x07\x08",
        );
        let out = dpapi_unprotect(&garbage);
        assert!(matches!(out, Err(CryptoError::ApiFailed(_))), "got {out:?}");
    }

    #[cfg(windows)]
    #[test]
    fn encrypted_frame_never_contains_plaintext() {
        let canary = b"PLAINTEXT-CANARY-9f3a";
        let c = dpapi_protect(canary).unwrap();
        assert!(!c.windows(canary.len()).any(|w| w == canary));
        let dir = super::super::paths::temp_dir().expect("temp dir");
        let path = dir.join(format!("lsdiag-canary-{}.tmp", std::process::id()));
        write_lsdiag(&path, canary).expect("write_lsdiag");
        let raw = std::fs::read(&path).expect("raw frame readable");
        assert!(!raw.windows(canary.len()).any(|w| w == canary));
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(windows)]
    #[test]
    fn truncated_frame_is_an_error() {
        let dir = super::super::paths::temp_dir().expect("temp dir");
        let path = dir.join(format!("lsdiag-trunc-{}.tmp", std::process::id()));
        write_lsdiag(&path, b"payload-for-truncation-test").expect("write_lsdiag");
        let mut raw = std::fs::read(&path).expect("raw readable");
        raw.truncate(9); // magic + 2 bytes of the length field
        std::fs::write(&path, &raw).expect("rewrite truncated");
        assert!(matches!(
            read_lsdiag(&path),
            Err(CryptoError::ApiFailed(_)) | Err(CryptoError::EmptyPayload)
        ));
        let _ = std::fs::remove_file(&path);
    }
}
