//! Container → LocalStack project association from explicit evidence
//! (spec §17–20, §56).
//!
//! The chain is: container → Compose identity → LocalStack project, graded
//! by [`Confidence`]. Only real evidence is used:
//!
//! 1. **Exact** — Compose `project.working_dir` label equals the project
//!    root (path-normalized, case-insensitive — Windows paths).
//! 2. **High** — a container *bind mount* source equals the project root.
//! 3. **Medium** — the project root lives *inside* a bind-mounted source.
//! 4. **Low** — Compose project name equals the project directory basename,
//!    and the match is *unambiguous* (exactly one candidate project).
//!    Sibling projects with the same basename are **never merged** (spec
//!    §56) — ambiguity yields no association, not a guess.
//!
//! Name similarity of the *container* itself is never evidence (spec §55).
//! Mount file contents are never read (spec §20); only the path strings
//! Docker already returned are compared.

use crate::intelligence::rules::Confidence;
use serde::Serialize;

use super::domain::DockerContainer;

/// One container's association outcome. `projectId` is `None` unless the
/// evidence reached at least Low confidence with a unique candidate
/// (spec §18: do not force low-confidence mappings).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct ContainerProjectLink {
    pub containerId: String,
    /// References [`crate::project::ProjectIdentity::id`] in the same
    /// snapshot when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectId: Option<String>,
    /// How strongly the association is evidenced (`unknown` when absent).
    pub confidence: String,
    /// Human-readable explanation of the winning evidence.
    pub evidence: String,
}

/// Associate every container with the best-evidence project, if any.
///
/// `projects` is the resolved [`crate::project::ProjectIdentity`] list from
/// the current discovery cycle. Deterministic: ties are resolved by
/// evidence strength first, then by project id for stability.
pub(crate) fn associate(
    containers: &[DockerContainer],
    projects: &[crate::project::ProjectIdentity],
) -> Vec<ContainerProjectLink> {
    containers
        .iter()
        .map(|container| link_for(container, projects))
        .collect()
}

fn link_for(container: &DockerContainer, projects: &[crate::project::ProjectIdentity]) -> ContainerProjectLink {
    let mut best: Option<(Confidence, String, String)> = None;

    // 1. Compose working directory == project root (spec §17 strong
    //    evidence). Strongest because compose knows where it ran.
    if let Some(compose) = &container.compose {
        if let Some(working_dir) = &compose.workingDir {
            if let Some(project) = projects
                .iter()
                .find(|p| same_path(&p.rootPath, working_dir))
            {
                consider(
                    &mut best,
                    Confidence::Exact,
                    project.id.clone(),
                    format!(
                        "Compose working directory matches project root '{}'",
                        project.name
                    ),
                );
            }
        }
    }

    // 2/3. Bind-mount evidence (spec §19). Source == root → High; root
    //      nested under the mount → Medium (the container may serve the
    //      whole parent directory, of which this project is one part).
    for (source, _destination) in &container.mounts {
        if let Some(project) = projects
            .iter()
            .find(|p| same_path(&p.rootPath, source))
        {
            consider(
                &mut best,
                Confidence::High,
                project.id.clone(),
                format!("Container bind mount is the project root '{}'", project.name),
            );
        } else if let Some(project) = projects
            .iter()
            .find(|p| path_contains(source, &p.rootPath))
        {
            consider(
                &mut best,
                Confidence::Medium,
                project.id.clone(),
                format!(
                    "Project root lives inside container bind mount '{}'",
                    project.name
                ),
            );
        }
    }

    // 4. Compose project name == project directory basename — weak, and
    //    only when unambiguous (spec §56: siblings never merged).
    if let Some(compose) = &container.compose {
        if let Some(project_name) = &compose.projectName {
            let matches: Vec<&crate::project::ProjectIdentity> = projects
                .iter()
                .filter(|p| basename(&p.rootPath).eq_ignore_ascii_case(project_name))
                .collect();
            if matches.len() == 1 {
                let project = matches[0];
                consider(
                    &mut best,
                    Confidence::Low,
                    project.id.clone(),
                    format!(
                        "Compose project name matches project directory (weak evidence)"
                    ),
                );
            }
            // len > 1 → ambiguous: deliberately no association.
        }
    }

    match best {
        Some((confidence, project_id, evidence)) => ContainerProjectLink {
            containerId: container.id.clone(),
            projectId: Some(project_id),
            confidence: confidence_label(confidence),
            evidence,
        },
        None => ContainerProjectLink {
            containerId: container.id.clone(),
            projectId: None,
            confidence: "unknown".to_string(),
            evidence: "No project evidence found for this container".to_string(),
        },
    }
}

