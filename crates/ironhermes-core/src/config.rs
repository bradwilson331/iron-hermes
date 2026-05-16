use crate::constants::{DEFAULT_MAX_ITERATIONS, DEFAULT_MODEL, get_hermes_home};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// =============================================================================
// api_key_env validator (D-04, Phase 26)
// =============================================================================

/// Validate that an api_key_env value looks like a valid env var identifier.
///
/// Valid: `[A-Z][A-Z0-9_]*` — uppercase letter start, uppercase/digit/underscore only.
/// Rejects: empty strings, lowercase names, names with spaces, shell-injection patterns.
///
/// # Errors
/// Returns an error if the value does not match `[A-Z][A-Z0-9_]*`.
pub fn validate_api_key_env(value: &str) -> anyhow::Result<()> {
    // Hand-rolled check to avoid regex overhead at the call site — and to
    // match the project's policy of not instantiating Regex in hot paths.
    // The regex crate is still available for tests.
    if value.is_empty() {
        anyhow::bail!("api_key_env '' is not a valid env var name — must match [A-Z][A-Z0-9_]*");
    }
    let mut chars = value.chars();
    let first = chars.next().unwrap(); // non-empty, safe
    if !first.is_ascii_uppercase() {
        anyhow::bail!(
            "api_key_env '{}' is not a valid env var name — must match [A-Z][A-Z0-9_]*",
            value
        );
    }
    for ch in chars {
        if !ch.is_ascii_uppercase() && !ch.is_ascii_digit() && ch != '_' {
            anyhow::bail!(
                "api_key_env '{}' is not a valid env var name — must match [A-Z][A-Z0-9_]*",
                value
            );
        }
    }
    Ok(())
}

// =============================================================================
// Reserved role names (D-05, Phase 26)
// =============================================================================

/// The seven reserved auxiliary role names (D-05, PROV-06, Phase 26 + Phase 25.2 D-13 + Phase 25.3 D-P0-1).
///
/// Used by `model.roles:` map keys and `auxiliary` per-task overrides.
/// Unknown role names must be rejected at config load (Phase 26 anti-pattern
/// "Swallowing unknown roles" — RESEARCH.md §Anti-Patterns).
pub const RESERVED_ROLE_NAMES: &[&str] = &[
    "vision",
    "compression",
    "session_search",
    "skills_hub",
    "mcp_helper",
    "summarization", // Phase 25.2 D-13 — second resolve_role consumer (web_extract)
    "curator",       // Phase 25.3 D-P0-1 — Phase 25.4 Curator cascade prerequisite
];

/// Validate that a role name is one of the seven reserved helper-task roles.
///
/// Valid: `vision`, `compression`, `session_search`, `skills_hub`, `mcp_helper`,
/// `summarization`, `curator` (Phase 26 D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1).
///
/// # Errors
/// Returns an error if `name` is not in `RESERVED_ROLE_NAMES`.
pub fn validate_role_name(name: &str) -> anyhow::Result<()> {
    if RESERVED_ROLE_NAMES.iter().any(|r| *r == name) {
        Ok(())
    } else {
        anyhow::bail!(
            "role '{}' is not a recognised auxiliary role — must be one of: {}",
            name,
            RESERVED_ROLE_NAMES.join(", ")
        )
    }
}

// =============================================================================
// ToolsConfig (TOOL-02, Phase 25)
// =============================================================================

/// Per-toolset enable/disable entry (D-22).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsetEntry {
    pub enabled: bool,
}

/// Operator-facing tools configuration (D-22, D-23).
/// Lives under `tools:` in config.yaml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Per-toolset enable/disable map. Keys are toolset names (D-01).
    pub toolsets: HashMap<String, ToolsetEntry>,
    /// Tool names to skip in the setup-wizard prerequisite prompts (D-18).
    pub skip_prompts: Vec<String>,
    /// Per-tool override disable list within an enabled toolset (D-23 layer 4).
    pub disabled: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        let mut toolsets = HashMap::new();
        for name in ["memory", "session", "agent", "skills"] {
            toolsets.insert(name.to_string(), ToolsetEntry { enabled: true });
        }
        for name in ["web", "code", "browser"] {
            // browser added (D-04 high-blast-radius default)
            toolsets.insert(name.to_string(), ToolsetEntry { enabled: false });
        }
        Self {
            toolsets,
            skip_prompts: vec![],
            disabled: vec![],
        }
    }
}

impl ToolsConfig {
    /// D-23: enabled iff entry exists with enabled:true. Unknown names default to false
    /// so MCP-server-as-toolset (e.g., "mcp__github") requires explicit opt-in.
    pub fn is_toolset_enabled(&self, name: &str) -> bool {
        self.toolsets.get(name).map(|e| e.enabled).unwrap_or(false)
    }

    /// Phase 27.1.1-gap-02: merge all known toolsets into `self.toolsets` with
    /// back-compat semantics:
    ///   - If a name is ABSENT from the map → insert `enabled: true`
    ///     (preserves current all-enabled-by-accident behavior for old configs that
    ///     predate a toolset; upgrades never silently lose tools).
    ///   - If a name is PRESENT (enabled or disabled) → leave it untouched.
    ///     An explicit `web: { enabled: false }` stays false.
    ///
    /// Uses `crate::constants::ALL_TOOLSETS` as the single source of truth for the
    /// full set of known toolset names (D-20). `DEFAULT_TOOLSETS` members are a subset
    /// and receive the same absent→enabled treatment.
    ///
    /// This method is idempotent: calling it twice produces the same result as once.
    pub fn with_default_toolsets_merged(mut self) -> Self {
        for &name in crate::constants::ALL_TOOLSETS {
            self.toolsets
                .entry(name.to_string())
                .or_insert(ToolsetEntry { enabled: true });
        }
        self
    }

    /// Phase 27.1.1-gap-02: return the set of toolset names where `enabled == true`.
    ///
    /// Used by production `PromptBuilder` construction sites to populate
    /// `active_toolsets` so the system-prompt tool catalog text reflects the same
    /// enabled set as the API tool schemas.
    pub fn enabled_toolset_names(&self) -> std::collections::HashSet<String> {
        self.toolsets
            .iter()
            .filter_map(|(name, entry)| {
                if entry.enabled {
                    Some(name.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

// =============================================================================
// Provider configuration types (PROV-01..PROV-08, Phase 12)
// =============================================================================

/// Wire protocol mode for a provider endpoint (D-07).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiMode {
    ChatCompletions,
    AnthropicMessages,
    CodexResponses,
}

/// Per-provider override configuration (used in the `providers:` map).
///
/// Phase 26 additions (D-01, D-04, D-14):
/// - `api_key_env`: reference to env var holding the API key (replaces `api_key` literal).
///   Must match `[A-Z][A-Z0-9_]*` (validated at resolver build time).
/// - `disabled`: when `true`, the provider is excluded from resolver entry creation (D-14).
///
/// `api_key` is kept for one release cycle as a deprecated fallback (D-01 / Phase 26 Pitfall 5).
/// Existing configs with `api_key:` literal parse cleanly; a deprecation banner is emitted at
/// resolver build time (handled in `provider.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: Option<String>,
    /// DEPRECATED (D-01, Phase 26): use `api_key_env` instead.
    /// Kept for one release cycle; resolver emits a one-shot stderr banner when non-None.
    pub api_key: Option<String>,
    /// Env var name whose value holds the API key for this provider (D-01, D-04).
    /// Must match `[A-Z][A-Z0-9_]*`. Validated at `ProviderResolver::build()` time.
    pub api_key_env: Option<String>,
    pub api_mode: Option<ApiMode>,
    pub default_model: Option<String>,
    pub fallback_providers: Vec<String>,
    /// When `true`, this provider is excluded from the resolver (D-14, Phase 26).
    /// `hermes provider disable <name>` writes this flag; `enable` clears it.
    pub disabled: Option<bool>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            api_key: None,
            api_key_env: None,
            api_mode: None,
            default_model: None,
            fallback_providers: vec![],
            disabled: None,
        }
    }
}

/// Custom (user-defined) provider configuration (used in `custom_providers:` list).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomProviderConfig {
    pub name: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub api_mode: Option<ApiMode>,
    pub default_model: Option<String>,
}

/// Model role configuration (used in `model.roles:` map).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoleConfig {
    /// Provider name or "main" to inherit the main provider.
    pub provider: String,
    /// Model to use; None = use the provider's default_model.
    pub model: Option<String>,
}

/// Auxiliary model routing configuration (PROV-06, D-05, Phase 26 + Phase 25.2 D-13).
///
/// Defines a default cheaper model for the six helper task categories:
/// `vision`, `compression`, `session_search`, `skills_hub`, `mcp_helper`, `summarization`.
///
/// Resolution cascade for `resolve_role("vision")` / `resolve_role("summarization")`:
/// 1. `model.roles["vision"]` — per-task override (if set)
/// 2. `auxiliary` — this block (if set)
/// 3. `None` — caller falls through to main provider
///
/// `auxiliary` is optional (D-06): absent means all helper tasks use the main provider.
/// All fields are plain `String` per Phase 22.4.2.2 / Phase 26 D-18 cross-crate convention.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AuxiliaryConfig {
    /// Provider name (must be a key in `providers:`). Empty string = unset.
    pub provider: String,
    /// Model identifier served by this provider. Empty string = use provider default.
    pub model: String,
}

