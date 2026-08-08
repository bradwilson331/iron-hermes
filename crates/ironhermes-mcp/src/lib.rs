/// IronHermes MCP client crate (Phase 21.2).
///
/// Provides:
/// - `auth_store_adapter`: D-05 bridge — `AuthStoreCredentialStore` implements rmcp's
///   `CredentialStore` trait via Phase 41's `AuthStore` (zero-DCR-on-restart, B-3)
/// - `config`: MCP server configuration types and env interpolation (D-17, D-18)
/// - `security`: Safe env filtering, credential stripping, PRM issuer validation (D-19, D-20, B-4)
/// - `tool`: McpTool implementing Tool trait with server__tool naming (D-06, D-11)
/// - `transport`: Stdio + HTTP transport helpers wrapping rmcp (D-03)
/// - `sampling`: SamplingHandler for server-initiated LLM requests (D-03)
/// - `server_task`: Per-server tokio task with reconnection (D-04, D-05)
/// - `manager`: McpManager orchestrating all server tasks (D-07, D-09)
/// - `www_auth`: WWW-Authenticate header parsing for OAuth 401 classification (B-2, D-07)
pub mod auth_store_adapter;
pub mod config;
pub mod manager;
pub mod sampling;
pub mod security;
pub mod server_task;
pub mod tool;
pub mod transport;
pub mod www_auth;

pub use auth_store_adapter::{AuthStoreCredentialStore, open_auth_store_with_mcp_refresh};
pub use config::{McpServerConfig, SamplingConfig, interpolate_config, interpolate_env};
pub use manager::{McpManager, StartResult};
pub use sampling::SamplingHandler;
pub use security::{CREDENTIAL_PATTERN, build_safe_env, sanitize_error, validate_prm_issuer};
pub use server_task::McpOAuthError;
pub use tool::{McpCallRequest, McpTool, make_prefixed_name, sanitize_server_name};
pub use transport::connect_http_oauth;
pub use www_auth::{OAuthChallengeError, parse_www_authenticate};
