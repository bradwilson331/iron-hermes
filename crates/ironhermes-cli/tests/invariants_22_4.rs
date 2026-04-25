//! Phase 22.4 static-grep regression gates.
//! Locks tui_rata/ wiring call sites + structural + ordering invariants.
//! Follows INV-21.7/22.1/22.3 `include_str!` pattern. No dev-deps.
//!
//! [Rule 1 - Bug] INV-22.4-04: plan spec used `run_agent_turn` but the actual
//! impl calls `agent.run(messages_snapshot)` via per-turn AgentLoop::new inside
//! `spawn_turn`. Updated grep to `agent.run` + `StreamEvent::Delta` which are
//! both present in event_loop.rs and correctly capture the streaming contract.

const MAIN_RS: &str = include_str!("../src/main.rs");
const TUI_RATA_MOD: &str = include_str!("../src/tui_rata/mod.rs");
const TUI_RATA_APP: &str = include_str!("../src/tui_rata/app.rs");
const TUI_RATA_EVLOOP: &str = include_str!("../src/tui_rata/event_loop.rs");
const TUI_RATA_UI: &str = include_str!("../src/tui_rata/ui.rs");
const TUI_RATA_STATUS: &str = include_str!("../src/tui_rata/status_line.rs");
const TUI_RATA_HISTORY: &str = include_str!("../src/tui_rata/history.rs");
const TUI_RATA_STREAM: &str = include_str!("../src/tui_rata/stream_events.rs");
const TUI_RATA_KB: &str = include_str!("../src/tui_rata/keybindings.rs");
const TUI_RATA_COMMANDS: &str = include_str!("../src/tui_rata/commands.rs");

/// WARNING-NEW-04 (iteration 2): friendly sentinel for Task-4 completeness.
/// Task 1 stub of commands.rs is ~500 bytes (SlashOutcome enum + stub fn).
/// Task 4 real impl is 4000+ bytes (dispatch_slash + suggest_typo + invoke_handler +
/// build_command_context + collect_known_command_names + render_help + map_core).
/// Threshold 1500 bytes sits safely between stub and real impl.
#[test]
fn invariant_22_4_00_commands_rs_task4_landed() {
    assert!(
        TUI_RATA_COMMANDS.len() > 1500,
        "INV-22.4-00 (WARNING-NEW-04): tui_rata/commands.rs is only {} bytes — plan 22.4-07 \
         Task 4 (full dispatch_slash impl) does not appear to have landed. The Task 1 stub \
         is ~500 bytes; the real implementation is ~4000+ bytes. Re-run plan 22.4-07 Task 4.",
        TUI_RATA_COMMANDS.len()
    );
}

#[test]
fn invariant_22_4_01_tui_rata_module_exports() {
    for submod in &[
        "pub mod app",
        "pub mod commands",
        "pub mod double_ctrl_c",
        "pub mod event_loop",
        "pub mod history",
        "pub mod keybindings",
        "pub mod knight_rider",
        "pub mod status_line",
        "pub mod stream_events",
        "pub mod ui",
    ] {
        assert!(
            TUI_RATA_MOD.contains(submod),
            "INV-22.4-01: tui_rata/mod.rs must declare `{submod};`"
        );
    }
    assert!(
        TUI_RATA_MOD.contains("pub use event_loop::run_chat_ratatui"),
        "INV-22.4-01: must re-export run_chat_ratatui"
    );
}

#[test]
fn invariant_22_4_02_classic_tui_flag_and_env_present() {
    assert!(MAIN_RS.contains("classic_tui"), "INV-22.4-02: classic_tui field");
    assert!(MAIN_RS.contains("IRONHERMES_CLASSIC_TUI"), "INV-22.4-02: env var");
}

#[test]
fn invariant_22_4_03_is_terminal_gate_both_fds() {
    assert!(
        MAIN_RS.contains("is_terminal()") || MAIN_RS.contains("IsTerminal"),
        "INV-22.4-03: IsTerminal"
    );
    assert!(
        MAIN_RS.contains("stdin()") && MAIN_RS.contains("stdout()"),
        "INV-22.4-03: gate on BOTH stdin AND stdout"
    );
}

