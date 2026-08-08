//! Hard pre-spawn dispatch gate — CLI-side sweep (Phase 47.4 Plan 10, GAP-1).
//!
//! The predicate itself now lives in
//! [`ironhermes_core::dispatch_gate`], and the authoritative enforcement point
//! is inside `ironhermes_kanban`'s dispatcher loop
//! (`claim_and_spawn`), so that EVERY dispatch path is gated — including the
//! gateway-hosted `run_dispatch_loop`, which is what actually spawns workers
//! in production and which this module alone never covered.
//!
//! See the module docs in `ironhermes-core/src/dispatch_gate.rs` for the full
//! rationale and the caller table.
//!
//! What remains here is the CLI's *pre-tick sweep*: `cmd_dispatch` blocks
//! undispatchable ready tasks up front so a one-shot
//! `ironhermes kanban dispatch` reports them in its stderr/`--json` output
//! rather than silently skipping them. The dispatcher's own per-task gate is
//! the backstop that catches everything this sweep cannot see (tasks that
//! become ready mid-tick, and every dispatch that does not go through the
//! CLI at all).
//!
//! Re-exported `pub` from `crates/ironhermes-cli/src/kanban/mod.rs`.

use std::collections::HashMap;

use ironhermes_kanban::KanbanStore;
use ironhermes_kanban::store::ListFilters;

// Re-exported so existing call sites (`cmd_dispatch` and the Plan 10
// regression suite) continue to name one predicate. These are the SAME items
// — not a parallel definition; the gate must never have two implementations
// that can disagree.
//
// `allow(unused_imports)`: `src/main.rs` re-declares `mod kanban;`, so this
// module is compiled twice — once into the lib (where the integration test
// `tests/dispatch_profile_gate.rs` consumes `evaluate_profile_dispatch_at`)
// and once into the bin (which only calls the sweep below). The bin's copy
// sees the seat re-export as unused. This suppresses that duplication
// artifact, not real dead code.
#[allow(unused_imports)]
pub use ironhermes_core::dispatch_gate::{
    DISPATCH_GATE_REASON_PREFIX, DispatchDecision, evaluate_profile_dispatch,
    evaluate_profile_dispatch_at,
};

/// Sweep every `status='ready'` task in `store`, evaluate its assignee's
/// dispatchability, and block every task whose profile is refused — BEFORE
/// the dispatcher ever attempts to spawn it. Returns the `(task_id, reason)`
/// pairs it blocked (the raw, unprefixed reason — callers own how they
/// present it; `cmd_dispatch` prefixes its own stderr line with
/// `"dispatch gate: "` verbatim). Memoizes per-assignee so N tasks for the
/// same profile do exactly one disk+resolver pass.
///
/// Naturally idempotent: it only ever selects `status='ready'` tasks, so a
/// second pass over an already-blocked task selects nothing and appends no
/// second event.
pub fn refuse_undispatchable_ready_tasks(store: &mut KanbanStore) -> Vec<(String, String)> {
    let mut cache: HashMap<String, DispatchDecision> = HashMap::new();
    let mut blocked = Vec::new();

    let tasks = match store.list_tasks(ListFilters {
        status: Some("ready".to_string()),
        ..Default::default()
    }) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("[kanban] dispatch gate could not list ready tasks: {e}");
            return blocked;
        }
    };

    for task in tasks {
        let decision = cache
            .entry(task.assignee.clone())
            .or_insert_with(|| evaluate_profile_dispatch(&task.assignee))
            .clone();

        if let DispatchDecision::Refuse { reason } = decision {
            let gate_reason = format!("{DISPATCH_GATE_REASON_PREFIX}{reason}");
            if let Err(e) = store.block_task(&task.id, &gate_reason, None) {
                tracing::warn!("[kanban] dispatch gate could not block task {}: {e}", task.id);
                continue;
            }
            blocked.push((task.id.clone(), reason));
        }
    }

    blocked
}
