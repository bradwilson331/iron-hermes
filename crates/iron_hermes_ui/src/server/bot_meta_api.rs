//! Phase 50.1 Plan 01 (D-08/D-11/D-13/D-23): UI-owned bot display-metadata
//! store.
//!
//! **Persistence shape — operator checkpoint decision.** The plan's blocking
//! checkpoint (`50.1-01-PLAN.md`'s `checkpoint:decision` task) offered three
//! options: (a) a single JSON map, (b) a dedicated SQLite database, or (c) a
//! per-profile sidecar file. The operator selected a HYBRID of (c) and (a)
//! over the planner's recommended pure (a) — full record in
//! `50.1-01-SUMMARY.md`'s Decisions section:
//!
//! - **CANONICAL** copy of one bot's [`crate::protocol::BotMeta`] lives at a
//!   per-profile sidecar file, `<hermes_home>/profiles/<name>/ui-meta.json`
//!   ([`bot_meta_sidecar_path`]) — option (c)'s shape. Deleting the profile
//!   directory removes this file with it; no separate cleanup path is
//!   needed for that common case.
//! - **CACHE/INDEX**: a single JSON map at `<hermes_home>/bot-meta.json`
//!   ([`bot_meta_index_path`]) — option (a)'s shape — kept in sync on every
//!   write. `list_bot_meta` (the roster's every-paint read) is served from
//!   this ONE file, never a per-profile fan-out — this is what resolves
//!   option (c)'s own "N file reads per paint" con and satisfies D-13's
//!   fence against per-profile read surface.
//! - **Merge source of truth**: [`apply_bot_meta_patch`] always merges onto
//!   the SIDECAR's prior value (via [`save_bot_meta_impl`]), never the
//!   index. A save for a name whose sidecar is absent (a fresh profile, or
//!   one whose directory was deleted-then-recreated with the same name)
//!   therefore never merges onto a stale index entry — the sidecar write
//!   is followed by an unconditional index-entry overwrite that self-heals
//!   any staleness there too. This is what resolves option (c)'s other
//!   recorded con ("a deleted-then-recreated profile silently inherits
//!   stale look data").
//! - [`reconcile_bot_meta_orphans`] (Task 3) is the explicit tool for
//!   dropping index entries whose backing profile directory disappeared
//!   OUTSIDE the UI's own delete flow (the UI's own flow calls
//!   [`delete_bot_meta`] synchronously, so it never leaves an orphan).
//!
//! Both files share ONE store-local mutex ([`BOT_META_LOCK`]) — a sibling to
//! `state_store`'s own connection/lock, never reached through, nested
//! inside, or sharing a lock with it (CONTEXT.md Integration Points hard
//! constraint).
//!
//! **Server-fn write protocol** (`profile_api.rs:330+` `update_profile_config`
//! is the reference shape): validate input → fresh
//! `ironhermes_core::config::Config::load()` →
//! `crate::server::profile_api::check_profile_write_gate(&config)` (fails
//! closed on `security.web_config_write_enabled`) → the real work inside
//! `tokio::task::spawn_blocking`. `list_bot_meta` is a read and skips the
//! gate, mirroring `list_profiles`.
//!
//! **Explicitly NOT built**: rename-sync plumbing. A direct grep across
//! `profile_api.rs` and all three `screens/kanban/{wizard,profile_drawer,
//! profile_switcher}.rs` files confirms no profile-identity rename
//! operation exists anywhere in this codebase — the only renames are atomic
//! temp-file writes onto the SAME final name. This module adds none either.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Mutex;

use crate::protocol::{BotMeta, BotMetaListResponse};
#[cfg(feature = "server")]
use crate::protocol::BotMetaPatch;

/// Phase 50.1 Plan 01: the bot-meta store's own mutex — isolated from
/// `state_store`'s connection/mutex per CONTEXT.md's Integration Points hard
/// constraint. Guards both the sidecar and the index file for the duration
/// of one read-modify-write sequence so two concurrent saves for different
/// names cannot interleave their sidecar-then-index writes.
#[cfg(feature = "server")]
static BOT_META_LOCK: Mutex<()> = Mutex::new(());

/// Phase 50.1 Plan 01: the central cache/index path (hybrid option-a half).
#[cfg(feature = "server")]
pub(crate) fn bot_meta_index_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("bot-meta.json")
}

