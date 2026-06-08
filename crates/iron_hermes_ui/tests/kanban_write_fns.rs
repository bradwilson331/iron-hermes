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
