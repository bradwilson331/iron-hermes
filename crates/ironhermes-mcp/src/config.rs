use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for a single MCP server (D-17: matches hermes-agent YAML schema).
///
/// Supports both stdio transport (`command`/`args`/`env`) and HTTP transport (`url`/`headers`).
/// Common options: `timeout`, `connect_timeout`, `enabled`, `enabled_tools`, `auth`.
/// `sampling` also parses here but is DOCUMENTED-INERT — see its own field doc
/// comment and `48.3-DEAD-CONFIG-AUDIT.md`.
///
/// **D-04 field liveness audit (48.3):** every field below carries a marker
/// recording whether it is ALIVE (read by a production call site), FIXED in
/// this phase or 48.3-01 (was dead, now wired), or DOCUMENTED-INERT (parses,
/// has no production reader, and now says so at runtime rather than being
/// silently accepted). See
/// `.planning/phases/48.3-tools-wizard-and-mcp-didn-t-work-for-atomic-mail/48.3-DEAD-CONFIG-AUDIT.md`
/// for the full re-verified table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    // --- stdio transport ---
    /// Command to run for stdio-based MCP servers (e.g., "npx").
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at
    /// `server_task.rs::connect_and_serve` (transport dispatch),
    /// `mcp_config.rs` (CLI connect path), `mcp_admin_api.rs` (import/probe paths).
    pub command: Option<String>,
    /// Arguments for the stdio command (e.g., ["-y", "@modelcontextprotocol/server-filesystem"]).
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at `transport.rs::connect_stdio`.
    pub args: Vec<String>,
    /// Environment variables passed to the stdio subprocess (filtered by build_safe_env).
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at
    /// `transport.rs::connect_stdio` via `security::build_safe_env`.
    pub env: HashMap<String, String>,

    // --- HTTP transport ---
    /// URL for HTTP/StreamableHTTP-based MCP servers.
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at
    /// `server_task.rs::connect_and_serve` (transport dispatch), `mcp_config.rs`
    /// (CLI connect path), `mcp_admin_api.rs` (import/probe paths).
    pub url: Option<String>,
    /// HTTP headers to include in requests.
    ///
    /// Values may use `${VAR}`, resolved from the process environment (the profile
    /// `.env` is loaded into it at startup). A variable that is unset now fails the
    /// connect with a name-only error instead of sending the literal text. Write `.env`
    /// values single-quoted: dotenvy substitutes unquoted `${...}` itself.
    ///
    /// Liveness (48.3 D-04 audit): FIXED in 48.3-01 — read at
    /// `transport.rs::connect_http`. Previously dead on every runtime path
    /// (the triggering Atomic Mail incident): deserialized, env-interpolated,
    /// and asserted on in unit tests, but never applied to the transport.
    pub headers: HashMap<String, String>,

    // --- common options ---
    /// Tool call timeout in seconds. Default: 120.
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at
    /// `server_task.rs::connect_and_serve` (wraps every `client.call_tool()` in
    /// `tokio::time::timeout`).
    pub timeout: u64,
    /// Connection timeout in seconds. Default: 60.
    ///
    /// Liveness (48.3 D-04 audit): FIXED in 48.3-02 Task 1 — read at
    /// `server_task.rs::connect_and_serve`, which now bounds each individual
    /// gateway connect attempt (stdio/OAuth-HTTP/plain-HTTP) by this value with
    /// a 1-second floor. Previously enforced only on the CLI
    /// (`mcp_config.rs::attempt_connect_and_list_with_timeout`) and web-probe
    /// (`mcp_admin_api.rs::run_probe`) paths — never on the gateway's own
    /// auto-start/reconnect path, the exact path the triggering incident hit.
    pub connect_timeout: u64,
    /// Whether this server is enabled. Default: true.
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at `manager.rs`, `boot_gate.rs`,
    /// `mcp_admin_api.rs`.
    pub enabled: bool,
    /// D-08: if set, only these tool names are registered from this server.
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at
    /// `server_task.rs::connect_and_serve` (filters registered tool names).
    pub enabled_tools: Option<Vec<String>>,
    /// Optional auth token or bearer credential.
    ///
    /// Liveness (48.3 D-04 audit): FIXED in 48.3-01 Task 2 — read at
    /// `transport.rs::connect_http`, applied as `Authorization: Bearer <token>`
    /// when no explicit `headers.Authorization` entry is set (explicit header
    /// wins). Previously dead everywhere outside this file's own
    /// env-interpolation and round-trip tests.
    pub auth: Option<String>,
    /// OAuth provider namespace for the rmcp DCR/discovery OAuth path (D-03, Phase 44).
    ///
    /// When `Some`, `server_task` routes to `connect_http_oauth` and the value names
    /// the `auth.json` token namespace (`tokens.<ns>`). DCR is persisted under
    /// `clients.<server-url>`. Does NOT cross-reference `auth.providers.<name>` (D-03).
    /// Never log this value alongside token data (A-1).
    ///
    /// Liveness (48.3 D-04 audit): ALIVE — read at `server_task.rs` (multiple
    /// sites), `transport.rs::build_oauth_manager`, `mcp_admin_api.rs`,
    /// `main.rs`, `tui_rata/event_loop.rs`.
    pub oauth_provider: Option<String>,
    /// Phase 46.1 D-01: per-server issuer pin for MCP-OAuth PRM validation (B-4).
    ///
    /// When `Some`, this single issuer host is **authoritative** for this server —
    /// the global `Config.mcp_oauth.issuer_allowlist` is NOT consulted, and the
    /// built-in baseline is bypassed too (tightest posture: this server can only
    /// talk to its own declared issuer). An empty/whitespace-only value is treated
    /// as absent (falls back to baseline ∪ global) rather than an empty allowlist
    /// (V5 input validation, fail-safe). See `security::resolve_allowed_issuers`.
    ///
    /// Liveness (48.3 D-04 audit): ALIVE (OAuth path only) — read at
    /// `transport.rs::build_oauth_manager`, `mcp_admin_api.rs`.
    pub allowed_issuer: Option<String>,
    /// Sampling/createMessage configuration.
    ///
    /// Liveness (48.3 D-04 audit): DOCUMENTED-INERT — no production reader;
    /// see `48.3-DEAD-CONFIG-AUDIT.md`. `SamplingHandler::new` (`sampling.rs`)
    /// is constructed only from that module's own unit tests; no call site
    /// builds one from a real `McpServerConfig.sampling` value or attaches one
    /// as an MCP client's `createMessage` handler. `server_task.rs`'s
    /// `sampling_inert_warning` emits a startup `tracing::warn!` when this
    /// field is set, so the inertness is audible rather than silent (D-04).
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
            oauth_provider: None,
            allowed_issuer: None,
            sampling: None,
        }
    }
}

