//! `POST /v1/chat/completions` — the OpenAI-compatible chat completion route,
//! streaming and non-streaming. Phase 36.7.1 Plan 08 Task 1.
//!
//! # STUB TURN NOTICE — read before assuming this calls a real model
//!
//! [`run_chat_completion_stub_turn`] below is NOT a real `ironhermes_agent::AgentLoop`
//! invocation. `ApiServerHandles` still carries no `AgentLoop`/`AgentRuntime` handle
//! (unchanged since Plan 06 established this exact stub pattern for `POST /v1/runs`,
//! and Plan 07 repeated it for `/api/sessions/{id}/chat`). The turn body deterministically
//! echoes the caller's messages back — real wire shapes, real streaming mechanics, real
//! model-registry gating, but a fabricated answer, not a model-produced one. This is a
//! deliberate, user-approved scope decision (see this plan's `<phase_decisions>`): a
//! single future plan threads a real agent handle into all three stub call sites
//! (`/v1/runs`, `/api/sessions/{id}/chat[/stream]`, `/v1/chat/completions`) at once.
//! Until then, do not read this endpoint's responses as model output.
//!
//! # Wire shapes
//!
//! Request/response types ([`ChatCompletionRequest`], [`ChatCompletionResponse`],
//! [`ChatCompletionChunk`]) mirror the OpenAI chat completions request/response/
//! streaming-chunk shapes a third-party client already speaks. The request's
//! `messages` field reuses [`ironhermes_core::ChatMessage`] directly (role +
//! content, already OpenAI-shaped) rather than a second message type — the whole
//! list is passed through to the turn body unmodified (a client's system/user/
//! assistant history is never truncated to the last message).
//!
//! # Streaming derivation (API-11)
//!
//! The streaming form derives every chunk from [`super::super::sse::SseFrame`] —
//! the ONE canonical mapping this crate has from [`ironhermes_agent::client::StreamEvent`]
//! (`api_server/sse.rs`'s own module doc). `SseFrame::from` is called on every event
//! this route's stub turn produces; this file defines no second `StreamEvent`-shaped
//! vocabulary and imports neither the terminal UI's nor the web UI's same-named
//! stream event enum. Unlike `routes::runs`/`routes::sessions`, this route does NOT
//! reuse `sse::build_stream` verbatim — that function commits to emitting the
//! internal `SseFrame` JSON shape (`{"type":"content_delta",...}`) on the wire, which
//! is the correct shape for `/v1/runs/{id}/events` and `/api/sessions/{id}/chat/stream`
//! but not for an OpenAI-compatible client, which expects
//! `{"choices":[{"delta":{"content":...}},...]}` chunks terminated by a literal
//! `data: [DONE]` line. This route re-presents the SAME canonical `SseFrame` values
//! (obtained via the shared bridge) as that different, standard wire shape, using the
//! identical termination triggers `build_stream` uses (done marker / provider error /
//! cancellation) so the two are provably in step.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_util::sync::CancellationToken;

use ironhermes_agent::client::StreamEvent;
use ironhermes_core::{ChatMessage, Usage};
use ironhermes_core::concurrency::{Surface, TurnEntry};

use super::super::ApiServerAdapter;
use super::super::sse::SseFrame;

