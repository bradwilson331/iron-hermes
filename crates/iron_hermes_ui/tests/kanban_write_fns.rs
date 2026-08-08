//! Phase 36.3.7.11 Plan 02 — Source-string locks for the four write-side
//! `#[server]` fns in `kanban_api.rs`.
//!
//! D-13: four write fns exist with the exact signatures from CONTEXT.md.
//! D-14: server-side transition validation reuses
//!       `kanban::transitions::is_drag_allowed`.
//! D-19: every fn takes `board: Option<String>`.
//! Q9 BRANCH: a comment naming the chosen branch + file:line of the
//!            decomposer kernel signature consulted is present.
//! Security: no `format!` calls embedded into SQL.
//!
//! Pattern: source-read assertion (mirrors `kanban_server_fns.rs`).
//! Runs without the `server` feature; the binary crate is not importable
//! externally.

use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

// ---------------------------------------------------------------------------
// D-13 — four write fn names exist
// ---------------------------------------------------------------------------

#[test]
fn kanban_api_declares_the_four_write_fns() {
    let src = read("src/server/kanban_api.rs");
    for name in [
        "patch_task_status",
        "post_comment",
        "create_task",
        "run_decompose_or_specify",
    ] {
        assert!(
            src.contains(&format!("fn {}", name)),
            "D-13: kanban_api.rs must declare `fn {}`",
            name,
        );
    }
}

// ---------------------------------------------------------------------------
// D-19 — `board: Option<String>` appears at minimum 4 + 5 = 9 times (Plan 01
// already had 5 fns; Task 2 adds 4 more). Allow ≥ 9 to be tolerant of
// future additions.
// ---------------------------------------------------------------------------

#[test]
fn kanban_api_every_write_fn_takes_board_option_string() {
    let src = read("src/server/kanban_api.rs");
    let board_param_count = src.matches("board: Option<String>").count();
    assert!(
        board_param_count >= 9,
        "D-19: every #[server] fn must take `board: Option<String>` — found {} occurrences (need ≥ 9: 5 reads from Plan 01 + 4 writes from Plan 02)",
        board_param_count,
    );
}

// ---------------------------------------------------------------------------
// D-14 — server-side validation: is_drag_allowed is imported AND called in
// patch_task_status.
// ---------------------------------------------------------------------------

#[test]
fn kanban_api_imports_is_drag_allowed_for_server_side_validation() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("use crate::kanban::transitions::is_drag_allowed"),
        "D-14: kanban_api.rs must `use crate::kanban::transitions::is_drag_allowed` \
         to enforce the same allowed table on the server"
    );
}

#[test]
fn patch_task_status_calls_is_drag_allowed() {
    let src = read("src/server/kanban_api.rs");
    // The fn body must reference `is_drag_allowed` at least once — proves
    // the import is actually consumed.
    let occurrences = src.matches("is_drag_allowed").count();
    assert!(
        occurrences >= 2,
        "D-14: patch_task_status body must call is_drag_allowed (the `use` line + \
         at least one call site = ≥ 2 occurrences); found {}",
        occurrences,
    );
}

// ---------------------------------------------------------------------------
// Security: no `format!` calls embedded into SQL keywords.
// ---------------------------------------------------------------------------

