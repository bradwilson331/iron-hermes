//! Phase 36.3.7.11 Plan 03 — Kanban modal dialogs.
//!
//! Four modals (UI-SPEC §3.10 / §7.2):
//! - `CompleteModal` — Header "Complete task" — summary + metadata
//!   textareas; dismiss "Keep editing".
//! - `BlockModal` — Header "Block task" — reason textarea;
//!   dismiss "Keep editing".
//! - `ArchiveConfirmModal` — Header "Archive task?" — confirm + dismiss
//!   "Keep task" (note: confirm CTA uses default primary styling per §7.2,
//!   NOT --danger).
//! - `CreateTaskModal` — Header "Create task" — title + assignee +
//!   priority segmented + tenant + body + Start in Triage checkbox;
//!   dismiss "Discard task".
//!
//! Shared spec (UI-SPEC §3.10): all modals use a `ModalShell` overlay with
//! `role="dialog"`, `aria-modal="true"`, focus trap, and Escape
//! dismisses (same behavior as the contextual dismiss button).
//!
//! Wire: each modal spawns the relevant Plan 02 `#[server]` write fn from
//! `crate::server::kanban_api`. Submit disabled while a required field
//! is empty (UI-SPEC §4.4). Inline error banner on submit failure.

use crate::protocol::{CreateTaskPayload, KanbanStatus, PromptPayload};
use crate::server::profile_api::list_profiles;
use dioxus::prelude::*;

// ============================================================================
// AssigneePicker — pure helpers (Phase 47.4 Plan 04, D-12)
// ============================================================================

/// DOM id shared by `CreateTaskModal`'s assignee `<input>` `list` attribute
/// and its paired `<datalist>`.
#[allow(dead_code)] // used in CreateTaskModal rsx!; dead_code fires on test target
pub(crate) const KN_ASSIGNEE_DATALIST_ID: &str = "kn-assignee-profiles";

/// Tri-state load status for the known-profile name set, shared by
/// `CreateTaskModal`'s datalist hint and `TaskDrawer`'s unmatched-assignee
/// marker (D-12). `Resolved` carries the actual names so callers can test
/// membership; `Loading`/`Error` intentionally do NOT carry a stale or
/// empty name list — each has its own honest treatment, not a treatment
/// that merely happens to look like zero known profiles.
#[allow(dead_code)] // constructed in CreateTaskModal/TaskDrawer; dead_code fires on test target
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProfileNameLoadState {
    Loading,
    Error,
    Resolved(Vec<String>),
}

/// The exact hint-line copy for a given load state (UI-SPEC Copywriting
/// Contract). A pure fn so the cardinality template and the loading/error
/// backstops are unit-testable without a browser.
#[allow(dead_code)] // used in CreateTaskModal rsx!; dead_code fires on test target
pub(crate) fn assignee_hint_text(state: &ProfileNameLoadState) -> String {
    match state {
        ProfileNameLoadState::Loading => "Loading profiles… · or type any assignee".to_string(),
        ProfileNameLoadState::Error => {
            "Profile list unavailable · or type any assignee".to_string()
        }
        ProfileNameLoadState::Resolved(names) => {
            format!("{} known profiles · or type any assignee", names.len())
        }
    }
}

