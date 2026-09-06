//! Read-only Git repository detection.
//!
//! Deliberately **no `git` CLI** is spawned — runtime intelligence must not
//! depend on repeatedly launching processes (and a refresh every ~3 s makes
//! that outright wasteful). Everything here is plain filesystem parsing:
//!
//! - A repository is found when a `.git` **directory** (normal repo) or a
//!   `.git` **file** (git worktree / submodule-style pointer) exists in one
//!   of the walked directories.
//! - The branch is parsed from the corresponding `HEAD` file:
//!   `ref: refs/heads/<branch>` → `<branch>`; a raw SHA (detached HEAD) →
//!   `None` — reported honestly rather than guessed.
//!
//! Nothing is ever written; there are no remote operations by design.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::markers::MAX_PARENT_DEPTH;

/// Git facts about a project root, as far as file parsing reveals them.
#[derive(Debug, Clone, PartialEq, Serialize)]
// Field names mirror the frontend contract (`domain.ts`).
#[allow(non_snake_case)]
pub(crate) struct GitInfo {
    /// Whether the project belongs to a Git repository at all.
    pub isRepository: bool,
    /// Repository working-tree root (the directory containing `.git`),
    /// `None` when not a repository.
    pub rootPath: Option<String>,
    /// Current branch name, or `None` for detached HEAD / unknown.
    pub branch: Option<String>,
}

impl GitInfo {
    pub(crate) fn not_a_repository() -> Self {
        Self {
            isRepository: false,
            rootPath: None,
            branch: None,
        }
    }
}

/// Find the Git repository containing `start`, walking upward at most
/// [`MAX_PARENT_DEPTH`] levels (bounded, like the marker walk).
///
/// Handles both repository layouts:
///
/// - `D:\repo\.git\` — a directory (normal clone);
/// - `D:\wt\.git` — a file containing `gitdir: <path>` (git worktree), with
///   `<path>` either absolute or relative to the directory holding the file.
#[must_use]
pub(crate) fn detect_git(start: &Path) -> GitInfo {
    let mut current = start;
    for _depth in 0..=MAX_PARENT_DEPTH {
        let dot_git = current.join(".git");
        if dot_git.is_dir() {
            return GitInfo {
                isRepository: true,
                rootPath: Some(current.to_string_lossy().into_owned()),
                branch: read_branch(&dot_git.join("HEAD")),
            };
        }
        if dot_git.is_file() {
            // Worktree: `.git` is a pointer file: `gitdir: <path to gitdir>`.
            if let Some(git_dir) = parse_gitdir_pointer(&dot_git, current) {
                return GitInfo {
                    isRepository: true,
                    rootPath: Some(current.to_string_lossy().into_owned()),
                    branch: read_branch(&git_dir.join("HEAD")),
                };
            }
            // A malformed pointer file is not a repository — keep walking.
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent,
            _ => break, // filesystem root reached
        }
    }
    GitInfo::not_a_repository()
}

/// Parse a worktree `.git` pointer file into the referenced git directory.
fn parse_gitdir_pointer(pointer_file: &Path, containing_dir: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(pointer_file).ok()?;
    let target = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?
        .trim();
    if target.is_empty() {
        return None;
    }
    let path = Path::new(target);
    if path.is_absolute() {
        Some(path.to_path_buf())
    } else {
        Some(containing_dir.join(path))
    }
}

/// Extract the branch name from a `HEAD` file.
///
/// `ref: refs/heads/main` → `Some("main")`. A raw object id (detached HEAD)
/// has no branch name → `None` (honest absence, not a guess).
fn read_branch(head_file: &Path) -> Option<String> {
    let text = std::fs::read_to_string(head_file).ok()?;
    let trimmed = text.trim();
    trimmed
        .strip_prefix("ref: refs/heads/")
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!("localstack-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create temp dir");
        base
    }

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(path, content).expect("write");
    }

    #[test]
    fn git_directory_is_detected_with_branch() {
        let repo = temp_dir("git-dir");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/master\n");
        let nested = repo.join("src/app");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let info = detect_git(&nested);
        assert!(info.isRepository);
        assert_eq!(info.rootPath.as_deref(), Some(repo.to_string_lossy().as_ref()));
        assert_eq!(info.branch.as_deref(), Some("master"));
    }

    #[test]
    fn detached_head_yields_no_branch() {
        let repo = temp_dir("git-detached");
        write(&repo.join(".git/HEAD"), "3f2a1b9c4d5e6f708192a3b4c5d6e7f8091a2b3c\n");

        let info = detect_git(&repo);
        assert!(info.isRepository);
        assert_eq!(info.branch, None, "detached HEAD must not invent a branch");
    }

    #[test]
    fn worktree_git_file_with_absolute_gitdir_is_supported() {
        let repo = temp_dir("wt-main");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");

        let worktree = temp_dir("wt-link");
        write(&worktree.join(".git"), &format!("gitdir: {}\n", repo.join(".git").display()));

        let info = detect_git(&worktree);
        assert!(info.isRepository);
        assert_eq!(info.rootPath.as_deref(), Some(worktree.to_string_lossy().as_ref()));
        assert_eq!(info.branch.as_deref(), Some("main"));
    }

    #[test]
    fn worktree_git_file_with_relative_gitdir_is_supported() {
        // gitdir entries are usually relative with forward slashes; resolve
        // against the directory holding the `.git` file.
        let repo = temp_dir("wt-rel-main");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/feature/x\n");

        let worktree = temp_dir("wt-rel");
        let repo_name = repo.file_name().expect("sibling name").to_string_lossy().into_owned();
        write(&worktree.join(".git"), &format!("gitdir: ../{repo_name}/.git\n"));

        let info = detect_git(&worktree);
        assert!(info.isRepository);
        assert_eq!(info.branch.as_deref(), Some("feature/x"));
    }

    #[test]
    fn no_git_anywhere_is_not_a_repository() {
        let dir = temp_dir("git-none");
        let nested = dir.join("a/b");
        std::fs::create_dir_all(&nested).expect("mkdir");

        let info = detect_git(&nested);
        assert!(!info.isRepository);
        assert_eq!(info.rootPath, None);
        assert_eq!(info.branch, None);
    }

    #[test]
    fn git_above_the_project_root_is_found_by_the_walk() {
        // Project root sits inside the repo; detection walks up to `.git`.
        let repo = temp_dir("git-walk");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/develop\n");
        let project = repo.join("services/api");
        std::fs::create_dir_all(&project).expect("mkdir");

        let info = detect_git(&project);
        assert!(info.isRepository);
        assert_eq!(info.branch.as_deref(), Some("develop"));
    }

    #[test]
    fn malformed_worktree_pointer_does_not_crash() {
        let dir = temp_dir("git-bad-pointer");
        write(&dir.join(".git"), "not a pointer file\n");

        let info = detect_git(&dir);
        assert!(!info.isRepository, "malformed pointer must not claim a repo");
    }
}
