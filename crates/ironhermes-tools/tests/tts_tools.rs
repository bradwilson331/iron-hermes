//! Phase 36.17.5 — Wave 0 stubs. Owner plans un-ignore each test as wiring lands:
//!   TTS-03 / TTS-04 / TTS-09 → PLAN 02 (Edge / ElevenLabs / ffmpeg probe)
//!   TTS-05 / TTS-06         → PLAN 03 (text_to_speech tool)
//!   TTS-10                  → PLAN 04 (live network integration; runs only with --ignored)
//!
//! Run: `cargo test -p ironhermes-tools --test tts_tools`

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
#[ignore = "TTS-03 — un-ignored by PLAN 02 once EdgeProvider lands"]
async fn test_edge_provider_available() {
    unimplemented!("PLAN 02 will replace")
}

#[tokio::test]
#[ignore = "TTS-04 — un-ignored by PLAN 02 once ElevenLabsProvider lands"]
async fn test_elevenlabs_unavailable_no_key() {
    unimplemented!("PLAN 02 will replace")
}

#[tokio::test]
#[ignore = "TTS-05 — un-ignored by PLAN 03 once TextToSpeechTool lands"]
async fn test_tts_tool_metadata() {
    unimplemented!("PLAN 03 will replace")
}

#[tokio::test]
#[ignore = "TTS-06 — un-ignored by PLAN 03 once TextToSpeechTool::execute lands"]
async fn test_tts_tool_creates_audio_cache_dir() {
    let _lock = env_lock().lock().unwrap();
    unimplemented!("PLAN 03 will replace")
}

#[tokio::test]
#[ignore = "TTS-09 — un-ignored by PLAN 02 once ffmpeg probe lands"]
async fn test_ffmpeg_probe_no_panic() {
    unimplemented!("PLAN 02 will replace")
}

#[tokio::test]
#[ignore = "TTS-10 — live network; opt-in via cargo test -- --ignored"]
async fn test_edge_synth_writes_file() {
    unimplemented!("PLAN 04 will un-ignore once live UAT path is wired")
}
