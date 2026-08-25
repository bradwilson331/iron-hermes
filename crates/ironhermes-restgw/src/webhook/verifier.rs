//! Webhook signature verification seam (D-09/D-10).
//!
//! [`VerifyRequest`] is a borrowing superset over what any of the four
//! signature schemes needs: the Twilio scheme signs the request URL
//! concatenated with alphabetically sorted form parameters, not the raw
//! body — a verifier signature that accepts only body bytes could not
//! implement Twilio at all. Plan 01 defined this shape while implementing
//! only `generic_v2`; this plan (Plan 02) proves it also carries Twilio
//! (HMAC-SHA1, URL+sorted-form, base64) and Telnyx (Ed25519, asymmetric,
//! timestamp+pipe+raw-payload) without reworking the seam.
//!
//! [`Verifier`] is an enum, not a trait object: the set of schemes is
//! closed at four for this phase, each new provider is an additive variant,
//! and enum dispatch avoids a boxed allocation per request. All four arms
//! now carry a real implementation — see
//! `all_four_selectors_have_a_real_implementation` in the sibling
//! `tests/webhook_signature_verification.rs` integration test file for the
//! falsifiable proof.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine as _;
use ed25519_dalek::Verifier as _; // brings VerifyingKey::verify() into scope; anonymous import so it does not collide with this module's own `Verifier` enum.
use ed25519_dalek::{Signature, VerifyingKey};
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
type HmacSha1 = Hmac<Sha1>;

/// Header carrying the lowercase-hex `generic_v2` signature (D-10). REQUIRED
/// for the `generic_v2` scheme — there is no fallback when it is absent.
pub const HEADER_SIGNATURE_V2: &str = "X-Webhook-Signature-V2";
/// Header carrying the decimal unix-seconds timestamp bound into the
/// `generic_v2` signed content. REQUIRED — there is no fallback when it is
/// absent (D-10).
pub const HEADER_TIMESTAMP: &str = "X-Webhook-Timestamp";
/// Header carrying the base64 HMAC-SHA1 Twilio signature. REQUIRED for the
/// `twilio` scheme — there is no fallback when it is absent (D-10).
pub const HEADER_TWILIO_SIGNATURE: &str = "X-Twilio-Signature";
/// Header carrying the base64 Ed25519 Telnyx signature. REQUIRED for the
/// `telnyx` scheme (D-10).
pub const HEADER_TELNYX_SIGNATURE: &str = "telnyx-signature-ed25519";
/// Header carrying the decimal unix-seconds timestamp bound into the Telnyx
/// signed content. REQUIRED (D-10) — a stale value outside the route's
/// `timestamp_skew_secs` is refused even when the signature itself is
/// cryptographically valid (replay protection).
pub const HEADER_TELNYX_TIMESTAMP: &str = "telnyx-timestamp";

/// Everything a signature verifier might need to check a request, borrowed
/// from the per-request handler's already-buffered state. Superset shape
/// per D-09 — see this module's doc comment.
#[derive(Debug, Clone, Copy)]
pub struct VerifyRequest<'a> {
    /// The exact bytes received on the wire, untouched by any parsing.
    /// `generic_v2` and Telnyx both sign these raw bytes — losing them to
    /// form-parsing would break both schemes.
    pub raw_body: &'a [u8],
    /// Present only when the request's `Content-Type` was
    /// `application/x-www-form-urlencoded`; `None` for every other content
    /// type, including JSON. Twilio signs the URL plus these decoded
    /// key/value pairs (sorted), not the raw body.
    pub parsed_form: Option<&'a HashMap<String, String>>,
    /// The exact request URL — scheme, host, path and query — AS THE
    /// SENDER ADDRESSED IT, with no normalisation (dropped default port,
    /// re-encoded query, added/removed trailing slash). Twilio signs this
    /// string byte for byte.
    pub request_url: &'a str,
    /// The full inbound header map.
    pub headers: &'a HeaderMap,
}

/// Result of a verifier's [`Verifier::verify`] call. Deliberately not a
/// bare `bool` — the rejection reason is logged (never the secret material)
/// so an operator can distinguish "wrong secret" from "missing header" from
/// "expired timestamp" without a debugger.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyOutcome {
    Accepted,
    Rejected(String),
}

