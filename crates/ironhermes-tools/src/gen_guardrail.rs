//! Phase 47 Plan 05 — `GenerationGuardrail`: the single, shared spend-guardrail
//! primitive enforced before every provider call on every generation surface
//! (chat, kanban regular + goal-mode, delegate children) — GEN-05 / D-04 /
//! D-05 / D-06 / D-08 / D-09.
//!
//! # D-08 reconciliation (Root vs Descendant)
//!
//! A [`ReservationKind::Root`] reservation is a top-level/direct-chat-session
//! generation. It is governed **exclusively** by the existing, co-located
//! per-section cap (`image_gen.session_cap` / `video_gen.session_cap` via
//! `SessionCounter`) — it is **never** subject to `per_child_cap` and **never**
//! draws the descendant `session_pool`. This preserves today's chat behavior
//! unchanged.
//!
//! A [`ReservationKind::Descendant`] reservation is a `delegate_task` child or
//! a kanban worker task. It is bounded by **two** tiers, both checked (and
//! consumed) atomically before any provider call:
//! 1. `per_child_cap` — bounds any single descendant from draining the pool
//!    alone. Always enforced in-process (no cross-process seam needed: a
//!    single descendant lives in one process).
//! 2. `session_pool` — the shared aggregate across ALL descendants of a root.
//!    For in-process delegate children this is a locked in-process counter;
//!    for cross-process kanban swarms it is delegated to an injected
//!    [`GenerationLedger`] (fulfilled by Plan 08's adapter over Plan 03's
//!    `KanbanStore::try_reserve_generation_slot`).
//!
//! # Why `try_reserve` (not `record_success`) performs the atomic consumption
//!
//! The GEN-05 concurrency requirement is that concurrent delegate siblings
//! racing against `session_pool = 1` must yield **exactly one** winner. This
//! is only achievable if the capacity check AND the consumption happen inside
//! ONE atomic critical section. If consumption were deferred to a separate
//! `record_success` call made after the provider call completes, two sibling
//! threads could both pass the `try_reserve` check (since neither has
//! consumed yet) before either called `record_success`, together exceeding
//! the cap — exactly the failure mode this guardrail exists to prevent.
//!
//! `try_reserve` therefore performs the full check-and-consume atomically
//! (mirroring `KanbanStore::try_reserve_generation_slot`, which commits the
//! ledger row within the same transaction as the check). A blocked
//! reservation consumes neither tier (rolled back if the per-child tier
//! succeeded but the pool tier subsequently blocked). [`record_success`]
//! remains a required companion call — it does not re-touch the tiers (that
//! would double-count) but records the `model`/`mode` of the completed
//! generation for the D-04 $-ready log (count-based this phase, no dollar
//! figure).
//!
//! # Fail-closed
//!
//! An error from an injected [`GenerationLedger`] (e.g. a `kanban.db` I/O
//! failure) is treated as a blocked pool reservation — never a silent grant.
//! Ambiguity always resolves to "deny" (T-47-04 mitigation).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tracing::warn;

// ---------------------------------------------------------------------------
// ReservationKind
// ---------------------------------------------------------------------------

/// Distinguishes a top-level/direct-chat-session reservation (`Root`, D-08
/// exempt from the two-tier descendant accounting) from a delegate child /
/// kanban worker reservation (`Descendant`, subject to both tiers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationKind {
    /// Top-level/direct chat session. Governed exclusively by the co-located
    /// per-section `session_cap` (`SessionCounter`) — never `per_child_cap`,
    /// never the descendant `session_pool`.
    Root,
    /// A delegate child or kanban worker task. `child_id` scopes the
    /// per-child sub-cap — typically the delegate child's task id or the
    /// kanban worker's own task id.
    Descendant { child_id: String },
}

// ---------------------------------------------------------------------------
// GenerationLedger — cross-process shared-pool seam
// ---------------------------------------------------------------------------

