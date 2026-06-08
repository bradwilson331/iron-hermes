// Phase 36.17.7 Plan 01 D-03-b — NotSupportedAudioDispatcher.
//
// Zero-impl AudioDispatcher stub for platforms that don't support audio
// delivery (Discord, Slack). Tools still register so the LLM tool-schema is
// consistent across platforms; send_audio returns a clean Err naming the
// platform so the agent can narrate the limitation. Deletion target when
// Discord and Slack receive real AudioDispatcher impls in a follow-up phase.
//
// Mirrors the AudioDispatcher trait definition at send_audio_tool.rs:24-40.

use async_trait::async_trait;

/// Stub `AudioDispatcher` for platforms without audio delivery support.
///
/// Both `send_voice_file` and `send_audio_file` return
/// `Err(anyhow!("audio dispatch not supported on {platform_name}"))`.
/// `platform_name` is a `&'static str` because the set of unsupported
/// platforms (Discord, Slack today) is known at compile time.
pub struct NotSupportedAudioDispatcher {
    platform_name: &'static str,
}

impl NotSupportedAudioDispatcher {
    pub fn new(platform_name: &'static str) -> Self {
        Self { platform_name }
    }
}

#[async_trait]
impl crate::AudioDispatcher for NotSupportedAudioDispatcher {
    async fn send_voice_file(
        &self,
        _chat_id: &str,
        _path: &std::path::Path,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("audio dispatch not supported on {}", self.platform_name)
    }

    async fn send_audio_file(
        &self,
        _chat_id: &str,
        _path: &std::path::Path,
        _thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("audio dispatch not supported on {}", self.platform_name)
    }
}
