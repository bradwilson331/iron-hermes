//! Phase 36.17.7 D-02-c: `GET /audio/:uuid` audio replay route.
//!
//! Serves bytes from `$IRONHERMES_HOME/audio_cache/{uuid}.mp3` to support
//! browser page-reload replay of audio blocks produced by `text_to_speech` +
//! `send_audio` (the WebAudioDispatcher's primary push path).
//!
//! T-path-traversal mitigations (HIGH per threat model):
//!   (a) `Uuid::parse_str` on the path segment — rejects anything that isn't
//!       a syntactically valid UUID before any filesystem access.
//!   (b) `path.canonicalize()` — resolves `..` / symlinks to absolute form.
//!   (c) `canon.starts_with(audio_cache_dir)` — guarantees the canonical path
//!       lives under the cache root.
//!
//! Auth: no explicit gate. Mirrors `ws.rs` and `kanban_ws.rs` which run on
//! operator-local origin without auth middleware. UUIDs from server-generated
//! audio are unguessable (122 bits of randomness).
//!
//! Streaming: no — typical mp3 < 1 MB, full-read response is fine.
//! `tokio::fs::read` keeps the route compatible with the axum runtime that
//! Dioxus fullstack mounts.

#[cfg(feature = "server")]
use axum::response::IntoResponse;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use ironhermes_core::constants::get_hermes_home;
#[cfg(feature = "server")]
use uuid::Uuid;

#[get("/audio/:uuid")]
pub async fn serve_audio(uuid: String) -> Result<Vec<u8>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        // (a) UUID format validation — rejects "../etc/passwd" and friends
        // before they ever touch the filesystem.
        if Uuid::parse_str(&uuid).is_err() {
            return Err(ServerFnError::new("invalid uuid"));
        }

        let audio_cache_dir = get_hermes_home().join("audio_cache");
        let candidate = audio_cache_dir.join(format!("{uuid}.mp3"));

        // (b) Canonicalize — resolves any `..` segments + symlinks. If the
        // file doesn't exist this errors; surface as a 404 to the client.
        let canon = match candidate.canonicalize() {
            Ok(p) => p,
            Err(_) => return Err(ServerFnError::new("not found")),
        };

        // (c) Containment guard — confirm the canonical path lives under the
        // cache root. Canonicalizing the cache root too defends against
        // symlinked cache dirs.
        let canon_root = match audio_cache_dir.canonicalize() {
            Ok(p) => p,
            Err(_) => return Err(ServerFnError::new("not found")),
        };
        if !canon.starts_with(&canon_root) {
            return Err(ServerFnError::new("forbidden"));
        }

        // Full read — files are < 1 MB.
        match tokio::fs::read(&canon).await {
            Ok(bytes) => Ok(bytes),
            Err(_) => Err(ServerFnError::new("not found")),
        }
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = uuid;
        Err(ServerFnError::new("server feature required"))
    }
}

/// Phase 36.17.7 D-02-c: lower-level axum handler used by the server router
/// to stream `audio/mpeg` bytes directly. The `#[get]` server function above
/// returns `Vec<u8>` (Dioxus fullstack contract); this handler is the raw
/// axum response form used when the route is mounted on the axum router.
///
/// Same three-layer T-path-traversal mitigations: Uuid::parse_str +
/// canonicalize + starts_with on `$IRONHERMES_HOME/audio_cache/`.
#[cfg(feature = "server")]
#[allow(dead_code)] // audio route; no Axum router entry wired yet (TTS phase pending)
pub async fn serve_audio_axum(
    axum::extract::Path(uuid): axum::extract::Path<String>,
) -> axum::response::Response {
    // (a) UUID format validation.
    if Uuid::parse_str(&uuid).is_err() {
        return (axum::http::StatusCode::BAD_REQUEST, "invalid uuid").into_response();
    }

    let audio_cache_dir = get_hermes_home().join("audio_cache");
    let candidate = audio_cache_dir.join(format!("{uuid}.mp3"));

    // (b) Canonicalize candidate.
    let canon = match candidate.canonicalize() {
        Ok(p) => p,
        Err(_) => return (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    };

    // (c) Canonicalize cache root and check containment.
    let canon_root = match audio_cache_dir.canonicalize() {
        Ok(p) => p,
        Err(_) => return (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    };
    if !canon.starts_with(&canon_root) {
        return (axum::http::StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    match tokio::fs::read(&canon).await {
        Ok(bytes) => ([(axum::http::header::CONTENT_TYPE, "audio/mpeg")], bytes).into_response(),
        Err(_) => (axum::http::StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