/// True only when the known-profile set is RESOLVED, the value is
/// non-empty, and the value is absent from the resolved set (D-12,
/// T-47.4-04-R1). An unresolved (loading or error) set never classifies a
/// value as unmatched — the marker is a positive claim that requires
/// evidence — and an empty value is "unassigned", a different thing from
/// "unmatched".
///
/// Phase 47.4 Plan 12 (GAP-3, T-47.4-12-01): membership is compared
/// ASCII-case-insensitively, never a Unicode case fold. Profile directory
/// names are lowercase-only by construction
/// (`ironhermes_core::profile::validate_profile_name`), while assignees are
/// arbitrary free text under D-12, so an assignee differing from a real
/// profile name only in case still names that profile — a Unicode fold
/// would introduce locale-dependent matches the backend would not honour.
#[allow(dead_code)] // used in TaskDrawer rsx!; dead_code fires on test target
pub(crate) fn is_assignee_unmatched(state: &ProfileNameLoadState, assignee: &str) -> bool {
    if assignee.is_empty() {
        return false;
    }
    match state {
        ProfileNameLoadState::Resolved(names) => {
            !names.iter().any(|n| n.eq_ignore_ascii_case(assignee))
        }
        ProfileNameLoadState::Loading | ProfileNameLoadState::Error => false,
    }
}

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
///
/// `initial_assignee`: Phase 47.4 Plan 09 (D-05 + D-12) — seeds the
/// `assignee` signal below with the board PROFILE lens name, if a lens is
/// active when the modal opens. This is an INITIAL value only, read once
/// at mount time (the modal is freshly mounted on every open per
/// `kanban.rs`'s `if *create_modal_open.read() { ... }` gate) — the
/// signal, its `<input>`, and its `oninput` handler are all unchanged
/// from before this plan, so the seeded value stays freely editable.
/// `None` seeds an empty string, exactly as before this plan added the
/// parameter.
#[component]
pub fn CreateTaskModal(
    on_dismiss: EventHandler<()>,
    on_success: EventHandler<()>,
    initial_assignee: Option<String>,
) -> Element {
    let mut title: Signal<String> = use_signal(String::new);
    let mut assignee: Signal<String> =
        use_signal(move || initial_assignee.clone().unwrap_or_default());
    let mut priority: Signal<i64> = use_signal(|| 2);
    let mut tenant: Signal<String> = use_signal(String::new);
    let mut body: Signal<String> = use_signal(String::new);
    let mut start_in_triage: Signal<bool> = use_signal(|| true);
    let mut submitting: Signal<bool> = use_signal(|| false);
    let mut error_msg: Signal<Option<String>> = use_signal(|| None);

    // Phase 47.4 Plan 04 (D-12): known-profile names for the assignee
    // datalist, seeded once into a local working copy — the crate's
    // established fix for the hook-order hazard documented in
    // providers.rs. Never call the resource's restart method after this
    // line; do so and every hook declared afterward desyncs.
    let profiles_resource = use_server_future(list_profiles)?;
    let mut profile_names_sig: Signal<Vec<String>> = use_signal(Vec::new);
    let mut profile_names_seeded: Signal<bool> = use_signal(|| false);
    {
        let loaded = match profiles_resource() {
            Some(Ok(ref rows)) => {
                Some(rows.iter().map(|r| r.name.clone()).collect::<Vec<String>>())
            }
            _ => None,
        };
        use_effect(move || {
            if let Some(ref names) = loaded {
                if !*profile_names_seeded.read() {
                    profile_names_sig.set(names.clone());
                    profile_names_seeded.set(true);
                }
            }
        });
    }

    let title_is_empty = title.read().is_empty();
    let is_submitting = *submitting.read();
    let submit_disabled = title_is_empty || is_submitting;
    let err_text = error_msg.read().clone();
    let pri = *priority.read();
    let start_triage = *start_in_triage.read();

    // Load-state snapshot for the hint + datalist — read before rsx! per
    // signal-borrow discipline (clippy.toml).
    let profile_load_state = match profiles_resource() {
        None => ProfileNameLoadState::Loading,
        Some(Err(_)) => ProfileNameLoadState::Error,
        Some(Ok(_)) => ProfileNameLoadState::Resolved(profile_names_sig.read().clone()),
    };
    let assignee_hint = assignee_hint_text(&profile_load_state);
    let datalist_names: Vec<String> = match &profile_load_state {
        ProfileNameLoadState::Resolved(names) => names.clone(),
        _ => Vec::new(),
    };

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
                list: KN_ASSIGNEE_DATALIST_ID,
                oninput: move |evt| assignee.set(evt.value()),
            }
            datalist { id: KN_ASSIGNEE_DATALIST_ID,
                for name in datalist_names.iter().cloned() {
                    option { key: "{name}", value: "{name}" }
                }
            }
            div { class: "kn-modal-hint kn-modal-hint--info", "{assignee_hint}" }
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

// ============================================================================
// AssigneePicker pure-fn tests (Phase 47.4 Plan 04 Task 3)
// ============================================================================
//
// `iron_hermes_ui` is a bin-only crate (no `src/lib.rs`) — integration
// tests under `tests/` cannot reach `pub(crate)` items in this module (see
// `tests/profile_health.rs`'s identical, already-established note from
// Plan 01). These behavioral tests — including the required mutation
// check — live here; `tests/assignee_picker.rs` locks this module's shape
// and the D-12 negative guarantee via source-string assertions.
#[cfg(test)]
mod assignee_picker_tests {
    use super::*;

    fn resolved(names: &[&str]) -> ProfileNameLoadState {
        ProfileNameLoadState::Resolved(names.iter().map(|s| s.to_string()).collect())
    }

