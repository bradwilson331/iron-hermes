//! Phase 36.3.7.11 Plan 03 (D-20 / D-21) — Kanban task detail drawer.
//!
//! Mid-tier panel that slides in from the right (UI-SPEC §3.9 / §5.2) when
//! `task_id.read().is_some()`. Renders 7 sections in the D-20 order:
//!   1. DrawerHeader (sticky)
//!   2. StatusActionRow (Complete... / Block... / Unblock task / Archive task)
//!   3. TriageActionRow (conditional — when status == Triage; ⚗ Decompose / ✨ Specify)
//!   4. WorkerContextBlock (parent_handoffs + prior_attempts per show.rs lines 218-232)
//!   5. RunHistorySection (task_runs with outcome badges per UI-SPEC §7.7)
//!   6. EventStreamSection (last 20 task_events per UI-SPEC §7.6 label)
//!   7. CommentSection (existing comments + compose box; POST wired in Task 2)
//!
//! Live-refresh contract (D-21 / UI-SPEC §8.4):
//! - `ScreenKanban` maintains `per_task_event_counter: Signal<HashMap<String, u64>>`
//!   that increments (after 200ms debounce) when a WS event for the open task
//!   arrives.
//! - Each `use_resource` reads `task_id()` AND `per_task_event_counter()`
//!   inside its async closure so Dioxus restarts the resource when either
//!   value changes (the 200ms debounce is enforced screen-side).
//!
//! Focus management (UI-SPEC §3.9 / §6.2):
//! - On open: focus moves to the close button (`web_sys::HtmlElement::focus()`
//!   on the element with id `kn-drawer-close`).
//! - On close: focus returns to the triggering card action button via the
//!   stable id `kn-card-action-<task_id>` set in Plan 02's card.rs.
//! - Escape closes the drawer (keydown handler on the drawer container).
//!
//! Modal-open events: drawer emits `on_open_complete_modal(task_id)`,
//! `on_open_block_modal(task_id)`, `on_open_archive_modal(task_id)` —
//! ScreenKanban handles these by setting modal-target signals that
//! `modals.rs` reads.

use crate::protocol::{
    CommentRow, KanbanEventRow, KanbanStatus, TaskRunRow, WorkerContextEnvelope,
};
use dioxus::prelude::*;
use std::collections::HashMap;

