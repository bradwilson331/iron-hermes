use crate::config_extras::ProviderModelConfig;
use crate::constants::{DEFAULT_MAX_ITERATIONS, DEFAULT_MODEL, get_hermes_home};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
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

/// The nine reserved auxiliary role names (D-05, PROV-06, Phase 26 + Phase 25.2 D-13 + Phase 25.3 D-P0-1 + Phase 36.3.7.10 + Phase 36.3.7.12).
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
    "kanban_decomposer", // Phase 36.3.7.10 — bridges to auxiliary.kanban_decomposer config key (reference.md §449-451)
    "kanban_judge",      // Phase 36.3.7.12 D-05 — auxiliary judge for the goal-mode worker loop
];

/// Validate that a role name is one of the nine reserved helper-task roles.
///
/// Valid: `vision`, `compression`, `session_search`, `skills_hub`, `mcp_helper`,
/// `summarization`, `curator`, `kanban_decomposer`, `kanban_judge`
/// (Phase 26 D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1 + Phase 36.3.7.10 + Phase 36.3.7.12).
///
/// # Errors
/// Returns an error if `name` is not in `RESERVED_ROLE_NAMES`.
pub fn validate_role_name(name: &str) -> anyhow::Result<()> {
    if RESERVED_ROLE_NAMES.contains(&name) {
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

/// Phase 41.3 D-08: legal provider names for `tools.web_search.chain`, in the
/// same order as the built-in default order (Claude's discretion under D-08,
/// constrained by D-09/D-10's requirement that DDG terminate the chain).
/// Single source of truth — Plan 07/08/09 read this rather than hard-coding a
/// parallel list.
pub const WEB_SEARCH_PROVIDERS: [&str; 4] = ["exa", "brave", "tavily", "ddg"];
/// Phase 41.3 D-08: legal provider names for `tools.web_answer.chain`.
pub const WEB_ANSWER_PROVIDERS: [&str; 4] = ["perplexity", "exa", "brave", "ddg"];
/// Phase 41.3 D-17: legal provider names for `tools.web_extract.chain` —
/// exactly the four backends `fetch_web_with_chain` dispatches to. Order
/// matches the fixed order that shipped in Phase 25.2 D-04 so the default
/// chain reproduces today's behavior byte-for-byte.
pub const WEB_EXTRACT_PROVIDERS: [&str; 4] = ["firecrawl", "exa", "tavily", "local"];

/// Phase 41.3 D-08: a single named provider chain for one of the three web
/// tools (`web_search`, `web_answer`, `web_extract`). A one-field struct
/// (rather than a bare `Vec<String>` directly on `ToolsConfig`) is
/// deliberate: it gives `tools.web_search.*` room to grow per-tool keys
/// later without another schema break.
///
/// `#[serde(default)]` means an explicitly-empty section
/// (`tools.web_extract: {}`) deserializes to an empty chain rather than a
/// parse error — `ToolsConfig::validate_chains()` then reports that empty
/// chain as a problem instead of silently falling back to a built-in
/// default the operator did not ask for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebToolChainConfig {
    pub chain: Vec<String>,
}

fn default_web_search_chain_config() -> WebToolChainConfig {
    WebToolChainConfig {
        chain: WEB_SEARCH_PROVIDERS.iter().map(|s| s.to_string()).collect(),
    }
}

fn default_web_answer_chain_config() -> WebToolChainConfig {
    WebToolChainConfig {
        chain: WEB_ANSWER_PROVIDERS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Phase 41.3 D-17: exactly the fixed order that shipped in Phase 25.2 D-04
/// (`Firecrawl > Exa > Tavily > Local`), so an operator who upgrades without
/// editing config.yaml sees no behavior change.
fn default_web_extract_chain_config() -> WebToolChainConfig {
    WebToolChainConfig {
        chain: WEB_EXTRACT_PROVIDERS.iter().map(|s| s.to_string()).collect(),
    }
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
    /// Phase 41.3 D-06/D-15: operator global default wall-clock bound (seconds)
    /// for tool execution, used when a tool does not declare its own budget via
    /// `Tool::timeout_secs()`. 60s is 2x `WebConfig.timeout_secs`'s 30s HTTP-leg
    /// default, so a wedged web call surfaces inside a minute.
    #[serde(default = "default_tools_timeout_secs")]
    pub timeout_secs: u64,
    /// Phase 41.3 D-06: per-tool timeout override in seconds, keyed by tool
    /// name. Deliberately **signed** — a configured value `<= 0` disables the
    /// bound for that tool, parity with the Python
    /// `HERMES_CONCURRENT_TOOL_TIMEOUT_S <= 0` convention. This arm wins even
    /// over a code-level `Tool::timeout_secs() -> None` opt-out.
    #[serde(default)]
    pub timeout_overrides: HashMap<String, i64>,
    /// Phase 41.3 D-08: config-ordered backend chain for the web_search tool.
    #[serde(default = "default_web_search_chain_config")]
    pub web_search: WebToolChainConfig,
    /// Phase 41.3 D-08: config-ordered backend chain for the web_answer tool.
    #[serde(default = "default_web_answer_chain_config")]
    pub web_answer: WebToolChainConfig,
    /// Phase 41.3 D-08/D-17: config-ordered backend chain for the web_extract
    /// tool, replacing the fixed env-order `select_backend()` ladder. Read at
    /// call time via `Config::load()` inside `fetch_web_with_chain` — never
    /// cached at construction.
    #[serde(default = "default_web_extract_chain_config")]
    pub web_extract: WebToolChainConfig,
    /// Phase 41.3 D-18/D-19: the **middle** tier of the env → config → vault tool
    /// credential precedence chain (`ironhermes_tools::credentials::ToolCredentials`),
    /// keyed by the canonical env-var name (e.g. `EXA_API_KEY`) — never by a
    /// `tools.`-prefixed variant, matching `EnvVarStore`'s refusal to invent a
    /// naming scheme (`env_var_store.rs:4-8`). Consulted only when the
    /// corresponding process env var is absent, and itself outranked by nothing
    /// except env — the vault is never even queried for a key this map already
    /// satisfies.
    ///
    /// **This puts a credential in a plaintext file on disk.** Env or the vault
    /// (`vault.enabled: true`) is the preferred home for a secret; this map exists
    /// for operators who cannot use either. That tradeoff is the operator's to
    /// make, but it is not free: anyone who can read `config.yaml` can read a value
    /// stored here.
    #[serde(default)]
    pub credentials: BTreeMap<String, String>,
}

/// Phase 41.3 D-15: trait-default / operator-global-default wall-clock bound
/// (seconds) for tool execution. Mirrors `ironhermes_tools::registry::DEFAULT_TOOL_TIMEOUT_SECS`
/// (duplicated rather than imported — `ironhermes-core` does not depend on
/// `ironhermes-tools`).
fn default_tools_timeout_secs() -> u64 {
    60
}

impl Default for ToolsConfig {
    fn default() -> Self {
        let mut toolsets = HashMap::new();
        // Phase 36.3.8: `messaging` (send_message + clarify) is a core agent
        // capability, default-on like agent/skills. Outbound send is already
        // whitelist-gated (D-12), so the blast radius is bounded.
        for name in ["memory", "session", "agent", "skills", "messaging"] {
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
            timeout_secs: default_tools_timeout_secs(),
            timeout_overrides: HashMap::new(),
            web_search: default_web_search_chain_config(),
            web_answer: default_web_answer_chain_config(),
            web_extract: default_web_extract_chain_config(),
            credentials: BTreeMap::new(),
        }
    }
}

impl ToolsConfig {
    /// D-23: enabled iff entry exists with enabled:true. Unknown names default to false
    /// so MCP-server-as-toolset (e.g., "mcp__github") requires explicit opt-in.
    pub fn is_toolset_enabled(&self, name: &str) -> bool {
        self.toolsets.get(name).map(|e| e.enabled).unwrap_or(false)
    }

    /// Phase 41.3 D-08: validate all three chains against their per-tool legal
    /// provider set. Returns one message per problem — an operator with two
    /// typos across two chains sees both, not just the first. Each message
    /// names both the offending value and the tool it was configured under
    /// (`unknown provider '<value>' in tools.<tool>.chain (expected one of: ...)`),
    /// so a typo is findable without bisecting. An empty chain is itself a
    /// problem (D-08: an operator who emptied a chain meant it — this is not
    /// a silent fall-through to the built-in default).
    pub fn validate_chains(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        Self::validate_one_chain(
            "web_search",
            &self.web_search.chain,
            &WEB_SEARCH_PROVIDERS,
            &mut errors,
        );
        Self::validate_one_chain(
            "web_answer",
            &self.web_answer.chain,
            &WEB_ANSWER_PROVIDERS,
            &mut errors,
        );
        Self::validate_one_chain(
            "web_extract",
            &self.web_extract.chain,
            &WEB_EXTRACT_PROVIDERS,
            &mut errors,
        );
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Validates a single tool's chain against its legal provider set,
    /// appending one message per problem to `errors`. The legal set is
    /// per-tool (not a global union across all three tools) — e.g. `local`
    /// is valid for `web_extract` but not `web_search`.
    fn validate_one_chain(tool: &str, chain: &[String], legal: &[&str], errors: &mut Vec<String>) {
        if chain.is_empty() {
            errors.push(format!(
                "empty chain for tools.{tool}.chain (expected one of: {})",
                legal.join(", ")
            ));
            return;
        }
        for value in chain {
            if !legal.contains(&value.as_str()) {
                errors.push(format!(
                    "unknown provider '{value}' in tools.{tool}.chain (expected one of: {})",
                    legal.join(", ")
                ));
            }
        }
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
#[derive(Default)]
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
    /// Per-provider extra request body options forwarded to the LLM API (D-01 fallback, Phase 36.15).
    /// Provider-level defaults; per-model overrides live under `models.<model>.extra_request_options`.
    /// Uses `HashMap<String, serde_json::Value>` (D-01 deviation — avoids serde_yaml untagged
    /// enum ambiguity with all-Optional struct fields; see config_extras.rs module doc).
    /// `#[serde(default)]` ensures pre-36.15 configs parse cleanly with an empty map.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra_request_options: HashMap<String, serde_json::Value>,
    /// Per-model configuration overrides (D-02, Phase 36.15).
    /// Keys are model identifiers (e.g., `"llama3.1:8b"`); YAML keys with colons must be quoted.
    /// `#[serde(default)]` ensures pre-36.15 configs parse cleanly with an empty map.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub models: HashMap<String, ProviderModelConfig>,
}

impl ProviderConfig {
    /// Phase 46.9 Plan 01 (D-01/T-46.9-03): presence-only check for whether a
    /// secret credential is configured for this provider — either the
    /// deprecated inline literal or a set+non-empty environment variable
    /// referenced by name. NEVER returns or exposes the credential value
    /// itself. Exists so callers that must redact secret values (e.g. the
    /// web `ProviderConfigSnapshot` DTO) can surface presence without ever
    /// naming the underlying field in their own source (keeps a strict
    /// `grep` redaction gate on those files honest).
    pub fn has_secret(&self) -> bool {
        let has_literal = self
            .api_key
            .as_deref()
            .map(|k| !k.is_empty())
            .unwrap_or(false);
        let has_env = self
            .api_key_env
            .as_deref()
            .map(|name| std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false))
            .unwrap_or(false);
        has_literal || has_env
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
#[derive(Default)]
pub struct AuxiliaryConfig {
    /// Provider name (must be a key in `providers:`). Empty string = unset.
    pub provider: String,
    /// Model identifier served by this provider. Empty string = use provider default.
    pub model: String,
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
    /// Phase 47.3 D-06: operator auth settings for the `iron_hermes_ui` web
    /// server. `#[serde(default)]` makes this optional in YAML — pre-47.3
    /// configs parse cleanly with auth disabled (no `password_hash`).
    #[serde(default)]
    pub web_ui: WebUiConfig,
    pub gateway: GatewayConfig,
    pub cron: CronConfig,
    pub security: SecurityConfig,
    pub rate_limit: RateLimitConfig,
    // SKILL-08: skills subsystem configuration (07.2 D-17, D-18)
    pub skills: SkillsConfig,
    // EXEC-01..04: code execution sandbox configuration
    pub exec: ExecConfig,
    // AGENT-01..05: subagent delegation configuration (D-07: renamed from subagent)
    pub delegation: SubagentConfig,
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
    /// Phase 36.2 (D-CACHE-02): prompt caching configuration.
    /// Pre-36.2 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub prompt_caching: PromptCachingConfig,
    /// Phase 36.3.7 (D-09): kanban subsystem configuration.
    ///
    /// Stored as raw `serde_yaml::Value` (same pattern as `mcp_servers`) to
    /// avoid a circular crate dependency — `ironhermes-kanban` already depends
    /// on `ironhermes-core`, so `ironhermes-core` cannot depend on
    /// `ironhermes-kanban`. The gateway runner deserializes this value into
    /// `ironhermes_kanban::KanbanConfig` at the task-spawn site.
    ///
    /// Pre-36.3.7 configs parse cleanly with a `Null` value, which the
    /// gateway runner treats as all-defaults.
    #[serde(default)]
    pub kanban: serde_yaml::Value,
    /// Phase 36.3.7.11 (D-17): dashboard tail consumer configuration.
    ///
    /// `dashboard.kanban.tail_interval_ms` controls the dashboard tail
    /// consumer's polling interval (default 250 ms). Pre-36.3.7.11 configs
    /// parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub dashboard: DashboardConfig,
    /// Phase 36.17.5 (D-12): TTS provider configuration.
    /// Pre-36.17.5 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub tts: TtsConfig,
    /// Phase 36.17.8 (D-06/D-18): STT provider configuration.
    /// Pre-36.17.8 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub stt: SttConfig,
    /// Phase 36.17.8 (D-09/D-18): Voice mode interaction configuration.
    /// Pre-36.17.8 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub voice: VoiceConfig,
    /// Phase 36.17.7 D-02-d: audio cache lifecycle policy.
    /// Controls GC of `$IRONHERMES_HOME/audio_cache/` files produced by
    /// `text_to_speech` + `send_audio` (web replay surface).
    /// Pre-36.17.7 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub audio_cache: AudioCacheConfig,
    /// Phase 39.1 (R39.1-03 / R39.1-04): concurrency caps for async multi-turn execution.
    ///
    /// `session_turn_cap` bounds concurrent turns per session (D-03).
    /// `global_turn_ceiling` bounds total concurrent turns process-wide (D-04).
    /// Pre-39.1 configs parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub concurrency: ConcurrencyConfig,
    /// Phase 01 (D-07); extended Phase 47 (D-01/D-02): image generation
    /// configuration — now multi-provider (fal.ai + venice.ai) with an
    /// independent `t2i.{provider,model}` alongside the legacy `default_model`.
    ///
    /// Exposed in `config.yaml` as a dedicated top-level `image_gen:` block holding
    /// `default_model`, `session_cap`, and `timeout_secs`. Pre-Phase-01 configs (no
    /// `image_gen:` section) parse cleanly via `#[serde(default)]` on both the field
    /// site here and the [`ImageGenConfig`] struct.
    #[serde(default)]
    pub image_gen: ImageGenConfig,
    /// Phase 36.3.3 (D-12); extended Phase 47 (D-01/D-02/D-12): video generation
    /// configuration — now multi-provider (fal.ai + venice.ai) with independent
    /// per-mode `{provider, model}` for t2v/i2v/v2v plus fixed-from-config
    /// `resolution`/`aspect_ratio`/`progress_ping_secs`.
    ///
    /// Exposed in `config.yaml` as a top-level `video_gen:` block. Pre-36.3.3 configs
    /// (no `video_gen:` section) parse cleanly via `#[serde(default)]` on both the
    /// field site here and the [`VideoGenConfig`] struct.
    #[serde(default)]
    pub video_gen: VideoGenConfig,
    /// Phase 47 (D-07): central cross-surface generation spend-policy block
    /// (`generation.guardrails`) consulted by every generation-capable surface —
    /// chat, kanban (regular + goal-mode), and `delegate_task` children.
    ///
    /// Pre-Phase-47 configs (no `generation:` section) parse cleanly via
    /// `#[serde(default)]` on both the field site here and the
    /// [`GenerationConfig`] struct. See [`GenerationGuardrailsConfig`] for the
    /// D-08 reconciliation with the existing per-section `image_gen.session_cap`
    /// / `video_gen.session_cap`.
    #[serde(default)]
    pub generation: GenerationConfig,
    /// Phase 40.5 (D-16/D-17): Per-identity appearance + voice profiles.
    ///
    /// Keyed by identity slug (e.g. `"orb_bloom"`, `"groovy"`). An identity record
    /// holds BOTH visual appearance knobs (for orb-type presets) and voice overrides
    /// (free-mode TTS + realtime). Fields left `None` inherit the global `tts`/`voice`
    /// defaults (D-11 partial-override model).
    ///
    /// The `serde(default)` attribute points to [`default_seed_identities`] so a
    /// config.yaml with NO `identities:` section yields the curated turn-key personas
    /// (D-13). A PARTIAL section is corrected by the explicit post-parse merge loop
    /// in [`Config::load_from`] (D-16) — `serde(default)` alone would replace a
    /// partial section entirely, dropping the shipped seed personas.
    ///
    /// Pre-40.5 configs parse cleanly: the field is absent → `default_seed_identities()`
    /// fires (T-40.5-01-01).
    #[serde(default = "default_seed_identities")]
    pub identities: std::collections::HashMap<String, IdentityRecord>,
    /// Phase 42 EXEC-01: User-defined Quick Commands (`type: exec`).
    ///
    /// Keys are command names (e.g. `"wipe-cache"`); values are dispatch specs.
    /// Pre-42 config.yaml files parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub quick_commands: std::collections::HashMap<String, crate::commands::QuickCommandDef>,
    /// Phase 42 D-10: Dangerous-command guardrail configuration (full-override).
    ///
    /// Operators can add patterns at either tier and relax/remove built-ins.
    /// Pre-42 config.yaml files parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub dangerous_commands: DangerousCommandsConfig,
    /// Phase 45 D-04: Approval gate configuration — timeout for pending approvals.
    ///
    /// `timeout_secs` (default 120): how long the gateway waits for an operator
    /// `/approve` or `/deny` before auto-expiring with `ApprovalOutcome::TimedOut`.
    /// Pre-45 config.yaml files parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub approvals: ApprovalsGatewayConfig,
    /// Phase 45 D-08/D-10: MCP mutation guardrail configuration (full-override).
    ///
    /// `patterns` (default empty): when empty the guardrail uses its built-in
    /// DEFAULT_VERBS set; a non-empty list fully overrides those built-ins.
    /// A startup `tracing::warn!` fires for every removed built-in verb (D-10).
    /// Pre-45 config.yaml files parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub mcp_mutation_guardrail: McpMutationGuardrailConfig,
    /// Phase 43 (OAUTH-03/D-02): OAuth provider configuration block.
    ///
    /// Each key under `auth.providers` is the namespace key used in
    /// `auth.json` `tokens.<name>` and is the `--provider` argument to
    /// `hermes auth login`. All endpoint fields are operator-supplied —
    /// there is NO built-in provider endpoint table (D-02).
    ///
    /// Pre-43 configs parse cleanly: the absent `auth:` block yields an
    /// empty providers map via `#[serde(default)]`.
    #[serde(default)]
    pub auth: AuthConfig,
    /// Phase 46 D-03: append-only audit log configuration (`~/.ironhermes/audit.jsonl`).
    ///
    /// No on/off disable knob by design (RESEARCH Open Question 2) — D-02's
    /// fail-loudly posture intentionally has no escape hatch. Pre-46 configs parse
    /// cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub audit: crate::audit::AuditConfig,
    /// Phase 46.1 D-01: global additive MCP-OAuth issuer allowlist.
    ///
    /// Consulted only when a `mcp_servers.<name>` entry declares no per-server
    /// `allowed_issuer` pin; additive over the built-in
    /// `security::BASELINE_ISSUER_ALLOWLIST` (`["cloudflare.com", "dash.cloudflare.com"]`).
    /// Pre-46.1 configs parse cleanly via `#[serde(default)]` — the baseline alone
    /// still applies, so the 4 existing Cloudflare servers keep working (CFL-02).
    #[serde(default)]
    pub mcp_oauth: McpOAuthConfig,
    /// Phase 46.8 D-10: vault subsystem configuration (provider API-key fallback).
    ///
    /// Defaults to `enabled: false` / `backend: "env-var"` — a zero-behavioral-change
    /// default so pre-46.8 `config.yaml` files parse cleanly via `#[serde(default)]`.
    /// When enabled, `ProviderResolver::apply_vault_fallback` consults the configured
    /// backend as resolution priority 5, ONLY after the existing 4-priority chain
    /// (api_key_env → deprecated literal → legacy env vars → deprecated model.api_key)
    /// all miss (D-02/D-07). Type lives in the zero-cycle leaf crate `ironhermes-vault`
    /// (D-01) so this crate can embed it without a dependency cycle.
    #[serde(default)]
    pub vault: ironhermes_vault::VaultConfig,
    /// Phase 46.9 Plan 11 (GAP-4/GAP-5): footer clock display configuration.
    ///
    /// Resolved by `iron_hermes_ui::server::display_tz_api::get_display_timezones`
    /// into a primary zone (`display.timezone.or(agent.timezone)`, else browser
    /// host-local) plus up to 4 extra labeled zones. Pre-46.9-11 `config.yaml`
    /// files (no `display:` key) parse cleanly via `#[serde(default)]`.
    #[serde(default)]
    pub display: DisplayConfig,
}

/// Phase 46.9 Plan 11 (GAP-4/GAP-5): footer clock display configuration.
///
/// `timezone`: an operator-supplied IANA zone name (e.g. `America/New_York`)
/// for the footer's PRIMARY wall-clock; when `None` the primary falls back to
/// [`AgentConfig::timezone`] (the existing Phase 38.1 IANA field), then to the
/// browser's host-local zone. `extra_timezones`: additional IANA zone names
/// rendered as labeled clocks alongside the primary — `get_display_timezones`
/// truncates this list to at most 4 (T-46.9-27 DoS mitigation: a server-side
/// cap, not a config validation error, so an oversized list never panics or
/// spawns unbounded footer clocks). `hour12`: quick-task QDG-01 footer clock
/// format — `None` (default) renders 24-hour, `Some(true)` renders 12-hour
/// AM/PM. All fields are `serde(default)` so pre-46.9-11 `config.yaml` files
/// parse unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub timezone: Option<String>,
    pub extra_timezones: Vec<String>,
    pub hour12: Option<bool>,
}

/// Phase 46.1 D-01: global additive MCP-OAuth issuer allowlist configuration.
///
/// Sibling to [`Config::audit`]. See [`Config::mcp_oauth`] for the layered
/// resolution semantics (per-server pin authoritative; this list is the
/// fallback, additive over the built-in baseline).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpOAuthConfig {
    /// Additional issuer hosts (beyond the built-in baseline) trusted for MCP
    /// OAuth PRM/RFC 8414 discovery when a server declares no per-server pin.
    pub issuer_allowlist: Vec<String>,
}

/// Phase 36.17.7 D-02-d: audio cache lifecycle policy.
///
/// Pre-36.17.7 configs parse cleanly via `#[serde(default)]` on this struct AND
/// on the `audio_cache:` field of [`Config`]. Defaults: 7 days max age,
/// 86400 seconds (daily) sweep interval.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioCacheConfig {
    /// Maximum age in days for files in `$IRONHERMES_HOME/audio_cache/`.
    /// Files older than this are removed by `gc_sweep_audio_cache` on startup
    /// and on every periodic sweep. Default: 7.
    pub max_age_days: u32,
    /// Periodic GC sweep interval in seconds. Default: 86400 (daily).
    /// Bounded by `max_age_days` — a sweep cadence longer than the max age
    /// would let stale files accumulate temporarily.
    pub sweep_interval_secs: u64,
}

impl Default for AudioCacheConfig {
    fn default() -> Self {
        Self {
            max_age_days: 7,
            sweep_interval_secs: 86400,
        }
    }
}

/// Phase 43 (OAUTH-03/D-02): Per-OAuth-provider configuration.
///
/// Lives under `auth.providers.<name>:` in config.yaml. `<name>` is the
/// namespace key used in `auth.json` `tokens.<name>` and is the `--provider`
/// argument to `hermes auth login`.
///
/// All endpoint fields are operator-supplied strings — there is NO built-in
/// endpoint registry (D-02). HTTPS enforcement is validated at flow-build
/// time in the PKCE/device flows, not at config-load time.
///
/// All fields are optional via `#[serde(default)]` so a partially-specified
/// provider block parses cleanly; missing fields become `None` / empty `Vec`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthProviderConfig {
    /// Human-readable label shown in `hermes auth status` output.
    /// Optional — if absent the provider key is used as the display name.
    pub display_name: Option<String>,
    /// OAuth 2.0 authorization endpoint (PKCE flow).
    /// e.g. `"https://accounts.x.ai/oauth/authorize"`
    pub authorization_url: Option<String>,
    /// OAuth 2.0 token endpoint (both PKCE and device-code flows).
    /// e.g. `"https://accounts.x.ai/oauth/token"`
    pub token_url: Option<String>,
    /// RFC 8628 device authorization endpoint (device-code flow).
    /// e.g. `"https://accounts.x.ai/oauth/device/code"`
    /// Optional — omit if the provider does not support device-code flow.
    pub device_authorization_url: Option<String>,
    /// OAuth 2.0 client ID for this operator's registered application.
    /// This is a public identifier; no client_secret is stored (PKCE/device
    /// flows are public-client flows by design — T-43-02b).
    pub client_id: Option<String>,
    /// OAuth scopes to request (e.g. `["read", "write"]`).
    /// Defaults to an empty list if omitted.
    pub scopes: Vec<String>,
}

/// Phase 43 (OAUTH-03/D-02): Top-level `auth:` configuration block.
///
/// Holds the map of named OAuth provider configurations. `AuthConfig::default()`
/// is an empty providers map — no built-in endpoint table ships in code (D-02).
///
/// Pre-43 configs without an `auth:` block parse cleanly via `#[serde(default)]`
/// and yield an empty `providers` map.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthConfig {
    /// Named OAuth provider configurations keyed by provider namespace.
    /// Each key is used as `tokens.<name>` in `auth.json` and as the
    /// `--provider` argument to `hermes auth login`.
    pub providers: HashMap<String, AuthProviderConfig>,
}

// =============================================================================
// ConcurrencyConfig (Phase 39.1 R39.1-03 / R39.1-04 / D-03 / D-04)
// =============================================================================

/// Phase 39.1 (R39.1-03 / R39.1-04): per-session and process-wide concurrency caps
/// for async multi-turn agent execution.
///
/// Exposed in `config.yaml` as a top-level `concurrency:` block.
/// `#[serde(default)]` on both the struct and the field site in [`Config`] ensures
/// pre-39.1 configs parse cleanly with the defaults applied.
///
/// - `session_turn_cap` (default 3): maximum concurrent in-flight turns per session
///   (D-03). Messages beyond this cap fall back to the existing FIFO queue.
/// - `global_turn_ceiling` (default 32): process-wide maximum across all sessions and
///   surfaces (D-04). Protects the host regardless of active conversation count.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConcurrencyConfig {
    /// Maximum concurrent in-flight turns per session (D-03). Default: 3.
    pub session_turn_cap: usize,
    /// Process-wide maximum concurrent turns across all sessions (D-04). Default: 32.
    pub global_turn_ceiling: usize,
}

