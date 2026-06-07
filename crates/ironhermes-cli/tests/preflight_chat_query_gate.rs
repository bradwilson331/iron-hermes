//! Phase 36.3.7.0 Plan 05 — BUG-36.3.7-04 regression tests.
//!
//! `ironhermes --profile P chat -q "PROMPT"` is functionally equivalent to
//! `ironhermes -e "PROMPT"` (both short-circuit through `run_single`), but the
//! original `run_preflight` gate at `main.rs:402` only excluded the `-e/--execute`
//! non-interactive path and not the new `chat -q` non-interactive path. The
//! preflight wizard then tried to read from stdin and the worker subprocess
//! died with `Error: EOF on stdin`.
//!
//! These are static-grep regression tests, using the same pattern as
//! `tests/kanban_worker_session.rs`. Runtime PASS of `chat -q` is exercised by
//! the UAT-09-A re-run in Phase 36.3.7.0 Plan 04.

const MAIN_RS: &str = include_str!("../src/main.rs");

/// BUG-36.3.7-04: the `run_preflight` gate must skip when `chat -q` is set.
/// This proves the gate condition includes `!chat_has_query` (or an equivalent
/// shape with the same effect).
#[test]
fn preflight_gate_excludes_chat_with_query() {
    assert!(
        MAIN_RS.contains("!chat_has_query"),
        "BUG-36.3.7-04 regression: main.rs must use `!chat_has_query` in the \
         run_preflight / is_interactive_repl gates so `chat -q` non-interactive \
         path skips the preflight wizard (Plan 05 fix)"
    );
}

/// BUG-36.3.7-04: the `chat_has_query` binding must correctly destructure
/// `Commands::Chat { query: Some(_), .. }` — anything broader would
/// incorrectly bypass the wizard for the interactive `chat` path too.
#[test]
fn chat_has_query_destructures_query_some() {
    assert!(
        MAIN_RS.contains("Commands::Chat { query: Some(_), .. }"),
        "BUG-36.3.7-04 regression: main.rs must destructure `Commands::Chat \
         {{ query: Some(_), .. }}` for the chat_has_query binding so only the \
         non-interactive chat -q path is excluded from the preflight wizard \
         (Plan 05 fix). A broader pattern would break the interactive `chat` \
         entry point's wizard auto-launch behavior."
    );
}

/// BUG-36.3.7-04: the sibling `is_interactive_repl` gate must ALSO exclude
/// `chat -q` — otherwise the interactive-log-filter would apply incorrectly
/// to the worker subprocess and bury the diagnostics operators need.
#[test]
fn interactive_repl_gate_also_excludes_chat_with_query() {
    // Count the occurrences of `!chat_has_query` — must be at least 2:
    // once in `run_preflight`, once in `is_interactive_repl`.
    let count = MAIN_RS.matches("!chat_has_query").count();
    assert!(
        count >= 2,
        "BUG-36.3.7-04 regression: main.rs must use `!chat_has_query` in BOTH \
         the run_preflight gate AND the is_interactive_repl gate (Plan 05 fix). \
         Found {count} occurrence(s); expected >= 2. A single occurrence means \
         only one of the two sibling gates was patched — `chat -q` would still \
         pick up the wrong log filter even if the wizard no longer trips."
    );
}