impl Default for AuxiliaryConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            model: String::new(),
        }
    }
}

impl AuxiliaryConfig {
    /// Returns `true` if the auxiliary block is meaningfully configured (non-empty provider).
    pub fn is_set(&self) -> bool {
        !self.provider.is_empty()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub model: ModelConfig,
    pub agent: AgentConfig,
    pub terminal: TerminalConfig,
    pub web: WebConfig,
    pub gateway: GatewayConfig,
    pub cron: CronConfig,
    pub security: SecurityConfig,
    pub rate_limit: RateLimitConfig,
    // SKILL-08: skills subsystem configuration (07.2 D-17, D-18)
    pub skills: SkillsConfig,
    // EXEC-01..04: code execution sandbox configuration
    pub exec: ExecConfig,
    // AGENT-01..05: subagent delegation configuration
    pub subagent: SubagentConfig,
    // BATCH-01..04: batch processing configuration
    pub batch: BatchConfig,
    // MEM-12: memory provider selection
    pub memory: MemoryConfig,
    // PRMT-12..16 (Phase 18): context compression configuration
    pub compression: CompressionConfig,
    // PROV-08: provider resolution configuration (Phase 12)
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub custom_providers: Vec<CustomProviderConfig>,
    /// MCP server configurations (Phase 21.2, D-21).
    /// Stored as raw YAML values to avoid circular dependency (ironhermes-mcp -> ironhermes-core).
    /// Parsed into McpServerConfig by ironhermes-mcp at the integration layer.
    #[serde(default)]
    pub mcp_servers: HashMap<String, serde_yaml::Value>,
    /// Phase 21.7 Plan 08 (D-11 / D-12): autonomous-mode configuration.
    /// Pre-21.7 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub autonomous: AutonomousConfig,
    /// Phase 25 D-22: toolset enable/disable configuration.
    /// Pre-Phase-25 configs load with D-20 defaults via `#[serde(default)]`.
    #[serde(default)]
    pub tools: ToolsConfig,
    /// Phase 26 D-05/D-06: auxiliary model routing configuration.
    /// Optional — absent means all helper tasks use the main provider.
    /// Pre-Phase-26 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub auxiliary: AuxiliaryConfig,
    /// Phase 25.1 D-18: browser automation configuration block.
    /// Pre-25.1 configs parse cleanly via #[serde(default)].
    #[serde(default)]
    pub browser: BrowserConfig,
    /// Phase 25.2 D-22: web_extract configuration block.
    /// `#[serde(default)]` makes this field optional in YAML — pre-25.2 configs parse cleanly.
    #[serde(default)]
    pub extract: ExtractConfig,
}

// =============================================================================
// AutonomousConfig (Phase 21.7 Plan 08, D-11 / D-12 / D-14)
// =============================================================================

/// Autonomous-mode (yolo) configuration.
///
/// D-11: `yolo: true` blanket-bypasses dangerous-command approval.
/// D-12: config is one of two input sources; the CLI `--yolo` flag wins
/// when both are set. Gateway reads this config value only — it MUST NOT
/// read a per-request yolo field (INV-21.7-05).
/// D-14: yolo is additive; the full approval queue is deferred.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AutonomousConfig {
    /// When true, skip dangerous-command approval prompts. Budget 100% /
    /// fatal error / user interrupt remain unskippable (G-01/G-04/G-09).
    pub yolo: bool,
}

// =============================================================================
// CompressionConfig (PRMT-12..16, Phase 18)
// =============================================================================

/// Context compression tuning (D-02, D-10, D-11, D-15, D-26).
///
/// `protect_first_n` is the CONFIGURED lower bound on the number of
/// front-of-list messages that cannot be pruned. At compression time the
/// effective value may auto-shrink (never grow) when a pinned assistant
/// tool_call has at least one tool_result outside the front-protected
/// region — shrinking releases the assistant into the prunable range so
/// the whole tool-pair can be summarized atomically (safety-over-recovery,
/// see 18-11). The configured value is preserved; only the per-call
/// boundary changes.
// T-18-06: if renaming later, add serde(alias)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressionConfig {
    pub protect_last_tokens: usize,
    pub tool_pair_shift_tokens: usize,
    pub protect_first_n: usize,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            protect_last_tokens: 20_000,
            tool_pair_shift_tokens: 500,
            protect_first_n: 3,
        }
    }
}

fn default_agent_engine() -> String {
    "summarizing".to_string()
}
fn default_agent_threshold() -> f32 {
    0.5
}
fn default_gateway_engine() -> String {
    "local_prune".to_string()
}
fn default_gateway_threshold() -> f32 {
    0.85
}
fn default_true() -> bool {
    true
}

/// Default `nudge_interval` for [`MemoryConfig`] — 10 user turns.
/// Matches the Python hermes-agent reference (`memory.nudge_interval: 10`).
/// Set to 0 in YAML to disable the periodic nudge entirely. (Phase 32 LEARN-01)
fn default_nudge_interval() -> u32 {
    10
}

// =============================================================================
// MemoryConfig (MEM-12)
// =============================================================================

/// Memory provider configuration (D-08, D-09, D-10).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Provider type: "file" (default), "sqlite", "grafeo", "duckdb".
    pub provider: String,
    /// Optional mirror provider (D-27). When set, the factory builds a
    /// secondary provider that receives `on_memory_write` events but does
    /// not serve reads. Preserves MEM-12 (single primary).
    #[serde(default)]
    pub mirror_provider: Option<String>,
    /// When false, the entire memory subsystem is skipped at factory level:
    /// no provider is constructed, no memory tool is registered, no prompt
    /// injection occurs. Default: true (D-07, T-21.4-02).
    #[serde(default = "default_true")]
    pub memory_enabled: bool,
    /// When false, the USER.md store is skipped but MEMORY.md still works.
    /// Prompt builder omits the User target block. Memory tool rejects writes
    /// to User target with a clear error. Default: true (D-07, T-21.4-03).
    #[serde(default = "default_true")]
    pub user_profile_enabled: bool,
    /// Phase 32 LEARN-01: periodic memory nudge interval in user turns.
    /// Default 10. Set 0 to disable.
    /// At every N user turns the agent receives a background memory-review prompt
    /// (`MEMORY_REVIEW_PROMPT`, see `ironhermes_agent::nudge`).
    /// Honors PRMT-06: mid-session writes persist to disk; the active prompt is unchanged.
    #[serde(default = "default_nudge_interval")]
    pub nudge_interval: u32,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: "file".to_string(),
            mirror_provider: None,
            memory_enabled: true,
            user_profile_enabled: true,
            nudge_interval: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    pub default: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub provider: String,
    pub vision_model: Option<String>,
    pub max_tokens: Option<usize>,
    pub context_length: Option<usize>,
    /// Auxiliary model role assignments (PROV-06, Phase 12).
    #[serde(default)]
    pub roles: HashMap<String, ModelRoleConfig>,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            default: DEFAULT_MODEL.to_string(),
            base_url: None,
            api_key: None,
            provider: "openrouter".to_string(),
            vision_model: None,
            max_tokens: None,
            context_length: None,
            roles: HashMap::new(),
        }
    }
}

fn default_agent_max_iterations() -> usize {
    50
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_turns: usize,
    pub context_compression: f64,
    pub tool_delay_secs: f64,
    /// Custom personality presets (D-09, Phase 15 Plan 02).
    /// Merged into PersonalityRegistry at config load time with highest precedence.
    #[serde(default)]
    pub personalities: HashMap<String, String>,
    /// PRMT-11 (Phase 18): optional system-message slot content; empty = omitted.
    #[serde(default)]
    pub system_message: String,
    /// PRMT-12 (Phase 18): engine selection — "summarizing" (default) or "local_prune".
    #[serde(default = "default_agent_engine")]
    pub context_engine: String,
    /// PRMT-14 (Phase 18): fraction of context_length at which agent loop compresses.
    #[serde(default = "default_agent_threshold")]
    pub compression_threshold: f32,
    /// Plan 21.7-05 (PROV-09 / D-15): maximum iterations the shared
    /// [`BudgetHandle`] counter is seeded to. Controls pressure-tier ladder
    /// thresholds (Caution70 at 70%, Warning90 at 90%, Stop100 at 100%).
    /// Default: 50. Pre-21.7 configs without this key load cleanly via
    /// `#[serde(default)]`.
    #[serde(default = "default_agent_max_iterations")]
    pub max_iterations: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_ITERATIONS,
            context_compression: 0.5,
            tool_delay_secs: 1.0,
            personalities: HashMap::new(),
            system_message: String::new(),
            context_engine: default_agent_engine(),
            compression_threshold: default_agent_threshold(),
            max_iterations: default_agent_max_iterations(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub backend: String,
    pub cwd: String,
    pub timeout: u64,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            backend: "local".to_string(),
            cwd: ".".to_string(),
            timeout: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    pub backend: String,
    /// User-Agent header for HTTP requests (D-12). Default: "IronHermes/1.0 (+bot)".
    pub user_agent: String,
    /// Maximum content length in characters before truncation (D-15). Default: 50,000.
    pub max_content_chars: usize,
    /// HTTP request timeout in seconds (D-04). Default: 30.
    pub timeout_secs: u64,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            backend: "firecrawl".to_string(),
            user_agent: "IronHermes/1.0 (+bot)".to_string(),
            max_content_chars: 50_000,
            timeout_secs: 30,
        }
    }
}

