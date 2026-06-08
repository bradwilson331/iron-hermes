//! Phase 36.3.7.11 Plan 02 (D-09 / D-06 / D-07) — Kanban board component.
//!
//! Renders six visible columns (TRIAGE / TODO / READY / IN PROGRESS /
//! BLOCKED / DONE) plus an optional 7th `ARCHIVED` column when the
//! toolbar toggle is on. Status taxonomy matches `KanbanStatus` exactly
//! (lowercase reference names — D-09).
//!
//! Plan 02 adds the **shared optimistic-move helper** `move_task_optimistic`
//! that both `KanbanColumn::ondrop` and `KanbanCard`'s keyboard-DnD
//! handler consume. The helper enforces the canonical clone-and-drop
//! pattern from PATTERNS.md Pattern B: scoped write block → drop guard →
//! `spawn(async move { ... })` calling `patch_task_status`. On Err the
//! signal is reverted (new write lock acquired inside the spawn body —
//! the prior guard is long gone).

use crate::components::hermes_app::screens::kanban::column::{KanbanColumn, KanbanColumnStatus};
use crate::protocol::{DecomposeOrSpecify, KanbanStatus, TaskRow};
use dioxus::prelude::*;
use std::collections::HashSet;

/// Phase 36.3.7.11 Plan 02: the 6-column board.
///
/// Props:
/// - `tasks`: shared mutable signal — board mutates it optimistically
///   on every drop; on `#[server]` error the spawn body re-acquires
///   the lock to revert (PATTERNS.md Pattern B).
/// - `archived_visible`: when true, render the 7th archived column.
/// - `on_open_drawer`: forwarded to each card.
/// - `dragged_task_id`: shared signal — card writes Some(id) on
///   ondragstart, column reads it in ondrop.
/// - `pending_task_ids`: tasks with in-flight #[server] calls.
/// - `toast_msg`: optimistic-revert toast surface (UI-SPEC §7.5).
/// - `live_region_msg`: WAI-ARIA live-region announcement surface
///   (UI-SPEC §6.5 — disallowed transitions + move confirmations).
/// - `archive_modal_task`: Plan 03 archive-confirm modal target. Plan 02
///   sets `Some(task_id)` on archived drop; Plan 03 reads + opens modal.
#[component]
pub fn KanbanBoard(
    tasks: Signal<Vec<TaskRow>>,
    archived_visible: ReadSignal<bool>,
    on_open_drawer: EventHandler<String>,
    dragged_task_id: Signal<Option<String>>,
    pending_task_ids: Signal<HashSet<String>>,
    toast_msg: Signal<Option<String>>,
    live_region_msg: Signal<Option<String>>,
    archive_modal_task: Signal<Option<String>>,
    // Plan 03 (D-12): TRIAGE-card decompose/specify action handler.
    on_triage_action: EventHandler<(String, DecomposeOrSpecify)>,
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
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Todo,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Ready,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            KanbanColumn {
                status: KanbanColumnStatus::InProgress,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Blocked,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            KanbanColumn {
                status: KanbanColumnStatus::Done,
                tasks: tasks,
                on_open_drawer: on_open_drawer,
                dragged_task_id: dragged_task_id,
                pending_task_ids: pending_task_ids,
                toast_msg: toast_msg,
                live_region_msg: live_region_msg,
                archive_modal_task: archive_modal_task,
                on_triage_action: on_triage_action,
            }
            if show_archived {
                KanbanColumn {
                    status: KanbanColumnStatus::Archived,
                    tasks: tasks,
                    on_open_drawer: on_open_drawer,
                    dragged_task_id: dragged_task_id,
                    pending_task_ids: pending_task_ids,
                    toast_msg: toast_msg,
                    live_region_msg: live_region_msg,
                    archive_modal_task: archive_modal_task,
                    on_triage_action: on_triage_action,
                }
            }
        }
    }
}

