//! `session/load` JSON-RPC handler (Phase 36.8, plan 02, task 2).
//!
//! RESEARCH Pitfall 4 (protocol version drift risk) is resolved per plan 01's SUMMARY
//! (Key Decisions): the pinned `agent-client-protocol-schema` 1.5.0 still names the live
//! method `session/load` (`LoadSessionRequest::new(session_id, cwd)`), not the v2-draft
//! `session/resume` — confirmed against the crate's own
//! `SESSION_LOAD_METHOD_NAME` constant, not assumed. The bounded head-aligned rehydration
//! algorithm itself (D-11) lives on `session_manager::bounded_head_aligned_rehydrate`;
//! this module is the thin JSON-RPC translation layer over
//! [`crate::session_manager::AcpSessionManager::load`].

use std::sync::Arc;

use agent_client_protocol::schema::v1::{
    ContentBlock, ContentChunk, LoadSessionRequest, LoadSessionResponse, SessionId,
    SessionNotification, SessionUpdate, TextContent,
};
use agent_client_protocol::{Client, ConnectionTo, Responder};
use ironhermes_core::{ChatMessage, Role};
use tokio::sync::Mutex as TokioMutex;

use crate::session_manager::AcpSessionManager;

/// `session/load` — idempotent lookup-or-rehydrate (D-11). An unknown session id
/// produces a JSON-RPC error naming the id (never a silent fresh session under that id
/// — a stale editor session must not quietly lose its history). Any other failure (e.g.
/// building the rehydrated session's `AgentRuntime`) surfaces as an internal error.
///
/// Plan 06 UAT gap fix (found via live Zed UAT, task 3 checkpoint step 12): a successful
/// load MUST replay the rehydrated conversation to the client as `session/update`
/// notifications BEFORE this handler responds — see [`replay_history`]'s doc for why. The
/// original implementation rehydrated `AcpSession.messages` correctly (proven by the
/// plan-02 manager-level tests) but never told the CLIENT what that history was, so a
/// real editor's conversation panel rendered empty despite the server-side state being
/// correct — see `crates/ironhermes-acp/tests/session_load.rs`'s
/// `session_load_replays_user_and_agent_history_before_responding` for the
/// protocol-level regression test this fix makes green.
pub async fn handle_session_load(
    req: LoadSessionRequest,
    responder: Responder<LoadSessionResponse>,
    cx: ConnectionTo<Client>,
    session_manager: Arc<TokioMutex<AcpSessionManager>>,
) -> Result<(), agent_client_protocol::Error> {
    let session_id = req.session_id.to_string();
    let cwd = req.cwd.clone();

    let messages = {
        let mut manager = session_manager.lock().await;
        if let Err(err) = manager.load(&session_id, cwd).await {
            let message = err.to_string();
            return if message.starts_with("unknown session:") {
                responder.respond_with_error(
                    agent_client_protocol::Error::invalid_params()
                        .data(format!("session/load: {message}")),
                )
            } else {
                responder.respond_with_error(
                    agent_client_protocol::Error::internal_error()
                        .data(format!("session/load {session_id}: {message}")),
                )
            };
        }
        manager
            .get(&session_id)
            .map(|session| session.messages.clone())
            .unwrap_or_default()
    };

    replay_history(&cx, req.session_id.clone(), &messages);

    responder.respond(LoadSessionResponse::new())
}

/// Replays a rehydrated session's conversation history to the client as
/// `user_message_chunk`/`agent_message_chunk` `session/update` notifications, called
/// BEFORE [`handle_session_load`] responds.
///
/// The v1 `LoadSessionResponse` carries no history fields at all (confirmed against the
/// pinned `agent-client-protocol-schema` 1.5.0's own struct definition) — replay
/// notifications are the ONLY wire mechanism a v1 ACP client has for learning what a
/// loaded session's prior conversation contains. Skips `Role::System`/`Role::Tool`
/// messages (not user-visible conversation turns), `is_recall_context` ephemeral
/// memory-recall injections (never really "said" by the user or agent — replaying one
/// would leak internal context injection into the visible chat), and any message with no
/// text content (an assistant turn that only carried tool calls, nothing to chunk). Each
/// stored message becomes exactly one non-streamed chunk (this replays already-finished
/// history, not a live turn — there is nothing to stream incrementally).
///
/// `ConnectionTo::send_notification` is synchronous — it writes to the outgoing
/// transport immediately and returns (confirmed via the SDK's own doc comment, and
/// already relied on by `event_bridge.rs`'s callback wiring). Calling it in this plain
/// loop, entirely before `handle_session_load` ever calls `responder.respond(...)`,
/// is sufficient on its own to guarantee wire order: unlike `AcpEventBridge`, this
/// function has no spawned drain task and therefore no cross-task race to fix (the
/// plan-03 "flush-before-respond" bug class does not apply here).
fn replay_history(cx: &ConnectionTo<Client>, session_id: SessionId, messages: &[ChatMessage]) {
    for message in messages {
        if message.is_recall_context {
            continue;
        }
        let update = match message.role {
            Role::User => {
                let Some(text) = message.content_text() else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                SessionUpdate::UserMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text.to_string()),
                )))
            }
            Role::Assistant => {
                let Some(text) = message.content_text() else {
                    continue;
                };
                if text.is_empty() {
                    continue;
                }
                SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
                    TextContent::new(text.to_string()),
                )))
            }
            Role::System | Role::Tool => continue,
        };
        let _ = cx.send_notification(SessionNotification::new(session_id.clone(), update));
    }
}