/// Phase 50.1 Plan 01: the canonical per-profile sidecar path (hybrid
/// option-c half). Reuses `profile_api::profile_dir_for` — the same
/// `<hermes_home>/profiles/<name>` join every other profile-scoped path in
/// this crate already uses — rather than re-deriving the join.
#[cfg(feature = "server")]
pub(crate) fn bot_meta_sidecar_path(name: &str) -> PathBuf {
    crate::server::profile_api::profile_dir_for(name).join("ui-meta.json")
}

/// Phase 50.1 Plan 01: unique-per-call atomic write — serialize to a temp
/// file in the same directory, `sync_all`, then `std::fs::rename` onto the
/// final path. Same temp-then-rename discipline `write_env_atomic_0600`
/// uses for `.env` writes (`profile_api.rs:595`) and `Config::save_to` uses
/// for `config.yaml` (`ironhermes-core/src/config.rs:3688`), minus the
/// 0600 permission step — bot-meta carries no secret material.
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

/// Phase 50.1 Plan 01: read the central index map. A missing file is
/// `Ok(BTreeMap::new())` — first run must never be a failure state (mirrors
/// `read_env_keys`'s absent-path contract, `profile_api.rs:204`). A corrupt
/// file propagates `Err` naming the failure class and the store path only —
/// never the file's contents or any parsed value (D-13's CR-05/CR-06
/// mechanism: an error `Display` that interpolates a raw failing value).
#[cfg(feature = "server")]
pub(crate) fn load_bot_meta_map(path: &Path) -> Result<BTreeMap<String, BotMeta>, String> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
    serde_json::from_str(&contents).map_err(|_| {
        format!("parse {path:?}: malformed bot-meta store — content withheld (D-13); repair or delete the file")
    })
}

/// Phase 50.1 Plan 01: atomically write the central index map.
#[cfg(feature = "server")]
pub(crate) fn write_bot_meta_map(path: &Path, map: &BTreeMap<String, BotMeta>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all {parent:?}: {e}"))?;
    }
    let contents =
        serde_json::to_string_pretty(map).map_err(|e| format!("serialize bot-meta map: {e}"))?;
    write_json_atomic(path, &contents).map_err(|e| format!("write {path:?}: {e}"))
}

/// Phase 50.1 Plan 01: read one profile's canonical sidecar. Absent is
/// `Ok(None)` (a bot with no saved look data yet), not an error. Same
/// content-withheld discipline as [`load_bot_meta_map`] on a parse failure.
#[cfg(feature = "server")]
fn load_bot_meta_sidecar(name: &str) -> Result<Option<BotMeta>, String> {
    let path = bot_meta_sidecar_path(name);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| format!("read {path:?}: {e}"))?;
    serde_json::from_str(&contents).map(Some).map_err(|_| {
        format!("parse {path:?}: malformed bot-meta sidecar — content withheld (D-13); repair or delete the file")
    })
}

/// Phase 50.1 Plan 01: atomically write one profile's canonical sidecar.
#[cfg(feature = "server")]
fn write_bot_meta_sidecar(name: &str, meta: &BotMeta) -> Result<(), String> {
    let path = bot_meta_sidecar_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create_dir_all {parent:?}: {e}"))?;
    }
    let contents =
        serde_json::to_string_pretty(meta).map_err(|e| format!("serialize bot-meta: {e}"))?;
    write_json_atomic(&path, &contents).map_err(|e| format!("write {path:?}: {e}"))
}

/// Phase 50.1 Plan 01 Task 3: delete one profile's sidecar. Idempotent — a
/// missing file is success, not an error (the delete-sync hook must be safe
/// to call for a profile that never had look data).
#[cfg(feature = "server")]
fn delete_bot_meta_sidecar(name: &str) -> Result<(), String> {
    let path = bot_meta_sidecar_path(name);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove {path:?}: {e}")),
    }
}

/// Phase 50.1 Plan 01: current wall-clock time in milliseconds. Never
/// panics — a clock error (time before the Unix epoch) degrades to `0`
/// rather than crashing a write.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.1 Plan 01: pure option-per-field merge — every `Some` field in
/// `patch` overwrites the corresponding field in `prior` (or the fresh
/// default when `prior` is `None`); every `None` field in `patch` leaves the
/// prior value untouched. `updated_at_ms` is always refreshed. Pure and
/// disk-I/O-free so it is directly unit-testable without touching the
/// filesystem.
#[cfg(feature = "server")]
pub(crate) fn apply_bot_meta_patch(
    prior: Option<BotMeta>,
    patch: &BotMetaPatch,
    now_ms: i64,
) -> BotMeta {
    let mut base = prior.unwrap_or_default();
    if let Some(title) = &patch.title {
        base.title = Some(title.clone());
    }
    if let Some(description) = &patch.description {
        base.description = Some(description.clone());
    }
    if let Some(avatar) = &patch.avatar {
        base.avatar = Some(avatar.clone());
    }
    if let Some(group) = &patch.group {
        base.group = Some(group.clone());
    }
    if let Some(preview) = &patch.preview {
        base.preview = Some(preview.clone());
    }
    if let Some(preview_at_ms) = patch.preview_at_ms {
        base.preview_at_ms = Some(preview_at_ms);
    }
    base.updated_at_ms = now_ms;
    base
}

