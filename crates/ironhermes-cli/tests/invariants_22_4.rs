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
const CORE_REGISTRY: &str = include_str!("../../ironhermes-core/src/commands/registry.rs");

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

// INV-22.4-23 (mouse_capture_paired) was REPLACED by INV-22.4-29
// (mouse_capture_paired_with_toggle) per Plan 22.4-16 / UAT Gap 3 closure.
// The numbering set is now {00..22, 24..29} — non-contiguous; future plans
// should pick up at INV-22.4-30, NOT 23.

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

/// INV-22.4-29 (Phase 22.4 gap closure — UAT Gap 3): replaces the
/// pre-existing INV-22.4-23. The locked decision was: mouse capture stays
/// ON by default, with `/mouse on` and `/mouse off` slash commands as the
/// runtime escape hatch into terminal-native text selection. This invariant
/// asserts the THREE wiring sites required to honour that contract:
///   (a) EnableMouseCapture is invoked at run_chat_ratatui startup
///       (capture-on by default — preserves existing scroll-wheel UX).
///   (b) The `/mouse` slash command is recognised by dispatch_slash
///       (fast-path guard with `input.strip_prefix("/mouse")` in commands.rs).
///   (c) DisableMouseCapture is invoked from the slash dispatcher (i.e.
///       it appears in tui_rata/commands.rs, not just in the RAII guard
///       at event_loop.rs Drop impl).
/// See 22.4-UAT.md Gap 3 root_cause + missing list.
#[test]
fn invariant_22_4_29_mouse_capture_paired_with_toggle() {
    // (a) capture-on default at startup
    assert!(
        TUI_RATA_EVLOOP.contains("execute!(io::stdout(), EnableMouseCapture)"),
        "INV-22.4-29 (a): tui_rata/event_loop.rs must call \
         `execute!(io::stdout(), EnableMouseCapture)` at run_chat_ratatui \
         startup so capture is on by default. See 22.4-UAT.md Gap 3."
    );

    // (b) /mouse fast-path RETIRED in Phase 22.4.1 Plan 01 (D-10/D-11)
    assert!(
        !TUI_RATA_COMMANDS.contains("input.strip_prefix(\"/mouse\")"),
        "INV-22.4-29 (b) [INVERTED — Plan 22.4.1-01]: tui_rata/commands.rs \
         dispatch_slash must NOT contain the literal \
         `input.strip_prefix(\"/mouse\")` fast-path after Phase 22.4.1 \
         re-port. /mouse is now registered in the core registry (Plan \
         22.4.1-00) and routed via the post-router App-side hook \
         `if def.name == \"mouse\"` per D-10/D-11/D-12. See Plan 22.4.1-01 \
         and INV-22.4-34 for the complementary multi-name absence check."
    );

    // (c) DisableMouseCapture is reachable from the slash dispatcher
    assert!(
        TUI_RATA_COMMANDS.contains("DisableMouseCapture"),
        "INV-22.4-29 (c): tui_rata/commands.rs must invoke \
         `DisableMouseCapture` inside the `/mouse off` arm so users can \
         drop into terminal-native text selection without exiting the REPL. \
         See Plan 22.4-16 Task 2."
    );
    assert!(
        TUI_RATA_COMMANDS.contains("EnableMouseCapture"),
        "INV-22.4-29 (c): tui_rata/commands.rs must invoke \
         `EnableMouseCapture` inside the `/mouse on` arm so users can \
         restore scroll-wheel scrolling after toggling off. See Plan 22.4-16 \
         Task 2."
    );

    // Sanity: the old RAII guard's DisableMouseCapture in event_loop.rs is
    // still present (final cleanup safety net — independent of slash state).
    assert!(
        TUI_RATA_EVLOOP.contains("DisableMouseCapture"),
        "INV-22.4-29 sanity: tui_rata/event_loop.rs must still contain \
         DisableMouseCapture in MouseCaptureGuard's Drop impl as the \
         unconditional terminal-cleanup safety net."
    );

    // The shared state handle must be threaded through (Task 1 wiring).
    assert!(
        TUI_RATA_EVLOOP.contains("mouse_capture_enabled"),
        "INV-22.4-29 sanity: tui_rata/event_loop.rs must construct the \
         shared `mouse_capture_enabled: Arc<AtomicBool>` in build_app_deps \
         and assign it to AppDeps."
    );
    assert!(
        TUI_RATA_COMMANDS.contains("mouse_capture_enabled"),
        "INV-22.4-29 sanity: tui_rata/commands.rs must reference \
         `app.mouse_capture_enabled` from the slash handler so live state \
         stays in sync with the executed crossterm command."
    );
}

