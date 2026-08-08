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
    // The WorkerGuard must outlive the TUI session — dropping it shuts down the
    // non-blocking file-appender thread and any buffered log lines are lost.
    let _file_log_guard = install_tui_logger_subscriber();

    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let _mouse_guard = MouseCaptureGuard;

    let result = run_with_deps(&mut terminal, cli, initial, yolo).await;

    drop(_mouse_guard);
    ratatui::restore();
    drop(_file_log_guard);
    result
}

// ── Tracing subscriber install ────────────────────────────────────────────────

/// Install `tui_logger::TuiTracingSubscriberLayer` + a daily-rolling file
/// appender writing to `$IRONHERMES_HOME/logs/tui.log` before `ratatui::init()`.
///
/// Returns the `tracing_appender::non_blocking::WorkerGuard` for the file
/// appender — the caller MUST hold this for the duration of the TUI session
/// (dropping it shuts down the writer thread and loses buffered output).
/// `None` is returned only when the logs directory can't be created (e.g.
/// read-only home in tests); the in-TUI log panel still works in that case.
///
/// Uses `try_init` so double-install in tests (or when the classic subscriber
/// is already installed) is a no-op rather than a panic (Pitfall 2).
fn install_tui_logger_subscriber() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::prelude::*;

    let log_dir = ironhermes_core::constants::get_hermes_home().join("logs");
    let file_layer_pair = std::fs::create_dir_all(&log_dir).ok().map(|_| {
        let file_appender = tracing_appender::rolling::daily(&log_dir, "tui.log");
        let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
        // NF-1 (46.8-gap, D-15): silence the vendored `rusty_vault` crate's
        // debug-level secret logging unconditionally — see main.rs's matching
        // comment for the full rationale. Applied after RUST_LOG is honored.
        let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ironhermes=info"))
            .add_directive("rusty_vault=off".parse().expect("valid static directive"));
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_target(true)
            .with_filter(env_filter);
        (layer, guard)
    });

    let tui_layer = tui_logger::TuiTracingSubscriberLayer;
    let registry = tracing_subscriber::registry().with(tui_layer);
    let guard = match file_layer_pair {
        Some((file_layer, guard)) => {
            let _ = registry.with(file_layer).try_init();
            Some(guard)
        }
        None => {
            let _ = registry.try_init();
            None
        }
    };
    let _ = tui_logger::init_logger(tui_logger::LevelFilter::Trace);
    tui_logger::set_default_level(tui_logger::LevelFilter::Info);
    guard
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
    let mut resolver = ProviderResolver::build(&config)?;

    // Phase 46.8 NF-2 close-out: apply the vault fallback (final provider-key
    // source) before the resolver is consumed to build clients below — same
    // guarded block as `build_client` (main.rs) and the embedded server. D-10
    // no-op when vault.enabled is false (default); D-07 loud hard error on a
    // sealed/broken enabled vault; shared `resolve_vault_config` fills the
    // data_dir sentinel (G-46.8-1). Interactive TUI parity with `run_chat`.
    if config.vault.enabled {
        let store = ironhermes_vault::open_store(&ironhermes_core::resolve_vault_config(&config))?;
        resolver.apply_vault_fallback(&*store).await?;
    }

    // Phase 41.3 Plan 11 (D-19): resolve the tool-credential snapshot here too —
    // this is a second production composition root (alongside
    // build_app_runtime_bundle) whose registry constructs WebSearchTool
    // directly. Same shape as the vault-fallback block immediately above:
    // only open the store when the operator enabled the vault, and propagate
    // a sealed/corrupt vault loudly via `?` rather than a silent keyless
    // default.
    let tool_credentials = {
        let store = if config.vault.enabled {
            Some(ironhermes_vault::open_store(
                &ironhermes_core::resolve_vault_config(&config),
            )?)
        } else {
            None
        };
        Arc::new(
            ironhermes_tools::credentials::ToolCredentials::resolve(&config, store.as_deref())
                .await?,
        )
    };

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
    // Phase 41.3 Plan 11 (D-19): install the resolved snapshot BEFORE
    // register_defaults_except runs, so web_search reads it instead of the
    // env-only default.
    registry.with_credentials(tool_credentials.clone());
    registry.register_defaults_except(&["terminal"]);
    // Phase 36.3.12 GAP 1 (D-01/D-06/D-07/D-09): pass the operator's full resolved
    // TerminalConfig (not just the env allowlist) so `terminal.backend: docker`/`ssh`
    // set in config.yaml actually selects that backend for this composition root
    // instead of silently no-opping to local.
    registry
        .register_terminal_tool_with_process_registry(process_registry.clone(), &config.terminal);

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
    let tui_subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(
        config.delegation.max_concurrent_children,
    ));
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
        // Phase 47 Plan 08: the TUI surface is out of this plan's scope (not
        // one of chat/kanban/delegate's shared-factory call sites) — no
        // generation wiring here, so a TUI delegate child's "generation"
        // toolset group (if ever requested) resolves EMPTY, same as
        // surfaces.delegate=false.
        None,
    );

    // RPC sub-registry (safe subset — no terminal, no execute_code)
    let mut rpc_registry = ironhermes_tools::ToolRegistry::new();
    // Phase 41.3 Plan 11 (D-19): same resolved snapshot as the main registry
    // above — this widens no capability (the hand-rolled tool list here is
    // unchanged); it only gives web_search a credential source.
    rpc_registry.with_credentials(tool_credentials.clone());
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool::new(
        tool_credentials.clone(),
    )));
    // Phase 41.3 Plan 08 (D-07): web_answer is the same trust class as
    // web_search in this sandbox (read-only outbound HTTP, no filesystem/
    // process/hardware access) — mirrors the identical addition in
    // ironhermes-agent's build_rpc_registry.
    rpc_registry.register(Box::new(ironhermes_tools::web_answer::WebAnswerTool::new(
        tool_credentials.clone(),
    )));
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
    //
    // Phase 46.9 Plan 03 (D-08/D-09): `tokens_limit` is seeded from the SAME
    // resolver path the web surface reads (`ResolvedEndpoint::context_length`,
    // D-06 precedence: user config.yaml override > model metadata > default),
    // not a hardcoded literal. `resolver` is still in scope here (moved into
    // the runtime bundle further below) and resolves to the active main
    // provider's model — the same model frozen for `client.model()` above.
    //
    // Phase 46.9 Plan 10 (GAP-3/D-07/D-09 TUI parity): `provider` below was
    // ALREADY seeded from `config.model.provider` — the same single source
    // the web's `ConfigSummary.provider` reads (api.rs `get_config_summary`,
    // `config.model.provider.clone()`) — before this plan; the live path
    // never used the `StatusLineState::default()` `"?"` placeholder. Plan 10
    // adds a dedicated adjacency regression test in `status_line.rs`
    // (`provider_renders_adjacent_to_model_pill`) confirming `build_pills`
    // keeps the provider pill immediately next to the model pill.
    let status_initial = StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        // Phase 36.6.2 Plan 04 (D-09): surface Ctrl+T/Ctrl+K/? alongside the
        // existing hints — pure string edit, no new StatusLineState field.
        // Phase 36.6.3 Plan 04 (D-08): trailing `/help commands` segment
        // replaced with the palette (`/ commands`) + `/model` picker
        // (`/model switch`) mentions, appended LAST so they truncate first
        // on a narrow terminal (UI-SPEC E4 overflow backstop). DUPLICATED at
        // status_line.rs's `StatusLineState::default()` — both sites MUST
        // change together (RESEARCH Pitfall 3): this is the literal the
        // running TUI actually shows.
        hint: "ctrl+c cancel · Ctrl+T thinking · Ctrl+K skills · ? help · / commands · /model switch"
            .to_string(),
        tokens_limit: resolver.resolve_for_main().context_length(),
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
        // D-08 (Phase 46 Plan 04): populate connected_mcp_servers so requires_mcp_servers-gated
        // skills (e.g. the Cloudflare skills) only surface when their MCP server is connected.
        prompt_builder.set_connected_mcp_servers(
            agent_runtime
                .mcp_manager()
                .map(|m| m.connected_server_names().into_iter().collect())
                .unwrap_or_default(),
        );
        // Phase 38.1 (D-04/D-05): freeze session timezone into PromptBuilder Timestamp slot.
        prompt_builder.set_timezone(config.agent.timezone.clone());
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

    // Phase 36.17.3 (D-03 / D-06 amended): TUI-owned queue + pause toggle.
    // The TUI uses a single fixed SessionKey (Platform::Local / "local" / "local"),
    // populated in App::new — only the queue + paused toggle flow through deps.
    let queue: Arc<dyn ironhermes_core::queue::MessageQueue<ironhermes_core::session::SessionKey>> =
        Arc::new(ironhermes_gateway::session_queue::SessionQueue::new());
    let queue_paused = Arc::new(std::sync::atomic::AtomicBool::new(false));

    // Phase 36.3.12 Plan 10 (WR-01): load ApprovalsStore ONCE for the process
    // lifetime — outside spawn_turn's per-turn scope — so a `[s]ession`
    // approval grant persists across every dispatch of this TUI session
    // instead of being discarded by a fresh `ApprovalsStore::load()` per turn.
    let approvals_store = Arc::new(ironhermes_core::ApprovalsStore::load().await);

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
        // Phase 36.17.3 (D-03 / D-06 amended): queue + paused toggle.
        queue,
        queue_paused,
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
        // Phase 36.3.12 Plan 10 (WR-01): process-lifetime store — see above.
        approvals_store,
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

    // 46.1-03 (D-04): build ns -> server-url map for the real rmcp-backed
    // RefreshFn — only configs that carry both oauth_provider and url can refresh.
    let ns_to_url: HashMap<String, String> = mcp_configs
        .values()
        .filter_map(|cfg| {
            let ns = cfg.oauth_provider.as_deref()?;
            let url = cfg.url.as_deref()?;
            Some((ns.to_string(), url.to_string()))
        })
        .collect();

    // 44-05 / 46.1-03: open auth store for OAuth-enabled MCP servers, wired to
    // the REAL rmcp-backed refresh function (D-04, D-05) — not the stub.
    // Non-fatal: OAuth servers are skipped with warn when store is unavailable (D-04).
    let auth_store: Option<Arc<ironhermes_core::auth::AuthStore>> =
        match ironhermes_mcp::open_auth_store_with_mcp_refresh(
            ironhermes_core::constants::get_hermes_home().join("auth.json"),
            ns_to_url,
        )
        .await
        {
            Ok(store) => Some(store),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "MCP: could not open OAuth token store; OAuth servers will be skipped (D-04)"
                );
                None
            }
        };

    // Spawn Phase 41 proactive refresh tasks for cached MCP OAuth namespaces
    // that actually have refresh capability (D-04: a namespace with no
    // refresh_token is never scheduled into a repeating failing refresh).
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(ref store) = auth_store {
        for cfg in mcp_configs.values() {
            if let Some(ns) = cfg.oauth_provider.as_deref()
                && let Some(tok) = store.get_token(ns).await
                && tok.refresh_token.is_some()
            {
                ironhermes_core::auth::AuthStore::spawn_refresh_task(store.clone(), ns.to_string());
            }
        }
    }

    // McpManager::new(registry).with_auth_store() then start_all(configs) (D-08: new() unchanged)
    // 46.1 BL-01: wire the config-driven global issuer allowlist here too — the TUI
    // auto-start must trust the same non-baseline OAuth issuers as the agent runtime,
    // or D-01 is inert at this surface.
    let manager = ironhermes_mcp::McpManager::new(registry)
        .with_auth_store(auth_store)
        .with_global_issuer_allowlist(config.mcp_oauth.issuer_allowlist.clone());
    manager.start_all(mcp_configs).await;
    Some(Arc::new(manager))
}

