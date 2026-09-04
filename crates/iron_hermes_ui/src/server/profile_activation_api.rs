//! Phase 49.4 Plan 08 (D-14): persisted profile activation-with-scope.
//!
//! Before this module, "the active profile" was purely a per-process fact —
//! `ironhermes_core::current_profile()` reverse-walks `get_hermes_home()` for
//! a `profiles/<slug>` ancestor at call time, so it can only ever answer
//! "which profile is THIS process running under", never "which profile did
//! the operator ask for". This module adds the missing layer: an explicit,
//! persisted operator intent (`ActiveProfileRecord`) with a two-level scope
//! (`ActivationScope::ChatOnly` / `Everywhere`), plus the single resolver
//! (`resolve_active_profile_for`) every read site is meant to call instead of
//! `current_profile()` directly.
//!
//! Checkpoint decision (resolved by the human operator before this plan's
//! Task 2 ran, never auto-approved — `gate="blocking-human"`): storage is a
//! new key block (`Config.active_profile`, `crates/ironhermes-core/src/config.rs`)
//! read via the existing config loader — option `config-key` from the three
//! offered. The "everywhere" scope affects newly spawned gateway/worker
//! processes only; an already-running process keeps its profile until
//! restart (no live re-read machinery this phase).
//!
//! Storage shape: `ironhermes_core::config::ActiveProfileConfig` is the
//! on-disk plain-`Option<String>` pair (that crate cannot depend on this
//! crate's wire DTOs — the dependency runs the other direction). This
//! module's [`read_active_profile_record`] / [`write_active_profile_record`]
//! are the ONLY place that shape is parsed into / rendered from the wire
//! DTOs ([`crate::protocol::ActiveProfileRecord`] /
//! [`crate::protocol::ActivationScope`]).
//!
//! Additive by construction: with no record persisted, every surface keeps
//! resolving to `current_profile()` exactly as before this module existed —
//! an un-activated install is byte-for-byte unaffected.
//!
//! Read-site audit (Task 2's own instruction: enumerate every
//! `current_profile()` call site workspace-wide before wiring any). The
//! full grep and its per-site verdict is recorded in
//! `49.4-08-SUMMARY.md` — summary of the finding: every EXISTING call site
//! is either (a) an artifact/task-tagging identity read inside a process
//! that IS already the resolved profile (`ironhermes-tools::artifact`,
//! `delegate_task`, `chat_capture`), (b) a spawn-time metadata stamp for the
//! OPERATOR's own gallery bucket (`ironhermes-kanban::worker_spawn`), (c) a
//! live-profile safety guard comparison (`cli_handoff::run_bot_handoff`), or
//! (d) a duplicate-algorithm identity/badge helper (`server::api`,
//! `bot_meta_api::live_profile_name`). None of them is a "which profile
//! should this NEW chat/editor/gateway-worker surface use" decision point —
//! the embedded chat runtime's `AppState` loads `Config` once at boot
//! (`state.rs::init`) and has no per-message profile-resolution site yet,
//! no "editor" concept exists in this codebase at all yet, and the Gateway/
//! Tools screens' scope selectors hardcode `ConfigScope::Root` as a literal
//! UI default rather than deriving it from anything. All are therefore
//! OUT OF SCOPE for rewriting in this plan — wiring any of them would also
//! reach outside this plan's declared `files_modified`
//! (`profile_activation_api.rs`, `profile_api.rs`, `server/mod.rs`,
//! `protocol.rs`). [`resolve_active_profile_for`] is the primitive plans
//! 10-12 (Soul page, topbar quick-switch) will call once they build the UI
//! surfaces that actually make this decision.

use dioxus::prelude::*;

use crate::protocol::{ActivationScope, ActiveProfileRecord};

