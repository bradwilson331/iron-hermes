//! `/api/sessions*` — session CRUD, messages, fork, model lock, and chat.
//! Phase 36.7.1 Plan 07.
//!
//! Projects the existing `ironhermes_state::StateStore` (Phase 25.x) rather than
//! inventing a second session store — every route in this family reads or writes
//! through the ONE store handle `ApiServerHandles` was constructed with (D-08). No
//! route accepts a store selector, a profile parameter or a database path; there is
//! exactly one store in this process, supplied at adapter construction.
//!
//! Every per-session verb returns not-found for an identifier that does not exist and
//! never creates one implicitly — a client's typo must never be indistinguishable from
//! a real session.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{KeepAlive, Sse};
use axum::routing::{get, patch, post};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use ironhermes_agent::client::StreamEvent;
use ironhermes_core::ChatMessage;
use ironhermes_core::concurrency::{Surface, TurnEntry};
use ironhermes_state::{Session, StateStore, StoredMessage};

use super::super::ApiServerAdapter;
use super::super::sse::build_stream;

pub const SESSION_PATHS: &[&str] = &[
    "/api/sessions",
    "/api/sessions/{id}",
    "/api/sessions/{id}/messages",
    "/api/sessions/{id}/fork",
    "/api/sessions/{id}/model",
    "/api/sessions/{id}/chat",
    "/api/sessions/{id}/chat/stream",
];

/// The `source` value stamped on every session this route family creates —
/// mirrors `ironhermes_core::concurrency::Surface::ApiServer`'s own `Display`
/// rendering ("api_server"), so a session's `source` column tells an operator
/// which surface started it, same as every other platform's sessions do.
const SESSION_SOURCE: &str = "api_server";

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RetitleRequest {
    pub title: String,
}

