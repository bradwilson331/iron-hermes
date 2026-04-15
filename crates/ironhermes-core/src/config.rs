use crate::constants::{get_hermes_home, DEFAULT_MAX_ITERATIONS, DEFAULT_MODEL};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_mode: Option<ApiMode>,
    pub default_model: Option<String>,
    pub fallback_providers: Vec<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            api_key: None,
            api_mode: None,
            default_model: None,
            fallback_providers: vec![],
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

fn default_agent_engine() -> String { "summarizing".to_string() }
fn default_agent_threshold() -> f32 { 0.5 }
fn default_gateway_engine() -> String { "local_prune".to_string() }
fn default_gateway_threshold() -> f32 { 0.85 }

// =============================================================================
// MemoryConfig (MEM-12)
// =============================================================================

/// Memory provider configuration (D-08, D-09, D-10).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Provider type: "file" (default), "sqlite", "grafeo", "duckdb".
    pub provider: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            provider: "file".to_string(),
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

fn default_session_timeout_hours() -> u64 { 24 }
fn default_max_concurrent_runs() -> usize { 8 }

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
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_paths: Vec::new(),
            credential_dir: None,
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

impl Config {
    /// Load config from the IronHermes home directory.
    pub fn load() -> anyhow::Result<Self> {
        let config_path = get_hermes_home().join("config.yaml");
        Self::load_from(&config_path)
    }

    /// Load config from a specific path, falling back to defaults.
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = serde_yaml::from_str(&content)?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(config.skills.extra_paths[0], PathBuf::from("/tmp/custom-skills"));
        assert_eq!(config.skills.extra_paths[1], PathBuf::from("/opt/shared/skills"));
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
            vec!["terminal".to_string(), "file".to_string(), "web".to_string()],
            "default_toolsets should be [terminal, file, web]"
        );
        assert!(default.model.is_none(), "model should default to None");
        assert!(default.provider.is_none(), "provider should default to None");
        assert!(default.base_url.is_none(), "base_url should default to None");
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
            vec!["terminal".to_string(), "file".to_string(), "web".to_string()]
        );
        assert!(config.subagent.model.is_none());
        assert!(config.subagent.provider.is_none());
        assert!(config.subagent.base_url.is_none());
        assert!(config.subagent.api_key.is_none());
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
        assert!(config.providers.is_empty(), "providers should default to empty map");
        assert!(config.custom_providers.is_empty(), "custom_providers should default to empty vec");
        assert!(config.model.roles.is_empty(), "model.roles should default to empty map");
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
}