impl VerifyOutcome {
    pub fn is_accepted(&self) -> bool {
        matches!(self, VerifyOutcome::Accepted)
    }
}

/// The four D-09 signature schemes, each carrying the resolved key material
/// for its route (never the raw env var name — that lives on
/// [`crate::webhook::route_config::WebhookRoute`], which is resolved into
/// one of these variants once at `WebhookAdapter::new` time).
#[derive(Debug, Clone)]
pub enum Verifier {
    /// D-10: the only generic scheme this phase ships. No weaker fallback
    /// exists — there is exactly one signed-content construction in this
    /// file and exactly one header name for the signature.
    GenericV2 { secret: String, skew_secs: u64 },
    /// D-10: accepts unconditionally at request time. Gated instead at
    /// `WebhookAdapter::new` construction time — see that function's own
    /// doc comment for the loopback-only rail.
    None,
    /// D-09/D-14: HMAC-SHA1 over the exact request URL concatenated with
    /// every parsed form parameter sorted alphabetically by key, base64
    /// encoded (see [`verify_twilio`]). NEVER falls back to HMAC-ing the raw
    /// body (D-10) — a request with no parsed form parameters (e.g. a JSON
    /// body) cannot be verified by this scheme and is refused outright.
    Twilio { auth_token: String },
    /// D-09: Ed25519 signature over `{timestamp}|{raw body}`, verified
    /// against the account's public key — asymmetric, no shared secret (see
    /// [`verify_telnyx`]). The key is parsed into a `VerifyingKey` at
    /// construction time via [`Verifier::telnyx_from_env_value`], not on the
    /// first live request — a malformed value fails `WebhookAdapter::new`
    /// outright.
    Telnyx {
        public_key: VerifyingKey,
        skew_secs: u64,
    },
}

impl Verifier {
    pub fn verify(&self, req: &VerifyRequest<'_>) -> VerifyOutcome {
        match self {
            Verifier::GenericV2 { secret, skew_secs } => verify_generic_v2(secret, *skew_secs, req),
            Verifier::None => VerifyOutcome::Accepted,
            Verifier::Twilio { auth_token } => verify_twilio(auth_token, req),
            Verifier::Telnyx {
                public_key,
                skew_secs,
            } => verify_telnyx(public_key, *skew_secs, req),
        }
    }

    /// Fallible constructor for the `Telnyx` variant (D-09/D-10): parses the
    /// account public key at construction time so a malformed value fails
    /// the whole adapter's construction — see `WebhookAdapter::new`'s call
    /// site in `webhook/mod.rs` — rather than surfacing on the first live
    /// webhook request. `skew_secs` reuses the route's own
    /// `timestamp_skew_secs` (one knob, not two — see [`verify_telnyx`]).
    pub fn telnyx_from_env_value(raw_public_key: &str, skew_secs: u64) -> Result<Verifier, String> {
        Ok(Verifier::Telnyx {
            public_key: parse_telnyx_public_key(raw_public_key)?,
            skew_secs,
        })
    }
}

/// Decode a base64-encoded 32-byte Ed25519 public key (the account public
/// key an operator copies from their Telnyx portal) into a `VerifyingKey`.
/// Fails on anything that is not valid base64, does not decode to exactly 32
/// bytes, or does not encode a valid curve point — called once at
/// construction time (see [`Verifier::telnyx_from_env_value`]), never per
/// request.
fn parse_telnyx_public_key(raw: &str) -> Result<VerifyingKey, String> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| format!("telnyx public key is not valid base64: {e}"))?;
    let array: [u8; 32] = bytes.try_into().map_err(|v: Vec<u8>| {
        format!(
            "telnyx public key must decode to exactly 32 bytes, got {}",
            v.len()
        )
    })?;
    VerifyingKey::from_bytes(&array)
        .map_err(|e| format!("telnyx public key is not a valid ed25519 point: {e}"))
}

