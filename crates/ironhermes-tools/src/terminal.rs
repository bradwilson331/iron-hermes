use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use serde_json::json;
use tokio::process::Command;
use tokio::time::{timeout, Duration};
use tracing::debug;

use crate::registry::Tool;

const MAX_OUTPUT_LEN: usize = 50_000;

pub struct TerminalTool;

#[async_trait]
impl Tool for TerminalTool {
    fn name(&self) -> &str {
        "terminal"
    }

    fn toolset(&self) -> &str {
        "system"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output (stdout + stderr combined)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "terminal",
            "Execute a shell command and return its output (stdout + stderr combined).",
            json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "The shell command to execute."
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds (default: 30).",
                        "default": 30
                    }
                },
                "required": ["command"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: command"))?;

        let timeout_secs = args["timeout"].as_u64().unwrap_or(30);

        debug!("Executing terminal command: {}", command);

        let fut = async {
            let output = Command::new("sh")
                .arg("-c")
                .arg(command)
                .output()
                .await?;

            let mut combined = String::new();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }

            if combined.is_empty() {
                combined = format!("(exit code: {})", output.status.code().unwrap_or(-1));
            } else if output.status.code().unwrap_or(0) != 0 {
                combined.push_str(&format!(
                    "\n(exit code: {})",
                    output.status.code().unwrap_or(-1)
                ));
            }

            Ok::<String, anyhow::Error>(combined)
        };

        let result =
            timeout(Duration::from_secs(timeout_secs), fut)
                .await
                .map_err(|_| anyhow::anyhow!("Command timed out after {}s", timeout_secs))??;

        if result.len() > MAX_OUTPUT_LEN {
            let truncated = &result[..MAX_OUTPUT_LEN];
            Ok(format!("{}\n[truncated]", truncated))
        } else {
            Ok(result)
        }
    }
}
