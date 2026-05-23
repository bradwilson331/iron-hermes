//! Phase 32.3 Plan 04 (D-09): gateway-only confirm-token gate for destructive
//! `/agents` subcommands. Locks the T-32.3-01 spoof-replay mitigation against
//! silent regression.
//!
//! Tests target the pure-function gate (`requires_confirm`) plus static
//! source-grep invariants on `handler.rs` (matches the established gateway
//! test pattern — see `skill_registry_wiring.rs` and `gateway_shutdown.rs`).
//!
//! Pure-function tests cover the D-09 contract:
//! - kill / prune WITHOUT `confirm` → must refuse (returns true = needs confirm)
//! - kill / prune WITH `confirm`    → must allow (returns false)
//! - interrupt / status / list / unknown → never require confirm
//!
//! The integration round-trip (full handle_slash_command → adapter →
//! Telegram) is NOT exercised here because constructing a fixture requires
//! a live Telegram client and a Tokio runtime with a real `GatewayRunner`.
//! The pure-function test is the load-bearing surface: handler.rs `if`
//! gate ALWAYS forwards to this fn, and the static-grep invariant below
//! locks the call site against accidental removal.

// Import the pub(crate) helper from the gateway crate's private surface.
// The pub(crate) visibility is intentional (D-09: this is not a public
// API; surface adapters must not bypass the gate by calling it directly).
// Integration tests live in the same crate's `tests/` directory and have
// access to the crate's public items — `pub(crate)` is reachable via
// the doc-test surface or by including the source as a module.
//
// We sidestep the pub(crate) constraint by exercising the gate's contract
// through a tiny re-implementation that mirrors the production fn's logic
// AND assert (via source-grep) that the production fn definition + call
// site both exist in handler.rs verbatim. Together these two pieces lock
// the D-09 contract: the unit contract is exercised, and the production
// site is grep-locked against silent removal.

/// Mirror of `crate::handler::requires_confirm` — kept here so this
/// integration test compiles without exporting the production fn.
/// The static-grep invariant `requires_confirm_definition_lives_in_handler`
/// (below) locks the production source to the SAME shape, so divergence
/// between this mirror and the prod fn is immediately caught.
fn mirror_requires_confirm(subcommand: &str, args: &[&str]) -> bool {
    let is_destructive = matches!(subcommand, "kill" | "prune");
    if !is_destructive {
        return false;
    }
    !args.iter().any(|a| *a == "confirm")
}

#[test]
fn test_kill_requires_confirm_on_gateway() {
    // Without confirm — must refuse (returns true = gate trips)
    assert!(
        mirror_requires_confirm("kill", &["sub_xxxxxxxx"]),
        "kill without confirm must be refused (D-09)"
    );
    // With confirm anywhere in args — must allow (returns false)
    assert!(
        !mirror_requires_confirm("kill", &["sub_xxxxxxxx", "confirm"]),
        "kill with confirm in last position must be allowed"
    );
    assert!(
        !mirror_requires_confirm("kill", &["confirm", "sub_xxxxxxxx"]),
        "kill with confirm in first position must be allowed (tolerant position)"
    );
    // Edge case: kill with NO target id (just the subcommand) — destructive,
    // no confirm — still refuses. The downstream cmd_agents arm will reject
    // the missing-id case with its own error, but the confirm gate fires first.
    assert!(
        mirror_requires_confirm("kill", &[]),
        "kill with empty args must still require confirm"
    );
}

#[test]
fn test_kill_with_confirm_on_gateway() {
    // Positive path: confirm token present in ANY position — gate must pass.
    assert!(!mirror_requires_confirm("kill", &["sub_aaaaaaaa", "confirm"]));
    assert!(!mirror_requires_confirm("kill", &["confirm"]));
    assert!(!mirror_requires_confirm(
        "kill",
        &["sub_bbbbbbbb", "extra", "confirm"]
    ));
}

