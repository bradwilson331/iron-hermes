// Phase 36.17.8 — STT provider abstraction (D-04 / D-05 / D-06 / D-17 / D-19)
//
// Trait + registry live in `ironhermes-core` so that `ironhermes-tools` can implement
// the trait without creating a circular crate dependency (D-17). Provider impls
// (`groq.rs`, `openai.rs`) live in `ironhermes-tools`.
//
// D-19: Module paths compile-time registered via BUILTIN_STT_NAMES.
// D-04: BUILTIN_STT_NAMES is the compile-time source of truth for the built-in set.
// Errors: anyhow::Result<String> — inter-crate transport uses plain String (D-19).

use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Minimal STT provider trait surface for Phase 36.17.8.
///
/// Mirrors the shape of the Python `SttProvider` ABC from `stt_provider.py`:
/// `name`, `display_name`, `is_available`, `transcribe`.
///
/// Object-safe with `async-trait`: the macro desugars `async fn` to
/// `Pin<Box<dyn Future + Send>>`, which is object-safe in `Arc<dyn SttProvider>`.
#[async_trait]
pub trait SttProvider: Send + Sync {
    /// Canonical lowercase provider name (e.g. `"groq"`, `"openai"`).
    fn name(&self) -> &str;

    /// Human-readable display name. Defaults to `self.name()`.
    fn display_name(&self) -> &str {
        self.name()
    }

    /// Returns `true` if the provider is usable in the current environment.
    ///
    /// For keyed providers (Groq, OpenAI) this checks the relevant env var.
    fn is_available(&self) -> bool;

    /// Transcribe audio at `wav_path` and return the transcript as a `String`.
    ///
    /// Returns the transcript text. Callers should trim leading/trailing
    /// whitespace from the result before display.
    async fn transcribe(&self, wav_path: &Path) -> anyhow::Result<String>;
}

/// D-04: Compile-time frozen set of built-in STT provider names.
///
/// `SttRegistry::register()` accepts only names in this set; any other name
/// triggers a `tracing::warn!` and is ignored. This pre-positions the invariant
/// for a future user-extension reversal without changing call sites.
pub const BUILTIN_STT_NAMES: &[&str] = &["groq", "openai"];

/// Registry mapping provider names to `Arc<dyn SttProvider>` instances.
///
/// Populated at startup by `ironhermes-tools::build_stt_registry()`.
/// Keyed by lowercase provider name for case-insensitive lookup.
pub struct SttRegistry {
    providers: HashMap<String, Arc<dyn SttProvider>>,
}

impl SttRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider.
    ///
    /// The provider's `name()` is lowercased before insertion.
    /// If the lowercased name is **not** in `BUILTIN_STT_NAMES`, the registration
    /// is rejected with a `tracing::warn!` (D-04 built-ins-always-win invariant).
    pub fn register(&mut self, provider: Arc<dyn SttProvider>) {
        let key = provider.name().to_lowercase();
        if BUILTIN_STT_NAMES.contains(&key.as_str()) {
            self.providers.insert(key, provider);
            return;
        }
        tracing::warn!(
            "SttRegistry::register: '{}' is not in BUILTIN_STT_NAMES; ignored",
            key
        );
    }

    /// Look up a provider by name (case-insensitive).
    ///
    /// Returns `None` if no provider with that name has been registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn SttProvider>> {
        self.providers.get(&name.to_lowercase()).cloned()
    }
}

impl Default for SttRegistry {
    fn default() -> Self {
        Self::new()
    }
}
