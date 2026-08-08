//! Schedules screen — Phase 46.9 Plan 04 (D-06): live-wired to
//! `ironhermes-cron`'s `JobStore` via `server::schedules_api`, replacing the
//! prior pure mock data source with full CRUD + enable/disable + run-now.
//!
//! Read side: `use_server_future(get_schedules)` seeds a local
//! `Signal<Vec<ScheduleRow>>` once (mirrors `providers.rs`'s optimistic
//! working-copy pattern — `use_server_future(...)?` early-returns while
//! loading, so `.restart()` would break hook ordering for signals declared
//! after it; a successful write instead calls `get_schedules()` directly to
//! refresh).
//!
//! Write side: the NEW JOB and EDIT actions open an inline
//! `schedule-editor-form` panel. SAVE JOB calls `create_schedule` or
//! `update_schedule` and disables itself, reading "SAVING…" while in
//! flight. The `.tgl` toggle in the STATE column calls
//! `set_schedule_enabled` directly (no form). RUN NOW calls
//! `run_schedule_now`. DELETE opens an inline red confirmation panel
//! ("Delete job") before calling `delete_schedule`.
//!
//! Schedules is the ONE surface in this phase with NO restart-required
//! banner — `JobStore::reload()` re-reads `jobs.json` every tick, so writes
//! apply live (D-10 schedule exemption). Do not add one here.

use dioxus::prelude::*;

use crate::server::schedules_api::{
    create_schedule, delete_schedule, get_schedules, run_schedule_now, set_schedule_enabled,
    update_schedule, ScheduleRow,
};

/// Map a server error into the two-line copy the UI-SPEC specifies. Invalid
/// schedules render the exact Copywriting Contract lines; everything else
/// falls back to a generic line plus the raw server message.
#[allow(dead_code)] // called from the SAVE JOB onclick closure in
                    // ScreenSchedules; dead_code fires under `--all-features
                    // --all-targets` (test target) — same known false
                    // positive as skills.rs's tab_predicate/search_matches
                    // helpers and providers.rs's map_save_error in this crate.
fn map_save_error(e: &ServerFnError) -> (String, String) {
    let msg = e.to_string();
    if msg.contains("Invalid schedule") {
        (
            "Invalid schedule.".to_string(),
            "Use a cron expression (0 9 * * *), interval (every 2h), or timestamp (2026-08-01T09:00Z).".to_string(),
        )
    } else {
        ("Save failed.".to_string(), msg)
    }
}

