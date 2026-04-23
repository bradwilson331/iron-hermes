//! `hermes status [--all] [--deep] [--json]` subcommand (D-18..D-22).
//!
//! Phase 21.7 Plan 04 locked the v1 schema; Plan 09 wires it to a real
//! collector, a colored default text output, and the `Commands::Status`
//! CLI entry point.
//!
//! Architecture:
//! - `StatusArgs` — clap::Args exposed to the main binary.
//! - `StatusReport` — the v1 JSON schema (E-07 insta snapshot locks it).
//! - `run_status(args)` — the async entry point invoked by `Commands::Status`.
//! - `StatusReport::collect(config, home, args, probe)` — reads runtime
//!   state into the report. Called by `run_status`.
//! - `format_styled(&snap) -> String` — builds the colored default-text
//!   output. Unit-tested under insta (E-10).
//! - `print_styled(&snap)` — convenience wrapper that prints `format_styled`.
//! - `probe::DeepProbe` — Live vs. Noop dispatch, gated by `--deep`.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

pub mod probe;

#[derive(clap::Args, Debug, Clone)]
pub struct StatusArgs {
    /// Include all optional subsections (per-MCP-server detail, full fallback chain, etc.)
    #[arg(long)]
    pub all: bool,

    /// Run live probes (provider HEAD, FTS5 integrity_check, MCP tools/list).
    #[arg(long)]
    pub deep: bool,

    /// Emit machine-readable JSON (stable v1 schema).
    #[arg(long)]
    pub json: bool,
}

