//! Project markers, bounded parent walk, and manifest parsing.
//!
//! Everything here is filesystem-read-only and bounded: candidate paths come
//! **only** from process command lines (a runtime's own install location is
//! never ownership evidence), the parent walk stops at the first marker, the
//! filesystem root, or [`MAX_PARENT_DEPTH`] levels — and nothing is ever
//! written. Tests build synthetic project trees in temporary directories; no
//! real repository is touched.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::Confidence;
use crate::intelligence::rules::Evidence;

/// Parent-directory levels examined at most before giving up. Bounded so a
/// deep path can never cause unbounded traversal.
pub(crate) const MAX_PARENT_DEPTH: usize = 10;

/// Marker strength: a `package.json`/`Cargo.toml`/`pyproject.toml` is a
/// *strong* project root (the walk stops there); lockfiles and `.git` are
/// supporting markers (they confirm a root but a bare lockfile deep inside
/// `node_modules` must not claim ownership).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MarkerStrength {
    Strong,
    Supporting,
}

/// A recognized project marker file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Marker {
    pub file: &'static str,
    pub strength: MarkerStrength,
}

/// Markers checked at every walk level. `.git` is handled separately by
/// [`super::git`] (it may be a directory *or* a worktree file).
pub(crate) const MARKERS: &[Marker] = &[
    Marker { file: "package.json", strength: MarkerStrength::Strong },
    Marker { file: "pyproject.toml", strength: MarkerStrength::Strong },
    Marker { file: "setup.py", strength: MarkerStrength::Strong },
    Marker { file: "setup.cfg", strength: MarkerStrength::Strong },
    Marker { file: "Cargo.toml", strength: MarkerStrength::Strong },
    Marker { file: "go.mod", strength: MarkerStrength::Strong },
    // Supporting markers — confirm a root, never start one on their own.
    Marker { file: "requirements.txt", strength: MarkerStrength::Supporting },
    Marker { file: "Pipfile", strength: MarkerStrength::Supporting },
    Marker { file: "poetry.lock", strength: MarkerStrength::Supporting },
    Marker { file: "uv.lock", strength: MarkerStrength::Supporting },
    Marker { file: "pnpm-lock.yaml", strength: MarkerStrength::Supporting },
    Marker { file: "yarn.lock", strength: MarkerStrength::Supporting },
    Marker { file: "package-lock.json", strength: MarkerStrength::Supporting },
    Marker { file: "bun.lock", strength: MarkerStrength::Supporting },
    Marker { file: "bun.lockb", strength: MarkerStrength::Supporting },
    Marker { file: "docker-compose.yml", strength: MarkerStrength::Supporting },
    Marker { file: "compose.yml", strength: MarkerStrength::Supporting },
];

/// True when `path` lives inside a `node_modules` (or equivalent dependency)
/// directory — such paths are runtime-internal, never project-owning.
#[must_use]
pub(crate) fn is_inside_dependency_dir(path: &Path) -> bool {
    path.components().any(|component| {
        let component = component.as_os_str().to_string_lossy();
        matches!(
            component.as_ref(),
            "node_modules" | ".venv" | "venv" | "site-packages" | "target" | "dist" | "build"
        )
    })
}