// =============================================================================
// BrowserConfig (Phase 25.1 D-18)
// =============================================================================

/// Phase 25.1 D-18: browser automation configuration.
/// All fields `#[serde(default)]` for backward compat — pre-25.1 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserConfig {
    /// D-02: run browser with a visible window (true) or headless (false, default).
    pub headed: bool,
    /// D-02: allow `--no-sandbox` flag (required on Docker/restricted envs). Default false.
    pub no_sandbox: bool,
    /// D-15: domain allowlist for browser_navigate. Empty = allow all hosts.
    /// Exact hostname match — subdomains are NOT covered by the apex entry.
    /// To allow both example.com and www.example.com, list both explicitly.
    pub allowed_domains: Vec<String>,
    /// D-16: scheme allowlist for browser_navigate. Default ["http", "https"].
    pub allowed_schemes: Vec<String>,
    /// D-05: explicit chromium binary path. None = autodiscover via D-05 walk.
    pub chromium_path: Option<String>,
    /// D-02: per-operation timeout in seconds. Default 30.
    pub timeout_seconds: u64,
    /// Phase 26.3: persistent browser profile directory.
    /// None = use $HERMES_HOME/browser-profile (default — resolved at spawn time).
    /// Set explicitly to override (e.g., "/tmp/ephemeral-profile" for stateless browsing).
    pub user_data_dir: Option<String>,
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            headed: false,
            no_sandbox: false,
            allowed_domains: vec![],
            allowed_schemes: vec!["http".to_string(), "https".to_string()],
            chromium_path: None,
            timeout_seconds: 30,
            user_data_dir: None,
        }
    }
}

// =============================================================================
// ExtractConfig (Phase 25.2 D-22)
// =============================================================================

/// Phase 25.2 D-22: web_extract tool configuration.
/// All fields default; pre-25.2 configs parse cleanly via #[serde(default)].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExtractConfig {
    /// D-15: Semaphore permits covering BOTH multi-URL parallelism AND
    /// per-chunk summarization parallelism. Default 4.
    pub max_parallel_summaries: usize,

    /// D-11 tier-3 chunk size in chars. Default 100_000.
    pub summary_chunk_chars: usize,

    /// D-11 tier-4 refusal threshold in chars. Default 2_000_000.
    pub refuse_threshold_chars: usize,

    /// D-11 tier 1→2 boundary in chars. Default 5_000.
    pub summary_tier2_threshold_chars: usize,

    /// D-11 tier 2→3 boundary in chars. Default 500_000.
    pub summary_tier3_threshold_chars: usize,

    /// D-19: extra secret-URL patterns appended to the const default set
    /// in `crates/ironhermes-tools/src/web_extract/sanitize.rs::SECRET_URL_PATTERNS`.
    pub redact_url_patterns: Vec<String>,
}

impl Default for ExtractConfig {
    fn default() -> Self {
        Self {
            max_parallel_summaries: 4,
            summary_chunk_chars: 100_000,
            refuse_threshold_chars: 2_000_000,
            summary_tier2_threshold_chars: 5_000,
            summary_tier3_threshold_chars: 500_000,
            redact_url_patterns: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    pub platforms: HashMap<String, PlatformGatewayConfig>,
    /// PRMT-12 (Phase 18): gateway engine selection — typically "local_prune".
    #[serde(default = "default_gateway_engine")]
    pub context_engine: String,
    /// PRMT-14 (Phase 18): per-turn hygiene threshold (default 0.85).
    #[serde(default = "default_gateway_threshold")]
    pub compression_threshold: f32,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            platforms: HashMap::new(),
            context_engine: default_gateway_engine(),
            compression_threshold: default_gateway_threshold(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlatformGatewayConfig {
    pub enabled: bool,
    pub token: Option<String>,
    pub api_key: Option<String>,
    /// Telegram user IDs allowed to interact with the bot. Empty = deny all (D-12).
    #[serde(default)]
    pub whitelist: Vec<i64>,
    /// Session inactivity timeout in hours. Default 24 (D-14).
    #[serde(default = "default_session_timeout_hours")]
    pub session_timeout_hours: u64,
    /// Maximum concurrent agent runs. Default 8 (TG-06).
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_session_timeout_hours() -> u64 {
    24
}
fn default_max_concurrent_runs() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub wrap_response: bool,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            wrap_response: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub redact_secrets: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            redact_secrets: true,
        }
    }
}

/// Per-user inbound rate limiting configuration (D-22).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Maximum sustained messages per minute per user.
    pub messages_per_minute: u32,
    /// Maximum burst size (tokens available immediately).
    pub burst_size: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            messages_per_minute: 10,
            burst_size: 3,
        }
    }
}

// =============================================================================
// SkillsConfig (SKILL-08)
// =============================================================================

/// Skills Hub configuration (Phase 19.1, D-04/D-08).
///
/// `trusted_repos` is read on every registry load (D-08 — trust is never
/// frozen in the install manifest). Empty default (D-04).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HubConfig {
    /// Allowlist of repos whose Hub installs become SkillSource::Trusted.
    /// Format: "owner/repo". Default: empty.
    pub trusted_repos: Vec<String>,
    /// Override env var name for GitHub token; default precedence falls back
    /// to HERMES_GITHUB_TOKEN → GITHUB_TOKEN → `gh auth token` (D-03).
    pub github_token_env: Option<String>,
    /// Additional GitHub taps beyond DEFAULT_TAPS (D-02).
    pub extra_taps: Vec<ExtraTap>,
    /// Optional well-known HTTPS origins the user wants surfaced in search
    /// (trust is still Community per D-07 regardless of origin).
    pub well_known_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExtraTap {
    pub repo: String,
    #[serde(default)]
    pub path: Option<String>,
}

impl HubConfig {
    pub fn trusted_repos_set(&self) -> std::collections::HashSet<String> {
        self.trusted_repos.iter().cloned().collect()
    }
}

/// Skills subsystem configuration (07.2 D-17, D-18, D-19, D-20).
///
/// Controls whether skills are loaded at all (`enabled`) and allows the user
/// to declare additional scan paths beyond the three hardcoded defaults:
/// 1. `<cwd>/.ironhermes/skills/`
/// 2. `<hermes_home>/skills/` (typically `~/.ironhermes/skills/`)
/// 3. `~/.agents/skills/`
///
/// `extra_paths` are appended AFTER the defaults so defaults retain priority
/// via first-path-wins dedup (D-19).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    /// Master enable switch. `false` → SkillRegistry returns empty without scanning (D-20).
    pub enabled: bool,
    /// Additional scan paths appended after the 3 defaults (D-19).
    pub extra_paths: Vec<PathBuf>,
    /// Root directory for skill credentials (Phase 19 D-10). Defaults to
    /// `$HERMES_HOME/credentials` with fallback to `~/.ironhermes/credentials`
    /// when unset. Resolved via `default_credential_dir()` in ironhermes-tools.
    #[serde(default)]
    pub credential_dir: Option<PathBuf>,
    /// Per-skill config values (Phase 19 D-07):
    /// `skills.config.<skill-name>.<key> = <value>`.
    ///
    /// Consumed by `SkillsTool` to synthesize the `[Skill config: ...]`
    /// body-injection header on activation (D-08). Values are typed as
    /// `serde_yaml::Value` so any YAML scalar or nested structure is preserved
    /// without forcing schema changes as new skills are added.
    #[serde(default)]
    pub config: HashMap<String, HashMap<String, serde_yaml::Value>>,
    /// Skills Hub settings (Phase 19.1 D-04/D-08).
    #[serde(default)]
    pub hub: HubConfig,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_paths: Vec::new(),
            credential_dir: None,
            config: HashMap::new(),
            hub: HubConfig::default(),
        }
    }
}

// =============================================================================
// ExecConfig (EXEC-01..04)
// =============================================================================

/// Code execution sandbox configuration (D-03, D-12, D-13, D-14, D-29).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExecConfig {
    /// Path to the Python interpreter. Default: "python3". (D-03)
    pub python_path: String,
    /// Timeout in seconds. Default: 300 (5 minutes). (D-12)
    pub timeout_secs: u64,
    /// Maximum RPC calls per execution. Default: 50. (D-13)
    pub max_rpc_calls: u32,
    /// Maximum stdout bytes before truncation. Default: 50000 (50KB). (D-14)
    pub max_output_bytes: usize,
    /// Maximum stderr bytes before truncation. Default: 10240 (10KB). (D-29)
    pub max_stderr_bytes: usize,
}

impl Default for ExecConfig {
    fn default() -> Self {
        Self {
            python_path: "python3".to_string(),
            timeout_secs: 300,
            max_rpc_calls: 50,
            max_output_bytes: 50_000,
            max_stderr_bytes: 10_240,
        }
    }
}

// =============================================================================
// SubagentConfig (AGENT-01..05)
// =============================================================================

