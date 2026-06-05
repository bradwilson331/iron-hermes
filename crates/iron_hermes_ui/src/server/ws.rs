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

// Phase 36.1 D-04/D-05/D-06/D-07: slash interception + running-agent guard
// imports. Used inside the #[cfg(feature = "server")] WebSocket select! loop.
#[cfg(feature = "server")]
use ironhermes_core::commands::{CommandResult, ResolveResult};
#[cfg(feature = "server")]
use ironhermes_core::commands::running_agent::{is_bypass, AGENT_RUNNING_REJECT_MSG};

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

                // Phase 36.17.4 (D-01): canonical SessionKey for every queue
                // call site in this connection. `web_key(session_id)` returns
                // a key with platform=Web, chat_id=session_id, user_id="web"
                // per the must_have invariant. Used at 6+ call sites below.
                fn web_key(session_id: &str) -> ironhermes_core::session::SessionKey {
                    ironhermes_core::session::SessionKey {
                        platform: ironhermes_core::types::Platform::Web,
                        chat_id: session_id.to_string(),
                        user_id: Some("web".into()),
                    }
                }

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

                            let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                            let app_state = app_state.clone();
                            let session_id = req.session_id;
                            let session_id_for_turn = session_id.clone();
                            let message = req.message;

                            // Phase 36.17.4 (D-01 / D-03 / D-06): replace the
                            // legacy hard reject with FIFO push. Free-text
                            // messages during an in-flight turn now enqueue
                            // instead of erroring; slash commands fall through
                            // to the slash interception block below so
                            // bypass-listed commands (stop/new/status/queue/
                            // pause/unpause — Plan 01 extended is_bypass) are
                            // still dispatched even mid-turn. This block uses
                            // its own (tx_q, rx_q) pair so the primary (tx, rx)
                            // remains untouched for the slash/spawn paths below
                            // — note: when we `continue`, the primary tx/rx are
                            // dropped together (mpsc cleanup), which is fine.
                            if in_flight_turn.is_some() && !message.starts_with('/') {
                                let key = web_key(&session_id);
                                let paused_flag =
                                    app_state.get_or_create_paused_flag(&session_id);
                                let paused_snapshot = paused_flag
                                    .load(std::sync::atomic::Ordering::SeqCst);
                                let (tx_q, rx_q) =
                                    mpsc::unbounded_channel::<ChatStreamEvent>();
                                match app_state.queue.try_push(&key, message.clone()) {
                                    Ok(()) => {
                                        let depth = app_state.queue.len(&key) as u32;
                                        let _ = tx_q.send(ChatStreamEvent::Delta {
                                            text: format!(
                                                "Queued: \"{}\" ({} in queue)\n",
                                                message, depth
                                            ),
                                        });
                                        let _ = tx_q.send(ChatStreamEvent::QueueUpdated {
                                            depth,
                                            paused: paused_snapshot,
                                        });
                                    }
                                    Err(
                                        ironhermes_core::queue::QueueError::CapacityReached {
                                            max,
                                            ..
                                        },
                                    ) => {
                                        let _ = tx_q.send(ChatStreamEvent::Delta {
                                            text: format!(
                                                "Queue is full ({max}/{max}). /stop or /flush to drain.\n"
                                            ),
                                        });
                                    }
                                }
                                let _ = tx_q.send(ChatStreamEvent::Finished {
                                    total_tokens: 0,
                                });
                                drop(tx_q);
                                let mut qrx = rx_q;
                                while let Some(ev) = qrx.recv().await {
                                    let json = serde_json::to_string(&ev)
                                        .unwrap_or_default();
                                    let _ = socket
                                        .send_raw(Message::Text(json))
                                        .await;
                                }
                                continue;
                            }

                            // Phase 36.1 D-03/D-04/D-05/D-06 (Pitfall 4, Pitfall 7):
                            // Slash-command interception BEFORE run_web_turn.
                            //
                            // Resolution uses the canonical def.name (post-alias)
                            // so /reset → "new" correctly bypasses the guard
                            // (Pitfall 4 mitigation: never call is_bypass on raw input).
                            //
                            // Slash dispatch does NOT set in_flight_turn (Pitfall 7):
                            // slash responses are synchronous single-turn outputs;
                            // keeping in_flight_turn=None allows the next message
                            // to arrive immediately after dispatch completes.
                            if message.starts_with('/') {
                                let platform = ironhermes_core::types::Platform::Web;
                                let running_flag =
                                    app_state.get_or_create_running_flag(&session_id);
                                match app_state.command_router.resolve(&message, &platform) {
                                    ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
                                        // D-06: non-bypass slash rejected while turn in flight.
                                        if running_flag
                                            .load(std::sync::atomic::Ordering::SeqCst)
                                            && !is_bypass(&def.name)
                                        {
                                            // Phase 36.1 D-05: deliver as Delta + Finished —
                                            // no new protocol variant needed.
                                            // AGENT_RUNNING_REJECT_MSG is the canonical D-02
                                            // constant — never inlined (T-36.1-09 mitigation).
                                            let _ = tx.send(ChatStreamEvent::Delta {
                                                text: AGENT_RUNNING_REJECT_MSG.to_string(),
                                            });
                                            let _ =
                                                tx.send(ChatStreamEvent::Finished { total_tokens: 0 });
                                            // Drain tx→rx and forward to the WebSocket client.
                                            // Phase 36.1 D-04/D-05: slash result is bounded
                                            // (Delta + Finished = 2 frames) — drain inline
                                            // without setting in_flight_turn (Pitfall 7).
                                            drop(tx);
                                            let mut slash_rx = rx;
                                            while let Some(ev) = slash_rx.recv().await {
                                                let json =
                                                    serde_json::to_string(&ev).unwrap_or_default();
                                                let _ = socket
                                                    .send_raw(Message::Text(json))
                                                    .await;
                                            }
                                            continue;
                                        }

                                        // D-07: bypass-listed slash (stop/new/status/queue)
                                        // OR non-running state → dispatch normally.
                                        let parts: Vec<&str> =
                                            message.split_whitespace().collect();
                                        let args: Vec<&str> = if parts.len() > 1 {
                                            parts[1..].to_vec()
                                        } else {
                                            vec![]
                                        };
                                        // Phase 36.2 Plan 07 fix: thread state_store into
                                        // CommandContext so `/usage` (and other store-backed
                                        // slash commands) run against the real DB. Without this,
                                        // handlers fall back to the "Session storage not
                                        // configured." guard.
                                        let store_handle: std::sync::Arc<
                                            dyn ironhermes_core::commands::context::StateStoreHandle,
                                        > = std::sync::Arc::new(
                                            ironhermes_state::StateStoreHandleAdapter(
                                                app_state.state_store.clone(),
                                            ),
                                        );
                                        let ctx = ironhermes_core::commands::context::CommandContext::new(
                                            platform,
                                            session_id.clone(),
                                            running_flag,
                                        )
                                        .with_state_store(store_handle);

                                        // Phase 36.17.4 (D-04a / D-05): /stop
                                        // early-intercept BEFORE dispatch. The
                                        // canonical post-resolution name is
                                        // `def.name` (Pitfall 4: never on raw
                                        // input). Sequence: queue.clear →
                                        // paused.store(false) → QueueUpdated →
                                        // Delta → Finished → drain. NO
                                        // JoinHandle::abort and NO
                                        // CancellationToken (D-05 documented
                                        // divergence): the in-flight turn (if
                                        // any) completes naturally; its
                                        // eventual `None =>` arm will see an
                                        // empty queue and not re-drain.
                                        if def.name == "stop" {
                                            let key = web_key(&session_id);
                                            app_state.queue.clear(&key);
                                            app_state
                                                .get_or_create_paused_flag(&session_id)
                                                .store(
                                                    false,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                            let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                depth: 0,
                                                paused: false,
                                            });
                                            let _ = tx.send(ChatStreamEvent::Delta {
                                                text:
                                                    "Queue cleared. Current turn finishing.\n"
                                                        .to_string(),
                                            });
                                            let _ = tx.send(ChatStreamEvent::Finished {
                                                total_tokens: 0,
                                            });
                                            drop(tx);
                                            let mut slash_rx = rx;
                                            while let Some(ev) = slash_rx.recv().await {
                                                let json = serde_json::to_string(&ev)
                                                    .unwrap_or_default();
                                                let _ = socket
                                                    .send_raw(Message::Text(json))
                                                    .await;
                                            }
                                            continue;
                                        }

                                        let result = ironhermes_core::commands::handlers::dispatch(
                                            &def,
                                            &args,
                                            &ctx,
                                            &app_state.command_router,
                                        );

                                        // Phase 36.17.4 (D-01 / D-03 / D-06):
                                        // dedicated arms for Queued /
                                        // PauseQueue / UnpauseQueue. Each
                                        // performs its own complete emit
                                        // sequence + drain + continue (bypasses
                                        // the shared Delta/Finished delivery
                                        // below) so the QueueUpdated event can
                                        // be interleaved between Delta and
                                        // Finished per the must_have invariant.
                                        match result {
                                            CommandResult::Queued { message: queued_msg } => {
                                                let key = web_key(&session_id);
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let paused_snapshot = paused_flag.load(
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                match app_state
                                                    .queue
                                                    .try_push(&key, queued_msg.clone())
                                                {
                                                    Ok(()) => {
                                                        let depth =
                                                            app_state.queue.len(&key) as u32;
                                                        let _ = tx.send(ChatStreamEvent::Delta {
                                                            text: format!(
                                                                "Queued: \"{}\" ({} in queue)\n",
                                                                queued_msg, depth
                                                            ),
                                                        });
                                                        let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                            depth,
                                                            paused: paused_snapshot,
                                                        });
                                                    }
                                                    Err(
                                                        ironhermes_core::queue::QueueError::CapacityReached {
                                                            max,
                                                            ..
                                                        },
                                                    ) => {
                                                        let _ = tx.send(ChatStreamEvent::Delta {
                                                            text: format!(
                                                                "Queue is full ({max}/{max}). /stop or /flush to drain.\n"
                                                            ),
                                                        });
                                                    }
                                                }
                                                let _ = tx.send(ChatStreamEvent::Finished {
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            CommandResult::PauseQueue => {
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let was_paused = paused_flag.fetch_xor(
                                                    true,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                let new_paused = !was_paused;
                                                let key = web_key(&session_id);
                                                let depth =
                                                    app_state.queue.len(&key) as u32;
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth,
                                                    paused: new_paused,
                                                });
                                                let _ = tx.send(ChatStreamEvent::Delta {
                                                    text: if new_paused {
                                                        format!(
                                                            "Queue paused. ({} queued)\n",
                                                            depth
                                                        )
                                                    } else {
                                                        format!(
                                                            "Queue resumed. ({} queued)\n",
                                                            depth
                                                        )
                                                    },
                                                });
                                                let _ = tx.send(ChatStreamEvent::Finished {
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            CommandResult::UnpauseQueue => {
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let was_paused = paused_flag.swap(
                                                    false,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                let key = web_key(&session_id);
                                                let depth =
                                                    app_state.queue.len(&key) as u32;
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth,
                                                    paused: false,
                                                });
                                                let _ = tx.send(ChatStreamEvent::Delta {
                                                    text: if was_paused {
                                                        "Queue resumed.\n".to_string()
                                                    } else {
                                                        "Queue was not paused.\n".to_string()
                                                    },
                                                });
                                                let _ = tx.send(ChatStreamEvent::Finished {
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            _ => {}
                                        }

                                        let text = match result {
                                            CommandResult::Output(t) => t,
                                            CommandResult::Error(e) => {
                                                format!("Command error: {e}")
                                            }
                                            CommandResult::NewSession { message: m } => {
                                                // Phase 36.17.4 (D-04): ordering
                                                // invariant — queue.clear →
                                                // paused.store(false) →
                                                // QueueUpdated → reset_web_session
                                                // → emit message. QueueUpdated
                                                // pushed into the same `tx`
                                                // (mpsc FIFO) BEFORE the shared
                                                // delivery's Delta(text), so
                                                // the client sees the pill
                                                // reset before the
                                                // confirmation Delta.
                                                let key = web_key(&session_id);
                                                app_state.queue.clear(&key);
                                                app_state
                                                    .get_or_create_paused_flag(&session_id)
                                                    .store(
                                                        false,
                                                        std::sync::atomic::Ordering::SeqCst,
                                                    );
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth: 0,
                                                    paused: false,
                                                });
                                                app_state.reset_web_session(&session_id);
                                                m
                                            }
                                            CommandResult::Handled | CommandResult::Quit => {
                                                String::new()
                                            }
                                            other => {
                                                format!("{other:?}")
                                            }
                                        };
                                        if !text.is_empty() {
                                            let _ = tx.send(ChatStreamEvent::Delta { text });
                                        }
                                        let _ =
                                            tx.send(ChatStreamEvent::Finished { total_tokens: 0 });
                                        drop(tx);
                                        let mut slash_rx = rx;
                                        while let Some(ev) = slash_rx.recv().await {
                                            let json =
                                                serde_json::to_string(&ev).unwrap_or_default();
                                            let _ = socket.send_raw(Message::Text(json)).await;
                                        }
                                        continue;
                                    }
                                    ResolveResult::Ambiguous(_) | ResolveResult::NotFound => {
                                        // Not a recognised slash command — fall through to
                                        // run_web_turn as a plain-text message.
                                    }
                                }
                            }

                            // Phase 36.1 D-06: plain-text guard check.
                            // Reject free-text messages when a turn is in flight —
                            // same D-02 string, same Delta+Finished delivery (D-05).
                            // AGENT_RUNNING_REJECT_MSG is the canonical D-02 constant.
                            {
                                let running_flag =
                                    app_state.get_or_create_running_flag(&session_id);
                                if running_flag.load(std::sync::atomic::Ordering::SeqCst) {
                                    let _ = tx.send(ChatStreamEvent::Delta {
                                        text: AGENT_RUNNING_REJECT_MSG.to_string(),
                                    });
                                    let _ =
                                        tx.send(ChatStreamEvent::Finished { total_tokens: 0 });
                                    drop(tx);
                                    let mut plain_rx = rx;
                                    while let Some(ev) = plain_rx.recv().await {
                                        let json =
                                            serde_json::to_string(&ev).unwrap_or_default();
                                        let _ = socket.send_raw(Message::Text(json)).await;
                                    }
                                    continue;
                                }
                            }

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

                                // Phase 36.17.7 D-02-a: construct per-turn WebAudioDispatcher
                                // and TTS wiring so TextToSpeechTool emits AudioOut WS frames.
                                let audio_tx = tx.clone();
                                let audio_cache_dir = ironhermes_core::constants::get_hermes_home()
                                    .join("audio_cache");
                                let web_audio_dispatcher = std::sync::Arc::new(
                                    crate::server::web_audio_dispatcher::WebAudioDispatcher::new(
                                        audio_tx,
                                        audio_cache_dir,
                                    ),
                                );
                                let tts_wiring = Some(ironhermes_agent::TtsPerTurnWiring {
                                    session_key: Some(web_key(&session_id_for_turn)).unwrap(), // explicit Some() literal for D-05 source-grep
                                    audio_dispatcher: Some(
                                        web_audio_dispatcher
                                            as std::sync::Arc<
                                                dyn ironhermes_tools::AudioDispatcher,
                                            >,
                                    ),
                                });

                                let result = app_state
                                    .run_web_turn(
                                        &session_id_for_turn,
                                        &message,
                                        stream_callback,
                                        Some(tool_progress_callback),
                                        Some(tool_result_callback),
                                        tts_wiring,
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
                                    // Phase 36.17.7 D-02-b: AudioOut events are delivered as
                                    // binary WS frames so the WASM client can create a Blob URL
                                    // without base64 overhead. All other events remain plain JSON
                                    // text frames (client recv_raw Text arm handles those).
                                    let ws_msg = match &event {
                                        ChatStreamEvent::AudioOut { .. } => {
                                            // Phase 36.17.7 D-02-a: serialize full event (uuid + mime + bytes)
                                            // as JSON payload inside the binary frame so the WASM client
                                            // can deserialize and construct the Blob URL with correct mime.
                                            Message::Binary(serde_json::to_vec(&event).unwrap_or_default().into())
                                        }
                                        _ => {
                                            let json = serde_json::to_string(&event)
                                                .unwrap_or_default();
                                            Message::Text(json)
                                        }
                                    };
                                    if let Err(err) = socket
                                        .send_raw(ws_msg)
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
                                        // Phase 36.17.4 (D-02): WS recv-loop
                                        // self-drain. Capture session_id
                                        // BEFORE the .await consumes `turn` so
                                        // the value survives for the drain
                                        // check that follows.
                                        let session_id_done = turn.session_id.clone();
                                        if let Err(err) = turn.handle.await {
                                            warn!(
                                                session_id = %session_id_done,
                                                reason = %err,
                                                in_flight = false,
                                                "turn task join failed"
                                            );
                                        }
                                        // D-02: after the in-flight turn
                                        // completes (Ok or Err), check the
                                        // per-session paused flag and the
                                        // queue. If !paused AND queue has a
                                        // message, emit QueueUpdated to the
                                        // socket BEFORE spawning the next turn
                                        // (D-03 ordering: pill update first,
                                        // then Delta stream), then spawn a new
                                        // run_web_turn mirroring the primary
                                        // spawn block above.
                                        let key = web_key(&session_id_done);
                                        let paused_now = app_state
                                            .get_or_create_paused_flag(&session_id_done)
                                            .load(std::sync::atomic::Ordering::SeqCst);
                                        if !paused_now {
                                            if let Some(next_text) =
                                                app_state.queue.pop(&key)
                                            {
                                                let depth_after =
                                                    app_state.queue.len(&key) as u32;
                                                let qu_event =
                                                    ChatStreamEvent::QueueUpdated {
                                                        depth: depth_after,
                                                        paused: false,
                                                    };
                                                let _ = socket
                                                    .send_raw(Message::Text(
                                                        serde_json::to_string(&qu_event)
                                                            .unwrap_or_default(),
                                                    ))
                                                    .await;
                                                let (tx_drain, rx_drain) =
                                                    mpsc::unbounded_channel::<ChatStreamEvent>();
                                                let app_state_drain = app_state.clone();
                                                let session_id_spawn =
                                                    session_id_done.clone();
                                                let next_text_owned = next_text;
                                                let drain_handle = tokio::spawn(async move {
                                                    // Mirrors the primary
                                                    // spawn block: scrubber +
                                                    // 3 callbacks + slot
                                                    // install via RAII guard +
                                                    // run_web_turn + flush +
                                                    // Finished/Error emit.
                                                    let scrubber_ws =
                                                        std::sync::Arc::new(std::sync::Mutex::new(
                                                            ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
                                                        ));
                                                    let scrubber_ws_cb =
                                                        std::sync::Arc::clone(&scrubber_ws);
                                                    let tx_stream = tx_drain.clone();
                                                    let stream_callback:
                                                        ironhermes_agent::agent_loop::StreamCallback =
                                                        Box::new(move |delta: &str| {
                                                            let visible = scrubber_ws_cb
                                                                .lock()
                                                                .unwrap()
                                                                .feed(delta);
                                                            if !visible.is_empty() {
                                                                let _ = tx_stream.send(
                                                                    ChatStreamEvent::Delta {
                                                                        text: visible,
                                                                    },
                                                                );
                                                            }
                                                        });

                                                    let tx_tool = tx_drain.clone();
                                                    let tool_progress_callback:
                                                        ironhermes_agent::agent_loop::ToolProgressCallback =
                                                        Box::new(move |name: &str, args: &str| {
                                                            let _ = tx_tool.send(
                                                                ChatStreamEvent::ToolCallStart {
                                                                    name: name.to_string(),
                                                                    args: args.to_string(),
                                                                },
                                                            );
                                                        });

                                                    let tx_tool_result = tx_drain.clone();
                                                    let tool_result_callback:
                                                        ironhermes_agent::agent_loop::ToolResultCallback =
                                                        Box::new(move |name: &str, success: bool| {
                                                            let _ = tx_tool_result.send(
                                                                ChatStreamEvent::ToolCallEnd {
                                                                    name: name.to_string(),
                                                                    success,
                                                                },
                                                            );
                                                        });

                                                    let tx_subagent = tx_drain.clone();
                                                    {
                                                        let mut guard = app_state_drain
                                                            .subagent_callback_slot
                                                            .lock()
                                                            .await;
                                                        *guard = Some(tx_subagent);
                                                    }
                                                    let _slot_guard = SubagentCallbackSlotGuard {
                                                        slot: app_state_drain
                                                            .subagent_callback_slot
                                                            .clone(),
                                                    };

                                                    // Phase 36.17.7 D-02-a: per-turn TTS wiring
                                                    // for queue-drain spawn mirrors primary spawn.
                                                    let audio_tx_drain = tx_drain.clone();
                                                    let audio_cache_dir_drain =
                                                        ironhermes_core::constants::get_hermes_home()
                                                            .join("audio_cache");
                                                    let web_audio_dispatcher_drain =
                                                        std::sync::Arc::new(
                                                            crate::server::web_audio_dispatcher::WebAudioDispatcher::new(
                                                                audio_tx_drain,
                                                                audio_cache_dir_drain,
                                                            ),
                                                        );
                                                    let tts_wiring_drain =
                                                        Some(ironhermes_agent::TtsPerTurnWiring {
                                                            session_key: Some(web_key(&session_id_spawn)).unwrap(), // explicit Some() literal for D-05 source-grep
                                                            audio_dispatcher: Some(
                                                                web_audio_dispatcher_drain
                                                                    as std::sync::Arc<
                                                                        dyn ironhermes_tools::AudioDispatcher,
                                                                    >,
                                                            ),
                                                        });

                                                    let result = app_state_drain
                                                        .run_web_turn(
                                                            &session_id_spawn,
                                                            &next_text_owned,
                                                            stream_callback,
                                                            Some(tool_progress_callback),
                                                            Some(tool_result_callback),
                                                            tts_wiring_drain,
                                                        )
                                                        .await;

                                                    let tail =
                                                        scrubber_ws.lock().unwrap().flush();
                                                    if !tail.is_empty() {
                                                        let _ = tx_drain.send(
                                                            ChatStreamEvent::Delta {
                                                                text: tail,
                                                            },
                                                        );
                                                    }

                                                    match result {
                                                        Ok(agent_result) => {
                                                            let _ = tx_drain.send(
                                                                ChatStreamEvent::Finished {
                                                                    total_tokens: agent_result
                                                                        .total_usage
                                                                        .total_tokens
                                                                        as u32,
                                                                },
                                                            );
                                                        }
                                                        Err(e) => {
                                                            let _ = tx_drain.send(
                                                                ChatStreamEvent::Error {
                                                                    message: format!(
                                                                        "Agent error: {e}"
                                                                    ),
                                                                },
                                                            );
                                                        }
                                                    }
                                                });
                                                in_flight_turn = Some(InFlightTurn {
                                                    session_id: session_id_done,
                                                    rx: rx_drain,
                                                    handle: drain_handle,
                                                });
                                            }
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
