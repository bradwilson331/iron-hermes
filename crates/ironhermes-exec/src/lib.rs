//! `ironhermes-exec` — sandbox runtime for executing Python scripts with
//! tool access via JSON-RPC over Unix domain sockets.

pub mod rpc_server;
pub mod sandbox;

pub use rpc_server::RpcServer;
pub use sandbox::{Sandbox, SandboxResult};

/// Embedded Python helper module that scripts import for tool access.
pub const HERMES_TOOLS_PY: &str = include_str!("hermes_tools.py");

// =============================================================================
// ToolDispatch trait — decouples ironhermes-exec from ironhermes-tools
// =============================================================================

/// Trait for dispatching tool calls from the RPC server to the agent's tool
/// registry. Implemented by `ExecuteCodeTool` (in ironhermes-tools) which holds
/// an `Arc<ToolRegistry>`.
#[async_trait::async_trait]
pub trait ToolDispatch: Send + Sync {
    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String>;
}

// =============================================================================
// SandboxConfig — runtime parameters for a single execution
// =============================================================================

/// Configuration for a single sandbox execution.
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    /// Path to the Python interpreter. Default: "python3".
    pub python_path: String,
    /// Timeout in seconds for the child process. Default: 300 (5 minutes).
    pub timeout_secs: u64,
    /// Maximum number of RPC calls allowed per execution. Default: 50.
    pub max_rpc_calls: u32,
    /// Maximum stdout bytes before truncation. Default: 50,000 (50 KB).
    pub max_output_bytes: usize,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            python_path: "python3".into(),
            timeout_secs: 300,
            max_rpc_calls: 50,
            max_output_bytes: 50_000,
        }
    }
}
