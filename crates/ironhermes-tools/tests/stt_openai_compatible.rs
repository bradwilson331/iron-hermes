//! Phase 54.7 Plan 04 — `openai_compatible` STT provider (whisper.cpp shape).
//!
//! The mock mirrors the operator's whisper.cpp server captured 2026-10-16:
//! `POST /inference` (multipart file, model, response_format=json) answers
//! `200 {"text":" Hi.\n"}` and `/v1/audio/transcriptions` is a 404.
//!
//! Run: `cargo test -p ironhermes-tools --test stt_openai_compatible -- --test-threads=1`
//!
//! Env-mutating tests serialise on `env_lock()` and restore every variable they
//! touch (Rust 2024 `unsafe` set_var/remove_var). Fixtures use fake values only.

use ironhermes_core::config::{OpenAiCompatibleSttConfig, SttConfig};
use ironhermes_tools::stt::build_stt_registry;
use std::path::PathBuf;
use std::sync::OnceLock;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─── Env-mutation guard ───────────────────────────────────────────────────────

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Restores the named environment variables when dropped, even on panic.
struct EnvRestore {
    saved: Vec<(&'static str, Option<String>)>,
}

impl EnvRestore {
    fn capture(names: &[&'static str]) -> Self {
        Self {
            saved: names.iter().map(|n| (*n, std::env::var(n).ok())).collect(),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        for (name, value) in &self.saved {
            // SAFETY: callers hold env_lock(); restoring the prior value.
            unsafe {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

// ─── Fixtures ────────────────────────────────────────────────────────────────

/// A tiny fake WAV (header-shaped bytes; the mock never decodes it).
fn fake_wav(dir: &TempDir) -> PathBuf {
    let p = dir.path().join("clip.wav");
    let mut bytes = b"RIFF\x24\x00\x00\x00WAVEfmt ".to_vec();
    bytes.extend_from_slice(&[0u8; 32]);
    std::fs::write(&p, bytes).expect("write fake wav");
    p
}

fn oc_config(base_url: &str, path: Option<&str>, model: &str) -> SttConfig {
    SttConfig {
        provider: "openai_compatible".to_string(),
        openai_compatible: OpenAiCompatibleSttConfig {
            base_url: base_url.to_string(),
            path: path.map(str::to_string),
            api_key_ref: None,
            model: model.to_string(),
        },
        ..SttConfig::default()
    }
}

fn whisper_cpp_ok() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string("{\"text\":\" Hi.\\n\"}")
}

// ─── Task 1 — tracer ─────────────────────────────────────────────────────────

/// D-03: base_url `{mock}` + `path: /inference` POSTs multipart to /inference and
/// the server's `" Hi.\n"` comes back trimmed.
#[tokio::test]
async fn oc_stt_transcribes_via_inference_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(whisper_cpp_ok())
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");
    let registry = build_stt_registry(&cfg);
    let provider = registry
        .get("openai_compatible")
        .expect("openai_compatible must be a registered STT provider");

    let text = provider.transcribe(&wav).await.expect("transcribe");
    assert_eq!(text, "Hi.");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    let ctype = req
        .headers
        .get("content-type")
        .map(|v| v.to_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(ctype.starts_with("multipart/form-data"), "{ctype}");
    let body = String::from_utf8_lossy(&req.body).to_string();
    assert!(body.contains("name=\"file\""), "missing file part");
    assert!(body.contains("name=\"model\""), "missing model part");
    assert!(body.contains("whisper-large-v3"), "missing model value");
    assert!(
        body.contains("name=\"response_format\""),
        "missing response_format part"
    );
}

/// D-03: no `path` -> the request lands on `/audio/transcriptions`.
#[tokio::test]
async fn oc_stt_default_path_is_audio_transcriptions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(whisper_cpp_ok())
        .expect(1)
        .mount(&server)
        .await;

    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = oc_config(&server.uri(), None, "whisper-1");
    let registry = build_stt_registry(&cfg);
    let provider = registry
        .get("openai_compatible")
        .expect("openai_compatible must be a registered STT provider");

    let text = provider.transcribe(&wav).await.expect("transcribe");
    assert_eq!(text, "Hi.");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// ─── Task 2 — clip transcriber ───────────────────────────────────────────────

fn cache_is_empty(dir: &TempDir) -> bool {
    std::fs::read_dir(dir.path())
        .map(|mut it| it.next().is_none())
        .unwrap_or(true)
}

/// D-17/D-19: the one clip transcriber selects the provider, writes UUID temp
/// files, transcodes (or passes through without ffmpeg), transcribes, trims, and
/// leaves nothing behind in the cache dir. Passes with or without ffmpeg.
#[tokio::test]
async fn clip_transcribes_with_openai_compatible_and_cleans_temp_files() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(whisper_cpp_ok())
        .expect(1)
        .mount(&server)
        .await;

    let cache = TempDir::new().unwrap();
    let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");
    let text = ironhermes_tools::stt::clip::transcribe_clip(
        b"not-really-webm",
        "audio/webm",
        &cfg,
        None,
        cache.path(),
    )
    .await
    .expect("clip transcribes");

    assert_eq!(text, "Hi.");
    assert!(cache_is_empty(&cache), "temp files must be removed");
}

/// T-54.7-17: a transcription error still removes both temp files.
#[tokio::test]
async fn clip_cleans_temp_files_on_transcription_error() {
    use ironhermes_core::voice_http::VoiceErrorCategory;
    use ironhermes_tools::stt::clip::{ClipError, transcribe_clip};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal secret detail"))
        .mount(&server)
        .await;

    let cache = TempDir::new().unwrap();
    let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");
    let err = transcribe_clip(b"not-really-webm", "audio/webm", &cfg, None, cache.path())
        .await
        .expect_err("a 500 must surface as an error");

    assert!(
        matches!(
            err,
            ClipError::Transcribe(VoiceErrorCategory::UnexpectedResponse)
        ),
        "{err:?}"
    );
    assert!(
        !err.to_string().contains("internal secret detail"),
        "error text must not carry the response body"
    );
    assert!(
        cache_is_empty(&cache),
        "temp files must be removed on error"
    );
}

// ─── Task 3 — key, redirect and error behaviour ──────────────────────────────

use ironhermes_core::voice_http::{VoiceErrorCategory, VoiceHttpError};

const PROVIDER_KEY_VARS: [&str; 3] = ["GROQ_API_KEY", "VOICE_TOOLS_OPENAI_KEY", "OPENAI_API_KEY"];

fn category_of(err: &anyhow::Error) -> Option<VoiceErrorCategory> {
    err.downcast_ref::<VoiceHttpError>().map(|e| e.category)
}

fn header_value(req: &wiremock::Request, name: &str) -> Option<String> {
    req.headers
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

/// Build a registry from `cfg` and transcribe `wav` through openai_compatible.
async fn run_oc(cfg: &SttConfig, wav: &std::path::Path) -> anyhow::Result<String> {
    let registry = build_stt_registry(cfg);
    registry
        .get("openai_compatible")
        .expect("openai_compatible registered")
        .transcribe(wav)
        .await
}

fn with_key_ref(mut cfg: SttConfig, key_ref: &str) -> SttConfig {
    cfg.openai_compatible.api_key_ref = Some(key_ref.to_string());
    cfg
}

/// D-02 / T-54.7-15: with no `api_key_ref`, no Authorization header goes out even
/// when every built-in provider key variable is set.
#[tokio::test]
async fn oc_stt_no_key_sends_no_authorization_even_with_openai_keys_set() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&PROVIDER_KEY_VARS);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe {
        for name in PROVIDER_KEY_VARS {
            std::env::set_var(name, "test-fake-key");
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(whisper_cpp_ok())
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");

    assert_eq!(run_oc(&cfg, &wav).await.unwrap(), "Hi.");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].headers.get("authorization").is_none(),
        "no Authorization header may be sent without api_key_ref"
    );
}

/// D-02: a `VOICE_KEY_*` reference resolves to `Authorization: Bearer <value>`.
#[tokio::test]
async fn oc_stt_env_key_ref_sends_bearer() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&["VOICE_KEY_IH_TEST_547_STT"]);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe { std::env::set_var("VOICE_KEY_IH_TEST_547_STT", "test-fake-key") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(whisper_cpp_ok())
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = with_key_ref(
        oc_config(&server.uri(), Some("/inference"), "whisper-large-v3"),
        "VOICE_KEY_IH_TEST_547_STT",
    );

    assert_eq!(run_oc(&cfg, &wav).await.unwrap(), "Hi.");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        header_value(&requests[0], "authorization").as_deref(),
        Some("Bearer test-fake-key")
    );
}

/// D-02: an unresolvable reference fails key-rejected before any request.
#[tokio::test]
async fn oc_stt_unresolved_key_ref_fails_before_sending() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&["VOICE_KEY_IH_TEST_547_STT_UNSET"]);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe { std::env::remove_var("VOICE_KEY_IH_TEST_547_STT_UNSET") };

    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = with_key_ref(
        oc_config(&server.uri(), Some("/inference"), "whisper-large-v3"),
        "VOICE_KEY_IH_TEST_547_STT_UNSET",
    );