/// [Rule 1 - Bug] Plan spec used `run_agent_turn` but actual impl calls
/// `agent.run(messages_snapshot)` inside `spawn_turn` using per-turn
/// `AgentLoop::new`. Updated to check `agent.run` which is present in
/// event_loop.rs and correctly captures the streaming contract.
#[test]
fn invariant_22_4_04_agent_loop_streaming_wired() {
    assert!(
        TUI_RATA_EVLOOP.contains("agent.run") || TUI_RATA_EVLOOP.contains("run_agent_turn"),
        "INV-22.4-04: event_loop.rs must call agent.run() or run_agent_turn for per-turn streaming"
    );
    assert!(TUI_RATA_EVLOOP.contains("StreamEvent::Delta"), "INV-22.4-04: StreamEvent::Delta");
}

#[test]
fn invariant_22_4_05_hook_registry_wired() {
    assert!(TUI_RATA_EVLOOP.contains("HookRegistry::new"), "INV-22.4-05");
    assert!(TUI_RATA_EVLOOP.contains("add_listener"), "INV-22.4-05: add_listener");
}

#[test]
fn invariant_22_4_06_mcp_manager_wired() {
    assert!(TUI_RATA_EVLOOP.contains("build_mcp_manager"), "INV-22.4-06");
}

#[test]
fn invariant_22_4_07_memory_manager_wired() {
    assert!(TUI_RATA_EVLOOP.contains("build_memory_manager"), "INV-22.4-07");
    assert!(TUI_RATA_EVLOOP.contains("register_memory_tool"), "INV-22.4-07");
}

#[test]
fn invariant_22_4_08_subagent_registry_wired() {
    assert!(TUI_RATA_EVLOOP.contains("SubagentRegistry::new"), "INV-22.4-08");
}

#[test]
fn invariant_22_4_09_process_registry_wired() {
    assert!(TUI_RATA_EVLOOP.contains("ProcessRegistry::new_for_session"), "INV-22.4-09");
}

#[test]
fn invariant_22_4_10_slash_router_wired() {
    assert!(
        TUI_RATA_EVLOOP.contains("CommandRouter") || TUI_RATA_EVLOOP.contains("build_command_registry"),
        "INV-22.4-10: CommandRouter"
    );
}

/// WARNING-08 (iteration 1) + BLOCKER-NEW-03 (iteration 2): typo suggester
/// wired in tui_rata/commands.rs + Enter-arm precheck in app.rs.
#[test]
fn invariant_22_4_11_typo_suggester_wired() {
    // commands.rs side (D-18 item 8 + BLOCKER-05)
    assert!(
        TUI_RATA_COMMANDS.contains("suggest_typo"),
        "INV-22.4-11: tui_rata/commands.rs must invoke suggest_typo"
    );
    assert!(
        TUI_RATA_COMMANDS.contains("Did you mean"),
        "INV-22.4-11: must surface `Did you mean` hint"
    );
    // app.rs side (BLOCKER-NEW-03 Enter-arm precheck)
    assert!(
        TUI_RATA_APP.contains("dispatch_slash"),
        "INV-22.4-11: app.rs must call dispatch_slash (BLOCKER-NEW-03)"
    );
    assert!(
        TUI_RATA_APP.contains("text.starts_with") || TUI_RATA_APP.contains(".starts_with('/')"),
        "INV-22.4-11: app.rs must detect slash prefix before submit() (BLOCKER-NEW-03)"
    );
}

#[test]
fn invariant_22_4_12_blocklist_guardrail_wired() {
    assert!(TUI_RATA_EVLOOP.contains("BlocklistGuardrail"), "INV-22.4-12");
}

#[test]
fn invariant_22_4_13_three_tool_registrations_wired() {
    assert!(TUI_RATA_EVLOOP.contains("register_cronjob_tool"), "INV-22.4-13 cron");
    assert!(TUI_RATA_EVLOOP.contains("register_skills_tool"), "INV-22.4-13 skills");
    assert!(TUI_RATA_EVLOOP.contains("register_execute_code_tool"), "INV-22.4-13 exec");
}

#[test]
fn invariant_22_4_14_yolo_banner_pre_alt_screen() {
    assert!(MAIN_RS.contains("print_yolo_banner_to_stderr"), "INV-22.4-14");
}

/// WARNING-03 (iteration 1): ≥ 2 child_token() calls.
#[test]
fn invariant_22_4_15_cancel_cascade_parent_and_child() {
    assert!(
        TUI_RATA_EVLOOP.contains("CancellationToken::new") || TUI_RATA_APP.contains("CancellationToken::new"),
        "INV-22.4-15: parent"
    );
    let total = TUI_RATA_EVLOOP.matches(".child_token()").count()
              + TUI_RATA_APP.matches(".child_token()").count();
    assert!(
        total >= 2,
        "INV-22.4-15: `.child_token()` must appear ≥ 2. Found {total}."
    );
}

