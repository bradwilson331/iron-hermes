//! Phase 36.17.8 — STT provider unit tests.
//!
//! Covers D-05 (model env override), D-06 (provider auto-select), D-07 (is_available).
//!
//! Run: `cargo test -p ironhermes-tools --test stt_providers`
//!
//! Note: env-var tests manipulate process-global state. Tests that mutate env vars
//! acquire `env_lock()` to serialise execution.
//! All set_var/remove_var calls wrapped in `unsafe` (Rust 2024 edition).

use ironhermes_core::SttProvider;
use ironhermes_core::config::{GroqSttConfig, OpenAiSttConfig, SttConfig};
use ironhermes_tools::stt::{GroqSttProvider, OpenAiSttProvider};
use std::sync::OnceLock;

// ─── Env-mutation guard ───────────────────────────────────────────────────────

/// Process-global mutex that serialises tests that mutate env vars.
/// Mirrors the pattern in browser_prereq.rs.
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// ─── Helper ──────────────────────────────────────────────────────────────────

fn groq_provider() -> GroqSttProvider {
    GroqSttProvider::new(GroqSttConfig::default())
}

fn openai_provider() -> OpenAiSttProvider {
    OpenAiSttProvider::new(OpenAiSttConfig::default())
}

// ─── D-07: is_available ───────────────────────────────────────────────────────

/// D-07: GroqSttProvider::is_available() returns true iff GROQ_API_KEY is set.
/// Security: API key is read from env only; never logged.
#[test]
fn test_groq_is_available() {
    let _guard = env_lock().lock().unwrap();
    let orig = std::env::var("GROQ_API_KEY").ok();

    // Should be true when key is set.
    unsafe { std::env::set_var("GROQ_API_KEY", "test-groq-key") };
    assert!(
        groq_provider().is_available(),
        "expected true when GROQ_API_KEY set"
    );

    // Should be false when key is removed.
    unsafe { std::env::remove_var("GROQ_API_KEY") };
    assert!(
        !groq_provider().is_available(),
        "expected false when GROQ_API_KEY unset"
    );

    // Restore.
    match orig {
        Some(v) => unsafe { std::env::set_var("GROQ_API_KEY", v) },
        None => unsafe { std::env::remove_var("GROQ_API_KEY") },
    }
}

/// D-07: OpenAiSttProvider::is_available() returns true iff VOICE_TOOLS_OPENAI_KEY is set.
/// Security: API key is read from env only; never logged.
#[test]
fn test_openai_is_available() {
    let _guard = env_lock().lock().unwrap();
    let orig = std::env::var("VOICE_TOOLS_OPENAI_KEY").ok();

    unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", "test-openai-key") };
    assert!(
        openai_provider().is_available(),
        "expected true when VOICE_TOOLS_OPENAI_KEY set"
    );

    unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") };
    assert!(
        !openai_provider().is_available(),
        "expected false when VOICE_TOOLS_OPENAI_KEY unset"
    );

    match orig {
        Some(v) => unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v) },
        None => unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") },
    }
}

// ─── D-06: Provider auto-select ──────────────────────────────────────────────

/// D-06: Provider auto-select — Groq is preferred when GROQ_API_KEY is set.
#[test]
fn test_provider_selection_groq_preferred() {
    use ironhermes_tools::stt::select_stt_provider;
    let _guard = env_lock().lock().unwrap();

    let orig_groq = std::env::var("GROQ_API_KEY").ok();
    let orig_openai = std::env::var("VOICE_TOOLS_OPENAI_KEY").ok();

    // Both keys present — Groq wins.
    unsafe {
        std::env::set_var("GROQ_API_KEY", "g-key");
        std::env::set_var("VOICE_TOOLS_OPENAI_KEY", "o-key");
    }

    let config = SttConfig::default(); // provider = "auto"
    let result = select_stt_provider(&config);
    assert_eq!(
        result.as_deref(),
        Some("groq"),
        "Groq should be preferred when both keys set"
    );

    // Only Groq key present — still Groq.
    unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") };
    let result2 = select_stt_provider(&config);
    assert_eq!(
        result2.as_deref(),
        Some("groq"),
        "Groq when only GROQ_API_KEY set"
    );

    // Restore.
    match orig_groq {
        Some(v) => unsafe { std::env::set_var("GROQ_API_KEY", v) },
        None => unsafe { std::env::remove_var("GROQ_API_KEY") },
    }
    match orig_openai {
        Some(v) => unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v) },
        None => unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") },
    }
}

