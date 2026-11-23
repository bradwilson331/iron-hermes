//! Phase 54.7 Plan 05 — wiremock-backed tests for server-side voice discovery
//! (D-06/D-08). Fixtures are the real responses captured from the operator's
//! servers on 2026-10-16. No test touches a real network.

use ironhermes_core::voice_http::VoiceErrorCategory;
use ironhermes_tools::voice_discovery::{
    DiscoveryOutcome, MAX_ITEMS, edge_voice_ids, list_compat_models, list_compat_voices,
    list_elevenlabs_models, list_elevenlabs_voices, parse_model_ids, parse_voice_ids,
    within_discovery_timeout,
};
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn ids(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn voices_fixture() -> serde_json::Value {
    json!({
        "object": "list",
        "model": "mlx-community/Kokoro-82M-bf16",
        "data": [
            {"id": "af_alloy", "name": "af_alloy"},
            {"id": "af_heart", "name": "af_heart"}
        ]
    })
}

#[tokio::test]
async fn compat_voices_for_model_percent_encodes_and_parses_data_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .and(query_param("model", "mlx-community/Kokoro-82M-bf16"))
        .respond_with(ResponseTemplate::new(200).set_body_json(voices_fixture()))
        .mount(&server)
        .await;

    let base = format!("{}/v1", server.uri());
    let out = list_compat_voices(&base, "mlx-community/Kokoro-82M-bf16", None).await;
    assert_eq!(
        out,
        DiscoveryOutcome::Loaded(vec!["af_alloy".to_string(), "af_heart".to_string()])
    );

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    let raw_query = received[0].url.query().unwrap_or_default().to_string();
    assert!(
        raw_query.contains("model=mlx-community%2FKokoro-82M-bf16"),
        "model must be percent-encoded via query_pairs_mut, got {raw_query}"
    );
}

#[tokio::test]
async fn compat_voices_without_model_hits_the_400_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/audio/voices"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"detail": "model query parameter is required"})),
        )
        .mount(&server)
        .await;

    let base = format!("{}/v1", server.uri());
    let out = list_compat_voices(&base, "", None).await;
    assert_eq!(
        out,
        DiscoveryOutcome::Failed(VoiceErrorCategory::UnexpectedResponse)
    );

    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].url.query(), None, "empty model sends no query");
}

// ---------------------------------------------------------------------------
// Task 2: tolerant parsers, model listing, ElevenLabs and Edge listings
// ---------------------------------------------------------------------------

#[test]
fn parse_voice_ids_accepts_three_shapes() {
    // data[].id
    assert_eq!(
        parse_voice_ids(&json!({"data": [{"id": "af_alloy"}, {"id": "af_heart"}]})),
        ids(&["af_alloy", "af_heart"])
    );
    // voices[].id and voices[].voice_id
    assert_eq!(
        parse_voice_ids(&json!({"voices": [{"id": "a1"}, {"voice_id": "b2"}]})),
        ids(&["a1", "b2"])
    );
    // voices[] strings
    assert_eq!(
        parse_voice_ids(&json!({"voices": ["x", "y"]})),
        ids(&["x", "y"])
    );
    // an id failing validate_voice_id is dropped, duplicates removed
    assert_eq!(
        parse_voice_ids(&json!({"data": [{"id": "a b"}, {"id": "ok"}, {"id": "ok"}]})),
        ids(&["ok"])
    );
    // 600 entries -> 500 returned
    let many: Vec<serde_json::Value> = (0..600).map(|i| json!({"id": format!("v{i}")})).collect();
    assert_eq!(parse_voice_ids(&json!({"data": many})).len(), MAX_ITEMS);
    assert_eq!(MAX_ITEMS, 500);
}

#[test]
fn parse_model_ids_accepts_data_ids_and_drops_invalid() {
    assert_eq!(
        parse_model_ids(&json!({"data": [{"id": "good/model-1"}, {"id": "a\"b"}, {"id": "ok"}]})),
        ids(&["good/model-1", "ok"])
    );
    // top-level array of {id} or {model_id}
    assert_eq!(
        parse_model_ids(&json!([{"id": "m1"}, {"model_id": "m2"}])),
        ids(&["m1", "m2"])
    );
}

#[tokio::test]
async fn compat_models_lists_the_captured_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{
                "id": "mlx-community/Kokoro-82M-bf16",
                "object": "model",
                "created": 1792209204,
                "owned_by": "system"
            }]
        })))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Loaded(ids(&["mlx-community/Kokoro-82M-bf16"]))
    );
}

