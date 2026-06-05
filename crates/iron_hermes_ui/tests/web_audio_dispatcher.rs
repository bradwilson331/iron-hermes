//! Phase 36.17.7 Plan 04 — Wave 0 tests for WebAudioDispatcher.
//!
//! Locks the source-shape of `server/web_audio_dispatcher.rs`:
//! - Struct fields (tx: UnboundedSender, audio_cache_dir: PathBuf)
//! - `impl AudioDispatcher for WebAudioDispatcher` presence
//! - `ChatStreamEvent::AudioOut` emission
//! - No re-persist (no `write_all` / `File::create`)
//!
//! NOTE: `iron_hermes_ui` is a binary crate (no `[lib]` target), so integration
//! tests cannot `use iron_hermes_ui::...` (see running_agent_guard_web_tests.rs).
//! These are source-string assertions. The behavioral test that constructs the
//! dispatcher, calls `send_audio_file`, and asserts the emitted
//! `ChatStreamEvent::AudioOut` lives as a `#[cfg(test)]` unit test INSIDE
//! `src/server/web_audio_dispatcher.rs` (`dispatcher_sends_audio_out_on_send_audio_file`),
//! where it can name the crate-private types directly.

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

/// Phase 36.17.7 D-02-a: the behavioral test must live as an in-source unit test
/// (the bin-crate convention) — assert its presence so it can't silently vanish.
#[test]
fn behavioral_unit_test_present_in_source() {
    assert!(
        SOURCE.contains("dispatcher_sends_audio_out_on_send_audio_file"),
        "D-02-a: web_audio_dispatcher.rs must contain the in-source behavioral unit test \
         `dispatcher_sends_audio_out_on_send_audio_file`"
    );
}
