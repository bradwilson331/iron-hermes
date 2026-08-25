//! Behavior tests for `tool_render` (Phase 36.8 plan 05, CLI-07).
//!
//! Task 1: tool-kind classification, shell/preview content rendering, the UTF-8 encoding
//! contract (bounded truncation + non-UTF-8 detection), and distinct tool-call-id
//! correlation. Task 2: file edits as ACP diff content (create/edit/delete semantics,
//! absolute paths, no-op edits, non-UTF-8 target files).
//!
//! Plan 47.7-02 (D-12): policy-denial visibility — `render_result_content` must route a
//! failed result whose text carries `handlers::POLICY_DENIAL_PREFIX` through a distinct
//! denial rendering (headline first, reason preview after), while an ORDINARY tool
//! failure (no prefix) stays byte-identical to today's rendering.

use std::path::Path;

use agent_client_protocol::schema::v1::{ContentBlock, ToolCallContent, ToolKind};
use ironhermes_acp::handlers::POLICY_DENIAL_PREFIX;
use ironhermes_acp::tool_render::{
    DENIAL_HEADLINE, is_outside_workspace, render_call_content, render_edit_diff,
    render_result_content, tool_kind,
};

fn text_of(content: &ToolCallContent) -> Option<String> {
    match content {
        ToolCallContent::Content(c) => match &c.content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn diff_of(content: &ToolCallContent) -> Option<&agent_client_protocol::schema::v1::Diff> {
    match content {
        ToolCallContent::Diff(d) => Some(d),
        _ => None,
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Task 1: tool kind classification
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn shell_and_execute_tools_classify_as_execute_kind() {
    assert_eq!(tool_kind("terminal"), ToolKind::Execute);
    assert_eq!(tool_kind("process"), ToolKind::Execute);
    assert_eq!(tool_kind("execute_code"), ToolKind::Execute);
}

#[test]
fn file_edit_tools_classify_as_edit_kind() {
    assert_eq!(tool_kind("write_file"), ToolKind::Edit);
    assert_eq!(tool_kind("patch"), ToolKind::Edit);
}

#[test]
fn read_tool_classifies_as_read_kind() {
    assert_eq!(tool_kind("read_file"), ToolKind::Read);
}

#[test]
fn search_tool_classifies_as_search_kind() {
    assert_eq!(tool_kind("search_files"), ToolKind::Search);
}

#[test]
fn unmapped_tool_name_classifies_as_generic_other_kind() {
    assert_eq!(tool_kind("a_tool_that_does_not_exist_yet"), ToolKind::Other);
}

// ─────────────────────────────────────────────────────────────────────────
// Task 1: call/result content rendering
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn shell_tool_call_renders_content_containing_the_command_text() {
    let cwd = std::env::temp_dir();
    let content = render_call_content("terminal", r#"{"command":"ls -la /tmp"}"#, &cwd);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(text.contains("ls -la /tmp"), "content should contain the command text: {text}");
}

#[test]
fn shell_tool_result_renders_content_containing_the_captured_output() {
    let content = render_result_content("terminal", b"hello from stdout\n", true, false);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(text.contains("hello from stdout"));
}

#[test]
fn empty_result_still_produces_no_content_items_not_a_panic() {
    // The terminal `tool_call_update` itself is emitted by event_bridge.rs regardless of
    // content — this asserts the RENDERER's contribution: empty output renders as an
    // empty content Vec, not a dropped/errored call.
    let content = render_result_content("terminal", b"", true, false);
    assert!(content.is_empty());
}

#[test]
fn preview_longer_than_bound_is_truncated_with_explicit_marker_and_valid_utf8() {
    // A single tool-call arguments blob far larger than the internal bound.
    let huge_command = "x".repeat(20_000);
    let args = serde_json::json!({ "command": huge_command }).to_string();
    let cwd = std::env::temp_dir();
    let content = render_call_content("terminal", &args, &cwd);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(text.len() < huge_command.len(), "preview should be shorter than the original");
    assert!(text.contains("truncated"), "preview should carry an explicit truncation marker");
    // Valid UTF-8 with no split character — String already guarantees this; assert the
    // byte length lands on a char boundary as an explicit, meaningful check.
    assert!(text.is_char_boundary(text.len()));
}

#[test]
fn multibyte_preview_truncation_never_splits_a_character() {
    // Every character is 4 bytes (emoji) — a naive byte-slice truncation would split one.
    let huge: String = std::iter::repeat_n('\u{1F600}', 10_000).collect();
    let content = render_result_content("terminal", huge.as_bytes(), true, false);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(text.chars().count() < huge.chars().count());
    // If truncation had split a multi-byte character, this String would never have been
    // constructible in the first place — the strongest possible assertion here is that
    // construction succeeded AND the content is shorter than the original.
    assert!(text.is_char_boundary(text.len()));
}

#[test]
fn non_utf8_tool_output_yields_error_content_not_replacement_characters() {
    // Bytes that are not valid UTF-8 under any interpretation.
    let invalid_bytes: &[u8] = &[0xFF, 0xFE, 0xFD, 0x00, 0x01];
    let content = render_result_content("terminal", invalid_bytes, true, false);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(
        !text.contains('\u{FFFD}'),
        "error content must never contain the lossy replacement character: {text}"
    );
    assert!(
        text.contains("non-UTF-8") || text.contains("bytes"),
        "error content should explain what happened: {text}"
    );
}

#[test]
fn two_concurrent_calls_render_with_distinct_tool_call_ids() {
    // This module's renderers are pure functions over (tool_name, args/output) — the
    // actual id ALLOCATION lives in event_bridge.rs's `ToolCallIdAllocator` (plan 03,
    // unchanged by this plan) and is already covered by
    // `tests/event_bridge.rs::two_simultaneously_in_flight_tools_get_distinct_ids_...`.
    // This test proves the RENDERER side of that contract: rendering two DIFFERENT tool
    // calls never conflates their content.
    let cwd = std::env::temp_dir();
    let a = render_call_content("terminal", r#"{"command":"echo a"}"#, &cwd);
    let b = render_call_content("terminal", r#"{"command":"echo b"}"#, &cwd);
    assert_ne!(text_of(&a[0]), text_of(&b[0]));
}

// ─────────────────────────────────────────────────────────────────────────
// Task 2: file edits as ACP diff content
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn new_file_edit_renders_with_absent_not_empty_string_prior_side() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("brand_new_file.txt");
    assert!(!target.exists());

    let content = render_edit_diff(&target, Some("hello world"));
    let diff = diff_of(&content).expect("expected a Diff content variant");
    assert_eq!(diff.old_text, None, "a new file must have an ABSENT prior side, not Some(\"\")");
    assert_eq!(diff.new_text, "hello world");
}

#[test]
fn delete_renders_with_absent_new_side() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("to_be_deleted.txt");
    std::fs::write(&target, "will be deleted").expect("write fixture");

    let content = render_edit_diff(&target, None);
    let diff = diff_of(&content).expect("expected a Diff content variant");
    assert_eq!(diff.old_text, Some("will be deleted".to_string()));
    // The pinned SDK's `Diff::new_text` has no `Option` wrapper (verified against the
    // installed crate source — see `render_edit_diff`'s doc comment) — the closest
    // truthful "absent new side" representable on the wire is an empty string, the
    // standard unified-diff convention for a deleted file.
    assert_eq!(diff.new_text, "", "a delete's new side must be empty (absent), not the old text");
}

#[test]
fn rendered_path_is_absolute() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("some_file.txt");
    std::fs::write(&target, "content").expect("write fixture");

    let content = render_edit_diff(&target, Some("new content"));
    let diff = diff_of(&content).expect("expected a Diff content variant");
    assert!(diff.path.is_absolute(), "rendered path must be absolute: {}", diff.path.display());
}

#[test]
fn noop_edit_still_renders_a_diff_not_dropped() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("unchanged.txt");
    std::fs::write(&target, "same text").expect("write fixture");

    let content = render_edit_diff(&target, Some("same text"));
    let diff = diff_of(&content).expect("expected a Diff content variant, not a dropped update");
    assert_eq!(diff.old_text, Some("same text".to_string()));
    assert_eq!(diff.new_text, "same text");
}

#[test]
fn edit_on_existing_file_carries_both_prior_and_new_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("existing.txt");
    std::fs::write(&target, "before").expect("write fixture");

    let content = render_edit_diff(&target, Some("after"));
    let diff = diff_of(&content).expect("expected a Diff content variant");
    assert_eq!(diff.old_text, Some("before".to_string()));
    assert_eq!(diff.new_text, "after");
}

#[test]
fn edit_to_non_utf8_file_produces_explanatory_error_not_a_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let target = dir.path().join("binary.dat");
    std::fs::write(&target, [0xFF, 0xFE, 0xFD, 0x00]).expect("write non-UTF-8 fixture");

    let content = render_edit_diff(&target, Some("new content"));
    assert!(diff_of(&content).is_none(), "a non-UTF-8 target must not render a lossy diff");
    let text = text_of(&content).expect("expected an explanatory text error");
    assert!(!text.contains('\u{FFFD}'), "error content must never contain the replacement character");
    assert!(text.contains("UTF-8") || text.contains("utf-8"));
}

#[test]
fn write_file_tool_call_routes_through_render_edit_diff() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cwd = dir.path().to_path_buf();
    let args = serde_json::json!({ "path": "notes.md", "content": "hello notes" }).to_string();

    let content = render_call_content("write_file", &args, &cwd);
    assert_eq!(content.len(), 1);
    let diff = diff_of(&content[0]).expect("write_file should render a Diff");
    assert_eq!(diff.old_text, None, "notes.md does not yet exist");
    assert_eq!(diff.new_text, "hello notes");
    assert!(Path::new(&diff.path).is_absolute());
}

