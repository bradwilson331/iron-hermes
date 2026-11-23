//! Phase 54.7 Plan 05 — tests for the RealtimeProvider trait and its one
//! OpenAI-Realtime-compatible implementation (D-10/D-12). Fixtures use fake
//! keys and wiremock's 127.0.0.1 URI; no test touches a real network.

use ironhermes_core::config::RealtimeConfig;
use ironhermes_core::voice_http::VoiceErrorCategory;
use ironhermes_tools::realtime::{MintedSession, RealtimeProvider, realtime_provider};
use ironhermes_tools::voice_discovery::DiscoveryOutcome;
use secrecy::SecretString;
use serde_json::json;
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn provider() -> Box<dyn RealtimeProvider> {
    realtime_provider("openai_compatible").expect("openai_compatible is registered")
}

fn cfg_for(server: &MockServer) -> RealtimeConfig {
    RealtimeConfig {
        base_url: format!("{}/v1", server.uri()),
        ..RealtimeConfig::default()
    }
}

fn fake_key() -> SecretString {
    SecretString::from("test-fake-key".to_string())
}

#[test]
fn realtime_urls_for_default_and_custom_base() {
    let p = provider();
    let default = RealtimeConfig::default();
    assert_eq!(
        p.mint_url(&default).unwrap().as_str(),
        "https://api.openai.com/v1/realtime/client_secrets"
    );
    assert_eq!(
        p.calls_url(&default).unwrap().as_str(),
        "https://api.openai.com/v1/realtime/calls"
    );
    let custom = RealtimeConfig {
        base_url: "http://rt.lan:9000/v1".to_string(),
        ..RealtimeConfig::default()
    };
    assert_eq!(
        p.mint_url(&custom).unwrap().as_str(),
        "http://rt.lan:9000/v1/realtime/client_secrets"
    );
    assert_eq!(
        p.calls_url(&custom).unwrap().as_str(),
        "http://rt.lan:9000/v1/realtime/calls"
    );
}

#[test]
fn realtime_session_body_shape() {
    let p = provider();
    let tool = json!({"type": "function", "name": "t"});
    let session = p.build_session(
        "gpt-realtime",
        "marin",
        "instr",
        std::slice::from_ref(&tool),
    );
    assert_eq!(session["type"], "realtime");
    assert_eq!(session["model"], "gpt-realtime");
    assert_eq!(session["instructions"], "instr");
    assert_eq!(session["tools"], json!([tool]));
    assert_eq!(session["tool_choice"], "auto");
    assert_eq!(session["audio"]["output"]["voice"], "marin");

    let req = p.build_mint_request(&session);
    assert_eq!(req, json!({ "session": session }));
}

#[tokio::test]
async fn realtime_mint_without_key_makes_no_request() {
    let server = MockServer::start().await;
    let cfg = cfg_for(&server);
    let p = provider();
    let body = p.build_mint_request(&p.build_session("gpt-realtime", "marin", "i", &[]));
    let minted = p.mint(&cfg, None, &body).await.expect("keyless mint is Ok");
    assert_eq!(minted, MintedSession { token: None });
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "no key -> no request"
    );
}

#[tokio::test]
async fn realtime_mint_with_key_returns_token() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .and(header("authorization", "Bearer test-fake-key"))
        .and(body_partial_json(json!({"session": {"type": "realtime"}})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "ek_fake"})))
        .mount(&server)
        .await;
    let cfg = cfg_for(&server);
    let p = provider();
    let body = p.build_mint_request(&p.build_session("gpt-realtime", "marin", "i", &[]));
    let key = fake_key();
    let minted = p.mint(&cfg, Some(&key), &body).await.expect("mint ok");
    assert_eq!(minted.token.as_deref(), Some("ek_fake"));
    assert!(
        !format!("{minted:?}").contains("ek_fake"),
        "Debug must redact the token"
    );
}

