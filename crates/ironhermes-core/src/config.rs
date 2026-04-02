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
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            backend: "firecrawl".to_string(),
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
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
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