#[test]
fn invariant_22_4_16_double_ctrl_c_wired() {
    assert!(TUI_RATA_APP.contains("DoubleCtrlCState::new"), "INV-22.4-16");
    assert!(TUI_RATA_APP.contains("CtrlCDecision::CancelTurn"), "INV-22.4-16");
}

#[test]
fn invariant_22_4_17_status_and_knight_rider_present() {
    assert!(TUI_RATA_STATUS.contains("StatusLineState"), "INV-22.4-17");
    assert!(TUI_RATA_UI.contains("knight_rider::frame"), "INV-22.4-17");
}

#[test]
fn invariant_22_4_18_ratatui_init_restore_paired() {
    assert!(TUI_RATA_EVLOOP.contains("ratatui::init()"), "INV-22.4-18 init");
    assert!(TUI_RATA_EVLOOP.contains("ratatui::restore()"), "INV-22.4-18 restore");
}

#[test]
fn invariant_22_4_19_event_stream_new_in_event_loop() {
    assert!(TUI_RATA_EVLOOP.contains("EventStream::new()"), "INV-22.4-19");
    assert!(!TUI_RATA_APP.contains("EventStream"), "INV-22.4-19: not on App");
}

#[test]
fn invariant_22_4_20_key_event_kind_press_filter() {
    assert!(TUI_RATA_APP.contains("KeyEventKind::Press"), "INV-22.4-20");
}

#[test]
fn invariant_22_4_21_stream_event_and_unbounded_sender() {
    assert!(TUI_RATA_STREAM.contains("pub enum StreamEvent"), "INV-22.4-21");
    assert!(
        TUI_RATA_EVLOOP.contains("unbounded_channel") || TUI_RATA_EVLOOP.contains("UnboundedSender"),
        "INV-22.4-21: unbounded"
    );
}

#[test]
fn invariant_22_4_22_unit_separator_codec_present() {
    assert!(
        TUI_RATA_HISTORY.contains(r"'\u{1F}'") || TUI_RATA_HISTORY.contains("\\u{1F}"),
        "INV-22.4-22"
    );
}

#[test]
fn invariant_22_4_23_mouse_capture_paired() {
    assert!(TUI_RATA_EVLOOP.contains("EnableMouseCapture"), "INV-22.4-23 enable");
    assert!(TUI_RATA_EVLOOP.contains("DisableMouseCapture"), "INV-22.4-23 disable");
}

/// WARNING-NEW-03 (iteration 2): classic registration order preserved.
/// Uses `.find()` for first-occurrence position comparison.
#[test]
fn invariant_22_4_24_registration_order_parity() {
    let find = |needle: &str| TUI_RATA_EVLOOP.find(needle);

    let agent_loop_pos = find("AgentLoop::new").expect(
        "INV-22.4-24: event_loop.rs must contain AgentLoop::new (D-18 item 1)"
    );

    let ordered_before: &[(&str, &str)] = &[
        ("HookRegistry::new",                  "D-18 item 2 — HookRegistry before AgentLoop::new"),
        ("build_memory_manager",               "D-18 item 4 — MemoryManager before AgentLoop::new"),
        ("build_mcp_manager",                  "D-18 item 3 — McpManager before AgentLoop::new"),
        ("ProcessRegistry::new_for_session",   "D-18 item 6 — ProcessRegistry before AgentLoop::new"),
        ("SubagentRegistry::new",              "D-18 item 5 — SubagentRegistry before AgentLoop::new"),
    ];
    for (needle, msg) in ordered_before {
        let pos = find(needle).unwrap_or_else(|| {
            panic!("INV-22.4-24: event_loop.rs must contain `{needle}` ({msg})")
        });
        assert!(
            pos < agent_loop_pos,
            "INV-22.4-24: `{needle}` must appear BEFORE `AgentLoop::new` ({msg}). \
             Found at {pos}; AgentLoop::new at {agent_loop_pos}."
        );
    }

    let status_pos = find("StatusLineState").expect(
        "INV-22.4-24: event_loop.rs must reference StatusLineState (D-10)"
    );
    let app_new_pos = find("App::new").expect(
        "INV-22.4-24: event_loop.rs must construct App via App::new"
    );
    assert!(
        status_pos < app_new_pos,
        "INV-22.4-24: StatusLineState seed must appear BEFORE App::new (D-10, D-18 item 14). \
         Status at {status_pos}; App::new at {app_new_pos}."
    );
}

