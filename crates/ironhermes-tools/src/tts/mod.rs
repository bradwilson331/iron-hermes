// Phase 36.17.5 — TTS provider impls (edge, elevenlabs)
//
// This module holds:
//   1. Module declarations for the two built-in providers.
//   2. The ffmpeg_available() lazy probe (D-04).
//   3. The build_tts_registry() factory — single source of truth for wiring
//      built-in providers into a TtsRegistry (D-10). PLAN 03 calls this.

pub mod audio_format;
pub mod edge;
pub mod elevenlabs;
pub mod openai;
pub mod openai_compatible;

pub use edge::EdgeProvider;
pub use elevenlabs::ElevenLabsProvider;
pub use openai::OpenAiTtsProvider;
pub use openai_compatible::OpenAiCompatibleTtsProvider;
use std::sync::OnceLock;

// D-04: Cache the ffmpeg probe result once per process. The probe is sync,
// safe to call from any context (inside or outside an async runtime), and
// never panics — `unwrap_or(false)` handles Windows-without-ffmpeg and any
// other platform where the binary is absent or the child process spawn fails.
static FFMPEG_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Returns `true` if `ffmpeg` is present and exits successfully on this host.
///
/// Result is cached in a `OnceLock` — the probe runs at most once per process.
/// Safe to call from any thread; never panics on any platform.
pub fn ffmpeg_available() -> bool {
    *FFMPEG_AVAILABLE.get_or_init(|| {
        std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Test-only helper that bypasses the `OnceLock` when a forced value is given.
///
/// When `force` is `Some(b)`, returns `b` directly (useful for deterministic
/// unit tests that must not race on the process-global `OnceLock`).
/// When `force` is `None`, delegates to `ffmpeg_available()`.
#[cfg(test)]
pub fn ffmpeg_available_for_test(force: Option<bool>) -> bool {
    match force {
        Some(b) => b,
        None => ffmpeg_available(),
    }
}

/// Build a `TtsRegistry` pre-populated with the built-in providers.
///
/// Environment key references (`VOICE_KEY_*`) resolve live on every surface through
/// this builder; vault references (`secret/voice/*`) need
/// [`build_tts_registry_with_vault`] (D-02). Delegates with no vault config.
pub fn build_tts_registry(
    config: &ironhermes_core::config::TtsConfig,
) -> ironhermes_core::TtsRegistry {
    build_tts_registry_with_vault(config, None)
}

/// Build a `TtsRegistry` whose `openai_compatible` provider can also resolve
/// `secret/voice/*` key references against `vault` (D-02, D-19).
///
/// This is the single production call site for built-in provider registration
/// (D-10). `ToolRegistry::register_tts_tools` calls it with the vault config derived
/// from the runtime config.
pub fn build_tts_registry_with_vault(
    config: &ironhermes_core::config::TtsConfig,
    vault: Option<ironhermes_vault::VaultConfig>,
) -> ironhermes_core::TtsRegistry {
    use std::sync::Arc;
    let mut registry = ironhermes_core::TtsRegistry::new();
    registry.register(Arc::new(crate::tts::edge::EdgeProvider::new(
        config.edge.clone(),
    )));
    registry.register(Arc::new(crate::tts::elevenlabs::ElevenLabsProvider::new(
        config.elevenlabs.clone(),
    )));
    registry.register(Arc::new(crate::tts::openai::OpenAiTtsProvider::new(
        config.openai.clone(),
    )));
    registry.register(Arc::new(
        crate::tts::openai_compatible::OpenAiCompatibleTtsProvider::new(
            config.openai_compatible.clone(),
            vault,
        ),
    ));
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_core::config::TtsConfig;

    /// D-02 / D-19: the vault-aware builder registers `openai_compatible`, the plain
    /// builder delegates to it, and `ToolRegistry::register_tts_tools` is wired through
    /// it with `voice_vault_config` so a `secret/voice/*` reference resolves on the
    /// agent-tool / gateway path.
    #[test]
    fn register_tts_tools_uses_vault_aware_builder() {
        let mut tts = TtsConfig {
            provider: "openai_compatible".to_string(),
            ..Default::default()
        };
        tts.openai_compatible.base_url = "http://127.0.0.1:1/v1".to_string();

        let with_vault = build_tts_registry_with_vault(&tts, None);
        let provider = with_vault
            .get("openai_compatible")
            .expect("vault-aware builder registers openai_compatible");
        assert!(provider.is_available());
        assert!(
            build_tts_registry(&tts).get("openai_compatible").is_some(),
            "build_tts_registry delegates to the vault-aware builder with None"
        );

        // The agent-tool path: register_tts_tools must build through the vault-aware
        // builder, passing the vault config derived from the runtime config.
        let src = include_str!("../registry.rs");
        let start = src
            .find("pub fn register_tts_tools(")
            .expect("register_tts_tools exists");
        let rest = &src[start + 1..];
        let end = rest.find("\n    pub fn ").unwrap_or(rest.len());
        let body = &rest[..end];
        assert!(
            body.contains("build_tts_registry_with_vault("),
            "register_tts_tools must use the vault-aware builder"
        );
        assert!(
            body.contains("voice_vault_config("),
            "register_tts_tools must pass voice_vault_config(&config)"
        );
    }
}
