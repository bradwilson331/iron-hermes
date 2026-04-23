use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use ironhermes_agent::{AgentLoop, AgentSubagentRunner, AnyClient, PressureTracker, PromptBuilder, build_client as build_provider_client, build_main_client};
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_core::{ChatMessage, Config, ProviderResolver, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_gateway::GatewayRunner;
use ironhermes_mcp::McpManager;
use ironhermes_tools::ToolRegistry;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use crate::tui::{ActivityState, CtrlCDecision, DoubleCtrlCState, StatusLineState, TuiHandle, prepare_prompt_with_reserve, finish_prompt_with_reserve};
use crate::tui::{dispatch_command, KeybindingRegistry, CommandResult};
use crate::tui::extension::{KeyContext, TuiExtension};
use crate::tui::commands::build_cli_router;
use ironhermes_core::commands::{CommandResult as CoreCommandResult, CommandRouter};
use ironhermes_core::commands::context::CommandContext;
use ironhermes_core::commands::registry::build_registry as build_command_registry;
use ironhermes_core::types::Platform;
use std::time::Instant;

mod cron;
mod batch;
mod memory_cmd;
mod memory_setup;
mod models_cmd;
mod mcp_config;
mod tui;
use ironhermes_cli::skills_cmd;

#[derive(Parser)]
#[command(
    name = "ironhermes",
    about = "IronHermes — The self-improving AI agent, rewritten in Rust",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Model to use (e.g., "anthropic/claude-sonnet-4-20250514")
    #[arg(short, long)]
    model: Option<String>,

    /// Provider (openrouter, anthropic, openai)
    #[arg(short, long)]
    provider: Option<String>,

    /// Enable streaming output
    #[arg(short, long, default_value_t = true)]
    stream: bool,

    /// Maximum iterations for the agent loop
    #[arg(long)]
    max_turns: Option<usize>,

    /// Run a single prompt non-interactively
    #[arg(short = 'e', long = "execute")]
    execute: Option<String>,

    /// Quiet mode (less output)
    #[arg(short, long)]
    quiet: bool,

    /// Phase 21.7 Plan 08 (D-11 / D-12): enable autonomous (yolo) mode.
    /// Blanket-bypasses dangerous-command approval. Iteration budget /
    /// fatal errors / user interrupt (G-01/G-04/G-09) still halt.
    /// Only honored on the batch (`-e`) and `chat` entry points — the
    /// `gateway` subcommand deliberately does NOT expose this flag
    /// (INV-21.7-05 / D-12). Top-level + Chat-subcommand flags OR together.
    #[arg(long, global = false)]
    yolo: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive chat mode (default)
    Chat {
        /// Initial message to send
        message: Option<String>,
        /// Phase 21.7 Plan 08 (D-11 / D-12): enable autonomous (yolo) mode
        /// for this chat session. OR'd with the top-level `--yolo` flag and
        /// `autonomous.yolo` config key; CLI wins over config (D-12).
        #[arg(long)]
        yolo: bool,
    },
    /// Show current configuration and status.
    ///
    /// Phase 21.7 Plan 09 (D-18..D-22): `--all`, `--deep`, `--json` flags.
    Status(ironhermes_cli::status_cmd::StatusArgs),
    /// Check configuration and dependencies
    Doctor,
    /// Show version information
    Version,
    /// Start the Telegram gateway bot
    Gateway {
        /// Override Telegram bot token (or set TELEGRAM_BOT_TOKEN env var)
        #[arg(long)]
        token: Option<String>,
    },
    /// Manage scheduled tasks
    Cron {
        #[command(subcommand)]
        command: cron::CronCommands,
    },
    /// Run batch prompt processing
    Batch {
        #[command(subcommand)]
        command: batch::BatchCommands,
    },
    /// Manage skills from the Hub (install/search/update/uninstall/list/trust).
    Skills {
        #[command(subcommand)]
        action: skills_cmd::SkillsAction,
    },
    /// Memory provider management (Plan 20-03, D-08).
    Memory {
        #[command(subcommand)]
        action: MemorySubcommand,
    },
    /// Model metadata management.
    Models {
        #[command(subcommand)]
        command: models_cmd::ModelsSubcommand,
    },
    /// Manage MCP server connections
    Mcp {
        #[command(subcommand)]
        action: mcp_config::McpAction,
    },
}

#[derive(Subcommand)]
enum MemorySubcommand {
    /// Interactive setup for the currently-selected memory provider.
    Setup,
    /// Display current memory subsystem state (D-09).
    Status,
    /// Disable external provider, keep built-in file memory (D-10).
    Off,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env file — must happen BEFORE Cli::parse() if any env var would
    // influence parsing; it is already safe to run here because nothing in
    // the clap derivation reads env.
    let env_path = Config::env_path();
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    // D-21: Create ~/.ironhermes/ subdirectories on first run (belt-and-suspenders)
    ensure_home_dirs().context("Failed to initialize IronHermes home directory")?;

    let cli = Cli::parse();

    // GAP-6a: interactive REPL entry points (`hermes chat`, bare `hermes`) get a
    // minimal log filter so the WARN flood from SkillRegistry / Cron / provider
    // diagnostics does not bury the prompt. Non-interactive entry points keep
    // today's default (`ironhermes=info` add_directive) so operators running
    // `hermes gateway` / `hermes agent` still see the diagnostics they expect.
    // RUST_LOG in the environment ALWAYS wins (via EnvFilter::try_from_default_env).
    // Interactive = `hermes chat` subcommand, OR bare `hermes` with no `-e/--execute` flag.
    // `hermes -e "prompt"` enters `run_single` via the `None` arm — that's batch, NOT interactive.
    let is_interactive_repl =
        matches!(cli.command, Some(Commands::Chat { .. }) | None) && cli.execute.is_none();
    let env_filter = match std::env::var("RUST_LOG") {
        Ok(_) => tracing_subscriber::EnvFilter::from_default_env(),
        Err(_) if is_interactive_repl => tracing_subscriber::EnvFilter::new("error"),
        Err(_) => tracing_subscriber::EnvFilter::from_default_env()
            .add_directive("ironhermes=info".parse().unwrap()),
    };
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .init();

    // Phase 21.3: eagerly initialize tiktoken BPE tables to avoid ~100ms
    // latency on first token count.
    ironhermes_core::warm_tiktoken_singletons();

    match cli.command {
        Some(Commands::Status(args)) => ironhermes_cli::status_cmd::run_status(args).await,
        Some(Commands::Doctor) => cmd_doctor(),
        Some(Commands::Version) => cmd_version(),
        Some(Commands::Chat { ref message, yolo: ref chat_yolo }) => {
            // Phase 21.7 Plan 08 (D-12): OR top-level + subcommand yolo flags.
            // `cli.yolo` captures `hermes --yolo chat ...`; `chat_yolo` captures
            // `hermes chat --yolo ...`. Either path reaches the REPL with the
            // same effective state.
            let cli_yolo_flag = cli.yolo || *chat_yolo;
            run_chat(&cli, message.clone(), cli_yolo_flag).await
        }
        Some(Commands::Gateway { ref token }) => run_gateway(&cli, token.clone()).await,
        Some(Commands::Cron { command }) => cron::handle_cron_command(command).await,
        Some(Commands::Batch { command }) => batch::handle_batch_command(command).await,
        Some(Commands::Skills { action }) => {
            let config_path = ironhermes_core::Config::config_path();
            match skills_cmd::dispatch(&config_path, action).await {
                Ok(code) => { std::process::exit(code); }
                Err(e) => { eprintln!("error: {}", e); std::process::exit(1); }
            }
        }
        Some(Commands::Memory { action: MemorySubcommand::Setup }) => {
            memory_setup::run_memory_setup(&cli).await
        }
        Some(Commands::Memory { action: MemorySubcommand::Status }) => {
            memory_cmd::handle_memory_status().await
        }
        Some(Commands::Memory { action: MemorySubcommand::Off }) => {
            memory_cmd::handle_memory_off().await
        }
        Some(Commands::Models { command }) => models_cmd::handle_models_command(command).await,
        Some(Commands::Mcp { action }) => {
            match mcp_config::handle_mcp_command(action).await {
                Ok(()) => Ok(()),
                Err(e) => {
                    eprintln!("{}: {}", "Error".red().bold(), e);
                    std::process::exit(1);
                }
            }
        },
        None => {
            if let Some(ref prompt) = cli.execute {
                // Phase 21.7 Plan 08 (D-12): `-e` batch mode honors top-level
                // `--yolo` only (no per-invocation subcommand). The config
                // value is OR'd inside `run_single` via `resolve_yolo`.
                run_single(&cli, prompt.clone(), cli.yolo).await
            } else {
                // Bare `hermes` -> `run_chat`. Top-level `--yolo` flows
                // through here unchanged.
                run_chat(&cli, None, cli.yolo).await
            }
        }
    }
}

fn cmd_version() -> Result<()> {
    println!(
        "{} v{}",
        "IronHermes".bold().cyan(),
        env!("CARGO_PKG_VERSION")
    );
    println!("The self-improving AI agent, rewritten in Rust");
    println!("Created by Nous Research");
    Ok(())
}