#[tokio::test]
async fn compat_models_404_is_empty_not_failed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Empty
    );

    // 405 is the same loaded-empty outcome
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(405))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Empty
    );
}

#[tokio::test]
async fn discovery_401_is_key_rejected_and_500_is_unexpected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Failed(VoiceErrorCategory::KeyRejected)
    );

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(500).set_body_string("secret internal detail"))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    let out = list_compat_models(&base, None).await;
    assert_eq!(
        out,
        DiscoveryOutcome::Failed(VoiceErrorCategory::UnexpectedResponse)
    );
    assert!(!format!("{out:?}").contains("secret internal detail"));
}

#[tokio::test]
async fn discovery_redirect_is_not_followed() {
    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{"id": "leak"}]})))
        .mount(&target)
        .await;

    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/v1/models", target.uri()).as_str()),
        )
        .mount(&origin)
        .await;

    let base = format!("{}/v1", origin.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Failed(VoiceErrorCategory::UnexpectedResponse)
    );
    assert_eq!(
        target.received_requests().await.unwrap().len(),
        0,
        "redirect target must receive zero requests"
    );
}

#[tokio::test]
async fn discovery_body_over_cap_is_unexpected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b' '; 2 * 1024 * 1024]))
        .mount(&server)
        .await;
    let base = format!("{}/v1", server.uri());
    assert_eq!(
        list_compat_models(&base, None).await,
        DiscoveryOutcome::Failed(VoiceErrorCategory::UnexpectedResponse)
    );
}

#[tokio::test]
async fn elevenlabs_voices_paginate_with_cap() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v2/voices"))
        .and(header("xi-api-key", "test-fake-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "voices": [{"voice_id": "21m00Tcm4TlvDq8ikWAM", "name": "Rachel"}],
            "has_more": true,
            "next_page_token": "tok"
        })))
        .mount(&server)
        .await;
    let key = SecretString::from("test-fake-key".to_string());
    let out = list_elevenlabs_voices(&server.uri(), &key).await;
    assert_eq!(
        out,
        DiscoveryOutcome::Loaded(ids(&["21m00Tcm4TlvDq8ikWAM"]))
    );
    let received = server.received_requests().await.unwrap();
    assert_eq!(received.len(), 5, "pagination is capped at 5 pages");
    assert!(
        received[0]
            .url
            .query()
            .unwrap_or_default()
            .contains("page_size=100")
    );
    assert!(
        received[1]
            .url
            .query()
            .unwrap_or_default()
            .contains("next_page_token=tok")
    );
}

#[tokio::test]
async fn elevenlabs_models_keep_tts_capable_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("xi-api-key", "test-fake-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {"model_id": "eleven_multilingual_v2", "can_do_text_to_speech": true},
            {"model_id": "eleven_english_sts_v2", "can_do_text_to_speech": false},
            {"model_id": "eleven_turbo_v2_5", "can_do_text_to_speech": true}
        ])))
        .mount(&server)
        .await;
    let key = SecretString::from("test-fake-key".to_string());
    assert_eq!(
        list_elevenlabs_models(&server.uri(), &key).await,
        DiscoveryOutcome::Loaded(ids(&["eleven_multilingual_v2", "eleven_turbo_v2_5"]))
    );
}

#[test]
fn edge_voice_ids_map_short_names() {
    let out = edge_voice_ids([
        Some("en-US-AriaNeural".to_string()),
        None,
        Some("en-GB-SoniaNeural".to_string()),
    ]);
    assert_eq!(out, ids(&["en-US-AriaNeural", "en-GB-SoniaNeural"]));
}

/// WR-06: a discovery fetch that never answers (Edge's client has no timeout of its
/// own) is cut off at the limit and reported as `timed out`.
#[tokio::test]
async fn stalled_discovery_fetch_times_out() {
    let started = std::time::Instant::now();
    let outcome = within_discovery_timeout(
        std::time::Duration::from_millis(50),
        std::future::pending::<DiscoveryOutcome>(),
    )
    .await;
    assert_eq!(
        outcome,
        DiscoveryOutcome::Failed(VoiceErrorCategory::TimedOut)
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(5));

    // A fetch that finishes in time is passed through untouched.
    let ok = within_discovery_timeout(std::time::Duration::from_secs(5), async {
        DiscoveryOutcome::Empty
    })
    .await;
    assert_eq!(ok, DiscoveryOutcome::Empty);
}
