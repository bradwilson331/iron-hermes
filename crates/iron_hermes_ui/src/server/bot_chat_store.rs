//! Phase 50.2 Plan 08 (D-13/D-23/D-11/D-04/D-21): the UI-owned per-bot Bot
//! Chat thread store — the OPERATOR half of "every bot has a persistent
//! Bot Chat" (the ROADMAP's first goal clause).
//!
//! Plan 02 gave the BOT its half: a `--session "Bot Chat"` title makes
//! consecutive `run_bot_handoff` dispatches resume one session and seed
//! prior turns into the model's own prompt. This store gives the OPERATOR
//! their half. 50.1's floating `BotChatWindow` (`bot_roster/chat_window.rs`)
//! held its thread in a session-transient `Signal<BTreeMap<String,
//! Vec<ChatWindowEntry>>>` owned by `bot_roster.rs` — that map disappeared
//! the moment `BotRoster` unmounted, so an operator who navigated away and
//! back saw a blank window even though the bot itself remembered the
//! conversation. This store is that thread's durable home.
//!
//! **Persistence shape**: one JSON file PER BOT, under that bot's OWN
//! profile directory — `<hermes_home>/profiles/<name>/bot-chat.json`
//! ([`bot_chat_thread_path`]), the same per-profile placement
//! [`crate::server::bot_meta_api::bot_meta_sidecar_path`] uses for
//! `ui-meta.json`. Deleting the profile directory removes the thread with
//! it — no separate cleanup path is needed, mirroring `bot_meta_api`'s own
//! sidecar-deletion reasoning.
//!
//! **Two stores, deliberately (D-13/D-23).** The bot's OWN memory lives in
//! its own `state.db`, written and read exclusively by the child CLI
//! process plan 02 dispatches. This store is the OPERATOR'S view, written
//! and read exclusively by the UI server process. Serving the operator's
//! view from the bot's own database would mean the UI server reading a
//! non-live profile's database from a browser-reachable path — the exact
//! cross-profile read surface D-13 and D-23 exist to prevent, and the same
//! reasoning that put group-room transcripts in their own sibling store
//! ([`crate::server::group_chat_store`]). This module never opens a
//! StateStore connection and never reads a database file.
//!
//! **Own mutex, sibling to [`crate::server::bot_meta_api::BOT_META_LOCK`],
//! [`crate::server::group_chat_store`]'s own lock, and `state_store`'s own
//! connection/lock** — [`BOT_CHAT_LOCK`] is never nested inside any of
//! them.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Mutex;

use crate::protocol::{BotChatEntry, BotChatThread};

/// Phase 50.2 Plan 08: the Bot Chat store's own mutex — a sibling to
/// `BOT_META_LOCK`/`GROUP_CHAT_LOCK`, never nested inside either or
/// `state_store`'s own lock. Guards one bot's thread file for the duration
/// of one read-modify-write sequence.
#[cfg(feature = "server")]
static BOT_CHAT_LOCK: Mutex<()> = Mutex::new(());

/// Phase 50.2 Plan 08: the operator's view is truncated to the most recent
/// 200 entries on every append — this bounds the OPERATOR'S VIEW only. The
/// bot's own recall is bounded separately by `ironhermes-cli`'s
/// `BOT_SESSION_HISTORY_LIMIT` (24, D-21's `GROUP_CHAT_HISTORY_LIMIT`); the
/// two bounds deliberately differ because they answer different questions:
/// how much the operator can scroll back, versus how much context each
/// turn carries.
#[cfg(feature = "server")]
pub(crate) const BOT_CHAT_THREAD_MAX_ENTRIES: usize = 200;

/// Phase 50.2 Plan 08: every failure mode on the Bot Chat store path,
/// modeled on [`crate::server::group_chat_store::GroupChatError`]'s
/// discipline — no variant's `Display` embeds a raw environment value, a
/// raw `.env` line, or raw child output.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum BotChatStoreError {
    InvalidBotName { reason: String },
    ThreadUnreadable { reason: String },
    StoreIo { reason: String },
}

#[cfg(feature = "server")]
impl std::fmt::Display for BotChatStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBotName { reason } => write!(f, "invalid bot name: {reason}"),
            Self::ThreadUnreadable { reason } => {
                write!(f, "bot chat thread unreadable: {reason}")
            }
            Self::StoreIo { reason } => write!(f, "bot chat store error: {reason}"),
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for BotChatStoreError {}

