//! Phase 36.17.7 D-02-a — WebAudioDispatcher: web-runtime AudioDispatcher impl.
//!
//! Reads an mp3 file from the audio cache directory, extracts the UUID from
//! the file stem, and emits a `ChatStreamEvent::AudioOut` via the per-turn
//! unbounded channel so the ws send loop can forward it to the browser as
//! a `Message::Binary` frame.
//!
//! Architecture note (Pitfall 2 prevention): this impl lives in `iron_hermes_ui`
//! NOT in `ironhermes-tools`. The `AudioDispatcher` trait is defined in
//! `ironhermes-tools`; the orphan rule requires the impl to live in the crate
//! that owns the channel type (`UnboundedSender<ChatStreamEvent>` from
//! iron_hermes_ui). Placing the impl in ironhermes-tools would create a
//! circular dependency (tools → ui → tools).
//!
//! Do NOT re-persist audio bytes here — `TextToSpeechTool` already wrote the
//! mp3 to disk. This dispatcher reads only.

#![cfg(feature = "server")]

use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedSender;

use crate::protocol::ChatStreamEvent;

/// Web-runtime audio dispatcher.
///
/// Constructed per-turn in `ws.rs` with a clone of the turn's `tx` sender
/// and the audio cache directory path. On `send_audio_file`, reads the mp3
/// bytes from disk, extracts the UUID from the file stem, and emits a
/// `ChatStreamEvent::AudioOut` into the channel.
///
/// The ws send loop's `Message::Binary` arm picks up AudioOut events and
/// forwards them to the browser client as binary WebSocket frames.
pub struct WebAudioDispatcher {
    tx: UnboundedSender<ChatStreamEvent>,
    audio_cache_dir: PathBuf,
}

impl WebAudioDispatcher {
    /// Construct a new dispatcher for a single web turn.
    ///
    /// `tx` — the per-turn unbounded sender for `ChatStreamEvent`; a clone
    ///        of the same `tx` used by the stream/tool callbacks in `ws.rs`.
    /// `audio_cache_dir` — the directory where TTS mp3 files are written
    ///                     (typically `$IRONHERMES_HOME/audio_cache/`).
    pub fn new(tx: UnboundedSender<ChatStreamEvent>, audio_cache_dir: PathBuf) -> Self {
        Self { tx, audio_cache_dir }
    }
}

#[async_trait]
impl ironhermes_tools::AudioDispatcher for WebAudioDispatcher {
    /// Send a voice file (OGG/Opus or MP3) to the web client.
    ///
    /// For the web runtime, voice files are treated identically to audio
    /// files — both are delivered as binary `AudioOut` WS frames (D-02-e).
    async fn send_voice_file(
        &self,
        chat_id: &str,
        path: &PathBuf,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.send_audio_file(chat_id, path, thread_id).await
    }

    /// Read an mp3 from `path`, emit a `ChatStreamEvent::AudioOut` event.
    ///
    /// Extracts the UUID from the file stem (the filename without extension).
    /// Does NOT re-persist bytes — reads only.
    async fn send_audio_file(
        &self,
        _chat_id: &str,
        path: &PathBuf,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Read mp3 bytes from disk. TextToSpeechTool already persisted;
        // WebAudioDispatcher is a read-only consumer.
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| anyhow::anyhow!("read audio file {}: {e}", path.display()))?;

        // Extract UUID from file stem (e.g. "abc123" from "abc123.mp3").
        let uuid = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let event = ChatStreamEvent::AudioOut {
            mime: "audio/mpeg".to_string(),
            uuid,
            bytes,
        };

        self.tx
            .send(event)
            .map_err(|e| anyhow::anyhow!("ws send failed: {e}"))?;

        Ok(())
    }
}
