//! Diagnostics-owned filesystem path resolution (Phase 11C).
//!
//! Single source of truth for the LocalStack app-data directory and the
//! diagnostics subtree layout:
//!
//! ```text
//! %LOCALAPPDATA%\localstack-control-center\
//!   localstack.log            (Phase 10B logger, path unchanged)
//!   settings.json             (settings.rs, via diagnostics::local_app_data_dir)
//!   diagnostics\              (Phase 11C, created lazily on demand)
//!     temp\                   (LocalStack-owned temp staging for atomic writes)
//! ```
//!
//! Later Phase 11C tasks consume these accessors one by one; until each
//! consumer lands, the unused ones carry `#[allow(dead_code)]` scoped to the
//! individual items (never module-wide, so real dead code still surfaces).
//!
//! `local_app_data_dir` preserves the exact Phase 10B contract: resolve
//! `FOLDERID_LocalAppData` once via `SHGetKnownFolderPath`, create the
//! LocalStack-owned directory, memoize with `OnceLock`. Failure returns
//! `None` and every caller degrades gracefully — diagnostics must never
//! become the reason the app fails. All other diagnostics paths derive from
//! this single root; nothing here performs arbitrary filesystem discovery.

use std::path::PathBuf;
use std::sync::OnceLock;

/// Memoized root (set on first successful resolution). Exposed so
/// `mod.rs` can hand out a stable `&'static Path` without re-cloning.
pub(crate) static LOCAL_APP_DATA: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Resolve (once) the LocalStack app-data directory under
/// `FOLDERID_LocalAppData`, creating it on demand. Shared by diagnostics
/// (log file), settings (settings.json), and (Phase 11C) the diagnostics
/// subtree so everything lives in the same LocalStack-owned location —
/// never inside a source tree (spec §E).
pub(crate) fn local_app_data_dir() -> Option<PathBuf> {
    LOCAL_APP_DATA
        .get_or_init(|| {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::Shell::{
                SHGetKnownFolderPath, FOLDERID_LocalAppData, KF_FLAG_DEFAULT,
            };
            unsafe {
                let mut raw: *mut u16 = std::ptr::null_mut();
                let hr = SHGetKnownFolderPath(
                    &FOLDERID_LocalAppData,
                    KF_FLAG_DEFAULT as u32,
                    std::ptr::null_mut(),
                    &mut raw,
                );
                if hr != 0 || raw.is_null() {
                    return None;
                }
                let len = {
                    let mut l = 0usize;
                    while *raw.add(l) != 0 {
                        l += 1;
                    }
                    l
                };
                let slice = std::slice::from_raw_parts(raw, len);
                let path = String::from_utf16_lossy(slice);
                windows_sys::Win32::System::Com::CoTaskMemFree(raw.cast());
                let dir = PathBuf::from(path).join("localstack-control-center");
                if std::fs::create_dir_all(&dir).is_err() {
                    return None;
                }
                Some(dir)
            }
        }
        #[cfg(not(windows))]
        {
            None
        }
    })
    .clone()
}
/// Diagnostics subtree root: `<app-data>\diagnostics\`. Created lazily on
/// demand; `None` when the app-data root itself is unavailable.
/// (Consumed from the bundle-index task onward.)
#[allow(dead_code)]
pub(crate) fn diagnostics_dir() -> Option<PathBuf> {
    ensure_subdir("diagnostics")
}

/// LocalStack-owned temp staging directory: `<app-data>\diagnostics\temp\`.
/// All atomic writes (index, bundles, emergency staging) land here first as
/// `create_new` temp files, are `sync_all`'d, then renamed same-volume into
/// their final LocalStack-owned destination. Never the OS temp directory.
/// (Consumed from the atomic-write helper task onward.)
#[allow(dead_code)]
pub(crate) fn temp_dir() -> Option<PathBuf> {
    ensure_subdir("diagnostics/temp")
}

/// `<app-data>\diagnostics\emergency\` — prepared eagerly at normal startup
/// (see `prepare_emergency_storage` in the panic/crash task) so the panic
/// hook never performs directory creation or discovery at panic time.
/// (Consumed from the panic-emergency-writer task onward.)
#[allow(dead_code)]
pub(crate) fn emergency_dir() -> Option<PathBuf> {
    ensure_subdir("diagnostics/emergency")
}

/// `<app-data>\diagnostics\failed\` — quarantine for emergency records that
/// exhausted their bounded recovery attempts (spec §7).
/// (Consumed from the crash-recovery task onward.)
#[allow(dead_code)]
pub(crate) fn failed_dir() -> Option<PathBuf> {
    ensure_subdir("diagnostics/failed")
}

/// `<app-data>\diagnostics\failed\` created without creating any sibling.
/// (Consumed from the crash-recovery task onward.)
#[allow(dead_code)]
pub(crate) fn failed_dir_exact() -> Option<PathBuf> {
    ensure_exact_subdir("diagnostics/failed")
}

/// `<app-data>\diagnostics\temp\` created without creating any sibling —
/// used by the startup reconciliation tests to prove that purging stale
/// temp files touches ONLY the LocalStack temp directory, never lazily
/// creating unrelated diagnostics subtrees as a side effect.
/// (Consumed from the atomic-write helper task onward.)
#[allow(dead_code)]
pub(crate) fn temp_dir_exact() -> Option<PathBuf> {
    ensure_exact_subdir("diagnostics/temp")
}

/// Resolve the LocalStack root, then create `sub` beneath the diagnostics
/// subtree on demand. Returns `None` on any failure (caller degrades).
#[allow(dead_code)] // consumed task-by-task; module-wide allow would hide real dead code
fn ensure_subdir(sub: &str) -> Option<PathBuf> {
    let root = local_app_data_dir()?;
    let dir = root.join("diagnostics").join(sub);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir)
}

/// Like [`ensure_subdir`] but creates ONLY `<app-data>\diagnostics\sub`,
/// leaving every other diagnostics subtree untouched (no side-effect
/// creation of bundles/, emergency/, failed/, etc.).
#[allow(dead_code)]
fn ensure_exact_subdir(sub: &str) -> Option<PathBuf> {
    let root = local_app_data_dir()?;
    let dir = root.join(sub);
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The diagnostics subtree derives from the same single LocalStack-owned
    /// root as the Phase 10B log file — no second root, no OS temp dir.
    #[test]
    fn diagnostics_paths_derive_from_single_owned_root() {
        let root = match local_app_data_dir() {
            Some(r) => r,
            None => return, // non-Windows test host: nothing to prove
        };
        assert!(
            root.ends_with("localstack-control-center"),
            "root must stay LocalStack-owned, got {root:?}"
        );
        assert!(temp_dir().unwrap().starts_with(&root));
        assert!(emergency_dir().unwrap().starts_with(&root));
        assert!(failed_dir().unwrap().starts_with(&root));
    }
}