#[derive(Debug, Deserialize)]
pub struct ModelLockRequest {
    pub model: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub source: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct MessagesResponse {
    pub messages: Vec<StoredMessage>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ChatRequest {
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub reply: String,
}

/// A prompt equal to this sentinel makes the deterministic stub turn body
/// ([`run_chat_stub_turn`]) emit a [`StreamEvent::ProviderError`] instead of an echo.
/// This route's turn body is fully deterministic and prompt-driven — mirrors
/// `routes::runs::run_stub_turn`, which is not a real `AgentLoop` invocation either
/// (that wiring is a later plan's concern) — and the provider-error-then-terminate
/// path needs a reachable trigger to exercise the real HTTP endpoint end-to-end.
const SIMULATE_PROVIDER_ERROR_PROMPT: &str = "__simulate_provider_error__";

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Look up a single session, mapping a genuine store error to `500` distinctly from a
/// missing identifier (`Ok(None)`) — a caller matches on this to tell "not found" apart
/// from "the store failed" rather than collapsing both into one response.
fn load_session(store: &StateStore, id: &str) -> Result<Option<Session>, StatusCode> {
    store.get_session(id).map_err(|e| {
        tracing::error!(error = %e, session_id = id, "state store get_session failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// Resolve a session for a route that will RUN A TURN against it and persist
/// the result, refusing any session this surface does not own (security audit
/// N-04).
///
/// `ApiServerHandles.state_store` is the SAME store the gateway persists
/// Telegram, Discord, Slack and Buzz conversations into, and the resume path
/// replays persisted rows back as model context. Without this check, `chat`
/// and `chat_stream` accepted any id `get_session` resolved and appended an
/// assistant message to it — so a REST caller could write fabricated assistant
/// turns into a live conversation belonging to another surface, where they
/// would later be replayed to the model as if the agent had said them.
///
/// Read and management verbs deliberately do NOT use this. D-08 makes the
/// surface single-tenant — one key, one profile, one operator — so inspecting
/// or administering another surface's session is a legitimate operator action.
/// Authoring conversation content on another surface's behalf is not, which is
/// why the gate sits on exactly the two routes that do it.
fn load_own_session_for_turn(store: &StateStore, id: &str) -> Result<Session, StatusCode> {
    match load_session(store, id)? {
        Some(session) if session.source == SESSION_SOURCE => Ok(session),
        Some(session) => {
            tracing::warn!(
                session_id = %id,
                source = %session.source,
                "refused a REST turn against a session owned by another surface — \
                 persisting a reply here would inject fabricated assistant content \
                 into that surface's conversation history (N-04)"
            );
            Err(StatusCode::NOT_FOUND)
        }
        None => Err(StatusCode::NOT_FOUND),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/sessions` — list sessions through the store's own filtered listing call.
async fn list(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let store = handles.state_store.lock().unwrap();
    match store.list_sessions_filtered(q.source.as_deref(), q.limit.unwrap_or(100), None) {
        Ok(sessions) => Json(serde_json::json!({ "sessions": sessions })).into_response(),
        Err(e) => {
            tracing::error!(error = %e, "list_sessions_filtered failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sessions` — create a session with no parent, since an unforked session
/// has none ([`fork`] is the only route that supplies one).
async fn create(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let id = uuid::Uuid::new_v4().to_string();
    let mut store = handles.state_store.lock().unwrap();
    if let Err(e) = store.create_session(
        &id,
        SESSION_SOURCE,
        req.model.as_deref(),
        req.system_prompt.as_deref(),
        None,
        None,
    ) {
        tracing::error!(error = %e, "create_session failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match load_session(&store, &id) {
        Ok(Some(session)) => (StatusCode::OK, Json(session)).into_response(),
        Ok(None) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(status) => status.into_response(),
    }
}

/// `GET /api/sessions/{id}` — fetch a single session, projecting its real fields.
async fn fetch(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let store = handles.state_store.lock().unwrap();
    match load_session(&store, &id) {
        Ok(Some(session)) => Json(session).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(status) => status.into_response(),
    }
}

/// `PATCH /api/sessions/{id}` — retitle. Not-found for an unknown identifier; never
/// creates the session as a side effect of patching it.
async fn retitle(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
    Json(req): Json<RetitleRequest>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let mut store = handles.state_store.lock().unwrap();
    match load_session(&store, &id) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(status) => return status.into_response(),
    }
    if let Err(e) = store.update_session_title(&id, &req.title) {
        tracing::error!(error = %e, session_id = %id, "update_session_title failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match load_session(&store, &id) {
        Ok(Some(session)) => Json(session).into_response(),
        Ok(None) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(status) => status.into_response(),
    }
}

/// `DELETE /api/sessions/{id}` — remove the session and its messages
/// ([`ironhermes_state::StateStore::delete_session`]). Not-found (not a vacuous
/// success) when the identifier never existed.
async fn delete_one(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let mut store = handles.state_store.lock().unwrap();
    match store.delete_session(&id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => {
            tracing::error!(error = %e, session_id = %id, "delete_session failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/sessions/{id}/messages` — the persisted messages, in insertion order. An
/// empty collection (not an error) for a session with none.
async fn messages(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let store = handles.state_store.lock().unwrap();
    match load_session(&store, &id) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(status) => return status.into_response(),
    }
    match store.get_messages(&id) {
        Ok(messages) => Json(MessagesResponse { messages }).into_response(),
        Err(e) => {
            tracing::error!(error = %e, session_id = %id, "get_messages failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/sessions/{id}/fork` — a store composition, not a new persistence concept:
/// read the source session, create a new session naming it as parent, and copy the
/// source's messages into the new session. Leaves the original completely untouched —
/// a fork that mutates its source is a move, not a branch.
async fn fork(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let mut store = handles.state_store.lock().unwrap();
    let original = match load_session(&store, &id) {
        Ok(Some(s)) => s,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(status) => return status.into_response(),
    };
    let source_messages = match store.get_messages(&id) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, session_id = %id, "get_messages failed during fork");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let new_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = store.create_session(
        &new_id,
        &original.source,
        original.model.as_deref(),
        original.system_prompt.as_deref(),
        Some(&id),
        original.workspace_root.as_deref(),
    ) {
        tracing::error!(error = %e, parent = %id, "create_session failed during fork");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    for stored in &source_messages {
        let Some(chat_msg) = ironhermes_state::chat_message_from_stored(stored) else {
            continue;
        };
        if let Err(e) = store.add_message(&new_id, &chat_msg) {
            tracing::error!(error = %e, fork_id = %new_id, "add_message failed during fork copy");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    match load_session(&store, &new_id) {
        Ok(Some(session)) => (StatusCode::OK, Json(session)).into_response(),
        Ok(None) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(status) => status.into_response(),
    }
}

/// `PATCH /api/sessions/{id}/model` — lock a session to a model, but only after the
/// registry confirms it exists. Storing an unvalidated model name means the failure
/// surfaces on the next turn, far from where the mistake was made.
async fn model_lock(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
    Json(req): Json<ModelLockRequest>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    let mut store = handles.state_store.lock().unwrap();
    match load_session(&store, &id) {
        Ok(Some(_)) => {}
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(status) => return status.into_response(),
    }
    if handles.model_registry.lookup(&req.model).is_none() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }
    if let Err(e) = store.update_session_model(&id, &req.model) {
        tracing::error!(error = %e, session_id = %id, "update_session_model failed");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    match load_session(&store, &id) {
        Ok(Some(session)) => Json(session).into_response(),
        Ok(None) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        Err(status) => status.into_response(),
    }
}

/// The deterministic stub turn body for session-scoped chat (single-shot and
/// streamed) — mirrors `routes::runs::run_stub_turn`'s own "not a real `AgentLoop`
/// invocation" stub. Echoes the prompt back word-by-word, so a streamed reply
/// produces multiple ordered [`StreamEvent::ContentDelta`] frames rather than one
/// opaque blob. Returns the assembled reply text on normal completion (`Some`), or
/// `None` on cancellation or a simulated provider error — neither has anything to
/// persist.
async fn run_chat_stub_turn(
    prompt: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Option<String> {
    tokio::select! {
        _ = cancel.cancelled() => return None,
        _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {}
    }
    if prompt == SIMULATE_PROVIDER_ERROR_PROMPT {
        let _ = tx.send(StreamEvent::ProviderError(
            "simulated provider error".to_string(),
        ));
        return None;
    }
    let mut reply = String::new();
    for word in prompt.split_whitespace() {
        if !reply.is_empty() {
            reply.push(' ');
        }
        reply.push_str(word);
        let _ = tx.send(StreamEvent::ContentDelta(word.to_string()));
    }
    let _ = tx.send(StreamEvent::Done(None));
    Some(reply)
}

/// `POST /api/sessions/{id}/chat` — a single-shot turn bound to the named session.
/// Both the prompt and the reply are persisted through the store so the messages
/// endpoint reflects the exchange. Not-found (running no turn) for an unknown
/// session — a typo must never look like a real conversation.
async fn chat(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> impl IntoResponse {
    let handles = adapter.handles();
    {
        let store = handles.state_store.lock().unwrap();
        if let Err(status) = load_own_session_for_turn(&store, &id) {
            return status.into_response();
        }
    }

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let reply = run_chat_stub_turn(&req.prompt, &tx, &cancel).await;
    drop(tx);

    // WR-06: the prompt is persisted only once the turn has produced a reply
    // to pair it with. Persisting BEFORE the turn left the user message
    // orphaned on every `None` return — the `SIMULATE_PROVIDER_ERROR_PROMPT`
    // path and cancellation — so the session kept a trailing user turn with
    // no assistant turn. The gateway's resume path replays persisted rows as
    // model context, and a client that got an error reasonably retries,
    // appending a second copy of the same prompt; repeated failures
    // accumulate duplicate user turns that then travel into every subsequent
    // turn's context.
    //
    // This is the same reasoning `ecbfc78` applied to the ownership gate ("a
    // check placed after it would leave a stray user message in the foreign
    // session even on the refusal path") — it simply was not applied to the
    // provider-error path in this same function.
    let Some(reply_text) = reply else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    // Both messages under ONE lock acquisition, so no concurrent reader can
    // observe the user turn without its reply.
    {
        let mut store = handles.state_store.lock().unwrap();
        if let Err(e) = store.add_message(&id, &ChatMessage::user(req.prompt.clone())) {
            tracing::error!(error = %e, session_id = %id, "failed to persist chat prompt");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        if let Err(e) = store.add_message(&id, &ChatMessage::assistant(reply_text.clone())) {
            tracing::error!(error = %e, session_id = %id, "failed to persist chat reply");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    (
        StatusCode::OK,
        Json(ChatResponse {
            session_id: id,
            reply: reply_text,
        }),
    )
        .into_response()
}

/// `POST /api/sessions/{id}/chat/stream` — the streamed twin of [`chat`], over the
/// SAME SSE bridge the runs family uses ([`build_stream`], Plan 06) — no second frame
/// type. Registers a `TurnEntry` under `Surface::ApiServer` BEFORE spawning (same
/// register-before-spawn discipline `routes::runs::submit` follows), so the turn is
/// visible to `GET /v1/runs/{id}` and stoppable by `POST /v1/runs/{id}/stop` like any
/// other run — a streamed chat that no cross-surface view can see would make the
/// running-agent guard under-report.
async fn chat_stream(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(id): Path<String>,
    Json(req): Json<ChatRequest>,
) -> Result<impl IntoResponse, StatusCode> {
    let handles = adapter.handles();
    {
        let store = handles.state_store.lock().unwrap();
        load_own_session_for_turn(&store, &id)?;
    }

    let turn_id = uuid::Uuid::new_v4();
    let cancel = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: id.clone(),
        surface: Surface::ApiServer,
        started_at: Instant::now(),
        cancel: cancel.clone(),
    };
    handles.turn_registry.register(entry).await; // MUST precede spawn.

    let (tx, rx) = mpsc::unbounded_channel();
    let store = handles.state_store.clone();
    let registry = handles.turn_registry.clone();
    let session_id = id.clone();
    let prompt = req.prompt.clone();
    let spawn_cancel = cancel.clone();
    tokio::spawn(async move {
        let reply = run_chat_stub_turn(&prompt, &tx, &spawn_cancel).await;
        // WR-06: the prompt is persisted here, WITH its reply, rather than in
        // the handler before the spawn. Persisting up front left the user
        // message orphaned whenever this task skipped the assistant write —
        // the `SIMULATE_PROVIDER_ERROR_PROMPT` path and cancellation. On the
        // streaming route that is the worse half of the defect: the client
        // received a `ProviderError` frame and reasonably retries, appending
        // a second copy of the same prompt, so repeated failures accumulate
        // duplicate user turns with no replies — all of which the gateway's
        // resume path then replays as model context.
        //
        // The ownership gate (`load_own_session_for_turn`) still runs in the
        // handler BEFORE this spawn, so a refused session is never written to
        // at all — moving the write later only shrinks that exposure.
        if let Some(reply_text) = reply {
            // Both messages under ONE lock acquisition, in order, so a
            // concurrent `GET /messages` never observes the user turn without
            // its reply.
            if let Ok(mut store) = store.lock() {
                let _ = store.add_message(&session_id, &ChatMessage::user(prompt.clone()));
                let _ = store.add_message(&session_id, &ChatMessage::assistant(reply_text));
            }
        }
        registry.deregister(turn_id).await;
    });

    Ok(Sse::new(build_stream(rx, cancel)).keep_alive(KeepAlive::default()))
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/api/sessions", get(list).post(create))
        .route(
            "/api/sessions/{id}",
            get(fetch).patch(retitle).delete(delete_one),
        )
        .route("/api/sessions/{id}/messages", get(messages))
        .route("/api/sessions/{id}/fork", post(fork))
        .route("/api/sessions/{id}/model", patch(model_lock))
        .route("/api/sessions/{id}/chat", post(chat))
        .route("/api/sessions/{id}/chat/stream", post(chat_stream))
}