    let err = run_oc(&cfg, &wav).await.unwrap_err();
    assert_eq!(category_of(&err), Some(VoiceErrorCategory::KeyRejected));
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "nothing may be sent for an unresolvable key reference"
    );
}

/// D-02 / T-54.7-70 (sink re-check): a hand-edited config naming an
/// out-of-namespace secret never sends it, even though it is set.
#[tokio::test]
async fn oc_stt_unrelated_secret_ref_is_never_sent() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&["GROQ_API_KEY"]);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe { std::env::set_var("GROQ_API_KEY", "test-fake-key") };

    let server = MockServer::start().await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    // Built directly, bypassing the web validator.
    let cfg = with_key_ref(
        oc_config(&server.uri(), Some("/inference"), "whisper-large-v3"),
        "GROQ_API_KEY",
    );

    let err = run_oc(&cfg, &wav).await.unwrap_err();
    assert_eq!(category_of(&err), Some(VoiceErrorCategory::KeyRejected));
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "an out-of-namespace secret must never reach the wire"
    );
}

/// D-04 / T-54.7-16: a 302 is not followed — the target sees zero requests.
#[tokio::test]
async fn oc_stt_redirect_is_not_followed() {
    let target = MockServer::start().await;
    let origin = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/landing", target.uri()).as_str()),
        )
        .mount(&origin)
        .await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = oc_config(&origin.uri(), Some("/inference"), "whisper-large-v3");

    let err = run_oc(&cfg, &wav).await.unwrap_err();
    assert_eq!(
        category_of(&err),
        Some(VoiceErrorCategory::UnexpectedResponse)
    );
    assert!(
        target.received_requests().await.unwrap().is_empty(),
        "the redirect target must receive zero requests"
    );
}

