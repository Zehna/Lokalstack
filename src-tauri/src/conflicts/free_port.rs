//! Pure Free Port Finder — advisory-only alternatives near a requested
//! port, computed from the current local listener table.
//!
//! Rules (spec §8–10):
//! - Read-only over local listener data; **no bind test is performed**
//!   (binding a socket would mutate system state; availability is judged
//!   from the listener table alone, conservatively).
//! - No remote scanning of any kind.
//! - Bounded: at most [`MAX_CANDIDATES`] consecutive ports are examined and
//!   only [`suggested_count`] results are returned.
//! - Deterministic: ports are examined in ascending order from
//!   `preferred + 1`.
//! - A port occupied on *any* family/address is `used` — wildcard semantics
//!   mean a same-family bind would collide even when the recorded bind
//!   looks specific.

use serde::Serialize;

/// Upper bound of consecutive ports examined per request (spec: ≤ 100).
pub(crate) const MAX_CANDIDATES: u16 = 100;

/// How many suggestions the UI shows by default.
pub(crate) const SUGGESTION_COUNT: usize = 5;

/// One candidate port in a free-port suggestion list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PortCandidate {
    pub port: u16,
    /// `available` | `used` | `potential_conflict`
    pub status: CandidateStatus,
    /// Short owner summary when used (advisory display only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usedBy: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CandidateStatus {
    Available,
    Used,
    /// Reserved for a future bind-test refinement (spec §8: "optionally a
    /// local bind test"); the UI contract already carries the state.
    #[allow(dead_code)]
    PotentialConflict,
}

/// Occupied-port index the caller builds from one listener snapshot:
/// port → human-facing owner summary (may repeat per port; joined later).
pub(crate) type OccupiedPorts<'a> = &'a std::collections::BTreeMap<u16, Vec<String>>;

/// Find free ports ascending from `preferred + 1`.
///
/// Examines at most `MAX_CANDIDATES` consecutive ports and returns at most
/// `count` *available* ones (the caller clamps `count` to a small number).
/// Ports occupied on any address are skipped and reported as `used`
/// context — including the preferred port itself, which leads the list when
/// occupied (the UI's "Used" section). A port is never suggested while it
/// appears in the listener table on any family. Used context entries do not
/// consume the requested suggestion count.
pub(crate) fn find_free_ports(
    preferred: u16,
    count: usize,
    occupied: OccupiedPorts<'_>,
) -> Vec<PortCandidate> {
    let want = count.min(SUGGESTION_COUNT);
    let mut candidates = Vec::new();

    if let Some(owners) = occupied.get(&preferred) {
        candidates.push(PortCandidate {
            port: preferred,
            status: CandidateStatus::Used,
            usedBy: Some(owners.join(", ")),
        });
    }

    let mut available = 0usize;
    let mut examined = 0u16;
    let mut port = preferred.saturating_add(1);
    while examined < MAX_CANDIDATES && available < want {
        match occupied.get(&port) {
            Some(owners) => candidates.push(PortCandidate {
                port,
                status: CandidateStatus::Used,
                usedBy: Some(owners.join(", ")),
            }),
            None => {
                candidates.push(PortCandidate {
                    port,
                    status: CandidateStatus::Available,
                    usedBy: None,
                });
                available += 1;
            }
        }
        examined += 1;
        match port.checked_add(1) {
            Some(next) => port = next,
            None => break, // port space exhausted
        }
    }
    candidates
}

/// Keep only the `available` suggestions (the UI's primary list); the used
/// context entries stay available in the full candidate list. The command
/// surface returns the full list; this filter is used by the tests and
/// available for the UI's primary list view.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn available_only(candidates: &[PortCandidate]) -> Vec<PortCandidate> {
    candidates
        .iter()
        .filter(|c| c.status == CandidateStatus::Available)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn occupied(entries: &[(u16, &str)]) -> std::collections::BTreeMap<u16, Vec<String>> {
        entries
            .iter()
            .map(|(port, owner)| (*port, vec![owner.to_string()]))
            .collect()
    }

    #[test]
    fn next_free_port_is_suggested_first() {
        let occupied = occupied(&[(3000, "Vite / LocalStack")]);
        let candidates = find_free_ports(3000, 3, &occupied);
        let available = available_only(&candidates);
        assert_eq!(available.iter().map(|c| c.port).collect::<Vec<_>>(), vec![3001, 3002, 3003]);
    }

    #[test]
    fn consecutive_occupied_ports_are_skipped_and_reported() {
        let occupied = occupied(&[(3000, "a"), (3001, "b"), (3002, "c")]);
        let candidates = find_free_ports(3000, 5, &occupied);
        assert_eq!(candidates[0].status, CandidateStatus::Used);
        assert_eq!(candidates[0].usedBy.as_deref(), Some("a"));
        assert_eq!(candidates[3].port, 3003);
        assert_eq!(candidates[3].status, CandidateStatus::Available);
    }

    #[test]
    fn result_count_is_bounded_by_the_request() {
        let occupied = std::collections::BTreeMap::new();
        let candidates = find_free_ports(8000, 5, &occupied);
        assert_eq!(candidates.len(), 5);
    }

    #[test]
    fn never_returns_a_currently_listening_port() {
        let occupied = occupied(&[(3001, "x"), (3002, "y")]);
        let candidates = find_free_ports(3000, 10, &occupied);
        assert!(candidates.iter().all(|c| !(c.port == 3001 || c.port == 3002) || c.status == CandidateStatus::Used));
        assert!(available_only(&candidates).iter().all(|c| c.status == CandidateStatus::Available));
    }

    #[test]
    fn examination_window_is_bounded() {
        // Every port in the window occupied → at most MAX_CANDIDATES entries.
        let occupied: std::collections::BTreeMap<u16, Vec<String>> = (0..MAX_CANDIDATES)
            .map(|offset| (3001 + offset, vec!["busy".to_string()]))
            .collect();
        let candidates = find_free_ports(3000, 5, &occupied);
        assert_eq!(candidates.len() as u16, MAX_CANDIDATES);
        assert!(candidates.iter().all(|c| c.status == CandidateStatus::Used));
    }

    #[test]
    fn port_space_edge_is_handled() {
        let occupied = std::collections::BTreeMap::new();
        let candidates = find_free_ports(u16::MAX - 1, 5, &occupied);
        assert_eq!(candidates.len(), 1, "only u16::MAX remains");
        assert_eq!(candidates[0].port, u16::MAX);
    }

    #[test]
    fn count_is_clamped_to_the_ui_maximum() {
        let occupied = std::collections::BTreeMap::new();
        let candidates = find_free_ports(9000, 50, &occupied);
        assert_eq!(candidates.len(), SUGGESTION_COUNT, "spec: return only a few best suggestions");
    }
}