/// Whether `ts` (a caller-supplied epoch-seconds value) sits within
/// `skew_secs` of `now`. Fails closed on arithmetic overflow.
///
/// `ts` arrives from a request header via `parse::<i64>()`, which accepts the
/// entire `i64` range — including values chosen to make the naive
/// `(now - ts).abs()` overflow. Both halves overflow, and both matter:
///
/// - `now - ts` overflows for `ts` near `i64::MIN`; and
/// - `i64::MIN.abs()` overflows, because `i64::MIN` has no positive
///   counterpart. `ts = now.wrapping_add(i64::MIN)` makes `now - ts` land on
///   exactly `i64::MIN`, and that value parses and transmits perfectly well.
///
/// In a release build (no `overflow-checks` is set anywhere in this
/// workspace) the naive form wrapped to `i64::MIN`, which is not `> skew`, so
/// the window silently ACCEPTED an arbitrarily stale timestamp. In a
/// `dev`/`test` build the same expression panicked the connection task —
/// before signature verification, so pre-auth.
///
/// Neither yielded a replay on its own: both skew-checked schemes bind the
/// timestamp into the signed content, so a captured request cannot be
/// re-timestamped without invalidating its signature. This is a
/// defence-in-depth layer restored, not a break repaired (36.7.1 security
/// audit N-01).
fn within_skew(now: i64, ts: i64, skew_secs: u64) -> bool {
    now.checked_sub(ts)
        .and_then(i64::checked_abs)
        .is_some_and(|delta| delta <= skew_secs as i64)
}

/// The `generic_v2` scheme (D-10, source_facts #13): signed content is the
/// decimal timestamp bytes, then a single ASCII period byte, then the raw
/// body bytes, HMAC-SHA256 under the route's shared secret. Rejects
/// outright — with no other check attempted — whenever the signature
/// header is absent, the timestamp header is absent, the timestamp does
/// not parse as an integer, or the absolute skew exceeds `skew_secs`.
/// Comparison uses [`Mac::verify_slice`] (constant time) — never a `==` on
/// hex strings.
pub fn verify_generic_v2(secret: &str, skew_secs: u64, req: &VerifyRequest<'_>) -> VerifyOutcome {
    let Some(sig_hex) = req
        .headers
        .get(HEADER_SIGNATURE_V2)
        .and_then(|v| v.to_str().ok())
    else {
        return VerifyOutcome::Rejected(format!("missing {HEADER_SIGNATURE_V2} header"));
    };

    let Some(ts_str) = req
        .headers
        .get(HEADER_TIMESTAMP)
        .and_then(|v| v.to_str().ok())
    else {
        return VerifyOutcome::Rejected(format!("missing {HEADER_TIMESTAMP} header"));
    };

    let Ok(ts) = ts_str.parse::<i64>() else {
        return VerifyOutcome::Rejected(format!("{HEADER_TIMESTAMP} is not a valid integer"));
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if !within_skew(now, ts, skew_secs) {
        return VerifyOutcome::Rejected(format!(
            "{HEADER_TIMESTAMP} outside the {skew_secs}s skew window"
        ));
    }

    let Ok(sig_bytes) = hex::decode(sig_hex) else {
        return VerifyOutcome::Rejected(format!("{HEADER_SIGNATURE_V2} is not valid hex"));
    };

    let mut signed_content = Vec::with_capacity(ts_str.len() + 1 + req.raw_body.len());
    signed_content.extend_from_slice(ts_str.as_bytes());
    signed_content.push(b'.');
    signed_content.extend_from_slice(req.raw_body);

    let Ok(mut mac) = HmacSha256::new_from_slice(secret.as_bytes()) else {
        // HMAC-SHA256 accepts any key length; this arm exists only so the
        // fallible constructor has somewhere to go, never expected to
        // trigger.
        return VerifyOutcome::Rejected("invalid secret key material".to_string());
    };
    mac.update(&signed_content);

    match mac.verify_slice(&sig_bytes) {
        Ok(()) => VerifyOutcome::Accepted,
        Err(_) => VerifyOutcome::Rejected("signature mismatch".to_string()),
    }
}

/// The Twilio scheme (D-09, source_facts #1/#3): HMAC-SHA1 keyed with the
/// account auth token, computed over the exact request URL concatenated
/// with every parsed form parameter — sorted alphabetically by key, written
/// key-immediately-followed-by-value with no separator between pairs — then
/// base64-encoded (NOT hex, unlike the other three schemes). Rejects
/// outright, with no fallback to a body-HMAC check (D-10), when: the
/// `X-Twilio-Signature` header is absent; the presented value is not valid
/// base64; or `req.parsed_form` is absent (a route receiving a JSON body has
/// no parameters to canonicalise and cannot be verified by this scheme —
/// guessing is not a recovery). Comparison uses [`Mac::verify_slice`]
/// (constant time) against the decoded signature bytes, never a string
/// equality on the base64 text.
pub fn verify_twilio(auth_token: &str, req: &VerifyRequest<'_>) -> VerifyOutcome {
    let Some(sig_b64) = req
        .headers
        .get(HEADER_TWILIO_SIGNATURE)
        .and_then(|v| v.to_str().ok())
    else {
        return VerifyOutcome::Rejected(format!("missing {HEADER_TWILIO_SIGNATURE} header"));
    };

    let Some(form) = req.parsed_form else {
        return VerifyOutcome::Rejected(
            "twilio verifier requires application/x-www-form-urlencoded parameters; none were \
             parsed from this request (a JSON body is never verified against the raw body for \
             this scheme)"
                .to_string(),
        );
    };

    // Sort by key before concatenating — wire order is not stable and a
    // verifier that concatenates in arrival order fails intermittently,
    // which is worse than failing always (source_facts #179).
    let mut params: Vec<(&str, &str)> = form
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    params.sort_by(|a, b| a.0.cmp(b.0));

    let mut canonical = String::from(req.request_url);
    for (key, value) in &params {
        canonical.push_str(key);
        canonical.push_str(value);
    }

    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig_b64) else {
        return VerifyOutcome::Rejected(format!("{HEADER_TWILIO_SIGNATURE} is not valid base64"));
    };

    let Ok(mut mac) = HmacSha1::new_from_slice(auth_token.as_bytes()) else {
        // HMAC-SHA1 accepts any key length; this arm exists only so the
        // fallible constructor has somewhere to go, never expected to
        // trigger.
        return VerifyOutcome::Rejected("invalid auth token key material".to_string());
    };
    mac.update(canonical.as_bytes());

    match mac.verify_slice(&sig_bytes) {
        Ok(()) => VerifyOutcome::Accepted,
        Err(_) => VerifyOutcome::Rejected("signature mismatch".to_string()),
    }
}