/// Phase 36.3.7.11 Plan 03 (D-20 / D-21 / UI-SPEC §3.9): the task detail
/// drawer. Mounts when `task_id.read().is_some()`.
///
/// Props:
/// - `task_id`: shared signal — Some(id) renders, None hides.
/// - `per_task_event_counter`: shared signal — D-21 refresh trigger.
/// - `on_close`: invoked when user clicks ✕ or presses Escape.
/// - `on_open_complete_modal` / `on_open_block_modal` / `on_open_archive_modal`:
///   Task 2 wires modal-open handlers to these events.
/// - `on_unblock`: Unblock is a one-click action (no modal) — Task 2 spawns
///   `patch_task_status(task_id, None, Ready, None)` on this event.
/// - `on_decompose` / `on_specify`: Task 2 spawns
///   `run_decompose_or_specify(task_id, None, action)` on these events; the
///   drawer surfaces `NotWired` via tooltip on the button (UI-SPEC §4.3).
/// - `on_post_comment`: Task 2 wires the compose POST.
#[allow(clippy::too_many_arguments)]
#[component]
pub fn TaskDrawer(
    task_id: ReadSignal<Option<String>>,
    per_task_event_counter: ReadSignal<HashMap<String, u64>>,
    on_close: EventHandler<()>,
    on_open_complete_modal: EventHandler<String>,
    on_open_block_modal: EventHandler<String>,
    on_open_archive_modal: EventHandler<String>,
    on_unblock: EventHandler<String>,
    on_decompose: EventHandler<String>,
    on_specify: EventHandler<String>,
    on_post_comment: EventHandler<(String, String)>,
) -> Element {
    // ALL hooks register unconditionally (Pattern E — PATTERNS.md). The
    // resources read `task_id()` and `per_task_event_counter()` inside the
    // async block; Dioxus tracks these calls and restarts the resource
    // when either signal changes.

    // Resource 1: worker_context envelope (D-20: header + worker_context block).
    let task_detail = use_resource(move || async move {
        let id = task_id();
        // Read per_task_event_counter to register dependency tracking (D-21).
        // The value is intentionally unused — its presence registers the
        // dependency so the resource restarts on counter increment.
        let _counter = per_task_event_counter();
        match id {
            Some(tid) => Some(crate::server::kanban_api::fetch_task(tid, None).await),
            None => None,
        }
    });

    // Resource 2: run history.
    let runs = use_resource(move || async move {
        let id = task_id();
        let _counter = per_task_event_counter();
        match id {
            Some(tid) => Some(crate::server::kanban_api::fetch_task_runs(tid, None).await),
            None => None,
        }
    });

    // Resource 3: last 20 events.
    let events = use_resource(move || async move {
        let id = task_id();
        let _counter = per_task_event_counter();
        match id {
            Some(tid) => Some(crate::server::kanban_api::fetch_task_events(tid, None, 20).await),
            None => None,
        }
    });

    // Resource 4: comments.
    let comments = use_resource(move || async move {
        let id = task_id();
        let _counter = per_task_event_counter();
        match id {
            Some(tid) => Some(crate::server::kanban_api::fetch_comments(tid, None).await),
            None => None,
        }
    });

    // Compose box state (Task 1 stub; Task 2 wires the actual POST via
    // on_post_comment). Register the signal unconditionally before any
    // conditional RSX.
    let mut comment_draft: Signal<String> = use_signal(String::new);

    // Focus the close button when the drawer opens (UI-SPEC §3.9). Use a
    // use_effect that fires when `task_id` becomes Some.
    use_effect(move || {
        let is_open = task_id.read().is_some();
        if is_open {
            // Schedule the focus call on the next render tick so the
            // close button DOM node exists when we look it up.
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Some(document) = window.document() {
                        if let Some(el) = document.get_element_by_id("kn-drawer-close") {
                            if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
                                let _ = html_el.focus();
                            }
                        }
                    }
                }
            }
        } else {
            // Drawer closed — restore focus to the triggering card action
            // button. Plan 02's card.rs assigns id `kn-card-action-<task_id>`
            // to the button that opened this drawer. We capture the last
            // opened task_id via a separate signal at the screen level;
            // here we just trigger a generic refocus by looking up the
            // first card action button (best-effort fallback). The screen
            // can provide a richer last_focused signal if needed.
            #[cfg(target_arch = "wasm32")]
            {
                // No-op: ScreenKanban owns the last_focused_card_id signal
                // and is the better place to restore focus on close.
            }
        }
    });

    // ARIA-conditional snapshot — read once outside RSX so we don't re-read
    // the signal mid-render in conditional branches.
    let open_id_opt = task_id.read().clone();
    let is_open = open_id_opt.is_some();

    // The drawer renders nothing unless open.
    if !is_open {
        // Still need to register all the hooks above on every render — we
        // returned early ONLY for the visible RSX subtree.
        return rsx! {};
    }

    // Pull the latest task detail, runs, events, comments out of their
    // Option<Result<_,_>> wrappers. None = loading; Some(Err) = error;
    // Some(Ok) = ready.
    let task_state = task_detail.value()();
    let runs_state = runs.value()();
    let events_state = events.value()();
    let comments_state = comments.value()();

    // Derive header content from task_state.
    let (title_str, status_str, assignee_str, priority_str, tenant_str, parsed_status) =
        match &task_state {
            Some(Some(Ok(env))) => (
                env.title.clone(),
                env.status.clone(),
                env.assignee.clone(),
                format!("P{}", env.priority),
                env.tenant.clone().unwrap_or_default(),
                KanbanStatus::from_wire_str(&env.status),
            ),
            _ => (
                String::from("Loading..."),
                String::from("unknown"),
                String::new(),
                String::new(),
                String::new(),
                None,
            ),
        };

    // UI-SPEC §4.3 state-matrix derivations.
    let is_running = matches!(parsed_status, Some(KanbanStatus::InProgress));
    let is_done = matches!(parsed_status, Some(KanbanStatus::Done));
    let is_archived = matches!(parsed_status, Some(KanbanStatus::Archived));
    let is_blocked = matches!(parsed_status, Some(KanbanStatus::Blocked));
    let is_triage = matches!(parsed_status, Some(KanbanStatus::Triage));

    // Action button visibility per UI-SPEC §4.3:
    //   RUNNING task: action buttons all disabled with tooltip.
    //   DONE task:    Complete/Block hidden; Unblock hidden; Archive remains.
    //   ARCHIVED:     all action buttons hidden.
    let show_complete = !is_done && !is_archived;
    let show_block = !is_done && !is_archived;
    let show_unblock = is_blocked && !is_archived;
    let show_archive = !is_archived;
    let actions_disabled = is_running;
    let actions_tooltip = if actions_disabled {
        "Dispatcher-owned — use CLI to manage running tasks"
    } else {
        ""
    };

    // Captured ids for the per-button onclick closures (move semantics).
    let open_id_for_complete = open_id_opt.clone().unwrap_or_default();
    let open_id_for_block = open_id_opt.clone().unwrap_or_default();
    let open_id_for_unblock = open_id_opt.clone().unwrap_or_default();
    let open_id_for_archive = open_id_opt.clone().unwrap_or_default();
    let open_id_for_decompose = open_id_opt.clone().unwrap_or_default();
    let open_id_for_specify = open_id_opt.clone().unwrap_or_default();
    let open_id_for_comment = open_id_opt.clone().unwrap_or_default();

    let aria_label = format!("Task detail: {}", title_str);

    rsx! {
        aside {
            class: "kn-drawer",
            "data-open": "true",
            role: "complementary",
            "aria-label": "{aria_label}",
            "aria-modal": "false",
            // UI-SPEC §6.2 / §3.9: Escape closes the drawer.
            onkeydown: move |event| {
                if event.key() == Key::Escape {
                    on_close.call(());
                }
            },
            // ------------------------------------------------------------
            // 1. DrawerHeader (sticky)
            // ------------------------------------------------------------
            div { class: "kn-drawer-header",
                div { class: "kn-drawer-header-row",
                    h2 { class: "kn-drawer-title", "{title_str}" }
                    button {
                        class: "kn-drawer-close",
                        id: "kn-drawer-close",
                        "aria-label": "Close task detail",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "kn-drawer-chips",
                    span {
                        class: "kn-chip",
                        "data-kind": "status",
                        "data-status": "{status_str}",
                        "{status_str}"
                    }
                    if !priority_str.is_empty() {
                        span { class: "kn-chip", "data-kind": "priority", "{priority_str}" }
                    }
                    if !assignee_str.is_empty() {
                        span { class: "kn-chip", "data-kind": "assignee", "{assignee_str}" }
                    }
                    if !tenant_str.is_empty() {
                        span { class: "kn-chip", "data-kind": "tenant", "{tenant_str}" }
                    }
                }
            }

            // ------------------------------------------------------------
            // 2. StatusActionRow
            // ------------------------------------------------------------
            div { class: "kn-drawer-section",
                if show_complete {
                    button {
                        class: "kn-action-btn",
                        disabled: actions_disabled,
                        title: "{actions_tooltip}",
                        onclick: move |_| {
                            if !actions_disabled {
                                on_open_complete_modal.call(open_id_for_complete.clone());
                            }
                        },
                        "Complete\u{2026}"
                    }
                }
                if show_block {
                    button {
                        class: "kn-action-btn",
                        disabled: actions_disabled,
                        title: "{actions_tooltip}",
                        onclick: move |_| {
                            if !actions_disabled {
                                on_open_block_modal.call(open_id_for_block.clone());
                            }
                        },
                        "Block\u{2026}"
                    }
                }
                if show_unblock {
                    button {
                        class: "kn-action-btn",
                        disabled: actions_disabled,
                        title: "{actions_tooltip}",
                        onclick: move |_| {
                            if !actions_disabled {
                                on_unblock.call(open_id_for_unblock.clone());
                            }
                        },
                        "Unblock task"
                    }
                }
                if show_archive {
                    button {
                        class: "kn-action-btn",
                        disabled: actions_disabled,
                        title: "{actions_tooltip}",
                        onclick: move |_| {
                            if !actions_disabled {
                                on_open_archive_modal.call(open_id_for_archive.clone());
                            }
                        },
                        "Archive task"
                    }
                }
            }

            // ------------------------------------------------------------
            // 3. TriageActionRow (conditional — only when status == Triage)
            // D-12: TRIAGE drawer exposes ⚗ Decompose / ✨ Specify.
            // ------------------------------------------------------------
            if is_triage {
                div { class: "kn-drawer-section kn-drawer-triage-actions",
                    button {
                        class: "kn-action-btn",
                        onclick: move |_| on_decompose.call(open_id_for_decompose.clone()),
                        "\u{2697} Decompose"
                    }
                    button {
                        class: "kn-action-btn",
                        onclick: move |_| on_specify.call(open_id_for_specify.clone()),
                        "\u{2728} Specify"
                    }
                }
            }

            // ------------------------------------------------------------
            // 4. WorkerContextBlock
            // ------------------------------------------------------------
            WorkerContextBlock { task_state: task_state.clone() }

            // ------------------------------------------------------------
            // 5. RunHistorySection
            // ------------------------------------------------------------
            RunHistorySection { runs_state: runs_state.clone() }

            // ------------------------------------------------------------
            // 6. EventStreamSection (last 20)
            // ------------------------------------------------------------
            EventStreamSection { events_state: events_state.clone() }

            // ------------------------------------------------------------
            // 7. CommentSection (existing + compose)
            // ------------------------------------------------------------
            div { class: "kn-drawer-section",
                div { class: "kn-drawer-section-label", "COMMENTS" }
                CommentList { comments_state: comments_state.clone() }
                div { class: "kn-comment-compose",
                    textarea {
                        class: "kn-comment-textarea",
                        placeholder: "Add a comment...",
                        value: "{comment_draft}",
                        oninput: move |evt| comment_draft.set(evt.value()),
                    }
                    button {
                        class: "kn-action-btn",
                        onclick: move |_| {
                            let body = comment_draft.read().clone();
                            if !body.is_empty() {
                                on_post_comment
                                    .call((open_id_for_comment.clone(), body));
                                comment_draft.set(String::new());
                            }
                        },
                        "Post comment"
                    }
                }
            }
        }
    }
}