/// Cross-process shared-pool accounting seam (D-08 cross-process constraint).
///
/// `ironhermes-tools` cannot depend on `ironhermes-kanban` (which itself
/// depends on `ironhermes-tools` — a cycle), so the cross-process kanban
/// swarm's shared-pool arm is abstracted behind this trait. Plan 08 supplies
/// the real implementation, adapting `KanbanStore::try_reserve_generation_slot`
/// (Plan 03) into this signature. Tests in this module use a hand-written
/// fake to prove the delegation path without any kanban dependency.
pub trait GenerationLedger: Send + Sync {
    /// Attempt to reserve one shared-pool slot for `root_id` (the swarm root
    /// task id, or the worker's own task id for a solitary non-swarm task).
    ///
    /// Returns `Ok(true)` when the reservation is granted (the ledger has
    /// already committed the increment — mirrors
    /// `KanbanStore::try_reserve_generation_slot`'s `Granted` outcome
    /// committing within the same transaction as the check). Returns
    /// `Ok(false)` when the pool is exhausted (mirrors `PoolExhausted`) — no
    /// mutation occurs on a blocked attempt. `model`/`mode` are recorded by
    /// the ledger implementation for the D-04 $-ready log.
    fn try_reserve_pool(
        &self,
        root_id: &str,
        pool: u32,
        model: &str,
        mode: &str,
    ) -> anyhow::Result<bool>;
}

// ---------------------------------------------------------------------------
// GuardrailBlock — the two non-retried breach messages (D-06)
// ---------------------------------------------------------------------------

/// A hard-block outcome from [`GenerationGuardrail::try_reserve`].
///
/// Both variants render to an exact, non-retried breach message (D-06)
/// matching the UI-SPEC Copywriting Contract strings verbatim — see
/// [`GuardrailBlock::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardrailBlock {
    /// The shared `session_pool` aggregate is exhausted (`count >= pool`).
    PoolExhausted { pool: u32 },
    /// The per-child `per_child_cap` sub-limit is exhausted for this child.
    PerChildExhausted { cap: u32 },
}

impl GuardrailBlock {
    /// Render the exact, non-retried UI-SPEC Copywriting Contract string for
    /// this breach (D-06). Neither string contains "retry"/"try again" —
    /// locked by the `guardrail_message_format_tests` module below.
    pub fn message(&self) -> String {
        match self {
            GuardrailBlock::PoolExhausted { pool } => format!(
                "Generation pool exhausted ({pool} generations for this session across all \
                 subagents/tasks). This is a hard limit to prevent runaway spend; no further \
                 images or videos will be generated until a new session starts."
            ),
            GuardrailBlock::PerChildExhausted { cap } => format!(
                "Per-child generation limit reached ({cap} generations for this task). This is \
                 a hard limit to prevent runaway spend; no further images or videos will be \
                 generated by this child."
            ),
        }
    }
}

impl std::fmt::Display for GuardrailBlock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

impl std::error::Error for GuardrailBlock {}

// ---------------------------------------------------------------------------
// GenerationRecord — D-04 $-ready telemetry (count-based this phase)
// ---------------------------------------------------------------------------

/// One successful-reservation record (D-04): `model` + `mode` only, no dollar
/// figure. `kind` is `"root"` or `"descendant:{child_id}"` for observability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationRecord {
    pub kind: String,
    pub mode: String,
    pub model: String,
}

// ---------------------------------------------------------------------------
// GenerationGuardrail
// ---------------------------------------------------------------------------

/// The single, shared spend-guardrail primitive (GEN-05).
///
/// Constructed once per root scope (a chat session's delegate tree, or a
/// kanban swarm's root task) from the `generation.guardrails.session_pool` /
/// `per_child_cap` config values (Plan 01), and shared via `Arc` across every
/// generation tool instance on that surface — mirroring the existing
/// `Option<SessionCounter>` / `Option<Arc<dyn AckSink>>` injection idiom.
pub struct GenerationGuardrail {
    session_pool: u32,
    per_child_cap: u32,
    /// In-process shared-pool counter, used when `ledger` is `None` (chat
    /// delegate default — in-process children share the parent's `Arc`).
    pool_used: Arc<Mutex<u32>>,
    /// Per-child sub-counters, keyed by `child_id`. Always in-process
    /// regardless of `ledger` presence — a single descendant lives in one
    /// process, so no cross-process seam is needed for this tier.
    per_child_used: Arc<Mutex<HashMap<String, u32>>>,
    /// Cross-process shared-pool seam (D-08). `None` = chat/delegate default
    /// (in-process `pool_used` counter); `Some` = cross-process kanban arm.
    ledger: Option<Arc<dyn GenerationLedger>>,
    /// The swarm root task id (or the worker's own task id for a solitary
    /// non-swarm task) — passed to the ledger as `root_id`.
    root_id: String,
    /// D-04 $-ready telemetry log: one entry per successful reservation.
    records: Arc<Mutex<Vec<GenerationRecord>>>,
}

