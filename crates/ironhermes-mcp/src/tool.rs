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
    // Allowlist, not denylist: map every character outside `[A-Za-z0-9_]` to
    // `_`. The old denylist (`-.@/`) let other characters through — notably
    // `:` from URL-keyed server names (e.g. `https://mcp.twilio.com/docs`),
    // which produced tool names Anthropic rejects with a 400
    // (`^[a-zA-Z0-9_-]{1,128}$`), dropping the whole request. Hyphens still
    // fold to `_`, preserving prior behavior for existing names.
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

/// D-06: Build the prefixed name `server__tool` with sanitization.
///
/// Every character outside `[A-Za-z0-9_]` in the server or tool name is
/// replaced with an underscore before joining with double-underscore, and the
/// joined name is capped at 128 chars. This keeps the result inside the
/// Anthropic/OpenAI tool-name charset (`^[a-zA-Z0-9_-]{1,128}$`) — a URL-keyed
/// server name like `https://mcp.twilio.com/docs` (note the `:`) would
/// otherwise emit an invalid name and get the entire request rejected. Covers
/// real-world npm package identifiers like `@modelcontextprotocol/server-filesystem`.
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
/// // URL-keyed server names (colon, slashes) are fully neutralized:
/// assert_eq!(make_prefixed_name("https://mcp.twilio.com/docs", "twilio__search"),
///     "https___mcp_twilio_com_docs__twilio__search");
/// ```
pub fn make_prefixed_name(server_name: &str, tool_name: &str) -> String {
    let safe_server = sanitize_server_name(server_name);
    let safe_tool = sanitize_server_name(tool_name);
    let mut prefixed = format!("{safe_server}__{safe_tool}");
    // Anthropic/OpenAI cap tool names at 128 chars. Sanitization guarantees
    // pure-ASCII output, so truncating on a byte boundary is char-safe.
    if prefixed.len() > 128 {
        prefixed.truncate(128);
    }
    prefixed
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.prefixed_name
    }

    fn toolset(&self) -> &str {
        // Single source of truth shared with the registry's toolset-filter exemption
        // (get_definitions): dynamic MCP tools bypass the built-in toolset-enabled filter.
        ironhermes_tools::registry::MCP_TOOLSET
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
    fn sanitize_server_name_replaces_colon_space_and_other_chars() {
        // Anthropic tool-name charset is [a-zA-Z0-9_-]; a URL-keyed server name
        // (colon + slashes) must not leak an invalid character to the wire.
        assert_eq!(
            sanitize_server_name("https://mcp.twilio.com/docs"),
            "https___mcp_twilio_com_docs"
        );
        assert_eq!(sanitize_server_name("has space"), "has_space");
        assert_eq!(sanitize_server_name(""), "_");
    }

    #[test]
    fn make_prefixed_name_url_keyed_server_is_valid() {
        let name = make_prefixed_name("https://mcp.twilio.com/docs", "twilio__search");
        assert_eq!(name, "https___mcp_twilio_com_docs__twilio__search");
        // Must satisfy the Anthropic/OpenAI tool-name pattern.
        assert!(
            name.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
        assert!(name.len() <= 128);
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
