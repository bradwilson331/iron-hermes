//! WebSocket endpoint for streaming agent chat responses.

use dioxus::prelude::*;
use dioxus_fullstack::{WebSocketOptions, Websocket};
#[cfg(feature = "server")]
use dioxus_fullstack::{body::Bytes, CloseCode, Message, TypedWebsocket};
#[cfg(feature = "server")]
use std::time::Duration;
#[cfg(feature = "server")]
use tokio::sync::mpsc;
#[cfg(feature = "server")]
use tokio::task::JoinHandle;
#[cfg(feature = "server")]
use tracing::warn;

pub use crate::protocol::{ChatRequest, ChatStreamEvent};

/// Server-side application-level WebSocket keepalive interval.
///
/// HUMAN-UAT Gap 3 (follow-up): even with proper close-frame emission,
/// live browser UAT showed `dx serve` / hyper proxy terminating idle
/// WebSocket connections after ~9-10s with no traffic. That close arrives
/// at the gloo-net client as `WebsocketError::ConnectionClosed`
/// ("Connection closed"), with no teardown log on the server because the
/// server loop never observed a recv error — it was simply idle on
/// `socket.recv()` when the proxy dropped the socket.
///
/// Application-level Ping frames keep any intermediate proxy's idle
/// timer reset and also exercise the send path so broken sockets are
/// detected promptly (the first failed Ping is classified as a
/// send-path failure per D-05, aborting any in-flight turn). Browsers
/// automatically respond to Ping with Pong at the WebSocket protocol
/// level, so the client requires no changes.
///
/// 5 seconds is well below the observed ~9s idle-close threshold and
/// matches the low end of common reverse-proxy keepalive intervals.
#[cfg(feature = "server")]
const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

#[cfg(feature = "server")]
fn is_clean_ws_disconnect(reason: &str) -> bool {
    let lower = reason.to_ascii_lowercase();
    lower.contains("closed")
        || lower.contains("close frame")
        || lower.contains("connection reset by peer")
        || lower.contains("eof")
}

/// Best-effort WebSocket close-frame emit before dropping the socket.
///
/// Ensures every teardown branch completes the WebSocket close handshake
/// so upstream proxies do not observe `Connection reset without closing
/// handshake`. Errors are intentionally swallowed: if the send fails, the
/// transport was already broken and the handler is exiting regardless —
/// we must not block teardown on close-frame delivery (respects D-06's
/// intent not to hang on broken-send paths, while closing HUMAN-UAT Gap 3
/// where the server previously dropped the socket with no close frame at
/// all).
#[cfg(feature = "server")]
async fn send_close_frame(
    socket: &mut TypedWebsocket<String, String>,
    code: CloseCode,
    reason: &str,
) {
    let _ = socket
        .send_raw(Message::Close {
            code,
            reason: reason.to_string(),
        })
        .await;
}

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

                // Keepalive interval closes HUMAN-UAT Gap 3 follow-up:
                // without periodic Pings the dev proxy (dx serve/hyper)
                // idle-closes the socket after ~9s, surfacing at the
                // client as `WebsocketError::ConnectionClosed`
                // ("Connection closed") with no server-side teardown
                // trace. See `WS_KEEPALIVE_INTERVAL` above for the full
                // rationale.
                let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
                keepalive
                    .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // First tick fires immediately; skip it so we don't
                // emit a Ping on connect before any idle window has
                // elapsed.
                keepalive.tick().await;

                loop {
                    tokio::select! {
                        msg = socket.recv() => {
                            let msg = match msg {
                                Ok(msg) => msg,
                                Err(err) => {
                                    let reason = err.to_string();
                                    let in_flight = in_flight_turn.is_some();
                                    let session_id = in_flight_turn
                                        .as_ref()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");

                                    if is_clean_ws_disconnect(&reason) {
                                        warn!(
                                            session_id = %session_id,
                                            reason = %reason,
                                            in_flight,
                                            "websocket recv closed cleanly; exiting connection"
                                        );
                                    } else {
                                        warn!(
                                            session_id = %session_id,
                                            reason = %reason,
                                            in_flight,
                                            "websocket recv failed; closing connection"
                                        );
                                    }

                                    if let Some(turn) = in_flight_turn.take() {
                                        if is_clean_ws_disconnect(&reason) {
                                            if let Err(join_err) = turn.handle.await {
                                                warn!(
                                                    session_id = %turn.session_id,
                                                    reason = %join_err,
                                                    in_flight = false,
                                                    "turn task join failed after clean websocket close"
                                                );
                                            }
                                        } else {
                                            turn.handle.abort();
                                        }
                                    }

                                    // Close HUMAN-UAT Gap 3: proactively emit a
                                    // WebSocket close frame on every recv-error
                                    // teardown branch so upstream proxies
                                    // observe a proper close handshake instead
                                    // of `Connection reset without closing
                                    // handshake`. Best-effort — swallow errors
                                    // since the transport may already be
                                    // half-broken.
                                    if is_clean_ws_disconnect(&reason) {
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Normal,
                                            "recv closed cleanly",
                                        )
                                        .await;
                                    } else {
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "recv failed",
                                        )
                                        .await;
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
                                        // Close HUMAN-UAT Gap 3: even on
                                        // broken-send paths, attempt a
                                        // best-effort close frame so the proxy
                                        // sees a close handshake instead of a
                                        // raw transport reset. The write will
                                        // most likely fail silently (transport
                                        // is already broken), which is
                                        // consistent with D-06's intent that
                                        // teardown not block on close-frame
                                        // delivery.
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "send failed",
                                        )
                                        .await;
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

                        _ = keepalive.tick() => {
                            // Keep intermediate proxies' idle timers
                            // reset and detect half-broken sockets
                            // promptly. A failed Ping is classified as
                            // a send-path failure per D-05: abort any
                            // in-flight turn, emit a close frame, and
                            // exit the connection loop.
                            if let Err(err) = socket
                                .send_raw(Message::Ping(Bytes::new()))
                                .await
                            {
                                let in_flight = in_flight_turn.is_some();
                                let session_id = in_flight_turn
                                    .as_ref()
                                    .map(|t| t.session_id.as_str())
                                    .unwrap_or("unknown");
                                warn!(
                                    session_id = %session_id,
                                    reason = %err,
                                    in_flight,
                                    "websocket keepalive ping failed; closing connection"
                                );
                                if let Some(turn) = in_flight_turn.take() {
                                    turn.handle.abort();
                                }
                                send_close_frame(
                                    &mut socket,
                                    CloseCode::Away,
                                    "keepalive failed",
                                )
                                .await;
                                break;
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
