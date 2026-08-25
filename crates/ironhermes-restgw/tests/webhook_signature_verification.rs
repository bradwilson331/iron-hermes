//! D-09 signature-verification proof (Phase 36.7.1 Plan 02): hand-constructed
//! test vectors for the Twilio and Telnyx `Verifier` arms, plus the
//! cross-variant discipline cases that make "all four selectors carry a real
//! implementation" and "no variant degrades on a missing header" falsifiable
//! rather than asserted.
//!
//! No test performs a network call to Twilio or Telnyx — both providers'
//! algorithms are fully specified (RESEARCH.md's live-confirmed capture),
//! so every expected signature is computed here with the same primitives
//! the production verifier uses (`hmac::Hmac<sha1::Sha1>` + base64 for
//! Twilio, `ed25519_dalek` for Telnyx) and asserted against.
//!
//! D-10 (V2-only, no-degradation posture) applies across the whole enum in
//! this file, not just to `generic_v2`: every signing variant is proven to
//! refuse outright — never fall back to a weaker check or another variant's
//! check — when its own header is absent or corrupted.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey};
use hmac::{Hmac, Mac};
use ironhermes_cron::DeliveryRegistry;
use ironhermes_restgw::webhook::WebhookAdapter;
use ironhermes_restgw::webhook::route_config::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
    WebhookRoutesConfig,
};
use ironhermes_restgw::webhook::verifier::{VerifyOutcome, VerifyRequest, Verifier};
use tokio::sync::RwLock;

type HmacSha1 = Hmac<sha1::Sha1>;

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

// ===========================================================================
// Twilio fixtures (D-09: HMAC-SHA1 over URL + alphabetically sorted form
// parameters, base64-encoded — never the raw body).
// ===========================================================================

const TWILIO_URL: &str = "https://example.test/webhook/sms";

fn twilio_form() -> HashMap<String, String> {
    let mut m = HashMap::new();
    m.insert("To".to_string(), "+15551234567".to_string());
    m.insert("From".to_string(), "+15559876543".to_string());
    m.insert("Body".to_string(), "hello world".to_string());
    m
}

/// Build the canonical Twilio signed string: URL + every param sorted
/// alphabetically by key, key-immediately-followed-by-value, no separators.
fn twilio_canonical(url: &str, form: &HashMap<String, String>) -> String {
    let mut keys: Vec<&String> = form.keys().collect();
    keys.sort();
    let mut s = String::from(url);
    for k in keys {
        s.push_str(k);
        s.push_str(&form[k]);
    }
    s
}

fn twilio_sign(auth_token: &str, canonical: &str) -> String {
    let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
    mac.update(canonical.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

fn twilio_headers(sig: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("X-Twilio-Signature", sig.parse().unwrap());
    h
}

// ===========================================================================
// Twilio test vectors
// ===========================================================================

#[test]
fn twilio_accepts_a_correctly_signed_request() {
    let auth_token = "twilio-auth-token-1";
    let form = twilio_form();
    let canonical = twilio_canonical(TWILIO_URL, &form);
    let sig = twilio_sign(auth_token, &canonical);
    let headers = twilio_headers(&sig);
    // A real Twilio inbound SMS body — proves D-14's content-type-aware
    // parsing carries a provider that sends form-encoded, not JSON.
    let body = b"To=%2B15551234567&From=%2B15559876543&Body=hello+world".to_vec();
    let req = VerifyRequest {
        raw_body: &body,
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &headers,
    };
    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };
    assert_eq!(verifier.verify(&req), VerifyOutcome::Accepted);
}

#[test]
fn twilio_signature_is_over_url_plus_sorted_params() {
    let auth_token = "twilio-auth-token-2";
    let form = twilio_form();
    let body = b"To=%2B15551234567&From=%2B15559876543&Body=hello+world".to_vec();
    // Sign over the raw body bytes instead of the canonical string — the
    // naive body-HMAC mistake this test exists to catch (RESEARCH.md
    // Pitfall 2).
    let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
    mac.update(&body);
    let wrong_sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let headers = twilio_headers(&wrong_sig);
    let req = VerifyRequest {
        raw_body: &body,
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &headers,
    };
    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn twilio_param_order_on_the_wire_does_not_matter() {
    let auth_token = "twilio-auth-token-3";
    let form = twilio_form();
    let canonical = twilio_canonical(TWILIO_URL, &form);
    let sig = twilio_sign(auth_token, &canonical);
    let headers = twilio_headers(&sig);

    // Two raw bodies carrying the same fields in different wire order. The
    // verifier signs `parsed_form` (sorted), never `raw_body` order, so both
    // must verify against the one signature computed from the sorted
    // canonical string.
    let body_a = b"To=a&From=b&Body=c".to_vec();
    let body_b = b"Body=c&From=b&To=a".to_vec();

    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };

    let req_a = VerifyRequest {
        raw_body: &body_a,
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &headers,
    };
    assert_eq!(verifier.verify(&req_a), VerifyOutcome::Accepted);

    let req_b = VerifyRequest {
        raw_body: &body_b,
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &headers,
    };
    assert_eq!(verifier.verify(&req_b), VerifyOutcome::Accepted);
}

#[test]
fn twilio_result_is_base64_not_hex() {
    let auth_token = "twilio-auth-token-4";
    let form = twilio_form();
    let canonical = twilio_canonical(TWILIO_URL, &form);
    let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
    mac.update(canonical.as_bytes());
    let digest = mac.finalize().into_bytes();
    let hex_sig = hex::encode(digest.as_slice());
    let b64_sig = base64::engine::general_purpose::STANDARD.encode(digest.as_slice());

    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };

    let hex_headers = twilio_headers(&hex_sig);
    let hex_req = VerifyRequest {
        raw_body: b"",
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &hex_headers,
    };
    assert!(!verifier.verify(&hex_req).is_accepted());

    let b64_headers = twilio_headers(&b64_sig);
    let b64_req = VerifyRequest {
        raw_body: b"",
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &b64_headers,
    };
    assert_eq!(verifier.verify(&b64_req), VerifyOutcome::Accepted);
}

