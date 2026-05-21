//! Phase 28.1 Plan 02 static-grep regression gates.
//!
//! Asserts the AgentRuntime migration invariants for the Telegram gateway:
//! (1) handler.rs production code contains no `with_budget(` or `BudgetHandle::new(`
//!     (comment-stripped + test-section-stripped so neither prose nor test code
//!     can self-invalidate the gate);
//! (2) handler.rs contains `run_turn` (delegates to AgentRuntime::run_turn);
//! (3) handler.rs contains the literal `gw:` session-id format string
//!     and `spawn_nudge_review` (LEARN-01 preservation).
//!
//! Mirrors the position-guard style from
//! `crates/ironhermes-cli/tests/invariants_22_4.rs`.

const HANDLER_SOURCE: &str = include_str!("../src/handler.rs");

/// Strip comment lines and the `#[cfg(test)]` module block from source text
/// so neither doc-prose nor test code can satisfy (or falsify) the gate.
fn strip_comments_and_tests(src: &str) -> String {
    let mut in_test_block = false;
    let mut brace_depth: i32 = 0;
    let mut result = String::with_capacity(src.len());

    for line in src.lines() {
        let trimmed = line.trim_start();

        // Detect start of test module
        if trimmed.contains("#[cfg(test)]") {
            in_test_block = true;
            brace_depth = 0;
        }

        if in_test_block {
            // Track brace depth to find where the test module ends
            for ch in line.chars() {
                match ch {
                    '{' => brace_depth += 1,
                    '}' => {
                        brace_depth -= 1;
                        if brace_depth <= 0 {
                            in_test_block = false;
                            brace_depth = 0;
                        }
                    }
                    _ => {}
                }
            }
            continue; // skip test module lines
        }

        // Skip pure comment lines
        if trimmed.starts_with("//") {
            continue;
        }

        result.push_str(line);
        result.push('\n');
    }
    result
}

/// T-28.1-03: handler.rs production code must contain no `BudgetHandle::new(`.
/// If this trips, a gateway-owned BudgetHandle was reintroduced — the budget is
/// now owned exclusively by AgentRuntime::from_config.
#[test]
fn no_budget_handle_new_in_handler_production_code() {
    let prod = strip_comments_and_tests(HANDLER_SOURCE);
    assert!(
        !prod.contains("BudgetHandle::new("),
        "T-28.1-03: handler.rs production code must NOT contain BudgetHandle::new(). \
         The gateway no longer constructs a BudgetHandle directly; AgentRuntime owns it. \
         (Plan 28.1-02)"
    );
}

/// T-28.1-03: handler.rs production code must contain no `with_budget(`.
/// Presence means an AgentLoop is being built by hand in the handler (old pattern).
#[test]
fn no_with_budget_in_handler_production_code() {
    let prod = strip_comments_and_tests(HANDLER_SOURCE);
    assert!(
        !prod.contains("with_budget("),
        "T-28.1-03: handler.rs production code must NOT contain with_budget(). \
         run_turn inside AgentRuntime handles budget wiring; the handler must not \
         build an AgentLoop by hand. (Plan 28.1-02)"
    );
}

/// handler.rs must delegate turns via AgentRuntime::run_turn.
#[test]
fn handler_delegates_via_run_turn() {
    assert!(
        HANDLER_SOURCE.contains("run_turn("),
        "handler.rs must call runtime.run_turn(...) to delegate top-level turns. \
         (Plan 28.1-02 migration invariant)"
    );
}

/// T-28.1-05: the canonical `gw:<chat>:<sender>` session-id format must still
/// be present in handler.rs so hooks / on_session_end / trajectory scoping
/// receive the correct identifier.
#[test]
fn gw_session_id_format_preserved() {
    assert!(
        HANDLER_SOURCE.contains("\"gw:{}:{}\""),
        "T-28.1-05: handler.rs must still format the session_id as \"gw:{{}}:{{}}\" \
         (event.chat_id + event.sender_id). Dropping this breaks hooks, on_session_end, \
         and trajectory scoping. (Plan 28.1-02 LEARN-01 preservation)"
    );
}

/// T-28.1-05: the LEARN-01 nudge mechanism (`spawn_nudge_review`) must still
/// be present in handler.rs so the periodic memory-review fires after each turn.
#[test]
fn spawn_nudge_review_preserved() {
    assert!(
        HANDLER_SOURCE.contains("spawn_nudge_review"),
        "T-28.1-05: handler.rs must still call spawn_nudge_review for the LEARN-01 \
         periodic memory-review nudge. (Plan 28.1-02 LEARN-01 preservation)"
    );
}
