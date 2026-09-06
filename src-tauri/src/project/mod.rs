//! # Project Intelligence (Phase 4)
//!
//! Associates running services with the local source-code project that
//! launched them, using **evidence** — never port numbers, never "nearest
//! package.json" guesses.
//!
//! ## Pipeline
//!
//! ```text
//! ProcessInfo (command line, executable path)
//!   → candidate path extraction (absolute paths only, quote-aware)
//!   → bounded parent walk to a confirmed project root (markers.rs)
//!   → manifest parsing (package.json / pyproject / Cargo.toml / go.mod)
//!   → Git detection (git.rs)
//!   → ProjectIdentity (explicit confidence + evidence)
//! ```
//!
//! ## Architecture boundaries
//!
//! - Separated from `discovery` (sockets), `process` (OS sampling), and
//!   `intelligence` (service/framework identity) — three distinct layers.
//! - Identity separation in the response: `projects` is a list of unique
//!   [`ProjectIdentity`] values; `projectLinks` maps each PID to a project
//!   id. Many PIDs share one project; nothing is duplicated per listener.
//!
//! ## Caching
//!
//! Resolution is content-addressed: the cache key is *(executable path,
//! command line)* — exactly the inputs that determine the outcome. Warm
//! cycles do **zero** filesystem work for unchanged processes. Invalidation:
//! command-line change (process restarted differently), cache eviction, or
//! an explicit refresh (`bypass_project_cache` from the manual Refresh
//! button). Git branch changes are picked up on the next invalidation — a
//! documented trade-off to keep 3-second polling filesystem-free.

pub(crate) mod git;
pub(crate) mod markers;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::process::ProcessInfo;

pub(crate) use git::GitInfo;
pub(crate) use markers::{
    detect_package_manager, infer_start_command, parse_cargo_name, parse_go_mod_name,
    parse_package_json, parse_pyproject_name, walk_to_project_root, PackageManager, StartCommand,
};
use markers::{is_inside_dependency_dir, MarkerStrength};

pub(crate) use crate::intelligence::rules::{Confidence, Evidence};
// ---------------------------------------------------------------------------
// Domain model (serde DTO — field names mirror `domain.ts`)
// ---------------------------------------------------------------------------

/// What kind of project the markers identify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProjectKind {
    NodeJs,
    Python,
    Rust,
    Go,
    /// Markers confirm a project root but not its ecosystem.
    Unknown,
}

/// A resolved local project, as far as evidence supports it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ProjectIdentity {
    /// Stable id — the project root path. Unique per project, shared by
    /// every process that belongs to it.
    pub id: String,
    /// Display name: manifest name when readable, else directory basename.
    pub name: String,
    /// Confirmed project root directory.
    pub rootPath: String,
    /// Ecosystem the markers identify.
    pub kind: ProjectKind,
    /// Git facts (repository, root, branch) — `isRepository: false` when none.
    pub git: GitInfo,
    /// JS package manager (`npm`, `pnpm`, `Yarn`, `Bun`, `Ambiguous`), or
    /// `None` for non-JS projects.
    pub packageManager: Option<String>,
    /// Inferred start command, or `None` when nothing honest can be said.
    pub startCommand: Option<StartCommand>,
    /// How strongly the process→project association is evidenced.
    pub confidence: Confidence,
    /// The evidence that produced this identity, strongest first.
    pub evidence: Vec<Evidence>,
}

/// A PID's link to a project — the normalized shape that keeps one
/// [`ProjectIdentity`] shared across all of a project's processes.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PidProjectLink {
    pub pid: u32,
    /// References [`ProjectIdentity::id`] in the same response.
    pub projectId: String,
}

// ---------------------------------------------------------------------------
// Candidate extraction (pure)
// ---------------------------------------------------------------------------

/// Extract absolute-path candidates from a process command line.
///
/// Quote-aware tokenization (Windows command lines quote paths with spaces),
/// keeping only tokens that look like absolute Windows paths (drive-letter
/// or UNC form). Relative paths are deliberately skipped: the process's
/// working directory is unknown, and resolving against LocalStack's own CWD
/// would fabricate evidence.
#[must_use]
pub(crate) fn extract_candidate_paths(command_line: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in command_line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    let mut candidates: Vec<String> = tokens
        .into_iter()
        .filter(|token| looks_like_absolute_path(token))
        // Bounded: a pathological command line cannot create unbounded work.
        .take(8)
        .collect();
    candidates.dedup();
    candidates
}