/// Phase 50.1 Plan 01: the `save_bot_meta` impl layer — validate → merge
/// onto the sidecar (never the index) → write sidecar → overwrite the
/// matching index entry. Called from `save_bot_meta` inside
/// `spawn_blocking`. `pub(crate)` (widened by Plan 06) so `profile_api.rs`'s
/// test module can seed bot-meta fixtures synchronously without a tokio
/// runtime.
#[cfg(feature = "server")]
pub(crate) fn save_bot_meta_impl(patch: &BotMetaPatch) -> Result<BotMeta, String> {
    let validated_name = ironhermes_core::profile::validate_profile_name(&patch.name)
        .map_err(|e| format!("invalid profile name: {e}"))?;

    // T-50.1-01-05: avatar bytes are a separate asset path (plan 50.1-04) —
    // reject an inline data URL before any byte is written.
    //
    // MI-04 fix (`50.1-REVIEW.md`): also leaf-validate `image_id` with the
    // same `ironhermes_core::safe_attachment_leaf` guard the read side
    // (`bot_avatar_path`, `bot_avatar_api.rs`) already enforces. The serve
    // route re-validates too, so this was never a file-disclosure hole, but
    // an unvalidated value (e.g. `../../auth/logout`, or anything carrying
    // a path separator) is interpolated raw into every viewer's `<img src>`
    // on the roster paint — the write side must enforce the same invariant
    // the read side assumes, not just rely on the read side to catch it.
    if let Some(avatar) = &patch.avatar {
        if let Some(image_id) = &avatar.image_id {
            if image_id.starts_with("data:") {
                return Err(
                    "avatar.image_id must not carry an inline data URL — image bytes belong to a separate asset path"
                        .to_string(),
                );
            }
            if ironhermes_core::safe_attachment_leaf(image_id).is_none() {
                return Err("invalid avatar identifier".to_string());
            }
        }
    }

    let _guard = BOT_META_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let prior = load_bot_meta_sidecar(&validated_name)?;
    let updated = apply_bot_meta_patch(prior, patch, now_ms());
    write_bot_meta_sidecar(&validated_name, &updated)?;

    let index_path = bot_meta_index_path();
    let mut index = load_bot_meta_map(&index_path)?;
    index.insert(validated_name, updated.clone());
    write_bot_meta_map(&index_path, &index)?;

    Ok(updated)
}

/// Phase 50.1 Plan 06 (D-17): copy the source key's stored look to the
/// target key, leaving the source entry unchanged — the bot-meta half of
/// `duplicate_profile_impl`'s clone (`profile_api.rs`). A source with no
/// saved look yet (no sidecar) is a no-op success, not an error: most bots
/// never touch the avatar/title/description surface before being cloned,
/// and a fresh clone with no look data is a legitimate outcome, not a
/// failure. `pub(crate)` and impl-layer only — deliberately no
/// `#[server]` wrapper, so this is never a browser-reachable endpoint of
/// its own (Task 1's own instruction); it is reached exclusively through
/// `duplicate_profile`.
#[cfg(feature = "server")]
pub(crate) fn copy_bot_meta(source: &str, target: &str) -> Result<(), String> {
    let validated_source = ironhermes_core::profile::validate_profile_name(source)
        .map_err(|e| format!("invalid source profile name: {e}"))?;
    let validated_target = ironhermes_core::profile::validate_profile_name(target)
        .map_err(|e| format!("invalid target profile name: {e}"))?;

    let _guard = BOT_META_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let Some(mut meta) = load_bot_meta_sidecar(&validated_source)? else {
        // Nothing saved for the source yet — a legitimate no-op, not an
        // error (mirrors save_bot_meta_impl's own "absent sidecar" case).
        return Ok(());
    };
    meta.updated_at_ms = now_ms();
    write_bot_meta_sidecar(&validated_target, &meta)?;

    let index_path = bot_meta_index_path();
    let mut index = load_bot_meta_map(&index_path)?;
    index.insert(validated_target, meta);
    write_bot_meta_map(&index_path, &index)?;

    Ok(())
}

