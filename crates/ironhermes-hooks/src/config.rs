use ironhermes_core::constants::get_hermes_home;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Detail level for error messages in hook events.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ErrorDetailLevel {
    /// Include full error details (default).
    #[default]
    Full,
    /// Include minimal error info (omit stack traces, internal paths).
    Minimal,
}

/// Configuration for a single webhook endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WebhookEndpointConfig {
    /// Target URL for POST delivery.
    pub url: String,
    /// Event kinds to deliver to this endpoint (empty = all events).
    pub events: Vec<String>,
    /// Static Authorization header value (e.g. "Bearer token123").
    pub auth_header: Option<String>,
    /// HMAC-SHA256 secret for request signing.
    pub hmac_secret: Option<String>,
    /// Maximum delivery retries. Default: 5.
    pub max_retries: Option<u32>,
    /// Time-to-live for queued events in hours. Default: 24.
    pub queue_ttl_hours: Option<u32>,
}

/// Configuration for the JSONL event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EventLogConfig {
    /// Whether event logging is enabled. Default: true.
    pub enabled: bool,
    /// Override path for events.jsonl. None = ~/.ironhermes/hooks/events.jsonl.
    pub path: Option<String>,
}

impl Default for EventLogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: None,
        }
    }
}

/// Top-level hooks configuration, loaded from `{hermes_home}/hooks.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HooksConfig {
    /// Event log (JSONL) settings.
    pub event_log: EventLogConfig,
    /// Tool names that the guardrail system should block.
    pub blocked_tools: Vec<String>,
    /// Webhook delivery endpoints (used in Plan 03).
    pub webhooks: Vec<WebhookEndpointConfig>,
    /// Error detail level for hook events.
    pub error_detail: ErrorDetailLevel,
}

impl HooksConfig {
    /// Load config from `{hermes_home}/hooks.toml`, falling back to `Default` if missing.
    pub fn load() -> anyhow::Result<Self> {
        let path = get_hermes_home().join("hooks.toml");
        Self::load_from(&path)
    }

    /// Load config from a specific path (testable override).
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)?;
        let config: HooksConfig = toml::from_str(&content)?;
        Ok(config)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_default_config() {
        let cfg = HooksConfig::default();
        assert!(cfg.event_log.enabled);
        assert!(cfg.event_log.path.is_none());
        assert!(cfg.blocked_tools.is_empty());
        assert!(cfg.webhooks.is_empty());
        assert_eq!(cfg.error_detail, ErrorDetailLevel::Full);
    }

    #[test]
    fn test_load_missing_file_returns_default() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nonexistent_hooks.toml");
        let cfg = HooksConfig::load_from(&path).expect("should not error");
        assert!(cfg.event_log.enabled);
        assert!(cfg.blocked_tools.is_empty());
    }

    #[test]
    fn test_deserialize_hooks_toml() {
        let toml_str = r#"
blocked_tools = ["terminal", "write_file"]
error_detail = "minimal"

[event_log]
enabled = true
path = "/tmp/test_events.jsonl"

[[webhooks]]
url = "https://example.com/hook"
events = ["tool_called", "tool_completed"]
auth_header = "Bearer secret123"
max_retries = 3
queue_ttl_hours = 12
"#;

        let mut tmp = NamedTempFile::new().expect("tempfile");
        write!(tmp, "{}", toml_str).expect("write");

        let cfg = HooksConfig::load_from(tmp.path()).expect("load");

        assert!(cfg.event_log.enabled);
        assert_eq!(
            cfg.event_log.path.as_deref(),
            Some("/tmp/test_events.jsonl")
        );
        assert_eq!(cfg.blocked_tools, vec!["terminal", "write_file"]);
        assert_eq!(cfg.error_detail, ErrorDetailLevel::Minimal);
        assert_eq!(cfg.webhooks.len(), 1);

        let wh = &cfg.webhooks[0];
        assert_eq!(wh.url, "https://example.com/hook");
        assert_eq!(wh.events, vec!["tool_called", "tool_completed"]);
        assert_eq!(wh.auth_header.as_deref(), Some("Bearer secret123"));
        assert_eq!(wh.max_retries, Some(3));
        assert_eq!(wh.queue_ttl_hours, Some(12));
    }
}