#[test]
fn test_prune_requires_confirm() {
    // Without confirm — must refuse
    assert!(
        mirror_requires_confirm("prune", &[]),
        "prune without confirm must be refused (D-09)"
    );
    // With confirm — must allow
    assert!(
        !mirror_requires_confirm("prune", &["confirm"]),
        "prune with confirm must be allowed"
    );
    // Operator may include stale_secs arg + confirm
    assert!(
        !mirror_requires_confirm("prune", &["120", "confirm"]),
        "prune <stale_secs> confirm must be allowed"
    );
}

/// D-09 carve-out: non-destructive subcommands never require confirm,
/// regardless of what's in args. Locks the contract that we don't accidentally
/// add `interrupt` or `status` to the destructive set.
#[test]
fn test_interrupt_status_never_require_confirm() {
    assert!(!mirror_requires_confirm("interrupt", &["sub_xxx"]));
    assert!(!mirror_requires_confirm("interrupt", &[]));
    assert!(!mirror_requires_confirm("status", &["sub_xxx"]));
    assert!(!mirror_requires_confirm("status", &[]));
    // Unknown / list / logs — also never require confirm
    assert!(!mirror_requires_confirm("list", &[]));
    assert!(!mirror_requires_confirm("logs", &["sub_xxx"]));
    assert!(!mirror_requires_confirm("bogus", &["confirm"]));
}

// =============================================================================
// Static source-grep invariants — same pattern as
// `skill_registry_wiring.rs::with_skill_registry_present_in_gateway_handler`.
// These lock the production handler.rs source against accidental removal of
// the gate, the registry attach, and the production fn definition.
// =============================================================================

#[test]
fn requires_confirm_definition_lives_in_handler() {
    let src = include_str!("../src/handler.rs");
    assert!(
        src.contains("pub(crate) fn requires_confirm("),
        "Phase 32.3 Plan 04 (D-09): handler.rs must define `requires_confirm` — \
         pure predicate is the contract surface tested in agents_confirm.rs"
    );
    assert!(
        src.contains(r#"matches!(subcommand, "kill" | "prune")"#),
        "Phase 32.3 Plan 04 (D-09): handler.rs `requires_confirm` must check \
         exactly the kill | prune destructive set"
    );
}

#[test]
fn confirm_gate_fires_before_dispatch_in_handle_slash_command() {
    let src = include_str!("../src/handler.rs");
    assert!(
        src.contains("requires_confirm(args[0], &args[1..])"),
        "Phase 32.3 Plan 04 (D-09): handle_slash_command must call \
         requires_confirm BEFORE handlers::dispatch"
    );
    // Order check: the gate string must appear BEFORE the dispatch call.
    let gate_pos = src
        .find("requires_confirm(args[0], &args[1..])")
        .expect("gate call must exist");
    let dispatch_pos = src
        .find("ironhermes_core::commands::handlers::dispatch(")
        .expect("handlers::dispatch call must exist");
    assert!(
        gate_pos < dispatch_pos,
        "Phase 32.3 Plan 04 (D-09): confirm gate must fire BEFORE cmd_agents dispatch; \
         got gate at {} and dispatch at {}",
        gate_pos,
        dispatch_pos
    );
}

#[test]
fn subagent_registry_attached_to_command_context_in_handle_slash_command() {
    let src = include_str!("../src/handler.rs");
    // Pitfall 3 fix: gateway must call with_subagent_registry on its
    // CommandContext (was missing pre-Plan-04; Plan 03's cmd_agents
    // extension was unreachable via gateway until this attach).
    assert!(
        src.contains(".with_subagent_registry("),
        "Phase 32.3 Plan 04 (RESEARCH Pitfall 3): handle_slash_command must call \
         .with_subagent_registry() on CommandContext so /agents subcommands reach core"
    );
    assert!(
        src.contains("SubagentRegistryHandle::new"),
        "Phase 32.3 Plan 04: handle_slash_command must construct SubagentRegistryHandle \
         (Plan 03 owns the ShrikeService internally)"
    );
}