/// INV-22.4-25 (Phase 22.4 gap closure — D-03): print_banner() must fire
/// in BOTH ratatui dispatch sites (Commands::Chat arm + bare-hermes arm)
/// BEFORE run_chat_ratatui() is called, so the banner lands in scrollback
/// pre-alt-screen. This is the static-grep companion to INV-22.4-14
/// (which locks the yolo banner). See 22.4-VERIFICATION.md Gap 1.
#[test]
fn invariant_22_4_25_print_banner_pre_ratatui() {
    // There must be at least 3 print_banner() call sites in main.rs:
    //   - classic run_chat (existing, line ~758)
    //   - Commands::Chat ratatui arm (new, Phase 22.4 plan 11)
    //   - bare-hermes ratatui arm (new, Phase 22.4 plan 11)
    let call_count = MAIN_RS.matches("print_banner();").count();
    assert!(
        call_count >= 3,
        "INV-22.4-25: main.rs must contain print_banner() in classic run_chat + \
         both ratatui dispatch branches (≥ 3 total call sites). Found {call_count}."
    );

    // Every run_chat_ratatui(...) dispatch site must be preceded by a print_banner()
    // call earlier in the file. Verified via first-occurrence ordering.
    let first_print_banner = MAIN_RS.find("print_banner();").expect(
        "INV-22.4-25: main.rs must contain print_banner();"
    );
    let first_run_chat_ratatui = MAIN_RS.find("run_chat_ratatui(").expect(
        "INV-22.4-25: main.rs must call tui_rata::run_chat_ratatui(...)"
    );
    assert!(
        first_print_banner < first_run_chat_ratatui,
        "INV-22.4-25: the first print_banner() must appear BEFORE the first \
         run_chat_ratatui( in main.rs. Found print_banner at {first_print_banner}, \
         run_chat_ratatui at {first_run_chat_ratatui}. D-03 requires banner \
         pre-alt-screen."
    );

    // The GAP-5 flush rationale is cross-cut: every new ratatui dispatch site
    // that calls print_banner() must also flush stdout + stderr. Grep-lock both.
    assert!(
        MAIN_RS.contains("io::stdout().flush().ok();"),
        "INV-22.4-25: main.rs must flush stdout after print_banner() (GAP-5, \
         classic run_chat line 763 precedent)."
    );
    assert!(
        MAIN_RS.contains("io::stderr().flush().ok();"),
        "INV-22.4-25: main.rs must flush stderr after print_banner() (GAP-5, \
         classic run_chat line 764 precedent)."
    );
}

/// INV-22.4-26 (Phase 22.4 gap closure — D-17 / CR-02): spawn_turn must
/// chain `.with_tool_progress(...)` and `.with_tool_result(...)` on the
/// per-turn AgentLoop builder so all 8 D-17 canonical StreamEvent variants
/// are reachable from production (not just from snapshot tests that directly
/// inject them via App::handle_stream_event). See 22.4-VERIFICATION.md Gap 2,
/// 22.4-REVIEW.md CR-02, and Plan 22.4-12 Tasks 1+2 (commits 8a39eed +
/// 8a9125b). This invariant is the carry-over from Plan 22.4-12 Task 3.
#[test]
fn invariant_22_4_26_tool_progress_wired() {
    assert!(
        TUI_RATA_EVLOOP.contains("with_tool_progress("),
        "INV-22.4-26 (D-17 / CR-02): tui_rata/event_loop.rs must call \
         AgentLoop::with_tool_progress(...) inside spawn_turn so the \
         tool-progress callback forwards StreamEvent::ToolCall + \
         StreamEvent::ToolProgress to the UI event loop. See Plan 22.4-12 \
         Task 2 (commit 8a9125b)."
    );
    assert!(
        TUI_RATA_EVLOOP.contains("with_tool_result("),
        "INV-22.4-26 (D-17 / CR-02): tui_rata/event_loop.rs must call \
         AgentLoop::with_tool_result(...) inside spawn_turn so the \
         tool-completion callback forwards StreamEvent::ToolResult to the \
         UI event loop. See Plan 22.4-12 Task 1+2 (commits 8a39eed + 8a9125b)."
    );
}

