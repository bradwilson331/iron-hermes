//! Phase 49.4 Plan 12 (D-16): the profile-to-bot binding store.
//!
//! Checkpoint decision (resolved by the human operator, `gate="blocking-human"`,
//! before this plan's Task 2 ran — full text recorded in
//! `49.4-12-SUMMARY.md`'s Decisions section): a "bot" for this binding is a
//! gateway PLATFORM ADAPTER instance — one of the fixed, closed six keys
//! already enumerated as `GatewayConfig.platforms`'s map keys
//! (`gateway_platform_status_api.rs::PLATFORM_KEYS`: `"telegram"`,
//! `"discord"`, `"slack"`, `"buzz"`, `"webhook"`, `"api_server"`). This
//! module's own entry points (`list_bot_bindings`/`set_bot_binding`/
//! `clear_bot_binding`/`resolve_profile_for_bot`) are key-agnostic — they
//! accept any `String` key — so the fixed key list itself is never needed
//! server-side; `bot_roster.rs` and `soul.rs` (the wasm-client UI callers)
//! each carry their own literal copy of the six keys per this crate's own
//! "each module owns its tiny constants" precedent
//! (`profile_activation_api::now_ms`'s doc comment).
//!
//! The operator's own framing (verbatim, synthesized by the orchestrator
//! from two answers): platform adapters (telegram/discord/slack/buzz/
//! webhook/api_server) are COMMUNICATION PATHS to agent instances — they do
//! not own personas. The profile a path uses is CONFIGURATION the serving
//! agent instance loads. This binding is therefore spawn-time routing
//! configuration — a persisted mapping read when gateway/agent processes
//! spawn — NOT a live per-message persona override, matching D-14's
//! "newly spawned processes only" activation semantics exactly.
//!
//! Kanban workers are explicitly OUT of this store's scope (the operator's
//! own second decision): a kanban worker is ALREADY bound to a profile via
//! its `--profile <assignee>` spawn argument (`ironhermes-kanban::worker_spawn`).
//! Building a second binding mechanism for an entity that already has one
//! would violate this plan's own "no two stores" prohibition — this module
//! adds no kanban-worker read/write path.
//!
//! **Storage** — a single atomic JSON map at
//! `<hermes_home>/bot-bindings.json` ([`bot_binding_store_path`]), never a
//! per-profile sidecar: unlike `bot_meta_api`'s per-profile display
//! metadata, a platform adapter is NOT a profile-scoped entity (it can be
//! bound to any profile, or none), so there is no natural per-profile
//! sidecar location for it. One writer, one reader — the "one underlying
//! mapping" D-16 must-have. The atomic temp-then-rename write helper is
//! duplicated from `bot_meta_api::write_json_atomic` per this crate's own
//! established per-module small-helper-duplication precedent (see that
//! function's neighbor `now_ms` for the same rationale).
//!
//! **Fallback semantics (additive by construction)**: a missing or
//! malformed store is treated identically to "no bindings at all" — never a
//! hard error, never a panic. [`resolve_profile_for_bot`] falls through to
//! [`crate::server::profile_activation_api::resolve_active_profile_for`]
//! for the `GatewayWorkerDefault` surface (a platform adapter is a
//! gateway-hosted surface) whenever no explicit binding exists for a key.
//!
//! **UI display simplification (UI-SPEC E14 partial, deliberate)**: an
//! unbound bot's selector shows the LITERAL profile named `"default"` (the
//! always-present profile — UI-SPEC E14 empty: "the default profile always
//! exists"), never the live-resolved activation fallback. This keeps the
//! displayed value single-sourced and predictable, and matches the archive-
//! revert target below exactly ("default", the literal profile) rather than
//! whatever happens to be globally active at read time. The two UI editors
//! (`bot_roster.rs`, `soul.rs`) therefore never need to call
//! [`clear_bot_binding`] at all — choosing "default" from either selector
//! is just [`set_bot_binding`] with `"default"` as the target, an explicit
//! and always-valid binding. [`clear_bot_binding`] remains a real, tested
//! entry point (removes the store entry entirely, falling through to the
//! activation resolver) for any future caller that needs the distinction.
//!
//! **Archive interaction (UI-SPEC E14 error / operator decision)**:
//! [`revert_bindings_for_archived_profile`] is called from
//! `profile_api::archive_profile_impl` (the archive path) after a
//! successful archive — never before, and never gating the archive itself.
//! Every binding pointing at the archived profile is rewritten to point at
//! [`DEFAULT_BOUND_PROFILE`] instead; the archive always succeeds
//! regardless of whether any binding existed. This fn does not re-check the
//! write gate itself — it is only ever reached after `archive_profile`'s
//! OWN gate has already passed, exactly as `bot_meta_api::delete_bot_meta_impl`'s
//! call from the same archive path does.
//!
//! **Known gap, documented honestly (same pattern as plan 08's and plan
//! 10's own read-site audits for the identical underlying limitation)**:
//! [`resolve_profile_for_bot`] is a real, tested primitive, but no
//! production gateway-spawn call site invokes it yet. The gateway runs as
//! ONE process per invocation (`gateway_control_api.rs::start_gateway`
//! spawns a single `ironhermes gateway` subprocess for one `ConfigScope`),
//! and that single process loads exactly ONE `Config` at boot — every
//! platform adapter it hosts (every key configured in that one `Config`)
//! necessarily shares that ONE profile. A per-platform-adapter binding
//! therefore cannot take differential effect across platforms within a
//! single running gateway process today; only a future gateway able to
//! host per-platform, per-profile sub-processes (or re-resolve per-message)
//! could honour genuinely divergent bindings. This is a real, pre-existing
//! architectural limit, not something this plan's declared `files_modified`
//! (`bot_binding_api.rs`, `server/mod.rs`, `protocol.rs`, plus the roster/
//! Soul UI files and the one-line archive-path hook) could or should fix —
//! recorded here, in the SUMMARY, and as `human_judgment: true` on the
//! affected coverage entries, exactly like plan 08's chat-runtime gap and
//! plan 10's identical documented gap.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::collections::BTreeMap;
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Mutex;