/// Windows absolute path: `C:\…`, `C:/…`, or UNC `\\server\share\…`.
fn looks_like_absolute_path(token: &str) -> bool {
    if token.starts_with(r"\\") {
        return true;
    }
    let bytes = token.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Resolve one process to a project, if the evidence supports it.
///
/// Candidates are tried in command-line order; the first candidate whose
/// parent walk confirms a project root wins. The process's own executable
/// path is a last-resort candidate only (useful for `cargo run`-style
/// binaries); a runtime's install location simply finds no marker and
/// yields no project — runtime install dirs are never ownership evidence.
#[must_use]
pub(crate) fn resolve_process(process: &ProcessInfo) -> Option<ProjectIdentity> {
    if !process.accessible {
        return None; // No command line, no path — no evidence, no guess.
    }

    let mut candidates = process
        .commandLine
        .as_deref()
        .map(extract_candidate_paths)
        .unwrap_or_default();
    if candidates.is_empty() {
        // No path tokens in the command line: the executable itself is the
        // only filesystem evidence (e.g. `target\debug\app.exe`).
        candidates = process.executablePath.clone().into_iter().collect();
    }

    for candidate in &candidates {
        let path = Path::new(candidate);
        if let Some((root, markers_found)) = walk_to_project_root(path) {
            return Some(build_identity(process, candidate, &root, &markers_found));
        }
    }
    None
}

/// Build a full [`ProjectIdentity`] from a confirmed root.
fn build_identity(
    process: &ProcessInfo,
    candidate: &str,
    root: &Path,
    markers_found: &[&'static str],
) -> ProjectIdentity {
    let root_str = root.to_string_lossy().into_owned();
    let basename = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root_str.clone());

    let strong = markers_found
        .iter()
        .any(|m| marker_is_strong(m));
    let lifted = is_inside_dependency_dir(Path::new(candidate));

    let kind = kind_from_markers(markers_found);

    let mut evidence: Vec<Evidence> = vec![Evidence::new("command_path", candidate)];
    evidence.extend(markers_found.iter().map(|m| Evidence::new("marker", *m)));

    // Manifest-parsed name (per ecosystem) with directory-basename fallback.
    let package = if kind == ProjectKind::NodeJs {
        let path = root.join("package.json");
        match parse_package_json(&path) {
            Ok(package) => Some(package),
            // A manifest that fails to parse must not fail the project.
            Err(_) => None,
        }
    } else {
        None
    };

    let (name, name_source): (String, Option<(&str, String)>) = match kind {
        ProjectKind::NodeJs => package
            .as_ref()
            .and_then(|p| p.name.clone())
            .map(|name| (name.clone(), Some(("package_json", format!("name: {name}"))))),
        ProjectKind::Python => {
            let path = root.join("pyproject.toml");
            parse_pyproject_name(&path).map(|name| (name.clone(), Some(("pyproject", format!("name: {name}")))))
        }
        ProjectKind::Rust => {
            let path = root.join("Cargo.toml");
            parse_cargo_name(&path).map(|name| (name.clone(), Some(("cargo_manifest", format!("name: {name}")))))
        }
        ProjectKind::Go => {
            let path = root.join("go.mod");
            parse_go_mod_name(&path).map(|name| (name.clone(), Some(("go_mod", format!("module: {name}")))))
        }
        ProjectKind::Unknown => None,
    }
    .unwrap_or_else(|| (basename.clone(), None));
    if let Some((source, value)) = name_source {
        evidence.push(Evidence::new(source, value));
    }

    // Package manager: JS projects only, never silently chosen.
    let package_manager_display: Option<String> = if kind == ProjectKind::NodeJs {
        let (manager, pm_evidence) = detect_package_manager(root, package.as_ref());
        evidence.extend(pm_evidence);
        Some(manager.display().to_string())
    } else {
        None
    };

    // Start command: manifest scripts (JS) or the honest raw command line.
    let manager = package_manager_display
        .as_deref()
        .and_then(manager_from_display)
        .unwrap_or(PackageManager::Ambiguous);
    let start_command = if kind == ProjectKind::NodeJs {
        infer_start_command(process.commandLine.as_deref(), package.as_ref(), manager)
    } else {
        infer_start_command(process.commandLine.as_deref(), None, PackageManager::Ambiguous)
    };
    if let Some(start) = &start_command {
        evidence.push(Evidence::new("start_command", start.command.clone()));
    }

    // Git facts (also walks upward; handles worktree `.git` files).
    let git = git::detect_git(root);
    if git.isRepository {
        if let Some(git_root) = &git.rootPath {
            evidence.push(Evidence::new("git_root", git_root.clone()));
        }
    }

    // Association confidence: direct in-root path beats a dependency-dir
    // lift; supporting-only markers are the weakest confirmation.
    let confidence = if lifted {
        Confidence::High
    } else if strong {
        Confidence::Exact
    } else {
        Confidence::Medium
    };

    ProjectIdentity {
        id: root_str.clone(),
        name,
        rootPath: root_str,
        kind,
        git,
        packageManager: package_manager_display,
        startCommand: start_command,
        confidence,
        evidence,
    }
}

fn marker_is_strong(marker: &str) -> bool {
    markers::MARKERS
        .iter()
        .any(|m| m.file == marker && m.strength == MarkerStrength::Strong)
}

fn kind_from_markers(markers: &[&str]) -> ProjectKind {
    if markers.contains(&"package.json") {
        ProjectKind::NodeJs
    } else if markers.contains(&"pyproject.toml")
        || markers.contains(&"setup.py")
        || markers.contains(&"setup.cfg")
    {
        ProjectKind::Python
    } else if markers.contains(&"Cargo.toml") {
        ProjectKind::Rust
    } else if markers.contains(&"go.mod") {
        ProjectKind::Go
    } else {
        ProjectKind::Unknown
    }
}

fn manager_from_display(display: &str) -> Option<PackageManager> {
    match display {
        "npm" => Some(PackageManager::Npm),
        "pnpm" => Some(PackageManager::Pnpm),
        "Yarn" => Some(PackageManager::Yarn),
        "Bun" => Some(PackageManager::Bun),
        _ => Some(PackageManager::Ambiguous),
    }
}

// ---------------------------------------------------------------------------
// Cache + cycle integration
// ---------------------------------------------------------------------------

/// Cache key: exactly the inputs that determine the resolution outcome.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct CacheKey {
    exe: Option<String>,
    command_line: Option<String>,
}

