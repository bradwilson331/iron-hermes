//! T-2 SQL-injection-via-slug mitigation proof (Phase 36.3.7.9 Plan 09).
//!
//! # What this test locks
//!
//! Every SQL statement in the kanban crate that involves board routing uses
//! parametrized `?N` binds OR uses the slug exclusively as a **filesystem path
//! component** (never as a literal interpolated into a SQL string). A board slug
//! used inside a `format!(...)` that feeds into a `.execute(...)` / `.query_row(...)`
//! call would be an injection surface — this test ensures that pattern does not
//! exist in the production source.
//!
//! # Heuristic
//!
//! We grep for `format!(` patterns whose result string contains `WHERE` (the
//! canonical marker of a SQL predicate). A match inside a `crates/ironhermes-kanban/src/`
//! file indicates a dynamic SQL string with a filter clause — which is the pattern
//! most likely to embed a slug literal (e.g., `format!("... WHERE slug = {slug}")`).
//!
//! Limitations:
//! - The heuristic only catches single-line `format!(...WHERE...)` constructions.
//!   Multi-line builds are out of scope; the store.rs review in Plans 01-02
//!   confirmed no such patterns exist.
//! - Comments containing both `format!` and `WHERE` would be a false positive.
//!   We filter those out by excluding lines that start with `//` after trimming.
//! - Test files (`/tests/`) are excluded from the audit scope since they may
//!   legitimately build dynamic SQL for seeding/verification.
//!
//! # How to read a failure
//!
//! If this test starts failing after a future PR, it means someone added a
//! `format!(...)` call with a `WHERE` clause inside `crates/ironhermes-kanban/src/`.
//! Inspect the matched line:
//! - If the slug/input is bound via `?N` parametrization ONLY and `format!` is used
//!   for unrelated string composition, add the false-positive line to the known
//!   whitelist below.
//! - If the slug is literally interpolated into the SQL predicate, that is a T-2
//!   regression — fix it by switching to `?N` parametrized binds.

use std::process::Command;

/// T-2 mitigation: no `format!(...)` call in the kanban crate's production source
/// contains a WHERE clause (which would indicate dynamic SQL construction that
/// could embed a slug literal).
///
/// The grep pattern `format!\([^)]*WHERE` matches lines where `format!(` and the
/// string `WHERE` both appear within the same argument span. Any match that is
/// NOT in a test file or a comment is a potential T-2 injection surface.
#[test]
fn no_sql_string_contains_a_slug_literal_in_kanban_crate() {
    // Locate the kanban crate source directory relative to CARGO_MANIFEST_DIR.
    // CARGO_MANIFEST_DIR for ironhermes-kanban points at
    // `crates/ironhermes-kanban/`, so `src/` is one level down.
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let src_dir = format!("{manifest_dir}/src");

    // Run: grep -rE "format!\([^)]*WHERE" <src_dir>
    // -r  = recursive
    // -E  = extended regex
    // -n  = include line numbers (for human-readable failure messages)
    let output = Command::new("grep")
        .args([
            "-rEn",
            r"format!\([^)]*WHERE",
            &src_dir,
        ])
        .output()
        .expect("grep command must be available (T-2 audit requires grep)");

    // Collect matched lines, excluding:
    //   1. Lines that are comments (trimmed line starts with "//")
    //   2. Lines from test modules (path contains "/tests/" — but src/ shouldn't
    //      have integration test files; this is a belt-and-suspenders exclusion)
    let stdout = String::from_utf8_lossy(&output.stdout);
    let violations: Vec<&str> = stdout
        .lines()
        .filter(|line| {
            // Split on ":" to get "path:linenum:content"
            let mut parts = line.splitn(3, ':');
            let _path = parts.next().unwrap_or("");
            let _linenum = parts.next().unwrap_or("");
            let content = parts.next().unwrap_or(line);

            // Exclude pure comment lines.
            let trimmed = content.trim();
            if trimmed.starts_with("//") {
                return false;
            }

            // Exclude test files that somehow end up under src/.
            // (Shouldn't happen in this crate, but be safe.)
            if _path.contains("/tests/") {
                return false;
            }

            true
        })
        .collect();

    assert!(
        violations.is_empty(),
        "T-2 SQL injection via slug: found {} potential dynamic-SQL-with-WHERE pattern(s) \
         in crates/ironhermes-kanban/src/. \
         Each match is a candidate for slug-literal SQL injection. \
         Inspect each match and either:\n\
         (a) Confirm the slug is NOT interpolated into the SQL predicate and add it \
             to the whitelist in sql_no_slug_literals.rs, OR\n\
         (b) Fix the injection by replacing the slug interpolation with a ?N parametrized bind.\n\n\
         Matches:\n{}",
        violations.len(),
        violations.join("\n")
    );
}
