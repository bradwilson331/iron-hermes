//! Phase 36.17.8 — STT tools integration tests.
//!
//! Covers D-03 (build_stt_registry), D-14 (mocked provider transcription).
//! Uses wiremock to mock the Whisper API endpoint — no live network calls in CI.
//!
//! Run: `cargo test -p ironhermes-tools --test stt_tools`
//!
//! All set_var/remove_var calls wrapped in `unsafe` (Rust 2024 edition).
//! Async tests that mutate env vars acquire `ENV_LOCK` to prevent interference.

use ironhermes_core::SttProvider;
use ironhermes_core::config::{GroqSttConfig, SttConfig};
use ironhermes_tools::stt::{GroqSttProvider, build_stt_registry};
use tempfile::NamedTempFile;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ─── Env-mutation guard ───────────────────────────────────────────────────────

/// Process-global mutex that serialises tests that mutate env vars.
/// Mirrors the pattern in cronjob_tool_default_deliver.rs and browser_prereq.rs.
fn env_lock() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// ─── WAV fixture helper ───────────────────────────────────────────────────────

/// Write a minimal valid WAV file (16 kHz, mono, 16-bit, 100 ms silence) into
/// a temp file and return the file handle. Uses `hound` (workspace dev-dep).
fn make_wav_fixture() -> NamedTempFile {
    let file = NamedTempFile::new().expect("tempfile");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(file.path(), spec).expect("hound writer");
    // 100 ms of silence at 16 kHz = 1600 samples.
    for _ in 0..1600i16 {
        writer.write_sample(0i16).expect("write sample");
    }
    writer.finalize().expect("finalize wav");
    file
}

// ─── D-03: build_stt_registry ─────────────────────────────────────────────────

/// D-03: `build_stt_registry()` registers both groq and openai built-in providers.
#[test]
fn test_build_stt_registry_registers_builtins() {
    let config = SttConfig::default();
    let registry = build_stt_registry(&config);

    assert!(
        registry.get("groq").is_some(),
        "registry should contain 'groq' provider"
    );
    assert!(
        registry.get("openai").is_some(),
        "registry should contain 'openai' provider"
    );

    // Case-insensitive lookup (SttRegistry contract).
    assert!(
        registry.get("GROQ").is_some(),
        "case-insensitive 'GROQ' lookup"
    );
    assert!(
        registry.get("OpenAI").is_some(),
        "case-insensitive 'OpenAI' lookup"
    );

    // Names are correct.
    assert_eq!(registry.get("groq").unwrap().name(), "groq");
    assert_eq!(registry.get("openai").unwrap().name(), "openai");
}

// ─── D-14: Transcription with wiremock ────────────────────────────────────────

/// D-14: Transcription with a mock provider (WAV fixture, wiremock HTTP mock).
/// Verifies the STT pipeline end-to-end without a live network call in CI.
#[tokio::test]
async fn test_transcribe_with_mock_provider() {
    let _guard = env_lock().lock().await;

    // Start wiremock server.
    let server = MockServer::start().await;

    // Mount a mock that returns {"text":"hello world"} for any multipart POST.
    Mock::given(method("POST"))
        .and(path("/openai/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "hello world"})),
        )
        .mount(&server)
        .await;

    // Point provider at mock server via GROQ_BASE_URL.
    let orig_base = std::env::var("GROQ_BASE_URL").ok();
    let orig_key = std::env::var("GROQ_API_KEY").ok();

    unsafe {
        std::env::set_var("GROQ_BASE_URL", server.uri());
        std::env::set_var("GROQ_API_KEY", "mock-key");
    }

    // Generate WAV fixture.
    let wav = make_wav_fixture();

    let provider = GroqSttProvider::new(GroqSttConfig::default());
    let result = provider.transcribe(wav.path()).await;

    // Restore env.
    unsafe {
        match orig_base {
            Some(v) => std::env::set_var("GROQ_BASE_URL", v),
            None => std::env::remove_var("GROQ_BASE_URL"),
        }
        match orig_key {
            Some(v) => std::env::set_var("GROQ_API_KEY", v),
            None => std::env::remove_var("GROQ_API_KEY"),
        }
    }

    assert!(result.is_ok(), "transcribe should succeed: {:?}", result);
    assert_eq!(result.unwrap(), "hello world");
}

/// Error path: a 401 response from the mock returns Err containing status + body.
#[tokio::test]
async fn test_transcribe_error_on_401() {
    let _guard = env_lock().lock().await;

    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/openai/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(serde_json::json!({"error": "Unauthorized"})),
        )
        .mount(&server)
        .await;

    let orig_base = std::env::var("GROQ_BASE_URL").ok();
    let orig_key = std::env::var("GROQ_API_KEY").ok();

    unsafe {
        std::env::set_var("GROQ_BASE_URL", server.uri());
        std::env::set_var("GROQ_API_KEY", "bad-key");
    }

    let wav = make_wav_fixture();
    let provider = GroqSttProvider::new(GroqSttConfig::default());
    let result = provider.transcribe(wav.path()).await;

    unsafe {
        match orig_base {
            Some(v) => std::env::set_var("GROQ_BASE_URL", v),
            None => std::env::remove_var("GROQ_BASE_URL"),
        }
        match orig_key {
            Some(v) => std::env::set_var("GROQ_API_KEY", v),
            None => std::env::remove_var("GROQ_API_KEY"),
        }
    }

    assert!(result.is_err(), "401 should return Err");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("401"),
        "error message should contain '401': {err_msg}"
    );
}

/// D-05: STT_GROQ_MODEL env override sends the overridden model in the multipart body.
#[tokio::test]
async fn test_groq_transcribe_model_override() {
    let _guard = env_lock().lock().await;

    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/openai/v1/audio/transcriptions"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"text": "override test"})),
        )
        .mount(&server)
        .await;

    let orig_base = std::env::var("GROQ_BASE_URL").ok();
    let orig_key = std::env::var("GROQ_API_KEY").ok();
    let orig_model = std::env::var("STT_GROQ_MODEL").ok();

    unsafe {
        std::env::set_var("GROQ_BASE_URL", server.uri());
        std::env::set_var("GROQ_API_KEY", "mock-key");
        std::env::set_var("STT_GROQ_MODEL", "whisper-large-v3");
    }

    let wav = make_wav_fixture();
    let provider = GroqSttProvider::new(GroqSttConfig::default());
    let result = provider.transcribe(wav.path()).await;

    // Restore.
    unsafe {
        match orig_base {
            Some(v) => std::env::set_var("GROQ_BASE_URL", v),
            None => std::env::remove_var("GROQ_BASE_URL"),
        }
        match orig_key {
            Some(v) => std::env::set_var("GROQ_API_KEY", v),
            None => std::env::remove_var("GROQ_API_KEY"),
        }
        match orig_model {
            Some(v) => std::env::set_var("STT_GROQ_MODEL", v),
            None => std::env::remove_var("STT_GROQ_MODEL"),
        }
    }

    assert!(
        result.is_ok(),
        "transcribe with model override should succeed: {:?}",
        result
    );
    assert_eq!(result.unwrap(), "override test");

    // Verify the request body contained the overridden model name.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one request");
    let body_str = String::from_utf8_lossy(&requests[0].body);
    assert!(
        body_str.contains("whisper-large-v3"),
        "request body should contain overridden model 'whisper-large-v3': {body_str}"
    );
}
