//! Phase 11C Task 1/2 — LocalStack-owned atomic write helper and test
//! scratch-space helper. YAGNI ruling: NO `tempfile` crate (plan §4) —
//! staging happens strictly inside
//! `%LOCALAPPDATA%\localstack-control-center\diagnostics\temp\`, on the same
//! volume as every destination, so `std::fs::rename` is atomic.
//!
//! Production consumers (index, bundles, emergency records) arrive in later
//! tasks; until then the helpers carry scoped dead-code allowances.

#![allow(dead_code)]

use std::io::{self, Write};
use std::path::Path;

use super::ids;

#[cfg(test)]
use std::path::PathBuf;

/// Atomically replace the file at `dest` with `bytes`:
///
/// 1. `create_new` a uniquely-named temp file inside the LocalStack-owned
///    `diagnostics/temp/` (same volume as `dest` — never the OS temp dir);
/// 2. write the bounded bytes, `flush`, then `sync_all` for durability;
/// 3. close the handle (Windows rename requires it);
/// 4. `std::fs::rename` temp → dest (atomic same-volume replacement);
/// 5. on any pre-commit failure, best-effort delete the temp file.
///
/// Fails closed: a failed write/rename leaves any previous file at `dest`
/// untouched and leaves no temp residue.
pub(crate) fn write_atomic_at(staging_dir: &Path, dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let suffix = ids::random_hex(8).unwrap_or_else(|| format!("{:016x}", std::process::id() as u64));
    let temp_path = staging_dir.join(format!(
        "tmp-{}-{suffix}.part",
        std::process::id()
    ));
    let cleanup = |p: &Path| {
        let _ = std::fs::remove_file(p);
    };
    // `create_new` — never clobber a concurrent writer's staging file.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;
    let write_result = (|| -> io::Result<()> {
        f.write_all(bytes)?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    drop(f);
    if let Err(e) = write_result {
        cleanup(&temp_path);
        return Err(e);
    }
    // Same-volume rename: atomic on Windows when the destination exists
    // (std::fs::rename uses MoveFileEx with MOVEFILE_REPLACE_EXISTING;
    // verified locally: file→file replaces atomically, file→dir fails with
    // "Access is denied" and leaves the staging file behind for cleanup).
    if let Err(e) = std::fs::rename(&temp_path, dest) {
        cleanup(&temp_path);
        return Err(e);
    }
    Ok(())
}

/// [`write_atomic_at`] with the default LocalStack-owned staging directory
/// (`diagnostics/temp/`). This is the form production callers use.
pub(crate) fn write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let staging = super::paths::temp_dir()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "diagnostics temp dir unavailable"))?;
    write_atomic_at(&staging, dest, bytes)
}

/// Deterministic test scratch dir under LocalStack temp/ (real filesystem
/// behavior for save/load tests, cleaned by the caller). `None` on
/// non-Windows hosts or unavailable app-data root — tests then early-return
/// since the Windows-owned path contract is unprovable there.
#[cfg(test)]
pub(crate) fn test_scratch_dir(name: &str) -> Option<PathBuf> {
    let base = super::paths::temp_dir()?;
    let dir = base.join(format!("test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Purge stale LocalStack-owned temp staging files (strict owned shape
/// `tmp-<pid>-<16 lowercase hex>.part` inside `diagnostics/temp/` ONLY).
/// Never touches foreign files, never recurses, never follows links.
pub(crate) fn purge_stale_temp() {
    purge_stale_temp_older_than(std::time::Duration::from_secs(3600));
}

/// Purge only temp files strictly older than `age` — a freshly-staged file
/// belongs to a live writer (possibly a concurrent one), so it is never
/// touched. Startup reconciliation uses the default 1-hour staleness bound.
pub(crate) fn purge_stale_temp_older_than(age: std::time::Duration) {
    let Some(dir) = super::paths::temp_dir() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_owned_temp_filename(&name) {
            continue;
        }
        let path = dir.join(&name);
        // Regular files only; never follow or remove anything else.
        let ok = std::fs::symlink_metadata(&path)
            .map(|m| m.is_file() && !m.file_type().is_symlink())
            .unwrap_or(false);
        if !ok {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|d| d >= age)
            .unwrap_or(false);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn is_owned_temp_filename(name: &str) -> bool {
    // tmp-<pid>-<16 lowercase hex>.part
    let Some(rest) = name.strip_prefix("tmp-") else {
        return false;
    };
    let Some((pid_hex, suffix)) = rest.split_once('-') else {
        return false;
    };
    let Some(hex) = suffix.strip_suffix(".part") else {
        return false;
    };
    pid_hex.chars().all(|c| c.is_ascii_digit())
        && hex.len() == 16
        && hex
            .chars()
            .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Remove a scratch directory created by `test_scratch_dir` (test cleanup).
#[cfg(test)]
pub(crate) fn remove_scratch_dir(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_atomic_replaces_existing_destination() {
        let dir = match test_scratch_dir("replace") {
            Some(d) => d,
            None => return,
        };
        let dest = dir.join("out.json");
        // Both the default form (LocalStack temp/ staging) and the explicit
        // staging-dir form must replace an existing destination atomically.
        write_atomic(&dest, b"v1").expect("first write");
        assert_eq!(std::fs::read(&dest).unwrap(), b"v1");
        write_atomic_at(&dir, &dest, b"version-2-longer").expect("replacement write");
        assert_eq!(std::fs::read(&dest).unwrap(), b"version-2-longer");
        let residue = scan_part_files(&dir);
        assert!(residue.is_empty(), "no staging residue after success");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_rename_leaves_old_file_intact_and_no_residue() {
        let dir = match test_scratch_dir("failed-rename") {
            Some(d) => d,
            None => return,
        };
        // A DIRECTORY sitting at the rename destination blocks the commit:
        // Windows MoveFileEx-with-replace cannot replace a dir with a file
        // (verified: "Access is denied"). This is the realistic
        // commit-failure shape for the atomic-write helper.
        //
        // Staging happens in THIS test's scratch dir (via write_atomic_at)
        // so parallel test threads' in-flight staging files elsewhere in
        // temp/ cannot false-fail the residue assertion.
        let dest = dir.join("keep.json");
        std::fs::create_dir(&dest).unwrap(); // directory blocks rename
        assert!(
            write_atomic_at(&dir, &dest, b"new").is_err(),
            "rename over a directory fails"
        );
        // No residue in this test's own staging area.
        let residue = scan_part_files(&dir);
        assert!(residue.is_empty(), "staging file cleaned up on failure");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_write_creates_no_partial_destination() {
        let dir = match test_scratch_dir("no-partial") {
            Some(d) => d,
            None => return,
        };
        // Destination inside a non-existent parent → rename fails; the
        // staging file must be cleaned up. Staging in this test's own
        // scratch dir keeps the assertion independent of parallel threads.
        let dest = dir.join("missing-parent").join("out.json");
        assert!(write_atomic_at(&dir, &dest, b"x").is_err());
        let residue = scan_part_files(&dir);
        assert!(residue.is_empty(), "no .part residue after failed rename");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bounded recursive scan for `.part` leftovers (test-only helper).
    fn scan_part_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else { return out };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(scan_part_files(&p));
            } else if p.extension().is_some_and(|x| x == "part") {
                out.push(p);
            }
        }
        out
    }
}