/// Phase 36.3.7.11 Plan 02 (D-06 / D-07 / D-14): shared optimistic-move
/// helper. Called by both `KanbanColumn::ondrop` and the keyboard-DnD
/// alternative.
///
/// Behavior:
/// 1. Same-status / no-op — return (no announcement).
/// 2. `is_drag_allowed(current, target) == false` — set toast + live
///    region with `drag_blocked_hint(target)` (UI-SPEC §7.4); no mutation.
/// 3. `target == Archived` — set `archive_modal_task = Some(task_id)`; the
///    archive-confirm modal in Plan 03 fires the actual `#[server]` call.
/// 4. Otherwise — D-07 optimistic update:
///    - Scoped write to `tasks` (status mutation); guard drops at brace.
///    - Scoped write to `pending_task_ids` (insert).
///    - `spawn(async move { patch_task_status(...).await })` —
///      NO signal write across the .await (PATTERNS.md Pattern B).
///    - On Ok: scoped write to remove from `pending_task_ids`.
///    - On Err: scoped write to revert `tasks` (status to prev) +
///      remove from `pending_task_ids` + set toast (UI-SPEC §7.5).
/// 5. Always: clear `dragged_task_id` as a safety net.
#[allow(clippy::too_many_arguments)] // 8 signals threaded through the DnD chain by design
#[allow(dead_code)] // called from column.rs ondrop handler; dead_code fires on test target
pub fn move_task_optimistic(
    task_id: String,
    target: KanbanStatus,
    mut tasks: Signal<Vec<TaskRow>>,
    mut pending_task_ids: Signal<HashSet<String>>,
    mut toast_msg: Signal<Option<String>>,
    mut live_region_msg: Signal<Option<String>>,
    mut archive_modal_task: Signal<Option<String>>,
    mut dragged_task_id: Signal<Option<String>>,
) {
    use crate::kanban::transitions::{drag_blocked_hint, is_drag_allowed};

    // Always clear the drag-source signal as a safety net (ondragend will
    // also clear it; double-clear is harmless).
    dragged_task_id.set(None);

    // Look up the current status + title under a scoped READ. We clone
    // owned data so the read guard drops immediately.
    let snapshot: Option<(KanbanStatus, String)> = {
        let read = tasks.read();
        read.iter().find(|t| t.id == task_id).map(|t| {
            let parsed = KanbanStatus::from_wire_str(&t.status).unwrap_or(KanbanStatus::Triage);
            (parsed, t.title.clone())
        })
    };
    let (current, title) = match snapshot {
        Some(pair) => pair,
        None => return, // unknown task — nothing to do
    };

    // 1. Same-status / no-op.
    if current == target {
        return;
    }

    // 2. Disallowed — surface hint via toast + live region.
    if !is_drag_allowed(current, target) {
        let hint = drag_blocked_hint(target)
            .unwrap_or("This transition skips lifecycle steps — drag to TODO first")
            .to_string();
        toast_msg.set(Some(hint.clone()));
        live_region_msg.set(Some(hint));
        return;
    }

    // 3. Archive → defer to Plan 03's confirm modal.
    if target == KanbanStatus::Archived {
        archive_modal_task.set(Some(task_id));
        return;
    }

    // 4. Optimistic update (D-07 / Pattern B).
    //
    // Step (a): scoped write — change the task's status field.
    {
        let mut w = tasks.write();
        if let Some(t) = w.iter_mut().find(|t| t.id == task_id) {
            t.status = target.as_wire_str().to_string();
        }
        // WriteLock drops at this closing brace — BEFORE the spawn.
    }
    // Step (b): scoped write — mark pending.
    {
        let mut p = pending_task_ids.write();
        p.insert(task_id.clone());
        // Write lock drops at this closing brace.
    }

    // Capture owned values for the spawn closure.
    let task_id_owned = task_id.clone();
    let target_owned = target;
    let prev_status = current;
    let title_owned = title;

    // Step (c): the async write. No signal borrow crosses .await.
    spawn(async move {
        match crate::server::kanban_api::patch_task_status(
            task_id_owned.clone(),
            None,
            target_owned,
            None,
        )
        .await
        {
            Ok(_) => {
                // (d): clear pending.
                {
                    let mut p = pending_task_ids.write();
                    p.remove(&task_id_owned);
                }
                // Announce the move via live region.
                let msg = format!(
                    "Task '{}' moved to {}",
                    title_owned,
                    target_owned.as_wire_str().to_uppercase()
                );
                live_region_msg.set(Some(msg));
            }
            Err(e) => {
                // (e): revert + toast + clear pending.
                {
                    let mut w = tasks.write();
                    if let Some(t) = w.iter_mut().find(|t| t.id == task_id_owned) {
                        t.status = prev_status.as_wire_str().to_string();
                    }
                }
                {
                    let mut p = pending_task_ids.write();
                    p.remove(&task_id_owned);
                }
                let toast = format!(
                    "Move failed — \"{}\" returned to {}. {}",
                    title_owned,
                    prev_status.as_wire_str().to_uppercase(),
                    e
                );
                toast_msg.set(Some(toast.clone()));
                live_region_msg.set(Some(toast));
            }
        }
    });
}