impl Default for ConcurrencyConfig {
    fn default() -> Self {
        Self {
            session_turn_cap: 3,
            global_turn_ceiling: 32,
        }
    }
}

// =============================================================================
// GenModeConfig (Phase 47 D-01/D-02/GEN-03): per-mode {provider, model} config
// =============================================================================

/// Phase 47 (D-01/D-02/GEN-03): per-generation-mode provider + model selection.
///
/// Attached independently at `image_gen.t2i` and `video_gen.{t2v,i2v,v2v}` so each
/// mode resolves its own `{provider, model}` — changing one mode's model never
/// alters another mode's config (GEN-03 adjacency).
///
/// **Resolution order (D-02):** an explicit `provider` field wins; when `provider`
/// is `None`, the resolver (Plan 04's `GenBackend::resolve`) falls back to
/// effective-model prefix inference — a `fal-ai/*` model routes to fal, anything
/// else routes to venice. This ordering is what makes the venice default flip
/// invisible to existing fal.ai operators: a legacy `fal-ai/*` model with no
/// `provider` field keeps routing to fal (D-02 prohibition: MUST NOT silently
/// flip existing fal.ai users to venice on upgrade).
///
/// `#[serde(default)]` on both the struct and the field site of every mode field
/// ensures a partial YAML block (e.g. only `model` set) merges cleanly over
/// defaults, and pre-Phase-47 configs (no mode block at all) parse unchanged.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GenModeConfig {
    /// Explicit backend selection (`"venice"` / `"fal"`). `None` = infer from the
    /// effective model's prefix (D-02) — see struct doc for the resolution order.
    pub provider: Option<String>,
    /// The model ID passed to the resolved provider's generation API.
    pub model: String,
}

/// Phase 47 Task 1 (D-03): live-confirmed via `GET /api/v1/models?type=image`
/// (Bearer `VENICE_API_KEY`) on 2026-07-19 — `flux-2-pro` is present verbatim in
/// the Venice catalog. `model_spec.constraints`: `aspectRatios` `["1:1","3:2",
/// "16:9","21:9","9:16","2:3","3:4","4:5"]`, `defaultAspectRatio` `"1:1"`,
/// `promptCharacterLimit` 3000, `steps` default 20 / max 50. Pricing: $0.03/image
/// generation. This is the exact string the [`ImageGenConfig`] `Default` impl
/// locks in for `t2i.model` (see 47-01-SUMMARY.md for the full recorded response).
const DEFAULT_T2I_MODEL: &str = "flux-2-pro";

/// Phase 47 (D-03): venice default video models per mode — live-confirmed
/// present in `GET /api/v1/models?type=video` on 2026-07-19 alongside Task 1's
/// image-model confirmation.
const DEFAULT_T2V_MODEL: &str = "wan-2-7-text-to-video";
const DEFAULT_I2V_MODEL: &str = "wan-2-7-image-to-video";
const DEFAULT_V2V_MODEL: &str = "wan-2-7-video-to-video";

fn default_t2i_mode() -> GenModeConfig {
    GenModeConfig {
        provider: Some("venice".to_string()),
        model: DEFAULT_T2I_MODEL.to_string(),
    }
}

fn default_t2v_mode() -> GenModeConfig {
    GenModeConfig {
        provider: Some("venice".to_string()),
        model: DEFAULT_T2V_MODEL.to_string(),
    }
}

fn default_i2v_mode() -> GenModeConfig {
    GenModeConfig {
        provider: Some("venice".to_string()),
        model: DEFAULT_I2V_MODEL.to_string(),
    }
}

fn default_v2v_mode() -> GenModeConfig {
    GenModeConfig {
        provider: Some("venice".to_string()),
        model: DEFAULT_V2V_MODEL.to_string(),
    }
}

// =============================================================================
// ImageGenConfig (Phase 01 — fal.ai image generation; D-07 / D-08 / D-03 / D-05)
// extended Phase 47 (D-01/D-02/D-03): multi-provider t2i mode + legacy back-compat
// =============================================================================

/// Phase 01 (D-07); extended Phase 47 (D-01/D-02/D-03): image-generation settings,
/// exposed as a dedicated top-level `image_gen:` block in `config.yaml`.
///
/// `#[serde(default)]` on both the struct and the field site in [`Config`] ensures
/// pre-Phase-01 configs (with no `image_gen:` section) parse cleanly with defaults.
///
/// - `default_model` (default `"fal-ai/flux/schnell"`, MODEL-01): **legacy flat
///   key**, kept byte-for-byte for back-compat (D-01). When a config explicitly
///   sets this to something other than the shipped fal default, `Deserialize`
///   (via [`ImageGenConfigShadow`]) maps the value into `t2i.model` and leaves
///   `t2i.provider` untouched — a legacy `fal-ai/*` value keeps routing to fal
///   via prefix inference (D-02). Still LLM-overridable per call (D-08).
/// - `session_cap` (default 20, D-03): per-chat-session hard cap on generations.
///   Config-only; not exposed in the tool schema. Governs DIRECT chat-session
///   generations only — see [`GenerationGuardrailsConfig`] for the D-08
///   reconciliation with the newer `generation.guardrails.per_child_cap` tier.
/// - `timeout_secs` (default 120, D-05): polling timeout budget for a single fal
///   queue job. Config-only; not exposed in the tool schema.
/// - `t2i` (Phase 47 D-01/D-03): the per-mode `{provider, model}` — defaults to
///   `provider: venice`, `model: flux-2-pro` (Task 1 live-confirmed). A config.yaml
///   with no `image_gen:` section (or one that never sets `default_model`) yields
///   this venice default; a legacy `default_model` override maps in here instead.
/// - `steps` (default 20): diffusion sampling-step count sent on every Venice
///   image request. Venice does NOT apply the model's own `constraints.steps.default`
///   when the field is omitted, so a missing `steps` produced under-denoised "blob"
///   images — this key is the fix. Config-only; not LLM-overridable. `0` omits the
///   field (server-side fallback). fal.ai models ignore it (fal derives steps from
///   the model). Keep within the model's `constraints.steps.max` (flux-2-pro /
///   krea-2-turbo max 50).
/// - `safe_mode` (default `false`): forwarded to Venice's `safe_mode` — when `true`
///   Venice blurs adult-flagged output. Defaults `false` for a single-user host so
///   borderline prompts are not silently blurred; set `true` to re-enable the blur.
///   Config-only; Venice-only (fal ignores it).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "ImageGenConfigShadow")]
pub struct ImageGenConfig {
    /// Legacy flat model id (MODEL-01). LLM-overridable per call (D-08). Kept
    /// for back-compat (D-01) — see struct doc for the reconciliation mapping.
    pub default_model: String,
    /// Per-session generation cap (D-03). Config-only — NOT LLM-overridable (D-08).
    pub session_cap: u32,
    /// Polling timeout in seconds (D-05). Config-only — NOT LLM-overridable (D-08).
    pub timeout_secs: u64,
    /// Per-mode `{provider, model}` for text-to-image (Phase 47 D-01/D-03).
    pub t2i: GenModeConfig,
    /// Venice diffusion sampling-step count (default 20). MUST be sent or Venice
    /// returns blob images — see struct doc. Config-only; `0` omits the field.
    pub steps: u32,
    /// Venice `safe_mode` (default `false`) — blurs adult-flagged output when `true`.
    /// Config-only; Venice-only. See struct doc.
    pub safe_mode: bool,
}

impl Default for ImageGenConfig {
    fn default() -> Self {
        Self {
            default_model: "fal-ai/flux/schnell".to_string(),
            session_cap: 20,
            timeout_secs: 120,
            t2i: default_t2i_mode(),
            steps: 20,
            safe_mode: false,
        }
    }
}

/// Phase 47 (D-01/D-02): the exact legacy default `image_gen.default_model` value
/// shipped since Phase 01 — used ONLY to detect whether an operator explicitly
/// overrode the legacy flat key (see [`ImageGenConfigShadow`]'s `From` impl).
const LEGACY_IMAGE_DEFAULT_MODEL: &str = "fal-ai/flux/schnell";

/// Phase 47 (D-01/D-02): deserialize-time shadow of [`ImageGenConfig`] that
/// performs the legacy-flat-key-to-mode-struct reconciliation.
///
/// Deserializing directly into [`ImageGenConfig`] would have no way to tell
/// "the operator explicitly re-set `default_model` to a legacy fal model" apart
/// from "the field is simply defaulted" — both look identical once each field is
/// deserialized independently. Routing through this shadow (via `#[serde(from =
/// "ImageGenConfigShadow")]` on `ImageGenConfig`) lets the `From` impl compare
/// the parsed `default_model` against [`LEGACY_IMAGE_DEFAULT_MODEL`] and only
/// override `t2i.model` when it differs — i.e. when the operator actually set it.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct ImageGenConfigShadow {
    default_model: String,
    session_cap: u32,
    timeout_secs: u64,
    t2i: GenModeConfig,
    steps: u32,
    safe_mode: bool,
}

impl Default for ImageGenConfigShadow {
    fn default() -> Self {
        let d = ImageGenConfig::default();
        Self {
            default_model: d.default_model,
            session_cap: d.session_cap,
            timeout_secs: d.timeout_secs,
            t2i: d.t2i,
            steps: d.steps,
            safe_mode: d.safe_mode,
        }
    }
}

impl From<ImageGenConfigShadow> for ImageGenConfig {
    fn from(shadow: ImageGenConfigShadow) -> Self {
        let mut t2i = shadow.t2i;
        // D-01/D-02: the legacy flat key was explicitly overridden away from the
        // shipped fal default -> map it into t2i.model and force t2i.provider to
        // None (even though the field-level default filled it with the venice
        // default when the `t2i:` key itself was absent) so a fal-ai/* legacy
        // model keeps routing to fal via prefix inference rather than being
        // silently flipped to venice.
        if shadow.default_model != LEGACY_IMAGE_DEFAULT_MODEL {
            t2i.model = shadow.default_model.clone();
            t2i.provider = None;
        }
        Self {
            default_model: shadow.default_model,
            session_cap: shadow.session_cap,
            timeout_secs: shadow.timeout_secs,
            t2i,
            steps: shadow.steps,
            safe_mode: shadow.safe_mode,
        }
    }
}

#[cfg(test)]
mod image_gen_config_tests {
    //! Phase 01 Task 1 — lock the `ImageGenConfig` defaults and serde-default
    //! deserialization shape (old configs with no `image_gen:` section must parse).
    //! Extended Phase 47 Task 2 — t2i mode struct + legacy back-compat mapping.
    use super::*;

    /// `ImageGenConfig::default()` yields the D-07/D-08/D-03/D-05 defaults, plus
    /// the Phase 47 venice `t2i` default (D-01/D-03).
    #[test]
    fn image_gen_config_defaults() {
        let cfg = ImageGenConfig::default();
        assert_eq!(cfg.default_model, "fal-ai/flux/schnell");
        assert_eq!(cfg.session_cap, 20);
        assert_eq!(cfg.timeout_secs, 120);
        assert_eq!(cfg.t2i.provider.as_deref(), Some("venice"));
        assert_eq!(cfg.t2i.model, "flux-2-pro");
        // fix(47): steps MUST default to a real denoise count (blob fix), and
        // safe_mode defaults off for a single-user host.
        assert_eq!(cfg.steps, 20);
        assert!(!cfg.safe_mode);
    }

    /// fix(47): `steps` / `safe_mode` are operator-tunable via `image_gen:` and
    /// override the defaults, while the pre-Phase-47 keys stay at their defaults.
    #[test]
    fn config_image_gen_steps_and_safe_mode_override() {
        let yaml = r#"
image_gen:
  steps: 40
  safe_mode: true
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.steps, 40);
        assert!(config.image_gen.safe_mode);
        // untouched keys keep their defaults via the struct-level serde default
        assert_eq!(config.image_gen.session_cap, 20);
        assert_eq!(config.image_gen.t2i.model, "flux-2-pro");
    }

    /// A pre-fix(47) `image_gen:` block (no `steps`/`safe_mode` keys) still parses
    /// and backfills the new fields with the blob-fix defaults.
    #[test]
    fn config_pre_fix47_image_gen_backfills_steps_default() {
        let yaml = r#"
image_gen:
  session_cap: 10
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.session_cap, 10);
        assert_eq!(config.image_gen.steps, 20);
        assert!(!config.image_gen.safe_mode);
    }

    /// A `config.yaml` with NO `image_gen:` section still deserializes, and
    /// `Config.image_gen` falls back to the defaults (serde default round-trip),
    /// including the venice `t2i` default (Phase 47 must-have: "no
    /// image_gen/video_gen/generation section deserializes and yields venice
    /// defaults for all four modes").
    #[test]
    fn config_without_image_gen_section_uses_defaults() {
        let yaml = r#"
model:
  default: "claude-sonnet-4"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.default_model, "fal-ai/flux/schnell");
        assert_eq!(config.image_gen.session_cap, 20);
        assert_eq!(config.image_gen.timeout_secs, 120);
        assert_eq!(config.image_gen.t2i.provider.as_deref(), Some("venice"));
        assert_eq!(config.image_gen.t2i.model, "flux-2-pro");
    }

    /// An explicit `image_gen:` section overrides the per-call model while leaving
    /// the other config-only keys at their defaults via the struct-level
    /// `#[serde(default)]`.
    #[test]
    fn config_with_partial_image_gen_section_overrides_model() {
        let yaml = r#"
image_gen:
  default_model: "fal-ai/flux/dev"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.default_model, "fal-ai/flux/dev");
        assert_eq!(config.image_gen.session_cap, 20);
        assert_eq!(config.image_gen.timeout_secs, 120);
    }

    /// D-01/D-02 back-compat: a legacy `image_gen.default_model` set to a
    /// non-default fal model maps into `t2i.model`, and `t2i.provider` stays
    /// `None` so Plan 04's resolver infers fal from the `fal-ai/` prefix — the
    /// venice default flip never overrides an existing fal.ai operator's model.
    #[test]
    fn legacy_default_model_maps_to_t2i_model_provider_unset() {
        let yaml = r#"
image_gen:
  default_model: "fal-ai/flux/dev"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.t2i.model, "fal-ai/flux/dev");
        assert_eq!(config.image_gen.t2i.provider, None);
    }
}

// =============================================================================
// VideoGenConfig (Phase 36.3.3 D-12)
// =============================================================================

