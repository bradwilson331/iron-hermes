//! Event loop + terminal lifecycle for the tui_rata REPL (Phase 22.4).
//!
//! Composes:
//! - Terminal init/restore via `ratatui::init()` + `ratatui::restore()` (D-15)
//! - Alt-screen via ratatui::init (calls EnterAlternateScreen — D-01)
//! - Mouse capture RAII guard (D-01, D-14)
//! - Tracing subscriber swap to `tui_logger::TuiTracingSubscriberLayer` (Pitfall 2)
//! - 14-item D-18 parity wiring + 4-arm tokio::select! + per-turn spawn (D-16)
//! - Slash-dispatch wrapper (tui_rata/commands.rs)

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use ratatui::DefaultTerminal;
use tokio::sync::RwLock;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use ironhermes_core::types::MessageContent;

use crate::tui_rata::app::{App, AppDeps};
use crate::tui_rata::status_line::StatusLineState;
use crate::tui_rata::stream_events::StreamEvent;
use crate::tui_rata::ui::ui;

// ── RAII mouse capture guard ──────────────────────────────────────────────────

struct MouseCaptureGuard;
impl Drop for MouseCaptureGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture);
    }
}

// ── Public entry point ────────────────────────────────────────────────────────

/// Public entry point. D-03 default for `hermes chat`.
///
/// Lifecycle:
/// 1. Tracing subscriber swap (Pitfall 2, pre-ratatui)
/// 2. `ratatui::init()` — raw mode + EnterAlternateScreen + panic hook (D-15)
/// 3. `EnableMouseCapture` + RAII `MouseCaptureGuard` (D-01 — ratatui::init does NOT capture mouse)
/// 4. Build 14-item D-18 parity deps (build_app_deps)
/// 5. `run_app_inner` 4-arm tokio::select! (D-16)
/// 6. Guard drop → DisableMouseCapture; `ratatui::restore()` → LeaveAlternateScreen + disable_raw_mode
pub async fn run_chat_ratatui(
    cli: &crate::cli_args::Cli,
    initial: Option<String>,
    yolo: bool,
) -> Result<()> {
    install_tui_logger_subscriber();

    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let _mouse_guard = MouseCaptureGuard;

    let result = run_with_deps(&mut terminal, cli, initial, yolo).await;

    drop(_mouse_guard);
    ratatui::restore();
    result
}

// ── Tracing subscriber install ────────────────────────────────────────────────

/// Install `tui_logger::TuiTracingSubscriberLayer` before `ratatui::init()`.
///
/// Uses `try_init` so double-install in tests (or when the classic subscriber
/// is already installed) is a no-op rather than a panic (Pitfall 2).
fn install_tui_logger_subscriber() {
    use tracing_subscriber::prelude::*;
    let layer = tui_logger::TuiTracingSubscriberLayer;
    let registry = tracing_subscriber::registry().with(layer);
    let _ = registry.try_init();
    let _ = tui_logger::init_logger(tui_logger::LevelFilter::Trace);
    tui_logger::set_default_level(tui_logger::LevelFilter::Info);
}

// ── Main bootstrap ────────────────────────────────────────────────────────────

async fn run_with_deps(
    terminal: &mut DefaultTerminal,
    cli: &crate::cli_args::Cli,
    initial: Option<String>,
    yolo: bool,
) -> Result<()> {
    let deps = build_app_deps(cli, yolo).await?;
    let mut app = App::new(deps);

    if let Some(msg) = initial {
        app.load_history_entry(&msg);
        // submit() handles the slash-precheck defensively (plan 22.4-05 BLOCKER-NEW-03)
        app.submit();
    }

    // Capture the Arc before run_app_inner consumes &mut app so the borrow
    // checker is satisfied even if app is moved or dropped during cleanup.
    let registry = app.registry.clone();
    let result = run_app_inner(terminal, &mut app).await;
    // D-15 (Phase 27.1.1): fire on_session_end on every registered tool --
    // HexapodTcpTool overrides this to send stop + relax (D-14). The ratatui
    // path had no shutdown hook before Phase 27.1.1; this closes the gap.
    // Best-effort; do not propagate any panic out of the hook.
    // Read lock only; do NOT hold a write lock here (see RESEARCH Pitfall 6).
    registry.read().await.call_session_end_hooks();
    result
}

// ── D-18 parity wiring — 14 items ────────────────────────────────────────────