/// INV-22.4-30 (Phase 22.4 gap closure — UAT Round 2 Gap 4): System messages
/// must be VISIBLE in the rendered transcript, not silently dropped.
///
/// Bug history: through Plan 22.4-16, `role_style` in tui_rata/app.rs
/// returned `None` for `Role::System`. The `transcript_text` loop's
/// `let Some(color) = color else { continue };` short-circuit then SKIPPED
/// every System message — including all four SlashOutcome variants
/// (Handled, ClearSession, Unknown, Error) that `apply_slash_outcome` pushes
/// as System rows. Result: /help, /clear "Conversation cleared.", /new
/// "New session started.", /mouse on|off "Mouse capture: <state>", and the
/// typo-suggester "Did you mean ...?" hint were ALL invisible to the user.
///
/// Plan 22.4-17 (Option B from the user's locked decision) made System
/// visible: role_style returns `Some(Color::DarkGray)` for System and
/// transcript_text applies `Modifier::DIM` for System rows so they read
/// as metadata while remaining observable.
///
/// This invariant grep-locks both halves of the fix so a future refactor
/// cannot accidentally re-suppress System rendering.
#[test]
fn invariant_22_4_30_system_messages_visible() {
    // Half 1: role_style must NOT return None for any Role variant.
    // The literal `Role::System => ("System".to_string(), None)` is the
    // exact pattern the bug had — assert it is gone.
    assert!(
        !TUI_RATA_APP.contains("Role::System => (\"System\".to_string(), None)"),
        "INV-22.4-30: tui_rata/app.rs role_style must NOT return `None` for \
         Role::System. The let-else short-circuit in transcript_text would \
         silently drop every System row (slash-command confirmations, typo \
         suggester output, etc.). See 22.4-UAT.md Round 2 Gap 4 root_cause."
    );

    // Half 1b: role_style must explicitly return Some(Color::DarkGray) for
    // Role::System per the locked Plan 22.4-17 spec.
    assert!(
        TUI_RATA_APP.contains("Role::System => (\"System\".to_string(), Some(Color::DarkGray))"),
        "INV-22.4-30: tui_rata/app.rs role_style must return \
         `Some(Color::DarkGray)` for Role::System (Plan 22.4-17 locked \
         Option B fix). Distinct dim gray distinguishes System from User \
         (Cyan) / Hermes (Green) / Tool (Yellow)."
    );

    // Half 2: transcript_text must apply Modifier::DIM for System rows so
    // the row visually demotes as metadata. The conditional ensures only
    // System gets DIM (User/Assistant/Tool stay full-bright).
    assert!(
        TUI_RATA_APP.contains("matches!(msg.role, Role::System)"),
        "INV-22.4-30: tui_rata/app.rs transcript_text must conditionally \
         apply DIM ONLY for Role::System rows via `matches!(msg.role, \
         Role::System)`. See Plan 22.4-17 Task 1 Edit 3."
    );
    assert!(
        TUI_RATA_APP.contains("add_modifier(Modifier::DIM)"),
        "INV-22.4-30: tui_rata/app.rs transcript_text must apply \
         `add_modifier(Modifier::DIM)` to the System Style so System rows \
         render visually demoted from real conversation rows."
    );

    // Sanity: every other Role variant still returns Some(...) so the
    // let-else passes for them too. This is the structural assertion that
    // no Role variant accidentally regresses to None.
    for role_arm in &[
        "Role::User => (\"You\".to_string(), Some(Color::Cyan))",
        "Role::Assistant => (\"Hermes\".to_string(), Some(Color::Green))",
        "Role::Tool => (\"Tool\".to_string(), Some(Color::Yellow))",
    ] {
        assert!(
            TUI_RATA_APP.contains(role_arm),
            "INV-22.4-30 sanity: tui_rata/app.rs role_style must contain \
             `{role_arm}` so the existing User/Hermes/Tool render paths \
             are unchanged by Plan 22.4-17."
        );
    }

    // Sanity: apply_slash_outcome still pushes Role::System for the four
    // visible-output variants. If a future refactor stops setting the role,
    // System rendering is moot.
    let system_pushes = TUI_RATA_APP.matches("msg.role = Role::System;").count()
                      + TUI_RATA_APP.matches("system.role = Role::System;").count();
    assert!(
        system_pushes >= 4,
        "INV-22.4-30 sanity: tui_rata/app.rs apply_slash_outcome must push \
         Role::System for at least 4 SlashOutcome arms (Handled, \
         ClearSession, Unknown, Error). Found {system_pushes} sites."
    );
}

