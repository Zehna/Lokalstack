//! Pure port-conflict rules — deterministic, fully unit-tested, no Windows
//! calls and no I/O. The caller (the `conflicts` facade and the workspace
//! preflight) supplies already-observed facts: the listener table and the
//! resolved owner metadata.
//!
//! # Product principle
//!
//! "Port 3000 already in use" is not an explanation. Every evaluation
//! answers *who* owns the port (PID, process, service identity, project,
//! managed/external lifecycle) and *how certain* the conflict is — with
//! explicit bind-address semantics instead of a bare `port == port` check.
//!
//! # Bind semantics (documented limits)
//!
//! - `0.0.0.0:P` covers every local IPv4 address, so it overlaps any IPv4
//!   bind on `P`.
//! - `::P` (IPv6 wildcard) covers all IPv6 addresses. On Windows a
//!   dual-stack wildcard bind usually also accepts IPv4-mapped connections,
//!   but the TCP table cannot reveal whether `IPV6_V6ONLY` is set — so a
//!   wildcard-IPv6 listener versus an IPv4 request is reported as
//!   [`BindRelation::Uncertain`], never as safe.
//! - Two specific addresses of different families (`127.0.0.1` vs `::1`)
//!   are genuinely disjoint — servers commonly bind both on one port.
//! - When the future bind of the *requester* is unknown (it always is
//!   before launch), any same-family listener is treated as a real
//!   (`Blocking`) conflict: a wildcard request would overlap it, and a
//!   loopback request overlaps loopback/wildcard listeners.

use std::net::Ipv6Addr;

use serde::Serialize;

use crate::discovery::PortListener;

// ---------------------------------------------------------------------------
// Bind scopes
// ---------------------------------------------------------------------------

/// What an address actually binds, reduced to the semantics that matter for
/// conflict analysis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BindScope {
    /// `0.0.0.0` — every IPv4 address on the host.
    WildcardV4,
    /// `::` — every IPv6 address (possibly dual-stack; see module docs).
    WildcardV6,
    /// `127.0.0.1` — IPv4 loopback.
    LoopbackV4,
    /// `::1` — IPv6 loopback only.
    LoopbackV6,
    /// One specific IPv4 address (not the wildcard, not loopback).
    SpecificV4,
    /// One specific IPv6 address (not the wildcard, not loopback).
    SpecificV6,
}

/// Reduce a discovered listener to its bind scope.
pub(crate) fn bind_scope_of(listener: &PortListener) -> BindScope {
    let address = listener.localAddress.trim();
    match listener.ipVersion {
        crate::discovery::ports::IpVersion::V4 => match address {
            "0.0.0.0" => BindScope::WildcardV4,
            "127.0.0.1" => BindScope::LoopbackV4,
            _ => BindScope::SpecificV4,
        },
        crate::discovery::ports::IpVersion::V6 => {
            // Normalize the many textual spellings of the IPv6 wildcard.
            let normalized = address.trim_start_matches('[').trim_end_matches(']');
            let is_wildcard = matches!(normalized, "::" | "0:0:0:0:0:0:0:0");
            if is_wildcard {
                return BindScope::WildcardV6;
            }
            if normalized.parse::<Ipv6Addr>() == Ok(Ipv6Addr::LOCALHOST) {
                BindScope::LoopbackV6
            } else {
                BindScope::SpecificV6
            }
        }
    }
}

/// How two bind scopes interact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BindRelation {
    /// Identical scope — binding both is impossible.
    Same,
    /// One covers the other (e.g. wildcard over loopback) — impossible.
    Overlap,
    /// Different families or distinct specific addresses — can coexist.
    Disjoint,
    /// Cannot be decided from the TCP table (dual-stack wildcard) —
    /// conservatively treated as a potential conflict.
    Uncertain,
}

