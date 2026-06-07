//! Phase 36.3.7.11 Plan 03 — Drawer + modals source-string assertions.
//!
//! Locks the D-20 drawer-section labels (UI-SPEC §7.6) in canonical order,
//! the 4 modal headers (UI-SPEC §7.2), and the contextual modal dismiss
//! labels (UI-SPEC §7.1) — with explicit prohibition of a generic
//! `"Cancel"` button.
//!
//! Pattern: source-read assertion (mirrors `kanban_dnd_wiring.rs` from
//! Plan 02 / `websocket_lifecycle_parity.rs` baseline).

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ===========================================================================
// drawer.rs — D-20 / UI-SPEC §7.6 section labels in order
// ===========================================================================

#[test]
fn drawer_source_contains_all_d20_section_labels() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    // The drawer source MUST contain each UI-SPEC §7.6 label as a literal
    // string. (Section *render* order is separately enforced by
    // `drawer_source_renders_d20_sections_in_order` below, which checks
    // the sub-component call sites in the main TaskDrawer body.)
    let expected_labels = [
        "WORKER CONTEXT",
        "PARENT HANDOFFS",
        "PRIOR ATTEMPTS",
        "RUN HISTORY",
        "EVENTS (last 20)",
        "COMMENTS",
    ];
    for label in expected_labels {
        assert!(
            src.contains(label),
            "D-20 / UI-SPEC §7.6: drawer.rs missing literal label `{}`",
            label
        );
    }
}

#[test]
fn drawer_source_renders_d20_sections_in_order() {
    // D-20: the SECTION RENDER ORDER is established by the call sites in
    // the main TaskDrawer body (sub-components encapsulate the labels).
    // We assert that the sub-component invocations appear in canonical
    // D-20 order: WorkerContextBlock → RunHistorySection →
    // EventStreamSection → CommentList (the COMMENTS label appears
    // immediately before CommentList in the COMMENTS section).
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    let order = [
        "WorkerContextBlock {",
        "RunHistorySection {",
        "EventStreamSection {",
        "CommentList {",
    ];
    let mut last_pos: Option<usize> = None;
    for name in order {
        let pos = src.find(name).unwrap_or_else(|| {
            panic!("D-20: drawer.rs missing sub-component call `{}`", name)
        });
        if let Some(prev) = last_pos {
            assert!(
                pos > prev,
                "D-20: sub-component `{}` must appear AFTER previous \
                 component in canonical D-20 render order",
                name
            );
        }
        last_pos = Some(pos);
    }
}

#[test]
fn drawer_source_declares_aria_modal_false() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("\"aria-modal\": \"false\""),
        "UI-SPEC §3.9: drawer is a `role=\"complementary\"` panel, NOT a \
         dialog — must declare `aria-modal: \"false\"`"
    );
}

#[test]
fn drawer_source_carries_close_button_aria_label() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("Close task detail"),
        "UI-SPEC §7.1: drawer close button must carry `aria-label: \
         \"Close task detail\"`"
    );
}

#[test]
fn drawer_source_uses_four_use_resource_calls() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    let count = src.matches("use_resource").count();
    assert!(
        count >= 4,
        "D-20 / D-21: drawer must register 4 use_resource calls (task / \
         runs / events / comments) — found {}",
        count
    );
}

#[test]
fn drawer_source_reads_per_task_event_counter_for_d21() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("per_task_event_counter"),
        "D-21: drawer must read `per_task_event_counter` inside its \
         resources for dependency tracking (re-fetch on event)"
    );
}

#[test]
fn drawer_source_declares_role_complementary() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("role: \"complementary\""),
        "UI-SPEC §3.9 / §6.1: drawer must declare `role: \"complementary\"`"
    );
}

#[test]
fn drawer_source_handles_escape_key_to_close() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("Key::Escape"),
        "UI-SPEC §3.9 / §6.2: drawer must intercept Escape to close"
    );
}

#[test]
fn drawer_source_renders_run_outcome_badges() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");
    assert!(
        src.contains("\"data-outcome\""),
        "UI-SPEC §7.7: RunHistorySection must emit `data-outcome` attribute \
         for the outcome badge CSS color rules"
    );
}

// ===========================================================================
// modals.rs — UI-SPEC §7.2 modal headers + §7.1 contextual dismiss labels
// ===========================================================================