// ============================================================================
// Sub-components — each takes the relevant resource state.
// ============================================================================

/// D-20 section: render parent_handoffs + prior_attempts blocks (1:1 from
/// Q5 worker_context envelope). Plain-text rendering only — no markdown.
#[component]
fn WorkerContextBlock(
    task_state: Option<Option<Result<WorkerContextEnvelope, ServerFnError>>>,
) -> Element {
    rsx! {
        div { class: "kn-drawer-section",
            div { class: "kn-drawer-section-label", "WORKER CONTEXT" }
            match task_state {
                Some(Some(Ok(env))) => rsx! {
                    if let Some(body) = env.body.as_ref() {
                        div { class: "kn-drawer-body", "{body}" }
                    }
                    div { class: "kn-drawer-section-label", "PARENT HANDOFFS" }
                    if env.parent_handoffs.is_empty() {
                        div { class: "kn-drawer-empty", "No parent handoffs." }
                    } else {
                        for handoff in env.parent_handoffs.iter() {
                            div { class: "kn-handoff-row",
                                div { class: "kn-handoff-parent",
                                    "{handoff.get(\"parent_title\").and_then(|v| v.as_str()).unwrap_or(\"(unknown parent)\")}"
                                }
                                div { class: "kn-handoff-status",
                                    "Status: {handoff.get(\"parent_status\").and_then(|v| v.as_str()).unwrap_or(\"\")}"
                                }
                                if let Some(summary) = handoff.get("summary").and_then(|v| v.as_str()) {
                                    div { class: "kn-handoff-summary", "{summary}" }
                                }
                            }
                        }
                    }
                    div { class: "kn-drawer-section-label", "PRIOR ATTEMPTS" }
                    if env.prior_attempts.is_empty() {
                        div { class: "kn-drawer-empty", "No prior attempts." }
                    } else {
                        for attempt in env.prior_attempts.iter() {
                            div { class: "kn-attempt-row",
                                span {
                                    class: "kn-badge",
                                    "data-outcome": "{attempt.get(\"outcome\").and_then(|v| v.as_str()).unwrap_or(\"\")}",
                                    "{attempt.get(\"outcome\").and_then(|v| v.as_str()).unwrap_or(\"unknown\")}"
                                }
                                if let Some(summary) = attempt.get("summary").and_then(|v| v.as_str()) {
                                    div { class: "kn-attempt-summary", "{summary}" }
                                }
                                if let Some(error) = attempt.get("error").and_then(|v| v.as_str()) {
                                    div { class: "kn-attempt-error", "{error}" }
                                }
                            }
                        }
                    }
                },
                Some(Some(Err(e))) => rsx! {
                    div { class: "kn-drawer-error", "Could not load task details. {e}" }
                },
                _ => rsx! {
                    div { class: "kn-drawer-loading", "Loading worker context\u{2026}" }
                },
            }
        }
    }
}