/// INV-22.4-31 (Phase 22.4 gap closure — UAT Round 2 Gap 5, updated Phase 22.4.2 Plan 01):
/// the high-traffic deferred handlers (/agents, /skills, /mcp, /sessions, /memory) MUST
/// be reachable through the wired dispatch path.
///
/// Phase 22.4.2 Plan 01 (D-01) collapses `invoke_handler` from a 30-arm match to a
/// single delegation to `core::handlers::dispatch`. After this change:
///   - The per-name `"agents" => CommandResult::Output(` arms are GONE (collapsed).
///   - The fast-path guards for /mcp, /sessions, /memory remain absent (already retired).
///   - Core::handlers::dispatch is the single dispatch table — it handles agents, skills,
///     mcp, sessions, memory via real handler bodies or todo_stub().
///   - The `not yet wired in Phase 22.4` fallback in invoke_handler is GONE (replaced
///     by the core dispatch fallback via `todo_stub`).
///
/// New assertions verify the D-01 architecture (one-line delegation).
#[test]
fn invariant_22_4_31_handler_coverage_high_traffic() {
    // D-01 architecture assertion: invoke_handler must delegate to core::handlers::dispatch.
    assert!(
        TUI_RATA_COMMANDS.contains("ironhermes_core::commands::handlers::dispatch("),
        "INV-22.4-31 (D-01): tui_rata/commands.rs invoke_handler must delegate to \
         `ironhermes_core::commands::handlers::dispatch(...)` after Phase 22.4.2 \
         Plan 01 collapse. The 30-arm match is replaced by one-line delegation."
    );

    // The per-name `"agents" => CommandResult::Output(` arms are GONE after collapse.
    // These were present in Phase 22.4.1 Plans 02; Phase 22.4.2 Plan 01 removes them.
    assert!(
        !TUI_RATA_COMMANDS.contains("\"agents\" => CommandResult::Output("),
        "INV-22.4-31 (a) [INVERTED — Plan 22.4.2-01]: tui_rata/commands.rs must NOT \
         contain `\"agents\" => CommandResult::Output(` after invoke_handler collapse \
         (D-01). /agents is dispatched via core::handlers::dispatch."
    );
    assert!(
        !TUI_RATA_COMMANDS.contains("\"skills\" => CommandResult::Output("),
        "INV-22.4-31 (a) [INVERTED — Plan 22.4.2-01]: tui_rata/commands.rs must NOT \
         contain `\"skills\" => CommandResult::Output(` after invoke_handler collapse \
         (D-01). /skills is dispatched via core::handlers::dispatch."
    );

    // fast-path guards for /mcp, /sessions, /memory remain absent (retired in Plan 22.4.1-01).
    for cmd in &["/mcp", "/sessions", "/memory"] {
        let needle = format!("input.strip_prefix(\"{cmd}\")");
        assert!(
            !TUI_RATA_COMMANDS.contains(&needle),
            "INV-22.4-31 (b) [INVERTED — Plan 22.4.1-01]: tui_rata/commands.rs \
             dispatch_slash must NOT contain the fast-path guard `{needle}`. \
             See Plan 22.4.1-01 and INV-22.4-34."
        );
    }

    // Sanity — render_help_router generates /help output from the core registry.
    // It must still be present so /help discoverability covers the full surface.
    assert!(
        TUI_RATA_COMMANDS.contains("render_help_router("),
        "INV-22.4-31 sanity: tui_rata/commands.rs must still call render_help_router \
         so /help output is generated from the registry. See Plan 22.4.1 D-13."
    );

    // Sanity — `not yet wired in Phase 22.4` fallback REMOVED after invoke_handler collapse.
    // The safety net is now core::handlers::dispatch's todo_stub() arm.
    assert!(
        !TUI_RATA_COMMANDS.contains("not yet wired in Phase 22.4"),
        "INV-22.4-31 sanity [INVERTED — Plan 22.4.2-01]: tui_rata/commands.rs must NOT \
         contain the `not yet wired in Phase 22.4` fallback after invoke_handler collapse. \
         The core::handlers::dispatch fallback (todo_stub) is the new safety net."
    );
}

