//! Phase 34: static-grep regression gates for multi-platform gateway routing.
//!
//! These tests lock the two cross-crate wiring surfaces introduced by Phase 34
//! (Plans 34-02, 34-03) so a future refactor cannot silently remove the
//! multimodal routing path for Discord and Slack adapters. They use the same
//! `include_str!`-at-compile-time pattern as `invariants_33.rs` and
//! `invariants_27_1_4_1.rs` — no dev-deps, no I/O at test time, no runtime
//! path resolution required.
//!
//! **Wave 0 RED gate:** This file intentionally FAILS TO COMPILE until
//! Wave 2/3 land `src/discord.rs` and `src/slack.rs`. The compile error:
//!
//!   error: couldn't read `../src/discord.rs`: No such file or directory
//!
//! is the documented Wave 0 RED gate. Execute-phase logs this expected red
//! and requires it to flip to GREEN at the end of Wave 3 when the source
//! files are created. Do NOT use `#[cfg(...)]` to suppress the failure —
//! the failure IS the gate.
//!
//! Why each invariant exists:
//!
//! INV-34-01 — Phase 34 D-10: the Discord adapter (`src/discord.rs`) must
//!   route all incoming messages through `GatewayMessageHandler::handle_with_multimodal`
//!   so the Learning Loop (Phase 32/33 skill_manage + nudge) is inherited by
//!   Discord sessions. A direct call to the agent loop that bypasses
//!   `handle_with_multimodal` would silently disconnect Discord from memory
//!   consolidation and tool registration — the breakage surfaces only at
//!   runtime as tool-not-found errors or missing nudge behavior.
//!
//! INV-34-02 — Phase 34 D-11: the Slack adapter (`src/slack.rs`) must route
//!   all incoming messages through `GatewayMessageHandler::handle_with_multimodal`
//!   for the same reasons as INV-34-01. Slack and Discord share the same
//!   multimodal handler contract; both must comply for the Learning Loop to
//!   cover every platform the gateway serves.
//!
//! INV-34-03 — Phase 34 D-10/D-14/D-15: `GatewayRunner::start()` must call
//!   `run_discord_adapter()` so the Discord adapter is actually spawned when
//!   the Discord platform section is configured. Without this call site in
//!   runner.rs, the Discord config section would be silently inert — the adapter
//!   code would compile and pass INV-34-01 yet never run. This invariant closes
//!   the D-14/D-15 sequential dependency: Phase 34 Plan 03 (adapter) → Plan 05
//!   (runner wiring) must both be present for Discord to be a live platform.
//!
//! INV-34-04 — Phase 34 D-11/D-14/D-15: `GatewayRunner::start()` must call
//!   `run_slack_adapter()` for the same reasons as INV-34-03 applied to Slack.
//!   Together INV-34-03 + INV-34-04 complete the Phase 34 trilogy lock: adapter
//!   routing (01/02) + runner wiring (03/04) must all be green for the gateway
//!   to serve Discord and Slack as first-class platforms.

const DISCORD_SOURCE: &str = include_str!("../src/discord.rs");
const SLACK_SOURCE: &str = include_str!("../src/slack.rs");
const RUNNER_SOURCE: &str = include_str!("../src/runner.rs");

/// INV-34-01: `handle_with_multimodal` is called in the Discord adapter so
/// Discord sessions route through the Learning Loop handler.
#[test]
fn inv_34_01_discord_routes_through_handle_with_multimodal() {
    let count = DISCORD_SOURCE.matches("handle_with_multimodal").count();
    assert!(
        count >= 1,
        "INV-34-01: crates/ironhermes-gateway/src/discord.rs must call \
         handle_with_multimodal() so Discord sessions route through \
         GatewayMessageHandler and inherit the Learning Loop. Found {count} \
         occurrences (expected >= 1). See Phase 34 D-10."
    );
}

/// INV-34-02: `handle_with_multimodal` is called in the Slack adapter so
/// Slack sessions route through the Learning Loop handler.
#[test]
fn inv_34_02_slack_routes_through_handle_with_multimodal() {
    let count = SLACK_SOURCE.matches("handle_with_multimodal").count();
    assert!(
        count >= 1,
        "INV-34-02: crates/ironhermes-gateway/src/slack.rs must call \
         handle_with_multimodal() so Slack sessions route through \
         GatewayMessageHandler and inherit the Learning Loop. Found {count} \
         occurrences (expected >= 1). See Phase 34 D-11."
    );
}

/// INV-34-03: GatewayRunner::start spawns run_discord_adapter so Discord
/// inherits the Learning Loop via handle_with_multimodal (D-10/D-14/D-15).
#[test]
fn inv_34_03_runner_spawns_discord() {
    let count = RUNNER_SOURCE.matches("run_discord_adapter").count();
    assert!(count >= 1, "INV-34-03: runner.rs must call run_discord_adapter() so Discord inherits the Learning Loop. Found {count}. See Phase 34 D-10/D-14/D-15.");
}

/// INV-34-04: GatewayRunner::start spawns run_slack_adapter so Slack
/// inherits the Learning Loop via handle_with_multimodal (D-11/D-14/D-15).
#[test]
fn inv_34_04_runner_spawns_slack() {
    let count = RUNNER_SOURCE.matches("run_slack_adapter").count();
    assert!(count >= 1, "INV-34-04: runner.rs must call run_slack_adapter() so Slack inherits the Learning Loop. Found {count}. See Phase 34 D-11/D-14/D-15.");
}