use crate::protocol::{BotBinding, BotKey};

/// Phase 49.4 Plan 12 (UI-SPEC E14 partial/error): the literal profile name
/// an unbound bot displays, and the profile an archived profile's bindings
/// revert to. Always a real, always-present profile (`is_deletion_protected`
/// refuses to ever archive it) — never a sentinel/blank value.
pub(crate) const DEFAULT_BOUND_PROFILE: &str = "default";

#[cfg(feature = "server")]
static BOT_BINDING_LOCK: Mutex<()> = Mutex::new(());

/// Phase 49.4 Plan 12: the single store path — one writer, one reader.
#[cfg(feature = "server")]
pub(crate) fn bot_binding_store_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("bot-bindings.json")
}

/// Phase 49.4 Plan 12: current wall-clock time in milliseconds. Duplicated
/// verbatim from `profile_activation_api::now_ms` / `bot_meta_api::now_ms`
/// per this crate's own established "each module duplicates this tiny
/// helper" precedent. Never panics — a clock error degrades to `0`.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 49.4 Plan 12: unique-per-call atomic write — serialize to a temp
/// file in the same directory, `sync_all`, then `std::fs::rename` onto the
/// final path. Duplicated from `bot_meta_api::write_json_atomic` (that fn
/// is private to its own module) per this crate's own small-helper-
/// duplication precedent.
#[cfg(feature = "server")]
fn write_json_atomic(final_path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write as _;

    static TMP_WRITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = TMP_WRITE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = final_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let tmp_path = final_path.with_file_name(format!(
        "{file_name}.tmp.{}.{}",
        std::process::id(),
        counter
    ));

    let write_result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)?;
        f.write_all(contents.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }
    std::fs::rename(&tmp_path, final_path)
}

