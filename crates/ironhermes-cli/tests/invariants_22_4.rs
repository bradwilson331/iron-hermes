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