impl GenerationGuardrail {
    /// Construct a guardrail with an in-process shared-pool counter (no
    /// cross-process ledger). `root_id` scopes the ledger key if
    /// [`GenerationGuardrail::with_ledger`] is later attached.
    pub fn new(session_pool: u32, per_child_cap: u32, root_id: impl Into<String>) -> Self {
        Self {
            session_pool,
            per_child_cap,
            pool_used: Arc::new(Mutex::new(0)),
            per_child_used: Arc::new(Mutex::new(HashMap::new())),
            ledger: None,
            root_id: root_id.into(),
            records: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Attach a cross-process [`GenerationLedger`] — the shared-pool arm then
    /// delegates to `ledger.try_reserve_pool` instead of the in-process
    /// counter. The per-child tier is unaffected (always in-process).
    #[must_use]
    pub fn with_ledger(mut self, ledger: Arc<dyn GenerationLedger>) -> Self {
        self.ledger = Some(ledger);
        self
    }

    /// Enforce the guardrail BEFORE any provider call (GEN-05 ordering).
    ///
    /// A [`ReservationKind::Root`] reservation always succeeds — it is exempt
    /// from both tiers per D-08 (the co-located per-section `session_cap`
    /// bounds it instead, elsewhere).
    ///
    /// A [`ReservationKind::Descendant`] reservation checks-and-consumes,
    /// atomically:
    /// 1. The per-child cap (always in-process). Blocked here consumes
    ///    nothing.
    /// 2. The shared pool (ledger if injected, else in-process). Blocked
    ///    here rolls back the per-child consumption from step 1, so a
    ///    pool-block never leaves a partial per-child debit behind.
    ///
    /// A ledger error is treated as blocked (fail-closed) — never a silent
    /// grant.
    pub fn try_reserve(
        &self,
        kind: &ReservationKind,
        mode: &str,
        model: &str,
    ) -> Result<(), GuardrailBlock> {
        let child_id = match kind {
            // D-08: Root is exempt from both tiers — the direct-chat cap
            // stays the co-located per-section SessionCounter.
            ReservationKind::Root => return Ok(()),
            ReservationKind::Descendant { child_id } => child_id,
        };

        // Tier 1: per-child cap (always in-process).
        {
            let mut per_child = self
                .per_child_used
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let used = per_child.entry(child_id.clone()).or_insert(0);
            if *used >= self.per_child_cap {
                return Err(GuardrailBlock::PerChildExhausted {
                    cap: self.per_child_cap,
                });
            }
            *used += 1;
        }

        // Tier 2: shared pool (ledger if injected, else in-process counter).
        let pool_granted = match &self.ledger {
            Some(ledger) => {
                match ledger.try_reserve_pool(&self.root_id, self.session_pool, model, mode) {
                    Ok(granted) => granted,
                    Err(e) => {
                        // Fail-closed: an I/O/db error from the ledger is NEVER a
                        // silent grant — treat it as pool-exhausted.
                        warn!(error = %e, root_id = %self.root_id, "generation ledger error — failing closed (blocked)");
                        false
                    }
                }
            }
            None => {
                let mut pool_used = self.pool_used.lock().unwrap_or_else(|e| e.into_inner());
                if *pool_used >= self.session_pool {
                    false
                } else {
                    *pool_used += 1;
                    true
                }
            }
        };

        if !pool_granted {
            // Roll back the per-child debit taken above — a blocked
            // reservation must consume nothing from EITHER tier.
            let mut per_child = self
                .per_child_used
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(used) = per_child.get_mut(child_id) {
                *used = used.saturating_sub(1);
            }
            return Err(GuardrailBlock::PoolExhausted {
                pool: self.session_pool,
            });
        }

        Ok(())
    }

    /// Record the `model`/`mode` of a completed generation (D-04 $-ready
    /// telemetry — count-based this phase, no dollar figure).
    ///
    /// Does NOT re-touch the pool/per-child tiers — those were already
    /// consumed atomically by the preceding [`GenerationGuardrail::try_reserve`]
    /// call (see the module-level doc for why consumption must happen there,
    /// not here, to satisfy the GEN-05 concurrency guarantee). Call only
    /// after a granted `try_reserve` — never after a block.
    pub fn record_success(&self, kind: &ReservationKind, mode: &str, model: &str) {
        let kind_label = match kind {
            ReservationKind::Root => "root".to_string(),
            ReservationKind::Descendant { child_id } => format!("descendant:{child_id}"),
        };
        self.records
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(GenerationRecord {
                kind: kind_label,
                mode: mode.to_string(),
                model: model.to_string(),
            });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------
//
// Split into modules whose names are substrings the plan's verify commands
// filter on (`gen_guardrail::pool`, `gen_guardrail::per_child`,
// `gen_guardrail::ledger`, `gen_guardrail::guardrail_message_format`).

#[cfg(test)]
fn test_guardrail(session_pool: u32, per_child_cap: u32) -> GenerationGuardrail {
    GenerationGuardrail::new(session_pool, per_child_cap, "test-root")
}

#[cfg(test)]
fn descendant(id: &str) -> ReservationKind {
    ReservationKind::Descendant {
        child_id: id.to_string(),
    }
}

#[cfg(test)]
mod pool_tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    /// Boundary: two children under pool=3 collectively reserve 3, then the
    /// pool blocks the 4th regardless of which child asks.
    #[test]
    fn two_children_collectively_exhaust_pool_regardless_of_which_child_asks() {
        let g = test_guardrail(3, 10);
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        assert!(g.try_reserve(&descendant("b"), "image", "m").is_ok());
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        let err = g
            .try_reserve(&descendant("b"), "image", "m")
            .expect_err("pool must be exhausted at 3/3");
        assert_eq!(err, GuardrailBlock::PoolExhausted { pool: 3 });
    }

    /// Boundary: session_pool = 0 blocks the very first reservation.
    #[test]
    fn zero_pool_blocks_first_reservation() {
        let g = test_guardrail(0, 10);
        let err = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("pool=0 must block immediately");
        assert_eq!(err, GuardrailBlock::PoolExhausted { pool: 0 });
    }

    /// GEN-05 concurrency: N threads reserving as Descendant against pool=1
    /// (distinct child ids so the per-child tier never interferes) — exactly
    /// one thread wins.
    #[test]
    fn concurrent_descendants_at_pool_one_exactly_one_wins() {
        let g = Arc::new(test_guardrail(1, 1000));
        let n = 16;
        let barrier = Arc::new(Barrier::new(n));
        let handles: Vec<_> = (0..n)
            .map(|i| {
                let g = Arc::clone(&g);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    g.try_reserve(&descendant(&format!("child-{i}")), "image", "m")
                        .is_ok()
                })
            })
            .collect();
        let successes = handles
            .into_iter()
            .map(|h| h.join().expect("thread must not panic"))
            .filter(|ok| *ok)
            .count();
        assert_eq!(successes, 1, "exactly one thread must win the pool=1 race");
    }

    /// A blocked (pool-exhausted) reservation increments neither the pool nor
    /// the per-child counter — nothing is consumed on a block.
    #[test]
    fn blocked_reservation_consumes_neither_tier() {
        let g = test_guardrail(0, 5);
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_err());
        assert_eq!(*g.pool_used.lock().unwrap(), 0);
        assert_eq!(
            *g.per_child_used.lock().unwrap().get("a").unwrap_or(&0),
            0,
            "per-child tier must be rolled back after a pool block"
        );
    }

