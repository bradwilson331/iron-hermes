// Phase 54.7 Plan 04 — the one clip transcriber (D-17 / D-19).
//
// The web voice WebSocket (plan 10) and the Test mic action (plan 07) both turn
// a captured browser clip into text through `transcribe_clip`: provider
// selection, UUID-named temp files under the audio cache, a 16 kHz mono ffmpeg
// transcode (passthrough when ffmpeg is absent or fails), the provider call, and
// temp-file cleanup on every exit path.
//
// T-54.7-17: both temp files are removed by a drop guard, so success, error and
// future cancellation all clean up.
// T-54.7-18: no client-controlled path or argument reaches ffmpeg — the input
// name is a UUID plus a literal extension derived from the mime type.

use ironhermes_core::config::SttConfig;
use ironhermes_core::voice_http::{VoiceErrorCategory, VoiceHttpError};
use std::path::{Path, PathBuf};

use crate::stt::{build_stt_registry_with_vault, select_stt_provider};

/// Why a clip could not be transcribed. `Display` never includes a path outside
/// the cache directory, a key or a response body.
#[derive(Debug)]
pub enum ClipError {
    /// STT is not configured (auto selection found nothing).
    NoProvider,
    /// The selected provider is unknown or not usable right now.
    ProviderUnavailable(String),
    /// A temp file could not be created or written.
    Io(String),
    /// The provider call failed; only the category is kept.
    Transcribe(VoiceErrorCategory),
}

impl std::fmt::Display for ClipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => f.write_str("no speech-to-text provider is configured"),
            Self::ProviderUnavailable(name) => {
                write!(f, "speech-to-text provider '{name}' is not available")
            }
            Self::Io(msg) => write!(f, "audio cache error: {msg}"),
            Self::Transcribe(category) => write!(f, "transcription failed: {}", category.as_str()),
        }
    }
}

impl std::error::Error for ClipError {}

/// Extension for the ffmpeg input file, derived from the clip's mime type.
///
/// `audio/mp4` -> `mp4`, `audio/ogg` -> `ogg`, anything else -> `webm`. The
/// result is always one of three literals, so a client mime cannot influence the
/// file name.
pub fn clip_input_extension(mime: &str) -> &'static str {
    let base = mime
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    match base.as_str() {
        "audio/mp4" | "video/mp4" => "mp4",
        "audio/ogg" => "ogg",
        _ => "webm",
    }
}

/// IN-08: log (once per process) that a non-WAV container is going to a custom
/// server. Without ffmpeg the original webm/mp4/ogg is passed through, and servers
/// such as whisper.cpp only accept WAV unless started with `--convert`; the
/// resulting failure is otherwise a generic "unexpected response".
fn warn_once_on_passthrough(provider: &str, audio_path: &Path) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    let passthrough = audio_path.extension().and_then(|e| e.to_str()) != Some("wav");
    if provider == "openai_compatible" && passthrough && !WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "audio-in: sending a non-WAV clip to an openai_compatible STT server (ffmpeg is \
             missing or failed); servers like whisper.cpp need WAV unless started with --convert"
        );
    }
}

/// Removes the clip's temp files when dropped, on every exit path.
struct TempFiles(Vec<PathBuf>);

