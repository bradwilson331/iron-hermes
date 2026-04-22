/// IronHermes MCP client crate (Phase 21.2).
///
/// Provides:
/// - `config`: MCP server configuration types and env interpolation (D-17, D-18)
/// - `security`: Safe env filtering and credential stripping (D-19, D-20)
pub mod config;
pub mod security;

pub use config::{interpolate_config, interpolate_env, McpServerConfig, SamplingConfig};
pub use security::{build_safe_env, sanitize_error, CREDENTIAL_PATTERN};
