//! WebSocket endpoint for streaming agent chat responses.
//!
//! Per CONTEXT D-01: WebSocket delivers streaming agent responses to
//! the block stream. Client sends user messages as JSON; server streams
//! back `ChatStreamEvent` variants as JSON lines.

use dioxus::prelude::*;
use dioxus_fullstack::{WebSocketOptions, Websocket};
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

/// WebSocket chat endpoint.
///
/// Client connects, sends `ChatRequest` as JSON, server streams back
/// `ChatStreamEvent` items. The WebSocket stays open for the session
/// duration; multiple messages can be sent/received.
///
/// Note: Full AgentLoop wiring happens in Plan 04. This plan establishes
/// the endpoint shape with echo/placeholder behavior.
#[get("/api/ws/chat")]
pub async fn ws_chat(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    Ok(ws.on_upgrade(|mut socket| async move {
        while let Ok(msg) = socket.recv().await {
            // Plan 04 replaces this echo with real AgentLoop dispatch.
            // For now, echo back as a ChatStreamEvent::Delta to prove the pipe works.
            let event = ChatStreamEvent::Delta {
                text: format!("[echo] {msg}"),
            };
            let json = serde_json::to_string(&event).unwrap_or_default();
            if socket.send(json).await.is_err() {
                break;
            }
            // Send a Finished event
            let fin = ChatStreamEvent::Finished { total_tokens: 0 };
            let fin_json = serde_json::to_string(&fin).unwrap_or_default();
            let _ = socket.send(fin_json).await;
        }
    }))
}
