//! Phase 36.3.7.11 Plan 01 (D-08 / D-15) — kanban dashboard WebSocket handler
//! and dashboard-side tail consumer.
//!
//! Two surface elements live here:
//!
//! 1. **`ws_kanban` route at `/api/ws/kanban`** — accepts a WS upgrade,
//!    subscribes to `AppState::kanban_tail_broadcast`, and forwards every
//!    new `KanbanWsEvent::TaskEventBatch` to the client. Lifecycle mirrors
//!    `server/ws.rs` byte-for-byte (5s keepalive Ping + close-frame on
//!    teardown + RAII receiver drop on exit).
//!
//! 2. **`run_kanban_tail_loop`** — polls `KanbanStore::list_all_events_after`
//!    at `tail_interval_ms` cadence (D-17 default 250 ms), advances a
//!    watermark, serializes a `KanbanWsEvent::TaskEventBatch`, and broadcasts
//!    the JSON string. Independent of the gateway notifier (D-15 — no
//!    `use ironhermes_kanban::notifier` import, no shared primitive).

use dioxus::prelude::*;
use dioxus_fullstack::{WebSocketOptions, Websocket};
#[cfg(feature = "server")]
use dioxus_fullstack::{body::Bytes, CloseCode, Message, TypedWebsocket};
#[cfg(feature = "server")]
use std::time::Duration;
#[cfg(feature = "server")]
use tracing::{info, warn};

#[cfg(feature = "server")]
use ironhermes_kanban::store::KanbanStore;

/// Phase 36.3.7.11 (D-08): kanban WebSocket application-level keepalive
/// interval. Identical to `server::ws::WS_KEEPALIVE_INTERVAL` — 5 s — well
/// under the ~9 s `dx serve` proxy idle-close threshold.
#[cfg(feature = "server")]
const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Phase 36.3.7.11 (D-08): best-effort close-frame emit before dropping the
/// socket. Mirrors `server::ws::send_close_frame` so every teardown branch
/// completes the WebSocket close handshake (no raw transport resets visible
/// to upstream proxies).
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

