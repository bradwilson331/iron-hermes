//! E-12 / S-17 — pressure-advisory injection check (Phase 21.7 Plan 05 Task 5-03).
//!
//! Drives a BudgetHandle across None→Caution70→Warning90→Stop100 and asserts
//! the AgentLoop-facing contract:
//!
//!   1. After crossing into Caution70, exactly ONE CAUTION_ADVISORY is injected.
//!   2. Steady-state at Caution70 does NOT spam additional CAUTION advisories.
//!   3. Crossing into Warning90 injects exactly ONE WARNING_ADVISORY.
//!   4. Stop100 consume() returns None (used by AgentLoop::run to clean-stop
//!      via AgentResult::budget_exhausted).
//!
//! This test is STRUCTURAL — it drives the handle directly and uses a
//! hand-rolled `last_tier_seen` cell to mirror the AgentLoop injection
//! semantics. The fully-plumbed run-loop e2e test is intentionally out of
//! scope to keep this test under the 15s Nyquist bound and avoid requiring
//! a recording provider mock.

use ironhermes_agent::budget::{
    BudgetHandle, CAUTION_ADVISORY, PressureTier, WARNING_ADVISORY, advisory_text,
};
use ironhermes_core::ChatMessage;

/// Mirrors the AgentLoop turn-top injection contract:
/// consume + if pressure tier crossed, push advisory to messages.
fn simulate_turn(
    handle: &BudgetHandle,
    messages: &mut Vec<ChatMessage>,
    last_tier_seen: &mut PressureTier,
) -> bool /* true iff turn proceeded; false iff Stop100 clean-stop */ {
    match handle.consume() {
        None => {
            // Stop100 clean-stop — agent loop returns budget_exhausted here.
            return false;
        }
        Some(_) => {}
    }
    let tier = handle.pressure();
    if tier != *last_tier_seen {
        *last_tier_seen = tier;
        if let Some(advisory) = advisory_text(tier) {
            messages.push(ChatMessage::system(advisory));
        }
    }
    true
}

#[test]
fn budget_tiers_reach_expected_states_at_exact_consume_counts() {
    // Unit-level pre-test: confirm BudgetHandle tier math matches what
    // AgentLoop will see when it consumes at turn-top.
    let b = BudgetHandle::new(100);
    assert_eq!(b.pressure(), PressureTier::None);
    for _ in 0..69 {
        b.consume();
    }
    assert_eq!(b.pressure(), PressureTier::None);
    b.consume(); // 70 used
    assert_eq!(b.pressure(), PressureTier::Caution70);
    for _ in 0..19 {
        b.consume();
    }
    assert_eq!(b.pressure(), PressureTier::Caution70);
    b.consume(); // 90 used
    assert_eq!(b.pressure(), PressureTier::Warning90);
    for _ in 0..9 {
        b.consume();
    }
    assert_eq!(b.pressure(), PressureTier::Warning90);
    b.consume(); // 100 used
    assert_eq!(b.pressure(), PressureTier::Stop100);
    assert!(b.consume().is_none());
}

#[test]
fn e12_advisory_injected_exactly_once_per_tier_crossing_and_stop100_clean_stops() {
    // max=10 → Caution70 at used=7, Warning90 at used=9, Stop100 at used=10.
    let handle = BudgetHandle::new(10);
    let mut messages: Vec<ChatMessage> = Vec::new();
    let mut last_tier_seen = PressureTier::None;

    // Turn 1 → used=1 → None; no advisory.
    assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    assert!(advisory_count(&messages, CAUTION_ADVISORY) == 0);
    assert!(advisory_count(&messages, WARNING_ADVISORY) == 0);

    // Turns 2..=6 → used=2..=6 → still None; no advisory.
    for _ in 2..=6 {
        assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    }
    assert_eq!(
        advisory_count(&messages, CAUTION_ADVISORY),
        0,
        "no CAUTION advisory before crossing 70%"
    );
    assert_eq!(advisory_count(&messages, WARNING_ADVISORY), 0);

    // Turn 7 → used=7 → Caution70 crossing → EXACTLY ONE CAUTION advisory.
    assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    assert_eq!(
        advisory_count(&messages, CAUTION_ADVISORY),
        1,
        "exactly one CAUTION_ADVISORY injected on None→Caution70 crossing (E-12)"
    );

    // Turn 8 → used=8 → still Caution70 → NO additional advisory (no spam, T-21.7-05-02).
    assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    assert_eq!(
        advisory_count(&messages, CAUTION_ADVISORY),
        1,
        "NO additional CAUTION advisory at steady-state (T-21.7-05-02 no spam)"
    );

    // Turn 9 → used=9 → Warning90 crossing → EXACTLY ONE WARNING advisory.
    assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    assert_eq!(
        advisory_count(&messages, WARNING_ADVISORY),
        1,
        "exactly one WARNING_ADVISORY injected on Caution70→Warning90 crossing (E-12)"
    );
    assert_eq!(
        advisory_count(&messages, CAUTION_ADVISORY),
        1,
        "CAUTION count unchanged across Warning90 transition"
    );

    // Turn 10 → used=10 → Stop100.
    // On this turn, consume() still returns Some(0) and pressure()==Stop100 but
    // advisory_text(Stop100)==None, so NO additional advisory is injected.
    assert!(simulate_turn(&handle, &mut messages, &mut last_tier_seen));
    assert_eq!(handle.pressure(), PressureTier::Stop100);
    assert_eq!(
        advisory_count(&messages, WARNING_ADVISORY),
        1,
        "WARNING count unchanged after reaching Stop100 — Stop100 has no advisory text"
    );

    // Turn 11 → used=10 (capped) → consume() returns None → clean-stop.
    let proceeded = simulate_turn(&handle, &mut messages, &mut last_tier_seen);
    assert!(
        !proceeded,
        "Stop100 consume() must return None — agent loop clean-stops to AgentResult::budget_exhausted (G-01)"
    );

    // Final tallies (exact-once per tier crossing, no spam, clean stop).
    assert_eq!(
        advisory_count(&messages, CAUTION_ADVISORY),
        1,
        "final: CAUTION_ADVISORY injected exactly once"
    );
    assert_eq!(
        advisory_count(&messages, WARNING_ADVISORY),
        1,
        "final: WARNING_ADVISORY injected exactly once"
    );
}

/// Helper — count occurrences of an advisory text across injected system messages.
fn advisory_count(messages: &[ChatMessage], advisory: &str) -> usize {
    messages
        .iter()
        .filter_map(|m| m.content_text().map(|t| t.to_string()))
        .filter(|t| t.contains(advisory))
        .count()
}
