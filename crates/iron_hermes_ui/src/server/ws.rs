//! WebSocket endpoint for streaming agent chat responses.
//!
//! Per CONTEXT D-01: WebSocket delivers streaming agent responses to
//! the block stream. Client sends user messages as JSON; server streams
//! back `ChatStreamEvent` variants as JSON lines.
//!
//! Phase 25.5 Plan 04: replaced echo handler with real AgentLoop dispatch.
//! User message → AgentLoop with StreamCallback → ChatStreamEvent over WS.

use dioxus::prelude::*;
use dioxus_fullstack::{WebSocketOptions, Websocket};

/// Re-export protocol types so the server module surface stays unchanged.
pub use crate::protocol::{ChatRequest, ChatStreamEvent};

/// WebSocket chat endpoint — real AgentLoop dispatch.
///
/// Client connects, sends `ChatRequest` as JSON, server streams back
/// `ChatStreamEvent` items. The WebSocket stays open for the session
/// duration; multiple messages can be sent/received.
///
/// Plan 04: builds a real AgentLoop per message, wires a StreamCallback
/// that sends deltas over a tokio mpsc channel, and forwards events
/// from the channel to the WebSocket.
#[get("/api/ws/chat")]
pub async fn ws_chat(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    Ok(ws.on_upgrade(|mut socket: dioxus_fullstack::TypedWebsocket<String, String>| async move {
        // Load AppState once for this connection.
        // AppState was initialized at startup and injected via Axum Extension.
        // Since we're inside the on_upgrade handler, we reconstruct from config
        // (the Extension is not directly accessible here in the Dioxus WS model).
        let app_state = match crate::server::state::AppState::init().await {
            Ok(state) => state,
            Err(e) => {
                let err = ChatStreamEvent::Error {
                    message: format!("Server initialization failed: {e}"),
                };
                let _ = socket
                    .send(serde_json::to_string(&err).unwrap_or_default())
                    .await;
                return;
            }
        };

        while let Ok(msg) = socket.recv().await {
            // Parse incoming ChatRequest
            let req: ChatRequest = match serde_json::from_str(&msg) {
                Ok(r) => r,
                Err(e) => {
                    let err = ChatStreamEvent::Error {
                        message: format!("Invalid request: {e}"),
                    };
                    let _ = socket
                        .send(serde_json::to_string(&err).unwrap_or_default())
                        .await;
                    continue;
                }
            };

            // Build an mpsc channel for streaming events back to WebSocket.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ChatStreamEvent>();

            // Construct the streaming callback that sends deltas to the channel.
            let tx_stream = tx.clone();
            let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                Box::new(move |delta: &str| {
                    let _ = tx_stream.send(ChatStreamEvent::Delta {
                        text: delta.to_string(),
                    });
                });

            // Construct tool progress callback for ToolCallStart events.
            let tx_tool = tx.clone();
            let tool_progress_callback: ironhermes_agent::agent_loop::ToolProgressCallback =
                Box::new(move |name: &str, args: &str| {
                    let _ = tx_tool.send(ChatStreamEvent::ToolCallStart {
                        name: name.to_string(),
                        args: args.to_string(),
                    });
                });

            // Build AgentLoop and run the agent turn in a background task.
            let message_text = req.message.clone();
            let state = app_state.clone();
            let tx_fin = tx.clone();
            tokio::spawn(async move {
                match state.build_agent_loop(stream_callback, Some(tool_progress_callback)) {
                    Ok(mut agent) => {
                        // Build system message and user message
                        let system_msg = ironhermes_core::ChatMessage::system(
                            "You are Hermes, an AI assistant. Be helpful, concise, and precise.",
                        );
                        let user_msg = ironhermes_core::ChatMessage::user(&message_text);
                        let messages = vec![system_msg, user_msg];

                        match agent.run(messages).await {
                            Ok(result) => {
                                let _ = tx_fin.send(ChatStreamEvent::Finished {
                                    total_tokens: result.total_usage.total_tokens as u32,
                                });
                            }
                            Err(e) => {
                                let _ = tx_fin.send(ChatStreamEvent::Error {
                                    message: format!("Agent error: {e}"),
                                });
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx_fin.send(ChatStreamEvent::Error {
                            message: format!("Failed to build agent: {e}"),
                        });
                    }
                }
                drop(tx_fin); // Signal channel closure
            });

            // Forward events from channel to WebSocket.
            // Drop our copy of tx so only the spawned task holds it.
            drop(tx);
            while let Some(event) = rx.recv().await {
                let json = serde_json::to_string(&event).unwrap_or_default();
                if socket.send(json).await.is_err() {
                    break;
                }
            }
        }
    }))
}
