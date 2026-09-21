//! Phase 49.7 D-05 pin test.
//!
//! Asserts that every goal-mode loop referencing the shared budget/retry
//! semantics does so through the named constants
//! `ironhermes_kanban::judge::GOAL_DEFAULT_MAX_TURNS` and
//! `ironhermes_kanban::judge::GOAL_JUDGE_ERROR_STRIKES`, never a bare
//! open-coded literal. Without this test nothing catches the kanban
//! card-level loop (`goal_loop.rs`) and the session loop
//! (`goal_session_loop.rs`, Plan 05) silently drifting apart on these two
//! values.
//!
//! # Deliberately out of scope
//!
//! `crates/ironhermes-kanban/src/schema.rs` and
//! `crates/ironhermes-cli/src/kanban/worker_spawn.rs` also contain the
//! literal `20` — as the `goal_max_turns INTEGER NOT NULL DEFAULT 20`
//! SQLite column default. That is the kanban card's own PER-TASK override
//! mechanism, a genuinely separate concept from the two loops' shared
//! fallback default. This test does not read either file, and must never
//! be widened to do so: "fixing" a SQL column default here would silently
//! change kanban card behavior (49.7-02-PLAN.md Task 3).

use std::path::Path;

/// Source files this pin test guards. Both loop files must reference the
/// shared D-05 constants rather than a bare literal.
const GUARDED_FILES: &[&str] = &[
    "src/kanban/goal_loop.rs",
    "../ironhermes-agent/src/goal_session_loop.rs",
];

/// Reads a guarded file relative to this crate's manifest dir and strips
/// every comment line, mirroring the idiom in
/// `iron_hermes_ui/tests/profile_verify_classification.rs`.
fn read_stripped(relative_path: &str) -> String {
    let full = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative_path);
    let src = std::fs::read_to_string(&full)
        .unwrap_or_else(|e| panic!("failed to read {}: {}", full.display(), e));
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn goal_constants_are_not_open_coded() {
    for path in GUARDED_FILES {
        let stripped = read_stripped(path);

        assert!(
            stripped.contains("GOAL_DEFAULT_MAX_TURNS"),
            "{path}: expected a reference to GOAL_DEFAULT_MAX_TURNS"
        );
        assert!(
            stripped.contains("GOAL_JUDGE_ERROR_STRIKES"),
            "{path}: expected a reference to GOAL_JUDGE_ERROR_STRIKES"
        );

        // Scope the negative assertions to the specific guarded expression
        // line, not the whole file — a bare count of the digit 2 over a
        // multi-hundred-line file would be self-invalidating (turn
        // numbers, indices, and array sizes all contain it).
        let max_turns_line = stripped
            .lines()
            .find(|l| l.contains(".unwrap_or("))
            .unwrap_or_else(|| panic!("{path}: no line containing `.unwrap_or(` found"));
        assert!(
            !max_turns_line.contains("unwrap_or(20)"),
            "{path}: max-turns fallback still open-codes the literal 20: {max_turns_line}"
        );

        let strikes_line = stripped
            .lines()
            .find(|l| l.contains("consecutive_judge_errors >="))
            .unwrap_or_else(|| {
                panic!("{path}: no line comparing `consecutive_judge_errors >=` found")
            });
        assert!(
            !strikes_line.contains(">= 2"),
            "{path}: judge-error-strikes comparison still open-codes the literal 2: {strikes_line}"
        );
    }
}