    /// Idempotency: re-calling after a pool breach stays blocked with the
    /// exact same message — a breach never resets.
    #[test]
    fn repeated_calls_after_pool_breach_stay_blocked_with_the_same_message() {
        let g = test_guardrail(0, 5);
        let first = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("must block");
        let second = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("must stay blocked");
        assert_eq!(first.message(), second.message());
    }

    /// A granted Descendant reservation followed by `record_success` consumes
    /// the pool exactly once (no double count from `record_success`).
    #[test]
    fn descendant_success_consumes_pool_exactly_once() {
        let g = test_guardrail(5, 5);
        g.try_reserve(&descendant("a"), "image", "flux")
            .expect("should be granted");
        g.record_success(&descendant("a"), "image", "flux");
        assert_eq!(*g.pool_used.lock().unwrap(), 1);
    }
}

#[cfg(test)]
mod per_child_tests {
    use super::*;

    /// Adjacency: a single child reserves up to per_child_cap=3 (ok) then
    /// blocks on its 4th, even though the pool has room left.
    #[test]
    fn single_child_blocks_at_its_cap_even_with_pool_room_remaining() {
        let g = test_guardrail(5, 3);
        for _ in 0..3 {
            assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        }
        let err = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("4th must block at per_child_cap=3");
        assert_eq!(err, GuardrailBlock::PerChildExhausted { cap: 3 });
        // Pool has plenty of room left — this child's own cap is the
        // constraint, not the pool.
        assert_eq!(*g.pool_used.lock().unwrap(), 3);
    }