// ── 4-arm tokio::select! event loop ──────────────────────────────────────────

async fn run_app_inner(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    use crossterm::event::EventStream;
    use tokio::{signal, time};
    use tokio_stream::StreamExt;

    let mut events = EventStream::new(); // Pitfall 10 — local to fn, not on App

    // Phase 36.6.2 Plan 03 (TUI-02): wire the approval channel. The sender is
    // cloned by spawn_turn to build the per-turn TuiApprovalGate; the receiver is
    // drained by recv_approval_request below (mirrors the pending_rx precedent).
    if app.approval_tx.is_none() {
        let (approval_tx, approval_rx) =
            tokio::sync::mpsc::unbounded_channel::<crate::tui_rata::approval_gate_tui::ApprovalRequest>();
        app.approval_tx = Some(approval_tx);
        app.approval_rx = Some(approval_rx);
    }
    // Move the receiver into a local so the `select!` approval arm borrows this
    // local (not a second `&mut app`) alongside `recv_pending`'s app borrow.
    let mut approval_rx = app.approval_rx.take();

    // Phase 41.1 Plan 10 (G-41.1-1): wire the clarify channel — mirrors the
    // approval channel above. The sender is cloned by spawn_turn to build the
    // per-turn TuiClarifyDispatcher; the receiver is drained by
    // recv_clarify_request below (surfacing the clarify overlay instead of
    // clarify_tool.rs's raw-println fallback).
    if app.clarify_tx.is_none() {
        let (clarify_tx, clarify_rx) = tokio::sync::mpsc::unbounded_channel::<
            crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest,
        >();
        app.clarify_tx = Some(clarify_tx);
        app.clarify_rx = Some(clarify_rx);
    }
    let mut clarify_rx = app.clarify_rx.take();

    let mut tick = time::interval(Duration::from_millis(100));
    tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    let ctrl_c = signal::ctrl_c(); // Pitfall 6 — created ONCE outside loop, then pinned
    tokio::pin!(ctrl_c);

    loop {
        let size = terminal.size()?;
        let transcript_area = compute_transcript_area(size);

        // Per-turn spawn: submit() sets pending_tx; we pick it up here and spawn.
        if app.pending_tx.is_some()
            && let Some(cancel) = app.cancel_child.clone()
        {
            let tx = app.pending_tx.take().expect("checked above");
            spawn_turn(app, tx, cancel);
        }

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(ev)) => app.handle_event(ev, transcript_area),
                Some(Err(e)) => { tracing::warn!("terminal event error: {e}"); }
                None => { app.should_quit = true; }
            },
            Some(se) = recv_pending(app) => app.handle_stream_event(se),
            // Phase 36.6.2 Plan 03 (TUI-02): drain approval requests from spawned
            // turn tasks and surface/enqueue them (mirrors recv_pending).
            Some(req) = recv_approval_request(&mut approval_rx) => app.surface_approval_request(req),
            // Phase 41.1 Plan 10 (G-41.1-1): drain clarify requests from spawned
            // turn tasks and surface/enqueue them (mirrors the approval arm above).
            Some(req) = recv_clarify_request(&mut clarify_rx) => app.surface_clarify_request(req),
            _ = &mut ctrl_c => app.handle_ctrl_c_signal(),
            _ = tick.tick() => {
                app.on_tick();
                // Phase 36.17.8 (D-08): drain any transcripts delivered by the
                // capture task since the last tick and submit them as user turns.
                app.poll_voice_transcripts();
            }
        }

        app.reconcile_scroll(transcript_area);
        terminal.draw(|f| {
            ui(f, app); // base frame, unchanged
            crate::tui_rata::overlay::render(f, app); // Clear + centered Block, AFTER ui() (Phase 36.6.2 Plan 01)
            crate::tui_rata::palette::render(f, app); // NEW (Phase 36.6.3 Plan 01) — self-gates via palette_query, AFTER overlay so a modal always wins
        })?;

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

