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

// ---------------------------------------------------------------------------
// Phase 24 D-14: Profile discovery helpers
// ---------------------------------------------------------------------------

/// Phase 24 D-14: per-profile summary entry shown in `hermes status` Profile section.
/// T-24-03: ONLY metadata fields permitted — no config values, no secrets.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProfileSummary {
    pub name: String,
    /// True if this profile matches the current invocation's active profile.
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_pid: Option<u32>,
    pub gateway_live: bool,
    /// RFC3339 last-modified of the profile dir's config.yaml, if present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
    /// Phase 23 Learning Loop status: "enabled", "disabled", or "unknown".
    pub learning_loop: String,
}

/// Phase 24: returns the active profile slug for the current invocation,
/// or "default" if IRONHERMES_HOME does not point under ~/.ironhermes/profiles/.
///
/// Reverse-checks the resolved IRONHERMES_HOME path. The bare-`hermes` root
/// (`~/.ironhermes/`) returns the literal string "default" per D-15.
pub fn current_profile() -> String {
    let home = ironhermes_core::get_hermes_home();
    let components: Vec<_> = home.components().collect();
    for window in components.windows(2) {
        if let std::path::Component::Normal(name) = window[0] {
            if name == ironhermes_core::PROFILES_SUBDIR {
                if let std::path::Component::Normal(slug) = window[1] {
                    return slug.to_string_lossy().to_string();
                }
            }
        }
    }
    "default".to_string()
}

/// Phase 24 D-14: enumerate `<ironhermes_root>/profiles/*/` subdirs that contain
/// a config.yaml file. Returns one ProfileSummary per matching subdir, sorted
/// alphabetically by name. Marks `active = true` for the entry whose name
/// matches `active_profile` (canonical: pass `&current_profile()`).
///
/// Returns an empty Vec if the profiles dir does not exist (the bare-hermes
/// install case before any --profile has been used).
pub fn enumerate_profiles(
    ironhermes_root: &std::path::Path,
    active_profile: &str,
) -> Vec<ProfileSummary> {
    let profiles_dir = ironhermes_root.join(ironhermes_core::PROFILES_SUBDIR);
    if !profiles_dir.exists() {
        return Vec::new();
    }
    let read_dir = match std::fs::read_dir(&profiles_dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut entries: Vec<ProfileSummary> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let cfg_path = path.join("config.yaml");
        if !cfg_path.exists() {
            continue;
        }
        let name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let last_modified = std::fs::metadata(&cfg_path)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| {
                use std::time::UNIX_EPOCH;
                t.duration_since(UNIX_EPOCH).ok().map(|d| {
                    chrono::DateTime::<chrono::Utc>::from(std::time::UNIX_EPOCH + d).to_rfc3339()
                })
            });
        // Per-profile gateway.pid probe via Plan 02 helpers.
        let (gateway_pid, gateway_live) = match ironhermes_gateway::pid::read_gateway_pid(&path) {
            Ok(Some(rec)) => {
                let live = matches!(
                    ironhermes_gateway::pid::is_pid_alive(rec.pid),
                    ironhermes_gateway::pid::PidLiveness::Live
                        | ironhermes_gateway::pid::PidLiveness::LiveOtherUser
                );
                (Some(rec.pid), live)
            }
            _ => (None, false),
        };
        // Phase 23 Learning Loop banner re-use: parse config.yaml and
        // mirror the cmd_config_show logic. Best-effort — on parse error,
        // emit "unknown".
        let learning_loop =
            read_learning_loop_status(&cfg_path).unwrap_or_else(|| "unknown".to_string());
        entries.push(ProfileSummary {
            name: name.clone(),
            active: name == active_profile,
            gateway_pid,
            gateway_live,
            last_modified,
            learning_loop,
        });
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries
}

