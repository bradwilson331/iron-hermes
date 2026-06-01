//! Phase 36.3.7.11 Plan 01 (D-01) — Kanban card component.
//!
//! Visual contract = Models-page reference (CONTEXT canonical_refs):
//! cyan-glow border, monospace heading, thin separator, footer row of meta
//! chips. Per UI-SPEC §3.7: heading slot, separator, footer chips
//! (StatusChip / PriorityChip / AssigneeChip / AgeChip), action button
//! glyph `▶` on the right.
//!
//! Drag-and-drop attributes are reserved stubs in Plan 01 — `draggable="true"`
//! plus no-op `ondragstart`/`ondragend` handlers. Plan 02 wires them to the
//! `dragged_task_id: Signal<Option<String>>` and the optimistic-update path.

use crate::protocol::TaskRow;
use dioxus::prelude::*;

/// Phase 36.3.7.11 Plan 01: a single kanban task card.
///
/// Props:
///
/// - `task`: read-only signal carrying the task row.
/// - `on_open_drawer`: handler invoked when the user clicks the action
///   button (or focuses + activates the card). Plan 03 wires the drawer.
#[component]
pub fn KanbanCard(
    task: ReadSignal<TaskRow>,
    on_open_drawer: EventHandler<String>,
) -> Element {
    let t = task.read().clone();
    // ARIA label per UI-SPEC §6.1.
    let aria = format!(
        "{} — {}, priority {}, assigned to {}",
        t.title, t.status, t.priority, t.assignee
    );
    let task_id_for_click = t.id.clone();
    let task_title_for_aria = t.title.clone();

    rsx! {
        div {
            class: "kn-card",
            role: "listitem",
            tabindex: "0",
            "aria-label": "{aria}",
            // Plan 02 wires drag — Plan 01 reserves the attribute so the
            // browser knows the element is draggable for future plans.
            draggable: "true",
            "data-task-id": "{t.id}",
            // Heading slot: monospace title.
            div { class: "kn-card-heading", "{t.title}" }
            // Thin internal separator (per UI-SPEC §3.7).
            div { class: "kn-card-separator" }
            // Footer chip row.
            div { class: "kn-card-footer",
                span {
                    class: "kn-chip",
                    "data-kind": "status",
                    "data-status": "{t.status}",
                    "{t.status}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "priority",
                    "P{t.priority}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "assignee",
                    "{t.assignee}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "age",
                    "{format_age_secs(t.created_at)}"
                }
                // Action button — Plan 03 opens the drawer.
                button {
                    class: "kn-card-action",
                    "aria-label": "Open {task_title_for_aria} detail",
                    onclick: move |_| {
                        on_open_drawer.call(task_id_for_click.clone());
                    },
                    "▶"
                }
            }
        }
    }
}

/// Phase 36.3.7.11 Plan 01: format a task's `created_at` (Unix epoch
/// seconds, float) as a short relative age string for the card AGE chip.
/// Plan 01 uses a coarse seconds/minutes/hours/days bucket — Plan 04 polish
/// may add a richer format. Falls back to `"--"` on any clock error.
fn format_age_secs(created_at: f64) -> String {
    let now = current_unix_time();
    if now <= 0.0 {
        return "--".to_string();
    }
    let age_secs = (now - created_at).max(0.0) as u64;
    if age_secs < 60 {
        format!("{}s", age_secs)
    } else if age_secs < 60 * 60 {
        format!("{}m", age_secs / 60)
    } else if age_secs < 24 * 60 * 60 {
        format!("{}h", age_secs / (60 * 60))
    } else {
        format!("{}d", age_secs / (24 * 60 * 60))
    }
}

/// Phase 36.3.7.11 Plan 01: current time in Unix seconds (float). On
/// WASM uses `js_sys::Date::now()`; on native uses `SystemTime`. Returns
/// `0.0` on error so the caller can fall back to a placeholder.
fn current_unix_time() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}
