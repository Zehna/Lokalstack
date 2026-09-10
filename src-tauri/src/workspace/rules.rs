//! Pure workspace rules — deterministic, fully unit-tested, no Windows calls
//! and no I/O beyond what the caller hands in as data.
//!
//! Everything the lifecycle engine decides is decided here first: candidate
//! derivation, launch-spec validation, duplicate/port-conflict preflight,
//! status derivation, ordering, log bounds, and state transitions.

use std::collections::VecDeque;

use serde::Serialize;

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

/// Role of a workspace service. Roles are labels, not assumptions: a
/// workspace only ever carries the roles its evidence supports. `Database`,
/// `Ai`, and `Worker` are reserved for future phases that can *derive* them
/// from evidence (managed databases, AI runners, background workers) —
/// Phase 6 never assigns them by guesswork.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // future-phase variants; see doc comment above
pub(crate) enum Role {
    Frontend,
    Backend,
    Database,
    Ai,
    Worker,
    Other,
}

impl Role {
    /// Deterministic launch order rank (lower launches first). External
    /// dependencies are never launched, so only managed roles rank here.
    pub(crate) fn order_rank(&self) -> u8 {
        match self {
            Role::Database => 0,
            Role::Backend => 1,
            Role::Worker => 2,
            Role::Frontend => 3,
            Role::Ai => 4,
            Role::Other => 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Launch spec + candidates
// ---------------------------------------------------------------------------

/// Structured launch specification. Never a raw shell string — `program` is
/// a resolved executable path and `args` are passed as an argument vector.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct LaunchSpec {
    /// Resolved absolute executable path (e.g. `C:\…\npm.cmd`,
    /// `C:\…\cargo.exe`).
    pub program: String,
    /// Structured arguments, passed individually (no shell interpolation).
    pub args: Vec<String>,
    /// Working directory — must remain inside the project root.
    pub cwd: String,
    /// How the program executes on Windows.
    pub kind: ProgramKind,
}

/// How Windows executes the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ProgramKind {
    /// A real executable — `CreateProcessW` runs it directly.
    Exe,
    /// A batch launcher (`npm.cmd`, `pnpm.cmd`, `yarn.cmd`, `*.bat`). Windows
    /// cannot `CreateProcessW` a batch file directly; it is run via
    /// `cmd.exe /d /s /c` with every argument individually quoted. This is
    /// the one genuinely unavoidable shell hop, and it is scoped: cmd runs
    /// exactly one command line assembled from structured parts.
    Batch,
}

/// One backend-derived launch candidate shown in the create-workspace flow.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct LaunchCandidate {
    pub role: Role,
    /// Display name, e.g. `Dev Server (vite)`.
    pub name: String,
    pub spec: LaunchSpec,
    /// Expected port, only when *evidence* states it (e.g. `--port 4173` in
    /// the script). Never guessed from convention.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expectedPort: Option<u16>,
    /// Where this candidate came from, e.g. `package.json scripts.dev`.
    pub source: String,
}

/// A script worth offering as a managed dev service.
const MANAGEABLE_SCRIPTS: &[&str] = &["dev", "start", "serve"];