#[test]
fn kanban_api_has_no_format_sql_injection_vectors() {
    let src = read("src/server/kanban_api.rs");
    // Strip line comments before scanning.
    let stripped: String = src
        .lines()
        .map(|l| {
            if let Some(ix) = l.find("//") {
                &l[..ix]
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Scan for `format!("...<KEYWORD>` directly — the SQL keyword must
    // appear inside the format string literal of a `format!(` call, not
    // somewhere on the same line as an unrelated format!. We pin the
    // detection to the first 80 characters AFTER `format!(` which covers
    // a typical query literal opening.
    for sql_kw in ["SELECT", "INSERT", "UPDATE", "DELETE"] {
        // Pattern: literal start of a format string that begins with a
        // SQL keyword (no leading whitespace inside the string before it).
        let pat_a = format!("format!(\"{}", sql_kw);
        let pat_b = format!("format!(\" {}", sql_kw);
        let pat_c = format!("format!(r#\"{}", sql_kw);
        assert!(
            !stripped.contains(&pat_a) && !stripped.contains(&pat_b) && !stripped.contains(&pat_c),
            "Security: kanban_api.rs has a `format!(...)` whose literal starts with \
             SQL keyword `{}`: use rusqlite::params! instead",
            sql_kw,
        );
    }
}

// ---------------------------------------------------------------------------
// Q9 — the executor must document which decompose branch was chosen at the
// call site (branch a wires decomposer.rs OR branch b returns NotWired).
// The comment must contain the string `Q9 BRANCH` and reference the
// decomposer.rs file.
// ---------------------------------------------------------------------------

#[test]
fn run_decompose_or_specify_documents_q9_branch_choice() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("Q9 BRANCH"),
        "Q9: run_decompose_or_specify must include a `Q9 BRANCH (a)` or `Q9 BRANCH (b)` \
         comment documenting the choice and the decomposer.rs file:line consulted"
    );
    assert!(
        src.contains("decomposer.rs"),
        "Q9: the Q9 branch comment must name the kernel signature file (decomposer.rs)"
    );
}

#[test]
fn run_decompose_or_specify_returns_a_decompose_result() {
    let src = read("src/server/kanban_api.rs");
    // One of the two return branches must appear.
    let has_ok = src.contains("DecomposeResult::Ok");
    let has_not_wired = src.contains("DecomposeResult::NotWired");
    assert!(
        has_ok || has_not_wired,
        "Q9: run_decompose_or_specify must construct DecomposeResult::Ok (branch a) \
         OR DecomposeResult::NotWired (branch b)"
    );
}

// ---------------------------------------------------------------------------
// D-13 PromptPayload routing: Complete/Block branches mentioned.
// ---------------------------------------------------------------------------

#[test]
fn patch_task_status_references_prompt_payload_variants() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("PromptPayload::Complete"),
        "D-13: patch_task_status must handle PromptPayload::Complete for Done transitions"
    );
    assert!(
        src.contains("PromptPayload::Block"),
        "D-13: patch_task_status must handle PromptPayload::Block for Blocked transitions"
    );
}

// ---------------------------------------------------------------------------
// D-14 / Risk 8: empty summary / empty reason is rejected.
// ---------------------------------------------------------------------------

#[test]
fn patch_task_status_rejects_empty_summary_and_empty_reason() {
    let src = read("src/server/kanban_api.rs");
    // The fn body should explicitly check `.is_empty()` on either summary
    // or reason for the Done / Blocked branches.
    assert!(
        src.contains("is_empty()"),
        "D-14 / Risk 8: patch_task_status must reject empty summary / empty reason \
         (look for `.is_empty()` guard)"
    );
}

// ---------------------------------------------------------------------------
// Phase 46.3 Plan 01 — D-01 / D-02 source-string regression locks
// ---------------------------------------------------------------------------

/// Phase 46.3 Plan 01 (D-01 lock): `run_decompose_or_specify` must dispatch
/// to the real kernel fns rather than reverting to the `NotWired`-only stub.
#[test]
fn run_decompose_or_specify_invokes_real_kernel_fns() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("decompose_triage_task("),
        "D-01: kanban_api.rs must call decompose_triage_task("
    );
    assert!(
        src.contains("specify_triage_task("),
        "D-01: kanban_api.rs must call specify_triage_task("
    );
}

/// Phase 46.3 Plan 01 (D-02 lock): the ported three-tier model cascade +
/// preserved CLI bail message must remain present in `kanban_api.rs`.
#[test]
fn run_decompose_or_specify_mirrors_cli_three_tier_cascade() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("decomposer_model"),
        "D-02: kanban_api.rs must reference `decomposer_model` (Tier 1 cascade)"
    );
    assert!(
        src.contains("kanban_decomposer"),
        "D-02: kanban_api.rs must reference `kanban_decomposer` (Tier 2 role cascade)"
    );
    assert!(
        src.contains("decomposer model not configured"),
        "D-02: kanban_api.rs must preserve the CLI's actionable bail message \
         substring `decomposer model not configured`"
    );
}

