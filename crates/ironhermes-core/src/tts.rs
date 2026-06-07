// Phase 36.17.5 — TTS provider abstraction (D-08 / D-09 / D-10 / D-11)
//
// Trait + registry live in `ironhermes-core` so that `ironhermes-tools` can implement
// the trait without creating a circular crate dependency (D-08). Provider impls
// (`edge.rs`, `elevenlabs.rs`) live in `ironhermes-tools`.
//
// D-09 DEFERRED: `stream()`, `list_voices()`, `list_models()`, `default_voice()`,
// `default_model()`, `get_setup_schema()` are intentionally absent from this trait.
// D-11: Error type is `anyhow::Result<PathBuf>` — no `TtsError` enum this phase.
// D-10: `BUILTIN_TTS_NAMES` is the compile-time source of truth for the built-in set.

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// D-09: Minimal TTS provider trait surface for Phase 36.17.5.
///
/// Mirrors the shape of the Python `TTSProvider` ABC from `tts_provider.py`:
/// `name`, `display_name`, `is_available`, `voice_compatible`, `synthesize`.
///
/// Object-safe with `async-trait`: the macro desugars `async fn` to
/// `Pin<Box<dyn Future + Send>>`, which is object-safe in `Arc<dyn TtsProvider>`.
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Canonical lowercase provider name (e.g. `"edge"`, `"elevenlabs"`).
    fn name(&self) -> &str;

    /// Human-readable display name. Defaults to `self.name()`.
    fn display_name(&self) -> &str {
        self.name()
    }

    /// Returns `true` if the provider is usable in the current environment.
    ///
    /// For keyless providers (Edge TTS) this is always `true`.
    /// For keyed providers (ElevenLabs) this checks the relevant env var.
    fn is_available(&self) -> bool;

    /// Returns `true` if the provider produces Opus/OGG output that Telegram
    /// accepts as a native voice bubble without ffmpeg conversion.
    ///
    /// Defaults to `false` (Edge TTS emits MP3; ffmpeg converts if present).
    fn voice_compatible(&self) -> bool {
        false
    }

    /// Synthesize `text` and write the audio to `output_path`.
    ///
    /// Returns the path that was written (may differ from `output_path` if the
    /// provider appended an extension). Callers should use the returned path.
    async fn synthesize(&self, text: &str, output_path: &Path) -> anyhow::Result<PathBuf>;
}

/// D-10: Compile-time frozen set of built-in TTS provider names.
///
/// `TtsRegistry::register()` accepts only names in this set; any other name
/// triggers a `tracing::warn!` and is ignored. This pre-positions the invariant
/// for a future user-extension reversal without changing call sites.
pub const BUILTIN_TTS_NAMES: &[&str] = &["edge", "elevenlabs"];

/// Registry mapping provider names to `Arc<dyn TtsProvider>` instances.
///
/// Populated at startup by `ironhermes-tools::register_tts_tools()`.
/// Keyed by lowercase provider name for case-insensitive lookup.
pub struct TtsRegistry {
    providers: HashMap<String, Arc<dyn TtsProvider>>,
}

impl TtsRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider.
    ///
    /// The provider's `name()` is lowercased before insertion.
    /// If the lowercased name is **not** in `BUILTIN_TTS_NAMES`, the registration
    /// is rejected with a `tracing::warn!` (D-10 built-ins-always-win invariant).
    pub fn register(&mut self, provider: Arc<dyn TtsProvider>) {
        let key = provider.name().to_lowercase();
        if BUILTIN_TTS_NAMES.contains(&key.as_str()) {
            self.providers.insert(key, provider);
            return;
        }
        tracing::warn!(
            "TtsRegistry::register: '{}' is not in BUILTIN_TTS_NAMES; ignored",
            key
        );
    }

    /// Look up a provider by name (case-insensitive).
    ///
    /// Returns `None` if no provider with that name has been registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn TtsProvider>> {
        self.providers.get(&name.to_lowercase()).cloned()
    }
}

impl Default for TtsRegistry {
    fn default() -> Self {
        Self::new()
    }
}