/// Derive launch candidates from a Node project's `package.json` scripts.
///
/// Rules:
/// - Only scripts named `dev`, `start`, or `serve` are offered — the
///   long-running development services a workspace manages.
/// - The launch program is the *detected package manager's* launcher
///   (`npm.cmd run dev`, `pnpm.cmd dev`, …). With an `Ambiguous` manager no
///   candidate is produced — LocalStack refuses to guess a manager.
/// - An expected port is taken only from an explicit `--port <n>` in the
///   script body; conventional ports are never assumed.
pub(crate) fn node_launch_candidates(
    package_manager: crate::project::markers::PackageManager,
    scripts: &[(String, String)],
) -> Vec<LaunchCandidate> {
    let launcher = match package_manager {
        crate::project::markers::PackageManager::Npm => ("npm.cmd", "run"),
        crate::project::markers::PackageManager::Pnpm => ("pnpm.cmd", ""),
        crate::project::markers::PackageManager::Yarn => ("yarn.cmd", ""),
        crate::project::markers::PackageManager::Bun => ("bun.exe", "run"),
        // An ambiguous manager means we cannot name an honest launcher.
        crate::project::markers::PackageManager::Ambiguous => return Vec::new(),
    };

    let mut candidates = Vec::new();
    for (script, underlying) in scripts {
        let script_l = script.to_lowercase();
        if !MANAGEABLE_SCRIPTS.contains(&script_l.as_str()) {
            continue;
        }
        let mut args = Vec::new();
        if !launcher.1.is_empty() {
            args.push(launcher.1.to_string());
        }
        args.push(script.clone());
        // Expected port: explicit evidence only (`--port 4173` etc.).
        let expected_port = explicit_port(underlying);
        let display_tool = underlying.split_whitespace().next().unwrap_or("dev");
        // Role: the underlying tool names the service kind. Vite/webpack/
        // next dev servers are frontend; anything else is honest `Other` —
        // never an assumed role.
        let role = if matches!(
            display_tool.to_lowercase().as_str(),
            "vite" | "next" | "webpack" | "webpack-dev-server" | "react-scripts" | "astro" | "turbo"
        ) {
            Role::Frontend
        } else {
            Role::Other
        };
        candidates.push(LaunchCandidate {
            role,
            name: format!("Dev Server ({display_tool})"),
            spec: LaunchSpec {
                program: launcher.0.to_string(),
                args,
                cwd: String::new(), // filled by the caller (project root)
                kind: if launcher.0.ends_with(".exe") {
                    ProgramKind::Exe
                } else {
                    ProgramKind::Batch
                },
            },
            expectedPort: expected_port,
            source: format!("package.json scripts.{script}"),
        });
    }
    candidates
}

/// Extract a `--port <n>` (or `--port=<n>`) value from a command string.
fn explicit_port(command: &str) -> Option<u16> {
    let tokens: Vec<&str> = command.split_whitespace().collect();
    for (index, token) in tokens.iter().enumerate() {
        let value = if let Some(rest) = token.strip_prefix("--port=") {
            Some(rest)
        } else if *token == "--port" {
            tokens.get(index + 1).copied()
        } else {
            None
        };
        if let Some(value) = value {
            return value.parse().ok();
        }
    }
    None
}

/// Candidate for Rust projects: `cargo run` is a known, trusted runner.
pub(crate) fn cargo_launch_candidates() -> Vec<LaunchCandidate> {
    vec![LaunchCandidate {
        role: Role::Backend,
        name: "Dev Run (cargo run)".to_string(),
        spec: LaunchSpec {
            program: "cargo.exe".to_string(),
            args: vec!["run".to_string()],
            cwd: String::new(),
            kind: ProgramKind::Exe,
        },
        expectedPort: None,
        source: "Cargo.toml".to_string(),
    }]
}

/// Candidate for Go projects: `go run .` is a known, trusted runner.
pub(crate) fn go_launch_candidates() -> Vec<LaunchCandidate> {
    vec![LaunchCandidate {
        role: Role::Backend,
        name: "Dev Run (go run)".to_string(),
        spec: LaunchSpec {
            program: "go.exe".to_string(),
            args: vec!["run".to_string(), ".".to_string()],
            cwd: String::new(),
            kind: ProgramKind::Exe,
        },
        expectedPort: None,
        source: "go.mod".to_string(),
    }]
}

// ---------------------------------------------------------------------------
// Program resolution (pure — the caller supplies the PATH entries)
// ---------------------------------------------------------------------------

/// A resolved launch program.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedProgram {
    pub path: String,
    pub kind: ProgramKind,
}

