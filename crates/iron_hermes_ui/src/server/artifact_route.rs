//! Phase 46.6 (D-02/D-03): `GET /artifacts/{id}` sandboxed artifact serving
//! route.
//!
//! Serves rendered artifact HTML (published via the `artifact` Tool,
//! `ironhermes-tools`) to the browser. Mounted as a **raw axum route** in
//! `main.rs` — NOT via the `#[get]` server-fn inventory. The server-fn codec
//! serializes a `Vec<u8>` return as a JSON number array (`[60,104,...]`), which
//! broke both the iframe viewer (`<iframe src="/artifacts/{id}">`) and direct
//! navigation (Phase 46.6 UAT). A raw handler gives full control over the
//! response body/headers/status.
//!
//! This is the load-bearing D-02 security surface:
//!   - The server is the ONLY trusted origin for the `Content-Security-Policy`
//!     header; agent-authored content must never be able to weaken it. Every
//!     return path (success AND error) goes through `respond`, which sets the
//!     CSP unconditionally (RESEARCH Pitfall 2 — no path may serve a body
//!     without it).
//!   - `Uuid::parse_str(&id)` validates the path segment BEFORE any store
//!     access (T-46.6-11 — SQL-injection/path-traversal-via-id mitigation);
//!     the store itself only ever uses `rusqlite` `params!` bindings, never
//!     `format!`-interpolated SQL (`ironhermes-artifacts`, Plan 01).
//!   - This route never grants the same-origin sandbox allowance — that iframe
//!     `sandbox` attribute belongs only to the Plan 05 viewer `<iframe>`, and
//!     must never be paired with `allow-scripts` (RESEARCH Pitfall 1 /
//!     MDN iframe#sandbox).
//!
//! Auth: no explicit gate, matching `audio_route.rs` and `ws.rs` — this route
//! runs on operator-local origin without auth middleware. Artifact ids are
//! server-generated UUIDs (122 bits of randomness).

#[cfg(feature = "server")]
use axum::body::Body;
#[cfg(feature = "server")]
use axum::extract::Path;
#[cfg(feature = "server")]
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
#[cfg(feature = "server")]
use axum::http::StatusCode;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};
#[cfg(feature = "server")]
use ironhermes_artifacts::ArtifactError;
#[cfg(feature = "server")]
use uuid::Uuid;

/// Phase 46.6 Plan 03 (D-02): the single authoritative Content-Security-Policy
/// string for artifact responses. One const — never scattered string
/// concatenation across call sites (RESEARCH Don't-Hand-Roll table).
///
/// - `default-src 'none'` + `connect-src 'none'`: blocks all outbound network
///   (fetch/XHR/WS/beacon) from the framed artifact regardless of sandbox
///   state (T-46.6-09 mitigation).
/// - `script-src 'unsafe-inline'` / `style-src 'unsafe-inline'`: agent-authored
///   artifacts are self-contained inline HTML/CSS/JS with no external assets.
/// - `img-src data:` / `font-src data:`: allow inline data-URI images/fonts
///   (common in self-contained artifacts) without allowing network fetches.
/// - `frame-src 'none'`: an artifact cannot frame further content.
/// - `form-action 'none'` / `base-uri 'none'`: no form submission or `<base>`
///   redirection out of the sandboxed origin.
#[cfg(feature = "server")]
const ARTIFACT_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; \
     style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; \
     frame-src 'none'; form-action 'none'; base-uri 'none'";

/// Build a response carrying the load-bearing D-02 headers (CSP + an explicit,
/// single `Content-Type` + `X-Content-Type-Options: nosniff`). Built via
/// `Response::builder` + `Body::from` so the body type never injects a
/// competing default `Content-Type` — every response, success or error, has
/// exactly one CSP, one content type, and the nosniff header.
///
/// WR-02 (code review): `nosniff` makes the explicit `Content-Type` binding
/// non-negotiable rather than relying on sniffing-algorithm details that
/// differ across clients — `serve_artifact_raw`'s own stated goal is that a
/// raw HTML artifact fetched through it must be displayed as text, never
/// executed as a document (T-52.1-05); this closes the gap in that guarantee
/// for every response through this shared helper, not only the raw route.
#[cfg(feature = "server")]
fn respond(status: StatusCode, content_type: &'static str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_SECURITY_POLICY, ARTIFACT_CSP)
        .header(CONTENT_TYPE, content_type)
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `GET /artifacts/{id}` — serve the rendered HTML for a published artifact.
///
/// Mounted explicitly on the axum router in `main.rs`
/// (`.route("/artifacts/{id}", get(serve_artifact))`).
#[cfg(feature = "server")]
pub async fn serve_artifact(Path(id): Path<String>) -> Response {
    // T-46.6-11 mitigation: reject anything that isn't a syntactically valid
    // UUID before it ever touches the store (SQL-injection / path-traversal-
    // via-id). A malformed id is a client error → 400.
    if Uuid::parse_str(&id).is_err() {
        return respond(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            b"invalid id".to_vec(),
        );
    }

    let state = crate::server::state::global_app_state();
    let loaded = {
        let store = match state.artifact_store.lock() {
            Ok(store) => store,
            Err(_) => {
                return respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8",
                    b"artifact store unavailable".to_vec(),
                );
            }
        };
        store.load_latest_html(&id)
    };

    match loaded {
        Ok(html) => respond(
            StatusCode::OK,
            "text/html; charset=utf-8",
            html.into_bytes(),
        ),
        // A well-formed id with no matching artifact/version is a 404, not a 500
        // — distinguished via the store's typed `NotFound`.
        Err(ArtifactError::NotFound(_)) => respond(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"not found".to_vec(),
        ),
        // Any other store failure (SQLite/render) is a genuine 500.
        Err(_) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            b"artifact load failed".to_vec(),
        ),
    }
}

