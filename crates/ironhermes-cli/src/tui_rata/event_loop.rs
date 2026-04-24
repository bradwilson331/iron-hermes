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
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

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

    run_app_inner(terminal, &mut app).await
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
    use ironhermes_agent::{build_main_client, build_client as build_provider_client};
    use ironhermes_agent::budget::BudgetHandle;
    use ironhermes_core::{Config, ProviderResolver};
    use ironhermes_core::commands::{CommandRouter, registry::build_registry as build_command_registry};

    // D-18 item 11: yolo banner — fires before alt-screen if run_chat_ratatui is
    // called from plan 22.4-08's main.rs arm. Defensive fire here too (D-18 parity).
    if yolo {
        crate::yolo::print_yolo_banner_to_stderr(yolo);
    }

    // Session setup — D-08 parity: ensure home dirs before anything else.
    let hermes_home = ironhermes_core::get_hermes_home();
    for sub in &["cron", "sessions", "logs", "hooks", "memories", "skills",
                 "workspace", "subagent-transcripts"] {
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
    let memory_manager = ironhermes_agent::memory::factory::build_memory_manager(&config.memory)
        .await?;

    // D-18 item 10: ToolRegistry + tool registrations
    let cron_dir = hermes_home.join("cron");
    let job_store = Arc::new(Mutex::new(
        ironhermes_cron::JobStore::open(cron_dir)?,
    ));
    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(
        ironhermes_core::SkillRegistry::load_with_config(&cwd, &config.skills)
    );
    let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);

    let mut registry = ironhermes_tools::ToolRegistry::new();
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

    // D-18 item 9: BlocklistGuardrail (before Arc wrap — D-05)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(RwLock::new(registry));

    // D-18 item 3: McpManager (Option<Arc<McpManager>>)
    let mcp_manager = build_mcp_manager(&config, registry.clone()).await;

    // D-18 item 2: HookRegistry + listeners (JSONL + webhooks + drain_retry_queue)
    let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());
    if hooks_config.event_log.enabled {
        let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
        hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
    }
    let retry_queue = Arc::new(
        ironhermes_hooks::RetryQueue::new(
            ironhermes_hooks::RetryQueue::default_path(),
        )?,
    );
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(
            ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone()),
        );
    }
    let hook_registry = Arc::new(hook_registry);
    let default_ttl = hooks_config.webhooks.first()
        .and_then(|e| e.queue_ttl_hours)
        .unwrap_or(24);
    ironhermes_hooks::drain_retry_queue(
        retry_queue,
        &hooks_config.webhooks,
        default_ttl,
    ).await;

    // D-18 item 7: CommandRouter from build_command_registry
    let command_router = Arc::new(CommandRouter::new(build_command_registry()));

    // D-18 item 1: client + resolver for per-turn AgentLoop construction
    let client = if let Some(ref model) = cli.model {
        let provider = cli.provider.as_deref().unwrap_or(resolver.main_provider());
        build_provider_client(&resolver, provider, model)?
    } else {
        build_main_client(&resolver)?
    };
    let context_length = resolver.resolve_for_main().context_length();
    let budget = BudgetHandle::new(
        cli.max_turns.unwrap_or(config.agent.max_turns),
    );

    // D-18 item 1 (continued): AgentLoop — App stores Arc<AgentLoop> for integrations.
    // Per-turn spawn in spawn_turn builds a fresh loop with streaming callback.
    let agent_loop_inst = ironhermes_agent::agent_loop::AgentLoop::new(
        client.clone(),
        registry.clone(),
        cli.max_turns.unwrap_or(config.agent.max_turns),
    )
    .with_budget(budget.clone())
    .with_cancellation_token(cancel_parent.clone())
    .with_hook_registry(hook_registry.clone())
    .with_compression(context_length, config.agent.context_compression);
    let agent_loop = Arc::new(agent_loop_inst);

    // D-18 item 14: StatusLineState initial seed
    let status_initial = StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        hint: "ctrl+c cancel · /help commands".to_string(),
        ..Default::default()
    };

    Ok(AppDeps {
        agent_loop,
        hook_registry,
        mcp_manager,
        memory_manager,
        subagent_registry,
        process_registry,
        command_router,
        session_id,
        history_path,
        status_initial,
        yolo_enabled: yolo,
        cancel_parent,
        client,
        registry,
        budget,
        context_length,
        config_compression: config.agent.context_compression,
        max_turns: cli.max_turns.unwrap_or(config.agent.max_turns),
    })
}

/// Create subdirectories under hermes_home (D-21 / ensure_home_dirs parity).
fn ensure_home_dirs(hermes_home: &std::path::Path) -> Result<()> {
    for sub in &["cron", "sessions", "logs", "hooks", "memories", "skills",
                 "workspace", "subagent-transcripts"] {
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
    use tokio_stream::StreamExt;
    use tokio::{signal, time};

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
    let frame_area = Rect { x: 0, y: 0, width: size.width, height: size.height };
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

/// Spawn an agent turn using a fresh `AgentLoop` built with a streaming callback.
///
/// Approach: duplicate the ~30-LOC AgentLoop-builder block from main.rs:1682
/// using `App.client` + `App.registry` (stored on App from AppDeps — Rule 2
/// addition in plan 22.4-07). This is the "duplicate 30 LOC" fallback from the
/// plan's §AgentLoop Integration three approaches.
///
/// Streaming deltas flow via `UnboundedSender<StreamEvent>`. All 4 terminal
/// variants (Started/Finished/Cancelled/Error) are emitted (D-17 / D-18 item 1).
fn spawn_turn(app: &App, tx: UnboundedSender<StreamEvent>, cancel: CancellationToken) {
    let client = app.client.clone();
    let registry = app.registry.clone();
    let hook_registry = app.hook_registry.clone();
    let budget = app.budget.clone();
    let context_length = app.context_length;
    let config_compression = app.config_compression;
    let max_turns = app.max_turns;
    let memory_manager = app.memory_manager.clone();
    let cancel_token = cancel.clone();
    let messages_snapshot = app.history.clone();
    let session_id = app.session_id.clone();
    let _ = app.agent_loop.clone(); // keep Arc alive for future method-call integrations

    tokio::spawn(async move {
        let _ = tx.send(StreamEvent::Started);

        // Build a per-turn AgentLoop with a streaming callback that sends Deltas.
        let tx_delta = tx.clone();
        let streaming_cb: ironhermes_agent::agent_loop::StreamCallback = Box::new(move |chunk: &str| {
            let _ = tx_delta.send(StreamEvent::Delta(chunk.to_string()));
        });

        let mut agent = ironhermes_agent::agent_loop::AgentLoop::new(
            client,
            registry,
            max_turns,
        )
        .with_budget(budget)
        .with_cancellation_token(cancel_token.clone())
        .with_hook_registry(hook_registry)
        .with_compression(context_length, config_compression)
        .with_streaming(streaming_cb);

        if let Some(mm) = memory_manager {
            agent = agent.with_memory_manager(mm);
        }

        let result = agent.run(messages_snapshot).await;

        let terminal_event = match result {
            Ok(_) => StreamEvent::Finished,
            Err(_) if cancel_token.is_cancelled() => StreamEvent::Cancelled,
            Err(e) => StreamEvent::Error(e.to_string()),
        };
        let _ = tx.send(terminal_event);
    });
}