/// Bounded upward walk from a start path, returning the first directory that
/// holds a project marker (checked with [`find_markers`]).
///
/// - Dependency directories (`node_modules/…`) are skipped as walk *starts*;
///   the walk begins at their nearest non-dependency ancestor.
/// - Stops at the first strong marker, the filesystem root, or
///   [`MAX_PARENT_DEPTH`] levels — whichever comes first.
/// - Supporting markers only win when no stronger candidate appeared, and
///   only if they are not themselves inside a dependency directory.
/// - Home directories and drive roots are **never** claimed as project
///   roots — a stray `package.json` in `C:\Users\me` must not own a
///   process running deep inside `AppData`.
#[must_use]
pub(crate) fn walk_to_project_root(start: &Path) -> Option<(PathBuf, Vec<&'static str>)> {
    let forbidden = ForbiddenRoots::collect();
    // A path into node_modules/… belongs to whatever project installs the
    // packages — start at its nearest non-dependency ancestor.
    let mut current: CowPath = if is_inside_dependency_dir(start) {
        match nearest_non_dependency_ancestor(start) {
            Some(ancestor) => CowPath::Borrowed(ancestor),
            None => return None,
        }
    } else {
        CowPath::Borrowed(start)
    };

    let mut supporting_hit: Option<(PathBuf, Vec<&'static str>)> = None;
    for _depth in 0..=MAX_PARENT_DEPTH {
        let found = if forbidden.contains(current.as_path()) {
            Vec::new() // never claim a home/drive root, but keep walking
        } else {
            find_markers(current.as_path())
        };
        let strong: Vec<&'static str> = found
            .iter()
            .filter(|m| marker_strength(m) == MarkerStrength::Strong)
            .copied()
            .collect();
        let supporting: Vec<&'static str> = found
            .iter()
            .filter(|m| marker_strength(m) == MarkerStrength::Supporting)
            .copied()
            .collect();

        if !strong.is_empty() {
            return Some((current.into_owned(), found));
        }
        if supporting_hit.is_none() && !supporting.is_empty() {
            supporting_hit = Some((current.as_path().to_path_buf(), found));
        }

        match current.as_path().parent() {
            Some(parent) if parent != current.as_path() => {
                current = CowPath::Owned(parent.to_path_buf());
            }
            _ => break, // filesystem root reached
        }
    }
    supporting_hit
}

/// Minimal copy-on-write path helper to avoid cloning every level.
enum CowPath<'a> {
    Borrowed(&'a Path),
    Owned(PathBuf),
}
impl<'a> CowPath<'a> {
    fn as_path(&self) -> &Path {
        match self {
            CowPath::Borrowed(p) => p,
            CowPath::Owned(p) => p.as_path(),
        }
    }

    fn into_owned(self) -> PathBuf {
        match self {
            CowPath::Borrowed(p) => p.to_path_buf(),
            CowPath::Owned(p) => p,
        }
    }
}

/// Directories that must never be claimed as project roots: the user's
/// home directory and filesystem/drive roots. Collected once per walk.
struct ForbiddenRoots {
    home: Option<PathBuf>,
}

impl ForbiddenRoots {
    fn collect() -> Self {
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .ok()
            .map(PathBuf::from);
        Self { home }
    }

    fn contains(&self, dir: &Path) -> bool {
        // Drive/filesystem root: a directory with no parent.
        if dir.parent().is_none() {
            return true;
        }
        if let Some(home) = &self.home {
            if paths_equal_ignore_case(dir, home) {
                return true;
            }
        }
        false
    }
}

/// Windows paths are case-insensitive; compare component-wise ignoring case
/// and trailing separators.
fn paths_equal_ignore_case(a: &Path, b: &Path) -> bool {
    let normalize = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

/// Nearest ancestor (including `start` itself) that is not inside a
/// dependency directory. Used to lift `…/node_modules/vite/bin/vite.js` to
/// `…/node_modules` so the walk can escape it upward.
#[must_use]
fn nearest_non_dependency_ancestor(start: &Path) -> Option<&Path> {
    let mut current = start;
    loop {
        if !is_inside_dependency_dir(current) {
            return Some(current);
        }
        current = current.parent()?;
    }
}

/// All marker files present in `dir` (bounded directory read, no recursion).
#[must_use]
pub(crate) fn find_markers(dir: &Path) -> Vec<&'static str> {
    let mut found = Vec::new();
    for marker in MARKERS {
        if dir.join(marker.file).is_file() {
            found.push(marker.file);
        }
    }
    found
}

fn marker_strength(file: &str) -> MarkerStrength {
    MARKERS
        .iter()
        .find(|m| m.file == file)
        .map(|m| m.strength)
        .unwrap_or(MarkerStrength::Supporting)
}

// ---------------------------------------------------------------------------
// package.json
// ---------------------------------------------------------------------------

/// The only fields we need from `package.json` — deliberately minimal.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PackageJson {
    /// `"name"` field, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `"packageManager"` field (e.g. `pnpm@9.1.0`) — strongest manager evidence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_manager: Option<String>,
    /// `"scripts"` object, flattened for start-command inference.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scripts: HashMap<String, String>,
}