/// Port of classic main.rs::run_chat registration block.
///
/// Order preserved per Phase 22 D-08 parity contract:
/// ensure_home_dirs → yolo_banner → ProcessRegistry → SubagentRegistry →
/// MemoryManager → register_memory_tool → ToolRegistry (cron/skills/execute_code) →
/// BlocklistGuardrail → McpManager → HookRegistry → CommandRouter → AgentLoop.
///
/// Concrete identifiers — grep-verified iteration 2. All 14 D-18 items below.
async fn build_app_deps(cli: &crate::cli_args::Cli, yolo: bool) -> Result<AppDeps> {
    use ironhermes_agent::{
        AgentRuntime, AgentRuntimeInput, AnyClientVisionHandle,
        build_client as build_provider_client, build_main_client,
    };
    use ironhermes_core::commands::{
        CommandRouter, registry::build_registry as build_command_registry,
    };
    use ironhermes_core::{Config, ProviderResolver};

    // UAT Gap 3 (Phase 22.4 Plan 22.4-16): shared mouse-capture state. Initial
    // value `true` matches the EnableMouseCapture call at run_chat_ratatui.
    // The `/mouse on|off` slash command flips this AtomicBool AND executes
    // the corresponding crossterm command. The MouseCaptureGuard Drop impl
    // is the final cleanup — it unconditionally disables on REPL exit.
    let mouse_capture_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // D-18 item 11: yolo banner — fires before alt-screen if run_chat_ratatui is
    // called from plan 22.4-08's main.rs arm. Defensive fire here too (D-18 parity).
    if yolo {
        crate::yolo::print_yolo_banner_to_stderr(yolo);
    }

    // Session setup — D-08 parity: ensure home dirs before anything else.
    let hermes_home = ironhermes_core::get_hermes_home();
    for sub in &[
        "cron",
        "sessions",
        "logs",
        "hooks",
        "memories",
        "skills",
        "workspace",
        "subagent-transcripts",
    ] {
        std::fs::create_dir_all(hermes_home.join(sub))?;
    }
    ensure_home_dirs(&hermes_home)?;

    let config = Config::load().unwrap_or_default();
    let resolver = ProviderResolver::build(&config)?;

    // D-18 item 13: session_id (uuid)
    let session_id = uuid::Uuid::new_v4().to_string();
    let history_path = hermes_home.join("repl_history");

    // D-18 item 12: parent CancellationToken (session-scoped, Pitfall 6)
    let cancel_parent = CancellationToken::new();

    // D-18 item 6: ProcessRegistry — session-scoped (D-29 / D-24)
    let process_registry = Arc::new(RwLock::new(
        ironhermes_exec::process_registry::ProcessRegistry::new_for_session(session_id.clone()),
    ));

    // D-18 item 5: SubagentRegistry
    let subagent_registry = Arc::new(RwLock::new(
        ironhermes_agent::subagent_registry::SubagentRegistry::new(),
    ));

    // D-18 item 4: MemoryManager (Option — None when config.memory.memory_enabled=false)
    let memory_manager =
        ironhermes_agent::memory::factory::build_memory_manager(&config.memory).await?;

    // Phase 28.1-05: client is kept on App for /model and /fast slash-command
    // mutations (interactive mid-session model switching). The runtime builds its
    // own client internally; this one is only used for status-line seeding and
    // /model//fast rebuilds. max_turns config-drift fix: AgentRuntime sizes from
    // config.agent.max_iterations (not max_turns); see objective note.
    // D-18 item 1 (client for status-line + slash-command mutations):
    let client = if let Some(ref model) = cli.model {
        let provider = cli.provider.as_deref().unwrap_or(resolver.main_provider());
        build_provider_client(&resolver, provider, model)?
    } else {
        build_main_client(&resolver)?
    };

    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();

    // D-18 item 10: ToolRegistry + tool registrations
    let cron_dir = hermes_home.join("cron");
    let job_store = Arc::new(Mutex::new(ironhermes_cron::JobStore::open(cron_dir)?));
    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(ironhermes_core::SkillRegistry::load_with_config(
        &cwd,
        &config.skills,
    ));
    let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);

    // Phase 27.1.1 gap-01: use the canonical entry point so every default tool
    // (including hexapod_tcp) is automatically present without hand-rolling the list.
    // Skip the plain TerminalTool; wire the process-registry variant below so
    // background terminal spawns flow through drain_and_kill_session.
    let mut registry = ironhermes_tools::ToolRegistry::new();
    registry.register_defaults_except(&["terminal"]);
    registry.register_terminal_tool_with_process_registry(process_registry.clone());

    // Runtime-handle tools — registered separately because they need instances
    // that cannot be constructed inside the registry crate itself.
    registry.register_cronjob_tool(job_store.clone());
    registry.register_skills_tool(
        skill_registry.clone(),
        active_skills.clone(),
        credential_dir,
        std::collections::HashMap::new(),
    );

    if let Some(ref mgr) = memory_manager {
        registry.register_memory_tool(mgr.clone());
    }

    // Phase 28.1-05: AgentRuntime::from_config (built below) owns the production
    // subagent runner + semaphore. The TUI's local registry (used only for slash
    // commands / session-end hooks, NOT for turns) still needs delegate_task
    // registered so /tools list and /agents reflect the tool. We build a
    // lightweight runner for the TUI registry only (separate from the runtime's).
    let tui_subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.delegation.max_concurrent_children));
    let tui_subagent_runner = Arc::new(
        ironhermes_agent::AgentSubagentRunner::new(client.clone(), resolver.clone(), None)
            .with_subagent_registry(subagent_registry.clone())
            .with_transcript_scope(hermes_home.clone(), session_id.clone()),
    );

    registry.register_delegate_task_tool(
        tui_subagent_runner,
        tui_subagent_semaphore,
        memory_manager
            .clone()
            .map(|m| m as ironhermes_tools::memory_tool::SharedMemoryManager),
        config.delegation.clone(),
        Some(cancel_parent.clone()),
        None, // no progress callback in Phase 22.4 (status-pill integration is follow-up)
    );

    // RPC sub-registry (safe subset — no terminal, no execute_code)
    let mut rpc_registry = ironhermes_tools::ToolRegistry::new();
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
    if let Some(ref mgr) = memory_manager {
        rpc_registry.register_memory_tool(mgr.clone());
    }

    registry.register_execute_code_tool_with_process_registry(
        Arc::new(rpc_registry),
        config.exec.clone(),
        active_skills.clone(),
        process_registry.clone(),
    );

    // Phase 25.1 D-04: build shared browser session Arc and register all 11 browser_* tools.
    // Wired identically across run_chat / run_single / run_gateway / run_chat_ratatui (Phase 22 D-04 invariant).
    // Phase 25.1 GAP-8 closure (plan 25.1-19): mirror of run_chat (main.rs:1173-1184) into the rata REPL bootstrap.
    // Without this block, `ironhermes chat` (which dispatches to run_chat_ratatui) omits all 11 browser_* tools.
    let browser_session: std::sync::Arc<
        tokio::sync::Mutex<Option<ironhermes_tools::browser_session::BrowserSession>>,
    > = std::sync::Arc::new(tokio::sync::Mutex::new(None));
    let vision_handle = std::sync::Arc::new(AnyClientVisionHandle::new(std::sync::Arc::new(
        resolver.clone(),
    )));
    registry.register_browser_tools_with_vision(
        browser_session.clone(),
        std::sync::Arc::new(resolver.clone()),
        vision_handle,
        std::sync::Arc::new(config.clone()),
    );

    // D-18 item 9: BlocklistGuardrail (before Arc wrap — D-05)
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(ironhermes_hooks::BlocklistGuardrail::from_config(
            &hooks_config,
        )));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    // Phase 28.1-05: compute merged_tools directly (same logic as
    // build_app_runtime_bundle internally). Previously this was extracted from the
    // now-removed `initial_runtime_bundle` call; compute it here so it is available
    // for set_toolset_config, ToolsetSessionHandle, and prompt_builder below.
    let merged_tools = config.tools.clone().with_default_toolsets_merged();

    // Phase 27.1.1-gap-02: push the merged toolset config into the local TUI registry
    // so get_definitions() filters tools per config.yaml at session start (same
    // semantics as build_app_runtime_bundle does for the non-TUI entry points).
    registry.set_toolset_config(Some(merged_tools.clone()));

    let registry = Arc::new(RwLock::new(registry));

    // Phase 25.2 Plan 15 follow-up (UAT Issue 2 / Symptom 1): construct the
    // production `ToolsetSessionHandle` for the ratatui REPL's slash dispatch
    // (`/toolset list/show/enable/disable`). Plan 15 wired this in
    // run_chat / run_single / run_gateway but missed run_chat_ratatui — the
    // default `hermes chat` entry since Phase 22.4. Without this, the REPL
    // returns "/toolset: toolset session handle not configured" because
    // `build_command_context` in tui_rata/commands.rs never attaches the
    // handle to CommandContext.
    // Phase 27.1.1-gap-02: use merged_tools (not raw config.tools) so
    // /toolset enable|disable mutates from the same baseline as the registry filter.
    let toolset_session: Arc<dyn ironhermes_core::commands::context::ToolsetSessionHandle> =
        Arc::new(ironhermes_tools::RegistryToolsetSession::new(
            registry.clone(),
            merged_tools.clone(),
        ));

    // Phase 25.3 D-W-1 / D-W-2: resolve workspace from cwd at session start
    // (frozen-snapshot pattern — Workspace never changes mid-session).
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|cwd| ironhermes_core::workspace::resolve_from_cwd(&cwd))
        .map(Arc::new);

    // Phase 25.3 D-T-2 / D-T-3: open TrajectoryWriter at workspace-scoped or global
    // path. Path = <workspace>/.ironhermes/sessions/<id>/trajectories.jsonl when a
    // Workspace is resolved, else ~/.ironhermes/sessions/<id>/trajectories.jsonl.
    // Uses the same session_id as the StateStore canonical UUID (resolved at L143).
    let trajectory_writer: Option<
        Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>,
    > = {
        let traj_dir = match &workspace {
            Some(ws) => ws
                .root
                .join(".ironhermes")
                .join("sessions")
                .join(&session_id),
            None => hermes_home.join("sessions").join(&session_id),
        };
        let traj_path = traj_dir.join("trajectories.jsonl");
        match ironhermes_trajectory::TrajectoryWriter::open(&traj_path) {
            Ok(w) => {
                // Plan 6 cycle-break: wrap the writer in TrajectoryWriterHandleImpl
                // so the handle satisfies Arc<dyn TrajectoryWriterHandle>.
                let arc_writer = Arc::new(std::sync::Mutex::new(w));
                let handle: Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle> =
                    Arc::new(ironhermes_trajectory::TrajectoryWriterHandleImpl::new(
                        arc_writer,
                    ));
                Some(handle)
            }
            Err(e) => {
                tracing::warn!(error = %e, path = %traj_path.display(),
                    "Phase 25.3: failed to open trajectory writer; per-tool-call ledger disabled for this session");
                None
            }
        }
    };

    // D-18 item 3: McpManager (Option<Arc<McpManager>>)
    let mcp_manager = build_mcp_manager(&config, registry.clone()).await;

    // D-18 item 2: HookRegistry + listeners (JSONL + webhooks + drain_retry_queue)
    let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());
    if hooks_config.event_log.enabled {
        let log_path = hooks_config
            .event_log
            .path
            .as_ref()
            .map(std::path::PathBuf::from);
        hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
    }
    let retry_queue = Arc::new(ironhermes_hooks::RetryQueue::new(
        ironhermes_hooks::RetryQueue::default_path(),
    )?);
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(ironhermes_hooks::create_webhook_listener(
            endpoint.clone(),
            retry_queue.clone(),
        ));
    }
    let hook_registry = Arc::new(hook_registry);
    let default_ttl = hooks_config
        .webhooks
        .first()
        .and_then(|e| e.queue_ttl_hours)
        .unwrap_or(24);
    ironhermes_hooks::drain_retry_queue(retry_queue, &hooks_config.webhooks, default_ttl).await;

    // D-18 item 7: CommandRouter from build_command_registry
    let command_router = Arc::new(CommandRouter::new(build_command_registry()));

    // Phase 28.1-05: Build one AgentRuntime per session. It owns the budget
    // (sized from config.agent.max_iterations — fixes the max_turns config drift),
    // tool registry, browser session, skills, and hook registry. spawn_turn will
    // call runtime.run_turn per turn (budget resets automatically at that boundary).
    //
    // NOTE: The TUI builds its own ToolRegistry above (with TUI-specific wiring
    // like terminal-with-process-registry, execute_code, browser tools). We pass
    // that registry clone via a separate channel and the runtime will hold it
    // through its bundle. However AgentRuntimeInput constructs its own bundle
    // (including a fresh registry) via build_app_runtime_bundle. To avoid
    // duplicate registry construction we store the pre-built registry on App
    // alongside agent_runtime for slash-dispatch and session-end hooks.
    // The runtime's run_turn uses runtime.bundle.registry which is built inside
    // from_config; the TUI registry stored on App is the one built above.
    //
    // DECISION (Phase 28.1-05): The TUI's pre-built registry (with its custom
    // tool set) is passed as the canonical registry. AgentRuntime::from_config
    // builds its own bundle internally. We store the pre-built TUI registry on
    // App for slash-dispatch and session-end hooks. The runtime uses the same
    // Arc<RwLock<ToolRegistry>> it builds internally via build_app_runtime_bundle;
    // since both registries share the same config, tool behaviour is equivalent.
    // The browser_session Arc on App tracks the TUI-side browser state.
    let agent_runtime = Arc::new(
        AgentRuntime::from_config(AgentRuntimeInput {
            config: Arc::new(config.clone()),
            resolver: Arc::new(resolver.clone()),
            cwd: cwd.clone(),
            process_registry: process_registry.clone(),
            // AgentRuntimeInput.memory_manager takes Arc<TokioMutex<MemoryManager>> directly;
            // from_config does the SharedMemoryManager cast internally.
            memory_manager: memory_manager.clone(),
            hooks_config: hooks_config.clone(),
            emit_mcp_startup_logs: true,
            subagent_registry: subagent_registry.clone(),
            transcript_scope: (hermes_home.clone(), session_id.clone()),
            subagent_progress_callback: None,
            subagent_cancel_token: Some(cancel_parent.clone()),
        })
        .await?,
    );

    // D-18 item 14: StatusLineState initial seed
    let status_initial = StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        hint: "ctrl+c cancel · /help commands".to_string(),
        ..Default::default()
    };

    // Phase 22.4.2 Plan 00: D-08 four subsystem handles
    // Phase 25.3-13 CR-01 close-out: persist a sessions row at REPL session start.
    // Without this, /sessions, /resume, /history, /export-session, and the
    // workspace_root filter all fail on the default chat surface.
    let state_store = match ironhermes_state::StateStore::open_default() {
        Ok(mut s) => {
            // Phase 25.3-16 CR-03: canonical_root_string for non-UTF-8 parity with the
            // prompt-line and /sessions --workspace filter (single source of truth).
            // workspace was resolved at line 309 (see above in this function).
            let workspace_root_canon = workspace.as_ref().map(|ws| ws.canonical_root_string());
            if let Err(e) = s.create_session(
                &session_id,
                "cli-repl",
                Some(client.model()),
                None,
                None,
                workspace_root_canon.as_deref(),
            ) {
                // Best-effort: log and continue with None state_store. /sessions,
                // /resume, etc. will report "session storage not configured".
                tracing::warn!(
                    error = %e,
                    "Phase 25.3-13: failed to persist REPL session row to state.db; \
                     /sessions and /resume will not see this session"
                );
                None
            } else {
                Some(Arc::new(std::sync::Mutex::new(s)))
            }
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "Phase 25.3-13: failed to open state.db for REPL; session persistence disabled"
            );
            None
        }
    };

    // Phase 25.3-13 CR-04 close-out: construct a PromptBuilder so the durable
    // [Workspace: <root>] Identity-slot line is injected into the REPL's system
    // message — same pattern as run_chat in main.rs:846-864. The system message
    // is seeded into app.history at App::from_deps so the per-turn AgentLoop's
    // messages_snapshot (event_loop.rs:608) carries it to every turn.
    //
    // Mirrors run_chat field-for-field except for `source = "cli-repl"`.
    let system_message: Option<ironhermes_core::types::ChatMessage> = {
        let mut prompt_builder =
            ironhermes_agent::prompt_builder::PromptBuilder::new(client.model(), "cli-repl");
        // Identity-slot workspace line — frozen at session start; never mutated mid-session
        // (D-W-1 frozen-snapshot pattern). Cache-stable in the durable slot 1.
        if let Some(ref ws) = workspace {
            prompt_builder = prompt_builder.with_workspace_root(&ws.root);
        }
        prompt_builder.set_skill_registry(skill_registry.clone());
        if let Some(ref mgr) = memory_manager {
            prompt_builder.set_memory_manager(mgr.clone());
        }
        prompt_builder.set_user_profile_enabled(config.memory.user_profile_enabled);
        // Phase 27.1.1-gap-02: populate active_toolsets so the system-prompt skills
        // catalog text reflects the same enabled set as the API tool schemas.
        prompt_builder.set_active_toolsets(merged_tools.enabled_toolset_names());
        prompt_builder.load_memory().await;
        prompt_builder.load_skills();
        Some(prompt_builder.build_system_message())
    };

    // PersonalityRegistry: load built-ins + any custom presets from hermes_home.
    let personality_overlay = Arc::new(ironhermes_agent::personality::PersonalityRegistry::load(
        &std::collections::HashMap::new(),
        &hermes_home,
    ));

    // Phase 22.4.2 Plan 00: D-09 session-toggle Arc fields
    let yolo_enabled = Arc::new(std::sync::atomic::AtomicBool::new(yolo));
    let verbose_enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let statusbar_enabled = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let debug_enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fast_enabled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let skin = Arc::new(std::sync::RwLock::new("default".to_string()));

    Ok(AppDeps {
        agent_runtime,
        hook_registry,
        mcp_manager,
        memory_manager,
        subagent_registry,
        process_registry,
        command_router,
        session_id,
        history_path,
        status_initial,
        cancel_parent,
        client,
        registry,
        browser_session: browser_session.clone(),
        mouse_capture_enabled,
        // Phase 22.4.2 Plan 00: D-08 subsystem handles
        state_store,
        resolver,
        context_compressor: None,
        personality_overlay,
        // Phase 22.4.2 Plan 00: D-09 toggle Arcs
        yolo_enabled,
        verbose_enabled,
        statusbar_enabled,
        debug_enabled,
        fast_enabled,
        skin,
        // Phase 25.2 Plan 15 follow-up: the wireup the original plan missed
        toolset_session: Some(toolset_session),
        // Phase 25.3 D-W-2 / D-T-3: resolved Workspace + TrajectoryWriter handle
        workspace,
        trajectory_writer,
        // Phase 25.3-13 CR-04: pre-built system message containing the durable
        // [Workspace: <root>] Identity-slot line. Seeded into App.history at
        // App::new so the per-turn AgentLoop sees it via messages_snapshot.
        system_message,
        // Phase 21.8.2: forward skill registry to App.
        skill_registry: Some(skill_registry.clone()),
        // Phase 21.8.2 Plan 03: SkillsConfig for hot-reload arm + pending overlays buffer.
        skills_config: config.skills.clone(),
        pending_skill_overlays: Vec::new(),
    })
}