/// Phase 50.2 Plan 08: one bot's Bot Chat thread file path — per-profile,
/// the same placement [`crate::server::bot_meta_api::bot_meta_sidecar_path`]
/// uses. `name` MUST already be validated by
/// `ironhermes_core::profile::validate_profile_name` before reaching this
/// fn — never a raw operator-supplied string.
#[cfg(feature = "server")]
pub(crate) fn bot_chat_thread_path(name: &str) -> PathBuf {
    crate::server::profile_api::profile_dir_for(name).join("bot-chat.json")
}

/// Phase 50.2 Plan 08: current wall-clock time in milliseconds. Duplicated
/// from `bot_meta_api::now_ms`/`group_chat_store::now_ms` (module-private
/// in both) rather than widening their visibility — the crate's own
/// sanctioned "duplicate the trivial helper" precedent.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.2 Plan 08: unique-per-call atomic write — temp file in the same
/// directory, `sync_all`, then `std::fs::rename` onto the final path. Same
/// discipline as `bot_meta_api::write_json_atomic`/
/// `group_chat_store::write_json_atomic` (duplicated rather than widened,
/// same "own mutex, own helpers" isolation those modules already
/// establish).
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

/// Phase 50.2 Plan 08: read one bot's thread file. A missing file is
/// `Ok(None)` — a bot with no thread yet must never be a failure state. A
/// corrupt file propagates a named error naming the store path only, never
/// the file's contents (D-13's CR-05/CR-06 mechanism).
#[cfg(feature = "server")]
fn load_thread_file(path: &Path) -> Result<Option<BotChatThread>, BotChatStoreError> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path).map_err(|e| BotChatStoreError::StoreIo {
        reason: format!("read {path:?}: {e}"),
    })?;
    serde_json::from_str(&contents)
        .map(Some)
        .map_err(|_| BotChatStoreError::ThreadUnreadable {
            reason: format!(
                "parse {path:?}: malformed bot chat thread — content withheld (D-13); repair or delete the file"
            ),
        })
}

/// Phase 50.2 Plan 08: atomically write one bot's thread file.
#[cfg(feature = "server")]
fn write_thread_file(path: &Path, thread: &BotChatThread) -> Result<(), BotChatStoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| BotChatStoreError::StoreIo {
            reason: format!("create_dir_all {parent:?}: {e}"),
        })?;
    }
    let contents = serde_json::to_string_pretty(thread).map_err(|e| BotChatStoreError::StoreIo {
        reason: format!("serialize bot chat thread: {e}"),
    })?;
    write_json_atomic(path, &contents).map_err(|e| BotChatStoreError::StoreIo {
        reason: format!("write {path:?}: {e}"),
    })
}

/// Phase 50.2 Plan 08: load one bot's Bot Chat thread. A bot with no
/// thread file yet returns an empty thread, not an error — the window's
/// first-ever open for a brand-new bot must render exactly like 50.1's
/// original empty-thread behavior.
#[cfg(feature = "server")]
pub(crate) fn load_bot_chat_thread_impl(name: &str) -> Result<BotChatThread, BotChatStoreError> {
    let validated =
        ironhermes_core::profile::validate_profile_name(name).map_err(|e| BotChatStoreError::InvalidBotName {
            reason: e.to_string(),
        })?;

    let _guard = BOT_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = bot_chat_thread_path(&validated);
    match load_thread_file(&path)? {
        Some(thread) => Ok(thread),
        None => Ok(BotChatThread {
            bot: validated,
            entries: Vec::new(),
        }),
    }
}