/// Phase 36.3.3 (D-12); extended Phase 47 (D-01/D-02/D-03/D-12/UI-SPEC): video
/// generation configuration.
///
/// `#[serde(default)]` on both the struct and the field site on [`Config`] ensures
/// pre-36.3.3 configs (with no `video_gen:` section) parse cleanly with defaults.
///
/// - `default_t2v_model` (default `"fal-ai/ltx-2.3/text-to-video"`, D-10): **legacy
///   flat key**, kept for back-compat (Phase 47 D-01). An explicit override maps
///   into `t2v.model` via [`VideoGenConfigShadow`], leaving `t2v.provider` unset
///   so a legacy fal model keeps routing to fal (D-02).
/// - `default_i2v_model` (default `"fal-ai/ltx-2.3/image-to-video"`, D-10): same
///   back-compat mapping into `i2v.model`.
/// - `session_cap` (default 5, D-06): per-chat-session hard cap on video generations.
///   Lower than image-gen (20) because video is a paid generation. Config-only.
///   Governs DIRECT chat-session generations only — see
///   [`GenerationGuardrailsConfig`] for the D-08 reconciliation.
/// - `timeout_secs` (default 300, D-04): polling timeout budget for a single fal
///   video queue job (5 min). Config-only.
/// - `max_inline_bytes` (default 50MB, D-07): maximum file size for inline video
///   delivery. Matches Telegram `sendVideo` ceiling.
/// - `default_duration_secs` (default 6, D-11): default clip duration in seconds.
///   LTX-2.3 minimum is 6s. Config-only.
/// - `t2v` / `i2v` / `v2v` (Phase 47 D-01/D-03): per-mode `{provider, model}`.
///   `v2v` is net-new (no legacy key) — the `video_to_video` tool (D-14).
/// - `resolution` (Phase 47 D-12, default `"720p"`): fixed-from-config, NOT
///   LLM-tunable — carries the 36.3.3 D-11 deferral forward. Must satisfy the
///   configured model's `model_spec.constraints.resolutions` (Plan 06 D-13).
/// - `aspect_ratio` (Phase 47 D-12, default `"16:9"`): same fixed-from-config
///   contract as `resolution`.
/// - `progress_ping_secs` (Phase 47 UI-SPEC, default 30): single integer —
///   `0`/absent = off (today's single-ack behavior), non-zero = the periodic
///   "Still working on your video…" ping cadence during the async poll,
///   backfilling 36.3.3's never-shipped D-05.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "VideoGenConfigShadow")]
pub struct VideoGenConfig {
    /// Legacy T2V flat model id (D-10). LLM-overridable per call. Kept for
    /// back-compat (Phase 47 D-01) — see struct doc for the reconciliation.
    pub default_t2v_model: String,
    /// Legacy I2V flat model id (D-10). LLM-overridable per call. Kept for
    /// back-compat (Phase 47 D-01) — see struct doc for the reconciliation.
    pub default_i2v_model: String,
    /// Per-session video generation cap (D-06). Config-only — NOT LLM-overridable.
    pub session_cap: u32,
    /// Polling timeout in seconds (D-04). Config-only — NOT LLM-overridable.
    pub timeout_secs: u64,
    /// Maximum inline delivery size in bytes (D-07). Default 50MB (Telegram sendVideo cap).
    pub max_inline_bytes: u64,
    /// Default clip duration in seconds (D-11). LTX-2.3 minimum is 6s.
    pub default_duration_secs: u32,
    /// Per-mode `{provider, model}` for text-to-video (Phase 47 D-01/D-03).
    pub t2v: GenModeConfig,
    /// Per-mode `{provider, model}` for image-to-video (Phase 47 D-01/D-03).
    pub i2v: GenModeConfig,
    /// Per-mode `{provider, model}` for video-to-video (Phase 47 D-01/D-03/D-14 — net-new).
    pub v2v: GenModeConfig,
    /// Fixed-from-config video resolution (Phase 47 D-12). Default `"720p"`.
    pub resolution: String,
    /// Fixed-from-config video aspect ratio (Phase 47 D-12). Default `"16:9"`.
    pub aspect_ratio: String,
    /// Periodic progress-ping cadence in seconds during the async poll (Phase 47
    /// UI-SPEC). `0` = off (today's single-ack behavior). Default 30.
    pub progress_ping_secs: u64,
}

impl Default for VideoGenConfig {
    fn default() -> Self {
        Self {
            default_t2v_model: "fal-ai/ltx-2.3/text-to-video".to_string(),
            default_i2v_model: "fal-ai/ltx-2.3/image-to-video".to_string(),
            // D-06: paid, lower than image-gen's 20
            session_cap: 5,
            // D-04: 5 min — do NOT copy image-gen's 120
            timeout_secs: 300,
            // D-07: 50MB Telegram sendVideo cap
            max_inline_bytes: 50 * 1024 * 1024,
            // D-11: LTX-2.3 minimum
            default_duration_secs: 6,
            // Phase 47 D-01/D-03: venice per-mode defaults
            t2v: default_t2v_mode(),
            i2v: default_i2v_mode(),
            v2v: default_v2v_mode(),
            // Phase 47 D-12: wan-2-7 family valid resolution/aspect defaults
            resolution: "720p".to_string(),
            aspect_ratio: "16:9".to_string(),
            // Phase 47 UI-SPEC: ~30s cadence, 0/absent = off
            progress_ping_secs: 30,
        }
    }
}

/// Phase 47 (D-01/D-02): the exact legacy default `video_gen.default_t2v_model` /
/// `default_i2v_model` values shipped since Phase 36.3.3 — used ONLY to detect an
/// explicit operator override (see [`VideoGenConfigShadow`]'s `From` impl).
const LEGACY_T2V_DEFAULT_MODEL: &str = "fal-ai/ltx-2.3/text-to-video";
const LEGACY_I2V_DEFAULT_MODEL: &str = "fal-ai/ltx-2.3/image-to-video";

/// Phase 47 (D-01/D-02): deserialize-time shadow of [`VideoGenConfig`] that
/// performs the legacy-flat-key-to-mode-struct reconciliation for `t2v`/`i2v`
/// (mirrors [`ImageGenConfigShadow`] — see its doc for the rationale). `v2v` has
/// no legacy key (net-new mode, D-14) so it never needs reconciliation.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct VideoGenConfigShadow {
    default_t2v_model: String,
    default_i2v_model: String,
    session_cap: u32,
    timeout_secs: u64,
    max_inline_bytes: u64,
    default_duration_secs: u32,
    t2v: GenModeConfig,
    i2v: GenModeConfig,
    v2v: GenModeConfig,
    resolution: String,
    aspect_ratio: String,
    progress_ping_secs: u64,
}

impl Default for VideoGenConfigShadow {
    fn default() -> Self {
        let d = VideoGenConfig::default();
        Self {
            default_t2v_model: d.default_t2v_model,
            default_i2v_model: d.default_i2v_model,
            session_cap: d.session_cap,
            timeout_secs: d.timeout_secs,
            max_inline_bytes: d.max_inline_bytes,
            default_duration_secs: d.default_duration_secs,
            t2v: d.t2v,
            i2v: d.i2v,
            v2v: d.v2v,
            resolution: d.resolution,
            aspect_ratio: d.aspect_ratio,
            progress_ping_secs: d.progress_ping_secs,
        }
    }
}

impl From<VideoGenConfigShadow> for VideoGenConfig {
    fn from(shadow: VideoGenConfigShadow) -> Self {
        let mut t2v = shadow.t2v;
        let mut i2v = shadow.i2v;
        // D-01/D-02: legacy flat keys explicitly overridden -> map into the
        // corresponding mode's model and force provider to None (even though the
        // field-level default filled it with the venice default when the mode
        // key itself was absent) so a legacy fal model keeps routing to fal via
        // prefix inference rather than being silently flipped to venice.
        if shadow.default_t2v_model != LEGACY_T2V_DEFAULT_MODEL {
            t2v.model = shadow.default_t2v_model.clone();
            t2v.provider = None;
        }
        if shadow.default_i2v_model != LEGACY_I2V_DEFAULT_MODEL {
            i2v.model = shadow.default_i2v_model.clone();
            i2v.provider = None;
        }
        Self {
            default_t2v_model: shadow.default_t2v_model,
            default_i2v_model: shadow.default_i2v_model,
            session_cap: shadow.session_cap,
            timeout_secs: shadow.timeout_secs,
            max_inline_bytes: shadow.max_inline_bytes,
            default_duration_secs: shadow.default_duration_secs,
            t2v,
            i2v,
            v2v: shadow.v2v,
            resolution: shadow.resolution,
            aspect_ratio: shadow.aspect_ratio,
            progress_ping_secs: shadow.progress_ping_secs,
        }
    }
}

#[cfg(test)]
mod video_gen_config_tests {
    //! Phase 36.3.3 Task 1 — lock the `VideoGenConfig` defaults and serde-default
    //! deserialization shape (old configs with no `video_gen:` section must parse).
    //! Extended Phase 47 Task 2 — t2v/i2v/v2v mode structs, resolution/aspect_ratio/
    //! progress_ping_secs, and legacy back-compat mapping.
    use super::*;

    /// `VideoGenConfig::default()` yields the D-06/D-04/D-07/D-11 defaults, plus
    /// the Phase 47 venice per-mode + video-param defaults.
    #[test]
    fn video_gen_config_defaults() {
        let cfg = VideoGenConfig::default();
        assert_eq!(cfg.default_t2v_model, "fal-ai/ltx-2.3/text-to-video");
        assert_eq!(cfg.default_i2v_model, "fal-ai/ltx-2.3/image-to-video");
        assert_eq!(cfg.session_cap, 5);
        assert_eq!(cfg.timeout_secs, 300);
        assert_eq!(cfg.max_inline_bytes, 50 * 1024 * 1024);
        assert_eq!(cfg.default_duration_secs, 6);
        assert_eq!(cfg.t2v.provider.as_deref(), Some("venice"));
        assert_eq!(cfg.t2v.model, "wan-2-7-text-to-video");
        assert_eq!(cfg.i2v.provider.as_deref(), Some("venice"));
        assert_eq!(cfg.i2v.model, "wan-2-7-image-to-video");
        assert_eq!(cfg.v2v.provider.as_deref(), Some("venice"));
        assert_eq!(cfg.v2v.model, "wan-2-7-video-to-video");
        assert_eq!(cfg.resolution, "720p");
        assert_eq!(cfg.aspect_ratio, "16:9");
        assert_eq!(cfg.progress_ping_secs, 30);
    }

    /// A `config.yaml` with NO `video_gen:` section still deserializes, and
    /// `Config.video_gen` falls back to the defaults (serde default round-trip),
    /// including venice defaults for all three video modes (Phase 47 must-have).
    #[test]
    fn config_without_video_gen_section_uses_defaults() {
        let yaml = r#"
model:
  default: "claude-sonnet-4"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(
            config.video_gen.default_t2v_model,
            "fal-ai/ltx-2.3/text-to-video"
        );
        assert_eq!(
            config.video_gen.default_i2v_model,
            "fal-ai/ltx-2.3/image-to-video"
        );
        assert_eq!(config.video_gen.session_cap, 5);
        assert_eq!(config.video_gen.timeout_secs, 300);
        assert_eq!(config.video_gen.max_inline_bytes, 50 * 1024 * 1024);
        assert_eq!(config.video_gen.default_duration_secs, 6);
        assert_eq!(config.video_gen.t2v.provider.as_deref(), Some("venice"));
        assert_eq!(config.video_gen.t2v.model, "wan-2-7-text-to-video");
        assert_eq!(config.video_gen.i2v.provider.as_deref(), Some("venice"));
        assert_eq!(config.video_gen.i2v.model, "wan-2-7-image-to-video");
        assert_eq!(config.video_gen.v2v.provider.as_deref(), Some("venice"));
        assert_eq!(config.video_gen.v2v.model, "wan-2-7-video-to-video");
        assert_eq!(config.video_gen.resolution, "720p");
        assert_eq!(config.video_gen.aspect_ratio, "16:9");
        assert_eq!(config.video_gen.progress_ping_secs, 30);
    }

    /// An explicit `video_gen:` section overrides only `session_cap` while leaving
    /// the other fields at their defaults via the struct-level `#[serde(default)]`.
    #[test]
    fn config_with_partial_video_gen_section_overrides_only_set_field() {
        let yaml = r#"
video_gen:
  session_cap: 3
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.video_gen.session_cap, 3);
        assert_eq!(
            config.video_gen.default_t2v_model,
            "fal-ai/ltx-2.3/text-to-video"
        );
        assert_eq!(config.video_gen.timeout_secs, 300);
        assert_eq!(config.video_gen.max_inline_bytes, 50 * 1024 * 1024);
        assert_eq!(config.video_gen.default_duration_secs, 6);
    }

    /// D-01/D-02 back-compat: legacy `default_t2v_model`/`default_i2v_model`
    /// overrides map into `t2v.model`/`i2v.model` respectively, with `provider`
    /// left `None` so Plan 04's resolver infers fal from the prefix.
    #[test]
    fn legacy_flat_video_models_map_to_mode_structs_provider_unset() {
        let yaml = r#"
video_gen:
  default_t2v_model: "fal-ai/ltx-2.3/text-to-video-custom"
  default_i2v_model: "fal-ai/ltx-2.3/image-to-video-custom"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(
            config.video_gen.t2v.model,
            "fal-ai/ltx-2.3/text-to-video-custom"
        );
        assert_eq!(config.video_gen.t2v.provider, None);
        assert_eq!(
            config.video_gen.i2v.model,
            "fal-ai/ltx-2.3/image-to-video-custom"
        );
        assert_eq!(config.video_gen.i2v.provider, None);
        // v2v has no legacy key — stays at the venice default (D-14 net-new mode).
        assert_eq!(config.video_gen.v2v.model, "wan-2-7-video-to-video");
        assert_eq!(config.video_gen.v2v.provider.as_deref(), Some("venice"));
    }
}

#[cfg(test)]
mod generation_mode_config {
    //! Phase 47 Task 2 — GEN-03 adjacency: each mode's `{provider, model}`
    //! resolves independently. Setting one mode's fields must never alter any
    //! other mode's config, across image_gen AND video_gen.
    use super::*;

    /// Setting only `video_gen.t2v.model` leaves i2v/v2v (same section) AND
    /// `image_gen.t2i` (different section) at their venice defaults.
    #[test]
    fn setting_only_t2v_model_leaves_i2v_v2v_and_image_t2i_at_defaults() {
        let yaml = r#"
video_gen:
  t2v:
    model: "wan-2-7-text-to-video-custom"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.video_gen.t2v.model, "wan-2-7-text-to-video-custom");
        // i2v/v2v untouched
        assert_eq!(config.video_gen.i2v.model, "wan-2-7-image-to-video");
        assert_eq!(config.video_gen.i2v.provider.as_deref(), Some("venice"));
        assert_eq!(config.video_gen.v2v.model, "wan-2-7-video-to-video");
        assert_eq!(config.video_gen.v2v.provider.as_deref(), Some("venice"));
        // image_gen.t2i untouched (different config section entirely)
        assert_eq!(config.image_gen.t2i.model, "flux-2-pro");
        assert_eq!(config.image_gen.t2i.provider.as_deref(), Some("venice"));
    }

    /// Setting a full `image_gen.t2i` block (both provider + model, explicit
    /// override to fal) leaves all video modes untouched (different config
    /// section entirely — GEN-03 adjacency).
    #[test]
    fn setting_full_t2i_block_leaves_video_modes_at_defaults() {
        let yaml = r#"
image_gen:
  t2i:
    provider: "fal"
    model: "fal-ai/flux/dev"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.t2i.provider.as_deref(), Some("fal"));
        assert_eq!(config.image_gen.t2i.model, "fal-ai/flux/dev");
        assert_eq!(config.video_gen.t2v.model, "wan-2-7-text-to-video");
        assert_eq!(config.video_gen.i2v.model, "wan-2-7-image-to-video");
        assert_eq!(config.video_gen.v2v.model, "wan-2-7-video-to-video");
    }

    /// An explicit `provider` field wins over prefix inference regardless of the
    /// model string's own prefix (D-02 ordering — explicit wins, inference is the
    /// fallback only).
    #[test]
    fn explicit_provider_present_regardless_of_model_prefix() {
        let yaml = r#"
image_gen:
  t2i:
    provider: "venice"
    model: "fal-ai/flux/dev"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        assert_eq!(config.image_gen.t2i.provider.as_deref(), Some("venice"));
        assert_eq!(config.image_gen.t2i.model, "fal-ai/flux/dev");
    }
}

// =============================================================================
// GenerationConfig / GenerationGuardrailsConfig / GenerationSurfaces
// (Phase 47 D-07/D-08): central cross-surface generation spend-policy block
// =============================================================================

/// Phase 47 (D-07): top-level `generation:` config block — the central home for
/// cross-surface generation spend policy (currently just `guardrails`).
///
/// `#[serde(default)]` on both the struct and the `Config.generation` field site
/// ensures pre-Phase-47 configs (no `generation:` section) parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GenerationConfig {
    /// Central spend-policy block — see [`GenerationGuardrailsConfig`] for the
    /// full D-07/D-08 semantics.
    pub guardrails: GenerationGuardrailsConfig,
}

/// Phase 47 (D-07/D-08): central spend-policy block consulted by every
/// generation-capable surface (chat, kanban regular + goal-mode, delegate
/// children).
///
/// **D-08 reconciliation (read before touching Plan 05/08):** this block does
/// **NOT** govern top-level/direct chat-session generations — those remain
/// governed EXCLUSIVELY by the existing per-section `image_gen.session_cap`
/// (default 20) / `video_gen.session_cap` (default 5), exactly as they do today.
/// A normal chat session may generate up to its `session_cap`, **NEVER**
/// `per_child_cap` (default 3) — chat is not subject to the per-child tier at
/// all; `surfaces.chat: true` means "chat may generate" (today's default-on
/// behavior) and never imposes `per_child_cap` on chat.
///
/// The `per_child_cap` + `session_pool` tiers apply **ONLY** to delegate/kanban
/// **descendants** (children spawned via `delegate_task`, or kanban worker tasks —
/// regular or goal-mode). A descendant generation decrements **BOTH** its own
/// `per_child_cap` allowance **AND** the shared `session_pool` — the pool bounds
/// the aggregate across all descendants of a root regardless of how many children
/// spawn, and `per_child_cap` bounds any single child from draining the whole
/// pool alone. Accounting is cross-process-safe by surface: in-process delegate
/// children share the parent's `Arc` counter (Plan 05); kanban swarms (separate
/// OS processes) account via `kanban.db`, keyed by the root task id (or the
/// worker's own task id for a solitary non-swarm task) (Plan 03/08).
///
/// `#[serde(default)]` on both the struct and every field ensures a partial
/// `generation.guardrails:` block merges cleanly over the D-07 default matrix.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationGuardrailsConfig {
    /// Shared aggregate cap across ALL delegate/kanban descendants of a given
    /// root (D-05/D-07). Default: 20. `0` = block descendant generations
    /// immediately (Plan 05 boundary case — representable, not just clamped).
    pub session_pool: u32,
    /// Per-child sub-cap — bounds any single delegate/kanban descendant from
    /// draining the whole `session_pool` alone (D-05/D-07). Default: 3. `0` =
    /// block immediately (Plan 05 boundary case).
    pub per_child_cap: u32,
    /// Per-surface enable map (D-07/D-09/D-11) — a `false` surface never
    /// registers its generation tools (the wall stays up at registration time,
    /// not just at call time).
    pub surfaces: GenerationSurfaces,
}

impl Default for GenerationGuardrailsConfig {
    fn default() -> Self {
        Self {
            session_pool: 20,
            per_child_cap: 3,
            surfaces: GenerationSurfaces::default(),
        }
    }
}

/// Phase 47 (D-07): per-surface generation enable map.
///
/// `#[serde(default)]` on both the struct and the field site ensures a PARTIAL
/// `surfaces:` map (e.g. only `delegate: true` set) merges over the defaults
/// below rather than replacing the whole map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GenerationSurfaces {
    /// Direct chat-session generation (works today). Default: true. Never
    /// subject to `per_child_cap` — see [`GenerationGuardrailsConfig`] doc.
    pub chat: bool,
    /// Regular (non-goal-mode) kanban worker generation — do NOT regress 46.5
    /// kanban image tasks. Default: true.
    pub kanban: bool,
    /// Kanban GOAL-MODE worker generation. Opt-in. Default: false.
    pub kanban_goal_mode: bool,
    /// `delegate_task` child generation (the `"generation"` toolset group,
    /// D-09). Opt-in. Default: false.
    pub delegate: bool,
}

impl Default for GenerationSurfaces {
    fn default() -> Self {
        Self {
            chat: true,
            kanban: true,
            kanban_goal_mode: false,
            delegate: false,
        }
    }
}

#[cfg(test)]
mod generation_config_tests {
    //! Phase 47 Task 3 — lock the `generation.guardrails` D-07 default matrix
    //! and the D-08 doc-comment contract.
    use super::*;

    /// Absent `generation:` section deserializes to the exact D-07 default
    /// matrix: session_pool=20, per_child_cap=3, surfaces
    /// {chat:true, kanban:true, kanban_goal_mode:false, delegate:false}.
    #[test]
    fn absent_generation_section_yields_d07_default_matrix() {
        let yaml = r#"
model:
  default: "claude-sonnet-4"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let g = &config.generation.guardrails;
        assert_eq!(g.session_pool, 20);
        assert_eq!(g.per_child_cap, 3);
        assert!(g.surfaces.chat);
        assert!(g.surfaces.kanban);
        assert!(!g.surfaces.kanban_goal_mode);
        assert!(!g.surfaces.delegate);
    }

    /// `GenerationConfig::default()` matches the same D-07 matrix directly
    /// (not just via serde round-trip).
    #[test]
    fn generation_config_default_matches_d07_matrix() {
        let g = GenerationConfig::default().guardrails;
        assert_eq!(g.session_pool, 20);
        assert_eq!(g.per_child_cap, 3);
        assert!(g.surfaces.chat);
        assert!(g.surfaces.kanban);
        assert!(!g.surfaces.kanban_goal_mode);
        assert!(!g.surfaces.delegate);
    }