/// Phase 50.1 Plan 03 (D-13/T-50.1-03-09): the roster card renders the
/// preview on a single ellipsized line (`.kn-bot-card-preview`, `card.rs`);
/// an unbounded blob in the metadata store is a growth hazard. This caps
/// what `write_preview_for` ever persists.
#[cfg(feature = "server")]
const PREVIEW_MAX_CHARS: usize = 160;

/// Phase 50.1 Plan 03: reduce a (possibly multi-line) bot reply to a single
/// storable preview line — the first meaningful (non-blank) line, trimmed,
/// truncated to [`PREVIEW_MAX_CHARS`] characters with a trailing ellipsis
/// when truncated. Pure and disk-I/O-free.
#[cfg(feature = "server")]
pub(crate) fn truncate_preview_text(text: &str) -> String {
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if first_line.chars().count() > PREVIEW_MAX_CHARS {
        let truncated: String = first_line.chars().take(PREVIEW_MAX_CHARS).collect();
        format!("{truncated}…")
    } else {
        first_line.to_string()
    }
}

/// Phase 50.1 Plan 03 (D-13): the CLI-handoff completion path's write-back —
/// called from `cli_handoff::run_bot_handoff` on a SUCCESSFUL dispatch only,
/// inside the same `spawn_blocking` context. Builds a `BotMetaPatch` that
/// touches only `preview`/`preview_at_ms` and delegates to
/// [`save_bot_meta_impl`]'s own merge-onto-sidecar-then-index-overwrite
/// protocol, so this reuses the exact same store-write discipline
/// `save_bot_meta` uses — no second hand-rolled write path. The caller is
/// responsible for never invoking this on a failed dispatch (`run_bot_handoff`
/// only calls it in its success branch) — that is what keeps an existing
/// stored preview from ever being clobbered with an empty value.
#[cfg(feature = "server")]
pub(crate) fn write_preview_for(name: &str, reply_text: &str, at_ms: i64) -> Result<BotMeta, String> {
    let patch = BotMetaPatch {
        name: name.to_string(),
        title: None,
        description: None,
        avatar: None,
        group: None,
        preview: Some(truncate_preview_text(reply_text)),
        preview_at_ms: Some(at_ms),
    };
    save_bot_meta_impl(&patch)
}

/// Phase 50.1 Plan 01 Task 3 / Plan 06: the `delete_bot_meta` impl layer —
/// the synchronous delete-sync hook `profile_api::delete_profile_impl`
/// (plan 50.1-06) calls directly, inside the same blocking call as the
/// profile directory removal. `pub(crate)` (widened from Plan 01's private
/// visibility) so `profile_api.rs` can call it synchronously — the
/// `#[server]` `delete_bot_meta` fn below is async and cannot be invoked
/// from inside another `spawn_blocking` closure. Validates the name before
/// any path is built (T-50.1-01-01), then removes the sidecar and the
/// matching index entry inside one lock hold. Missing entries at either
/// layer are success, not an error.
#[cfg(feature = "server")]
pub(crate) fn delete_bot_meta_impl(name: &str) -> Result<(), String> {
    let validated_name = ironhermes_core::profile::validate_profile_name(name)
        .map_err(|e| format!("invalid profile name: {e}"))?;

    let _guard = BOT_META_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    delete_bot_meta_sidecar(&validated_name)?;
    let index_path = bot_meta_index_path();
    let mut index = load_bot_meta_map(&index_path)?;
    index.remove(&validated_name);
    write_bot_meta_map(&index_path, &index)?;
    Ok(())
}

/// Phase 50.1 Plan 01: the `list_bot_meta` impl layer — a single read of the
/// central index, never a per-profile fan-out (D-13).
#[cfg(feature = "server")]
fn list_bot_meta_impl() -> Result<BTreeMap<String, BotMeta>, String> {
    let _guard = BOT_META_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_bot_meta_map(&bot_meta_index_path())
}