/// Phase 50.2 Plan 08: append entries to a bot's thread — lock, load,
/// extend, truncate to [`BOT_CHAT_THREAD_MAX_ENTRIES`], write. `at_ms` is
/// assigned here (server wall-clock), overwriting whatever the caller
/// supplied, so a thread's row order never depends on a client clock.
/// Called with a single-element `Vec` from the `#[server]` wrapper below,
/// but takes a `Vec` so a future batch-append caller needs no new impl fn.
#[cfg(feature = "server")]
pub(crate) fn append_bot_chat_entries_impl(
    name: &str,
    new_entries: Vec<BotChatEntry>,
) -> Result<BotChatThread, BotChatStoreError> {
    let validated =
        ironhermes_core::profile::validate_profile_name(name).map_err(|e| BotChatStoreError::InvalidBotName {
            reason: e.to_string(),
        })?;

    let _guard = BOT_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = bot_chat_thread_path(&validated);
    let mut thread = load_thread_file(&path)?.unwrap_or_else(|| BotChatThread {
        bot: validated.clone(),
        entries: Vec::new(),
    });

    for mut entry in new_entries {
        entry.at_ms = now_ms();
        thread.entries.push(entry);
    }
    if thread.entries.len() > BOT_CHAT_THREAD_MAX_ENTRIES {
        let drop_count = thread.entries.len() - BOT_CHAT_THREAD_MAX_ENTRIES;
        thread.entries.drain(0..drop_count);
    }

    write_thread_file(&path, &thread)?;
    Ok(thread)
}

/// Phase 50.2 Plan 08: delete a bot's thread file — used by profile
/// deletion if a hook already exists, otherwise left available and unwired
/// rather than forcing a new hook this plan's own `files_modified` does not
/// name. A missing file is success, not an error (idempotent, mirrors
/// `group_chat_store::delete_room_impl`). `#[allow(dead_code)]`: exercised
/// by this plan's own test module only — same precedent
/// `group_chat_store::set_room_needs_you_impl` already set for this crate.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn clear_bot_chat_thread_impl(name: &str) -> Result<(), BotChatStoreError> {
    let validated =
        ironhermes_core::profile::validate_profile_name(name).map_err(|e| BotChatStoreError::InvalidBotName {
            reason: e.to_string(),
        })?;

    let _guard = BOT_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = bot_chat_thread_path(&validated);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(BotChatStoreError::StoreIo {
            reason: format!("remove {path:?}: {e}"),
        }),
    }
}

/// Phase 50.2 Plan 08: load a bot's Bot Chat thread — a read, so it is
/// ungated by `security.web_config_write_enabled` (mirrors
/// `list_bot_meta`).
#[server]
pub async fn load_bot_chat_thread(name: String) -> Result<BotChatThread, ServerFnError> {
    #[cfg(feature = "server")]
    {
        tokio::task::spawn_blocking(move || load_bot_chat_thread_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "load_bot_chat_thread unavailable without `server` feature",
        ))
    }
}