/// Keep the strongest evidence; ties keep the first (later calls with an
/// equal-or-weaker confidence do not overwrite).
fn consider(
    best: &mut Option<(Confidence, String, String)>,
    confidence: Confidence,
    project_id: String,
    evidence: String,
) {
    if best.as_ref().is_none_or(|(c, _, _)| confidence > *c) {
        *best = Some((confidence, project_id, evidence));
    }
}

fn confidence_label(confidence: Confidence) -> String {
    match confidence {
        Confidence::Exact => "exact".to_string(),
        Confidence::High => "high".to_string(),
        Confidence::Medium => "medium".to_string(),
        Confidence::Low => "low".to_string(),
    }
}

/// Windows-path equality: unify separators, drop trailing separators,
/// compare case-insensitively (Docker returns `D:/x`, the shell uses `D:\X`).
fn same_path(a: &str, b: &str) -> bool {
    normalize_path(a) == normalize_path(b)
}

/// Is `root` inside (or equal to a *child* of) directory `dir`?
fn path_contains(dir: &str, root: &str) -> bool {
    let dir = normalize_path(dir);
    let root = normalize_path(root);
    root.len() > dir.len()
        && root.starts_with(&dir)
        && root.as_bytes().get(dir.len()) == Some(&b'\\')
}

fn normalize_path(path: &str) -> String {
    let mut normalized = path.replace('/', "\\");
    while normalized.ends_with('\\') {
        normalized.pop();
    }
    normalized.to_ascii_lowercase()
}

/// Last path component of a Windows path (its directory name).
fn basename(path: &str) -> String {
    path.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_string()
}

