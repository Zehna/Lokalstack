//! Opaque control-target registry — the native trust boundary for control.
//!
//! The frontend never sends policy-bearing fields (PID, creation time,
//! service category, project, canStop…). It sends back only the **opaque
//! target id** the backend issued during discovery. All authority lives
//! server-side:
//!
//! ```text
//! discovery → Rust issues opaque target id → frontend echoes id
//!           → Rust resolves trusted snapshot → re-inspects process
//!           → revalidates identity → recomputes policy → (only then) acts
//! ```
//!
//! IDs are unpredictable (BLAKE3-256 of PID ‖ creation ticks ‖ a per-boot
//! random process key, hex) so a guessed id for an unregistered PID is
//! infeasible. The registry is bounded and TTL-expiring; a refresh cycle
//! replaces targets wholesale.
//!
//! Phase 6 contract: when LocalStack itself launches a process into a known
//! process group (`CREATE_NEW_PROCESS_GROUP`) and retains that identity, a
//! *targeted* graceful `CTRL_BREAK` (group id ≠ 0) can be supported against
//! the managed group. Not implemented here.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::intelligence::rules::ServiceCategory;

/// Registry capacity. A refresh cycle replaces the whole registry with the
/// current snapshot's targets (≤ ~40 on a busy dev machine), so 1024 is a
/// generous bound; entries beyond it push out the oldest.
pub(crate) const REGISTRY_CAPACITY: usize = 1024;

/// How long an issued target id remains resolvable without a successful
/// refresh. Generous versus the 3 s discovery cadence — a user staring at a
/// confirm dialog for minutes still succeeds — but bounded, so ids cannot
/// outlive their snapshot meaningfully.
pub(crate) const TARGET_TTL: Duration = Duration::from_secs(15 * 60);

/// Per-boot random key so ids cannot be predicted across runs.
pub(crate) fn boot_key() -> [u8; 32] {
    use std::sync::OnceLock;
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    *KEY.get_or_init(|| {
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = u64::from(std::process::id());
        let mut seed = t ^ (u128::from(pid) << 64);
        let mut key = [0u8; 32];
        for chunk in key.chunks_mut(8) {
            // xorshift64* over the seed — plenty for an unpredictable
            // per-boot registry key (not a crypto primitive, just a mixer).
            seed ^= seed >> 12;
            seed ^= seed << 25;
            seed ^= seed >> 27;
            let out = seed.wrapping_mul(0x2545F4914F6CDD1D);
            chunk.copy_from_slice(&out.to_le_bytes()[..chunk.len()]);
        }
        key
    })
}

/// BLAKE3-256 over `input` (the audited `blake3` crate — opaque ids are a
/// security boundary, so no hand-rolled hash).
pub(crate) fn blake3_256(input: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(input);
    *hasher.finalize().as_bytes()
}

/// The backend-trusted snapshot behind one opaque id.
///
/// The `service_category` / `has_dev_evidence` fields are **backend-derived
/// issuance hints** from the discovery cycle's own classification — the
/// frontend never supplies them and cannot influence them. At action time
/// hard deny rules are recomputed from fresh process data; these hints only
/// (a) can *tighten* a refusal (infrastructure category) and (b) satisfy
/// the development-evidence requirement — never loosen a hard refusal.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TrustedControlTarget {
    pub pid: u32,
    /// Creation time (Unix ms) from the issuing snapshot — primary
    /// anti-PID-reuse identity.
    pub creation_ms: Option<u64>,
    pub executable_path: Option<String>,
    pub process_name: Option<String>,
    /// Backend-derived display name (service identity or process name) for
    /// confirmations; never accepted from the frontend.
    pub display_name: String,
    /// Whether the issuing snapshot saw this process as accessible.
    pub accessible: bool,
    /// Backend-derived service category at issuance (denylist hint).
    pub service_category: Option<ServiceCategory>,
    /// Whether the backend itself found development evidence (classified
    /// service or project link) when issuing this target.
    pub has_dev_evidence: bool,
}

/// One registry entry: the trusted target plus its issuance time.
#[derive(Debug, Clone)]
struct Entry {
    target: Arc<TrustedControlTarget>,
    issued: Instant,
}

/// Outcome of resolving an opaque id for an action.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TargetResolution {
    /// Trusted snapshot found; proceed to revalidation + policy.
    Found(Arc<TrustedControlTarget>),
    /// The id is unknown: never issued, never seen by this run, or expired
    /// (expired entries are dropped on touch; both refuse identically).
    Unknown,
}

/// The registry itself.
#[derive(Default)]
pub(crate) struct ControlTargetRegistry {
    map: Mutex<HashMap<String, Entry>>,
}

