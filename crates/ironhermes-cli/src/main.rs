use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use ironhermes_agent::{AgentLoop, AgentSubagentRunner, AnyClient, PressureTracker, PromptBuilder, build_client as build_provider_client, build_main_client};
use ironhermes_core::{ChatMessage, Config, ProviderResolver, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_gateway::GatewayRunner;
use ironhermes_tools::ToolRegistry;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
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
mod memory_setup;
mod models_cmd;
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
}

#[derive(Subcommand)]
enum Commands {
    /// Interactive chat mode (default)
    Chat {
        /// Initial message to send
        message: Option<String>,
    },
    /// Show current configuration and status
    Status,
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
}

#[derive(Subcommand)]
enum MemorySubcommand {
    /// Interactive setup for the currently-selected memory provider.
    Setup,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("ironhermes=info".parse().unwrap()),
        )
        .with_target(false)
        .init();

    // Load .env file
    let env_path = Config::env_path();
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }

    let cli = Cli::parse();

    // Phase 21.3: eagerly initialize tiktoken BPE tables to avoid ~100ms
    // latency on first token count.
    ironhermes_core::warm_tiktoken_singletons();

    match cli.command {
        Some(Commands::Status) => cmd_status(),
        Some(Commands::Doctor) => cmd_doctor(),
        Some(Commands::Version) => cmd_version(),
        Some(Commands::Chat { ref message }) => run_chat(&cli, message.clone()).await,
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
        Some(Commands::Models { command }) => models_cmd::handle_models_command(command).await,
        None => {
            if let Some(ref prompt) = cli.execute {
                run_single(&cli, prompt.clone()).await
            } else {
                run_chat(&cli, None).await
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

fn cmd_status() -> Result<()> {
    let config = Config::load().unwrap_or_default();

    println!("{}", "IronHermes Status".bold().cyan());
    println!("{}", "─".repeat(40));
    println!(
        "  Home:     {}",
        ironhermes_core::display_hermes_home()
    );
    println!("  Model:    {}", config.model.default);
    println!("  Provider: {}", config.model.provider);
    println!("  Terminal: {}", config.terminal.backend);
    println!("  Web:      {}", config.web.backend);

    // Check API keys
    let has_openrouter = std::env::var("OPENROUTER_API_KEY").is_ok();
    let has_anthropic = std::env::var("ANTHROPIC_API_KEY").is_ok();
    let has_openai = std::env::var("OPENAI_API_KEY").is_ok();

    println!();
    println!("{}", "API Keys".bold());
    println!(
        "  OpenRouter:  {}",
        if has_openrouter {
            "configured".green()
        } else {
            "not set".red()
        }
    );
    println!(
        "  Anthropic:   {}",
        if has_anthropic {
            "configured".green()
        } else {
            "not set".red()
        }
    );
    println!(
        "  OpenAI:      {}",
        if has_openai {
            "configured".green()
        } else {
            "not set".red()
        }
    );

    Ok(())
}

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
async fn run_single(cli: &Cli, prompt: String) -> Result<()> {
    let (client, config, resolver) = build_client(cli)?;

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
    let budget = Arc::new(AtomicUsize::new(0));
    let mut registry = build_registry();

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
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        client.clone(),
        resolver.clone(),
        Some(budget.clone()),
    ));
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

    // Phase 22: execute_code with active_skills for skill env var bypass (per D-02)
    registry.register_execute_code_tool_with_active_skills(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
    );

    // Phase 22: guardrails (per D-02, D-08 — before Arc wrap)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(registry);

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
async fn run_chat(cli: &Cli, initial_message: Option<String>) -> Result<()> {
    print_banner();

    let (client, config, resolver) = build_client(cli)?;

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
    let budget = Arc::new(AtomicUsize::new(0));
    // Phase 18-14: session-scoped PressureTracker + compression_count.
    // Constructed once per REPL session so hysteresis (above_threshold,
    // pending_transient) and the summarizing engine's prior-summary chain
    // survive across all run_agent_turn calls within this session.
    let pressure_tracker = Arc::new(PressureTracker::new());
    let compression_count = Arc::new(AtomicUsize::new(0));
    let mut registry = build_registry();

    // Plan 21-03: spawn the bottom-bar TUI (status line + knight-rider scanner).
    // Activity is Idle at startup; turns publish ActivityState::Streaming/ToolCall.
    let initial_status = StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        tokens_used: 0,
        tokens_limit: context_length,
        hint: "ctrl+c cancel · /help commands".to_string(),
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
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        client.clone(),
        resolver.clone(),
        Some(budget.clone()),
    ));
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

    // D-19: CLI tree-view progress callback for subagent tool calls
    let subagent_progress: ironhermes_tools::delegate_task::SubagentProgressCallback =
        Arc::new(|index, event| {
            use ironhermes_tools::delegate_task::SubagentProgress;
            match event {
                SubagentProgress::Started { task_summary } => {
                    eprintln!(
                        "  {} {}",
                        format!("[subagent-{}]", index + 1).cyan().dimmed(),
                        task_summary.dimmed(),
                    );
                }
                SubagentProgress::ToolCall { tool_name } => {
                    eprintln!(
                        "  {} {} {}",
                        format!("[subagent-{}]", index + 1).cyan().dimmed(),
                        "Running:".dimmed(),
                        tool_name.yellow().dimmed(),
                    );
                }
                SubagentProgress::Completed => {
                    eprintln!(
                        "  {} {}",
                        format!("[subagent-{}]", index + 1).cyan().dimmed(),
                        "Done.".dimmed(),
                    );
                }
            }
        });

    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
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

    // Phase 22: execute_code with active_skills for skill env var bypass (per D-02)
    registry.register_execute_code_tool_with_active_skills(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
    );

    // Phase 22: guardrails (per D-02, D-08 — before Arc wrap)
    let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
    if !hooks_config.blocked_tools.is_empty() {
        registry.add_guardrail(Box::new(
            ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
        ));
    }
    registry.set_error_detail(hooks_config.error_detail.clone());

    let registry = Arc::new(registry);

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

                    // Build CommandContext for this turn.
                    let cmd_ctx = CommandContext::new(
                        Platform::Local,
                        session_id.clone(),
                        agent_running.clone(),
                    );

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
                                    // D-12: memory flush not available via flush_to_disk —
                                    // on_session_end requires MemoryEntries; skip with debug log.
                                    tracing::debug!("tui: memory flush on interrupted-exit skipped — no flush_to_disk API");
                                    tui.cleanup_on_exit();
                                    let _ = state_store.end_session(&session_id, "interrupted");
                                    std::process::exit(0);
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
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                match double_ctrl_c.on_ctrl_c(Instant::now(), false) {
                    CtrlCDecision::ExitCleanly => {
                        println!("{}", "Goodbye!".dimmed());
                        tui.cleanup_on_exit();
                        let _ = state_store.end_session(&session_id, "interrupted");
                        std::process::exit(0);
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
    registry: Arc<ToolRegistry>,
    messages: &mut Vec<ChatMessage>,
    max_turns: usize,
    config: &Config,
    resolver: &ProviderResolver,
    budget: &Arc<AtomicUsize>,
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
    tui.set_status(StatusLineState {
        mode: "Chat".to_string(),
        model_short: client.model().to_string(),
        provider: config.model.provider.clone(),
        tokens_used: result.total_usage.total_tokens,
        tokens_limit: context_length,
        hint: "ctrl+c cancel · /help commands".to_string(),
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

    // Plan 20-02 / GAP-4: build the MemoryManager once — returns None when
    // config.memory.memory_enabled=false (T-21.4-02). All consumers guard
    // with if-let so None propagates cleanly.
    let memory_manager: Option<Arc<tokio::sync::Mutex<ironhermes_agent::MemoryManager>>> =
        ironhermes_agent::memory::factory::build_memory_manager(&config.memory).await?;

    // Build registry and register memory tool before Arc wrapping
    let mut registry = build_registry();
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
    // Phase 19 Plan 06 (D-05): pass active_skills so skill-declared env vars
    // bypass the sandbox secret-strip in `Sandbox::build_env`.
    registry.register_execute_code_tool_with_active_skills(
        rpc_registry,
        config.exec.clone(),
        active_skills.clone(),
    );

    // Register delegate_task tool (AGENT-01..05, AGENT-03 semaphore enforcement)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let gateway_client = build_main_client(&resolver)?;
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        gateway_client,
        resolver.clone(),
        None, // gateway budget wired per-request in handler
    ));
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

    let registry = Arc::new(registry);

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
    if let Some(mgr) = memory_manager {
        runner.set_memory_manager(mgr);
    }
    runner.set_job_store(job_store);
    runner.set_skill_registry(skill_registry);
    runner.set_hook_registry(hook_registry);
    runner.set_active_skills(active_skills);
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
}
