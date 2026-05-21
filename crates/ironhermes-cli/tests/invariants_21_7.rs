//! E-09 / AI-SPEC Pitfall 1: three-site wiring-parity static-grep gate.
//!
//! Ports the INV-21.1 / INV-21.4 / INV-21.2 template from `main.rs` (the
//! `mod mcp_wiring_tests` / `mod ensure_home_dirs_tests` modules) into Phase
//! 21.7. Several invariants are RED in Wave 0 (the Wave-2 Plan 05/06/07 work
//! is what turns them fully green); others are GREEN today because the
//! three-site call-count precedent from Phase 22 is already in place.

const MAIN_RS: &str = include_str!("../src/main.rs");

// Phase 28.1-02: run_gateway no longer constructs the subagent runner inline.
// It now builds the shared runtime via `AgentRuntime::from_config`, which
// constructs the runner (and threads the subagent registry) internally. The
// three logical call sites are therefore split across two files: run_chat +
// run_single in main.rs, and the gateway/runtime site in agent_runtime.rs.
const AGENT_RUNTIME_RS: &str =
    include_str!("../../ironhermes-agent/src/agent_runtime.rs");

#[test]
fn invariant_21_7_01_three_agent_subagent_runner_new_sites() {
    let main_sites = MAIN_RS.matches("AgentSubagentRunner::new(").count();
    let runtime_sites = AGENT_RUNTIME_RS.matches("AgentSubagentRunner::new(").count();
    let count = main_sites + runtime_sites;
    assert_eq!(
        count, 3,
        "INV-21.7-01: run_chat + run_single construct AgentSubagentRunner::new( in main.rs \
         ({main_sites}), and run_gateway's runner is built inside AgentRuntime::from_config \
         in agent_runtime.rs ({runtime_sites}). Expected 3 logical call sites total; found {count}. \
         If you added or removed a site, update this test with justification."
    );
}

// Phase 25.6 consolidated all per-path tool registration into the shared
// runtime factory `ironhermes_agent::app_runtime_factory::build_app_runtime_bundle`,
// which run_chat / run_single / run_gateway each call (the 3-call-site contract is
// itself locked by the `build_app_runtime_bundle`-count tests inside main.rs).
// register_delegate_task_tool / register_execute_code_tool therefore no longer
// appear inline in main.rs — they live in the factory. INV-21.7-02/-03 are
// retargeted to assert the registration survives in that single wiring path.
// Cross-crate `include_str!` precedent: INV-21.7-07 below reads the gateway handler.
const RUNTIME_FACTORY: &str = include_str!("../../ironhermes-agent/src/app_runtime_factory.rs");

#[test]
fn invariant_21_7_02_delegate_task_registered_in_runtime_factory() {
    assert!(
        RUNTIME_FACTORY.contains("register_delegate_task_tool("),
        "INV-21.7-02: delegate_task must remain wired via the shared runtime factory \
         (build_app_runtime_bundle, used by run_chat/run_single/run_gateway). If this \
         fails, the delegate_task registration was dropped from app_runtime_factory.rs."
    );
}

