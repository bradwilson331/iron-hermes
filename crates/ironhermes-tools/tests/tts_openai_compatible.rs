//! Phase 54.7 Plan 03 — `openai_compatible` TTS provider integration tests.
//!
//! The operator's Kokoro server speaks the OpenAI `/audio/speech` shape. These
//! tests drive the agent `text_to_speech` tool end-to-end against a wiremock
//! server. Fixtures use obviously fake values and wiremock's own 127.0.0.1 URI.
//!
//! Run: `cargo test -p ironhermes-tools --test tts_openai_compatible -- --test-threads=1`

use std::sync::Arc;

use ironhermes_core::Config;
use ironhermes_tools::registry::Tool;
use ironhermes_tools::tts::build_tts_registry;
use ironhermes_tools::tts_tool::TextToSpeechTool;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// A short non-empty byte string with an ID3 prefix, standing in for MP3 audio.
const FAKE_MP3: &[u8] = b"ID3\x03\x00\x00\x00\x00\x00\x00fake-kokoro-mp3-bytes";

fn compatible_config(base_url: String) -> Config {
    let mut config = Config::default();
    config.tts.provider = "openai_compatible".to_string();
    config.tts.openai_compatible.base_url = base_url;
    config.tts.openai_compatible.model = "mlx-community/Kokoro-82M-bf16".to_string();
    config.tts.openai_compatible.voice = "af_heart".to_string();
    config
}

#[tokio::test]
async fn oc_tts_tool_synthesizes_through_compatible_server() {
    let _lock = env_lock().lock().await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/speech"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_MP3))
        .mount(&server)
        .await;

    let tmpdir = TempDir::new().expect("create tempdir");
    // SAFETY: env_lock() serialises IRONHERMES_HOME mutations across tests.
    unsafe { std::env::set_var("IRONHERMES_HOME", tmpdir.path()) };

    let config = Arc::new(compatible_config(format!("{}/v1", server.uri())));
    let registry = build_tts_registry(&config.tts);
    let tool = TextToSpeechTool::new_with_registry(config, registry);

    let result = tool.execute(serde_json::json!({"text": "hi"})).await;

    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    let written = result.expect("tool must synthesize through the compatible server");
    let written = std::path::PathBuf::from(written);
    assert!(
        written.starts_with(tmpdir.path().join("audio_cache")),
        "output must land inside audio_cache: {written:?}"
    );
    assert_eq!(
        std::fs::read(&written).expect("audio file readable"),
        FAKE_MP3,
        "written bytes must equal the mock body"
    );

    let requests = server.received_requests().await.expect("recording enabled");
    assert_eq!(requests.len(), 1, "exactly one request must be sent");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("JSON body");
    assert_eq!(body["model"], "mlx-community/Kokoro-82M-bf16");
    assert_eq!(body["voice"], "af_heart");
    assert_eq!(body["input"], "hi");
    assert_eq!(body["response_format"], "mp3");
}

// ---------------------------------------------------------------------------
// Task 3 — key-reference, no-redirect and error-category behaviour
// ---------------------------------------------------------------------------

use ironhermes_core::tts::TtsProvider;
use ironhermes_core::voice_http::{VoiceErrorCategory, VoiceHttpError};

/// Sets environment variables for one test and restores the prior values on drop.
/// Callers hold `env_lock()` for the guard's whole life.
struct EnvGuard {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvGuard {
    fn new() -> Self {
        Self { saved: Vec::new() }
    }

    fn set(&mut self, key: &'static str, value: &str) {
        self.saved.push((key, std::env::var(key).ok()));
        // SAFETY: env_lock() serialises every environment mutation in this binary.
        unsafe { std::env::set_var(key, value) };
    }