/// Resolve a launch program against PATH entries (pure, testable).
///
/// - A program already containing a path separator is used as-is when it
///   exists (`.exe` direct, `.cmd`/`.bat` batch).
/// - Otherwise each PATH directory is searched for `<name>.exe` first, then
///   `<name>.cmd`, then `<name>.bat` — matching Windows launcher conventions
///   (`npm` → `npm.cmd`).
pub(crate) fn resolve_program(
    program: &str,
    path_entries: &[String],
    exists: &dyn Fn(&str) -> bool,
) -> Option<ResolvedProgram> {
    let has_separator = program.contains('\\') || program.contains('/');
    if has_separator {
        let kind = kind_of(program)?;
        return exists(program).then(|| ResolvedProgram {
            path: program.to_string(),
            kind,
        });
    }
    // A program that already carries an extension (`node.exe`, `npm.cmd`)
    // is tried as-is — appending suffixes would search `node.exe.exe`.
    let already_suffixed = [".exe", ".cmd", ".bat"].iter().any(|ext| program.to_lowercase().ends_with(ext));
    let candidates: Vec<(String, ProgramKind)> = if already_suffixed {
        vec![(program.to_string(), kind_of(program)?)]
    } else {
        [
            (".exe", ProgramKind::Exe),
            (".cmd", ProgramKind::Batch),
            (".bat", ProgramKind::Batch),
        ]
        .iter()
        .map(|(suffix, kind)| (format!("{program}{suffix}"), *kind))
        .collect()
    };
    for entry in path_entries {
        for (name, kind) in &candidates {
            let candidate = format!("{entry}\\{name}");
            if exists(&candidate) {
                return Some(ResolvedProgram {
                    path: candidate,
                    kind: *kind,
                });
            }
        }
    }
    None
}