/// Best-effort: read config.yaml and return "enabled" / "disabled" matching
/// Phase 23 D-17's Learning Loop banner logic. Returns None on read/parse
/// failure (caller defaults to "unknown").
fn read_learning_loop_status(cfg_path: &std::path::Path) -> Option<String> {
    let raw = std::fs::read_to_string(cfg_path).ok()?;
    let parsed: serde_yaml::Value = serde_yaml::from_str(&raw).ok()?;
    let memory_enabled = parsed
        .get("memory")
        .and_then(|m| m.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let skill_gen = parsed
        .get("skills")
        .and_then(|s| s.get("generation_enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if memory_enabled && skill_gen {
        Some("enabled".to_string())
    } else {
        Some("disabled".to_string())
    }
}

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
    /// Phase 24 D-14: additive. Skip-if-None keeps v1 JSON schema non-breaking.
    /// Bare-hermes installs without any profiles/ dir get None → field absent in JSON.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub profiles: Option<Vec<ProfileSummary>>,
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
            profiles: None,
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
            profiles: None, // Phase 24 D-14: populated by run_status after collect()
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

    // Phase 24 D-14: enumerate profiles and populate the additive JSON field.
    let active = current_profile();
    // Determine the canonical ironhermes root from IRONHERMES_HOME:
    // - Bare hermes: IRONHERMES_HOME = ~/.ironhermes → use it directly
    // - Profile mode: IRONHERMES_HOME = ~/.ironhermes/profiles/<slug>
    //   → the root is two levels up (strip profiles/<slug>)
    // Using hermes_home respects IRONHERMES_HOME overrides (e.g. test tempdirs).
    let ironhermes_root = if active == "default" {
        hermes_home.clone()
    } else {
        // profiles/<slug> → parent() = profiles → parent() = root
        hermes_home
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| hermes_home.clone())
    };
    let profile_entries = enumerate_profiles(&ironhermes_root, &active);
    let profile_field = if profile_entries.is_empty() {
        None
    } else {
        Some(profile_entries.clone())
    };
    let mut snap = snap;
    snap.profiles = profile_field;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print_styled(&snap);
        // Phase 24 D-14: render Profile section after the main status output.
        if !profile_entries.is_empty() {
            use colored::Colorize;
            println!();
            println!("{}", "Profiles".bold().cyan());
            for p in &profile_entries {
                let marker = if p.active { "*" } else { " " };
                let pid_note = if p.gateway_live {
                    format!(" gateway pid {} (live)", p.gateway_pid.unwrap_or(0))
                } else if p.gateway_pid.is_some() {
                    format!(" gateway pid {} (stale)", p.gateway_pid.unwrap_or(0))
                } else {
                    String::new()
                };
                println!(
                    "  {} {} [Learning Loop: {}]{}",
                    marker, p.name, p.learning_loop, pid_note
                );
            }
        }
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
            "approvals bypassed; iteration budget + ctrl-c + fatal error remain.".red()
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
    let _ = writeln!(out, "{}", "Subagents + Processes + MCP".bold().underline());
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

fn resolve_fallback_chain(config: &ironhermes_core::Config, provider_name: &str) -> Vec<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn enumerate_profiles_empty_when_no_profiles_dir() {
        let dir = TempDir::new().unwrap();
        let entries = enumerate_profiles(dir.path(), "default");
        assert!(entries.is_empty());
    }

    #[test]
    fn enumerate_profiles_returns_alphabetical_entries() {
        let dir = TempDir::new().unwrap();
        let profiles = dir.path().join("profiles");
        std::fs::create_dir_all(profiles.join("work")).unwrap();
        std::fs::write(
            profiles.join("work/config.yaml"),
            "memory:\n  enabled: true\nskills:\n  generation_enabled: true\n",
        )
        .unwrap();
        std::fs::create_dir_all(profiles.join("personal")).unwrap();
        std::fs::write(
            profiles.join("personal/config.yaml"),
            "memory:\n  enabled: true\nskills:\n  generation_enabled: false\n",
        )
        .unwrap();
        let entries = enumerate_profiles(dir.path(), "work");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "personal");
        assert_eq!(entries[1].name, "work");
        assert!(entries[1].active);
        assert!(!entries[0].active);
        assert_eq!(entries[1].learning_loop, "enabled");
        assert_eq!(entries[0].learning_loop, "disabled");
    }

    #[test]
    fn enumerate_profiles_skips_subdirs_without_config_yaml() {
        let dir = TempDir::new().unwrap();
        let profiles = dir.path().join("profiles");
        std::fs::create_dir_all(profiles.join("empty")).unwrap();
        let entries = enumerate_profiles(dir.path(), "default");
        assert!(
            entries.is_empty(),
            "subdir without config.yaml must be skipped"
        );
    }

    #[test]
    fn current_profile_returns_default_for_bare_home() {
        let bare = std::path::PathBuf::from("/home/u/.ironhermes");
        let mut last: Option<&std::ffi::OsStr> = None;
        let comps: Vec<_> = bare.components().collect();
        for w in comps.windows(2) {
            if let std::path::Component::Normal(n) = w[0] {
                if n == "profiles" {
                    if let std::path::Component::Normal(slug) = w[1] {
                        last = Some(slug);
                    }
                }
            }
        }
        let result = last
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "default".to_string());
        assert_eq!(result, "default");
    }

    #[test]
    fn current_profile_extracts_slug_from_profile_path() {
        let p = std::path::PathBuf::from("/home/u/.ironhermes/profiles/work");
        let mut last: Option<&std::ffi::OsStr> = None;
        let comps: Vec<_> = p.components().collect();
        for w in comps.windows(2) {
            if let std::path::Component::Normal(n) = w[0] {
                if n == "profiles" {
                    if let std::path::Component::Normal(slug) = w[1] {
                        last = Some(slug);
                    }
                }
            }
        }
        let result = last
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "default".to_string());
        assert_eq!(result, "work");
    }

    #[test]
    fn status_report_profiles_field_absent_when_none() {
        let report = StatusReport::fixture();
        assert!(report.profiles.is_none());
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            !json.contains("\"profiles\""),
            "profiles field must be absent from JSON when None; got: {}",
            json
        );
    }

    #[test]
    fn status_report_profiles_field_present_when_some_empty_vec() {
        let mut report = StatusReport::fixture();
        report.profiles = Some(vec![]);
        let json = serde_json::to_string(&report).unwrap();
        assert!(
            json.contains("\"profiles\""),
            "profiles field must be present when Some([]); got: {}",
            json
        );
    }
}
