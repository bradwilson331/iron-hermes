//! Phase 36.3.7.11 Plan 02 — Drag-allowed transition validator (D-10) +
//! drag-blocked hint copy table (D-11).
//!
//! Client-side copy of D-10 allowed transitions. Server-side canonical:
//! `crate::server::kanban_api::patch_task_status` (which imports this
//! `is_drag_allowed` fn directly — D-14 defense-in-depth reuses the same
//! function, so the only drift surface is the wire `KanbanStatus` enum
//! versus `ironhermes_kanban::KanbanStatus`. KEEP IN SYNC.
//!
//! Q10 (RESEARCH.md): the validator is a pure-Rust `matches!` table — no
//! network, no DB, no state. Safe to call from the WASM client AND from
//! `#[server]` fn bodies; both compile against this module.
//!
//! D-10 allowed pairs (anything else → false):
//! - (Triage, Todo)
//! - (Todo, Ready)
//! - (Blocked, Ready)
//! - (_, Archived)  — any non-archived status archives, modal-confirmed.
//!
//! D-11 hint copy (verbatim UI-SPEC §7.4):
//! - Done    → "Completing a task needs a summary — open the card …"
//! - Blocked → "Blocking a task needs a reason — open the card …"
//! - Running → "The dispatcher owns this transition — cannot assign …"
//! - Others  → None (caller may fall back to a generic cross-skip hint).

use crate::protocol::KanbanStatus;

/// D-10: returns `true` only for the four allowed drag transitions.
/// Same-status pairs return `false` (no-op moves are not drags).
pub fn is_drag_allowed(from: KanbanStatus, to: KanbanStatus) -> bool {
    // Same-status guard: a no-op drag should never trigger a write.
    if from == to {
        return false;
    }
    // Archiving from a non-archived source is always allowed (with
    // confirm modal in the UI). Treat (Archived, Archived) as a no-op
    // — already caught by the same-status guard above.
    if to == KanbanStatus::Archived {
        return true;
    }
    // The three explicit promotion arms.
    matches!(
        (from, to),
        (KanbanStatus::Triage, KanbanStatus::Todo)
            | (KanbanStatus::Todo, KanbanStatus::Ready)
            | (KanbanStatus::Blocked, KanbanStatus::Ready)
    )
}

/// D-11 / UI-SPEC §7.4: hint copy for disallowed drag targets that need a
/// structured action button instead. Verbatim UI-SPEC strings — DO NOT
/// paraphrase. Returns `None` for targets that have no per-column hint
/// (callers fall back to "cross-skip" / "backwards-move" copy from the
/// general hint table).
#[allow(dead_code)] // Task 3 consumer wires this into KanbanColumn's disallowed-target overlay.
pub fn drag_blocked_hint(to: KanbanStatus) -> Option<&'static str> {
    match to {
        KanbanStatus::Done => {
            Some("Completing a task needs a summary — open the card and click Complete…")
        }
        KanbanStatus::Blocked => {
            Some("Blocking a task needs a reason — open the card and click Block…")
        }
        // The canonical "running" status — wire-encoded as `InProgress` on
        // the client (the wire KanbanStatus serializes to "running"; see
        // protocol.rs).
        KanbanStatus::InProgress => {
            Some("The dispatcher owns this transition — cannot assign running status from UI")
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Inline behavior tests (run via `cargo test -p iron_hermes_ui --bin
// iron_hermes_ui -- kanban::transitions`). The `tests/` integration file
// does source-string asserts only because the crate is a `[[bin]]` and
// cannot be imported externally.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::KanbanStatus;

    const ALL: &[KanbanStatus] = &[
        KanbanStatus::Triage,
        KanbanStatus::Todo,
        KanbanStatus::Ready,
        KanbanStatus::InProgress,
        KanbanStatus::Blocked,
        KanbanStatus::Done,
        KanbanStatus::Archived,
    ];

    fn expected_allowed(from: KanbanStatus, to: KanbanStatus) -> bool {
        if from == to {
            return false;
        }
        if to == KanbanStatus::Archived {
            return true;
        }
        matches!(
            (from, to),
            (KanbanStatus::Triage, KanbanStatus::Todo)
                | (KanbanStatus::Todo, KanbanStatus::Ready)
                | (KanbanStatus::Blocked, KanbanStatus::Ready)
        )
    }

    #[test]
    fn d10_allows_the_three_explicit_promotions() {
        assert!(is_drag_allowed(KanbanStatus::Triage, KanbanStatus::Todo));
        assert!(is_drag_allowed(KanbanStatus::Todo, KanbanStatus::Ready));
        assert!(is_drag_allowed(KanbanStatus::Blocked, KanbanStatus::Ready));
    }

    #[test]
    fn d10_allows_archive_from_every_non_archived_source() {
        for &from in ALL {
            if from == KanbanStatus::Archived {
                continue;
            }
            assert!(
                is_drag_allowed(from, KanbanStatus::Archived),
                "{:?} → Archived must be allowed",
                from
            );
        }
    }

    #[test]
    fn d10_rejects_same_status_no_ops() {
        for &s in ALL {
            assert!(
                !is_drag_allowed(s, s),
                "({:?},{:?}) same-status no-op must return false",
                s,
                s,
            );
        }
    }

    #[test]
    fn d10_exhaustive_7x7_grid() {
        for &from in ALL {
            for &to in ALL {
                let expected = expected_allowed(from, to);
                let actual = is_drag_allowed(from, to);
                assert_eq!(
                    actual, expected,
                    "({:?},{:?}): expected={}, actual={}",
                    from, to, expected, actual,
                );
            }
        }
    }

    #[test]
    fn d11_verbatim_hint_copy_done() {
        assert_eq!(
            drag_blocked_hint(KanbanStatus::Done),
            Some("Completing a task needs a summary — open the card and click Complete…")
        );
    }

    #[test]
    fn d11_verbatim_hint_copy_blocked() {
        assert_eq!(
            drag_blocked_hint(KanbanStatus::Blocked),
            Some("Blocking a task needs a reason — open the card and click Block…")
        );
    }

    #[test]
    fn d11_verbatim_hint_copy_in_progress() {
        assert_eq!(
            drag_blocked_hint(KanbanStatus::InProgress),
            Some("The dispatcher owns this transition — cannot assign running status from UI")
        );
    }

    #[test]
    fn d11_no_hint_for_other_targets() {
        for &s in &[
            KanbanStatus::Triage,
            KanbanStatus::Todo,
            KanbanStatus::Ready,
            KanbanStatus::Archived,
        ] {
            assert!(
                drag_blocked_hint(s).is_none(),
                "{:?} must have no hint copy",
                s,
            );
        }
    }
}
