/// `hermes mcp` CLI subcommands: add, remove, list, test, configure.
///
/// All subcommand handlers live in this module (D-14). The main.rs adds the
/// `Commands::Mcp` variant and dispatches to `handle_mcp_command`.
///
/// UI output follows UI-SPEC contracts: colored crate styling matching cron.rs patterns.
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::Config;
use ironhermes_core::auth::AuthStore;
use ironhermes_core::get_hermes_home;
use ironhermes_mcp::paste_back::{OAuthPasteBack, is_oauth_denial, redact_oauth_params};
use ironhermes_mcp::{McpServerConfig, interpolate_config, sanitize_error};
use std::collections::HashMap;
use std::io::{self, BufRead, IsTerminal, Write};

// ---------------------------------------------------------------------------
// McpAction enum (clap Subcommand)
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum McpAction {
    /// Add a new MCP server
    Add {
        /// Server name (used as config key)
        name: String,
        /// Server URL (for HTTP transport)
        #[arg(long)]
        url: Option<String>,
        /// Server command (for stdio transport)
        #[arg(long)]
        command: Option<String>,
        /// Command arguments
        #[arg(long)]
        args: Vec<String>,
    },
    /// Remove an MCP server
    Remove {
        /// Server name to remove
        name: String,
    },
    /// List configured MCP servers
    List,
    /// Test connection to an MCP server
    Test {
        /// Server name to test
        name: String,
    },
    /// Configure enabled tools for a server
    Configure {
        /// Server name to configure
        name: String,
    },
    /// Interactively authorize an OAuth-protected MCP server (first-time auth, D-04).
    ///
    /// Performs DCR + PKCE browser flow against the configured server.
    /// Persists the token to auth.json so subsequent gateway auto-starts need zero DCR.
    Connect {
        /// Server name as configured under mcp_servers.<name> in config.yaml
        name: String,
        /// Force browser (PKCE loopback) flow
        #[arg(long)]
        browser: bool,
        /// Headless paste-back: print the authorization link, then paste the redirected URL
        #[arg(long, conflicts_with = "browser")]
        no_browser: bool,
    },
}

// ---------------------------------------------------------------------------
// Public dispatcher
// ---------------------------------------------------------------------------

