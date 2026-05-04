/// IronHermes MCP client crate (Phase 21.2).
///
/// Provides:
/// - `config`: MCP server configuration types and env interpolation (D-17, D-18)
/// - `security`: Safe env filtering and credential stripping (D-19, D-20)
/// - `tool`: McpTool implementing Tool trait with server__tool naming (D-06, D-11)
/// - `transport`: Stdio + HTTP transport helpers wrapping rmcp (D-03)
/// - `sampling`: SamplingHandler for server-initiated LLM requests (D-03)
/// - `server_task`: Per-server tokio task with reconnection (D-04, D-05)
/// - `manager`: McpManager orchestrating all server tasks (D-07, D-09)
pub mod config;
pub mod manager;
pub mod sampling;
pub mod security;
pub mod server_task;
pub mod tool;
pub mod transport;

pub use config::{McpServerConfig, SamplingConfig, interpolate_config, interpolate_env};
pub use manager::{McpManager, StartResult};
pub use sampling::SamplingHandler;
pub use security::{CREDENTIAL_PATTERN, build_safe_env, sanitize_error};
pub use tool::{McpCallRequest, McpTool, make_prefixed_name, sanitize_server_name};
