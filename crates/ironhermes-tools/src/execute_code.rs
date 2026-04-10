//! `execute_code` tool — executes Python scripts in an isolated sandbox
//! with RPC access to a safe subset of agent tools.

use std::sync::Arc;

use async_trait::async_trait;
use ironhermes_core::{ExecConfig, ToolSchema};
use ironhermes_exec::{Sandbox, SandboxConfig, ToolDispatch};
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
}

impl ExecuteCodeTool {
    /// Create a new ExecuteCodeTool.
    ///
    /// `rpc_registry` must contain ONLY D-07 safe tools (no terminal, no execute_code).
    /// `config` provides python path, timeout, call limit, and output cap.
    pub fn new(rpc_registry: Arc<ToolRegistry>, config: ExecConfig) -> Self {
        Self {
            rpc_registry,
            config,
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
        };

        let sandbox = Sandbox::new(sandbox_config);

        // Create the ToolDispatch adapter wrapping the RPC-safe registry
        let dispatch: Arc<dyn ToolDispatch> = Arc::new(RegistryDispatch {
            registry: Arc::clone(&self.rpc_registry),
        });

        let result = sandbox.run(code, dispatch).await?;

        // Format result per D-15: separate stdout/stderr/exit_code sections
        let mut output = String::new();

        if result.timed_out {
            output.push_str(&format!(
                "[timed out after {}s]\n",
                self.config.timeout_secs
            ));
        }

        output.push_str("[stdout]\n");
        output.push_str(&result.stdout);
        output.push_str("\n[stderr]\n");
        output.push_str(&result.stderr);

        let exit_code = result
            .exit_code
            .map(|c| c.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        output.push_str(&format!("\n[exit_code: {}]", exit_code));

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execute_code_tool_basic() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config);

        let result = tool
            .execute(json!({"code": "print('hello from python')"}))
            .await
            .unwrap();
        assert!(result.contains("[stdout]"));
        assert!(result.contains("hello from python"));
        assert!(result.contains("[exit_code: 0]"));
    }

    #[tokio::test]
    async fn test_execute_code_tool_with_rpc() {
        // Build registry with file tools for RPC
        let mut rpc_registry = ToolRegistry::new();
        rpc_registry.register_defaults();
        let rpc_registry = Arc::new(rpc_registry);
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config);

        // Write a temp file, then have Python read it via hermes_tools
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "test content from rust").unwrap();
        let script = format!(
            "from hermes_tools import read_file\nresult = read_file('{}')\nprint(result)",
            tmp.path().display()
        );
        let result = tool.execute(json!({"code": script})).await.unwrap();
        assert!(
            result.contains("test content from rust"),
            "RPC read_file should return file content, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_execute_code_tool_timeout_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let mut config = ExecConfig::default();
        config.timeout_secs = 2;
        let tool = ExecuteCodeTool::new(rpc_registry, config);

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            tool.execute(json!({"code": "import time; time.sleep(999)"})),
        )
        .await
        .expect("test must complete within 10s")
        .unwrap();

        assert!(
            result.contains("[timed out"),
            "should contain timeout notice, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_execute_code_result_format() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config);

        let result = tool
            .execute(json!({"code": "import sys; print('out'); sys.stderr.write('err')"}))
            .await
            .unwrap();
        assert!(result.contains("[stdout]"), "missing [stdout] section");
        assert!(result.contains("out"), "missing stdout content");
        assert!(result.contains("[stderr]"), "missing [stderr] section");
        assert!(result.contains("err"), "missing stderr content");
        assert!(
            result.contains("[exit_code: 0]"),
            "missing exit_code, got: {}",
            result
        );
    }

    #[tokio::test]
    async fn test_execute_code_missing_code_param() {
        let rpc_registry = Arc::new(ToolRegistry::new());
        let config = ExecConfig::default();
        let tool = ExecuteCodeTool::new(rpc_registry, config);

        let result = tool.execute(json!({})).await;
        assert!(result.is_err(), "should error on missing code param");
        assert!(
            result.unwrap_err().to_string().contains("code"),
            "error should mention 'code'"
        );
    }
}