    /// A partial `generation.guardrails.surfaces` map merges over defaults —
    /// setting `delegate: true` leaves the other three surfaces at default.
    #[test]
    fn partial_surfaces_map_merges_over_defaults() {
        let yaml = r#"
generation:
  guardrails:
    surfaces:
      delegate: true
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let g = &config.generation.guardrails;
        assert!(g.surfaces.delegate);
        assert!(g.surfaces.chat);
        assert!(g.surfaces.kanban);
        assert!(!g.surfaces.kanban_goal_mode);
        // session_pool/per_child_cap untouched by a surfaces-only override
        assert_eq!(g.session_pool, 20);
        assert_eq!(g.per_child_cap, 3);
    }

    /// `session_pool` and `per_child_cap` set to 0 are representable (boundary —
    /// Plan 05 treats 0 as "block immediately", not an invalid/clamped value).
    #[test]
    fn zero_caps_are_representable_boundary_values() {
        let yaml = r#"
generation:
  guardrails:
    session_pool: 0
    per_child_cap: 0
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let g = &config.generation.guardrails;
        assert_eq!(g.session_pool, 0);
        assert_eq!(g.per_child_cap, 0);
    }
}

// =============================================================================
// DashboardConfig + DashboardKanbanConfig (Phase 36.3.7.11 D-17)
// =============================================================================

/// Phase 36.3.7.11 (D-17): dashboard configuration block.
///
/// Surfaced in `config.yaml` as a top-level `dashboard:` block with a
/// `kanban:` sub-block. `#[serde(default)]` on the field site in
/// [`Config`] makes pre-36.3.7.11 configs parse cleanly with the
/// defaults applied.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DashboardConfig {
    pub kanban: DashboardKanbanConfig,
}

/// Phase 36.3.7.11 (D-17): kanban-specific dashboard configuration.
///
/// `tail_interval_ms` controls the tail consumer's polling interval in
/// milliseconds. Default 250 ms (sub-second perceived latency). The
/// tail loop is spawned in `AppState::init` at this cadence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardKanbanConfig {
    /// Tail consumer polling interval in milliseconds. Default: 250 ms.
    pub tail_interval_ms: u64,
}

impl Default for DashboardKanbanConfig {
    fn default() -> Self {
        Self {
            tail_interval_ms: 250,
        }
    }
}

#[cfg(test)]
mod dashboard_config_tests {
    use super::*;

    /// Phase 36.3.7.11 (D-17): YAML without `dashboard:` key parses with
    /// the canonical default `tail_interval_ms = 250`.
    #[test]
    fn dashboard_default_tail_interval_is_250() {
        let yaml = "model:\n  default: gpt-4\n";
        let cfg: Config = serde_yaml::from_str(yaml).expect("parse base config");
        assert_eq!(
            cfg.dashboard.kanban.tail_interval_ms, 250,
            "missing `dashboard:` key must default to 250 ms (D-17)"
        );
    }

    /// Phase 36.3.7.11 (D-17): YAML override of
    /// `dashboard.kanban.tail_interval_ms` deserializes correctly.
    #[test]
    fn dashboard_yaml_override() {
        let yaml = "dashboard:\n  kanban:\n    tail_interval_ms: 500\n";
        let cfg: Config = serde_yaml::from_str(yaml).expect("parse override config");
        assert_eq!(cfg.dashboard.kanban.tail_interval_ms, 500);
    }
}

// =============================================================================
// PromptCachingConfig + CacheTtl (Phase 36.2 D-CACHE-02)
// =============================================================================

/// Phase 36.2 (D-CACHE-02): prompt caching configuration.
///
/// Surfaced in `config.yaml` as a top-level `prompt_caching:` block with two
/// fields: `ttl` (`"5m"` | `"1h"`, default `"1h"`) and `enabled` (default `true`).
/// `#[serde(default)]` on the field site in [`Config`] makes pre-36.2 configs
/// parse cleanly with the defaults applied.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptCachingConfig {
    /// Cache TTL for Anthropic `cache_control` markers. Only `"5m"` and `"1h"`
    /// are valid — see [`CacheTtl`]. Default: `"1h"` (D-CACHE-02).
    pub ttl: CacheTtl,
    /// Enable prompt caching for Anthropic models. When `false`, no
    /// `cache_control` markers are attached to the request body.
    /// Default: `true` (D-CACHE-02).
    pub enabled: bool,
}

impl Default for PromptCachingConfig {
    fn default() -> Self {
        Self {
            ttl: CacheTtl::OneHour,
            enabled: true,
        }
    }
}

/// Phase 36.2 (D-CACHE-02): closed enum for Anthropic `cache_control.ttl`.
///
/// Anthropic's `cache_control` envelope accepts exactly two TTL string values:
/// `"5m"` and `"1h"`. Modeling this as a closed enum with `#[serde(rename = ...)]`
/// rejects any other value at deserialization time (T-36.2-05-CFG mitigation —
/// no string interpolation into the request body).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CacheTtl {
    /// `"5m"` — short TTL for rolling breakpoints during burst conversations.
    #[serde(rename = "5m")]
    FiveMinutes,
    /// `"1h"` — long TTL for cross-session cached prefixes (D-CACHE-02 default).
    #[default]
    #[serde(rename = "1h")]
    OneHour,
}

impl CacheTtl {
    /// Return the string form used in the Anthropic `cache_control` JSON
    /// envelope (`{"type":"ephemeral","ttl":"1h"}` or `"5m"`).
    pub fn as_anthropic_ttl(&self) -> &'static str {
        match self {
            CacheTtl::FiveMinutes => "5m",
            CacheTtl::OneHour => "1h",
        }
    }
}

// =============================================================================
// TtsConfig + sub-structs (Phase 36.17.5 D-12)
// =============================================================================

/// Phase 36.17.5 (D-12): Edge TTS provider configuration.
///
/// All fields have `#[serde(default)]` so pre-36.17.5 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EdgeTtsConfig {
    /// Edge TTS voice name (default: `"en-US-AriaNeural"`).
    pub voice: String,
    /// Audio output format returned by Edge TTS (default: `"mp3"`).
    pub output_format: String,
}

impl Default for EdgeTtsConfig {
    fn default() -> Self {
        Self {
            voice: "en-US-AriaNeural".to_string(),
            output_format: "mp3".to_string(),
        }
    }
}

/// Phase 36.17.5 (D-12): ElevenLabs TTS provider configuration.
///
/// All fields have `#[serde(default)]` so pre-36.17.5 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ElevenLabsConfig {
    /// ElevenLabs voice ID (default: `"pNInz6obpgDQGcFmaJgB"` — Adam voice).
    pub voice_id: String,
    /// ElevenLabs model ID (default: `"eleven_multilingual_v2"`).
    pub model_id: String,
    /// Audio output format (default: `"mp3"`).
    ///
    /// [NOTE] Opus container handling deferred — RESEARCH Open Q #2 / D-04 ElevenLabs Opus path.
    /// ElevenLabs added `opus_48000_*` output formats 2025-03-31; enabling native Opus for
    /// Telegram voice bubbles without ffmpeg is a follow-up task.
    pub output_format: String,
}

impl Default for ElevenLabsConfig {
    fn default() -> Self {
        Self {
            voice_id: "pNInz6obpgDQGcFmaJgB".to_string(), // Adam
            model_id: "eleven_multilingual_v2".to_string(),
            output_format: "mp3".to_string(), // Opus opt-in deferred (RESEARCH Open Q #2)
        }
    }
}

/// Phase 40.5 (D-10): OpenAI TTS provider configuration.
///
/// Mirrors [`ElevenLabsConfig`] shape exactly: model + voice + format.
/// All fields have `#[serde(default)]` so pre-40.5 YAML configs parse cleanly.
///
/// The OpenAI TTS provider implementation (HTTP streaming via the
/// `speech` endpoint) ships in Plan 03; this struct is the config-side
/// foundation that provider reads. No API key is stored here —
/// `OPENAI_API_KEY` stays env-var server-side (T-40.5-01-02).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAiTtsConfig {
    /// OpenAI TTS model (default: `"tts-1"`).
    pub model: String,
    /// OpenAI TTS voice (default: `"alloy"`).
    pub voice: String,
    /// Audio output format (default: `"mp3"`).
    pub format: String,
}

impl Default for OpenAiTtsConfig {
    fn default() -> Self {
        Self {
            model: "tts-1".to_string(),
            voice: "alloy".to_string(),
            format: "mp3".to_string(),
        }
    }
}

// =============================================================================
// Phase 40.5 identity schema (D-08/D-09/D-11/D-13/D-16/D-17)
// =============================================================================

/// Phase 40.5 (D-17/D-02): Per-identity appearance knobs for orb-type identities.
///
/// All fields are `Option<T>` so `None` means "inherit the orb-preset registry
/// default" (D-11 partial-override model). `#[serde(default)]` ensures pre-40.5
/// YAML without these keys parses cleanly (T-40.5-01-01).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IdentityAppearance {
    /// Orb render style: `"classic"` | `"bloom"` | `"ascii"` | `"network"`.
    /// `None` = inherit registry default.
    pub style: Option<String>,
    /// Idle base hue 0–360. Listening/thinking/speaking shift relative to it
    /// (D-05 per-state feedback). `None` = inherit registry default.
    pub base_hue: Option<u16>,
    /// Scale factor 0.5–2.0. `None` = inherit registry default.
    pub size: Option<f32>,
    /// Glow intensity 0.0–1.0. `None` = inherit registry default.
    pub glow: Option<f32>,
}

/// Phase 40.5 (D-08/D-09/D-11): Per-identity voice overrides.
///
/// `None` fields inherit the global `tts.provider` / voice setting (D-11).
///
/// **D-08 scope fence:** no `llm`, `model`, or `stt` field — voice only.
/// An identity is a communication path (free-mode TTS + realtime voice),
/// not a separate agent. The LLM and STT stay global.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IdentityVoiceProfile {
    /// Override TTS provider name for free-mode turns (D-10).
    /// Legal values: `"edge"` | `"openai"` | `"elevenlabs"`. `None` = inherit global.
    pub free_mode_tts_provider: Option<String>,
    /// Override TTS voice for free-mode turns. Interpreted against the active
    /// provider's voice catalog. `None` = inherit global.
    pub free_mode_tts_voice: Option<String>,
    /// Override OpenAI Realtime API voice (D-09). `None` = inherit global.
    pub realtime_voice: Option<String>,
}

/// Phase 40.5 (D-17): One identity record — appearance (orb) + voice (free/realtime).
///
/// A single record holds BOTH the visual knobs (for orb-type identities) and
/// the voice overrides. The active-identity pointer lives in `AvatarPrefs.active_identity`
/// in localStorage; the records themselves live here in `config.yaml` (D-16).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IdentityRecord {
    /// Human-readable display name (optional, shown in the "applies to" selector).
    pub display_name: Option<String>,
    /// Orb visual knobs. `None` for non-orb / head identities (no appearance override).
    pub appearance: Option<IdentityAppearance>,
    /// Voice profile overrides. `None` fields inherit global defaults (D-11).
    pub voice: IdentityVoiceProfile,
}

/// Phase 40.5 (D-13): Curated turn-key identity profiles shipped with IronHermes.
///
/// Called by the `#[serde(default = "…")]` attribute on `Config.identities` when
/// the YAML has no `identities:` section, and also by `Config::load_from`'s
/// post-parse merge loop to backfill missing shipped personas into a PARTIAL
/// operator-supplied section (D-16).
///
/// Add new shipped personas here — no other Rust change needed.
pub fn default_seed_identities() -> std::collections::HashMap<String, IdentityRecord> {
    let mut map = std::collections::HashMap::new();

    // orb_bloom — ElevenLabs Adam voice in free mode; shimmer in realtime (D-13).
    map.insert(
        "orb_bloom".to_string(),
        IdentityRecord {
            display_name: Some("Bloom".to_string()),
            appearance: Some(IdentityAppearance {
                style: Some("bloom".to_string()),
                base_hue: Some(280),
                size: Some(1.0),
                glow: Some(0.8),
            }),
            voice: IdentityVoiceProfile {
                free_mode_tts_provider: Some("elevenlabs".to_string()),
                free_mode_tts_voice: Some("pNInz6obpgDQGcFmaJgB".to_string()), // Adam
                realtime_voice: Some("shimmer".to_string()),
            },
        },
    );

    // groovy — ElevenLabs Rachel voice in free mode; nova in realtime (D-13).
    map.insert(
        "groovy".to_string(),
        IdentityRecord {
            display_name: Some("Groovy".to_string()),
            appearance: None,
            voice: IdentityVoiceProfile {
                free_mode_tts_provider: Some("elevenlabs".to_string()),
                free_mode_tts_voice: Some("21m00Tcm4TlvDq8ikWAM".to_string()), // Rachel
                realtime_voice: Some("nova".to_string()),
            },
        },
    );

    // orb_classic — inherits global (all-None voice); listed for is_known_identity coverage.
    map.insert(
        "orb_classic".to_string(),
        IdentityRecord {
            display_name: Some("Classic".to_string()),
            appearance: Some(IdentityAppearance {
                style: Some("classic".to_string()),
                base_hue: Some(186),
                size: Some(1.0),
                glow: Some(0.5),
            }),
            voice: IdentityVoiceProfile::default(), // all-None — inherits global
        },
    );

    // orb_ascii — inherits global voice.
    map.insert(
        "orb_ascii".to_string(),
        IdentityRecord {
            display_name: Some("ASCII".to_string()),
            appearance: Some(IdentityAppearance {
                style: Some("ascii".to_string()),
                base_hue: Some(120),
                size: Some(1.0),
                glow: Some(0.3),
            }),
            voice: IdentityVoiceProfile::default(),
        },
    );

    // orb_network — inherits global voice.
    map.insert(
        "orb_network".to_string(),
        IdentityRecord {
            display_name: Some("Network".to_string()),
            appearance: Some(IdentityAppearance {
                style: Some("network".to_string()),
                base_hue: Some(200),
                size: Some(1.2),
                glow: Some(0.6),
            }),
            voice: IdentityVoiceProfile::default(),
        },
    );

    // facecap — head preset; inherits global voice.
    map.insert(
        "facecap".to_string(),
        IdentityRecord {
            display_name: Some("Morph Head".to_string()),
            appearance: None,
            voice: IdentityVoiceProfile::default(),
        },
    );

    map
}

/// Phase 36.17.5 (D-12): Top-level TTS configuration block.
///
/// Strongly-typed per-provider sub-blocks — no `serde_json::Value` escape hatches
/// per D-12. Adding a new provider in a future phase = adding a new typed field here.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TtsConfig {
    /// Active TTS provider name (default: `"edge"`).
    pub provider: String,
    /// Optional path to ffmpeg binary for MP3→Opus conversion (D-04).
    /// `None` means auto-detect via `std::process::Command::new("ffmpeg")`.
    pub ffmpeg_path: Option<String>,
    /// Edge TTS provider configuration.
    pub edge: EdgeTtsConfig,
    /// ElevenLabs TTS provider configuration.
    pub elevenlabs: ElevenLabsConfig,
    /// Phase 40.5 (D-10): OpenAI TTS provider configuration.
    /// Pre-40.5 configs parse cleanly via `#[serde(default)]`.
    pub openai: OpenAiTtsConfig,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            provider: "edge".to_string(),
            ffmpeg_path: None,
            edge: EdgeTtsConfig::default(),
            elevenlabs: ElevenLabsConfig::default(),
            openai: OpenAiTtsConfig::default(),
        }
    }
}

// =============================================================================
// SttConfig + sub-structs (Phase 36.17.8 D-04 / D-05 / D-06 / D-18)
// =============================================================================

/// Phase 36.17.8 (D-05/D-18): Groq STT provider configuration.
///
/// All fields have `#[serde(default)]` so pre-36.17.8 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GroqSttConfig {
    /// Groq Whisper model name (default: `"whisper-large-v3-turbo"`).
    pub model: String,
}

impl Default for GroqSttConfig {
    fn default() -> Self {
        Self {
            model: "whisper-large-v3-turbo".to_string(),
        }
    }
}

/// Phase 36.17.8 (D-05/D-18): OpenAI STT provider configuration.
///
/// All fields have `#[serde(default)]` so pre-36.17.8 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAiSttConfig {
    /// OpenAI Whisper model name (default: `"whisper-1"`).
    pub model: String,
}

impl Default for OpenAiSttConfig {
    fn default() -> Self {
        Self {
            model: "whisper-1".to_string(),
        }
    }
}

/// Phase 36.17.8 (D-06/D-18): Top-level STT configuration block.
///
/// Strongly-typed per-provider sub-blocks — no `serde_json::Value` escape hatches
/// per D-18. Adding a new provider in a future phase = adding a new typed field here.
///
/// `provider: "auto"` means select the first available built-in at runtime
/// (D-06 key-presence selection).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SttConfig {
    /// Active STT provider name (default: `"auto"`).
    ///
    /// `"auto"` selects the first available provider at runtime (D-06).
    pub provider: String,
    /// Groq STT provider configuration.
    pub groq: GroqSttConfig,
    /// OpenAI STT provider configuration.
    pub openai: OpenAiSttConfig,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            provider: "auto".to_string(),
            groq: GroqSttConfig::default(),
            openai: OpenAiSttConfig::default(),
        }
    }
}

// =============================================================================
// VoiceConfig (Phase 36.17.8 D-08 / D-09 / D-11 / D-18)
// Phase 36.17.9 D-08 (BargeInMode) + D-10 (WakeWordConfig)
// =============================================================================

/// Phase 36.17.9 (D-08): Barge-in behavior during agent speech.
///
/// Three variants present for forward-compatibility even though only
/// `PushToInterrupt` is active in v1. `HalfDuplex` and `OpenMic` are
/// deferred (D-08 note: full-duplex paths require Wave D server work).
///
/// Serializes with `snake_case` — e.g. `push_to_interrupt`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BargeInMode {
    /// (v1 active) User must interrupt the agent by pressing push-to-talk.
    /// This is the default and the only mode wired end-to-end in v1.
    #[default]
    PushToInterrupt,
    /// (deferred, D-08) Half-duplex: server pauses TTS when mic opens.
    HalfDuplex,
    /// (deferred, D-08) Open-mic: both directions active simultaneously.
    OpenMic,
}

/// Phase 36.17.9 (D-10/D-11/D-12): Wake-word configuration.
///
/// Off by default (`enabled: false`) so existing deployments are unaffected.
/// When `enabled: true`, the voice loop sends frames with `wake_word_check: true`
/// and the server checks the STT result against `phrase` before committing a turn.
///
/// `#[serde(default)]` on the struct ensures pre-36.17.9 YAML configs parse cleanly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WakeWordConfig {
    /// Enable wake-word gating (default: false — off by default, D-10).
    pub enabled: bool,
    /// Wake phrase to match (case-insensitive contains). Default: "hey hermes".
    pub phrase: String,
}

impl Default for WakeWordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            phrase: "hey hermes".to_string(),
        }
    }
}

