use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
pub const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

pub const NOUS_API_BASE_URL: &str = "https://inference-api.nousresearch.com/v1";
pub const NOUS_API_CHAT_URL: &str = "https://inference-api.nousresearch.com/v1/chat/completions";

pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

pub const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-5";
// Lowered from 90 to bound the worst case when a small parent model loops on
// failed tool calls / delegations (see runaway-delegation guard in AgentLoop).
pub const DEFAULT_MAX_ITERATIONS: usize = 50;
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
/// — writes only to IRONHERMES_HOME/skills/. Same risk profile as "memory" (T-33-03-A).
/// web and code are disabled by default (require API keys / high blast radius).
pub const DEFAULT_TOOLSETS: &[&str] = &[
    "memory", "session", "agent", "skills", "robotics", "learning",
];

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
    "memory",
    "session",
    "agent",
    "skills",
    "robotics",
    "learning",
    "web",
    "code",
    "browser",
    "voice",     // Phase 36.17.5 D-15
    "artifacts", // Phase 46.6 D-01
];

/// Filename of the OAuth token store (D-06, D-08).
pub const AUTH_JSON_FILENAME: &str = "auth.json";

/// Returns the path to the OAuth token store (`~/.ironhermes/auth.json` or
/// `$IRONHERMES_HOME/auth.json`). The file is written at mode 0600 on first use
/// by `AuthStore::save_to_disk` (T-41-01, PITFALLS §A-1).
pub fn auth_json_path() -> PathBuf {
    get_hermes_home().join(AUTH_JSON_FILENAME)
}

/// Get the IronHermes home directory (default: ~/.ironhermes).
pub fn get_hermes_home() -> PathBuf {
    match std::env::var("IRONHERMES_HOME") {
        Ok(p) if !p.is_empty() => PathBuf::from(p),
        _ => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".ironhermes"),
    }
}

/// Env var (Phase 46.6 gap-closure, Option A): the profile bucket an artifact is
/// stamped with on publish. Set by the kanban dispatcher
/// (`build_kanban_worker_env`) to the OPERATOR's profile so unattended kanban
/// producers land in the operator's gallery bucket rather than the worker's
/// assignee bucket. Mirrors `IRONHERMES_ARTIFACTS_DB` (which redirects the store
/// FILE) — this redirects the profile COLUMN. When unset, producers fall back to
/// [`current_profile`] (chat/delegate run in the operator's own process, so that
/// already resolves to the operator's profile).
pub const ARTIFACTS_PROFILE_ENV: &str = "IRONHERMES_ARTIFACTS_PROFILE";