/// INV-22.4-27 (Phase 22.4 gap closure — D-17 / CR-02): the ToolCall,
/// ToolProgress, and ToolResult StreamEvent variants must be CONSTRUCTED
/// in tui_rata/event_loop.rs (i.e. in the production sender path), not only
/// HANDLED in app.rs. Before Plan 22.4-12 closure, app.rs had handle_stream_event
/// arms for all three but spawn_turn never sent them — so the production
/// code path was dead. This invariant locks the production-path emission
/// so a future refactor cannot silently break the contract. See
/// 22.4-VERIFICATION.md Gap 2 Data-Flow Trace.
#[test]
fn invariant_22_4_27_tool_variants_constructed() {
    for variant in &[
        "StreamEvent::ToolCall",
        "StreamEvent::ToolProgress",
        "StreamEvent::ToolResult",
    ] {
        assert!(
            TUI_RATA_EVLOOP.contains(variant),
            "INV-22.4-27 (D-17 / CR-02): tui_rata/event_loop.rs must construct \
             `{variant}` in a sender closure so the variant is emitted from \
             the production spawn_turn path. Dead-code handlers in app.rs \
             alone do not count. See Plan 22.4-12 Task 2 (commit 8a9125b)."
        );
    }
}

/// INV-22.4-28 (Phase 22.4 gap closure — UAT Gap 2 / D-18 items 1+5+10):
/// Locks the four new tool/registry wirings landed in Plan 22.4-15:
///   (a) WebSearchTool on the MAIN registry (top-level visible to LLM)
///   (b) WebReadTool on the MAIN registry (top-level visible to LLM)
///   (c) register_delegate_task_tool called on the MAIN registry (subagents)
///   (d) AgentLoop::with_fallback chained in spawn_turn (PROV-07 parity)
/// See 22.4-UAT.md Gap 2 root_cause + missing list.
#[test]
fn invariant_22_4_28_tool_registry_parity() {
    // (a) + (b) — Web tools on the MAIN registry. The string
    // `registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool))`
    // appears TWICE in event_loop.rs after Plan 22.4-15: once on the main
    // `registry` and once on the `rpc_registry` sub-tree. Either count >= 2
    // is acceptable; we assert >= 2 explicitly so the main-registry
    // registration cannot silently regress.
    let web_search_count = TUI_RATA_EVLOOP
        .matches("registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool))")
        .count();
    assert!(
        web_search_count >= 2,
        "INV-22.4-28 (a): tui_rata/event_loop.rs must register WebSearchTool \
         on BOTH the main `registry` AND the `rpc_registry` (>= 2 sites). \
         Found {web_search_count}. See 22.4-UAT.md Gap 2 (a)."
    );
    let web_read_count = TUI_RATA_EVLOOP
        .matches("registry.register(Box::new(ironhermes_tools::web_read::WebReadTool))")
        .count();
    assert!(
        web_read_count >= 2,
        "INV-22.4-28 (b): tui_rata/event_loop.rs must register WebReadTool \
         on BOTH the main `registry` AND the `rpc_registry` (>= 2 sites). \
         Found {web_read_count}. See 22.4-UAT.md Gap 2 (a)."
    );

    // (c) — register_delegate_task_tool called on the MAIN registry.
    assert!(
        TUI_RATA_EVLOOP.contains("registry.register_delegate_task_tool("),
        "INV-22.4-28 (c): tui_rata/event_loop.rs must call \
         registry.register_delegate_task_tool(...) inside build_app_deps to \
         match classic main.rs:500 + :978 (D-18 item 5 / AGENT-01..05). \
         See 22.4-UAT.md Gap 2 (b)."
    );
    assert!(
        TUI_RATA_EVLOOP.contains("AgentSubagentRunner::new("),
        "INV-22.4-28 (c): tui_rata/event_loop.rs must construct \
         AgentSubagentRunner inside build_app_deps so register_delegate_task_tool \
         receives a real runner (not a stub). Mirrors classic main.rs:491-499."
    );

    // (d) — with_fallback chained in spawn_turn.
    assert!(
        TUI_RATA_EVLOOP.contains(".with_fallback("),
        "INV-22.4-28 (d): tui_rata/event_loop.rs must chain \
         .with_fallback(fb_client) on the per-turn AgentLoop builder inside \
         spawn_turn so PROV-07 fallback parity with classic main.rs:631-637 \
         is restored. See 22.4-UAT.md Gap 2 (c)."
    );
    // The fallback_client identifier must appear in event_loop.rs in BOTH
    // build_app_deps (the let binding + AppDeps assignment) AND spawn_turn
    // (the clone + the if-let guard). Total >= 4 occurrences.
    let fallback_count = TUI_RATA_EVLOOP.matches("fallback_client").count();
    assert!(
        fallback_count >= 4,
        "INV-22.4-28 (d): the `fallback_client` identifier must appear >= 4 \
         times in event_loop.rs (build_app_deps let + AppDeps init + \
         spawn_turn clone + spawn_turn if-let). Found {fallback_count}."
    );
}