    /// Boundary: per_child_cap = 0 blocks a child's first reservation.
    #[test]
    fn zero_per_child_cap_blocks_first_reservation() {
        let g = test_guardrail(10, 0);
        let err = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("per_child_cap=0 must block immediately");
        assert_eq!(err, GuardrailBlock::PerChildExhausted { cap: 0 });
    }

    /// Adjacency: a per-child cap exhaustion blocks only that child — a
    /// sibling under the same pool is unaffected, and the exhausted child's
    /// blocked attempts never drain the shared pool.
    #[test]
    fn per_child_exhaustion_blocks_only_that_child_not_siblings() {
        let g = test_guardrail(5, 1);
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        assert!(
            g.try_reserve(&descendant("a"), "image", "m").is_err(),
            "child a is now at its own cap"
        );
        assert!(
            g.try_reserve(&descendant("b"), "image", "m").is_ok(),
            "sibling b must be unaffected by a's exhaustion"
        );
        assert_eq!(
            *g.pool_used.lock().unwrap(),
            2,
            "only the two GRANTED reservations (a's first, b's) drew the pool"
        );
    }

    /// D-08: a Root reservation is exempt from `per_child_cap` — it succeeds
    /// MORE times than the cap, and never draws the descendant `session_pool`.
    #[test]
    fn root_reservation_exceeds_per_child_cap_without_being_blocked() {
        let g = test_guardrail(100, 3);
        for _ in 0..5 {
            assert!(
                g.try_reserve(&ReservationKind::Root, "image", "m").is_ok(),
                "Root must never be blocked by per_child_cap"
            );
        }
        assert_eq!(
            *g.pool_used.lock().unwrap(),
            0,
            "Root must never draw the descendant session_pool"
        );
        assert!(
            g.per_child_used.lock().unwrap().is_empty(),
            "Root must never touch the per-child tier"
        );
    }

