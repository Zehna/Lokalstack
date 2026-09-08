//! Pure dependency-graph logic: cycle detection and topological start
//! ordering. Deterministic and fully unit-tested — no I/O.
//!
//! Graph direction: an edge `A → B` means "A depends on B", so B must start
//! before A. External dependencies are sinks (they are never started by
//! LocalStack) and therefore cannot participate in a cycle.

use serde::Serialize;

/// One directed edge: `source` depends on `target` (both are workspace
/// service ids). External targets never appear here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ServiceEdge {
    pub source: String,
    pub target: String,
}

/// A detected dependency cycle, returned as the offending path.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct DependencyCycle {
    /// Service ids forming the cycle, in order, first repeated at the end.
    pub path: Vec<String>,
}

/// Deterministic topological start order for workspace services.
///
/// Kahn's algorithm with a stable priority: ready services are emitted by
/// (in-degree zero, role rank, service id) so runs are reproducible.
/// Returns `Err(cycle)` when the graph is not a DAG — the caller refuses
/// ordered start instead of recursing forever.
pub(crate) fn topological_order(
    services: &[(String, u8)], // (service id, role rank) in workspace order
    edges: &[ServiceEdge],
) -> Result<Vec<String>, DependencyCycle> {
    // Keep only edges between known services (external targets are sinks).
    let known: std::collections::HashSet<&String> =
        services.iter().map(|(id, _)| id).collect();
    let internal: Vec<&ServiceEdge> = edges
        .iter()
        .filter(|e| known.contains(&e.source) && known.contains(&e.target))
        .collect();

    let mut indegree: std::collections::HashMap<&String, usize> =
        services.iter().map(|(id, _)| (id, 0)).collect();
    let mut dependents: std::collections::HashMap<&String, Vec<&String>> =
        services.iter().map(|(id, _)| (id, Vec::new())).collect();
    for edge in &internal {
        indegree.entry(&edge.source).and_modify(|d| *d += 1);
        dependents
            .entry(&edge.target)
            .or_default()
            .push(&edge.source);
    }

    let rank = |id: &String| -> u8 {
        services.iter().find(|(s, _)| s == id).map(|(_, r)| *r).unwrap_or(u8::MAX)
    };
    let mut ready: Vec<&&String> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(id, _)| id)
        .collect();
    ready.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));

    let mut order: Vec<String> = Vec::with_capacity(services.len());
    let mut ready: std::collections::BTreeSet<(u8, String)> =
        ready.into_iter().map(|id| (rank(id), (*id).clone())).collect();
    while let Some((_, id)) = ready.pop_first() {
        order.push(id.clone());
        for &dependent in dependents
            .get(&id)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
        {
            let entry = indegree.get_mut(dependent).expect("known service");
            *entry -= 1;
            if *entry == 0 {
                ready.insert((rank(dependent), dependent.clone()));
            }
        }
    }

    if order.len() != services.len() {
        return Err(find_cycle(services, &internal).unwrap_or(DependencyCycle {
            path: services.iter().map(|(id, _)| id.clone()).collect(),
        }));
    }
    Ok(order)
}

