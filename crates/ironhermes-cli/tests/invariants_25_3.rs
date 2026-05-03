//! Phase 25.3 LOAD-BEARING parity-guard tests.
//!
//! Locks the 4-site wireup contract for the new CommandContext fields
//! (`workspace`, `trajectory_writer`) BEFORE Plan 8 implements the wireup.
//!
//! ## Why this file exists (RESEARCH.md Pitfall 1)
//!
//! Phase 25.2 Plan 15's parity-guard test only grep'd `main.rs`. It MISSED:
//!   - `crates/ironhermes-cli/src/tui_rata/commands.rs::build_command_context`
//!     (the DEFAULT `hermes chat` REPL since Phase 22.4)
//!   - `crates/ironhermes-gateway/src/handler.rs::handle_slash_command`
//!     (Telegram per-message slash dispatch)
//!
//! Result: Phase 25.2 UAT FAILED for `/toolset` in REPL + Telegram. Two follow-up
//! commits (`61ba493`, `3f25ac5`) were required after the verifier returned PASS.
//! See `.planning/phases/25.2-web-extract-tools/25.2-VERIFICATION.md` Addendum (2026-05-03)
//! for the full post-mortem.
//!
//! ## Wave 0 -> Plan 8 RED-then-GREEN protocol
//!
//! These tests are RED at Wave 0 — the wireup does not exist until Plan 8 (4-site
//! wireup of Workspace + TrajectoryWriter into all CommandContext construction sites
//! AND GatewayRunner setters). They are gated with `#[ignore]` so other Wave 0 / 1
//! plans' `cargo test --workspace` runs are not blocked by the planned-RED state.
//!
//! Plan 8's FIRST task MUST remove the `#[ignore]` attributes after wiring the 4 sites.
//! Plan 8's success criterion is that all 6 tests turn GREEN with the ignores removed.
//!
//! ## Coverage matrix
//!
//! | Field            | Site                                       | Test                                                          |
//! |------------------|--------------------------------------------|---------------------------------------------------------------|
//! | workspace        | main.rs::build_cmd_ctx                     | workspace_wired_in_all_commandcontext_construction_sites      |
//! | workspace        | tui_rata/commands.rs::build_command_context| workspace_wired_in_all_commandcontext_construction_sites      |
//! | workspace        | gateway/handler.rs::handle_slash_command   | workspace_wired_in_gateway_handler                            |
//! | workspace        | gateway/runner.rs::set_workspace setter    | workspace_wired_in_gateway_runner_setter                      |
//! | trajectory_writer| main.rs::build_cmd_ctx                     | trajectory_writer_wired_in_all_commandcontext_construction_sites|
//! | trajectory_writer| tui_rata/commands.rs::build_command_context| trajectory_writer_wired_in_all_commandcontext_construction_sites|
//! | trajectory_writer| gateway/handler.rs::handle_slash_command   | trajectory_writer_wired_in_gateway_handler                    |
//! | trajectory_writer| gateway/runner.rs::set_trajectory_writer   | trajectory_writer_wired_in_gateway_runner_setter              |

const MAIN_RS: &str = include_str!("../src/main.rs");
const TUI_COMMANDS: &str = include_str!("../src/tui_rata/commands.rs");
const GW_HANDLER: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
const GW_RUNNER: &str = include_str!("../../ironhermes-gateway/src/runner.rs");

// ============================================================================
// Workspace parity (Phase 25.3 D-W-2)
// ============================================================================

#[test]
fn workspace_wired_in_all_commandcontext_construction_sites() {
    // Strip line comments first (per planner antipattern: bare grep can match
    // a doc comment and produce a self-invalidating gate).
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        main_no_comments.contains(".with_workspace("),
        "INV-25.3-01a: main.rs::build_cmd_ctx must call `.with_workspace(handle)` on \
         the CommandContext builder. Phase 25.2 lesson: missing this attach makes \
         cmd_sessions --workspace fall back to None for the CLI subcommand entry points."
    );
    assert!(
        tui_no_comments.contains(".with_workspace("),
        "INV-25.3-01b: tui_rata/commands.rs::build_command_context must call \
         `.with_workspace(handle)`. The ratatui REPL is the DEFAULT hermes chat surface \
         since Phase 22.4 — missing this attach is the EXACT regression that bit Phase 25.2."
    );
}

#[test]
fn workspace_wired_in_gateway_handler() {
    let no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains(".with_workspace("),
        "INV-25.3-02: gateway/handler.rs::handle_slash_command must call \
         `.with_workspace(handle)` on the per-message CommandContext. Without this, \
         /sessions --workspace returns None for Telegram users."
    );
}

#[test]
fn workspace_wired_in_gateway_runner_setter() {
    let no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains("set_workspace("),
        "INV-25.3-03: gateway/runner.rs must define `pub fn set_workspace(...)` setter \
         so run_gateway can install the resolved Workspace before runner.start(). \
         Mirrors set_toolset_session at line 111. build_gateway_handler clones it into the handler."
    );
}

// ============================================================================
// TrajectoryWriter parity (Phase 25.3 D-T-3)
// ============================================================================

#[test]
fn trajectory_writer_wired_in_all_commandcontext_construction_sites() {
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        main_no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-04a: main.rs::build_cmd_ctx must call `.with_trajectory_writer(handle)`. \
         Without this, slash commands dispatched in the CLI cannot record trajectory entries."
    );
    assert!(
        tui_no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-04b: tui_rata/commands.rs::build_command_context must call \
         `.with_trajectory_writer(handle)`. The ratatui REPL is the default chat surface — \
         missing this attach starves Phase 25.4 Curator's training-data source."
    );
}

#[test]
fn trajectory_writer_wired_in_gateway_handler() {
    let no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains(".with_trajectory_writer("),
        "INV-25.3-05: gateway/handler.rs::handle_slash_command must call \
         `.with_trajectory_writer(handle)` on the per-message CommandContext. \
         Telegram is the primary user-facing surface — excluding it would starve 25.4 Curator."
    );
}

#[test]
fn trajectory_writer_wired_in_gateway_runner_setter() {
    let no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        no_comments.contains("set_trajectory_writer("),
        "INV-25.3-06: gateway/runner.rs must define `pub fn set_trajectory_writer(...)` setter \
         so run_gateway can install the TrajectoryWriter before runner.start(). \
         Mirrors set_toolset_session at line 111. build_gateway_handler clones it into the handler."
    );
}

// ============================================================================
// Sanity check — these pass at Wave 0 (locks the existing toolset_session parity
// so Plan 8 cannot accidentally remove it while adding workspace + trajectory).
// ============================================================================

#[test]
fn existing_toolset_session_parity_still_holds() {
    // This is GREEN at Wave 0 because Phase 25.2 Plan 15 already wired toolset_session.
    // Plan 8 must NOT regress these calls when adding workspace + trajectory.
    let main_no_comments: String = MAIN_RS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let tui_no_comments: String = TUI_COMMANDS
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let gw_handler_no_comments: String = GW_HANDLER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let gw_runner_no_comments: String = GW_RUNNER
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(main_no_comments.contains(".with_toolset_session("));
    assert!(tui_no_comments.contains(".with_toolset_session("));
    assert!(gw_handler_no_comments.contains(".with_toolset_session("));
    assert!(gw_runner_no_comments.contains("set_toolset_session("));
}