/// Creates the standard ~/.ironhermes/ subdirectory tree on first run.
/// Idempotent -- safe to call on every startup (D-21 belt-and-suspenders).
fn ensure_home_dirs() -> Result<()> {
    let home = ironhermes_core::get_hermes_home();
    for sub in &[
        "cron",
        "sessions",
        "logs",
        "hooks",
        "memories",
        "skills",
        "workspace",
        "subagent-transcripts", // D-05 (Phase 21.7): JSONL transcript store for subagent runs.
    ] {
        std::fs::create_dir_all(home.join(sub))
            .with_context(|| format!("Failed to create {}/{}", home.display(), sub))?;
    }
    Ok(())
}

// Phase 21.7 Plan 09 Task 9-02: the previous `cmd_status` stub has been
// replaced by `ironhermes_cli::status_cmd::run_status`, which reads the
// full D-18..D-22 status surface (provider, memory, gateway, subagents,
// processes, MCP, yolo) and supports `--all`, `--deep`, `--json` flags.
// The dispatch arm in `main()` calls `run_status(args).await`.

fn cmd_doctor() -> Result<()> {
    println!("{}", "IronHermes Doctor".bold().cyan());
    println!("{}", "─".repeat(40));

    // Check home directory
    let home = ironhermes_core::get_hermes_home();
    print_check("Home directory", home.exists());

    // Check config
    let config_path = Config::config_path();
    print_check("Config file", config_path.exists());

    // Check .env
    let env_path = Config::env_path();
    print_check(".env file", env_path.exists());

    // Check API keys
    print_check(
        "OpenRouter API key",
        std::env::var("OPENROUTER_API_KEY").is_ok(),
    );
    print_check(
        "Anthropic API key",
        std::env::var("ANTHROPIC_API_KEY").is_ok(),
    );

    // Check state database
    let db_path = home.join("state.db");
    print_check("State database", db_path.exists());

    println!();
    println!(
        "{}",
        "Run `ironhermes status` for more details.".dimmed()
    );

    Ok(())
}

fn print_check(name: &str, ok: bool) {
    let icon = if ok {
        "OK".green()
    } else {
        "MISSING".yellow()
    };
    println!("  [{icon}] {name}");
}

/// Run a single prompt and exit.
async fn run_single(cli: &Cli, prompt: String, cli_yolo_flag: bool) -> Result<()> {
    let (client, config, resolver) = build_client(cli)?;
    // Phase 21.7 Plan 08 (D-11 / D-12 / D-14): resolve yolo from CLI + config,
    // print the banner ONCE per session. `run_single` is batch mode — a single
    // `-e "prompt"` invocation — so "per session" means "per process".
    let (yolo_enabled, _yolo_source) =
        ironhermes_cli::resolve_yolo(cli_yolo_flag, config.autonomous.yolo);
    ironhermes_cli::print_yolo_banner_to_stderr(yolo_enabled);

    // Phase 21.3: initialize global token estimator from model's encoding
    let main_ep = resolver.resolve_for_main();
    let encoding_name = main_ep.model_metadata
        .as_ref()
        .map(|m| m.tokenizer.as_str())
        .unwrap_or("cl100k_base");
    ironhermes_core::init_global_estimator(
        ironhermes_core::TiktokenEncoding::from_name(encoding_name)
    );
    let context_length = main_ep.context_length();

    // Per D-03: CLI shares the same state.db; per D-11: CLI uses its own Connection
    let mut state_store = ironhermes_state::StateStore::open_default()
        .context("failed to open state.db for CLI")?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state_store.create_session(&session_id, "cli", Some(client.model()), None, None)
        .context("failed to create CLI session")?;
    // Plan 21.7-05 (PROV-09/PROV-10/D-15): construct BudgetHandle seeded from
    // config.agent.max_iterations so the shared counter starts FULL and
    // drains per-turn (Caution70 at 70% used, Warning90 at 90%, Stop100 at
    // 100%). Parent + subagents clone the handle and share the counter.
    let budget = BudgetHandle::new(config.agent.max_iterations);
    // Plan 21.7-06 (D-29, D-24): session-scoped ProcessRegistry for
    // terminal/execute_code `background=true` spawns. Kept in scope so
    // on_session_end can call drain_and_kill_session below.
    let process_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_exec::process_registry::ProcessRegistry::new_for_session(
            session_id.clone(),
        ),
    ));
    // Plan 21.7-07 (D-03 / D-04 / D-05): session-scoped SubagentRegistry +
    // HERMES_HOME for transcripts. The runner threads both into each
    // `run_child` call so spawn/complete/cancel update state + transcripts.
    let subagent_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_agent::subagent_registry::SubagentRegistry::new(),
    ));
    let hermes_home = ironhermes_core::get_hermes_home();
    let mut registry = build_registry_with_process_registry(process_registry.clone());

    // Plan 20-03 Fix 2 / GAP-4: build memory manager — returns None when
    // config.memory.memory_enabled=false (T-21.4-02). All downstream
    // consumers guard with if-let so None propagates cleanly.
    let memory_manager: Option<Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>> =
        ironhermes_agent::memory::factory::build_memory_manager(&config.memory)
            .await
            .context("building memory manager for single-prompt mode")?;
    if let Some(ref mgr) = memory_manager {
        registry.register_memory_tool(mgr.clone());
    }

    // Register delegate_task tool (AGENT-01..05)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    // Plan 21.7-07 (D-03 / D-04 / D-05): thread SubagentRegistry +
    // transcript scope into the runner so lifecycle events update state.
    let subagent_runner = Arc::new(
        AgentSubagentRunner::new(
            client.clone(),
            resolver.clone(),
            Some(budget.clone()),
        )
        .with_subagent_registry(subagent_registry.clone())
        .with_transcript_scope(hermes_home.clone(), session_id.clone()),
    );
    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
        memory_manager.clone().map(|m| m as ironhermes_tools::memory_tool::SharedMemoryManager),
        config.subagent.clone(),
        None, // no cancel token in single mode
        None, // no progress callback in single mode
    );

    // Phase 22: register cron_tool (per D-02/D-03)
    let cron_dir = ironhermes_core::get_hermes_home().join("cron");
    let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
    registry.register_cronjob_tool(job_store.clone());

    // Phase 22: skills tool (per D-02/D-03)
    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);
    registry.register_skills_tool(
        skill_registry.clone(),
        active_skills.clone(),
        credential_dir,
        std::collections::HashMap::new(),
    );

    // Phase 22: RPC dispatch registry — safe subset per D-04 (no terminal, no execute_code)
    let mut rpc_registry = ToolRegistry::new();
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
    if let Some(ref mgr) = memory_manager {
        rpc_registry.register_memory_tool(mgr.clone());
    }
    let rpc_registry = Arc::new(rpc_registry);

    // Phase 22 / Plan 21.7-06: execute_code with active_skills bypass AND the
    // session-scoped ProcessRegistry so `background=true` spawns are tracked
    // and drained on on_session_end (D-24, D-29).
    registry.register_execute_code_tool_with_process_registry(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
        process_registry.clone(),
    );

    // Phase 22: guardrails (per D-02, D-08 — before Arc wrap)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(RwLock::new(registry));

    // Phase 21.2: MCP tool discovery (run_single)
    let mcp_manager = build_mcp_manager(&config, registry.clone()).await;

    // Phase 22: Build HookRegistry (per D-05, D-06, D-07)
    let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());

    // JSONL listener — default when event_log.enabled (per D-06)
    if hooks_config.event_log.enabled {
        let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
        hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
    }

    // Webhook listeners — opt-in per D-07
    let retry_queue = Arc::new(
        ironhermes_hooks::RetryQueue::new(
            ironhermes_hooks::RetryQueue::default_path()
        ).context("Failed to initialize webhook retry queue")?
    );
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(
            ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone())
        );
    }
    let hook_registry = Arc::new(hook_registry);

    // Drain persistent retry queue
    let default_ttl = hooks_config.webhooks.first()
        .and_then(|e| e.queue_ttl_hours)
        .unwrap_or(24);
    ironhermes_hooks::drain_retry_queue(
        retry_queue.clone(),
        &hooks_config.webhooks,
        default_ttl,
    ).await;

    let max_turns = cli
        .max_turns
        .unwrap_or(config.agent.max_turns);

    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .with_provider(&config.model.provider)
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry.clone());
    // Plan 20-03 Fix 2 / GAP-4: inject manager (when Some) so the frozen-snapshot
    // memory block renders into the system prompt. Skip when memory is disabled.
    if let Some(ref mgr) = memory_manager {
        prompt_builder.set_memory_manager(mgr.clone());
    }
    prompt_builder.set_user_profile_enabled(config.memory.user_profile_enabled);
    prompt_builder.load_memory().await;
    prompt_builder.load_skills();
    let system_msg = prompt_builder.build_system_message();

    let user_msg = ChatMessage::user(prompt);
    state_store.add_message(&session_id, &user_msg)
        .context("failed to persist user message")?;
    let messages = vec![system_msg, user_msg];

    let mut agent = AgentLoop::new(client, registry, max_turns)
        .with_budget(budget)
        .with_hook_registry(hook_registry.clone())   // Phase 22: D-05
        .with_compression(context_length, config.agent.context_compression)
        .with_streaming(Box::new(|delta| {
            print!("{}", delta);
            io::stdout().flush().ok();
        }))
        .with_tool_progress(Box::new(|name, args| {
            eprintln!("{} {} {}", "Tool:".dimmed(), name.yellow(), args.dimmed());
        }));

    // Wire fallback from resolver
    let main_endpoint = resolver.resolve_for_main();
    if let Some(fb_name) = main_endpoint.fallback_providers.first() {
        if let Ok(fb_client) = build_provider_client(&resolver, fb_name, &main_endpoint.default_model) {
            agent = agent.with_fallback(fb_client);
        }
    }

    // Phase 18 Plan 09: wire agent-side context compression (honors
    // config.agent.context_engine + config.agent.compression_threshold).
    // Phase 18-14: one-shot path — fresh tracker is fine (single turn then exits).
    // GAP-1/GAP-2: pass memory_manager so on_pre_compress fires on compression.
    agent = ironhermes_agent::attach_context_engine(
        agent,
        &config,
        &resolver,
        session_id.as_str(),
        Some(hook_registry.clone()),   // Phase 22: D-09
        None, // one-shot: fresh tracker per run
        context_length, // Phase 21.3
        memory_manager.clone(), // GAP-1/GAP-2: wire into context engine
    );

    let result = agent.run(messages).await?;

    // Plan 21.7-06 (D-24, T-21.7-06-01): drain + kill any background processes
    // tracked by this session's ProcessRegistry before exit. Best-effort —
    // matches the surrounding on_session_end pattern (log-and-continue). If
    // this is skipped on a crash path, LRU + TTL prune will catch up
    // eventually, but the right-by-construction route is explicit drain.
    if let Err(e) = process_registry
        .write()
        .await
        .drain_and_kill_session(&session_id)
        .await
    {
        tracing::warn!(
            error = %e,
            "process_registry drain_and_kill_session failed in run_single (best-effort)"
        );
    }

    // Plan 21.7-07 (D-05 / INV-21.7-09): drain pending fire-and-forget
    // transcript writes before we return from run_single. Every
    // TranscriptWriter::append dispatches to tokio::spawn, so turn events
    // that raced with the drain_and_kill above may still be in-flight.
    // 200ms is the recommendation from Plan 03 (real writes complete in
    // <10ms). Keep this as Duration::from_millis(200) — INV-21.7-09 greps
    // for that exact substring.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Touch the subagent_registry binding so the compiler keeps it alive
    // until the end of run_single (otherwise it would drop before the
    // final transcript drain). This is a cheap read — the registry is
    // stored here so D-03 consumers (Plan 08) can wire it into
    // CommandContext for on-demand /agents listing in future releases.
    let _ = subagent_registry.read().await.active_count();

    // GAP-6: notify provider of session end on natural exit (best-effort).
    // Uses default MemoryEntries — providers use their own internal state.
    if let Some(ref mgr) = memory_manager {
        let mgr_lock = mgr.lock().await;
        let entries = ironhermes_core::memory_provider::MemoryEntries::default();
        if let Err(e) = mgr_lock.on_session_end(&session_id, &entries).await {
            tracing::debug!(error = %e, "on_session_end failed in run_single (best-effort)");
        }
    }

    // Persist assistant response messages to SQLite
    for msg in &result.messages {
        if msg.role == ironhermes_core::Role::Assistant {
            let _ = state_store.add_message(&session_id, msg);
        }
    }
    state_store.end_session(&session_id, "completed")
        .context("failed to end CLI session")?;

    // Ensure newline after streaming
    println!();

    if !cli.quiet {
        eprintln!(
            "\n{} turns={}, tokens={}",
            "Stats:".dimmed(),
            result.turns_used,
            result.total_usage.total_tokens
        );
    }

    Ok(())
}

