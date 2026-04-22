/// IronHermes MCP client crate (Phase 21.2).
///
/// Provides:
/// - `config`: MCP server configuration types and env interpolation (D-17, D-18)
/// - `security`: Safe env filtering and credential stripping (D-19, D-20)
/// - `tool`: McpTool implementing Tool trait with server__tool naming (D-06, D-11)
/// - `transport`: Stdio + HTTP transport helpers wrapping rmcp (D-03)
/// - `sampling`: SamplingHandler for server-initiated LLM requests (D-03)
/// - `server_task`: Per-server tokio task with reconnection (D-04, D-05) [added in Task 2]
/// - `manager`: McpManager orchestrating all server tasks (D-07, D-09) [added in Task 2]
pub mod config;
pub mod sampling;
pub mod security;
pub mod tool;
pub mod transport;
// manager and server_task added in Task 2

pub use config::{interpolate_config, interpolate_env, McpServerConfig, SamplingConfig};
pub use sampling::SamplingHandler;
pub use security::{build_safe_env, sanitize_error, CREDENTIAL_PATTERN};
pub use tool::{make_prefixed_name, McpCallRequest, McpTool};
