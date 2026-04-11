use crate::constants::{get_hermes_home, DEFAULT_MAX_ITERATIONS, DEFAULT_MODEL};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_turns: usize,
    pub context_compression: f64,
    pub tool_delay_secs: f64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_ITERATIONS,
            context_compression: 0.5,
            tool_delay_secs: 1.0,
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    pub platforms: HashMap<String, PlatformGatewayConfig>,
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
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_paths: Vec::new(),
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

    /// Resolve the API base URL from config or environment.
    pub fn resolve_base_url(&self) -> String {
        if let Some(ref url) = self.model.base_url {
            return url.clone();
        }
        if let Ok(url) = std::env::var("OPENAI_BASE_URL") {
            return url;
        }
        crate::constants::OPENROUTER_BASE_URL.to_string()
    }

    /// Resolve the API key from config or environment.
    pub fn resolve_api_key(&self) -> Option<String> {
        if let Some(ref key) = self.model.api_key {
            return Some(key.clone());
        }
        // Try provider-specific keys first
        match self.model.provider.as_str() {
            "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
            "openai" => std::env::var("OPENAI_API_KEY").ok(),
            _ => std::env::var("OPENROUTER_API_KEY")
                .or_else(|_| std::env::var("OPENAI_API_KEY"))
                .ok(),
        }
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
}