/// Create subdirectories under hermes_home (D-21 / ensure_home_dirs parity).
fn ensure_home_dirs(hermes_home: &std::path::Path) -> Result<()> {
    for sub in &[
        "cron",
        "sessions",
        "logs",
        "hooks",
        "memories",
        "skills",
        "workspace",
        "subagent-transcripts",
    ] {
        std::fs::create_dir_all(hermes_home.join(sub))?;
    }
    Ok(())
}

/// Build and start an McpManager if the config has MCP servers configured.
/// Returns `Some(Arc<McpManager>)` when ≥1 enabled server is configured.
async fn build_mcp_manager(
    config: &ironhermes_core::Config,
    registry: Arc<RwLock<ironhermes_tools::ToolRegistry>>,
) -> Option<Arc<ironhermes_mcp::McpManager>> {
    use std::collections::HashMap;
    let mcp_configs: HashMap<String, ironhermes_mcp::McpServerConfig> = config
        .mcp_servers
        .iter()
        .filter_map(|(name, val)| {
            serde_yaml::from_value::<ironhermes_mcp::McpServerConfig>(val.clone())
                .ok()
                .map(|cfg| (name.clone(), cfg))
        })
        .collect();

    if mcp_configs.is_empty() {
        return None;
    }

    // McpManager::new(registry) then start_all(configs) per manager.rs:62,76
    let manager = ironhermes_mcp::McpManager::new(registry);
    manager.start_all(mcp_configs).await;
    Some(Arc::new(manager))
}

