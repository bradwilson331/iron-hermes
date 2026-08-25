//! `/v1/runs*` — Tasks 2 and 3 of Phase 36.7.1 Plan 06.
//!
//! Projects the process-wide `TurnRegistry` (Phase 39.1) rather than inventing a
//! second notion of an in-flight run: submitting here registers a `TurnEntry` under
//! `Surface::ApiServer` (register BEFORE spawn, per the registry's own doc comment),
//! and every other route in this family is a read or an action against that SAME
//! registry entry — status and the SSE stream project it, stop cancels its token,
//! approval translates onto `ironhermes_core::ApprovalGate::resolve`.
//!
//! The run identifier IS the turn's session identifier (both are the same minted
//! UUID string) — no separate lookup table maps one to the other.

use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{KeepAlive, Sse};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use ironhermes_agent::client::StreamEvent;
use ironhermes_core::concurrency::{Surface, TurnEntry, TurnId};

use super::super::ApiServerAdapter;
use super::super::sse::build_stream;

pub const RUN_PATHS: &[&str] = &[
    "/v1/runs",
    "/v1/runs/{run_id}",
    "/v1/runs/{run_id}/events",
    "/v1/runs/{run_id}/approval",
    "/v1/runs/{run_id}/stop",
];

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
pub struct RunSubmitRequest {
    /// The prompt to run. This plan wires the registry/route mechanics, not a real
    /// agent turn (that is `ironhermes_agent::AgentLoop`, not reachable from
    /// `ironhermes-restgw` via `ApiServerHandles` yet — a later plan's concern). A
    /// production submission gets a minimal, deterministic echo turn so the
    /// register-before-spawn / observable-status / cancellable / SSE-observable
    /// contract is real and exercised end-to-end through this one HTTP entry point,
    /// not only through directly-registered stub turns in tests.
    #[serde(default)]
    pub prompt: String,
}

#[derive(Debug, Serialize)]
pub struct RunSubmitResponse {
    pub run_id: String,
}

#[derive(Debug, Serialize)]
pub struct RunStatusResponse {
    pub run_id: String,
    /// `"in_flight"` or `"completed"`. A run that never existed is reported as
    /// `404 Not Found` (no body carrying this type at all) — the two are never
    /// collapsed into the same shape (T-36.7.1-49).
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u128>,
}

