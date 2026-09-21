//! Phase 11C Task 18 — notification decision logic (spec §23).
//! Pure, independently unit-testable decision input (Focus) → decision.
//! Foreground → in-app banner; Background → native toast; Unknown/query
//! failure → conservative InAppBanner (never guess background). Dispatch is
//! injected; plugin failure is swallowed with one bounded warning — a
//! notification failure never affects bundle persistence or the core app.

/// Focus state of the main window at decision time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Foreground,
    Background,
    Unknown,
}

/// What the notification layer should do after a capture outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotificationDecision {
    InAppBanner,
    NativeToast,
    Suppress,
}

/// Fixed, non-sensitive notification template (spec §23: no paths, IDs,
/// usernames, tokens, or raw errors in notification text).
pub(crate) const CAPTURED_BANNER_TEXT: &str = "A support bundle was captured. Open Diagnostics to review it.";
pub(crate) const CAPTURED_TOAST_TEXT: &str = "A support bundle was captured.";

/// Map a window handle to a Focus value. `None` or any query error →
/// `Unknown` (conservative).
pub(crate) fn focus_of(win: Option<&tauri::WebviewWindow>) -> Focus {
    let Some(win) = win else {
        return Focus::Unknown;
    };
    match win.is_focused() {
        Ok(true) => Focus::Foreground,
        Ok(false) => Focus::Background,
        Err(_) => Focus::Unknown,
    }
}

/// Pure decision function (spec §23 + amendment ruling J):
/// Foreground && !already-notified → InAppBanner
/// Background && !already-notified && !cooldown → NativeToast
/// Unknown → InAppBanner (query failure NEVER guesses Background)
/// else Suppress.
pub(crate) fn decide(
    focus: Focus,
    already_notified_this_capture: bool,
    cooldown_active: bool,
) -> NotificationDecision {
    if already_notified_this_capture {
        return NotificationDecision::Suppress;
    }
    match focus {
        Focus::Foreground => NotificationDecision::InAppBanner,
        Focus::Background => {
            if cooldown_active {
                NotificationDecision::Suppress
            } else {
                NotificationDecision::NativeToast
            }
        }
        // Amendment ruling J: on focus-query failure fall back to the banner,
        // which remains visible when the user returns to the app.
        Focus::Unknown => NotificationDecision::InAppBanner,
    }
}

/// Execute a decision. The native toast closure is injected; any failure is
/// swallowed with one bounded warning (never propagated, never affects the
/// capture worker or bundle persistence).
pub(crate) fn dispatch(
    decision: NotificationDecision,
    native: &dyn Fn(&str) -> Result<(), String>,
) {
    match decision {
        NotificationDecision::InAppBanner => {
            // The banner is surfaced through the diagnostics state polled by
            // the frontend (Task 14 state slot); nothing OS-side to run here.
        }
        NotificationDecision::NativeToast => {
            if let Err(err) = native(CAPTURED_TOAST_TEXT) {
                crate::diagnostics::warn(
                    "diagnostics",
                    &format!("native notification unavailable: {err}"),
                );
            }
        }
        NotificationDecision::Suppress => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_query_error_falls_back_to_in_app_banner() {
        // focus_of(None) → Unknown (query error/no window), and Unknown
        // decides InAppBanner — NEVER NativeToast (amendment ruling J).
        assert_eq!(focus_of(None), Focus::Unknown);
        assert!(matches!(
            decide(Focus::Unknown, false, false),
            NotificationDecision::InAppBanner
        ));
        assert!(!matches!(
            decide(Focus::Unknown, false, false),
            NotificationDecision::NativeToast
        ));
    }

    #[test]
    fn foreground_prefers_in_app_banner() {
        assert!(matches!(
            decide(Focus::Foreground, false, false),
            NotificationDecision::InAppBanner
        ));
    }

    #[test]
    fn background_uses_native_toast_once() {
        assert!(matches!(
            decide(Focus::Background, false, false),
            NotificationDecision::NativeToast
        ));
    }

    #[test]
    fn duplicates_and_cooldowns_are_suppressed() {
        assert!(matches!(
            decide(Focus::Background, true, false),
            NotificationDecision::Suppress
        ));
        assert!(matches!(
            decide(Focus::Background, false, true),
            NotificationDecision::Suppress
        ));
    }

    #[test]
    fn native_failure_is_swallowed() {
        // A failing native closure must not panic or propagate; the banner
        // path logs nothing (banner is not dispatched here).
        let failing = |_: &str| -> Result<(), String> { Err("synthetic failure".to_string()) };
        dispatch(NotificationDecision::NativeToast, &failing); // must not panic
    }

    #[test]
    fn notification_text_has_no_sensitive_content() {
        // Fixed templates only — no interpolation of paths/errors/IDs.
        assert_eq!(
            CAPTURED_TOAST_TEXT,
            "A support bundle was captured."
        );
        assert!(!CAPTURED_BANNER_TEXT.contains('\\'));
        assert!(!CAPTURED_BANNER_TEXT.contains("C:"));
        assert!(!CAPTURED_TOAST_TEXT.contains(':'));
    }
}