fn kind_of(program: &str) -> Option<ProgramKind> {
    let lowered = program.to_lowercase();
    if lowered.ends_with(".exe") {
        Some(ProgramKind::Exe)
    } else if lowered.ends_with(".cmd") || lowered.ends_with(".bat") {
        Some(ProgramKind::Batch)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Launch-spec validation
// ---------------------------------------------------------------------------

/// Why a launch spec is stale/invalid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpecProblem {
    /// The project root directory no longer exists.
    RootMissing,
    /// The working directory is no longer inside the project root.
    CwdOutsideRoot,
    /// The program cannot be resolved on PATH / at its recorded path.
    ProgramMissing,
    /// A structured argument contains characters that cannot be passed
    /// safely (NUL, CR/LF).
    UnsafeArgument,
}

/// Validate a launch spec against current reality, inside the expected
/// project root. Pure over the caller-supplied probes — fully testable.
pub(crate) fn validate_spec(
    spec: &LaunchSpec,
    project_root: &str,
    path_entries: &[String],
    exists: &dyn Fn(&str) -> bool,
) -> Result<(), SpecProblem> {
    // 1. Project root still exists.
    if !exists(project_root) {
        return Err(SpecProblem::RootMissing);
    }
    // 2. cwd remains inside the project root (case-insensitive component
    //    prefix, Windows-style).
    let cwd = spec.cwd.replace('/', "\\");
    let root = project_root.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let cwd_l = cwd.trim_end_matches('\\').to_lowercase();
    if !(cwd_l == root || cwd_l.strip_prefix(&root).is_some_and(|rest| rest.starts_with('\\'))) {
        return Err(SpecProblem::CwdOutsideRoot);
    }
    // 3. Every argument is passable without surprises.
    for arg in &spec.args {
        if arg.is_empty()
            || arg.as_bytes().iter().any(|&b| b == 0 || b == b'\r' || b == b'\n')
        {
            return Err(SpecProblem::UnsafeArgument);
        }
    }
    // 4. The program still resolves.
    resolve_program(&spec.program, path_entries, exists).ok_or(SpecProblem::ProgramMissing)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Duplicate launch + port-conflict preflight
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Managed service / workspace status derivation
// ---------------------------------------------------------------------------

/// Lifecycle state of one managed service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub(crate) enum ManagedState {
    /// Launched, waiting for readiness (expected port or grace period).
    Starting,
    /// Ready: expected port observed, or process alive with no expected port.
    Running,
    /// Exited during `Starting` — includes the exit code when available.
    StartFailed { exit_code: Option<u32> },
    /// Exited after having been Running (user-initiated stop reports
    /// `Stopped`; this is an unplanned exit).
    Exited { exit_code: Option<u32> },
    /// Graceful stop in progress.
    Stopping,
    /// Graceful stop completed.
    Stopped,
    /// Graceful stop timed out; force stop is now offered.
    StopTimeout,
    /// Alive well past the readiness window with no expected port seen.
    Degraded,
}

impl ManagedState {
    /// Whether the underlying process is believed to be alive.
    pub(crate) fn alive(&self) -> bool {
        matches!(
            self,
            ManagedState::Starting | ManagedState::Running | ManagedState::Stopping | ManagedState::Degraded
        )
    }
}

/// Workspace-level status, derived from service states.
/// Derived workspace status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[allow(dead_code)] // `Conflict` is set by the launch preflight path (mod.rs)
pub(crate) enum WorkspaceStatus {
    Stopped,
    Starting,
    Running,
    Partial,
    Stopping,
    Error,
    Conflict,
}    /// Derive the workspace status from its services' states (pure).
    ///
    /// Precedence: any `StopTimeout` or `StartFailed` → `Error`… except a
    /// port-conflict block is reported as `Conflict` by the caller separately
    /// (it is a launch *preflight* outcome, not a process state). Otherwise any
    /// `Starting` → `Starting`, any `Stopping` → `Stopping`, mixed
    /// running/stopped → `Partial`, all running → `Running`, else `Stopped`.
    #[allow(dead_code)] // Conflict: set by the launch preflight path (mod.rs), not a process state.
    pub(crate) fn workspace_status(states: &[ManagedState]) -> WorkspaceStatus {
    if states.is_empty() {
        return WorkspaceStatus::Stopped;
    }
    if states.iter().any(|s| matches!(s, ManagedState::StopTimeout | ManagedState::StartFailed { .. })) {
        return WorkspaceStatus::Error;
    }
    if states.iter().any(|s| matches!(s, ManagedState::Starting)) {
        return WorkspaceStatus::Starting;
    }
    if states.iter().any(|s| matches!(s, ManagedState::Stopping)) {
        return WorkspaceStatus::Stopping;
    }
    let running = states.iter().any(|s| matches!(s, ManagedState::Running | ManagedState::Degraded));
    let stopped = states.iter().any(|s| matches!(s, ManagedState::Stopped | ManagedState::Exited { .. }));
    if running && stopped {
        WorkspaceStatus::Partial
    } else if running {
        WorkspaceStatus::Running
    } else {
        WorkspaceStatus::Stopped
    }
}

/// Log ring buffer
// ---------------------------------------------------------------------------
// Log ring buffer
// (Launch ordering moved to `crate::dependencies::graph::topological_order`
// in Phase 7: the dependency graph is authoritative, with
// `Role::order_rank` as the deterministic tiebreak for independent
// services.)
// ---------------------------------------------------------------------------

/// One captured output line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct LogLine {
    /// Unix epoch milliseconds when the line was captured.
    pub at: u64,
    /// `stdout` or `stderr`.
    pub stream: &'static str,
    pub line: String,
}

/// Bounded in-memory log ring — oldest lines drop when capacity is hit.
#[derive(Debug)]
pub(crate) struct LogRing {
    lines: VecDeque<LogLine>,
    capacity: usize,
    /// Monotonic index of the last line (for incremental polling).
    last_index: u64,
}

