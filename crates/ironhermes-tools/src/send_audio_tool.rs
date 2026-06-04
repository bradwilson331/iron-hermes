// Phase 36.17.5 D-14 / D-15 — SendAudioTool: platform-agnostic audio dispatch tool.
//
// AudioDispatcher trait lives here (in ironhermes-tools) so that gateway can
// implement it for TelegramAdapter without creating a circular crate dependency
// (gateway → tools, not the reverse). The orphan rule requires the impl to live
// in the gateway crate (crates/ironhermes-gateway/src/telegram.rs).
//
// D-14: Single tool, constructor-injected SessionKey + optional dispatcher.
// D-15: Platform-dispatch tree — Local→rodio, Telegram→AudioDispatcher methods.

use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

/// Platform-agnostic audio delivery abstraction.
///
/// Implemented in `ironhermes-gateway` for `TelegramAdapter` (orphan-rule-safe:
/// trait defined here in tools, type defined there in gateway).
/// The `ironhermes-tools` crate itself never imports gateway types.
///
/// Method names are distinct from `TelegramAdapter`'s inherent methods
/// (`send_voice` / `send_audio`) to avoid ambiguity in the gateway impl block.
#[async_trait]
pub trait AudioDispatcher: Send + Sync {
    /// Send an OGG/Opus (or other voice-native) file to the chat.
    async fn send_voice_file(
        &self,
        chat_id: &str,
        path: &PathBuf,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a generic audio file (e.g. MP3) to the chat when ffmpeg is absent.
    async fn send_audio_file(
        &self,
        chat_id: &str,
        path: &PathBuf,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}

/// D-14: LLM-callable tool that dispatches a synthesized audio file to the
/// current platform. Constructor-injected SessionKey + optional AudioDispatcher
/// so the tool is statically typed to the session at registration time.
pub struct SendAudioTool {
    session_key: ironhermes_core::SessionKey,
    dispatcher: Option<Arc<dyn AudioDispatcher>>,
    config: Arc<ironhermes_core::Config>,
}

impl SendAudioTool {
    pub fn new(
        session_key: ironhermes_core::SessionKey,
        dispatcher: Option<Arc<dyn AudioDispatcher>>,
        config: Arc<ironhermes_core::Config>,
    ) -> Self {
        Self {
            session_key,
            dispatcher,
            config,
        }
    }
}

#[async_trait]
impl crate::registry::Tool for SendAudioTool {
    fn name(&self) -> &str {
        "send_audio"
    }

    fn toolset(&self) -> &str {
        "voice"
    }

    fn description(&self) -> &str {
        "Send a previously-synthesized audio file to the current platform's reply path."
    }

    fn schema(&self) -> ironhermes_core::ToolSchema {
        ironhermes_core::ToolSchema::new(
            "send_audio",
            "Send a previously-synthesized audio file to the current platform.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute path to the audio file (typically returned by text_to_speech)."
                    }
                },
                "required": ["path"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;
        let path = std::path::PathBuf::from(path_str);
        if !path.exists() {
            anyhow::bail!("Audio file does not exist: {}", path.display());
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        match self.session_key.platform {
            ironhermes_core::Platform::Local => {
                // D-15 Local arm: rodio playback. Handles MP3 + OGG via `mp3` feature.
                // rodio 0.22 API: DeviceSinkBuilder → Player (replaces OutputStream/Sink).
                let file = std::fs::File::open(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to open audio file: {}", e))?;
                let stream_handle = rodio::DeviceSinkBuilder::open_default_sink()
                    .map_err(|e| anyhow::anyhow!("No audio device: {}", e))?;
                let player = rodio::Player::connect_new(stream_handle.mixer());
                let source = rodio::Decoder::try_from(file)
                    .map_err(|e| anyhow::anyhow!("Failed to decode audio: {}", e))?;
                player.append(source);
                player.sleep_until_end();
                Ok(format!("played: {}", path.display()))
            }
            ironhermes_core::Platform::Telegram => {
                let dispatcher = self.dispatcher.as_ref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "send_audio on Telegram requires an AudioDispatcher (none attached this session)"
                    )
                })?;
                match ext.as_str() {
                    "ogg" | "opus" => {
                        dispatcher
                            .send_voice_file(&self.session_key.chat_id, &path, None)
                            .await?;
                        Ok(format!("sent voice: {}", path.display()))
                    }
                    "mp3" if crate::tts::ffmpeg_available() => {
                        // D-04 + Pitfall 1: MP3 → OGG/opus via ffmpeg subprocess
                        let opus_path = path.with_extension("ogg");
                        let status = tokio::process::Command::new(
                            self.config
                                .tts
                                .ffmpeg_path
                                .as_deref()
                                .unwrap_or("ffmpeg"),
                        )
                        .args(["-y", "-i"])
                        .arg(&path)
                        .args(["-c:a", "libopus", "-b:a", "64k", "-ar", "48000", "-ac", "1"])
                        .arg(&opus_path)
                        .status()
                        .await
                        .map_err(|e| anyhow::anyhow!("ffmpeg spawn failed: {}", e))?;
                        if !status.success() {
                            anyhow::bail!(
                                "ffmpeg conversion failed with status {}",
                                status
                            );
                        }
                        dispatcher
                            .send_voice_file(&self.session_key.chat_id, &opus_path, None)
                            .await?;
                        Ok(format!(
                            "converted+sent voice: {}",
                            opus_path.display()
                        ))
                    }
                    "mp3" => {
                        dispatcher
                            .send_audio_file(&self.session_key.chat_id, &path, None)
                            .await?;
                        Ok(format!("sent audio (no ffmpeg): {}", path.display()))
                    }
                    other => anyhow::bail!(
                        "send_audio Telegram arm: unsupported extension '{}' (need .mp3, .ogg, .opus)",
                        other
                    ),
                }
            }
            ref other => anyhow::bail!(
                "send_audio not yet wired for platform: {:?}",
                other
            ),
        }
    }
}
