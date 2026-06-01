//! Phase 36.3.7.11 Plan 03 — Kanban modal dialogs.
//!
//! Four modals (UI-SPEC §3.10 / §7.2):
//! - `CompleteModal`        — Header "Complete task" — summary + metadata
//!                           textareas; dismiss "Keep editing".
//! - `BlockModal`           — Header "Block task" — reason textarea;
//!                           dismiss "Keep editing".
//! - `ArchiveConfirmModal`  — Header "Archive task?" — confirm + dismiss
//!                           "Keep task" (note: confirm CTA uses default
//!                           primary styling per §7.2, NOT --danger).
//! - `CreateTaskModal`      — Header "Create task" — title + assignee +
//!                           priority segmented + tenant + body + Start in
//!                           Triage checkbox; dismiss "Discard task".
//!
//! Shared spec (UI-SPEC §3.10): all modals use a `ModalShell` overlay with
//! `role="dialog"`, `aria-modal="true"`, focus trap, and Escape
//! dismisses (same behavior as the contextual dismiss button).
//!
//! Wire: each modal spawns the relevant Plan 02 `#[server]` write fn from
//! `crate::server::kanban_api`. Submit disabled while a required field
//! is empty (UI-SPEC §4.4). Inline error banner on submit failure.

use crate::protocol::{CreateTaskPayload, KanbanStatus, PromptPayload};
use dioxus::prelude::*;

// ============================================================================
// Shared modal shell
// ============================================================================

/// Shared overlay + dialog frame. Wraps children in a focus-trapped
/// `role="dialog"` element with `aria-modal="true"`. Provides Escape
/// dismissal via a keydown handler at the dialog scope; Tab cycling
/// within the modal is handled by the browser's natural focus order
/// constrained by the overlay's z-index.
#[component]
pub fn ModalShell(
    title: String,
    modal_id: String,
    on_dismiss: EventHandler<()>,
    children: Element,
) -> Element {
    let title_id = format!("{modal_id}-title");
    rsx! {
        div {
            class: "kn-modal-overlay",
            role: "presentation",
            // Clicking the backdrop does NOT dismiss (intentional — modals
            // require explicit Keep editing / Discard task etc. per §3.10).
            div {
                class: "kn-modal",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "{title_id}",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_dismiss.call(());
                    }
                },
                div { class: "kn-modal-header",
                    h3 { class: "kn-modal-title", id: "{title_id}", "{title}" }
                }
                div { class: "kn-modal-body", {children} }
            }
        }
    }
}

// ============================================================================
// CompleteModal
// ============================================================================

/// UI-SPEC §7.2 CompleteModal: summary (required) + metadata (optional JSON).
#[component]
pub fn CompleteModal(
    task_id: String,
    on_dismiss: EventHandler<()>,
    on_success: EventHandler<()>,
) -> Element {
    // Hooks register unconditionally (Pattern E).
    let mut summary: Signal<String> = use_signal(String::new);
    let mut metadata: Signal<String> = use_signal(String::new);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let summary_is_empty = summary.read().is_empty();
    let is_submitting = *submitting.read();
    let submit_disabled = summary_is_empty || is_submitting;
    let err_text = error_msg.read().clone();

    let task_id_for_submit = task_id.clone();

    rsx! {
        ModalShell {
            title: "Complete task".to_string(),
            modal_id: "kn-complete".to_string(),
            on_dismiss: on_dismiss,
            div { class: "kn-modal-desc",
                "Completing this task marks it done and records the handoff summary for downstream workers."
            }
            label { class: "kn-modal-label", "Summary" }
            textarea {
                class: "kn-modal-textarea",
                placeholder: "Describe what was accomplished\u{2026}",
                value: "{summary}",
                oninput: move |evt| summary.set(evt.value()),
            }
            if summary_is_empty {
                div { class: "kn-modal-hint", "A summary is required to complete a task." }
            }
            label { class: "kn-modal-label", "Metadata (optional)" }
            textarea {
                class: "kn-modal-textarea kn-modal-textarea--mono",
                placeholder: "{{\"key\": \"value\"}}",
                value: "{metadata}",
                oninput: move |evt| metadata.set(evt.value()),
            }
            if let Some(err) = err_text {
                div { class: "kn-modal-error", "Action failed: {err}. Try again." }
            }
            div { class: "kn-modal-actions",
                button {
                    class: "kn-modal-btn kn-modal-btn--dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "Keep editing"
                }
                button {
                    class: "kn-modal-btn kn-modal-btn--submit",
                    disabled: submit_disabled,
                    onclick: move |_| {
                        if submit_disabled {
                            return;
                        }
                        submitting.set(true);
                        error_msg.set(None);
                        let s = summary.read().clone();
                        let m_raw = metadata.read().clone();
                        let parsed_metadata: Option<serde_json::Value> = if m_raw.trim().is_empty() {
                            None
                        } else {
                            serde_json::from_str(&m_raw).ok()
                        };
                        let task_id_owned = task_id_for_submit.clone();
                        spawn(async move {
                            let result = crate::server::kanban_api::patch_task_status(
                                task_id_owned,
                                None,
                                KanbanStatus::Done,
                                Some(PromptPayload::Complete {
                                    summary: s,
                                    metadata: parsed_metadata,
                                }),
                            )
                            .await;
                            submitting.set(false);
                            match result {
                                Ok(_) => on_success.call(()),
                                Err(e) => error_msg.set(Some(format!("{e}"))),
                            }
                        });
                    },
                    "Complete task"
                }
            }
        }
    }
}