/// Phase 49.4 Plan 08 (D-14): the three surfaces an activation record's
/// scope can cover. `ChatOnly` covers only [`ActivationSurface::Chat`];
/// `Everywhere` covers all three. No production call site constructs this
/// enum yet (see the read-site audit in this module's doc comment) — it
/// exists so [`resolve_active_profile_for`] has a real, unit-tested
/// contract for plans 10-12 to call into.
// `#[allow(dead_code)]`: no production call site constructs this enum yet
// (the read-site audit in this module's doc comment found none within this
// plan's declared `files_modified`) — it is exercised today only by this
// module's own tests, which the `--all-targets`-agnostic plain lib build
// does not see. Same class of forward-declared, test-exercised-only item as
// `profile_api::provider_key_env_name_for`.
#[cfg(feature = "server")]
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivationSurface {
    Chat,
    EditorDefault,
    GatewayWorkerDefault,
}

#[cfg(feature = "server")]
impl ActivationScope {
    /// Whether this scope's activation reaches `surface`. See
    /// `ActivationSurface`'s doc comment for the `#[allow(dead_code)]`
    /// rationale — this method has the same "no production caller yet"
    /// status.
    #[allow(dead_code)]
    fn covers(self, surface: ActivationSurface) -> bool {
        match self {
            ActivationScope::Everywhere => true,
            ActivationScope::ChatOnly => matches!(surface, ActivationSurface::Chat),
        }
    }
}

/// Phase 49.4 Plan 08: current wall-clock time in milliseconds. Never
/// panics — a clock error (time before the Unix epoch) degrades to `0`
/// rather than crashing a write. Duplicated verbatim from
/// `bot_meta_api::now_ms` per this crate's own established "each module
/// duplicates this tiny helper" precedent (see that fn's doc comment).
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 49.4 Plan 08 (D-14): parse the on-disk [`ironhermes_core::config::ActiveProfileConfig`]
/// shape into the wire [`ActiveProfileRecord`] DTO. Returns `None` — never
/// panics or propagates an error — for every "no valid record" case: no
/// name persisted, or a `scope` string that is not exactly `"chat_only"` or
/// `"everywhere"` (a malformed/foreign value). A `None` here is what makes
/// [`resolve_active_profile_for`] fall back to `current_profile()`.
#[cfg(feature = "server")]
fn parse_active_profile_record(
    raw: &ironhermes_core::config::ActiveProfileConfig,
) -> Option<ActiveProfileRecord> {
    let name = raw.name.clone()?;
    let scope = match raw.scope.as_deref() {
        Some("chat_only") => ActivationScope::ChatOnly,
        Some("everywhere") => ActivationScope::Everywhere,
        other => {
            tracing::warn!(
                scope = ?other,
                "profile_activation: malformed/unknown activation scope on disk; \
                 falling back to the environment-derived active profile"
            );
            return None;
        }
    };
    Some(ActiveProfileRecord {
        name,
        scope,
        updated_at_ms: raw.updated_at_ms.unwrap_or(0),
    })
}

/// Phase 49.4 Plan 08 (D-14): render the wire DTO into the on-disk shape —
/// the inverse of [`parse_active_profile_record`].
#[cfg(feature = "server")]
fn render_active_profile_record(record: &ActiveProfileRecord) -> ironhermes_core::config::ActiveProfileConfig {
    ironhermes_core::config::ActiveProfileConfig {
        name: Some(record.name.clone()),
        scope: Some(match record.scope {
            ActivationScope::ChatOnly => "chat_only",
            ActivationScope::Everywhere => "everywhere",
        }.to_string()),
        updated_at_ms: Some(record.updated_at_ms),
    }
}

/// Phase 49.4 Plan 08 (D-14): read the persisted activation record, if any.
///
/// A config load failure (missing/unparseable `config.yaml`) is logged and
/// treated identically to "no record" — this fn never panics and never
/// propagates an error, because every caller's fallback (the
/// environment-derived active profile) is always available regardless.
#[cfg(feature = "server")]
pub(crate) fn read_active_profile_record() -> Option<ActiveProfileRecord> {
    let config = match ironhermes_core::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "profile_activation: failed to load config while reading the activation \
                 record; falling back to the environment-derived active profile"
            );
            return None;
        }
    };
    parse_active_profile_record(&config.active_profile)
}

