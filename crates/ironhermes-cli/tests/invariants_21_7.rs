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

#[test]
fn invariant_21_7_08_subagent_registry_on_context_and_runner_sites() {
    // Plan 07 (Wave 2) / D-03 / D-04: every AgentSubagentRunner::new site
    // must thread a SubagentRegistry via `.with_subagent_registry(...)`,
    // AND the CommandContext::new site in run_chat must also install the
    // registry handle for Plan 08 (/agents list/kill/logs) consumers.
    //
    // Expected count: 3 runner sites (run_single, run_chat, run_gateway) +
    // 1 CommandContext site (run_chat) = at least 4.
    let count = MAIN_RS.matches("with_subagent_registry").count();
    assert!(
        count >= 4,
        "INV-21.7-08 / D-03 / D-04: subagent_registry must be threaded \
         through 3 AgentSubagentRunner::new sites + CommandContext::new \
         in run_chat. Found {}.",
        count
    );
}

#[test]
fn invariant_21_7_09_transcript_flush_on_session_end() {
    // Plan 07 (Wave 2) / D-05: both CLI `on_session_end` sites (run_single
    // and run_chat) must sleep 200ms before returning so pending
    // fire-and-forget TranscriptWriter::append futures can drain. This is
    // the Plan 03 open-question resolution (real writes complete in <10ms;
    // 200ms is cheap at session exit). `Duration::from_millis(200)` is the
    // exact substring because there's no other 200ms drain in main.rs.
    let count = MAIN_RS.matches("Duration::from_millis(200)").count();
    assert!(
        count >= 2,
        "INV-21.7-09 / D-05: transcript-flush drain must exist at both \
         on_session_end sites (run_single, run_chat). Found {}.",
        count
    );
}

#[test]
fn invariant_21_7_11_pill_refresh_uses_send_modify_and_no_await_on_render_path() {
    // Plan 07 / D-04 / Pitfall 8 / ISS-05: the render path
    // (`crates/ironhermes-cli/src/tui/status_line.rs`) must NEVER await
    // `registry.read().await` or `registry.write().await`. The pill reads
    // `state.active_subagents: usize`, copied from a send_modify performed
    // by a spawned task OFF the render path.
    const STATUS_LINE: &str = include_str!("../src/tui/status_line.rs");
    let await_reads = STATUS_LINE.matches("registry.read().await").count();
    let await_writes = STATUS_LINE.matches("registry.write().await").count();
    assert_eq!(
        await_reads, 0,
        "ISS-05 / Pitfall 8: status_line.rs must not contain \
         `registry.read().await` — move to a spawned task."
    );
    assert_eq!(
        await_writes, 0,
        "ISS-05 / Pitfall 8: status_line.rs must not contain \
         `registry.write().await`."
    );

    // And main.rs's SubagentProgressCallback must emit
    // `status_tx.send_modify` at least once in a spawned task so the pill
    // refresh is sync (channel-side) on the emission side.
    let send_sites = MAIN_RS.matches("status_tx.send_modify").count();
    assert!(
        send_sites >= 1,
        "ISS-05 / D-04: pill refresh must use status_tx.send_modify (sync) \
         at least once in main.rs. Found {}.",
        send_sites
    );
}

#[test]
fn invariant_21_7_10_gateway_subcommand_rejects_yolo_flag() {
    // ISS-06 / D-12: gateway CLI subcommand does NOT accept --yolo.
    // Parse-level assertion via clap — robust to formatting / field reordering.
    use clap::Parser;
    let err = ironhermes_cli::cli_args::Cli::try_parse_from([
        "hermes", "gateway", "--yolo",
    ])
    .unwrap_err();
    assert!(
        matches!(
            err.kind(),
            clap::error::ErrorKind::UnknownArgument
                | clap::error::ErrorKind::InvalidSubcommand
        ),
        "INV-21.7-10 / D-12 / ISS-06: `hermes gateway --yolo` must fail at \
         the clap parser. Got kind: {:?}",
        err.kind()
    );
}

#[test]
fn invariant_21_7_12_mid_turn_slash_dispatch_arm_exists() {
    // Plan 11 / GAP-21.7-01: run_chat's mid-turn tokio::select! loop must
    // have an arm that polls the ReplInputChannel so slash commands
    // dispatch while the agent turn is in flight. The arm is the sole
    // cure for the "subagents already unregistered by the time the user
    // types /agents list" defect.
    //
    // Accept either the legacy channel-name spelling (`slash_input_rx`)
    // or the canonical production spelling (`repl_input.recv_line()`) —
    // either one proves the arm exists. The static grep matches across
    // the entire main.rs (the actual arm sits inside the `'turn: loop`
    // select, but substring match is sufficient here).
    let slash_spelling = MAIN_RS.matches("slash_input_rx.recv()").count();
    let repl_spelling = MAIN_RS.matches("repl_input.recv_line()").count();
    assert!(
        slash_spelling >= 1 || repl_spelling >= 1,
        "INV-21.7-12 / GAP-21.7-01: run_chat must have a mid-turn select arm \
         polling the ReplInputChannel so `/agents list` works during an \
         in-flight turn. Expected >=1 match of `slash_input_rx.recv()` OR \
         `repl_input.recv_line()`; found slash={}, repl={}.",
        slash_spelling,
        repl_spelling,
    );
}

#[test]
fn invariant_21_7_13_repl_input_channel_spawned() {
    // Plan 11 / GAP-21.7-01: run_chat must spawn a ReplInputChannel at
    // startup so rustyline's blocking DefaultEditor lives on a dedicated
    // thread. Without this, tokio::select! cannot race the user's input
    // against the agent turn future.
    let count = MAIN_RS.matches("ReplInputChannel::spawn").count();
    assert!(
        count >= 1,
        "INV-21.7-13 / GAP-21.7-01: run_chat must call \
         `ReplInputChannel::spawn` so the rustyline editor runs off the \
         main tokio task. Found {}.",
        count
    );
}
