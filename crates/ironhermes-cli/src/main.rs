use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use colored::Colorize;
use ironhermes_agent::{AgentLoop, LlmClient, PromptBuilder};
use ironhermes_core::{ChatMessage, Config};
use ironhermes_tools::ToolRegistry;
use std::io::{self, Write};
use std::sync::Arc;
use tracing::info;

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
    let registry = build_registry();
    let max_turns = cli
        .max_turns
        .unwrap_or(config.agent.max_turns);

    let prompt_builder = PromptBuilder::new(client.model(), "cli");
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
    let registry = Arc::new(build_registry());
    let max_turns = cli.max_turns.unwrap_or(config.agent.max_turns);

    let prompt_builder = PromptBuilder::new(client.model(), "cli");
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
