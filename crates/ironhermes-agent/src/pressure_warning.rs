// Phase 18 Plan 05: Three-channel pressure warning (D-23, D-24).
//
// PressureTracker fires at 85% of the engine's compression threshold:
//   1. tracing::warn! with structured fields
//   2. HookEventKind::ContextPressure hook event (via fire_awaitable)
//   3. Transient system message queued for next turn (one-shot, consumed on read)
//
// Cooldown is per-session: re-fires only after the ratio descends below
// the warning trigger and then crosses back above.

use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Fraction of the engine's compression threshold at which the pressure
/// warning fires (85%).
pub const PRESSURE_WARNING_FRACTION: f32 = 0.85;

/// Text injected as a transient system message on the next turn when
/// context pressure is high.
pub const TRANSIENT_WARNING_TEXT: &str =
    "[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]";

/// Per-session state for cooldown tracking.
#[derive(Default)]
struct SessionState {
    /// True once we've fired a warning during the current "above threshold"
    /// crossing.  Cleared when the ratio descends below the trigger.
    above_threshold: bool,
    /// Pending one-shot transient message for this session (consumed by
    /// `take_transient`).
    pending_transient: Option<String>,
    /// Phase 18-14: running total of warnings fired for this session across
    /// all crossings.  Used by REPL-harness tests to assert exactly-once
    /// firing across multiple turns.
    warn_count: u32,
}

/// Thread-safe per-session pressure tracker.
///
/// Clone-safe: all clones share the same inner state via `Arc<Mutex<…>>`.
#[derive(Default, Clone)]
pub struct PressureTracker {
    inner: Arc<Mutex<HashMap<String, SessionState>>>,
}

