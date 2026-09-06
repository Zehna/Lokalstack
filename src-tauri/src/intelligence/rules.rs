//! Pure, deterministic service- and framework-detection rules.
//!
//! # Product principle
//!
//! **Never claim a framework or service identity without sufficient
//! evidence.** "Node.js" is always preferred over an incorrect "Next.js".
//! Every identity carries an explicit confidence and the evidence that
//! produced it, so the UI (and the user) can judge why.
//!
//! # Model
//!
//! `ProcessEvidence` (normalized process facts) → [`detect_service`] →
//! [`ServiceIdentity`] (kind, display name, category, confidence, evidence).
//!
//! Everything in this module is pure and deterministic — no Windows calls,
//! fully unit-testable. The input evidence is produced by the process engine
//! (executable path + command line via the PEB walk) and the port engine
//! (listening ports, as *weak supporting* evidence only — a port number
//! never produces a high-confidence identity).

use serde::{Deserialize, Serialize};

use crate::process::ProcessInfo;

/// How sure the detector is about an identity.
///
/// An enum rather than a number: there is no meaningful arithmetic behind a
/// percentage, and honest buckets are easier to render and to reason about.
/// Ordered so stronger confidences compare greater.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Confidence {
    /// Weak indication, may easily be wrong (e.g. path mentions).
    Low,
    /// Plausible but not conclusive (e.g. module-ish arguments).
    Medium,
    /// Strong command-line or path evidence.
    High,
    /// The executable itself identifies the service (postgres.exe, ollama.exe).
    Exact,
}

/// What a detected service is used for, from a developer's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ServiceCategory {
    Frontend,
    Backend,
    Database,
    Ai,
    Infrastructure,
    Unknown,
}

/// The concrete identity kinds the detector can emit. `Unknown*` kinds are
/// the honest fallbacks: the runtime is known, the framework is not claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ServiceKind {
    // --- Frontend / JS runtimes & frameworks -------------------------------
    NodeJs,
    Vite,
    NextJs,
    Express,
    // --- Python runtimes & frameworks ---------------------------------------
    Python,
    Flask,
    Uvicorn,
    FastApi,
    Django,
    Gunicorn,
    // --- Databases ----------------------------------------------------------
    PostgreSql,
    MySql,
    MariaDb,
    Redis,
    // --- Local AI -----------------------------------------------------------
    Ollama,
    LlamaCpp,
    ComfyUi,
    Gradio,
    OpenWebUi,
    // --- Infrastructure -----------------------------------------------------
    DockerDesktop,
    // --- Fallbacks ----------------------------------------------------------
    /// A Windows-native executable we cannot further classify.
    WindowsProcess,
    /// A known runtime without framework evidence (honest fallback).
    UnknownRuntime,
    /// Nothing meaningful is known (inaccessible process).
    Unknown,
}

/// One piece of evidence backing a classification, retained for the future
/// Details view and for debugging false positives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub(crate) struct Evidence {
    /// Where the evidence came from: `process_name`, `executable_path`,
    /// `command_line`, `argument`.
    pub source: String,
    /// The matched value (the substring or argument that fired the rule).
    pub value: String,
}

impl Evidence {
    pub(crate) fn new(source: &str, value: impl Into<String>) -> Self {
        Self {
            source: source.to_string(),
            value: value.into(),
        }
    }
}

/// The result of classification: what it is, how sure, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(non_snake_case)]
pub(crate) struct ServiceIdentity {
    /// The concrete identity kind.
    pub kind: ServiceKind,
    /// Human-facing display name, e.g. `PostgreSQL`, `Vite`, `Node.js`.
    pub displayName: String,
    /// Developer-facing category.
    pub category: ServiceCategory,
    /// How confident the detector is — never a fabricated number.
    pub confidence: Confidence,
    /// The evidence that produced this identity, strongest first.
    pub evidence: Vec<Evidence>,
}