#[test]
fn twilio_altered_url_is_refused() {
    let auth_token = "twilio-auth-token-5";
    let form = twilio_form();
    let canonical = twilio_canonical(TWILIO_URL, &form);
    let sig = twilio_sign(auth_token, &canonical);
    let headers = twilio_headers(&sig);
    let altered_url = "https://example.test/webhook/DIFFERENT-ROUTE";
    let req = VerifyRequest {
        raw_body: b"",
        parsed_form: Some(&form),
        request_url: altered_url,
        headers: &headers,
    };
    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn twilio_json_body_is_refused_not_body_hmacked() {
    let auth_token = "twilio-auth-token-6";
    let body: &[u8] = br#"{"hello":"world"}"#;
    // Sign the raw JSON body directly — if the verifier ever fell back to a
    // body-HMAC check when `parsed_form` is absent, this would be accepted.
    let mut mac = HmacSha1::new_from_slice(auth_token.as_bytes()).unwrap();
    mac.update(body);
    let sig = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    let headers = twilio_headers(&sig);
    let req = VerifyRequest {
        raw_body: body,
        parsed_form: None, // a JSON body never populates the parsed-form map (D-14)
        request_url: TWILIO_URL,
        headers: &headers,
    };
    let verifier = Verifier::Twilio {
        auth_token: auth_token.to_string(),
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn twilio_missing_header_is_refused() {
    let form = twilio_form();
    let headers = HeaderMap::new();
    let req = VerifyRequest {
        raw_body: b"",
        parsed_form: Some(&form),
        request_url: TWILIO_URL,
        headers: &headers,
    };
    let verifier = Verifier::Twilio {
        auth_token: "whatever-token".to_string(),
    };
    assert!(!verifier.verify(&req).is_accepted());
}

// ===========================================================================
// Telnyx fixtures (D-09: Ed25519 over timestamp + pipe + raw payload).
// ===========================================================================

const TELNYX_URL: &str = "http://example.test/webhook/telnyx";

/// Fixed 32-byte seed — a deterministic test fixture, never a real
/// credential.
fn telnyx_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

fn telnyx_other_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

fn telnyx_sign(signing_key: &SigningKey, ts: i64, payload: &[u8]) -> String {
    let ts_str = ts.to_string();
    let mut content = Vec::with_capacity(ts_str.len() + 1 + payload.len());
    content.extend_from_slice(ts_str.as_bytes());
    content.push(b'|');
    content.extend_from_slice(payload);
    let signature: Signature = signing_key.sign(&content);
    base64::engine::general_purpose::STANDARD.encode(signature.to_bytes())
}

fn telnyx_headers(sig_b64: &str, ts: i64) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("telnyx-signature-ed25519", sig_b64.parse().unwrap());
    h.insert("telnyx-timestamp", ts.to_string().parse().unwrap());
    h
}

// ===========================================================================
// Telnyx test vectors
// ===========================================================================

#[test]
fn telnyx_ed25519_over_timestamp_pipe_payload() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix();
    let payload: &[u8] = br#"{"data":{"payload":{"text":"hi","from":{"phone_number":"+15551234567"}}}}"#;
    let sig = telnyx_sign(&signing_key, ts, payload);
    let headers = telnyx_headers(&sig, ts);
    let req = VerifyRequest {
        raw_body: payload,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    assert_eq!(verifier.verify(&req), VerifyOutcome::Accepted);
}

#[test]
fn telnyx_wrong_key_is_refused() {
    let signing_key = telnyx_signing_key();
    let ts = now_unix();
    let payload = b"payload-bytes";
    let sig = telnyx_sign(&signing_key, ts, payload);
    let headers = telnyx_headers(&sig, ts);
    let req = VerifyRequest {
        raw_body: payload,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let other_public_key = telnyx_other_signing_key().verifying_key();
    let verifier = Verifier::Telnyx {
        public_key: other_public_key,
        skew_secs: 300,
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn telnyx_tampered_payload_is_refused() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix();
    let payload = b"original payload bytes";
    let sig = telnyx_sign(&signing_key, ts, payload);
    let headers = telnyx_headers(&sig, ts);
    let tampered: &[u8] = b"original PAYLOAD bytes";
    let req = VerifyRequest {
        raw_body: tampered,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn telnyx_reserialized_json_is_refused() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix();
    let original: &[u8] = br#"{"a":1,"b":2}"#;
    let sig = telnyx_sign(&signing_key, ts, original);
    let headers = telnyx_headers(&sig, ts);
    // Semantically identical JSON, re-serialised with different whitespace
    // and key order — proves the RAW bytes are signed, not a parsed value.
    let reserialized: &[u8] = br#"{"b": 2, "a": 1}"#;
    let req = VerifyRequest {
        raw_body: reserialized,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn telnyx_stale_timestamp_is_refused() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix() - 600; // outside the 300s tolerance below
    let payload = b"payload-bytes";
    let sig = telnyx_sign(&signing_key, ts, payload);
    let headers = telnyx_headers(&sig, ts);
    let req = VerifyRequest {
        raw_body: payload,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    // The signature itself is cryptographically valid — only the timestamp
    // is stale. A captured request cannot be replayed indefinitely.
    assert!(!verifier.verify(&req).is_accepted());
}

/// 36.7.1 security audit N-01, Telnyx call site.
///
/// The `generic_v2` half is covered by `verifier.rs`'s own unit tests; this is
/// the second call site of the same skew check, and it is a separate line that
/// a regression could revert on its own.
///
/// Both halves of `(now - ts).abs()` overflow for adversarial `ts`, and
/// `telnyx-timestamp` is caller-supplied through `parse::<i64>()`, which
/// accepts the whole `i64` range. `ts = now.wrapping_add(i64::MIN)` makes the
/// difference land on exactly `i64::MIN`; the naive form wrapped that back to
/// `i64::MIN`, which is not `> 300`, so the window ACCEPTED it. This test signs
/// the crafted timestamp correctly, so the skew check is the only thing left
/// standing between the request and acceptance.
#[test]
fn telnyx_overflowing_timestamp_is_refused_even_when_correctly_signed() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let payload = b"payload-bytes";
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };

    for ts in [
        now_unix().wrapping_add(i64::MIN),
        i64::MIN,
        i64::MIN + 1,
        i64::MAX,
        i64::MAX - 1,
    ] {
        let sig = telnyx_sign(&signing_key, ts, payload);
        let headers = telnyx_headers(&sig, ts);
        let req = VerifyRequest {
            raw_body: payload,
            parsed_form: None,
            request_url: TELNYX_URL,
            headers: &headers,
        };
        // Must refuse — and must not panic. In a build with `overflow-checks`
        // on (the default for the profile this test runs under) the naive form
        // panicked here, before the Ed25519 verify, i.e. pre-auth.
        assert!(
            !verifier.verify(&req).is_accepted(),
            "ts={ts} must be refused by the skew window without panicking"
        );
    }
}

#[test]
fn telnyx_missing_signature_header_is_refused() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix();
    let mut headers = HeaderMap::new();
    headers.insert("telnyx-timestamp", ts.to_string().parse().unwrap());
    // No telnyx-signature-ed25519 header at all.
    let req = VerifyRequest {
        raw_body: b"payload-bytes",
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    assert!(!verifier.verify(&req).is_accepted());
}

#[test]
fn telnyx_missing_timestamp_header_is_refused() {
    let signing_key = telnyx_signing_key();
    let public_key = signing_key.verifying_key();
    let ts = now_unix();
    let payload = b"payload-bytes";
    let sig = telnyx_sign(&signing_key, ts, payload);
    let mut headers = HeaderMap::new();
    headers.insert("telnyx-signature-ed25519", sig.parse().unwrap());
    // No telnyx-timestamp header at all — no signature check is attempted
    // without it.
    let req = VerifyRequest {
        raw_body: payload,
        parsed_form: None,
        request_url: TELNYX_URL,
        headers: &headers,
    };
    let verifier = Verifier::Telnyx {
        public_key,
        skew_secs: 300,
    };
    assert!(!verifier.verify(&req).is_accepted());
}

/// Helper: a minimal `WebhookRoute` selecting `signature: telnyx`, used only
/// by the construction-time key-parsing test below (the rest of this file
/// exercises `Verifier`/`verify_telnyx` directly, without going through
/// `WebhookAdapter::new`).
fn telnyx_route(name: &str, public_key_env: &str) -> WebhookRoute {
    WebhookRoute {
        name: name.to_string(),
        path: format!("/webhook/{name}"),
        signature: SignatureKind::Telnyx,
        secret_env: None,
        auth_token_env: None,
        public_key_env: Some(public_key_env.to_string()),
        timestamp_skew_secs: 300,
        prompt_template: "{}".to_string(),
        deliver: DeliverTarget::Platform,
        deliver_url: None,
        deliver_platform: Some("teststub".to_string()),
        deliver_chat_id: None,
        deliver_only: false,
        outbound_auth: OutboundAuth::None,
        session: SessionMode::Ephemeral,
        rails: RouteRails::default(),
    }
}

#[test]
fn telnyx_malformed_public_key_fails_at_construction() {
    let env_name = "RESTGW_TEST_TELNYX_MALFORMED_PUBLIC_KEY";
    unsafe {
        std::env::set_var(env_name, "this-is-not-a-valid-base64-ed25519-key!!!");
    }

    let route = telnyx_route("telnyx-malformed", env_name);
    let config = WebhookRoutesConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        public_opt_in: false,
        external_base_url: None,
        routes: vec![route],
    };
    let registry = Arc::new(RwLock::new(DeliveryRegistry::new()));

    let result = WebhookAdapter::new(config, registry);
    assert!(
        result.is_err(),
        "adapter construction must fail on a malformed telnyx public key, not on the first live request"
    );

    unsafe {
        std::env::remove_var(env_name);
    }
}

// ===========================================================================
// Cross-variant discipline: D-09's "four real implementations" claim and
// D-10's "no degradation" posture, applied across the whole enum.
// ===========================================================================

#[test]
fn all_four_selectors_have_a_real_implementation() {
    // GenericV2: correct input accepts, corrupted input refuses.
    {
        let secret = "generic-secret";
        let body: &[u8] = b"payload-bytes";
        let ts = now_unix();
        let ts_str = ts.to_string();
        let mut content = Vec::with_capacity(ts_str.len() + 1 + body.len());
        content.extend_from_slice(ts_str.as_bytes());
        content.push(b'.');
        content.extend_from_slice(body);
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&content);
        let sig_hex = hex::encode(mac.finalize().into_bytes());
        let mut headers = HeaderMap::new();
        headers.insert("X-Webhook-Signature-V2", sig_hex.parse().unwrap());
        headers.insert("X-Webhook-Timestamp", ts_str.parse().unwrap());
        let verifier = Verifier::GenericV2 {
            secret: secret.to_string(),
            skew_secs: 300,
        };
        let req = VerifyRequest {
            raw_body: body,
            parsed_form: None,
            request_url: "http://example.test/webhook/g",
            headers: &headers,
        };
        assert_eq!(verifier.verify(&req), VerifyOutcome::Accepted);

        let tampered_req = VerifyRequest {
            raw_body: b"corrupted-payload",
            ..req
        };
        assert!(!verifier.verify(&tampered_req).is_accepted());
    }

    // None: accepts at request time regardless of input — its gate is the
    // construction-time loopback rail (`WebhookAdapter::new`), not a
    // per-request check.
    {
        let headers = HeaderMap::new();
        let req = VerifyRequest {
            raw_body: b"",
            parsed_form: None,
            request_url: "http://example.test/webhook/n",
            headers: &headers,
        };
        assert!(Verifier::None.verify(&req).is_accepted());

        let corrupted_headers = HeaderMap::new();
        let corrupted_req = VerifyRequest {
            raw_body: b"garbage-that-should-not-matter",
            parsed_form: None,
            request_url: "http://example.test/webhook/n",
            headers: &corrupted_headers,
        };
        assert!(Verifier::None.verify(&corrupted_req).is_accepted());
    }

    // Twilio: correct input accepts, corrupted input refuses.
    {
        let auth_token = "twilio-cross-variant-token";
        let form = twilio_form();
        let canonical = twilio_canonical(TWILIO_URL, &form);
        let sig = twilio_sign(auth_token, &canonical);
        let headers = twilio_headers(&sig);
        let verifier = Verifier::Twilio {
            auth_token: auth_token.to_string(),
        };
        let req = VerifyRequest {
            raw_body: b"",
            parsed_form: Some(&form),
            request_url: TWILIO_URL,
            headers: &headers,
        };
        assert_eq!(verifier.verify(&req), VerifyOutcome::Accepted);

        let mut bad_headers = HeaderMap::new();
        bad_headers.insert("X-Twilio-Signature", "corrupted-signature".parse().unwrap());
        let bad_req = VerifyRequest {
            raw_body: b"",
            parsed_form: Some(&form),
            request_url: TWILIO_URL,
            headers: &bad_headers,
        };
        assert!(!verifier.verify(&bad_req).is_accepted());
    }

    // Telnyx: correct input accepts, corrupted input refuses.
    {
        let signing_key = telnyx_signing_key();
        let public_key = signing_key.verifying_key();
        let ts = now_unix();
        let payload: &[u8] = br#"{"event":"ok"}"#;
        let sig = telnyx_sign(&signing_key, ts, payload);
        let headers = telnyx_headers(&sig, ts);
        let verifier = Verifier::Telnyx {
            public_key,
            skew_secs: 300,
        };
        let req = VerifyRequest {
            raw_body: payload,
            parsed_form: None,
            request_url: TELNYX_URL,
            headers: &headers,
        };
        assert_eq!(verifier.verify(&req), VerifyOutcome::Accepted);

        let tampered_req = VerifyRequest {
            raw_body: br#"{"event":"BAD"}"#,
            ..req
        };
        assert!(!verifier.verify(&tampered_req).is_accepted());
    }
}

#[test]
fn no_variant_degrades_on_missing_header() {
    // GenericV2 with its signature header stripped.
    {
        let headers = HeaderMap::new();
        let req = VerifyRequest {
            raw_body: b"payload-bytes",
            parsed_form: None,
            request_url: "http://example.test/webhook/g",
            headers: &headers,
        };
        let verifier = Verifier::GenericV2 {
            secret: "s".to_string(),
            skew_secs: 300,
        };
        assert!(!verifier.verify(&req).is_accepted());
    }

    // Twilio with its signature header stripped — no fallback to any other
    // variant's check, and no acceptance despite a valid parsed form.
    {
        let form = twilio_form();
        let headers = HeaderMap::new();
        let req = VerifyRequest {
            raw_body: b"",
            parsed_form: Some(&form),
            request_url: TWILIO_URL,
            headers: &headers,
        };
        let verifier = Verifier::Twilio {
            auth_token: "irrelevant-token".to_string(),
        };
        assert!(!verifier.verify(&req).is_accepted());
    }

    // Telnyx with its signature header stripped (timestamp present).
    {
        let signing_key = telnyx_signing_key();
        let public_key = signing_key.verifying_key();
        let ts = now_unix();
        let mut headers = HeaderMap::new();
        headers.insert("telnyx-timestamp", ts.to_string().parse().unwrap());
        let req = VerifyRequest {
            raw_body: b"payload-bytes",
            parsed_form: None,
            request_url: TELNYX_URL,
            headers: &headers,
        };
        let verifier = Verifier::Telnyx {
            public_key,
            skew_secs: 300,
        };
        assert!(!verifier.verify(&req).is_accepted());
    }
}