/// Phase 36.17.8 (D-08/D-09/D-11/D-18): Voice mode interaction configuration.
///
/// Controls recording key-binding, VAD silence detection, TTS auto-play,
/// beep feedback, and maximum recording duration. All fields are strongly typed —
/// no `serde_json::Value` escape hatches (D-18). `#[serde(default)]` ensures
/// pre-36.17.8 YAML configs parse cleanly with defaults applied.
///
/// Phase 36.17.9 additions: `barge_in_mode` (D-08) and `wake_word` (D-10).
/// Both use their own `Default` impls so no new required fields are introduced.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Key binding to start/stop recording in voice mode (default: `"ctrl+b"`).
    pub record_key: String,
    /// RMS energy threshold below which audio is considered silence (default: 200).
    pub silence_threshold: i32,
    /// Seconds of continuous silence that ends a recording (default: 3.0).
    pub silence_duration: f64,
    /// Automatically pipe agent responses to TTS after transcription (default: false).
    pub auto_tts: bool,
    /// Play an audio beep when recording starts/stops (default: true).
    pub beep_enabled: bool,
    /// Maximum allowed recording length in seconds (default: 120).
    pub max_recording_seconds: u32,
    /// Phase 36.17.9 (D-08): Barge-in behavior during agent TTS playback.
    /// Default: PushToInterrupt. HalfDuplex/OpenMic are deferred.
    pub barge_in_mode: BargeInMode,
    /// Phase 36.17.9 (D-10): Wake-word configuration.
    /// Default: disabled, phrase = "hey hermes".
    pub wake_word: WakeWordConfig,
    /// Phase 36.17.10 Plan 01 (D — web VAD): Web Audio AnalyserNode byte-domain RMS threshold
    /// below which audio is considered silence (default: 5.0). DISTINCT from
    /// `silence_threshold: i32` (native PCM amplitude scale, value 200) — these are
    /// incompatible unit scales and must never be aliased (RESEARCH pitfall 6).
    /// Used by voice_loop.rs vad_params::RMS_THRESHOLD on the web path only.
    #[serde(default = "default_web_silence_threshold_rms")]
    pub web_silence_threshold_rms: f32,
    /// Phase 36.17.12 (BUG 3 fix): OpenAI Realtime API model name for the open-mic
    /// WebRTC path. The GA production model is `"gpt-realtime"` (as of 2026).
    /// Whitelist-validated in `issue_realtime_token` before any network call (T-V5).
    /// Changing this field to an unlisted name will trigger the D-07 fallback to
    /// turn-based voice (no crash, graceful degradation).
    /// Default: `"gpt-realtime"`.
    #[serde(default = "default_realtime_model")]
    pub realtime_model: String,
    /// Phase 36.17.12: OpenAI Realtime agent voice for the open-mic WebRTC path,
    /// sent as `session.audio.output.voice` / passed to the ephemeral-token request.
    /// Server-resolved and whitelist-validated (alloy/shimmer/echo/verse/ash/ballad/
    /// coral/sage); an unlisted value triggers the D-07 fallback. Default: `"shimmer"`.
    #[serde(default = "default_realtime_voice")]
    pub realtime_voice: String,
    /// Phase 36.17.12: OpenAI Realtime input-audio transcription model. When set,
    /// the provider transcribes the USER's speech and emits
    /// `conversation.item.input_audio_transcription.completed` events, which the
    /// web UI shows in the transcript card. One of `"gpt-4o-mini-transcribe"`
    /// (default), `"gpt-4o-transcribe"`, `"whisper-1"`, or `"off"` to disable
    /// (sends `transcription: null`). Sent as `session.audio.input.transcription.model`.
    #[serde(default = "default_realtime_transcription_model")]
    pub realtime_transcription_model: String,
    /// Phase 36.17.12 (BUG 5 fix): OpenAI Realtime input noise-reduction profile,
    /// sent as `session.audio.input.noise_reduction`. Filters mic audio BEFORE VAD
    /// and the model, sharply reducing false triggers from background noise.
    /// One of `"far_field"` (laptop/built-in mic — default), `"near_field"`
    /// (headset/earbuds), or `"off"` (disable, sends null). Invalid values fall
    /// back to `"far_field"`. Default: `"far_field"`.
    #[serde(default = "default_realtime_noise_reduction")]
    pub realtime_noise_reduction: String,
    /// Phase 36.17.12 (BUG 5 fix): Realtime turn-detection (VAD) mode, sent as
    /// `session.audio.input.turn_detection.type`. `"semantic_vad"` (default —
    /// model-based, robust to background noise/humming) or `"server_vad"`
    /// (energy-based, lower latency). Invalid values fall back to `"semantic_vad"`.
    #[serde(default = "default_realtime_vad_mode")]
    pub realtime_vad_mode: String,
    /// Phase 36.17.12 (BUG 5 fix): `server_vad` activation threshold (0.0–1.0).
    /// Higher = less sensitive to quiet/background sound. Clamped to [0,1].
    /// Only applies when `realtime_vad_mode == "server_vad"`. Default: `0.5`.
    #[serde(default = "default_realtime_vad_threshold")]
    pub realtime_vad_threshold: f32,
    /// Phase 36.17.12 (BUG 5 fix): `server_vad` trailing-silence (ms) that ends a
    /// turn. Only applies when `realtime_vad_mode == "server_vad"`. Default: `500`.
    #[serde(default = "default_realtime_vad_silence_ms")]
    pub realtime_vad_silence_ms: u32,
    /// Phase 36.17.12 (BUG 5 fix): `server_vad` prefix padding (ms) of audio kept
    /// before detected speech. Only applies when `realtime_vad_mode == "server_vad"`.
    /// Default: `300`.
    #[serde(default = "default_realtime_vad_prefix_ms")]
    pub realtime_vad_prefix_ms: u32,
}

/// Serde default helper for `VoiceConfig::web_silence_threshold_rms`.
/// Needed because the struct uses `#[serde(default)]` at the struct level, but
/// f32 default is 0.0 — we need 5.0. A field-level `#[serde(default = "...")]`
/// with this helper overrides the struct-level default for this field only.
fn default_web_silence_threshold_rms() -> f32 {
    5.0
}

/// Serde default helper for `VoiceConfig::realtime_model`.
/// The GA OpenAI Realtime API model name (2026). A field-level
/// `#[serde(default = "...")]` is used so pre-36.17.12 configs that lack this
/// key parse cleanly and get the correct default (not an empty string).
fn default_realtime_model() -> String {
    "gpt-realtime".to_string()
}

/// Serde default helper for `VoiceConfig::realtime_voice` (OpenAI Realtime agent voice).
fn default_realtime_voice() -> String {
    "shimmer".to_string()
}

/// Serde default helper for `VoiceConfig::realtime_transcription_model`.
fn default_realtime_transcription_model() -> String {
    "gpt-4o-mini-transcribe".to_string()
}

/// Serde default helpers for the Phase 36.17.12 BUG 5 realtime VAD/noise fields.
/// Field-level defaults so pre-BUG-5 configs that lack these keys parse cleanly.
fn default_realtime_noise_reduction() -> String {
    "far_field".to_string()
}
fn default_realtime_vad_mode() -> String {
    "semantic_vad".to_string()
}
fn default_realtime_vad_threshold() -> f32 {
    0.5
}
fn default_realtime_vad_silence_ms() -> u32 {
    500
}
fn default_realtime_vad_prefix_ms() -> u32 {
    300
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            record_key: "ctrl+b".to_string(),
            silence_threshold: 200,
            silence_duration: 3.0,
            auto_tts: false,
            beep_enabled: true,
            max_recording_seconds: 120,
            barge_in_mode: BargeInMode::default(),
            wake_word: WakeWordConfig::default(),
            web_silence_threshold_rms: 5.0,
            realtime_model: "gpt-realtime".to_string(),
            realtime_voice: "shimmer".to_string(),
            realtime_transcription_model: "gpt-4o-mini-transcribe".to_string(),
            realtime_noise_reduction: "far_field".to_string(),
            realtime_vad_mode: "semantic_vad".to_string(),
            realtime_vad_threshold: 0.5,
            realtime_vad_silence_ms: 500,
            realtime_vad_prefix_ms: 300,
        }
    }
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
/// `protect_first_n` is the CONFIGURED upper bound on the number of LEADING
/// SYSTEM messages protected from compression — it caps the leading
/// system-message run, not a raw front-of-list message count (Phase 47.5
/// D-05: the first conversation pair is no longer pinned; see
/// `ContextCompressor::system_prefix_len`). At compression time the
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
/// Default `stale_warn_seconds` for [`SubagentConfig`] — 120 (2 minutes).
///
/// Phase 32.3 Plan 01 (D-05): seconds of inactivity before a subagent is
/// flagged Stale. Per-call override via the delegate_task JSON schema; this
/// is the fallback when no override is provided. D-07 ceiling
/// (`child_timeout_seconds`, default 300) remains the hard-kill bound; this
/// is only a soft warn threshold.
fn default_stale_warn_seconds() -> u64 {
    120
}

/// Default `max_spawn_depth` for [`SubagentConfig`] — 1 (flat delegation only).
/// Matches the Python hermes-agent reference (D-02, Phase 32.2).
fn default_max_spawn_depth() -> u32 {
    1
}

/// Default `nudge_interval` for [`MemoryConfig`] — 10 user turns.
/// Matches the Python hermes-agent reference (`memory.nudge_interval: 10`).
/// Set to 0 in YAML to disable the periodic nudge entirely. (Phase 32 LEARN-01)
fn default_nudge_interval() -> u32 {
    10
}

/// Default `skill_creation_guidance` for [`MemoryConfig`] — true (Phase 33 LEARN-04).
/// When the `skill_manage` tool is registered AND this flag is true, the system
/// prompt includes the "Skill Creation (Learning Loop)" trigger guidance block
/// per RESEARCH.md Pattern 6. Set to `false` in YAML to suppress the section.
///
/// Lives on `MemoryConfig` (typed) rather than the wizard-managed raw-YAML
/// `learning:` block because Plan 33-01 needs a typed, Config-readable flag
/// the prompt builder can consume directly; the `learning:` block has no
/// typed `Config` analog today (see `wizard.rs` raw-YAML splice).
fn default_skill_creation_guidance() -> bool {
    true
}

/// Default `recall_min_score` for [`MemoryConfig`] — 0.0000072 (Phase 47.5-02, D-03).
///
/// Must mirror `memory_sqlite::SqliteMemoryProvider::DEFAULT_RECALL_MIN_SCORE`
/// — `ironhermes-core` cannot depend on the provider crate, so the value is
/// duplicated here. See that constant's doc comment for the fixture
/// provenance (a two-document calibration fixture), the sign convention
/// (higher `relevance_score` = more relevant, floor is `>=`), and the
/// corpus-relative bm25 warning (production scores will not resemble the
/// fixture's). Operators can override via `memory.recall_min_score` in
/// config.yaml.
fn default_recall_min_score() -> f64 {
    0.0000072
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
    /// Phase 33 LEARN-04: when true (default), the system prompt includes the
    /// "Skill Creation (Learning Loop)" trigger guidance block whenever the
    /// `skill_manage` tool is registered in the active tool set. Set to false
    /// in YAML to suppress the block (e.g. for child agents or restricted
    /// deployments). Read by `PromptBuilder::set_skill_creation_guidance` at
    /// session freeze time.
    #[serde(default = "default_skill_creation_guidance")]
    pub skill_creation_guidance: bool,
    /// Phase 47.5-02 (D-03): minimum bm25 relevance score for `memory_recall`
    /// results from the sqlite provider. Below-floor matches are dropped so
    /// off-topic memories that only match a query on low-value tokens are
    /// never recalled. Must mirror `memory_sqlite::SqliteMemoryProvider::
    /// DEFAULT_RECALL_MIN_SCORE` — `ironhermes-core` cannot depend on the
    /// provider crate. No-op for the duckdb provider (substring match with
    /// synthetic 1.0/0.5 scores; any floor <= 1.0 is a no-op there).
    #[serde(default = "default_recall_min_score")]
    pub recall_min_score: f64,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: "file".to_string(),
            mirror_provider: None,
            memory_enabled: true,
            user_profile_enabled: true,
            nudge_interval: 10,
            skill_creation_guidance: true,
            recall_min_score: default_recall_min_score(),
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
    // Unified per-turn cap (Phase: AgentRuntime). Both the AgentLoop turn cap and
    // the shared BudgetHandle are sized from this single value; it defaults to the
    // historical loop default so behavior matches the more permissive of the two
    // pre-unification knobs.
    DEFAULT_MAX_ITERATIONS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// DEPRECATED alias for [`AgentConfig::max_iterations`]. Retained so existing
    /// config.yaml files that set `max_turns` keep working; `normalize()` folds a
    /// tuned value into `max_iterations` (the single canonical per-turn cap) and
    /// warns. Do not read this field — read `max_iterations`.
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
    /// Phase 36.3.8 (D-06): maximum seconds a suspended `clarify` tool waits
    /// for a human button response before returning a timeout sentinel
    /// (`{"answered":false,"reason":"timeout"}`). A never-answered clarify
    /// must not pin a turn slot forever — this bounds that window.
    /// Default: 120 (2 minutes). Consumed by Plan 02's ClarifyTool.
    #[serde(default = "default_clarify_timeout_secs")]
    pub clarify_timeout_secs: u64,
    /// Phase 38.1 (D-04/D-05): IANA timezone name for the Timestamp slot
    /// (e.g. `America/Los_Angeles`). `None` → host local timezone
    /// (`chrono::Local`). Not cache-breaking (D-07) — affects only the
    /// ephemeral Timestamp slot (slot 7), never the cached prefix.
    #[serde(default)]
    pub timezone: Option<String>,
}

impl AgentConfig {
    /// Collapse the deprecated `max_turns` alias into the canonical
    /// `max_iterations`. Honors a tuned `max_turns` only when `max_iterations`
    /// was left at the default (so an explicit `max_iterations` always wins),
    /// then keeps both fields in sync for any not-yet-migrated reader.
    pub fn normalize(&mut self) {
        if self.max_turns != self.max_iterations
            && self.max_iterations == default_agent_max_iterations()
        {
            eprintln!(
                "[config] agent.max_turns is deprecated; using its value ({}) as \
                 agent.max_iterations. Set agent.max_iterations instead to silence this.",
                self.max_turns
            );
            self.max_iterations = self.max_turns;
        }
        self.max_turns = self.max_iterations;
    }
}

fn default_clarify_timeout_secs() -> u64 {
    120
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
            clarify_timeout_secs: default_clarify_timeout_secs(),
            timezone: None,
        }
    }
}

// =============================================================================
// DangerousCommandsConfig (Phase 42 D-10)
// =============================================================================

/// Phase 42 D-10: Operator config for the dangerous-command guardrail.
///
/// Full-override: operators can ADD patterns at either tier AND RELAX/remove
/// built-in patterns. A startup `tracing::warn!` is emitted for each relaxed
/// built-in so the security floor is never silently weakened (D-10 guardrail).
///
/// All fields default to empty, so pre-42 config.yaml files parse cleanly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DangerousCommandsConfig {
    /// Additional Tier-1 patterns (approval-required) beyond built-ins.
    pub add_tier1: Vec<String>,
    /// Additional Tier-2 patterns (hard-block) beyond built-ins.
    pub add_tier2: Vec<String>,
    /// Built-in pattern strings to relax/remove. Each entry must be the exact
    /// pattern string from `DANGEROUS_PATTERNS`. A `tracing::warn!` is logged
    /// at startup for each match (D-10).
    pub relax: Vec<String>,
}

/// Phase 45 D-04: Approval-gate timeout configuration.
///
/// Lives in `ironhermes-core` (next to `DangerousCommandsConfig`) so both
/// the gateway coordinator and any future surface read the same configured value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalsGatewayConfig {
    /// Seconds to wait for an operator response before auto-expiring (D-04).
    /// Default: 120 (2 minutes).
    pub timeout_secs: u64,
}

impl Default for ApprovalsGatewayConfig {
    fn default() -> Self {
        Self { timeout_secs: 120 }
    }
}

/// Phase 45 D-08/D-10: MCP mutation guardrail verb configuration.
///
/// When `patterns` is empty (the default) the `McpMutationGuardrail` uses its
/// built-in DEFAULT_VERBS set.  A non-empty list is treated as a full override
/// — every removed built-in verb triggers a startup `tracing::warn!` (D-10).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct McpMutationGuardrailConfig {
    /// Full-override list of destructive verb strings.  Empty = use DEFAULT_VERBS.
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub backend: String,
    pub cwd: String,
    pub timeout: u64,
    /// Phase 42 D-05: Global operator-declared env vars to pass through to
    /// terminal subprocesses, on top of the base SAFE_ENV_KEYS + XDG_* set.
    /// Example: `terminal_env_allowlist: [KUBECONFIG, AWS_PROFILE]`
    #[serde(default)]
    pub terminal_env_allowlist: Vec<String>,
    /// Phase 36.3.12 D-07: container CLI to shell out to for the `docker`
    /// backend. Explicit, no auto-detect — auto-picking a runtime by probing
    /// PATH is a silent-fallback footgun. Values: `"docker"` | `"podman"`.
    #[serde(default = "default_container_runtime")]
    pub container_runtime: String,
    /// Base image for the `docker` backend's persistent container.
    /// Claude's discretion default: a small, widely-available Debian base.
    #[serde(default = "default_terminal_image")]
    pub image: String,
    /// Phase 36.3.12 D-09: explicit credential allowlist for env vars forwarded
    /// across the docker/ssh backend boundary. Default empty — nothing secret
    /// crosses unless a var name is opted in here. Mirrors hermes-agent's
    /// `docker_forward_env`.
    #[serde(default)]
    pub forward_env: Vec<String>,
    /// Phase 36.3.12 D-02 (RESEARCH.md Open Question Q2): orphan-reaper
    /// "lifetime" knob in seconds — the reaper GCs labeled containers idle
    /// longer than 2x this value. An explicit knob, not derived from an
    /// unrelated timeout. Default: 86400 (24h).
    #[serde(default = "default_container_reap_after_secs")]
    pub container_reap_after_secs: u64,
    /// Container resource limits for the `docker` backend.
    #[serde(default)]
    pub container: ContainerResourceConfig,
    /// SSH backend connection details. `None` (default) means the `ssh`
    /// backend cannot be constructed — `create_environment` hard-errors
    /// per D-05 rather than falling back silently.
    #[serde(default)]
    pub ssh: Option<SshBackendConfig>,
}

fn default_container_runtime() -> String {
    "docker".to_string()
}

fn default_terminal_image() -> String {
    "debian:stable-slim".to_string()
}

fn default_container_reap_after_secs() -> u64 {
    86400
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            backend: "local".to_string(),
            cwd: ".".to_string(),
            timeout: 30,
            terminal_env_allowlist: Vec::new(),
            container_runtime: default_container_runtime(),
            image: default_terminal_image(),
            forward_env: Vec::new(),
            container_reap_after_secs: default_container_reap_after_secs(),
            container: ContainerResourceConfig::default(),
            ssh: None,
        }
    }
}

/// Phase 36.3.12: container resource knobs for the `docker`/`podman` backend
/// (D-07 runtime toggle shares this same resource surface). Mirrors the
/// hermes-agent Python reference's `container_cpu`/`container_memory`/
/// `container_disk`/`container_persistent` config keys.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerResourceConfig {
    /// CPU limit (fractional cores). Default: 1.0.
    pub cpu: f64,
    /// Memory limit in MiB. Default: 5120 (5 GiB).
    pub memory_mib: u64,
    /// Disk limit in MiB for the workspace mount/tmpfs. Default: 51200 (50 GiB).
    pub disk_mib: u64,
    /// `--pids-limit` process-count cap. Default: 256.
    pub pids_limit: u64,
    /// Bind-mount (persistent, survives container recreation) vs tmpfs
    /// (ephemeral). Default: true.
    pub persistent: bool,
    /// Container networking. Default: false → `--network=none` (security-hardened
    /// default per docs/EXEC-BACKENDS-ARCHITECTURE.md §4.5, D-09).
    pub network: bool,
}

impl Default for ContainerResourceConfig {
    fn default() -> Self {
        Self {
            cpu: 1.0,
            memory_mib: 5120,
            disk_mib: 51200,
            pids_limit: 256,
            persistent: true,
            network: false,
        }
    }
}

/// Phase 36.3.12: SSH backend connection config (D-04). No `Default` values
/// for `host`/`user` are meaningful — this struct is only consulted when the
/// operator has set `terminal.backend: ssh` and populated `terminal.ssh.*`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SshBackendConfig {
    /// SSH host to connect to.
    pub host: String,
    /// SSH user to connect as.
    pub user: String,
    /// SSH port. Default: 22.
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    /// Path to an SSH private key file. `None` uses the ssh CLI's default
    /// identity resolution (agent, `~/.ssh/id_*`, etc).
    #[serde(default)]
    pub key_path: Option<String>,
}

impl Default for SshBackendConfig {
    fn default() -> Self {
        Self {
            host: String::new(),
            user: String::new(),
            port: default_ssh_port(),
            key_path: None,
        }
    }
}

fn default_ssh_port() -> u16 {
    22
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
// WebUiConfig (Phase 47.3 D-06): operator auth settings for iron_hermes_ui
// =============================================================================

/// Phase 47.3 D-06: operator authentication settings for the `iron_hermes_ui`
/// web server. Lives at a NEW top-level `web_ui:` key — deliberately NOT
/// nested inside [`WebConfig`] above, which is the unrelated web-*browsing*
/// config (`backend`/`user_agent`/`max_content_chars`/`timeout_secs`). CONTEXT.md
/// D-06: nesting operator credentials inside `web:` would conflate two
/// unrelated concerns and is explicitly prohibited.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WebUiConfig {
    pub auth: WebUiAuthConfig,
}

/// Phase 47.3 D-06/D-07: single-operator credential + session settings for
/// the `iron_hermes_ui` login boundary.
///
/// `password_hash`'s full layering (config.yaml > `IRONHERMES_WEB_PASSWORD_HASH`
/// env > vault `SecretStore` key `web_ui/auth/password_hash`) is resolved by
/// `iron_hermes_ui::server::auth::auth_config_from`, not here — this struct
/// only carries what's on disk in `config.yaml`. `None` here does not by
/// itself mean auth is disabled; the env/vault fallback may still resolve a
/// hash at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebUiAuthConfig {
    /// argon2id PHC string (e.g. `$argon2id$v=19$m=19456,t=2,p=1$…`). `None`
    /// here plus no env/vault fallback means auth is disabled (today's
    /// loopback-only posture stands).
    pub password_hash: Option<String>,
    /// D-01/D-02: selected login treatment slug (`basic` default). Server-
    /// rendered — the selection is a deployment property, not a per-browser
    /// localStorage value, so every device sees the same treatment with no
    /// flash-of-wrong-theme.
    pub login_theme: String,
    /// Adds `Secure` to the session cookie. Default `false` — plain-LAN HTTP
    /// would otherwise never send the cookie back. Cannot safely be `true`
    /// until this project ships a TLS story (AUTH-DESIGN §7 / Deferred Ideas).
    pub cookie_secure: bool,
    /// Absolute session lifetime, in hours. Default 168 (7 days).
    pub session_ttl_hours: u64,
    /// Sliding idle timeout, in hours. Default 24.
    pub idle_timeout_hours: u64,
}