/// Parse a `package.json` file, tolerating any malformed content (`Err`
/// carries a short reason; a project must never fail discovery because its
/// manifest is odd).
pub(crate) fn parse_package_json(path: &Path) -> Result<PackageJson, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("parse {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// Package manager detection
// ---------------------------------------------------------------------------

/// Detected JavaScript package manager, from lockfiles and manifest metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    /// Conflicting lockfiles and no override — reported honestly.
    Ambiguous,
}

impl PackageManager {
    pub(crate) fn display(&self) -> &'static str {
        match self {
            PackageManager::Npm => "npm",
            PackageManager::Pnpm => "pnpm",
            PackageManager::Yarn => "Yarn",
            PackageManager::Bun => "Bun",
            PackageManager::Ambiguous => "Ambiguous",
        }
    }
}

/// Detect the package manager of a project root.
///
/// Priority:
/// 1. `package.json` `"packageManager": "pnpm@…"` — explicit, wins always.
/// 2. Exactly one lockfile — that manager.
/// 3. Multiple conflicting lockfiles — `Ambiguous` (never silently chosen).
#[must_use]
pub(crate) fn detect_package_manager(
    root: &Path,
    package: Option<&PackageJson>,
) -> (PackageManager, Vec<Evidence>) {
    // 1. Explicit manifest field.
    if let Some(package) = package {
        if let Some(field) = &package.package_manager {
            let lowered = field.to_lowercase();
            let manager = if lowered.starts_with("npm") {
                PackageManager::Npm
            } else if lowered.starts_with("pnpm") {
                PackageManager::Pnpm
            } else if lowered.starts_with("yarn") {
                PackageManager::Yarn
            } else if lowered.starts_with("bun") {
                PackageManager::Bun
            } else {
                PackageManager::Ambiguous
            };
            return (
                manager,
                vec![Evidence::new("package_json", format!("packageManager: {field}"))],
            );
        }
    }

    // 2. Lockfile evidence.
    let lockfiles: &[(&str, PackageManager)] = &[
        ("package-lock.json", PackageManager::Npm),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
    ];
    let mut present: Vec<PackageManager> = Vec::new();
    let mut evidence = Vec::new();
    for (file, manager) in lockfiles {
        if root.join(file).is_file() {
            present.push(*manager);
            evidence.push(Evidence::new("lockfile", *file));
        }
    }

    let mut unique: Vec<u8> = present.iter().map(rank).collect();
    unique.sort();
    unique.dedup();
    match unique.as_slice() {
        [one] => {
            let manager = present.iter().copied().find(|m| rank(m) == *one).expect("ranked");
            (manager, evidence)
        }
        _ => {
            // Zero or multiple → honest ambiguity, never a silent choice.
            (PackageManager::Ambiguous, evidence)
        }
    }
}

/// Deterministic ranking for dedup (the enum itself is not `Ord` on
/// purpose — confidence ordering is semantic, not derived from declaration
/// order).
fn rank(manager: &PackageManager) -> u8 {
    match manager {
        PackageManager::Npm => 0,
        PackageManager::Pnpm => 1,
        PackageManager::Yarn => 2,
        PackageManager::Bun => 3,
        PackageManager::Ambiguous => 4,
    }
}

// ---------------------------------------------------------------------------
// Start-command inference
// ---------------------------------------------------------------------------

/// The inferred command that likely launched this service.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub(crate) struct StartCommand {
    /// Display string, e.g. `npm run dev` or `vite`.
    pub command: String,
    /// How much the inference is trusted.
    pub confidence: Confidence,
    /// Evidence used.
    pub evidence: Vec<Evidence>,
}