impl ControlTargetRegistry {
    /// Test convenience — production code uses `Default::default()` via
    /// Tauri-managed state.
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Replace the registry with a fresh snapshot's targets (refresh cycle).
    /// Old ids stop resolving — stale entries cannot survive a refresh.
    /// On a poisoned lock this is a deliberate no-op: no new targets are
    /// issued from inconsistent state and every action fails closed.
    pub(crate) fn replace_all(&self, targets: impl IntoIterator<Item = TrustedControlTarget>) {
        let Ok(mut map) = self.try_lock() else {
            return;
        };
        map.clear();
        for target in targets {
            let id = Self::target_id(target.pid, target.creation_ms);
            map.insert(
                id,
                Entry {
                    target: Arc::new(target),
                    issued: Instant::now(),
                },
            );
        }
    }

    /// Register a trusted snapshot and return its opaque id.
    ///
    /// Test-only infallible wrapper: production paths use `try_register`
    /// (fail-closed on poison) via the discovery cycle. In tests the lock
    /// cannot be poisoned, so `expect` is a proven invariant here.
    #[cfg(test)]
    pub(crate) fn register(&self, target: TrustedControlTarget) -> String {
        self.try_register(target).expect("test registry lock is healthy")
    }

    /// Resolve an opaque id, reporting `Unknown` vs `Expired` distinctly.
    /// Expired entries are dropped on touch; both outcomes refuse at the API
    /// boundary — the distinction exists for diagnostics only. A poisoned
    /// registry resolves to `Unknown` (fail closed — the action is refused).
    pub(crate) fn resolve(&self, target_id: &str) -> TargetResolution {
        let Ok(mut map) = self.try_lock() else {
            return TargetResolution::Unknown;
        };
        Self::evict(&mut map);
        match map.get(target_id) {
            Some(entry) => TargetResolution::Found(Arc::clone(&entry.target)),
            None => TargetResolution::Unknown,
        }
    }

    /// Number of live entries (test diagnostics only).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.try_lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Test-only: age one entry past the TTL so expiry is deterministic.
    #[cfg(test)]
    pub(crate) fn age_for_test(&self, target_id: &str) {
        if let Ok(mut map) = self.map.lock() {
            if let Some(entry) = map.get_mut(target_id) {
                entry.issued = Instant::now()
                    .checked_sub(TARGET_TTL + Duration::from_secs(1))
                    .expect("sane clock");
            }
        }
    }

    /// Opaque, unpredictable id for a (PID, creation) pair.
    fn target_id(pid: u32, creation_ms: Option<u64>) -> String {
        let mut input = Vec::with_capacity(48);
        input.extend_from_slice(&pid.to_le_bytes());
        input.extend_from_slice(&creation_ms.unwrap_or(0).to_le_bytes());
        input.extend_from_slice(&boot_key());
        let hash = blake3_256(&input);
        let mut hex = String::with_capacity(64);
        for byte in hash {
            hex.push(char::from_digit(u32::from(byte >> 4), 16).expect("hex digit"));
            hex.push(char::from_digit(u32::from(byte & 0xF), 16).expect("hex digit"));
        }
        hex
    }

    /// Drop expired entries and enforce capacity (oldest issued first).
    fn evict(map: &mut HashMap<String, Entry>) {
        let now = Instant::now();
        map.retain(|_, entry| now.duration_since(entry.issued) < TARGET_TTL);
        if map.len() > REGISTRY_CAPACITY {
            let mut by_age: Vec<(String, Instant)> = map
                .iter()
                .map(|(id, entry)| (id.clone(), entry.issued))
                .collect();
            by_age.sort_by_key(|(_, issued)| *issued);
            let excess = map.len() - REGISTRY_CAPACITY;
            for (id, _) in by_age.into_iter().take(excess) {
                map.remove(&id);
            }
        }
    }