/// Sampling (server-initiated LLM completion) configuration (D-03).
///
/// **D-04 note:** this whole struct is reachable only through
/// `McpServerConfig.sampling`, which is DOCUMENTED-INERT — see that field's doc
/// comment and `48.3-DEAD-CONFIG-AUDIT.md`. Its own fields below inherit that
/// same disposition: they parse, but since no production code ever constructs a
/// `SamplingHandler` from a real config value, none of these three fields is
/// read by anything at runtime either.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SamplingConfig {
    /// Whether sampling is enabled for this server. Default: false.
    ///
    /// Liveness (48.3 D-04 audit): DOCUMENTED-INERT — no production reader;
    /// inherits `sampling`'s disposition (see `48.3-DEAD-CONFIG-AUDIT.md`).
    pub enabled: bool,
    /// Maximum requests per minute for sampling. Default: 10.
    ///
    /// Liveness (48.3 D-04 audit): DOCUMENTED-INERT — no production reader;
    /// inherits `sampling`'s disposition (see `48.3-DEAD-CONFIG-AUDIT.md`).
    pub max_rpm: u32,
    /// Maximum tool rounds per sampling request. Default: 5.
    ///
    /// Liveness (48.3 D-04 audit): DOCUMENTED-INERT — no production reader;
    /// inherits `sampling`'s disposition (see `48.3-DEAD-CONFIG-AUDIT.md`).
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

/// One `${VAR}` that is still literal after [`interpolate_config`] ran, i.e. the
/// variable is not set in the process environment.
///
/// Carries the field and the variable NAME only; never a value (T-54.8-06).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedPlaceholder {
    /// Human-readable field, e.g. `header 'Authorization'`, `auth` or `url`.
    pub field: String,
    /// The environment variable name between `${` and `}`.
    pub var: String,
}