/// Relate two bind scopes (pure; order matters only for which side is the
/// wildcard, not for the outcome).
pub(crate) fn bind_relation(requester: &BindScope, listener: &BindScope) -> BindRelation {
    use BindRelation::{Disjoint, Overlap, Same, Uncertain};
    use BindScope::*;
    if requester == listener {
        return Same;
    }
    match (requester, listener) {
        // Identical wildcards (the early equality check handles all other
        // identical pairs; the compiler still wants these covered).
        (WildcardV4, WildcardV4) | (WildcardV6, WildcardV6) => Same,
        // A wildcard covers everything of its own family.
        (WildcardV4, LoopbackV4 | SpecificV4) | (LoopbackV4 | SpecificV4, WildcardV4) => Overlap,
        (WildcardV6, LoopbackV6 | SpecificV6) | (LoopbackV6 | SpecificV6, WildcardV6) => Overlap,
        // Dual-stack uncertainty: IPv6 wildcard may also serve IPv4.
        (WildcardV6, WildcardV4) | (WildcardV4, WildcardV6) => Uncertain,
        (WildcardV6, LoopbackV4 | SpecificV4) | (LoopbackV4 | SpecificV4, WildcardV6) => Uncertain,
        // Distinct families with no wildcard: genuinely separate sockets.
        (WildcardV4 | LoopbackV4 | SpecificV4, LoopbackV6 | SpecificV6)
        | (LoopbackV6 | SpecificV6, WildcardV4 | LoopbackV4 | SpecificV4) => Disjoint,
        // Same family, two distinct specific addresses: separate sockets.
        (LoopbackV4 | SpecificV4, LoopbackV4 | SpecificV4) => Disjoint,
        (LoopbackV6 | SpecificV6, LoopbackV6 | SpecificV6) => Disjoint,
    }
}

// ---------------------------------------------------------------------------
// Owner + conflict model
// ---------------------------------------------------------------------------

/// Normalized owner descriptor for one listener (spec §32). Fields stay
/// `None` when discovery could not resolve them — never fabricated.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[allow(non_snake_case)]
pub(crate) struct PortOwner {
    pub pid: u32,
    /// Image basename, when Windows revealed it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processName: Option<String>,
    /// Evidence-based service identity display name, when classified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub serviceDisplayName: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectId: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projectName: Option<String>,
    /// `managed` when the PID is a LocalStack-launched root process.
    pub lifecycle: OwnerLifecycle,
    /// Workspace/service owning the managed process, when managed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managedWorkspace: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managedService: Option<String>,
}

/// Lifecycle of the owning process relative to LocalStack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum OwnerLifecycle {
    Managed,
    External,
}

/// What kind of conflict (or non-conflict) a requested port is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictKind {
    /// The port is free.
    NoConflict,
    /// The port belongs to the exact same managed service instance — not a
    /// conflict; the service is simply already running.
    AlreadyRunning,
    /// An externally launched process of the same project holds the port.
    SameProjectExternal,
    /// Another LocalStack-managed service of the same project holds it.
    SameProjectManaged,
    /// A process belonging to a different known project holds the port.
    OtherProject,
    /// A listener exists but its owner could not be resolved.
    UnknownOwner,
    /// The only overlap is via possible dual-stack wildcard semantics —
    /// always reported with `Potential` severity, never as safe.
    DualStackEquivalent,
    /// Windows-reserved or unverifiable owner (PID 0/4, system socket).
    ReservedOrUnverifiable,
}

/// How badly the conflict blocks a launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConflictSeverity {
    /// Advisory — e.g. the same service is already running.
    Info,
    /// Bind ambiguity; the launch *may* fail.
    Potential,
    /// A same-family listener holds the port; the launch will fail.
    Blocking,
}

/// Per-listener evaluation outcome.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ListenerVerdict {
    pub kind: ConflictKind,
    pub severity: ConflictSeverity,
}

