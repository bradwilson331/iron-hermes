//! `execute_code` tool — executes Python scripts in an isolated sandbox
//! with RPC access to a safe subset of agent tools.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{ExecConfig, ToolSchema};
use ironhermes_exec::{CancellationToken, Sandbox, SandboxConfig, ToolDispatch};
use serde_json::json;
use tracing::debug;

use crate::registry::{Tool, ToolRegistry};

// =============================================================================
// RegistryDispatch — adapter bridging ToolRegistry to ToolDispatch trait
// =============================================================================

/// Adapter that implements `ironhermes_exec::ToolDispatch` by wrapping an
/// `Arc<ToolRegistry>`. Lives in ironhermes-tools (not ironhermes-exec) to
/// avoid circular crate dependencies.
pub(crate) struct RegistryDispatch {
    registry: Arc<ToolRegistry>,
}

#[async_trait]
impl ToolDispatch for RegistryDispatch {
    async fn dispatch(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        self.registry.dispatch(tool_name, args).await
    }
}

// =============================================================================
// ExecuteCodeTool
// =============================================================================

/// Agent tool that executes Python scripts in an isolated sandbox.
///
/// The sandbox has access to a safe subset of agent tools (D-07) via
/// JSON-RPC over a Unix domain socket. The RPC dispatch registry is
/// separate from the main registry and intentionally excludes `terminal`
/// and `execute_code` (defense-in-depth against recursion).
pub struct ExecuteCodeTool {
    /// Registry containing only D-07 safe tools for RPC dispatch.
    rpc_registry: Arc<ToolRegistry>,
    /// Execution configuration (python path, timeout, limits).
    config: ExecConfig,
    /// Cancellation token — when triggered (e.g., by user sending a new message),
    /// the running sandbox child process is killed immediately.
    /// Integration: The gateway/handler creates a CancellationToken per tool execution
    /// and triggers it when a new message arrives from the same user.
    /// TODO: Wire CancellationToken creation in gateway handler (Phase 8 follow-up).
    cancel_token: Option<CancellationToken>,
}

impl ExecuteCodeTool {
    /// Create a new ExecuteCodeTool.
    ///
    /// `rpc_registry` must contain ONLY D-07 safe tools (no terminal, no execute_code).
    /// `config` provides python path, timeout, call limit, and output cap.
    /// `cancel_token` is optionally provided by the gateway to support user interruption (D-40).
    pub fn new(
        rpc_registry: Arc<ToolRegistry>,
        config: ExecConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Self {
        Self {
            rpc_registry,
            config,
            cancel_token,
        }
    }
}

#[async_trait]
impl Tool for ExecuteCodeTool {
    fn name(&self) -> &str {
        "execute_code"
    }

    fn toolset(&self) -> &str {
        "code"
    }

    fn description(&self) -> &str {
        "Execute a Python script in an isolated sandbox. The script can call agent tools \
         (read_file, web_search, etc.) via 'from hermes_tools import <tool>'. Returns \
         stdout, stderr, and exit code."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "execute_code",
            self.description(),
            json!({
                "type": "object",
                "properties": {
                    "code": {
                        "type": "string",
                        "description": "Python script source code to execute."
                    }
                },
                "required": ["code"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let code = args["code"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: code"))?;

        debug!("Executing Python script ({} bytes)", code.len());

        // Build sandbox config from ExecConfig
        let sandbox_config = SandboxConfig {
            python_path: self.config.python_path.clone(),
            timeout_secs: self.config.timeout_secs,
            max_rpc_calls: self.config.max_rpc_calls,
            max_output_bytes: self.config.max_output_bytes,
            max_stderr_bytes: self.config.max_stderr_bytes,
        };

        let sandbox = Sandbox::new(sandbox_config);

        // Create the ToolDispatch adapter wrapping the RPC-safe registry
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(RegistryDispatch {
            registry: Arc::clone(&self.rpc_registry),
        });

        let result = sandbox.run(code, dispatch, self.cancel_token.clone()).await?;

        // D-25..D-27: Structured JSON response format
        let status = if result.interrupted {
            "interrupted"
        } else if result.timed_out {
            "timeout"
        } else if result.exit_code == Some(0) {
            "success"
        } else {
            "error"
        };

        let mut response = json!({
            "status": status,
            "output": result.stdout,
            "exit_code": result.exit_code.unwrap_or(-1),
            "tool_calls_made": result.tool_calls_made,
            "duration_seconds": result.duration_seconds,
        });

        // D-30: include stderr when non-empty (truncation already handled by sandbox)
        if !result.stderr.is_empty() {
            response["stderr"] = serde_json::Value::String(result.stderr);
        }

        Ok(response.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_code_tool_basic() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool
            .execute(json!({"code": "print('hello from python')"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(parsed["output"].as_str().unwrap().contains("hello from python"));
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed["duration_seconds"].as_f64().unwrap() >= 0.0);
        assert_eq!(parsed["tool_calls_made"], 0);
    }

    #[tokio::test]
    async fn test_execute_code_tool_with_rpc() {
        // Build registry with file tools for RPC
        let mut rpc_registry = ToolRegistry::new();
        rpc_registry.register_defaults();
        let rpc_registry = Arc::new(rpc_registry);
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        // Write a temp file, then have Python read it via hermes_tools
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "test content from rust").unwrap();
        let script = format!(
            "from hermes_tools import read_file\nresult = read_file('{}')\nprint(result)",
            tmp.path().display()
        );
        let result = tool.execute(json!({"code": script})).await.unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(
            parsed["output"].as_str().unwrap().contains("test content from rust"),
            "RPC read_file should return file content, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_execute_code_tool_timeout_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let mut config = ExecConfig::default();
        config.timeout_secs = 2;
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tool.execute(json!({"code": "import time; time.sleep(999)"})),
        )
        .await
        .expect("test must complete within 10s")
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "timeout");
        assert!(parsed["stderr"].as_str().unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn test_execute_code_result_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool
            .execute(json!({"code": "import sys; print('out'); sys.stderr.write('err')"}))
            .await
            .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&result)
            .expect("response should be valid JSON");
        assert_eq!(parsed["status"], "success");
        assert!(parsed["output"].as_str().unwrap().contains("out"), "missing stdout content");
        assert_eq!(parsed["stderr"], "err", "missing stderr content");
        assert_eq!(parsed["exit_code"], 0);
        assert!(parsed.get("tool_calls_made").is_some(), "missing tool_calls_made");
        assert!(parsed.get("duration_seconds").is_some(), "missing duration_seconds");
    }

    #[tokio::test]
    async fn test_execute_code_missing_code_param() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config, None);

        let result = tool.execute(json!({})).await;
        assert!(result.is_err(), "should error on missing code param");
        assert!(
            result.unwrap_err().to_string().contains("code"),
            "error should mention 'code'"
        );
    }
}