/// Run interactive chat mode.
async fn run_chat(
    cli: &Cli,
    initial_message: Option<String>,
    cli_yolo_flag: bool,
) -> Result<()> {
    print_banner();
    // GAP-5: force banner to hit the terminal BEFORE any async MCP startup
    // message can interleave on stderr. Without this flush, the stdout buffer
    // can be deferred until rustyline repaints on first keystroke, making the
    // CLI look frozen when mcp_servers is configured (violates D-07).
    io::stdout().flush().ok();
    io::stderr().flush().ok();

    let (client, config, resolver) = build_client(cli)?;
    // Phase 21.7 Plan 08 (D-11 / D-12 / D-14): resolve yolo + emit the
    // bold-red stderr banner ONCE per REPL session before we enter the
    // main loop. CLI flag wins over config; gateway reads config only
    // (see run_gateway below and INV-21.7-05).
    let (yolo_enabled, _yolo_source) =
        ironhermes_cli::resolve_yolo(cli_yolo_flag, config.autonomous.yolo);
    ironhermes_cli::print_yolo_banner_to_stderr(yolo_enabled);

    // Phase 21.3: initialize global token estimator from model's encoding
    let main_ep = resolver.resolve_for_main();
    let encoding_name = main_ep.model_metadata
        .as_ref()
        .map(|m| m.tokenizer.as_str())
        .unwrap_or("cl100k_base");
    ironhermes_core::init_global_estimator(
        ironhermes_core::TiktokenEncoding::from_name(encoding_name)
    );
    let context_length = main_ep.context_length();

    // Per D-03: CLI shares the same state.db; per D-11: CLI uses its own Connection
    let mut state_store = ironhermes_state::StateStore::open_default()
        .context("failed to open state.db for CLI")?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state_store.create_session(&session_id, "cli", Some(client.model()), None, None)
        .context("failed to create CLI session")?;
    // Plan 21.7-05 (PROV-09/PROV-10/D-15): BudgetHandle seeded from
    // config.agent.max_iterations. Parent + subagents share the counter
    // via BudgetHandle::clone; the per-turn run_agent_turn() inherits
    // the same handle and decrements it at turn-top.
    let budget = BudgetHandle::new(config.agent.max_iterations);
    // Plan 21.7-06 (D-29, D-24): session-scoped ProcessRegistry for
    // terminal/execute_code `background=true` spawns. Cloned into the tool
    // registration below and drained on natural REPL exit.
    let process_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_exec::process_registry::ProcessRegistry::new_for_session(
            session_id.clone(),
        ),
    ));
    // Plan 21.7-07 (D-03 / D-04 / D-05): session-scoped SubagentRegistry +
    // HERMES_HOME for transcripts. Cloned into the runner (so lifecycle
    // events register/unregister + write transcripts) and into the
    // SubagentProgressCallback (so the pill refreshes on Started/Completed).
    let subagent_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_agent::subagent_registry::SubagentRegistry::new(),
    ));
    let hermes_home = ironhermes_core::get_hermes_home();
    // Phase 18-14: session-scoped PressureTracker + compression_count.
    // Constructed once per REPL session so hysteresis (above_threshold,
    // pending_transient) and the summarizing engine's prior-summary chain
    // survive across all run_agent_turn calls within this session.
    let pressure_tracker = Arc::new(PressureTracker::new());
    let compression_count = Arc::new(AtomicUsize::new(0));
    let mut registry = build_registry_with_process_registry(process_registry.clone());

    // Plan 21-03: spawn the bottom-bar TUI (status line + knight-rider scanner).
    // Activity is Idle at startup; turns publish ActivityState::Streaming/ToolCall.
    let initial_status = StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        tokens_used: 0,
        tokens_limit: context_length,
        hint: "ctrl+c cancel · /help commands".to_string(),
        // Plan 21.7-07 (D-04): pill starts hidden (active=0); seed the
        // denominator from config so the pill renders as "N/M" the moment
        // a subagent registers.
        active_subagents: 0,
        max_subagents: config.subagent.max_subagents,
    };
    // Phase 22.1: construct TuiHandle with extensions (empty vec for now --
    // no extensions are registered yet, but the hook mechanism is active).
    let extensions: Vec<Box<dyn TuiExtension>> = Vec::new();
    let tui = Arc::new(TuiHandle::new_with_extensions(initial_status, extensions));

    // Build keybinding registry from registered extensions.
    let keybinding_registry = KeybindingRegistry::register_from_extensions(tui.extensions());

    // Plan 20-03 Fix 2 / GAP-4: wire MemoryManager into run_chat — returns None
    // when config.memory.memory_enabled=false (T-21.4-02).
    let memory_manager: Option<Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>> =
        ironhermes_agent::memory::factory::build_memory_manager(&config.memory)
            .await
            .context("building memory manager for chat mode")?;
    if let Some(ref mgr) = memory_manager {
        registry.register_memory_tool(mgr.clone());
    }

    // Register delegate_task tool (AGENT-01..05)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    // Plan 21.7-07 (D-03 / D-04 / D-05): thread SubagentRegistry +
    // transcript scope into the runner so lifecycle events update state.
    let subagent_runner = Arc::new(
        AgentSubagentRunner::new(
            client.clone(),
            resolver.clone(),
            Some(budget.clone()),
        )
        .with_subagent_registry(subagent_registry.clone())
        .with_transcript_scope(hermes_home.clone(), session_id.clone()),
    );
    // Plan 21-03: parent CancellationToken lives the full chat session; per-turn
    // children are issued via `.child_token()` so cancelling one turn does NOT
    // poison the session. (See RESEARCH §Pitfall 2: CancellationToken cancel is permanent.)
    let chat_cancel_parent = CancellationToken::new();
    let mut chat_cancel_token = chat_cancel_parent.child_token();

    // Double-ctrl-c state machine (D-10..D-14). 1.5s debounce window is baked in.
    let mut double_ctrl_c = DoubleCtrlCState::new();

    // Emergency 3rd-press escape per RESEARCH §Pitfall 7: track first-press time
    // across the whole session. If 3 ctrl-c events arrive within 3 seconds of the
    // FIRST press, we std::process::exit(130) to avoid tokio's permanent-handler
    // footgun where shutdown itself could hang.
    let mut emergency_first_press: Option<Instant> = None;
    let mut emergency_press_count: u32 = 0;

    // D-19 / Plan 21.7-07 (D-04 / ISS-05 / Pitfall 8): CLI tree-view progress
    // callback for subagent tool calls + status-line pill refresh.
    //
    // The pill refresh spawns a short-lived task that does ONE `registry.read().await`
    // (off the render path) and then calls the SYNC `status_tx.send_modify(...)`
    // to update `state.active_subagents`. The render path itself never awaits on
    // the registry — it only reads the copied usize from the watch channel.
    // INV-21.7-11 locks this invariant statically.
    let status_tx = tui.status_tx_handle();
    let reg_for_cb = subagent_registry.clone();
    let subagent_progress: ironhermes_tools::delegate_task::SubagentProgressCallback =
        Arc::new(move |index, event| {
            use ironhermes_tools::delegate_task::SubagentProgress;
            let tag = format!("[subagent-{}]", index + 1);
            match &event {
                SubagentProgress::Started { task_summary } => {
                    eprintln!("  {} {}", tag.clone().cyan().dimmed(), task_summary.dimmed());
                }
                SubagentProgress::ToolCall { tool_name } => {
                    eprintln!(
                        "  {} {} {}",
                        tag.clone().cyan().dimmed(),
                        "Running:".dimmed(),
                        tool_name.yellow().dimmed(),
                    );
                }
                SubagentProgress::Completed => {
                    eprintln!("  {} {}", tag.clone().cyan().dimmed(), "Done.".dimmed());
                }
            }
            // Pill refresh fires on Started and Completed (ToolCall is a
            // mid-lifecycle event; count hasn't changed). NOTE: there is no
            // Cancelled variant — cancellation is observed via the registry
            // unregister path inside AgentSubagentRunner::run_child.
            if matches!(
                event,
                SubagentProgress::Started { .. } | SubagentProgress::Completed
            ) {
                let reg = reg_for_cb.clone();
                let tx = status_tx.clone();
                tokio::spawn(async move {
                    // Only `read().await` is awaited — on a spawned task,
                    // NEVER on the render path (Pitfall 8 / ISS-05).
                    let n = reg.read().await.active_count();
                    // send_modify is SYNC (channel-side semantic). Updates
                    // `active_subagents` without clobbering unrelated state.
                    tx.send_modify(|s| s.active_subagents = n);
                });
            }
        });

    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore.clone(),
        memory_manager.clone().map(|m| m as ironhermes_tools::memory_tool::SharedMemoryManager),
        config.subagent.clone(),
        Some(chat_cancel_parent.child_token()), // delegate_task gets its own long-lived child
        Some(subagent_progress),
    );

    // Phase 22: register cron_tool (per D-02, mirroring run_gateway)
    let cron_dir = ironhermes_core::get_hermes_home().join("cron");
    let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
    registry.register_cronjob_tool(job_store.clone());

    // Phase 22: skills tool with shared active_skills Arc (per D-02, D-08)
    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);
    registry.register_skills_tool(
        skill_registry.clone(),
        active_skills.clone(),
        credential_dir,
        std::collections::HashMap::new(),
    );

    // Phase 22: RPC dispatch registry — safe subset per D-04 (no terminal, no execute_code)
    let mut rpc_registry = ToolRegistry::new();
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
    if let Some(ref mgr) = memory_manager {
        rpc_registry.register_memory_tool(mgr.clone());
    }
    let rpc_registry = Arc::new(rpc_registry);

    // Phase 22 / Plan 21.7-06: execute_code with active_skills bypass AND the
    // session-scoped ProcessRegistry so `background=true` spawns are tracked
    // and drained on REPL exit (D-24, D-29).
    registry.register_execute_code_tool_with_process_registry(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
        process_registry.clone(),
    );

    // Phase 22: guardrails (per D-02, D-08 — before Arc wrap)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(RwLock::new(registry));

    // Phase 21.2: MCP tool discovery (run_chat)
    let mcp_manager = build_mcp_manager(&config, registry.clone()).await;

    // Phase 22: Build HookRegistry (per D-05, D-06, D-07)
    let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());

    // JSONL listener — default when event_log.enabled (per D-06)
    if hooks_config.event_log.enabled {
        let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
        hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
    }

    // Webhook listeners — opt-in per D-07 (registered only if config has entries)
    let retry_queue = Arc::new(
        ironhermes_hooks::RetryQueue::new(
            ironhermes_hooks::RetryQueue::default_path()
        ).context("Failed to initialize webhook retry queue")?
    );
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(
            ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone())
        );
    }
    let hook_registry = Arc::new(hook_registry);

    // Drain persistent retry queue from previous runs (mirrors gateway behavior)
    let default_ttl = hooks_config.webhooks.first()
        .and_then(|e| e.queue_ttl_hours)
        .unwrap_or(24);
    ironhermes_hooks::drain_retry_queue(
        retry_queue.clone(),
        &hooks_config.webhooks,
        default_ttl,
    ).await;

    let max_turns = cli.max_turns.unwrap_or(config.agent.max_turns);

    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .with_provider(&config.model.provider)
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry);
    // Plan 20-03 Fix 2 / GAP-4: inject manager (when Some) before load_memory.
    if let Some(ref mgr) = memory_manager {
        prompt_builder.set_memory_manager(mgr.clone());
    }
    prompt_builder.set_user_profile_enabled(config.memory.user_profile_enabled);
    prompt_builder.load_memory().await;
    prompt_builder.load_skills();
    let system_msg = prompt_builder.build_system_message();

    let mut messages = vec![system_msg];

    // Phase 21.1 Plan 02: unified CommandRouter and agent_running flag.
    let command_router = CommandRouter::new(build_command_registry());
    let agent_running = Arc::new(AtomicBool::new(false));

    // Use rustyline for readline with history
    let mut rl = rustyline::DefaultEditor::new().context("Failed to initialize readline")?;

    // Handle initial message if provided
    if let Some(msg) = initial_message {
        let user_msg = ChatMessage::user(&msg);
        let _ = state_store.add_message(&session_id, &user_msg);
        messages.push(user_msg);
        println!("{} {}", "You:".bold().green(), msg);
        let response = run_agent_turn(
            &client,
            registry.clone(),
            &mut messages,
            max_turns,
            &config,
            &resolver,
            &budget,
            &session_id,
            pressure_tracker.clone(),
            compression_count.clone(),
            tui.clone(),
            chat_cancel_token.clone(),
            hook_registry.clone(),   // Phase 22: D-05
            context_length, // Phase 21.3
            memory_manager.clone(), // GAP-1: wire queue_prefetch
        )
        .await?;
        // Persist assistant response
        if let Some(ref text) = response {
            let assistant_msg = ChatMessage::assistant(text);
            let _ = state_store.add_message(&session_id, &assistant_msg);
            println!();
            println!("{} {}", "Hermes:".bold().cyan(), text);
        }
        println!();
    }

    let mut exit_cleanly = false;
    // GAP-5: belt-and-braces flush — if any later synchronous println/eprintln
    // happened between the banner and here (e.g., context injection, token
    // estimator init, rustyline setup), force it to the terminal before we
    // block inside rl.readline. The background MCP task's eprintln! has already
    // raced with this point by the time we get here; flushing guarantees the
    // user sees the prompt line rather than waiting on a keystroke.
    io::stdout().flush().ok();
    io::stderr().flush().ok();
    loop {
        // Phase 22.1 D-05: pre-readline keybinding check for Idle/Always bindings.
        // Uses non-blocking poll(Duration::ZERO) so we only consume events that are
        // already buffered. Only modifier-key combos (Ctrl+X, Alt+X) should be
        // registered as Idle bindings to avoid stealing chars from rustyline.
        //
        // WR-02 fix: only poll when extensions actually have Idle/Always keybindings
        // registered. This avoids consuming (and losing) unmatched key events when
        // no extensions are active (the current default).
        let has_idle_bindings = !keybinding_registry.help_entries().is_empty();
        if has_idle_bindings && crossterm::event::poll(std::time::Duration::ZERO).unwrap_or(false) {
            if let Ok(crossterm::event::Event::Key(key_event)) = crossterm::event::read() {
                if let Some(action) = keybinding_registry.match_key(&key_event, &KeyContext::Idle) {
                    // Dispatch keybinding action -- for now, actions are logged.
                    // Future extensions will handle actions via their own callbacks.
                    tracing::debug!("tui: keybinding dispatched: {}", action);
                    continue; // Skip readline for this iteration
                }
                // Key didn't match any binding -- it's consumed and lost.
                // Acceptable for modifier-key combos that rustyline wouldn't process.
            }
        }

        prepare_prompt_with_reserve(tui.reserved_row_count());
        let readline = rl.readline(&format!("{} ", "You:".bold().green()));
        finish_prompt_with_reserve(tui.reserved_row_count());
        match readline {
            Ok(line) => {
                let input = line.trim().to_string();
                if input.is_empty() {
                    continue;
                }

                // Phase 21.1 D-06/D-07/D-08: extension-first command dispatch via CommandRouter.
                if input.starts_with('/') {
                    let parts: Vec<&str> = input[1..].split_whitespace().collect();
                    if parts.is_empty() {
                        continue;
                    }
                    let cmd = parts[0];
                    let args = &parts[1..];

                    // Build CommandContext for this turn (Phase 21.2: wire mcp_reloader D-15).
                    // Plan 21.7-07: wire subagent_registry / process_registry /
                    // budget / subagent_semaphore + max_subagents handles so
                    // Plan 08 (/agents list/kill/logs) and Plan 09 (hermes status)
                    // consumers have live state to read. Each handle is a
                    // trait-object so ironhermes-core stays a leaf crate.
                    let cmd_ctx = {
                        let base = CommandContext::new(
                            Platform::Local,
                            session_id.clone(),
                            agent_running.clone(),
                        );
                        let base = if let Some(ref mgr) = mcp_manager {
                            base.with_mcp_reloader(mgr.clone())
                        } else {
                            base
                        };
                        let base = base
                            .with_subagent_registry(Arc::new(
                                ironhermes_agent::subagent_registry::SubagentRegistryHandle::new(
                                    subagent_registry.clone(),
                                ),
                            ))
                            .with_process_registry(Arc::new(
                                ironhermes_exec::process_registry::ProcessRegistryHandle::new(
                                    process_registry.clone(),
                                ),
                            ))
                            .with_budget(Arc::new(budget.clone()))
                            .with_subagent_semaphore(subagent_semaphore.clone())
                            .with_max_subagents(config.subagent.max_subagents);
                        base
                    };

                    // dispatch_command: extension-first -> CommandRouter -> skill catch-all
                    match dispatch_command(tui.extensions(), cmd, args, &command_router, &cmd_ctx) {
                        CommandResult::Handled(output) => {
                            println!("{}", output);
                            continue;
                        }
                        CommandResult::Quit => {
                            println!("{}", "Goodbye!".dimmed());
                            break;
                        }
                        CommandResult::ClearSession(output) => {
                            messages.truncate(1); // Keep system message
                            println!("{}", output.dimmed());
                            continue;
                        }
                        CommandResult::Silent => {
                            continue;
                        }
                        CommandResult::Error(msg) => {
                            eprintln!("{}", format!("Error: {}", msg).red());
                            continue;
                        }
                        // Phase 21.2 Plan 04: MCP reload — async reload via McpReloader (D-12).
                        CommandResult::McpReload => {
                            if let Some(ref mgr) = mcp_manager {
                                use ironhermes_core::commands::context::McpReloader;
                                let old_names = mgr.connected_server_names();
                                // Call through dyn McpReloader to avoid name collision with
                                // McpManager::reload(new_configs) (the concrete method).
                                let reloader: &dyn McpReloader = mgr.as_ref();
                                let result = reloader.reload().await;
                                let mut parts = vec![format!(
                                    "{} tool(s) from {} server(s)",
                                    result.tool_count,
                                    result.connected.len()
                                )];
                                let added: Vec<&String> = result
                                    .connected
                                    .iter()
                                    .filter(|n| !old_names.contains(n))
                                    .collect();
                                let removed: Vec<&String> = old_names
                                    .iter()
                                    .filter(|n| !result.connected.contains(n))
                                    .collect();
                                if !added.is_empty() {
                                    parts.push(format!(
                                        "Added: {}",
                                        added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                                    ));
                                }
                                if !removed.is_empty() {
                                    parts.push(format!(
                                        "Removed: {}",
                                        removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
                                    ));
                                }
                                if !result.failed.is_empty() {
                                    let fails: Vec<String> = result
                                        .failed
                                        .iter()
                                        .map(|(n, e)| format!("{} ({})", n, e))
                                        .collect();
                                    parts.push(format!("Failed: {}", fails.join(", ")));
                                }
                                println!("MCP reloaded. {}", parts.join(". "));
                            }
                            // (else: mcp_reloader is None, cmd_reload_mcp returned Output already)
                            continue;
                        }
                    }
                }

                let _ = rl.add_history_entry(&input);
                let user_msg = ChatMessage::user(&input);
                let _ = state_store.add_message(&session_id, &user_msg);
                messages.push(user_msg);

                // D-13: fresh user input resets the 1.5s debounce window.
                double_ctrl_c.reset();

                // Plan 21-03 Task 2: wrap in-flight agent turn in tokio::select!
                // racing against tokio::signal::ctrl_c() per D-10.
                // The future is pinned outside the select loop so a CancelTurn
                // decision can `continue` and the agent sees the cancelled token.
                agent_running.store(true, std::sync::atomic::Ordering::SeqCst);
                let mut run_fut = Box::pin(run_agent_turn(
                    &client,
                    registry.clone(),
                    &mut messages,
                    max_turns,
                    &config,
                    &resolver,
                    &budget,
                    &session_id,
                    pressure_tracker.clone(),
                    compression_count.clone(),
                    tui.clone(),
                    chat_cancel_token.clone(),
                    hook_registry.clone(),   // Phase 22: D-05
                    context_length, // Phase 21.3
                    memory_manager.clone(), // GAP-1: wire queue_prefetch
                ));

                let response: Option<String> = 'turn: loop {
                    tokio::select! {
                        biased;
                        _ = tokio::signal::ctrl_c() => {
                            let now = Instant::now();
                            // Emergency escape: 3 presses within 3s of first → hard exit 130.
                            emergency_press_count += 1;
                            if emergency_first_press.is_none() {
                                emergency_first_press = Some(now);
                            }
                            if let Some(first) = emergency_first_press && emergency_press_count >= 3
                                && now.duration_since(first) <= std::time::Duration::from_secs(3)
                            {
                                eprintln!("{}", "^C×3 — emergency exit".red());
                                tui.cleanup_on_exit();
                                std::process::exit(130);
                            }

                            match double_ctrl_c.on_ctrl_c(now, /* in_flight = */ true) {
                                CtrlCDecision::CancelTurn => {
                                    chat_cancel_token.cancel();
                                    println!("{}", "^C — turn cancelled".dimmed());
                                    tui.set_activity(ActivityState::Idle);
                                    // Stay in the select loop so the cancel propagates
                                    // and the agent future resolves naturally.
                                    continue 'turn;
                                }
                                CtrlCDecision::ExitCleanly => {
                                    chat_cancel_token.cancel();
                                    println!("{}", "Goodbye!".dimmed());
                                    tui.cleanup_on_exit();
                                    let _ = state_store.end_session(&session_id, "interrupted");
                                    // Break to outer loop cleanup so on_session_end fires.
                                    exit_cleanly = true;
                                    break 'turn None;
                                }
                                CtrlCDecision::ShowPromptHint => {
                                    // Unreachable here — we're in-flight. Defensive no-op.
                                    continue 'turn;
                                }
                            }
                        }
                        r = &mut run_fut => { break 'turn r?; }
                    }
                };

                agent_running.store(false, std::sync::atomic::Ordering::SeqCst);

                // Only reset the double-ctrl-c window on clean completion.
                // After CancelTurn, keep the window open so a second ctrl-c
                // at the prompt (caught by rustyline) triggers ExitCleanly.
                if !chat_cancel_token.is_cancelled() {
                    double_ctrl_c.reset();
                    emergency_press_count = 0;
                    emergency_first_press = None;
                }
                chat_cancel_token = chat_cancel_parent.child_token();

                // Persist assistant response
                if let Some(ref text) = response {
                    let assistant_msg = ChatMessage::assistant(text);
                    let _ = state_store.add_message(&session_id, &assistant_msg);
                    // If we were streaming, just add a newline
                    println!();
                }
                println!();

                // WR-04 fix: if ExitCleanly was signalled from the 'turn loop,
                // break the outer REPL loop to reach on_session_end cleanup.
                if exit_cleanly {
                    break;
                }
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                match double_ctrl_c.on_ctrl_c(Instant::now(), false) {
                    CtrlCDecision::ExitCleanly => {
                        println!("{}", "Goodbye!".dimmed());
                        tui.cleanup_on_exit();
                        let _ = state_store.end_session(&session_id, "interrupted");
                        // Break to outer loop cleanup so on_session_end fires.
                        break;
                    }
                    _ => {
                        println!("{}", "^C — type /quit to exit".dimmed());
                    }
                }
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                println!("{}", "Goodbye!".dimmed());
                break;
            }
            Err(err) => {
                eprintln!("Error: {}", err);
                break;
            }
        }
    }

    // Plan 21-03: shut down the TUI before ending the session so the bottom bar
    // is cleared cleanly. Arc::try_unwrap succeeds here because tui.clone() in
    // the REPL loop is dropped at end-of-scope; if any clone outlives this point
    // (shouldn't happen), we log and skip — the render task is cancelled on runtime drop.
    match Arc::try_unwrap(tui) {
        Ok(handle) => handle.shutdown().await,
        Err(_) => tracing::debug!("tui: Arc still has outstanding clones at clean-exit — skipping explicit shutdown"),
    }

    // Plan 21.7-06 (D-24, T-21.7-06-01): drain + kill any background processes
    // tracked by this session's ProcessRegistry before exit. Best-effort —
    // matches the surrounding on_session_end pattern (log-and-continue). Same
    // considerations as run_single's drain site.
    if let Err(e) = process_registry
        .write()
        .await
        .drain_and_kill_session(&session_id)
        .await
    {
        tracing::warn!(
            error = %e,
            "process_registry drain_and_kill_session failed in run_chat (best-effort)"
        );
    }

    // Plan 21.7-07 (D-05 / INV-21.7-09): drain pending fire-and-forget
    // transcript writes. Every TranscriptWriter::append dispatches to
    // tokio::spawn; 200ms drain matches Plan 03's open-question
    // resolution (real writes complete in <10ms).
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    // Keep the subagent_registry binding alive across the drain window.
    let _ = subagent_registry.read().await.active_count();

    // GAP-6: notify provider of clean session end (best-effort).
    // Only fires on natural REPL exit (EOF or /quit) — not on ctrl-c
    // (that path calls std::process::exit(0) above, preserving the
    // existing skip comment at the ExitCleanly branch).
    if let Some(ref mgr) = memory_manager {
        let mgr_lock = mgr.lock().await;
        let entries = ironhermes_core::memory_provider::MemoryEntries::default();
        if let Err(e) = mgr_lock.on_session_end(&session_id, &entries).await {
            tracing::debug!(error = %e, "on_session_end failed in run_chat (best-effort)");
        }
    }

    state_store.end_session(&session_id, "completed")
        .context("failed to end CLI session")?;

    Ok(())
}