/// Evaluate one listener against a port request whose future bind address
/// is unknown (the requester has not launched yet). `requester_is_v6` carries
/// the family the requester would most likely bind (its expected evidence,
/// when any); `None` means "either family".
pub(crate) fn evaluate_listener(
    requester_is_v6: Option<bool>,
    listener: &PortListener,
    owner_is_same_managed_service: bool,
    owner_is_reserved: bool,
) -> ListenerVerdict {
    let scope = bind_scope_of(listener);
    let listener_is_v6 = matches!(
        scope,
        BindScope::WildcardV6 | BindScope::LoopbackV6 | BindScope::SpecificV6
    );

    if owner_is_reserved {
        return ListenerVerdict {
            kind: ConflictKind::ReservedOrUnverifiable,
            severity: ConflictSeverity::Blocking,
        };
    }
    if owner_is_same_managed_service {
        return ListenerVerdict {
            kind: ConflictKind::AlreadyRunning,
            severity: ConflictSeverity::Info,
        };
    }

    // The requester's bind is unknown. Ask: is there a plausible bind for
    // the requester that this listener blocks?
    let cross_family_only = requester_is_v6.is_some_and(|is_v6| is_v6 != listener_is_v6);
    // The requester's plausible scope comes from the *requester's* family,
    // not the listener's: loopback or wildcard both overlap any same-family
    // listener, so same-family => Blocking.
    let requester_scope = match requester_is_v6 {
        Some(false) => BindScope::LoopbackV4,
        Some(true) => BindScope::LoopbackV6,
        None => {
            // Bind family is unknown before launch. Any same-family listener
            // must be assumed blocking; but an IPv6 *wildcard* listener can
            // only block a request through unverifiable dual-stack
            // behavior — conservatively Potential, never claimed safe or
            // certain (spec §7).
            if listener_is_v6 {
                return match scope {
                    BindScope::WildcardV6 => ListenerVerdict {
                        kind: ConflictKind::DualStackEquivalent,
                        severity: ConflictSeverity::Potential,
                    },
                    _ => ListenerVerdict {
                        kind: ConflictKind::OtherProject, // refined by the caller
                        severity: ConflictSeverity::Blocking,
                    },
                };
            }
            BindScope::LoopbackV4
        }
    };
    match bind_relation(&requester_scope, &scope) {
        BindRelation::Same | BindRelation::Overlap => ListenerVerdict {
            kind: ConflictKind::OtherProject, // refined by the caller with owner metadata
            severity: ConflictSeverity::Blocking,
        },
        BindRelation::Uncertain => ListenerVerdict {
            kind: ConflictKind::DualStackEquivalent,
            severity: ConflictSeverity::Potential,
        },
        BindRelation::Disjoint if cross_family_only => ListenerVerdict {
            kind: ConflictKind::DualStackEquivalent,
            severity: ConflictSeverity::Potential,
        },
        BindRelation::Disjoint => ListenerVerdict {
            kind: ConflictKind::OtherProject,
            severity: ConflictSeverity::Potential,
        },
    }
}

