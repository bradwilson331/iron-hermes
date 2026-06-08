//! `notifier.toml` parser for the subscribe_boards config (Phase 36.3.7.9, D-03).
//!
//! # Design
//!
//! The default shipped value is `subscribe_boards = ["*"]`, meaning the notifier
//! loop sweeps ALL boards (default + every named board). A missing or malformed
//! `notifier.toml` silently falls back to this default (Pitfall 5) — the notifier
//! loop MUST NOT crash on user-supplied config errors.
//!
//! The `Explicit` variant ships inert in v1: the codepath is compiled and unit-tested
//! but `run_notifier_tick` does not surface a UI to activate it (D-03 inert-explicit
//! invariant). A future phase will expose the explicit-list surface when a
//! multi-profile use-case requires it.
//!
//! # Pitfall 2 — default must be synthesized first
//!
//! `resolve_subscribe_boards(All)` prepends `"default"` before the list returned
//! by `paths::list_boards()`. The `list_boards()` helper deliberately excludes
//! `"default"` because the default board lives at a legacy path
//! (`~/.ironhermes/kanban.db`) rather than under `boards/`. Callers must not
//! assume that `list_boards()` alone covers all active boards.

use serde::Deserialize;

// ---------------------------------------------------------------------------
// NotifierToml — parsed representation of notifier.toml
// ---------------------------------------------------------------------------

/// Deserialized form of `~/.ironhermes/kanban/notifier.toml`.
///
/// All fields have serde defaults so a completely absent file — or a file
/// that omits any field — behaves identically to the built-in default.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct NotifierToml {
    #[serde(default = "default_subscribe_boards")]
    pub subscribe_boards: Vec<String>,
}

fn default_subscribe_boards() -> Vec<String> {
    vec!["*".to_string()]
}

impl Default for NotifierToml {
    fn default() -> Self {
        Self {
            subscribe_boards: default_subscribe_boards(),
        }
    }
}

impl NotifierToml {
    /// Classify the `subscribe_boards` list into a typed config variant.
    ///
    /// - If the list contains `"*"` anywhere → [`SubscribeBoardsConfig::All`].
    /// - Otherwise → [`SubscribeBoardsConfig::Explicit`] with the literal list.
    pub fn resolve(&self) -> SubscribeBoardsConfig {
        if self.subscribe_boards.iter().any(|s| s == "*") {
            SubscribeBoardsConfig::All
        } else {
            SubscribeBoardsConfig::Explicit(self.subscribe_boards.clone())
        }
    }
}

// ---------------------------------------------------------------------------
// SubscribeBoardsConfig — typed result of NotifierToml::resolve()
// ---------------------------------------------------------------------------

/// Typed representation of the `subscribe_boards` config value.
///
/// The `Explicit` variant ships **inert** in v1 (D-03): the codepath is
/// compiled and unit-tested, but `run_notifier_tick` only activates it when
/// a future phase exposes the configuration surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeBoardsConfig {
    /// Glob-all: the notifier sweeps `"default"` plus every board returned
    /// by [`crate::paths::list_boards`] (Pitfall 2 — `"default"` is prepended).
    All,
    /// Literal slug list: the notifier sweeps only the listed slugs.
    /// Activated by a future phase; ships inert in v1 (D-03).
    Explicit(Vec<String>),
}

// ---------------------------------------------------------------------------
// load_notifier_toml — reads notifier.toml with silent fallback (Pitfall 5)
// ---------------------------------------------------------------------------

/// Read `notifier.toml` from [`crate::paths::notifier_config_path`].
///
/// # Fallback policy (Pitfall 5)
///
/// - File absent (ENOENT) → [`NotifierToml::default`] silently.
/// - Other IO error → [`NotifierToml::default`] with a `warn` log.
/// - TOML parse error → [`NotifierToml::default`] with a `warn` log.
///
/// The notifier loop MUST NOT crash because the user has a malformed config.
pub fn load_notifier_toml() -> NotifierToml {
    let path = crate::paths::notifier_config_path();
    match std::fs::read_to_string(&path) {
        Ok(s) => match toml::from_str::<NotifierToml>(&s) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(
                    "kanban notifier: failed to parse {:?}: {e}; using default config",
                    path
                );
                NotifierToml::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => NotifierToml::default(),
        Err(e) => {
            tracing::warn!(
                "kanban notifier: failed to read {:?}: {e}; using default config",
                path
            );
            NotifierToml::default()
        }
    }
}

// ---------------------------------------------------------------------------
// resolve_subscribe_boards — expand SubscribeBoardsConfig into a slug list
// ---------------------------------------------------------------------------

