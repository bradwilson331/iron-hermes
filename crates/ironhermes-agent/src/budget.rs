//! Shared parent/child iteration-budget handle (PROV-10, D-10/D-15/D-16/D-17).
//!
//! Wave-0 SHELL — concrete impl lands in Wave 1 Plan 02.
//! AI-SPEC Pitfall 9 (E-05): use SeqCst ordering only on this shared counter.
//! The lax Relaxed variant is forbidden here because the pressure-tier
//! transitions must be observed in a consistent total order across threads.
//! The budget_ordering_grep static-grep test enforces this at CI time.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Shared iteration budget handle. Clones share the same underlying counter
/// via [`Arc`], so parent and child subagent loops can see each other's
/// deductions without duplicated bookkeeping.
#[derive(Clone)]
pub struct BudgetHandle {
    remaining: Arc<AtomicUsize>,
    max: usize,
}

/// Pressure tier surfaced to the agent loop so it can emit advisory messages
/// as the budget drains. Wave-1 Plan 02 fills in the `pressure()` mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureTier {
    None,
    Caution70,
    Warning90,
    Stop100,
}

impl BudgetHandle {
    /// Construct a fresh handle with `max` iterations remaining.
    pub fn new(max: usize) -> Self {
        Self {
            remaining: Arc::new(AtomicUsize::new(max)),
            max,
        }
    }

    /// Wrap an existing shared counter (allows parent/child sharing when the
    /// counter was created elsewhere).
    pub fn new_from_arc(remaining: Arc<AtomicUsize>, max: usize) -> Self {
        Self { remaining, max }
    }

    /// Borrow the inner `Arc<AtomicUsize>` so advanced callers (e.g. passing
    /// to an already-Arc-aware subagent API) can share the counter directly.
    pub fn inner(&self) -> Arc<AtomicUsize> {
        self.remaining.clone()
    }

    /// Maximum iterations this handle was constructed with.
    pub fn max(&self) -> usize {
        self.max
    }

    /// Current remaining iterations (SeqCst load).
    pub fn remaining(&self) -> usize {
        self.remaining.load(Ordering::SeqCst)
    }

    /// Iterations consumed so far.
    pub fn used(&self) -> usize {
        self.max.saturating_sub(self.remaining())
    }

    /// Decrement. Returns Some(new_remaining) on success; None when budget
    /// was already 0 at entry (D-15 Stop100 tier).
    ///
    /// SeqCst ordering (AI-SPEC Pitfall 9 / E-05): reader on `hermes status`
    /// sees the post-decrement value.
    pub fn consume(&self) -> Option<usize> {
        let prev = self.remaining.fetch_sub(1, Ordering::SeqCst);
        if prev == 0 {
            // Compensate: fetch_sub on 0 wraps to usize::MAX; restore to 0.
            self.remaining.fetch_add(1, Ordering::SeqCst);
            None
        } else {
            Some(prev - 1)
        }
    }

    /// Compute the current pressure tier (D-15).
    /// Uses integer arithmetic (floor-division) to avoid float drift.
    /// No transient tier reads during clone/fork — SeqCst load provides a
    /// globally-ordered view.
    pub fn pressure(&self) -> PressureTier {
        if self.max == 0 {
            return PressureTier::None;
        }
        let used = self.used();
        // Integer pct = used * 100 / max, floor.
        // Multiplication order avoids overflow for realistic max (< 2^56).
        let used_pct = (used * 100) / self.max;
        match used_pct {
            p if p >= 100 => PressureTier::Stop100,
            p if p >= 90 => PressureTier::Warning90,
            p if p >= 70 => PressureTier::Caution70,
            _ => PressureTier::None,
        }
    }
}

/// Helper for Plan 05 turn-boundary injection.
/// Returns the exact advisory text for a tier (None for None/Stop — Stop
/// terminates before the next provider call, so no injection needed).
pub fn advisory_text(tier: PressureTier) -> Option<&'static str> {
    match tier {
        PressureTier::Caution70 => Some(CAUTION_ADVISORY),
        PressureTier::Warning90 => Some(WARNING_ADVISORY),
        PressureTier::None | PressureTier::Stop100 => None,
    }
}

/// Advisory string appended to the prompt when pressure hits the 70% mark.
pub const CAUTION_ADVISORY: &str = "You have used approximately 70% of your iteration budget. Consider consolidating remaining work and moving toward a final answer.";

