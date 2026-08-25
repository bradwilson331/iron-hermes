//! D-06/D-08: the single, router-wide bearer-key authentication layer for
//! [`crate::api_server`].
//!
//! Applied once, in [`crate::api_server::routes::build_router`], as an
//! `axum::middleware::from_fn_with_state` layer over the FULLY MERGED
//! router — not per-handler. That is what makes "every route is gated" a
//! structural property rather than a habit: a route added by a later plan
//! is gated by construction, and [`crate::api_server::routes::all_registered_paths`]
//! is what lets a test assert that structurally (enumerate the router's own
//! path list, don't spot-check favourites).
//!
//! **D-08 — exactly one authentication path.** This module contains no
//! cookie extraction and no password-hash verification. OpenAI-compatible
//! clients send a bearer header; a cookie session does not serve them, and
//! two auth paths on one surface is two things to get wrong. There is also
//! no per-request profile selector here or anywhere in `api_server` —
//! IronHermes profiles are process-scoped (the profile flag pivots the home
//! directory at process start), so a per-request profile parameter would be
//! a promise the runtime cannot keep. An operator who wants per-profile REST
//! access runs a second gateway, with a different profile, on a different
//! port.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::ApiServerAdapter;

/// Router-wide auth middleware. Extracts the bearer value from the
/// `Authorization` header and compares it against the adapter's configured
/// key using [`constant_time_eq`] — never `==` on the raw strings, which
/// short-circuits on the first differing byte and leaks the key one byte at
/// a time to a caller who can measure response latency.
///
/// Refuses with `401` and a body that does NOT echo the presented value,
/// whether the header is absent, malformed, or simply wrong.
pub async fn require_bearer(
    State(adapter): State<Arc<ApiServerAdapter>>,
    req: Request,
    next: Next,
) -> Response {
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let admitted = match presented {
        Some(token) => constant_time_eq(token.as_bytes(), adapter.api_key().as_bytes()),
        None => false,
    };

    if !admitted {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    next.run(req).await
}

/// Constant-time byte comparison. Deliberately does NOT length-short-circuit
/// into an early `return` before scanning — the loop below always walks
/// every byte of the shorter iterator's length (via `zip`) and additionally
/// folds a length-mismatch signal into the same accumulator, so a
/// prefix-correct wrong key and a wholly wrong key take the same code path
/// and produce the same `bool` through the same instruction sequence.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let len_diff = (a.len() != b.len()) as u8;
    let mut diff: u8 = len_diff;
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_equal_slices() {
        assert!(constant_time_eq(b"secret-key", b"secret-key"));
    }

    #[test]
    fn constant_time_eq_rejects_wrong_value_same_length() {
        assert!(!constant_time_eq(b"secret-key", b"secret-keZ"));
    }

    #[test]
    fn constant_time_eq_rejects_wrong_length() {
        assert!(!constant_time_eq(b"secret-key", b"secret-key-longer"));
        assert!(!constant_time_eq(b"secret-key", b"short"));
    }

    #[test]
    fn constant_time_eq_rejects_empty_against_nonempty() {
        assert!(!constant_time_eq(b"", b"secret-key"));
    }
}
