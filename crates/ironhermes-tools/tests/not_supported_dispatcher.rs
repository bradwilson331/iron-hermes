//! Phase 36.17.7 Plan 01 — Wave 0 tests for NotSupportedAudioDispatcher (D-03-b).
//!
//! Tests 1–2 are source-grep tests; they compile RED until Task 3 ships
//! `crates/ironhermes-tools/src/not_supported_dispatcher.rs`.
//! Tests 3–4 are behavioral `#[tokio::test]` tests that also require Task 3.

use ironhermes_tools::NotSupportedAudioDispatcher;
use std::path::PathBuf;

// ── Test 1: Source-grep — uses anyhow::bail!, no panic! or .unwrap() ─────────

#[test]
fn source_uses_anyhow_bail_not_panic() {
    const SRC: &str = include_str!("../src/not_supported_dispatcher.rs");
    assert!(
        SRC.contains("anyhow::bail!"),
        "not_supported_dispatcher.rs must use anyhow::bail! for error returns (not panic! or unwrap)"
    );
    assert!(
        !SRC.contains("panic!"),
        "not_supported_dispatcher.rs must NOT use panic! — use anyhow::bail! instead"
    );
    assert!(
        !SRC.contains(".unwrap()"),
        "not_supported_dispatcher.rs must NOT use .unwrap() — use anyhow::bail! for error paths"
    );
}

// ── Test 2: Source-grep — declares #[async_trait] impl AudioDispatcher ────────

#[test]
fn source_declares_async_trait_impl() {
    const SRC: &str = include_str!("../src/not_supported_dispatcher.rs");
    assert!(
        SRC.contains("#[async_trait]"),
        "not_supported_dispatcher.rs must declare #[async_trait] on the AudioDispatcher impl"
    );
    assert!(
        SRC.contains("AudioDispatcher for NotSupportedAudioDispatcher"),
        "not_supported_dispatcher.rs must impl AudioDispatcher for NotSupportedAudioDispatcher"
    );
}

// ── Test 3: Behavioral — discord send_audio returns Err with "discord" ────────

#[tokio::test]
async fn discord_send_audio_returns_err_with_discord_in_message() {
    use ironhermes_tools::AudioDispatcher;

    let dispatcher = NotSupportedAudioDispatcher::new("discord");
    let result = dispatcher
        .send_audio_file("chat1", &PathBuf::from("/tmp/x.mp3"), None)
        .await;

    assert!(
        result.is_err(),
        "send_audio_file must return Err for discord dispatcher"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("discord"),
        "error message must contain 'discord', got: {err_msg}"
    );
}

// ── Test 4: Behavioral — slack send_voice returns Err with "slack" ────────────

#[tokio::test]
async fn slack_send_voice_returns_err_with_slack_in_message() {
    use ironhermes_tools::AudioDispatcher;

    let dispatcher = NotSupportedAudioDispatcher::new("slack");
    let result = dispatcher
        .send_voice_file("chat1", &PathBuf::from("/tmp/y.ogg"), None)
        .await;

    assert!(
        result.is_err(),
        "send_voice_file must return Err for slack dispatcher"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.contains("slack"),
        "error message must contain 'slack', got: {err_msg}"
    );
}
