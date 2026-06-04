//! Phase 36.17.5 — Wave 0/2 TTS tool tests. Owner plans un-ignore each test as wiring lands:
//!   TTS-03 / TTS-04 / TTS-09 → PLAN 02 (Edge / ElevenLabs / ffmpeg probe) ← UN-IGNORED HERE
//!   TTS-05 / TTS-06         → PLAN 03 (text_to_speech tool)
//!   TTS-10                  → PLAN 04 (live network integration; runs only with --ignored)
//!
//! Run: `cargo test -p ironhermes-tools --test tts_tools`

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// TTS-03 (un-ignored by PLAN 02): EdgeProvider::is_available() always returns true (D-03).
#[test]
fn test_edge_provider_available() {
    use ironhermes_core::config::EdgeTtsConfig;
    use ironhermes_tools::tts::EdgeProvider;
    use ironhermes_core::tts::TtsProvider;

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
    use ironhermes_tools::tts::ElevenLabsProvider;
    use ironhermes_core::tts::TtsProvider;

    let _lock = env_lock().lock().unwrap_or_else(|p| p.into_inner());

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

// TTS-05 (still ignored — PLAN 03 will un-ignore)
#[tokio::test]
#[ignore = "TTS-05 — un-ignored by PLAN 03 once TextToSpeechTool lands"]
async fn test_tts_tool_metadata() {
    unimplemented!("PLAN 03 will replace")
}

// TTS-06 (still ignored — PLAN 03 will un-ignore)
#[tokio::test]
#[ignore = "TTS-06 — un-ignored by PLAN 03 once TextToSpeechTool::execute lands"]
async fn test_tts_tool_creates_audio_cache_dir() {
    let _lock = env_lock().lock().unwrap();
    unimplemented!("PLAN 03 will replace")
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
    use ironhermes_tools::tts::EdgeProvider;
    use ironhermes_core::tts::TtsProvider;

    let provider = EdgeProvider::new(EdgeTtsConfig::default());
    assert_eq!(provider.name(), "edge");
}

// TTS-10 (still ignored — live network; opt-in via cargo test -- --ignored)
#[tokio::test]
#[ignore = "TTS-10 — live network; opt-in via cargo test -- --ignored"]
async fn test_edge_synth_writes_file() {
    unimplemented!("PLAN 04 will un-ignore once live UAT path is wired")
}