/// Advisory string appended to the prompt when pressure hits the 90% mark.
pub const WARNING_ADVISORY: &str = "You have used approximately 90% of your iteration budget. Respond with your final answer now.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consume_from_fresh_decrements_by_one() {
        let b = BudgetHandle::new(100);
        assert_eq!(b.consume(), Some(99));
        assert_eq!(b.remaining(), 99);
    }

    #[test]
    fn consume_at_zero_returns_none_and_does_not_underflow() {
        let b = BudgetHandle::new(2);
        assert_eq!(b.consume(), Some(1));
        assert_eq!(b.consume(), Some(0));
        assert_eq!(b.consume(), None);
        assert_eq!(b.remaining(), 0, "must not underflow past 0");
        // Repeated calls at 0 stay at 0.
        assert_eq!(b.consume(), None);
        assert_eq!(b.remaining(), 0);
    }

    #[test]
    fn used_reflects_decrements() {
        let b = BudgetHandle::new(10);
        b.consume();
        b.consume();
        b.consume();
        assert_eq!(b.used(), 3);
        assert_eq!(b.remaining(), 7);
    }

    #[test]
    fn clone_shares_same_counter() {
        let a = BudgetHandle::new(5);
        let b = a.clone();
        a.consume();
        a.consume();
        assert_eq!(b.remaining(), 3, "clones share the Arc<AtomicUsize>");
    }

    #[test]
    fn new_from_arc_round_trips() {
        let original = BudgetHandle::new(20);
        let raw = original.inner();
        let reconstructed = BudgetHandle::new_from_arc(raw, 20);
        original.consume();
        assert_eq!(
            reconstructed.remaining(),
            19,
            "new_from_arc shares the same Arc<AtomicUsize>"
        );
    }

    #[test]
    fn pressure_none_below_70() {
        let b = BudgetHandle::new(100);
        for _ in 0..69 {
            b.consume();
        }
        assert_eq!(b.pressure(), PressureTier::None);
        assert_eq!(b.used(), 69);
    }

    #[test]
    fn pressure_caution70_at_exactly_70() {
        let b = BudgetHandle::new(100);
        for _ in 0..70 {
            b.consume();
        }
        assert_eq!(b.pressure(), PressureTier::Caution70);
        assert_eq!(b.used(), 70);
    }

    #[test]
    fn pressure_warning90_at_exactly_90() {
        let b = BudgetHandle::new(100);
        for _ in 0..90 {
            b.consume();
        }
        assert_eq!(b.pressure(), PressureTier::Warning90);
    }

    #[test]
    fn pressure_stop100_at_exhaustion() {
        let b = BudgetHandle::new(100);
        for _ in 0..100 {
            b.consume();
        }
        assert_eq!(b.pressure(), PressureTier::Stop100);
        assert!(b.consume().is_none());
        assert_eq!(b.pressure(), PressureTier::Stop100);
    }

    #[test]
    fn pressure_handles_zero_max_without_panic() {
        let b = BudgetHandle::new(0);
        assert_eq!(b.pressure(), PressureTier::None);
        assert!(b.consume().is_none());
    }

    #[test]
    fn pressure_non_100_max_tiers_correct() {
        // max=50 — 70%=35, 90%=45, 100%=50
        let b = BudgetHandle::new(50);
        for _ in 0..34 {
            b.consume();
        }
        assert_eq!(b.pressure(), PressureTier::None);
        b.consume(); // 35
        assert_eq!(b.pressure(), PressureTier::Caution70);
        for _ in 0..10 {
            b.consume();
        } // 45
        assert_eq!(b.pressure(), PressureTier::Warning90);
        for _ in 0..5 {
            b.consume();
        } // 50
        assert_eq!(b.pressure(), PressureTier::Stop100);
    }

    #[test]
    fn advisory_text_for_each_tier() {
        assert_eq!(advisory_text(PressureTier::None), None);
        assert_eq!(
            advisory_text(PressureTier::Caution70),
            Some(CAUTION_ADVISORY)
        );
        assert_eq!(
            advisory_text(PressureTier::Warning90),
            Some(WARNING_ADVISORY)
        );
        assert_eq!(advisory_text(PressureTier::Stop100), None);
    }
}
