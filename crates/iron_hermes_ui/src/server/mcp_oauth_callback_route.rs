//! Phase 48.2 Plan 09 (D-03/T-48.2-09-01/T-48.2-09-04): `GET /oauth/mcp/callback`
//! — the public, unauthenticated raw axum route that finishes a web MCP
//! OAuth authorization.
//!
//! # Why this route is unauthenticated
//!
//! The operator session cookie is `SameSite=Strict`
//! (`crate::server::auth::session_cookie`, `auth.rs:371-377`). An MCP
//! authorization server's redirect back to this origin is a **cross-site
//! top-level navigation**, on which a browser does not send a
//! `SameSite=Strict` cookie. Requiring a session on this route would
//! therefore break the exact flow it exists to serve. The capability this
//! route relies on instead is the unguessable, single-use OAuth `state`
//! value, matched against a server-side pending map that 48.2-08's
//! `McpManager` bounds by age (300s TTL) and count (cap 8); `rmcp`
//! independently re-validates that same state against its own session store
//! plus the RFC 9207 `iss` parameter during the token exchange. This is
//! structurally the same security model the pre-existing host-local
//! loopback listener already relies on — that listener is likewise
//! unauthenticated, with an ephemeral port standing in for a session.
//!
//! # Response discipline
//!
//! Every response — success, denial, malformed callback, or unknown state —
//! funnels through the single [`respond`] chokepoint, which sets
//! `Content-Type: text/html; charset=utf-8`, `X-Content-Type-Options:
//! nosniff`, and a neutralizing `default-src 'none'; sandbox` CSP. Neither
//! page ever echoes a query parameter (`error`, `error_description`, `code`,
//! `state`, or anything else) into its body — an authorization server's
//! denial reason is attacker-influenceable text arriving on an
//! unauthenticated route, so it is used only to decide which fixed page to
//! render, never rendered itself. This route never sets a `Location`
//! header on any path — it does not redirect anywhere, which is what makes
//! it useless as an open-redirect primitive.
//!
//! Status codes: 200 for a successful completion, 400 for a malformed or
//! denied callback, 404 for an unknown/already-used/expired `state`.

#[cfg(feature = "server")]
use axum::body::Body;
#[cfg(feature = "server")]
use axum::extract::Query;
#[cfg(feature = "server")]
use axum::http::header::{CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
#[cfg(feature = "server")]
use axum::http::{HeaderMap, StatusCode, Uri};
#[cfg(feature = "server")]
use axum::response::{IntoResponse, Response};

/// The three query parameters this route ever reads. `error_description`
/// and `iss` are deliberately NOT fields here — this crate has no `url`
/// dependency (this plan adds none), and this struct exists only to decide
/// which fixed page to render, never to read or render anything beyond
/// presence/identity. `#[serde(default)]` on every field means a query
/// missing any of them deserializes successfully rather than rejecting.
#[cfg(feature = "server")]
#[derive(serde::Deserialize, Default)]
struct CallbackQuery {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// The single authoritative CSP for this route's responses — a page that
/// only ever renders fixed, non-interactive markup earns the tightest CSP in
/// the crate (mirrors `bot_avatar_route.rs`'s `AVATAR_CSP` posture).
#[cfg(feature = "server")]
const CALLBACK_CSP: &str = "default-src 'none'; sandbox";

#[cfg(feature = "server")]
const SUCCESS_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization complete</title></head><body><h1>Authorization complete</h1><p>Close this tab and return to the Tools page.</p></body></html>";

#[cfg(feature = "server")]
const FAILURE_HTML: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Authorization failed</title></head><body><h1>Authorization failed</h1><p>Close this tab and return to the Tools page.</p></body></html>";

/// Build a response with an explicit `Content-Type`, `nosniff`, and CSP —
/// every path in this module funnels through here, success or error alike.
/// Never sets a `Location` header.
#[cfg(feature = "server")]
fn respond(status: StatusCode, html: &'static str) -> Response {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header(X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(CONTENT_SECURITY_POLICY, CALLBACK_CSP)
        .body(Body::from(html))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Reconstruct an absolute callback URL from the request's own headers and
/// path — this server never terminates TLS itself, so the scheme/authority
/// must be read from the request rather than assumed. Scheme comes from the
/// first comma-separated value of `X-Forwarded-Proto` when it is exactly
/// `http` or `https`, else `http`. Authority comes from `Host`, rejecting a
/// value that is empty or contains whitespace, control characters, or a
/// slash (none of those are legal in an HTTP authority, and rejecting them
/// here means a malformed `Host` can never produce a URL that silently
/// parses into something unexpected). If the reconstruction does not parse
/// as an absolute URI — most commonly because `Host` is absent or was
/// rejected — this falls back to a fixed loopback base. `rmcp` reads only
/// the `code`, `state`, and `iss` query parameters from the URL this
/// produces, so the base itself is not security-relevant; it only needs to
/// be syntactically valid, or the downstream parse fails and the callback is
/// treated as malformed. Validated with `axum::http::Uri` (already a
/// transitive dependency via `axum`) rather than the `url` crate — this
/// plan adds no new `Cargo.toml` dependency line.
#[cfg(feature = "server")]
fn reconstruct_callback_url(headers: &HeaderMap, uri: &Uri) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| *v == "http" || *v == "https")
        .unwrap_or("http");

    let authority = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|h| {
            !h.is_empty()
                && !h.chars().any(|c| c.is_whitespace() || c.is_control())
                && !h.contains('/')
        });

    let path_and_query = uri.path_and_query().map(|pq| pq.as_str()).unwrap_or("/");

    if let Some(authority) = authority {
        let candidate = format!("{scheme}://{authority}{path_and_query}");
        if let Ok(parsed) = candidate.parse::<Uri>() {
            if parsed.scheme().is_some() && parsed.authority().is_some() {
                return candidate;
            }
        }
    }

    // Fixed loopback fallback. `path_and_query` always starts with `/` for
    // an origin-form request-target, so this is always a syntactically
    // valid absolute URI.
    format!("http://127.0.0.1{path_and_query}")
}