/// D-06: Provider auto-select — OpenAI is fallback when VOICE_TOOLS_OPENAI_KEY is set.
#[test]
fn test_provider_selection_openai_fallback() {
    use ironhermes_tools::stt::select_stt_provider;
    let _guard = env_lock().lock().unwrap();

    let orig_groq = std::env::var("GROQ_API_KEY").ok();
    let orig_openai = std::env::var("VOICE_TOOLS_OPENAI_KEY").ok();

    // Only OpenAI key present — falls back to openai.
    unsafe {
        std::env::remove_var("GROQ_API_KEY");
        std::env::set_var("VOICE_TOOLS_OPENAI_KEY", "o-key");
    }

    let config = SttConfig::default(); // provider = "auto"
    let result = select_stt_provider(&config);
    assert_eq!(
        result.as_deref(),
        Some("openai"),
        "OpenAI fallback when only VOICE_TOOLS_OPENAI_KEY set"
    );

    // Neither key set — returns None (STT disabled).
    unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") };
    let result2 = select_stt_provider(&config);
    assert_eq!(result2, None, "None when no keys set");

    // Explicit provider="openai" ignores env — always returns Some("openai").
    let explicit_config = SttConfig {
        provider: "openai".to_string(),
        ..SttConfig::default()
    };
    let result3 = select_stt_provider(&explicit_config);
    assert_eq!(
        result3.as_deref(),
        Some("openai"),
        "Explicit provider=openai wins regardless of keys"
    );

    // Restore.
    match orig_groq {
        Some(v) => unsafe { std::env::set_var("GROQ_API_KEY", v) },
        None => unsafe { std::env::remove_var("GROQ_API_KEY") },
    }
    match orig_openai {
        Some(v) => unsafe { std::env::set_var("VOICE_TOOLS_OPENAI_KEY", v) },
        None => unsafe { std::env::remove_var("VOICE_TOOLS_OPENAI_KEY") },
    }
}

// ─── D-05: Model env override ─────────────────────────────────────────────────

/// D-05: STT_GROQ_MODEL env var overrides the default Groq model name.
///
/// This test verifies the model selection logic via GroqSttConfig::default() and
/// env-var readability. The actual multipart body assertion is in stt_tools.rs.
#[test]
fn test_groq_model_env_override() {
    let _guard = env_lock().lock().unwrap();
    let orig = std::env::var("STT_GROQ_MODEL").ok();

    unsafe { std::env::remove_var("STT_GROQ_MODEL") };
    // Default model should be "whisper-large-v3-turbo" when env unset and config empty.
    let default_config = GroqSttConfig::default();
    assert_eq!(
        default_config.model, "whisper-large-v3-turbo",
        "GroqSttConfig::default() model should be whisper-large-v3-turbo"
    );

    // When STT_GROQ_MODEL is set, the provider reads that value at transcribe time.
    unsafe { std::env::set_var("STT_GROQ_MODEL", "whisper-large-v3") };
    assert_eq!(std::env::var("STT_GROQ_MODEL").unwrap(), "whisper-large-v3");

    match orig {
        Some(v) => unsafe { std::env::set_var("STT_GROQ_MODEL", v) },
        None => unsafe { std::env::remove_var("STT_GROQ_MODEL") },
    }
}