/// INV-22.4-32 (Phase 22.4.1 Plan 00 — D-01 / D-14): the four new CommandDef
/// entries (mouse, mcp, sessions, memory) added to the core CommandRouter
/// registry MUST be present in registry.rs. Backstops D-01 — these names
/// are required by Plan 22.4.1-01 (which retires the four `strip_prefix`
/// fast-paths in tui_rata/dispatch_slash) and Plan 22.4.1-02 (which depends
/// on the router resolving them as ResolveResult::Exact so invoke_handler
/// receives the canonical def.name). A future refactor that removes any of
/// these entries would silently regress the unified dispatch contract.
#[test]
fn invariant_22_4_32_router_membership() {
    for name in &["mouse", "mcp", "sessions", "memory"] {
        let needle = format!("CommandDef::new(\"{name}\"");
        assert!(
            CORE_REGISTRY.contains(&needle),
            "INV-22.4-32: ironhermes-core/src/commands/registry.rs must \
             contain `{needle}` — the {name} command must be registered \
             in the core registry per Phase 22.4.1 Plan 00. See D-01."
        );
    }
}

/// INV-22.4-34 (Phase 22.4.1 Plan 01 — D-02 / D-11 / D-14): tui_rata/commands.rs
/// must contain ZERO occurrences of the four retired literal fast-path strings.
///
/// Phase 22.4.1 Plan 01 retires the `strip_prefix` fast-paths for /mouse, /mcp,
/// /sessions, /memory in `dispatch_slash`. The post-router App-side hook for
/// /mouse uses `strip_prefix(&format!("/{{}}", def.name))` (D-11) — a NON-literal
/// string interpolation that does NOT match the literal grep below. Therefore
/// each of the four literal strings below must appear ZERO times in
/// tui_rata/commands.rs after Plan 22.4.1-01 lands.
///
/// This is the multi-name companion to INV-22.4-29 sub-(b) (which only covers
/// /mouse). Together they backstop D-02's "zero strip_prefix fast-paths"
/// commitment.
#[test]
fn invariant_22_4_34_dispatch_slash_no_strip_prefix() {
    for literal in &[
        "strip_prefix(\"/mouse\")",
        "strip_prefix(\"/mcp\")",
        "strip_prefix(\"/sessions\")",
        "strip_prefix(\"/memory\")",
    ] {
        assert!(
            !TUI_RATA_COMMANDS.contains(literal),
            "INV-22.4-34: tui_rata/commands.rs must NOT contain `{literal}` \
             after Phase 22.4.1 Plan 01 re-port. The four slash fast-paths \
             are retired; the surviving `def.name`-interpolated args extraction \
             uses `strip_prefix(&format!(\"/{{}}\", def.name))` per D-11. See \
             Plan 22.4.1-01."
        );
    }
}

