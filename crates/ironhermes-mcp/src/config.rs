use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a single MCP server (D-17: matches hermes-agent YAML schema).
///
/// Supports both stdio transport (`command`/`args`/`env`) and HTTP transport (`url`/`headers`).
/// Common options: `timeout`, `connect_timeout`, `enabled`, `enabled_tools`, `auth`, `sampling`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    // --- stdio transport ---
    /// Command to run for stdio-based MCP servers (e.g., "npx").
    pub command: Option<String>,
    /// Arguments for the stdio command (e.g., ["-y", "@modelcontextprotocol/server-filesystem"]).
    pub args: Vec<String>,
    /// Environment variables passed to the stdio subprocess (filtered by build_safe_env).
    pub env: HashMap<String, String>,

    // --- HTTP transport ---
    /// URL for HTTP/StreamableHTTP-based MCP servers.
    pub url: Option<String>,
    /// HTTP headers to include in requests.
    pub headers: HashMap<String, String>,

    // --- common options ---
    /// Tool call timeout in seconds. Default: 120.
    pub timeout: u64,
    /// Connection timeout in seconds. Default: 60.
    pub connect_timeout: u64,
    /// Whether this server is enabled. Default: true.
    pub enabled: bool,
    /// D-08: if set, only these tool names are registered from this server.
    pub enabled_tools: Option<Vec<String>>,
    /// Optional auth token or bearer credential.
    pub auth: Option<String>,
    /// Sampling/createMessage configuration.
    pub sampling: Option<SamplingConfig>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: None,
            headers: HashMap::new(),
            timeout: 120,
            connect_timeout: 60,
            enabled: true,
            enabled_tools: None,
            auth: None,
            sampling: None,
        }
    }
}

/// Sampling (server-initiated LLM completion) configuration (D-03).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    /// Whether sampling is enabled for this server. Default: false.
    pub enabled: bool,
    /// Maximum requests per minute for sampling. Default: 10.
    pub max_rpm: u32,
    /// Maximum tool rounds per sampling request. Default: 5.
    pub max_tool_rounds: u32,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_rpm: 10,
            max_tool_rounds: 5,
        }
    }
}

/// Replace `${ENV_VAR}` placeholders with values from `std::env`.
///
/// Unresolvable variables are left as literal `"${VAR}"` text.
/// D-18: interpolation from `std::env` only — no shell expansion, no command substitution.
pub fn interpolate_env(value: &str) -> String {
    let re = regex::Regex::new(r"\$\{([^}]+)\}").unwrap();
    re.replace_all(value, |caps: &regex::Captures| {
        std::env::var(&caps[1]).unwrap_or_else(|_| caps[0].to_string())
    })
    .into_owned()
}