#[test]
fn invariant_21_7_03_execute_code_registered_in_runtime_factory() {
    // Accept either the legacy signature OR the with_process_registry variant.
    let legacy = RUNTIME_FACTORY
        .matches("register_execute_code_tool_with_active_skills(")
        .count();
    let new_variant = RUNTIME_FACTORY
        .matches("register_execute_code_tool_with_process_registry(")
        .count();
    assert!(
        legacy + new_variant >= 1,
        "INV-21.7-03: execute_code must remain wired via the shared runtime factory. \
         legacy={legacy}, new={new_variant}."
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
    let count = MAIN_RS
        .matches("drain_and_kill_session(&session_id)")
        .count();
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
    //
    // Phase 28.1-02: run_gateway's runner site moved into AgentRuntime::from_config
    // (agent_runtime.rs), so the threading is now split across two files —
    // run_single + run_chat runner sites + the run_chat CommandContext site in
    // main.rs, plus the gateway runner site in agent_runtime.rs.
    let count = MAIN_RS.matches("with_subagent_registry").count()
        + AGENT_RUNTIME_RS.matches("with_subagent_registry").count();
    assert!(
        count >= 4,
        "INV-21.7-08 / D-03 / D-04: subagent_registry must be threaded \
         through 3 AgentSubagentRunner::new sites (run_single + run_chat in main.rs, \
         run_gateway via AgentRuntime::from_config in agent_runtime.rs) + \
         CommandContext::new in run_chat. Found {}.",
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
    let err =
        ironhermes_cli::cli_args::Cli::try_parse_from(["hermes", "gateway", "--yolo"]).unwrap_err();
    assert!(
        matches!(
            err.kind(),
            clap::error::ErrorKind::UnknownArgument | clap::error::ErrorKind::InvalidSubcommand
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

#[test]
fn invariant_21_7_14_reserved_rows_on_prompt_request_and_worker() {
    // Plan 12 / GAP-21.7-02: the worker-thread cursor-positioning
    // contract must be structurally present. Three structural checks
    // together guarantee the prompt anchors deterministically:
    //
    //   (a) `PromptRequest` in repl_input.rs carries a `reserved_rows`
    //       field — without it, the main task cannot tell the worker
    //       where to paint.
    //   (b) The worker loop calls `prompt_position_ansi(` at least once
    //       — proving worker-side positioning is live (the whole point
    //       of the fix).
    //   (c) main.rs's prompt-time site passes `reserved_rows: Some(` at
    //       least once — proving the main task actually forwards the
    //       reserved count rather than defaulting to None everywhere.
    const REPL_INPUT: &str = include_str!("../src/repl_input.rs");
    assert!(
        REPL_INPUT.contains("reserved_rows"),
        "INV-21.7-14 (a) / GAP-21.7-02: PromptRequest must carry \
         `reserved_rows` so the worker can position the cursor on the \
         same thread as rl.readline."
    );
    assert!(
        REPL_INPUT.contains("prompt_position_ansi("),
        "INV-21.7-14 (b) / GAP-21.7-02: repl_input worker must call \
         `prompt_position_ansi(` before rl.readline — worker-thread \
         positioning is the structural fix for GAP-21.7-02."
    );
    let count = MAIN_RS.matches("reserved_rows: Some(").count();
    assert!(
        count >= 1,
        "INV-21.7-14 (c) / GAP-21.7-02: main.rs prompt-time \
         PromptRequest must carry `reserved_rows: Some(tui.reserved_row_count())`. \
         Found {}.",
        count
    );
}

#[test]
fn invariant_21_7_15_readline_barrier_on_render_loop() {
    // Plan 12 / GAP-21.7-02: the TUI render loop's status-row write
    // must be gated by `readline_active` so the 100ms ticker cannot
    // race the worker's prompt paint with its
    // SavePosition/MoveTo(bottom)/RestorePosition sequence.
    //
    //   (a) render.rs references `readline_active` at least twice
    //       (field declaration + load/check on render path).
    //   (b) main.rs calls `readline_active_handle()` at least once so
    //       the prompt-time site has a handle to toggle.
    //   (c) main.rs issues `.store(true` and `.store(false` at least
    //       once each — the bracketing toggle around the request.
    const RENDER: &str = include_str!("../src/tui/render.rs");
    let render_refs = RENDER.matches("readline_active").count();
    assert!(
        render_refs >= 2,
        "INV-21.7-15 (a) / GAP-21.7-02: render.rs must reference \
         `readline_active` at least twice (field decl + load on render \
         path). Found {}.",
        render_refs
    );
    let handle_calls = MAIN_RS.matches("readline_active_handle()").count();
    assert!(
        handle_calls >= 1,
        "INV-21.7-15 (b) / GAP-21.7-02: main.rs must call \
         `TuiHandle.readline_active_handle()` to obtain the barrier \
         flag at the prompt-time site. Found {}.",
        handle_calls
    );
    let store_true = MAIN_RS.matches(".store(true").count();
    let store_false = MAIN_RS.matches(".store(false").count();
    assert!(
        store_true >= 1 && store_false >= 1,
        "INV-21.7-15 (c) / GAP-21.7-02: main.rs must bracket the \
         prompt-time readline with `.store(true` BEFORE and \
         `.store(false` AFTER. Found store(true)={} store(false)={}.",
        store_true,
        store_false
    );
}