/// Phase 50.1 Plan 01 Task 3: compare the index's stored keys against the
/// set of profile names actually on disk. Returns the orphan list (index
/// keys with no backing profile); when `prune` is set, also removes them
/// from the index inside the same write. This is the reconciliation path
/// for entries whose profile directory disappeared OUTSIDE the UI's own
/// delete flow (which calls `delete_bot_meta` synchronously and so never
/// leaves an orphan in the first place). Pure/synchronous apart from the
/// index file I/O, so directly testable without a server runtime.
#[cfg(feature = "server")]
pub(crate) fn reconcile_bot_meta_orphans(
    live_names: &BTreeSet<String>,
    prune: bool,
) -> Result<Vec<String>, String> {
    let _guard = BOT_META_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let index_path = bot_meta_index_path();
    let mut index = load_bot_meta_map(&index_path)?;
    let orphans: Vec<String> = index
        .keys()
        .filter(|k| !live_names.contains(k.as_str()))
        .cloned()
        .collect();
    if prune && !orphans.is_empty() {
        for name in &orphans {
            index.remove(name);
        }
        write_bot_meta_map(&index_path, &index)?;
    }
    Ok(orphans)
}

/// Phase 50.1 Plan 01 (D-13): read the whole bot-meta index in one call —
/// the roster's every-paint data source. A read, so it is ungated by
/// `security.web_config_write_enabled` (matches `list_profiles`).
///
/// Phase 50.1 OF-4 (gap task): also joins the ROOT kanban board's assignee
/// set into the response — ONE `fetch_board` read per call (D-13 fence:
/// never a per-profile fan-out), reusing the crate's existing store-access
/// path (`kanban_api::fetch_board`) rather than opening the kanban store a
/// second, divergent way. `include_archived: true` so a profile whose only
/// kanban work is now archived still reads as a kanban worker. A board-read
/// failure (e.g. no kanban store configured) degrades gracefully to an
/// empty set — the roster still renders, badges are simply absent; this is
/// a presentation nicety, never a hard dependency on the kanban store.
#[server]
pub async fn list_bot_meta() -> Result<BotMetaListResponse, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let map = tokio::task::spawn_blocking(list_bot_meta_impl)
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;

        let kanban_worker_names: std::collections::BTreeSet<String> =
            match crate::server::kanban_api::fetch_board(None, true).await {
                Ok(tasks) => tasks.into_iter().map(|t| t.assignee).collect(),
                Err(_) => std::collections::BTreeSet::new(),
            };

        Ok(BotMetaListResponse {
            meta: map,
            kanban_worker_names,
        })
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "list_bot_meta unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 01 (D-08/D-11): apply an option-per-field patch to one
/// bot's metadata. Follows the crate's four-step write protocol: validate →
/// fresh `Config::load()` → `check_profile_write_gate` (fails closed) →
/// `spawn_blocking`.
#[server]
pub async fn save_bot_meta(patch: crate::protocol::BotMetaPatch) -> Result<BotMeta, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let result = tokio::task::spawn_blocking(move || save_bot_meta_impl(&patch))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(result)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = patch;
        Err(ServerFnError::new(
            "save_bot_meta unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 01 Task 3 (D-18/D-13): the synchronous delete-sync hook
/// plan 50.1-06's profile delete will call — removes the key inside the
/// same `spawn_blocking` write, no deferred sweep. Follows the same
/// four-step write protocol as `save_bot_meta`.
#[server]
pub async fn delete_bot_meta(name: String) -> Result<(), ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        tokio::task::spawn_blocking(move || delete_bot_meta_impl(&name))
            .await
            .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
            .map_err(ServerFnError::new)?;
        Ok(())
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = name;
        Err(ServerFnError::new(
            "delete_bot_meta unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 01 Task 3: thin `#[server]` wrapper over
/// [`reconcile_bot_meta_orphans`] — resolves the live profile-name set via
/// the existing `list_profiles()` enumeration, then delegates the pure
/// comparison/prune to the impl fn inside `spawn_blocking`. Gated by the
/// same write protocol as `save_bot_meta`/`delete_bot_meta` since `prune:
/// true` mutates the index.
#[server]
pub async fn reconcile_bot_meta(prune: bool) -> Result<Vec<String>, ServerFnError> {
    #[cfg(feature = "server")]
    {
        let config = ironhermes_core::config::Config::load()
            .map_err(|e| ServerFnError::new(format!("Config load failed: {e}")))?;
        crate::server::profile_api::check_profile_write_gate(&config)
            .map_err(ServerFnError::new)?;

        let profiles = crate::server::profile_api::list_profiles().await?;
        let live_names: BTreeSet<String> = profiles.into_iter().map(|p| p.name).collect();

        let orphans =
            tokio::task::spawn_blocking(move || reconcile_bot_meta_orphans(&live_names, prune))
                .await
                .map_err(|e| ServerFnError::new(format!("spawn_blocking join: {e}")))?
                .map_err(ServerFnError::new)?;
        Ok(orphans)
    }
    #[cfg(not(feature = "server"))]
    {
        let _ = prune;
        Err(ServerFnError::new(
            "reconcile_bot_meta unavailable without `server` feature",
        ))
    }
}

/// Phase 50.1 Plan 01 (D-04): the live-bot LIVE badge's data source. Only
/// the resulting profile name crosses the wire — reuses the pre-existing
/// `ironhermes_core::current_profile()` resolver directly (the same
/// algorithm `crate::server::api::active_profile_identifier` wraps at
/// `server/api.rs:342`; that fn is module-private there, so this plan calls
/// the shared resolver directly rather than widening that fn's visibility —
/// `server/api.rs` stays untouched by this plan).
#[server]
pub async fn live_profile_name() -> Result<String, ServerFnError> {
    #[cfg(feature = "server")]
    {
        Ok(ironhermes_core::current_profile())
    }
    #[cfg(not(feature = "server"))]
    {
        Err(ServerFnError::new(
            "live_profile_name unavailable without `server` feature",
        ))
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::protocol::BotAvatarDescriptor;

    /// RAII guard that sets `IRONHERMES_HOME` and restores the previous
    /// value on drop. Duplicated from `profile_api.rs`'s own
    /// `ScopedEnv` — each `#[cfg(test)]` module is its own namespace, so
    /// this is the crate's own sanctioned "duplicate the guard" precedent,
    /// not drift.
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
            // SAFETY: still holding env_lock() from the caller.
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

    fn sample_meta(title: &str) -> BotMeta {
        BotMeta {
            title: Some(title.to_string()),
            description: Some("a test bot".to_string()),
            avatar: Some(BotAvatarDescriptor {
                kind: "generated".to_string(),
                shape: Some("circle".to_string()),
                color: Some("#4ec9b0".to_string()),
                image_id: None,
            }),
            group: Some("scouts".to_string()),
            preview: Some("hey there".to_string()),
            preview_at_ms: Some(1_700_000_000_000),
            updated_at_ms: 1_700_000_000_001,
        }
    }

    // -------------------------------------------------------------------
    // apply_bot_meta_patch
    // -------------------------------------------------------------------

    #[test]
    fn apply_bot_meta_patch_with_every_field_none_returns_prior_unchanged_except_timestamp() {
        let _lock = crate::server::test_support::env_lock();
        let prior = sample_meta("Scout");
        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: None,
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let result = apply_bot_meta_patch(Some(prior.clone()), &patch, 999);
        assert_eq!(result.title, prior.title);
        assert_eq!(result.description, prior.description);
        assert_eq!(result.avatar, prior.avatar);
        assert_eq!(result.group, prior.group);
        assert_eq!(result.preview, prior.preview);
        assert_eq!(result.preview_at_ms, prior.preview_at_ms);
        assert_eq!(result.updated_at_ms, 999);
    }

    #[test]
    fn apply_bot_meta_patch_title_over_absent_prior_yields_title_and_defaults() {
        let _lock = crate::server::test_support::env_lock();
        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let result = apply_bot_meta_patch(None, &patch, 42);
        assert_eq!(result.title.as_deref(), Some("Scout"));
        assert_eq!(result.description, None);
        assert_eq!(result.avatar, None);
        assert_eq!(result.group, None);
        assert_eq!(result.preview, None);
        assert_eq!(result.preview_at_ms, None);
        assert_eq!(result.updated_at_ms, 42);
    }

    // -------------------------------------------------------------------
    // load_bot_meta_map / write_bot_meta_map
    // -------------------------------------------------------------------

    #[test]
    fn load_bot_meta_map_on_absent_path_returns_empty_map_not_error() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let path = dir.path().join("does-not-exist.json");
        let result = load_bot_meta_map(&path).expect("absent path must not be an error");
        assert!(result.is_empty());
    }

    #[test]
    fn load_bot_meta_map_on_corrupt_file_errs_without_leaking_content() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let path = dir.path().join("corrupt.json");
        let secret_looking_line = "SUPER_SECRET_TOKEN=sk-abc123";
        std::fs::write(&path, format!("{{ not json {secret_looking_line}")).expect("write fixture");
        let err = load_bot_meta_map(&path).expect_err("malformed file must error");
        assert!(
            !err.contains(secret_looking_line) && !err.contains("not json"),
            "error must never embed file contents (D-13): {err}"
        );
    }

    #[test]
    fn write_then_load_bot_meta_map_round_trips_every_field() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let path = dir.path().join("bot-meta.json");
        let mut map = BTreeMap::new();
        map.insert("scout".to_string(), sample_meta("Scout"));
        write_bot_meta_map(&path, &map).expect("write must succeed");
        let loaded = load_bot_meta_map(&path).expect("load must succeed");
        assert_eq!(loaded, map);
    }

    // -------------------------------------------------------------------
    // save_bot_meta_impl / delete_bot_meta_impl (Task 1 tracer coverage)
    // -------------------------------------------------------------------

    #[test]
    fn save_bot_meta_impl_writes_sidecar_and_index_entry() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let saved = save_bot_meta_impl(&patch).expect("save must succeed");
        assert_eq!(saved.title.as_deref(), Some("Scout"));

        let sidecar = load_bot_meta_sidecar("scout")
            .expect("sidecar read must succeed")
            .expect("sidecar must exist after save");
        assert_eq!(sidecar.title.as_deref(), Some("Scout"));

        let index = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert_eq!(index.get("scout").and_then(|m| m.title.clone()).as_deref(), Some("Scout"));
    }

    #[test]
    fn save_bot_meta_impl_rejects_inline_data_url_avatar_and_writes_nothing() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: None,
            description: None,
            avatar: Some(BotAvatarDescriptor {
                kind: "custom".to_string(),
                shape: None,
                color: None,
                image_id: Some("data:image/png;base64,aaaa".to_string()),
            }),
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let err = save_bot_meta_impl(&patch).expect_err("inline data URL must be rejected");
        assert!(err.contains("data URL"));
        assert!(
            !bot_meta_sidecar_path("scout").exists(),
            "a rejected patch must write nothing to disk"
        );
    }

    #[test]
    fn save_bot_meta_impl_rejects_traversal_shaped_image_id_and_writes_nothing() {
        // MI-04 regression (`50.1-REVIEW.md`): image_id is interpolated raw
        // into every viewer's `<img src>` on the roster paint, so the write
        // side must leaf-validate it (same guard the read side already
        // enforces) rather than trust the client-supplied string.
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: None,
            description: None,
            avatar: Some(BotAvatarDescriptor {
                kind: "custom".to_string(),
                shape: None,
                color: None,
                image_id: Some("../../auth/logout".to_string()),
            }),
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let err = save_bot_meta_impl(&patch)
            .expect_err("traversal-shaped image_id must be rejected");
        assert!(err.contains("invalid avatar identifier"));
        assert!(
            !bot_meta_sidecar_path("scout").exists(),
            "a rejected patch must write nothing to disk"
        );
    }

    #[test]
    fn save_bot_meta_impl_rejects_invalid_name_and_writes_nothing() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let patch = BotMetaPatch {
            name: "../escape".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let err = save_bot_meta_impl(&patch).expect_err("invalid name must be rejected");
        assert!(err.contains("invalid profile name"));
        assert!(
            !bot_meta_index_path().exists()
                || load_bot_meta_map(&bot_meta_index_path())
                    .expect("index read must succeed")
                    .is_empty()
        );
    }

    #[test]
    fn two_sequential_saves_for_different_names_both_survive() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let patch_a = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let patch_b = BotMetaPatch {
            name: "ranger".to_string(),
            title: Some("Ranger".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        save_bot_meta_impl(&patch_a).expect("save scout must succeed");
        save_bot_meta_impl(&patch_b).expect("save ranger must succeed");

        let index = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert_eq!(index.get("scout").and_then(|m| m.title.clone()).as_deref(), Some("Scout"));
        assert_eq!(index.get("ranger").and_then(|m| m.title.clone()).as_deref(), Some("Ranger"));
        assert_eq!(index.len(), 2, "the second save must not clobber the first's entry");
    }

    // -------------------------------------------------------------------
    // delete_bot_meta_impl (Task 3 hardening)
    // -------------------------------------------------------------------

    #[test]
    fn delete_bot_meta_impl_on_absent_entry_succeeds_and_leaves_map_unchanged() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        save_bot_meta_impl(&patch).expect("save must succeed");
        let before = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");

        delete_bot_meta_impl("never-existed").expect("deleting an absent entry must succeed");

        let after = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert_eq!(before, after, "a no-op delete must leave the map byte-identical");
    }

    #[test]
    fn delete_bot_meta_impl_removes_exactly_one_key_and_leaves_siblings_untouched() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let patch_a = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let patch_b = BotMetaPatch {
            name: "ranger".to_string(),
            title: Some("Ranger".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        save_bot_meta_impl(&patch_a).expect("save scout must succeed");
        let ranger_saved = save_bot_meta_impl(&patch_b).expect("save ranger must succeed");

        delete_bot_meta_impl("scout").expect("delete must succeed");

        let index = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert!(!index.contains_key("scout"), "scout must be removed");
        assert_eq!(index.get("ranger"), Some(&ranger_saved), "ranger must survive byte-identical");
        assert!(
            !bot_meta_sidecar_path("scout").exists(),
            "scout's sidecar must also be removed"
        );
        assert!(
            bot_meta_sidecar_path("ranger").exists(),
            "ranger's sidecar must survive"
        );
    }

    // -------------------------------------------------------------------
    // copy_bot_meta (Phase 50.1 Plan 06, D-17)
    // -------------------------------------------------------------------

    #[test]
    fn copy_bot_meta_copies_the_look_to_the_target_and_leaves_source_unchanged() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let patch = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: Some("a friendly bot".to_string()),
            avatar: Some(BotAvatarDescriptor {
                kind: "generated".to_string(),
                shape: Some("hexagon".to_string()),
                color: Some("#4ec9b0".to_string()),
                image_id: None,
            }),
            group: None,
            preview: Some("hi there".to_string()),
            preview_at_ms: Some(1_700_000_000_000),
        };
        let source_saved = save_bot_meta_impl(&patch).expect("seed source bot-meta");

        copy_bot_meta("scout", "scout-clone").expect("copy_bot_meta should succeed");

        let index = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        let cloned = index
            .get("scout-clone")
            .expect("target must have a bot-meta entry");
        assert_eq!(cloned.title, source_saved.title);
        assert_eq!(cloned.description, source_saved.description);
        assert_eq!(cloned.avatar, source_saved.avatar);

        // Source entry is unchanged (still present, still itself).
        let source_after = index.get("scout").expect("source entry must survive");
        assert_eq!(source_after.title, source_saved.title);
        assert_eq!(source_after.avatar, source_saved.avatar);
    }

    #[test]
    fn copy_bot_meta_on_source_with_no_saved_look_is_a_no_op_success() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        // Deliberately no bot-meta ever saved for "no-look-bot".

        let result = copy_bot_meta("no-look-bot", "no-look-bot-clone");
        assert!(result.is_ok(), "an absent source sidecar must be a no-op success");

        let index = load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert!(
            !index.contains_key("no-look-bot-clone"),
            "no entry should be created when the source has no saved look"
        );
    }

    // -------------------------------------------------------------------
    // reconcile_bot_meta_orphans
    // -------------------------------------------------------------------

    #[test]
    fn reconcile_bot_meta_orphans_reports_and_prunes_only_the_missing_key() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        let patch_a = BotMetaPatch {
            name: "scout".to_string(),
            title: Some("Scout".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        let patch_b = BotMetaPatch {
            name: "ranger".to_string(),
            title: Some("Ranger".to_string()),
            description: None,
            avatar: None,
            group: None,
            preview: None,
            preview_at_ms: None,
        };
        save_bot_meta_impl(&patch_a).expect("save scout must succeed");
        save_bot_meta_impl(&patch_b).expect("save ranger must succeed");

        // Only "ranger" is reported live — "scout"'s profile directory
        // disappeared outside the UI's own delete flow.
        let live_names: BTreeSet<String> = ["ranger".to_string()].into_iter().collect();

        // Report-only pass (prune: false) must not mutate the index.
        let orphans_reported =
            reconcile_bot_meta_orphans(&live_names, false).expect("reconcile must succeed");
        assert_eq!(orphans_reported, vec!["scout".to_string()]);
        let after_report =
            load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert!(after_report.contains_key("scout"), "report-only pass must not prune");

        // Prune pass removes the orphan and preserves the live key.
        let orphans_pruned =
            reconcile_bot_meta_orphans(&live_names, true).expect("reconcile must succeed");
        assert_eq!(orphans_pruned, vec!["scout".to_string()]);
        let after_prune =
            load_bot_meta_map(&bot_meta_index_path()).expect("index read must succeed");
        assert!(!after_prune.contains_key("scout"), "scout must be pruned");
        assert!(after_prune.contains_key("ranger"), "ranger must still be present");
    }
}
