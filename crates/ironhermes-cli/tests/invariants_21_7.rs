//! E-09 / AI-SPEC Pitfall 1: three-site wiring-parity static-grep gate.
//!
//! Ports the INV-21.1 / INV-21.4 / INV-21.2 template from `main.rs` (the
//! `mod mcp_wiring_tests` / `mod ensure_home_dirs_tests` modules) into Phase
//! 21.7. Several invariants are RED in Wave 0 (the Wave-2 Plan 05/06/07 work
//! is what turns them fully green); others are GREEN today because the
//! three-site call-count precedent from Phase 22 is already in place.

const MAIN_RS: &str = include_str!("../src/main.rs");

#[test]
fn invariant_21_7_01_three_agent_subagent_runner_new_sites() {
    let count = MAIN_RS.matches("AgentSubagentRunner::new(").count();
    assert_eq!(
        count, 3,
        "INV-21.7-01: run_chat, run_single, and run_gateway each construct AgentSubagentRunner::new(. \
         Expected 3 call sites; found {}. If you added or removed a site, update this test with justification.",
        count
    );
}

#[test]
fn invariant_21_7_02_three_register_delegate_task_sites() {
    let count = MAIN_RS.matches("register_delegate_task_tool(").count();
    assert_eq!(
        count, 3,
        "INV-21.7-02: three register_delegate_task_tool( sites must remain (Phase 22 three-site precedent)."
    );
}

#[test]
fn invariant_21_7_03_three_register_execute_code_sites() {
    // Either the legacy signature OR the new with_process_registry variant —
    // total across the two spellings must equal 3 after Wave 2 Plan 06 lands
    // the rename. Today Wave 0 sees 3 legacy / 0 new.
    let legacy = MAIN_RS
        .matches("register_execute_code_tool_with_active_skills(")
        .count();
    let new_variant = MAIN_RS
        .matches("register_execute_code_tool_with_process_registry(")
        .count();
    assert_eq!(
        legacy + new_variant,
        3,
        "INV-21.7-03: execute_code registration must total 3 sites across CLI + gateway. legacy={}, new={}.",
        legacy,
        new_variant
    );
}

#[test]
fn invariant_21_7_04_budget_handle_threaded_through_all_three_sites() {
    // Plan 05 (Wave 2): every AgentSubagentRunner::new( call passes a
    // BudgetHandle. The skip-path has been removed — this is now a strict
    // regression gate. BudgetHandle must appear in run_chat, run_single,
    // and run_gateway (and also in at least one shared helper such as
    // run_agent_turn). Minimum count = 3 across all three sites.
    let marker = MAIN_RS.matches("BudgetHandle").count();
    assert!(
        marker >= 3,
        "INV-21.7-04 / E-09: BudgetHandle must appear in all 3 registration sites \
         (run_single, run_chat, run_gateway). Found {}.",
        marker
    );
}

#[test]
fn invariant_21_7_05_gateway_does_not_read_per_request_yolo() {
    // D-12: gateway mode reads config only; no per-message --yolo. This test
    // looks for the specific anti-pattern of reading yolo from request args.
    assert!(
        !MAIN_RS.contains("request.yolo") && !MAIN_RS.contains("req.yolo"),
        "INV-21.7-05 / D-12: gateway path must NOT read a per-request yolo field."
    );
}

#[test]
fn invariant_21_7_06_drain_and_kill_session_at_all_on_session_end_sites() {
    // Plan 06 (Wave 2) / T-21.7-06-01: both CLI `on_session_end` sites
    // (run_single at L549, run_chat at L1135) must call
    // `drain_and_kill_session(&session_id)` alongside the existing
    // memory-provider on_session_end so background processes tracked by
    // the ProcessRegistry are reaped before the session exits. Minimum
    // count = 2 across the two CLI sites (the gateway drain is gated
    // separately by INV-21.7-07).
    let count = MAIN_RS.matches("drain_and_kill_session(&session_id)").count();
    assert!(
        count >= 2,
        "INV-21.7-06: both on_session_end sites in main.rs must call \
         drain_and_kill_session(&session_id). Found {}.",
        count
    );
}

#[test]
fn invariant_21_7_07_gateway_drain_and_kill_session() {
    // Plan 06 (Wave 2) / T-21.7-06-01: the third on_session_end site lives
    // in ironhermes-gateway::handler::run_agent and must also drain its
    // gateway-scoped ProcessRegistry. Implemented as a separate static
    // grep because `include_str!` only sees ../src/main.rs from this crate.
    const GW_HANDLER: &str = include_str!("../../ironhermes-gateway/src/handler.rs");
    assert!(
        GW_HANDLER.contains("drain_and_kill_session"),
        "INV-21.7-07: gateway handler.rs on_session_end site must call \
         drain_and_kill_session so the third Plan 06 drain gate closes."
    );
}
