use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
pub const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

pub const NOUS_API_BASE_URL: &str = "https://inference-api.nousresearch.com/v1";
pub const NOUS_API_CHAT_URL: &str = "https://inference-api.nousresearch.com/v1/chat/completions";

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4-20250514";
pub const DEFAULT_MAX_ITERATIONS: usize = 90;
pub const DEFAULT_CONTEXT_LENGTH: usize = 128_000;
pub const DEFAULT_TOOL_DELAY_SECS: f64 = 1.0;

pub const VALID_REASONING_EFFORTS: &[&str] = &["xhigh", "high", "medium", "low", "minimal"];

/// Get the IronHermes home directory (default: ~/.ironhermes).
pub fn get_hermes_home() -> PathBuf {
    match std::env::var("IRONHERMES_HOME") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ironhermes"),
    }
}

/// Get a display-friendly path for the home directory.
pub fn display_hermes_home() -> String {
    let home = get_hermes_home();
    if let Some(user_home) = dirs::home_dir()
        && let Ok(relative) = home.strip_prefix(&user_home)
    {
        return format!("~/{}", relative.display());
    }
    home.display().to_string()
}