// ---------------------------------------------------------------------------
// Phase 46.4 Plan 06 — D-03/D-04/D-10/D-19 source-string regression locks
// ---------------------------------------------------------------------------

/// D-03/D-04: `attach_file` and `fetch_attachments` #[server] fns exist and
/// take `board: Option<String>` (D-19).
#[test]
fn kanban_api_declares_attach_file_and_fetch_attachments() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("fn attach_file"),
        "D-03/D-04: kanban_api.rs must declare `fn attach_file`"
    );
    assert!(
        src.contains("fn fetch_attachments"),
        "D-03/D-04: kanban_api.rs must declare `fn fetch_attachments`"
    );
}

/// D-03/D-04: `attach_file` decodes a base64 payload and persists via the
/// SAME shared store method the CLI's `kanban attach` verb uses — never a
/// hand-rolled INSERT / inline SQL.
#[test]
fn attach_file_decodes_base64_and_calls_add_attachment() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("base64::engine::general_purpose::STANDARD") || src.contains("base64::"),
        "D-03/D-04: attach_file must base64-decode the inbound content_b64 payload"
    );
    assert!(
        src.contains(".add_attachment("),
        "D-03/D-04: attach_file must persist via `store.add_attachment(...)` — \
         the single shared traversal-safe writer, not inline SQL"
    );
}

/// T-46.4-06: the 10MB cap is enforced server-side before any store write.
#[test]
fn attach_file_enforces_max_attachment_bytes_cap() {
    let src = read("src/server/kanban_api.rs");
    assert!(
        src.contains("MAX_ATTACHMENT_BYTES"),
        "T-46.4-06: attach_file must enforce a documented MAX_ATTACHMENT_BYTES cap"
    );
    assert!(
        src.contains("10 * 1024 * 1024"),
        "T-46.4-06: MAX_ATTACHMENT_BYTES must be the 10MB cap"
    );
}

/// D-05 prohibition: the web attach surface only stores — it never copies
/// attachments into any workspace (copy-into-workspace is Plan 04's
/// spawn-time job, not a web-request concern).
#[test]
fn kanban_api_never_copies_attachments_into_a_workspace() {
    let src = read("src/server/kanban_api.rs");
    assert_eq!(
        src.matches("copy_attachments_into").count(),
        0,
        "D-05: kanban_api.rs must NOT call copy_attachments_into — the web \
         surface only stores via add_attachment; workspace copy-in happens \
         at spawn time (Plan 04), not here"
    );
}

/// D-10: every `TaskRow { ... }` construction sets `output_path` so the
/// wire field round-trips from every read/write fn (fetch_board,
/// patch_task_status, create_task).
#[test]
fn every_task_row_construction_sets_output_path() {
    let src = read("src/server/kanban_api.rs");
    let task_row_count = src.matches("TaskRow {").count();
    let output_path_count = src.matches("output_path:").count();
    assert!(
        output_path_count >= task_row_count,
        "D-10: every `TaskRow {{ ... }}` construction ({} found) must set \
         `output_path:` ({} found) — output_path must round-trip on every \
         TaskRow-returning fn",
        task_row_count,
        output_path_count,
    );
}

/// D-19: the base count of 9 (5 reads + 4 writes from Plan 01/02) is now 11
/// with attach_file + fetch_attachments added in Plan 06.
#[test]
fn kanban_api_board_option_string_count_includes_plan_06_fns() {
    let src = read("src/server/kanban_api.rs");
    let board_param_count = src.matches("board: Option<String>").count();
    assert!(
        board_param_count >= 11,
        "D-19: every #[server] fn must take `board: Option<String>` — found {} \
         occurrences (need ≥ 11: 9 from Plans 01/02 + attach_file + fetch_attachments)",
        board_param_count,
    );
}

/// The `iron_hermes_ui` Cargo.toml references the pre-pinned `base64`
/// workspace dependency (used for base64 decode server-side / encode
/// client-side).
#[test]
fn cargo_toml_references_base64_workspace_dep() {
    let src = read("Cargo.toml");
    assert!(
        src.contains("base64 = { workspace = true }"),
        "Cargo.toml must reference `base64 = {{ workspace = true }}`"
    );
}