/// Phase 49.4 Plan 08 (D-14): persist an activation record. Loads a fresh
/// `Config` (never the caller's possibly-stale copy — matches
/// `create_profile_impl`'s own discipline), overwrites only the
/// `active_profile` block, and saves via `Config::save_to`'s existing
/// write-then-rename atomic strategy (`config.rs:3735` — no new atomic-write
/// primitive is introduced by this module). Caller is responsible for the
/// fail-closed write gate ([`crate::server::profile_api::check_profile_write_gate`])
/// — this fn does not check it, matching `create_profile_impl`'s own
/// separation of concerns (the `#[server]` wrapper gates, the impl writes).
#[cfg(feature = "server")]
pub(crate) fn write_active_profile_record(record: &ActiveProfileRecord) -> Result<(), String> {
    let mut config =
        ironhermes_core::config::Config::load().map_err(|e| format!("Config load failed: {e}"))?;
    config.active_profile = render_active_profile_record(record);
    config
        .save()
        .map_err(|e| format!("Config save failed: {e}"))
}

/// Phase 49.4 Plan 08 (D-14): the single resolver every read site is meant
/// to call instead of `ironhermes_core::current_profile()` directly.
///
/// `bot_binding_override`: when `Some`, a bot's explicit profile binding —
/// this ALWAYS short-circuits the persisted record, regardless of its scope
/// (bot bindings win, per this plan's `must_haves.truths`). Plan 12 owns
/// the binding store itself; this parameter is the contract point a future
/// caller threads its resolved binding through. Pass `None` when the
/// caller has no explicit binding to check.
///
/// Resolution order:
/// 1. `bot_binding_override`, if `Some` — returned immediately.
/// 2. The persisted record, if one exists AND its scope covers `surface`.
/// 3. `ironhermes_core::current_profile()` — the pre-existing
///    environment-derived fallback, used both when no record exists and
///    when the record's scope does not reach `surface`.
///
/// `#[allow(dead_code)]`: no production call site invokes this yet — see
/// `ActivationSurface`'s doc comment for the shared rationale.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn resolve_active_profile_for(
    surface: ActivationSurface,
    bot_binding_override: Option<&str>,
) -> String {
    if let Some(bound) = bot_binding_override {
        return bound.to_string();
    }
    match read_active_profile_record() {
        Some(record) if record.scope.covers(surface) => record.name,
        _ => ironhermes_core::current_profile(),
    }
}