/// The Telnyx scheme (D-09, source_facts #2): Ed25519 signature — asymmetric,
/// no shared secret — over the decimal timestamp header bytes, a single
/// ASCII pipe (`|`), then the raw payload bytes exactly as received (never a
/// re-serialised JSON value — re-serialisation changes whitespace or key
/// order and invalidates the signature). Enforces the replay tolerance
/// before attempting the signature check: rejects when `telnyx-timestamp` is
/// absent, does not parse as an integer, or lies further from the current
/// unix time than `skew_secs` allows — a cryptographically valid signature
/// with a stale timestamp is a replay. Rejects outright, with no fallback
/// (D-10), when `telnyx-signature-ed25519` is absent or is not valid
/// base64/64 bytes. Never implements curve arithmetic by hand —
/// `ed25519_dalek::VerifyingKey::verify` (via the `ed25519_dalek::Verifier`
/// trait) does the actual point-on-curve verification.
pub fn verify_telnyx(public_key: &VerifyingKey, skew_secs: u64, req: &VerifyRequest<'_>) -> VerifyOutcome {
    let Some(sig_b64) = req
        .headers
        .get(HEADER_TELNYX_SIGNATURE)
        .and_then(|v| v.to_str().ok())
    else {
        return VerifyOutcome::Rejected(format!("missing {HEADER_TELNYX_SIGNATURE} header"));
    };

    let Some(ts_str) = req
        .headers
        .get(HEADER_TELNYX_TIMESTAMP)
        .and_then(|v| v.to_str().ok())
    else {
        return VerifyOutcome::Rejected(format!("missing {HEADER_TELNYX_TIMESTAMP} header"));
    };

    let Ok(ts) = ts_str.parse::<i64>() else {
        return VerifyOutcome::Rejected(format!("{HEADER_TELNYX_TIMESTAMP} is not a valid integer"));
    };

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    if !within_skew(now, ts, skew_secs) {
        return VerifyOutcome::Rejected(format!(
            "{HEADER_TELNYX_TIMESTAMP} outside the {skew_secs}s skew window"
        ));
    }

    let Ok(sig_bytes) = base64::engine::general_purpose::STANDARD.decode(sig_b64) else {
        return VerifyOutcome::Rejected(format!("{HEADER_TELNYX_SIGNATURE} is not valid base64"));
    };

    let Ok(signature) = Signature::from_slice(&sig_bytes) else {
        return VerifyOutcome::Rejected(format!(
            "{HEADER_TELNYX_SIGNATURE} did not decode to a 64-byte signature"
        ));
    };

    let mut signed_content = Vec::with_capacity(ts_str.len() + 1 + req.raw_body.len());
    signed_content.extend_from_slice(ts_str.as_bytes());
    signed_content.push(b'|');
    signed_content.extend_from_slice(req.raw_body);

    match public_key.verify(&signed_content, &signature) {
        Ok(()) => VerifyOutcome::Accepted,
        Err(_) => VerifyOutcome::Rejected("signature mismatch".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_headers(secret: &str, ts: i64, body: &[u8]) -> HeaderMap {
        let ts_str = ts.to_string();
        let mut signed_content = Vec::with_capacity(ts_str.len() + 1 + body.len());
        signed_content.extend_from_slice(ts_str.as_bytes());
        signed_content.push(b'.');
        signed_content.extend_from_slice(body);
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&signed_content);
        let sig_hex = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(HEADER_SIGNATURE_V2, sig_hex.parse().unwrap());
        headers.insert(HEADER_TIMESTAMP, ts_str.parse().unwrap());
        headers
    }

    fn now_unix() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[test]
    fn valid_signature_in_window_accepted() {
        let secret = "shh";
        let body = b"{\"hello\":\"world\"}";
        let ts = now_unix();
        let headers = signed_headers(secret, ts, body);
        let req = VerifyRequest {
            raw_body: body,
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert_eq!(verify_generic_v2(secret, 300, &req), VerifyOutcome::Accepted);
    }

    #[test]
    fn tampered_body_rejected() {
        let secret = "shh";
        let body = b"{\"hello\":\"world\"}";
        let ts = now_unix();
        let headers = signed_headers(secret, ts, body);
        let tampered = b"{\"hello\":\"WORLD\"}";
        let req = VerifyRequest {
            raw_body: tampered,
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(!verify_generic_v2(secret, 300, &req).is_accepted());
    }

    #[test]
    fn expired_timestamp_rejected() {
        let secret = "shh";
        let body = b"payload";
        let ts = now_unix() - 400; // outside default 300s skew
        let headers = signed_headers(secret, ts, body);
        let req = VerifyRequest {
            raw_body: body,
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(!verify_generic_v2(secret, 300, &req).is_accepted());
    }

    // --- 36.7.1 security audit N-01: skew arithmetic overflow ---

    #[test]
    fn within_skew_fails_closed_on_overflow() {
        let now = 1_770_000_000_i64;
        // `now - ts` lands on exactly `i64::MIN`, whose `.abs()` has no
        // positive counterpart. The naive `(now - ts).abs() > skew` wrapped
        // back to `i64::MIN`, which is NOT `> 300`, so the window accepted.
        let overflow_ts = now.wrapping_add(i64::MIN);
        assert!(
            !within_skew(now, overflow_ts, 300),
            "a timestamp that overflows the difference must be refused, not accepted"
        );
        // The other overflowing half: `now - ts` itself, for `ts` at the rail.
        assert!(!within_skew(now, i64::MIN, 300));
        assert!(!within_skew(now, i64::MAX, 300));

        // And the fix must not over-reject the ordinary cases.
        assert!(within_skew(now, now, 300), "an exact match is in-window");
        assert!(within_skew(now, now - 300, 300), "the boundary is inclusive");
        assert!(within_skew(now, now + 300, 300), "clock skew runs both ways");
        assert!(!within_skew(now, now - 301, 300));
        assert!(!within_skew(now, now + 301, 300));
    }

    #[test]
    fn generic_v2_overflowing_timestamp_is_refused_even_when_correctly_signed() {
        // The strongest form of the assertion: the attacker holds the secret's
        // output for THIS crafted timestamp (the header is signed correctly),
        // so only the skew check stands between the request and acceptance.
        let secret = "shh";
        let body = b"payload";
        let overflow_ts = now_unix().wrapping_add(i64::MIN);
        let headers = signed_headers(secret, overflow_ts, body);
        let req = VerifyRequest {
            raw_body: body,
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(
            !verify_generic_v2(secret, 300, &req).is_accepted(),
            "an overflowing timestamp must be refused by the skew window"
        );
    }

    #[test]
    fn generic_v2_extreme_timestamps_do_not_panic() {
        // In a build with `overflow-checks` on — which is the DEFAULT for the
        // `dev`/`test` profile this very test runs under — the naive form
        // panicked here, before signature verification, i.e. pre-auth on an
        // unauthenticated request.
        let secret = "shh";
        let body = b"payload";
        for ts in [i64::MIN, i64::MIN + 1, i64::MAX, i64::MAX - 1, 0] {
            let headers = signed_headers(secret, ts, body);
            let req = VerifyRequest {
                raw_body: body,
                parsed_form: None,
                request_url: "http://example.test/webhook/r",
                headers: &headers,
            };
            assert!(
                !verify_generic_v2(secret, 300, &req).is_accepted(),
                "ts={ts} must be refused without panicking"
            );
        }
    }

    #[test]
    fn missing_timestamp_with_signature_present_rejected() {
        let secret = "shh";
        let body = b"payload";
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        let sig_hex = hex::encode(mac.finalize().into_bytes());
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_SIGNATURE_V2, sig_hex.parse().unwrap());
        // No X-Webhook-Timestamp header at all.
        let req = VerifyRequest {
            raw_body: body,
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        let outcome = verify_generic_v2(secret, 300, &req);
        assert!(!outcome.is_accepted());
        match outcome {
            VerifyOutcome::Rejected(msg) => assert!(msg.contains(HEADER_TIMESTAMP)),
            VerifyOutcome::Accepted => unreachable!(),
        }
    }

    #[test]
    fn malformed_signature_hex_rejected_not_weaker_check() {
        let ts = now_unix();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_SIGNATURE_V2, "not-hex-at-all!!".parse().unwrap());
        headers.insert(HEADER_TIMESTAMP, ts.to_string().parse().unwrap());
        let req = VerifyRequest {
            raw_body: b"payload",
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(!verify_generic_v2("shh", 300, &req).is_accepted());
    }

    #[test]
    fn missing_signature_header_rejected() {
        let ts = now_unix();
        let mut headers = HeaderMap::new();
        headers.insert(HEADER_TIMESTAMP, ts.to_string().parse().unwrap());
        let req = VerifyRequest {
            raw_body: b"payload",
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(!verify_generic_v2("shh", 300, &req).is_accepted());
    }

    #[test]
    fn none_variant_always_accepts() {
        let headers = HeaderMap::new();
        let req = VerifyRequest {
            raw_body: b"",
            parsed_form: None,
            request_url: "http://example.test/webhook/r",
            headers: &headers,
        };
        assert!(Verifier::None.verify(&req).is_accepted());
    }

    // The Twilio/Telnyx real-implementation and cross-variant discipline
    // cases (`twilio_*`, `telnyx_*`, `all_four_selectors_have_a_real_implementation`,
    // `no_variant_degrades_on_missing_header`) live in the dedicated
    // integration test file `tests/webhook_signature_verification.rs` per
    // this plan's `<files>` list — this module's own test-vector-free
    // `twilio_and_telnyx_stubs_reject` stub test is removed rather than
    // kept, since both variants now carry real implementations and no
    // longer unconditionally reject.

    #[test]
    fn parse_telnyx_public_key_rejects_non_base64() {
        assert!(super::parse_telnyx_public_key("not valid base64 !!!").is_err());
    }

    #[test]
    fn parse_telnyx_public_key_rejects_wrong_length() {
        // Valid base64, but decodes to fewer than 32 bytes.
        let short = base64::engine::general_purpose::STANDARD.encode(b"too-short");
        assert!(super::parse_telnyx_public_key(&short).is_err());
    }
}