/// Phase 49.4 Plan 12: read the store. A missing OR malformed file is
/// treated identically as "no bindings" (empty map) — never a panic, never
/// a propagated error, so [`resolve_profile_for_bot`]'s fallback path is
/// always reachable regardless of the store's on-disk health. Malformed
/// contents are never logged verbatim (D-13-style content-withheld
/// discipline, matching `bot_meta_api::load_bot_meta_map`'s own error
/// text) — only the path.
#[cfg(feature = "server")]
pub(crate) fn load_bindings_map(path: &Path) -> BTreeMap<String, BotBinding> {
    if !path.exists() {
        return BTreeMap::new();
    }
    match std::fs::read_to_string(path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|_| {
            tracing::warn!(
                path = ?path,
                "bot_binding: malformed store; falling back to no bindings (content withheld)"
            );
            BTreeMap::new()
        }),
        Err(e) => {
            tracing::warn!(
                path = ?path,
                error = %e,
                "bot_binding: failed to read store; falling back to no bindings"
            );
            BTreeMap::new()
        }
    }
}

/// Phase 49.4 Plan 12: atomically write the store.
#[cfg(feature = "server")]
pub(crate) fn write_bindings_map(
    path: &Path,
    map: &BTreeMap<String, BotBinding>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all {parent:?}: {e}"))?;
    }
    let contents = serde_json::to_string_pretty(map)
        .map_err(|e| format!("serialize bot-bindings map: {e}"))?;
    write_json_atomic(path, &contents).map_err(|e| format!("write {path:?}: {e}"))
}

/// Phase 49.4 Plan 12 (D-16): the single resolver every future gateway-spawn
/// read site is meant to call instead of reading the binding store or
/// `current_profile()` directly. Mirrors
/// `profile_activation_api::resolve_active_profile_for`'s own doc-comment
/// honesty about having no production caller yet (see this module's doc
/// comment's "Known gap" section) — this fn IS real and unit-tested; only
/// the production wiring into a live gateway-spawn call site is the
/// documented gap.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn resolve_profile_for_bot(bot_key: &str) -> String {
    let map = load_bindings_map(&bot_binding_store_path());
    if let Some(binding) = map.get(bot_key) {
        return binding.profile_name.clone();
    }
    crate::server::profile_activation_api::resolve_active_profile_for(
        crate::server::profile_activation_api::ActivationSurface::GatewayWorkerDefault,
        None,
    )
}

/// Phase 49.4 Plan 12 (D-16, UI-SPEC E14 error / operator decision): revert
/// every binding pointing at `archived_profile` to [`DEFAULT_BOUND_PROFILE`].
/// Called from `profile_api::archive_profile_impl` AFTER a successful
/// archive — never gates or blocks it (archiving is never blocked by
/// bindings, per the operator's explicit decision). A write failure here is
/// logged and swallowed — the archive itself already succeeded and must
/// never be undone by a binding-store hiccup. Returns the affected bot
/// keys (possibly empty) so a future UI caller can render the transient
/// per-row revert notice.
#[cfg(feature = "server")]
pub(crate) fn revert_bindings_for_archived_profile(archived_profile: &str) -> Vec<String> {
    let path = bot_binding_store_path();
    let _guard = BOT_BINDING_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut map = load_bindings_map(&path);
    let mut affected = Vec::new();
    for (key, binding) in map.iter_mut() {
        if binding.profile_name == archived_profile {
            binding.profile_name = DEFAULT_BOUND_PROFILE.to_string();
            binding.updated_at_ms = now_ms();
            affected.push(key.clone());
        }
    }
    if !affected.is_empty() {
        if let Err(e) = write_bindings_map(&path, &map) {
            tracing::warn!(
                error = %e,
                archived_profile,
                "bot_binding: failed to persist archive-revert; affected bindings remain \
                 pointed at the now-archived profile on disk until the next successful write"
            );
        }
    }
    affected
}