/// D-20 section: render task_runs rows with outcome badges per UI-SPEC §7.7.
#[component]
fn RunHistorySection(
    runs_state: Option<Option<Result<Vec<TaskRunRow>, ServerFnError>>>,
) -> Element {
    rsx! {
        div { class: "kn-drawer-section",
            div { class: "kn-drawer-section-label", "RUN HISTORY" }
            match runs_state {
                Some(Some(Ok(rows))) if rows.is_empty() => rsx! {
                    div { class: "kn-drawer-empty", "No runs yet." }
                },
                Some(Some(Ok(rows))) => rsx! {
                    for row in rows.iter() {
                        div { class: "kn-run-row",
                            span {
                                class: "kn-badge",
                                "data-outcome": "{row.outcome.as_deref().unwrap_or(\"active\")}",
                                "{outcome_label(row.outcome.as_deref())}"
                            }
                            if let Some(ms) = row.elapsed_ms {
                                span { class: "kn-run-elapsed", "{format_elapsed_ms(ms)}" }
                            }
                            if let Some(summary) = row.summary.as_ref() {
                                div { class: "kn-run-summary", "{summary}" }
                            }
                            if let Some(error) = row.error.as_ref() {
                                div { class: "kn-run-error", "{error}" }
                            }
                        }
                    }
                },
                Some(Some(Err(e))) => rsx! {
                    div { class: "kn-drawer-error", "Could not load runs. {e}" }
                },
                _ => rsx! {
                    div { class: "kn-drawer-loading", "Loading runs\u{2026}" }
                },
            }
        }
    }
}