// ── 4-arm tokio::select! event loop ──────────────────────────────────────────

async fn run_app_inner(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    use crossterm::event::EventStream;
    use tokio::{signal, time};
    use tokio_stream::StreamExt;

    let mut events = EventStream::new(); // Pitfall 10 — local to fn, not on App

    let mut tick = time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    let ctrl_c = signal::ctrl_c(); // Pitfall 6 — created ONCE outside loop, then pinned
    tokio::pin!(ctrl_c);

    loop {
        let size = terminal.size()?;
        let transcript_area = compute_transcript_area(size);

        // Per-turn spawn: submit() sets pending_tx; we pick it up here and spawn.
        if app.pending_tx.is_some() {
            if let Some(cancel) = app.cancel_child.clone() {
                let tx = app.pending_tx.take().expect("checked above");
                spawn_turn(app, tx, cancel);
            }
        }

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(ev)) => app.handle_event(ev, transcript_area),
                Some(Err(e)) => { tracing::warn!("terminal event error: {e}"); }
                None => { app.should_quit = true; }
            },
            Some(se) = recv_pending(app) => app.handle_stream_event(se),
            _ = &mut ctrl_c => app.handle_ctrl_c_signal(),
            _ = tick.tick() => app.on_tick(),
        }

        app.reconcile_scroll(transcript_area);
        terminal.draw(|f| ui(f, app))?;

        if app.should_quit {
            let _ = app.history_store.save(&app.history_path);
            break;
        }
    }
    Ok(())
}