/// INV-22.4-33 (Phase 22.4.1 Plan 02 — D-05/D-08/D-14, INVERTED Phase 22.4.2 Plan 01 — D-01/D-10):
///
/// Phase 22.4.1 Plan 02 originally asserted that 26 per-name
/// `"<name>" => CommandResult::Output(` arms MUST be present in invoke_handler,
/// and that `Phase 22.4.1 stub:` markers appeared at least 26 times.
///
/// Phase 22.4.2 Plan 01 (D-01) collapses invoke_handler from a 30-arm match to a
/// one-line delegation to `core::handlers::dispatch`. After this change:
///
///   - All 26 per-name arm literals are GONE (invoke_handler has no match arms).
///   - All `Phase 22.4.1 stub:` markers are GONE (the stub text moved to todo_stub()
///     in core::handlers, which is NOT in tui_rata/commands.rs).
///   - `/voice` and `/prompt` keep their stub text in core::handlers::todo_stub()
///     (NOT in tui_rata/commands.rs).
///
/// This inverted test asserts:
///   (a) The 24 wired-command per-name arm literals are ABSENT from tui_rata/commands.rs.
///   (b) `Phase 22.4.1 stub:` appears ZERO times in tui_rata/commands.rs.
///   (c) Real handler functions exist in core/handlers.rs for the 5 StateStore commands.
#[test]
fn invariant_22_4_33_invoke_handler_arms() {
    // (a) All 26 per-name arm literals must be ABSENT after invoke_handler collapse.
    // These names were in the Phase 22.4.1 Plan 02 expected_arms list.
    let wired_names: &[&str] = &[
        // Session category
        "history", "save", "retry", "undo", "title", "compress", "rollback", "stop",
        "background", "btw", "queue", "status", "resume",
        // Configuration category
        "config", "provider", "prompt", "personality", "statusbar", "verbose",
        "yolo", "reasoning", "skin", "voice", "model", "fast", "debug",
    ];
    for name in wired_names {
        let needle = format!("\"{name}\" => CommandResult::Output(");
        assert!(
            !TUI_RATA_COMMANDS.contains(&needle),
            "INV-22.4-33 [INVERTED — Plan 22.4.2-01]: tui_rata/commands.rs must NOT \
             contain `{needle}` after invoke_handler collapse (D-01). The 30-arm \
             match is replaced by one-line delegation to core::handlers::dispatch. \
             See Phase 22.4.2 Plan 01 D-01."
        );
    }

    // (b) `Phase 22.4.1 stub:` must appear ZERO times in tui_rata/commands.rs.
    // All stub text is now in core::handlers::todo_stub() — NOT in tui_rata/commands.rs.
    let stub_count = TUI_RATA_COMMANDS.matches("Phase 22.4.1 stub:").count();
    assert!(
        stub_count == 0,
        "INV-22.4-33 [INVERTED — Plan 22.4.2-01]: tui_rata/commands.rs must contain \
         ZERO occurrences of `Phase 22.4.1 stub:` after invoke_handler collapse. \
         Found {stub_count}. The stub text lives in core::handlers::todo_stub() now."
    );

    // (c) Real handler function bodies exist in core handlers for the 5 StateStore commands.
    for fn_name in &["cmd_sessions", "cmd_resume", "cmd_save", "cmd_history"] {
        assert!(
            CORE_HANDLERS.contains(fn_name),
            "INV-22.4-33 (c): ironhermes-core/src/commands/handlers.rs must contain \
             `{fn_name}` — the real handler body added by Phase 22.4.2 Plan 01. \
             See D-03."
        );
    }
    // cmd_title exists from before Plan 01; assert it reads ctx.state_store now.
    assert!(
        CORE_HANDLERS.contains("ctx.state_store"),
        "INV-22.4-33 (c): ironhermes-core/src/commands/handlers.rs must contain \
         `ctx.state_store` — cmd_title and the new StateStore handlers read this \
         field (D-04/D-05). See Phase 22.4.2 Plan 01."
    );
}

const CORE_HANDLERS: &str =
    include_str!("../../ironhermes-core/src/commands/handlers.rs");

const CORE_CONTEXT: &str =
    include_str!("../../ironhermes-core/src/commands/context.rs");