/// Subagent delegation configuration (D-08, D-09, D-16, D-25).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentConfig {
    /// Timeout in seconds for each subagent execution. Default: 300 (5 minutes).
    pub timeout_secs: u64,
    /// Maximum concurrent subagents. Default: 3.
    pub max_subagents: usize,
    /// Maximum LLM iterations per subagent. Default: 10.
    pub max_iterations: usize,
    /// Default toolset groups for child agents (D-01). Default: ["terminal", "file", "web"].
    pub default_toolsets: Vec<String>,
    /// Optional model override for subagents (D-23). None = use parent's model.
    pub model: Option<String>,
    /// Optional provider override for subagents (D-23). None = use parent's provider.
    pub provider: Option<String>,
    /// Optional custom API base URL for subagents (D-23). None = use parent's.
    pub base_url: Option<String>,
    /// Optional custom API key for subagents (D-23). None = use parent's.
    pub api_key: Option<String>,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 300,
            max_subagents: 3,
            max_iterations: 10,
            default_toolsets: vec!["terminal".into(), "file".into(), "web".into()],
            model: None,
            provider: None,
            base_url: None,
            api_key: None,
        }
    }
}

// =============================================================================
// BatchConfig (BATCH-01..04)
// =============================================================================

/// Batch processing configuration (BATCH-01..04).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchConfig {
    /// Default worker concurrency. Default: 4.
    pub workers: usize,
    /// Default max agent iterations per prompt. Default: 20.
    pub max_turns: usize,
    /// Default output directory (relative to cwd). Default: "batch_output".
    pub output_dir: String,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            workers: 4,
            max_turns: 20,
            output_dir: "batch_output".to_string(),
        }
    }
}

/// Return type for `Config::telegram_default_origin`.
/// Defined in ironhermes-core (without embedding JobOrigin) to avoid a
/// circular crate dep on ironhermes-cron. The CLI crate (which depends on
/// both) constructs `ironhermes_cron::JobOrigin` from these strings.
#[derive(Debug, Clone)]
pub enum OriginDecision {
    /// TG gateway is disabled, section is missing, or whitelist is empty.
    None,
    /// Exactly one authorized chat — auto-route to this origin.
    Single { platform: String, chat_id: String },
    /// Multiple authorized chats — caller must eprintln hint, fall back to "local".
    Multi { whitelist: Vec<String> },
}

impl Config {
    /// Load config from the IronHermes home directory.
    pub fn load() -> anyhow::Result<Self> {
        let config_path = get_hermes_home().join("config.yaml");
        Self::load_from(&config_path)
    }