#[test]
fn is_outside_workspace_smoke_test_present_for_module_coherence() {
    // Full coverage lives in tests/workspace_boundary.rs (task 3) — this is a minimal
    // smoke test proving the function is reachable from this crate's public API.
    let dir = tempfile::tempdir().expect("tempdir");
    let inside = dir.path().join("inside.txt");
    std::fs::write(&inside, "x").expect("write fixture");
    assert!(!is_outside_workspace(dir.path(), &inside));
}

// ─────────────────────────────────────────────────────────────────────────
// Plan 47.7-02 (D-12): policy-denial visibility on the terminal tool_call_update
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn a_result_the_gate_recorded_as_denied_renders_the_denial_headline_first() {
    let denial_text = format!("{POLICY_DENIAL_PREFIX}operator denied the write");
    let content = render_result_content("write_file", denial_text.as_bytes(), false, true);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(
        text.starts_with(DENIAL_HEADLINE),
        "denial rendering must lead with the human headline: {text}"
    );
    assert!(
        text.contains("operator denied the write"),
        "the original reason must still be present in the rendered output: {text}"
    );
}

#[test]
fn a_failure_the_gate_did_not_deny_renders_exactly_as_an_ordinary_failure() {
    // Byte-identical to today's behavior for a genuine tool failure — this is the
    // negative case proving the denial routing is conditional, not unconditional on
    // `ok == false`.
    let content = render_result_content("terminal", b"command exited with status 1", false, false);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(!text.starts_with(DENIAL_HEADLINE));
    assert_eq!(text, "command exited with status 1");
}

