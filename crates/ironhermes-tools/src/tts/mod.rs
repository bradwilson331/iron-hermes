// Phase 36.17.5 — TTS provider impls (edge, elevenlabs)
//
// This module holds:
//   1. Module declarations for the two built-in providers.
//   2. The ffmpeg_available() lazy probe (D-04).
//   3. The build_tts_registry() factory — single source of truth for wiring
//      built-in providers into a TtsRegistry (D-10). PLAN 03 calls this.

pub mod edge;
pub mod elevenlabs;

pub use edge::EdgeProvider;
pub use elevenlabs::ElevenLabsProvider;
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

/// Build a `TtsRegistry` pre-populated with the two built-in providers.
///
/// This is the single production call site for built-in provider registration
/// (D-10). PLAN 03's `register_tts_tools()` calls this function.
pub fn build_tts_registry(
    config: &ironhermes_core::config::TtsConfig,
) -> ironhermes_core::TtsRegistry {
    use std::sync::Arc;
    let mut registry = ironhermes_core::TtsRegistry::new();
    registry.register(Arc::new(crate::tts::edge::EdgeProvider::new(
        config.edge.clone(),
    )));
    registry.register(Arc::new(crate::tts::elevenlabs::ElevenLabsProvider::new(
        config.elevenlabs.clone(),
    )));
    registry
}
