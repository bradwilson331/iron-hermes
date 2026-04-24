//! Phase 22.3 static-grep regression gates.
//! Locks the structural fixes from Plans 22.3-01 .. 22.3-05.
//! Follows the INV-21.7-14/15 + INV-22.1-04 `include_str!` pattern.
//! No new dev-deps (Phase 21 D-18). No vt100/pty harness (CONTEXT D-03).
//!
//! Pairs with the runtime tests in `crates/ironhermes-agent/tests/transcript_touch.rs`
//! (INV-22.3-05's behavioral half — file-exists-after-touch).

const MAIN_RS: &str = include_str!("../src/main.rs");
const REPL_INPUT_RS: &str = include_str!("../src/repl_input.rs");
const HANDLERS_RS: &str = include_str!("../../ironhermes-core/src/commands/handlers.rs");

/// INV-22.3-01: A `CommandResult::ResetTerminal` arm exists in `main.rs`'s
/// prompt-time slash-dispatch match. Plan 22.3-05 added it; this test locks
/// it against future deletion or accidental rename.
#[test]
fn invariant_22_3_01_reset_terminal_arm_exists_in_main() {
    assert!(
        MAIN_RS.contains("CommandResult::ResetTerminal"),
        "INV-22.3-01: a CommandResult::ResetTerminal match arm must exist in main.rs \
         (added by Plan 22.3-05 at the prompt-time slash-dispatch match site). \
         Without it `/clear` regresses to a no-op or compile error."
    );
    // Total occurrences must be at least 2: one prompt-time arm, one mid-turn arm
    // (Pitfall 5 — exhaustive match requires both).
    let count = MAIN_RS.matches("CommandResult::ResetTerminal").count();
    assert!(
        count >= 2,
        "INV-22.3-01: at least 2 CommandResult::ResetTerminal arms required in main.rs \
         (prompt-time + mid-turn). Found {}.",
        count
    );
}

/// INV-22.3-02: `print_banner()` is called exactly once in `main.rs`, and the
/// call site is strictly before `TuiHandle::new_with_extensions`. Locks the
/// structural fix for D-4 banner-bleed (RESEARCH §Banner-Bleed Probe verdict:
/// single call site + `/clear`'s scrollback erase is sufficient — no AtomicBool
/// guard needed).
///
/// NOTE: We match `print_banner();` (with semicolon) to count only the actual
/// call site — not the fn definition (`fn print_banner()`) or doc-comment
/// references to the function name.
#[test]
fn invariant_22_3_02_banner_called_exactly_once_before_tui_init() {
    let count = MAIN_RS.matches("print_banner();").count();
    assert_eq!(
        count, 1,
        "INV-22.3-02: print_banner() must be called exactly once in main.rs. \
         Found {} call sites. A second call site would re-introduce the D-4 \
         banner-bleed visible in 22.3-UAT-EVIDENCE.md.",
        count
    );
    let banner_pos = MAIN_RS
        .find("print_banner();")
        .expect("print_banner(); not found in main.rs");
    let tui_pos = MAIN_RS
        .find("new_with_extensions")
        .expect("TuiHandle::new_with_extensions not found in main.rs");
    assert!(
        banner_pos < tui_pos,
        "INV-22.3-02: print_banner() must appear before TuiHandle::new_with_extensions \
         in source order so the banner reaches scrollback BEFORE DECSTBM is established. \
         Reversed ordering would surface banner content into the scroll region after init."
    );
}

/// INV-22.3-03: `cmd_clear` in `handlers.rs` returns `CommandResult::ResetTerminal`
/// (NOT `ClearSession`); `cmd_new` still returns `NewSession`. Locks the
/// /clear ≠ /new disambiguation (UI-SPEC §"Disambiguating /clear from /new").
#[test]
fn invariant_22_3_03_cmd_clear_returns_reset_terminal_and_cmd_new_unchanged() {
    // cmd_clear must reference ResetTerminal in its body.
    assert!(
        HANDLERS_RS.contains("CommandResult::ResetTerminal"),
        "INV-22.3-03: cmd_clear in handlers.rs must return CommandResult::ResetTerminal \
         (not ClearSession). Plan 22.3-04 made the change; this test locks it."
    );
    // cmd_new must still reference NewSession (preserves /new truncate semantics).
    assert!(
        HANDLERS_RS.contains("CommandResult::NewSession"),
        "INV-22.3-03: cmd_new in handlers.rs must still return CommandResult::NewSession. \
         Removing this would silently break /new's session-truncate behavior."
    );
    // Source-order: cmd_new appears before cmd_clear in handlers.rs (lines 70 then 79).
    // Verifies the file structure is intact.
    let new_pos = HANDLERS_RS.find("fn cmd_new(").expect("fn cmd_new not found");
    let clear_pos = HANDLERS_RS.find("fn cmd_clear(").expect("fn cmd_clear not found");
    assert!(
        new_pos < clear_pos,
        "INV-22.3-03: cmd_new must appear before cmd_clear in handlers.rs source order \
         (file structure invariant from Phase 21.1). If this trips, the file was \
         restructured and other invariants may be invalid."
    );
}