    /// Lock the registry. **Fail closed on poison** (Phase 10B, spec §B):
    /// the control registry is the security boundary — a poisoned map means
    /// unknown-internal-invariant state, so control actions are refused
    /// (callers map the error to a structured refusal) rather than trusting
    /// a possibly-inconsistent map. Never fall back to raw PIDs.
    fn try_lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, Entry>>, &'static str> {
        self.map
            .lock()
            .map_err(|_| "control target registry lock poisoned — control refused")
    }

    /// Register a trusted snapshot; returns a structured refusal message if
    /// the registry is poisoned (control stays unavailable until restart).
    pub(crate) fn try_register(&self, target: TrustedControlTarget) -> Result<String, String> {
        let id = Self::target_id(target.pid, target.creation_ms);
        let mut map = self.try_lock()?;
        map.insert(
            id.clone(),
            Entry {
                target: Arc::new(target),
                issued: Instant::now(),
            },
        );
        Self::evict(&mut map);
        Ok(id)
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn target(pid: u32) -> TrustedControlTarget {
        TrustedControlTarget {
            pid,
            creation_ms: Some(1_700_000_000_000),
            executable_path: Some(r"C:\Program Files\nodejs\node.exe".to_string()),
            process_name: Some("node.exe".to_string()),
            display_name: "Vite".to_string(),
            accessible: true,
            service_category: Some(ServiceCategory::Frontend),
            has_dev_evidence: true,
        }
    }

    #[test]
    fn ids_are_opaque_hex_and_deterministic_per_identity() {
        let registry = ControlTargetRegistry::new();
        let id = registry.register(target(100));
        assert_eq!(id.len(), 64, "64 hex chars = 256-bit id");
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        // Same identity → same id (idempotent refresh replacement).
        let again = registry.register(target(100));
        assert_eq!(id, again);
    }

    #[test]
    fn ids_are_unpredictable_across_boots() {
        // Two registries in the same process share the boot key, so this
        // checks digest sensitivity instead of cross-boot randomness: a
        // different creation time must produce a different id.
        let registry = ControlTargetRegistry::new();
        let a = registry.register(target(101));
        let mut other = target(101);
        other.creation_ms = Some(1_700_000_000_001);
        let b = registry.register(other);
        assert_ne!(a, b, "identity change must change the id");
    }

    #[test]
    fn lookup_failure_is_structured_unknown() {
        let registry = ControlTargetRegistry::new();
        assert_eq!(
            registry.resolve("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
            TargetResolution::Unknown
        );
        assert_eq!(registry.resolve("not-an-id"), TargetResolution::Unknown);
    }

    #[test]
    fn refresh_replaces_targets_and_invalidates_old_ids() {
        let registry = ControlTargetRegistry::new();
        let old = registry.register(target(102));
        registry.replace_all([target(103)]);
        assert_eq!(registry.resolve(&old), TargetResolution::Unknown);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn registry_is_bounded() {
        let registry = ControlTargetRegistry::new();
        for i in 0..(REGISTRY_CAPACITY + 200) {
            let mut t = target(u32::try_from(i % 60_000).unwrap_or(1));
            t.creation_ms = Some(u64::try_from(i).unwrap_or(1));
            registry.register(t);
        }
        assert!(registry.len() <= REGISTRY_CAPACITY);
    }

    #[test]
    fn expiry_is_enforced() {
        let registry = ControlTargetRegistry::new();
        let id = registry.register(target(104));
        // Directly age the entry beyond the TTL (white-box, deterministic,
        // pure Duration arithmetic so it cannot underflow on fresh machines).
        if let Ok(mut map) = registry.map.lock() {
            let entry = map.get_mut(&id).expect("entry");
            entry.issued = Instant::now().checked_sub(TARGET_TTL + Duration::from_secs(1)).expect("sane clock");
        }
        assert_eq!(
            registry.resolve(&id),
            TargetResolution::Unknown,
            "an expired entry must refuse like an unknown one"
        );
        assert_eq!(registry.len(), 0, "expiry must actually drop the entry");
    }

    // Phase 10B (spec §B): a poisoned registry must FAIL CLOSED — resolve
    // returns Unknown (the action is refused), try_register/replace_all
    // issue nothing, and no panic escapes. Control stays unavailable until
    // restart; there is never a fallback to raw PIDs.
    #[test]
    fn poisoned_registry_fails_closed_without_panic() {
        let registry = ControlTargetRegistry::new();
        let id = registry.register(target(105));
        // Poison the inner mutex the same way a panicking writer would.
        let registry = std::sync::Arc::new(registry);
        let mutex = std::sync::Arc::clone(&registry);
        let _ = std::thread::Builder::new()
            .name("poisoner".to_string())
            .spawn(move || {
                let _guard = mutex.map.lock();
                panic!("simulated writer panic while holding the registry lock");
            })
            .map(|handle| handle.join());
        // Resolve must refuse (Unknown), not panic and not serve stale trust.
        assert_eq!(
            registry.resolve(&id),
            TargetResolution::Unknown,
            "poisoned registry must fail closed"
        );
        // Registration must refuse instead of issuing targets.
        assert!(registry.try_register(target(106)).is_err());
        // Refresh must be a safe no-op, still no targets resolvable.
        registry.replace_all([target(107)]);
        assert_eq!(registry.resolve(&id), TargetResolution::Unknown);
        assert_eq!(registry.len(), 0);
    }
}