pub const CHAT_PATHS: &[&str] = &["/v1/chat/completions"];

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChatCompletionChoice>,
    pub usage: Usage,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionChoice {
    pub index: usize,
    pub message: ChatMessage,
    pub finish_reason: String,
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunk {
    id: String,
    object: &'static str,
    created: u64,
    model: String,
    choices: Vec<ChatCompletionChunkChoice>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunkChoice {
    index: usize,
    delta: ChatCompletionChunkDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatCompletionChunkDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Join every message's text content, in order, into one prompt string for the
/// deterministic stub turn body. A real `AgentLoop` invocation would instead pass
/// the whole `messages` list through untouched (the plan's own `<action>` text) —
/// this concatenation exists only so the stub turn has *something* observable and
/// order-preserving to echo, proving the full history (not just the last message)
/// reached the turn.
fn join_message_texts(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter_map(|m| m.content.as_ref().and_then(|c| c.as_text()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The deterministic stub turn body — see the module-level STUB TURN NOTICE.
/// Mirrors `routes::runs::run_stub_turn` / `routes::sessions::run_chat_stub_turn`:
/// echoes the joined prompt back word-by-word (multiple ordered `ContentDelta`
/// frames), watches `cancel` throughout, and returns the assembled reply text on
/// normal completion (`Some`) or `None` on cancellation.
async fn run_chat_completion_stub_turn(
    prompt: &str,
    tx: &mpsc::UnboundedSender<StreamEvent>,
    cancel: &CancellationToken,
) -> Option<String> {
    tokio::select! {
        _ = cancel.cancelled() => return None,
        _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {}
    }
    let mut reply = String::new();
    for word in prompt.split_whitespace() {
        if !reply.is_empty() {
            reply.push(' ');
        }
        reply.push_str(word);
        let _ = tx.send(StreamEvent::ContentDelta(word.to_string()));
    }
    let _ = tx.send(StreamEvent::Done(Some("stop".to_string())));
    Some(reply)
}

fn chunk(id: &str, model: &str, content: Option<String>, finish_reason: Option<String>) -> ChatCompletionChunk {
    ChatCompletionChunk {
        id: id.to_string(),
        object: "chat.completion.chunk",
        created: now_unix(),
        model: model.to_string(),
        choices: vec![ChatCompletionChunkChoice {
            index: 0,
            delta: ChatCompletionChunkDelta { content },
            finish_reason,
        }],
    }
}

fn chunk_to_event(c: &ChatCompletionChunk) -> Event {
    match serde_json::to_string(c) {
        Ok(body) => Event::default().data(body),
        Err(_) => Event::default().data("{}"),
    }
}

/// Build the OpenAI-shaped SSE chunk stream for one turn's event channel.
///
/// Derives every chunk from [`SseFrame`] — the shared bridge's canonical mapping
/// from [`StreamEvent`] — via `SseFrame::from(&event)`, so this file introduces no
/// second `StreamEvent`-shaped vocabulary of its own (API-11). Uses the identical
/// termination triggers `super::super::sse::build_stream` uses: the producer
/// closing its channel (after `Done`/`ProviderError`), or `cancel` firing —
/// whichever happens first. Always ends with a literal `data: [DONE]` line, the
/// sentinel a standard OpenAI-compatible client looks for so it does not hang
/// waiting for more.
fn build_openai_stream(
    id: String,
    model: String,
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
    cancel: CancellationToken,
) -> impl Stream<Item = Result<Event, Infallible>> + Send + 'static {
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Result<Event, Infallible>>();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                maybe_event = rx.recv() => {
                    match maybe_event {
                        Some(event) => match SseFrame::from(&event) {
                            SseFrame::ContentDelta { text } => {
                                let c = chunk(&id, &model, Some(text), None);
                                if out_tx.send(Ok(chunk_to_event(&c))).is_err() {
                                    break; // client disconnected.
                                }
                            }
                            SseFrame::Done { reason } => {
                                let c = chunk(&id, &model, None, Some(reason.unwrap_or_else(|| "stop".to_string())));
                                let _ = out_tx.send(Ok(chunk_to_event(&c)));
                                let _ = out_tx.send(Ok(Event::default().data("[DONE]")));
                                break;
                            }
                            SseFrame::ProviderError { message } => {
                                let _ = out_tx.send(Ok(Event::default().event("error").data(message)));
                                let _ = out_tx.send(Ok(Event::default().data("[DONE]")));
                                break;
                            }
                            // Neither the stub turn body nor a real AgentLoop tool-use/usage
                            // frame changes this route's minimal chunk shape today — forwarded
                            // as a no-op rather than silently dropped from the match.
                            SseFrame::ToolCallDelta { .. } | SseFrame::Usage { .. } => {}
                        },
                        None => break, // producer finished without a Done/ProviderError frame.
                    }
                }
            }
        }
    });
    UnboundedReceiverStream::new(out_rx)
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /v1/chat/completions` — resolves the requested model against the
/// registry (refusing an unknown one with a client error and running no turn —
/// never substituting a default, API-08), then either runs the stub turn
/// synchronously (non-streaming) or registers it in the process-wide
/// `TurnRegistry` under `Surface::ApiServer` before spawning it and streaming the
/// result (streaming — mirrors `routes::sessions::chat_stream`'s
/// register-before-spawn discipline, so a long-lived completion is visible to
/// `GET /v1/runs/{id}` and stoppable by `POST /v1/runs/{id}/stop`). A short-lived,
/// single-shot completion that returns within one request/response cycle has no
/// window an external surface would need to observe or stop it mid-flight the way
/// the streaming form does — the same reasoning Plan 07 documented for session
/// chat vs. session chat-stream.
async fn completions(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Json(req): Json<ChatCompletionRequest>,
) -> axum::response::Response {
    let handles = adapter.handles();
    if handles.model_registry.lookup(&req.model).is_none() {
        return StatusCode::UNPROCESSABLE_ENTITY.into_response();
    }

    let prompt = join_message_texts(&req.messages);
    let id = format!("chatcmpl-{}", uuid::Uuid::new_v4());

    if req.stream {
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
        let registry = handles.turn_registry.clone();
        let spawn_cancel = cancel.clone();
        tokio::spawn(async move {
            run_chat_completion_stub_turn(&prompt, &tx, &spawn_cancel).await;
            registry.deregister(turn_id).await;
        });

        return Sse::new(build_openai_stream(id, req.model, rx, cancel))
            .keep_alive(KeepAlive::default())
            .into_response();
    }

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let reply = run_chat_completion_stub_turn(&prompt, &tx, &cancel).await;
    drop(tx);

    let Some(reply_text) = reply else {
        return StatusCode::BAD_GATEWAY.into_response();
    };

    let prompt_tokens = prompt.split_whitespace().count();
    let completion_tokens = reply_text.split_whitespace().count();
    let body = ChatCompletionResponse {
        id,
        object: "chat.completion".to_string(),
        created: now_unix(),
        model: req.model,
        choices: vec![ChatCompletionChoice {
            index: 0,
            message: ChatMessage::assistant(reply_text),
            finish_reason: "stop".to_string(),
        }],
        usage: Usage {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
            cache_read_input_tokens: None,
            cache_creation_input_tokens: None,
        },
    };
    (StatusCode::OK, Json(body)).into_response()
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new().route("/v1/chat/completions", post(completions))
}