/// Phase 49.4 Plan 12: read every binding — ungated, like `list_profiles`/
/// `list_bot_meta` (a read, never behind `web_config_write_enabled`).
#[server]
pub async fn list_bot_bindings() -> Result<Vec<BotBinding>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let bindings = tokio::task::spawn_blocking(|| {
            let _guard = BOT_BINDING_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            load_bindings_map(&bot_binding_store_path())
                .into_values()
                .collect::<Vec<_>>()
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?;
        Ok(bindings)
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_bot_bindings unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4 Plan 12 (D-16): set (or overwrite) one bot's binding. Follows
/// this crate's four-step gated-write protocol verbatim (`profile_api.rs`
/// `create_profile`, `profile_activation_api::activate_profile`): validate
/// the target profile name → reject a profile whose directory does not
/// exist → fresh `Config::load()` → `check_profile_write_gate`
/// (fail-closed) → the write, inside `spawn_blocking`.
#[server]
pub async fn set_bot_binding(bot_key: String, profile_name: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        // Phase 49.4: `DEFAULT_BOUND_PROFILE` is the sentinel for "no profile —
        // use the root/master agent", not a `profiles/<name>` directory (there
        // is none, by design). The store already holds this value: it is the
        // read-side fallback for an unbound platform, and `archive_profile`
        // rewrites bindings to it. Accepting it here lets an operator explicitly
        // bind a platform BACK to the default agent, which was previously a
        // one-way door.
        //
        // The sentinel is checked BEFORE `validate_profile_name`, which
        // deliberately REJECTS "default" as a reserved name — that validator
        // guards profile CREATION (you may not create a profile called
        // "default"), a different question from which value a binding may hold.
        // Every non-sentinel value still goes through validation and must name a
        // real profile directory.
        let validated = if profile_name == DEFAULT_BOUND_PROFILE {
            DEFAULT_BOUND_PROFILE.to_string()
        } else {
            let validated = ironhermes_core::profile::validate_profile_name(&profile_name)
                .map_err(|e| ServerFnError::new(format!("invalid profile name: {e}")))?;
            if !crate::server::profile_api::profile_dir_for(&validated).is_dir() {
                return Err(ServerFnError::new(format!(
                    "profile '{validated}' does not exist"
                )));
            }
            validated
        };

        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || {
            let path = bot_binding_store_path();
            let _guard = BOT_BINDING_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut map = load_bindings_map(&path);
            map.insert(
                bot_key.clone(),
                BotBinding {
                    bot_key: BotKey(bot_key),
                    profile_name: validated,
                    updated_at_ms: now_ms(),
                },
            );
            write_bindings_map(&path, &map)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;

        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (bot_key, profile_name);
        Err(ServerFnError::new(
            "set_bot_binding unavailable without `server` feature",
        ))
    }
}

/// Phase 49.4 Plan 12 (D-16): remove one bot's explicit binding — it then
/// falls through to [`resolve_profile_for_bot`]'s activation-resolved
/// fallback. Gated identically to [`set_bot_binding`] (no profile name to
/// validate here, but the write gate still applies — this mutates the same
/// store).
#[server]
pub async fn clear_bot_binding(bot_key: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || {
            let path = bot_binding_store_path();
            let _guard = BOT_BINDING_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut map = load_bindings_map(&path);
            map.remove(&bot_key);
            write_bindings_map(&path, &map)
        })
        .await
        .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
        .map_err(ServerFnError::new)?;

        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = bot_key;
        Err(ServerFnError::new(
            "clear_bot_binding unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod bot_binding_tests {
    use super::*;

    /// RAII guard, duplicated per this crate's own established precedent
    /// (see `profile_activation_api.rs::profile_activation_tests::ScopedEnv`'s
    /// doc comment — each `#[cfg(test)]` module is its own namespace).
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

    /// `security.web_config_write_enabled` defaults to `false` (fail-closed)
    /// — every test that expects a `set_bot_binding`/`clear_bot_binding`
    /// call to SUCCEED must seed a config with the gate explicitly enabled
    /// first, mirroring `mcp_admin_api.rs`'s own precedent for the same
    /// gate.
    fn enable_write_gate() {
        let mut config = ironhermes_core::config::Config::load().unwrap_or_default();
        config.security.web_config_write_enabled = true;
        config.save().expect("seed enabled-gate config");
    }

    // -------------------------------------------------------------------
    // Behavior 1: an empty store resolves every bot to the profile the
    // activation resolver returns — the binding layer is additive.
    // -------------------------------------------------------------------

    #[test]
    fn empty_store_resolves_to_activation_resolved_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scoped_home = dir.path().join("profiles").join("env-bot");
        std::fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", scoped_home.to_str().expect("utf8 path"));

        assert_eq!(resolve_profile_for_bot("telegram"), "env-bot");
    }

    // -------------------------------------------------------------------
    // Behavior 2 (acceptance-criteria-required): a bound bot resolves to
    // its bound profile even when a DIFFERENT profile is active at the
    // everywhere scope.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn bound_bot_resolves_to_bound_profile_while_a_different_profile_is_active_everywhere() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "active-everywhere");
        scaffold_profile(dir.path(), "bound-target");

        crate::server::profile_activation_api::write_active_profile_record(
            &crate::protocol::ActiveProfileRecord {
                name: "active-everywhere".to_string(),
                scope: crate::protocol::ActivationScope::Everywhere,
                updated_at_ms: 1,
            },
        )
        .expect("seed activation record");

        set_bot_binding("telegram".to_string(), "bound-target".to_string())
            .await
            .expect("set_bot_binding should succeed");

        assert_eq!(resolve_profile_for_bot("telegram"), "bound-target");
        // An unbound key still resolves to the globally-active profile.
        assert_eq!(resolve_profile_for_bot("discord"), "active-everywhere");
    }

    // -------------------------------------------------------------------
    // Behavior 3: setting a binding for one bot does not change any other
    // bot's resolution.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn setting_one_bot_binding_does_not_affect_another() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "profile-a");
        scaffold_profile(dir.path(), "profile-b");

        set_bot_binding("telegram".to_string(), "profile-a".to_string())
            .await
            .expect("set telegram binding");
        set_bot_binding("discord".to_string(), "profile-b".to_string())
            .await
            .expect("set discord binding");

        assert_eq!(resolve_profile_for_bot("telegram"), "profile-a");
        assert_eq!(resolve_profile_for_bot("discord"), "profile-b");
    }

    // -------------------------------------------------------------------
    // Behavior 4: clearing a binding returns that bot to the
    // activation-resolved profile (not necessarily "default").
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn clearing_a_binding_returns_to_activation_resolved_profile() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "globally-active");
        scaffold_profile(dir.path(), "bound-one");

        crate::server::profile_activation_api::write_active_profile_record(
            &crate::protocol::ActiveProfileRecord {
                name: "globally-active".to_string(),
                scope: crate::protocol::ActivationScope::Everywhere,
                updated_at_ms: 1,
            },
        )
        .expect("seed activation record");

        set_bot_binding("telegram".to_string(), "bound-one".to_string())
            .await
            .expect("set binding");
        assert_eq!(resolve_profile_for_bot("telegram"), "bound-one");

        clear_bot_binding("telegram".to_string())
            .await
            .expect("clear binding");
        assert_eq!(resolve_profile_for_bot("telegram"), "globally-active");
    }

    // -------------------------------------------------------------------
    // Behavior 5: setting a binding to a profile that does not exist
    // returns an error and leaves the store unchanged.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn setting_binding_to_nonexistent_profile_errors_and_leaves_store_unchanged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "seed");
        set_bot_binding("telegram".to_string(), "seed".to_string())
            .await
            .expect("seed binding");

        let result = set_bot_binding("telegram".to_string(), "never-existed".to_string()).await;
        assert!(result.is_err());

        assert_eq!(resolve_profile_for_bot("telegram"), "seed");
    }

    // -------------------------------------------------------------------
    // Phase 49.4: the DEFAULT_BOUND_PROFILE sentinel is accepted even though
    // no `profiles/default` directory exists — that is what lets an operator
    // bind a platform back to the root/master agent from the UI. Without the
    // carve-out this hit the "profile does not exist" rejection above, making
    // "bound to a real profile" a one-way door.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn setting_binding_to_the_default_sentinel_succeeds_without_a_profile_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "seed");
        set_bot_binding("telegram".to_string(), "seed".to_string())
            .await
            .expect("seed binding");

        assert!(
            !crate::server::profile_api::profile_dir_for(DEFAULT_BOUND_PROFILE).is_dir(),
            "precondition: the default sentinel must NOT be a real profile dir"
        );

        set_bot_binding("telegram".to_string(), DEFAULT_BOUND_PROFILE.to_string())
            .await
            .expect("binding back to the default sentinel must be accepted");

        let map = load_bindings_map(&bot_binding_store_path());
        assert_eq!(
            map.get("telegram").map(|b| b.profile_name.as_str()),
            Some(DEFAULT_BOUND_PROFILE),
            "the sentinel must be persisted like any other binding value"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 6: setting a binding with a name that fails the shared
    // profile-name validator returns an error before any path is resolved.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn setting_binding_with_invalid_profile_name_is_rejected_before_resolving_any_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let result =
            set_bot_binding("telegram".to_string(), "../../etc/passwd".to_string()).await;
        assert!(result.is_err());
        let map = load_bindings_map(&bot_binding_store_path());
        assert!(
            map.is_empty(),
            "a validation-rejected name must never write a binding"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 7: setting a binding with the web config write gate
    // disabled returns the gate refusal and persists nothing.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn setting_binding_refuses_when_write_gate_disabled_and_persists_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "gated-target");

        let mut config = ironhermes_core::config::Config::default();
        config.security.web_config_write_enabled = false;
        config.save().expect("seed disabled-gate config");

        let result = set_bot_binding("telegram".to_string(), "gated-target".to_string()).await;
        assert!(result.is_err(), "a disabled write gate must refuse the set");
        let map = load_bindings_map(&bot_binding_store_path());
        assert!(map.is_empty(), "a gate refusal must persist nothing");
    }

    // -------------------------------------------------------------------
    // Behavior 8 (acceptance-criteria-required): archiving a profile that
    // one or more bots are bound to reverts each of those bindings to the
    // default profile, and the archive itself still succeeds.
    // -------------------------------------------------------------------

    #[test]
    fn archiving_a_bound_profile_reverts_binding_and_archive_still_succeeds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile(dir.path(), "throwaway");

        let path = bot_binding_store_path();
        let mut map = BTreeMap::new();
        map.insert(
            "telegram".to_string(),
            BotBinding {
                bot_key: BotKey("telegram".to_string()),
                profile_name: "throwaway".to_string(),
                updated_at_ms: 1,
            },
        );
        write_bindings_map(&path, &map).expect("seed binding");

        let archive_result = crate::server::profile_api::archive_profile_impl("throwaway");
        assert!(
            archive_result.is_ok(),
            "archive must succeed even though a binding exists: {archive_result:?}"
        );

        let after = load_bindings_map(&path);
        assert_eq!(
            after.get("telegram").map(|b| b.profile_name.as_str()),
            Some(DEFAULT_BOUND_PROFILE),
            "the binding must revert to the default profile after its bound profile is archived"
        );
    }

    // -------------------------------------------------------------------
    // Behavior 9 (acceptance-criteria-required): a malformed or unreadable
    // store falls back to the activation-resolved profile rather than
    // panicking.
    // -------------------------------------------------------------------

    #[test]
    fn malformed_store_falls_back_to_activation_resolved_profile_without_panicking() {
        let dir = tempfile::tempdir().expect("tempdir");
        let scoped_home = dir.path().join("profiles").join("fallback-bot");
        std::fs::create_dir_all(&scoped_home).expect("mkdir scoped home");
        let _guard = ScopedEnv::set("IRONHERMES_HOME", scoped_home.to_str().expect("utf8 path"));

        let path = bot_binding_store_path();
        std::fs::create_dir_all(path.parent().expect("store path has a parent"))
            .expect("mkdir store parent");
        std::fs::write(&path, b"not valid json{{{").expect("write malformed store");

        assert_eq!(resolve_profile_for_bot("telegram"), "fallback-bot");
    }

    // -------------------------------------------------------------------
    // Behavior 10: the store round-trips — writing then reading returns
    // the same bindings.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn bindings_store_round_trips_through_set_and_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        enable_write_gate();
        scaffold_profile(dir.path(), "roundtrip-target");

        set_bot_binding("telegram".to_string(), "roundtrip-target".to_string())
            .await
            .expect("set binding");

        let bindings = list_bot_bindings().await.expect("list should succeed");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].bot_key, BotKey("telegram".to_string()));
        assert_eq!(bindings[0].profile_name, "roundtrip-target");
    }
}