/// Run one agent turn (may involve multiple tool calls).
#[allow(clippy::too_many_arguments)]
async fn run_agent_turn(
    client: &AnyClient,
    registry: Arc<RwLock<ToolRegistry>>,
    messages: &mut Vec<ChatMessage>,
    max_turns: usize,
    config: &Config,
    resolver: &ProviderResolver,
    budget: &BudgetHandle,
    session_id: &str,
    pressure_tracker: Arc<PressureTracker>,
    compression_count: Arc<AtomicUsize>,
    tui: Arc<TuiHandle>,   // Plan 21-03: TUI handle for activity publishing
    cancel_token: CancellationToken,
    hook_registry: Arc<ironhermes_hooks::HookRegistry>,   // Phase 22: D-05
    context_length: usize,  // Phase 21.3: resolved from model metadata
    memory_manager: Option<Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>>,  // GAP-1: wire queue_prefetch
) -> Result<Option<String>> {
    // Phase 18-14: seed the AgentLoop's compression_count from the shared
    // session-scoped counter so the summarizing engine's prior-summary chain
    // continues across REPL turns instead of resetting to 0 each prompt.
    let starting_count = compression_count.load(Ordering::SeqCst);

    let tui_stream = tui.clone();
    let tui_tool = tui.clone();

    let mut agent = AgentLoop::new(client.clone(), registry, max_turns)
        .with_budget(budget.clone())
        .with_cancellation_token(cancel_token)
        .with_hook_registry(hook_registry.clone())   // Phase 22: D-05
        .with_compression(context_length, config.agent.context_compression)
        .with_compression_count(starting_count)
        .with_streaming(Box::new(move |delta| {
            // Stream tokens directly to stdout (D-22: stream appears inline above the prompt).
            // Keep on stdout per RESEARCH §Pitfall 5 — ExternalPrinter is too high-frequency.
            print!("{}", delta);
            io::stdout().flush().ok();
            // Publish coarse activity state (best-effort; watch coalesces rapid updates).
            tui_stream.set_activity(ActivityState::Streaming);
        }))
        .with_tool_progress(Box::new(move |name, _args| {
            // D-08: REPLACE the old `eprint!("\r Running: ...")` clutter with a
            // watch publish. The render task renders the scanner + label at bottom row
            // every 100ms — no more inline stderr spray.
            tui_tool.set_activity(ActivityState::ToolCall { name: name.to_string() });
        }));

    // Wire fallback from resolver
    let main_endpoint = resolver.resolve_for_main();
    if let Some(fb_name) = main_endpoint.fallback_providers.first() {
        if let Ok(fb_client) = build_provider_client(resolver, fb_name, &main_endpoint.default_model) {
            agent = agent.with_fallback(fb_client);
        }
    }

    // GAP-1: wire memory_manager to AgentLoop so queue_prefetch fires after
    // each natural-end agent turn. Guard with if-let per T-21.4-04.
    if let Some(ref mgr) = memory_manager {
        agent = agent.with_memory_manager(mgr.clone());
    }

    // Phase 18 Plan 09: wire agent-side context compression.
    // Phase 18-14: reuse the session-scoped PressureTracker so hysteresis
    // state (above_threshold, pending_transient) survives across turns.
    // GAP-1/GAP-2: pass memory_manager so on_pre_compress fires on compression.
    agent = ironhermes_agent::attach_context_engine(
        agent,
        config,
        resolver,
        session_id,
        Some(hook_registry.clone()),   // Phase 22: D-09
        Some(pressure_tracker.clone()),
        context_length, // Phase 21.3
        memory_manager.clone(), // GAP-2: wire into context engine
    );

    // Pass a clone of messages so agent can work with them
    let result = agent.run(messages.clone()).await?;

    // After the turn completes, reset activity to Idle so the scanner hides (D-08).
    tui.set_activity(ActivityState::Idle);

    // Update the status line with post-turn token count (D-05).
    // Plan 21.7-07 (D-04): re-seed the pill fields from config each turn; the
    // live count comes from the registry via a spawned send_modify in the
    // SubagentProgressCallback, so full `set_status` writes that don't know
    // about the live count would zero it otherwise. Reading the current
    // watch state and preserving `active_subagents` avoids that regression.
    tui.set_status(StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        tokens_used: result.total_usage.total_tokens,
        tokens_limit: context_length,
        hint: "ctrl+c cancel · /help commands".to_string(),
        active_subagents: tui.status_snapshot().active_subagents,
        max_subagents: config.subagent.max_subagents,
    });

    // Phase 18-14: persist the post-turn compression_count back into the
    // shared counter so the next turn seeds its AgentLoop with the right value.
    compression_count.store(result.compression_count_after, Ordering::SeqCst);

    // Update messages with the full conversation (includes tool calls and results)
    *messages = result.messages;

    Ok(result.final_response)
}

