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

    /// Wave-1 Plan 02 fills this in. Shell panics on use so accidental
    /// production callers fail loudly rather than silently under-counting.
    pub fn consume(&self) -> Option<usize> {
        unimplemented!("BudgetHandle::consume — implemented in Plan 02 (Wave 1)")
    }

    /// Wave-1 Plan 02 fills this in.
    pub fn pressure(&self) -> PressureTier {
        unimplemented!("BudgetHandle::pressure — implemented in Plan 02 (Wave 1)")
    }
}

/// Advisory string appended to the prompt when pressure hits the 70% mark.
pub const CAUTION_ADVISORY: &str = "You have used approximately 70% of your iteration budget. Consider consolidating remaining work and moving toward a final answer.";

/// Advisory string appended to the prompt when pressure hits the 90% mark.
pub const WARNING_ADVISORY: &str = "You have used approximately 90% of your iteration budget. Respond with your final answer now.";