/// Await the next StreamEvent from the pending receiver, or `future::pending()`
/// when no turn is running (keeps the select! arm from busy-spinning).
async fn recv_pending(app: &mut App) -> Option<StreamEvent> {
    match app.pending_rx.as_mut() {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Compute the transcript chunk area by mirroring the 4-chunk layout from ui.rs.
///
/// Used by `run_app_inner` to pass `transcript_area` to `reconcile_scroll`.
fn compute_transcript_area(size: ratatui::prelude::Size) -> ratatui::layout::Rect {
    use ratatui::layout::{Constraint, Direction, Layout, Rect};
    let frame_area = Rect {
        x: 0,
        y: 0,
        width: size.width,
        height: size.height,
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .split(frame_area);
    chunks[0]
}

// ── Per-turn spawn (approach 3: duplicate AgentLoop builder) ──────────────────

/// Spawn an agent turn via `AgentRuntime::run_turn` (Phase 28.1-05).
///
/// Replaces the per-turn `AgentLoop` builder approach used before this plan.
/// `runtime.run_turn` resets the shared budget at the turn boundary (fixes the
/// latent TUI latch — T-28.1-11), handles fallback wiring, and attaches the
/// context engine internally.
///
/// Streaming deltas + tool lifecycle flow via `UnboundedSender<StreamEvent>`.
/// All 8 D-17 canonical variants are emitted (Phase 22.4 gap closure Plan 22.4-12):
///   - Lifecycle: Started, Finished, Cancelled, Error
///   - Streaming: Delta
///   - Tool: ToolCall, ToolProgress, ToolResult
fn spawn_turn(app: &App, tx: UnboundedSender<StreamEvent>, cancel: CancellationToken) {
    let runtime = app.agent_runtime.clone();
    let trajectory_writer = app.trajectory_writer.clone(); // Phase 25.3 D-T-3
    let cancel_token = cancel.clone();
    let mut messages_snapshot = app.history.clone();

    // Phase 21.8.3.1 D-03 / D-04 / D-06: inject active personality overlay
    // into the per-turn system message clone. Mutates messages_snapshot only;
    // app.history[0] is never touched. Field is session-persistent — re-read
    // every turn, never cleared by spawn_turn.
    if let Some(overlay_text) = &app.active_personality_overlay {
        if !messages_snapshot.is_empty() {
            if let Some(MessageContent::Text(ref mut s)) = messages_snapshot[0].content {
                s.push_str("\n\n");
                s.push_str(overlay_text);
            }
        }
    }
    let session_id = app.session_id.clone();

    tokio::spawn(async move {
        let _ = tx.send(StreamEvent::Started);

        // Build streaming + tool callbacks that forward to the UI event loop.
        // Phase 22.4 D-17 / CR-02 gap closure: all 3 callback types preserved.
        let tx_delta = tx.clone();
        let streaming_cb: ironhermes_agent::agent_loop::StreamCallback =
            Box::new(move |chunk: &str| {
                let _ = tx_delta.send(StreamEvent::Delta(chunk.to_string()));
            });

        // Emit BOTH ToolCall (status-pill hint) AND ToolProgress (args preview).
        let tx_tool_progress = tx.clone();
        let tool_progress_cb: ironhermes_agent::agent_loop::ToolProgressCallback =
            Box::new(move |name: &str, phase: &str| {
                let _ = tx_tool_progress.send(StreamEvent::ToolCall {
                    name: name.to_string(),
                });
                let _ = tx_tool_progress.send(StreamEvent::ToolProgress {
                    name: name.to_string(),
                    phase: phase.to_string(),
                });
            });

        // Fires once per tool completion (6 ToolCompleted sites in AgentLoop).
        let tx_tool_result = tx.clone();
        let tool_result_cb: ironhermes_agent::agent_loop::ToolResultCallback =
            Box::new(move |name: &str, ok: bool| {
                let _ = tx_tool_result.send(StreamEvent::ToolResult {
                    name: name.to_string(),
                    ok,
                });
            });

        // Phase 28.1-05: Build TurnRequest and call run_turn.
        // run_turn resets the budget, builds AgentLoop, attaches context engine,
        // wires fallback — all durable resources stay in the runtime.
        // browser_session and memory_manager are DURABLE (runtime owns them).
        // compression and context_length are DURABLE (runtime owns them).
        // fallback is DURABLE (run_turn calls wire_fallback_if_configured).
        // TUI carries no per-session compression_count or pressure_tracker;
        // leave them at default (0 / None) as documented in plan interfaces.
        let request = ironhermes_agent::TurnRequest {
            messages: messages_snapshot,
            session_id,
            cancel_token: Some(cancel_token.clone()),
            stream: Some(streaming_cb),
            tool_progress: Some(tool_progress_cb),
            tool_result: Some(tool_result_cb),
            trajectory_writer,
            pressure_tracker: None,
            state_store: None,
            compression_count: 0,
        };

        let result = runtime.run_turn(request).await;

        let terminal_event = match result {
            Ok(_) => StreamEvent::Finished,
            Err(_) if cancel_token.is_cancelled() => StreamEvent::Cancelled,
            Err(e) => StreamEvent::Error(e.to_string()),
        };
        let _ = tx.send(terminal_event);
    });
}

#[cfg(test)]
mod tests {
    /// INV-25.1-19: Phase 25.1 GAP-8 closure.
    /// The rata chat REPL bootstrap MUST register browser tools and wire the
    /// shared Arc into BOTH the App-level AgentLoop AND the per-turn AgentLoop
    /// in spawn_turn. Without these wirings, `ironhermes chat` omits all 11
    /// browser_* tools (the GAP-8 root cause).
    #[test]
    fn inv_25_1_gap8_browser_tools_wired_in_rata_chat() {
        let source = include_str!("event_loop.rs");
        // Filter comments to dodge the self-invalidating-grep-gate trap.
        let non_comment: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        let reg_count = non_comment
            .matches("register_browser_tools_with_vision(")
            .count();
        assert!(
            reg_count >= 1,
            "Phase 25.1 GAP-8 (plan 25.1-19): rata bootstrap MUST call \
             register_browser_tools_with_vision in build_app_deps; got {} non-comment calls",
            reg_count
        );

        // Plan-14 Arc<Config> threading: the call MUST receive Arc::new(config.clone()) as its 4th arg.
        let cfg_count = non_comment.matches("Arc::new(config.clone())").count();
        assert!(
            cfg_count >= 1,
            "Phase 25.1 GAP-8 + plan 25.1-14: register_browser_tools_with_vision in the \
             rata bootstrap MUST receive Arc::new(config.clone()) so allowlist (D-15) and \
             yolo gating (D-13) reach the chat REPL's browser tools; got {} occurrences",
            cfg_count
        );

        // Both AgentLoop builders MUST chain .with_browser_session(...) — one in build_app_deps,
        // one in spawn_turn. So we expect at least 2 occurrences.
        let with_count = non_comment.matches(".with_browser_session(").count();
        assert!(
            with_count >= 2,
            "Phase 25.1 GAP-8 (plan 25.1-19): BOTH the App-level AgentLoop (build_app_deps) \
             AND the per-turn AgentLoop (spawn_turn) MUST chain .with_browser_session(); \
             got {} occurrences",
            with_count
        );
    }

    /// Phase 25.1 GAP-8 behavioral test: verify that calling register_browser_tools_with_vision
    /// with the same 4-arg call shape used in build_app_deps produces a registry containing
    /// all 11 browser_* tools. This is the 2nd layer of the regression net:
    /// registry.rs locks the registration function (plan 09);
    /// this test locks the rata-side call site (this plan).
    #[test]
    fn rata_bootstrap_registry_contains_all_11_browser_tools() {
        use ironhermes_agent::AnyClientVisionHandle;
        use ironhermes_core::{Config, provider::ProviderResolver};
        use ironhermes_tools::ToolRegistry;
        use std::sync::Arc;

        let mut registry = ToolRegistry::new();
        let config = Config::default();
        let resolver = ProviderResolver::build(&config)
            .expect("ProviderResolver::build with default Config must not fail in test context");

        let browser_session = Arc::new(tokio::sync::Mutex::new(None));
        let vision_handle = Arc::new(AnyClientVisionHandle::new(Arc::new(resolver.clone())));

        registry.register_browser_tools_with_vision(
            browser_session,
            Arc::new(resolver),
            vision_handle,
            Arc::new(config),
        );

        let names: std::collections::HashSet<String> = registry
            .list_tools()
            .into_iter()
            .map(|s| s.to_string())
            .collect();

        for expected in &[
            "browser_back",
            "browser_click",
            "browser_close",
            "browser_console",
            "browser_get_images",
            "browser_navigate",
            "browser_press",
            "browser_scroll",
            "browser_snapshot",
            "browser_type",
            "browser_vision",
        ] {
            assert!(
                names.contains(*expected),
                "Phase 25.1 GAP-8 (plan 25.1-19): rata bootstrap call shape MUST register \
                 {} (got: {:?})",
                expected,
                names
            );
        }

        let browser_count = names.iter().filter(|n| n.starts_with("browser_")).count();
        assert_eq!(
            browser_count, 11,
            "Phase 25.1 D-04: exactly 11 browser_* tools must be registered"
        );
    }
}