/// Start the Telegram gateway bot.
async fn run_gateway(cli: &Cli, token_override: Option<String>) -> Result<()> {
    let (_, mut config, resolver) = build_client(cli)?;
    // Phase 21.7 Plan 08 (D-12 / INV-21.7-05): gateway reads yolo from the
    // on-disk config ONLY. NO per-request field, NO CLI flag — the clap
    // `Gateway` variant intentionally omits `--yolo`. `resolve_yolo(false, ...)`
    // locks the flag source to `"config"` when enabled.
    let (yolo_enabled, _yolo_source) =
        ironhermes_cli::resolve_yolo(false, config.autonomous.yolo);
    ironhermes_cli::print_yolo_banner_to_stderr(yolo_enabled);

    // Plan 20-02 / GAP-4: build the MemoryManager once — returns None when
    // config.memory.memory_enabled=false (T-21.4-02). All consumers guard
    // with if-let so None propagates cleanly.
    let memory_manager: Option<Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>> =
        ironhermes_agent::memory::factory::build_memory_manager(&config.memory).await?;

    // Plan 21.7-06 (D-29, D-24): gateway-scoped ProcessRegistry. Same
    // lifecycle trade-off as the BudgetHandle decision in Plan 05 — a true
    // per-session registry requires SessionStore plumbing deferred to Plan 09.
    // task_id = "gateway" so drain_and_kill_session(&gw_session_id) at the
    // per-request end site is a documented no-op (session_id mismatch);
    // LRU + FINISHED_TTL prune handles cleanup across long-running bots.
    let process_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_exec::process_registry::ProcessRegistry::new_for_session(
            "gateway".to_string(),
        ),
    ));
    // Plan 21.7-07 (D-03 / D-04 / D-05): gateway-scoped SubagentRegistry +
    // HERMES_HOME for transcripts. Per-request run_agent threads the same
    // registry via GatewayMessageHandler::set_subagent_registry (below).
    // Transcripts key by the per-request gw_session_id so they don't
    // collide between users.
    let subagent_registry = Arc::new(tokio::sync::RwLock::new(
        ironhermes_agent::subagent_registry::SubagentRegistry::new(),
    ));
    let hermes_home = ironhermes_core::get_hermes_home();

    // Build registry and register memory tool before Arc wrapping
    let mut registry = build_registry_with_process_registry(process_registry.clone());
    if let Some(ref mgr) = memory_manager {
        registry.register_memory_tool(mgr.clone());
    }

    // Open cron job store and register the cronjob tool
    let cron_dir = ironhermes_core::get_hermes_home().join("cron");
    let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
    registry.register_cronjob_tool(job_store.clone());

    // Discover skills and register the skills tool
    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);
    registry.register_skills_tool(
        skill_registry.clone(),
        active_skills.clone(),
        credential_dir,
        std::collections::HashMap::new(),
    );

    // Build RPC dispatch registry — only D-07 safe tools for sandbox (no terminal, no execute_code)
    let mut rpc_registry = ToolRegistry::new();
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
    rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
    rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
    if let Some(ref mgr) = memory_manager {
        rpc_registry.register_memory_tool(mgr.clone());
    }
    let rpc_registry = Arc::new(rpc_registry);

    // Register execute_code tool with the RPC dispatch registry.
    // Phase 19 Plan 06 (D-05) + Plan 21.7-06 (D-29): active_skills env-var
    // bypass AND the gateway-scoped ProcessRegistry for background spawns.
    registry.register_execute_code_tool_with_process_registry(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
        process_registry.clone(),
    );

    // Register delegate_task tool (AGENT-01..05, AGENT-03 semaphore enforcement)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let gateway_client = build_main_client(&resolver)?;
    // Plan 21.7-05 (PROV-09/PROV-10/D-15 + RESEARCH Open Q#4): construct a
    // BudgetHandle at gateway startup seeded from config.agent.max_iterations
    // and thread it into the shared AgentSubagentRunner. Per-request handler
    // code builds its per-message AgentLoop via GatewayMessageHandler::new,
    // which receives the SAME handle via with_budget (see handler.rs
    // GatewayMessageHandler::with_budget_handle). All clones share the
    // underlying counter so parent + subagents decrement together (PROV-10).
    //
    // Lifecycle: gateway-scoped (not per-user-session). A true per-session
    // budget requires plumbing through SessionStore; that's deferred to
    // Plan 09 (hermes status) where session-scoped budget readouts live.
    let budget = BudgetHandle::new(config.agent.max_iterations);
    // Plan 21.7-07 (D-03 / D-04 / D-05): thread the gateway-scoped
    // SubagentRegistry + HERMES_HOME into the runner. Transcript paths use
    // a process-wide "gateway" session id — the per-request runtime could
    // use the per-user session id but that requires a per-request runner,
    // which would break the shared delegate_task Arc. Gateway-process
    // scope matches the BudgetHandle + ProcessRegistry Plan 05/06 decision.
    let subagent_runner = Arc::new(
        AgentSubagentRunner::new(
            gateway_client,
            resolver.clone(),
            Some(budget.clone()),
        )
        .with_subagent_registry(subagent_registry.clone())
        .with_transcript_scope(hermes_home.clone(), "gateway".to_string()),
    );
    let gateway_cancel_token = CancellationToken::new();
    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
        memory_manager.clone().map(|m| m as ironhermes_tools::memory_tool::SharedMemoryManager),
        config.subagent.clone(),
        Some(gateway_cancel_token.clone()),
        None, // D-20: gateway uses tracing::info only, no stderr progress
    );

    // Load hooks config and wire guardrails (before Arc wrapping)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();

    // Register guardrails on ToolRegistry (per D-05) — must happen before Arc wrapping
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(RwLock::new(registry));

    // Phase 21.2: MCP tool discovery (run_gateway)
    let mcp_manager = build_mcp_manager(&config, registry.clone()).await;

    // Build HookRegistry
    let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());

    // Register JSONL event log listener (per D-04)
    if hooks_config.event_log.enabled {
        let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
        hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
    }

    // Create shared retry queue for webhook persistence (per D-09)
    let retry_queue = std::sync::Arc::new(
        ironhermes_hooks::RetryQueue::new(
            ironhermes_hooks::RetryQueue::default_path()
        ).expect("Failed to initialize webhook retry queue")
    );

    // Register webhook listeners (per D-08, D-09, D-10)
    for endpoint in &hooks_config.webhooks {
        hook_registry.add_listener(
            ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone())
        );
    }

    let hook_registry = std::sync::Arc::new(hook_registry);

    // Drain persistent retry queue from previous runs (per D-09)
    let default_ttl = hooks_config.webhooks.first()
        .and_then(|e| e.queue_ttl_hours)
        .unwrap_or(24);
    ironhermes_hooks::drain_retry_queue(
        retry_queue.clone(),
        &hooks_config.webhooks,
        default_ttl,
    ).await;

    // Override token if provided via --token flag
    if let Some(token) = token_override {
        let tg = config
            .gateway
            .platforms
            .entry("telegram".to_string())
            .or_default();
        tg.token = Some(token);
        tg.enabled = true;
    }

    info!("Starting IronHermes Telegram Gateway");
    let mut runner = GatewayRunner::new(config, resolver, registry);
    // Plan 21.7-05: thread the same BudgetHandle constructed above into the
    // runner so each per-request AgentLoop shares the counter with the
    // registered AgentSubagentRunner (PROV-10 parent/child shared).
    runner.set_budget_handle(budget.clone());
    // Plan 21.7-06 (D-29, D-24): thread the gateway-scoped ProcessRegistry
    // into the runner so the per-request handler can call
    // drain_and_kill_session at on_session_end (grep-gate satisfied; no-op
    // in practice for the gateway-scoped task_id — see construction comment).
    runner.set_process_registry(process_registry.clone());
    // Plan 21.7-07 (D-03 / D-04 / D-05): thread the gateway-scoped
    // SubagentRegistry into the runner so per-request on_session_end
    // can drain pending transcript writes and Plan 08 (/agents list)
    // can read live subagent state via the SAME registry backing the
    // delegate_task runner registered on the tool registry above.
    runner.set_subagent_registry(subagent_registry.clone());
    if let Some(mgr) = memory_manager {
        runner.set_memory_manager(mgr);
    }
    runner.set_job_store(job_store);
    runner.set_skill_registry(skill_registry);
    runner.set_hook_registry(hook_registry);
    runner.set_active_skills(active_skills);
    // GAP-8 (Phase 21.2 Plan 11): wire MCP manager into runner's shutdown
    // path so Ctrl+C actually returns when stdio MCP servers are connected.
    // build_mcp_manager returns Option<Arc<McpManager>>; pass the Arc clone
    // if Some. Without this, `ironhermes gateway` hangs indefinitely on
    // Ctrl+C because the tokio process reaper keeps the runtime alive until
    // MCP children are reaped.
    if let Some(ref mgr) = mcp_manager {
        runner.set_mcp_manager(mgr.clone());
    }
    runner.start().await
}