/// Await the next `ApprovalRequest` from a spawned turn task, or `pending()` when
/// no approval channel is wired (defensive — it is always wired at startup).
/// Mirrors `recv_pending`, but takes the receiver directly (not `&mut App`) so the
/// `select!` arm shares a single App borrow with `recv_pending` instead of a
/// conflicting second `&mut App` (Phase 36.6.2 Plan 03, RESEARCH Pattern 3).
async fn recv_approval_request(
    rx: &mut Option<
        tokio::sync::mpsc::UnboundedReceiver<crate::tui_rata::approval_gate_tui::ApprovalRequest>,
    >,
) -> Option<crate::tui_rata::approval_gate_tui::ApprovalRequest> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Await the next `ClarifyRequest` from a spawned turn task, or `pending()` when
/// no clarify channel is wired (defensive — it is always wired at startup).
/// Mirrors `recv_approval_request` exactly (Phase 41.1 Plan 10, G-41.1-1).
async fn recv_clarify_request(
    rx: &mut Option<
        tokio::sync::mpsc::UnboundedReceiver<crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest>,
    >,
) -> Option<crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest> {
    match rx.as_mut() {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// Compute the transcript chunk area by mirroring the 4-chunk layout from ui.rs.
///
/// Used by `run_app_inner` to pass `transcript_area` to `reconcile_scroll`.
pub(crate) fn compute_transcript_area(size: ratatui::prelude::Size) -> ratatui::layout::Rect {
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

/// Build the tui_rata per-turn `MessagingPerTurnWiring` (Phase 41.1 Plan 10,
/// G-41.1-1). Extracted as a small pure fn — separate from `spawn_turn`'s
/// large `tokio::spawn` closure — so a test can construct it directly and
/// assert `clarify_dispatcher.is_some()` without spinning up the whole event
/// loop. This is the regression fence for the HARD RULE that
/// `clarify_dispatcher` must never silently revert to `None`: doing so would
/// route every clarify call back to `clarify_tool.rs`'s raw `println!`
/// fallback, re-opening G-41.1-1's terminal corruption.
fn build_messaging_wiring(
    session_key: ironhermes_core::SessionKey,
    clarify_tx: Option<
        tokio::sync::mpsc::UnboundedSender<crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest>,
    >,
    clarify_registry: std::sync::Arc<ironhermes_tools::PendingClarifyRegistry>,
    cancel_token: Option<CancellationToken>,
) -> ironhermes_agent::MessagingPerTurnWiring {
    ironhermes_agent::MessagingPerTurnWiring {
        session_key,
        message_dispatcher: None,
        clarify_dispatcher: clarify_tx.map(|tx| {
            std::sync::Arc::new(crate::tui_rata::clarify_dispatcher_tui::TuiClarifyDispatcher::new(
                tx,
            )) as std::sync::Arc<dyn ironhermes_tools::ClarifyDispatcher>
        }),
        clarify_registry,
        cancel_token,
    }
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
    // Phase 39.1 Plan 04 (R39.1-01 / R39.1-05): acquire semaphore permits and register
    // the TurnEntry before spawning. RunningAgentGuard (the old AtomicBool RAII guard)
    // is REMOVED — the TurnRegistry is the new source of truth for in-flight turns.
    //
    // Pitfall 1 (from RESEARCH): all Arc clones must be done in the SYNC body so they
    // can be moved into the async block. Avoid holding std::sync::Mutex across await.
    let turn_registry = app.turn_registry.clone();
    let concurrency = app.concurrency.clone();
    // Phase 36.2 Plan 07 fix: thread state_store so the post-LLM-call
    // write site in `agent_loop.rs` records `usage_events` rows and
    // updates session aggregates. Without this, the write is silently
    // skipped (`if let Some(store) = &self.state_store`) and /usage stays
    // empty, the status-bar cost/tok pills never render.
    let state_store = app.state_store.clone();
    let mut messages_snapshot = app.history.clone();

    // Phase 36.17.8: snapshot voice-reply state for the post-turn TTS decision.
    // The Arcs read live runtime toggles when the turn finishes; `turn_was_voice`
    // is fixed at spawn time (this turn's input source). `should_speak` combines
    // them: `/voice tts` (auto_tts) speaks every reply; `/voice on` (enabled)
    // speaks only voice-input turns.
    let voice_auto_tts = app.voice.auto_tts.clone();
    let voice_enabled = app.voice.enabled.clone();
    let turn_was_voice = app.last_turn_was_voice;

    // Phase 21.8.3.1 D-03 / D-04 / D-06: inject active personality overlay
    // into the per-turn system message clone. Mutates messages_snapshot only;
    // app.history[0] is never touched. Field is session-persistent — re-read
    // every turn, never cleared by spawn_turn.
    if let Some(overlay_text) = &app.active_personality_overlay
        && !messages_snapshot.is_empty()
        && let Some(MessageContent::Text(ref mut s)) = messages_snapshot[0].content
    {
        s.push_str("\n\n");
        s.push_str(overlay_text);
    }
    let session_id = app.session_id.clone();

    // Phase 46.7 Plan 06 (D-22): post-turn deliverable capture. `cwd_for_capture`
    // is the operator's REAL CWD (the TUI never redirects to a session
    // workspace — `std::env::current_dir()` is stable for the process
    // lifetime since no `cd` tool call from a spawned turn can change the
    // TUI's own process CWD). `captured_artifacts` + `session_id_for_capture`
    // + `text_for_opt_out` are cloned here (sync body) so the spawned async
    // block can use them without borrowing `app`.
    let cwd_for_capture = std::env::current_dir().unwrap_or_default();
    let captured_artifacts = app.captured_artifacts.clone();
    let session_id_for_capture = session_id.clone();
    let text_for_opt_out = app.last_submitted_text.clone();

    // Phase 36.3.12 D-08: snapshot the live `/yolo` toggle in the sync body (mirrors
    // the voice Arcs above) so the spawned async block can read its CURRENT value at
    // gating time without borrowing `app` across the `tokio::spawn` boundary.
    let yolo_enabled_for_gating = app.yolo_enabled.clone();
    // Phase 36.3.12 Plan 10 (WR-01): the process-lifetime store (see AppDeps
    // doc) — cloning the Arc, not calling `ApprovalsStore::load()`, is what
    // makes the `[s]ession` tier persist across every spawn_turn dispatch.
    let approvals_for_gating = app.approvals_store.clone();
    // Phase 36.6.2 Plan 03 (TUI-02): the approval-channel sender, cloned so the
    // spawned task can build a channel-based TuiApprovalGate that surfaces the
    // overlay instead of the blocking CliApprovalGate stdin prompt (RESEARCH
    // Pitfall 2 — a blocking stdin read conflicts with the raw-mode EventStream).
    let approval_tx_for_gate = app.approval_tx.clone();
    // Phase 41.1 Plan 10 (G-41.1-1): the clarify-channel sender + the SHARED
    // registry, cloned so the spawned task's messaging_wiring routes clarify
    // through the overlay instead of clarify_tool.rs's raw-println fallback.
    // MUST be the SAME Arc App owns (not a fresh PendingClarifyRegistry::new())
    // — App::answer_clarify/cancel_clarify call take()/remove() on this exact
    // instance, and only reach the awaiter if it's the one the turn inserted into.
    let clarify_tx_for_dispatcher = app.clarify_tx.clone();
    let clarify_registry_shared = app.clarify_registry.clone();

    tokio::spawn(async move {
        // Phase 39.1 Plan 04 (R39.1-01 / R39.1-05 / D-09): acquire semaphore permits and
        // register a TurnEntry BEFORE spawning agent work (register-before-spawn discipline).
        // Permits are held for the lifetime of this task (RAII) and dropped on completion.
        let tui_turn_id = ironhermes_core::concurrency::TurnId::new_v4();
        let (per_permit, global_permit) = match concurrency.try_acquire() {
            Some(p) => p,
            None => {
                // Cap reached — await a permit (TUI is single-user; waiting is correct).
                let per = concurrency
                    .per_session
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("per_session semaphore never closed");
                let global = concurrency
                    .global
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("global semaphore never closed");
                (per, global)
            }
        };
        let entry = ironhermes_core::concurrency::TurnEntry {
            turn_id: tui_turn_id,
            session_id: session_id.clone(),
            surface: ironhermes_core::concurrency::Surface::Cli,
            started_at: std::time::Instant::now(),
            cancel: cancel_token.clone(),
        };
        turn_registry.register(entry).await;

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

        // Phase 36.17.7 D-01: TUI uses Platform::Local; dispatcher is None because
        // SendAudioTool's Local arm handles rodio playback directly via DeviceSinkBuilder.
        let session_key = ironhermes_core::SessionKey {
            platform: ironhermes_core::types::Platform::Local,
            chat_id: session_id.clone(),
            user_id: None,
        };
        let tts_wiring = Some(ironhermes_agent::TtsPerTurnWiring {
            session_key: session_key.clone(), // D-05 source-grep anchor (TtsPerTurnWiring.session_key is a non-Option SessionKey)
            audio_dispatcher: None,
        });

        // Phase 36.6.2 Plan 03 (TUI-02 / RESEARCH Pitfall 2): build the
        // channel-based TuiApprovalGate for this turn. Terminal/execute_code
        // approvals MUST route through this gate (which surfaces the overlay),
        // NOT the blocking CliApprovalGate stdin prompt that hangs under crossterm
        // raw mode. The gate is passed to BOTH the intercepts (which own the real
        // terminal/execute_code gating via execute_gated_command) AND the
        // TurnRequest.approval_gate field (which gates the guardrail
        // NeedsApproval branch for other tools, e.g. MCP mutations).
        let yolo_now = yolo_enabled_for_gating.load(std::sync::atomic::Ordering::SeqCst);
        let tui_gate: Option<std::sync::Arc<dyn ironhermes_core::ApprovalGate>> =
            approval_tx_for_gate.map(|tx| {
                std::sync::Arc::new(crate::tui_rata::approval_gate_tui::TuiApprovalGate::new(
                    tx,
                    approvals_for_gating.clone(),
                )) as std::sync::Arc<dyn ironhermes_core::ApprovalGate>
            });

        let (approval_gate_field, terminal_intercept, execute_code_intercept) = match tui_gate {
            // Channel wired (production): surface the overlay for every gated call.
            Some(gate) => (
                Some(gate.clone()),
                Some(crate::tui_rata::approval_gate_tui::build_tui_gated_terminal_intercept(
                    runtime.terminal_tool_arc(),
                    runtime.config().clone(),
                    session_id.clone(),
                    "tui",
                    session_id.clone(),
                    yolo_now,
                    gate.clone(),
                )),
                Some(crate::tui_rata::approval_gate_tui::build_tui_gated_execute_code_intercept(
                    runtime.execute_code_tool_arc(),
                    runtime.config().clone(),
                    session_id.clone(),
                    "tui",
                    session_id.clone(),
                    yolo_now,
                    gate.clone(),
                )),
            ),
            // Defensive fallback (channel not wired — should not happen in the
            // real event loop): keep the legacy CLI-gated intercepts.
            None => (
                None,
                Some(crate::approval_gate::build_gated_terminal_intercept(
                    runtime.terminal_tool_arc(),
                    runtime.config().clone(),
                    session_id.clone(),
                    "tui",
                    session_id.clone(),
                    yolo_now,
                    approvals_for_gating.clone(),
                )),
                Some(crate::approval_gate::build_gated_execute_code_intercept(
                    runtime.execute_code_tool_arc(),
                    runtime.config().clone(),
                    session_id.clone(),
                    "tui",
                    session_id.clone(),
                    yolo_now,
                    approvals_for_gating.clone(),
                )),
            ),
        };

        let request = ironhermes_agent::TurnRequest {
            messages: messages_snapshot,
            session_id: session_id.clone(),
            cancel_token: Some(cancel_token.clone()),
            stream: Some(streaming_cb),
            tool_progress: Some(tool_progress_cb),
            tool_result: Some(tool_result_cb),
            trajectory_writer,
            pressure_tracker: None,
            state_store,
            compression_count: 0,
            tts_wiring,
            turn_id: None, // Phase 39.2: wired to TUI turn_id in Plan 04 Task 1
            // Phase 41.1 Plan 10 (G-41.1-1): a real TuiClarifyDispatcher over the
            // SHARED app.clarify_registry — replaces the old clarify_dispatcher:
            // None + fresh-per-turn PendingClarifyRegistry::new(), which routed
            // every clarify call to clarify_tool.rs's raw println! fallback and
            // corrupted the raw-mode/alt-screen transcript (see
            // .planning/debug/41.1-tui-interactive-render-corruption.md). The
            // turn's cancel token is still threaded so /stop reaches a suspended
            // clarify.
            messaging_wiring: Some(build_messaging_wiring(
                session_key.clone(),
                clarify_tx_for_dispatcher,
                clarify_registry_shared,
                Some(cancel_token.clone()),
            )),
            // Phase 36.6.2 Plan 03 (TUI-02): channel-based TuiApprovalGate (or the
            // defensive CLI fallback) — surfaces the approval overlay for the
            // guardrail NeedsApproval branch. Replaces the old `approval_gate: None`.
            approval_gate: approval_gate_field,
            // Phase 36.3.12 D-08/D-10 + 36.6.2 Plan 03: gate the LLM's
            // terminal/execute_code calls through the same TuiApprovalGate so a real
            // gated call surfaces the overlay instead of the blocking stdin prompt.
            terminal_intercept,
            execute_code_intercept,
        };

        // Phase 46.7 Plan 06 (D-22): captured immediately before the turn runs
        // so the post-turn mtime gate below only accepts a deliverable
        // modified during THIS turn's window.
        let turn_start = std::time::SystemTime::now();
        let result = runtime.run_turn(request).await;

        // Phase 46.7 Plan 06 (D-13/D-15/D-22): deterministic post-turn
        // deliverable capture, gated on successful completion only (an
        // errored/cancelled turn didn't necessarily finish writing anything).
        if result.is_ok() {
            let opt_out = ironhermes_tools::chat_capture::detect_turn_opt_out(&text_for_opt_out);
            match capture_turn_scoped_deliverable(
                &cwd_for_capture,
                &session_id_for_capture,
                opt_out,
                turn_start,
            ) {
                Ok(Some(artifact)) => {
                    if let Ok(mut guard) = captured_artifacts.lock() {
                        guard.push(artifact);
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "TUI post-turn capture failed");
                }
            }
        }

        // Phase 36.17.8: capture the reply text for optional spoken playback
        // before `result` is consumed by the terminal-event match below.
        let reply_for_tts = result.as_ref().ok().and_then(|r| r.final_response.clone());

        let terminal_event = match result {
            // Phase 36.2 Plan 07/10 fix: forward the per-turn aggregated
            // token count so the status-bar `tokens_used` field updates.
            Ok(agent_result) => StreamEvent::Finished {
                total_tokens: agent_result.total_usage.total_tokens,
            },
            Err(_) if cancel_token.is_cancelled() => StreamEvent::Cancelled,
            Err(e) => StreamEvent::Error(e.to_string()),
        };
        let _ = tx.send(terminal_event);

        // Phase 36.17.8: speak the reply when voice TTS is active. Sent AFTER the
        // Finished event so the transcript renders immediately; playback then runs
        // in this detached task without blocking the UI event loop. All failures
        // are swallowed inside `speak_reply` — a missing voice never breaks the turn.
        use std::sync::atomic::Ordering;
        if let Some(reply) = reply_for_tts
            && crate::tui_rata::voice_reply::should_speak(
                voice_auto_tts.load(Ordering::Relaxed),
                voice_enabled.load(Ordering::Relaxed),
                turn_was_voice,
            )
        {
            let spoken = crate::tui_rata::voice_reply::spoken_text(&reply);
            if !spoken.trim().is_empty() {
                let config =
                    std::sync::Arc::new(ironhermes_core::Config::load().unwrap_or_default());
                let home = ironhermes_core::constants::get_hermes_home();
                crate::tui_rata::voice_reply::speak_reply(config, &home, &spoken).await;
            }
        }

        // Phase 39.1 Plan 04 (R39.1-01 / R39.1-05): deregister turn and drop
        // permits (RAII). Runs on all exit paths: completion, cancellation, error.
        turn_registry.deregister(tui_turn_id).await;
        drop(per_permit);
        drop(global_permit);
    });
}

// ── Phase 46.7 Plan 06: TUI post-turn deliverable capture (D-13/D-15/D-22) ──

/// Turn-scoped wrapper around `capture_chat_deliverable` (Plan 03).
///
/// `locate_deliverable` (which `capture_chat_deliverable` calls internally)
/// is a non-recursive, single-directory scan of `scan_root` — for the TUI,
/// `scan_root` is always the operator's real CWD (D-22; the TUI never
/// redirects to a session workspace, unlike the web-chat surface). That CWD
/// is an uncontrolled, potentially long-lived project directory, so a bare
/// scan would "recapture" a pre-existing `index.html`/`README.md` sitting in
/// that directory on EVERY turn, not just the turn that actually produced it.
///
/// This wrapper closes that gap with an mtime gate: the located deliverable
/// is only captured when its modification time is `>= turn_start`. RESEARCH
/// Open Question 2 recommended true per-turn write-event tracking (recording
/// paths from the write_file/edit tool callbacks) as the more precise
/// mechanism; that was out of this plan's budget (no existing per-turn
/// write-path tracking exists in the TUI turn path to hook into without a
/// wider AgentLoop change). The mtime gate is the documented fallback — it
/// bounds false positives to "a file with the exact/largest-html candidate
/// name happened to be modified during this exact turn's wall-clock window"
/// rather than "any pre-existing deliverable in CWD, ever".
fn capture_turn_scoped_deliverable(
    scan_root: &std::path::Path,
    session_id: &str,
    turn_opt_out: bool,
    turn_start: std::time::SystemTime,
) -> anyhow::Result<Option<ironhermes_tools::chat_capture::CapturedArtifact>> {
    if turn_opt_out {
        return Ok(None); // D-15
    }

    // The mtime gate must filter CANDIDATE SELECTION, not judge an
    // already-selected candidate: `locate_deliverable` picks by name priority,
    // so a stale `README.md` sitting in the operator's CWD used to win, fail
    // the freshness test, and suppress capture of the `*.html` the turn had
    // just written — every turn, forever (Phase 46.7 UAT test 7).
    ironhermes_tools::chat_capture::capture_chat_deliverable_since(
        scan_root,
        session_id,
        turn_opt_out,
        Some(turn_start),
    )
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

        // Phase 28.1-05: spawn_turn no longer hand-builds an AgentLoop — it delegates
        // to AgentRuntime::run_turn, which chains .with_browser_session(...) on the
        // per-turn loop. The old "count .with_browser_session( in event_loop.rs >= 2"
        // assertion became a tautology after the migration (its only matches were this
        // test's own assertion-string literals, not production code). Verify the real
        // wiring path instead: (a) spawn_turn delegates to run_turn, and (b) run_turn
        // wires the browser session onto the agent loop in agent_runtime.rs.
        assert!(
            non_comment.contains("run_turn("),
            "Phase 28.1-05: spawn_turn MUST delegate to AgentRuntime::run_turn so the \
             per-turn AgentLoop is built (with browser session) by the shared runtime; \
             `run_turn(` not found in event_loop.rs."
        );
        const AGENT_RUNTIME_SRC: &str =
            include_str!("../../../ironhermes-agent/src/agent_runtime.rs");
        let runtime_non_comment: String = AGENT_RUNTIME_SRC
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            runtime_non_comment.contains(".with_browser_session("),
            "Phase 25.1 GAP-8 / 28.1-05: AgentRuntime::run_turn MUST chain \
             .with_browser_session(...) so the rata chat REPL's browser tools reach \
             the per-turn agent loop."
        );
    }

    /// INV-36.2-07-TUI: Phase 36.2 Plan 07 regression net.
    /// `spawn_turn` MUST thread `app.state_store.clone()` into the per-turn
    /// `TurnRequest`. If `state_store: None` is passed, the post-LLM-call write
    /// site in `agent_loop.rs` (gated by `if let Some(store) = &self.state_store`)
    /// silently skips — `usage_events` stays empty, `/usage` returns "no data",
    /// and the status-bar cost/tok pills (Plan 10) never render.
    #[test]
    fn inv_36_2_07_tui_threads_state_store_into_turn_request() {
        let source = include_str!("event_loop.rs");
        let non_comment: String = source
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            non_comment.contains("let state_store = app.state_store.clone();"),
            "Phase 36.2 Plan 07 fix: spawn_turn MUST clone app.state_store into a \
             local so it can be moved into the tokio::spawn body and threaded into \
             TurnRequest; otherwise usage_events writes silently skip in the TUI."
        );

        // Ensure the TurnRequest in spawn_turn does NOT pass the unwired sentinel
        // (the literal pattern is intentionally split across concatenated string
        // literals so this assertion's own message does not match itself).
        let bad_pattern = concat!("state_store", ": None");
        assert!(
            !non_comment.contains(bad_pattern),
            "Phase 36.2 Plan 07 fix: spawn_turn MUST thread the state store \
             through the TurnRequest; passing the unwired sentinel disables the \
             agent_loop write site and breaks /usage + status-bar cost/tok pills."
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

// ── Phase 46.7 Plan 06 tests: tui_turn_capture (D-13/D-15/D-22) ─────────────

#[cfg(test)]
mod tui_turn_capture {
    use super::capture_turn_scoped_deliverable;
    use std::time::{Duration, SystemTime};

    /// Process-wide lock serializing `IRONHERMES_ARTIFACTS_DB` mutation across
    /// these tests. Mirrors the `toolset_cmd.rs::env_lock` idiom — plain
    /// `cargo test` runs tests as threads in ONE process (only nextest gives
    /// process-per-test), so two tests setting this global env var in parallel
    /// would cross-wire each other's artifact store.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    /// Redirects `ArtifactStore::open_default()` to a fresh tempdir DB for this
    /// test. The returned `MutexGuard` must be held for the test's full
    /// duration — capture reads the env lazily via `open_default()`, not just
    /// at setup time.
    fn redirect_artifacts_db() -> (std::sync::MutexGuard<'static, ()>, tempfile::TempDir) {
        let guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("artifacts.db");
        unsafe {
            std::env::set_var("IRONHERMES_ARTIFACTS_DB", &db_path);
        }
        (guard, dir)
    }

    #[test]
    fn index_html_written_during_the_turn_window_is_captured() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        let turn_start = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(scan_dir.path().join("index.html"), "<html>hi</html>").unwrap();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s1", false, turn_start).unwrap();
        assert!(
            captured.is_some(),
            "index.html written during the turn window must be captured"
        );
        assert_eq!(captured.unwrap().filename, "index.html");
    }

    #[test]
    fn preexisting_deliverable_older_than_turn_start_is_not_recaptured() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        std::fs::write(scan_dir.path().join("index.html"), "<html>old</html>").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let turn_start = SystemTime::now();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s2", false, turn_start).unwrap();
        assert!(
            captured.is_none(),
            "a pre-existing deliverable older than turn_start must NOT be recaptured every turn"
        );
    }

    /// Phase 46.7 UAT test 7 regression: the operator's real CWD (D-22) is an
    /// uncontrolled project directory that almost always ALREADY contains a
    /// stale `README.md` (an exact-name CAPTURE_CANDIDATE). `locate_deliverable`
    /// picks candidates by NAME priority, so it returned that stale README on
    /// every turn; the mtime gate then judged the README (old) instead of the
    /// deliverable the agent had just written, and capture returned None forever.
    /// Freshness must participate in SELECTION, not be applied after it.
    #[test]
    fn fresh_html_is_captured_even_when_a_stale_readme_shadows_it() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        // A long-lived project README — the exact-name candidate that shadowed
        // everything else. Written BEFORE the turn starts.
        std::fs::write(scan_dir.path().join("README.md"), "# project\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let turn_start = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        // The deliverable the agent actually produced this turn.
        std::fs::write(
            scan_dir.path().join("dashboard.html"),
            "<html>dashboard</html>",
        )
        .unwrap();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s5", false, turn_start).unwrap();
        assert!(
            captured.is_some(),
            "a stale README.md in CWD must not shadow the fresh deliverable this turn produced \
             — this is why the TUI never rendered an artifact chip"
        );
        assert_eq!(
            captured.unwrap().filename,
            "dashboard.html",
            "the captured deliverable must be the file written during the turn window"
        );
    }

    /// The stale-README case must still not capture anything when the turn
    /// produced NOTHING — the mtime gate's original purpose (no recapture).
    #[test]
    fn stale_readme_alone_is_still_not_captured() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        std::fs::write(scan_dir.path().join("README.md"), "# project\n").unwrap();
        std::thread::sleep(Duration::from_millis(20));
        let turn_start = SystemTime::now();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s6", false, turn_start).unwrap();
        assert!(
            captured.is_none(),
            "a turn that produced no deliverable must capture nothing"
        );
    }

    #[test]
    fn opt_out_suppresses_even_a_fresh_deliverable() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        let turn_start = SystemTime::now();
        std::thread::sleep(Duration::from_millis(20));
        std::fs::write(scan_dir.path().join("index.html"), "<html>hi</html>").unwrap();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s3", true, turn_start).unwrap();
        assert!(
            captured.is_none(),
            "D-15: turn_opt_out=true must suppress capture"
        );
    }

    #[test]
    fn empty_scan_root_yields_none() {
        let (_env_guard, _db_dir) = redirect_artifacts_db();
        let scan_dir = tempfile::tempdir().unwrap();
        let turn_start = SystemTime::now();

        let captured =
            capture_turn_scoped_deliverable(scan_dir.path(), "tui-s4", false, turn_start).unwrap();
        assert!(captured.is_none());
    }
}