impl ServiceIdentity {
    /// Build an identity with a single evidence item.
    fn new(
        kind: ServiceKind,
        display_name: &str,
        category: ServiceCategory,
        confidence: Confidence,
        evidence: Vec<Evidence>,
    ) -> Self {
        Self {
            kind,
            displayName: display_name.to_string(),
            category,
            confidence,
            evidence,
        }
    }
}

/// Normalized input facts for the detector. Constructed from a
/// [`ProcessInfo`] plus the PID's listening ports (weak evidence only).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProcessEvidence {
    /// Executable basename, lowercased (Windows is case-insensitive), e.g.
    /// `node.exe`, `postgres.exe`. `None` when the process was inaccessible.
    pub executable: Option<String>,
    /// Original-cased executable basename for display (fallback identities
    /// keep the real name, e.g. `OneDrive.Sync.Service.exe`).
    pub executable_display: Option<String>,
    /// Full executable path, lowercased, when known.
    pub executable_path: Option<String>,
    /// Full command line when readable (Phase 3 PEB walk), lowercased is
    /// applied at matching time; stored verbatim here for display.
    pub command_line: Option<String>,
    /// Ports this PID listens on (weak supporting evidence only).
    pub ports: Vec<u16>,
}

impl ProcessEvidence {
    /// Build from a process snapshot plus the PID's listening ports.
    pub(crate) fn from_process(process: &ProcessInfo, ports: Vec<u16>) -> Self {
        Self {
            executable: process.name.as_ref().map(|name| name.to_lowercase()),
            executable_display: process.name.clone(),
            executable_path: process
                .executablePath
                .as_ref()
                .map(|path| path.to_lowercase()),
            command_line: process.commandLine.clone(),
            ports,
        }
    }

    /// Lowercased command line for matching.
    fn cmdline_lc(&self) -> Option<&str> {
        self.command_line.as_deref()
    }
}

/// Everything the detector knows about one concrete service identity.
struct Def {
    kind: ServiceKind,
    display_name: &'static str,
    category: ServiceCategory,
}

const POSTGRESQL: Def = Def { kind: ServiceKind::PostgreSql, display_name: "PostgreSQL", category: ServiceCategory::Database };
const MYSQL: Def = Def { kind: ServiceKind::MySql, display_name: "MySQL", category: ServiceCategory::Database };
const MARIADB: Def = Def { kind: ServiceKind::MariaDb, display_name: "MariaDB", category: ServiceCategory::Database };
const REDIS: Def = Def { kind: ServiceKind::Redis, display_name: "Redis", category: ServiceCategory::Database };
const OLLAMA: Def = Def { kind: ServiceKind::Ollama, display_name: "Ollama", category: ServiceCategory::Ai };
const LLAMA_CPP: Def = Def { kind: ServiceKind::LlamaCpp, display_name: "llama.cpp", category: ServiceCategory::Ai };
const COMFY_UI: Def = Def { kind: ServiceKind::ComfyUi, display_name: "ComfyUI", category: ServiceCategory::Ai };
const OPEN_WEBUI: Def = Def { kind: ServiceKind::OpenWebUi, display_name: "Open WebUI", category: ServiceCategory::Ai };
const DOCKER: Def = Def { kind: ServiceKind::DockerDesktop, display_name: "Docker Desktop", category: ServiceCategory::Infrastructure };
const NEXT_JS: Def = Def { kind: ServiceKind::NextJs, display_name: "Next.js", category: ServiceCategory::Frontend };
const VITE: Def = Def { kind: ServiceKind::Vite, display_name: "Vite", category: ServiceCategory::Frontend };
const EXPRESS: Def = Def { kind: ServiceKind::Express, display_name: "Express", category: ServiceCategory::Backend };
const NODE_JS: Def = Def { kind: ServiceKind::NodeJs, display_name: "Node.js", category: ServiceCategory::Backend };
const FLASK: Def = Def { kind: ServiceKind::Flask, display_name: "Flask", category: ServiceCategory::Backend };
const UVICORN: Def = Def { kind: ServiceKind::Uvicorn, display_name: "Uvicorn", category: ServiceCategory::Backend };
const FASTAPI: Def = Def { kind: ServiceKind::FastApi, display_name: "FastAPI", category: ServiceCategory::Backend };
const DJANGO: Def = Def { kind: ServiceKind::Django, display_name: "Django", category: ServiceCategory::Backend };
const GUNICORN: Def = Def { kind: ServiceKind::Gunicorn, display_name: "Gunicorn", category: ServiceCategory::Backend };
const GRADIO: Def = Def { kind: ServiceKind::Gradio, display_name: "Gradio", category: ServiceCategory::Ai };
const PYTHON: Def = Def { kind: ServiceKind::Python, display_name: "Python", category: ServiceCategory::Backend };
const JAVA: Def = Def { kind: ServiceKind::UnknownRuntime, display_name: "Java Process", category: ServiceCategory::Backend };