/// The active profile identifier: reverse-walk [`get_hermes_home`] for a
/// `<PROFILES_SUBDIR>/<slug>` ancestor and return `<slug>`, else the literal
/// `"default"`.
///
/// Single source of truth shared by the artifacts publish path
/// (`ironhermes-tools`), the gallery list (`iron_hermes_ui`), and the kanban
/// dispatcher (`ironhermes-kanban`) — so a producer and the operator gallery
/// never disagree on which profile owns an artifact. Mirrors the algorithm in
/// `ironhermes-cli::status_cmd::current_profile`.
pub fn current_profile() -> String {
    let home = get_hermes_home();
    let components: Vec<_> = home.components().collect();
    for window in components.windows(2) {
        if let std::path::Component::Normal(name) = window[0]
            && name == PROFILES_SUBDIR
            && let std::path::Component::Normal(slug) = window[1]
        {
            return slug.to_string_lossy().to_string();
        }
    }
    "default".to_string()
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

/// `$IRONHERMES_HOME/sessions/<session_id>/workspace` — the chat session's
/// working directory for code/tool file I/O (D-21, Phase 46.7).
///
/// Mirrors the `kanban_workspace_for` construction style (join chain off
/// [`get_hermes_home`]). Pure path resolver — deterministic, side-effect-free,
/// does NOT create the directory; callers create on demand so the "workspace
/// is a fixed derivable location" contract holds even before first use.
pub fn session_workspace_dir(session_id: &str) -> PathBuf {
    get_hermes_home()
        .join("sessions")
        .join(session_id)
        .join("workspace")
}

/// `$IRONHERMES_HOME/sessions/<session_id>/attachments` — root for files
/// uploaded into a chat session, sibling of [`session_workspace_dir`] (D-21,
/// Phase 46.7).
///
/// Mirrors the `kanban_attachments_dir` construction style. Pure path
/// resolver — deterministic, side-effect-free, does NOT create the directory.
pub fn session_attachments_dir(session_id: &str) -> PathBuf {
    get_hermes_home()
        .join("sessions")
        .join(session_id)
        .join("attachments")
}

/// Validate an attachment's client-supplied filename before it is ever used
/// as a filesystem leaf (T-46.4-03 traversal defense, mirrored from
/// `ironhermes-kanban::store::add_attachment`).
///
/// Returns `Some(filename)` only when `filename` is non-empty and contains
/// no `".."`, no `'/'`, and no `'\\'`. Callers store bytes under an
/// opaque-id subdirectory using this validated leaf as the final path
/// segment so a malicious filename can never escape its per-attachment
/// directory (Phase 46.7 T-46.7-01).
pub fn safe_attachment_leaf(filename: &str) -> Option<&str> {
    if filename.is_empty() || filename.contains("..") || filename.contains(['/', '\\']) {
        return None;
    }
    Some(filename)
}

/// Validate a client-supplied `session_id` before it is ever joined into a
/// filesystem path by [`session_workspace_dir`] / [`session_attachments_dir`]
/// (Phase 46.7 CR-01 traversal defense).
///
/// Both dir resolvers are pure `join`s, so a `session_id` like
/// `"../../../../tmp/evil"` would escape `$IRONHERMES_HOME/sessions/`. This is
/// reachable from unauthenticated web entrypoints (the WS chat path and the
/// `upload_attachment`/`fetch_session_attachments` server fns), so every such
/// ingress MUST gate the id through this validator before the path helpers.
///
/// This is a STRUCTURAL guard, not a shape check: it accepts the canonical web
/// session key `agent:main:web:dm:<uuid>` (colons are not path separators) and
/// any other id that cannot escape a single directory level, while rejecting
/// empty, `".."`, `'/'`, `'\\'`, a leading `'~'` (home expansion), and any NUL
/// or ASCII control char. A passing id can only ever name one subdirectory
/// under `sessions/`, which is the intended containment.
pub fn safe_session_id(session_id: &str) -> Option<&str> {
    if session_id.is_empty()
        || session_id.contains("..")
        || session_id.contains(['/', '\\'])
        || session_id.starts_with('~')
        || session_id.chars().any(|c| c.is_control())
    {
        return None;
    }
    Some(session_id)
}

#[cfg(test)]
mod session_dir_helpers {
    use super::*;

    #[test]
    fn session_workspace_and_attachments_dirs_are_siblings_with_distinct_leaves() {
        let workspace = session_workspace_dir("s_abc");
        let attachments = session_attachments_dir("s_abc");

        assert_eq!(workspace.parent(), attachments.parent());
        assert_ne!(workspace, attachments);
        assert_eq!(workspace.file_name().unwrap(), "workspace");
        assert_eq!(attachments.file_name().unwrap(), "attachments");
        assert!(workspace.ends_with("sessions/s_abc/workspace"));
        assert!(attachments.ends_with("sessions/s_abc/attachments"));
    }

    #[test]
    fn distinct_session_ids_resolve_to_distinct_nonoverlapping_dirs() {
        let a_ws = session_workspace_dir("s_aaa");
        let b_ws = session_workspace_dir("s_bbb");
        let a_att = session_attachments_dir("s_aaa");
        let b_att = session_attachments_dir("s_bbb");

        assert_ne!(a_ws, b_ws);
        assert_ne!(a_att, b_att);
        assert_ne!(a_ws, a_att);
        assert_ne!(b_ws, b_att);
        assert!(!b_ws.starts_with(&a_ws));
        assert!(!a_ws.starts_with(&b_ws));
    }

    #[test]
    fn safe_attachment_leaf_rejects_empty_dotdot_and_separators() {
        assert_eq!(safe_attachment_leaf(""), None);
        assert_eq!(safe_attachment_leaf(".."), None);
        assert_eq!(safe_attachment_leaf("a/b"), None);
        assert_eq!(safe_attachment_leaf("a\\b"), None);
        assert_eq!(safe_attachment_leaf("../../etc/passwd"), None);
    }

    #[test]
    fn safe_attachment_leaf_accepts_plain_filename() {
        assert_eq!(safe_attachment_leaf("photo.png"), Some("photo.png"));
    }

    #[test]
    fn safe_session_id_rejects_traversal_and_separators() {
        // CR-01: these are the attacker-controlled vectors that would escape
        // $IRONHERMES_HOME/sessions/ via the pure-join dir resolvers.
        assert_eq!(safe_session_id(""), None);
        assert_eq!(safe_session_id(".."), None);
        assert_eq!(safe_session_id("../../../../tmp/evil"), None);
        assert_eq!(safe_session_id("/etc/passwd"), None);
        assert_eq!(safe_session_id("a/b"), None);
        assert_eq!(safe_session_id("a\\b"), None);
        assert_eq!(safe_session_id("~/secrets"), None);
        assert_eq!(safe_session_id("bad\0id"), None);
        assert_eq!(safe_session_id("line\nbreak"), None);
    }

    #[test]
    fn safe_session_id_accepts_canonical_and_plain_ids() {
        // The canonical web-DM key uses colons, which are NOT path separators.
        let web_key = "agent:main:web:dm:1e9d0e3a-2b7c-4a1f-9c2e-0f5b8a1d2c3e";
        assert_eq!(safe_session_id(web_key), Some(web_key));
        assert_eq!(safe_session_id("sess-up-1"), Some("sess-up-1"));
        assert_eq!(
            safe_session_id("1e9d0e3a2b7c4a1f9c2e0f5b8a1d2c3e"),
            Some("1e9d0e3a2b7c4a1f9c2e0f5b8a1d2c3e")
        );
    }

    #[test]
    fn safe_session_id_output_cannot_escape_sessions_dir() {
        // A validated id resolves to exactly one child under sessions/.
        let id = safe_session_id("agent:main:web:dm:abc").unwrap();
        let ws = session_workspace_dir(id);
        assert!(ws.ends_with("sessions/agent:main:web:dm:abc/workspace"));
    }
}