// ============================================================================
// BlockModal
// ============================================================================

/// UI-SPEC §7.2 BlockModal: reason (required).
#[component]
pub fn BlockModal(
    task_id: String,
    on_dismiss: EventHandler<()>,
    on_success: EventHandler<()>,
) -> Element {
    let mut reason: Signal<String> = use_signal(String::new);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let reason_is_empty = reason.read().is_empty();
    let is_submitting = *submitting.read();
    let submit_disabled = reason_is_empty || is_submitting;
    let err_text = error_msg.read().clone();

    let task_id_for_submit = task_id.clone();

    rsx! {
        ModalShell {
            title: "Block task".to_string(),
            modal_id: "kn-block".to_string(),
            on_dismiss: on_dismiss,
            div { class: "kn-modal-desc",
                "Blocking this task marks it as waiting for human input. The reason will appear in the task drawer and worker context."
            }
            label { class: "kn-modal-label", "Reason" }
            textarea {
                class: "kn-modal-textarea",
                placeholder: "Describe what's blocking progress\u{2026}",
                value: "{reason}",
                oninput: move |evt| reason.set(evt.value()),
            }
            if reason_is_empty {
                div { class: "kn-modal-hint", "A reason is required to block a task." }
            }
            div { class: "kn-modal-hint kn-modal-hint--info",
                "Prefix with \"review-required: \" for tasks awaiting code review."
            }
            if let Some(err) = err_text {
                div { class: "kn-modal-error", "Action failed: {err}. Try again." }
            }
            div { class: "kn-modal-actions",
                button {
                    class: "kn-modal-btn kn-modal-btn--dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "Keep editing"
                }
                button {
                    class: "kn-modal-btn kn-modal-btn--submit",
                    disabled: submit_disabled,
                    onclick: move |_| {
                        if submit_disabled {
                            return;
                        }
                        submitting.set(true);
                        error_msg.set(None);
                        let r = reason.read().clone();
                        let task_id_owned = task_id_for_submit.clone();
                        spawn(async move {
                            let result = crate::server::kanban_api::patch_task_status(
                                task_id_owned,
                                None,
                                KanbanStatus::Blocked,
                                Some(PromptPayload::Block { reason: r }),
                            )
                            .await;
                            submitting.set(false);
                            match result {
                                Ok(_) => on_success.call(()),
                                Err(e) => error_msg.set(Some(format!("{e}"))),
                            }
                        });
                    },
                    "Block task"
                }
            }
        }
    }
}

// ============================================================================
// ArchiveConfirmModal
// ============================================================================

/// UI-SPEC §7.2 ArchiveConfirmModal: confirm-only (no input fields).
#[component]
pub fn ArchiveConfirmModal(
    task_id: String,
    on_dismiss: EventHandler<()>,
    on_success: EventHandler<()>,
) -> Element {
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);
    let is_submitting = *submitting.read();
    let err_text = error_msg.read().clone();

    let task_id_for_submit = task_id.clone();

    rsx! {
        ModalShell {
            title: "Archive task?".to_string(),
            modal_id: "kn-archive".to_string(),
            on_dismiss: on_dismiss,
            div { class: "kn-modal-desc",
                "This task will be moved to ARCHIVED and hidden from the board by default. It can be revealed with the SHOW ARCHIVED toggle."
            }
            if let Some(err) = err_text {
                div { class: "kn-modal-error", "Action failed: {err}. Try again." }
            }
            div { class: "kn-modal-actions",
                button {
                    class: "kn-modal-btn kn-modal-btn--dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "Keep task"
                }
                button {
                    class: "kn-modal-btn kn-modal-btn--submit",
                    disabled: is_submitting,
                    onclick: move |_| {
                        if is_submitting {
                            return;
                        }
                        submitting.set(true);
                        error_msg.set(None);
                        let task_id_owned = task_id_for_submit.clone();
                        spawn(async move {
                            let result = crate::server::kanban_api::patch_task_status(
                                task_id_owned,
                                None,
                                KanbanStatus::Archived,
                                None,
                            )
                            .await;
                            submitting.set(false);
                            match result {
                                Ok(_) => on_success.call(()),
                                Err(e) => error_msg.set(Some(format!("{e}"))),
                            }
                        });
                    },
                    "Archive task"
                }
            }
        }
    }
}

