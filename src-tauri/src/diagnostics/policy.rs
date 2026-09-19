//! Phase 11C Task 3 — severe/critical capture decision engine (pure policy,
//! spec §4). No I/O, no clock reads: time is injected as `now_ms`, so every
//! rule is deterministically unit-testable.
//!
//! Trigger policy (spec §4, plan §8):
//! - 2 severe events for the same fingerprint within 2 minutes → one bundle;
//! - after capture, 10-minute cooldown for that fingerprint;
//! - during cooldown, events still update history (Task 2 counters) but
//!   never produce another capture;
//! - critical bypasses the 2-in-2-minutes threshold but honors cooldown.

use super::incidents::{IncidentKey, Severity};

/// Severe threshold: 2 matching severe failures...
#[allow(dead_code)] // consumed by the reporting path in Task 4/6
pub(crate) const SEVERE_THRESHOLD: usize = 2;
/// ...within this window (2 minutes, ms).
#[allow(dead_code)]
pub(crate) const SEVERE_WINDOW_MS: u64 = 120_000;
/// Cooldown after a capture request (10 minutes, ms).
#[allow(dead_code)]
pub(crate) const COOLDOWN_MS: u64 = 600_000;

/// What the policy decided for one reported event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum CaptureDecision {
    /// No capture (below threshold / in cooldown / warning-or-info).
    None,
    /// Build one support bundle for this fingerprint.
    RequestBundle,
}

/// Per-fingerprint policy state. The engine owns one map from incident key
/// to this state; kept tiny and bounded by the incident index cap that
/// feeds it (keys are only added when incidents are recorded).
#[derive(Debug, Default, Clone)]
#[allow(dead_code)]
pub(crate) struct CapturePolicyState {
    severe_timestamps_ms: Vec<u64>,
    cooldown_until_ms: Option<u64>,
}