/// D-04: 401 maps to key-rejected and the error text leaks neither key nor body.
#[tokio::test]
async fn oc_stt_401_maps_to_key_rejected() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&["VOICE_KEY_IH_TEST_547_STT"]);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe { std::env::set_var("VOICE_KEY_IH_TEST_547_STT", "test-fake-key") };

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(ResponseTemplate::new(401).set_body_string("rejected-body-marker"))
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = with_key_ref(
        oc_config(&server.uri(), Some("/inference"), "whisper-large-v3"),
        "VOICE_KEY_IH_TEST_547_STT",
    );

    let err = run_oc(&cfg, &wav).await.unwrap_err();
    assert_eq!(category_of(&err), Some(VoiceErrorCategory::KeyRejected));
    for text in [format!("{err}"), format!("{err:#}"), format!("{err:?}")] {
        assert!(!text.contains("test-fake-key"), "key leaked: {text}");
        assert!(
            !text.contains("rejected-body-marker"),
            "response body leaked: {text}"
        );
    }
}

/// D-04: a refused connection maps to server-unreachable.
#[tokio::test]
async fn oc_stt_connect_failure_maps_to_server_unreachable() {
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    // Port 1 on loopback is not listening.
    let cfg = oc_config("http://127.0.0.1:1", Some("/inference"), "whisper-large-v3");

    let err = run_oc(&cfg, &wav).await.unwrap_err();
    assert_eq!(
        category_of(&err),
        Some(VoiceErrorCategory::ServerUnreachable)
    );
}

/// Pitfall 14: no env override changes the openai_compatible model.
#[tokio::test]
async fn oc_stt_model_ignores_env_overrides() {
    let _guard = env_lock().lock().await;
    let _restore = EnvRestore::capture(&["STT_OPENAI_MODEL", "STT_GROQ_MODEL"]);
    // SAFETY: serialised by env_lock(); restored by EnvRestore.
    unsafe {
        std::env::set_var("STT_OPENAI_MODEL", "env-openai-model");
        std::env::set_var("STT_GROQ_MODEL", "env-groq-model");
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/inference"))
        .respond_with(whisper_cpp_ok())
        .mount(&server)
        .await;
    let dir = TempDir::new().unwrap();
    let wav = fake_wav(&dir);
    let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");

    assert_eq!(run_oc(&cfg, &wav).await.unwrap(), "Hi.");
    let requests = server.received_requests().await.unwrap();
    let body = String::from_utf8_lossy(&requests[0].body).to_string();
    assert!(
        body.contains("whisper-large-v3"),
        "configured model missing"
    );
    assert!(!body.contains("env-openai-model"));
    assert!(!body.contains("env-groq-model"));
}

/// IN-08: the multipart part is named and typed for the REAL container, so a
/// passthrough webm/mp4/ogg clip is not labelled `audio.wav` / `audio/wav`.
#[tokio::test]
async fn oc_stt_labels_the_real_container() {
    for (file, name, mime) in [
        ("clip.webm", "audio.webm", "audio/webm"),
        ("clip.mp4", "audio.mp4", "audio/mp4"),
        ("clip.ogg", "audio.ogg", "audio/ogg"),
        ("clip.wav", "audio.wav", "audio/wav"),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/inference"))
            .respond_with(whisper_cpp_ok())
            .expect(1)
            .mount(&server)
            .await;
        let dir = TempDir::new().unwrap();
        let clip = dir.path().join(file);
        std::fs::write(&clip, b"fake container bytes").unwrap();
        let cfg = oc_config(&server.uri(), Some("/inference"), "whisper-large-v3");
        let registry = build_stt_registry(&cfg);
        let provider = registry.get("openai_compatible").expect("provider");
        provider.transcribe(&clip).await.expect("transcribe");

        let requests = server.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&requests[0].body).to_string();
        assert!(
            body.contains(&format!("filename=\"{name}\"")),
            "{file}: part must be named {name}: {body}"
        );
        assert!(
            body.contains(&format!("Content-Type: {mime}")),
            "{file}: part must be typed {mime}: {body}"
        );
        if name != "audio.wav" {
            assert!(!body.contains("audio.wav"), "{file}: must not claim WAV");
        }
    }
}