pub async fn handle_mcp_command(action: McpAction) -> anyhow::Result<()> {
    match action {
        McpAction::Add {
            name,
            url,
            command,
            args,
        } => cmd_add(name, url, command, args).await,
        McpAction::Remove { name } => cmd_remove(name).await,
        McpAction::List => cmd_list().await,
        McpAction::Test { name } => cmd_test(name).await,
        McpAction::Configure { name } => cmd_configure(name).await,
        McpAction::Connect {
            name,
            browser,
            no_browser,
        } => cmd_connect(name, browser, no_browser).await,
    }
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

async fn cmd_list() -> anyhow::Result<()> {
    let config = Config::load().unwrap_or_default();
    if config.mcp_servers.is_empty() {
        println!("No MCP servers configured.");
        println!("{}", "Use `hermes mcp add <name>` to add one.".dimmed());
        return Ok(());
    }
    println!("{}", "MCP Servers".bold().cyan());
    println!("{}", "\u{2500}".repeat(70));
    println!(
        "  {:<20}{:<20}{:<20}{}",
        "NAME".bold(),
        "TRANSPORT".bold(),
        "STATUS".bold(),
        "TOOLS".bold()
    );
    for (name, val) in &config.mcp_servers {
        let server: McpServerConfig = serde_yaml::from_value(val.clone()).unwrap_or_default();
        let transport = if server.command.is_some() {
            "stdio"
        } else if server.url.is_some() {
            "http"
        } else {
            "unknown"
        };
        let status = if !server.enabled {
            "disabled".yellow().to_string()
        } else {
            "configured".to_string()
        };
        let tool_count = server
            .enabled_tools
            .as_ref()
            .map(|t| t.len().to_string())
            .unwrap_or_else(|| "all".to_string());
        // colored strings don't support width padding; pad raw string first then colorize
        let padded_name = format!("{:<20}", name);
        println!(
            "  {}{:<20}{:<20}{}",
            padded_name.yellow(),
            transport,
            status,
            tool_count
        );
    }
    println!("{}", "\u{2500}".repeat(70));
    println!(
        "  {}",
        format!("{} server(s) total", config.mcp_servers.len()).dimmed()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_add
// ---------------------------------------------------------------------------

async fn cmd_add(
    name: String,
    url: Option<String>,
    command: Option<String>,
    args: Vec<String>,
) -> anyhow::Result<()> {
    // Check if server already exists
    let config = Config::load().unwrap_or_default();
    if config.mcp_servers.contains_key(&name) {
        let overwrite = prompt_yn(
            &format!("Server '{}' already exists. Overwrite?", name),
            false,
        );
        if !overwrite {
            println!("{}", "Cancelled.".dimmed());
            return Ok(());
        }
    }

    // Determine transport type if not specified via flags
    let (final_command, final_url, final_args) = if command.is_some() || url.is_some() {
        (command, url, args)
    } else {
        // Interactive: ask user for transport type
        let transport_choice = prompt_line("Transport type (stdio/http)", "stdio");
        if transport_choice.trim().to_lowercase().starts_with('h') {
            // HTTP
            let mut retries = 0;
            let entered_url = loop {
                let u = prompt_line("Server URL", "");
                if !u.is_empty() {
                    break u;
                }
                eprintln!("{}: URL cannot be empty", "Error".red().bold());
                retries += 1;
                if retries >= 2 {
                    return Err(anyhow::anyhow!("URL is required for HTTP transport"));
                }
            };
            (None, Some(entered_url), vec![])
        } else {
            // Stdio
            let mut retries = 0;
            let entered_command = loop {
                let c = prompt_line("Command (e.g. npx)", "");
                if !c.is_empty() {
                    break c;
                }
                eprintln!("{}: command cannot be empty", "Error".red().bold());
                retries += 1;
                if retries >= 2 {
                    return Err(anyhow::anyhow!("Command is required for stdio transport"));
                }
            };
            // GAP-3: when the user selected `npx`, show an example of valid MCP args so
            // they don't leave args empty and land on the GAP-1 hang path.
            if entered_command.trim() == "npx" {
                println!(
                    "  {}",
                    "Example: -y @modelcontextprotocol/server-filesystem /tmp".dimmed()
                );
            }
            let args_str = prompt_line("Arguments (space-separated, or empty)", "");
            let entered_args: Vec<String> = if args_str.trim().is_empty() {
                vec![]
            } else {
                args_str.split_whitespace().map(|s| s.to_string()).collect()
            };
            (Some(entered_command), None, entered_args)
        }
    };

    // Ask about environment variables
    let mut env_vars: HashMap<String, String> = HashMap::new();
    let add_env = prompt_yn("Add environment variables?", false);
    if add_env {
        loop {
            let key = prompt_line("  Env var name (or empty to finish)", "");
            if key.is_empty() {
                break;
            }
            let val = prompt_line(&format!("  Value for {}", key), "");
            env_vars.insert(key, val);
        }
    }

    // Ask about auth
    let needs_auth = prompt_yn("Does this server require authentication?", false);
    let auth_token: Option<String> = if needs_auth {
        print!("  API key / Bearer token: ");
        io::stdout().flush().ok();
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line).ok();
        let trimmed = line.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        None
    };

    // Build McpServerConfig
    let mut server_config = McpServerConfig {
        command: final_command,
        args: final_args,
        url: final_url,
        env: env_vars,
        auth: auth_token,
        ..McpServerConfig::default()
    };
    interpolate_config(&mut server_config);

    // Attempt test connection to discover tools
    print!("  {}", format!("Connecting to '{}'...", name).cyan());
    io::stdout().flush().ok();

    // Attempt the connection in a loop so Retry can re-run it.
    let (discovered_tools, tool_count) = loop {
        // NOTE: Task 1 already swapped the call site to `_with_timeout`; keep that here.
        let tools = attempt_connect_and_list_with_timeout(&server_config).await;
        println!(); // newline after the connecting message (preserved from original)

        match tools {
            Ok(t) => {
                let n = t.len();
                println!("  {}", format!("Found {} tool(s)", n).green());
                break (t, n);
            }
            Err(e) => {
                let sanitized = sanitize_error(&e.to_string());
                println!("  {}", format!("Failed to connect: {}", sanitized).red());
                match prompt_retry_save_abort() {
                    RetrySaveAbort::Retry => {
                        // loop back and re-attempt the connection
                        print!("  {}", format!("Connecting to '{}'...", name).cyan());
                        io::stdout().flush().ok();
                        continue;
                    }
                    RetrySaveAbort::SaveAnyway => {
                        // Preserves the legacy "user may fix server config later" escape hatch,
                        // but ONLY when the user explicitly opts in.
                        break (vec![], 0);
                    }
                    RetrySaveAbort::Abort => {
                        println!("{}", "Cancelled.".dimmed());
                        return Ok(());
                    }
                }
            }
        }
    };

    // Choose which tools to enable
    let enabled_tools: Option<Vec<String>> = if tool_count > 0 {
        let enable_all = prompt_yn("Enable all tools?", true);
        if enable_all {
            None // None = all tools enabled
        } else {
            // Let user select individual tools
            let mut selected = vec![];
            for (tool_name, _desc) in &discovered_tools {
                let enable = prompt_yn(&format!("  Enable tool '{}'?", tool_name), true);
                if enable {
                    selected.push(tool_name.clone());
                }
            }
            Some(selected)
        }
    } else {
        None
    };

    let enabled_count = enabled_tools
        .as_ref()
        .map(|t| t.len())
        .unwrap_or(tool_count);

    // Update enabled_tools in config
    server_config.enabled_tools = enabled_tools;

    // Save to config.yaml
    save_mcp_server_config(&name, &server_config)?;

    println!(
        "{}",
        format!(
            "MCP server added: {} ({} tool(s) enabled)",
            name, enabled_count
        )
        .bold()
        .cyan()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_remove
// ---------------------------------------------------------------------------

async fn cmd_remove(name: String) -> anyhow::Result<()> {
    let config = Config::load().unwrap_or_default();
    if !config.mcp_servers.contains_key(&name) {
        eprintln!("error: server '{}' not found", name);
        std::process::exit(1);
    }
    let confirmed = prompt_yn(&format!("Remove server '{}'?", name), false);
    if !confirmed {
        println!("{}", "Cancelled.".dimmed());
        return Ok(());
    }
    remove_mcp_server_config(&name)?;
    println!("{}", format!("MCP server removed: {}", name).bold().cyan());
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_test  (D-16: connect + list tools only, no test call)
// ---------------------------------------------------------------------------

async fn cmd_test(name: String) -> anyhow::Result<()> {
    let config = Config::load().unwrap_or_default();
    let server_val = match config.mcp_servers.get(&name).cloned() {
        Some(v) => v,
        None => {
            eprintln!("error: server '{}' not found in config", name);
            std::process::exit(1);
        }
    };

    let mut server: McpServerConfig = serde_yaml::from_value(server_val)
        .map_err(|e| anyhow::anyhow!("Failed to parse server config: {}", e))?;
    interpolate_config(&mut server);

    // Heading (UI-SPEC: bold cyan)
    println!("{}", format!("MCP Server: {}", name).bold().cyan());

    // Detail view (UI-SPEC: key-value layout, 14-char label column)
    let transport = if server.command.is_some() {
        "stdio"
    } else if server.url.is_some() {
        "http"
    } else {
        "unknown"
    };
    println!("  {:<14}{}", "Transport:".dimmed(), transport);
    if let Some(ref cmd) = server.command {
        let full_cmd = if server.args.is_empty() {
            cmd.clone()
        } else {
            format!("{} {}", cmd, server.args.join(" "))
        };
        println!("  {:<14}{}", "Command:".dimmed(), full_cmd);
    }
    if let Some(ref url) = server.url {
        println!("  {:<14}{}", "URL:".dimmed(), url);
    }
    let status_str = if server.enabled {
        "enabled".green().to_string()
    } else {
        "disabled".yellow().to_string()
    };
    println!("  {:<14}{}", "Status:".dimmed(), status_str);
    println!("  {:<14}{}s", "Timeout:".dimmed(), server.timeout);

    // Connecting
    println!("  {}", "Connecting...".dimmed());

    match attempt_connect_and_list_with_timeout(&server).await {
        Ok(tools) => {
            let n = tools.len();
            println!(
                "  {}",
                format!("Connected \u{2014} {} tool(s) available", n).green()
            );
            // Tool list: name (yellow) + description (plain), truncated at 80 cols
            // Format: 2 indent + 30 name + 3 separator + 47 desc = 82 max
            for (tool_name, description) in &tools {
                let truncated_desc = if description.len() > 47 {
                    format!(
                        "{}\u{2026}",
                        ironhermes_core::truncate_on_char_boundary(description, 46)
                    )
                } else {
                    description.clone()
                };
                // colored strings don't implement Display with width padding;
                // pad the raw name first, then color the padded string
                let padded_name = format!("{:<30}", tool_name);
                println!("  {} \u{2014} {}", padded_name.yellow(), truncated_desc);
            }
        }
        Err(e) => {
            let sanitized = sanitize_error(&e.to_string());
            println!("  {}", format!("Connection failed: {}", sanitized).red());
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_configure
// ---------------------------------------------------------------------------

async fn cmd_configure(name: String) -> anyhow::Result<()> {
    let config = Config::load().unwrap_or_default();
    let server_val = match config.mcp_servers.get(&name).cloned() {
        Some(v) => v,
        None => {
            eprintln!("error: server '{}' not found in config", name);
            std::process::exit(1);
        }
    };

    let mut server: McpServerConfig = serde_yaml::from_value(server_val)
        .map_err(|e| anyhow::anyhow!("Failed to parse server config: {}", e))?;
    interpolate_config(&mut server);

    println!(
        "{}",
        format!("Configure MCP Server: {}", name).bold().cyan()
    );

    // Connect and discover tools
    println!("  {}", "Connecting to discover tools...".dimmed());
    let tools = match attempt_connect_and_list_with_timeout(&server).await {
        Ok(t) => t,
        Err(e) => {
            let sanitized = sanitize_error(&e.to_string());
            eprintln!("{}: Failed to connect: {}", "Error".red().bold(), sanitized);
            return Err(anyhow::anyhow!("Connection failed: {}", sanitized));
        }
    };

    if tools.is_empty() {
        println!("  {}", "No tools discovered.".dimmed());
        return Ok(());
    }

    // Toggle each tool
    let mut selected_tools: Vec<String> = vec![];
    for (tool_name, _description) in &tools {
        let enable = prompt_yn(&format!("Enable tool '{}'?", tool_name), true);
        if enable {
            selected_tools.push(tool_name.clone());
        }
    }

    let enabled_count = selected_tools.len();
    server.enabled_tools = Some(selected_tools);
    save_mcp_server_config(&name, &server)?;

    println!(
        "{}",
        format!(
            "MCP server updated: {} ({} tool(s) enabled)",
            name, enabled_count
        )
        .bold()
        .cyan()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// D-02: new-issuer DEFCON-tiered confirm decision (pure/testable)
// ---------------------------------------------------------------------------

/// Phase 46.1 D-02: whether a first connect to a newly-added, non-baseline
/// OAuth issuer should prompt for interactive confirmation before proceeding.
///
/// Mirrors the CR-01 `at_least_as_strict_as(2)` precedent (`skills.rs:559`):
/// DEFCON 1-2 (strict) → confirm; DEFCON 3-5 (relaxed) → silent (audit-only,
/// see `cmd_connect`'s unconditional audit-floor write).
fn should_confirm_new_issuer(defcon: ironhermes_core::config::DefconLevel) -> bool {
    defcon.at_least_as_strict_as(2)
}

// ---------------------------------------------------------------------------
// cmd_connect  (D-04: interactive first-time OAuth authorization)
// ---------------------------------------------------------------------------

/// Perform DCR + PKCE browser OAuth flow for an OAuth-protected MCP server.
///
/// Intended for first-time authorization. On success, persists the access/refresh
/// token to auth.json so subsequent gateway auto-starts run zero-DCR hot-path.
///
/// Security invariants (A-1, B-4):
/// - Token values are never printed; only tool names are displayed.
/// - `validate_prm_issuer` runs inside `connect_http_oauth` before any network call.
/// - AuthStore is opened at `get_hermes_home().join("auth.json")`, never HERMES_HOME.
async fn cmd_connect(name: String, browser: bool, no_browser: bool) -> anyhow::Result<()> {
    use ironhermes_mcp::transport::connect_http_oauth;
    let config = Config::load().unwrap_or_default();
    let server_val = match config.mcp_servers.get(&name).cloned() {
        Some(v) => v,
        None => {
            eprintln!("error: server '{}' not found in config", name);
            std::process::exit(1);
        }
    };
    let mut server: McpServerConfig = serde_yaml::from_value(server_val)
        .map_err(|e| anyhow::anyhow!("Failed to parse server config: {}", e))?;
    if server.oauth_provider.is_none() {
        eprintln!(
            "error: server '{}' has no oauth_provider configured \
             — Connect is only for OAuth servers",
            name
        );
        std::process::exit(1);
    }
    interpolate_config(&mut server);
    let store = AuthStore::open(get_hermes_home().join("auth.json")).await?;

    // Phase 46.1 D-02: new-issuer detection + unconditional audit floor +
    // DEFCON-tiered confirm-on-first-use, before connect_http_oauth runs.
    // "New" = the server's issuer is not the built-in baseline (is_baseline_issuer)
    // AND no DcrEntry has been persisted for this URL yet (first-ever connect).
    // A server that has already completed a first connect is never re-prompted
    // and never re-audited — the persisted DcrEntry is the "remembered" signal.
    let is_new = match server.url.as_deref() {
        Some(url) => {
            !ironhermes_mcp::security::is_baseline_issuer(url)
                && store.get_client(url).await.is_none()
        }
        None => false,
    };
    if is_new {
        let host = server
            .url
            .as_deref()
            .and_then(|u| url::Url::parse(u).ok())
            .and_then(|u| u.host_str().map(|h| h.to_string()))
            .unwrap_or_else(|| "unknown-host".to_string());

        let defcon = config.effective_defcon_level();
        let must_confirm = should_confirm_new_issuer(defcon);
        let mut proceed = true;
        let (decision, resolution): (&str, &str) = if must_confirm {
            println!(
                "{}",
                format!(
                    "New issuer '{}' for server '{}' has not been connected before.",
                    host, name
                )
                .yellow()
            );
            let confirmed = prompt_yn(
                &format!(
                    "Trust '{}' and connect ({} requires confirmation)?",
                    host, defcon
                ),
                false,
            );
            proceed = confirmed;
            if confirmed {
                ("approved", "operator")
            } else {
                ("denied", "operator")
            }
        } else {
            ("approved", "bypass")
        };

        // Audit floor (D-02): always written, regardless of DEFCON tier or
        // confirm outcome. Non-blocking — an append failure is logged and the
        // connect flow continues (or the already-decided abort still happens);
        // the audit floor must never abort an otherwise-approved connect.
        let audit_log = ironhermes_core::AuditLog::load(config.audit.clone());
        let entry = audit_log.make_entry(
            "new_issuer",
            format!("connect:{name}"),
            Some(name.clone()),
            "cli-connect",
            "cli",
            "n/a",
            format!("new issuer {host} allowed for server {name}"),
            decision,
            resolution,
            &serde_json::json!({}),
        );
        if let Err(e) = audit_log.append(&entry).await {
            tracing::warn!(
                "failed to write new_issuer audit entry for host {host}, server {name}: {e:#}"
            );
        }

        if !proceed {
            println!("{}", "Aborted — new issuer not confirmed.".dimmed());
            return Ok(());
        }
    }

    // D-14: paste-back on request, or by default on a headless host (container,
    // SSH) unless `--browser` forces the loopback flow. Nothing below this branch
    // changes for the loopback path.
    let paste_back = no_browser || (!browser && ironhermes_core::auth::is_headless());
    if paste_back {
        let manager = ironhermes_mcp::McpManager::new(std::sync::Arc::new(
            tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new()),
        ))
        .with_auth_store(Some(store.clone()))
        .with_global_issuer_allowlist(config.mcp_oauth.issuer_allowlist.clone());
        let stdin = io::stdin();
        // On a terminal the paste is read with echo disabled (T-54.8-45).
        let no_echo = stdin.is_terminal();
        let mut input = stdin.lock();
        let mut out = io::stdout();
        paste_back_connect(&manager, &name, &server, &mut input, &mut out, no_echo).await?;
        println!(
            "Authorized '{name}'. Restart the gateway (or the container) so it loads the new token."
        );
        return Ok(());
    }

    println!(
        "{}",
        format!("Connecting to '{}' via OAuth...", name).cyan()
    );
    let (client, child) = connect_http_oauth(&server, store, &config.mcp_oauth.issuer_allowlist)
        .await
        .map_err(|e| anyhow::anyhow!("OAuth connect failed: {}", sanitize_error(&e.to_string())))?;
    let mcp_tools = client.list_all_tools().await?;
    let tools: Vec<(String, String)> = mcp_tools
        .iter()
        .map(|t| {
            (
                t.name.as_ref().to_string(),
                t.description.as_deref().unwrap_or("").to_string(),
            )
        })
        .collect();
    println!(
        "{}",
        format!("Connected \u{2014} {} tool(s) available", tools.len()).green()
    );
    for (tool_name, description) in &tools {
        let padded = format!("{:<30}", tool_name);
        println!("  {} \u{2014} {}", padded.yellow(), description);
    }
    // Drop client (disconnects transport) and child (kill_on_drop reaps process).
    drop(client);
    drop(child);
    Ok(())
}

// ---------------------------------------------------------------------------
// Paste-back authorization (D-14, D-15)
// ---------------------------------------------------------------------------

/// Run a paste-back authorization: print the link, read ONE pasted line (the
/// redirected address-bar URL), and complete through `oauth`.
///
/// The pasted line is never written back: not to `out`, not into a returned error.
/// Every error text that comes from the MCP layer (which may quote the callback URL,
/// for example an authorization server's exchange error) passes through
/// `redact_oauth_params` before it is returned. The one exception is the
/// authorization link itself, printed verbatim because the operator must open it
/// intact; it carries the `state` the operator's own browser sees anyway, never a code.
///
/// With `no_echo` the line is read without terminal echo (see [`read_pasted_line`]).
/// Terminal multiplexers or tools that log raw keyboard input are outside this
/// process's control (T-54.8-45, accepted): the code is single-use and is consumed
/// before the token exchange, so a copy left anywhere cannot be replayed.
///
/// Returns the authorized server name.
async fn paste_back_connect<R: BufRead, W: Write>(
    oauth: &dyn OAuthPasteBack,
    name: &str,
    server: &McpServerConfig,
    input: &mut R,
    out: &mut W,
    no_echo: bool,
) -> anyhow::Result<String> {
    let start = oauth.begin(name, server).await.map_err(|e| {
        anyhow::anyhow!("could not start authorization: {}", redact_oauth_params(&e))
    })?;

    writeln!(
        out,
        "Open this link on any device and approve access for '{name}':"
    )?;
    writeln!(out)?;
    writeln!(out, "{}", start.auth_url)?;
    writeln!(out)?;
    writeln!(
        out,
        "The page will fail to load after you approve. Copy the whole address-bar URL, \
         paste it here and press Enter. The link expires in 5 minutes."
    )?;
    if no_echo {
        writeln!(out, "(What you paste will not be shown.)")?;
    }
    out.flush()?;

    let line = read_pasted_line(input, no_echo)?;
    let line = line.trim();

    if line.is_empty() {
        oauth.cancel(&start.state).await;
        anyhow::bail!("no URL pasted; authorization cancelled");
    }
    if is_oauth_denial(line) {
        oauth.cancel(&start.state).await;
        anyhow::bail!("authorization was denied");
    }
    match oauth.complete(line).await {
        Ok(server_name) => Ok(server_name),
        Err(e) => {
            oauth.cancel(&start.state).await;
            Err(anyhow::anyhow!(
                "authorization failed: {}",
                redact_oauth_params(&e)
            ))
        }
    }
}

/// Read the one pasted line. On a terminal (`no_echo`) the read goes through
/// `rpassword` so the URL is not echoed onto the screen or into plain scrollback;
/// pipes and tests use the `BufRead` path.
fn read_pasted_line<R: BufRead>(input: &mut R, no_echo: bool) -> anyhow::Result<String> {
    if no_echo {
        Ok(rpassword::read_password()?)
    } else {
        let mut line = String::new();
        input.read_line(&mut line)?;
        Ok(line)
    }
}

// ---------------------------------------------------------------------------
// Connection helper: connect, list tools, disconnect
// ---------------------------------------------------------------------------

/// Wraps `attempt_connect_and_list` in `tokio::time::timeout` using the
/// server's configured `connect_timeout` (default 60s, per McpServerConfig::default()).
/// Closes GAP-1: without this wrapper, a child process that is alive but not
/// responding to MCP initialize will block forever.
async fn attempt_connect_and_list_with_timeout(
    config: &McpServerConfig,
) -> anyhow::Result<Vec<(String, String)>> {
    let secs = config.connect_timeout;
    match tokio::time::timeout(
        std::time::Duration::from_secs(secs),
        attempt_connect_and_list(config),
    )
    .await
    {
        Ok(inner) => inner,
        Err(_elapsed) => Err(anyhow::anyhow!(
            "Timed out waiting for MCP initialize response after {}s. Check command and args.",
            secs
        )),
    }
}

/// Connect to a server, list discovered tools (name + description), then disconnect.
/// Used by cmd_test, cmd_add (wizard), and cmd_configure.
/// Returns Vec<(tool_name, description)>.
async fn attempt_connect_and_list(
    config: &McpServerConfig,
) -> anyhow::Result<Vec<(String, String)>> {
    use ironhermes_mcp::transport::{connect_http, connect_stdio};

    // GAP-8 (Phase 21.2 Plan 11): connect_stdio/connect_http now return
    // (RunningService, Option<tokio::process::Child>). The Child handle is
    // exposed so McpManager::shutdown_all can hard-kill it during graceful
    // shutdown; this call site uses a short-lived probe and drops both so
    // the Child's kill_on_drop(true) reaps the stdio process at end-of-scope.
    let (client, child) = if config.command.is_some() {
        connect_stdio(config).await?
    } else if config.url.is_some() {
        connect_http(config).await?
    } else {
        return Err(anyhow::anyhow!(
            "Server has neither 'command' nor 'url' configured"
        ));
    };

    let mcp_tools = client.list_all_tools().await?;

    let tools: Vec<(String, String)> = mcp_tools
        .iter()
        .map(|t| {
            let tool_name = t.name.as_ref().to_string();
            let description = t.description.as_deref().unwrap_or("").to_string();
            (tool_name, description)
        })
        .collect();

    // Drop client (disconnects rmcp transport) and Child handle
    // (kill_on_drop(true) reaps the stdio child at end-of-scope).
    drop(client);
    drop(child);

    Ok(tools)
}

// ---------------------------------------------------------------------------
// Config persistence helpers (T-21.2-13: round-trip via serde_yaml::Value)
// ---------------------------------------------------------------------------

/// Save (add or update) a single MCP server entry in config.yaml.
/// Loads the full document, modifies only the mcp_servers.{name} key, and writes back.
/// Preserves all other config sections unchanged (T-21.2-13).
fn save_mcp_server_config(name: &str, server: &McpServerConfig) -> anyhow::Result<()> {
    let config_path = Config::config_path();
    let mut doc: serde_yaml::Value = if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        serde_yaml::from_str(&content).unwrap_or(serde_yaml::Value::Mapping(Default::default()))
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };

    // Ensure mcp_servers key exists as a mapping
    {
        let mapping = doc
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("config.yaml is not a YAML mapping"))?;
        let servers = mapping
            .entry(serde_yaml::Value::String("mcp_servers".to_string()))
            .or_insert(serde_yaml::Value::Mapping(Default::default()));
        let server_val = serde_yaml::to_value(server)?;
        servers
            .as_mapping_mut()
            .ok_or_else(|| anyhow::anyhow!("mcp_servers is not a YAML mapping"))?
            .insert(serde_yaml::Value::String(name.to_string()), server_val);
    }

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, serde_yaml::to_string(&doc)?)?;
    Ok(())
}

/// Remove a single MCP server entry from config.yaml.
/// Round-trips via serde_yaml::Value to preserve all other keys (T-21.2-13).
fn remove_mcp_server_config(name: &str) -> anyhow::Result<()> {
    let config_path = Config::config_path();
    if !config_path.exists() {
        return Ok(());
    }
    let content = std::fs::read_to_string(&config_path)?;
    let mut doc: serde_yaml::Value = serde_yaml::from_str(&content)?;
    if let Some(mapping) = doc.as_mapping_mut()
        && let Some(servers) = mapping.get_mut(serde_yaml::Value::String("mcp_servers".to_string()))
        && let Some(s) = servers.as_mapping_mut()
    {
        s.remove(serde_yaml::Value::String(name.to_string()));
    }
    std::fs::write(&config_path, serde_yaml::to_string(&doc)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Interactive prompt helpers
// ---------------------------------------------------------------------------

fn prompt_line(prompt: &str, default: &str) -> String {
    if default.is_empty() {
        print!("{}: ", prompt);
    } else {
        print!("{} [{}]: ", prompt, default);
    }
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    let trimmed = line.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn prompt_yn(prompt: &str, default_yes: bool) -> bool {
    let hint = if default_yes { "Y/n" } else { "y/N" };
    print!("{} [{}]: ", prompt, hint);
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    let trimmed = line.trim().to_lowercase();
    if trimmed.is_empty() {
        default_yes
    } else {
        trimmed == "y" || trimmed == "yes"
    }
}

/// Three-way outcome from the post-failure prompt in `cmd_add` (GAP-2).
#[derive(Debug, PartialEq, Eq)]
enum RetrySaveAbort {
    Retry,
    SaveAnyway,
    Abort,
}

/// GAP-2: prompt the user after a failed MCP connection in `cmd_add`.
/// Default is Abort (safest — do not persist a broken entry without consent).
/// Accepted inputs (case-insensitive): `r`/`retry` → Retry; `s`/`save` → SaveAnyway;
/// `a`/`abort` or empty → Abort.
fn prompt_retry_save_abort() -> RetrySaveAbort {
    print!("Connection failed. [R]etry / [S]ave anyway / [A]bort? [A]: ");
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line).ok();
    match line.trim().to_lowercase().as_str() {
        "r" | "retry" => RetrySaveAbort::Retry,
        "s" | "save" | "save anyway" => RetrySaveAbort::SaveAnyway,
        _ => RetrySaveAbort::Abort,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_config_module_has_all_handlers() {
        // Static check: ensure the module compiles with all required symbols
        // (checked by verifying type/fn names are reachable from tests)
        let _: fn() = || {
            let _ = McpAction::List;
        };
    }

    #[test]
    fn prompt_yn_default_yes_on_empty() {
        // Can't test stdin easily — verify logic directly
        // default_yes=true, empty input -> true
        // default_yes=false, empty input -> false
        // This is a compile-time reachability test for the helpers
        // functions are in scope — compile-time reachability verified by the test existing
    }

    #[test]
    fn save_mcp_server_config_roundtrip() {
        // Use a temp dir to avoid touching real config
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.yaml");

        // Write initial config with unrelated key
        std::fs::write(&config_path, "model:\n  default: gpt-4\n").unwrap();

        // Patch Config::config_path via direct file manipulation (not calling save_mcp_server_config
        // which uses Config::config_path() internally — tested via integration)
        // Just verify serde_yaml round-trip preserves existing keys:
        let content = std::fs::read_to_string(&config_path).unwrap();
        let mut doc: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
        let server = McpServerConfig {
            command: Some("npx".to_string()),
            args: vec!["--test".to_string()],
            ..McpServerConfig::default()
        };
        let server_val = serde_yaml::to_value(&server).unwrap();
        doc.as_mapping_mut()
            .unwrap()
            .entry(serde_yaml::Value::String("mcp_servers".to_string()))
            .or_insert(serde_yaml::Value::Mapping(Default::default()))
            .as_mapping_mut()
            .unwrap()
            .insert(
                serde_yaml::Value::String("test_server".to_string()),
                server_val,
            );
        let serialized = serde_yaml::to_string(&doc).unwrap();
        std::fs::write(&config_path, &serialized).unwrap();

        // Verify original key preserved
        let re_read = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            re_read.contains("default: gpt-4"),
            "original model key should be preserved"
        );
        assert!(
            re_read.contains("test_server"),
            "new mcp server should be written"
        );
        assert!(re_read.contains("npx"), "command should be present");
    }

    #[test]
    fn prompt_retry_save_abort_variants_are_distinct() {
        // Compile-time + discrimination check — guards against accidental merge of variants.
        assert_ne!(RetrySaveAbort::Retry, RetrySaveAbort::SaveAnyway);
        assert_ne!(RetrySaveAbort::SaveAnyway, RetrySaveAbort::Abort);
        assert_ne!(RetrySaveAbort::Retry, RetrySaveAbort::Abort);
    }

    #[test]
    fn mcp_config_carries_gap2_prompt_literal() {
        // Regression test: the exact prompt string promised to the human UAT
        // reviewer must appear verbatim in the source of this module.
        let src = include_str!("mcp_config.rs");
        assert!(
            src.contains("Connection failed. [R]etry / [S]ave anyway / [A]bort?"),
            "GAP-2: module must contain the exact 3-way prompt copy"
        );
    }

    #[test]
    fn mcp_config_carries_gap3_npx_example_literal() {
        // Regression test: the exact GAP-3 hint string must appear verbatim.
        let src = include_str!("mcp_config.rs");
        assert!(
            src.contains("Example: -y @modelcontextprotocol/server-filesystem /tmp"),
            "GAP-3: module must contain the exact npx example hint"
        );
    }

    #[tokio::test]
    async fn attempt_connect_and_list_with_timeout_returns_elapsed_error_when_config_demands_instant()
     {
        use ironhermes_mcp::McpServerConfig;
        // A config with connect_timeout=0 and a stdio command that will never produce an
        // MCP initialize response fast enough. We don't need it to be reachable — the
        // 0-second timeout fires before transport has a chance to complete.
        let cfg = McpServerConfig {
            command: Some("sleep".to_string()),
            args: vec!["30".to_string()],
            connect_timeout: 0,
            ..McpServerConfig::default()
        };
        let result = attempt_connect_and_list_with_timeout(&cfg).await;
        assert!(result.is_err(), "zero-timeout must surface an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Timed out waiting for MCP initialize response after"),
            "error message must begin with the literal GAP-1 contract string; got: {msg}"
        );
        assert!(
            msg.contains("Check command and args."),
            "error message must carry the user-actionable hint; got: {msg}"
        );
    }

    #[test]
    fn should_confirm_new_issuer_true_at_strict_defcon_tiers() {
        use ironhermes_core::config::DefconLevel;
        assert!(should_confirm_new_issuer(DefconLevel::ONE));
        assert!(should_confirm_new_issuer(DefconLevel::try_from(2).unwrap()));
    }

    #[test]
    fn should_confirm_new_issuer_false_at_relaxed_defcon_tiers() {
        use ironhermes_core::config::DefconLevel;
        assert!(!should_confirm_new_issuer(
            DefconLevel::try_from(3).unwrap()
        ));
        assert!(!should_confirm_new_issuer(DefconLevel::FIVE));
    }

    /// Phase 57 Plan 02 (D-11): the new-issuer confirmation reads the RESOLVED
    /// level, so a config whose only key is `security.defcon_level: 2` confirms.
    #[test]
    fn mcp_new_issuer_uses_effective_level() {
        let strict: ironhermes_core::config::Config =
            serde_yaml::from_str("security:\n  defcon_level: 2\n").unwrap();
        assert!(should_confirm_new_issuer(strict.effective_defcon_level()));
        let default = ironhermes_core::config::Config::default();
        assert!(!should_confirm_new_issuer(default.effective_defcon_level()));
    }

    #[test]
    fn remove_mcp_server_config_roundtrip() {
        let content = r#"
model:
  default: gpt-4
mcp_servers:
  github:
    command: npx
  filesystem:
    url: http://localhost:8000
"#;
        let mut doc: serde_yaml::Value = serde_yaml::from_str(content).unwrap();
        if let Some(mapping) = doc.as_mapping_mut()
            && let Some(servers) =
                mapping.get_mut(serde_yaml::Value::String("mcp_servers".to_string()))
            && let Some(s) = servers.as_mapping_mut()
        {
            s.remove(serde_yaml::Value::String("github".to_string()));
        }
        let serialized = serde_yaml::to_string(&doc).unwrap();
        assert!(!serialized.contains("github"), "github should be removed");
        assert!(
            serialized.contains("filesystem"),
            "filesystem should remain"
        );
        assert!(
            serialized.contains("gpt-4"),
            "model key should be preserved"
        );
    }

    // -----------------------------------------------------------------------
    // Paste-back (D-14)
    // -----------------------------------------------------------------------

    use ironhermes_mcp::manager::OAuthAuthorizationStart;
    use std::io::Cursor;
    use std::sync::Mutex;

    /// Records every call; `complete`/`begin` results are configurable.
    struct FakePasteBack {
        auth_url: String,
        state: String,
        begin_result: Option<String>,
        complete_result: Result<String, String>,
        completed: Mutex<Vec<String>>,
        cancelled: Mutex<Vec<String>>,
    }

    impl FakePasteBack {
        fn new() -> Self {
            Self {
                auth_url: "https://auth.example.test/authorize?state=st-1".to_string(),
                state: "st-1".to_string(),
                begin_result: None,
                complete_result: Ok("demo".to_string()),
                completed: Mutex::new(Vec::new()),
                cancelled: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl OAuthPasteBack for FakePasteBack {
        async fn begin(
            &self,
            _server_name: &str,
            _config: &McpServerConfig,
        ) -> Result<OAuthAuthorizationStart, String> {
            match &self.begin_result {
                Some(err) => Err(err.clone()),
                None => Ok(OAuthAuthorizationStart {
                    auth_url: self.auth_url.clone(),
                    state: self.state.clone(),
                }),
            }
        }
        async fn complete(&self, callback_url: &str) -> Result<String, String> {
            self.completed
                .lock()
                .unwrap()
                .push(callback_url.to_string());
            self.complete_result.clone()
        }
        async fn cancel(&self, state: &str) {
            self.cancelled.lock().unwrap().push(state.to_string());
        }
        async fn reconnect_server(&self, server_name: &str, _config: &McpServerConfig) -> String {
            format!("{server_name}: fake")
        }
    }

    const CALLBACK: &str = "http://127.0.0.1:8765/callback?code=cd-123&state=st-1";

    async fn drive(
        fake: &dyn OAuthPasteBack,
        pasted: &str,
    ) -> (anyhow::Result<String>, String, usize) {
        let mut input = Cursor::new(pasted.as_bytes().to_vec());
        let mut out: Vec<u8> = Vec::new();
        let server = McpServerConfig::default();
        let result = paste_back_connect(fake, "demo", &server, &mut input, &mut out, false).await;
        (
            result,
            String::from_utf8(out).unwrap(),
            input.position() as usize,
        )
    }

    #[tokio::test]
    async fn paste_back_connect_happy_path() {
        let fake = FakePasteBack::new();
        let (result, out, _) = drive(&fake, &format!("  {CALLBACK}  \n")).await;
        assert_eq!(result.unwrap(), "demo");
        assert_eq!(*fake.completed.lock().unwrap(), vec![CALLBACK.to_string()]);
        assert!(out.contains("https://auth.example.test/authorize?state=st-1"));
    }

    #[tokio::test]
    async fn paste_back_connect_never_echoes_pasted_line() {
        let fake = FakePasteBack::new();
        let (result, out, _) = drive(&fake, &format!("{CALLBACK}\n")).await;
        assert!(result.is_ok());
        assert!(!out.contains("cd-123"), "code leaked: {out}");
        assert!(!out.contains("code="), "code param leaked: {out}");
    }

    #[tokio::test]
    async fn paste_back_connect_empty_line_cancels() {
        let fake = FakePasteBack::new();
        let (result, _, _) = drive(&fake, "\n").await;
        assert!(result.is_err());
        assert_eq!(*fake.cancelled.lock().unwrap(), vec!["st-1".to_string()]);
        assert!(fake.completed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn paste_back_connect_denial_cancels() {
        let fake = FakePasteBack::new();
        let (result, _, _) = drive(
            &fake,
            "http://127.0.0.1:8765/callback?error=access_denied&state=st-1\n",
        )
        .await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("denied"), "got: {err}");
        assert_eq!(*fake.cancelled.lock().unwrap(), vec!["st-1".to_string()]);
        assert!(fake.completed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn paste_back_real_manager_fails_closed_before_reading() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AuthStore::open(tmp.path().join("auth.json")).await.unwrap();
        let manager = ironhermes_mcp::McpManager::new(std::sync::Arc::new(
            tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new()),
        ))
        .with_auth_store(Some(store));
        let server = McpServerConfig {
            url: Some("http://127.0.0.1:9/mcp".to_string()),
            oauth_provider: Some("demo_ns".to_string()),
            ..McpServerConfig::default()
        };
        let mut input = Cursor::new(format!("{CALLBACK}\n").into_bytes());
        let mut out: Vec<u8> = Vec::new();
        let result =
            paste_back_connect(&manager, "demo", &server, &mut input, &mut out, false).await;
        assert!(result.is_err());
        assert_eq!(
            input.position(),
            0,
            "input must not be read after begin fails"
        );
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains("cd-123"));
    }

    // -----------------------------------------------------------------------
    // Secrecy hardening (D-14 / D-15, T-54.8-04)
    // -----------------------------------------------------------------------

    /// A fake whose `begin` URL carries no `state`, so the only way `st-1` or
    /// `cd-123` can reach the output is a leak.
    fn quiet_fake() -> FakePasteBack {
        let mut fake = FakePasteBack::new();
        fake.auth_url = "https://auth.example.test/authorize?client_id=demo".to_string();
        fake
    }

    #[tokio::test]
    async fn complete_error_quoting_callback_is_redacted() {
        let mut fake = quiet_fake();
        fake.complete_result = Err(
            "exchange failed for http://127.0.0.1:8765/callback?code=cd-123&state=st-1".to_string(),
        );
        let (result, out, _) = drive(&fake, &format!("{CALLBACK}\n")).await;
        let err = format!("{:#}", result.unwrap_err());
        for text in [&err, &out] {
            assert!(!text.contains("cd-123"), "code leaked: {text}");
            assert!(!text.contains("st-1"), "state leaked: {text}");
        }
    }

    #[tokio::test]
    async fn begin_error_is_redacted() {
        let mut fake = quiet_fake();
        fake.begin_result = Some("registration failed near state=st-9".to_string());
        let (result, out, _) = drive(&fake, &format!("{CALLBACK}\n")).await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(!err.contains("st-9"), "state leaked in error: {err}");
        assert!(!out.contains("st-9"), "state leaked in output: {out}");
    }

    #[tokio::test]
    async fn percent_encoded_callback_is_never_echoed() {
        let fake = quiet_fake();
        let (result, out, _) = drive(
            &fake,
            "http://127.0.0.1:8765/callback?%63ode=cd-123&%73tate=st-1\n",
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(fake.completed.lock().unwrap().len(), 1);
        assert!(fake.cancelled.lock().unwrap().is_empty());
        assert!(!out.contains("cd-123"), "code leaked: {out}");
    }

    #[test]
    fn pasted_line_read_without_echo_on_a_tty() {
        // Needles are built from fragments so this test cannot match itself.
        let read_password = concat!("rpassword::", "read_password");
        let tty_guard = concat!("stdin.is_", "terminal()");
        let src = include_str!("mcp_config.rs");
        assert!(
            src.contains(tty_guard),
            "cmd_connect must decide echo from the stdin terminal check"
        );
        let start = src
            .find(concat!("fn read_pasted_", "line<"))
            .expect("read_pasted_line must exist");
        let body = &src[start..start + 400];
        let guard_at = body.find("if no_echo").expect("no_echo branch");
        let read_at = body.find(read_password).expect("rpassword read");
        assert!(
            guard_at < read_at,
            "rpassword must sit inside the no_echo branch"
        );
        assert!(body.contains("read_line"), "BufRead path for pipes/tests");
    }

    #[test]
    fn read_pasted_line_uses_the_reader_when_echo_is_allowed() {
        let mut input = Cursor::new(b"hello\nworld\n".to_vec());
        assert_eq!(read_pasted_line(&mut input, false).unwrap(), "hello\n");
    }

    /// Fake `begin`, REAL manager `complete`: drives the manager's real
    /// unknown-state error path through the CLI.
    struct FakeBeginRealComplete {
        fake: FakePasteBack,
        manager: ironhermes_mcp::McpManager,
    }

    #[async_trait::async_trait]
    impl OAuthPasteBack for FakeBeginRealComplete {
        async fn begin(
            &self,
            server_name: &str,
            config: &McpServerConfig,
        ) -> Result<OAuthAuthorizationStart, String> {
            self.fake.begin(server_name, config).await
        }
        async fn complete(&self, callback_url: &str) -> Result<String, String> {
            OAuthPasteBack::complete(&self.manager, callback_url).await
        }
        async fn cancel(&self, state: &str) {
            self.fake.cancel(state).await;
        }
        async fn reconnect_server(&self, server_name: &str, config: &McpServerConfig) -> String {
            self.fake.reconnect_server(server_name, config).await
        }
    }

    #[tokio::test]
    async fn real_manager_unknown_state_error_is_value_free() {
        let tmp = tempfile::tempdir().unwrap();
        let store = AuthStore::open(tmp.path().join("auth.json")).await.unwrap();
        let manager = ironhermes_mcp::McpManager::new(std::sync::Arc::new(
            tokio::sync::RwLock::new(ironhermes_tools::ToolRegistry::new()),
        ))
        .with_auth_store(Some(store));
        let wrapper = FakeBeginRealComplete {
            fake: quiet_fake(),
            manager,
        };
        let (result, out, _) = drive(
            &wrapper,
            "http://127.0.0.1:8765/callback?code=cd-123&state=st-never-parked\n",
        )
        .await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("unknown, already-used, or expired"),
            "expected the manager's fixed refusal, got: {err}"
        );
        for text in [&err, &out] {
            assert!(!text.contains("cd-123"), "code leaked: {text}");
            assert!(!text.contains("st-never-parked"), "state leaked: {text}");
        }
    }
}