impl PressureTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check whether the current token ratio crosses the warning trigger for
    /// this session and, if so, emit all three channels.
    ///
    /// Returns `true` if a warning was emitted during this call; `false` if
    /// the ratio is below the trigger or the cooldown is still active.
    ///
    /// # Arguments
    ///
    /// * `session_id`       – opaque session identifier
    /// * `engine_threshold` – the engine's compression threshold (e.g. 0.5)
    /// * `estimated_tokens` – current estimated token count
    /// * `context_length`   – model's context window size
    /// * `mode`             – `"soft"` or `"hard"`
    /// * `hooks`            – optional hook registry for the `context:pressure`
    ///   channel; pass `None` in environments without a hook bus
    pub async fn check_and_maybe_emit(
        &self,
        session_id: &str,
        engine_threshold: f32,
        estimated_tokens: usize,
        context_length: usize,
        mode: &str,
        hooks: Option<&HookRegistry>,
    ) -> bool {
        let percent = estimated_tokens as f32 / context_length.max(1) as f32;
        let warning_trigger = engine_threshold * PRESSURE_WARNING_FRACTION;
        let crossed = percent >= warning_trigger;

        {
            let mut map = self.inner.lock().unwrap();
            let state = map.entry(session_id.to_string()).or_default();

            if !crossed {
                // Descent below trigger resets the cooldown so the next
                // crossing will fire again.
                state.above_threshold = false;
                return false;
            }
            if state.above_threshold {
                // Already warned this crossing — cooldown active.
                return false;
            }
            // First crossing — arm the cooldown and queue the transient.
            state.above_threshold = true;
            state.pending_transient = Some(TRANSIENT_WARNING_TEXT.to_string());
            state.warn_count = state.warn_count.saturating_add(1);
        }

        // ── Channel 1: tracing ────────────────────────────────────────────
        tracing::warn!(
            session_id = %session_id,
            estimated_tokens,
            threshold = engine_threshold,
            percent_used = percent,
            mode = %mode,
            "context pressure warning (85% of compression threshold)"
        );

        // ── Channel 2: hook event ─────────────────────────────────────────
        if let Some(reg) = hooks {
            let event = HookEvent::new(
                "req-pressure",
                HookEventKind::ContextPressure {
                    session_id: session_id.to_string(),
                    estimated_tokens,
                    threshold: engine_threshold,
                    percent_used: percent,
                    mode: mode.to_string(),
                },
            );
            reg.fire_awaitable(event).await;
        }

        // ── Channel 3: transient message already queued above ─────────────
        true
    }

    /// Consume the pending transient message for this session (one-shot).
    ///
    /// Returns `Some(msg)` the first time after a warning was fired, then
    /// `None` on subsequent calls until the next warning cycle.
    pub fn take_transient(&self, session_id: &str) -> Option<String> {
        let mut map = self.inner.lock().unwrap();
        map.get_mut(session_id)
            .and_then(|s| s.pending_transient.take())
    }

    /// Phase 18-13: test-only accessor — returns `true` if this session has
    /// crossed the pressure threshold at least once (i.e., `above_threshold`
    /// is currently set).  Used by unit tests that verify tracker fires
    /// without an active hook registry.
    #[cfg(test)]
    pub fn was_warned(&self, session_id: &str) -> bool {
        let map = self.inner.lock().unwrap();
        map.get(session_id)
            .map(|s| s.above_threshold)
            .unwrap_or(false)
    }

    /// Phase 18-14: test-only accessor — returns the running count of
    /// warnings fired for this session across all crossings.  Used by
    /// REPL-harness tests to assert "fired exactly once across 3 turns".
    #[cfg(test)]
    pub fn warn_count(&self, session_id: &str) -> u32 {
        let map = self.inner.lock().unwrap();
        map.get(session_id).map(|s| s.warn_count).unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_hooks::{HookEventKind, HookRegistry, HooksConfig};

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_registry_with_capture() -> (
        Arc<HookRegistry>,
        Arc<Mutex<Vec<ironhermes_hooks::HookEvent>>>,
    ) {
        use ironhermes_hooks::HookEvent;
        let captured: Arc<Mutex<Vec<HookEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let cap = Arc::clone(&captured);
        let mut registry = HookRegistry::new(HooksConfig::default());
        registry.add_async_listener(Arc::new(move |event: HookEvent| {
            let cap = Arc::clone(&cap);
            Box::pin(async move {
                cap.lock().unwrap().push(event);
            })
        }));
        (Arc::new(registry), captured)
    }

    // ── Tests ────────────────────────────────────────────────────────────────

    /// All three channels fire when estimated_tokens/context_length exceeds 85%
    /// of the engine threshold.
    ///
    /// Engine threshold = 0.5 → warning_trigger = 0.425.
    /// Ratio = 0.45 (>0.425) → should fire.
    #[tokio::test]
    async fn pressure_warning_all_channels() {
        let (reg, captured) = make_registry_with_capture();
        let tracker = PressureTracker::new();

        // 450/1000 = 0.45 > 0.5 * 0.85 = 0.425
        let fired = tracker
            .check_and_maybe_emit("sess-a", 0.5, 450, 1000, "soft", Some(&reg))
            .await;
        assert!(fired, "should have fired");

        // Channel 2: hook event captured
        let events = captured.lock().unwrap();
        let pressure_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, HookEventKind::ContextPressure { .. }))
            .collect();
        assert_eq!(
            pressure_events.len(),
            1,
            "exactly one ContextPressure event"
        );

        if let HookEventKind::ContextPressure {
            session_id,
            estimated_tokens,
            threshold,
            percent_used,
            mode,
        } = &pressure_events[0].kind
        {
            assert_eq!(session_id, "sess-a");
            assert_eq!(*estimated_tokens, 450);
            assert!((threshold - 0.5).abs() < f32::EPSILON);
            assert!((percent_used - 0.45).abs() < 0.001);
            assert_eq!(mode, "soft");
        } else {
            panic!("wrong kind");
        }
        drop(events);

        // Channel 3: transient message queued
        let msg = tracker.take_transient("sess-a");
        assert_eq!(msg.as_deref(), Some(TRANSIENT_WARNING_TEXT));
    }

    /// Calling check_and_maybe_emit twice above threshold → second call is
    /// suppressed (cooldown).  After a descent below threshold, the next
    /// above-threshold call fires again.
    #[tokio::test]
    async fn pressure_cooldown() {
        let tracker = PressureTracker::new();

        // First call: above threshold → fires
        let fired1 = tracker
            .check_and_maybe_emit("sess-b", 0.5, 450, 1000, "hard", None)
            .await;
        assert!(fired1, "first call should fire");

        // Second call: still above threshold → cooldown suppresses
        let fired2 = tracker
            .check_and_maybe_emit("sess-b", 0.5, 450, 1000, "hard", None)
            .await;
        assert!(!fired2, "second call should be suppressed by cooldown");

        // Descent: ratio below trigger (400/1000 = 0.40 < 0.425) → reset
        let fired3 = tracker
            .check_and_maybe_emit("sess-b", 0.5, 400, 1000, "hard", None)
            .await;
        assert!(!fired3, "below threshold should not fire");

        // Re-crossing: ratio above trigger again → fires again
        let fired4 = tracker
            .check_and_maybe_emit("sess-b", 0.5, 450, 1000, "hard", None)
            .await;
        assert!(
            fired4,
            "should fire again after cooldown cleared on descent"
        );
    }

    /// Sessions A and B are independent: both fire on first crossing without
    /// interfering with each other's cooldowns.
    #[tokio::test]
    async fn pressure_cooldown_per_session() {
        let tracker = PressureTracker::new();

        // Session A fires
        let a1 = tracker
            .check_and_maybe_emit("sess-c", 0.5, 450, 1000, "soft", None)
            .await;
        assert!(a1, "session A first call should fire");

        // Session B fires independently
        let b1 = tracker
            .check_and_maybe_emit("sess-d", 0.5, 450, 1000, "soft", None)
            .await;
        assert!(b1, "session B first call should fire independently");

        // Both are now in cooldown
        let a2 = tracker
            .check_and_maybe_emit("sess-c", 0.5, 450, 1000, "soft", None)
            .await;
        let b2 = tracker
            .check_and_maybe_emit("sess-d", 0.5, 450, 1000, "soft", None)
            .await;
        assert!(!a2, "session A second call should be suppressed");
        assert!(!b2, "session B second call should be suppressed");
    }

    /// take_transient returns Some once, then None (consumed semantics).
    #[tokio::test]
    async fn pressure_transient_message_one_shot() {
        let tracker = PressureTracker::new();

        tracker
            .check_and_maybe_emit("sess-e", 0.5, 450, 1000, "soft", None)
            .await;

        let first = tracker.take_transient("sess-e");
        assert_eq!(
            first.as_deref(),
            Some(TRANSIENT_WARNING_TEXT),
            "first take should return the message"
        );

        let second = tracker.take_transient("sess-e");
        assert!(
            second.is_none(),
            "second take should return None (consumed)"
        );
    }

    /// No warning fires when the ratio is below the 85% trigger.
    ///
    /// Engine threshold = 0.5 → trigger = 0.425.
    /// Ratio = 0.40 < 0.425 → no fire.
    #[tokio::test]
    async fn pressure_no_warn_below_threshold() {
        let (reg, captured) = make_registry_with_capture();
        let tracker = PressureTracker::new();

        // 400/1000 = 0.40 < 0.425
        let fired = tracker
            .check_and_maybe_emit("sess-f", 0.5, 400, 1000, "soft", Some(&reg))
            .await;
        assert!(!fired, "should not fire below threshold");

        let events = captured.lock().unwrap();
        let pressure_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e.kind, HookEventKind::ContextPressure { .. }))
            .collect();
        assert!(
            pressure_events.is_empty(),
            "no hook event should be emitted"
        );

        let msg = tracker.take_transient("sess-f");
        assert!(msg.is_none(), "no transient message below threshold");
    }
}
