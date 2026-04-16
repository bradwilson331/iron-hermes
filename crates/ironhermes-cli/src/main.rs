use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use ironhermes_agent::{AgentLoop, AgentSubagentRunner, AnyClient, PressureTracker, PromptBuilder, build_client as build_provider_client, build_main_client};
use ironhermes_core::{ChatMessage, Config, MemoryProvider, ProviderResolver, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_gateway::GatewayRunner;
use ironhermes_tools::ToolRegistry;
use std::io::{self, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::info;

mod cron;
mod batch;
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

    // Per D-03: CLI shares the same state.db; per D-11: CLI uses its own Connection
    let mut state_store = ironhermes_state::StateStore::open_default()
        .context("failed to open state.db for CLI")?;
    let session_id = uuid::Uuid::new_v4().to_string();
    state_store.create_session(&session_id, "cli", Some(client.model()), None, None)
        .context("failed to create CLI session")?;
    let budget = Arc::new(AtomicUsize::new(0));
    let mut registry = build_registry();

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
        None, // no memory store in single mode
        config.subagent.clone(),
        None, // no cancel token in single mode
        None, // no progress callback in single mode
    );

    let max_turns = cli
        .max_turns
        .unwrap_or(config.agent.max_turns);

    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .with_provider(&config.model.provider)
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry.clone());
    prompt_builder.load_memory();
    prompt_builder.load_skills();
    let system_msg = prompt_builder.build_system_message();

    let user_msg = ChatMessage::user(prompt);
    state_store.add_message(&session_id, &user_msg)
        .context("failed to persist user message")?;
    let messages = vec![system_msg, user_msg];

    let mut agent = AgentLoop::new(client, Arc::new(registry), max_turns)
        .with_budget(budget)
        .with_compression(128_000, config.agent.context_compression)
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
    agent = ironhermes_agent::attach_context_engine(
        agent,
        &config,
        &resolver,
        session_id.as_str(),
        None, // CLI does not register a hook registry
        None, // one-shot: fresh tracker per run
    );

    let result = agent.run(messages).await?;

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

    // Register delegate_task tool (AGENT-01..05)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        client.clone(),
        resolver.clone(),
        Some(budget.clone()),
    ));
    let chat_cancel_token = CancellationToken::new();

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
        None, // no memory store in chat mode
        config.subagent.clone(),
        Some(chat_cancel_token.clone()),
        Some(subagent_progress),
    );

    let registry = Arc::new(registry);
    let max_turns = cli.max_turns.unwrap_or(config.agent.max_turns);

    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .with_provider(&config.model.provider)
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry);
    prompt_builder.load_memory();
    prompt_builder.load_skills();
    let system_msg = prompt_builder.build_system_message();

    let mut messages = vec![system_msg];

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
        let readline = rl.readline(&format!("{} ", "You:".bold().green()));
        match readline {
            Ok(line) => {
                let input = line.trim().to_string();
                if input.is_empty() {
                    continue;
                }

                // Handle special commands
                match input.as_str() {
                    "/quit" | "/exit" | "/q" => {
                        println!("{}", "Goodbye!".dimmed());
                        break;
                    }
                    "/clear" => {
                        messages.truncate(1); // Keep system message
                        println!("{}", "Conversation cleared.".dimmed());
                        continue;
                    }
                    "/status" => {
                        cmd_status()?;
                        continue;
                    }
                    "/help" => {
                        print_help();
                        continue;
                    }
                    _ => {}
                }

                let _ = rl.add_history_entry(&input);
                let user_msg = ChatMessage::user(&input);
                let _ = state_store.add_message(&session_id, &user_msg);
                messages.push(user_msg);

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
                )
                .await?;
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
                println!("{}", "^C — type /quit to exit".dimmed());
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
) -> Result<Option<String>> {
    // Phase 18-14: seed the AgentLoop's compression_count from the shared
    // session-scoped counter so the summarizing engine's prior-summary chain
    // continues across REPL turns instead of resetting to 0 each prompt.
    let starting_count = compression_count.load(Ordering::SeqCst);
    let mut agent = AgentLoop::new(client.clone(), registry, max_turns)
        .with_budget(budget.clone())
        .with_compression(128_000, config.agent.context_compression)
        .with_compression_count(starting_count)
        .with_streaming(Box::new(|delta| {
            print!("{}", delta);
            io::stdout().flush().ok();
        }))
        .with_tool_progress(Box::new(|name, _args| {
            eprint!("\r{} {}...", "Running:".dimmed(), name.yellow());
            io::stderr().flush().ok();
        }));

    // Wire fallback from resolver
    let main_endpoint = resolver.resolve_for_main();
    if let Some(fb_name) = main_endpoint.fallback_providers.first() {
        if let Ok(fb_client) = build_provider_client(resolver, fb_name, &main_endpoint.default_model) {
            agent = agent.with_fallback(fb_client);
        }
    }

    // Phase 18 Plan 09: wire agent-side context compression.
    // Phase 18-14: reuse the session-scoped PressureTracker so hysteresis
    // state (above_threshold, pending_transient) survives across turns.
    agent = ironhermes_agent::attach_context_engine(
        agent,
        config,
        resolver,
        session_id,
        None,
        Some(pressure_tracker.clone()),
    );

    // Pass a clone of messages so agent can work with them
    let result = agent.run(messages.clone()).await?;

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

    // Build the memory provider from config (MEM-12, D-09, D-11, D-12).
    // Feature-gated: memory.provider=sqlite requires --features memory-sqlite, etc.
    // Factory handles disk-load for the file provider internally.
    let memory_store: Arc<Mutex<dyn MemoryProvider + Send>> =
        ironhermes_agent::memory::factory::build_memory_provider(&config.memory).await?;

    // Build registry and register memory tool before Arc wrapping
    let mut registry = build_registry();
    registry.register_memory_tool(memory_store.clone());

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
    rpc_registry.register_memory_tool(memory_store.clone());
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
        Some(memory_store.clone()),
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
    runner.set_memory_store(memory_store);
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
    println!("{}", "Commands:".bold());
    println!("  {}  — Exit the chat", "/quit".yellow());
    println!("  {}  — Clear conversation history", "/clear".yellow());
    println!("  {} — Show status", "/status".yellow());
    println!("  {}   — Show this help", "/help".yellow());
}
