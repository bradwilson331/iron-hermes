//! Phase 36.3.7.11 Plan 01 (D-09) — Kanban board component.
//!
//! Renders six visible columns (TRIAGE / TODO / READY / IN PROGRESS /
//! BLOCKED / DONE) plus an optional 7th `ARCHIVED` column when the
//! toolbar toggle is on. Status taxonomy matches `KanbanStatus` exactly
//! (lowercase reference names — D-09).

use crate::components::hermes_app::screens::kanban::column::{
    KanbanColumn, KanbanColumnStatus,
};
use crate::protocol::TaskRow;
use dioxus::prelude::*;

/// Phase 36.3.7.11 Plan 01: the 6-column board.
///
/// Props:
/// - `tasks`: read-only signal carrying the full board task list.
/// - `archived_visible`: when true, render the 7th archived column.
/// - `on_open_drawer`: forwarded to each card.
#[component]
pub fn KanbanBoard(
    tasks: ReadSignal<Vec<TaskRow>>,
    archived_visible: ReadSignal<bool>,
    on_open_drawer: EventHandler<String>,
) -> Element {
    let show_archived = *archived_visible.read();
    rsx! {
        div {
            class: "kn-board",
            role: "region",
            "aria-label": "Kanban board",
            KanbanColumn {
                status: KanbanColumnStatus::Triage,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Todo,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Ready,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            KanbanColumn {
                status: KanbanColumnStatus::InProgress,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Blocked,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Done,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
            }
            if show_archived {
                KanbanColumn {
                    status: KanbanColumnStatus::Archived,
                    tasks: tasks,
                    on_open_drawer: on_open_drawer,
                }
            }
        }
    }
}
