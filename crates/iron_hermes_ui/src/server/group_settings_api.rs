//! Phase 50.2 Plan 05 (D-21/D-10/D-11): the persisted, app-wide Group Chat
//! Settings record and its `#[server]` CRUD.
//!
//! **What this closes.** D-21's headline requirement is that the
//! group-orchestration constants ship as DEFAULTS exposed as a real,
//! editable settings surface — not merely a config key. Plan 01 shipped
//! `GroupChatSettings::default()` (3/10/24/2/6, D-21's constants verbatim
//! from `plugin.js:3295-3298`) but no persistence and no way to change it.
//! This module gives the struct a durable, validated home.
//!
//! **Persistence shape.** A single app-wide record — not a per-key map, not
//! a per-room fan-out — at `<hermes_home>/group-chat-settings.json`
//! ([`group_settings_path`]), mirroring `bot_meta_api.rs`'s single-JSON-file
//! discipline with NO sidecar tier (there is exactly one of these, ever).
//!
//! **Own lock, sibling to [`crate::server::group_chat_store`]'s room-store
//! mutex and [`crate::server::bot_meta_api`]'s `BOT_META_LOCK` — never
//! nested inside either.** A settings save must not queue behind a running
//! room's transcript appends: rooms and settings are written on unrelated
//! paths, and sharing the room store's own mutex would serialize an
//! operator's settings save behind whatever in-flight round dispatch
//! happens to be appending messages at that moment. [`GROUP_SETTINGS_LOCK`]
//! guards this file's own read-modify-write sequence only.
//!
//! **`load_group_settings_impl` is `pub(crate)` for a specific handoff.**
//! Plan 01's `group_chat_api.rs` (owned by a sibling wave-2 plan this wave,
//! not touched by this plan) currently sources the round driver's settings
//! from a one-line `GroupChatSettings::default()` helper. This module does
//! NOT edit that file — instead it exposes [`load_group_settings_impl`] at
//! `pub(crate)` visibility so plan 06's integration task can flip that one
//! line to read the persisted record instead. Until that flip lands, this
//! plan's settings UI is real (validated, persisted, round-trips) but the
//! round driver itself still runs on the compiled-in default — recorded
//! here and in the plan summary so the gap is never silently assumed
//! closed.
//!
//! **Every ceiling in [`clamp_group_settings`] exists to keep a persisted
//! record from turning the round driver into an unbounded loop or a
//! wider-than-sanctioned subprocess fan-out (T-50.2-05-01/02):**
//! `max_rounds` capped at 10, `max_messages` at 100, `history_limit` at
//! 200 — generous ceilings an operator would never need to approach, but a
//! hard backstop against a persisted record with no ceiling at all.
//! `max_members` is capped at 6 and `min_members` floored at 2 — D-21's own
//! room-size rule, not wideable from the UI; a room larger than 6 members
//! would spawn more concurrent subprocesses than the decision sanctions.
//!
//! **The per-member turn timeout is NOT part of this record.** 180 seconds
//! (`BOT_HANDOFF_TIMEOUT_SECONDS`) is a fixed cap on `run_bot_handoff` with
//! no extend-while-working mechanism (RESEARCH Pitfall 4) — the settings
//! drawer surfaces it as read-only information, never a field this module
//! would validate or persist.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Mutex;

use crate::protocol::GroupChatSettings;

/// Phase 50.2 Plan 05: the settings store's own mutex — a sibling to the
/// group-chat room store's own mutex and `BOT_META_LOCK`, never nested
/// inside either (see module doc for the reasoning: a settings save must
/// not queue behind a running room's transcript appends).
#[cfg(feature = "server")]
static GROUP_SETTINGS_LOCK: Mutex<()> = Mutex::new(());

/// Phase 50.2 Plan 05: the single app-wide settings record's path — one
/// record, no per-key tier.
#[cfg(feature = "server")]
pub(crate) fn group_settings_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("group-chat-settings.json")
}

