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
use tracing::{info, warn};

pub use crate::protocol::{ChatRequest, ChatStreamEvent};

/// Phase 26.7.1 Plan 02 (D-06 / Path A): RAII guard that clears the per-turn
/// callback slot on drop. Ensures the slot is reset to None even if
/// `run_web_turn` panics — the tokio task's drop machinery runs Drop before
/// the JoinHandle's error propagates.
#[cfg(feature = "server")]
struct SubagentCallbackSlotGuard {
    slot: std::sync::Arc<
        tokio::sync::Mutex<
            Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ChatStreamEvent>>,
        >,
    >,
}

#[cfg(feature = "server")]
impl Drop for SubagentCallbackSlotGuard {
    fn drop(&mut self) {
        // Best-effort clear. Use try_lock since Drop cannot await.
        // The slot is held only across very short windows; contention is
        // not expected outside of pathological teardown cases.
        if let Ok(mut guard) = self.slot.try_lock() {
            *guard = None;
        }
        // If try_lock fails (extremely unlikely — only the callback's
        // try_lock contends, and it doesn't hold the lock across .send),
        // we leak a stale Some(tx) until the next turn overwrites it. The
        // closed channel makes any further send a silent no-op. Acceptable.
    }
}

/// Server-side application-level WebSocket keepalive interval.
///
/// Application-level Ping frames keep intermediate proxy idle timers
/// reset and detect half-broken sockets promptly. Browsers automatically
/// respond to Ping with Pong at the WebSocket protocol level, so the
/// client requires no changes. Pong frames are skipped in the recv_raw
/// match arm.
///
/// 5 seconds is well below the ~9s idle-close threshold observed with
/// the dx serve proxy and matches the low end of common reverse-proxy
/// keepalive intervals.
#[cfg(feature = "server")]
const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Best-effort WebSocket close-frame emit before dropping the socket.
///
/// Ensures every teardown branch completes the WebSocket close handshake
/// so upstream proxies do not observe a raw transport reset.
/// Errors are intentionally swallowed — if the send fails the transport
/// is already broken and we must not block teardown.
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

                info!("websocket chat connection established");
                let mut in_flight_turn: Option<InFlightTurn> = None;

                let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // Skip first tick so we don't Ping immediately on connect.
                keepalive.tick().await;

                loop {
                    tokio::select! {
                        // ── Incoming frames from the client ──────────────────────
                        //
                        // Use recv_raw so we handle each frame type explicitly.
                        // TypedWebsocket::recv() (the typed/Stream path) tries to
                        // JSON-decode the text frame as type String, which fails for
                        // raw JSON object payloads like {"session_id":...,"message":...}
                        // because a JSON object is not a JSON string literal. Using
                        // recv_raw bypasses that decode layer entirely — we read the
                        // raw text and parse it ourselves as ChatRequest.
                        raw = socket.recv_raw() => {
                            let text = match raw {
                                Ok(Message::Text(t)) => {
                                    info!("websocket chat message received (len={})", t.len());
                                    t
                                }
                                Ok(Message::Close { code, reason }) => {
                                    let in_flight = in_flight_turn.is_some();
                                    let session_id = in_flight_turn
                                        .as_ref()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");
                                    warn!(
                                        session_id = %session_id,
                                        code = ?code,
                                        reason = %reason,
                                        in_flight,
                                        "websocket close frame received; exiting connection"
                                    );
                                    if let Some(turn) = in_flight_turn.take() {
                                        let _ = turn.handle.await;
                                    }
                                    send_close_frame(
                                        &mut socket,
                                        CloseCode::Normal,
                                        "recv closed cleanly",
                                    )
                                    .await;
                                    break;
                                }
                                // Ping/Pong/Binary — skip silently.
                                Ok(_) => continue,
                                Err(err) => {
                                    let reason = err.to_string();
                                    let in_flight = in_flight_turn.is_some();
                                    let session_id = in_flight_turn
                                        .as_ref()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");
                                    warn!(
                                        session_id = %session_id,
                                        reason = %reason,
                                        in_flight,
                                        "websocket recv failed; closing connection"
                                    );
                                    if let Some(turn) = in_flight_turn.take() {
                                        turn.handle.abort();
                                    }
                                    send_close_frame(&mut socket, CloseCode::Away, "recv failed")
                                        .await;
                                    break;
                                }
                            };

                            let req: ChatRequest = match serde_json::from_str(&text) {
                                Ok(r) => r,
                                Err(e) => {
                                    let err_event = ChatStreamEvent::Error {
                                        message: format!("Invalid request: {e}"),
                                    };
                                    let _ = socket
                                        .send_raw(Message::Text(
                                            serde_json::to_string(&err_event)
                                                .unwrap_or_default(),
                                        ))
                                        .await;
                                    continue;
                                }
                            };

                            // WR-02: clear finished turn handle before busy-gate check
                            // to avoid false rejection when a frame arrives just after
                            // the prior turn's task has completed but before its tear-down.
                            if let Some(turn) = in_flight_turn.as_ref() {
                                if turn.handle.is_finished() {
                                    in_flight_turn = None;
                                }
                            }

                            if in_flight_turn.is_some() {
                                let busy = ChatStreamEvent::Error {
                                    message: "Another request is already in progress".to_string(),
                                };
                                let _ = socket
                                    .send_raw(Message::Text(
                                        serde_json::to_string(&busy).unwrap_or_default(),
                                    ))
                                    .await;
                                continue;
                            }

                            let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                            let app_state = app_state.clone();
                            let session_id = req.session_id;
                            let session_id_for_turn = session_id.clone();
                            let message = req.message;
                            let handle = tokio::spawn(async move {
                                // Phase 34a MEM-READ-05: scrub <memory-context> fence tags.
                                let scrubber_ws = std::sync::Arc::new(std::sync::Mutex::new(
                                    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
                                ));
                                let scrubber_ws_cb = std::sync::Arc::clone(&scrubber_ws);
                                let tx_stream = tx.clone();
                                let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                                    Box::new(move |delta: &str| {
                                        let visible = scrubber_ws_cb.lock().unwrap().feed(delta);
                                        if !visible.is_empty() {
                                            let _ = tx_stream.send(ChatStreamEvent::Delta {
                                                text: visible,
                                            });
                                        }
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

                                // Phase 26.7.1 Plan 02 (D-06 / Path A): install this turn's tx into the
                                // callback slot so the singleton SubagentProgressCallback baked into
                                // AppRuntimeBundle can forward SubagentEvent {} to this client.
                                let tx_subagent = tx.clone();
                                {
                                    let mut guard = app_state.subagent_callback_slot.lock().await;
                                    *guard = Some(tx_subagent);
                                }
                                let _slot_guard = SubagentCallbackSlotGuard {
                                    slot: app_state.subagent_callback_slot.clone(),
                                };
                                // _slot_guard is dropped at end-of-block (after run_web_turn returns or
                                // panics), restoring slot to None.

                                let result = app_state
                                    .run_web_turn(
                                        &session_id_for_turn,
                                        &message,
                                        stream_callback,
                                        Some(tool_progress_callback),
                                        Some(tool_result_callback),
                                    )
                                    .await;

                                // Phase 34a MEM-READ-05: flush scrubber tail after stream ends.
                                let tail = scrubber_ws.lock().unwrap().flush();
                                if !tail.is_empty() {
                                    let _ = tx.send(ChatStreamEvent::Delta { text: tail });
                                }

                                match result {
                                    Ok(agent_result) => {
                                        let _ = tx.send(ChatStreamEvent::Finished {
                                            total_tokens: agent_result.total_usage.total_tokens
                                                as u32,
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

                        // ── Agent stream events → client ──────────────────────────
                        maybe_event = async {
                            match in_flight_turn.as_mut() {
                                Some(turn) => turn.rx.recv().await,
                                None => std::future::pending().await,
                            }
                        } => {
                            match maybe_event {
                                Some(event) => {
                                    // Use send_raw(Text) so the client receives a plain
                                    // JSON text frame. TypedWebsocket::send() (the Sink
                                    // path) encodes via JsonEncoding into a binary frame,
                                    // which doesn't match the client's recv_raw Text arm.
                                    let json = serde_json::to_string(&event).unwrap_or_default();
                                    if let Err(err) = socket
                                        .send_raw(Message::Text(json))
                                        .await
                                    {
                                        if let Some(turn) = in_flight_turn.take() {
                                            warn!(
                                                session_id = %turn.session_id,
                                                reason = %err,
                                                in_flight = true,
                                                "websocket send failed; aborting in-flight turn"
                                            );
                                            turn.handle.abort();
                                        }
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
                                            warn!(
                                                session_id = %turn.session_id,
                                                reason = %err,
                                                in_flight = false,
                                                "turn task join failed"
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // ── Keepalive Ping ────────────────────────────────────────
                        _ = keepalive.tick() => {
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
                        .send_raw(Message::Text(
                            serde_json::to_string(&unavailable).unwrap_or_default(),
                        ))
                        .await;
                }
            }
        },
    ))
}

#[cfg(test)]
#[cfg(feature = "server")]
mod plan_26_7_1_02_tests {
    use super::*;
    use crate::protocol::ChatStreamEvent;
    use ironhermes_tools::delegate_task::{SubagentProgress, SubagentProgressCallback};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    /// Phase 26.7.1 Plan 02 (Wave 0): D-06 callback wiring shape.
    /// Mirrors the callback constructed in state.rs Task 2: lock the slot,
    /// read Some(tx), send ChatStreamEvent::SubagentEvent {}.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_subagent_callback_emits_event() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
        let slot: Arc<Mutex<Option<mpsc::UnboundedSender<ChatStreamEvent>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let cb_slot = slot.clone();
        let cb: SubagentProgressCallback = Arc::new(move |_index: usize, _event: SubagentProgress| {
            if let Ok(guard) = cb_slot.try_lock() {
                if let Some(s) = guard.as_ref() {
                    let _ = s.send(ChatStreamEvent::SubagentEvent {});
                }
            }
        });

        // Invoke the callback as the delegate-task runner would.
        cb(0, SubagentProgress::Completed);

        let received = rx.recv().await.expect("expected SubagentEvent");
        assert!(
            matches!(received, ChatStreamEvent::SubagentEvent {}),
            "callback must send the SubagentEvent variant"
        );

        // After clearing the slot, the callback becomes a silent no-op.
        {
            let mut g = slot.lock().await;
            *g = None;
        }
        cb(1, SubagentProgress::Completed);
        // Nothing should arrive — give the runtime a moment to surface anything.
        // Accept either: Err(Elapsed) = timeout (slot None, channel still open),
        // or Ok(None) = channel closed (all senders dropped when slot cleared).
        // Both mean no SubagentEvent was sent by the second cb invocation.
        let timed = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        let no_spurious_event = match timed {
            Err(_) => true,          // timeout — nothing in channel
            Ok(None) => true,        // channel closed — all senders dropped
            Ok(Some(_)) => false,    // unexpected event sent after slot was cleared
        };
        assert!(no_spurious_event, "no events should be received after slot is cleared");
    }
}