/// INV-22.3-04: `repl_input.add_history(` appears AFTER `starts_with('/')` in
/// `main.rs`. This is the slash-side history wiring (Plan 22.3-05). The total
/// count must be at least 2 (slash-time + chat-time at line 1213).
#[test]
fn invariant_22_3_04_slash_commands_added_to_history_at_prompt_time() {
    let slash_check_pos = MAIN_RS
        .find("starts_with('/')")
        .expect("starts_with('/') not found in main.rs");
    let add_history_pos = MAIN_RS
        .find("repl_input.add_history(")
        .expect("repl_input.add_history( not found in main.rs");
    assert!(
        slash_check_pos < add_history_pos,
        "INV-22.3-04: repl_input.add_history must appear AFTER the first \
         starts_with('/') check in main.rs. Plan 22.3-05 added the slash-side \
         add_history call at the prompt-time dispatch site. Without this \
         ordering, slash commands are not recorded in unified rustyline history \
         (UI-SPEC HIST-2 regression)."
    );
    let count = MAIN_RS.matches("repl_input.add_history(").count();
    assert!(
        count >= 2,
        "INV-22.3-04: at least 2 repl_input.add_history( calls must exist in main.rs \
         (prompt-time slash-dispatch + chat-prompt at the existing line ~1213). \
         Found {}.",
        count
    );
}

/// INV-22.3-05: rustyline 15 correct API surface used in `repl_input.rs`.
/// The correct method `set_history_ignore_dups` MUST be present; the wrong
/// API names from CONTEXT D-08 (which RESEARCH corrected) — `set_history_duplicates`
/// and `HistoryDuplicates::Prev` — MUST be absent. Without this gate, a future
/// editor reverting to the CONTEXT-original wording would silently break the
/// build (or worse, compile against a different rustyline version with
/// different semantics).
///
/// The behavioral half of INV-22.3-05 (transcript file exists after touch) is
/// in `crates/ironhermes-agent/tests/transcript_touch.rs` (Plan 22.3-02).
#[test]
fn invariant_22_3_05_rustyline_correct_api_used_in_repl_input() {
    assert!(
        REPL_INPUT_RS.contains("set_history_ignore_dups"),
        "INV-22.3-05: repl_input.rs must call rl.set_history_ignore_dups(true) \
         per RESEARCH §rustyline API Notes. Plan 22.3-03 wired this. The CONTEXT \
         D-08 mention of `set_history_duplicates(HistoryDuplicates::Prev)` was \
         INCORRECT (those names do not exist in rustyline 15.0.0)."
    );
    // Check for the receiver-call form `rl.set_history_duplicates(` which is
    // what a revert to the wrong API would look like in actual code. A comment
    // in repl_input.rs documents the wrong API name in backticks for educational
    // purposes — the comment contains `set_history_duplicates(` but NOT the
    // receiver-prefixed form `rl.set_history_duplicates(`. Checking for the
    // dot-method form catches all real call sites without false-positive on the
    // explanatory comment.
    assert!(
        !REPL_INPUT_RS.contains("rl.set_history_duplicates("),
        "INV-22.3-05: repl_input.rs must NOT call `rl.set_history_duplicates(` — \
         that method does not exist in rustyline 15.0.0. Use set_history_ignore_dups \
         instead (RESEARCH §rustyline API Notes)."
    );
    // `HistoryDuplicates::Prev` appears in a comment at line 249 documenting
    // the incorrect CONTEXT D-08 API. The actual live-code guard is already
    // covered by the `set_history_duplicates(` check above. We gate on the
    // import form instead — if `use rustyline::history::HistoryDuplicates` ever
    // reappears, that signals a revert attempt.
    assert!(
        !REPL_INPUT_RS.contains("use rustyline::history::HistoryDuplicates"),
        "INV-22.3-05: repl_input.rs must NOT import `HistoryDuplicates` — \
         HistoryDuplicates::Prev does not exist in rustyline 15.0.0 and the \
         whole enum is unused with the correct set_history_ignore_dups(bool) API."
    );
    assert!(
        REPL_INPUT_RS.contains("set_max_history_size(1000)"),
        "INV-22.3-05: repl_input.rs must cap history at 1000 entries per UI-SPEC HIST-5. \
         Plan 22.3-03 wired this."
    );
    // The drop-site `let _ = history_path;` (the original line 244 stub) MUST be gone.
    assert!(
        !REPL_INPUT_RS.contains("let _ = history_path;"),
        "INV-22.3-05: the `let _ = history_path;` drop site at the original \
         repl_input.rs:244 must be replaced with the load_history wiring \
         (Plan 22.3-03). If this string reappears, history persistence regressed."
    );
}

/// INV-22.3-06: Mid-turn slash dispatch does NOT call `add_history`. Total
/// `repl_input.add_history(` call count in main.rs is EXACTLY 2 (prompt-time
/// slash-dispatch site + the existing chat-prompt site at line 1213). UI-SPEC
/// HIST-8 / CONTEXT D-14 enforce that mid-turn lines are not recorded — they
/// arrive mid-flight via recv_line() and the user has already had a chance to
/// commit them via the prompt-time path.
#[test]
fn invariant_22_3_06_mid_turn_dispatch_has_no_add_history() {
    let count = MAIN_RS.matches("repl_input.add_history(").count();
    assert_eq!(
        count, 2,
        "INV-22.3-06: exactly 2 repl_input.add_history( calls must exist in main.rs \
         (prompt-time slash-dispatch + prompt-time chat). The mid-turn select arm \
         must NOT call add_history per UI-SPEC HIST-8 / CONTEXT D-14. Found {}. \
         If count is 3+, the mid-turn arm picked up an accidental add_history call \
         and is recording mid-flight inputs that the user did not commit at a \
         prompt.",
        count
    );
}
