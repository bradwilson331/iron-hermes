//! Phase 36.3.7.11 Plan 02 (D-09 / D-06 / D-11) — Kanban column component.
//!
//! Renders one status column: header (label + count) and either the
//! filtered cards or the empty-state copy per UI-SPEC §7.3.
//!
//! Plan 02 wires the **drop zone**:
//! - `ondragover`: MUST call `event.prevent_default()` on the outer
//!   `Event<DragData>` wrapper (Risk 4 — without this `ondrop` never
//!   fires).
//! - `ondrop`: validates via `is_drag_allowed` (D-14 client copy) and
//!   delegates to `board::move_task_optimistic` for the optimistic
//!   update + spawn pattern.
//! - Visual state: `data-drag-valid="true"` / `data-drag-invalid="true"`
//!   attributes drive the CSS overlay (UI-SPEC §2.6 / §4.1). When the
//!   column is the invalid target a `.kn-drag-hint` div renders the
//!   verbatim UI-SPEC §7.4 hint copy.

use crate::components::hermes_app::screens::kanban::board::move_task_optimistic;
use crate::components::hermes_app::screens::kanban::card::KanbanCard;
use crate::kanban::transitions::{drag_blocked_hint, is_drag_allowed};
use crate::protocol::{KanbanStatus, TaskRow};
use dioxus::prelude::*;
use std::collections::HashSet;

/// Phase 36.3.7.11 Plan 02: status taxonomy for the 6 visible columns +
/// archived (toolbar toggle). Values match
/// `ironhermes_kanban::types::KanbanStatus` lowercase reference names
/// exactly (D-09).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KanbanColumnStatus {
    Triage,
    Todo,
    Ready,
    InProgress,
    Blocked,
    Done,
    Archived,
}

impl KanbanColumnStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Triage => "triage",
            Self::Todo => "todo",
            Self::Ready => "ready",
            Self::InProgress => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Archived => "archived",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Triage => "TRIAGE",
            Self::Todo => "TODO",
            Self::Ready => "READY",
            Self::InProgress => "IN PROGRESS",
            Self::Blocked => "BLOCKED",
            Self::Done => "DONE",
            Self::Archived => "ARCHIVED",
        }
    }

    /// UI-SPEC §7.3 empty-state copy table.
    pub fn empty_copy(self) -> &'static str {
        match self {
            Self::Triage => "No tasks to triage.",
            Self::Todo => "Nothing queued.",
            Self::Ready => "Awaiting dispatch.",
            Self::InProgress => "No active workers.",
            Self::Blocked => "Nothing blocked.",
            Self::Done => "Nothing completed yet.",
            Self::Archived => "Archive is empty.",
        }
    }

    /// Map to the wire `KanbanStatus` enum (Plan 02 transition table).
    pub fn to_kanban_status(self) -> KanbanStatus {
        match self {
            Self::Triage => KanbanStatus::Triage,
            Self::Todo => KanbanStatus::Todo,
            Self::Ready => KanbanStatus::Ready,
            Self::InProgress => KanbanStatus::InProgress,
            Self::Blocked => KanbanStatus::Blocked,
            Self::Done => KanbanStatus::Done,
            Self::Archived => KanbanStatus::Archived,
        }
    }
}

