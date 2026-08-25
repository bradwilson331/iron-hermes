//! Phase 50.1 Plan 04 (D-06 analog): `GET /bot-avatars/{name}/{image_id}` —
//! raw avatar byte-serving route.
//!
//! Same raw-axum-handler pattern as `chat_attachment_route.rs` (and
//! `artifact_route.rs` before it) for the identical reason: the
//! `#[get]`/`#[server]` codec serializes a `Vec<u8>` return as a JSON
//! number array, which would break the `<img src>` avatar render this
//! route exists to serve. Mounted explicitly on the axum router in
//! `main.rs`/`login_page.rs::test_router` in the SAME router assembly as
//! `chat_attachment_route::serve_chat_attachment`, immediately after it.
//!
//! Security:
//!   - `name`/`image_id` resolve through `bot_avatar_api::bot_avatar_path`,
//!     which profile-name-validates `name` and leaf-validates `image_id`
//!     BEFORE either is joined into a path (T-50.1-04-01) — this route
//!     never trusts either URL segment directly as a filesystem path.
//!   - Content-type is derived from the SNIFFED LEADING BYTES, never a
//!     stored extension or client-supplied header — mirrors
//!     `chat_attachment_route.rs`'s XSS-hardening precedent. Every
//!     avatar was already signature-checked at upload/generation time, so
//!     this is defense in depth, not the primary gate.
//!   - Every response (success and error) carries
//!     `X-Content-Type-Options: nosniff` and a neutralizing CSP
//!     (`default-src 'none'; sandbox`), set in the single `respond`
//!     chokepoint — the same "headers on every path" discipline
//!     `chat_attachment_route.rs`/`artifact_route.rs` already established.
//!
//! Auth: no explicit gate, matching `chat_attachment_route.rs`/
//! `artifact_route.rs`/`ws.rs` — this route runs on operator-local origin
//! without auth middleware.

#[cfg(feature = "server")]
use axum::body::Body;
#[cfg(feature = "server")]
use axum::extract::Path;
#[cfg(feature = "server")]
use axum::http::header::{
    CONTENT_DISPOSITION, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS,
};
#[cfg(feature = "server")]
use axum::http::StatusCode;
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};

/// The single authoritative CSP for avatar responses — mirrors
/// `chat_attachment_route.rs`'s `ATTACHMENT_CSP` verbatim (a bot avatar is
/// as untrusted, byte-for-byte, as a chat attachment: it originated from
/// either an operator-controlled upload or a third-party image provider).
#[cfg(feature = "server")]
const AVATAR_CSP: &str = "default-src 'none'; sandbox";

/// Build a response with an explicit `Content-Type`, `nosniff`, and CSP —
/// every path in this module funnels through here, success or error alike.
#[cfg(feature = "server")]
fn respond(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_DISPOSITION, "inline")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CONTENT_SECURITY_POLICY, AVATAR_CSP)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Sniff the leading bytes for one of the four raster signatures this
/// crate accepts at avatar upload/generation time. A deliberate copy of
/// `bot_avatar_api.rs`'s own sniffer (porting-not-sharing convention,
/// matching the crate's established precedent) — kept local so this route
/// never depends on `bot_avatar_api`'s private items for anything beyond
/// the public path resolver.
#[cfg(feature = "server")]
fn sniff_raster_signature(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// `GET /bot-avatars/{name}/{image_id}` — serve the raw bytes for one
/// stored bot avatar.
#[cfg(feature = "server")]
pub async fn serve_bot_avatar(Path((name, image_id)): Path<(String, String)>) -> Response {
    let path = match crate::server::bot_avatar_api::bot_avatar_path(&name, &image_id) {
        Ok(p) => p,
        Err(_) => {
            return respond(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                b"invalid avatar reference".to_vec(),
            );
        }
    };

    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            return respond(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", b"not found".to_vec());
        }
    };

    match sniff_raster_signature(&bytes) {
        Some(mime) => respond(StatusCode::OK, mime, bytes),
        // Every write path into bot-avatars/ already signature-checked its
        // bytes — reaching here means an on-disk file was corrupted or
        // hand-placed outside this crate's own write paths. Refuse to
        // guess a content type for it.
        None => respond(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "text/plain; charset=utf-8",
            b"unsupported avatar content".to_vec(),
        ),
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use axum::http::header::{CONTENT_SECURITY_POLICY, X_CONTENT_TYPE_OPTIONS};

    fn header<'a>(resp: &'a Response, name: &axum::http::HeaderName) -> Option<&'a str> {
        resp.headers().get(name).and_then(|v| v.to_str().ok())
    }

    #[test]
    fn respond_sets_nosniff_and_csp_unconditionally() {
        let resp = respond(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", b"not found".to_vec());
        assert_eq!(header(&resp, &X_CONTENT_TYPE_OPTIONS), Some("nosniff"));
        let csp = header(&resp, &CONTENT_SECURITY_POLICY).expect("CSP required");
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("sandbox"));
    }

    #[test]
    fn sniffer_allowlists_only_the_four_supported_raster_types() {
        assert_eq!(sniff_raster_signature(b"\x89PNG\r\n\x1a\njunk"), Some("image/png"));
        assert_eq!(sniff_raster_signature(b"\xFF\xD8\xFFjunk"), Some("image/jpeg"));
        assert_eq!(sniff_raster_signature(b"GIF89ajunk"), Some("image/gif"));
        assert_eq!(sniff_raster_signature(b"RIFF\x00\x00\x00\x00WEBPjunk"), Some("image/webp"));
        assert_eq!(sniff_raster_signature(b"<svg><script>evil()</script></svg>"), None);
        assert_eq!(sniff_raster_signature(b""), None);
    }
}