    // --- is_assignee_unmatched -------------------------------------------

    #[test]
    fn matched_value_in_resolved_set_is_not_unmatched() {
        let state = resolved(&["backend-dev", "researcher"]);
        assert!(!is_assignee_unmatched(&state, "backend-dev"));
    }

    #[test]
    fn empty_value_is_never_unmatched() {
        let state = resolved(&["backend-dev"]);
        assert!(!is_assignee_unmatched(&state, ""));
    }

    #[test]
    fn empty_value_is_not_unmatched_even_with_empty_resolved_set() {
        let state = resolved(&[]);
        assert!(!is_assignee_unmatched(&state, ""));
    }

    #[test]
    fn loading_set_never_classifies_as_unmatched() {
        assert!(!is_assignee_unmatched(
            &ProfileNameLoadState::Loading,
            "some-random-person"
        ));
    }

    #[test]
    fn error_set_never_classifies_as_unmatched() {
        assert!(!is_assignee_unmatched(
            &ProfileNameLoadState::Error,
            "some-random-person"
        ));
    }

    #[test]
    fn resolved_nonempty_value_absent_from_set_is_unmatched() {
        let state = resolved(&["backend-dev", "researcher"]);
        assert!(is_assignee_unmatched(&state, "some-random-person"));
    }

    #[test]
    fn resolved_empty_set_with_nonempty_value_is_unmatched() {
        // Zero known profiles — an ad-hoc/CLI-created assignee is still
        // classified as unmatched (D-12: nothing is rewritten, but the
        // marker still fires honestly against an empty known set).
        let state = resolved(&[]);
        assert!(is_assignee_unmatched(&state, "cli-created-worker"));
    }

    // --- assignee_hint_text ------------------------------------------------

    #[test]
    fn hint_at_zero_profiles_reads_locked_copy() {
        let state = resolved(&[]);
        assert_eq!(
            assignee_hint_text(&state),
            "0 known profiles · or type any assignee"
        );
    }

    #[test]
    fn hint_at_one_profile_reads_from_the_same_template() {
        let state = resolved(&["backend-dev"]);
        assert_eq!(
            assignee_hint_text(&state),
            "1 known profiles · or type any assignee"
        );
    }

    #[test]
    fn hint_at_many_profiles_reads_the_live_count() {
        let state = resolved(&["a", "b", "c"]);
        assert_eq!(
            assignee_hint_text(&state),
            "3 known profiles · or type any assignee"
        );
    }

    #[test]
    fn loading_hint_renders_distinct_copy_not_a_count() {
        let text = assignee_hint_text(&ProfileNameLoadState::Loading);
        assert_eq!(text, "Loading profiles… · or type any assignee");
        assert!(!text.contains("known profiles"));
    }

    #[test]
    fn error_hint_renders_distinct_copy_never_the_raw_error() {
        let text = assignee_hint_text(&ProfileNameLoadState::Error);
        assert_eq!(text, "Profile list unavailable · or type any assignee");
        assert!(!text.contains("known profiles"));
    }

    // --- case-insensitive matching (Phase 47.4 Plan 12, GAP-3) -----------

    #[test]
    fn uppercase_assignee_matches_lowercase_profile_name() {
        let state = resolved(&["bdev01"]);
        assert!(!is_assignee_unmatched(&state, "BDEV01"));
    }

    #[test]
    fn mixed_case_assignee_matches_lowercase_profile_name() {
        let state = resolved(&["bdev01"]);
        assert!(!is_assignee_unmatched(&state, "BdEv01"));
    }

    #[test]
    fn genuinely_different_name_is_still_unmatched() {
        let state = resolved(&["bdev01"]);
        assert!(is_assignee_unmatched(&state, "bdev02"));
    }

    #[test]
    fn resolved_state_preserves_enumeration_order_with_no_coercion() {
        // No sorting, casing, or filtering happens between the enumerated
        // profile rows and the Resolved variant's Vec<String> — the
        // hint/predicate fns consume it exactly as given.
        let names = vec!["Zeta-Worker".to_string(), "alpha".to_string()];
        let state = ProfileNameLoadState::Resolved(names.clone());
        match &state {
            ProfileNameLoadState::Resolved(inner) => {
                assert_eq!(inner, &names, "Resolved must preserve exact names and order")
            }
            _ => panic!("expected Resolved"),
        }
    }
}
