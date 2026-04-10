/// UDS JSON-RPC 2.0 server with call counter for sandboxed tool dispatch.
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tracing::{debug, warn};

use crate::ToolDispatch;

/// Tool names allowed through the RPC bridge (D-07).
/// `terminal` and `execute_code` are explicitly excluded to prevent
/// sandbox escape and recursion respectively.
const ALLOWED_TOOLS: &[&str] = &[
    "read_file",
    "write_file",
    "patch",
    "search_files",
    "web_search",
    "web_read",
    "memory",
];

/// JSON-RPC 2.0 server that listens on a Unix domain socket and dispatches
/// tool calls to the agent's tool registry via `ToolDispatch`.
pub struct RpcServer {
    listener: UnixListener,
    dispatch: Arc<dyn ToolDispatch>,
    max_calls: u32,
    call_count: Arc<AtomicU32>,
}

impl RpcServer {
    /// Create a new RPC server bound to the given `UnixListener`.
    pub fn new(
        listener: UnixListener,
        dispatch: Arc<dyn ToolDispatch>,
        max_calls: u32,
    ) -> Self {
        Self {
            listener,
            dispatch,
            max_calls,
            call_count: Arc::new(AtomicU32::new(0)),
        }
    }

    /// Serve a single client connection until it closes.
    ///
    /// Accepts one connection (one per sandbox execution, per D-09/D-11),
    /// reads newline-delimited JSON-RPC 2.0 requests, dispatches to the
    /// tool callback, and writes responses.
    pub async fn serve(self) -> anyhow::Result<()> {
        let (stream, _) = self.listener.accept().await?;
        debug!("RPC client connected");
        self.handle_connection(stream).await
    }

    async fn handle_connection(&self, stream: UnixStream) -> anyhow::Result<()> {
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();

        while let Some(line) = lines.next_line().await? {
            let response = self.handle_request(&line).await;
            writer.write_all(response.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }

        debug!("RPC client disconnected");
        Ok(())
    }

    async fn handle_request(&self, line: &str) -> String {
        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                warn!("Invalid JSON-RPC request: {}", e);
                return Self::error_response(serde_json::Value::Null, -32700, "Parse error");
            }
        };

        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = match req.get("method").and_then(|m| m.as_str()) {
            Some(m) => m.to_string(),
            None => {
                return Self::error_response(id, -32600, "Invalid request: missing method");
            }
        };
        let params = req.get("params").cloned().unwrap_or(serde_json::json!({}));

        // Check method is in allowlist (D-07)
        if !ALLOWED_TOOLS.contains(&method.as_str()) {
            warn!("RPC method not allowed: {}", method);
            return Self::error_response(
                id,
                -32601,
                &format!("Method not found: {}", method),
            );
        }

        // Check call limit (D-13)
        let count = self.call_count.fetch_add(1, Ordering::SeqCst);
        if count >= self.max_calls {
            warn!("RPC call limit exceeded ({} calls)", self.max_calls);
            return Self::error_response(
                id,
                -32000,
                &format!("RPC call limit exceeded ({} calls)", self.max_calls),
            );
        }

        // Dispatch tool call
        match self.dispatch.dispatch(&method, params).await {
            Ok(result) => {
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": result
                })
                .to_string()
            }
            Err(e) => {
                warn!("Tool dispatch error for {}: {}", method, e);
                Self::error_response(id, -32603, &format!("Internal error: {}", e))
            }
        }
    }

    fn error_response(id: serde_json::Value, code: i32, message: &str) -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message
            }
        })
        .to_string()
    }
}