impl Drop for TempFiles {
    fn drop(&mut self) {
        for p in &self.0 {
            // Best effort: a file that was never created is not an error.
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Transcribe one captured browser clip.
///
/// Returns the trimmed transcript; callers apply any hallucination filtering or
/// wake-word matching. `cache_dir` is created if missing.
pub async fn transcribe_clip(
    bytes: &[u8],
    mime: &str,
    stt: &SttConfig,
    vault: Option<&ironhermes_vault::VaultConfig>,
    cache_dir: &Path,
) -> Result<String, ClipError> {
    let name = select_stt_provider(stt).ok_or(ClipError::NoProvider)?;
    let registry = build_stt_registry_with_vault(stt, vault.cloned());
    let provider = match registry.get(&name) {
        Some(p) if p.is_available() => p,
        _ => return Err(ClipError::ProviderUnavailable(name)),
    };

    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|e| ClipError::Io(format!("cannot create cache directory ({:?})", e.kind())))?;

    let file_uuid = uuid::Uuid::new_v4().to_string();
    let input_path = cache_dir.join(format!("{file_uuid}-in.{}", clip_input_extension(mime)));
    let wav_path = cache_dir.join(format!("{file_uuid}.wav"));
    // Registered before the first write so a partial write is cleaned too.
    let _cleanup = TempFiles(vec![input_path.clone(), wav_path.clone()]);

    tokio::fs::write(&input_path, bytes)
        .await
        .map_err(|e| ClipError::Io(format!("cannot write audio ({:?})", e.kind())))?;

    // Transcode to 16 kHz mono WAV when ffmpeg is available; otherwise (or on
    // failure) hand the original container to the provider.
    let transcribe_path = if crate::tts::ffmpeg_available() {
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-y", "-i"])
            .arg(&input_path)
            .args(["-ar", "16000", "-ac", "1", "-f", "wav"])
            .arg(&wav_path)
            .output()
            .await;
        match status {
            Ok(out) if out.status.success() => wav_path.clone(),
            Ok(out) => {
                tracing::warn!(
                    stderr = %String::from_utf8_lossy(&out.stderr),
                    "audio-in: ffmpeg transcode failed; falling back to passthrough"
                );
                input_path.clone()
            }
            Err(e) => {
                tracing::warn!(
                    reason = %e,
                    "audio-in: ffmpeg spawn failed; falling back to passthrough"
                );
                input_path.clone()
            }
        }
    } else {
        input_path.clone()
    };

    warn_once_on_passthrough(provider.name(), &transcribe_path);

    match provider.transcribe(&transcribe_path).await {
        Ok(text) => Ok(text.trim().to_string()),
        Err(e) => {
            let category = e
                .downcast_ref::<VoiceHttpError>()
                .map(|v| v.category)
                .unwrap_or(VoiceErrorCategory::UnexpectedResponse);
            Err(ClipError::Transcribe(category))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::OpenAiCompatibleSttConfig;
    use tempfile::TempDir;

    /// Restores GROQ_API_KEY / VOICE_TOOLS_OPENAI_KEY when dropped, including
    /// on the assertion-failure path.
    struct KeyRestore {
        groq: Option<String>,
        openai: Option<String>,
    }

    impl KeyRestore {
        /// Save both variables and remove them (caller holds `crate::ENV_LOCK`).
        fn clear() -> Self {
            let saved = Self {
                groq: std::env::var("GROQ_API_KEY").ok(),
                openai: std::env::var("VOICE_TOOLS_OPENAI_KEY").ok(),
            };
            // SAFETY: serialised by crate::ENV_LOCK; restored in Drop.
            unsafe {
                std::env::remove_var("GROQ_API_KEY");
                std::env::remove_var("VOICE_TOOLS_OPENAI_KEY");
            }
            saved
        }
    }

    impl Drop for KeyRestore {
        fn drop(&mut self) {
            // SAFETY: caller still holds crate::ENV_LOCK while this drops.
            unsafe {
                match &self.groq {
                    Some(v) => std::env::set_var("GROQ_API_KEY", v),
                    None => std::env::remove_var("GROQ_API_KEY"),
                }
                match &self.openai {
                    Some(v) => std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v),
                    None => std::env::remove_var("VOICE_TOOLS_OPENAI_KEY"),
                }
            }
        }
    }

    fn dir_is_empty(dir: &Path) -> bool {
        match std::fs::read_dir(dir) {
            Ok(mut it) => it.next().is_none(),
            Err(_) => true,
        }
    }

    #[tokio::test]
    async fn clip_reports_no_provider_when_auto_has_nothing() {
        let _guard = crate::ENV_LOCK.lock().await;
        let _restore = KeyRestore::clear();
        let cache = TempDir::new().unwrap();
        let cfg = SttConfig::default(); // provider = auto, empty openai_compatible.base_url

        let err = transcribe_clip(b"webm-bytes", "audio/webm", &cfg, None, cache.path())
            .await
            .expect_err("auto with nothing configured must fail");
        assert!(matches!(err, ClipError::NoProvider), "{err:?}");
        assert!(dir_is_empty(cache.path()), "no file may be written");
    }

    #[tokio::test]
    async fn clip_reports_unavailable_provider() {
        let cache = TempDir::new().unwrap();
        let cfg = SttConfig {
            provider: "openai_compatible".to_string(),
            openai_compatible: OpenAiCompatibleSttConfig::default(), // empty base_url
            ..SttConfig::default()
        };

        let err = transcribe_clip(b"webm-bytes", "audio/webm", &cfg, None, cache.path())
            .await
            .expect_err("unconfigured provider must fail");
        assert!(
            matches!(&err, ClipError::ProviderUnavailable(n) if n == "openai_compatible"),
            "{err:?}"
        );
        assert!(dir_is_empty(cache.path()), "no file may be written");
    }

    #[test]
    fn clip_input_extension_follows_mime() {
        assert_eq!(clip_input_extension("audio/mp4"), "mp4");
        assert_eq!(clip_input_extension("audio/ogg"), "ogg");
        assert_eq!(clip_input_extension("audio/webm"), "webm");
        assert_eq!(clip_input_extension("audio/webm;codecs=opus"), "webm");
        assert_eq!(clip_input_extension("application/octet-stream"), "webm");
        assert_eq!(clip_input_extension(""), "webm");
        // A client-supplied mime can never inject a path or arbitrary extension.
        assert_eq!(clip_input_extension("audio/../../etc/passwd"), "webm");
        assert_eq!(clip_input_extension("audio/mp4/../x"), "webm");
    }
}