/// Phase 49.4 Plan 08 (D-14): activate a profile with a scope. Four-step
/// write protocol (mirrors `update_provider_config`,
/// `provider_config_api.rs:240-281`, and `profile_api::create_profile`):
///
/// 1. validate `name` through the shared profile-name validator — before
///    any path is resolved (T-49.4-08-02-adjacent discipline)
/// 2. reject a name whose profile directory does not exist
/// 3. `Config::load()` fresh from disk
/// 4. fail-closed gate (`security.web_config_write_enabled`) — reused
///    verbatim from `profile_api::check_profile_write_gate`, the same flag
///    every other browser-reachable config/credential write in this crate
///    enforces
///
/// Persistence itself runs off the async runtime via `spawn_blocking`,
/// matching every other write in this crate.
#[server]
pub async fn activate_profile(name: String, scope: ActivationScope) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        // Step 1: validate the name before any path is resolved.
        let validated = ironhermes_core::profile::validate_profile_name(&name)
            .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;

        // Reject a name with no profile directory on disk. Runs before the
        // config load/gate check (matches this plan's own behavior bullet:
        // "leaves the previous record unchanged").
        if !crate::server::profile_api::profile_dir_for(&validated).is_dir() {
            return Err(ServerFnError::new(format!(
                "profile '{validated}' does not exist"
            )));
        }

        // Step 3: fresh disk read (NOT app_state.config — the startup snapshot).
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;

        // Step 4: fail-closed gate.
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let record = ActiveProfileRecord {
            name: validated,
            scope,
            updated_at_ms: now_ms(),
        };

        tokio::task::spawn_blocking(move || write_active_profile_record(&record))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;

        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (name, scope);
        Err(ServerFnError::new(
            "activate_profile unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4 Plan 08 (D-14): read the current activation record for the
/// UI to render — the Soul page / topbar's "what is active right now"
/// question. A read, ungated like `list_profiles`/`get_provider_config`
/// (matches this crate's convention that reads are never behind
/// `web_config_write_enabled`).
#[server]
pub async fn get_active_profile() -> Result<Option<ActiveProfileRecord>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let record = tokio::task::spawn_blocking(read_active_profile_record)
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?;
        Ok(record)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "get_active_profile unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod profile_activation_tests {
    use super::*;

    /// RAII guard, duplicated per this crate's own established precedent
    /// (see `profile_api.rs::profile_scaffold_tests::ScopedEnv`'s doc
    /// comment — each `#[cfg(test)]` module is its own namespace).
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; no concurrent env access.
            unsafe { std::env::set_var(key, value) };
            Self {
                key: key.to_string(),
                prev,
            }
        }
    }

    impl Drop for ScopedEnv {
        fn drop(&mut self) {
            // SAFETY: single-threaded test context; no concurrent env access.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(&self.key, v) },
                None => unsafe { std::env::remove_var(&self.key) },
            }
        }
    }

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    fn scaffold_profile(home_dir: &std::path::Path, name: &str) {
        std::fs::create_dir_all(home_dir.join("profiles").join(name)).expect("mkdir profile dir");
    }

    // -------------------------------------------------------------------
    // Behavior 1: no record persisted -> resolver matches the
    // environment-derived function exactly, for every surface.
    // -------------------------------------------------------------------

    #[test]
    fn no_record_resolves_to_environment_derived_profile_for_every_surface() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scoped_home = dir.path().join("profiles").join("env-bot");
        std::fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", scoped_home.to_str().expect("utf8 path"));

        let expected = ironhermes_core::current_profile();
        assert_eq!(expected, "env-bot");

        for surface in [
            ActivationSurface::Chat,
            ActivationSurface::EditorDefault,
            ActivationSurface::GatewayWorkerDefault,
        ] {
            assert_eq!(resolve_active_profile_for(surface, None), expected);
        }
    }

    // -------------------------------------------------------------------
    // Behavior 2: chat-only scope reaches Chat, not the other two surfaces.
    // -------------------------------------------------------------------

    #[test]
    fn chat_only_scope_covers_chat_but_leaves_other_surfaces_environment_derived() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "chat-bot");

        let record = ActiveProfileRecord {
            name: "chat-bot".to_string(),
            scope: ActivationScope::ChatOnly,
            updated_at_ms: 1,
        };
        write_active_profile_record(&record).expect("write should succeed");

        assert_eq!(
            resolve_active_profile_for(ActivationSurface::Chat, None),
            "chat-bot"
        );
        let env_derived = ironhermes_core::current_profile();
        assert_eq!(
            resolve_active_profile_for(ActivationSurface::EditorDefault, None),
            env_derived
        );
        assert_eq!(
            resolve_active_profile_for(ActivationSurface::GatewayWorkerDefault, None),
            env_derived
        );
    }

    // -------------------------------------------------------------------
    // Behavior 3: everywhere scope reaches all three surfaces.
    // -------------------------------------------------------------------

    #[test]
    fn everywhere_scope_covers_all_three_surfaces() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "everywhere-bot");

        let record = ActiveProfileRecord {
            name: "everywhere-bot".to_string(),
            scope: ActivationScope::Everywhere,
            updated_at_ms: 1,
        };
        write_active_profile_record(&record).expect("write should succeed");

        for surface in [
            ActivationSurface::Chat,
            ActivationSurface::EditorDefault,
            ActivationSurface::GatewayWorkerDefault,
        ] {
            assert_eq!(
                resolve_active_profile_for(surface, None),
                "everywhere-bot"
            );
        }
    }

    // -------------------------------------------------------------------
    // Behavior 4: activating a nonexistent profile errors and leaves the
    // previous record unchanged.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn activate_profile_impl_on_missing_profile_directory_leaves_record_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "prior-bot");

        let prior = ActiveProfileRecord {
            name: "prior-bot".to_string(),
            scope: ActivationScope::ChatOnly,
            updated_at_ms: 42,
        };
        write_active_profile_record(&prior).expect("seed prior record");

        let result = activate_profile("never-existed".to_string(), ActivationScope::Everywhere)
            .await;
        assert!(result.is_err(), "activating a nonexistent profile must error");

        let after = read_active_profile_record().expect("prior record must still exist");
        assert_eq!(after.name, "prior-bot");
        assert_eq!(after.scope, ActivationScope::ChatOnly);
    }

    // -------------------------------------------------------------------
    // Behavior 5: an invalid name is rejected before any path is resolved.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn activate_profile_rejects_invalid_name_before_resolving_any_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result = activate_profile("../../etc/passwd".to_string(), ActivationScope::ChatOnly)
            .await;
        assert!(result.is_err());
        assert!(
            read_active_profile_record().is_none(),
            "a validation-rejected name must never write a record"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 6: the web config write gate disabled refuses and persists
    // nothing.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn activate_profile_refuses_when_write_gate_disabled_and_persists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "gated-bot");

        let mut config = ironhermes_core::config::Config::default();
        config.security.web_config_write_enabled = false;
        config.save().expect("seed disabled-gate config");

        let result = activate_profile("gated-bot".to_string(), ActivationScope::ChatOnly).await;
        assert!(result.is_err(), "a disabled write gate must refuse activation");
        assert!(
            read_active_profile_record().is_none(),
            "a gate refusal must persist nothing"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 7: the record round-trips.
    // -------------------------------------------------------------------

    #[test]
    fn active_profile_record_round_trips_through_write_and_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "roundtrip-bot");

        let record = ActiveProfileRecord {
            name: "roundtrip-bot".to_string(),
            scope: ActivationScope::Everywhere,
            updated_at_ms: 12345,
        };
        write_active_profile_record(&record).expect("write should succeed");

        let read_back = read_active_profile_record().expect("record must be present");
        assert_eq!(read_back, record);
    }

    // -------------------------------------------------------------------
    // Behavior 8: a bot binding override always wins, regardless of the
    // active record or its scope.
    // -------------------------------------------------------------------

    #[test]
    fn bot_binding_override_wins_regardless_of_active_record_or_scope() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "activated-bot");

        let record = ActiveProfileRecord {
            name: "activated-bot".to_string(),
            scope: ActivationScope::Everywhere,
            updated_at_ms: 1,
        };
        write_active_profile_record(&record).expect("write should succeed");

        assert_eq!(
            resolve_active_profile_for(ActivationSurface::Chat, Some("bound-bot")),
            "bound-bot"
        );
        assert_eq!(
            resolve_active_profile_for(ActivationSurface::GatewayWorkerDefault, Some("bound-bot")),
            "bound-bot"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 9: a malformed persisted record falls back to the
    // environment-derived profile rather than panicking.
    // -------------------------------------------------------------------

    #[test]
    fn malformed_scope_falls_back_to_environment_derived_profile_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scoped_home = dir.path().join("profiles").join("fallback-bot");
        std::fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", scoped_home.to_str().expect("utf8 path"));

        let mut config = ironhermes_core::config::Config::default();
        config.active_profile = ironhermes_core::config::ActiveProfileConfig {
            name: Some("some-other-bot".to_string()),
            scope: Some("not-a-real-scope".to_string()),
            updated_at_ms: Some(1),
        };
        config.save().expect("seed malformed record");

        assert!(read_active_profile_record().is_none());
        assert_eq!(
            resolve_active_profile_for(ActivationSurface::Chat, None),
            "fallback-bot"
        );
    }

    #[test]
    fn missing_config_falls_back_to_none_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // No config.yaml written at all — Config::load() returns Ok(default)
        // for a missing file (see `Config::load_from`'s own contract), so
        // this exercises the "no record" path via a genuinely absent file
        // rather than a malformed one.
        assert!(read_active_profile_record().is_none());
    }
}