/// Aggregate per-listener verdicts into the report the UI shows: the worst
/// severity wins; `NoConflict` only when there is nothing to evaluate.
pub(crate) fn aggregate(verdicts: &[ListenerVerdict]) -> Option<ListenerVerdict> {
    verdicts.iter().max_by_key(|v| match v.severity {
        ConflictSeverity::Blocking => 3,
        ConflictSeverity::Potential => 2,
        ConflictSeverity::Info => 1,
    })
    .cloned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::ports::{IpVersion, ListenerState, Protocol};

    fn listener(v6: bool, address: &str, port: u16) -> PortListener {
        PortListener {
            protocol: Protocol::Tcp,
            ipVersion: if v6 { IpVersion::V6 } else { IpVersion::V4 },
            localAddress: address.to_string(),
            port,
            pid: 42,
            state: ListenerState::Listen,
        }
    }

    // --- bind scopes --------------------------------------------------------

    #[test]
    fn bind_scopes_are_classified() {
        assert_eq!(bind_scope_of(&listener(false, "0.0.0.0", 80)), BindScope::WildcardV4);
        assert_eq!(bind_scope_of(&listener(false, "127.0.0.1", 80)), BindScope::LoopbackV4);
        assert_eq!(bind_scope_of(&listener(false, "192.168.1.10", 80)), BindScope::SpecificV4);
        assert_eq!(bind_scope_of(&listener(true, "::", 80)), BindScope::WildcardV6);
        assert_eq!(bind_scope_of(&listener(true, "[::]", 80)), BindScope::WildcardV6);
        assert_eq!(bind_scope_of(&listener(true, "::1", 80)), BindScope::LoopbackV6);
        assert_eq!(bind_scope_of(&listener(true, "fe80::1", 80)), BindScope::SpecificV6);
    }

    // --- bind relations -------------------------------------------------------

    #[test]
    fn wildcard_v4_overlaps_every_ipv4_scope() {
        assert_eq!(bind_relation(&BindScope::WildcardV4, &BindScope::LoopbackV4), BindRelation::Overlap);
        assert_eq!(bind_relation(&BindScope::WildcardV4, &BindScope::SpecificV4), BindRelation::Overlap);
        assert_eq!(bind_relation(&BindScope::LoopbackV4, &BindScope::WildcardV4), BindRelation::Overlap);
    }

    #[test]
    fn loopback_and_specific_ipv4_are_disjoint() {
        assert_eq!(bind_relation(&BindScope::LoopbackV4, &BindScope::SpecificV4), BindRelation::Disjoint);
        assert_eq!(bind_relation(&BindScope::SpecificV4, &BindScope::LoopbackV4), BindRelation::Disjoint);
    }

    #[test]
    fn identical_scopes_are_same() {
        assert_eq!(bind_relation(&BindScope::LoopbackV4, &BindScope::LoopbackV4), BindRelation::Same);
        assert_eq!(bind_relation(&BindScope::LoopbackV6, &BindScope::LoopbackV6), BindRelation::Same);
        assert_eq!(bind_relation(&BindScope::WildcardV6, &BindScope::WildcardV6), BindRelation::Same);
    }

    #[test]
    fn ipv6_wildcard_vs_ipv4_is_uncertain_never_safe() {
        // Dual-stack behavior cannot be observed from the TCP table.
        assert_eq!(bind_relation(&BindScope::WildcardV6, &BindScope::WildcardV4), BindRelation::Uncertain);
        assert_eq!(bind_relation(&BindScope::LoopbackV4, &BindScope::WildcardV6), BindRelation::Uncertain);
        assert_eq!(bind_relation(&BindScope::SpecificV4, &BindScope::WildcardV6), BindRelation::Uncertain);
    }

    #[test]
    fn ipv6_loopback_vs_ipv4_wildcard_is_disjoint() {
        // A v4 wildcard bind never covers ::1 and vice versa; these
        // coexist on one port (this is why servers bind both).
        assert_eq!(bind_relation(&BindScope::LoopbackV6, &BindScope::WildcardV4), BindRelation::Disjoint);
        assert_eq!(bind_relation(&BindScope::SpecificV6, &BindScope::WildcardV4), BindRelation::Disjoint);
        assert_eq!(bind_relation(&BindScope::WildcardV6, &BindScope::LoopbackV6), BindRelation::Overlap);
    }

    // --- evaluation -------------------------------------------------------------

    #[test]
    fn same_managed_service_is_already_running_not_a_conflict() {
        let verdict = evaluate_listener(None, &listener(false, "127.0.0.1", 1420), true, false);
        assert_eq!(verdict.kind, ConflictKind::AlreadyRunning);
        assert_eq!(verdict.severity, ConflictSeverity::Info);
    }

    #[test]
    fn reserved_owner_is_blocking_unverifiable() {
        let verdict = evaluate_listener(None, &listener(false, "0.0.0.0", 135), false, true);
        assert_eq!(verdict.kind, ConflictKind::ReservedOrUnverifiable);
        assert_eq!(verdict.severity, ConflictSeverity::Blocking);
    }

    #[test]
    fn same_family_listener_is_blocking() {
        let verdict = evaluate_listener(Some(false), &listener(false, "127.0.0.1", 3000), false, false);
        assert_eq!(verdict.severity, ConflictSeverity::Blocking);
    }

    #[test]
    fn ipv6_wildcard_vs_ipv4_request_is_dual_stack_potential() {
        let verdict = evaluate_listener(Some(false), &listener(true, "::", 3000), false, false);
        assert_eq!(verdict.kind, ConflictKind::DualStackEquivalent);
        assert_eq!(verdict.severity, ConflictSeverity::Potential);
    }

    #[test]
    fn ipv6_specific_vs_ipv4_request_is_potential_not_blocking() {
        let verdict = evaluate_listener(Some(false), &listener(true, "fe80::1", 3000), false, false);
        assert_eq!(verdict.severity, ConflictSeverity::Potential);
    }

    #[test]
    fn aggregate_picks_the_worst_verdict() {
        let verdicts = [
            ListenerVerdict { kind: ConflictKind::DualStackEquivalent, severity: ConflictSeverity::Potential },
            ListenerVerdict { kind: ConflictKind::OtherProject, severity: ConflictSeverity::Blocking },
        ];
        let worst = aggregate(&verdicts).expect("verdicts");
        assert_eq!(worst.severity, ConflictSeverity::Blocking);
        assert!(aggregate(&[]).is_none());
    }
}
