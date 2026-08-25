//! `POST /v1/responses`, `GET /v1/responses/{id}`, `DELETE /v1/responses/{id}` —
//! the three stateful Responses API routes. Phase 36.7.1 Plan 08 Task 2.
//!
//! # Why mounted, not omitted (D-03, O-01)
//!
//! An unmounted path returns a routing 404 that a client cannot distinguish from
//! a mistyped URL. These three routes ARE mounted — inheriting the router-wide
//! bearer layer like every other family, so the unauthenticated case returns 401
//! rather than 404 (`responses_routes_are_mounted_not_absent` proves this: 401,
//! not 404, is what proves the route exists) — and each one returns an explicit
//! `501 Not Implemented` naming the alternatives a client should use instead. That
//! is the honest form of a gap: real, deliberately unavailable, and pointing
//! somewhere useful, rather than a silent absence a caller has to guess about.
//!
//! # Why there is no backing store (source-fact #4/#5, confirmed first-hand)
//!
//! The only existing work in this repository on the OpenAI Responses API wire
//! format is the OUTBOUND direction: `ironhermes-agent/src/codex_client.rs`
//! implements this repository as a CLIENT of that API surface against one
//! provider (`merge`). Its own module doc records, in its own words, "There is NO
//! `instructions`, NO `store`" on the request this repository sends — server-side
//! conversation state is explicitly disabled on the one caller this repository
//! already has, and no chained `previous_response_id` is ever threaded through.
//! There is therefore zero precedent in this codebase for hosting the INBOUND
//! stateful surface: no table keyed by response identifier, no lifecycle for what
//! a later response "chained" onto a deleted one would mean, no query that walks
//! a chain. The session store (`ironhermes_state::StateStore`, backing
//! `routes::sessions`) persists sessions and messages, not response objects with
//! chaining — a structurally different shape.
//!
//! Recorded here (not rediscovered by a later phase) is what a real
//! implementation would need:
//! 1. A table keyed by response identifier, carrying: session, creation time,
//!    model, input, output, status, and the previous-response identifier.
//! 2. A defined lifecycle for deletion — specifically what happens when a later
//!    response chains off one that has been deleted (refuse the delete? orphan
//!    the chain? tombstone and keep serving reads?).
//! 3. A chain-resolution query — given a response id, walk `previous_response_id`
//!    back to reconstruct the conversation the way `/v1/responses` clients expect.
//!
//! That is its own slice of work, not a fraction of this one.
//!
//! # Agreement with `/v1/capabilities` (T-36.7.1-69)
//!
//! [`super::meta::capabilities`] already reports `features.responses_api: false` —
//! set when that route was written, specifically anticipating this gap (see its
//! own module doc). This file changes nothing there; `capabilities_and_responses_routes_agree`
//! asserts the two facts (the map says unavailable, the routes refuse) can never
//! silently drift apart. Plan 09 generalises this into a router-wide drift test.
//!
//! None of the three handlers below invokes the agent, the turn registry, or any
//! store — a client retrying against any of them costs nothing.

use std::sync::Arc;

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};

use super::super::ApiServerAdapter;

pub const RESPONSES_PATHS: &[&str] = &["/v1/responses", "/v1/responses/{id}"];

/// The supported alternatives named in every not-implemented body — stateless
/// turns via chat completions, or persisted-history conversations via the
/// session chat routes. Both are real, both are mounted, neither requires the
/// backing store this surface would need.
fn not_implemented_body(action: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "message": format!(
                "The stateful Responses API ({action}) is not implemented on this server — no \
                 backing store exists for server-side conversation state. Use POST \
                 /v1/chat/completions for a stateless turn, or POST /api/sessions/{{id}}/chat \
                 (or /chat/stream) for a conversation with persisted history."
            ),
            "type": "not_implemented",
        },
        "alternatives": [
            "POST /v1/chat/completions",
            "POST /api/sessions/{id}/chat",
            "POST /api/sessions/{id}/chat/stream",
        ],
    })
}

/// `POST /v1/responses` — create. Not implemented; runs no turn.
async fn create() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(not_implemented_body("create")),
    )
}

/// `GET /v1/responses/{id}` — fetch. Not implemented; reads nothing, the `{id}`
/// path parameter is accepted only so the route exists at the shape a client
/// expects, never resolved against anything.
async fn fetch(Path(_id): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(not_implemented_body("fetch")),
    )
}

/// `DELETE /v1/responses/{id}` — delete. Not implemented; deletes nothing.
async fn delete_one(Path(_id): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(not_implemented_body("delete")),
    )
}

pub fn router() -> axum::Router<Arc<ApiServerAdapter>> {
    axum::Router::new()
        .route("/v1/responses", post(create))
        .route("/v1/responses/{id}", get(fetch).delete(delete_one))
}