#[test]
fn modals_source_contains_four_modal_headers() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    let expected_headers = [
        "Complete task",
        "Block task",
        "Archive task?",
        "Create task",
    ];
    for header in expected_headers {
        assert!(
            src.contains(header),
            "UI-SPEC §7.2: modals.rs must contain modal header `{}`",
            header
        );
    }
}

#[test]
fn modals_source_contains_contextual_dismiss_labels() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    // UI-SPEC §7.1 / §7.2 mandates contextual dismiss copy — NOT a generic
    // "Cancel" — across the four modals.
    let expected_dismiss = ["Keep editing", "Keep task", "Discard task"];
    for label in expected_dismiss {
        assert!(
            src.contains(label),
            "UI-SPEC §7.1: modals.rs must contain contextual dismiss label `{}`",
            label
        );
    }
}

#[test]
fn modals_source_has_no_generic_cancel_label() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    // Walk word boundaries: forbid the literal whole-word "Cancel". The
    // substring "cancel" is allowed inside other words (e.g.
    // "cancellation") in case future copy needs it. We split on
    // non-alphanumeric characters and look for an exact match.
    for token in src.split(|c: char| !c.is_alphanumeric()) {
        assert!(
            token != "Cancel",
            "UI-SPEC §7.1: modals.rs MUST NOT contain a generic `Cancel` \
             label — use contextual dismiss labels instead (Keep editing / \
             Keep task / Discard task)"
        );
    }
}

#[test]
fn modals_source_declares_aria_modal_true() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("aria_modal: \"true\""),
        "UI-SPEC §3.10: modals are `role=\"dialog\"` — must declare \
         `aria-modal: \"true\"`"
    );
}

#[test]
fn modals_source_declares_role_dialog() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("role: \"dialog\""),
        "UI-SPEC §3.10 / §6.1: modals must declare `role: \"dialog\"`"
    );
}

#[test]
fn modals_source_complete_calls_patch_status_with_prompt_complete() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("PromptPayload::Complete"),
        "D-13: CompleteModal must invoke patch_task_status with \
         `PromptPayload::Complete`"
    );
}

#[test]
fn modals_source_block_calls_patch_status_with_prompt_block() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("PromptPayload::Block"),
        "D-13: BlockModal must invoke patch_task_status with \
         `PromptPayload::Block`"
    );
}

#[test]
fn modals_source_archive_calls_patch_status_archived() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("KanbanStatus::Archived"),
        "D-13: ArchiveConfirmModal must invoke patch_task_status with \
         `KanbanStatus::Archived`"
    );
}

#[test]
fn modals_source_create_calls_create_task() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    assert!(
        src.contains("create_task("),
        "D-13: CreateTaskModal must invoke the `create_task` server fn"
    );
}

#[test]
fn modals_source_required_field_error_messages_verbatim() {
    let src = read("src/components/hermes_app/screens/kanban/modals.rs");
    // UI-SPEC §4.4 verbatim required-field error copy.
    let expected_errors = [
        "A summary is required to complete a task.",
        "A reason is required to block a task.",
        "A title is required.",
    ];
    for err in expected_errors {
        assert!(
            src.contains(err),
            "UI-SPEC §4.4: modals.rs must contain verbatim required-field \
             error message: `{}`",
            err
        );
    }
}

// ===========================================================================
// card.rs — D-12 TRIAGE buttons reach the action handler
// ===========================================================================

#[test]
fn card_source_has_triage_decompose_button() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        src.contains("Decompose"),
        "D-12 / UI-SPEC §3.7: TRIAGE cards must render a `Decompose` button"
    );
}

#[test]
fn card_source_has_triage_specify_button() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    assert!(
        src.contains("Specify"),
        "D-12 / UI-SPEC §3.7: TRIAGE cards must render a `Specify` button"
    );
}

#[test]
fn card_source_calls_on_triage_action_with_decompose_or_specify() {
    let src = read("src/components/hermes_app/screens/kanban/card.rs");
    // D-12 contract: card emits the action via `on_triage_action` —
    // screen-side dispatches `run_decompose_or_specify` (the same write
    // fn the drawer's TriageActionRow uses).
    assert!(
        src.contains("on_triage_action"),
        "D-12: card.rs must declare an `on_triage_action` EventHandler prop"
    );
    assert!(
        src.contains("DecomposeOrSpecify"),
        "D-12: card.rs must import + use the `DecomposeOrSpecify` enum"
    );
}

// ===========================================================================
// kanban.rs — drawer mount + modal-target signals + per_task_event_counter
// ===========================================================================