/// Find one concrete cycle path among the remaining (unorderable) services.
fn find_cycle(
    services: &[(String, u8)],
    edges: &[&ServiceEdge],
) -> Option<DependencyCycle> {
    let mut adjacency: std::collections::HashMap<&String, Vec<&String>> =
        services.iter().map(|(id, _)| (id, Vec::new())).collect();
    for edge in edges {
        adjacency.entry(&edge.source).or_default().push(&edge.target);
    }

    // Iterative DFS with colors: 0 = unvisited, 1 = in stack, 2 = done.
    let mut color: std::collections::HashMap<&String, u8> =
        services.iter().map(|(id, _)| (id, 0)).collect();
    let mut stack: Vec<(&String, usize)> = Vec::new();

    for (start, _) in services {
        if color[start] != 0 {
            continue;
        }
        stack.clear();
        stack.push((start, 0));
        color.insert(start, 1);
        while let Some((node, index)) = stack.pop() {
            let neighbors = adjacency.get(node).map(|v| v.as_slice()).unwrap_or(&[]);
            if index < neighbors.len() {
                let next = neighbors[index];
                stack.push((node, index + 1));
                match color[next] {
                    0 => {
                        color.insert(next, 1);
                        stack.push((next, 0));
                    }
                    1 => {
                        // Found a back-edge: extract the cycle from the stack.
                        let path_start = stack
                            .iter()
                            .position(|(n, _)| *n == next)
                            .unwrap_or(0);
                        let mut path: Vec<String> =
                            stack[path_start..].iter().map(|(n, _)| (*n).clone()).collect();
                        path.push(next.clone());
                        return Some(DependencyCycle { path });
                    }
                    _ => {}
                }
            } else {
                color.insert(node, 2);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn services(entries: &[(&str, u8)]) -> Vec<(String, u8)> {
        entries
            .iter()
            .map(|(id, rank)| (id.to_string(), *rank))
            .collect()
    }

    fn edge(source: &str, target: &str) -> ServiceEdge {
        ServiceEdge { source: source.to_string(), target: target.to_string() }
    }

    #[test]
    fn independent_services_keep_role_order() {
        let list = services(&[("frontend", 3), ("backend", 1), ("worker", 2)]);
        let order = topological_order(&list, &[]).expect("acyclic");
        assert_eq!(order, vec!["backend", "worker", "frontend"]);
    }

    #[test]
    fn dependencies_start_before_dependents() {
        let list = services(&[("frontend", 3), ("backend", 1), ("worker", 2)]);
        let edges = vec![edge("frontend", "backend"), edge("backend", "worker")];
        let order = topological_order(&list, &edges).expect("acyclic");
        assert_eq!(order, vec!["worker", "backend", "frontend"]);
    }

    #[test]
    fn independent_service_is_not_delayed_by_others_chains() {
        // `api` depends on `db`; `web` is independent with a worse role rank.
        let list = services(&[("web", 3), ("api", 1), ("db", 0)]);
        let edges = vec![edge("api", "db")];
        let order = topological_order(&list, &edges).expect("acyclic");
        // db first (dependency), then api/web by role rank (api 1 < web 3).
        assert_eq!(order, vec!["db", "api", "web"]);
    }

    #[test]
    fn direct_cycle_is_detected() {
        let list = services(&[("a", 0), ("b", 1)]);
        let edges = vec![edge("a", "b"), edge("b", "a")];
        let cycle = topological_order(&list, &edges).expect_err("cycle");
        assert_eq!(cycle.path.len(), 3, "a → b → a (first repeated at the end)");
        assert_eq!(cycle.path.first(), cycle.path.last());
    }

    #[test]
    fn self_dependency_is_a_cycle() {
        let list = services(&[("a", 0)]);
        let edges = vec![edge("a", "a")];
        assert!(topological_order(&list, &edges).is_err());
    }

    #[test]
    fn long_cycle_is_detected_without_recursing_forever() {
        let list = services(&[("a", 0), ("b", 1), ("c", 2), ("d", 3)]);
        let edges = vec![
            edge("a", "b"),
            edge("b", "c"),
            edge("c", "d"),
            edge("d", "a"),
        ];
        let cycle = topological_order(&list, &edges).expect_err("cycle");
        assert_eq!(cycle.path.len(), 5);
    }

    #[test]
    fn external_targets_are_sinks_and_ignored_for_cycles() {
        // An edge pointing at a non-service target must not break the order.
        let list = services(&[("api", 1)]);
        let edges = vec![ServiceEdge {
            source: "api".to_string(),
            target: "external:postgres".to_string(),
        }];
        let order = topological_order(&list, &edges).expect("external sinks are ignored");
        assert_eq!(order, vec!["api"]);
    }

    #[test]
    fn empty_workspace_orders_empty() {
        assert_eq!(topological_order(&[], &[]).expect("empty"), Vec::<String>::new());
    }
}