/// Phase 36.3.7.11 (D-08 / D-22): kanban dashboard WebSocket endpoint.
///
/// Auth inherited from the same session model that protects `/api/ws/chat`
/// (D-22 — no kanban-specific token). Subscribes to the dashboard tail
/// broadcaster and forwards JSON-serialized `KanbanWsEvent` frames to the
/// client. RAII `Receiver` drop on exit removes the client from the
/// broadcast fan-out (no explicit unsubscribe needed).
#[get("/api/ws/kanban")]
pub async fn ws_kanban(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    #[cfg(feature = "server")]
    let app_state = crate::server::state::global_app_state().clone();

    Ok(ws.on_upgrade(
        move |mut socket: dioxus_fullstack::TypedWebsocket<String, String>| {
            #[cfg(feature = "server")]
            let app_state = app_state.clone();
            async move {
                #[cfg(feature = "server")]
                {
                    info!("websocket kanban connection established");
                    let mut broadcast_rx = app_state.kanban_tail_broadcast.subscribe();

                    let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
                    keepalive.set_missed_tick_behavior(
                        tokio::time::MissedTickBehavior::Skip,
                    );
                    keepalive.tick().await;

                    loop {
                        tokio::select! {
                            // ── Incoming frames from the client ──────────
                            raw = socket.recv_raw() => {
                                match raw {
                                    Ok(Message::Text(_)) => {
                                        // Dashboard WS is server→client only; ignore
                                        // any text the client sends.
                                        continue;
                                    }
                                    Ok(Message::Close { code, reason }) => {
                                        warn!(
                                            code = ?code,
                                            reason = %reason,
                                            "websocket kanban close frame received; exiting connection"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Normal,
                                            "recv closed cleanly",
                                        )
                                        .await;
                                        break;
                                    }
                                    Ok(_) => continue,
                                    Err(err) => {
                                        warn!(
                                            reason = %err,
                                            "websocket kanban recv failed; closing connection"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "recv failed",
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                            // ── Tail broadcast → client ──────────────────
                            maybe_event = broadcast_rx.recv() => {
                                match maybe_event {
                                    Ok(json) => {
                                        if let Err(err) = socket
                                            .send_raw(Message::Text(json))
                                            .await
                                        {
                                            warn!(
                                                reason = %err,
                                                "websocket kanban send failed; closing connection"
                                            );
                                            send_close_frame(
                                                &mut socket,
                                                CloseCode::Away,
                                                "send failed",
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        // Client missed N events — log and continue.
                                        // Plan 02 reconnect logic will replay from
                                        // last_event_id cursor (D-08).
                                        warn!(
                                            lagged = n,
                                            "websocket kanban receiver lagged; events skipped"
                                        );
                                        continue;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                        // Tail loop dropped the sender — exit.
                                        warn!(
                                            "websocket kanban broadcast channel closed; exiting"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "broadcast closed",
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                            // ── Keepalive Ping ──────────────────────────
                            _ = keepalive.tick() => {
                                if let Err(err) = socket
                                    .send_raw(Message::Ping(Bytes::new()))
                                    .await
                                {
                                    warn!(
                                        reason = %err,
                                        "websocket kanban keepalive ping failed; closing connection"
                                    );
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
                    let unavailable = crate::protocol::KanbanWsEvent::Error {
                        message: "Kanban websocket unavailable without `server` feature"
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

// ---------------------------------------------------------------------------
// run_kanban_tail_loop — D-15 dashboard tail consumer
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-15 / D-17): dashboard tail consumer loop.
///
/// Polls `KanbanStore::list_all_events_after(watermark)` every
/// `interval_ms` milliseconds. On new events:
/// 1. Advance the watermark to `max(id)`.
/// 2. Build a `KanbanWsEvent::TaskEventBatch { events, last_event_id }`.
/// 3. Serialize to JSON and broadcast via `tx.send(json)`.
///
/// **Independent of the gateway notifier (D-15):** no
/// `use ironhermes_kanban::notifier` import; this loop only depends on
/// `KanbanStore::list_all_events_after` (Wave 0 helper).
///
/// Cancel-safety: every `tokio::select!` arm releases the `MutexGuard`
/// before any `.await` boundary that could be cancelled — preserves the
/// rusqlite `!Send` discipline (Pattern G in PATTERNS.md).
#[cfg(feature = "server")]
pub async fn run_kanban_tail_loop(
    tx: tokio::sync::broadcast::Sender<String>,
    cancel: tokio_util::sync::CancellationToken,
    interval_ms: u64,
) {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    // Open the dashboard's own KanbanStore connection (D-16: cross-process
    // WAL boundary — gateway notifier holds its own connection to the same
    // file). Q8: persistent per-loop connection avoids per-tick open cost.
    let store = match KanbanStore::open_default() {
        Ok(s) => Arc::new(TokioMutex::new(s)),
        Err(e) => {
            warn!(error = %e, "kanban tail: failed to open default board; tail disabled");
            return;
        }
    };

    // Initialize watermark via the lock-scope-before-anything-else pattern
    // (notifier.rs lines 426-442).
    let mut watermark: i64 = {
        let s = store.lock().await;
        match s.max_event_id() {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "kanban tail: max_event_id failed; tail disabled");
                return;
            }
        }
    };

    info!(
        interval_ms,
        watermark, "kanban tail consumer started"
    );

    let poll_interval = Duration::from_millis(interval_ms);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {
                // Read events under lock; release lock BEFORE broadcast send
                // (Pattern G — rusqlite !Send discipline).
                let read_result = {
                    let s = store.lock().await;
                    s.list_all_events_after(watermark)
                };
                let events = match read_result {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "kanban tail: list_all_events_after failed; continuing");
                        continue;
                    }
                };
                if events.is_empty() {
                    continue;
                }
                // Advance watermark BEFORE building the payload (events
                // already sorted id ASC by SQL).
                if let Some(last) = events.last() {
                    watermark = last.id;
                }
                let rows: Vec<crate::protocol::KanbanEventRow> = events
                    .into_iter()
                    .map(|e| crate::protocol::KanbanEventRow {
                        id: e.id,
                        task_id: e.task_id,
                        kind: e.kind,
                        payload: e.payload,
                        created_at: e.created_at,
                    })
                    .collect();
                let batch = crate::protocol::KanbanWsEvent::TaskEventBatch {
                    events: rows,
                    last_event_id: watermark,
                };
                let json = match serde_json::to_string(&batch) {
                    Ok(j) => j,
                    Err(e) => {
                        warn!(error = %e, "kanban tail: serialize TaskEventBatch failed; skipping");
                        continue;
                    }
                };
                // Silently discard SendError when no receivers (Q1).
                let _ = tx.send(json);
            }
            _ = cancel.cancelled() => {
                info!("kanban tail consumer cancelled; exiting");
                break;
            }
        }
    }
}