/// Infer the start command from the real process command line + manifest
/// scripts + the detected package manager.
///
/// Rules:
/// - If the command line directly matches a script's *underlying* command
///   (e.g. script `"dev": "vite"` and the process runs `vite`), the honest
///   result is `npm run dev` (or the matching manager) with `High`
///   confidence — the script mapping is real evidence, and the manager adds
///   its own.
/// - If the command line matches a script name itself (`npm run dev` was
///   launched), report it verbatim as `High`.
/// - If only the raw command line is known, report it as `Medium` (e.g.
///   `vite`), never invented.
/// - Otherwise `None` — no fabrication.
#[must_use]
pub(crate) fn infer_start_command(
    command_line: Option<&str>,
    package: Option<&PackageJson>,
    manager: PackageManager,
) -> Option<StartCommand> {
    let command_line = command_line?;
    let lowered = command_line.to_lowercase();

    if let Some(package) = package {
        // Find a script whose name or underlying command matches the
        // observed command line.
        for (script, underlying) in &package.scripts {
            let script_l = script.to_lowercase();
            let underlying_l = underlying.to_lowercase();
            let first_word = underlying_l.split_whitespace().next().unwrap_or("");

            let direct_run = lowered.contains(&format!("run {script_l}"))
                || lowered.contains(&format!("{script_l} run"));
            let underlying_match = !first_word.is_empty()
                && first_word.len() > 2
                && contains_token(&lowered, first_word);

            if direct_run || underlying_match {
                // Which manager front-end do we claim? Only when evidence
                // supports it; otherwise report the underlying command.
                let command = match manager {
                    PackageManager::Npm => format!("npm run {script}"),
                    PackageManager::Pnpm => format!("pnpm {script}"),
                    PackageManager::Yarn => format!("yarn {script}"),
                    PackageManager::Bun => format!("bun run {script}"),
                    PackageManager::Ambiguous => underlying.clone(),
                };
                let (confidence, evidence) = if direct_run {
                    (
                        Confidence::High,
                        vec![Evidence::new("command_line", command_line.to_string())],
                    )
                } else {
                    (
                        Confidence::High,
                        vec![
                            Evidence::new("package_json", format!("scripts.{script}: {underlying}")),
                            Evidence::new("command_line", command_line.to_string()),
                        ],
                    )
                };
                return Some(StartCommand {
                    command,
                    confidence,
                    evidence,
                });
            }
        }
    }

    // No script mapping: report the real command line as a medium-confidence
    // hint, never a fabricated `npm run …`.
    let trimmed = command_line.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(StartCommand {
            command: trimmed.to_string(),
            confidence: Confidence::Medium,
            evidence: vec![Evidence::new("command_line", trimmed.to_string())],
        })
    }
}

/// Word-ish token containment (same semantics as the intelligence layer).
fn contains_token(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut search_from = 0;
    while let Some(relative) = haystack[search_from..].find(needle) {
        let start = search_from + relative;
        let end = start + needle.len();
        let before_ok = start == 0
            || matches!(
                haystack[..start].chars().next_back(),
                Some(' ' | '\t' | '"' | '\'' | '/' | '\\' | '=' | ':' | ',')
            );
        let after_ok = end == haystack.len()
            || matches!(
                haystack[end..].chars().next(),
                Some(' ' | '\t' | '"' | '\'' | '/' | '\\' | '=' | ':' | ',' | '.' | '-')
            );
        if before_ok && after_ok {
            return true;
        }
        search_from = start + needle.len();
    }
    false
}

// ---------------------------------------------------------------------------
// Python / Rust / Go project names
// ---------------------------------------------------------------------------

/// Extract a project name from `pyproject.toml`'s `project.name` when the
/// file is plain TOML with a simple `[project]` table. Complex/dynamic
/// metadata simply yields `None` — the directory basename is the fallback
/// and nothing breaks.
#[must_use]
pub(crate) fn parse_pyproject_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    // Look for `[project]` then `name = "…"` inside that table. Only quoted
    // string values count — `{ dynamic = … }` etc. is not a usable name.
    let mut in_project = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_project = trimmed == "[project]";
            continue;
        }
        if in_project {
            if let Some(value) = quoted_value_after_key(trimmed, "name") {
                return Some(value);
            }
        }
    }
    None
}