/// INV-22.4-35 (Phase 22.4.2 Plan 00 — D-04 / D-14): the eight new
/// `Option<Arc<dyn ...>>` / `Option<Arc<std::sync::RwLock<...>>>` handle fields
/// added to `CommandContext` in Plan 00 MUST be present in context.rs.
///
/// Backstops D-04 — a future refactor that removes any of these fields would
/// silently break the plans that read them (Plans 01-04).
#[test]
fn invariant_22_4_35_command_context_field_membership() {
    let expected_fields: &[&str] = &[
        "mcp_manager",
        "memory_manager",
        "state_store",
        "provider_resolver",
        "context_compressor",
        "personality_overlay",
        "history",
        "agent_loop",
    ];
    for field in expected_fields {
        assert!(
            CORE_CONTEXT.contains(field),
            "INV-22.4-35: ironhermes-core/src/commands/context.rs must contain \
             the field `{field}` added by Phase 22.4.2 Plan 00 (D-04). \
             A future refactor must not remove it — Plans 01-04 read these \
             handles in their handler bodies."
        );
    }
    // Also assert the eight `with_<name>` builder methods exist.
    let expected_builders: &[&str] = &[
        "with_mcp_manager",
        "with_memory_manager",
        "with_state_store",
        "with_provider_resolver",
        "with_context_compressor",
        "with_personality_overlay",
        "with_history",
        "with_agent_loop",
    ];
    for builder in expected_builders {
        assert!(
            CORE_CONTEXT.contains(builder),
            "INV-22.4-35: ironhermes-core/src/commands/context.rs must contain \
             builder `fn {builder}(...)` added by Phase 22.4.2 Plan 00 (D-04). \
             Plans 01-04 use these builders in build_command_context."
        );
    }
}

/// INV-22.4-36 (Phase 22.4.2 Plan 00 — D-08 / D-09 / D-14): the ten new
/// fields added to `App` and `AppDeps` in Plan 00 MUST be present in app.rs.
///
/// Four subsystem handles (D-08): `state_store`, `resolver`,
/// `context_compressor`, `personality_overlay`.
/// Six toggle Arcs (D-09): `yolo_enabled` (UPGRADED), `verbose_enabled`,
/// `statusbar_enabled`, `debug_enabled`, `fast_enabled`, `skin`.
///
/// Backstops D-08 / D-09 — a future refactor that removes any of these fields
/// would silently break Plans 01-04 that read them from App.
#[test]
fn invariant_22_4_36_app_field_membership() {
    let expected_fields: &[&str] = &[
        // D-08 four subsystem handles
        "state_store",
        "resolver",
        "context_compressor",
        "personality_overlay",
        // D-09 six toggle Arcs (yolo_enabled is the upgrade; rest are new)
        "verbose_enabled",
        "statusbar_enabled",
        "debug_enabled",
        "fast_enabled",
        "skin",
    ];
    for field in expected_fields {
        assert!(
            TUI_RATA_APP.contains(field),
            "INV-22.4-36: crates/ironhermes-cli/src/tui_rata/app.rs must \
             contain the field `{field}` added by Phase 22.4.2 Plan 00 \
             (D-08/D-09). Plans 01-04 read these fields in build_command_context \
             and the tui_rata post-router hook."
        );
    }
    // yolo_enabled must be Arc<AtomicBool> (not plain bool) after the D-09 upgrade.
    assert!(
        TUI_RATA_APP.contains("yolo_enabled: Arc<AtomicBool>"),
        "INV-22.4-36: app.rs must declare `yolo_enabled: Arc<AtomicBool>` \
         (D-09 upgrade from plain `bool`). The post-router toggle hook uses \
         fetch_xor on this AtomicBool."
    );
}

// =============================================================================
// Phase 22.4.2 Plan 01 — INV-22.4-37 through INV-22.4-41
// Per-command static-grep INVs for the 5 StateStore commands (D-10).
// Each asserts:
//   (a) The stub marker is ABSENT from tui_rata/commands.rs (invoke_handler collapsed).
//   (b) The real handler function exists in core/handlers.rs.
// =============================================================================

/// INV-22.4-37 (Phase 22.4.2 Plan 01 — D-03/D-06/D-10): `/sessions` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"sessions" => CommandResult::Output(` —
///     the per-name arm was removed when invoke_handler collapsed to one-line delegation.
/// (b) `fn cmd_sessions` must exist in core/handlers.rs with a real body reading ctx.state_store.
#[test]
fn invariant_22_4_37_sessions_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"sessions\" => CommandResult::Output("),
        "INV-22.4-37 (a): tui_rata/commands.rs must NOT contain \
         `\"sessions\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_sessions("),
        "INV-22.4-37 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_sessions(` — real handler body added by Phase 22.4.2 Plan 01 (D-03)."
    );
    assert!(
        CORE_HANDLERS.contains("\"sessions\" => cmd_sessions("),
        "INV-22.4-37 (b): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"sessions\"` to `cmd_sessions(` — added by Phase 22.4.2 Plan 01."
    );
}

