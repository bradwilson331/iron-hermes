//! `session/cancel` (Phase 36.8 plan 03, task 3) — fires the target session's CURRENT
//! per-turn cancellation token so an in-flight `run_turn` observes it (via `AgentLoop`'s
//! `tokio::select!` race against the LLM call — `crates/ironhermes-agent/src/
//! agent_loop.rs`) and returns the cancelled stop reason.
//!
//! Per the pinned `agent-client-protocol-schema` 1.5.0 (confirmed in plan 01's recorded
//! SDK surface), `session/cancel` arrives as a NOTIFICATION (`CancelNotification`), never
//! a request/response pair — this handler MUST be registered via `on_receive_notification`
//! (never `on_receive_request`); responding to a notification would desync a hand-rolled
//! client.
//!
//! Deliberately takes `AcpSessionManager::cancel_tokens()`'s map directly, NOT
//! `Arc<TokioMutex<AcpSessionManager>>`: `handle_session_prompt` holds the manager's own
//! lock for the full duration of an in-flight `run_turn` (plan 01's documented
//! simplification), so a cancel handler that needed the SAME lock could never fire a
//! token until the very turn it's cancelling had already finished on its own — defeating
//! cancellation entirely. The `std::sync::Mutex`-guarded token map is cheap to acquire
//! (never held across an `.await`) even while the big manager lock is held elsewhere.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::v1::CancelNotification;
use tokio_util::sync::CancellationToken;

/// Handles a `session/cancel` notification: looks the session up in the shared
/// cancel-token map and fires its current `CancellationToken`.
///
/// - Unknown session id -> `Err` (not a panic) — the notification dispatcher owns whether
///   that's surfaced anywhere observable; there is no JSON-RPC response channel for a
///   notification, so this is Rust-level error signaling, not a wire-level error reply.
/// - A session with no in-flight turn -> `Ok(())`, a harmless no-op: firing an unused
///   token has no observable effect. The NEXT `session/prompt` on this session replaces
///   the token with a fresh one before the turn starts (see `handlers::handle_session_prompt`),
///   so a stray/early cancel can never poison a future turn (T-36.8-16).
pub async fn handle_session_cancel(
    notif: CancelNotification,
    cancel_tokens: Arc<Mutex<HashMap<String, CancellationToken>>>,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = notif.session_id.to_string();
    let tokens = cancel_tokens.lock().unwrap_or_else(|e| e.into_inner());
    match tokens.get(&session_id) {
        Some(token) => {
            token.cancel();
            Ok(())
        }
        None => Err(agent_client_protocol::Error::invalid_params()
            .data(format!("session/cancel: unknown session: {session_id}"))),
    }
}
