//! WebSocket endpoint for streaming agent chat responses.

use dioxus::prelude::*;
use dioxus_fullstack::{WebSocketOptions, Websocket};
#[cfg(feature = "server")]
use tokio::sync::mpsc;
#[cfg(feature = "server")]
use tokio::task::JoinHandle;
#[cfg(feature = "server")]
use tracing::warn;

pub use crate::protocol::{ChatRequest, ChatStreamEvent};

#[get("/api/ws/chat")]
pub async fn ws_chat(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    #[cfg(feature = "server")]
    let app_state = crate::server::state::global_app_state().clone();

    Ok(ws.on_upgrade(
        move |mut socket: dioxus_fullstack::TypedWebsocket<String, String>| {
            #[cfg(feature = "server")]
            let app_state = app_state.clone();
            async move {
                #[cfg(feature = "server")]
                {
                struct InFlightTurn {
                    session_id: String,
                    rx: mpsc::UnboundedReceiver<ChatStreamEvent>,
                    handle: JoinHandle<()>,
                }

                let mut in_flight_turn: Option<InFlightTurn> = None;

                loop {
                    tokio::select! {
                        msg = socket.recv() => {
                            let msg = match msg {
                                Ok(msg) => msg,
                                Err(err) => {
                                    let session_id = in_flight_turn
                                        .as_ref()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");
                                    warn!(session_id = %session_id, reason = %err, in_flight = in_flight_turn.is_some(), "websocket recv failed; aborting connection");
                                    if let Some(turn) = in_flight_turn.take() {
                                        turn.handle.abort();
                                    }
                                    break;
                                }
                            };

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

                            if in_flight_turn.is_some() {
                                let busy = ChatStreamEvent::Error {
                                    message: "Another request is already in progress".to_string(),
                                };
                                let _ = socket
                                    .send(serde_json::to_string(&busy).unwrap_or_default())
                                    .await;
                                continue;
                            }

                            let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                            let app_state = app_state.clone();
                            let session_id = req.session_id;
                            let session_id_for_turn = session_id.clone();
                            let message = req.message;
                            let handle = tokio::spawn(async move {
                                let tx_stream = tx.clone();
                                let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                                    Box::new(move |delta: &str| {
                                        let _ = tx_stream.send(ChatStreamEvent::Delta {
                                            text: delta.to_string(),
                                        });
                                    });

                                let tx_tool = tx.clone();
                                let tool_progress_callback: ironhermes_agent::agent_loop::ToolProgressCallback =
                                    Box::new(move |name: &str, args: &str| {
                                        let _ = tx_tool.send(ChatStreamEvent::ToolCallStart {
                                            name: name.to_string(),
                                            args: args.to_string(),
                                        });
                                    });

                                let tx_tool_result = tx.clone();
                                let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                    Box::new(move |name: &str, success: bool| {
                                        let _ = tx_tool_result.send(ChatStreamEvent::ToolCallEnd {
                                            name: name.to_string(),
                                            success,
                                        });
                                    });

                                let result = app_state
                                    .run_web_turn(
                                        &session_id_for_turn,
                                        &message,
                                        stream_callback,
                                        Some(tool_progress_callback),
                                        Some(tool_result_callback),
                                    )
                                    .await;

                                match result {
                                    Ok(agent_result) => {
                                        let _ = tx.send(ChatStreamEvent::Finished {
                                            total_tokens: agent_result.total_usage.total_tokens as u32,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(ChatStreamEvent::Error {
                                            message: format!("Agent error: {e}"),
                                        });
                                    }
                                }
                            });

                            in_flight_turn = Some(InFlightTurn {
                                session_id,
                                rx,
                                handle,
                            });
                        }

                        maybe_event = async {
                            match in_flight_turn.as_mut() {
                                Some(turn) => turn.rx.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            match maybe_event {
                                Some(event) => {
                                    let json = serde_json::to_string(&event).unwrap_or_default();
                                    if let Err(err) = socket.send(json).await {
                                        if let Some(turn) = in_flight_turn.take() {
                                            warn!(session_id = %turn.session_id, reason = %err, in_flight = true, "websocket send failed; aborting in-flight turn");
                                            turn.handle.abort();
                                        }
                                        break;
                                    }
                                }
                                None => {
                                    if let Some(turn) = in_flight_turn.take() {
                                        if let Err(err) = turn.handle.await {
                                            warn!(session_id = %turn.session_id, reason = %err, in_flight = false, "turn task join failed");
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                }

                #[cfg(not(feature = "server"))]
                {
                    let unavailable = ChatStreamEvent::Error {
                        message: "Websocket chat route is unavailable without `server` feature"
                            .to_string(),
                    };
                    let _ = socket
                        .send(serde_json::to_string(&unavailable).unwrap_or_default())
                        .await;
                }
            }
        },
    ))
}