/// Phase 50.2 Plan 05: every failure mode on the settings store path,
/// modeled on `GroupChatError`'s discipline (`group_chat_store.rs`) — no
/// variant's `Display` embeds a raw environment value, a raw file path
/// outside the hermes home, or raw file contents.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GroupSettingsError {
    FieldOutOfRange { field: String, reason: String },
    MemberBoundsInverted,
    RecordUnreadable { reason: String },
    StoreIo { reason: String },
}

#[cfg(feature = "server")]
impl std::fmt::Display for GroupSettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FieldOutOfRange { field, reason } => {
                write!(f, "{field} is out of range: {reason}")
            }
            Self::MemberBoundsInverted => {
                write!(f, "min_members must not exceed max_members")
            }
            Self::RecordUnreadable { reason } => {
                write!(f, "group-chat settings record unreadable: {reason}")
            }
            Self::StoreIo { reason } => write!(f, "group-chat settings store error: {reason}"),
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for GroupSettingsError {}

/// Phase 50.2 Plan 05: unique-per-call atomic write — temp file in the same
/// directory, `sync_all`, then `std::fs::rename` onto the final path. Same
/// discipline as `group_chat_store::write_json_atomic`/
/// `bot_meta_api::write_json_atomic` (duplicated rather than widened, the
/// crate's own sanctioned "own mutex, own helpers" isolation precedent).
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

/// Phase 50.2 Plan 05: read the persisted settings record. A missing file
/// is NOT an error — it returns [`GroupChatSettings::default`] (D-21's
/// constants verbatim) so a fresh install always resolves to the shipped
/// defaults. A malformed file IS a named error rather than a silent
/// default, so a corrupted record surfaces instead of quietly reverting the
/// operator's configuration.
///
/// `pub(crate)` per this module's own doc: plan 06's `group_chat_api.rs`
/// integration task consumes this directly once it flips the driver's
/// one-line `Default::default()` helper onto this fn.
#[cfg(feature = "server")]
pub(crate) fn load_group_settings_impl() -> Result<GroupChatSettings, GroupSettingsError> {
    let path = group_settings_path();
    if !path.exists() {
        return Ok(GroupChatSettings::default());
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| GroupSettingsError::StoreIo {
        reason: format!("read {path:?}: {e}"),
    })?;
    serde_json::from_str(&contents).map_err(|_| GroupSettingsError::RecordUnreadable {
        reason: format!(
            "parse {path:?}: malformed group-chat-settings record — content withheld (D-13 discipline); repair or delete the file"
        ),
    })
}

/// Phase 50.2 Plan 05 (T-50.2-05-01/02): validate a settings record before
/// it is ever written. Every ceiling here exists so a persisted record
/// cannot turn the round driver into an unbounded loop or a
/// wider-than-sanctioned member fan-out — see the module doc for the full
/// reasoning per field. Checks run in field order so each rejection case
/// reports a distinct field.
#[cfg(feature = "server")]
pub(crate) fn clamp_group_settings(s: &GroupChatSettings) -> Result<(), GroupSettingsError> {
    if s.max_rounds < 1 || s.max_rounds > 10 {
        return Err(GroupSettingsError::FieldOutOfRange {
            field: "max_rounds".to_string(),
            reason: "must be between 1 and 10".to_string(),
        });
    }
    if s.max_messages < 1 || s.max_messages > 100 {
        return Err(GroupSettingsError::FieldOutOfRange {
            field: "max_messages".to_string(),
            reason: "must be between 1 and 100".to_string(),
        });
    }
    if s.history_limit < 1 || s.history_limit > 200 {
        return Err(GroupSettingsError::FieldOutOfRange {
            field: "history_limit".to_string(),
            reason: "must be between 1 and 200".to_string(),
        });
    }
    // D-21's own room-size rule — not wideable from the UI. A room larger
    // than 6 members would spawn more concurrent subprocesses than the
    // decision sanctions.
    if s.min_members < 2 {
        return Err(GroupSettingsError::FieldOutOfRange {
            field: "min_members".to_string(),
            reason: "must be at least 2".to_string(),
        });
    }
    if s.max_members > 6 {
        return Err(GroupSettingsError::FieldOutOfRange {
            field: "max_members".to_string(),
            reason: "must be at most 6".to_string(),
        });
    }
    if s.min_members > s.max_members {
        return Err(GroupSettingsError::MemberBoundsInverted);
    }
    Ok(())
}