impl Default for WebUiAuthConfig {
    fn default() -> Self {
        Self {
            password_hash: None,
            login_theme: "basic".to_string(),
            cookie_secure: false,
            session_ttl_hours: 168,
            idle_timeout_hours: 24,
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
    /// None = use $IRONHERMES_HOME/browser-profile (default — resolved at spawn time).
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

    /// Phase 41.3 D-16: per-URL extraction deadline in seconds, applied inside
    /// `WebExtractTool::execute`'s spawned per-URL task (after the semaphore permit
    /// is acquired, around the `process_one_url` call). A URL that exceeds this
    /// budget surfaces as an in-array `ExtractionResult::error` entry at its
    /// original index instead of sinking the whole batch — the fix for blackbox
    /// run `0eaed980` (a bot-walled homepage that never answered). Default 60s:
    /// generous against the 7-13s every single-URL extract took in that run, and
    /// 2x `WebConfig.timeout_secs`'s 30s HTTP-leg default.
    #[serde(default = "default_per_url_timeout_secs")]
    pub per_url_timeout_secs: u64,
}

/// Phase 41.3 D-16 default for [`ExtractConfig::per_url_timeout_secs`].
fn default_per_url_timeout_secs() -> u64 {
    60
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
            per_url_timeout_secs: default_per_url_timeout_secs(),
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
    /// Phase 36.17.9: persist gateway session routing (SessionKey to session_id)
    /// and per-session voice mode to `state.db`, so an ongoing platform
    /// conversation RESUMES after a restart instead of starting fresh. Defaults
    /// to `true`; set `false` to restore the legacy stateless behavior (D-02).
    #[serde(default = "default_true")]
    pub persist_sessions: bool,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            platforms: HashMap::new(),
            context_engine: default_gateway_engine(),
            compression_threshold: default_gateway_threshold(),
            persist_sessions: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PlatformGatewayConfig {
    pub enabled: bool,
    /// Bot token: Telegram (TELEGRAM_BOT_TOKEN), Discord (DISCORD_BOT_TOKEN), or Slack bot token (xoxb-, SLACK_BOT_TOKEN).
    pub token: Option<String>,
    /// Slack Socket Mode app-level token (xapp-). Telegram/Discord leave this None.
    #[serde(default)]
    pub app_token: Option<String>,
    pub api_key: Option<String>,
    /// Canonical cross-platform sender allowlist (Phase 47.6 Plan 01, P0-2/D-05).
    /// No longer Telegram-specific: holds Telegram numeric chat IDs, Slack `U…`
    /// member IDs, Discord numeric user IDs (as strings), and Buzz hex pubkeys —
    /// one shared field gates every platform's inbound access. Empty = deny all
    /// (D-08/D-12). Deserializes via [`deserialize_whitelist`] so existing
    /// operator configs holding bare YAML numbers keep working unchanged: each
    /// element may be written as either a YAML integer or a YAML string, and
    /// both forms coerce to the same canonical `String` — a whitelist holding
    /// both `12345` and `"12345"` merges into one authorization, it does not
    /// mint two.
    #[serde(default, deserialize_with = "deserialize_whitelist")]
    pub whitelist: Vec<String>,
    /// Phase 36.3.8 (D-01): explicit Telegram home channel used by `send_message`
    /// when the bare `telegram` target is given and the whitelist does not have
    /// exactly one entry. When `None` and `whitelist.len() == 1` the single
    /// whitelist entry is used as the home channel. When `None` and
    /// `whitelist.len() != 1` the tool returns an "ambiguous home channel" error.
    /// Set this field to make the home channel unambiguous for multi-user bots.
    #[serde(default)]
    pub home_channel_id: Option<String>,
    /// Session inactivity timeout in hours. Default 24 (D-14).
    #[serde(default = "default_session_timeout_hours")]
    pub session_timeout_hours: u64,
    /// Maximum concurrent agent runs. Default 8 (TG-06).
    #[serde(default = "default_max_concurrent_runs")]
    pub max_concurrent_runs: usize,
    /// Phase 47.6 Plan 01 (P1-3): the single Nostr relay this Buzz platform
    /// section connects to. `None`/absent on every other platform.
    #[serde(default)]
    pub relay_url: Option<String>,
    /// Phase 47.6 Plan 01: Buzz channel/group identifiers (NIP-29 `#h` values)
    /// the adapter subscribes to. Empty on every other platform.
    #[serde(default)]
    pub channels: Vec<String>,
    /// Phase 47.6 Plan 01 (D-08): Buzz channel trust posture. `Closed` (the
    /// `Default`) requires the sender's pubkey hex to be in `whitelist`;
    /// `Open` treats channel membership itself as sufficient. `Open` is never
    /// silently the default — enabling it is an explicit operator opt-in and
    /// the adapter logs a loud `tracing::warn!` at startup when it is set.
    #[serde(default)]
    pub channel_trust: ChannelTrust,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

fn default_session_timeout_hours() -> u64 {
    24
}
fn default_max_concurrent_runs() -> usize {
    8
}

/// Phase 47.6 Plan 01 (D-08): Buzz channel-scoped trust posture.
///
/// `Closed` is the [`Default`] — deny-all is the default security posture for
/// every platform this workspace supports, and Buzz's channel trust dial must
/// never become an exception. `Open` is reachable only by an explicit
/// operator `channel_trust: open` in `config.yaml`; the adapter logs a
/// `tracing::warn!` naming the relay URL whenever it resolves to `Open` so
/// enabling it is loud, not silent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelTrust {
    #[default]
    Closed,
    Open,
}

/// Backward-compatible whitelist deserializer (Phase 47.6 Plan 01, P0-2/D-05).
///
/// Accepts a YAML sequence whose elements are either integers or strings and
/// coerces every element to its canonical `String` form (integers via their
/// decimal representation) — an operator config written before this migration
/// (`whitelist: [12345, 67890]`) keeps loading with no action required, and a
/// config already using quoted strings (Slack `"U012AB3CD"`, Buzz hex
/// pubkeys) is preserved exactly. List order is preserved; nothing an
/// operator can reasonably have written today is rejected.
pub fn deserialize_whitelist<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum WhitelistEntry {
        Int(i64),
        Str(String),
    }

    let entries: Vec<WhitelistEntry> = Vec::deserialize(deserializer)?;
    Ok(entries
        .into_iter()
        .map(|e| match e {
            WhitelistEntry::Int(i) => i.to_string(),
            WhitelistEntry::Str(s) => s,
        })
        .collect())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CronConfig {
    pub wrap_response: bool,
    /// Maximum cron jobs run concurrently per tick. Bounds the
    /// parent×subagent memory product so several jobs due at once cannot
    /// spike memory and trigger an OOM/jetsam kill. `0` = unbounded (legacy
    /// behavior). The `IRONHERMES_CRON_MAX_PARALLEL` env var overrides this
    /// when set. Default 2.
    pub max_parallel: usize,
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            wrap_response: true,
            max_parallel: 2,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub redact_secrets: bool,
    /// Phase 36.17.10 Plan 01 (D — config write-back security): DEFCON-tiered gate that
    /// allows browser-initiated writes to `config.yaml`. Defaults `false` (closed) so
    /// web config writes are disabled until the operator explicitly opts in.
    /// Set to `true` in `config.yaml` to enable the `update_voice_config` server fn.
    pub web_config_write_enabled: bool,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            redact_secrets: true,
            web_config_write_enabled: false,
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

// =============================================================================
// DefconLevel (CR-01 / security tier)
// =============================================================================

/// Security enforcement tier, inspired by the US DEFCON scale.
///
/// 5 = least restrictive (permissive warnings only).
/// 1 = most restrictive (hard-reject any suspicious path).
///
/// Defaults to `5` so existing deployments are unaffected.
/// Operators can tighten to `1` in `config.yaml` under `skills.defcon_level`.
///
/// Used today to gate CR-01 symlink-bypass enforcement in `SkillRegistry`:
///   DEFCON 1-2 → hard-reject skill paths that escape their declared root.
///   DEFCON 3-5 → warn-but-load (backward-compatible).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct DefconLevel(u8);

impl DefconLevel {
    /// DEFCON 5 — least restrictive, the default.
    pub const FIVE: Self = Self(5);
    /// DEFCON 1 — most restrictive.
    pub const ONE: Self = Self(1);

    /// Returns the numeric level (1–5).
    pub fn level(self) -> u8 {
        self.0
    }

    /// True when this level is stricter than or equal to `threshold`
    /// (i.e. level number ≤ threshold number, because lower = stricter).
    pub fn at_least_as_strict_as(self, threshold: u8) -> bool {
        self.0 <= threshold
    }
}

impl Default for DefconLevel {
    fn default() -> Self {
        Self::FIVE
    }
}

impl TryFrom<u8> for DefconLevel {
    type Error = String;
    fn try_from(v: u8) -> Result<Self, Self::Error> {
        if (1..=5).contains(&v) {
            Ok(Self(v))
        } else {
            Err(format!("defcon_level must be 1–5, got {v}"))
        }
    }
}

impl From<DefconLevel> for u8 {
    fn from(d: DefconLevel) -> u8 {
        d.0
    }
}

impl std::fmt::Display for DefconLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DEFCON {}", self.0)
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
    /// `$IRONHERMES_HOME/credentials` with fallback to `~/.ironhermes/credentials`
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
    /// Phase 26.7.3 D-06 — opt-out list; names present here are explicitly
    /// disabled. All other skills are on by default. Cross-surface: agent
    /// loop, web UI, and TUI all read this field. `#[serde(default)]`
    /// ensures existing config.yaml files without this key still parse.
    #[serde(default)]
    pub disabled: Vec<String>,
    /// CR-01 / DEFCON tier for skills symlink-bypass enforcement.
    ///
    /// DEFCON 1–2: hard-reject any skill whose canonicalized path escapes its
    /// declared search root (symlink bypass attempt).
    /// DEFCON 3–5 (default 5): warn-but-load (backward-compatible).
    ///
    /// Set in `config.yaml` as `skills.defcon_level: 2` (integer 1–5).
    #[serde(default)]
    pub defcon_level: DefconLevel,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_paths: Vec::new(),
            credential_dir: None,
            config: HashMap::new(),
            hub: HubConfig::default(),
            disabled: Vec::new(),
            defcon_level: DefconLevel::default(),
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

/// Subagent delegation configuration (D-07/D-08/D-09, Phase 32.2).
///
/// YAML key: `delegation:` (renamed from `subagent:` in Phase 32.2 D-07).
/// A startup-detection gate in `Config::load_from` rejects configs that still
/// use the old `subagent:` key with an actionable error message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentConfig {
    /// Timeout in seconds for each child agent execution. Default: 300 (5 minutes).
    /// (D-07: renamed from `timeout_secs`)
    ///
    /// Phase 32.3 D-07: ceiling stays at 300 — the 6.7-hour ghost bug is the
    /// leak surviving timeout, not the timeout not firing. RegistrationGuard
    /// (Plan 01) closes the leak; this ceiling is unchanged.
    pub child_timeout_seconds: u64,
    /// Phase 32.3 Plan 01 (D-05/D-06): seconds of inactivity before a subagent
    /// is flagged Stale. Default: 120 (2 minutes). Per-call override via the
    /// delegate_task JSON schema (`stale_warn_seconds`); this is the fallback
    /// when no per-call value is supplied. Only a soft warn threshold — the
    /// hard-kill ceiling remains `child_timeout_seconds` (D-07 unchanged).
    #[serde(default = "default_stale_warn_seconds")]
    pub stale_warn_seconds: u64,
    /// Maximum concurrent child agents. Default: 3.
    /// (D-07: renamed from `max_subagents`)
    pub max_concurrent_children: usize,
    /// Maximum LLM iterations per child agent. Default: 20 (lowered from 50 to
    /// bound the cost when a failing child loops on tool errors; D-08 had raised
    /// from 10 to 50 but the worst case there pairs with the parent budget for a
    /// multi-hour grind on a single bad delegation).
    pub max_iterations: usize,
    /// Default toolset groups for child agents (D-01). Default: ["terminal", "file", "web"].
    pub default_toolsets: Vec<String>,
    /// Optional model override for child agents (D-23). None = use parent's model.
    pub model: Option<String>,
    /// Optional provider override for child agents (D-23). None = use parent's provider.
    pub provider: Option<String>,
    /// Optional custom API base URL for child agents (D-23). None = use parent's.
    pub base_url: Option<String>,
    /// Optional custom API key for child agents (D-23). None = use parent's.
    pub api_key: Option<String>,
    /// Maximum spawn depth for orchestrator chains (D-02). Default: 1 (flat delegation only).
    /// Raise to 2 to allow orchestrators to spawn leaf grandchildren, 3 for three levels.
    #[serde(default = "default_max_spawn_depth")]
    pub max_spawn_depth: u32,
    /// Global kill switch: when false, every child is downgraded to leaf role (D-03).
    /// Default: true (orchestration allowed up to max_spawn_depth).
    #[serde(default = "default_true")]
    pub orchestrator_enabled: bool,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            child_timeout_seconds: 300,
            // Phase 32.3 Plan 01 (D-05): default soft-stale warn threshold.
            // D-07 confirmed no-op: child_timeout_seconds stays at 300.
            stale_warn_seconds: 120,
            max_concurrent_children: 3,
            max_iterations: 20,
            default_toolsets: vec!["terminal".into(), "file".into(), "web".into()],
            model: None,
            provider: None,
            base_url: None,
            api_key: None,
            max_spawn_depth: 1,
            orchestrator_enabled: true,
        }
    }
}

/// Detect the legacy `subagent:` YAML key in raw config file content.
///
/// Returns `Some(message)` when a line beginning with `subagent:` is found
/// (after stripping leading whitespace), indicating the user must rename the
/// key to `delegation:`.  Returns `None` when no legacy key is detected.
///
/// The check is line-start–only so that a string value containing the word
/// "subagent" (e.g., a task description) does NOT trigger the gate.
pub(crate) fn detect_legacy_subagent_key(content: &str) -> Option<String> {
    let found = content
        .lines()
        .any(|line| line.trim_start().starts_with("subagent:"));
    if found {
        Some(
            "Config key 'subagent:' is deprecated and no longer supported. \
             Rename it to 'delegation:' in your config.yaml."
                .to_string(),
        )
    } else {
        None
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
            // D-07: detect legacy `subagent:` key before parse so users get an
            // actionable message rather than silent defaults.
            if let Some(msg) = detect_legacy_subagent_key(&content) {
                eprintln!("{}", msg);
                std::process::exit(1);
            }
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
                            extra_request_options: HashMap::new(),
                            models: HashMap::new(),
                        },
                    );
                }
            }
            // Collapse the deprecated agent.max_turns alias into the single
            // canonical agent.max_iterations cap (AgentRuntime unification).
            config.agent.normalize();
            // Phase 40.5 (D-16): backfill missing shipped seed personas into a
            // partial operator-supplied identities section. `#[serde(default =
            // "default_seed_identities")]` only fires when the `identities:` key
            // is ENTIRELY ABSENT from the YAML; if a partial section is present
            // serde parses only what is there, leaving shipped personas out. This
            // explicit merge mirrors the custom_providers migration loop above.
            for (slug, record) in default_seed_identities() {
                config.identities.entry(slug).or_insert(record);
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

    /// Save config to a specific path using an atomic temp+rename strategy.
    ///
    /// Phase 36.17.10 Plan 01: upgraded from bare `std::fs::write` (non-atomic) to
    /// write-to-temp-then-rename so a crash or power loss mid-write can never leave
    /// a partial `config.yaml`. The temp file is `path.with_extension("yaml.tmp")`;
    /// `std::fs::rename` is atomic on POSIX (T-36.17.10-01-03 mitigation).
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)?;
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, &content)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Get the config file path.
    pub fn config_path() -> PathBuf {
        get_hermes_home().join("config.yaml")
    }

    /// Phase 40.5 (D-11): Resolve the TTS provider + voice for an identity slug.
    ///
    /// Returns `(provider_name, voice_override)`:
    /// - If `identity_slug` is `Some` and the slug is found in `self.identities`,
    ///   the identity's `free_mode_tts_provider` (or the global `tts.provider` as
    ///   fallback) and its `free_mode_tts_voice` are returned.
    /// - Otherwise the global `tts.provider` is returned with no voice override.
    ///
    /// This is the lightweight helper for callers that only need the resolved
    /// provider name + optional voice. Use [`Self::effective_tts_config_for_identity`]
    /// when you need a full `TtsConfig` clone (e.g. to feed into `build_tts_registry`).
    pub fn resolve_tts_override(&self, identity_slug: Option<&str>) -> (String, Option<String>) {
        if let Some(slug) = identity_slug
            && let Some(record) = self.identities.get(slug)
        {
            let provider = record
                .voice
                .free_mode_tts_provider
                .as_deref()
                .unwrap_or(&self.tts.provider)
                .to_string();
            return (provider, record.voice.free_mode_tts_voice.clone());
        }
        (self.tts.provider.clone(), None)
    }

    /// Phase 40.5 (D-11): Build a `TtsConfig` with the identity's overrides applied.
    ///
    /// Clones `self.tts` and, when `identity_slug` names a known identity with a
    /// `free_mode_tts_provider` or `free_mode_tts_voice`, applies those values onto
    /// the clone's matching provider sub-config:
    ///
    /// - `"edge"` → `clone.edge.voice = voice`
    /// - `"elevenlabs"` → `clone.elevenlabs.voice_id = voice`
    /// - `"openai"` → `clone.openai.voice = voice`
    ///
    /// The caller (Plan 08's `auto_speak_reply`) feeds the returned `TtsConfig` to
    /// `build_tts_registry` and then calls `provider.synthesize(text, path)` unchanged
    /// — no voice parameter is threaded into the trait (addresses review concern:
    /// `TtsProvider::synthesize` has no voice param).
    pub fn effective_tts_config_for_identity(&self, identity_slug: Option<&str>) -> TtsConfig {
        let mut effective = self.tts.clone();
        if let Some(slug) = identity_slug
            && let Some(record) = self.identities.get(slug)
        {
            // Override the active provider name when the identity specifies one.
            if let Some(ref prov) = record.voice.free_mode_tts_provider {
                effective.provider = prov.clone();
            }
            // Wire the voice into the matching provider sub-config so that
            // the existing voiceless synthesize(text, path) trait is honoured.
            if let Some(ref voice) = record.voice.free_mode_tts_voice {
                match effective.provider.as_str() {
                    "edge" => effective.edge.voice = voice.clone(),
                    "elevenlabs" => effective.elevenlabs.voice_id = voice.clone(),
                    "openai" => effective.openai.voice = voice.clone(),
                    _ => {} // unknown provider — leave sub-config unchanged
                }
            }
        }
        effective
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
                chat_id: tg.whitelist[0].clone(),
            },
            _ => OriginDecision::Multi {
                whitelist: tg.whitelist.clone(),
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

    // =========================================================================
    // Phase 46.8 Plan 04 D-10: Config.vault serde-default backward compat
    // =========================================================================

    #[test]
    fn config_yaml_without_vault_section_parses_with_vault_disabled() {
        // D-10: pre-46.8 config.yaml files have no `vault:` key at all — must
        // parse cleanly with vault disabled (zero behavioral change).
        let yaml = r#"
web:
  backend: "firecrawl"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(!c.vault.enabled);
        assert_eq!(c.vault.backend, "env-var");
    }

    #[test]
    fn config_default_has_vault_disabled() {
        let c = Config::default();
        assert!(!c.vault.enabled);
        assert_eq!(c.vault.backend, "env-var");
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
            "Phase 26.3 UDD-01: user_data_dir must default to None (computed from IRONHERMES_HOME at spawn time)"
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
        assert_eq!(default.child_timeout_seconds, 300);
        assert_eq!(default.max_concurrent_children, 3);
        // Lowered from 50 to 20 to bound runaway-delegation cost (supersedes D-08).
        assert_eq!(default.max_iterations, 20);
        // Phase 32.3 Plan 01 (D-05): stale_warn_seconds defaults to 120.
        assert_eq!(
            default.stale_warn_seconds, 120,
            "stale_warn_seconds must default to 120 per D-05"
        );
        // Phase 32.3 Plan 01 (D-07): child_timeout_seconds ceiling is unchanged
        // — the 6.7-hour ghost bug is the LEAK surviving timeout, not the
        // timeout not firing. RegistrationGuard closes the leak structurally;
        // the hard-kill ceiling stays at 300s.
        assert_eq!(
            default.child_timeout_seconds, 300,
            "D-07: child_timeout_seconds ceiling is unchanged in 32.3"
        );
    }

    /// Phase 32.3 Plan 01 (D-05 + D-07): standalone defaults check for the new
    /// `stale_warn_seconds` field. Mirrored from `test_subagent_config_default`
    /// per the plan's locked acceptance criteria — a dedicated test name
    /// makes the regression intent explicit when greps line up against this
    /// file.
    #[test]
    fn test_subagent_config_stale_warn_default() {
        let default = SubagentConfig::default();
        assert_eq!(
            default.stale_warn_seconds, 120,
            "stale_warn_seconds must default to 120 per D-05"
        );
        assert_eq!(
            default.child_timeout_seconds, 300,
            "D-07: child_timeout_seconds ceiling is unchanged in 32.3"
        );
    }

    #[test]
    fn test_config_default_includes_subagent() {
        let config = Config::default();
        assert_eq!(config.delegation.child_timeout_seconds, 300);
        assert_eq!(config.delegation.max_concurrent_children, 3);
        // Lowered from 50 to 20 to bound runaway-delegation cost (supersedes D-08).
        assert_eq!(config.delegation.max_iterations, 20);
    }

    #[test]
    fn test_config_parses_without_subagent_section() {
        let yaml = r#"
model:
  default: "test-model"
  provider: "openrouter"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.delegation.child_timeout_seconds, 300);
        assert_eq!(config.delegation.max_concurrent_children, 3);
        // Lowered from 50 to 20 to bound runaway-delegation cost (supersedes D-08).
        assert_eq!(config.delegation.max_iterations, 20);
    }

    #[test]
    fn test_subagent_config_defaults_include_new_fields() {
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
        // D-32.2 new fields
        assert_eq!(
            default.max_spawn_depth, 1,
            "max_spawn_depth must default to 1"
        );
        assert!(
            default.orchestrator_enabled,
            "orchestrator_enabled must default to true"
        );
        // Lowered from 50 to 20 to bound runaway-delegation cost (supersedes D-08).
        assert_eq!(
            default.max_iterations, 20,
            "max_iterations must default to 20 (runaway-delegation guard)"
        );
        // Phase 32.3 Plan 01 (D-05): new soft-stale warn threshold field.
        assert_eq!(
            default.stale_warn_seconds, 120,
            "stale_warn_seconds must default to 120 per D-05"
        );
    }

    #[test]
    fn test_subagent_config_backward_compat_parse() {
        // Only child_timeout_seconds in YAML (new name) — all other fields should get defaults
        let yaml = r#"
delegation:
  child_timeout_seconds: 600
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.delegation.child_timeout_seconds, 600);
        assert_eq!(config.delegation.max_concurrent_children, 3);
        // Lowered from 50 to 20 to bound runaway-delegation cost (supersedes D-08).
        assert_eq!(config.delegation.max_iterations, 20);
        assert_eq!(
            config.delegation.default_toolsets,
            vec![
                "terminal".to_string(),
                "file".to_string(),
                "web".to_string()
            ]
        );
        assert!(config.delegation.model.is_none());
        assert!(config.delegation.provider.is_none());
        assert!(config.delegation.base_url.is_none());
        assert!(config.delegation.api_key.is_none());
        assert_eq!(config.delegation.max_spawn_depth, 1);
        assert!(config.delegation.orchestrator_enabled);
        // Phase 32.3 Plan 01 (D-05): serde default kicks in when YAML omits the field.
        assert_eq!(
            config.delegation.stale_warn_seconds, 120,
            "stale_warn_seconds must default to 120 via #[serde(default = ...)]"
        );
    }

    #[test]
    fn test_config_parses_delegation_key() {
        let yaml = r#"
delegation:
  max_concurrent_children: 5
  child_timeout_seconds: 120
  max_spawn_depth: 2
  orchestrator_enabled: false
  max_iterations: 100
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("must parse");
        assert_eq!(config.delegation.max_concurrent_children, 5);
        assert_eq!(config.delegation.child_timeout_seconds, 120);
        assert_eq!(config.delegation.max_spawn_depth, 2);
        assert!(!config.delegation.orchestrator_enabled);
        assert_eq!(config.delegation.max_iterations, 100);
    }

    #[test]
    fn test_legacy_subagent_key_detected() {
        let yaml = "subagent:\n  max_subagents: 5\n";
        let result = detect_legacy_subagent_key(yaml);
        assert!(result.is_some(), "legacy subagent: key must be detected");
        let msg = result.unwrap();
        assert!(
            msg.contains("subagent:"),
            "message must mention the old key"
        );
        assert!(
            msg.contains("delegation:"),
            "message must mention the new key"
        );
        assert!(
            msg.contains("config.yaml"),
            "message must mention the config file"
        );
    }

    #[test]
    fn test_delegation_key_not_flagged() {
        let yaml = "delegation:\n  max_concurrent_children: 3\n";
        let result = detect_legacy_subagent_key(yaml);
        assert!(
            result.is_none(),
            "delegation: key must NOT trigger the legacy detector"
        );
    }

    #[test]
    fn test_subagent_substring_in_value_not_flagged() {
        // A string value containing "subagent" must NOT trigger the gate
        let yaml = r#"
agent:
  name: "my_subagent_runner"
  description: "subagent: handles delegated tasks"
"#;
        let result = detect_legacy_subagent_key(yaml);
        assert!(
            result.is_none(),
            "subagent substring inside a value must NOT trigger the gate (line-start check only)"
        );
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
    // Phase 26.7.3 Plan 01: SkillsConfig.disabled (D-06) serde tests
    // =========================================================================

    #[test]
    fn skills_config_default_has_empty_disabled() {
        let default = SkillsConfig::default();
        assert_eq!(
            default.disabled,
            Vec::<String>::new(),
            "disabled must be Vec::new() by default — all skills on by default"
        );
    }

    #[test]
    fn skills_config_disabled_field_round_trip() {
        let cfg = SkillsConfig {
            disabled: vec!["foo".to_string(), "bar".to_string()],
            ..Default::default()
        };
        let ser = serde_yaml::to_string(&cfg).expect("serialize");
        let de: SkillsConfig = serde_yaml::from_str(&ser).expect("deserialize");
        assert_eq!(
            de.disabled,
            vec!["foo".to_string(), "bar".to_string()],
            "disabled list must round-trip through YAML serde"
        );
        assert!(
            de.enabled,
            "other fields must be preserved after round-trip"
        );
    }

    #[test]
    fn skills_config_missing_disabled_key_defaults_empty() {
        // Pre-phase config.yaml files have no `disabled:` key.
        // #[serde(default)] must produce Vec::new() — not an error.
        let yaml = "enabled: true\nextra_paths: []\n";
        let cfg: SkillsConfig =
            serde_yaml::from_str(yaml).expect("must parse without disabled key");
        assert!(
            cfg.disabled.is_empty(),
            "missing disabled key must deserialize to empty Vec via #[serde(default)]"
        );
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

    // =========================================================================
    // Phase 47.6 Plan 03 (P0-2/D-05): whitelist backward-compatibility matrix.
    //
    // Every fixture below is parsed as a full `config.yaml` string through
    // `serde_yaml::from_str::<Config>(..)` — the same load path operators hit —
    // rather than constructing `PlatformGatewayConfig` struct literals directly.
    // A struct-literal test would stay green even if `deserialize_whitelist`
    // were deleted entirely, which is exactly the "green test that verifies
    // its own assumptions" failure class this project has been burned by
    // before (see MEMORY.md `feedback_tests_that_verify_their_own_assumptions`).
    //
    // The pre-existing `test_telegram_default_origin_*` fixtures above are
    // left completely untouched — they are the control proving `OriginDecision`
    // semantics survived the `Vec<i64>` -> `Vec<String>` type change unchanged.
    // =========================================================================

    #[test]
    fn whitelist_bare_yaml_numbers_still_load() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345, 67890]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(whitelist, &vec!["12345".to_string(), "67890".to_string()]);
    }

    #[test]
    fn whitelist_quoted_numeric_strings_load() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: ["12345", "67890"]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(whitelist, &vec!["12345".to_string(), "67890".to_string()]);
    }

    #[test]
    fn whitelist_slack_member_id_loads() {
        // Impossible before the Vec<i64> -> Vec<String> migration — the
        // regression guard for the latent Slack defect D-05 closes.
        let yaml = r#"
gateway:
  platforms:
    slack:
      enabled: true
      whitelist: ["U012AB3CD"]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("slack").unwrap().whitelist;
        assert_eq!(whitelist, &vec!["U012AB3CD".to_string()]);
    }

    #[test]
    fn whitelist_nostr_hex_pubkey_loads() {
        let hex_pubkey = "a".repeat(64);
        let yaml = format!(
            r#"
gateway:
  platforms:
    buzz:
      enabled: true
      whitelist: ["{hex_pubkey}"]
"#
        );
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("buzz").unwrap().whitelist;
        assert_eq!(whitelist.len(), 1);
        assert_eq!(whitelist[0].len(), 64);
        assert_eq!(whitelist[0], hex_pubkey);
    }

    #[test]
    fn whitelist_mixed_forms_load_in_order() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345, "67890", "U012AB3CD", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef"]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(
            whitelist,
            &vec![
                "12345".to_string(),
                "67890".to_string(),
                "U012AB3CD".to_string(),
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string(),
            ]
        );
    }

    #[test]
    fn whitelist_missing_key_is_empty() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert!(whitelist.is_empty());
    }

    #[test]
    fn whitelist_explicit_empty_list_is_empty() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: []
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert!(whitelist.is_empty());
    }

    #[test]
    fn whitelist_preserves_surrounding_whitespace_verbatim() {
        // The deserializer must not trim — trimming would make a malformed
        // entry silently match a well-formed sender (P0-2 encoding).
        let yaml = "gateway:\n  platforms:\n    telegram:\n      enabled: true\n      whitelist: [\"  12345  \"]\n";
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(whitelist, &vec!["  12345  ".to_string()]);
    }

    #[test]
    fn whitelist_preserves_hex_case_verbatim() {
        // Built programmatically (rather than hand-typed) so the fixture's
        // exact length is never a manual-counting risk.
        let mixed_case: String = "AbCdEf0123456789".repeat(4);
        let yaml = format!(
            r#"
gateway:
  platforms:
    buzz:
      enabled: true
      whitelist: ["{mixed_case}"]
"#
        );
        let config: Config = serde_yaml::from_str(&yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("buzz").unwrap().whitelist;
        assert_eq!(whitelist, &vec![mixed_case.to_string()]);
    }

    #[test]
    fn whitelist_numeric_and_quoted_forms_of_one_id_are_equal_strings() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345, "12345"]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let whitelist = &config.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(whitelist.len(), 2);
        assert_eq!(whitelist[0], whitelist[1]);
        assert_eq!(whitelist[0], "12345");
    }

    #[test]
    fn whitelist_double_load_is_byte_identical() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [12345, "67890", "U012AB3CD"]
