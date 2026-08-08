//! Phase 36.17.5 — Wave 0/2/3 TTS tool tests. Owner plans un-ignore each test as wiring lands:
//!   TTS-03 / TTS-04 / TTS-09 → PLAN 02 (Edge / ElevenLabs / ffmpeg probe) ← UN-IGNORED
//!   TTS-05 / TTS-06         → PLAN 03 (text_to_speech tool) ← UN-IGNORED HERE
//!   TTS-10                  → PLAN 04 (live network integration; runs only with --ignored)
//!
//! Run: `cargo test -p ironhermes-tools --test tts_tools`

fn env_lock() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

// TTS-03 (un-ignored by PLAN 02): EdgeProvider::is_available() always returns true (D-03).
#[test]
fn test_edge_provider_available() {
    use ironhermes_core::config::EdgeTtsConfig;
    use ironhermes_core::tts::TtsProvider;
    use ironhermes_tools::tts::EdgeProvider;

    let provider = EdgeProvider::new(EdgeTtsConfig::default());
    assert!(
        provider.is_available(),
        "EdgeProvider must always be available (D-03 — no API key required)"
    );
}

// TTS-04 (un-ignored by PLAN 02): ElevenLabsProvider::is_available() reflects ELEVENLABS_API_KEY.
#[test]
fn test_elevenlabs_unavailable_no_key() {
    use ironhermes_core::config::ElevenLabsConfig;
    use ironhermes_core::tts::TtsProvider;
    use ironhermes_tools::tts::ElevenLabsProvider;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let _lock = rt.block_on(env_lock().lock());

    // SAFETY: env_lock() serializes all env-var mutations across tests in this
    // binary, so no other thread reads ELEVENLABS_API_KEY concurrently.
    unsafe { std::env::remove_var("ELEVENLABS_API_KEY") };
    let provider = ElevenLabsProvider::new(ElevenLabsConfig::default());
    assert!(
        !provider.is_available(),
        "ElevenLabsProvider must be unavailable when ELEVENLABS_API_KEY is unset"
    );

    unsafe { std::env::set_var("ELEVENLABS_API_KEY", "test-key") };
    assert!(
        provider.is_available(),
        "ElevenLabsProvider must be available when ELEVENLABS_API_KEY is set"
    );

    unsafe { std::env::remove_var("ELEVENLABS_API_KEY") };
}

// TTS-05 (un-ignored by PLAN 03): TextToSpeechTool metadata — D-05 / D-06 / D-07.
#[test]
fn test_tts_tool_metadata() {
    use ironhermes_core::Config;
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::tts_tool::TextToSpeechTool;
    use std::sync::Arc;

    let config = Arc::new(Config::default());
    let registry = ironhermes_core::tts::TtsRegistry::new();
    let tool = TextToSpeechTool::new_with_registry(config, registry);

    assert_eq!(tool.name(), "text_to_speech");
    assert_eq!(tool.toolset(), "voice");

    let schema = tool.schema();
    let props = schema
        .function
        .parameters
        .get("properties")
        .expect("schema must have properties");
    assert!(
        props.get("text").is_some(),
        "schema must have 'text' property (D-07)"
    );
    assert!(
        props.get("output_path").is_some(),
        "schema must have 'output_path' property (D-07)"
    );

    let required = schema
        .function
        .parameters
        .get("required")
        .and_then(|r: &serde_json::Value| r.as_array())
        .expect("schema must have 'required' array");
    assert!(
        required
            .iter()
            .any(|v: &serde_json::Value| v.as_str() == Some("text")),
        "'text' must be in required (D-07)"
    );
    assert!(
        !required
            .iter()
            .any(|v: &serde_json::Value| v.as_str() == Some("output_path")),
        "'output_path' must NOT be required (D-07 — optional)"
    );
}