/// Returns true when `haystack` contains `needle` as a whole "word-ish"
/// token: surrounded by delimiters that are plausible in a command line
/// (start/end, whitespace, quotes, path separators, `=`, `:`).
///
/// This prevents the classic false positive where matching the substring
/// "run" would fire inside "flask run", or "next" inside "--experimental".
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

/// Quote-aware argument split of a command line (Windows-ish rules: double
/// quotes group, embedded quotes are rare and simply split here). The first
/// element is the executable. Used by the quoted-argument handling tests;
/// detection itself matches tokens over the whole command line.
#[cfg_attr(not(test), allow(dead_code))]
fn split_args(command_line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in command_line.chars() {
        match ch {
            '"' => {
                in_quotes = !in_quotes;
                // Keep quote boundaries as arg delimiters; the quotes themselves
                // are dropped.
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            ' ' | '\t' if !in_quotes => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// Rule table entry for executable-based detection: (basename, definition,
/// confidence). The basename comparison is case-insensitive and exact.
const EXACT_EXECUTABLE_RULES: &[(&str, Def, Confidence)] = &[
    ("postgres.exe", POSTGRESQL, Confidence::Exact),
    ("mysqld.exe", MYSQL, Confidence::Exact),
    ("mariadbd.exe", MARIADB, Confidence::Exact),
    ("redis-server.exe", REDIS, Confidence::Exact),
    ("ollama.exe", OLLAMA, Confidence::Exact),
    ("ollama_llama_server.exe", LLAMA_CPP, Confidence::High),
    ("llama-server.exe", LLAMA_CPP, Confidence::High),
    ("llama-cli.exe", LLAMA_CPP, Confidence::High),
    ("comfyui.exe", COMFY_UI, Confidence::High),
    ("open-webui.exe", OPEN_WEBUI, Confidence::High),
    ("flask.exe", FLASK, Confidence::High),
    ("docker desktop.exe", DOCKER, Confidence::High),
    ("com.docker.dev-envs.exe", DOCKER, Confidence::Medium),
    ("com.docker.service", DOCKER, Confidence::Medium),
];

/// Command-line rules, evaluated per runtime family in priority order.
/// Each entry: (needle tokens that must all appear, definition, confidence,
/// evidence source label).
const NODE_CMD_RULES: &[(&[&str], Def, Confidence)] = &[
    // `next dev` / `next start` / next-server (Next.js's own runner binary).
    (&["next", "dev"], NEXT_JS, Confidence::High),
    (&["next", "start"], NEXT_JS, Confidence::High),
    (&["next-server"], NEXT_JS, Confidence::High),
    // Vite: `vite`, `node .../vite.js`, `node .../bin/vite`.
    (&["vite"], VITE, Confidence::High),
    // Express: only when the entry script itself is the express binary.
    (&["express"], EXPRESS, Confidence::Medium),
];

const PYTHON_CMD_RULES: &[(&[&str], Def, Confidence)] = &[
    // `-m flask` / `flask run` — Flask's own runner.
    (&["-m", "flask"], FLASK, Confidence::High),
    (&["flask", "run"], FLASK, Confidence::High),
    // manage.py runserver — Django's dev server.
    (&["manage.py", "runserver"], DJANGO, Confidence::High),
    // Uvicorn runner; NOT FastAPI — uvicorn serves any ASGI app.
    (&["uvicorn"], UVICORN, Confidence::High),
    (&["-m", "uvicorn"], UVICORN, Confidence::High),
    // FastAPI's own runner (fastapi-cli). Uvicorn alone must stay Uvicorn.
    (&["-m", "fastapi"], FASTAPI, Confidence::Medium),
    (&["fastapi", "run"], FASTAPI, Confidence::Medium),
    // Gunicorn (rare on Windows, but explicit).
    (&["gunicorn"], GUNICORN, Confidence::High),
];

const AI_CMD_RULES: &[(&[&str], Def, Confidence)] = &[
    // ComfyUI's main script is main.py inside a ComfyUI folder — path token.
    (&["comfyui"], COMFY_UI, Confidence::High),
    // Gradio's CLI (`gradio run`) — only the CLI fires, never a mere port or
    // an import that is not visible in the command line.
    (&["gradio"], GRADIO, Confidence::Medium),
    // Both spellings: the pip module is `open_webui`, the console script
    // `open-webui`.
    (&["open_webui"], OPEN_WEBUI, Confidence::Medium),
    (&["open-webui"], OPEN_WEBUI, Confidence::Medium),
];

/// Executables that are always classified by their runtime name first.
const NODE_RUNTIME_BASENAMES: &[&str] = &["node.exe", "node"];
const PYTHON_RUNTIME_BASENAMES: &[&str] = &[
    "python.exe", "python3.exe", "python3.13.exe", "python3.12.exe",
    "python3.11.exe", "python3.10.exe", "pythonw.exe", "python",
];
const JAVA_RUNTIME_BASENAMES: &[&str] = &["java.exe", "javaw.exe"];

/// Classify one process from normalized evidence.
///
/// Priority: exact executables → command-line rules scoped to the detected
/// runtime → runtime fallbacks. The result is always a usable identity —
/// at worst "unknown.exe" with `Unknown`/`Low` or "Node.js" with
/// `UnknownRuntime`.
#[must_use]
pub(crate) fn detect_service(evidence: &ProcessEvidence) -> ServiceIdentity {
    let executable = evidence.executable.as_deref();
    let cmdline = evidence.cmdline_lc();
    let path = evidence.executable_path.as_deref();

    // 1. Exact executable rules — the executable itself identifies the
    //    service (postgres.exe → PostgreSQL). Strongest evidence there is.
    //    Case-insensitive: Windows paths are.
    if let Some(exe) = executable {
        for (basename, def, confidence) in EXACT_EXECUTABLE_RULES {
            if exe.eq_ignore_ascii_case(basename) {
                return ServiceIdentity::new(
                    def.kind,
                    def.display_name,
                    def.category,
                    *confidence,
                    vec![
                        Evidence::new("process_name", *basename),
                        path.map(|p| Evidence::new("executable_path", p.to_string()))
                            .unwrap_or_else(|| Evidence::new("process_name", *basename)),
                    ],
                );
            }
        }
    }

    // 2. Node family: framework from command line, else honest Node.js.
    if let Some(exe) = executable {
        if NODE_RUNTIME_BASENAMES.iter().any(|b| exe.eq_ignore_ascii_case(b)) {
            if let Some(identity) = match_rules(NODE_CMD_RULES, cmdline, "command_line") {
                return identity;
            }
            return ServiceIdentity::new(
                NODE_JS.kind,
                NODE_JS.display_name,
                NODE_JS.category,
                Confidence::Exact, // the runtime identification is exact…
                vec![Evidence::new("process_name", exe)],
            );
        }
    }

    // 3. Python family: framework from command line, else honest Python.
    if let Some(exe) = executable {
        if PYTHON_RUNTIME_BASENAMES.iter().any(|b| exe.eq_ignore_ascii_case(b)) {
            if let Some(identity) = match_rules(PYTHON_CMD_RULES, cmdline, "command_line") {
                return identity;
            }
            // AI runners (ComfyUI, Gradio, Open WebUI) are recognizable from
            // distinctive folder/package tokens; check the command line and
            // the executable path. Port numbers are deliberately NOT input.
            if let Some(identity) = match_rules(AI_CMD_RULES, cmdline, "command_line") {
                return identity;
            }
            if let Some(path) = path {
                if let Some(identity) = match_rules(AI_CMD_RULES, Some(path), "executable_path") {
                    return identity;
                }
            }
            return ServiceIdentity::new(
                PYTHON.kind,
                PYTHON.display_name,
                PYTHON.category,
                Confidence::Exact,
                vec![Evidence::new("process_name", exe)],
            );
        }
    }

    // 4. Java family fallback.
    if let Some(exe) = executable {
        if JAVA_RUNTIME_BASENAMES.iter().any(|b| exe.eq_ignore_ascii_case(b)) {
            return ServiceIdentity::new(
                JAVA.kind,
                JAVA.display_name,
                JAVA.category,
                Confidence::Exact,
                vec![Evidence::new("process_name", exe)],
            );
        }
    }

    // 5. Generic fallback: keep the real executable name, honest Unknown.
    let display = evidence
        .executable_display
        .as_deref()
        .or(executable);
    match display {
        Some(exe) => ServiceIdentity::new(
            ServiceKind::WindowsProcess,
            exe,
            ServiceCategory::Unknown,
            Confidence::Low,
            vec![Evidence::new(
                "process_name",
                executable.unwrap_or(exe),
            )],
        ),
        None => ServiceIdentity::new(
            ServiceKind::Unknown,
            "Unavailable",
            ServiceCategory::Unknown,
            Confidence::Low,
            vec![Evidence::new("process_state", "inaccessible")],
        ),
    }
}

/// Evaluate a rule table against the lowercased haystack. All tokens of a
/// rule must match as tokens. On success, build the identity with one
/// Evidence entry per matched token (strongest first).
fn match_rules(
    rules: &[(&[&str], Def, Confidence)],
    haystack: Option<&str>,
    source: &str,
) -> Option<ServiceIdentity> {
    let haystack = haystack?;
    for (tokens, def, confidence) in rules {
        let matched: Vec<Evidence> = tokens
            .iter()
            .filter(|token| contains_token(haystack, token))
            .map(|token| Evidence::new(source, (*token).to_string()))
            .collect();
        if matched.len() == tokens.len() {
            return Some(ServiceIdentity::new(
                def.kind,
                def.display_name,
                def.category,
                *confidence,
                matched,
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evidence helper: executable + optional command line (verbatim; the
    /// detector lowercases at match time where needed).
    fn ev(executable: &str, command_line: Option<&str>) -> ProcessEvidence {
        ProcessEvidence {
            executable: Some(executable.to_string()),
            executable_display: Some(executable.to_string()),
            executable_path: Some(format!(r"c:\program files\{executable}")),
            command_line: command_line.map(str::to_string),
            ports: vec![],
        }
    }

    fn evidence_of<'a>(identity: &'a ServiceIdentity, source: &'a str) -> Vec<&'a Evidence> {
        identity.evidence.iter().filter(|e| e.source == source).collect()
    }

    // --- exact executables ------------------------------------------------

    #[test]
    fn postgres_executable_is_exact_postgresql() {
        let id = detect_service(&ev("postgres.exe", None));
        assert_eq!(id.kind, ServiceKind::PostgreSql);
        assert_eq!(id.displayName, "PostgreSQL");
        assert_eq!(id.category, ServiceCategory::Database);
        assert_eq!(id.confidence, Confidence::Exact);
        assert!(!evidence_of(&id, "process_name").is_empty());
    }

    #[test]
    fn ollama_executable_is_exact_ollama() {
        let id = detect_service(&ev("ollama.exe", Some(r#""C:\Users\x\ollama.exe" serve"#)));
        assert_eq!(id.kind, ServiceKind::Ollama);
        assert_eq!(id.confidence, Confidence::Exact);
    }

    #[test]
    fn llama_server_is_high_confidence_llama_cpp() {
        let id = detect_service(&ev("llama-server.exe", None));
        assert_eq!(id.kind, ServiceKind::LlamaCpp);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn mysql_and_redis_executables_are_exact() {
        assert_eq!(detect_service(&ev("mysqld.exe", None)).kind, ServiceKind::MySql);
        assert_eq!(detect_service(&ev("redis-server.exe", None)).kind, ServiceKind::Redis);
    }

    #[test]
    fn executable_matching_is_case_insensitive() {
        // Windows paths are case-insensitive; POSTGRES.EXE must classify.
        let mut e = ev("POSTGRES.EXE", None);
        e.executable = Some("POSTGRES.EXE".to_string());
        e.executable_path = Some(r"C:\PROGRAM FILES\POSTGRESQL\BIN\POSTGRES.EXE".to_string());
        let id = detect_service(&e);
        assert_eq!(id.kind, ServiceKind::PostgreSql);
        assert_eq!(id.confidence, Confidence::Exact);
    }

    // --- node family --------------------------------------------------------

    #[test]
    fn node_with_next_dev_is_nextjs() {
        let id = detect_service(&ev(
            "node.exe",
            Some(r#""C:\Program Files\nodejs\node.exe" "D:\proj\node_modules\.bin\next" dev"#),
        ));
        assert_eq!(id.kind, ServiceKind::NextJs);
        assert_eq!(id.category, ServiceCategory::Frontend);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn node_with_next_start_is_nextjs() {
        let id = detect_service(&ev("node.exe", Some("node next start -p 3000")));
        assert_eq!(id.kind, ServiceKind::NextJs);
    }

    #[test]
    fn node_with_vite_is_vite() {
        let id = detect_service(&ev(
            "node.exe",
            Some(r#""C:\Program Files\nodejs\node.exe" "D:\proj\node_modules\.bin\vite""#),
        ));
        assert_eq!(id.kind, ServiceKind::Vite);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn node_with_vite_js_script_is_vite() {
        let id = detect_service(&ev("node.exe", Some(r"node D:\proj\node_modules\vite\bin\vite.js --port 5173")));
        assert_eq!(id.kind, ServiceKind::Vite);
    }

    #[test]
    fn node_alone_is_honest_nodejs_not_nextjs() {
        let id = detect_service(&ev("node.exe", Some(r#"node "D:\proj\server.js""#)));
        assert_eq!(id.kind, ServiceKind::NodeJs);
        assert_eq!(id.displayName, "Node.js");
        assert_eq!(id.confidence, Confidence::Exact);
        assert_eq!(id.category, ServiceCategory::Backend);
    }

    #[test]
    fn node_server_js_must_not_become_express() {
        // The classic false positive we refuse: `node server.js` stays Node.js.
        let id = detect_service(&ev("node.exe", Some("node server.js")));
        assert_eq!(id.kind, ServiceKind::NodeJs);
    }

    #[test]
    fn node_word_next_in_unrelated_flag_must_not_fire() {
        // "--experimental" contains "next"?? No — but "ex-next" style tokens
        // must not match; verify token boundaries via a custom flag.
        let id = detect_service(&ev("node.exe", Some("node app.js --flag=nextstuff")));
        assert_eq!(id.kind, ServiceKind::NodeJs);
    }

    #[test]
    fn node_with_next_but_no_dev_or_start_stays_nodejs() {
        // `next` present but neither dev/start/next-server: not conclusive.
        let id = detect_service(&ev("node.exe", Some("node build-system/next-transform.js build")));
        assert_eq!(id.kind, ServiceKind::NodeJs);
    }

    // --- port-number anti-rules ----------------------------------------------

    #[test]
    fn port_3000_alone_must_not_produce_nextjs() {
        // node.exe listening on 3000 with a plain command line: Node.js.
        let mut e = ev("node.exe", Some("node server.js"));
        e.ports = vec![3000];
        let id = detect_service(&e);
        assert_eq!(id.kind, ServiceKind::NodeJs);
    }

    #[test]
    fn port_7860_alone_must_not_produce_gradio() {
        let mut e = ev("python.exe", Some("python app.py"));
        e.ports = vec![7860];
        let id = detect_service(&e);
        assert_eq!(id.kind, ServiceKind::Python);
    }

    #[test]
    fn port_5432_alone_must_not_produce_postgresql() {
        // Some non-postgres binary on 5432 must stay unknown.
        let mut e = ev("myserver.exe", Some("myserver --port 5432"));
        e.ports = vec![5432];
        let id = detect_service(&e);
        assert_eq!(id.kind, ServiceKind::WindowsProcess);
        assert_ne!(id.kind, ServiceKind::PostgreSql);
    }

    // --- python family --------------------------------------------------------

    #[test]
    fn python_with_flask_run_is_flask() {
        let id = detect_service(&ev("python.exe", Some(r#"python -m flask run --port 5000"#)));
        assert_eq!(id.kind, ServiceKind::Flask);
        assert_eq!(id.category, ServiceCategory::Backend);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn python_with_flask_command_is_flask() {
        let id = detect_service(&ev("flask.exe", Some(r#""C:\venv\Scripts\flask.exe" run"#)));
        assert_eq!(id.kind, ServiceKind::Flask);
    }

    #[test]
    fn python_with_manage_py_runserver_is_django() {
        let id = detect_service(&ev("python.exe", Some(r"python manage.py runserver 8000")));
        assert_eq!(id.kind, ServiceKind::Django);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn python_with_uvicorn_is_uvicorn_not_fastapi() {
        let id = detect_service(&ev("python.exe", Some("python -m uvicorn app:app --reload")));
        assert_eq!(id.kind, ServiceKind::Uvicorn);
        assert_ne!(id.kind, ServiceKind::FastApi);
    }

    #[test]
    fn uvicorn_without_fastapi_evidence_never_becomes_fastapi() {
        let id = detect_service(&ev("python.exe", Some("python -m uvicorn main:app --port 8000")));
        assert_eq!(id.kind, ServiceKind::Uvicorn);
        assert_eq!(id.confidence, Confidence::High);
    }

    #[test]
    fn python_with_fastapi_cli_is_fastapi_medium() {
        let id = detect_service(&ev("python.exe", Some("python -m fastapi run main.py")));
        assert_eq!(id.kind, ServiceKind::FastApi);
        assert_eq!(id.confidence, Confidence::Medium);
    }

    #[test]
    fn python_alone_is_honest_python() {
        let id = detect_service(&ev("python.exe", Some(r"python app.py")));
        assert_eq!(id.kind, ServiceKind::Python);
        assert_eq!(id.confidence, Confidence::Exact);
        assert_eq!(id.category, ServiceCategory::Backend);
    }

    #[test]
    fn gunicorn_is_recognized() {
        let id = detect_service(&ev("python.exe", Some("python -m gunicorn app:app")));
        assert_eq!(id.kind, ServiceKind::Gunicorn);
    }

    // --- AI family --------------------------------------------------------------

    #[test]
    fn comfyui_path_evidence_is_comfyui() {
        let mut e = ev("python.exe", Some(r"python D:\ai\ComfyUI\main.py --port 8188"));
        e.executable_path = Some(r"d:\ai\comfyui\main.py".to_string());
        let id = detect_service(&e);
        assert_eq!(id.kind, ServiceKind::ComfyUi);
        assert_eq!(id.category, ServiceCategory::Ai);
    }

    #[test]
    fn gradio_cli_is_medium_gradio() {
        let id = detect_service(&ev("python.exe", Some("python -m gradio app.py")));
        assert_eq!(id.kind, ServiceKind::Gradio);
        assert_eq!(id.confidence, Confidence::Medium);
    }

    #[test]
    fn open_webui_command_is_open_webui() {
        let id = detect_service(&ev("python.exe", Some("python -m open_webui serve")));
        assert_eq!(id.kind, ServiceKind::OpenWebUi);
    }

    // --- fallbacks ---------------------------------------------------------------

    #[test]
    fn unknown_executable_keeps_its_name_as_unknown() {
        let id = detect_service(&ev("weirdservice.exe", None));
        assert_eq!(id.kind, ServiceKind::WindowsProcess);
        assert_eq!(id.displayName, "weirdservice.exe");
        assert_eq!(id.category, ServiceCategory::Unknown);
        assert_eq!(id.confidence, Confidence::Low);
    }

    #[test]
    fn inaccessible_process_is_honest_unknown() {
        let evidence = ProcessEvidence {
            executable: None,
            executable_display: None,
            executable_path: None,
            command_line: None,
            ports: vec![135],
        };
        let id = detect_service(&evidence);
        assert_eq!(id.kind, ServiceKind::Unknown);
        assert_eq!(id.displayName, "Unavailable");
        assert_eq!(id.confidence, Confidence::Low);
    }

    #[test]
    fn fallback_display_keeps_original_case() {
        let mut e = ev("placeholder", None);
        e.executable = Some("onedrive.sync.service.exe".to_string());
        e.executable_display = Some("OneDrive.Sync.Service.exe".to_string());
        let id = detect_service(&e);
        assert_eq!(id.displayName, "OneDrive.Sync.Service.exe");
        assert_eq!(id.kind, ServiceKind::WindowsProcess);
    }

    #[test]
    fn java_gets_generic_identity() {
        let id = detect_service(&ev("java.exe", Some("java -jar app.jar")));
        assert_eq!(id.displayName, "Java Process");
        assert_eq!(id.kind, ServiceKind::UnknownRuntime);
    }

    // --- token matching internals --------------------------------------------------

    #[test]
    fn token_matching_respects_boundaries() {
        assert!(contains_token("node next dev", "next"));
        assert!(contains_token(r#"node .bin/next dev"#, "next"));
        assert!(!contains_token("node app.js --flag=nextstuff", "next"));
        assert!(!contains_token("node net.js", "next"));
        assert!(contains_token("python -m flask run", "-m"));
    }

    #[test]
    fn arg_splitting_handles_quotes() {
        let args = split_args(r#""C:\Program Files\nodejs\node.exe" "D:\my proj\vite""#);
        assert_eq!(args[0], r"C:\Program Files\nodejs\node.exe");
        assert_eq!(args[1], r"D:\my proj\vite");
    }

    // --- evidence ordering ---------------------------------------------------------

    #[test]
    fn evidence_carries_matched_values() {
        let id = detect_service(&ev("python.exe", Some("python -m flask run --port 5000")));
        let sources: Vec<&str> = id.evidence.iter().map(|e| e.source.as_str()).collect();
        assert!(sources.contains(&"command_line"));
        assert!(id.evidence.iter().any(|e| e.value == "flask"));
    }

    #[test]
    fn confidence_ordering_is_sensible() {
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
        assert!(Confidence::High < Confidence::Exact);
    }
}