/// INV-22.4-38 (Phase 22.4.2 Plan 01 — D-03/D-06/D-10): `/resume` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"resume" => CommandResult::Output(`.
/// (b) `fn cmd_resume` must exist in core/handlers.rs.
#[test]
fn invariant_22_4_38_resume_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"resume\" => CommandResult::Output("),
        "INV-22.4-38 (a): tui_rata/commands.rs must NOT contain \
         `\"resume\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_resume("),
        "INV-22.4-38 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_resume(` — real handler body added by Phase 22.4.2 Plan 01 (D-03)."
    );
    assert!(
        CORE_HANDLERS.contains("\"resume\" => cmd_resume("),
        "INV-22.4-38 (b): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"resume\"` to `cmd_resume(` — added by Phase 22.4.2 Plan 01."
    );
}

/// INV-22.4-39 (Phase 22.4.2 Plan 01 — D-03/D-06/D-10): `/save` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"save" => CommandResult::Output(`.
/// (b) `fn cmd_save` must exist in core/handlers.rs.
#[test]
fn invariant_22_4_39_save_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"save\" => CommandResult::Output("),
        "INV-22.4-39 (a): tui_rata/commands.rs must NOT contain \
         `\"save\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_save("),
        "INV-22.4-39 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_save(` — real handler body added by Phase 22.4.2 Plan 01 (D-03)."
    );
    assert!(
        CORE_HANDLERS.contains("\"save\" => cmd_save("),
        "INV-22.4-39 (b): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"save\"` to `cmd_save(` — added by Phase 22.4.2 Plan 01."
    );
}

/// INV-22.4-40 (Phase 22.4.2 Plan 01 — D-03/D-06/D-10): `/history` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"history" => CommandResult::Output(`.
/// (b) `fn cmd_history` must exist in core/handlers.rs.
#[test]
fn invariant_22_4_40_history_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"history\" => CommandResult::Output("),
        "INV-22.4-40 (a): tui_rata/commands.rs must NOT contain \
         `\"history\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_history("),
        "INV-22.4-40 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_history(` — real handler body added by Phase 22.4.2 Plan 01 (D-03)."
    );
    assert!(
        CORE_HANDLERS.contains("\"history\" => cmd_history("),
        "INV-22.4-40 (b): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"history\"` to `cmd_history(` — added by Phase 22.4.2 Plan 01."
    );
}

/// INV-22.4-41 (Phase 22.4.2 Plan 01 — D-03/D-06/D-10): `/title` StateStore wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"title" => CommandResult::Output(`.
/// (b) `fn cmd_title` must exist in core/handlers.rs and must read ctx.state_store.
#[test]
fn invariant_22_4_41_title_state_store_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"title\" => CommandResult::Output("),
        "INV-22.4-41 (a): tui_rata/commands.rs must NOT contain \
         `\"title\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_title("),
        "INV-22.4-41 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_title(` — upgraded to read ctx.state_store in Phase 22.4.2 Plan 01."
    );
    // cmd_title must reference ctx.state_store (upgraded from stub in Plan 01).
    assert!(
        CORE_HANDLERS.contains("ctx.state_store"),
        "INV-22.4-41 (b): ironhermes-core/src/commands/handlers.rs cmd_title must \
         reference `ctx.state_store` to persist the title (D-04/D-05). \
         See Phase 22.4.2 Plan 01."
    );
}

// =============================================================================
// Phase 22.4.2 Plan 02 — INV-22.4-42 through INV-22.4-44
// Per-command static-grep INVs for the 3 ProviderResolver commands (D-10).
// Each asserts:
//   (a) The stub arm is ABSENT from tui_rata/commands.rs (invoke_handler collapsed in Plan 01).
//   (b) The real handler function exists in core/handlers.rs.
//   (c) The dispatch() routing arm exists in core/handlers.rs.
// =============================================================================