/// v1 stable schema — document in phase SUMMARY (D-20).
/// Breaking changes require a v2 + migration path.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct StatusReport {
    pub provider: ProviderStatus,
    pub memory: MemoryStatus,
    pub gateway: GatewayStatus,
    pub subagents: SubagentStatus,
    pub processes: ProcessesStatus,
    pub mcp: McpStatus,
    pub yolo: YoloStatus,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProviderStatus {
    pub name: String,
    pub model: String,
    pub api_mode: String,
    pub fallback_chain: Vec<String>,
    /// Populated only when `--deep` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryStatus {
    pub provider: String,
    pub memory_md: MemoryFile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_md: Option<MemoryFile>,
    pub sessions: usize,
    pub state_db_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_db_healthy: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct MemoryFile {
    pub entries: usize,
    pub chars_used: usize,
    pub chars_max: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct GatewayStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub platforms: Vec<String>,
    pub allowlist_count: usize,
    pub telegram_authed: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct SubagentStatus {
    pub active: usize,
    pub max: usize,
    pub budget: BudgetView,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct BudgetView {
    pub iterations_used: usize,
    pub iterations_max: usize,
    /// One of: "none" | "caution70" | "warning90" | "stop100" (D-15).
    pub pressure: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProcessesStatus {
    pub tracked: usize,
    pub entries: Vec<ProcessEntry>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProcessEntry {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub task_id: String,
    pub command: String,
    pub uptime_secs: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpStatus {
    pub servers: usize,
    pub tools_total: usize,
    pub connected: usize,
    /// Populated only with `--all` or `--deep`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub per_server: Option<Vec<McpServer>>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct McpServer {
    pub name: String,
    pub connected: bool,
    pub tool_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reachable: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct YoloStatus {
    pub enabled: bool,
    /// One of: "flag" | "config" | "disabled" (D-12).
    pub source: String,
}

impl StatusReport {
    /// Deterministic fixture used by insta snapshots (E-07).
    /// Every field has a stable, non-time-sensitive value.
    pub fn fixture() -> Self {
        Self {
            provider: ProviderStatus {
                name: "anthropic".into(),
                model: "claude-sonnet-4".into(),
                api_mode: "messages".into(),
                fallback_chain: vec!["anthropic".into(), "openai".into()],
                healthy: None,
            },
            memory: MemoryStatus {
                provider: "file".into(),
                memory_md: MemoryFile {
                    entries: 12,
                    chars_used: 4210,
                    chars_max: 20_000,
                },
                user_md: Some(MemoryFile {
                    entries: 3,
                    chars_used: 512,
                    chars_max: 8_000,
                }),
                sessions: 47,
                state_db_bytes: 1_048_576,
                state_db_healthy: None,
            },
            gateway: GatewayStatus {
                running: true,
                pid: Some(12_345),
                platforms: vec!["telegram".into()],
                allowlist_count: 3,
                telegram_authed: true,
            },
            subagents: SubagentStatus {
                active: 1,
                max: 4,
                budget: BudgetView {
                    iterations_used: 23,
                    iterations_max: 50,
                    pressure: "caution70".into(),
                },
            },
            processes: ProcessesStatus {
                tracked: 2,
                entries: vec![ProcessEntry {
                    id: "proc_a1b2c3d4e5f6".into(),
                    pid: Some(67_890),
                    task_id: "sess-abc".into(),
                    command: "cargo watch -x test".into(),
                    uptime_secs: 42,
                    exit_code: None,
                }],
            },
            mcp: McpStatus {
                servers: 3,
                tools_total: 18,
                connected: 3,
                per_server: None,
            },
            yolo: YoloStatus {
                enabled: false,
                source: "disabled".into(),
            },
        }
    }

    /// Phase 21.7 Plan 09 Task 9-02: collect a live status snapshot.
    ///
    /// Reads config-derived values directly (provider/model/gateway shape,
    /// mcp servers, autonomous.yolo). Reads on-disk memory files + sessions
    /// dir for memory stats. Subagents/processes report zeros because
    /// `hermes status` is a one-shot CLI snapshot — it does not inspect a
    /// live agent's in-memory registries (that is a REPL slash-command
    /// surface wired in Plan 08).
    pub async fn collect(
        config: &ironhermes_core::Config,
        hermes_home: &std::path::Path,
        args: &StatusArgs,
        probe: &dyn probe::DeepProbe,
    ) -> anyhow::Result<Self> {
        let (yolo_enabled, yolo_source) = resolve_yolo_from_config(config.autonomous.yolo);

        // -- Provider (D-18 section a) --------------------------------------
        let provider_name = config.model.provider.clone();
        let provider_healthy = if args.deep {
            Some(
                probe
                    .provider_health(&provider_name, std::time::Duration::from_secs(3))
                    .await
                    .unwrap_or(false),
            )
        } else {
            None
        };
        let provider = ProviderStatus {
            name: provider_name.clone(),
            model: config.model.default.clone(),
            api_mode: resolve_api_mode(config, &provider_name),
            fallback_chain: resolve_fallback_chain(config, &provider_name),
            healthy: provider_healthy,
        };

        // -- Memory (D-18 section b) ----------------------------------------
        let state_db_path = hermes_home.join("state.db");
        let state_db_bytes = std::fs::metadata(&state_db_path)
            .map(|m| m.len())
            .unwrap_or(0);
        let state_db_healthy = if args.deep {
            Some(
                probe
                    .fts5_integrity(&state_db_path, std::time::Duration::from_secs(3))
                    .await
                    .unwrap_or(false),
            )
        } else {
            None
        };
        let memory = MemoryStatus {
            provider: config.memory.provider.clone(),
            memory_md: read_memory_file_stats(
                &hermes_home.join("memories").join("MEMORY.md"),
                20_000,
            ),
            user_md: if config.memory.user_profile_enabled {
                Some(read_memory_file_stats(
                    &hermes_home.join("memories").join("USER.md"),
                    8_000,
                ))
            } else {
                None
            },
            sessions: count_sessions(&hermes_home.join("sessions")),
            state_db_bytes,
            state_db_healthy,
        };

        // -- Gateway (D-18 section c) ---------------------------------------
        let gateway = GatewayStatus {
            running: false, // No cross-process gateway-liveness probe in 21.7.
            pid: None,
            platforms: enabled_platforms(config),
            allowlist_count: total_allowlist_count(config),
            telegram_authed: telegram_authed(config),
        };

        // -- Subagents / Processes / MCP (D-18 section d) -------------------
        let subagents = SubagentStatus {
            active: 0,
            max: config.subagent.max_subagents,
            budget: BudgetView {
                iterations_used: 0,
                iterations_max: config.agent.max_iterations,
                pressure: "none".into(),
            },
        };
        let processes = ProcessesStatus {
            tracked: 0,
            entries: vec![],
        };

        // MCP — read configured servers; under --all/--deep populate per-server.
        let server_names: Vec<String> = {
            let mut names: Vec<String> = config.mcp_servers.keys().cloned().collect();
            names.sort();
            names
        };
        let per_server = if args.all || args.deep {
            let mut v = Vec::new();
            for name in &server_names {
                let reachable = if args.deep {
                    Some(
                        probe
                            .mcp_server(name, std::time::Duration::from_secs(3))
                            .await
                            .map(|r| r.reachable)
                            .unwrap_or(false),
                    )
                } else {
                    None
                };
                v.push(McpServer {
                    name: name.clone(),
                    connected: false,
                    tool_count: 0,
                    reachable,
                });
            }
            Some(v)
        } else {
            None
        };
        let mcp = McpStatus {
            servers: server_names.len(),
            tools_total: 0,
            connected: 0,
            per_server,
        };

        Ok(StatusReport {
            provider,
            memory,
            gateway,
            subagents,
            processes,
            mcp,
            yolo: YoloStatus {
                enabled: yolo_enabled,
                source: yolo_source.into(),
            },
        })
    }
}

/// Plan 09 entry point — invoked from `main::Commands::Status`.
pub async fn run_status(args: StatusArgs) -> anyhow::Result<()> {
    let config = ironhermes_core::Config::load().unwrap_or_default();
    let hermes_home = ironhermes_core::get_hermes_home();

    // Dispatch: Noop by default, Live under `--deep`. LiveDeepProbe is
    // `#[cfg(not(test))]`-gated so tests driving `run_status` indirectly
    // would fail to compile — but no test does that; tests drive
    // `StatusReport::collect` directly with `MockDeepProbe`.
    let noop_probe = probe::NoopDeepProbe;
    #[cfg(not(test))]
    let live_probe = if args.deep {
        Some(probe::LiveDeepProbe::new())
    } else {
        None
    };

    let snap = if args.deep {
        #[cfg(not(test))]
        {
            let probe_ref: &dyn probe::DeepProbe = live_probe
                .as_ref()
                .map(|p| p as &dyn probe::DeepProbe)
                .unwrap_or(&noop_probe);
            StatusReport::collect(&config, &hermes_home, &args, probe_ref).await?
        }
        #[cfg(test)]
        {
            StatusReport::collect(&config, &hermes_home, &args, &noop_probe).await?
        }
    } else {
        StatusReport::collect(&config, &hermes_home, &args, &noop_probe).await?
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print_styled(&snap);
    }
    Ok(())
}

/// Build the colored default-text output. Pure — no I/O — so the E-10
/// insta snapshot test can lock the byte shape.
pub fn format_styled(s: &StatusReport) -> String {
    use colored::Colorize;
    use std::fmt::Write;
    let mut out = String::new();

    let _ = writeln!(out, "{}", "=== IronHermes Status ===".bold());
    let _ = writeln!(out);

    // Provider ----------------------------------------------------------
    let _ = writeln!(out, "{}", "Provider".bold().underline());
    let _ = writeln!(out, "  name:       {}", s.provider.name.cyan());
    let _ = writeln!(out, "  model:      {}", s.provider.model.cyan());
    let _ = writeln!(out, "  api_mode:   {}", s.provider.api_mode.cyan());
    if !s.provider.fallback_chain.is_empty() {
        let _ = writeln!(
            out,
            "  fallback:   {}",
            s.provider.fallback_chain.join(" -> ")
        );
    }
    if let Some(h) = s.provider.healthy {
        let status = if h { "yes".green() } else { "no".red() };
        let _ = writeln!(out, "  healthy:    {}", status);
    }
    if s.yolo.enabled {
        let _ = writeln!(
            out,
            "  {} {}",
            "--yolo enabled:".bold().red(),
            "approvals bypassed; iteration budget + ctrl-c + fatal error remain."
                .red()
        );
    }
    let _ = writeln!(out);

    // Memory -----------------------------------------------------------
    let _ = writeln!(out, "{}", "Memory".bold().underline());
    let _ = writeln!(out, "  provider:   {}", s.memory.provider.cyan());
    let _ = writeln!(
        out,
        "  MEMORY.md:  {} entries, {} / {} chars",
        s.memory.memory_md.entries,
        s.memory.memory_md.chars_used,
        s.memory.memory_md.chars_max.to_string().dimmed()
    );
    if let Some(ref user) = s.memory.user_md {
        let _ = writeln!(
            out,
            "  USER.md:    {} entries, {} / {} chars",
            user.entries,
            user.chars_used,
            user.chars_max.to_string().dimmed()
        );
    } else {
        let _ = writeln!(out, "  USER.md:    {}", "disabled".dimmed());
    }
    let _ = writeln!(out, "  sessions:   {}", s.memory.sessions);
    let _ = writeln!(out, "  state.db:   {} bytes", s.memory.state_db_bytes);
    if let Some(h) = s.memory.state_db_healthy {
        let status = if h { "ok".green() } else { "bad".red() };
        let _ = writeln!(out, "  fts5:       {}", status);
    }
    let _ = writeln!(out);

    // Gateway ----------------------------------------------------------
    let _ = writeln!(out, "{}", "Gateway".bold().underline());
    let _ = writeln!(
        out,
        "  running:    {}",
        if s.gateway.running {
            "yes".green()
        } else {
            "no".red()
        }
    );
    let _ = writeln!(out, "  platforms:  {:?}", s.gateway.platforms);
    let _ = writeln!(out, "  allowlist:  {} entries", s.gateway.allowlist_count);
    let _ = writeln!(
        out,
        "  tg_authed:  {}",
        if s.gateway.telegram_authed {
            "yes".green()
        } else {
            "no".dimmed()
        }
    );
    let _ = writeln!(out);

    // Subagents + Processes + MCP --------------------------------------
    let _ = writeln!(
        out,
        "{}",
        "Subagents + Processes + MCP".bold().underline()
    );
    let _ = writeln!(
        out,
        "  subagents:  {} / {} active",
        s.subagents.active, s.subagents.max
    );
    let pressure = match s.subagents.budget.pressure.as_str() {
        "none" => "none".dimmed().to_string(),
        "caution70" => "caution".yellow().to_string(),
        "warning90" => "warning".red().to_string(),
        "stop100" => "stop".red().bold().to_string(),
        o => o.to_string(),
    };
    let _ = writeln!(
        out,
        "  iterations: {} / {} ({})",
        s.subagents.budget.iterations_used, s.subagents.budget.iterations_max, pressure
    );
    let _ = writeln!(out, "  processes:  {} tracked", s.processes.tracked);
    let _ = writeln!(
        out,
        "  mcp:        {} server(s), {} tools",
        s.mcp.servers, s.mcp.tools_total
    );
    if let Some(ref per) = s.mcp.per_server {
        for server in per {
            let reach = match server.reachable {
                Some(true) => "reachable".green().to_string(),
                Some(false) => "unreachable".red().to_string(),
                None => "-".dimmed().to_string(),
            };
            let _ = writeln!(
                out,
                "    - {:<20} {} tool(s)   {}",
                server.name, server.tool_count, reach
            );
        }
    }

    out
}

pub fn print_styled(s: &StatusReport) {
    print!("{}", format_styled(s));
}

// ---------------------------------------------------------------------
// Helpers — config-derived and filesystem-derived.
// ---------------------------------------------------------------------

fn resolve_yolo_from_config(config_yolo: bool) -> (bool, &'static str) {
    // `hermes status` is a CLI snapshot — there is no per-invocation
    // --yolo flag on the Status subcommand. Only the config contributes.
    if config_yolo {
        (true, "config")
    } else {
        (false, "disabled")
    }
}

fn resolve_api_mode(config: &ironhermes_core::Config, provider_name: &str) -> String {
    // Prefer provider-specific override; fall back to a sensible label per
    // provider name. The ApiMode enum serializes in snake_case.
    if let Some(pc) = config.providers.get(provider_name) {
        if let Some(ref mode) = pc.api_mode {
            return match mode {
                ironhermes_core::ApiMode::ChatCompletions => "chat_completions".into(),
                ironhermes_core::ApiMode::AnthropicMessages => "anthropic_messages".into(),
                ironhermes_core::ApiMode::CodexResponses => "codex_responses".into(),
            };
        }
    }
    match provider_name {
        "anthropic" => "anthropic_messages".into(),
        "openai" | "openrouter" => "chat_completions".into(),
        _ => "unknown".into(),
    }
}

fn resolve_fallback_chain(
    config: &ironhermes_core::Config,
    provider_name: &str,
) -> Vec<String> {
    if let Some(pc) = config.providers.get(provider_name) {
        if !pc.fallback_providers.is_empty() {
            let mut chain = vec![provider_name.to_string()];
            chain.extend(pc.fallback_providers.clone());
            return chain;
        }
    }
    vec![provider_name.to_string()]
}

fn read_memory_file_stats(path: &std::path::Path, chars_max: usize) -> MemoryFile {
    let body = std::fs::read_to_string(path).unwrap_or_default();
    let entries = body
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("- ") || t.starts_with("* ")
        })
        .count();
    MemoryFile {
        entries,
        chars_used: body.chars().count(),
        chars_max,
    }
}

fn count_sessions(sessions_dir: &std::path::Path) -> usize {
    std::fs::read_dir(sessions_dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_type()
                        .map(|t| t.is_file() || t.is_dir())
                        .unwrap_or(false)
                })
                .count()
        })
        .unwrap_or(0)
}

fn enabled_platforms(config: &ironhermes_core::Config) -> Vec<String> {
    let mut names: Vec<String> = config
        .gateway
        .platforms
        .iter()
        .filter_map(|(name, p)| if p.enabled { Some(name.clone()) } else { None })
        .collect();
    names.sort();
    names
}

fn total_allowlist_count(config: &ironhermes_core::Config) -> usize {
    config
        .gateway
        .platforms
        .values()
        .map(|p| p.whitelist.len())
        .sum()
}

fn telegram_authed(config: &ironhermes_core::Config) -> bool {
    config
        .gateway
        .platforms
        .get("telegram")
        .map(|p| p.token.is_some() || p.api_key.is_some())
        .unwrap_or(false)
}