#[component]
pub fn ScreenSchedules(is_active: bool) -> Element {
    let schedules_resource = use_server_future(get_schedules)?;

    // Extract data BEFORE rsx! — signal borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let is_loading = schedules_resource().is_none();
    let load_error = matches!(schedules_resource(), Some(Err(_)));

    // Optimistic local working copy — seeded once from the resource (see
    // module doc / providers.rs precedent for why this is not driven by
    // `.restart()`).
    let mut schedule_list_sig: Signal<Vec<ScheduleRow>> = use_signal(Vec::new);
    let mut seeded = use_signal(|| false);
    {
        let loaded = match schedules_resource() {
            Some(Ok(ref rows)) => Some(rows.clone()),
            _ => None,
        };
        use_effect(move || {
            if let Some(ref rows) = loaded {
                if !*seeded.read() {
                    schedule_list_sig.set(rows.clone());
                    seeded.set(true);
                }
            }
        });
    }

    let schedule_list = schedule_list_sig.read().clone();

    // ── Editor form state ───────────────────────────────────────────────
    let mut editor_open = use_signal(|| false);
    let mut editor_is_new = use_signal(|| false);
    let mut editor_id = use_signal(String::new);
    let mut editor_name = use_signal(String::new);
    let mut editor_schedule = use_signal(String::new);
    let mut editor_prompt = use_signal(String::new);
    let mut editor_deliver = use_signal(String::new);
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<(String, String)>> = use_signal(|| None);

    // ── Delete-confirm state ────────────────────────────────────────────
    let mut delete_confirm: Signal<Option<ScheduleRow>> = use_signal(|| None);
    let mut deleting = use_signal(|| false);

    // Read all signal values into owned locals BEFORE rsx! (Pattern B).
    let editor_open_val = *editor_open.read();
    let editor_is_new_val = *editor_is_new.read();
    let editor_name_val = editor_name.read().clone();
    let editor_schedule_val = editor_schedule.read().clone();
    let editor_prompt_val = editor_prompt.read().clone();
    let editor_deliver_val = editor_deliver.read().clone();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let delete_confirm_val = delete_confirm.read().clone();
    let deleting_val = *deleting.read();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-schedules",
            "data-screen-label": "09 Schedules",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 09" }
                    h1 { class: "screen-title", "Schedules" }
                    p { class: "screen-sub",
                        "Cron-driven jobs with delivery targets. Hermes runs the prompt, formats the output, and sends it where you choose."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⏵ HISTORY" }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| {
                            editor_is_new.set(true);
                            editor_id.set(String::new());
                            editor_name.set(String::new());
                            editor_schedule.set(String::new());
                            editor_prompt.set(String::new());
                            editor_deliver.set(String::new());
                            save_error.set(None);
                            editor_open.set(true);
                        },
                        "+ NEW JOB"
                    }
                }
            }

            if let Some(ref row) = delete_confirm_val {
                div {
                    class: "panel",
                    style: "margin-top:14px;border-color:rgba(248,81,73,0.45);background:rgba(248,81,73,0.06);",
                    div { class: "panel-title", style: "color:var(--red);", "Delete job" }
                    p { style: "color:var(--text);font-size:12px;margin:0 0 12px 0;",
                        "This permanently removes the job and its run history. Continue?"
                    }
                    p { style: "color:var(--gray);font-size:11px;margin:0 0 12px 0;", "{row.name}" }
                    div { style: "display:flex;gap:10px;",
                        button {
                            class: "btn btn--sm",
                            style: "background:var(--red);border-color:var(--red);",
                            disabled: deleting_val,
                            onclick: move |_| {
                                let Some(row) = delete_confirm.read().clone() else { return };
                                let id = row.id.clone();
                                deleting.set(true);
                                spawn(async move {
                                    match delete_schedule(id.clone()).await {
                                        Ok(()) => {
                                            if let Ok(fresh) = get_schedules().await {
                                                schedule_list_sig.set(fresh);
                                            }
                                            deleting.set(false);
                                            delete_confirm.set(None);
                                        }
                                        Err(_e) => {
                                            deleting.set(false);
                                        }
                                    }
                                });
                            },
                            if deleting_val { "DELETING…" } else { "DELETE" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: deleting_val,
                            onclick: move |_| delete_confirm.set(None),
                            "CANCEL"
                        }
                    }
                }
            }

            if editor_open_val {
                div { class: "panel", style: "margin-top:14px;",
                    div { class: "panel-title",
                        if editor_is_new_val { "New Job" } else { "Edit Job" }
                    }

                    if let Some((ref line1, ref line2)) = save_error_val {
                        div { style: "color:var(--red);font-size:12px;margin-bottom:10px;",
                            div { "{line1}" }
                            div { style: "margin-top:2px;", "{line2}" }
                        }
                    }

                    div { class: "field-row",
                        div { class: "field-label", "Job name" }
                        input {
                            class: "field-input",
                            placeholder: "e.g. daily-report",
                            value: "{editor_name_val}",
                            oninput: move |e| editor_name.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label",
                            "Schedule"
                            span { class: "help", "cron, \"every 2h\", or a timestamp" }
                        }
                        input {
                            class: "field-input",
                            placeholder: "0 9 * * *",
                            value: "{editor_schedule_val}",
                            oninput: move |e| {
                                save_error.set(None);
                                editor_schedule.set(e.value());
                            },
                        }
                        if let Some((ref line1, ref line2)) = save_error_val {
                            if line1.starts_with("Invalid schedule") {
                                div { style: "color:var(--red);font-size:11px;margin-top:4px;",
                                    div { "{line1}" }
                                    div { "{line2}" }
                                }
                            }
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label", "Prompt" }
                        textarea {
                            class: "field-input",
                            style: "min-height:64px;resize:vertical;",
                            rows: "3",
                            placeholder: "Summarize yesterday's activity and send a digest.",
                            value: "{editor_prompt_val}",
                            oninput: move |e| editor_prompt.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label",
                            "Delivery"
                            span { class: "help", "local, origin, telegram:<chat_id>, webhook:<url>" }
                        }
                        input {
                            class: "field-input",
                            placeholder: "local",
                            value: "{editor_deliver_val}",
                            oninput: move |e| editor_deliver.set(e.value()),
                        }
                    }

                    div { style: "display:flex;gap:10px;margin-top:6px;",
                        button {
                            class: "btn btn--sm",
                            disabled: saving_val,
                            onclick: move |_| {
                                // Pattern B: read all signal values into owned
                                // locals BEFORE spawn — no borrow across .await.
                                let id_local = editor_id.read().clone();
                                let is_new_local = *editor_is_new.read();
                                let name_local = editor_name.read().clone();
                                let schedule_local = editor_schedule.read().clone();
                                let prompt_local = editor_prompt.read().clone();
                                let deliver_local = editor_deliver.read().clone();

                                saving.set(true);
                                save_error.set(None);

                                spawn(async move {
                                    let result = if is_new_local {
                                        create_schedule(name_local, schedule_local, prompt_local, deliver_local).await
                                    } else {
                                        update_schedule(id_local, name_local, schedule_local, prompt_local, deliver_local).await
                                    };
                                    match result {
                                        Ok(_row) => {
                                            // Re-fetch authoritative state directly
                                            // (NOT schedules_resource.restart() —
                                            // see module doc).
                                            if let Ok(fresh) = get_schedules().await {
                                                schedule_list_sig.set(fresh);
                                            }
                                            saving.set(false);
                                            editor_open.set(false);
                                        }
                                        Err(e) => {
                                            saving.set(false);
                                            save_error.set(Some(map_save_error(&e)));
                                        }
                                    }
                                });
                            },
                            if saving_val { "SAVING…" } else { "SAVE JOB" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: saving_val,
                            onclick: move |_| editor_open.set(false),
                            "CANCEL"
                        }
                    }
                }
            }

            if load_error {
                div {
                    style: "color:var(--red);font-size:12px;margin-top:14px;",
                    p { style: "margin:0 0 2px 0;font-weight:700;", "Could not load schedules." }
                    p { style: "margin:0;", "Check the server connection and retry." }
                }
            } else if is_loading {
                div { class: "row-list", style: "margin-top:14px;",
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                    ScheduleGhostRow {}
                }
            } else if schedule_list.is_empty() {
                div {
                    class: "card",
                    style: "align-items:center;text-align:center;padding:32px 18px;margin-top:14px;",
                    div { class: "card-title", "No jobs scheduled." }
                    div { class: "card-meta", style: "margin-top:4px;",
                        "+ NEW JOB to run a prompt on a schedule and deliver the result."
                    }
                }
            } else {
                div { class: "row-list", style: "margin-top:14px;",
                    div { class: "sched-row head",
                        span {}
                        span { "JOB" }
                        span { "SCHEDULE" }
                        span { "DELIVERY" }
                        span { "LAST RUN" }
                        span { style: "text-align:right;", "STATE" }
                        span {}
                    }
                    for row in schedule_list.iter() {
                        ScheduleRowView {
                            key: "{row.id}",
                            schedule: row.clone(),
                            on_edit: move |r: ScheduleRow| {
                                editor_is_new.set(false);
                                editor_id.set(r.id.clone());
                                editor_name.set(r.name.clone());
                                editor_schedule.set(r.schedule_raw.clone());
                                editor_prompt.set(r.prompt.clone());
                                editor_deliver.set(r.deliver.clone());
                                save_error.set(None);
                                editor_open.set(true);
                            },
                            on_delete: move |r: ScheduleRow| {
                                delete_confirm.set(Some(r));
                            },
                            on_toggled: move |fresh: Vec<ScheduleRow>| {
                                schedule_list_sig.set(fresh);
                            },
                            on_run_now: move |fresh: Vec<ScheduleRow>| {
                                schedule_list_sig.set(fresh);
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Ghost placeholder row for the loading state — visually distinct from
/// both the empty panel and a populated row (opacity-dimmed, no data).
#[component]
fn ScheduleGhostRow() -> Element {
    rsx! {
        div {
            class: "sched-row",
            style: "opacity:0.35;",
            "aria-hidden": "true",
            span {}
            div { class: "row-main",
                span { class: "row-title", "…" }
            }
            span { class: "sched-cron", "…" }
            span { class: "row-sub", "…" }
            span { class: "row-sub", "…" }
            span {}
        }
    }
}

#[component]
fn ScheduleRowView(
    schedule: ScheduleRow,
    on_edit: EventHandler<ScheduleRow>,
    on_delete: EventHandler<ScheduleRow>,
    on_toggled: EventHandler<Vec<ScheduleRow>>,
    on_run_now: EventHandler<Vec<ScheduleRow>>,
) -> Element {
    let mut toggling = use_signal(|| false);
    let mut running = use_signal(|| false);
    let toggling_val = *toggling.read();
    let running_val = *running.read();

    let row_for_edit = schedule.clone();
    let row_for_delete = schedule.clone();
    let id_for_toggle = schedule.id.clone();
    let id_for_run = schedule.id.clone();
    let currently_enabled = schedule.enabled;
    let last_run_display = schedule
        .last_run_at
        .clone()
        .unwrap_or_else(|| "—".to_string());

    rsx! {
        div {
            class: "sched-row",
            class: if !schedule.is_valid { "is-invalid" },
            span { style: if schedule.enabled { "color:var(--green);" } else { "color:var(--amber);" }, "●" }
            div { class: "row-main",
                span { class: "row-title", "{schedule.name}" }
                span { class: "row-sub", "—" }
            }
            span { class: "sched-cron", "{schedule.schedule_display}" }
            span { class: "row-sub", "{schedule.deliver}" }
            span { class: "row-sub", "{last_run_display}" }
            div { class: "sched-state",
                if !schedule.is_valid {
                    span { class: "pill amber", "INVALID" }
                } else {
                    span {
                        class: if schedule.enabled { "pill green" } else { "pill amber" },
                        if schedule.enabled { "ACTIVE" } else { "PAUSED" }
                    }
                    div {
                        class: if schedule.enabled { "tgl on" } else { "tgl" },
                        role: "switch",
                        aria_checked: "{schedule.enabled}",
                        "aria-disabled": if toggling_val { "true" } else { "false" },
                        onclick: move |_| {
                            if toggling_val {
                                return;
                            }
                            let id = id_for_toggle.clone();
                            let next_enabled = !currently_enabled;
                            toggling.set(true);
                            spawn(async move {
                                if set_schedule_enabled(id, next_enabled).await.is_ok() {
                                    if let Ok(fresh) = get_schedules().await {
                                        on_toggled.call(fresh);
                                    }
                                }
                                toggling.set(false);
                            });
                        },
                    }
                }
            }
            div { class: "sched-actions",
                button {
                    class: "btn btn--ghost btn--sm",
                    disabled: running_val,
                    onclick: move |_| {
                        if running_val {
                            return;
                        }
                        let id = id_for_run.clone();
                        running.set(true);
                        spawn(async move {
                            if run_schedule_now(id).await.is_ok() {
                                if let Ok(fresh) = get_schedules().await {
                                    on_run_now.call(fresh);
                                }
                            }
                            running.set(false);
                        });
                    },
                    if running_val { "…" } else { "RUN NOW" }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_edit.call(row_for_edit.clone()),
                    "EDIT"
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_delete.call(row_for_delete.clone()),
                    "DELETE"
                }
            }
        }
    }
}