/// `GET /oauth/mcp/callback` — receive the authorization server's redirect
/// and drive the handshake to completion (or a documented, non-leaking
/// refusal).
#[cfg(feature = "server")]
pub async fn serve_mcp_oauth_callback(headers: HeaderMap, uri: Uri) -> Response {
    let callback_url = reconstruct_callback_url(&headers, &uri);

    // Parsed from the ORIGINAL incoming request URI, not the reconstructed
    // string — the query portion is identical either way, and this avoids
    // re-parsing a URL string with anything beyond axum's own extractor.
    let query = Query::<CallbackQuery>::try_from_uri(&uri)
        .map(|q| q.0)
        .unwrap_or_default();

    if let Some(error) = query.error {
        // Never read, log, or render the `error`/`error_description` value
        // itself — it is attacker-influenceable text on an unauthenticated
        // route. Only its presence is acted on; the value is immediately
        // dropped.
        drop(error);
        if let Some(state) = query.state {
            crate::server::mcp_admin_api::abandon_oauth_from_callback(&state).await;
        }
        return respond(StatusCode::BAD_REQUEST, FAILURE_HTML);
    }

    if query.code.is_none() {
        // Neither an error nor a code — a malformed or replayed callback.
        return respond(StatusCode::BAD_REQUEST, FAILURE_HTML);
    }

    match crate::server::mcp_admin_api::complete_oauth_from_callback(&callback_url).await {
        Ok(()) => respond(StatusCode::OK, SUCCESS_HTML),
        Err(reason) => {
            let status = if reason == crate::server::mcp_admin_api::UNKNOWN_OAUTH_STATE_MESSAGE {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            respond(status, FAILURE_HTML)
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn headers_with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    fn uri_for(path_and_query: &str) -> Uri {
        path_and_query.parse().unwrap()
    }

    #[test]
    fn respond_sets_nosniff_and_csp_and_never_a_location_header() {
        let resp = respond(StatusCode::BAD_REQUEST, FAILURE_HTML);
        assert_eq!(
            resp.headers()
                .get(&X_CONTENT_TYPE_OPTIONS)
                .and_then(|v| v.to_str().ok()),
            Some("nosniff")
        );
        let csp = resp
            .headers()
            .get(&CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok())
            .expect("CSP required");
        assert!(csp.contains("default-src 'none'"));
        assert!(csp.contains("sandbox"));
        assert!(
            resp.headers().get(axum::http::header::LOCATION).is_none(),
            "this route must never set a Location header on any path"
        );
    }

    /// Accepts a plain `Host` header with no `X-Forwarded-Proto` at all —
    /// scheme defaults to `http`.
    #[test]
    fn reconstruct_accepts_a_plain_host_header() {
        let headers = headers_with(&[("host", "hermes.example.com")]);
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert_eq!(
            url,
            "http://hermes.example.com/oauth/mcp/callback?code=abc&state=xyz"
        );
    }

    /// An `X-Forwarded-Proto: https, http` list takes the FIRST value.
    #[test]
    fn reconstruct_takes_the_first_x_forwarded_proto_value() {
        let headers = headers_with(&[
            ("host", "hermes.example.com"),
            ("x-forwarded-proto", "https, http"),
        ]);
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert!(url.starts_with("https://hermes.example.com"));
    }

    /// A `Host` containing whitespace (the character class a CR/LF header
    /// injection attempt would also fall into) is rejected as an authority
    /// candidate — reconstruction falls back to the fixed loopback base
    /// rather than building a URL from an attacker-shaped `Host` value.
    /// `axum::http::HeaderValue::from_str` itself already rejects raw CR/LF
    /// bytes, so this test exercises the `is_whitespace`/`is_control` filter
    /// directly with a value that IS constructible as a `HeaderValue` (a
    /// plain ASCII space is legal there) but must still be rejected by this
    /// module's own authority filter.
    #[test]
    fn reconstruct_rejects_a_host_containing_whitespace() {
        let headers = headers_with(&[("host", "hermes.example.com evil")]);
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert_eq!(url, "http://127.0.0.1/oauth/mcp/callback?code=abc&state=xyz");
    }

    /// A `Host` containing a slash is rejected as an authority candidate.
    #[test]
    fn reconstruct_rejects_a_host_containing_a_slash() {
        let headers = headers_with(&[("host", "hermes.example.com/evil")]);
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert_eq!(url, "http://127.0.0.1/oauth/mcp/callback?code=abc&state=xyz");
    }

    /// No `Host` header at all falls back to the fixed loopback base.
    #[test]
    fn reconstruct_falls_back_to_loopback_when_host_is_absent() {
        let headers = HeaderMap::new();
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert_eq!(url, "http://127.0.0.1/oauth/mcp/callback?code=abc&state=xyz");
    }

    /// An empty `Host` header falls back to the fixed loopback base.
    #[test]
    fn reconstruct_falls_back_to_loopback_when_host_is_empty() {
        let headers = headers_with(&[("host", "")]);
        let uri = uri_for("/oauth/mcp/callback?code=abc&state=xyz");
        let url = reconstruct_callback_url(&headers, &uri);
        assert_eq!(url, "http://127.0.0.1/oauth/mcp/callback?code=abc&state=xyz");
    }

    /// A malformed callback with neither `error` nor `code` is refused
    /// with 400.
    #[tokio::test]
    async fn serve_callback_with_neither_error_nor_code_is_400() {
        let headers = headers_with(&[("host", "hermes.example.com")]);
        let uri = uri_for("/oauth/mcp/callback?state=xyz");
        let resp = serve_mcp_oauth_callback(headers, uri).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    /// A denial (`error` present) is refused with 400, and never echoes the
    /// error text into the body.
    #[tokio::test]
    async fn serve_callback_with_error_param_is_400_and_never_echoes_it() {
        const SENTINEL: &str = "planted-attacker-controlled-denial-reason";
        let headers = headers_with(&[("host", "hermes.example.com")]);
        let uri = uri_for(&format!(
            "/oauth/mcp/callback?error=access_denied&error_description={SENTINEL}&state=unknown-state-xyz"
        ));
        let resp = serve_mcp_oauth_callback(headers, uri).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            !text.contains(SENTINEL),
            "the denial page must never echo the error_description value"
        );
    }

    /// An unknown `state` on a `code` callback (no attempt was ever started
    /// for it) is refused with 404, not 400 — the caller never gets a token
    /// exchange attempted.
    #[tokio::test]
    async fn serve_callback_with_unknown_state_and_code_is_404() {
        let headers = headers_with(&[("host", "hermes.example.com")]);
        let uri = uri_for(
            "/oauth/mcp/callback?code=some-code&state=definitely-never-issued-by-this-process",
        );
        let resp = serve_mcp_oauth_callback(headers, uri).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
