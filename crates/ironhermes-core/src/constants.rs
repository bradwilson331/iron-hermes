use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
pub const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

pub const NOUS_API_BASE_URL: &str = "https://inference-api.nousresearch.com/v1";
pub const NOUS_API_CHAT_URL: &str = "https://inference-api.nousresearch.com/v1/chat/completions";

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4";
pub const DEFAULT_MAX_ITERATIONS: usize = 90;
pub const DEFAULT_CONTEXT_LENGTH: usize = 128_000;
pub const DEFAULT_TOOL_DELAY_SECS: f64 = 1.0;

pub const VALID_REASONING_EFFORTS: &[&str] = &["xhigh", "high", "medium", "low", "minimal"];

/// Memory subsystem constants (D-05, D-06)
pub const ENTRY_DELIMITER: &str = "\n\u{00a7}\n";
pub const MEMORY_CHAR_LIMIT: usize = 2_200;
pub const USER_CHAR_LIMIT: usize = 1_375;
pub const MEMORY_FILENAME: &str = "MEMORY.md";
pub const USER_FILENAME: &str = "USER.md";
pub const MEMORIES_DIR: &str = "memories";

/// Profile isolation constants (D-04, Phase 24)
pub const PROFILES_SUBDIR: &str = "profiles";

/// D-20 (Phase 25): toolsets enabled on a fresh install.
/// "memory", "session", "agent", "skills" are internal toolsets with no external prereqs.
/// "robotics" (Phase 27.1.1): toolset is enabled by default so HexapodTcpTool reaches
/// `is_available()`, which then gates on HEXAPOD_IP per Phase 27.1.1 D-13. Without this
/// entry, even a perfectly-configured robot would have its tool filtered out before
/// the prerequisite check runs.
/// "learning" (Phase 33): autonomous skill creation via skill_manage. No external prereqs
/// — writes only to HERMES_HOME/skills/. Same risk profile as "memory" (T-33-03-A).
/// web and code are disabled by default (require API keys / high blast radius).
pub const DEFAULT_TOOLSETS: &[&str] =
    &["memory", "session", "agent", "skills", "robotics", "learning"];

/// D-20 (Phase 27.1.1-gap-02): canonical exhaustive list of all known toolset names.
///
/// This is the single source of truth for the full toolset name set. Both
/// `toolset_cmd.rs::KNOWN_TOOLSETS` (CLI display/validation) and
/// `toolset_session.rs::members_map()` (slash dispatch) should agree with this list.
/// `with_default_toolsets_merged()` in `ToolsConfig` uses this to ensure every known
/// toolset has an entry after merging — absent entries default to `enabled: true`
/// (backward-compat: upgrading users don't silently lose access to new toolsets).
///
/// "browser" is disabled by default (high blast radius / requires chromium prereq);
/// "web" and "code" require external API keys. All other toolsets are enabled by default
/// as they have no external prerequisites.
pub const ALL_TOOLSETS: &[&str] = &[
    "memory", "session", "agent", "skills", "robotics", "learning", "web", "code", "browser",
];

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