impl LogRing {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity.min(64)),
            capacity,
            last_index: 0,
        }
    }

    pub(crate) fn push(&mut self, line: LogLine) {
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.last_index += 1;
        self.lines.push_back(line);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Vec<LogLine> {
        self.lines.iter().cloned().collect()
    }

    /// Lines newer than `after_index` (incremental poll).
    pub(crate) fn since(&self, after_index: u64) -> Vec<LogLine> {
        let skip = self.lines.len().saturating_sub(
            usize::try_from(self.last_index.saturating_sub(after_index)).unwrap_or(usize::MAX),
        );
        self.lines.iter().skip(skip).cloned().collect()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.lines.len()
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(crate) fn last_index(&self) -> u64 {
        self.last_index
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::markers::PackageManager;

    fn spec(cwd: &str, program: &str) -> LaunchSpec {
        LaunchSpec {
            program: program.to_string(),
            args: vec!["run".to_string(), "dev".to_string()],
            cwd: cwd.to_string(),
            kind: ProgramKind::Batch,
        }
    }

    // --- candidates ---------------------------------------------------------

    #[test]
    fn npm_dev_script_yields_frontend_candidate() {
        let candidates = node_launch_candidates(
            PackageManager::Npm,
            &[("dev".to_string(), "vite".to_string())],
        );
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.role, Role::Frontend);
        assert_eq!(candidate.spec.program, "npm.cmd");
        assert_eq!(candidate.spec.args, vec!["run", "dev"]);
        assert_eq!(candidate.spec.kind, ProgramKind::Batch);
        assert!(candidate.expectedPort.is_none(), "no fabricated ports");
    }

    #[test]
    fn explicit_port_in_script_is_evidence_for_expected_port() {
        let candidates = node_launch_candidates(
            PackageManager::Npm,
            &[("dev".to_string(), "vite --port 4173".to_string())],
        );
        assert_eq!(candidates[0].expectedPort, Some(4173));
        let with_eq = node_launch_candidates(
            PackageManager::Npm,
            &[("dev".to_string(), "vite --port=5199".to_string())],
        );
        assert_eq!(with_eq[0].expectedPort, Some(5199));
    }

    #[test]
    fn ambiguous_manager_produces_no_candidates() {
        let candidates = node_launch_candidates(
            PackageManager::Ambiguous,
            &[("dev".to_string(), "vite".to_string())],
        );
        assert!(candidates.is_empty(), "never guess a manager");
    }

    #[test]
    fn non_manageable_scripts_are_ignored() {
        let candidates = node_launch_candidates(
            PackageManager::Npm,
            &[
                ("build".to_string(), "vite build".to_string()),
                ("test".to_string(), "vitest".to_string()),
            ],
        );
        assert!(candidates.is_empty());
    }

    #[test]
    fn pnpm_uses_bare_script_form() {
        let candidates = node_launch_candidates(
            PackageManager::Pnpm,
            &[("dev".to_string(), "vite".to_string())],
        );
        assert_eq!(candidates[0].spec.args, vec!["dev"]);
    }

    #[test]
    fn rust_and_go_projects_have_known_runners() {
        let cargo = cargo_launch_candidates();
        assert_eq!(cargo[0].spec.program, "cargo.exe");
        assert_eq!(cargo[0].spec.args, vec!["run"]);
        let go = go_launch_candidates();
        assert_eq!(go[0].spec.program, "go.exe");
        assert_eq!(go[0].spec.args, vec!["run", "."]);
    }

    // --- program resolution ---------------------------------------------------

    #[test]
    fn bare_program_resolves_through_path_entries() {
        let path = vec!["C:\\Program Files\\nodejs".to_string(), "C:\\Windows".to_string()];
        let resolved = resolve_program("npm", &path, &|p| p.ends_with("nodejs\\npm.cmd"));
        assert_eq!(
            resolved,
            Some(ResolvedProgram {
                path: "C:\\Program Files\\nodejs\\npm.cmd".to_string(),
                kind: ProgramKind::Batch,
            })
        );
    }

    #[test]
    fn exe_wins_over_cmd_in_the_same_directory() {
        let path = vec!["C:\\tools".to_string()];
        let resolved = resolve_program("bun", &path, &|p| p.ends_with("tools\\bun.exe"));
        assert!(matches!(resolved, Some(r) if r.kind == ProgramKind::Exe));
    }

    #[test]
    fn unresolved_program_is_none() {
        let path = vec!["C:\\nowhere".to_string()];
        assert_eq!(resolve_program("missing", &path, &|_| false), None);
    }

    // --- spec validation --------------------------------------------------------

    #[test]
    fn valid_spec_passes() {
        let exists = |p: &str| {
            p == r"D:\Projects\app"
                || p == r"C:\Program Files\nodejs\npm.cmd"
                || p.ends_with("nodejs\\npm.cmd")
        };
        let mut s = spec(r"D:\Projects\app", "npm");
        s.cwd = r"D:\Projects\app".to_string();
        let resolved = resolve_program(
            "npm",
            &[r"C:\Program Files\nodejs".to_string()],
            &exists,
        );
        assert!(
            resolved.is_some(),
            "test PATH entry must contain a resolvable npm.cmd"
        );
        assert!(
            validate_spec(&s, r"D:\Projects\app", &[r"C:\Program Files\nodejs".to_string()], &exists)
                .is_ok()
        );
    }

    #[test]
    fn missing_root_is_rejected() {
        let exists = |p: &str| p.ends_with("npm.cmd");
        let mut s = spec(r"D:\Projects\gone", "npm");
        s.cwd = r"D:\Projects\gone".to_string();
        assert_eq!(
            validate_spec(&s, r"D:\Projects\gone", &[], &exists),
            Err(SpecProblem::RootMissing)
        );
    }

    #[test]
    fn cwd_outside_root_is_rejected() {
        let exists = |p: &str| p.starts_with(r"D:\Projects") || p.ends_with("npm.cmd");
        let mut s = spec(r"D:\Other\app", "npm");
        s.cwd = r"D:\Other\app".to_string();
        assert_eq!(
            validate_spec(&s, r"D:\Projects\app", &[], &exists),
            Err(SpecProblem::CwdOutsideRoot)
        );
    }

    #[test]
    fn sibling_prefix_root_is_not_treated_as_containment() {
        // `D:\Projects\app2` must NOT satisfy containment in `D:\Projects\app`.
        let exists = |p: &str| p.starts_with(r"D:\Projects") || p.ends_with("npm.cmd");
        let mut s = spec(r"D:\Projects\app2", "npm");
        s.cwd = r"D:\Projects\app2".to_string();
        assert_eq!(
            validate_spec(&s, r"D:\Projects\app", &[], &exists),
            Err(SpecProblem::CwdOutsideRoot)
        );
    }

    #[test]
    fn unresolvable_program_is_stale() {
        let exists = |p: &str| p == r"D:\Projects\app";
        let mut s = spec(r"D:\Projects\app", "npm");
        s.cwd = r"D:\Projects\app".to_string();
        assert_eq!(
            validate_spec(&s, r"D:\Projects\app", &[], &exists),
            Err(SpecProblem::ProgramMissing)
        );
    }

    #[test]
    fn argument_with_newline_is_unsafe() {
        let exists = |p: &str| p == r"D:\Projects\app" || p.ends_with("npm.cmd");
        let mut s = spec(r"D:\Projects\app", "npm");
        s.args.push("bad\narg".to_string());
        assert_eq!(
            validate_spec(&s, r"D:\Projects\app", &[], &exists),
            Err(SpecProblem::UnsafeArgument)
        );
    }

    // --- status derivation ---------------------------------------------------------

    #[test]
    fn workspace_status_precedence() {
        let stopped = [ManagedState::Stopped, ManagedState::Stopped];
        assert_eq!(workspace_status(&stopped), WorkspaceStatus::Stopped);

        let running = [ManagedState::Running, ManagedState::Running];
        assert_eq!(workspace_status(&running), WorkspaceStatus::Running);

        let partial = [ManagedState::Running, ManagedState::Stopped];
        assert_eq!(workspace_status(&partial), WorkspaceStatus::Partial);

        let starting = [ManagedState::Starting, ManagedState::Running];
        assert_eq!(workspace_status(&starting), WorkspaceStatus::Starting);

        let stopping = [ManagedState::Stopping, ManagedState::Running];
        assert_eq!(workspace_status(&stopping), WorkspaceStatus::Stopping);

        let failed = [ManagedState::StartFailed { exit_code: Some(1) }, ManagedState::Running];
        assert_eq!(workspace_status(&failed), WorkspaceStatus::Error);

        let timeout = [ManagedState::StopTimeout];
        assert_eq!(workspace_status(&timeout), WorkspaceStatus::Error);

        assert_eq!(workspace_status(&[]), WorkspaceStatus::Stopped);
    }

    #[test]
    fn degraded_counts_as_alive_for_status() {
        let states = [ManagedState::Degraded, ManagedState::Stopped];
        assert_eq!(workspace_status(&states), WorkspaceStatus::Partial);
        assert!(ManagedState::Degraded.alive());
    }

    // --- log ring ---------------------------------------------------------------------

    #[test]
    fn log_ring_is_bounded_and_ordered() {
        let mut ring = LogRing::new(3);
        for i in 0..5u64 {
            ring.push(LogLine {
                at: i,
                stream: "stdout",
                line: format!("line {i}"),
            });
        }
        assert_eq!(ring.len(), 3, "capacity is respected");
        let lines = ring.snapshot();
        assert_eq!(lines[0].line, "line 2", "oldest lines drop first");
        assert_eq!(lines[2].line, "line 4");
    }

    /// Phase 10B (spec §J): a pathological child emitting a huge burst of
    /// output (and binary garbage) must not grow the ring beyond capacity,
    /// must not panic on invalid UTF-8 (lossy conversion), and must keep
    /// the newest lines visible.
    #[test]
    fn log_ring_survives_flood_and_binary_output() {
        let mut ring = LogRing::new(1_000);
        // 50_000 lines — far beyond capacity; only the last 1_000 remain.
        for i in 0..50_000u64 {
            ring.push(LogLine {
                at: i,
                stream: "stdout",
                line: format!("flood {i}"),
            });
        }
        assert_eq!(ring.len(), 1_000, "flood must not exceed the bound");
        let lines = ring.snapshot();
        assert_eq!(lines[0].line, "flood 49000", "oldest evicted, newest kept");
        assert_eq!(lines[999].line, "flood 49999");
        // Memory stays bounded by capacity, not by total pushed lines — the
        // VecDeque can never exceed `capacity` entries (proven by len).

        // Binary garbage must not panic (lossy conversion happens upstream
        // in the pipe reader; the ring stores whatever text arrives).
        let weird = "\u{0}\u{1}\u{FFFD} broken \u{7f}";
        ring.push(LogLine { at: 1, stream: "stderr", line: weird.to_string() });
        assert_eq!(ring.snapshot().last().map(|l| l.line.as_str()), Some(weird));
    }

    #[test]
    fn log_ring_incremental_since() {
        let mut ring = LogRing::new(100);
        for i in 0..3u64 {
            ring.push(LogLine { at: i, stream: "stderr", line: format!("{i}") });
        }
        let fresh = ring.since(1);
        assert_eq!(fresh.len(), 2, "only lines after index 1");
        assert_eq!(fresh[0].line, "1");
    }

    // --- state helpers --------------------------------------------------------------------

    #[test]
    fn alive_states() {
        assert!(ManagedState::Starting.alive());
        assert!(ManagedState::Running.alive());
        assert!(ManagedState::Stopping.alive());
        assert!(ManagedState::Degraded.alive());
        assert!(!ManagedState::Stopped.alive());
        assert!(!ManagedState::StartFailed { exit_code: None }.alive());
        assert!(!ManagedState::Exited { exit_code: None }.alive());
        assert!(!ManagedState::StopTimeout.alive());
    }
}