impl CacheKey {
    fn of(process: &ProcessInfo) -> Self {
        Self {
            exe: process.executablePath.clone(),
            command_line: process.commandLine.clone(),
        }
    }
}

/// Content-addressed resolution cache. Bounded: when it exceeds
/// [`CACHE_CAPACITY`] entries it is cleared wholesale (an LRU would be
/// over-engineering for a ≤ 40-PID machine; a full clear is rare and cheap).
pub(crate) type ProjectCache = HashMap<CacheKey, Arc<ProjectIdentity>>;

pub(crate) const CACHE_CAPACITY: usize = 256;

/// Per-cycle outcome of project resolution (for logging/perf reporting).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ResolutionStats {
    pub resolved: usize,
    pub cache_hits: usize,
    pub projects: usize,
}

/// Tauri-managed state for the project engine.
#[derive(Default)]
pub(crate) struct ProjectEngineState {
    pub(crate) cache: Arc<Mutex<ProjectCache>>,
}

/// Resolve projects for a whole discovery cycle against the shared cache.
///
/// Returns the unique project list and per-PID links. With
/// `bypass_cache` (manual refresh) every process is re-resolved from the
/// filesystem — the cache is refreshed with the new results.
pub(crate) fn resolve_projects(
    processes: &[ProcessInfo],
    cache: &mut ProjectCache,
    bypass_cache: bool,
) -> (Vec<ProjectIdentity>, Vec<PidProjectLink>, ResolutionStats) {
    if bypass_cache && !cache.is_empty() {
        cache.clear();
    }

    let mut by_id: HashMap<String, Arc<ProjectIdentity>> = HashMap::new();
    let mut links: Vec<PidProjectLink> = Vec::new();
    let mut stats = ResolutionStats::default();

    for process in processes {
        if !process.accessible {
            continue;
        }
        let key = CacheKey::of(process);
        let identity: Arc<ProjectIdentity> = if !bypass_cache {
            if let Some(hit) = cache.get(&key) {
                stats.cache_hits += 1;
                Arc::clone(hit)
            } else {
                let Some(resolved) = resolve_process(process) else {
                    continue;
                };
                stats.resolved += 1;
                let arc = Arc::new(resolved);
                insert_bounded(cache, key, Arc::clone(&arc));
                arc
            }
        } else {
            let Some(resolved) = resolve_process(process) else {
                continue;
            };
            stats.resolved += 1;
            let arc = Arc::new(resolved);
            insert_bounded(cache, key, Arc::clone(&arc));
            arc
        };

        if !by_id.contains_key(&identity.id) {
            by_id.insert(identity.id.clone(), Arc::clone(&identity));
        }
        links.push(PidProjectLink {
            pid: process.pid,
            projectId: identity.id.clone(),
        });
    }

    stats.projects = by_id.len();
    (
        by_id.into_values().map(|arc| (*arc).clone()).collect(),
        links,
        stats,
    )
}