fn build_client(cli: &Cli) -> Result<(AnyClient, Config, ProviderResolver)> {
    let config = Config::load().unwrap_or_default();
    let resolver = ProviderResolver::build(&config)?;

    // If user specified a model override, build client with that model
    let client = if let Some(ref model) = cli.model {
        let provider = cli.provider.as_deref().unwrap_or(resolver.main_provider());
        build_provider_client(&resolver, provider, model)?
    } else {
        build_main_client(&resolver)?
    };

    info!(model = %client.model(), provider = %resolver.main_provider(), "Initializing LLM client via ProviderResolver");

    Ok((client, config, resolver))
}

fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_defaults();
    registry
}

/// Plan 21.7-06 (D-29): build a ToolRegistry with a TerminalTool that is
/// wired to the session-scoped ProcessRegistry so `terminal background=true`
/// spawns flow through `drain_and_kill_session` at on_session_end. All other
/// default tools register identically to `build_registry`.
fn build_registry_with_process_registry(
    process_registry: Arc<tokio::sync::RwLock<ironhermes_exec::process_registry::ProcessRegistry>>,
) -> ToolRegistry {
    use ironhermes_tools::file_tools::{PatchFileTool, ReadFileTool, SearchFilesTool, WriteFileTool};
    use ironhermes_tools::web_read::WebReadTool;
    use ironhermes_tools::web_search::WebSearchTool;
    let mut registry = ToolRegistry::new();
    // Terminal with ProcessRegistry wiring.
    registry.register_terminal_tool_with_process_registry(process_registry.clone());
    // Other defaults mirror `register_defaults()` sans the plain TerminalTool.
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(PatchFileTool));
    registry.register(Box::new(SearchFilesTool));
    registry.register(Box::new(WebSearchTool));
    registry.register(Box::new(WebReadTool));
    registry
}