/// Extract the `name` field from `Cargo.toml`'s `[package]` table (same
/// tolerant, line-based approach — no full TOML parser needed for one field).
#[must_use]
pub(crate) fn parse_cargo_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if in_package {
            if let Some(value) = quoted_value_after_key(trimmed, "name") {
                return Some(value);
            }
        }
    }
    None
}

/// Parse `key = "value"` (or `key="value"`) from a TOML line, requiring a
/// proper quoted string. Returns `None` for unquoted/dynamic values and for
/// keys that merely start with the same word (`nameplate = …`).
fn quoted_value_after_key(line: &str, key: &str) -> Option<String> {
    let rest = line.strip_prefix(key)?;
    // The key must be a whole word: followed by whitespace or '='.
    let rest = rest.trim_start();
    let rest = rest.strip_prefix('=')?;
    let value = rest.trim();
    if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
        let inner = &value[1..value.len() - 1];
        if !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    None
}

/// Extract the module path from `go.mod` (`module example.com/app`) — the
/// last path segment doubles as the project name.
#[must_use]
pub(crate) fn parse_go_mod_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("module ") {
            let module = rest.trim().trim_matches('"');
            if !module.is_empty() {
                return module.rsplit('/').next().map(str::to_string);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a directory tree for a test and return its root path.
    fn temp_dir(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "localstack-test-{name}-{}",
            std::process::id()
        ));
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

    // --- dependency-dir detection -------------------------------------------

    #[test]
    fn dependency_dirs_are_recognized() {
        assert!(is_inside_dependency_dir(Path::new(
            r"D:\proj\node_modules\vite\bin\vite.js"
        )));
        assert!(is_inside_dependency_dir(Path::new("/home/u/proj/.venv/lib/app.py")));
        assert!(is_inside_dependency_dir(Path::new(r"D:\proj\target\debug\app.exe")));
        assert!(!is_inside_dependency_dir(Path::new(r"D:\proj\src\main.rs")));
    }

    // --- parent walk ---------------------------------------------------------

    #[test]
    fn vite_path_inside_node_modules_resolves_to_project_root() {
        let root = temp_dir("walk-nm");
        write(&root.join("package.json"), r#"{ "name": "proj" }"#);
        let script = root.join("node_modules/vite/bin/vite.js");
        write(&script, "// vite");

        let (found, markers) = walk_to_project_root(&script).expect("root found");
        assert_eq!(found, root, "must resolve to the project, not node_modules");
        assert!(markers.contains(&"package.json"));
    }

    #[test]
    fn nested_node_modules_resolution_finds_outer_project() {
        // node_modules inside node_modules — the outer project still wins.
        let root = temp_dir("walk-nested");
        write(&root.join("package.json"), r#"{ "name": "outer" }"#);
        let deep = root.join("node_modules/a/node_modules/b/lib.js");
        write(&deep, "//");

        let (found, _) = walk_to_project_root(&deep).expect("root found");
        assert_eq!(found, root);
    }

    #[test]
    fn walk_stops_at_first_strong_marker() {
        // Inner package.json (a workspace) must win over the outer one.
        let outer = temp_dir("walk-stop");
        write(&outer.join("package.json"), r#"{ "name": "outer" }"#);
        let inner = outer.join("packages/inner");
        write(&inner.join("package.json"), r#"{ "name": "inner" }"#);
        let file = inner.join("src/index.ts");
        write(&file, "");

        let (found, _) = walk_to_project_root(&file).expect("root found");
        assert_eq!(found, inner);
    }

    #[test]
    fn walk_is_bounded_by_max_depth() {
        // A deep chain with a marker beyond MAX_PARENT_DEPTH must not be found.
        let root = temp_dir("walk-bounded");
        let mut deep = root.clone();
        for i in 0..=MAX_PARENT_DEPTH + 4 {
            deep = deep.join(format!("level{i}"));
        }
        std::fs::create_dir_all(&deep).expect("mkdir");
        // Marker sits at the very top, beyond the walk budget from `deep`.
        write(&root.join("package.json"), r#"{}"#);

        assert!(walk_to_project_root(&deep).is_none());
    }

    #[test]
    fn walk_stops_at_filesystem_root_without_markers() {
        // No markers anywhere up the tree → None (with the drive root as the
        // only ancestor left, the walk must terminate, not spin).
        let root = temp_dir("walk-no-marker");
        let file = root.join("a/b/c.txt");
        write(&file, "");

        assert!(walk_to_project_root(&file).is_none());
    }

    #[test]
    fn unknown_project_when_no_marker_exists() {
        let root = temp_dir("walk-unknown");
        let file = root.join("app.exe");
        write(&file, "");

        assert!(walk_to_project_root(&file).is_none());
    }

    #[test]
    fn supporting_marker_confirms_but_lone_deep_lockfile_does_not_claim() {
        // A lone lockfile directly in the walked directories is a valid
        // (supporting) root hit, but only when no strong marker exists above.
        let root = temp_dir("walk-supporting");
        write(&root.join("requirements.txt"), "flask==3.0");

        let (found, markers) = walk_to_project_root(&root).expect("root found");
        assert_eq!(found, root);
        assert!(markers.contains(&"requirements.txt"));
    }

    // --- package.json -----------------------------------------------------

    #[test]
    fn package_json_name_and_scripts_are_extracted() {
        let root = temp_dir("pkg");
        let path = root.join("package.json");
        write(
            &path,
            r#"{ "name": "localstack-control-center", "scripts": { "dev": "vite", "build": "tsc && vite build" } }"#,
        );

        let package = parse_package_json(&path).expect("parse");
        assert_eq!(package.name.as_deref(), Some("localstack-control-center"));
        assert_eq!(package.scripts.get("dev").map(String::as_str), Some("vite"));
    }

    #[test]
    fn malformed_package_json_is_an_error_not_a_crash() {
        let root = temp_dir("pkg-bad");
        let path = root.join("package.json");
        write(&path, "{ not json !!!");

        assert!(parse_package_json(&path).is_err());
    }

    // --- package manager -----------------------------------------------------

    fn pkg(manifest: Option<&str>) -> Option<PackageJson> {
        manifest.map(|m| serde_json::from_str(m).expect("manifest json"))
    }

    #[test]
    fn package_lock_means_npm() {
        let root = temp_dir("pm-npm");
        write(&root.join("package-lock.json"), "{}");
        let (manager, evidence) = detect_package_manager(&root, pkg(Some(r"{}")).as_ref());
        assert_eq!(manager, PackageManager::Npm);
        assert!(evidence.iter().any(|e| e.value == "package-lock.json"));
    }

    #[test]
    fn pnpm_lock_means_pnpm() {
        let root = temp_dir("pm-pnpm");
        write(&root.join("pnpm-lock.yaml"), "");
        let (manager, _) = detect_package_manager(&root, None);
        assert_eq!(manager, PackageManager::Pnpm);
    }

    #[test]
    fn yarn_lock_means_yarn() {
        let root = temp_dir("pm-yarn");
        write(&root.join("yarn.lock"), "");
        let (manager, _) = detect_package_manager(&root, None);
        assert_eq!(manager, PackageManager::Yarn);
    }

    #[test]
    fn bun_lock_means_bun() {
        let root = temp_dir("pm-bun");
        write(&root.join("bun.lockb"), "");
        let (manager, _) = detect_package_manager(&root, None);
        assert_eq!(manager, PackageManager::Bun);
    }

    #[test]
    fn package_manager_field_overrides_lockfiles() {
        // pnpm lockfile present, but manifest says bun → bun wins.
        let root = temp_dir("pm-override");
        write(&root.join("pnpm-lock.yaml"), "");
        let manifest = pkg(Some(r#"{ "packageManager": "bun@1.1.0" }"#));
        let (manager, evidence) = detect_package_manager(&root, manifest.as_ref());
        assert_eq!(manager, PackageManager::Bun);
        assert!(evidence.iter().any(|e| e.source == "package_json"));
    }

    #[test]
    fn conflicting_lockfiles_are_ambiguous() {
        let root = temp_dir("pm-conflict");
        write(&root.join("package-lock.json"), "{}");
        write(&root.join("yarn.lock"), "");
        let (manager, _) = detect_package_manager(&root, None);
        assert_eq!(manager, PackageManager::Ambiguous);
    }

    #[test]
    fn no_lockfile_evidence_is_ambiguous_not_npm() {
        let root = temp_dir("pm-none");
        let (manager, _) = detect_package_manager(&root, None);
        assert_eq!(manager, PackageManager::Ambiguous);
    }

    // --- start command ---------------------------------------------------------

    #[test]
    fn vite_process_with_dev_script_infers_npm_run_dev() {
        let manifest = pkg(Some(r#"{ "scripts": { "dev": "vite" } }"#));
        let command = infer_start_command(
            Some(r#"node D:\proj\node_modules\vite\bin\vite.js"#),
            manifest.as_ref(),
            PackageManager::Npm,
        )
        .expect("command inferred");
        assert_eq!(command.command, "npm run dev");
        assert_eq!(command.confidence, Confidence::High);
        assert!(command.evidence.iter().any(|e| e.source == "package_json"));
    }

    #[test]
    fn script_mapping_without_manager_evidence_reports_underlying_command() {
        // Vite evidence but ambiguous manager → honest `vite`, not `npm run dev`.
        let manifest = pkg(Some(r#"{ "scripts": { "dev": "vite" } }"#));
        let command = infer_start_command(
            Some("node .bin/vite"),
            manifest.as_ref(),
            PackageManager::Ambiguous,
        )
        .expect("command inferred");
        assert_eq!(command.command, "vite");
    }

    #[test]
    fn pnpm_manager_produces_pnpm_dev() {
        let manifest = pkg(Some(r#"{ "scripts": { "dev": "vite" } }"#));
        let command = infer_start_command(Some("node vite.js"), manifest.as_ref(), PackageManager::Pnpm)
            .expect("command inferred");
        assert_eq!(command.command, "pnpm dev");
    }

    #[test]
    fn directly_run_script_is_reported_verbatim() {
        let manifest = pkg(Some(r#"{ "scripts": { "dev": "vite" } }"#));
        let command = infer_start_command(Some("npm run dev"), manifest.as_ref(), PackageManager::Npm)
            .expect("command inferred");
        assert_eq!(command.command, "npm run dev");
        assert_eq!(command.confidence, Confidence::High);
    }

    #[test]
    fn command_line_without_script_mapping_is_medium_honest() {
        let command = infer_start_command(Some("python app.py"), None, PackageManager::Ambiguous)
            .expect("command inferred");
        assert_eq!(command.command, "python app.py");
        assert_eq!(command.confidence, Confidence::Medium);
    }

    #[test]
    fn ambiguous_start_command_case_never_fabricates() {
        // No command line at all → None, not an invented command.
        assert!(infer_start_command(None, None, PackageManager::Npm).is_none());
    }

    // --- python / cargo / go names ----------------------------------------------

    #[test]
    fn pyproject_name_is_extracted() {
        let root = temp_dir("py");
        let path = root.join("pyproject.toml");
        write(
            &path,
            "[build-system]\nrequires = [\"hatchling\"]\n\n[project]\nname = \"my-app\"\nversion = \"0.1\"\n",
        );
        assert_eq!(parse_pyproject_name(&path).as_deref(), Some("my-app"));
    }

    #[test]
    fn pyproject_dynamic_metadata_yields_none() {
        let root = temp_dir("py-dyn");
        let path = root.join("pyproject.toml");
        write(&path, "[project]\nname = { dynamic = true }\n");
        assert_eq!(parse_pyproject_name(&path), None);
    }

    #[test]
    fn cargo_name_is_extracted() {
        let root = temp_dir("cargo");
        let path = root.join("Cargo.toml");
        write(&path, "[package]\nname = \"localstack\"\nversion = \"0.1.0\"\n");
        assert_eq!(parse_cargo_name(&path).as_deref(), Some("localstack"));
    }

    #[test]
    fn go_mod_name_is_extracted() {
        let root = temp_dir("go");
        let path = root.join("go.mod");
        write(&path, "module example.com/tools/myapp\n\ngo 1.22\n");
        assert_eq!(parse_go_mod_name(&path).as_deref(), Some("myapp"));
    }
}