// ---------------------------------------------------------------------------
// Tests (spec §56)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::domain::{
        ContainerHealth, ContainerState, DockerComposeIdentity, DockerContainer,
    };
    use crate::project::ProjectIdentity;

    fn container(id: &str, compose: Option<DockerComposeIdentity>, mounts: Vec<(String, String)>) -> DockerContainer {
        DockerContainer {
            id: id.to_string(),
            shortId: id.chars().take(12).collect(),
            name: format!("c-{id}"),
            image: "img:1".to_string(),
            imageId: None,
            imageDigest: None,
            state: ContainerState::Running,
            status: "Up".to_string(),
            health: ContainerHealth::None,
            createdAt: None,
            ports: Vec::new(),
            compose,
            networks: Vec::new(),
            mounts,
            stats: None,
        }
    }

    fn compose(project: &str, service: &str, working_dir: Option<&str>) -> DockerComposeIdentity {
        DockerComposeIdentity {
            projectName: Some(project.to_string()),
            serviceName: Some(service.to_string()),
            containerNumber: Some("1".to_string()),
            workingDir: working_dir.map(str::to_string),
            configFiles: None,
        }
    }

    fn project(id: &str, name: &str, root: &str) -> ProjectIdentity {
        ProjectIdentity {
            id: id.to_string(),
            name: name.to_string(),
            rootPath: root.to_string(),
            kind: crate::project::ProjectKind::NodeJs,
            git: crate::project::GitInfo {
                isRepository: false,
                rootPath: None,
                branch: None,
            },
            packageManager: None,
            startCommand: None,
            confidence: Confidence::High,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn compose_working_dir_is_exact_evidence() {
        // Docker returns forward slashes; the project root uses backslashes.
        let containers = [container(
            "a1",
            Some(compose("historyai", "frontend", Some("D:/Projects/HistoryAI"))),
            Vec::new(),
        )];
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\Projects\\HistoryAI")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId.as_deref(), Some("D:\\Projects\\HistoryAI"));
        assert_eq!(links[0].confidence, "exact");
    }

    #[test]
    fn case_insensitive_path_match() {
        let containers = [container(
            "a2",
            Some(compose("h", "web", Some("d:\\projects\\historyai"))),
            Vec::new(),
        )];
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\PROJECTS\\HistoryAI\\")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].confidence, "exact");
    }

    #[test]
    fn bind_mount_exact_root_is_high() {
        let containers = [container(
            "a3",
            None,
            vec![("D:\\Projects\\HistoryAI".to_string(), "/app".to_string())],
        )];
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\Projects\\HistoryAI")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId.as_deref(), Some("D:\\Projects\\HistoryAI"));
        assert_eq!(links[0].confidence, "high");
    }

    #[test]
    fn project_inside_mount_is_medium() {
        let containers = [container(
            "a4",
            None,
            vec![("D:\\Projects".to_string(), "/src".to_string())],
        )];
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\Projects\\HistoryAI")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].confidence, "medium");
    }

    #[test]
    fn compose_name_only_unique_is_low() {
        let containers = [container(
            "a5",
            Some(compose("historyai", "db", None)),
            Vec::new(),
        )];
        let projects = [project("D:\\Other\\historyai", "historyai", "D:\\Other\\historyai")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId.as_deref(), Some("D:\\Other\\historyai"));
        assert_eq!(links[0].confidence, "low");
    }

    #[test]
    fn sibling_same_basename_never_merged() {
        let containers = [container(
            "a6",
            Some(compose("historyai", "db", None)),
            Vec::new(),
        )];
        let projects = [
            project("D:\\A\\historyai", "historyai", "D:\\A\\historyai"),
            project("D:\\B\\historyai", "historyai", "D:\\B\\historyai"),
        ];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId, None, "ambiguous name-only match must not associate (spec §56)");
        assert_eq!(links[0].confidence, "unknown");
    }

    #[test]
    fn no_evidence_is_unknown() {
        let containers = [container("a7", Some(compose("unrelated", "x", None)), Vec::new())];
        let projects = [project("D:\\Projects\\Other", "Other", "D:\\Projects\\Other")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId, None);
        assert_eq!(links[0].confidence, "unknown");
    }

    #[test]
    fn similar_container_name_alone_is_nothing() {
        // Name historyai-frontend-1 without any labels/mounts — no evidence.
        let mut c = container("a8", None, Vec::new());
        c.name = "historyai-frontend-1".to_string();
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\Projects\\HistoryAI")];
        let links = associate(&[c], &projects);
        assert_eq!(links[0].projectId, None, "name similarity is never evidence (spec §55)");
    }

    #[test]
    fn working_dir_containers_path_is_not_windows_root() {
        // A Linux-container-mode working_dir (/app) must not match anything.
        let containers = [container("a9", Some(compose("p", "s", Some("/app"))), Vec::new())];
        let projects = [project("D:\\Projects\\app", "app", "D:\\Projects\\app")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].projectId, None);
    }

    #[test]
    fn strongest_evidence_wins() {
        // Name-only (low) + working dir (exact) → exact wins.
        let containers = [container(
            "b1",
            Some(compose("HistoryAI", "frontend", Some("D:/Projects/HistoryAI"))),
            Vec::new(),
        )];
        let projects = [project("D:\\Projects\\HistoryAI", "HistoryAI", "D:\\Projects\\HistoryAI")];
        let links = associate(&containers, &projects);
        assert_eq!(links[0].confidence, "exact");
    }
}