/// Apply env interpolation to all string fields in a `McpServerConfig`.
///
/// Mutates in-place: command, url, args, env values, header values, and auth.
pub fn interpolate_config(config: &mut McpServerConfig) {
    if let Some(ref mut cmd) = config.command {
        *cmd = interpolate_env(cmd);
    }
    if let Some(ref mut url) = config.url {
        *url = interpolate_env(url);
    }
    config.args = config.args.iter().map(|a| interpolate_env(a)).collect();
    let env_copy: HashMap<String, String> = config
        .env
        .iter()
        .map(|(k, v)| (k.clone(), interpolate_env(v)))
        .collect();
    config.env = env_copy;
    let hdr_copy: HashMap<String, String> = config
        .headers
        .iter()
        .map(|(k, v)| (k.clone(), interpolate_env(v)))
        .collect();
    config.headers = hdr_copy;
    if let Some(ref mut auth) = config.auth {
        *auth = interpolate_env(auth);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // McpServerConfig deserialization tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_mcp_server_config_stdio_deserialize() {
        let yaml = r#"
command: npx
args:
  - "-y"
  - "@modelcontextprotocol/server-github"
env:
  GITHUB_TOKEN: "ghp_test"
"#;
        let cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.command.as_deref(), Some("npx"));
        assert_eq!(cfg.args, vec!["-y", "@modelcontextprotocol/server-github"]);
        assert_eq!(
            cfg.env.get("GITHUB_TOKEN").map(|s| s.as_str()),
            Some("ghp_test")
        );
        assert!(cfg.url.is_none());
    }

    #[test]
    fn test_mcp_server_config_http_deserialize() {
        let yaml = r#"
url: "https://mcp.example.com/v1"
headers:
  Authorization: "Bearer token123"
"#;
        let cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.url.as_deref(), Some("https://mcp.example.com/v1"));
        assert_eq!(
            cfg.headers.get("Authorization").map(|s| s.as_str()),
            Some("Bearer token123")
        );
        assert!(cfg.command.is_none());
    }

    #[test]
    fn test_mcp_server_config_defaults() {
        let cfg = McpServerConfig::default();
        assert_eq!(cfg.timeout, 120);
        assert_eq!(cfg.connect_timeout, 60);
        assert!(cfg.enabled);
        assert!(cfg.command.is_none());
        assert!(cfg.url.is_none());
        assert!(cfg.args.is_empty());
        assert!(cfg.env.is_empty());
        assert!(cfg.headers.is_empty());
        assert!(cfg.enabled_tools.is_none());
        assert!(cfg.auth.is_none());
        assert!(cfg.sampling.is_none());
    }

    #[test]
    fn test_mcp_server_config_defaults_from_yaml_empty() {
        // Empty YAML should produce all defaults
        let cfg: McpServerConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(cfg.timeout, 120);
        assert_eq!(cfg.connect_timeout, 60);
        assert!(cfg.enabled);
    }

    // -------------------------------------------------------------------------
    // SamplingConfig tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_sampling_config_defaults() {
        let sc = SamplingConfig::default();
        assert!(!sc.enabled);
        assert_eq!(sc.max_rpm, 10);
        assert_eq!(sc.max_tool_rounds, 5);
    }

    #[test]
    fn test_sampling_config_deserialize() {
        let yaml = r#"
enabled: true
max_rpm: 20
max_tool_rounds: 3
"#;
        let sc: SamplingConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(sc.enabled);
        assert_eq!(sc.max_rpm, 20);
        assert_eq!(sc.max_tool_rounds, 3);
    }

    #[test]
    fn test_enabled_tools_roundtrip() {
        let yaml = r#"
command: npx
enabled_tools:
  - "read_file"
  - "write_file"
"#;
        let cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        let tools = cfg.enabled_tools.as_ref().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0], "read_file");
        assert_eq!(tools[1], "write_file");

        // Round-trip serialize → deserialize
        let ser = serde_yaml::to_string(&cfg).unwrap();
        let re: McpServerConfig = serde_yaml::from_str(&ser).unwrap();
        assert_eq!(re.enabled_tools, cfg.enabled_tools);
    }

    // -------------------------------------------------------------------------
    // interpolate_env tests (D-18)
    // -------------------------------------------------------------------------

    #[test]
    fn test_interpolate_env_replaces_home() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/test".to_string());
        let result = interpolate_env("${HOME}/bin");
        assert_eq!(result, format!("{}/bin", home));
    }

    #[test]
    fn test_interpolate_env_leaves_missing_var_literal() {
        // NONEXISTENT_VAR_XYZ_123 should not exist in any test environment
        let result = interpolate_env("${NONEXISTENT_VAR_XYZ_123}/path");
        assert_eq!(result, "${NONEXISTENT_VAR_XYZ_123}/path");
    }

    #[test]
    fn test_interpolate_env_multiple_placeholders() {
        // Set up: use HOME and USER which are almost always present
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/test".to_string());
        let user = std::env::var("USER").unwrap_or_else(|_| "testuser".to_string());
        let input = "${HOME}/users/${USER}/config";
        let result = interpolate_env(input);
        assert_eq!(result, format!("{}/users/{}/config", home, user));
    }

    #[test]
    fn test_interpolate_env_no_placeholders() {
        let result = interpolate_env("plain text with no placeholders");
        assert_eq!(result, "plain text with no placeholders");
    }

    #[test]
    fn test_interpolate_env_empty_string() {
        let result = interpolate_env("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_interpolate_config_applies_to_all_fields() {
        // SAFETY: test-only env mutation; unique key unlikely to conflict with parallel tests.
        unsafe { std::env::set_var("IH_MCP_TEST_TOKEN", "test_token_value") };
        let yaml = r#"
command: "npx"
args:
  - "--token=${IH_MCP_TEST_TOKEN}"
env:
  TOKEN: "${IH_MCP_TEST_TOKEN}"
headers:
  Authorization: "Bearer ${IH_MCP_TEST_TOKEN}"
auth: "${IH_MCP_TEST_TOKEN}"
"#;
        let mut cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        interpolate_config(&mut cfg);
        assert_eq!(cfg.args[0], "--token=test_token_value");
        assert_eq!(
            cfg.env.get("TOKEN").map(|s| s.as_str()),
            Some("test_token_value")
        );
        assert_eq!(
            cfg.headers.get("Authorization").map(|s| s.as_str()),
            Some("Bearer test_token_value")
        );
        assert_eq!(cfg.auth.as_deref(), Some("test_token_value"));
        unsafe { std::env::remove_var("IH_MCP_TEST_TOKEN") };
    }
}