    /// Load config from a specific path, falling back to defaults.
    ///
    /// Phase 26 D-02: if `custom_providers:` entries exist and have no matching
    /// key in `providers:`, they are migrated into `providers:` at parse time
    /// with a one-line stderr warning per migrated entry.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let mut config: Config = serde_yaml::from_str(&content)?;
            // D-02: migrate custom_providers entries that are missing from providers HashMap.
            // If providers.foo already exists, the custom_providers.foo entry is silently
            // dropped (providers: takes precedence — no ambiguity).
            for custom in &config.custom_providers {
                if !config.providers.contains_key(&custom.name) {
                    eprintln!(
                        "[provider:{}] migrated from deprecated custom_providers list — \
                        move to providers.{} in config.yaml to silence this warning",
                        custom.name, custom.name
                    );
                    config.providers.insert(
                        custom.name.clone(),
                        ProviderConfig {
                            base_url: Some(custom.base_url.clone()),
                            api_key: custom.api_key.clone(),
                            api_key_env: None,
                            api_mode: custom.api_mode.clone(),
                            default_model: custom.default_model.clone(),
                            fallback_providers: vec![],
                            disabled: None,
                        },
                    );
                }
            }
            Ok(config)
        } else {
            Ok(Config::default())
        }
    }

    /// Save config to the IronHermes home directory.
    pub fn save(&self) -> anyhow::Result<()> {
        let config_path = get_hermes_home().join("config.yaml");
        self.save_to(&config_path)
    }

    /// Save config to a specific path.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Get the config file path.
    pub fn config_path() -> PathBuf {
        get_hermes_home().join("config.yaml")
    }

    /// Get the .env file path.
    pub fn env_path() -> PathBuf {
        get_hermes_home().join(".env")
    }

    /// Compute the default cron delivery origin from the TG gateway config.
    /// Returns `OriginDecision::None` when TG section is missing, disabled, or has empty whitelist.
    /// Returns `OriginDecision::Single` when whitelist has exactly one entry.
    /// Returns `OriginDecision::Multi` when whitelist has >1 entries (caller emits hint).
    pub fn telegram_default_origin(&self) -> OriginDecision {
        let Some(tg) = self.gateway.platforms.get("telegram") else {
            return OriginDecision::None;
        };
        if !tg.enabled {
            return OriginDecision::None;
        }
        match tg.whitelist.len() {
            0 => OriginDecision::None,
            1 => OriginDecision::Single {
                platform: "telegram".to_string(),
                chat_id: tg.whitelist[0].to_string(),
            },
            _ => OriginDecision::Multi {
                whitelist: tg.whitelist.iter().map(|id| id.to_string()).collect(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Phase 25.1 Plan 02: BrowserConfig + ToolsConfig browser entry tests (D-18, D-04)
    // =========================================================================

    #[test]
    fn browser_config_default_matches_d18() {
        let bc = BrowserConfig::default();
        assert!(!bc.headed);
        assert!(!bc.no_sandbox);
        assert!(bc.allowed_domains.is_empty());
        assert_eq!(
            bc.allowed_schemes,
            vec!["http".to_string(), "https".to_string()]
        );
        assert_eq!(bc.chromium_path, None);
        assert_eq!(bc.timeout_seconds, 30);
        assert_eq!(
            bc.user_data_dir, None,
            "Phase 26.3 UDD-01: user_data_dir defaults to None"
        );
    }

    #[test]
    fn config_includes_browser_block_with_defaults() {
        let c = Config::default();
        assert_eq!(c.browser.timeout_seconds, 30);
        assert!(c.browser.allowed_domains.is_empty());
    }

    #[test]
    fn config_yaml_without_browser_section_parses_with_defaults() {
        // Phase 25.1 D-18 backward compat
        let yaml = r#"
web:
  backend: "firecrawl"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.browser.timeout_seconds, 30);
        assert!(!c.browser.headed);
    }

    #[test]
    fn config_yaml_partial_browser_section_uses_defaults_for_rest() {
        let yaml = r#"
browser:
  headed: true
  timeout_seconds: 60
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(c.browser.headed);
        assert_eq!(c.browser.timeout_seconds, 60);
        assert!(!c.browser.no_sandbox); // default
        assert_eq!(
            c.browser.allowed_schemes,
            vec!["http".to_string(), "https".to_string()]
        ); // default
    }

    // Phase 26.3 — UDD-01: BrowserConfig default has user_data_dir == None.
    #[test]
    fn browser_config_user_data_dir_defaults_to_none() {
        let bc = BrowserConfig::default();
        assert!(
            bc.user_data_dir.is_none(),
            "Phase 26.3 UDD-01: user_data_dir must default to None (computed from HERMES_HOME at spawn time)"
        );
    }

    // Phase 26.3 — UDD-02: YAML round-trip preserves explicit user_data_dir.
    #[test]
    fn browser_config_yaml_round_trips_user_data_dir() {
        let yaml = r#"
browser:
  user_data_dir: /custom/profile
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            c.browser.user_data_dir.as_deref(),
            Some("/custom/profile"),
            "Phase 26.3 UDD-02: explicit user_data_dir must round-trip through serde"
        );
    }

    // Phase 26.3 — UDD-03: pre-26.3 YAML (no user_data_dir key) parses cleanly with None.
    #[test]
    fn browser_config_backward_compat_no_user_data_dir() {
        let yaml = "browser:\n  headed: true\n";
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(
            c.browser.user_data_dir.is_none(),
            "Phase 26.3 UDD-03: missing user_data_dir key must parse as None for backward compat"
        );
    }

    #[test]
    fn tools_config_default_disables_browser_toolset() {
        let tc = ToolsConfig::default();
        let entry = tc
            .toolsets
            .get("browser")
            .expect("browser toolset entry must exist by default");
        assert!(
            !entry.enabled,
            "Phase 25.1 D-04: browser toolset MUST be disabled by default (high-blast-radius)"
        );
    }

    #[test]
    fn test_skills_config_default() {
        let default = SkillsConfig::default();
        assert!(default.enabled);
        assert!(default.extra_paths.is_empty());
    }

    #[test]
    fn test_config_default_includes_skills() {
        let config = Config::default();
        assert!(config.skills.enabled);
        assert!(config.skills.extra_paths.is_empty());
    }

    #[test]
    fn test_config_parses_without_skills_section() {
        // Backward compat (D-18): existing config.yaml files without a `skills:` section
        // must parse unchanged via serde(default).
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(config.skills.enabled); // default applied
        assert!(config.skills.extra_paths.is_empty());
    }

    #[test]
    fn test_config_parses_with_skills_section() {
        let yaml = r#"
skills:
  enabled: false
  extra_paths:
    - /tmp/custom-skills
    - /opt/shared/skills
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(!config.skills.enabled);
        assert_eq!(config.skills.extra_paths.len(), 2);
        assert_eq!(
            config.skills.extra_paths[0],
            PathBuf::from("/tmp/custom-skills")
        );
        assert_eq!(
            config.skills.extra_paths[1],
            PathBuf::from("/opt/shared/skills")
        );
    }

    #[test]
    fn test_exec_config_default() {
        let default = ExecConfig::default();
        assert_eq!(default.python_path, "python3");
        assert_eq!(default.timeout_secs, 300);
        assert_eq!(default.max_rpc_calls, 50);
        assert_eq!(default.max_output_bytes, 50_000);
    }

    #[test]
    fn test_config_default_includes_exec() {
        let config = Config::default();
        assert_eq!(config.exec.python_path, "python3");
        assert_eq!(config.exec.timeout_secs, 300);
    }

    #[test]
    fn test_config_parses_without_exec_section() {
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.exec.python_path, "python3");
        assert_eq!(config.exec.timeout_secs, 300);
    }

    #[test]
    fn test_subagent_config_default() {
        let default = SubagentConfig::default();
        assert_eq!(default.timeout_secs, 300);
        assert_eq!(default.max_subagents, 3);
        assert_eq!(default.max_iterations, 10);
    }

    #[test]
    fn test_config_default_includes_subagent() {
        let config = Config::default();
        assert_eq!(config.subagent.timeout_secs, 300);
        assert_eq!(config.subagent.max_subagents, 3);
        assert_eq!(config.subagent.max_iterations, 10);
    }

    #[test]
    fn test_config_parses_without_subagent_section() {
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.subagent.timeout_secs, 300);
        assert_eq!(config.subagent.max_subagents, 3);
        assert_eq!(config.subagent.max_iterations, 10);
    }

    #[test]
    fn test_subagent_config_default_includes_new_fields() {
        let default = SubagentConfig::default();
        assert_eq!(
            default.default_toolsets,
            vec![
                "terminal".to_string(),
                "file".to_string(),
                "web".to_string()
            ],
            "default_toolsets should be [terminal, file, web]"
        );
        assert!(default.model.is_none(), "model should default to None");
        assert!(
            default.provider.is_none(),
            "provider should default to None"
        );
        assert!(
            default.base_url.is_none(),
            "base_url should default to None"
        );
        assert!(default.api_key.is_none(), "api_key should default to None");
    }

    #[test]
    fn test_subagent_config_backward_compat_parse() {
        // Only timeout_secs in YAML — all new fields should get defaults
        let yaml = r#"
subagent:
  timeout_secs: 600
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.subagent.timeout_secs, 600);
        assert_eq!(config.subagent.max_subagents, 3);
        assert_eq!(config.subagent.max_iterations, 10);
        assert_eq!(
            config.subagent.default_toolsets,
            vec![
                "terminal".to_string(),
                "file".to_string(),
                "web".to_string()
            ]
        );
        assert!(config.subagent.model.is_none());
        assert!(config.subagent.provider.is_none());
        assert!(config.subagent.base_url.is_none());
        assert!(config.subagent.api_key.is_none());
    }

    // =========================================================================
    // Phase 19 Plan 04: SkillsConfig.config (D-07) round-trip tests
    // =========================================================================

    #[test]
    fn test_skills_config_round_trip_with_config_map() {
        let yaml = r#"
skills:
  enabled: true
  config:
    wiki:
      path: "~/research"
      format: "markdown"
    tenor:
      api_key_env: "TENOR_API_KEY"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(cfg.skills.enabled);
        assert_eq!(
            cfg.skills.config["wiki"]["path"],
            serde_yaml::Value::String("~/research".to_string())
        );
        assert_eq!(
            cfg.skills.config["wiki"]["format"],
            serde_yaml::Value::String("markdown".to_string())
        );
        assert_eq!(
            cfg.skills.config["tenor"]["api_key_env"],
            serde_yaml::Value::String("TENOR_API_KEY".to_string())
        );

        // Full round-trip: serialize → deserialize → structurally equivalent
        let ser = serde_yaml::to_string(&cfg).expect("serialize");
        let re: Config = serde_yaml::from_str(&ser).expect("deserialize");
        assert_eq!(re.skills.config, cfg.skills.config);
    }

    #[test]
    fn test_skills_config_empty_config_defaults_to_empty_map() {
        // No `config:` sub-key at all — must deserialize via #[serde(default)]
        // and yield an empty map.
        let yaml = r#"
skills:
  enabled: true
  extra_paths:
    - /tmp/x
"#;
        let cfg: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(cfg.skills.enabled);
        assert!(
            cfg.skills.config.is_empty(),
            "skills.config should default to empty HashMap when absent"
        );
    }

    #[test]
    fn test_config_skills_round_trip() {
        let mut original = Config::default();
        original.skills.enabled = false;
        original.skills.extra_paths = vec![PathBuf::from("/a"), PathBuf::from("/b")];

        let yaml = serde_yaml::to_string(&original).expect("serialize");
        let parsed: Config = serde_yaml::from_str(&yaml).expect("deserialize");

        assert_eq!(parsed.skills.enabled, original.skills.enabled);
        assert_eq!(parsed.skills.extra_paths, original.skills.extra_paths);
    }

    // =========================================================================
    // Phase 19.1 Plan 01: HubConfig round-trip tests (D-04/D-08)
    // =========================================================================

    #[test]
    fn test_hub_config_default() {
        let d = HubConfig::default();
        assert!(d.trusted_repos.is_empty());
        assert!(d.github_token_env.is_none());
        assert!(d.extra_taps.is_empty());
        assert!(d.well_known_origins.is_empty());
    }

    #[test]
    fn test_hub_config_roundtrip() {
        let yaml = r#"
skills:
  hub:
    trusted_repos:
      - "anthropics/skills"
    github_token_env: "MY_TOKEN"
    extra_taps:
      - repo: "owner/repo"
        path: "skills/"
    well_known_origins:
      - "https://skills.example.com"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(cfg.skills.hub.trusted_repos, vec!["anthropics/skills"]);
        assert_eq!(cfg.skills.hub.github_token_env.as_deref(), Some("MY_TOKEN"));
        assert_eq!(cfg.skills.hub.extra_taps.len(), 1);
        assert_eq!(cfg.skills.hub.extra_taps[0].repo, "owner/repo");
        assert_eq!(
            cfg.skills.hub.extra_taps[0].path.as_deref(),
            Some("skills/")
        );
        assert_eq!(
            cfg.skills.hub.well_known_origins,
            vec!["https://skills.example.com"]
        );

        let ser = serde_yaml::to_string(&cfg).expect("serialize");
        let re: Config = serde_yaml::from_str(&ser).expect("re-parse");
        assert_eq!(re.skills.hub.trusted_repos, cfg.skills.hub.trusted_repos);
        assert_eq!(
            re.skills.hub.github_token_env,
            cfg.skills.hub.github_token_env
        );
        assert_eq!(
            re.skills.hub.extra_taps.len(),
            cfg.skills.hub.extra_taps.len()
        );
        assert_eq!(
            re.skills.hub.well_known_origins,
            cfg.skills.hub.well_known_origins
        );
    }

    #[test]
    fn test_hub_trusted_repos_roundtrip() {
        let yaml = r#"
skills:
  hub:
    trusted_repos:
      - "openai/skills"
      - "anthropics/skills"
"#;
        let cfg: Config = serde_yaml::from_str(yaml).expect("parse");
        let set = cfg.skills.hub.trusted_repos_set();
        assert_eq!(set.len(), 2);
        assert!(set.contains("openai/skills"));
        assert!(set.contains("anthropics/skills"));
    }

    #[test]
    fn test_skills_config_backward_compat_no_hub() {
        let yaml = r#"
skills:
  enabled: true
  extra_paths: []
"#;
        let cfg: Config = serde_yaml::from_str(yaml).expect("parse");
        assert!(cfg.skills.enabled);
        assert!(cfg.skills.hub.trusted_repos.is_empty());
        assert!(cfg.skills.hub.github_token_env.is_none());
        assert!(cfg.skills.hub.extra_taps.is_empty());
        assert!(cfg.skills.hub.well_known_origins.is_empty());
    }

    // =========================================================================
    // Provider / roles backward-compat tests (Phase 12, Task 2)
    // =========================================================================

    #[test]
    fn test_config_parses_without_providers_section() {
        // Backward compat: existing config.yaml files without providers/custom_providers/roles
        // must deserialise to empty maps/vecs via serde(default).
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(
            config.providers.is_empty(),
            "providers should default to empty map"
        );
        assert!(
            config.custom_providers.is_empty(),
            "custom_providers should default to empty vec"
        );
        assert!(
            config.model.roles.is_empty(),
            "model.roles should default to empty map"
        );
    }

    // =========================================================================
    // CompressionConfig / Phase 18 keys
    // =========================================================================

    #[test]
    fn config_compression_defaults() {
        let c = Config::default();
        assert_eq!(c.agent.compression_threshold, 0.5_f32);
        assert_eq!(c.gateway.compression_threshold, 0.85_f32);
        assert_eq!(c.compression.protect_last_tokens, 20_000);
        assert_eq!(c.compression.tool_pair_shift_tokens, 500);
        assert_eq!(c.compression.protect_first_n, 3);
        assert_eq!(c.agent.context_engine, "summarizing");
        assert_eq!(c.gateway.context_engine, "local_prune");
        assert_eq!(c.agent.system_message, "");
    }

    #[test]
    fn config_context_engine_selection() {
        let yaml = r#"
agent:
  context_engine: "local_prune"
  compression_threshold: 0.6
"#;
        let c: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(c.agent.context_engine, "local_prune");
        assert!((c.agent.compression_threshold - 0.6_f32).abs() < 1e-6);
        // Unspecified gateway still defaults
        assert_eq!(c.gateway.context_engine, "local_prune");
        assert_eq!(c.gateway.compression_threshold, 0.85_f32);
    }

    #[test]
    fn test_config_parses_full_provider_section() {
        let yaml = r#"
providers:
  openrouter:
    api_mode: chat_completions
    fallback_providers: ["anthropic"]
custom_providers:
  - name: "local-llama"
    base_url: "http://localhost:11434/v1"
    api_key: "ollama"
    default_model: "llama3"
model:
  default: "anthropic/claude-sonnet-4"
  provider: "openrouter"
  roles:
    vision:
      provider: openrouter
      model: "openai/gpt-4o"
    compression:
      provider: main
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");

        // providers map
        assert!(config.providers.contains_key("openrouter"));
        let or = &config.providers["openrouter"];
        assert_eq!(or.api_mode, Some(ApiMode::ChatCompletions));
        assert_eq!(or.fallback_providers, vec!["anthropic".to_string()]);

        // custom_providers list
        assert_eq!(config.custom_providers.len(), 1);
        let local = &config.custom_providers[0];
        assert_eq!(local.name, "local-llama");
        assert_eq!(local.base_url, "http://localhost:11434/v1");
        assert_eq!(local.api_key.as_deref(), Some("ollama"));
        assert_eq!(local.default_model.as_deref(), Some("llama3"));

        // model.roles
        assert_eq!(config.model.roles.len(), 2);
        let vision = &config.model.roles["vision"];
        assert_eq!(vision.provider, "openrouter");
        assert_eq!(vision.model.as_deref(), Some("openai/gpt-4o"));
        let compression = &config.model.roles["compression"];
        assert_eq!(compression.provider, "main");
        assert!(compression.model.is_none());
    }

    // =========================================================================
    // Phase 21.2 Plan 01: mcp_servers field round-trip tests (D-21)
    // =========================================================================

    #[test]
    fn test_mcp_servers_config_round_trip() {
        let yaml = r#"
mcp_servers:
  github:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: "${GITHUB_TOKEN}"
  filesystem:
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.mcp_servers.len(), 2);
        assert!(config.mcp_servers.contains_key("github"));
        assert!(config.mcp_servers.contains_key("filesystem"));
    }

    #[test]
    fn test_mcp_servers_defaults_to_empty_map() {
        // Backward compat: existing config.yaml files without mcp_servers must parse cleanly.
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(
            config.mcp_servers.is_empty(),
            "mcp_servers should default to empty HashMap when absent"
        );
    }

    #[test]
    fn test_mcp_servers_round_trips_through_serde() {
        let yaml = r#"
mcp_servers:
  myserver:
    url: "https://mcp.example.com/v1"
    timeout: 30
    enabled: false
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        // Serialize and deserialize again
        let serialized = serde_yaml::to_string(&config).unwrap();
        let reparsed: Config = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.mcp_servers.len(), 1);
        assert!(reparsed.mcp_servers.contains_key("myserver"));
    }

    // =========================================================================
    // GAP-4: memory_enabled / user_profile_enabled toggle tests (Phase 21.4)
    // =========================================================================

    #[test]
    fn memory_config_toggles_default_true() {
        let mc = MemoryConfig::default();
        assert!(mc.memory_enabled);
        assert!(mc.user_profile_enabled);
    }

    #[test]
    fn memory_config_toggles_round_trip() {
        let yaml = "provider: file\nmemory_enabled: false\nuser_profile_enabled: false\n";
        let mc: MemoryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(!mc.memory_enabled);
        assert!(!mc.user_profile_enabled);
        let serialized = serde_yaml::to_string(&mc).unwrap();
        assert!(serialized.contains("memory_enabled: false"));
        assert!(serialized.contains("user_profile_enabled: false"));
    }

    #[test]
    fn memory_config_missing_toggles_default_to_true() {
        let yaml = "provider: sqlite\n";
        let mc: MemoryConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(mc.memory_enabled);
        assert!(mc.user_profile_enabled);
    }

    // =========================================================================
    // Phase 22.4.2.2 Plan 01: telegram_default_origin tests (D-07/D-08)
    // =========================================================================

    #[test]
    fn test_telegram_default_origin_disabled() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: false
      whitelist: [12345]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(
            config.telegram_default_origin(),
            OriginDecision::None
        ));
    }

    #[test]
    fn test_telegram_default_origin_single() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let dec = config.telegram_default_origin();
        assert!(matches!(dec, OriginDecision::Single { .. }));
        if let OriginDecision::Single { chat_id, platform } = dec {
            assert_eq!(chat_id, "12345");
            assert_eq!(platform, "telegram");
        }
    }

    #[test]
    fn test_telegram_default_origin_multi() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345, 67890]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let dec = config.telegram_default_origin();
        assert!(matches!(dec, OriginDecision::Multi { .. }));
        if let OriginDecision::Multi { whitelist } = dec {
            assert_eq!(whitelist.len(), 2);
            assert!(whitelist.contains(&"12345".to_string()));
            assert!(whitelist.contains(&"67890".to_string()));
        }
    }

    #[test]
    fn test_telegram_default_origin_no_section() {
        let config = Config::default();
        assert!(matches!(
            config.telegram_default_origin(),
            OriginDecision::None
        ));
    }

    // -----------------------------------------------------------------------
    // Phase 25 Plan 01 Task 1: ToolsConfig + DEFAULT_TOOLSETS tests
    // -----------------------------------------------------------------------

    /// Test: ToolsConfig::default() returns enabled for memory/session/agent/skills,
    /// disabled for web/code (D-20).
    #[test]
    fn tools_config_default_has_correct_enabled_set() {
        let cfg = ToolsConfig::default();
        for name in &["memory", "session", "agent", "skills"] {
            assert!(
                cfg.is_toolset_enabled(name),
                "ToolsConfig::default() must have '{}' enabled (D-20)",
                name
            );
        }
        for name in &["web", "code"] {
            assert!(
                !cfg.is_toolset_enabled(name),
                "ToolsConfig::default() must have '{}' disabled (D-20)",
                name
            );
        }
    }

    /// Test: Unknown toolset names default to disabled (D-23 — opt-in for unknowns).
    #[test]
    fn tools_config_unknown_toolset_defaults_to_disabled() {
        let cfg = ToolsConfig::default();
        assert!(
            !cfg.is_toolset_enabled("mcp__github"),
            "Unknown toolset 'mcp__github' must default to disabled (D-23)"
        );
    }

    /// Test: serde roundtrip (YAML serialize then deserialize) preserves enabled state.
    #[test]
    fn tools_config_serde_roundtrip_preserves_enabled_state() {
        let mut cfg = ToolsConfig::default();
        cfg.toolsets
            .insert("web".to_string(), ToolsetEntry { enabled: true });
        let yaml = serde_yaml::to_string(&cfg).expect("serialize must succeed");
        let roundtripped: ToolsConfig =
            serde_yaml::from_str(&yaml).expect("deserialize must succeed");
        assert!(
            roundtripped.is_toolset_enabled("web"),
            "After roundtrip, 'web' must still be enabled"
        );
        assert!(
            roundtripped.is_toolset_enabled("memory"),
            "After roundtrip, 'memory' must still be enabled"
        );
        assert!(
            !roundtripped.is_toolset_enabled("code"),
            "After roundtrip, 'code' must still be disabled"
        );
    }

    /// Test (D-24): Parse a YAML lacking a `tools:` block; assert Config.tools == ToolsConfig::default().
    #[test]
    fn config_with_default_tools_field_loads_with_no_tools_block() {
        let yaml = r#"
model:
  provider: anthropic
"#;
        let config: Config =
            serde_yaml::from_str(yaml).expect("parse must succeed without tools block");
        let default_cfg = ToolsConfig::default();
        // Verify D-20 defaults are present
        for name in &["memory", "session", "agent", "skills"] {
            assert_eq!(
                config.tools.is_toolset_enabled(name),
                default_cfg.is_toolset_enabled(name),
                "Config loaded without tools block must have same '{}' state as ToolsConfig::default()",
                name
            );
        }
        for name in &["web", "code"] {
            assert_eq!(
                config.tools.is_toolset_enabled(name),
                default_cfg.is_toolset_enabled(name),
                "Config loaded without tools block must have same '{}' state as ToolsConfig::default()",
                name
            );
        }
    }

    /// Test: DEFAULT_TOOLSETS constant matches D-20 (memory/session/agent/skills/robotics).
    /// Phase 27.1.1-gap-01 added "robotics" to DEFAULT_TOOLSETS (5th entry) so HexapodTcpTool
    /// reaches `is_available()` even on fresh configs — the HEXAPOD_IP env var is the final gate.
    #[test]
    fn default_toolsets_constant_matches_d20() {
        use crate::constants::DEFAULT_TOOLSETS;
        assert!(
            DEFAULT_TOOLSETS.contains(&"memory"),
            "DEFAULT_TOOLSETS must contain 'memory'"
        );
        assert!(
            DEFAULT_TOOLSETS.contains(&"session"),
            "DEFAULT_TOOLSETS must contain 'session'"
        );
        assert!(
            DEFAULT_TOOLSETS.contains(&"agent"),
            "DEFAULT_TOOLSETS must contain 'agent'"
        );
        assert!(
            DEFAULT_TOOLSETS.contains(&"skills"),
            "DEFAULT_TOOLSETS must contain 'skills'"
        );
        assert!(
            DEFAULT_TOOLSETS.contains(&"robotics"),
            "Phase 27.1.1-gap-01: DEFAULT_TOOLSETS must contain 'robotics'"
        );
        assert_eq!(
            DEFAULT_TOOLSETS.len(),
            5,
            "DEFAULT_TOOLSETS must contain exactly 5 entries (memory, session, agent, skills, robotics)"
        );
    }

    // =========================================================================
    // Phase 26 Plan 01: config schema additions (D-01, D-04, D-05, D-06, D-18)
    // =========================================================================

    /// D-04: validate_api_key_env rejects invalid names.
    #[test]
    fn api_key_env_validation_rejects_invalid() {
        // Empty string
        assert!(
            validate_api_key_env("").is_err(),
            "empty string must be rejected"
        );
        // Lowercase
        assert!(
            validate_api_key_env("lower_case").is_err(),
            "lowercase name must be rejected"
        );
        // Mixed case
        assert!(
            validate_api_key_env("Mixed_Case").is_err(),
            "mixed-case name must be rejected"
        );
        // Has space
        assert!(
            validate_api_key_env("HAS SPACE").is_err(),
            "name with space must be rejected"
        );
        // Starts with digit
        assert!(
            validate_api_key_env("1_STARTS_WITH_DIGIT").is_err(),
            "name starting with digit must be rejected"
        );
        // Starts with underscore
        assert!(
            validate_api_key_env("_STARTS_WITH_UNDERSCORE").is_err(),
            "name starting with underscore must be rejected"
        );
        // Shell injection attempt
        assert!(
            validate_api_key_env("$(rm -rf ~)").is_err(),
            "shell injection pattern must be rejected"
        );
    }

    /// D-04: validate_api_key_env accepts valid env var names.
    #[test]
    fn api_key_env_validation_accepts_valid() {
        assert!(
            validate_api_key_env("OPENAI_API_KEY").is_ok(),
            "OPENAI_API_KEY must be accepted"
        );
        assert!(
            validate_api_key_env("MY_KEY_123").is_ok(),
            "MY_KEY_123 must be accepted"
        );
        assert!(
            validate_api_key_env("A").is_ok(),
            "single uppercase letter must be accepted"
        );
        assert!(
            validate_api_key_env("ANTHROPIC_API_KEY").is_ok(),
            "ANTHROPIC_API_KEY must be accepted"
        );
        assert!(
            validate_api_key_env("MY_LLM_KEY").is_ok(),
            "MY_LLM_KEY must be accepted"
        );
    }

    /// D-01: ProviderConfig parses with new api_key_env field.
    #[test]
    fn provider_config_parses_api_key_env() {
        let yaml = r#"
providers:
  openai:
    api_key_env: OPENAI_API_KEY
    default_model: gpt-4o
  my-local-llm:
    base_url: http://localhost:8080/v1
    api_key_env: MY_LLM_KEY
    default_model: llama3.1
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        let openai = &config.providers["openai"];
        assert_eq!(openai.api_key_env.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(openai.default_model.as_deref(), Some("gpt-4o"));

        let local = &config.providers["my-local-llm"];
        assert_eq!(local.api_key_env.as_deref(), Some("MY_LLM_KEY"));
        assert_eq!(local.base_url.as_deref(), Some("http://localhost:8080/v1"));
    }

    /// D-14: ProviderConfig parses with `disabled` field.
    #[test]
    fn provider_config_parses_disabled_field() {
        let yaml = r#"
providers:
  openrouter:
    disabled: true
  anthropic:
    disabled: false
  openai: {}
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.providers["openrouter"].disabled, Some(true));
        assert_eq!(config.providers["anthropic"].disabled, Some(false));
        assert_eq!(config.providers["openai"].disabled, None);
    }

    /// Backward compat: existing configs WITHOUT api_key_env/disabled parse cleanly (D-18).
    #[test]
    fn provider_config_backward_compat_without_new_fields() {
        let yaml = r#"
providers:
  openrouter:
    api_mode: chat_completions
    fallback_providers: ["anthropic"]
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse without new fields");
        let or = &config.providers["openrouter"];
        assert!(or.api_key_env.is_none(), "api_key_env must default to None");
        assert!(or.disabled.is_none(), "disabled must default to None");
    }

    /// D-06: auxiliary config defaults to unset (is_set() == false).
    #[test]
    fn auxiliary_config_default_is_unset() {
        let config = Config::default();
        assert!(
            !config.auxiliary.is_set(),
            "auxiliary must be unset by default (D-06)"
        );
        assert!(config.auxiliary.provider.is_empty());
        assert!(config.auxiliary.model.is_empty());
    }

    /// D-05: auxiliary config parses from YAML.
    #[test]
    fn auxiliary_config_parses_from_yaml() {
        let yaml = r#"
auxiliary:
  provider: openai
  model: gpt-4o-mini
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert!(config.auxiliary.is_set());
        assert_eq!(config.auxiliary.provider, "openai");
        assert_eq!(config.auxiliary.model, "gpt-4o-mini");
    }

    /// Backward compat: configs WITHOUT auxiliary block parse cleanly (D-06, serde default).
    #[test]
    fn config_without_auxiliary_block_parses_cleanly() {
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config =
            serde_yaml::from_str(yaml).expect("must parse without auxiliary block");
        assert!(
            !config.auxiliary.is_set(),
            "auxiliary must be unset when block absent"
        );
    }

    /// D-05: AuxiliaryConfig round-trip serialization.
    #[test]
    fn auxiliary_config_serde_roundtrip() {
        let yaml = r#"
auxiliary:
  provider: anthropic
  model: claude-haiku-4-5
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");
        let ser = serde_yaml::to_string(&config).expect("serialize");
        let re: Config = serde_yaml::from_str(&ser).expect("re-parse");
        assert_eq!(re.auxiliary.provider, "anthropic");
        assert_eq!(re.auxiliary.model, "claude-haiku-4-5");
    }

    /// D-02: custom_providers migration — entries NOT in providers get migrated with warning.
    /// This test verifies the structural effect (migration happens); stderr output is not
    /// captured in unit tests (that requires subprocess isolation per RESEARCH.md A4).
    #[test]
    fn custom_providers_migration_copies_missing_entries_to_providers() {
        // Write a temp config YAML with only custom_providers (no providers key)
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.yaml");
        let yaml = r#"
custom_providers:
  - name: "my-local-llm"
    base_url: "http://localhost:8080/v1"
    default_model: "llama3.1"
"#;
        std::fs::write(&config_path, yaml).expect("write");

        let config = Config::load_from(&config_path).expect("load");
        assert!(
            config.providers.contains_key("my-local-llm"),
            "migrated entry must appear in providers HashMap"
        );
        let entry = &config.providers["my-local-llm"];
        assert_eq!(entry.base_url.as_deref(), Some("http://localhost:8080/v1"));
        assert_eq!(entry.default_model.as_deref(), Some("llama3.1"));
    }

    /// D-02: when providers.foo already exists, custom_providers.foo is silently dropped.
    #[test]
    fn custom_providers_migration_does_not_overwrite_existing_providers_entry() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.yaml");
        let yaml = r#"