/// INV-22.4-42 (Phase 22.4.2 Plan 02 — D-03/D-06/D-10): `/model` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"model" => CommandResult::Output(` —
///     the per-name arm was absent before invoke_handler collapse (it was a stub),
///     and must remain absent after Plan 02 lands the real core handler.
/// (b) `fn cmd_model` must exist in core/handlers.rs reading ctx.provider_resolver.
/// (c) dispatch() must route `"model"` to `cmd_model(`.
#[test]
fn invariant_22_4_42_model_wired() {
    // (a) stub arm absent from tui_rata (invoke_handler is a one-liner post Plan 01)
    assert!(
        !TUI_RATA_COMMANDS.contains("\"model\" => CommandResult::Output("),
        "INV-22.4-42 (a): tui_rata/commands.rs must NOT contain \
         `\"model\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_model("),
        "INV-22.4-42 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_model(` — real handler body added by Phase 22.4.2 Plan 02 (D-03)."
    );
    // (c) dispatch routing arm present
    assert!(
        CORE_HANDLERS.contains("\"model\" => cmd_model("),
        "INV-22.4-42 (c): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"model\"` to `cmd_model(` — added by Phase 22.4.2 Plan 02."
    );
    // (d) handler reads ctx.provider_resolver (D-05 guard pattern)
    assert!(
        CORE_HANDLERS.contains("ctx.provider_resolver"),
        "INV-22.4-42 (d): ironhermes-core/src/commands/handlers.rs must reference \
         `ctx.provider_resolver` — cmd_model reads this handle (D-04/D-05). \
         See Phase 22.4.2 Plan 02."
    );
}

/// INV-22.4-43 (Phase 22.4.2 Plan 02 — D-03/D-06/D-10): `/provider` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"provider" => CommandResult::Output(`.
/// (b) `fn cmd_provider` must exist in core/handlers.rs reading ctx.provider_resolver.
/// (c) dispatch() must route `"provider"` to `cmd_provider(`.
#[test]
fn invariant_22_4_43_provider_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"provider\" => CommandResult::Output("),
        "INV-22.4-43 (a): tui_rata/commands.rs must NOT contain \
         `\"provider\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_provider("),
        "INV-22.4-43 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_provider(` — real handler body added by Phase 22.4.2 Plan 02 (D-03)."
    );
    // (c) dispatch routing arm present
    assert!(
        CORE_HANDLERS.contains("\"provider\" => cmd_provider("),
        "INV-22.4-43 (c): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"provider\"` to `cmd_provider(` — added by Phase 22.4.2 Plan 02."
    );
}

/// INV-22.4-44 (Phase 22.4.2 Plan 02 — D-03/D-06/D-10): `/fast` wire-up.
///
/// (a) tui_rata/commands.rs must NOT contain `"fast" => CommandResult::Output(`.
/// (b) `fn cmd_fast` must exist in core/handlers.rs reading ctx.provider_resolver.
/// (c) dispatch() must route `"fast"` to `cmd_fast(`.
#[test]
fn invariant_22_4_44_fast_wired() {
    // (a) stub arm absent from tui_rata
    assert!(
        !TUI_RATA_COMMANDS.contains("\"fast\" => CommandResult::Output("),
        "INV-22.4-44 (a): tui_rata/commands.rs must NOT contain \
         `\"fast\" => CommandResult::Output(` — stub arm removed by \
         Phase 22.4.2 Plan 01 invoke_handler collapse (D-01)."
    );
    // (b) real handler present in core
    assert!(
        CORE_HANDLERS.contains("fn cmd_fast("),
        "INV-22.4-44 (b): ironhermes-core/src/commands/handlers.rs must contain \
         `fn cmd_fast(` — real handler body added by Phase 22.4.2 Plan 02 (D-03)."
    );
    // (c) dispatch routing arm present
    assert!(
        CORE_HANDLERS.contains("\"fast\" => cmd_fast("),
        "INV-22.4-44 (c): ironhermes-core/src/commands/handlers.rs dispatch() must \
         route `\"fast\"` to `cmd_fast(` — added by Phase 22.4.2 Plan 02."
    );
    // (d) cmd_fast reads ctx.provider_resolver for fast_role_model() (D-05 guard pattern)
    assert!(
        CORE_HANDLERS.contains("fast_role_model"),
        "INV-22.4-44 (d): ironhermes-core/src/commands/handlers.rs cmd_fast must call \
         `fast_role_model()` on the ProviderResolverHandle (D-04/D-05). \
         See Phase 22.4.2 Plan 02."
    );
}