    fn unset(&mut self, key: &'static str) {
        self.saved.push((key, std::env::var(key).ok()));
        // SAFETY: env_lock() serialises every environment mutation in this binary.
        unsafe { std::env::remove_var(key) };
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, prior) in self.saved.drain(..).rev() {
            // SAFETY: env_lock() is still held by the test that owns this guard.
            unsafe {
                match prior {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

/// Build the configured provider through the registry (the production seam).
fn provider_for(config: &Config) -> Arc<dyn TtsProvider> {
    build_tts_registry(&config.tts)
        .get("openai_compatible")
        .expect("openai_compatible is registered")
}

async fn mount_speech_ok(server: &MockServer, route: &str) {
    Mock::given(method("POST"))
        .and(path(route))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_MP3))
        .mount(server)
        .await;
}

fn category_of(err: &anyhow::Error) -> VoiceErrorCategory {
    err.downcast_ref::<VoiceHttpError>()
        .unwrap_or_else(|| panic!("error must downcast to VoiceHttpError: {err:#}"))
        .category
}

#[tokio::test]
async fn oc_tts_no_key_sends_no_authorization_even_with_openai_key_set() {
    let _lock = env_lock().lock().await;
    let mut env = EnvGuard::new();
    env.set("OPENAI_API_KEY", "test-fake-key");
    env.set("VOICE_TOOLS_OPENAI_KEY", "test-fake-key");

    let server = MockServer::start().await;
    mount_speech_ok(&server, "/audio/speech").await;
    let config = compatible_config(server.uri());
    assert!(config.tts.openai_compatible.api_key_ref.is_none());

    let dir = TempDir::new().unwrap();
    provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect("keyless synthesis succeeds");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "no api_key_ref -> no Authorization header, even with OPENAI_API_KEY set (D-02)"
    );
}

#[tokio::test]
async fn oc_tts_env_key_ref_sends_bearer() {
    let _lock = env_lock().lock().await;
    let mut env = EnvGuard::new();
    env.set("VOICE_KEY_IH_TEST_547_TTS", "test-fake-key");

    let server = MockServer::start().await;
    mount_speech_ok(&server, "/audio/speech").await;
    let mut config = compatible_config(server.uri());
    config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_TTS".to_string());

    let dir = TempDir::new().unwrap();
    provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect("keyed synthesis succeeds");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok()),
        Some("Bearer test-fake-key")
    );
}

#[tokio::test]
async fn oc_tts_unrelated_secret_ref_is_never_sent() {
    let _lock = env_lock().lock().await;
    let mut env = EnvGuard::new();
    env.set("OPENAI_API_KEY", "test-fake-key");

    let server = MockServer::start().await;
    mount_speech_ok(&server, "/audio/speech").await;
    // Built directly, bypassing the web validator, as a hand-edited config.yaml would.
    let mut config = compatible_config(server.uri());
    config.tts.openai_compatible.api_key_ref = Some("OPENAI_API_KEY".to_string());

    let dir = TempDir::new().unwrap();
    let err = provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect_err("an out-of-namespace reference must not resolve (T-54.7-70)");

    assert_eq!(category_of(&err), VoiceErrorCategory::KeyRejected);
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "the server must receive zero requests"
    );
}

#[tokio::test]
async fn oc_tts_unresolved_key_ref_fails_before_sending() {
    let _lock = env_lock().lock().await;
    let mut env = EnvGuard::new();
    env.unset("VOICE_KEY_IH_TEST_547_TTS_UNSET");

    let server = MockServer::start().await;
    mount_speech_ok(&server, "/audio/speech").await;
    let mut config = compatible_config(server.uri());
    config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_TTS_UNSET".to_string());

    let dir = TempDir::new().unwrap();
    let err = provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect_err("an unresolved reference must fail");

    assert_eq!(category_of(&err), VoiceErrorCategory::KeyRejected);
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn oc_tts_redirect_is_not_followed() {
    let _lock = env_lock().lock().await;
    let target = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_MP3))
        .mount(&target)
        .await;

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("{}/elsewhere", target.uri())),
        )
        .mount(&server)
        .await;
    let config = compatible_config(server.uri());

    let dir = TempDir::new().unwrap();
    let err = provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect_err("a 302 must surface as an error, not be followed");

    assert_eq!(category_of(&err), VoiceErrorCategory::UnexpectedResponse);
    assert!(
        target.received_requests().await.unwrap().is_empty(),
        "the redirect target must receive zero requests (T-54.7-11)"
    );
}

#[tokio::test]
async fn oc_tts_401_maps_to_key_rejected() {
    let _lock = env_lock().lock().await;
    let mut env = EnvGuard::new();
    env.set("VOICE_KEY_IH_TEST_547_TTS", "test-fake-key");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/speech"))
        .respond_with(ResponseTemplate::new(401).set_body_string("body-marker-do-not-leak"))
        .mount(&server)
        .await;
    let mut config = compatible_config(server.uri());
    config.tts.openai_compatible.api_key_ref = Some("VOICE_KEY_IH_TEST_547_TTS".to_string());

    let dir = TempDir::new().unwrap();
    let err = provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect_err("401 must fail");

    assert_eq!(category_of(&err), VoiceErrorCategory::KeyRejected);
    for rendered in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
        assert!(
            !rendered.contains("test-fake-key"),
            "key leaked: {rendered}"
        );
        assert!(
            !rendered.contains("body-marker-do-not-leak"),
            "response body leaked: {rendered}"
        );
    }
}

#[tokio::test]
async fn oc_tts_path_override_is_used() {
    let _lock = env_lock().lock().await;
    let server = MockServer::start().await;
    mount_speech_ok(&server, "/tts").await;
    let mut config = compatible_config(server.uri());
    config.tts.openai_compatible.path = Some("/tts".to_string());

    let dir = TempDir::new().unwrap();
    provider_for(&config)
        .synthesize("hi", &dir.path().join("out.mp3"))
        .await
        .expect("synthesis via the path override succeeds");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/tts");
}