/// Phase 50.2 Plan 08: append one entry to a bot's Bot Chat thread. Follows
/// the crate's four-step write protocol: validate (inside the impl fn) →
/// fresh `Config::load()` → `check_profile_write_gate` (fails closed) →
/// `spawn_blocking`.
#[server]
pub async fn append_bot_chat_entry(name: String, entry: BotChatEntry) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || append_bot_chat_entries_impl(&name, vec![entry]))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = (name, entry);
        Err(ServerFnError::new(
            "append_bot_chat_entry unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::protocol::BotChatEntryKind;

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

    fn scaffold_profile(name: &str) {
        std::fs::create_dir_all(crate::server::profile_api::profile_dir_for(name))
            .expect("mkdir profile dir");
    }

    fn entry(kind: BotChatEntryKind, text: &str) -> BotChatEntry {
        BotChatEntry {
            kind,
            text: text.to_string(),
            at_ms: 0,
        }
    }

    #[test]
    fn missing_thread_file_loads_as_empty_thread() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        let thread = load_bot_chat_thread_impl("scout").expect("load should succeed");
        assert_eq!(thread.bot, "scout");
        assert!(thread.entries.is_empty());
    }

    #[test]
    fn append_operator_then_bot_loads_back_in_order() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        append_bot_chat_entries_impl("scout", vec![entry(BotChatEntryKind::Operator, "hello")])
            .expect("append operator entry");
        append_bot_chat_entries_impl("scout", vec![entry(BotChatEntryKind::Bot, "hi there")])
            .expect("append bot entry");

        let thread = load_bot_chat_thread_impl("scout").expect("load should succeed");
        assert_eq!(thread.entries.len(), 2);
        assert_eq!(thread.entries[0].kind, BotChatEntryKind::Operator);
        assert_eq!(thread.entries[0].text, "hello");
        assert_eq!(thread.entries[1].kind, BotChatEntryKind::Bot);
        assert_eq!(thread.entries[1].text, "hi there");
    }

    /// The partially-persisted case matters most: an operator message with
    /// its reply missing must resume as exactly one entry — synthesizing a
    /// placeholder reply is the specific failure the UI Considerations
    /// backstop rules out.
    #[test]
    fn operator_only_thread_loads_back_as_exactly_one_entry() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        append_bot_chat_entries_impl(
            "scout",
            vec![entry(BotChatEntryKind::Operator, "are you there?")],
        )
        .expect("append operator entry");

        let thread = load_bot_chat_thread_impl("scout").expect("load should succeed");
        assert_eq!(thread.entries.len(), 1);
        assert_eq!(thread.entries[0].kind, BotChatEntryKind::Operator);
        assert_eq!(thread.entries[0].text, "are you there?");
    }

    #[test]
    fn appending_past_the_bound_keeps_most_recent_and_drops_oldest() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        for i in 0..(BOT_CHAT_THREAD_MAX_ENTRIES + 5) {
            append_bot_chat_entries_impl(
                "scout",
                vec![entry(BotChatEntryKind::Operator, &format!("msg {i}"))],
            )
            .expect("append should succeed");
        }

        let thread = load_bot_chat_thread_impl("scout").expect("load should succeed");
        assert_eq!(thread.entries.len(), BOT_CHAT_THREAD_MAX_ENTRIES);
        assert_eq!(thread.entries.first().unwrap().text, "msg 5");
        assert_eq!(
            thread.entries.last().unwrap().text,
            format!("msg {}", BOT_CHAT_THREAD_MAX_ENTRIES + 4)
        );
    }

    #[test]
    fn invalid_bot_name_is_rejected_before_any_filesystem_access() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let err = load_bot_chat_thread_impl("../etc/passwd").unwrap_err();
        assert!(matches!(err, BotChatStoreError::InvalidBotName { .. }));
        // No path could have been derived from a rejected name — the
        // profiles dir itself was never created by this test.
        assert!(!dir.path().join("profiles").exists());
    }

    #[test]
    fn appending_to_an_invalid_bot_name_is_also_rejected() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let err = append_bot_chat_entries_impl(
            "../etc/passwd",
            vec![entry(BotChatEntryKind::Operator, "hi")],
        )
        .unwrap_err();
        assert!(matches!(err, BotChatStoreError::InvalidBotName { .. }));
    }

    #[test]
    fn clear_bot_chat_thread_impl_removes_the_file_and_is_idempotent() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        append_bot_chat_entries_impl("scout", vec![entry(BotChatEntryKind::Operator, "hi")])
            .expect("append should succeed");
        assert!(bot_chat_thread_path("scout").exists());

        clear_bot_chat_thread_impl("scout").expect("clear should succeed");
        assert!(!bot_chat_thread_path("scout").exists());
        // Idempotent: clearing an already-absent thread is still success.
        clear_bot_chat_thread_impl("scout").expect("clearing an absent thread is success");
    }

    #[test]
    fn malformed_thread_file_is_a_named_error_not_a_silent_empty_thread() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        let path = bot_chat_thread_path("scout");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not valid json").unwrap();

        let err = load_bot_chat_thread_impl("scout").unwrap_err();
        assert!(matches!(err, BotChatStoreError::ThreadUnreadable { .. }));
    }

    /// Exhaustive match with no wildcard arm: this only compiles if
    /// `BotChatEntryKind` and `chat_window::ChatWindowRole` both carry
    /// exactly these three variants, one-to-one.
    #[test]
    fn bot_chat_entry_kind_has_exactly_the_three_chat_window_role_variants() {
        use crate::components::hermes_app::screens::bot_roster::chat_window::ChatWindowRole;

        fn from_role(role: ChatWindowRole) -> BotChatEntryKind {
            match role {
                ChatWindowRole::Operator => BotChatEntryKind::Operator,
                ChatWindowRole::Bot => BotChatEntryKind::Bot,
                ChatWindowRole::Error => BotChatEntryKind::Error,
            }
        }

        assert_eq!(from_role(ChatWindowRole::Operator), BotChatEntryKind::Operator);
        assert_eq!(from_role(ChatWindowRole::Bot), BotChatEntryKind::Bot);
        assert_eq!(from_role(ChatWindowRole::Error), BotChatEntryKind::Error);
    }
}