#[derive(Debug, Deserialize)]
pub struct ApprovalRequestBody {
    pub approved: bool,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/runs` — mint a turn identifier, register it under `Surface::ApiServer`
/// BEFORE spawning the turn task (register-before-spawn — a client that submits and
/// immediately polls is the normal REST pattern; registering after the spawn would
/// create a window where a legitimate identifier reports not-found), then spawn.
async fn submit(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Json(req): Json<RunSubmitRequest>,
) -> impl IntoResponse {
    let turn_id: TurnId = Uuid::new_v4();
    // The run identifier IS the turn's session identifier — the key
    // ApprovalGate::resolve already indexes on. No separate mapping table.
    let session_id = turn_id.to_string();
    let cancel = CancellationToken::new();
    let entry = TurnEntry {
        turn_id,
        session_id: session_id.clone(),
        surface: Surface::ApiServer,
        started_at: Instant::now(),
        cancel: cancel.clone(),
    };

    let handles = adapter.handles().clone();
    handles.turn_registry.register(entry).await; // MUST precede spawn.
    let tx = handles.run_events.register(turn_id).await;

    let registry = handles.turn_registry.clone();
    let events = handles.run_events.clone();
    let prompt = req.prompt;
    tokio::spawn(async move {
        run_stub_turn(prompt, tx, cancel).await;
        events.mark_completed(turn_id).await;
        registry.deregister(turn_id).await;
    });

    (
        StatusCode::OK,
        Json(RunSubmitResponse { run_id: session_id }),
    )
}

/// The default production turn body: a bounded, deterministic "simulated work"
/// window (long enough that a client which submits and immediately polls observes
/// the run in flight) followed by an echo of the prompt and a `Done` marker.
/// Watches `cancel` throughout — a `/stop` mid-window exits without emitting `Done`;
/// the SSE stream still closes because `tx` is dropped on return either way.
async fn run_stub_turn(
    prompt: String,
    tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
    cancel: CancellationToken,
) {
    tokio::select! {
        _ = cancel.cancelled() => return,
        _ = tokio::time::sleep(std::time::Duration::from_millis(150)) => {}
    }
    if !prompt.is_empty() {
        let _ = tx.send(StreamEvent::ContentDelta(prompt));
    }
    let _ = tx.send(StreamEvent::Done(None));
}

/// `GET /v1/runs/{run_id}` — project the registry's by-identifier lookup. Reports
/// `in_flight` while the lookup finds an entry, `completed` for a run this adapter
/// saw finish, and `404` for an identifier that was never registered — three honest
/// states, never collapsed (T-36.7.1-49).
async fn status(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let Ok(turn_id) = Uuid::parse_str(&run_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let handles = adapter.handles();
    if let Some(summary) = handles.turn_registry.get_one(turn_id).await {
        return Json(RunStatusResponse {
            run_id,
            status: "in_flight",
            surface: Some(summary.surface.to_string()),
            elapsed_ms: Some(summary.elapsed_ms),
        })
        .into_response();
    }
    if handles.run_events.is_completed(turn_id).await {
        return Json(RunStatusResponse {
            run_id,
            status: "completed",
            surface: None,
            elapsed_ms: None,
        })
        .into_response();
    }
    StatusCode::NOT_FOUND.into_response()
}

/// `POST /v1/runs/{run_id}/stop` — cancel-by-identifier. Not-found (rather than a
/// vacuous success) when the identifier was never registered, so a client learns its
/// identifier was wrong instead of believing it cancelled something (T-36.7.1-49).
async fn stop(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(run_id): Path<String>,
) -> impl IntoResponse {
    let Ok(turn_id) = Uuid::parse_str(&run_id) else {
        return StatusCode::NOT_FOUND;
    };
    if adapter.handles().turn_registry.cancel_one(turn_id).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// `POST /v1/runs/{run_id}/approval` — translates onto
/// `ironhermes_core::ApprovalGate::resolve`, but ONLY for a turn this same lookup
/// reports as REST-originated (T-36.7.1-48/D-08): the registry lookup and surface
/// comparison happen BEFORE any call to `resolve`, and a turn registered under any
/// other surface — or an unregistered identifier — is refused with not-found without
/// ever reaching the gate. Reads no role, scope, permission or caller-identity field;
/// `body.approved` is the only input beyond the path identifier (D-08 — a finer
/// authorisation model is out of scope for this surface, T-36.7.1-48R).
async fn approval(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(run_id): Path<String>,
    Json(body): Json<ApprovalRequestBody>,
) -> impl IntoResponse {
    let Ok(turn_id) = Uuid::parse_str(&run_id) else {
        return StatusCode::NOT_FOUND;
    };
    let handles = adapter.handles();
    let Some(summary) = handles.turn_registry.get_one(turn_id).await else {
        return StatusCode::NOT_FOUND;
    };
    if summary.surface != Surface::ApiServer {
        // T-36.7.1-48: a turn parked by another surface is unreachable from here.
        return StatusCode::NOT_FOUND;
    }
    // T-36.7.1-48 (security audit AR-02): authorise on the SAME key we act on.
    // The surface check above is necessary but not sufficient — `submit` is not
    // the only producer of `Surface::ApiServer` entries. `sessions::chat_stream`
    // registers one whose `session_id` is the caller-supplied path parameter,
    // so any store session — including one owned by Telegram, the TUI, the web
    // UI or the CLI — can be named by a REST caller and would otherwise be the
    // value handed to `gate.resolve` below.
    //
    // `submit` mints `session_id = turn_id.to_string()` (see its doc comment:
    // "the run identifier IS the turn's session identifier"). Requiring that
    // equality here confines this route to runs THIS route created. Compared
    // against the canonical `turn_id`, not the raw `run_id` path parameter,
    // because `Uuid::parse_str` also accepts unhyphenated, braced and URN
    // forms — comparing the raw string would 404 a legitimate run submitted in
    // any of them.
    if summary.session_id != turn_id.to_string() {
        return StatusCode::NOT_FOUND;
    }
    let Some(gate) = handles.approval_gate.as_ref() else {
        // No approval mechanism wired for this adapter instance — nothing to
        // resolve against. Fail closed (not-found), never silently succeed.
        return StatusCode::NOT_FOUND;
    };
    if gate.resolve(&summary.session_id, body.approved).await {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

/// `GET /v1/runs/{run_id}/events` — SSE bridge from the run's `StreamEvent` channel.
/// Rejects an unregistered identifier with not-found BEFORE opening the stream (an
/// empty stream that never produces anything is indistinguishable from a slow run,
/// and a client would wait on it indefinitely). Terminates on the done marker, a
/// provider error (forwarded first), or cancellation — see [`super::super::sse::build_stream`].
async fn events(
    State(adapter): State<Arc<ApiServerAdapter>>,
    Path(run_id): Path<String>,
) -> Result<impl IntoResponse, StatusCode> {
    let turn_id = Uuid::parse_str(&run_id).map_err(|_| StatusCode::NOT_FOUND)?;
    let handles = adapter.handles();
    if handles.turn_registry.get_one(turn_id).await.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let rx = handles
        .run_events
        .take(turn_id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let cancel = handles
        .turn_registry
        .get_cancel_token(turn_id)
        .await
        .unwrap_or_else(CancellationToken::new);

    Ok(Sse::new(build_stream(rx, cancel)).keep_alive(KeepAlive::default()))
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/v1/runs", post(submit))
        .route("/v1/runs/{run_id}", get(status))
        .route("/v1/runs/{run_id}/events", get(events))
        .route("/v1/runs/{run_id}/approval", post(approval))
        .route("/v1/runs/{run_id}/stop", post(stop))
}