"#;
        let first: Config = serde_yaml::from_str(yaml).unwrap();
        let second: Config = serde_yaml::from_str(yaml).unwrap();
        let first_whitelist = &first.gateway.platforms.get("telegram").unwrap().whitelist;
        let second_whitelist = &second.gateway.platforms.get("telegram").unwrap().whitelist;
        assert_eq!(first_whitelist, second_whitelist);
    }

    #[test]
    fn telegram_default_origin_single_entry_unchanged() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [42]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match config.telegram_default_origin() {
            OriginDecision::Single { platform, chat_id } => {
                assert_eq!(platform, "telegram");
                assert_eq!(chat_id, "42");
            }
            other => panic!("expected OriginDecision::Single, got {other:?}"),
        }
    }

    #[test]
    fn telegram_default_origin_multi_entry_unchanged() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: [42, 43]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        match config.telegram_default_origin() {
            OriginDecision::Multi { whitelist } => {
                assert_eq!(whitelist, vec!["42".to_string(), "43".to_string()]);
            }
            other => panic!("expected OriginDecision::Multi, got {other:?}"),
        }
    }

    #[test]
    fn telegram_default_origin_empty_is_none() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: true
      whitelist: []
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(matches!(
            config.telegram_default_origin(),
            OriginDecision::None
        ));
    }

    #[test]
    fn telegram_default_origin_disabled_is_none() {
        let yaml = r#"
gateway:
  platforms:
    telegram:
      enabled: false
      whitelist: [42]
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
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
        // Phase 36.3.8: messaging joins the default-on core set.
        for name in &["memory", "session", "agent", "skills", "messaging"] {
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
        assert!(
            DEFAULT_TOOLSETS.contains(&"learning"),
            "Phase 33 LEARN-03..05: DEFAULT_TOOLSETS must contain 'learning' \
             (autonomous skill creation via skill_manage; no external prereqs)"
        );
        assert_eq!(
            DEFAULT_TOOLSETS.len(),
            6,
            "DEFAULT_TOOLSETS must contain exactly 6 entries (memory, session, agent, skills, robotics, learning)"
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

    /// D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1 + Phase 36.3.7.10 + Phase 36.3.7.12:
    /// RESERVED_ROLE_NAMES must hold exactly the nine roles (5 from Phase 26 +
    /// summarization from Phase 25.2 + curator from Phase 25.3 + kanban_decomposer
    /// from Phase 36.3.7.10 + kanban_judge from Phase 36.3.7.12).
    #[test]
    fn reserved_role_names_contains_all_nine_roles_with_kanban_judge() {
        assert_eq!(
            RESERVED_ROLE_NAMES.len(),
            9,
            "Phase 36.3.7.12 adds kanban_judge as the 9th reserved role"
        );
        for required in &[
            "vision",
            "compression",
            "session_search",
            "skills_hub",
            "mcp_helper",
            "summarization",
            "curator",
            "kanban_decomposer",
            "kanban_judge",
        ] {
            assert!(
                RESERVED_ROLE_NAMES.contains(required),
                "RESERVED_ROLE_NAMES must contain '{}'",
                required
            );
        }
        // Phase 36.3.7.12 — explicit "WHY the count went up" anchor.
        assert!(
            RESERVED_ROLE_NAMES.contains(&"kanban_judge"),
            "Phase 36.3.7.12 D-05 requires kanban_judge in RESERVED_ROLE_NAMES"
        );
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
            ..Default::default()
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
            ..Default::default()
        };
        let merged = cfg.with_default_toolsets_merged();
        // web stays disabled.
        assert!(
            !merged.toolsets["web"].enabled,
            "explicit web: disabled must be preserved after merge"
        );
        // robotics (absent) is enabled.
        assert!(
            merged
                .toolsets
                .get("robotics")
                .map(|e| e.enabled)
                .unwrap_or(false),
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
            ..Default::default()
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
            ..Default::default()
        };
        let names = cfg.enabled_toolset_names();
        assert!(names.contains("web"), "web must be in enabled set");
        assert!(names.contains("memory"), "memory must be in enabled set");
        assert!(
            !names.contains("code"),
            "code (disabled) must NOT be in enabled set"
        );
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
            ..Default::default()
        };
        // Apply merge twice.
        let once = cfg.clone().with_default_toolsets_merged();
        let twice = cfg
            .with_default_toolsets_merged()
            .with_default_toolsets_merged();
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

    // =========================================================================
    // Phase 36.17.9 Plan 01 — Wave A config tests (D-08 / D-10 / back-compat)
    // =========================================================================

    /// Phase 36.17.9 (D-08): each BargeInMode variant round-trips; default is
    /// PushToInterrupt and serializes as "push_to_interrupt" (snake_case).
    #[test]
    fn test_barge_in_mode_serde() {
        use crate::config::BargeInMode;
        // Default must be PushToInterrupt.
        let default_mode = BargeInMode::default();
        assert_eq!(
            default_mode,
            BargeInMode::PushToInterrupt,
            "D-08: default must be PushToInterrupt"
        );

        // Serialize default → snake_case string.
        let json = serde_json::to_string(&BargeInMode::PushToInterrupt)
            .expect("serialize PushToInterrupt");
        assert_eq!(
            json, r#""push_to_interrupt""#,
            "D-08: PushToInterrupt must serialize as push_to_interrupt"
        );

        // All three variants round-trip.
        for variant in [
            BargeInMode::PushToInterrupt,
            BargeInMode::HalfDuplex,
            BargeInMode::OpenMic,
        ] {
            let j = serde_json::to_string(&variant).expect("serialize BargeInMode variant");
            let parsed: BargeInMode =
                serde_json::from_str(&j).expect("deserialize BargeInMode variant");
            assert_eq!(parsed, variant, "D-08: BargeInMode variant must round-trip");
        }
    }

    /// Phase 36.17.9 (D-10): WakeWordConfig defaults to enabled=false, phrase="hey hermes".
    #[test]
    fn test_wake_word_config_defaults() {
        use crate::config::WakeWordConfig;
        let ww = WakeWordConfig::default();
        assert!(
            !ww.enabled,
            "D-10: WakeWordConfig must default to enabled=false"
        );
        assert_eq!(
            ww.phrase, "hey hermes",
            "D-10: WakeWordConfig phrase must default to 'hey hermes'"
        );
    }

    /// Phase 36.17.9 back-compat: a VoiceConfig YAML body lacking barge_in_mode
    /// and wake_word must deserialize cleanly with defaults applied (no new required fields).
    #[test]
    fn test_voice_config_legacy_parse() {
        // A pre-36.17.9 VoiceConfig YAML — no barge_in_mode, no wake_word.
        let yaml = r#"
record_key: "ctrl+b"
auto_tts: false
"#;
        let vc: VoiceConfig = serde_yaml::from_str(yaml)
            .expect("pre-36.17.9 VoiceConfig YAML must parse with new fields defaulting");
        // New fields must take their defaults.
        assert_eq!(
            vc.barge_in_mode,
            crate::config::BargeInMode::PushToInterrupt,
            "D-08: legacy VoiceConfig must default barge_in_mode to PushToInterrupt"
        );
        assert!(
            !vc.wake_word.enabled,
            "D-10: legacy VoiceConfig must default wake_word.enabled to false"
        );
        assert_eq!(
            vc.wake_word.phrase, "hey hermes",
            "D-10: legacy VoiceConfig must default wake_word.phrase"
        );
        // Existing fields must still be honoured.
        assert_eq!(vc.record_key, "ctrl+b");
        assert!(!vc.auto_tts);
    }

    /// Phase 36.15 backward compat: pre-36.15 configs (no extra_request_options, no models)
    /// must parse cleanly with the new fields defaulting to empty maps.
    #[test]
    fn pre_36_15_config_yaml_still_parses() {
        let yaml = r#"
providers:
  ollama:
    base_url: "http://localhost:11434/v1"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("pre-36.15 Config must parse");
        let provider = config
            .providers
            .get("ollama")
            .expect("ollama provider must be present");
        assert!(
            provider.extra_request_options.is_empty(),
            "extra_request_options must default to empty map for pre-36.15 configs"
        );
        assert!(
            provider.models.is_empty(),
            "models must default to empty map for pre-36.15 configs"
        );
    }

    // =========================================================================
    // Phase 36.3.12 Plan 01: TerminalConfig exec-backend surface (D-06/D-07/D-09)
    // =========================================================================

    #[test]
    fn terminal_config_default_preserves_local_only_behavior() {
        // D-06: default backend stays "local" — zero change for existing users.
        // D-07: default container_runtime is explicit "docker" (no auto-detect).
        let tc = TerminalConfig::default();
        assert_eq!(tc.backend, "local");
        assert_eq!(tc.container_runtime, "docker");
        assert_eq!(tc.image, "debian:stable-slim");
        assert!(tc.forward_env.is_empty());
        assert_eq!(tc.container_reap_after_secs, 86400);
        assert_eq!(tc.container.cpu, 1.0);
        assert_eq!(tc.container.memory_mib, 5120);
        assert_eq!(tc.container.disk_mib, 51200);
        assert_eq!(tc.container.pids_limit, 256);
        assert!(tc.container.persistent);
        assert!(!tc.container.network);
        assert!(tc.ssh.is_none());
    }

    #[test]
    fn terminal_config_backward_compat_minimal_yaml_parses() {
        // D-06: every existing config.yaml must continue to deserialize unchanged —
        // all new terminal.* fields serde-default.
        let tc: TerminalConfig = serde_yaml::from_str("cwd: /tmp").expect("must parse");
        assert_eq!(tc.cwd, "/tmp");
        assert_eq!(tc.backend, "local");
        assert_eq!(tc.container_runtime, "docker");
        assert!(tc.forward_env.is_empty());
        assert_eq!(tc.container_reap_after_secs, 86400);
        assert!(tc.ssh.is_none());
    }

    #[test]
    fn terminal_config_omitted_from_full_config_yaml_still_parses() {
        let yaml = r#"
web:
  backend: "firecrawl"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.terminal.backend, "local");
        assert_eq!(c.terminal.container_runtime, "docker");
    }
}

#[cfg(test)]
mod extras_canary {
    //! Wave 0 (Phase 36.15) canary tests. These lock the D-03 YAML deserialization shape.
    //!
    //! D-01 DEVIATION applied: the original plan used `#[serde(untagged)] enum ProviderExtraOptions`
    //! on ProviderConfig.extra_request_options. This was tested and FAILED for the OpenRouter
    //! variant (serde_yaml with all-Optional fields always matches the first variant — Pitfall 1).
    //! The fallback `HashMap<String, serde_json::Value>` is now in use. These tests have been
    //! updated accordingly to use .get("key") HashMap access instead of enum pattern matching.
    use super::*;

    /// Test 1: provider-level extra_request_options.num_ctx = 8192 round-trips through Config.
    ///
    /// Locks the D-03 acceptance shape: a YAML doc with
    /// `providers.ollama.extra_request_options.num_ctx = 8192` must deserialize into Config
    /// such that `config.providers["ollama"].extra_request_options["num_ctx"] == 8192`.
    #[test]
    fn extras_canary_provider_level_num_ctx_roundtrips() {
        let yaml = r#"
providers:
  ollama:
    base_url: "http://localhost:11434"
    extra_request_options:
      num_ctx: 8192
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let provider = config
            .providers
            .get("ollama")
            .expect("ollama provider must be present");
        assert!(
            !provider.extra_request_options.is_empty(),
            "extra_request_options must be non-empty"
        );
        assert_eq!(
            provider.extra_request_options.get("num_ctx"),
            Some(&serde_json::json!(8192)),
            "provider-level num_ctx must round-trip as 8192"
        );
    }