/// Expand a [`SubscribeBoardsConfig`] into the concrete board slugs to sweep.
///
/// # Pitfall 2 — default is prepended for `All`
///
/// For [`SubscribeBoardsConfig::All`], `"default"` is always first in the
/// returned list, followed by the named boards from [`crate::paths::list_boards`]
/// (sorted alphabetically, `_`-prefixed entries excluded). This is correct
/// because the default board lives outside the `boards/` directory.
///
/// For [`SubscribeBoardsConfig::Explicit`], the caller's literal list is
/// returned as-is. Slug validation is the caller's responsibility.
pub fn resolve_subscribe_boards(cfg: &SubscribeBoardsConfig) -> std::io::Result<Vec<String>> {
    match cfg {
        SubscribeBoardsConfig::All => {
            let mut all = vec!["default".to_string()];
            all.extend(crate::paths::list_boards()?);
            Ok(all)
        }
        SubscribeBoardsConfig::Explicit(list) => Ok(list.clone()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // RAII guard that sets an env var and restores the previous value on drop.
    // Required for tests that mutate IRONHERMES_HOME to avoid parallel-test races.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: test context — guarded by ENV_LOCK in callers.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    // Env-mutating tests serialise via the process-wide crate::ENV_LOCK.

    // -----------------------------------------------------------------------
    // 1. default_is_subscribe_all_star
    // -----------------------------------------------------------------------

    #[test]
    fn default_is_subscribe_all_star() {
        let cfg = NotifierToml::default();
        assert_eq!(
            cfg.subscribe_boards,
            vec!["*".to_string()],
            "default subscribe_boards must be [\"*\"]"
        );
    }

    // -----------------------------------------------------------------------
    // 2. parse_explicit_list
    // -----------------------------------------------------------------------

    #[test]
    fn parse_explicit_list() {
        let src = r#"subscribe_boards = ["alpha", "beta"]"#;
        let parsed: NotifierToml = toml::from_str(src).expect("parse explicit list");
        assert_eq!(
            parsed.subscribe_boards,
            vec!["alpha".to_string(), "beta".to_string()]
        );
        assert_eq!(
            parsed.resolve(),
            SubscribeBoardsConfig::Explicit(vec!["alpha".to_string(), "beta".to_string()])
        );
    }

    // -----------------------------------------------------------------------
    // 3. parse_subscribe_all_via_star
    // -----------------------------------------------------------------------

    #[test]
    fn parse_subscribe_all_via_star() {
        let src = r#"subscribe_boards = ["*"]"#;
        let parsed: NotifierToml = toml::from_str(src).expect("parse star");
        assert_eq!(parsed.resolve(), SubscribeBoardsConfig::All);
    }

    // -----------------------------------------------------------------------
    // 4. missing_field_uses_default (toml with only an unrelated section)
    // -----------------------------------------------------------------------

    #[test]
    fn missing_field_uses_default() {
        // A toml file that has no subscribe_boards field at all.
        let src = "[other_section]\nfoo = \"bar\"";
        let parsed: NotifierToml = toml::from_str(src).expect("parse missing field");
        assert_eq!(
            parsed.subscribe_boards,
            vec!["*".to_string()],
            "missing subscribe_boards must default to [\"*\"]"
        );
    }

    // -----------------------------------------------------------------------
    // 5. resolve_all_prepends_default
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_all_prepends_default() {
        let _lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let boards = dir.path().join("kanban").join("boards");
        fs::create_dir_all(boards.join("alpha")).unwrap();
        fs::create_dir_all(boards.join("beta")).unwrap();

        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let result = resolve_subscribe_boards(&SubscribeBoardsConfig::All)
            .expect("resolve_all should succeed");

        assert_eq!(
            result,
            vec![
                "default".to_string(),
                "alpha".to_string(),
                "beta".to_string()
            ],
            "All must produce [default, alpha, beta] (Pitfall 2)"
        );
    }

    // -----------------------------------------------------------------------
    // 6. resolve_explicit_returns_literal_list
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_explicit_returns_literal_list() {
        let slugs = vec!["foo".to_string(), "bar".to_string()];
        let result = resolve_subscribe_boards(&SubscribeBoardsConfig::Explicit(slugs.clone()))
            .expect("resolve explicit");
        assert_eq!(result, slugs);
    }

    // -----------------------------------------------------------------------
    // 7. resolve_all_with_no_boards_dir_returns_default_only
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_all_with_no_boards_dir_returns_default_only() {
        let _lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // No boards/ directory created — list_boards() should return empty.
        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let result = resolve_subscribe_boards(&SubscribeBoardsConfig::All)
            .expect("resolve_all with no boards dir");

        assert_eq!(
            result,
            vec!["default".to_string()],
            "All with no named boards must return only [\"default\"]"
        );
    }

    // -----------------------------------------------------------------------
    // 8. parse_error_uses_default (T-36.3.7.9-05-01 lock)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_error_uses_default() {
        let _lock = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let kanban_dir = dir.path().join("kanban");
        fs::create_dir_all(&kanban_dir).unwrap();
        // Write malformed TOML to trigger a parse error
        let cfg_path = kanban_dir.join("notifier.toml");
        fs::write(&cfg_path, b"subscribe_boards = [[[not valid toml").unwrap();

        let _guard = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

        let result = load_notifier_toml();
        assert_eq!(
            result.subscribe_boards,
            vec!["*".to_string()],
            "parse error must fall back to default (T-36.3.7.9-05-01)"
        );
    }
}
