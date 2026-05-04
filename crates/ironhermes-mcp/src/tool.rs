use async_trait::async_trait;
use ironhermes_core::ToolSchema;
use ironhermes_tools::registry::Tool;
use tokio::sync::{mpsc, oneshot};

/// Request sent from McpTool::execute to the server task's dispatch loop.
pub struct McpCallRequest {
    /// Original MCP tool name (not prefixed with server name).
    pub tool_name: String,
    /// JSON arguments to pass to the tool.
    pub arguments: serde_json::Value,
    /// Channel to send the tool call result back to the caller.
    pub response_tx: oneshot::Sender<anyhow::Result<String>>,
}

/// A discovered MCP tool registered as a `Box<dyn Tool>` in the ToolRegistry.
///
/// Each `McpTool` wraps a single tool discovered from an MCP server. Tool calls
/// are dispatched via an mpsc channel to the server's background task, which
/// holds the live rmcp client connection.
pub struct McpTool {
    /// Prefixed name in `server__tool` format (D-06). Used for registration.
    prefixed_name: String,
    /// Original server name (e.g. "filesystem").
    server_name: String,
    /// Original tool name from the MCP server (e.g. "read_file").
    original_name: String,
    /// Description with [MCP: server_name] prefix (D-11).
    description: String,
    /// ToolSchema for LLM function calling.
    schema: ToolSchema,
    /// Sender side of the channel to the server task dispatch loop.
    call_tx: mpsc::Sender<McpCallRequest>,
}

impl McpTool {
    /// Create a new McpTool.
    ///
    /// - `server_name`: name of the MCP server (e.g. "filesystem")
    /// - `original_name`: tool name as reported by the MCP server (e.g. "read_file")
    /// - `original_description`: tool description from the MCP server
    /// - `input_schema`: JSON Schema object for tool arguments
    /// - `call_tx`: channel sender to the server task's dispatch loop
    pub fn new(
        server_name: &str,
        original_name: &str,
        original_description: &str,
        input_schema: serde_json::Value,
        call_tx: mpsc::Sender<McpCallRequest>,
    ) -> Self {
        let prefixed_name = make_prefixed_name(server_name, original_name);
        // D-11: prepend [MCP: server_name] to description for LLM context
        let description = format!("[MCP: {server_name}] {original_description}");
        let schema = ToolSchema::new(&prefixed_name, &description, input_schema);
        Self {
            prefixed_name,
            server_name: server_name.to_string(),
            original_name: original_name.to_string(),
            description,
            schema,
            call_tx,
        }
    }
}

/// Sanitize a server name for use as a tool-registry prefix.
///
/// Replaces characters that are illegal inside OpenAI-style tool names
/// (`-`, `.`, `@`, `/`) with underscores. Used by:
/// - `make_prefixed_name`, at tool-REGISTRATION time.
/// - `McpManager::shutdown_all`, to compute the same prefix at
///   tool-UNREGISTRATION time (closes GAP-4 / CR-01).
///
/// MUST be the single source of truth for this transformation — both
/// sides of the lifecycle depend on byte-for-byte agreement.
pub fn sanitize_server_name(name: &str) -> String {
    name.replace('-', "_")
        .replace('.', "_")
        .replace('@', "_")
        .replace('/', "_")
}

/// D-06: Build the prefixed name `server__tool` with sanitization.
///
/// Hyphens, dots, `@`, and `/` in server or tool names are replaced with
/// underscores before joining with double-underscore. This matches
/// hermes-agent's `_convert_mcp_schema` sanitization and covers real-world
/// npm package identifiers like `@modelcontextprotocol/server-filesystem`.
///
/// Delegates to [`sanitize_server_name`] so the transform is single-source.
///
/// # Examples
/// ```
/// use ironhermes_mcp::make_prefixed_name;
/// assert_eq!(make_prefixed_name("github", "create_issue"), "github__create_issue");
/// assert_eq!(make_prefixed_name("my-server", "read-file"), "my_server__read_file");
/// assert_eq!(make_prefixed_name("a.b.c", "x.y"), "a_b_c__x_y");
/// assert_eq!(make_prefixed_name("@modelcontextprotocol/server-filesystem", "read_file"),
///     "_modelcontextprotocol_server_filesystem__read_file");
/// ```
pub fn make_prefixed_name(server_name: &str, tool_name: &str) -> String {
    let safe_server = sanitize_server_name(server_name);
    let safe_tool = sanitize_server_name(tool_name);
    format!("{safe_server}__{safe_tool}")
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn toolset(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> ToolSchema {
        self.schema.clone()
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let (resp_tx, resp_rx) = oneshot::channel();
        self.call_tx
            .send(McpCallRequest {
                tool_name: self.original_name.clone(),
                arguments: args,
                response_tx: resp_tx,
            })
            .await
            .map_err(|_| anyhow::anyhow!("MCP server '{}' disconnected", self.server_name))?;
        match resp_rx.await {
            Ok(result) => result.map_err(|e| {
                anyhow::anyhow!("{}", crate::security::sanitize_error(&e.to_string()))
            }),
            Err(_) => Err(anyhow::anyhow!(
                "MCP server '{}' dropped request",
                self.server_name
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_prefixed_name_basic() {
        assert_eq!(
            make_prefixed_name("github", "create_issue"),
            "github__create_issue"
        );
    }

    #[test]
    fn test_make_prefixed_name_hyphen_sanitization() {
        assert_eq!(
            make_prefixed_name("my-server", "read-file"),
            "my_server__read_file"
        );
    }

    #[test]
    fn test_make_prefixed_name_dot_sanitization() {
        assert_eq!(make_prefixed_name("a.b.c", "x.y"), "a_b_c__x_y");
    }

    #[test]
    fn test_make_prefixed_name_mixed() {
        assert_eq!(
            make_prefixed_name("my-server.v2", "some-tool.v1"),
            "my_server_v2__some_tool_v1"
        );
    }

    #[test]
    fn test_make_prefixed_name_no_sanitization_needed() {
        assert_eq!(make_prefixed_name("fs", "read_file"), "fs__read_file");
    }

    #[test]
    fn sanitize_server_name_replaces_at_and_slash() {
        assert_eq!(sanitize_server_name("@scope/pkg"), "_scope_pkg");
    }

    #[test]
    fn sanitize_server_name_replaces_all_four_characters() {
        // All four characters GAP-4 requires, in one payload.
        assert_eq!(sanitize_server_name("@a-b.c/d"), "_a_b_c_d");
    }

    #[test]
    fn make_prefixed_name_handles_real_world_npm_package_name() {
        // Direct regression for the HUMAN-UAT.md GAP-4 evidence string.
        assert_eq!(
            make_prefixed_name("@modelcontextprotocol/server-filesystem", "read_file"),
            "_modelcontextprotocol_server_filesystem__read_file"
        );
    }

    #[test]
    fn make_prefixed_name_agrees_with_sanitize_server_name() {
        // Structural invariant: prefix must equal sanitize_server_name output +
        // `__` + sanitize_server_name of tool. Prevents future drift.
        let raw_server = "@a/b-c.d";
        let raw_tool = "x-y.z";
        let prefixed = make_prefixed_name(raw_server, raw_tool);
        let expected = format!(
            "{}__{}",
            sanitize_server_name(raw_server),
            sanitize_server_name(raw_tool)
        );
        assert_eq!(prefixed, expected);
    }
}