fn placeholder_regex() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| regex::Regex::new(r"\$\{([^}]+)\}").expect("static regex is valid"))
}

/// Find every `${VAR}` left in `url`, header values and `auth` of an
/// already-interpolated config (D-18, T-54.8-06). Headers are visited in sorted
/// name order so the result is deterministic.
pub fn unresolved_placeholders(config: &McpServerConfig) -> Vec<UnresolvedPlaceholder> {
    let re = placeholder_regex();
    let mut found = Vec::new();
    let mut scan = |field: String, text: &str| {
        for caps in re.captures_iter(text) {
            found.push(UnresolvedPlaceholder {
                field: field.clone(),
                var: caps[1].to_string(),
            });
        }
    };
    if let Some(url) = &config.url {
        scan("url".to_string(), url);
    }
    let mut header_names: Vec<&String> = config.headers.keys().collect();
    header_names.sort();
    for name in header_names {
        scan(format!("header '{name}'"), &config.headers[name]);
    }
    if let Some(auth) = &config.auth {
        scan("auth".to_string(), auth);
    }
    found
}

/// Fail closed when `url`, a header or `auth` still holds a `${VAR}` after
/// interpolation: sending the literal `${CLOUDFLARE_API_TOKEN}` as a bearer is a
/// silent 401 at best and a credential-shaped string sent to a third party at worst.
///
/// The error text is `<field> references unset environment variable <VAR>` for each
/// entry, joined by `; ` -- names only, never a value and never a whole header.
///
/// This closes the UNSET-variable case only. A variable that IS set and referenced by
/// a config writer is still expanded and sent (T-54.8-43, accepted).
pub fn ensure_placeholders_resolved(config: &McpServerConfig) -> anyhow::Result<()> {
    let unresolved = unresolved_placeholders(config);
    if unresolved.is_empty() {
        return Ok(());
    }
    let message = unresolved
        .iter()
        .map(|p| {
            format!(
                "{} references unset environment variable {}",
                p.field, p.var
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(anyhow::anyhow!(message))
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
    if let Some(ref mut ns) = config.oauth_provider {
        *ns = interpolate_env(ns);
    }
    if let Some(ref mut pin) = config.allowed_issuer {
        *pin = interpolate_env(pin);
    }
}

/// Value substituted for every `env` entry in [`render_mcp_json`].
pub const REDACTED_ENV_VALUE: &str = "REDACTED";

/// Render one server as the `mcp.json` document that the security sidecar screens
/// (Phase 57 D-04(c)): `{"mcpServers":{"<name>":{command,args,url,env}}}`.
///
/// Only the keys the server actually has are present. `env` keeps its variable
/// names (a name such as `LD_PRELOAD` is itself a finding) with every value
/// replaced by [`REDACTED_ENV_VALUE`]. `headers` and `auth` are credentials and are
/// never rendered. `command`, `args` and `url` are rendered as configured, before
/// `${VAR}` expansion, so no expanded secret reaches the screen through them.
pub fn render_mcp_json(name: &str, server: &McpServerConfig) -> String {
    let mut entry = serde_json::Map::new();
    if let Some(command) = &server.command {
        entry.insert("command".to_string(), serde_json::json!(command));
    }
    if !server.args.is_empty() {
        entry.insert("args".to_string(), serde_json::json!(server.args));
    }
    if let Some(url) = &server.url {
        entry.insert("url".to_string(), serde_json::json!(url));
    }
    if !server.env.is_empty() {
        let env: serde_json::Map<String, serde_json::Value> = server
            .env
            .keys()
            .map(|k| (k.clone(), serde_json::json!(REDACTED_ENV_VALUE)))
            .collect();
        entry.insert("env".to_string(), serde_json::Value::Object(env));
    }
    let mut servers = serde_json::Map::new();
    servers.insert(name.to_string(), serde_json::Value::Object(entry));
    let mut root = serde_json::Map::new();
    root.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
    serde_json::Value::Object(root).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // render_mcp_json (Phase 57 plan 08, D-04c): the screened text of a server
    // -------------------------------------------------------------------------

    #[test]
    fn render_mcp_json_shape() {
        let mut env = HashMap::new();
        env.insert(
            "GITHUB_TOKEN".to_string(),
            "ghp_supersecretvalue".to_string(),
        );
        env.insert("LOG_LEVEL".to_string(), "debug".to_string());
        let server = McpServerConfig {
            command: Some("npx".to_string()),
            args: vec![
                "-y".to_string(),
                "@modelcontextprotocol/server-github".to_string(),
            ],
            env,
            url: Some("https://mcp.example.com/mcp".to_string()),
            headers: HashMap::from([("Authorization".to_string(), "Bearer hdrsecret".to_string())]),
            auth: Some("authsecret".to_string()),
            ..Default::default()
        };
        let rendered = render_mcp_json("github", &server);
        let v: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
        let entry = &v["mcpServers"]["github"];
        assert_eq!(entry["command"], "npx");
        assert_eq!(
            entry["args"],
            serde_json::json!(["-y", "@modelcontextprotocol/server-github"])
        );
        assert_eq!(entry["url"], "https://mcp.example.com/mcp");
        assert_eq!(entry["env"]["GITHUB_TOKEN"], "REDACTED");
        assert_eq!(entry["env"]["LOG_LEVEL"], "REDACTED");
        for secret in ["ghp_supersecretvalue", "debug", "hdrsecret", "authsecret"] {
            assert!(
                !rendered.contains(secret),
                "rendered config must not carry {secret}: {rendered}"
            );
        }
        assert!(
            entry.get("headers").is_none() && entry.get("auth").is_none(),
            "headers and auth are credentials and are never rendered: {rendered}"
        );

        // A bare stdio server renders only the keys it has.
        let bare = McpServerConfig {
            command: Some("uvx".to_string()),
            ..Default::default()
        };
        let v: serde_json::Value = serde_json::from_str(&render_mcp_json("py", &bare)).unwrap();
        let entry = v["mcpServers"]["py"].as_object().unwrap();
        assert_eq!(entry.len(), 1, "only command is present: {entry:?}");
    }

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

    // -------------------------------------------------------------------------
    // oauth_provider field tests (D-03, Phase 44)
    // -------------------------------------------------------------------------

    #[test]
    fn test_mcp_server_config_oauth_provider_roundtrip() {
        let yaml = r#"
url: "https://docs.mcp.cloudflare.com/mcp"
oauth_provider: "cloudflare_mcp_docs"
"#;
        let cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.oauth_provider.as_deref(), Some("cloudflare_mcp_docs"));
        // Confirm existing auth field unaffected (D-03: no cross-reference to auth.providers)
        assert!(cfg.auth.is_none());
    }

    #[test]
    fn test_mcp_server_config_oauth_provider_default_none() {
        let cfg = McpServerConfig::default();
        assert!(cfg.oauth_provider.is_none());
    }

    #[test]
    fn test_cloudflare_mcp_server_entries_roundtrip() {
        // D-04/CFL-02 (Phase 46): the 4 Cloudflare control-plane servers.
        // `cloudflare_api`'s url is the ROOT domain, NOT api.mcp.cloudflare.com
        // (which does not resolve — RESEARCH Pitfall 4).
        let cases = [
            (
                r#"
url: "https://mcp.cloudflare.com/mcp"
oauth_provider: "cloudflare_api"
"#,
                "https://mcp.cloudflare.com/mcp",
                "cloudflare_api",
            ),
            (
                r#"
url: "https://bindings.mcp.cloudflare.com/mcp"
oauth_provider: "cloudflare_bindings"
"#,
                "https://bindings.mcp.cloudflare.com/mcp",
                "cloudflare_bindings",
            ),
            (
                r#"
url: "https://observability.mcp.cloudflare.com/mcp"
oauth_provider: "cloudflare_observability"
"#,
                "https://observability.mcp.cloudflare.com/mcp",
                "cloudflare_observability",
            ),
            (
                r#"
url: "https://builds.mcp.cloudflare.com/mcp"
oauth_provider: "cloudflare_builds"
"#,
                "https://builds.mcp.cloudflare.com/mcp",
                "cloudflare_builds",
            ),
        ];

        for (yaml, expected_url, expected_oauth_provider) in cases {
            let cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
            assert_eq!(cfg.url.as_deref(), Some(expected_url));
            assert_eq!(cfg.oauth_provider.as_deref(), Some(expected_oauth_provider));
        }
    }

    #[test]
    fn test_interpolate_config_expands_oauth_provider() {
        // SAFETY: test-only env mutation; unique key unlikely to conflict with parallel tests.
        unsafe { std::env::set_var("IH_MCP_TEST_NS", "cf_docs") };
        let yaml = r#"
url: "https://docs.mcp.cloudflare.com/mcp"
oauth_provider: "${IH_MCP_TEST_NS}"
"#;
        let mut cfg: McpServerConfig = serde_yaml::from_str(yaml).unwrap();
        interpolate_config(&mut cfg);
        assert_eq!(cfg.oauth_provider.as_deref(), Some("cf_docs"));
        unsafe { std::env::remove_var("IH_MCP_TEST_NS") };
    }

    // -------------------------------------------------------------------------
    // Fail-closed placeholder checks (54.8-02, D-18, T-54.8-06)
    // -------------------------------------------------------------------------

    #[test]
    fn unresolved_header_placeholder_reported_by_name() {
        // SAFETY: test-only env mutation on a unique name.
        unsafe { std::env::remove_var("IH_TEST_548_UNSET") };
        let mut cfg = McpServerConfig::default();
        cfg.headers.insert(
            "Authorization".to_string(),
            "Bearer ${IH_TEST_548_UNSET}".to_string(),
        );
        let found = unresolved_placeholders(&cfg);
        assert_eq!(
            found,
            vec![UnresolvedPlaceholder {
                field: "header 'Authorization'".to_string(),
                var: "IH_TEST_548_UNSET".to_string(),
            }]
        );
        let err = ensure_placeholders_resolved(&cfg).unwrap_err().to_string();
        assert!(err.contains("IH_TEST_548_UNSET"), "got: {err}");
        assert!(err.contains("Authorization"), "got: {err}");
    }

    #[test]
    fn unresolved_auth_and_url_reported() {
        // SAFETY: test-only env mutation on unique names.
        unsafe {
            std::env::remove_var("IH_TEST_548_A");
            std::env::remove_var("IH_TEST_548_H");
        }
        let cfg = McpServerConfig {
            url: Some("https://${IH_TEST_548_H}/mcp".to_string()),
            auth: Some("${IH_TEST_548_A}".to_string()),
            ..McpServerConfig::default()
        };
        let found = unresolved_placeholders(&cfg);
        assert_eq!(found.len(), 2, "got: {found:?}");
        assert!(
            found
                .iter()
                .any(|p| p.field == "auth" && p.var == "IH_TEST_548_A")
        );
        assert!(
            found
                .iter()
                .any(|p| p.field == "url" && p.var == "IH_TEST_548_H")
        );
    }

    #[test]
    fn resolved_config_has_no_unresolved_placeholders() {
        let mut cfg = McpServerConfig {
            url: Some("https://example.test/mcp".to_string()),
            auth: Some("literal-token".to_string()),
            ..McpServerConfig::default()
        };
        cfg.headers
            .insert("X-Thing".to_string(), "plain".to_string());
        assert!(unresolved_placeholders(&cfg).is_empty());
        assert!(ensure_placeholders_resolved(&cfg).is_ok());
    }

    #[test]
    fn unresolved_error_never_contains_a_set_variables_value() {
        // SAFETY: test-only env mutation on unique names.
        unsafe {
            std::env::set_var("IH_TEST_548_SET", "test-value-548");
            std::env::remove_var("IH_TEST_548_MISSING");
        }
        let mut cfg = McpServerConfig::default();
        cfg.headers.insert(
            "Authorization".to_string(),
            "Bearer ${IH_TEST_548_SET} ${IH_TEST_548_MISSING}".to_string(),
        );
        interpolate_config(&mut cfg);
        let err = ensure_placeholders_resolved(&cfg).unwrap_err().to_string();
        assert!(err.contains("IH_TEST_548_MISSING"), "got: {err}");
        assert!(!err.contains("test-value-548"), "value leaked: {err}");
        unsafe { std::env::remove_var("IH_TEST_548_SET") };
    }
}