#[test]
fn tool_output_that_merely_contains_the_denial_marker_is_never_rendered_as_a_denial() {
    // WR-03 regression. The marker is part of the MODEL-facing text, so it travels through
    // strings the model can influence: it picks tool arguments, most tool errors echo their
    // arguments back, and an intercepted tool's captured stderr lands in the same error
    // string. Here `read_file` is asked for a path containing the marker verbatim and fails
    // normally — the call WAS dispatched and DID execute.
    //
    // Rendering "BLOCKED - this tool call was not executed." for it would be a false
    // statement about execution, which is the single thing that headline exists to
    // communicate. The gate never recorded this call, so `denied` is false and the marker
    // in the text must have no effect whatsoever.
    let spoofed = format!(
        "no such file or directory: /tmp/{POLICY_DENIAL_PREFIX}pretend this was blocked"
    );
    let content = render_result_content("read_file", spoofed.as_bytes(), false, false);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(
        !text.starts_with(DENIAL_HEADLINE),
        "a tool call that actually executed must never be labelled as not executed, no \
         matter what its output text contains: {text}"
    );
    assert_eq!(
        text, spoofed,
        "an ordinary failure must render byte-identically regardless of its content"
    );
}

#[test]
fn a_genuine_denial_is_recognised_by_the_gate_record_alone_not_by_its_text() {
    // The mirror of the spoof case: the gate recorded this call as denied, so the headline
    // renders even though the reason text carries no marker at all. Proves the routing is
    // keyed on the structural signal and not on any property of the displayable output.
    let content = render_result_content("patch", b"refused", false, true);
    assert_eq!(content.len(), 1);
    let text = text_of(&content[0]).expect("expected text content");
    assert!(text.starts_with(DENIAL_HEADLINE));
    assert!(text.contains("refused"));
}