/// Phase 21.2: Build and start an McpManager if the config has any MCP servers.
///
/// Returns `Some(Arc<McpManager>)` when at least one enabled server is configured.
/// Background tasks are spawned via `start_all()` (D-07 fire-and-forget).
/// Prints startup messages to stderr per UI-SPEC (dimmed).
async fn build_mcp_manager(
    config: &Config,
    registry: Arc<RwLock<ToolRegistry>>,
) -> Option<Arc<McpManager>> {
    let mcp_configs: HashMap<String, ironhermes_mcp::McpServerConfig> = config
        .mcp_servers
        .iter()
        .filter_map(|(name, val)| {
            serde_yaml::from_value::<ironhermes_mcp::McpServerConfig>(val.clone())
                .ok()
                .map(|c| (name.clone(), c))
        })
        .collect();

    if mcp_configs.is_empty() {
        return None;
    }

    let n = mcp_configs.len();
    let mgr = Arc::new(McpManager::new(registry));
    let mgr_clone = mgr.clone();
    let configs_clone = mcp_configs.clone();

    // D-07: background discovery — agent starts immediately without waiting
    eprintln!(
        "{}",
        format!("MCP: connecting to {} server(s) in background...", n).dimmed()
    );
    tokio::spawn(async move {
        mgr_clone.start_all(configs_clone).await;
        let names = mgr_clone.connected_server_names();
        let tool_count = mgr_clone.registered_tool_count().await;
        if names.is_empty() {
            eprintln!("{}", "MCP: all servers failed to connect.".dimmed());
        } else if names.len() < n {
            eprintln!(
                "{}",
                format!(
                    "MCP: {} tool(s) ready. {} server(s) failed.",
                    tool_count,
                    n - names.len()
                )
                .dimmed()
            );
        } else {
            eprintln!(
                "{}",
                format!(
                    "MCP: {} tool(s) ready from {} server(s).",
                    tool_count,
                    names.len()
                )
                .dimmed()
            );
        }
    });

    Some(mgr)
}

