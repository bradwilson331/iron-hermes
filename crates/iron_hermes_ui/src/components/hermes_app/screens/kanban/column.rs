//! Phase 36.3.7.11 Plan 01 (D-09) — Kanban column component.
//!
//! Renders one status column: header (label + count) and either the
//! filtered cards or the empty-state copy per UI-SPEC §7.3.

use crate::components::hermes_app::screens::kanban::card::KanbanCard;
use crate::protocol::TaskRow;
use dioxus::prelude::*;

/// Phase 36.3.7.11 Plan 01: status taxonomy for the 6 visible columns +
/// archived (toolbar toggle). Values match `ironhermes_kanban::types::KanbanStatus`
/// lowercase reference names exactly (D-09).
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
}

/// Phase 36.3.7.11 Plan 01: a single kanban column.
///
/// Props:
/// - `status`: which column this is (drives label + filter key).
/// - `tasks`: read-only signal carrying the FULL board task list. The
///   column filters by `status` itself so each render reads the same
///   source-of-truth signal (no per-column slicing in the parent).
/// - `on_open_drawer`: forwarded to KanbanCard.
#[component]
pub fn KanbanColumn(
    status: KanbanColumnStatus,
    tasks: ReadSignal<Vec<TaskRow>>,
    on_open_drawer: EventHandler<String>,
) -> Element {
    let status_str = status.as_str();
    let filtered: Vec<TaskRow> = tasks
        .read()
        .iter()
        .filter(|t| t.status == status_str)
        .cloned()
        .collect();
    let count = filtered.len();
    let aria_label = format!("{} column, {} tasks", status.label(), count);

    rsx! {
        section {
            class: "kn-column",
            "data-status": "{status_str}",
            role: "list",
            "aria-label": "{aria_label}",
            div { class: "kn-column-header",
                span { class: "kn-column-label", "{status.label()}" }
                span { class: "kn-column-count", "{count}" }
            }
            div { class: "kn-column-body",
                if filtered.is_empty() {
                    div { class: "kn-column-empty", "{status.empty_copy()}" }
                } else {
                    for task in filtered.into_iter() {
                        KanbanCard {
                            key: "{task.id}",
                            task: ReadSignal::new(Signal::new(task)),
                            on_open_drawer: on_open_drawer,
                        }
                    }
                }
            }
        }
    }
}
