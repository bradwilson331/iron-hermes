//! Phase 36.17.7 Plan 04 — Wave 0 tests for WebAudioDispatcher.
//!
//! Locks the source-shape of `server/web_audio_dispatcher.rs`:
//! - Struct fields (tx: UnboundedSender, audio_cache_dir: PathBuf)
//! - `impl AudioDispatcher for WebAudioDispatcher` presence
//! - `ChatStreamEvent::AudioOut` emission
//! - No re-persist (no `write_all` / `File::create`)
//!
//! Integration test (`dispatcher_sends_audio_out_on_send_audio_file`) builds
//! a temp dir with a known mp3 file, constructs the dispatcher, calls
//! `send_audio_file`, and asserts the resulting `ChatStreamEvent::AudioOut`
//! carries the correct fields.

#![cfg(feature = "server")]

use iron_hermes_ui::protocol::ChatStreamEvent;
use iron_hermes_ui::server::web_audio_dispatcher::WebAudioDispatcher;
use ironhermes_tools::AudioDispatcher;
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::sync::mpsc;

const SOURCE: &str = include_str!("../src/server/web_audio_dispatcher.rs");

// ─────────────────────────────────────────────────────────────────────────────
// Source-shape locks
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 36.17.7 D-02-a: WebAudioDispatcher must implement AudioDispatcher.
#[test]
fn dispatcher_impl_for_audio_dispatcher_present() {
    assert!(
        SOURCE.contains("AudioDispatcher for WebAudioDispatcher"),
        "D-02-a: web_audio_dispatcher.rs must contain `impl ... AudioDispatcher for WebAudioDispatcher`"
    );
}

/// Phase 36.17.7 D-02-a: struct must hold tx (UnboundedSender) and audio_cache_dir.
#[test]
fn dispatcher_struct_holds_tx_and_audio_cache_dir() {
    assert!(
        SOURCE.contains("tx:") && SOURCE.contains("UnboundedSender"),
        "D-02-a: WebAudioDispatcher struct must hold `tx: UnboundedSender<...>` field"
    );
    assert!(
        SOURCE.contains("audio_cache_dir:"),
        "D-02-a: WebAudioDispatcher struct must hold `audio_cache_dir:` field"
    );
}

/// Phase 36.17.7 D-02-a: dispatcher must emit ChatStreamEvent::AudioOut.
#[test]
fn dispatcher_emits_audio_out_event() {
    assert!(
        SOURCE.contains("ChatStreamEvent::AudioOut"),
        "D-02-a: web_audio_dispatcher.rs must construct and send `ChatStreamEvent::AudioOut`"
    );
}

/// Phase 36.17.7 D-02-a: dispatcher must NOT re-persist audio bytes
/// (TextToSpeechTool already persisted; WebAudioDispatcher only reads).
#[test]
fn dispatcher_does_not_re_persist() {
    assert!(
        !SOURCE.contains("write_all") && !SOURCE.contains("File::create"),
        "D-02-a: WebAudioDispatcher must not re-persist audio bytes (reads only)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Integration test — construct + call + assert event
// ─────────────────────────────────────────────────────────────────────────────

/// Phase 36.17.7 D-02-a: dispatcher reads mp3 bytes, emits AudioOut event.
#[tokio::test]
async fn dispatcher_sends_audio_out_on_send_audio_file() {
    let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

    let temp_dir = TempDir::new().expect("create temp dir");
    let uuid = "00000000-0000-0000-0000-000000000000";
    let mp3_path = temp_dir.path().join(format!("{uuid}.mp3"));

    // Write known bytes to the temp mp3 file.
    let expected_bytes: Vec<u8> = vec![0xFF, 0xFB, 0x90, 0x00];
    std::fs::write(&mp3_path, &expected_bytes).expect("write temp mp3");

    let dispatcher = WebAudioDispatcher::new(tx, temp_dir.path().to_path_buf());

    let result = dispatcher
        .send_audio_file("session", &mp3_path, None)
        .await;

    assert!(result.is_ok(), "send_audio_file must return Ok(())");

    let event = rx.recv().await.expect("must receive AudioOut event");
    match event {
        ChatStreamEvent::AudioOut {
            mime,
            uuid: got_uuid,
            bytes,
        } => {
            assert_eq!(
                mime, "audio/mpeg",
                "D-02-a: AudioOut mime must be audio/mpeg"
            );
            assert_eq!(
                got_uuid, uuid,
                "D-02-a: AudioOut uuid must match file stem"
            );
            assert_eq!(
                bytes, expected_bytes,
                "D-02-a: AudioOut bytes must match file contents"
            );
        }
        other => panic!("Expected ChatStreamEvent::AudioOut, got {other:?}"),
    }
}