fn insert_bounded(cache: &mut ProjectCache, key: CacheKey, identity: Arc<ProjectIdentity>) {
    if cache.len() >= CACHE_CAPACITY {
        cache.clear();
    }
    cache.insert(key, identity);
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("localstack-proj-{name}-{}", std::process::id()));
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

    fn process(pid: u32, exe: Option<&str>, command_line: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid,
            name: Some("node.exe".to_string()),
            executablePath: exe.map(str::to_string),
            startedAt: Some(1_700_000_000_000),
            memoryBytes: Some(1_000_000),
            cpuPercent: None,
            commandLine: command_line.map(str::to_string),
            accessible: true,
        }
    }

    // --- candidate extraction ------------------------------------------------

    #[test]
    fn quoted_paths_with_spaces_are_single_candidates() {
        let candidates = extract_candidate_paths(
            r#""C:\Program Files\nodejs\node.exe" "D:\My Projects\app\server.js" --port 3000"#,
        );
        assert_eq!(
            candidates,
            vec![
                r"C:\Program Files\nodejs\node.exe".to_string(),
                r"D:\My Projects\app\server.js".to_string(),
            ]
        );
    }

    #[test]
    fn relative_and_flag_tokens_are_not_candidates() {
        let candidates = extract_candidate_paths("node ./bin/vite.js --host --port 1420");
        assert!(candidates.is_empty(), "relative paths must be skipped, got {candidates:?}");
    }

    #[test]
    fn drive_letter_and_unc_paths_are_candidates() {
        assert_eq!(
            extract_candidate_paths(r"python D:\proj\app.py"),
            vec![r"D:\proj\app.py".to_string()]
        );
        assert_eq!(
            extract_candidate_paths(r"app \\server\share\tool.exe"),
            vec![r"\\server\share\tool.exe".to_string()]
        );
    }

    // --- resolution -----------------------------------------------------------

    #[test]
    fn vite_command_line_resolves_to_enclosing_project() {
        let root = temp_dir("resolve-vite");
        write(
            &root.join("package.json"),
            r#"{ "name": "localstack-control-center", "scripts": { "dev": "vite" } }"#,
        );
        write(&root.join("package-lock.json"), "{}");
        write(&root.join("node_modules/vite/bin/vite.js"), "//");

        let resolved = resolve_process(&process(
            1,
            Some(r"C:\Program Files\nodejs\node.exe"),
            Some(&format!(
                r#""C:\Program Files\nodejs\node.exe" "{}""#,
                root.join("node_modules/vite/bin/vite.js").display()
            )),
        ))
        .expect("project resolved");

        assert_eq!(resolved.rootPath, root.to_string_lossy());
        assert_eq!(resolved.name, "localstack-control-center");
        assert_eq!(resolved.kind, ProjectKind::NodeJs);
        assert_eq!(resolved.confidence, Confidence::High, "dep-dir lift = High");
        assert_eq!(resolved.packageManager.as_deref(), Some("npm"));
        assert!(resolved.git.rootPath.is_none(), "no .git in synthetic tree");
        assert!(resolved
            .startCommand
            .as_ref()
            .is_some_and(|s| s.command == "npm run dev"));
        assert!(resolved.evidence.iter().any(|e| e.source == "command_path"));
    }

    #[test]
    fn direct_in_root_script_is_exact_confidence() {
        let root = temp_dir("resolve-direct");
        write(&root.join("pyproject.toml"), "[project]\nname = \"my-app\"\n");
        write(&root.join("app.py"), "");

        let resolved = resolve_process(&process(
            2,
            Some(r"C:\Python312\python.exe"),
            Some(&format!(r"C:\Python312\python.exe {}", root.join("app.py").display())),
        ))
        .expect("project resolved");

        assert_eq!(resolved.rootPath, root.to_string_lossy());
        assert_eq!(resolved.name, "my-app");
        assert_eq!(resolved.kind, ProjectKind::Python);
        assert_eq!(resolved.confidence, Confidence::Exact);
        assert_eq!(resolved.packageManager, None, "non-JS projects have no package manager");
    }

    #[test]
    fn cargo_run_binary_resolves_via_executable_fallback() {
        let root = temp_dir("resolve-cargo");
        write(&root.join("Cargo.toml"), "[package]\nname = \"my-tool\"\n");
        write(&root.join("target/debug/app.exe"), "");

        let resolved = resolve_process(&process(
            3,
            Some(&root.join("target/debug/app.exe").display().to_string()),
            None,
        ))
        .expect("project resolved");

        assert_eq!(resolved.name, "my-tool");
        assert_eq!(resolved.kind, ProjectKind::Rust);
        assert_eq!(resolved.confidence, Confidence::High, "target/ is a dependency dir");
    }

    #[test]
    fn go_module_root_is_resolved() {
        let root = temp_dir("resolve-go");
        write(&root.join("go.mod"), "module example.com/tools/myapp\n");
        write(&root.join("bin/tool.exe"), "");

        let resolved = resolve_process(&process(
            4,
            None,
            Some(&format!("cmd /c {}", root.join("bin/tool.exe").display())),
        ))
        .expect("project resolved");

        assert_eq!(resolved.kind, ProjectKind::Go);
        assert_eq!(resolved.name, "myapp");
    }

    #[test]
    fn runtime_install_dir_yields_no_project() {
        // node.exe in its install dir: no markers anywhere up the tree.
        let resolved = resolve_process(&process(
            5,
            Some(r"C:\Program Files\nodejs\node.exe"),
            Some(r#""C:\Program Files\nodejs\node.exe" --eval "server""#),
        ));
        // Note: this asserts the *typical* machine layout; if a marker file
        // exists up the tree the test env differs — but Program Files does
        // not contain project manifests in practice.
        assert!(resolved.is_none());
    }

    #[test]
    fn inaccessible_process_is_never_resolved() {
        let mut inaccessible = process(6, Some(r"C:\Windows\System32\svchost.exe"), None);
        inaccessible.accessible = false;
        assert!(resolve_process(&inaccessible).is_none());
    }

    #[test]
    fn git_root_and_branch_are_attached() {
        let root = temp_dir("resolve-git");
        write(&root.join("package.json"), r#"{ "name": "proj" }"#);
        write(&root.join(".git/HEAD"), "ref: refs/heads/master\n");

        let resolved = resolve_process(&process(
            7,
            None,
            Some(&format!("node {}", root.join("server.js").display())),
        ))
        .expect("project resolved");

        assert!(resolved.git.isRepository);
        assert_eq!(resolved.git.branch.as_deref(), Some("master"));
        assert!(resolved.evidence.iter().any(|e| e.source == "git_root"));
    }

    // --- cycle-level resolution + cache ----------------------------------------

    fn setup_project(name: &str) -> PathBuf {
        let root = temp_dir(name);
        write(&root.join("package.json"), &format!(r#"{{ "name": "{name}" }}"#));
        root
    }

    #[test]
    fn multiple_pids_map_to_one_project_identity() {
        let root = setup_project("multi-pid");
        let script = root.join("node_modules/tool/bin/tool.js");
        write(&script, "//");
        let cl = format!(r#"node "{}""#, script.display());

        let processes = vec![process(10, None, Some(&cl)), process(11, None, Some(&cl))];
        let mut cache = ProjectCache::new();
        let (projects, links, stats) = resolve_projects(&processes, &mut cache, false);

        assert_eq!(projects.len(), 1, "one project, not two");
        assert_eq!(links.len(), 2);
        assert!(links.iter().all(|l| l.projectId == projects[0].id));
        assert_eq!(stats.projects, 1);
    }

    #[test]
    fn sibling_projects_are_not_merged() {
        let a = setup_project("proj-a");
        let b = setup_project("proj-b");
        let processes = vec![
            process(20, None, Some(&format!("node {}", a.join("s.js").display()))),
            process(21, None, Some(&format!("node {}", b.join("s.js").display()))),
        ];
        let mut cache = ProjectCache::new();
        let (projects, links, _) = resolve_projects(&processes, &mut cache, false);

        assert_eq!(projects.len(), 2, "siblings stay separate");
        assert_ne!(links[0].projectId, links[1].projectId);
    }

    #[test]
    fn warm_cycle_uses_cache_and_cold_cycle_resolves() {
        let root = setup_project("cache-cycles");
        let cl = format!(r#"node "{}""#, root.join("s.js").display());
        let processes = vec![process(30, None, Some(&cl))];

        let mut cache = ProjectCache::new();
        let (_, _, cold) = resolve_projects(&processes, &mut cache, false);
        assert_eq!(cold.resolved, 1);
        assert_eq!(cold.cache_hits, 0);

        let (_, _, warm) = resolve_projects(&processes, &mut cache, false);
        assert_eq!(warm.resolved, 0);
        assert_eq!(warm.cache_hits, 1);
    }

    #[test]
    fn bypass_cache_re_resolves_from_filesystem() {
        let root = setup_project("cache-bypass");
        let cl = format!(r#"node "{}""#, root.join("s.js").display());
        let processes = vec![process(31, None, Some(&cl))];

        let mut cache = ProjectCache::new();
        let (_, _, _) = resolve_projects(&processes, &mut cache, false);
        let (_, _, bypassed) = resolve_projects(&processes, &mut cache, true);
        assert_eq!(bypassed.resolved, 1, "manual refresh re-reads the filesystem");
        assert_eq!(bypassed.cache_hits, 0);
    }

    #[test]
    fn command_line_change_invalidates_cache_entry() {
        let root = setup_project("cache-key");
        let cl_a = format!(r#"node "{}""#, root.join("a.js").display());
        let cl_b = format!(r#"node "{}""#, root.join("b.js").display());
        write(&root.join("b.js"), "");

        let mut cache = ProjectCache::new();
        let (_, _, first) = resolve_projects(&[process(32, None, Some(&cl_a))], &mut cache, false);
        assert_eq!(first.resolved, 1);

        // Same PID, different command line → different key → re-resolved.
        let (_, _, second) = resolve_projects(&[process(32, None, Some(&cl_b))], &mut cache, false);
        assert_eq!(second.resolved, 1);
        assert_eq!(second.cache_hits, 0);
    }

    #[test]
    fn unknown_project_when_no_marker_exists() {
        let bare = temp_dir("no-marker");
        let processes = vec![process(40, None, Some(&format!("node {}", bare.join("s.js").display())))];
        let mut cache = ProjectCache::new();
        let (projects, links, _) = resolve_projects(&processes, &mut cache, false);
        assert!(projects.is_empty(), "no evidence → no project, honest unknown");
        assert!(links.is_empty());
    }

    #[test]
    fn cache_is_bounded() {
        let mut cache = ProjectCache::new();
        let root = setup_project("cache-bound");
        // Fill beyond capacity with distinct command lines.
        for i in 0..(CACHE_CAPACITY + 10) {
            let cl = format!(r#"node "{}" --arg-{i}"#, root.join("s.js").display());
            resolve_projects(&[process(1000 + i as u32, None, Some(&cl))], &mut cache, false);
        }
        assert!(cache.len() <= CACHE_CAPACITY);
    }
}