    /// Test 2: per-model override wins; provider-level default preserved at deserialization.
    ///
    /// Locks D-03: YAML with provider-level num_ctx=8192 AND
    /// providers.ollama.models."llama3.1:8b".extra_request_options.num_ctx=32768
    /// must deserialize such that:
    ///   - the per-model entry's num_ctx == 32768
    ///   - the provider-level num_ctx == 8192 (merge happens in resolve_extras, not at deserialization)
    #[test]
    fn extras_canary_per_model_override_wins() {
        let yaml = r#"
providers:
  ollama:
    base_url: "http://localhost:11434"
    extra_request_options:
      num_ctx: 8192
    models:
      "llama3.1:8b":
        extra_request_options:
          num_ctx: 32768
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let provider = config
            .providers
            .get("ollama")
            .expect("ollama provider must be present");

        // Provider-level num_ctx must remain 8192 (deserialization only; merge happens in resolve_extras).
        assert_eq!(
            provider.extra_request_options.get("num_ctx"),
            Some(&serde_json::json!(8192)),
            "provider-level num_ctx must be preserved as 8192"
        );

        // Per-model entry must have num_ctx == 32768.
        let model_cfg = provider
            .models
            .get("llama3.1:8b")
            .expect("llama3.1:8b model entry must be present");
        assert_eq!(
            model_cfg.extra_request_options.get("num_ctx"),
            Some(&serde_json::json!(32768)),
            "per-model num_ctx must be 32768"
        );
    }

    /// Test 3: YAML key with colon ("llama3.1:8b") parses without serde error.
    ///
    /// Locks the quoted-key requirement: YAML keys containing colons must be quoted.
    /// Verifies that serde_yaml handles the quoted key `"llama3.1:8b"` in providers.models.
    #[test]
    fn extras_canary_quoted_yaml_key_with_colon_parses() {
        let yaml = r#"
providers:
  ollama:
    models:
      "llama3.1:8b":
        extra_request_options:
          num_ctx: 32768
      "llama3.1:70b":
        extra_request_options:
          num_ctx: 4096
"#;
        let config: Config =
            serde_yaml::from_str(yaml).expect("Config must parse with colon-bearing keys");
        let provider = config
            .providers
            .get("ollama")
            .expect("ollama provider must be present");
        assert!(
            provider.models.contains_key("llama3.1:8b"),
            "models map must contain key 'llama3.1:8b' (colon in key)"
        );
        assert!(
            provider.models.contains_key("llama3.1:70b"),
            "models map must contain key 'llama3.1:70b' (colon in key)"
        );
    }

    /// Test 4: OpenRouter nested provider.order list round-trips.
    ///
    /// Locks D-03: YAML with providers.openrouter.extra_request_options.provider.order =
    /// ["anthropic", "openai"] must deserialize such that the nested order object is preserved.
    /// Under the D-01 fallback (HashMap deserialization), the nested `provider` object becomes
    /// a serde_json::Value::Object, and `order` is a Value::Array — both survive the round-trip.
    #[test]
    fn extras_canary_openrouter_provider_order_nested() {
        let yaml = r#"
providers:
  openrouter:
    base_url: "https://openrouter.ai/api/v1"
    extra_request_options:
      provider:
        order:
          - anthropic
          - openai
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("Config must parse");
        let provider = config
            .providers
            .get("openrouter")
            .expect("openrouter provider must be present");
        assert!(
            !provider.extra_request_options.is_empty(),
            "extra_request_options must be non-empty for openrouter"
        );
        let provider_val = provider
            .extra_request_options
            .get("provider")
            .expect("provider key must be present in extras");
        let order = provider_val
            .as_object()
            .and_then(|o| o.get("order"))
            .and_then(|v| v.as_array())
            .expect("provider.order must be a JSON array");
        assert_eq!(
            order,
            &vec![
                serde_json::Value::String("anthropic".to_string()),
                serde_json::Value::String("openai".to_string()),
            ],
            "OpenRouter provider.order must round-trip as [anthropic, openai]"
        );
    }

    // =========================================================================
    // Phase 36.17.10 Plan 01 Task 1: atomic save_to + new config schema fields
    // =========================================================================

    /// Verifies that save_to writes through a temp file (`path.with_extension("yaml.tmp")`)
    /// and renames it over the target, and that no `.yaml.tmp` sibling remains after save.
    #[test]
    fn save_to_atomic() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.yaml");
        let config = Config::default();

        config.save_to(&config_path).expect("save_to must succeed");

        // No .yaml.tmp sibling must remain after atomic rename
        let tmp_path = config_path.with_extension("yaml.tmp");
        assert!(
            !tmp_path.exists(),
            "No .yaml.tmp file should remain after save_to; tmp path: {tmp_path:?}"
        );

        // Round-trip: the file must parse back to an equal Config
        let reloaded = Config::load_from(&config_path).expect("reload must succeed");
        // Compare key fields (Config doesn't derive PartialEq, so check representative fields)
        assert_eq!(
            reloaded.voice.silence_threshold, config.voice.silence_threshold,
            "silence_threshold must survive round-trip"
        );
        assert_eq!(
            reloaded.voice.silence_duration, config.voice.silence_duration,
            "silence_duration must survive round-trip"
        );
        assert_eq!(
            reloaded.voice.web_silence_threshold_rms, config.voice.web_silence_threshold_rms,
            "web_silence_threshold_rms must survive round-trip"
        );
        assert_eq!(
            reloaded.security.web_config_write_enabled, config.security.web_config_write_enabled,
            "web_config_write_enabled must survive round-trip"
        );
    }

    /// VoiceConfig::default().web_silence_threshold_rms must be 5.0 (Web Audio byte-domain).
    /// Deserializing a config.yaml that omits web_silence_threshold_rms must also yield 5.0.
    /// The existing native silence_threshold (i32, value 200) must remain untouched.
    #[test]
    fn voice_config_web_vad_defaults() {
        // Default impl provides 5.0
        let default_vc = VoiceConfig::default();
        assert_eq!(
            default_vc.web_silence_threshold_rms, 5.0f32,
            "web_silence_threshold_rms default must be 5.0"
        );
        // Existing field must not be touched
        assert_eq!(
            default_vc.silence_threshold, 200i32,
            "native silence_threshold must remain 200"
        );

        // Deserialization of a config that omits the new field must apply the 5.0 default
        let yaml = r#"
voice:
  silence_threshold: 200
  silence_duration: 3.0
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");
        assert_eq!(
            config.voice.web_silence_threshold_rms, 5.0f32,
            "missing web_silence_threshold_rms in YAML must fall back to 5.0"
        );
        assert_eq!(
            config.voice.silence_threshold, 200i32,
            "native silence_threshold must remain 200 when deserialized"
        );
    }

    /// SecurityConfig::default().web_config_write_enabled must be false.
    /// Deserializing a config.yaml that omits web_config_write_enabled must also yield false.
    #[test]
    fn security_web_write_default_false() {
        let default_sc = SecurityConfig::default();
        assert!(
            !default_sc.web_config_write_enabled,
            "web_config_write_enabled default must be false"
        );

        // Deserialization without the field must also yield false
        let yaml = r#"
security:
  redact_secrets: true
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");
        assert!(
            !config.security.web_config_write_enabled,
            "missing web_config_write_enabled in YAML must fall back to false"
        );
    }
}

// =============================================================================
// Phase 40.5 identity config tests (D-10/D-11/D-13/D-16)
// =============================================================================
#[cfg(test)]
mod identity_config_40_5 {
    use super::*;

    /// D-16 / T-40.5-01-01: YAML with an identities: section round-trips cleanly.
    /// The parsed record's free_mode_tts_provider must match what was written.
    #[test]
    fn identities_yaml_round_trip() {
        let yaml = r#"
identities:
  orb_bloom:
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: "pNInz6obpgDQGcFmaJgB"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse failed");
        let rec = config
            .identities
            .get("orb_bloom")
            .expect("missing orb_bloom");
        assert_eq!(
            rec.voice.free_mode_tts_provider.as_deref(),
            Some("elevenlabs")
        );
        assert_eq!(
            rec.voice.free_mode_tts_voice.as_deref(),
            Some("pNInz6obpgDQGcFmaJgB")
        );
    }

    /// D-13 / D-16: YAML with NO identities: key must still yield the curated seed.
    /// This proves serde(default = "default_seed_identities") fires on absence.
    #[test]
    fn legacy_yaml_without_identities_parses() {
        let yaml = r#"
voice:
  auto_tts: true
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse failed");
        // Must yield the curated seed, not an empty map (D-13)
        assert!(
            !config.identities.is_empty(),
            "no identities: section must yield curated seed (D-13), not empty map"
        );
        // orb_bloom must be seeded with a non-None curated voice
        let bloom = config
            .identities
            .get("orb_bloom")
            .expect("orb_bloom missing from curated seed");
        assert!(
            bloom.voice.free_mode_tts_voice.is_some(),
            "curated seed for orb_bloom must carry a non-None free_mode_tts_voice (D-13)"
        );
    }

    /// D-16: PARTIAL identities: section must keep the operator entry AND backfill
    /// shipped seed personas — proving serde(default) replacement is corrected by the
    /// explicit load_from merge loop.
    #[test]
    fn partial_identities_section_backfills_seed() {
        let yaml = r#"
identities:
  orb_bloom:
    voice:
      free_mode_tts_provider: openai
      free_mode_tts_voice: nova
"#;
        // Route through Config::load_from (via temp file) so the merge loop executes.
        let tmp = tempfile::NamedTempFile::new().expect("tmp file");
        std::fs::write(tmp.path(), yaml).expect("write yaml");
        let config = Config::load_from(tmp.path()).expect("load_from");

        // Operator entry preserved (not overwritten by the seed)
        let bloom = config
            .identities
            .get("orb_bloom")
            .expect("orb_bloom missing");
        assert_eq!(
            bloom.voice.free_mode_tts_provider.as_deref(),
            Some("openai"),
            "operator-supplied orb_bloom entry must be preserved"
        );

        // Shipped seed persona backfilled — groovy is a curated seed persona
        assert!(
            config.identities.contains_key("groovy"),
            "groovy missing — load_from merge must backfill missing shipped seed personas (D-16)"
        );
    }

    /// D-11: An identity whose voice fields are all None must fall back to the global provider.
    #[test]
    fn identity_override_inherits_global() {
        let yaml = r#"
tts:
  provider: edge
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");
        // orb_classic may be in the seed with all-None voice fields, or absent — both
        // should return the global provider and no voice override.
        let (provider, voice) = config.resolve_tts_override(Some("orb_classic"));
        assert_eq!(
            provider, "edge",
            "identity with all-None voice must inherit global tts.provider"
        );
        assert!(
            voice.is_none(),
            "all-None identity must yield no voice override"
        );
    }

    /// D-11: An identity with free_mode_tts_provider set must override the global provider.
    #[test]
    fn identity_override_wins() {
        let yaml = r#"
tts:
  provider: edge
identities:
  orb_bloom:
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: "pNInz6obpgDQGcFmaJgB"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");
        let (provider, voice) = config.resolve_tts_override(Some("orb_bloom"));
        assert_eq!(
            provider, "elevenlabs",
            "identity with free_mode_tts_provider must override global provider"
        );
        assert_eq!(
            voice.as_deref(),
            Some("pNInz6obpgDQGcFmaJgB"),
            "identity must return its curated voice"
        );
    }

    /// D-11: effective_tts_config_for_identity must clone TtsConfig with identity
    /// overrides applied to the matching provider sub-config field.
    #[test]
    fn effective_config_applies_override() {
        let yaml = r#"
tts:
  provider: edge
identities:
  orb_bloom:
    voice:
      free_mode_tts_provider: elevenlabs
      free_mode_tts_voice: "pNInz6obpgDQGcFmaJgB"
"#;
        let config: Config = serde_yaml::from_str(yaml).expect("parse");

        // With identity slug: provider + elevenlabs.voice_id must reflect identity values
        let eff = config.effective_tts_config_for_identity(Some("orb_bloom"));
        assert_eq!(
            eff.provider, "elevenlabs",
            "effective config provider must be the identity override"
        );
        assert_eq!(
            eff.elevenlabs.voice_id, "pNInz6obpgDQGcFmaJgB",
            "effective config elevenlabs.voice_id must be the identity's free_mode_tts_voice"
        );

        // Without slug: global TtsConfig unchanged
        let global = config.effective_tts_config_for_identity(None);
        assert_eq!(
            global.provider, "edge",
            "effective_tts_config_for_identity(None) must return global tts unchanged"
        );
    }

    /// D-13: default_seed_identities must include at least one persona with a non-None voice.
    #[test]
    fn curated_seed_present() {
        let seed = default_seed_identities();
        let has_curated = seed.values().any(|r| r.voice.free_mode_tts_voice.is_some());
        assert!(
            has_curated,
            "default_seed_identities must include at least one curated voice (D-13)"
        );
    }

    /// Phase 45 D-04: ApprovalsGatewayConfig default timeout is 120 seconds.
    #[test]
    fn approvals_gateway_config_default_timeout() {
        let cfg = ApprovalsGatewayConfig::default();
        assert_eq!(
            cfg.timeout_secs, 120,
            "ApprovalsGatewayConfig default must be 120s (D-04: '2 minutes')"
        );
    }

    /// Phase 45 D-10: McpMutationGuardrailConfig default patterns list is empty (use built-in DEFAULT_VERBS).
    #[test]
    fn mcp_mutation_guardrail_config_default_empty() {
        let cfg = McpMutationGuardrailConfig::default();
        assert!(
            cfg.patterns.is_empty(),
            "McpMutationGuardrailConfig default must be empty (D-10: empty = use DEFAULT_VERBS)"
        );
    }

    // =========================================================================
    // Phase 46.9 Plan 11 (GAP-4/GAP-5): DisplayConfig serde-default backward compat
    // =========================================================================

    #[test]
    fn config_yaml_without_display_section_parses_with_defaults() {
        // GAP-4/GAP-5: pre-46.9-11 config.yaml files have no `display:` key at
        // all — must parse cleanly with DisplayConfig::default() (no required keys).
        let yaml = r#"
web:
  backend: "firecrawl"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.display.timezone, None);
        assert!(c.display.extra_timezones.is_empty());
    }

    #[test]
    fn config_default_has_display_none() {
        let c = Config::default();
        assert_eq!(c.display.timezone, None);
        assert!(c.display.extra_timezones.is_empty());
    }

    #[test]
    fn config_yaml_partial_display_section_uses_defaults_for_rest() {
        let yaml = r#"
display:
  timezone: "America/New_York"
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.display.timezone, Some("America/New_York".to_string()));
        assert!(c.display.extra_timezones.is_empty()); // default
    }

    #[test]
    fn display_hour12_defaults_none_and_parses_true() {
        // QDG-01: hour12 defaults to None (24-hour, unchanged behavior); a
        // config.yaml with `display.hour12: true` parses to Some(true).
        assert_eq!(Config::default().display.hour12, None);

        let yaml = r#"
display:
  hour12: true
"#;
        let c: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(c.display.hour12, Some(true));
    }

    // =========================================================================
    // Phase 41.3 Plan 05 (D-08/D-17): web tool chain config schema
    // =========================================================================

    #[test]
    fn default_chains_preserve_shipped_order() {
        let cfg = ToolsConfig::default();
        assert_eq!(
            cfg.web_extract.chain,
            vec!["firecrawl", "exa", "tavily", "local"],
            "D-17: default web_extract chain must reproduce the fixed order that \
             shipped in Phase 25.2 D-04 exactly, so an operator who upgrades \
             without editing config sees no behavior change"
        );
    }

    #[test]
    fn default_search_and_answer_chains_terminate_in_ddg() {
        let cfg = ToolsConfig::default();
        assert_eq!(
            cfg.web_search.chain.last().map(String::as_str),
            Some("ddg"),
            "D-09/D-10: web_search chain must terminate in the keyless ddg \
             backend so nothing hard-fails on a fresh install"
        );
        assert_eq!(
            cfg.web_answer.chain.last().map(String::as_str),
            Some("ddg"),
            "D-09/D-10: web_answer chain must terminate in the keyless ddg \
             backend so nothing hard-fails on a fresh install"
        );
    }

    // =========================================================================
    // Phase 47.3 Plan 01 (D-06): web_ui.auth.* config section
    // =========================================================================

    /// D-06: defaults must be auth-disabled (`password_hash: None`), theme
    /// `basic`, `cookie_secure: false`, 168h/24h TTLs — matches today's
    /// no-auth posture byte-for-byte until an operator opts in.
    #[test]
    fn web_ui_auth_defaults_are_auth_disabled() {
        let cfg = WebUiAuthConfig::default();
        assert!(
            cfg.password_hash.is_none(),
            "default password_hash must be None (auth opt-in, not opt-out)"
        );
        assert_eq!(cfg.login_theme, "basic");
        assert!(!cfg.cookie_secure);
        assert_eq!(cfg.session_ttl_hours, 168);
        assert_eq!(cfg.idle_timeout_hours, 24);
    }

    /// D-06: a config.yaml written before Phase 47.3 (no `web_ui:` key at
    /// all) must still deserialize cleanly via `#[serde(default)]`, exactly
    /// like the `web_config_write_enabled` precedent above.
    #[test]
    fn web_ui_missing_from_yaml_falls_back_to_defaults() {
        let yaml = "model: {}\n";
        let config: Config = serde_yaml::from_str(yaml).expect("Config must deserialize");
        assert!(
            config.web_ui.auth.password_hash.is_none(),
            "missing web_ui in YAML must fall back to auth-disabled defaults"
        );
        assert_eq!(config.web_ui.auth.login_theme, "basic");
    }

    /// D-06: `web_ui.auth.*` round-trips through YAML serialization —
    /// guards against a future field rename silently breaking persistence.
    #[test]
    fn web_ui_auth_round_trips_through_yaml() {
        let mut config = Config::default();
        config.web_ui.auth.password_hash = Some("$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".to_string());
        config.web_ui.auth.login_theme = "matrix-rain".to_string();
        config.web_ui.auth.cookie_secure = true;

        let yaml = serde_yaml::to_string(&config).expect("Config must serialize");
        let reloaded: Config = serde_yaml::from_str(&yaml).expect("Config must deserialize");

        assert_eq!(reloaded.web_ui.auth.password_hash, config.web_ui.auth.password_hash);
        assert_eq!(reloaded.web_ui.auth.login_theme, config.web_ui.auth.login_theme);
        assert_eq!(reloaded.web_ui.auth.cookie_secure, config.web_ui.auth.cookie_secure);
    }

    /// D-06 prohibition: operator credentials must NOT live on `WebConfig`
    /// (the unrelated web-*browsing* config) — `web_ui` is its own top-level
    /// struct, never a field nested inside `web:`.
    #[test]
    fn web_ui_auth_is_not_nested_inside_web_config() {
        // WebConfig's field list is fixed at 4 fields; this test fails to
        // compile (not just fails at runtime) if an `auth` field is ever
        // added to WebConfig, since the struct literal below would then be
        // missing a field.
        let _ = WebConfig {
            backend: String::new(),
            user_agent: String::new(),
            max_content_chars: 0,
            timeout_secs: 0,
        };
    }

    #[test]
    fn validate_chains_accepts_the_shipped_defaults() {
        let cfg = ToolsConfig::default();
        assert!(
            cfg.validate_chains().is_ok(),
            "the shipped default chains must be valid"
        );
    }

    #[test]
    fn validate_chains_rejects_an_unknown_provider_named_per_tool() {
        let mut cfg = ToolsConfig::default();
        cfg.web_search.chain = vec!["exa".to_string(), "gooogle".to_string()];
        let errors = cfg
            .validate_chains()
            .expect_err("unknown provider must be rejected");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("gooogle") && e.contains("web_search")),
            "error must name both the offending value and the tool so an \
             operator can find the typo without bisecting; got: {:?}",
            errors
        );
    }

    #[test]
    fn validate_chains_rejects_a_provider_valid_for_a_different_tool() {
        let mut cfg = ToolsConfig::default();
        cfg.web_search.chain = vec!["local".to_string()];
        let errors = cfg
            .validate_chains()
            .expect_err("`local` is a web_extract backend, not a web_search provider");
        assert!(
            errors
                .iter()
                .any(|e| e.contains("local") && e.contains("web_search")),
            "the legal provider set must be per-tool, not one global union; got: {:?}",
            errors
        );
    }

    #[test]
    fn validate_chains_rejects_an_empty_chain() {
        let mut cfg = ToolsConfig::default();
        cfg.web_extract.chain = vec![];
        let errors = cfg
            .validate_chains()
            .expect_err("an explicitly-empty chain must be an error, not a silent fall-through");
        assert!(
            errors.iter().any(|e| e.contains("web_extract")),
            "error must name the empty tool's chain; got: {:?}",
            errors
        );
    }

    #[test]
    fn chain_config_round_trips_through_yaml() {
        let mut cfg = ToolsConfig::default();
        cfg.web_extract.chain = vec!["exa".to_string(), "local".to_string()];
        cfg.web_search.chain = vec!["tavily".to_string(), "ddg".to_string()];
        cfg.web_answer.chain = vec!["perplexity".to_string(), "ddg".to_string()];

        let yaml = serde_yaml::to_string(&cfg).expect("ToolsConfig must serialize");
        let roundtripped: ToolsConfig =
            serde_yaml::from_str(&yaml).expect("ToolsConfig must deserialize");

        assert_eq!(roundtripped.web_search.chain, cfg.web_search.chain);
        assert_eq!(roundtripped.web_answer.chain, cfg.web_answer.chain);
        assert_eq!(roundtripped.web_extract.chain, cfg.web_extract.chain);
    }
}