/// `GET /artifacts/{id}/raw` — serve the unrendered stored body for a
/// published artifact as plain text (Plan 02, D-03).
///
/// Built from the same four ingredients as [`serve_artifact`]: the UUID gate
/// before any store access, the lock-poison-to-500 branch, the store call,
/// and the [`respond`] helper for every return path so the CSP is set
/// unconditionally. The only differences: this calls
/// [`ironhermes_artifacts::ArtifactStore::load_latest_source`] instead of
/// `load_latest_html`, and it responds with `text/plain; charset=utf-8`
/// rather than `text/html` — a raw HTML artifact fetched through this route
/// must be displayed as text, never executed as a document (T-52.1-05).
///
/// Mounted explicitly on the axum router in `main.rs` AND `login_page.rs`
/// (`.route("/artifacts/{id}/raw", get(serve_artifact_raw))`) — both router
/// constructions must mirror each other exactly or the raw download button
/// 404s under one of the two server entry points.
#[cfg(feature = "server")]
pub async fn serve_artifact_raw(Path(id): Path<String>) -> Response {
    // T-52.1-06 mitigation: reject anything that isn't a syntactically valid
    // UUID before it ever touches the store, same as serve_artifact.
    if Uuid::parse_str(&id).is_err() {
        return respond(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8",
            b"invalid id".to_vec(),
        );
    }

    let state = crate::server::state::global_app_state();
    let loaded = {
        let store = match state.artifact_store.lock() {
            Ok(store) => store,
            Err(_) => {
                return respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8",
                    b"artifact store unavailable".to_vec(),
                );
            }
        };
        store.load_latest_source(&id)
    };

    match loaded {
        Ok((_format, body)) => {
            respond(StatusCode::OK, "text/plain; charset=utf-8", body.into_bytes())
        }
        // A well-formed id with no matching artifact/version is a 404, not a 500
        // — distinguished via the store's typed `NotFound`.
        Err(ArtifactError::NotFound(_)) => respond(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8",
            b"not found".to_vec(),
        ),
        // Any other store failure (SQLite) is a genuine 500.
        Err(_) => respond(
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain; charset=utf-8",
            b"artifact load failed".to_vec(),
        ),
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use axum::extract::Path;

    /// D-02 / T-46.6-11: serve_artifact must reject a non-UUID id BEFORE any
    /// store access, returning 400 (not the old blanket 500) — for both a
    /// garden-variety invalid string and a path-traversal attempt — and it must
    /// still carry the CSP on that error path.
    ///
    /// WR-02 (code review): also asserts `X-Content-Type-Options: nosniff` on
    /// the same error path, since `respond()` sets both headers
    /// unconditionally — extends this existing header-assertion test rather
    /// than adding a parallel one.
    #[tokio::test]
    async fn artifact_route_rejects_invalid_id_as_400() {
        for bad in ["abc", "../etc/passwd"] {
            let resp = serve_artifact(Path(bad.to_string())).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "a non-uuid id ({bad}) must be rejected with 400"
            );
            assert_eq!(
                resp.headers()
                    .get(CONTENT_SECURITY_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some(ARTIFACT_CSP),
                "the CSP must be present even on the 400 error path (D-02)"
            );
            assert_eq!(
                resp.headers()
                    .get(X_CONTENT_TYPE_OPTIONS)
                    .and_then(|v| v.to_str().ok()),
                Some("nosniff"),
                "nosniff must be present even on the 400 error path (WR-02)"
            );
        }
    }

    /// D-03 / T-52.1-06: serve_artifact_raw must reject a non-UUID id BEFORE
    /// any store access, returning 400, and must still carry the CSP on that
    /// error path — same shape as `artifact_route_rejects_invalid_id_as_400`
    /// above, for the raw route.
    ///
    /// WR-02 (code review): also asserts `X-Content-Type-Options: nosniff` —
    /// the raw route's own stated goal is that its content must never be
    /// executed as a document, so this header matters most here.
    #[tokio::test]
    async fn artifact_route_raw_rejects_invalid_id_as_400() {
        for bad in ["abc", "../etc/passwd"] {
            let resp = serve_artifact_raw(Path(bad.to_string())).await;
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "a non-uuid id ({bad}) must be rejected with 400 on the raw route"
            );
            assert_eq!(
                resp.headers()
                    .get(CONTENT_SECURITY_POLICY)
                    .and_then(|v| v.to_str().ok()),
                Some(ARTIFACT_CSP),
                "the CSP must be present even on the raw route's 400 error path (D-02)"
            );
            assert_eq!(
                resp.headers()
                    .get(X_CONTENT_TYPE_OPTIONS)
                    .and_then(|v| v.to_str().ok()),
                Some("nosniff"),
                "nosniff must be present even on the raw route's 400 error path (WR-02)"
            );
        }
    }

    /// D-02 / T-46.6-10: ARTIFACT_CSP must carry every required directive —
    /// this test guards against a future edit silently weakening the policy.
    #[test]
    fn artifact_route_csp_has_all_directives() {
        for directive in [
            "default-src 'none'",
            "connect-src 'none'",
            "base-uri 'none'",
            "form-action 'none'",
            "frame-src 'none'",
            "img-src data:",
            "font-src data:",
        ] {
            assert!(
                ARTIFACT_CSP.contains(directive),
                "ARTIFACT_CSP missing required directive: {directive}"
            );
        }
    }
}
