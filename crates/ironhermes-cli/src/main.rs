use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use ironhermes_agent::{AgentLoop, AgentSubagentRunner, LlmClient, PromptBuilder};
use ironhermes_core::{ChatMessage, Config, MemoryStore, SkillRegistry};
use ironhermes_cron::JobStore;
use ironhermes_gateway::GatewayRunner;
use ironhermes_tools::ToolRegistry;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

mod cron;
mod batch;

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
    let (client, config) = build_client(cli)?;
    let mut registry = build_registry();

    // Register delegate_task tool (AGENT-01..05)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        client.clone(),
        config.resolve_base_url(),
        config.resolve_api_key().unwrap_or_default(),
        config.subagent.base_url.clone(),
        config.subagent.api_key.clone(),
    ));
    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
        None, // no memory store in single mode
        config.subagent.clone(),
    );

    let max_turns = cli
        .max_turns
        .unwrap_or(config.agent.max_turns);

    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry.clone());
    let system_msg = prompt_builder.build_system_message();

    let messages = vec![system_msg, ChatMessage::user(prompt)];

    let agent = AgentLoop::new(client, Arc::new(registry), max_turns)
        .with_compression(128_000, config.agent.context_compression)
        .with_streaming(Box::new(|delta| {
            print!("{}", delta);
            io::stdout().flush().ok();
        }))
        .with_tool_progress(Box::new(|name, args| {
            eprintln!("{} {} {}", "Tool:".dimmed(), name.yellow(), args.dimmed());
        }));

    let result = agent.run(messages).await?;

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

    let (client, config) = build_client(cli)?;
    let mut registry = build_registry();

    // Register delegate_task tool (AGENT-01..05)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        client.clone(),
        config.resolve_base_url(),
        config.resolve_api_key().unwrap_or_default(),
        config.subagent.base_url.clone(),
        config.subagent.api_key.clone(),
    ));
    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
        None, // no memory store in chat mode
        config.subagent.clone(),
    );

    let registry = Arc::new(registry);
    let max_turns = cli.max_turns.unwrap_or(config.agent.max_turns);

    let cwd = std::env::current_dir().unwrap_or_default();
    let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
    let mut prompt_builder = PromptBuilder::new(client.model(), "cli")
        .load_context(&cwd);
    prompt_builder.set_skill_registry(skill_registry);
    let system_msg = prompt_builder.build_system_message();

    let mut messages = vec![system_msg];

    // Use rustyline for readline with history
    let mut rl = rustyline::DefaultEditor::new().context("Failed to initialize readline")?;

    // Handle initial message if provided
    if let Some(msg) = initial_message {
        messages.push(ChatMessage::user(&msg));
        println!("{} {}", "You:".bold().green(), msg);
        let response = run_agent_turn(&client, registry.clone(), &mut messages, max_turns, &config).await?;
        if let Some(text) = response {
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
                messages.push(ChatMessage::user(&input));

                let response =
                    run_agent_turn(&client, registry.clone(), &mut messages, max_turns, &config).await?;
                if let Some(_text) = response {
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

    Ok(())
}

/// Run one agent turn (may involve multiple tool calls).
async fn run_agent_turn(
    client: &LlmClient,
    registry: Arc<ToolRegistry>,
    messages: &mut Vec<ChatMessage>,
    max_turns: usize,
    config: &Config,
) -> Result<Option<String>> {
    let agent = AgentLoop::new(client.clone(), registry, max_turns)
        .with_compression(128_000, config.agent.context_compression)
        .with_streaming(Box::new(|delta| {
            print!("{}", delta);
            io::stdout().flush().ok();
        }))
        .with_tool_progress(Box::new(|name, _args| {
            eprint!("\r{} {}...", "Running:".dimmed(), name.yellow());
            io::stderr().flush().ok();
        }));

    // Pass a clone of messages so agent can work with them
    let result = agent.run(messages.clone()).await?;

    // Update messages with the full conversation (includes tool calls and results)
    *messages = result.messages;

    Ok(result.final_response)
}

/// Start the Telegram gateway bot.
async fn run_gateway(cli: &Cli, token_override: Option<String>) -> Result<()> {
    let (_, mut config) = build_client(cli)?;

    // Create MemoryStore and load from disk
    let memory_dir = ironhermes_core::get_hermes_home().join("memories");
    let mut store = MemoryStore::new(memory_dir);
    if let Err(e) = store.load_from_disk() {
        warn!("Failed to load memory from disk: {}", e);
    }
    let memory_store = Arc::new(Mutex::new(store));

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
    registry.register_skills_tool(skill_registry.clone(), active_skills.clone());

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

    // Register execute_code tool with the RPC dispatch registry
    registry.register_execute_code_tool(rpc_registry, config.exec.clone());

    // Register delegate_task tool (AGENT-01..05, AGENT-03 semaphore enforcement)
    let subagent_semaphore = Arc::new(tokio::sync::Semaphore::new(config.subagent.max_subagents));
    let gateway_client = LlmClient::new(
        config.resolve_base_url(),
        config.resolve_api_key().unwrap_or_default(),
        config.model.default.clone(),
    );
    let subagent_runner = Arc::new(AgentSubagentRunner::new(
        gateway_client,
        config.resolve_base_url(),
        config.resolve_api_key().unwrap_or_default(),
        config.subagent.base_url.clone(),
        config.subagent.api_key.clone(),
    ));
    registry.register_delegate_task_tool(
        subagent_runner,
        subagent_semaphore,
        Some(memory_store.clone()),
        config.subagent.clone(),
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
    let mut runner = GatewayRunner::new(config, registry);
    runner.set_memory_store(memory_store);
    runner.set_job_store(job_store);
    runner.set_skill_registry(skill_registry);
    runner.set_hook_registry(hook_registry);
    runner.set_active_skills(active_skills);
    runner.start().await
}

fn build_client(cli: &Cli) -> Result<(LlmClient, Config)> {
    let config = Config::load().unwrap_or_default();

    let model = cli
        .model
        .as_deref()
        .unwrap_or(&config.model.default);
    let base_url = config.resolve_base_url();
    let api_key = config
        .resolve_api_key()
        .context("No API key configured. Set OPENROUTER_API_KEY, ANTHROPIC_API_KEY, or OPENAI_API_KEY.")?;

    info!(model = %model, base_url = %base_url, "Initializing LLM client");

    Ok((LlmClient::new(base_url, api_key, model), config))
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
