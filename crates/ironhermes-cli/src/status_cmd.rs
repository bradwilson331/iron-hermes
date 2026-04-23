//! `hermes status [--all] [--deep] [--json]` subcommand (D-18..D-22).
//! Wave-1 SKELETON: schema locked + DeepProbe trait seam.
//! Plan 09 (Wave 3) wires this into the `Commands::Status` enum and fills
//! out the real data collectors.

#![allow(dead_code)] // Plan 09 consumes these.

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
}

/// Plan 09 entry point — stub body that Plan 09 replaces with real collectors.
pub async fn run_status(_args: StatusArgs) -> anyhow::Result<()> {
    // Wave-1 stub: compile and return Err so `Commands::Status` wiring (Plan 09)
    // doesn't trip over an unresolved symbol. Real body lands in Plan 09 T-09-02.
    anyhow::bail!("run_status not yet implemented — fills in Plan 09 (Wave 3)")
}

pub fn print_styled(_snap: &StatusReport) {
    // Wave-1 stub. Plan 09 fills with `colored` (matching memory_cmd.rs:22-75 style).
}