/// Phase 50.2 Plan 05: validate then atomically write the settings record.
/// Locks only for the write itself — `clamp_group_settings` runs first, so
/// a rejected save never touches the lock or the file.
#[cfg(feature = "server")]
pub(crate) fn save_group_settings_impl(
    settings: &GroupChatSettings,
) -> Result<GroupChatSettings, GroupSettingsError> {
    clamp_group_settings(settings)?;

    let _guard = GROUP_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let path = group_settings_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GroupSettingsError::StoreIo {
            reason: format!("create_dir_all {parent:?}: {e}"),
        })?;
    }
    let contents = serde_json::to_string_pretty(settings).map_err(|e| GroupSettingsError::StoreIo {
        reason: format!("serialize group-chat settings: {e}"),
    })?;
    write_json_atomic(&path, &contents).map_err(|e| GroupSettingsError::StoreIo {
        reason: format!("write {path:?}: {e}"),
    })?;

    Ok(settings.clone())
}

/// Phase 50.2 Plan 05: read the persisted group-chat settings record — a
/// read, so it is ungated by `security.web_config_write_enabled` (mirrors
/// `list_bot_meta`/`list_group_rooms`'s own read-path exemption).
#[server]
pub async fn load_group_chat_settings() -> Result<GroupChatSettings, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(load_group_settings_impl)
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "load_group_chat_settings unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 05: save the group-chat settings record. Four-step write
/// protocol, exactly as `create_group_room`/`save_bot_meta` do it:
/// `Config::load` -> `profile_api::check_profile_write_gate` (fails closed
/// on `security.web_config_write_enabled`) -> the real work
/// ([`save_group_settings_impl`], which itself validates via
/// `clamp_group_settings` before writing) inside `spawn_blocking`.
#[server]
pub async fn save_group_chat_settings(
    settings: GroupChatSettings,
) -> Result<GroupChatSettings, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || save_group_settings_impl(&settings))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = settings;
        Err(ServerFnError::new(
            "save_group_chat_settings unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard that sets `IRONHERMES_HOME` and restores the previous
    /// value on drop. Duplicated from `group_chat_store.rs`'s own
    /// `ScopedEnv` — each `#[cfg(test)]` module is its own namespace, the
    /// crate's own sanctioned "duplicate the guard" precedent.
    struct ScopedEnv {
        key: String,
        prev: Option<String>,
    }

    impl ScopedEnv {
        fn set(key: &str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: single-threaded test context; env_lock() below
            // serializes every test in this crate that mutates process env.
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

    fn home(dir: &tempfile::TempDir) -> ScopedEnv {
        ScopedEnv::set(
            "IRONHERMES_HOME",
            dir.path().to_str().expect("tempdir path must be utf8"),
        )
    }

    // -------------------------------------------------------------------
    // load_group_settings_impl
    // -------------------------------------------------------------------

    #[test]
    fn load_group_settings_impl_on_missing_file_returns_shipped_defaults() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let loaded = load_group_settings_impl().expect("missing file must not be an error");
        assert_eq!(loaded.max_rounds, 3);
        assert_eq!(loaded.max_messages, 10);
        assert_eq!(loaded.history_limit, 24);
        assert_eq!(loaded.min_members, 2);
        assert_eq!(loaded.max_members, 6);
    }

    #[test]
    fn load_group_settings_impl_on_corrupt_file_errs_without_leaking_content() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let secret_looking_line = "SUPER_SECRET_TOKEN=sk-abc123";
        std::fs::write(
            group_settings_path(),
            format!("{{ not json {secret_looking_line}"),
        )
        .expect("write fixture");

        let err = load_group_settings_impl().expect_err("malformed file must error");
        assert!(matches!(err, GroupSettingsError::RecordUnreadable { .. }));
        let msg = err.to_string();
        assert!(
            !msg.contains(secret_looking_line) && !msg.contains("not json"),
            "error must never embed file contents (D-13): {msg}"
        );
    }

    // -------------------------------------------------------------------
    // save_group_settings_impl / round trip
    // -------------------------------------------------------------------

    #[test]
    fn save_then_load_round_trips_every_field() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let custom = GroupChatSettings {
            max_rounds: 1,
            max_messages: 5,
            history_limit: 12,
            min_members: 3,
            max_members: 5,
        };
        let saved = save_group_settings_impl(&custom).expect("save must succeed");
        assert_eq!(saved, custom);

        let loaded = load_group_settings_impl().expect("load must succeed");
        assert_eq!(loaded, custom);
    }

    #[test]
    fn save_group_settings_impl_rejects_invalid_record_and_writes_nothing() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let invalid = GroupChatSettings {
            max_rounds: 0,
            ..GroupChatSettings::default()
        };
        let err = save_group_settings_impl(&invalid).expect_err("invalid record must be rejected");
        assert!(matches!(err, GroupSettingsError::FieldOutOfRange { .. }));
        assert!(
            !group_settings_path().exists(),
            "a rejected save must write nothing to disk"
        );
    }

    // -------------------------------------------------------------------
    // clamp_group_settings
    // -------------------------------------------------------------------

    #[test]
    fn clamp_group_settings_accepts_the_default_record_unchanged() {
        clamp_group_settings(&GroupChatSettings::default())
            .expect("shipped defaults must pass their own clamp");
    }

    #[test]
    fn clamp_rejects_max_rounds_zero() {
        let s = GroupChatSettings {
            max_rounds: 0,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "max_rounds"
        ));
    }

    #[test]
    fn clamp_rejects_max_rounds_over_ceiling() {
        let s = GroupChatSettings {
            max_rounds: 11,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "max_rounds"
        ));
    }

    #[test]
    fn clamp_rejects_max_messages_zero() {
        let s = GroupChatSettings {
            max_messages: 0,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "max_messages"
        ));
    }

    #[test]
    fn clamp_rejects_max_messages_over_ceiling() {
        let s = GroupChatSettings {
            max_messages: 101,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "max_messages"
        ));
    }

    #[test]
    fn clamp_rejects_history_limit_zero() {
        let s = GroupChatSettings {
            history_limit: 0,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "history_limit"
        ));
    }

    #[test]
    fn clamp_rejects_history_limit_over_ceiling() {
        let s = GroupChatSettings {
            history_limit: 201,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "history_limit"
        ));
    }

    #[test]
    fn clamp_rejects_min_members_below_two() {
        let s = GroupChatSettings {
            min_members: 1,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "min_members"
        ));
    }

    #[test]
    fn clamp_rejects_max_members_seven_the_room_size_ceiling_is_not_wideable() {
        let s = GroupChatSettings {
            max_members: 7,
            ..GroupChatSettings::default()
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(
            err,
            GroupSettingsError::FieldOutOfRange { ref field, .. } if field == "max_members"
        ));
    }

    #[test]
    fn clamp_rejects_min_members_greater_than_max_members() {
        let s = GroupChatSettings {
            max_rounds: 3,
            max_messages: 10,
            history_limit: 24,
            min_members: 5,
            max_members: 4,
        };
        let err = clamp_group_settings(&s).unwrap_err();
        assert!(matches!(err, GroupSettingsError::MemberBoundsInverted));
    }

    // -------------------------------------------------------------------
    // pinned defaults (D-21 constants must never silently drift)
    // -------------------------------------------------------------------

    /// Phase 50.2 Plan 01: `GroupChatSettings::default()` must return exactly
    /// D-21's constants — this test is duplicated here (also asserted in
    /// `protocol.rs`) so a future edit to the `Default` impl trips this
    /// module's own test suite too, not just `protocol.rs`'s.
    #[test]
    fn shipped_defaults_are_exactly_d21s_constants() {
        let defaults = GroupChatSettings::default();
        assert_eq!(defaults.max_rounds, 3);
        assert_eq!(defaults.max_messages, 10);
        assert_eq!(defaults.history_limit, 24);
        assert_eq!(defaults.min_members, 2);
        assert_eq!(defaults.max_members, 6);
    }
}