    /// A Root `record_success` records model+mode but decrements neither
    /// tier (there is nothing to decrement — Root never drew either).
    #[test]
    fn root_record_success_touches_neither_tier() {
        let g = test_guardrail(1, 1);
        g.try_reserve(&ReservationKind::Root, "image", "m")
            .expect("Root always succeeds");
        g.record_success(&ReservationKind::Root, "image", "m");
        assert_eq!(*g.pool_used.lock().unwrap(), 0);
        assert!(g.per_child_used.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Hand-written fake `GenerationLedger` — proves the delegation path
    /// without any dependency on `ironhermes-kanban`.
    struct FakeLedger {
        remaining: StdMutex<u32>,
        calls: StdMutex<Vec<(String, u32, String, String)>>,
    }

    impl FakeLedger {
        fn with_remaining(remaining: u32) -> Self {
            Self {
                remaining: StdMutex::new(remaining),
                calls: StdMutex::new(Vec::new()),
            }
        }
    }

    impl GenerationLedger for FakeLedger {
        fn try_reserve_pool(
            &self,
            root_id: &str,
            pool: u32,
            model: &str,
            mode: &str,
        ) -> anyhow::Result<bool> {
            self.calls.lock().unwrap().push((
                root_id.to_string(),
                pool,
                model.to_string(),
                mode.to_string(),
            ));
            let mut remaining = self.remaining.lock().unwrap();
            if *remaining == 0 {
                Ok(false)
            } else {
                *remaining -= 1;
                Ok(true)
            }
        }
    }

    /// With an injected ledger, the shared-pool arm consults the ledger (NOT
    /// the in-process counter) and blocks at the ledger's pool, while the
    /// per-child cap is still enforced in-process.
    #[test]
    fn injected_ledger_is_consulted_for_the_pool_arm_not_the_in_process_counter() {
        let ledger = Arc::new(FakeLedger::with_remaining(1));
        let g = test_guardrail(100, 10).with_ledger(ledger.clone());

        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        let err = g
            .try_reserve(&descendant("b"), "image", "m")
            .expect_err("ledger's pool is exhausted");
        assert_eq!(err, GuardrailBlock::PoolExhausted { pool: 100 });

        // The in-process pool counter must never be touched when a ledger is
        // injected — the ledger is the sole source of truth for the pool tier.
        assert_eq!(*g.pool_used.lock().unwrap(), 0);
        assert_eq!(ledger.calls.lock().unwrap().len(), 2);
    }

    /// With no ledger injected (`None`), the shared-pool arm falls back to
    /// the in-process counter (chat/delegate default).
    #[test]
    fn no_ledger_falls_back_to_the_in_process_counter() {
        let g = test_guardrail(1, 10);
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        assert_eq!(*g.pool_used.lock().unwrap(), 1);
        let err = g
            .try_reserve(&descendant("b"), "image", "m")
            .expect_err("in-process pool must be exhausted");
        assert_eq!(err, GuardrailBlock::PoolExhausted { pool: 1 });
    }

    /// The per-child cap is enforced in-process regardless of ledger
    /// presence — a ledger with plenty of pool room does not bypass it.
    #[test]
    fn per_child_cap_is_enforced_in_process_even_with_a_ledger_injected() {
        let ledger = Arc::new(FakeLedger::with_remaining(100));
        let g = test_guardrail(100, 1).with_ledger(ledger);
        assert!(g.try_reserve(&descendant("a"), "image", "m").is_ok());
        let err = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("per_child_cap=1 must still block a's 2nd request");
        assert_eq!(err, GuardrailBlock::PerChildExhausted { cap: 1 });
    }

    /// Fail-closed: a ledger error is treated as a blocked pool reservation,
    /// never a silent grant.
    #[test]
    fn ledger_error_fails_closed_as_pool_exhausted() {
        struct ErroringLedger;
        impl GenerationLedger for ErroringLedger {
            fn try_reserve_pool(
                &self,
                _root_id: &str,
                _pool: u32,
                _model: &str,
                _mode: &str,
            ) -> anyhow::Result<bool> {
                anyhow::bail!("simulated kanban.db I/O failure")
            }
        }
        let g = test_guardrail(100, 10).with_ledger(Arc::new(ErroringLedger));
        let err = g
            .try_reserve(&descendant("a"), "image", "m")
            .expect_err("a ledger error must fail closed (blocked), never grant");
        assert_eq!(err, GuardrailBlock::PoolExhausted { pool: 100 });
    }
}

#[cfg(test)]
mod guardrail_message_format_tests {
    use super::*;

    /// Ban list mirrors the shipped `image_gen.rs`/`video_gen.rs` precedent —
    /// neither breach message may contain re-request wording so the agent
    /// cannot loop on it (D-06).
    const BANNED_SUBSTRINGS: &[&str] = &["retry", "try again"];

    #[test]
    fn pool_exhausted_message_names_pool_scope_and_hard_limit() {
        let msg = GuardrailBlock::PoolExhausted { pool: 20 }.message();
        assert!(msg.contains("20"), "must name the pool count: {msg}");
        assert!(
            msg.contains("session") && msg.contains("subagent"),
            "must name the session/subagent scope: {msg}"
        );
        assert!(
            msg.to_lowercase().contains("hard limit"),
            "must state the hard-limit consequence: {msg}"
        );
        let lower = msg.to_lowercase();
        for banned in BANNED_SUBSTRINGS {
            assert!(
                !lower.contains(banned),
                "message must not contain '{banned}': {msg}"
            );
        }
    }

    #[test]
    fn per_child_exhausted_message_names_cap_task_scope_and_hard_limit() {
        let msg = GuardrailBlock::PerChildExhausted { cap: 3 }.message();
        assert!(msg.contains('3'), "must name the per-child cap: {msg}");
        assert!(msg.contains("task"), "must name the task scope: {msg}");
        assert!(
            msg.to_lowercase().contains("hard limit"),
            "must state the hard-limit consequence: {msg}"
        );
        let lower = msg.to_lowercase();
        for banned in BANNED_SUBSTRINGS {
            assert!(
                !lower.contains(banned),
                "message must not contain '{banned}': {msg}"
            );
        }
    }

    #[test]
    fn display_impl_matches_message() {
        let block = GuardrailBlock::PoolExhausted { pool: 5 };
        assert_eq!(block.to_string(), block.message());
    }
}