/// D-20 section: render the last 20 task_events. Read-only.
#[component]
fn EventStreamSection(
    events_state: Option<Option<Result<Vec<KanbanEventRow>, ServerFnError>>>,
) -> Element {
    rsx! {
        div { class: "kn-drawer-section",
            div { class: "kn-drawer-section-label", "EVENTS (last 20)" }
            match events_state {
                Some(Some(Ok(rows))) if rows.is_empty() => rsx! {
                    div { class: "kn-drawer-empty", "No events." }
                },
                Some(Some(Ok(rows))) => rsx! {
                    // Newest first per UI-SPEC §3.9 / drawer scrim convention.
                    for row in rows.iter().rev() {
                        div { class: "kn-event-row",
                            span { class: "kn-event-kind", "{row.kind}" }
                            span { class: "kn-event-time", "{format_unix_secs(row.created_at)}" }
                        }
                    }
                },
                Some(Some(Err(e))) => rsx! {
                    div { class: "kn-drawer-error", "Could not load events. {e}" }
                },
                _ => rsx! {
                    div { class: "kn-drawer-loading", "Loading events\u{2026}" }
                },
            }
        }
    }
}

/// D-20 sub-section: render existing comments.
#[component]
fn CommentList(
    comments_state: Option<Option<Result<Vec<CommentRow>, ServerFnError>>>,
) -> Element {
    rsx! {
        match comments_state {
            Some(Some(Ok(rows))) if rows.is_empty() => rsx! {
                div { class: "kn-drawer-empty", "No comments yet." }
            },
            Some(Some(Ok(rows))) => rsx! {
                for row in rows.iter() {
                    div { class: "kn-comment-row",
                        div { class: "kn-comment-meta",
                            span { class: "kn-comment-author", "{row.author}" }
                            span { class: "kn-comment-time", "{format_unix_secs(row.created_at)}" }
                        }
                        // Plain-text body — no markdown rendering (threat T-36.3.7.11-03-T02).
                        div { class: "kn-comment-body", "{row.body}" }
                    }
                }
            },
            Some(Some(Err(e))) => rsx! {
                div { class: "kn-drawer-error", "Could not load comments. {e}" }
            },
            _ => rsx! {
                div { class: "kn-drawer-loading", "Loading comments\u{2026}" }
            },
        }
    }
}

// ============================================================================
// Pure helpers
// ============================================================================

/// Map `task_runs.outcome` value → human-readable badge label per UI-SPEC §7.7.
fn outcome_label(outcome: Option<&str>) -> &'static str {
    match outcome {
        Some("completed") => "COMPLETED",
        Some("blocked") => "BLOCKED",
        Some("crashed") => "CRASHED",
        Some("gave_up") => "GAVE UP",
        Some("timed_out") => "TIMED OUT",
        Some("spawn_failed") => "SPAWN FAILED",
        Some("active") => "ACTIVE",
        None => "ACTIVE",
        _ => "UNKNOWN",
    }
}

/// Format an elapsed duration in milliseconds as a short string.
fn format_elapsed_ms(ms: i64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// Format a Unix epoch (float seconds) as a short relative age string.
fn format_unix_secs(secs: f64) -> String {
    let now = current_unix_time();
    if now <= 0.0 {
        return "--".to_string();
    }
    let age = (now - secs).max(0.0) as u64;
    if age < 60 {
        format!("{}s ago", age)
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    }
}

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

// Bring web-sys's dyn_into trait into scope for the focus-on-open use_effect.
#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;