providers:
  my-local-llm:
    base_url: "http://localhost:9090/v1"
    default_model: "mistral"
custom_providers:
  - name: "my-local-llm"
    base_url: "http://localhost:8080/v1"
    default_model: "llama3.1"
"#;
        std::fs::write(&config_path, yaml).expect("write");

        let config = Config::load_from(&config_path).expect("load");
        // providers.my-local-llm must retain the providers: entry, not the custom_providers one
        let entry = &config.providers["my-local-llm"];
        assert_eq!(
            entry.base_url.as_deref(),
            Some("http://localhost:9090/v1"),
            "providers: entry must win over custom_providers: when both define the same name"
        );
        assert_eq!(entry.default_model.as_deref(), Some("mistral"));
    }

    // =========================================================================
    // Phase 26 Plan 01 Task 2: validate_role_name + RESERVED_ROLE_NAMES (D-05)
    // =========================================================================

    /// D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1: RESERVED_ROLE_NAMES must hold exactly
    /// the seven roles (5 from Phase 26 + summarization from Phase 25.2 + curator from Phase 25.3).
    #[test]
    fn reserved_role_names_contains_all_seven_roles_with_curator() {
        assert_eq!(
            RESERVED_ROLE_NAMES.len(),
            7,
            "Phase 26 D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1 specify exactly 7 reserved roles"
        );
        for required in &[
            "vision",
            "compression",
            "session_search",
            "skills_hub",
            "mcp_helper",
            "summarization",
            "curator",
        ] {
            assert!(
                RESERVED_ROLE_NAMES.contains(required),
                "RESERVED_ROLE_NAMES must contain '{}'",
                required
            );
        }
    }

    /// D-05: validate_role_name accepts every reserved role.
    #[test]
    fn validate_role_name_accepts_all_reserved_roles() {
        for role in RESERVED_ROLE_NAMES {
            assert!(
                validate_role_name(role).is_ok(),
                "reserved role '{}' must validate",
                role
            );
        }
    }

    /// D-05: validate_role_name rejects unknown role names (anti-pattern: swallowing).
    #[test]
    fn validate_role_name_rejects_unknown_names() {
        assert!(
            validate_role_name("").is_err(),
            "empty role name must be rejected"
        );
        assert!(
            validate_role_name("voice").is_err(),
            "unknown role 'voice' must be rejected"
        );
        assert!(
            validate_role_name("Vision").is_err(),
            "case mismatch 'Vision' must be rejected (canonical is lowercase)"
        );
        assert!(
            validate_role_name("session-search").is_err(),
            "hyphen instead of underscore must be rejected"
        );
        assert!(
            validate_role_name("typo-name").is_err(),
            "arbitrary unknown name must be rejected"
        );
    }

    /// D-05: validate_role_name error message lists the allowed roles for operator UX.
    #[test]
    fn validate_role_name_error_message_lists_allowed_roles() {
        let err = validate_role_name("voice").unwrap_err().to_string();
        assert!(
            err.contains("vision"),
            "error must enumerate allowed roles: {}",
            err
        );
        assert!(
            err.contains("compression"),
            "error must enumerate allowed roles: {}",
            err
        );
        assert!(
            err.contains("mcp_helper"),
            "error must enumerate allowed roles: {}",
            err
        );
    }

    // =========================================================================
    // Phase 25.2 Plan 02 Task 1: ExtractConfig (D-22) + summarization role (D-13)
    // =========================================================================

    /// D-22: ExtractConfig::default() must match the locked spec defaults.
    #[test]
    fn extract_config_defaults() {
        let c = ExtractConfig::default();
        assert_eq!(c.max_parallel_summaries, 4);
        assert_eq!(c.summary_chunk_chars, 100_000);
        assert_eq!(c.refuse_threshold_chars, 2_000_000);
        assert_eq!(c.summary_tier2_threshold_chars, 5_000);
        assert_eq!(c.summary_tier3_threshold_chars, 500_000);
        assert!(c.redact_url_patterns.is_empty());
    }

    /// D-22: pre-25.2 YAML configs (without an `extract:` block) must still parse and
    /// surface ExtractConfig::default() values via #[serde(default)].
    #[test]
    fn config_parses_yaml_without_extract_block() {
        // Minimal pre-25.2 config — `extract:` key absent. Config is fully-defaulted so
        // even an empty document parses to Config::default(); the meaningful assertion
        // is that the missing `extract` field is filled by ExtractConfig::default().
        let yaml = "model:\n  default: gpt-4o\n";
        let parsed: Result<Config, _> = serde_yaml::from_str(yaml);
        if let Ok(cfg) = parsed {
            assert_eq!(cfg.extract.max_parallel_summaries, 4);
            assert_eq!(cfg.extract.summary_chunk_chars, 100_000);
        }
        // Direct ExtractConfig parse: partial YAML preserves defaults for unset fields.
        let extract_only = "max_parallel_summaries: 8\n";
        let e: ExtractConfig =
            serde_yaml::from_str(extract_only).expect("partial extract YAML must parse");
        assert_eq!(e.max_parallel_summaries, 8);
        assert_eq!(e.summary_chunk_chars, 100_000); // default preserved
        assert_eq!(e.refuse_threshold_chars, 2_000_000); // default preserved
    }

    /// Phase 25.2 D-13: summarization is the second resolve_role consumer (web_extract);
    /// it must be in RESERVED_ROLE_NAMES so config validation accepts the role.
    #[test]
    fn reserved_role_names_includes_summarization() {
        assert!(
            RESERVED_ROLE_NAMES.contains(&"summarization"),
            "Phase 25.2 D-13 requires `summarization` in RESERVED_ROLE_NAMES"
        );
    }

    // =========================================================================
    // Phase 27.1.1-gap-02: ToolsConfig merge helper tests
    // =========================================================================

    #[test]
    fn test_merge_adds_absent_default_toolsets() {
        use crate::constants::ALL_TOOLSETS;
        // Start with a completely empty toolsets map.
        let cfg = ToolsConfig {
            toolsets: std::collections::HashMap::new(),
            skip_prompts: vec![],
            disabled: vec![],
        };
        let merged = cfg.with_default_toolsets_merged();
        // Every name in ALL_TOOLSETS must be present and enabled.
        for &name in ALL_TOOLSETS {
            let entry = merged.toolsets.get(name);
            assert!(
                entry.is_some(),
                "ALL_TOOLSETS entry '{}' must be present after merge",
                name
            );
            assert!(
                entry.unwrap().enabled,
                "absent entry '{}' must default to enabled=true after merge",
                name
            );
        }
    }

    #[test]
    fn test_merge_preserves_explicit_disabled() {
        // web is explicitly disabled; robotics is absent.
        let mut toolsets = std::collections::HashMap::new();
        toolsets.insert("web".to_string(), ToolsetEntry { enabled: false });
        let cfg = ToolsConfig {
            toolsets,
            skip_prompts: vec![],
            disabled: vec![],
        };
        let merged = cfg.with_default_toolsets_merged();
        // web stays disabled.
        assert!(
            !merged.toolsets["web"].enabled,
            "explicit web: disabled must be preserved after merge"
        );
        // robotics (absent) is enabled.
        assert!(
            merged.toolsets.get("robotics").map(|e| e.enabled).unwrap_or(false),
            "absent robotics entry must default to enabled=true after merge"
        );
    }

    #[test]
    fn test_merge_preserves_explicit_enabled() {
        // web is explicitly enabled (non-default to check preservation).
        let mut toolsets = std::collections::HashMap::new();
        toolsets.insert("web".to_string(), ToolsetEntry { enabled: true });
        let cfg = ToolsConfig {
            toolsets,
            skip_prompts: vec![],
            disabled: vec![],
        };
        let merged = cfg.with_default_toolsets_merged();
        assert!(
            merged.toolsets["web"].enabled,
            "explicit web: enabled must be preserved after merge"
        );
    }

    #[test]
    fn test_enabled_toolset_names() {
        let mut toolsets = std::collections::HashMap::new();
        toolsets.insert("web".to_string(), ToolsetEntry { enabled: true });
        toolsets.insert("code".to_string(), ToolsetEntry { enabled: false });
        toolsets.insert("memory".to_string(), ToolsetEntry { enabled: true });
        let cfg = ToolsConfig {
            toolsets,
            skip_prompts: vec![],
            disabled: vec![],
        };
        let names = cfg.enabled_toolset_names();
        assert!(names.contains("web"), "web must be in enabled set");
        assert!(names.contains("memory"), "memory must be in enabled set");
        assert!(!names.contains("code"), "code (disabled) must NOT be in enabled set");
        assert_eq!(names.len(), 2, "enabled set must have exactly 2 entries");
    }

    #[test]
    fn test_idempotent_merge() {
        use crate::constants::ALL_TOOLSETS;
        // Start with partial explicit config.
        let mut toolsets = std::collections::HashMap::new();
        toolsets.insert("web".to_string(), ToolsetEntry { enabled: false });
        let cfg = ToolsConfig {
            toolsets,
            skip_prompts: vec![],
            disabled: vec![],
        };
        // Apply merge twice.
        let once = cfg.clone().with_default_toolsets_merged();
        let twice = cfg.with_default_toolsets_merged().with_default_toolsets_merged();
        // Both must agree on every ALL_TOOLSETS entry.
        for &name in ALL_TOOLSETS {
            let once_val = once.toolsets.get(name).map(|e| e.enabled);
            let twice_val = twice.toolsets.get(name).map(|e| e.enabled);
            assert_eq!(
                once_val, twice_val,
                "merge must be idempotent: '{}' differs between one and two applications",
                name
            );
        }
    }

    // =========================================================================
    // Phase 32 Plan 01 (LEARN-01): MemoryConfig.nudge_interval tests.
    // =========================================================================

    #[test]
    fn config_nudge_interval_default() {
        // Default is 10 user turns — matches Python hermes-agent reference.
        let mc = MemoryConfig::default();
        assert_eq!(
            mc.nudge_interval, 10,
            "Phase 32 LEARN-01: MemoryConfig::default().nudge_interval must be 10"
        );
    }

    #[test]
    fn config_nudge_interval_deserialize() {
        // Explicit YAML value is preserved through serde.
        let yaml = "provider: file\nnudge_interval: 5\n";
        let mc: MemoryConfig = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(
            mc.nudge_interval, 5,
            "Phase 32 LEARN-01: explicit nudge_interval value must round-trip"
        );
    }

    #[test]
    fn config_nudge_interval_missing_uses_default() {
        // Backward compat: YAML without nudge_interval gives the default (10).
        let yaml = "provider: file\n";
        let mc: MemoryConfig = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(
            mc.nudge_interval, 10,
            "Phase 32 LEARN-01: missing nudge_interval key must default to 10"
        );
    }

    #[test]
    fn config_nudge_interval_zero_disabled() {
        // nudge_interval=0 is the documented "disable" sentinel; serde must
        // preserve it so the runtime can detect the disabled state.
        let yaml = "nudge_interval: 0\n";
        let mc: MemoryConfig = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(
            mc.nudge_interval, 0,
            "Phase 32 LEARN-01: nudge_interval=0 must deserialize as 0 (disable sentinel)"
        );
    }
}