impl CapturePolicyState {
    /// Process one reported event for `key` at `now_ms`.
    #[allow(dead_code)]
    pub(crate) fn on_event(
        &mut self,
        _key: &IncidentKey,
        severity: Severity,
        now_ms: u64,
    ) -> CaptureDecision {
        // Anti-spam is universal: warnings/info never capture, and nothing
        // captures during cooldown.
        match severity {
            Severity::Info | Severity::Warning => return CaptureDecision::None,
            Severity::Severe | Severity::Critical => {}
        }
        if let Some(until) = self.cooldown_until_ms {
            if now_ms < until {
                // History still updates (Task 2); only capture is suppressed.
                // Nothing from inside the cooldown counts toward the next
                // post-cooldown window: the cooldown is a clean reset.
                return CaptureDecision::None;
            }
            self.cooldown_until_ms = None;
            self.severe_timestamps_ms.clear();
        }
        match severity {
            Severity::Critical => {
                // Bypass the threshold; still anti-spam via cooldown.
                self.cooldown_until_ms = Some(now_ms.saturating_add(COOLDOWN_MS));
                self.severe_timestamps_ms.clear();
                CaptureDecision::RequestBundle
            }
            Severity::Severe => {
                self.severe_timestamps_ms.push(now_ms);
                let cutoff = now_ms.saturating_sub(SEVERE_WINDOW_MS);
                self.severe_timestamps_ms.retain(|&t| t >= cutoff);
                if self.severe_timestamps_ms.len() >= SEVERE_THRESHOLD {
                    self.cooldown_until_ms = Some(now_ms.saturating_add(COOLDOWN_MS));
                    self.severe_timestamps_ms.clear();
                    CaptureDecision::RequestBundle
                } else {
                    CaptureDecision::None
                }
            }
            Severity::Info | Severity::Warning => unreachable!("filtered above"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> IncidentKey {
        IncidentKey {
            subsystem: "docker".into(),
            fingerprint: super::super::incidents::fingerprint(
                "docker", "ERR_X", "probe", Severity::Severe,
            ),
        }
    }

    #[test]
    fn two_severe_within_two_minutes_request_bundle() {
        let mut st = CapturePolicyState::default();
        let k = key();
        assert_eq!(st.on_event(&k, Severity::Severe, 0), CaptureDecision::None);
        assert_eq!(
            st.on_event(&k, Severity::Severe, 60_000),
            CaptureDecision::RequestBundle,
            "2 severe within 2 minutes → capture"
        );
    }

    #[test]
    fn two_severe_outside_window_do_not() {
        let mut st = CapturePolicyState::default();
        let k = key();
        assert_eq!(st.on_event(&k, Severity::Severe, 0), CaptureDecision::None);
        // Window is 120_000 ms inclusive of start; strictly past it → no.
        assert_eq!(
            st.on_event(&k, Severity::Severe, SEVERE_WINDOW_MS + 1),
            CaptureDecision::None,
            "outside the 2-minute window → no capture"
        );
    }

    #[test]
    fn cooldown_blocks_second_bundle_for_10_minutes() {
        let mut st = CapturePolicyState::default();
        let k = key();
        st.on_event(&k, Severity::Severe, 0);
        assert_eq!(
            st.on_event(&k, Severity::Severe, 60_000),
            CaptureDecision::RequestBundle
        );
        // Inside cooldown: more severes never re-capture.
        for t in [120_000, 300_000, 599_999, COOLDOWN_MS - 1] {
            assert_eq!(
                st.on_event(&k, Severity::Severe, t),
                CaptureDecision::None,
                "t={t} still in cooldown"
            );
        }
        // Cooldown timeline: the capture happened at t=60_000, so cooldown
        // runs until 60_000 + 600_000 = 660_000. The window is a clean
        // reset, so the next TWO severes after expiry capture again.
        assert_eq!(
            st.on_event(&k, Severity::Severe, 660_000),
            CaptureDecision::None,
            "first severe after expiry alone is not enough"
        );
        assert_eq!(
            st.on_event(&k, Severity::Severe, 660_001),
            CaptureDecision::RequestBundle,
            "post-cooldown capture works"
        );
    }

    #[test]
    fn critical_bypasses_threshold_but_not_cooldown() {
        let mut st = CapturePolicyState::default();
        let k = key();
        assert_eq!(
            st.on_event(&k, Severity::Critical, 0),
            CaptureDecision::RequestBundle,
            "critical captures immediately"
        );
        assert_eq!(
            st.on_event(&k, Severity::Critical, 1_000),
            CaptureDecision::None,
            "critical 1 s later is cooldown-suppressed"
        );
        assert_eq!(
            st.on_event(&k, Severity::Critical, COOLDOWN_MS),
            CaptureDecision::RequestBundle,
            "after cooldown, critical captures again"
        );
    }

    #[test]
    fn warning_and_info_never_capture() {
        let mut st = CapturePolicyState::default();
        let k = key();
        for i in 0..100u64 {
            assert_eq!(st.on_event(&k, Severity::Warning, i), CaptureDecision::None);
            assert_eq!(st.on_event(&k, Severity::Info, i), CaptureDecision::None);
        }
        // …and they never set a cooldown that would block a later severe.
        st.on_event(&k, Severity::Severe, 1_000);
        assert_eq!(
            st.on_event(&k, Severity::Severe, 2_000),
            CaptureDecision::RequestBundle
        );
    }

    #[test]
    fn hundred_matching_severe_events_produce_at_most_one_bundle_per_cooldown() {
        let mut st = CapturePolicyState::default();
        let k = key();
        let mut captures = 0;
        for i in 0..100u64 {
            if st.on_event(&k, Severity::Severe, i * 50) == CaptureDecision::RequestBundle {
                captures += 1;
            }
        }
        assert_eq!(captures, 1, "hammering coalesces to one capture");
    }

    #[test]
    fn severe_events_far_apart_never_accumulate_past_window() {
        let mut st = CapturePolicyState::default();
        let k = key();
        // One severe every 3 minutes: the window always holds ≤ 1.
        for i in 0..10u64 {
            let t = i * 180_000;
            assert_eq!(st.on_event(&k, Severity::Severe, t), CaptureDecision::None);
        }
    }
}