// TTS-06 (un-ignored by PLAN 03): TextToSpeechTool creates audio_cache dir (D-16).
// Uses FakeProvider via new_with_registry to avoid live network.
#[tokio::test]
async fn test_tts_tool_creates_audio_cache_dir() {
    use async_trait::async_trait;
    use ironhermes_core::Config;
    use ironhermes_core::tts::{TtsProvider, TtsRegistry};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::tts_tool::TextToSpeechTool;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    // FakeProvider: returns Ok(output_path.clone()) without network I/O.
    struct FakeProvider;

    #[async_trait]
    impl TtsProvider for FakeProvider {
        fn name(&self) -> &str {
            "edge"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn synthesize(
            &self,
            _text: &str,
            output_path: &std::path::Path,
        ) -> anyhow::Result<PathBuf> {
            // Write a tiny stub file so the path exists after "synthesis"
            std::fs::write(output_path, b"fake-audio")?;
            Ok(output_path.to_path_buf())
        }
    }

    let _lock = env_lock().lock().await;
    let tmpdir = TempDir::new().expect("create tempdir");

    // SAFETY: env_lock() serializes IRONHERMES_HOME mutations across tests.
    unsafe { std::env::set_var("IRONHERMES_HOME", tmpdir.path()) };

    let mut registry = TtsRegistry::new();
    registry.register(Arc::new(FakeProvider));

    let config = Arc::new(Config::default());
    let tool = TextToSpeechTool::new_with_registry(config, registry);

    let result: anyhow::Result<String> = tool.execute(serde_json::json!({"text": "x"})).await;

    // D-16: audio_cache must exist regardless of synth result
    let audio_cache = tmpdir.path().join("audio_cache");
    assert!(
        audio_cache.exists(),
        "audio_cache dir must be created by execute() (D-16): {:?}",
        audio_cache
    );

    assert!(
        result.is_ok(),
        "FakeProvider synthesize should succeed: {:?}",
        result
    );

    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}

// Regression (debug: web-tts-zero-byte-audio): a provider that "succeeds" but
// writes a 0-byte file must NOT be reported as success — execute() must bail so
// the empty/corrupt audio never reaches the web client.
#[tokio::test]
async fn test_tts_tool_rejects_zero_byte_output() {
    use async_trait::async_trait;
    use ironhermes_core::Config;
    use ironhermes_core::tts::{TtsProvider, TtsRegistry};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::tts_tool::TextToSpeechTool;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    // EmptyProvider: mimics Edge TTS returning Ok with empty audio_bytes —
    // writes a 0-byte file and reports success.
    struct EmptyProvider;

    #[async_trait]
    impl TtsProvider for EmptyProvider {
        fn name(&self) -> &str {
            "edge"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn synthesize(
            &self,
            _text: &str,
            output_path: &std::path::Path,
        ) -> anyhow::Result<PathBuf> {
            std::fs::write(output_path, b"")?;
            Ok(output_path.to_path_buf())
        }
    }

    let _lock = env_lock().lock().await;
    let tmpdir = TempDir::new().expect("create tempdir");

    // SAFETY: env_lock() serializes IRONHERMES_HOME mutations across tests.
    unsafe { std::env::set_var("IRONHERMES_HOME", tmpdir.path()) };

    let mut registry = TtsRegistry::new();
    registry.register(Arc::new(EmptyProvider));

    let config = Arc::new(Config::default());
    let tool = TextToSpeechTool::new_with_registry(config, registry);

    let result: anyhow::Result<String> = tool.execute(serde_json::json!({"text": "x"})).await;

    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    assert!(
        result.is_err(),
        "execute() must reject a 0-byte audio file, got Ok: {:?}",
        result
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("empty audio file"),
        "error must explain the empty-file rejection, got: {msg}"
    );
}

// T-output-path BLOCKING test: LLM-supplied path traversal must be rejected.
#[tokio::test]
async fn test_output_path_traversal_blocked() {
    use async_trait::async_trait;
    use ironhermes_core::Config;
    use ironhermes_core::tts::{TtsProvider, TtsRegistry};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::tts_tool::TextToSpeechTool;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    struct FakeProvider;

    #[async_trait]
    impl TtsProvider for FakeProvider {
        fn name(&self) -> &str {
            "edge"
        }
        fn is_available(&self) -> bool {
            true
        }
        async fn synthesize(
            &self,
            _text: &str,
            output_path: &std::path::Path,
        ) -> anyhow::Result<PathBuf> {
            Ok(output_path.to_path_buf())
        }
    }

    let _lock = env_lock().lock().await;
    let tmpdir = TempDir::new().expect("create tempdir");

    // SAFETY: env_lock() serializes IRONHERMES_HOME mutations across tests.
    unsafe { std::env::set_var("IRONHERMES_HOME", tmpdir.path()) };

    let mut registry = TtsRegistry::new();
    registry.register(Arc::new(FakeProvider));

    let config = Arc::new(Config::default());
    let tool = TextToSpeechTool::new_with_registry(config, registry);

    // Attempt path traversal with /etc/passwd
    let result: anyhow::Result<String> = tool
        .execute(serde_json::json!({
            "text": "x",
            "output_path": "/etc/passwd"
        }))
        .await;

    assert!(
        result.is_err(),
        "output_path traversal to /etc/passwd must return Err (T-output-path BLOCKING)"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("audio_cache"),
        "error message must mention audio_cache (got: {})",
        err_msg
    );

    unsafe { std::env::remove_var("IRONHERMES_HOME") };
}

// Wiring test: register_tts_tools registers BOTH text_to_speech AND send_audio.
#[test]
fn test_register_tts_tools_registers_both() {
    use ironhermes_core::{Config, Platform, SessionKey};
    use ironhermes_tools::ToolRegistry;
    use std::sync::Arc;

    let mut registry = ToolRegistry::new();
    let config = Arc::new(Config::default());
    let session_key = SessionKey::new(Platform::Local, "test");

    registry.register_tts_tools(session_key, None, config);

    let all_tools = registry.list_tools();
    assert!(
        all_tools.contains(&"text_to_speech"),
        "text_to_speech must be registered after register_tts_tools; got: {:?}",
        all_tools
    );
    assert!(
        all_tools.contains(&"send_audio"),
        "send_audio must be registered after register_tts_tools; got: {:?}",
        all_tools
    );
}

// TTS-09 (un-ignored by PLAN 02): ffmpeg_available() never panics on any platform (D-04).
#[test]
fn test_ffmpeg_probe_no_panic() {
    // The result (true/false) depends on the host machine; we MUST NOT assert
    // a specific value — only that the call returns without panicking.
    let _ = ironhermes_tools::tts::ffmpeg_available();
}

// New test (PLAN 02): EdgeProvider::name() matches BUILTIN_TTS_NAMES[0].
#[test]
fn test_edge_provider_name_is_edge() {
    use ironhermes_core::config::EdgeTtsConfig;
    use ironhermes_core::tts::TtsProvider;
    use ironhermes_tools::tts::EdgeProvider;

    let provider = EdgeProvider::new(EdgeTtsConfig::default());
    assert_eq!(provider.name(), "edge");
}

// New test (PLAN 02 Task 3): ElevenLabsProvider::name() matches BUILTIN_TTS_NAMES[1].
#[test]
fn test_elevenlabs_provider_name_is_elevenlabs() {
    use ironhermes_core::config::ElevenLabsConfig;
    use ironhermes_core::tts::TtsProvider;
    use ironhermes_tools::tts::ElevenLabsProvider;

    let provider = ElevenLabsProvider::new(ElevenLabsConfig::default());
    assert_eq!(provider.name(), "elevenlabs");
}

// TTS-10 (un-ignored by PLAN 04): EdgeProvider::synthesize writes a real MP3 via live network.
// Runs ONLY with `cargo test -- --ignored` (live Edge TTS network call).
// Asserts: returned PathBuf == tmp_path; file exists; file size > 0; MP3/ID3 magic bytes present.
#[tokio::test]
#[ignore = "TTS-10 — live network; runs with cargo test -- --ignored"]
async fn test_edge_synth_writes_file() {
    use ironhermes_core::config::EdgeTtsConfig;
    use ironhermes_core::tts::TtsProvider;
    use ironhermes_tools::tts::EdgeProvider;
    use tempfile::TempDir;

    // msedge-tts tokio-runtime uses rustls 0.23 with aws-lc-rs backend.
    // The process-level CryptoProvider must be installed before any TLS connection.
    // `.ok()` is intentional: if another test already installed a provider, ignore the error.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let _lock = env_lock().lock().await;
    let tmpdir = TempDir::new().expect("create tempdir");

    // SAFETY: env_lock() serializes IRONHERMES_HOME mutations across tests.
    unsafe { std::env::set_var("IRONHERMES_HOME", tmpdir.path()) };

    let provider = EdgeProvider::new(EdgeTtsConfig::default());
    assert!(
        provider.is_available(),
        "EdgeProvider must be available (D-03)"
    );

    let tmp_path = tmpdir.path().join(format!("{}.mp3", uuid::Uuid::new_v4()));
    let result = provider
        .synthesize("Hello from IronHermes Phase 36.17.5 TTS test.", &tmp_path)
        .await;

    unsafe { std::env::remove_var("IRONHERMES_HOME") };

    let written =
        result.expect("EdgeProvider::synthesize must succeed against live Edge TTS (D-03)");

    // Assert returned PathBuf matches the requested path.
    assert_eq!(written, tmp_path, "synthesize must return the output path");

    // Assert file exists on disk.
    assert!(
        tmp_path.exists(),
        "synthesized file must exist at {:?} (D-16)",
        tmp_path
    );

    // Assert file is non-empty (real audio bytes, not a stub).
    let metadata = std::fs::metadata(&tmp_path).expect("stat synthesized file");
    assert!(
        metadata.len() > 0,
        "synthesized file must be non-empty (D-04 / D-05); got {} bytes",
        metadata.len()
    );

    // Assert MP3 frame sync or ID3 tag in first 3 bytes.
    // MP3: 0xFF 0xFB / 0xFF 0xF3 / 0xFF 0xF2 | ID3: 0x49 0x44 0x33 ("ID3")
    let bytes = std::fs::read(&tmp_path).expect("read synthesized file");
    assert!(
        bytes.len() >= 3,
        "synthesized file must have at least 3 bytes; got {}",
        bytes.len()
    );
    let magic_ok = matches!(
        (bytes[0], bytes[1]),
        (0xFF, 0xFB) | (0xFF, 0xF3) | (0xFF, 0xF2)
    ) || (bytes[0] == 0x49 && bytes[1] == 0x44 && bytes[2] == 0x33); // "ID3"
    assert!(
        magic_ok,
        "synthesized file must start with MP3 frame sync (0xFF 0xFB/F3/F2) or ID3 tag (0x49 0x44 0x33); got: {:02x} {:02x} {:02x}",
        bytes[0], bytes[1], bytes[2]
    );
}

// ============================================================================
// SendAudioTool tests (PLAN 03 Task 3)
// ============================================================================

#[test]
fn test_send_audio_tool_metadata() {
    use ironhermes_core::{Config, Platform, SessionKey};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_audio_tool::SendAudioTool;
    use std::sync::Arc;

    let config = Arc::new(Config::default());
    let session_key = SessionKey::new(Platform::Local, "test");
    let tool = SendAudioTool::new(session_key, None, config);

    assert_eq!(tool.name(), "send_audio");
    assert_eq!(tool.toolset(), "voice");

    let schema = tool.schema();
    let props = schema
        .function
        .parameters
        .get("properties")
        .expect("schema must have properties");
    assert!(
        props.get("path").is_some(),
        "schema must have 'path' property (D-14)"
    );

    let required = schema
        .function
        .parameters
        .get("required")
        .and_then(|r: &serde_json::Value| r.as_array())
        .expect("schema must have required array");
    assert!(
        required
            .iter()
            .any(|v: &serde_json::Value| v.as_str() == Some("path")),
        "'path' must be in required"
    );
}

#[tokio::test]
async fn test_send_audio_local_arm_plays_or_errors() {
    use ironhermes_core::{Config, Platform, SessionKey};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_audio_tool::SendAudioTool;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    let config = Arc::new(Config::default());
    let session_key = SessionKey::new(Platform::Local, "test");
    let tool = SendAudioTool::new(session_key, None, config);

    // Create a real temp file (but not a valid MP3 so rodio will error)
    let tmpfile = NamedTempFile::new().expect("create temp file");
    std::fs::write(tmpfile.path(), b"not-a-real-mp3-file").unwrap();

    let result = tool
        .execute(serde_json::json!({
            "path": tmpfile.path().to_str().unwrap()
        }))
        .await;

    // The file exists but is not valid audio — rodio returns Err, NOT a panic.
    // If the host has no audio device, we also get Err with "No audio device".
    // Either way: must be Err, must not panic.
    assert!(
        result.is_err(),
        "Local arm with a non-audio file must return Err, not panic; got: {:?}",
        result
    );
    // The error must be an anyhow error string (not an unwrap panic trace)
    let msg = result.unwrap_err().to_string();
    assert!(!msg.is_empty(), "error message must be non-empty");
}

#[tokio::test]
async fn test_send_audio_telegram_no_adapter_errors() {
    use ironhermes_core::{Config, Platform, SessionKey};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_audio_tool::SendAudioTool;
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    let config = Arc::new(Config::default());
    let session_key = SessionKey::new(Platform::Telegram, "chat123");
    let tool = SendAudioTool::new(session_key, None, config);

    let tmpfile = NamedTempFile::new().expect("create temp file");
    std::fs::write(tmpfile.path(), b"fake-audio").unwrap();

    let result = tool
        .execute(serde_json::json!({
            "path": tmpfile.path().to_str().unwrap()
        }))
        .await;

    assert!(
        result.is_err(),
        "Telegram arm without dispatcher must return Err"
    );
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.to_lowercase().contains("adapter") || msg.to_lowercase().contains("dispatcher"),
        "error message must mention adapter or dispatcher; got: {}",
        msg
    );
}

#[tokio::test]
async fn test_send_audio_unsupported_platform_bails() {
    use async_trait::async_trait;
    use ironhermes_core::{Config, Platform, SessionKey};
    use ironhermes_tools::registry::Tool;
    use ironhermes_tools::send_audio_tool::{AudioDispatcher, SendAudioTool};
    use std::sync::Arc;
    use tempfile::NamedTempFile;

    struct MockDispatcher;

    #[async_trait]
    impl AudioDispatcher for MockDispatcher {
        async fn send_voice_file(
            &self,
            _chat_id: &str,
            _path: &std::path::Path,
            _thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn send_audio_file(
            &self,
            _chat_id: &str,
            _path: &std::path::Path,
            _thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    let config = Arc::new(Config::default());
    // Discord is an unsupported platform (D-15 default branch)
    let session_key = SessionKey::new(Platform::Discord, "chat123");
    let tool = SendAudioTool::new(session_key, Some(Arc::new(MockDispatcher)), config);

    let tmpfile = NamedTempFile::new().expect("create temp file");
    std::fs::write(tmpfile.path(), b"fake-audio").unwrap();

    let result = tool
        .execute(serde_json::json!({
            "path": tmpfile.path().to_str().unwrap()
        }))
        .await;

    assert!(
        result.is_err(),
        "Discord platform must return Err (not yet wired per D-15)"
    );
    let msg = result.unwrap_err().to_string().to_lowercase();
    assert!(
        msg.contains("not yet wired") || msg.contains("discord"),
        "error must mention 'not yet wired' or 'discord'; got: {}",
        msg
    );
}