// ── Phase 41.1 Plan 10 tests: clarify wiring guard (G-41.1-1) ──────────────────

/// Regression fence for the G-41.1-1 fix: `build_messaging_wiring` (the pure
/// builder `spawn_turn` calls) must always wire `clarify_dispatcher: Some(..)`
/// when given a `Some` sender — if a future change reverts to passing `None`
/// through, `ClarifyTool` falls back to `clarify_tool.rs`'s raw `println!`
/// and this test fails. Mirrors `approval_gate_tui.rs`'s
/// `#[cfg(all(test, feature = "test-support"))]` channel round-trip tests.
#[cfg(all(test, feature = "test-support"))]
mod clarify_wiring_tests {
    use super::build_messaging_wiring;

    #[test]
    fn build_messaging_wiring_wires_clarify_dispatcher_some() {
        let (clarify_tx, _clarify_rx) = tokio::sync::mpsc::unbounded_channel::<
            crate::tui_rata::clarify_dispatcher_tui::ClarifyRequest,
        >();
        let session_key = ironhermes_core::SessionKey {
            platform: ironhermes_core::types::Platform::Local,
            chat_id: "guard-test".to_string(),
            user_id: None,
        };
        let registry = std::sync::Arc::new(ironhermes_tools::PendingClarifyRegistry::new());

        let wiring = build_messaging_wiring(session_key, Some(clarify_tx), registry, None);

        assert!(
            wiring.clarify_dispatcher.is_some(),
            "G-41.1-1 regression fence: tui_rata's messaging_wiring MUST wire \
             clarify_dispatcher: Some(TuiClarifyDispatcher) — None would route \
             every clarify call to clarify_tool.rs's raw println! fallback and \
             corrupt the raw-mode/alt-screen transcript"
        );
    }

    /// The builder is a pure pass-through: a `None` sender (e.g. the channel
    /// hasn't been wired yet) still wires `clarify_dispatcher: None`, never
    /// panics or silently substitutes a dispatcher — proving the `Some(..)`
    /// case above is asserting the caller's real input, not a hardcoded
    /// `Some` inside the builder.
    #[test]
    fn build_messaging_wiring_passes_through_none_sender() {
        let session_key = ironhermes_core::SessionKey {
            platform: ironhermes_core::types::Platform::Local,
            chat_id: "guard-test-none".to_string(),
            user_id: None,
        };
        let registry = std::sync::Arc::new(ironhermes_tools::PendingClarifyRegistry::new());

        let wiring = build_messaging_wiring(session_key, None, registry, None);

        assert!(wiring.clarify_dispatcher.is_none());
    }
}
