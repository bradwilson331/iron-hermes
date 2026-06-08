//! Phase 36.3.7.5 BUG-36.3.7.5-04 — pure-function spawn-gating helper.
//!
//! The kanban notifier loop (Plan 02, `ironhermes_kanban::run_notifier_loop`) is
//! spawned by the gateway runner (Plan 03 — `runner.rs`). Whether the spawn happens
//! at all is decided by `compute_notifier_gate`:
//!
//! 1. `notification_sources` must be `Some(non_empty)` in `KanbanConfig`. The
//!    locked CONTEXT.md default-off semantics require an explicit opt-in.
//! 2. At least one platform name in `notification_sources` must intersect with
//!    `enabled_platforms` (case-insensitive).
//!
//! Returns `NotifierGate::Enabled { sources }` carrying ONLY the overlapping
//! platform names — the gateway uses this list to log + build the `send_fn`
//! adapter snapshot.
//!
//! # Visibility
//!
//! Both `NotifierGate` and `compute_notifier_gate` are `pub` so the
//! integration test in `crates/ironhermes-gateway/tests/notifier_spawn_gating.rs`
//! can reach them. They are documented as "Phase 36.3.7.5 — `pub` for
//! receiver-end tests only" — internal callers should not depend on them as a
//! stable API. The pure-function shape means there is no runtime risk to the
//! exposure (no DB access, no IO, no state).

/// Outcome of the notifier-spawn gate check.
///
/// `DisabledNoSources` — `notification_sources` is `None` or empty.
/// `DisabledNoOverlap` — sources configured but no enabled-platform overlap.
/// `Enabled { sources }` — at least one platform in `notification_sources`
/// intersects with the enabled set; `sources` carries the overlap (subset of
/// `notification_sources` in the ORIGINAL case as configured).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotifierGate {
    DisabledNoSources,
    DisabledNoOverlap {
        wanted: Vec<String>,
        enabled: Vec<String>,
    },
    Enabled {
        sources: Vec<String>,
    },
}

/// Pure-function gate: returns `Enabled` iff `notification_sources` is
/// `Some(non_empty)` AND at least one platform in that list intersects with
/// `enabled_platforms` (case-insensitive).
///
/// Default-off semantics (locked CONTEXT.md decision D-gateway-gating) are
/// preserved: `None` and `Some(empty)` BOTH collapse to `DisabledNoSources`.
///
/// The returned `Enabled.sources` carries the overlap in the ORIGINAL casing
/// from `notification_sources` — the gateway uses this to log what the
/// operator wrote, not what the adapters internally name themselves.
pub fn compute_notifier_gate(
    notification_sources: Option<&[String]>,
    enabled_platforms: &[String],
) -> NotifierGate {
    let sources = match notification_sources {
        Some(s) if !s.is_empty() => s,
        _ => return NotifierGate::DisabledNoSources,
    };
    let overlap: Vec<String> = sources
        .iter()
        .filter(|s| enabled_platforms.iter().any(|e| e.eq_ignore_ascii_case(s)))
        .cloned()
        .collect();
    if overlap.is_empty() {
        NotifierGate::DisabledNoOverlap {
            wanted: sources.to_vec(),
            enabled: enabled_platforms.to_vec(),
        }
    } else {
        NotifierGate::Enabled { sources: overlap }
    }
}
