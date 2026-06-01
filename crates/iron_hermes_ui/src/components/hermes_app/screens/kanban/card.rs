//! Phase 36.3.7.11 Plan 02 (D-01 / D-06) — Kanban card component.
//!
//! Visual contract = Models-page reference (CONTEXT canonical_refs):
//! cyan-glow border, monospace heading, thin separator, footer row of meta
//! chips. Per UI-SPEC §3.7: heading slot, separator, footer chips
//! (StatusChip / PriorityChip / AssigneeChip / AgeChip), action button
//! glyph `▶` on the right.
//!
//! Plan 02 wires the drag-source: `draggable="true"` plus real
//! `ondragstart`/`ondragend` handlers that update a shared
//! `dragged_task_id: Signal<Option<String>>`. Plan 03 will consume the
//! `kn-card-action-<id>` id for drawer-close focus restoration.

use crate::protocol::TaskRow;
use dioxus::prelude::*;

/// Phase 36.3.7.11 Plan 02: a single kanban task card.
///
/// Props:
///
/// - `task`: read-only signal carrying the task row.
/// - `on_open_drawer`: handler invoked when the user clicks the action
///   button. Plan 03 wires the drawer.
/// - `dragged_task_id`: shared signal tracking the currently-dragged
///   task. `ondragstart` writes `Some(task_id)`; `ondragend` clears it.
/// - `is_pending`: true while an optimistic update is in flight for this
///   card (UI-SPEC §4.2 `kn-pending-pulse` animation).
#[component]
pub fn KanbanCard(
    task: ReadSignal<TaskRow>,
    on_open_drawer: EventHandler<String>,
    dragged_task_id: Signal<Option<String>>,
    is_pending: bool,
) -> Element {
    let t = task.read().clone();
    // ARIA label per UI-SPEC §6.1.
    let aria = format!(
        "{} — {}, priority {}, assigned to {}",
        t.title, t.status, t.priority, t.assignee
    );
    let task_id_for_click = t.id.clone();
    let task_id_for_start = t.id.clone();
    let task_title_for_aria = t.title.clone();

    // Plan 03 dependency: stable id `kn-card-action-<task_id>` so the
    // drawer-close focus restore can target the originating card. Same id
    // is used by the drawer's `on_close` handler in Plan 03.
    let action_id = format!("kn-card-action-{}", t.id);

    // is_dragging is true when THIS card is the active drag source — driven
    // by the shared dragged_task_id signal at board scope. Reading the
    // signal here causes Dioxus to subscribe; the read is value-copy so no
    // lock is held.
    let is_dragging = dragged_task_id
        .read()
        .as_deref()
        .map(|id| id == t.id)
        .unwrap_or(false);

    rsx! {
        div {
            class: "kn-card",
            role: "listitem",
            tabindex: "0",
            "aria-label": "{aria}",
            // D-06: drag source flag — required for ondragstart/ondrop to
            // fire in HTML5 native DnD.
            draggable: "true",
            "data-task-id": "{t.id}",
            // UI-SPEC §4.2: visual state attribute toggles for the
            // .kn-card[data-dragging] / .kn-card[data-pending] CSS rules.
            "data-dragging": if is_dragging { "true" },
            "data-pending": if is_pending { "true" },
            // D-06: drag-start — write Some(task_id) into the shared signal
            // so the drop target can read it. Signal `.write()` is a
            // value-copy operation; no borrow held across `.await` (there
            // is no await here — this is a synchronous handler).
            ondragstart: move |_| {
                *dragged_task_id.write() = Some(task_id_for_start.clone());
            },
            // D-06: drag-end (covers both successful drops and cancels) —
            // clear the shared signal as a safety net even though
            // KanbanColumn's ondrop also clears it.
            ondragend: move |_| {
                *dragged_task_id.write() = None;
            },
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
                // Action button — Plan 03 opens the drawer. The stable
                // id `kn-card-action-<task_id>` enables drawer-close focus
                // restoration per UI-SPEC §6.2.
                button {
                    class: "kn-card-action",
                    id: "{action_id}",
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
