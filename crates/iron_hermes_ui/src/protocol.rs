//! Shared protocol types for WebSocket chat.
//!
//! These types are used on BOTH client and server — they are simple
//! serializable data structures with no server-only dependencies.
//! Kept in a separate unconditional module so the WASM client can
//! compile them without pulling in the `server` feature.

use serde::{Deserialize, Serialize};

/// Client → server message (user input).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatRequest {
    pub session_id: String,
    pub message: String,
}

/// Server → client streaming events.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ChatStreamEvent {
    /// Streaming text delta from the agent.
    Delta { text: String },
    /// Agent started a tool call.
    ToolCallStart { name: String, args: String },
    /// Tool call completed.
    ToolCallEnd { name: String, success: bool },
    /// Agent response finished.
    Finished { total_tokens: u32 },
    /// Error during agent execution.
    Error { message: String },
}