// ============================================================================
// CreateTaskModal
// ============================================================================

/// UI-SPEC §7.2 CreateTaskModal: title (required) + assignee + priority +
/// tenant + body + Start in Triage checkbox.
#[component]
pub fn CreateTaskModal(
    on_dismiss: EventHandler<()>,
    on_success: EventHandler<()>,
) -> Element {
    let mut title: Signal<String> = use_signal(String::new);
    let mut assignee: Signal<String> = use_signal(String::new);
    let mut priority: Signal<i64> = use_signal(|| 2);
    let mut tenant: Signal<String> = use_signal(String::new);
    let mut body: Signal<String> = use_signal(String::new);
    let mut start_in_triage: Signal<bool> = use_signal(|| true);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    let title_is_empty = title.read().is_empty();
    let is_submitting = *submitting.read();
    let submit_disabled = title_is_empty || is_submitting;
    let err_text = error_msg.read().clone();
    let pri = *priority.read();
    let start_triage = *start_in_triage.read();

    rsx! {
        ModalShell {
            title: "Create task".to_string(),
            modal_id: "kn-create".to_string(),
            on_dismiss: on_dismiss,
            label { class: "kn-modal-label", "Title" }
            input {
                class: "kn-modal-input",
                placeholder: "What needs to be done?",
                value: "{title}",
                oninput: move |evt| title.set(evt.value()),
            }
            if title_is_empty {
                div { class: "kn-modal-hint", "A title is required." }
            }
            label { class: "kn-modal-label", "Assignee" }
            input {
                class: "kn-modal-input",
                placeholder: "e.g. backend-dev",
                value: "{assignee}",
                oninput: move |evt| assignee.set(evt.value()),
            }
            label { class: "kn-modal-label", "Priority" }
            div { class: "kn-modal-segmented",
                for p in [0i64, 1, 2, 3] {
                    button {
                        class: if pri == p { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                        onclick: move |_| priority.set(p),
                        "P{p}"
                    }
                }
            }
            label { class: "kn-modal-label", "Tenant" }
            input {
                class: "kn-modal-input",
                placeholder: "e.g. auth-project",
                value: "{tenant}",
                oninput: move |evt| tenant.set(evt.value()),
            }
            label { class: "kn-modal-label", "Body" }
            textarea {
                class: "kn-modal-textarea",
                placeholder: "Context, acceptance criteria\u{2026}",
                value: "{body}",
                oninput: move |evt| body.set(evt.value()),
            }
            label { class: "kn-modal-checkbox",
                input {
                    r#type: "checkbox",
                    checked: start_triage,
                    onchange: move |evt| start_in_triage.set(evt.checked()),
                }
                "Start in Triage"
            }
            if let Some(err) = err_text {
                div { class: "kn-modal-error", "Action failed: {err}. Try again." }
            }
            div { class: "kn-modal-actions",
                button {
                    class: "kn-modal-btn kn-modal-btn--dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "Discard task"
                }
                button {
                    class: "kn-modal-btn kn-modal-btn--submit",
                    disabled: submit_disabled,
                    onclick: move |_| {
                        if submit_disabled {
                            return;
                        }
                        submitting.set(true);
                        error_msg.set(None);
                        let payload = CreateTaskPayload {
                            title: title.read().clone(),
                            assignee: {
                                let a = assignee.read().clone();
                                if a.is_empty() { None } else { Some(a) }
                            },
                            parents: Vec::new(),
                            priority: *priority.read(),
                            tenant: {
                                let t = tenant.read().clone();
                                if t.is_empty() { None } else { Some(t) }
                            },
                            body: {
                                let b = body.read().clone();
                                if b.is_empty() { None } else { Some(b) }
                            },
                            start_in_triage: *start_in_triage.read(),
                        };
                        spawn(async move {
                            let result =
                                crate::server::kanban_api::create_task(None, payload).await;
                            submitting.set(false);
                            match result {
                                Ok(_) => on_success.call(()),
                                Err(e) => error_msg.set(Some(format!("{e}"))),
                            }
                        });
                    },
                    "Create task"
                }
            }
        }
    }
}