#[tokio::test]
async fn realtime_mint_401_and_model_errors_map_to_categories() {
    let p = provider();
    let key = fake_key();
    let body = p.build_mint_request(&p.build_session("gpt-realtime", "marin", "i", &[]));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let err = p
        .mint(&cfg_for(&server), Some(&key), &body)
        .await
        .unwrap_err();
    assert_eq!(err.category, VoiceErrorCategory::KeyRejected);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({"error": {"message": "model not found", "param": "model"}})),
        )
        .mount(&server)
        .await;
    let err = p
        .mint(&cfg_for(&server), Some(&key), &body)
        .await
        .unwrap_err();
    assert_eq!(err.category, VoiceErrorCategory::ModelNotAvailable);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let err = p
        .mint(&cfg_for(&server), Some(&key), &body)
        .await
        .unwrap_err();
    assert_eq!(err.category, VoiceErrorCategory::UnexpectedResponse);
}

#[tokio::test]
async fn realtime_mint_redirect_is_not_followed() {
    let target = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": "ek_leak"})))
        .mount(&target)
        .await;

    let origin = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("{}/v1/realtime/client_secrets", target.uri()).as_str(),
        ))
        .mount(&origin)
        .await;

    let p = provider();
    let key = fake_key();
    let body = p.build_mint_request(&p.build_session("gpt-realtime", "marin", "i", &[]));
    let err = p
        .mint(&cfg_for(&origin), Some(&key), &body)
        .await
        .unwrap_err();
    assert_eq!(err.category, VoiceErrorCategory::UnexpectedResponse);
    assert_eq!(
        target.received_requests().await.unwrap().len(),
        0,
        "redirect target must receive zero requests"
    );
}

#[tokio::test]
async fn realtime_model_discovery_only_for_custom_base() {
    let p = provider();
    assert!(!p.should_discover_models(&RealtimeConfig::default()));

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [{"id": "my-realtime-model"}]
        })))
        .mount(&server)
        .await;
    let cfg = cfg_for(&server);
    assert!(p.should_discover_models(&cfg));
    assert_eq!(
        p.list_models(&cfg, None).await,
        DiscoveryOutcome::Loaded(vec!["my-realtime-model".to_string()])
    );
}

#[test]
fn realtime_provider_registry_has_one_entry() {
    let p = realtime_provider("openai_compatible").expect("registered");
    assert_eq!(p.id(), "openai_compatible");
    assert!(realtime_provider("gemini_live").is_none());
    assert!(realtime_provider("").is_none());
}

#[test]
fn realtime_curated_lists_match_the_ui_registry() {
    let p = provider();
    assert_eq!(
        p.curated_models(),
        [
            "gpt-realtime",
            "gpt-realtime-2",
            "gpt-realtime-2.1",
            "gpt-realtime-1.5"
        ]
    );
    assert_eq!(
        p.curated_voices(),
        [
            "alloy", "ash", "ballad", "cedar", "coral", "echo", "marin", "sage", "shimmer", "verse"
        ]
    );
    assert_eq!(
        p.curated_transcription_models(),
        [
            "gpt-4o-mini-transcribe",
            "gpt-4o-transcribe",
            "whisper-1",
            "off"
        ]
    );
}

/// IN-03: a 400/404 is "model not available" only when the structured error is
/// about the model -- not whenever the echoed message happens to contain "model".
#[tokio::test]
async fn realtime_mint_400_is_model_not_available_only_for_model_errors() {
    let p = provider();
    let key = fake_key();
    let body = p.build_mint_request(&p.build_session("gpt-realtime", "marin", "i", &[]));

    // An invalid VOICE whose message echoes the request (and says "model").
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/realtime/client_secrets"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "error": {
                "message": "Invalid voice 'nope' for session (model: gpt-realtime)",
                "param": "session.audio.output.voice",
                "code": "invalid_value"
            }
        })))
        .mount(&server)
        .await;
    let err = p
        .mint(&cfg_for(&server), Some(&key), &body)
        .await
        .unwrap_err();
    assert_ne!(err.category, VoiceErrorCategory::ModelNotAvailable);

    // The structured code or a session.model param still maps to the model.
    for error in [
        json!({"code": "model_not_found", "message": "x"}),
        json!({"param": "session.model", "message": "x"}),
        // A minimal server that sends only a message keeps the text fallback.
        json!({"message": "The model `gpt-realtime` does not exist"}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/realtime/client_secrets"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": error})))
            .mount(&server)
            .await;
        let err = p
            .mint(&cfg_for(&server), Some(&key), &body)
            .await
            .unwrap_err();
        assert_eq!(err.category, VoiceErrorCategory::ModelNotAvailable);
    }
}