#[test]
fn screen_source_renders_task_drawer() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        src.contains("TaskDrawer {"),
        "Plan 03: ScreenKanban must mount the TaskDrawer component"
    );
}

#[test]
fn screen_source_declares_per_task_event_counter_signal() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    assert!(
        src.contains("per_task_event_counter"),
        "D-21: ScreenKanban must declare a `per_task_event_counter` \
         signal that the drawer consumes for refresh-on-event"
    );
}

#[test]
fn screen_source_declares_modal_target_signals() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    for sig in [
        "complete_modal_task",
        "block_modal_task",
        // archive_modal_task already exists from Plan 02
        "create_modal_open",
    ] {
        assert!(
            src.contains(sig),
            "D-13: ScreenKanban must declare `{}` signal so the drawer \
             modal-open events resolve to modal renders",
            sig,
        );
    }
}

#[test]
fn screen_source_mounts_all_four_modals() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    for modal in [
        "CompleteModal {",
        "BlockModal {",
        "ArchiveConfirmModal {",
        "CreateTaskModal {",
    ] {
        assert!(
            src.contains(modal),
            "D-13: ScreenKanban must mount `{}` so the modal renders \
             when its target signal is set",
            modal,
        );
    }
}

#[test]
fn screen_source_has_200ms_debounce_for_per_task_counter() {
    let src = read("src/components/hermes_app/screens/kanban.rs");
    // UI-SPEC §8.4: 200ms debounce on the per-task event counter.
    assert!(
        src.contains("200"),
        "UI-SPEC §8.4 / D-21: ScreenKanban WS handler must apply a 200ms \
         debounce on per-task-event-counter increments"
    );
}

// ===========================================================================
// Quick task 260602-nd7 (U9 fix) — bilateral consumer-end regression.
//
// The coarse `drawer_source_reads_per_task_event_counter_for_d21` test at
// line ~115 asserts the literal `per_task_event_counter` substring appears
// SOMEWHERE in drawer.rs. With four use_resource calls (task / runs / events
// / comments), three could read the counter and one — the comments
// resource — could omit it, and the coarse test would still pass. That is
// exactly the U9 failure mode if the producer fix were absent: a regressor
// could remove `let _counter = per_task_event_counter();` from the comments
// closure and ship without triggering any test. This fine-grained test
// localizes the assertion to the comments resource's closure span so a
// future regression on the consumer end is caught.
//
// Implementation: locate the byte offset of `"let comments = use_resource"`
// and the offset of `"fetch_comments"` (which appears INSIDE that closure
// per drawer.rs:111). Assert `per_task_event_counter()` appears in the
// source slice between those offsets — i.e. inside the comments resource
// closure body specifically, not just anywhere in the file.
// ===========================================================================

#[test]
fn comments_resource_reads_per_task_event_counter_for_d21() {
    let src = read("src/components/hermes_app/screens/kanban/drawer.rs");

    let comments_decl_offset = src
        .find("let comments = use_resource")
        .expect(
            "drawer.rs must declare a `let comments = use_resource(...)` \
             resource — the COMMENTS section of the D-20 drawer depends on \
             it; see .planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/",
        );
    let fetch_comments_offset = src
        .find("fetch_comments")
        .expect(
            "drawer.rs must call `kanban_api::fetch_comments` inside the \
             comments resource closure — see .planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/",
        );

    assert!(
        fetch_comments_offset > comments_decl_offset,
        "drawer.rs: `fetch_comments` call (offset={}) must appear AFTER the \
         `let comments = use_resource` declaration (offset={}) — the \
         comments resource closure body wraps the fetch call; see .planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/",
        fetch_comments_offset,
        comments_decl_offset,
    );

    let comments_closure_span = &src[comments_decl_offset..fetch_comments_offset];
    assert!(
        comments_closure_span.contains("per_task_event_counter()"),
        "D-21 / U9 bilateral lock: the `comments` use_resource closure body \
         (the slice from `let comments = use_resource` to the `fetch_comments` \
         call site) MUST call `per_task_event_counter()` so Dioxus restarts \
         the resource when the counter bumps. The coarse \
         `drawer_source_reads_per_task_event_counter_for_d21` test at line \
         ~115 only checks the substring appears somewhere in the file — it \
         passes even if the comments resource specifically omits the read. \
         This test catches that regression. \
         See .planning/quick/260602-nd7-fix-u9-drawer-comments-auto-refresh/260602-nd7-PLAN.md."
    );
}