fn print_banner() {
    println!(
        "{}",
        r#"
  ___              _  _
 |_ _|_ _ ___ _ _ | || |___ _ _ _ __  ___ ___
  | || '_/ _ \ ' \| __ / -_) '_| '  \/ -_|_-<
 |___|_| \___/_||_|_||_\___|_| |_|_|_\___/__/
"#
        .cyan()
    );
    println!(
        "  {} v{} — {}",
        "IronHermes".bold().cyan(),
        env!("CARGO_PKG_VERSION"),
        "Type /help for commands, /quit to exit".dimmed()
    );
    println!();
}

fn print_help() {
    // Phase 21.1 Plan 02: delegate to format_help via CommandRouter (no keybinding registry at startup).
    // When run_chat wires extensions, it uses dispatch_command which calls format_help directly.
    let router = crate::tui::commands::build_cli_router();
    println!(
        "{}",
        crate::tui::commands::format_help(&[], None, &router, &Platform::Local)
    );
}

#[cfg(test)]
mod tui_extension_wiring_tests {
    /// INV-22.1-01: run_chat uses new_with_extensions (not bare TuiHandle::new)
    #[test]
    fn run_chat_uses_new_with_extensions() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("new_with_extensions"),
            "run_chat must use TuiHandle::new_with_extensions"
        );
    }

    /// INV-22.1-02: run_chat uses dispatch_command (not inline match)
    #[test]
    fn run_chat_uses_dispatch_command() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("dispatch_command("),
            "run_chat must use dispatch_command for command routing"
        );
    }

    /// INV-22.1-03: run_chat builds KeybindingRegistry
    #[test]
    fn run_chat_builds_keybinding_registry() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("KeybindingRegistry::register_from_extensions"),
            "run_chat must build keybinding registry from extensions"
        );
    }

    /// INV-22.1-04: run_chat uses prepare_prompt_with_reserve
    #[test]
    fn run_chat_uses_dynamic_prompt_reserve() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("prepare_prompt_with_reserve"),
            "run_chat must use dynamic prompt reserve, not hardcoded prepare_prompt()"
        );
    }

    /// INV-22.1-05: run_chat has pre-readline keybinding check
    #[test]
    fn run_chat_has_pre_readline_keybinding_check() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("match_key(") && src.contains("KeyContext::Idle"),
            "run_chat must check keybindings before readline"
        );
    }

    /// INV-21.1-01: run_chat constructs a CommandRouter from build_command_registry
    #[test]
    fn run_chat_constructs_command_router() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("CommandRouter::new(build_command_registry())"),
            "run_chat must construct CommandRouter::new(build_command_registry())"
        );
    }

    /// INV-21.1-02: run_chat constructs a CommandContext
    #[test]
    fn run_chat_constructs_command_context() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("CommandContext::new("),
            "run_chat must construct CommandContext::new() for command dispatch"
        );
    }

    /// INV-21.1-03: run_chat has agent_running flag
    #[test]
    fn run_chat_has_agent_running_flag() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("agent_running"),
            "run_chat must track agent_running state"
        );
    }

    /// INV-21.4-GAP6-01: run_single calls on_session_end after agent.run()
    #[test]
    fn run_single_calls_on_session_end() {
        let src = include_str!("main.rs");
        let run_single_body = src.split("async fn run_single").nth(1).unwrap_or("");
        // Extract until the next async fn definition
        let until_next_fn = run_single_body
            .split("\nasync fn ")
            .next()
            .unwrap_or(run_single_body);
        assert!(
            until_next_fn.contains("on_session_end"),
            "run_single must call on_session_end before returning (GAP-6)"
        );
    }

    /// INV-21.4-GAP6-02: run_chat clean exit calls on_session_end
    #[test]
    fn run_chat_clean_exit_calls_on_session_end() {
        let src = include_str!("main.rs");
        let run_chat_body = src.split("async fn run_chat").nth(1).unwrap_or("");
        let until_next_fn = run_chat_body
            .split("\nasync fn ")
            .next()
            .unwrap_or(run_chat_body);
        assert!(
            until_next_fn.contains("on_session_end"),
            "run_chat must call on_session_end on clean exit (GAP-6)"
        );
    }

    /// GAP-5: run_chat must flush stdout after print_banner() so the
    /// banner reaches the terminal before rl.readline blocks on stdin.
    /// Without this flush, the CLI appears frozen when mcp_servers is
    /// configured (violates D-07 non-blocking-startup contract).
    #[test]
    fn initial_prompt_flush_precedes_readline() {
        let src = include_str!("main.rs");

        // The flush call must appear in the file (at least one of the two new sites).
        assert!(
            src.contains("io::stdout().flush().ok();"),
            "GAP-5: run_chat must call io::stdout().flush().ok() to force the banner paint"
        );

        // Ordering: at least one stdout flush must appear BEFORE the main REPL
        // `let readline = rl.readline(` call site. Using byte-offset comparison
        // of the first match of each literal.
        let flush_idx = src
            .find("io::stdout().flush().ok();")
            .expect("flush call must exist somewhere in main.rs");
        let readline_idx = src
            .find("let readline = rl.readline(")
            .expect("run_chat must still call rl.readline for the REPL");
        assert!(
            flush_idx < readline_idx,
            "GAP-5: io::stdout().flush().ok() must appear in source order BEFORE \
             the `let readline = rl.readline(` site so the banner paints before \
             the first stdin block. flush_idx={flush_idx}, readline_idx={readline_idx}"
        );
    }

    /// GAP-5 companion: stderr flush must also exist (complements GAP-6 plan 09)
    /// so the synchronous `MCP: connecting to N server(s) in background...`
    /// line is not left in stderr's buffer behind the banner paint.
    #[test]
    fn initial_prompt_flushes_stderr_too() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("io::stderr().flush().ok();"),
            "GAP-5: run_chat must also call io::stderr().flush().ok() after \
             print_banner() so the 'MCP: connecting ...' dimmed line is not \
             left buffered behind the prompt"
        );
    }
}

#[cfg(test)]
mod ensure_home_dirs_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_ensure_home_dirs_creates_all_subdirs() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        ensure_home_dirs().unwrap();

        for sub in &["cron", "sessions", "logs", "hooks", "memories", "skills", "workspace"] {
            assert!(tmp.path().join(sub).is_dir(), "Missing directory: {}", sub);
        }

        ensure_home_dirs().unwrap();
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }
    }

    /// D-05 (Phase 21.7): `$HERMES_HOME/subagent-transcripts/` must be part of
    /// the first-run scaffold so downstream writers can
    /// `tokio::fs::write(subagent_transcripts_dir.join(...))` without existence
    /// checks.
    #[test]
    fn home_dirs_includes_subagent_transcripts() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
        ensure_home_dirs().unwrap();
        assert!(
            tmp.path().join("subagent-transcripts").is_dir(),
            "D-05: $HERMES_HOME/subagent-transcripts must exist after first-run scaffold"
        );
        ensure_home_dirs().unwrap();
        unsafe { std::env::remove_var("IRONHERMES_HOME"); }
    }
}

#[cfg(test)]
mod mcp_wiring_tests {
    /// INV-21.2-01: run_chat constructs McpManager (static-grep regression guard)
    #[test]
    fn run_chat_wires_mcp_manager() {
        let src = include_str!("main.rs");
        assert!(src.contains("McpManager::new"), "run_chat must construct McpManager");
        assert!(src.contains("start_all"), "run_chat must call start_all");
    }

    /// INV-21.2-02: McpManager wired in at least 2 run paths (run_chat + run_single)
    #[test]
    fn run_single_wires_mcp_manager() {
        let src = include_str!("main.rs");
        // build_mcp_manager is called in run_chat, run_single, and run_gateway
        let count = src.matches("build_mcp_manager").count();
        assert!(
            count >= 3,
            "build_mcp_manager must be called in at least 3 run paths (chat, single, gateway), got {}",
            count
        );
    }

    /// INV-21.2-03: CommandContext has mcp_reloader wired (D-15)
    #[test]
    fn command_context_has_mcp_reloader() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("with_mcp_reloader"),
            "CommandContext must have MCP reloader wired via with_mcp_reloader"
        );
    }

    /// INV-21.2-04: McpReload arm handles result.failed for D-12 partial failure display
    #[test]
    fn mcp_reload_arm_handles_failures() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("result.failed.is_empty()"),
            "McpReload arm must check result.failed.is_empty() for D-12 partial failure display"
        );
    }

    /// INV-21.2-05: MCP startup message printed to stderr
    #[test]
    fn mcp_startup_message_printed_to_stderr() {
        let src = include_str!("main.rs");
        assert!(
            src.contains("MCP: connecting to"),
            "MCP startup message must be printed to stderr"
        );
    }

    /// INV-21.2-06: Commands enum has Mcp variant and dispatches to handle_mcp_command (D-13, D-14)
    #[test]
    fn commands_enum_has_mcp_variant() {
        let src = include_str!("main.rs");
        assert!(src.contains("Commands::Mcp"), "Commands enum must have Mcp variant");
        assert!(src.contains("handle_mcp_command"), "main must dispatch to handle_mcp_command");
    }
}