/// Phase 36.3.7.11 Plan 02: a single kanban column with a drop zone.
///
/// Props:
/// - `status`: which column this is (drives label + filter key + drop target).
/// - `tasks`: shared mutable signal carrying the FULL board task list.
/// - `on_open_drawer`: forwarded to KanbanCard.
/// - `dragged_task_id`: shared signal — column reads it in ondragover
///   to compute valid/invalid state, and in ondrop to identify the source.
/// - `pending_task_ids`: forwarded to the shared move helper for the
///   optimistic-pending marker.
/// - `toast_msg`, `live_region_msg`: surfaced by the shared helper on
///   error / disallowed transitions.
/// - `archive_modal_task`: Plan 03 modal target (helper sets Some(id)
///   when target == Archived).
#[component]
pub fn KanbanColumn(
    status: KanbanColumnStatus,
    tasks: Signal<Vec<TaskRow>>,
    on_open_drawer: EventHandler<String>,
    dragged_task_id: Signal<Option<String>>,
    pending_task_ids: Signal<HashSet<String>>,
    toast_msg: Signal<Option<String>>,
    live_region_msg: Signal<Option<String>>,
    archive_modal_task: Signal<Option<String>>,
) -> Element {
    let status_str = status.as_str();
    let target_status = status.to_kanban_status();
    let filtered: Vec<TaskRow> = tasks
        .read()
        .iter()
        .filter(|t| t.status == status_str)
        .cloned()
        .collect();
    let count = filtered.len();
    let aria_label = format!("{} column, {} tasks", status.label(), count);

    // D-06 / D-11: compute valid/invalid target highlight when a drag is
    // active. Reading dragged_task_id + tasks here makes the column
    // re-render when either changes; the closures used below capture
    // copies of the signals (Dioxus Signal<T> is Copy).
    let dragged = dragged_task_id.read().clone();
    let (is_valid_target, is_invalid_target, drag_source_current) = match dragged.as_deref() {
        Some(tid) => {
            // Look up the source task's current status to evaluate
            // is_drag_allowed.
            let source = tasks
                .read()
                .iter()
                .find(|t| t.id == tid)
                .map(|t| KanbanStatus::from_wire_str(&t.status).unwrap_or(KanbanStatus::Triage));
            match source {
                Some(from) if from == target_status => (false, false, Some(from)),
                Some(from) if is_drag_allowed(from, target_status) => (true, false, Some(from)),
                Some(from) => (false, true, Some(from)),
                None => (false, false, None),
            }
        }
        None => (false, false, None),
    };

    // D-11 hint copy — only meaningful when this column is the invalid
    // target. Pick the per-target hint, falling back to a generic
    // cross-skip message when the target has no per-column hint.
    let drag_hint: Option<&'static str> = if is_invalid_target {
        Some(drag_blocked_hint(target_status).unwrap_or(
            "This transition skips lifecycle steps — drag to TODO first",
        ))
    } else {
        None
    };
    let _ = drag_source_current; // captured for potential keyboard-DnD reuse

    rsx! {
        section {
            class: "kn-column",
            "data-status": "{status_str}",
            "data-drag-valid": if is_valid_target { "true" },
            "data-drag-invalid": if is_invalid_target { "true" },
            role: "list",
            "aria-label": "{aria_label}",
            // D-06: ondragover MUST call event.prevent_default() on the
            // outer Event<DragData> wrapper (Risk 4). Without this the
            // browser's default rejects the drop and ondrop never fires.
            ondragover: move |event: Event<DragData>| {
                event.prevent_default();
            },
            // D-06: ondrop — delegate to the shared move_task_optimistic
            // helper which enforces D-14 (is_drag_allowed) + D-07
            // (optimistic update with revert).
            ondrop: move |_event| {
                let tid = dragged_task_id.read().clone();
                if let Some(task_id) = tid {
                    move_task_optimistic(
                        task_id,
                        target_status,
                        tasks,
                        pending_task_ids,
                        toast_msg,
                        live_region_msg,
                        archive_modal_task,
                        dragged_task_id,
                    );
                }
            },
            div { class: "kn-column-header",
                span { class: "kn-column-label", "{status.label()}" }
                span { class: "kn-column-count", "{count}" }
            }
            // D-11 / UI-SPEC §7.4: drag-blocked hint overlay — only
            // renders while this column is the invalid drop target.
            if let Some(hint) = drag_hint {
                div { class: "kn-drag-hint", role: "tooltip", "{hint}" }
            }
            div { class: "kn-column-body",
                if filtered.is_empty() {
                    div { class: "kn-column-empty", "{status.empty_copy()}" }
                } else {
                    for task in filtered.into_iter() {
                        {
                            let task_id_for_card = task.id.clone();
                            // Check optimistic-pending state for this card.
                            let is_pending = pending_task_ids.read().contains(&task_id_for_card);
                            rsx! {
                                KanbanCard {
                                    key: "{task_id_for_card}",
                                    task: ReadSignal::new(Signal::new(task)),
                                    on_open_drawer: on_open_drawer,
                                    dragged_task_id: dragged_task_id,
                                    is_pending: is_pending,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
