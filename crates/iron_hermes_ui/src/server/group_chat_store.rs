//! Phase 50.2 Plan 01 (D-13/D-23/D-05/D-20): the UI-owned sibling store for
//! group-chat rooms.
//!
//! **Persistence shape** (RESEARCH Assumption A4): a room spans N members, so
//! its transcript cannot live in any single profile's `state.db` without
//! recreating the cross-profile read surface D-13 exists to prevent. This
//! store instead follows `bot_meta_api.rs`'s own hybrid shape, keyed by ROOM
//! rather than PROFILE:
//!
//! - **INDEX**: a single JSON map at `<hermes_home>/group-rooms.json`
//!   ([`group_room_index_path`]) — `BTreeMap<String, GroupRoom>` keyed by
//!   room id — the roster's every-paint read source (`list_rooms_impl`),
//!   never a per-room fan-out.
//! - **PER-ROOM TRANSCRIPT**: one JSON file per room at
//!   `<hermes_home>/group-rooms/<id>.json`
//!   ([`group_room_transcript_path`]) — the canonical
//!   [`crate::protocol::GroupRoomTranscript`] (room record + every message).
//!   A room's `GroupRoom` record is written in BOTH places, mirroring
//!   `bot_meta_api.rs`'s sidecar-then-index-overwrite discipline; the
//!   transcript file's copy is canonical, the index copy is the roster-paint
//!   cache, refreshed on every write.
//!
//! **Own mutex, sibling to [`crate::server::bot_meta_api::BOT_META_LOCK`]
//! and `state_store`'s own connection/lock** — never nested inside either
//! (CONTEXT.md Integration Points hard constraint, carried forward from
//! 50.1). [`GROUP_CHAT_LOCK`] guards both files for the duration of one
//! read-modify-write sequence.
//!
//! **`validate_group_room_name` is deliberately NOT
//! `ironhermes_core::profile::validate_profile_name`** (RESEARCH Security
//! V5): a group name is not a profile name — it has its own charset and
//! length rule, defined here.

use dioxus::prelude::*;

#[cfg(feature = "server")]
use std::collections::BTreeMap;
#[cfg(feature = "server")]
use std::path::{Path, PathBuf};
#[cfg(feature = "server")]
use std::sync::Mutex;

#[cfg(feature = "server")]
use crate::protocol::{
    GroupChatSettings, GroupRoom, GroupRoomMessage, GroupRoomSummary, GroupRoomTranscript,
    MemberTurnStatus,
};

// Phase 50.2 Plan 11 (G-3): the room preview line is PERSISTED
// pre-truncated (write_room_preview, below) — no later surface can strip a
// reasoning block out of a value already sliced by the character cap, so
// it must be stripped before it is stored. Both this module and
// `cli_handoff.rs` are `#[cfg(feature = "server")]`, so this is a
// same-gate, same-crate call — no wasm implication, no duplicate needed.
#[cfg(feature = "server")]
use crate::server::cli_handoff::strip_think_blocks;

/// Phase 50.2 Plan 01: the group-chat store's own mutex — a sibling to
/// `BOT_META_LOCK`, never nested inside it or `state_store`'s own lock.
#[cfg(feature = "server")]
static GROUP_CHAT_LOCK: Mutex<()> = Mutex::new(());

/// Phase 50.2 Plan 01: the central room index path — one JSON map, the
/// roster's every-paint read source.
#[cfg(feature = "server")]
pub(crate) fn group_room_index_path() -> PathBuf {
    ironhermes_core::get_hermes_home().join("group-rooms.json")
}

/// Phase 50.2 Plan 01: one room's canonical transcript file path. `room_id`
/// MUST already be a validated slug (from [`slugify_room_name`]) before
/// reaching this fn — never a raw operator-supplied string.
#[cfg(feature = "server")]
pub(crate) fn group_room_transcript_path(room_id: &str) -> PathBuf {
    ironhermes_core::get_hermes_home()
        .join("group-rooms")
        .join(format!("{room_id}.json"))
}

/// Phase 50.2 Plan 01: every failure mode on the group-chat store path,
/// modeled on [`crate::server::cli_handoff::BotHandoffError`]'s discipline —
/// no variant's `Display` embeds a raw environment value, a raw `.env`
/// line, or raw child output.
#[cfg(feature = "server")]
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GroupChatError {
    RoomNameRejected { reason: String },
    DuplicateRoomName { name: String },
    MemberCountOutOfRange { count: usize, min: u32, max: u32 },
    MemberNotFound { name: String },
    RoomNotFound { id: String },
    StoreIo { reason: String },
}

#[cfg(feature = "server")]
impl std::fmt::Display for GroupChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoomNameRejected { reason } => write!(f, "room name rejected: {reason}"),
            Self::DuplicateRoomName { name } => {
                write!(f, "a room named \"{name}\" already exists")
            }
            Self::MemberCountOutOfRange { count, min, max } => write!(
                f,
                "member count {count} is out of range [{min}, {max}]"
            ),
            Self::MemberNotFound { name } => write!(f, "\"{name}\" is not a known bot"),
            Self::RoomNotFound { id } => write!(f, "room \"{id}\" was not found"),
            Self::StoreIo { reason } => write!(f, "group-chat store error: {reason}"),
        }
    }
}

#[cfg(feature = "server")]
impl std::error::Error for GroupChatError {}

/// Phase 50.2 Plan 01 (RESEARCH Security V5): validates a room name —
/// deliberately NOT `ironhermes_core::profile::validate_profile_name`, a
/// group name is not a profile name. Rejects empty/whitespace-only, names
/// over 64 chars, any character outside `[A-Za-z0-9 ._-]`, and any
/// occurrence of `..`, `/`, `\`, or `$`. Returns the trimmed name.
#[cfg(feature = "server")]
pub(crate) fn validate_group_room_name(name: &str) -> Result<String, GroupChatError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(GroupChatError::RoomNameRejected {
            reason: "room name is empty or whitespace-only".to_string(),
        });
    }
    if trimmed.chars().count() > 64 {
        return Err(GroupChatError::RoomNameRejected {
            reason: "room name exceeds 64 characters".to_string(),
        });
    }
    let has_invalid_char = trimmed
        .chars()
        .any(|c| !(c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-')));
    if has_invalid_char {
        return Err(GroupChatError::RoomNameRejected {
            reason: "room name contains a character outside [A-Za-z0-9 ._-]".to_string(),
        });
    }
    if trimmed.contains("..")
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains('$')
    {
        return Err(GroupChatError::RoomNameRejected {
            reason: "room name contains a path-traversal-shaped sequence".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

/// Phase 50.2 Plan 01: derives a room's `id` (and transcript filename stem)
/// from an ALREADY-VALIDATED name — lowercase, non-alphanumerics collapsed
/// to a single `-`, trimmed of leading/trailing `-`. Must never be called on
/// an unvalidated string.
#[cfg(feature = "server")]
pub(crate) fn slugify_room_name(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut last_was_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    slug.trim_matches('-').to_string()
}

/// Phase 50.2 Plan 01: current wall-clock time in milliseconds. Duplicated
/// from `bot_meta_api::now_ms`/`cli_handoff::now_ms` (module-private in both)
/// rather than widening their visibility — the crate's own sanctioned
/// "duplicate the trivial helper" precedent.
#[cfg(feature = "server")]
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Phase 50.2 Plan 01: unique-per-call atomic write — temp file in the same
/// directory, `sync_all`, then `std::fs::rename` onto the final path. Same
/// discipline as `bot_meta_api::write_json_atomic` (duplicated rather than
/// widened, same "own mutex, own helpers" isolation this module's doc
/// records).
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

/// Phase 50.2 Plan 01: read the central room index. A missing file is
/// `Ok(BTreeMap::new())` — first run must never be a failure state. A
/// corrupt file propagates a named error naming the store path only, never
/// the file's contents (D-13's CR-05/CR-06 mechanism).
#[cfg(feature = "server")]
fn load_room_index() -> Result<BTreeMap<String, GroupRoom>, GroupChatError> {
    let path = group_room_index_path();
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| GroupChatError::StoreIo {
        reason: format!("read {path:?}: {e}"),
    })?;
    serde_json::from_str(&contents).map_err(|_| GroupChatError::StoreIo {
        reason: format!(
            "parse {path:?}: malformed group-room index — content withheld (D-13); repair or delete the file"
        ),
    })
}

/// Phase 50.2 Plan 01: atomically write the central room index.
#[cfg(feature = "server")]
fn write_room_index(map: &BTreeMap<String, GroupRoom>) -> Result<(), GroupChatError> {
    let path = group_room_index_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GroupChatError::StoreIo {
            reason: format!("create_dir_all {parent:?}: {e}"),
        })?;
    }
    let contents = serde_json::to_string_pretty(map).map_err(|e| GroupChatError::StoreIo {
        reason: format!("serialize group-room index: {e}"),
    })?;
    write_json_atomic(&path, &contents).map_err(|e| GroupChatError::StoreIo {
        reason: format!("write {path:?}: {e}"),
    })
}

/// Phase 50.2 Plan 01: read one room's canonical transcript. Absent is
/// `Ok(None)` (no such room), not an error.
#[cfg(feature = "server")]
fn load_transcript(room_id: &str) -> Result<Option<GroupRoomTranscript>, GroupChatError> {
    let path = group_room_transcript_path(room_id);
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(&path).map_err(|e| GroupChatError::StoreIo {
        reason: format!("read {path:?}: {e}"),
    })?;
    serde_json::from_str(&contents).map(Some).map_err(|_| GroupChatError::StoreIo {
        reason: format!(
            "parse {path:?}: malformed group-room transcript — content withheld (D-13); repair or delete the file"
        ),
    })
}

/// Phase 50.2 Plan 01: atomically write one room's canonical transcript.
#[cfg(feature = "server")]
fn write_transcript(room_id: &str, transcript: &GroupRoomTranscript) -> Result<(), GroupChatError> {
    let path = group_room_transcript_path(room_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| GroupChatError::StoreIo {
            reason: format!("create_dir_all {parent:?}: {e}"),
        })?;
    }
    let contents =
        serde_json::to_string_pretty(transcript).map_err(|e| GroupChatError::StoreIo {
            reason: format!("serialize group-room transcript: {e}"),
        })?;
    write_json_atomic(&path, &contents).map_err(|e| GroupChatError::StoreIo {
        reason: format!("write {path:?}: {e}"),
    })
}

/// Phase 50.2 Plan 01: the roster card's preview line cap — mirrors
/// `bot_meta_api::PREVIEW_MAX_CHARS`.
#[cfg(feature = "server")]
pub(crate) const ROOM_PREVIEW_MAX_CHARS: usize = 160;

/// Phase 50.2 Plan 01: reduce a (possibly multi-line) message to a single
/// storable preview line — mirrors `bot_meta_api::truncate_preview_text`.
#[cfg(feature = "server")]
fn truncate_room_preview_text(text: &str) -> String {
    let first_line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if first_line.chars().count() > ROOM_PREVIEW_MAX_CHARS {
        let truncated: String = first_line.chars().take(ROOM_PREVIEW_MAX_CHARS).collect();
        format!("{truncated}…")
    } else {
        first_line.to_string()
    }
}

/// Phase 50.2 Plan 01: refresh a transcript's room-level preview from its
/// last message whose status is [`MemberTurnStatus::Replied`] — a passed or
/// failed turn carries no preview-worthy text (mirrors `bot_meta_api`'s
/// `write_preview_for`, called from inside [`append_room_messages_impl`]).
///
/// Phase 50.2 Plan 11 (G-3): the selected message's text is run through
/// [`strip_think_blocks`] BEFORE [`truncate_room_preview_text`], so the
/// `ROOM_PREVIEW_MAX_CHARS` cap counts visible characters — a reasoning
/// block stripped AFTER truncation could already be sliced mid-tag, and
/// the roster row that renders this preview (`bot_roster/group_row.rs`)
/// has no strip opportunity of its own. A reasoning-only reply strips to
/// an empty string, so the room stores an empty preview line rather than
/// a raw tag fragment.
#[cfg(feature = "server")]
fn write_room_preview(transcript: &mut GroupRoomTranscript) {
    if let Some(last) = transcript
        .messages
        .iter()
        .rev()
        .find(|m| matches!(m.status, MemberTurnStatus::Replied))
    {
        let stripped = strip_think_blocks(&last.text);
        transcript.room.preview = Some(truncate_room_preview_text(&stripped));
        transcript.room.preview_at_ms = Some(last.at_ms);
    }
}

/// Phase 50.2 Plan 15 (G-50.2-2a): the single member-validation
/// implementation shared by [`create_room_impl`] and
/// [`update_room_members_impl`] — extracted from `create_room_impl`'s own
/// pre-lock validation block so the `[min_members, max_members]` bound and
/// the on-disk bot-existence check are never re-derived at a second call
/// site. De-duplicates `members` case-sensitively, preserving first-seen
/// order, BEFORE checking the count — so a caller-supplied duplicate never
/// spuriously trips the upper bound. Then checks the de-duplicated count
/// against [`GroupChatSettings::default`]'s `[min_members, max_members]`
/// bound, returning [`GroupChatError::MemberCountOutOfRange`]. Then
/// validates each name through
/// `ironhermes_core::profile::validate_profile_name` plus a
/// `crate::server::profile_api::profile_dir_for(...).is_dir()` existence
/// check, returning [`GroupChatError::MemberNotFound`] for either failure.
/// Performs NO locking and NO I/O beyond the directory-existence probe —
/// exactly like the block it replaces, which always ran before the lock was
/// taken.
#[cfg(feature = "server")]
pub(crate) fn validate_room_members(members: &[String]) -> Result<Vec<String>, GroupChatError> {
    let mut deduped: Vec<String> = Vec::with_capacity(members.len());
    for member in members {
        if !deduped.contains(member) {
            deduped.push(member.clone());
        }
    }

    let settings = GroupChatSettings::default();
    if deduped.len() < settings.min_members as usize || deduped.len() > settings.max_members as usize {
        return Err(GroupChatError::MemberCountOutOfRange {
            count: deduped.len(),
            min: settings.min_members,
            max: settings.max_members,
        });
    }

    let mut validated_members = Vec::with_capacity(deduped.len());
    for member in &deduped {
        let validated = ironhermes_core::profile::validate_profile_name(member)
            .map_err(|_| GroupChatError::MemberNotFound {
                name: member.clone(),
            })?;
        if !crate::server::profile_api::profile_dir_for(&validated).is_dir() {
            return Err(GroupChatError::MemberNotFound { name: validated });
        }
        validated_members.push(validated);
    }

    Ok(validated_members)
}

/// Phase 50.2 Plan 01: create a room. Validates the name (own validator,
/// never `validate_profile_name`) and the members through
/// [`validate_room_members`] — the count against
/// [`GroupChatSettings::default`]'s `[min_members, max_members]` bound, and
/// every member name against `ironhermes_core::profile::validate_profile_name`
/// plus an on-disk existence check (a room member must be a real bot). Locks
/// once for the whole create sequence: index-duplicate check, transcript
/// write, index write.
#[cfg(feature = "server")]
pub(crate) fn create_room_impl(name: &str, members: &[String]) -> Result<GroupRoom, GroupChatError> {
    let validated_name = validate_group_room_name(name)?;
    let validated_members = validate_room_members(members)?;

    let slug = slugify_room_name(&validated_name);

    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut index = load_room_index()?;
    if index.contains_key(&slug) {
        return Err(GroupChatError::DuplicateRoomName { name: validated_name });
    }

    let now = now_ms();
    let room = GroupRoom {
        id: slug.clone(),
        name: validated_name,
        members: validated_members,
        group: None,
        needs_you: false,
        preview: None,
        preview_at_ms: None,
        created_at_ms: now,
        updated_at_ms: now,
    };

    write_transcript(
        &slug,
        &GroupRoomTranscript {
            room: room.clone(),
            messages: Vec::new(),
        },
    )?;
    index.insert(slug, room.clone());
    write_room_index(&index)?;

    Ok(room)
}

/// Phase 50.2 Plan 01: the roster's every-paint read — one index read, never
/// a per-room fan-out.
#[cfg(feature = "server")]
pub(crate) fn list_rooms_impl() -> Result<Vec<GroupRoomSummary>, GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let index = load_room_index()?;
    Ok(index
        .into_values()
        .map(|room| GroupRoomSummary {
            id: room.id,
            name: room.name,
            members: room.members,
            // Phase 50.2 Plan 03 (D-11, reachability fix): pass through
            // rather than drop — the roster's group-sectioning join needs
            // this to interleave a labeled room into its bot section.
            group: room.group,
            needs_you: room.needs_you,
            preview: room.preview,
            preview_at_ms: room.preview_at_ms,
            active_round: None,
        })
        .collect())
}

/// Phase 50.2 Plan 01: load one room's full transcript.
#[cfg(feature = "server")]
pub(crate) fn load_room_impl(room_id: &str) -> Result<GroupRoomTranscript, GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })
}

/// Phase 50.2 Plan 01: append messages to a room's transcript — the
/// lock→load→mutate→write-detail→write-index sequence
/// `bot_meta_api::save_bot_meta_impl` uses. Refreshes the room's roster-row
/// preview (from the last `Replied` message) and `updated_at_ms` inside the
/// same write.
#[cfg(feature = "server")]
pub(crate) fn append_room_messages_impl(
    room_id: &str,
    new_messages: Vec<GroupRoomMessage>,
) -> Result<GroupRoomTranscript, GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;

    transcript.messages.extend(new_messages);
    transcript.room.updated_at_ms = now_ms();
    write_room_preview(&mut transcript);

    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(transcript)
}

/// Phase 50.2 Plan 01: delete a room — removes both the transcript file and
/// the matching index entry inside one lock hold. The id must be a room-index
/// key (`RoomNotFound` otherwise — T-50.2-06-01); for an index-confirmed room
/// a missing transcript file is still success, not an error (idempotent,
/// mirrors `bot_meta_api::delete_bot_meta_sidecar`).
#[cfg(feature = "server")]
pub(crate) fn delete_room_impl(room_id: &str) -> Result<(), GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // T-50.2-06-01: the transcript path may only be derived from an
    // index-confirmed id — index keys are validated slugs from creation, so a
    // client-supplied id that is not an index key deletes nothing.
    let mut index = load_room_index()?;
    if !index.contains_key(room_id) {
        return Err(GroupChatError::RoomNotFound {
            id: room_id.to_string(),
        });
    }

    let path = group_room_transcript_path(room_id);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(GroupChatError::StoreIo {
                reason: format!("remove {path:?}: {e}"),
            });
        }
    }

    index.remove(room_id);
    write_room_index(&index)?;
    Ok(())
}

/// Phase 50.2 Plan 01: set (or clear) a room's `needs_you` escalation flag.
/// `pub(crate)`, impl-layer only for this plan — plan 04 wires this through
/// a `#[server]` fn once the needs-you trigger logic lands; kept here now so
/// the store's fn set matches the plan's full contract from the start
/// (mirrors `VerifyOutcome`'s "future plan consumes this" precedent in
/// `protocol.rs`). `#[allow(dead_code)]`: exercised by this plan's own test
/// module only (`set_room_needs_you_impl_toggles_the_flag`) — no production
/// call site until plan 04 wires one, same precedent `VerifyOutcome` and
/// `CloneFromChoice::Import` already set in `protocol.rs`.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn set_room_needs_you_impl(room_id: &str, needs_you: bool) -> Result<(), GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;
    transcript.room.needs_you = needs_you;
    transcript.room.updated_at_ms = now_ms();
    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(())
}

/// Phase 50.2 Plan 15 (G-50.2-2a): change an EXISTING room's membership —
/// the capability `50.2-UI-SPEC.md:316`'s `Edit members` overflow item
/// specced and plan 06 deferred (`group_chat_workspace.rs:611-613`). Follows
/// [`set_room_needs_you_impl`]'s exact lock→load→mutate→write-transcript→
/// write-index pattern: BOTH persisted copies are written inside one
/// [`GROUP_CHAT_LOCK`] hold, because the room-workspace header reads the
/// transcript copy (via [`load_room_impl`]) while the roster row reads the
/// index copy (via [`list_rooms_impl`]) — a write that lands on only one of
/// the two would make those two read paths disagree about who is in the
/// room. Validates through [`validate_room_members`] BEFORE the lock is
/// taken, so a rejected update never reaches either write and both
/// persisted copies stay byte-for-byte as they were. Touches only `members`
/// and `updated_at_ms` — `needs_you`, `preview`, `preview_at_ms`,
/// `created_at_ms`, `group`, `id`, `name` and the whole message transcript
/// all survive untouched.
#[cfg(feature = "server")]
pub(crate) fn update_room_members_impl(
    room_id: &str,
    members: &[String],
) -> Result<GroupRoom, GroupChatError> {
    let validated_members = validate_room_members(members)?;

    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;
    transcript.room.members = validated_members;
    transcript.room.updated_at_ms = now_ms();
    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(transcript.room)
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    /// RAII guard that sets/removes an env var and restores the previous
    /// value on drop. Duplicated from `cli_handoff.rs`'s own `ScopedEnv` —
    /// each `#[cfg(test)]` module is its own namespace, the crate's own
    /// sanctioned "duplicate the guard" precedent.
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

    // -------------------------------------------------------------------
    // validate_group_room_name
    // -------------------------------------------------------------------

    #[test]
    fn validate_group_room_name_accepts_a_plain_name() {
        assert_eq!(
            validate_group_room_name("Ops Room").expect("plain name should be accepted"),
            "Ops Room"
        );
    }

    #[test]
    fn validate_group_room_name_rejects_empty() {
        let err = validate_group_room_name("").unwrap_err();
        assert!(matches!(err, GroupChatError::RoomNameRejected { .. }));
    }

    #[test]
    fn validate_group_room_name_rejects_whitespace_only() {
        let err = validate_group_room_name("   ").unwrap_err();
        assert!(matches!(err, GroupChatError::RoomNameRejected { .. }));
    }

    #[test]
    fn validate_group_room_name_rejects_over_length() {
        let long_name = "a".repeat(65);
        let err = validate_group_room_name(&long_name).unwrap_err();
        assert!(matches!(err, GroupChatError::RoomNameRejected { .. }));
    }

    #[test]
    fn validate_group_room_name_rejects_traversal_shaped_input() {
        for candidate in ["../etc/passwd", "room/../../secret", "a/b", "a\\b"] {
            let err = validate_group_room_name(candidate).unwrap_err();
            assert!(
                matches!(err, GroupChatError::RoomNameRejected { .. }),
                "expected rejection for {candidate:?}"
            );
        }
    }

    #[test]
    fn validate_group_room_name_rejects_dollar_sign() {
        let err = validate_group_room_name("room-$HOME").unwrap_err();
        assert!(matches!(err, GroupChatError::RoomNameRejected { .. }));
    }

    // -------------------------------------------------------------------
    // store round-trip + member-count bounds
    // -------------------------------------------------------------------

    #[test]
    fn store_round_trip_create_append_load_delete() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
        )
        .expect("create_room_impl should succeed");
        assert_eq!(room.id, "ops-room");
        assert_eq!(room.members, vec!["scout".to_string(), "zig".to_string()]);

        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Operator,
            text: "hello room".to_string(),
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
        }];
        append_room_messages_impl(&room.id, messages)
            .expect("append_room_messages_impl should succeed");

        let loaded = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.room.preview.as_deref(), Some("hello room"));

        let index = load_room_index().expect("load index");
        assert!(index.contains_key(&room.id), "index must agree with the transcript");
        assert_eq!(
            index.get(&room.id).unwrap().preview.as_deref(),
            Some("hello room")
        );

        delete_room_impl(&room.id).expect("delete_room_impl should succeed");
        let index_after = load_room_index().expect("load index after delete");
        assert!(!index_after.contains_key(&room.id), "index entry must be removed");
        assert!(
            !group_room_transcript_path(&room.id).exists(),
            "transcript file must be removed"
        );
    }

    #[test]
    fn delete_room_impl_rejects_traversal_shaped_room_id() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);

        // A victim file OUTSIDE group-rooms/ that a traversal-shaped id would
        // resolve to: {home}/group-rooms/../victim.json == {home}/victim.json.
        let victim = dir.path().join("victim.json");
        std::fs::write(&victim, "{}").expect("write victim file");

        let err = delete_room_impl("../victim").unwrap_err();
        assert!(
            matches!(err, GroupChatError::RoomNotFound { .. }),
            "T-50.2-06-01: a non-index-key id must resolve to RoomNotFound, got {err:?}"
        );
        assert!(
            victim.exists(),
            "T-50.2-06-01: the traversal target must NOT be deleted"
        );
    }

    #[test]
    fn delete_room_impl_rejects_unknown_room_id() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let err = delete_room_impl("not-a-room").unwrap_err();
        assert!(
            matches!(err, GroupChatError::RoomNotFound { .. }),
            "an unknown-but-valid-shaped id must resolve to RoomNotFound, got {err:?}"
        );
        assert!(
            group_room_transcript_path(&room.id).exists(),
            "the real room's transcript must be untouched"
        );
    }

    #[test]
    fn set_room_needs_you_impl_toggles_the_flag() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");
        assert!(!room.needs_you);

        set_room_needs_you_impl(&room.id, true).expect("set needs_you true");
        let loaded = load_room_impl(&room.id).expect("load after set");
        assert!(loaded.room.needs_you);

        set_room_needs_you_impl(&room.id, false).expect("set needs_you false");
        let loaded = load_room_impl(&room.id).expect("load after clear");
        assert!(!loaded.room.needs_you);
    }

    #[test]
    fn create_room_impl_rejects_one_member() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        let err = create_room_impl("Solo Room", &["scout".to_string()]).unwrap_err();
        assert!(matches!(
            err,
            GroupChatError::MemberCountOutOfRange { count: 1, min: 2, max: 6 }
        ));
    }

    #[test]
    fn create_room_impl_rejects_seven_members() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let members: Vec<String> = (0..7).map(|i| format!("bot{i}")).collect();
        for m in &members {
            scaffold_profile(m);
        }

        let err = create_room_impl("Crowd Room", &members).unwrap_err();
        assert!(matches!(
            err,
            GroupChatError::MemberCountOutOfRange { count: 7, min: 2, max: 6 }
        ));
    }

    #[test]
    fn create_room_impl_accepts_two_members() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        create_room_impl("Two Room", &["scout".to_string(), "zig".to_string()])
            .expect("2 members must be accepted");
    }

    #[test]
    fn create_room_impl_accepts_six_members() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let members: Vec<String> = (0..6).map(|i| format!("bot{i}")).collect();
        for m in &members {
            scaffold_profile(m);
        }

        create_room_impl("Six Room", &members).expect("6 members must be accepted");
    }

    #[test]
    fn create_room_impl_rejects_duplicate_room_name() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        create_room_impl("Dup Room", &["scout".to_string(), "zig".to_string()])
            .expect("first create should succeed");
        let err = create_room_impl("Dup Room", &["scout".to_string(), "zig".to_string()])
            .unwrap_err();
        assert!(matches!(err, GroupChatError::DuplicateRoomName { .. }));
    }

    #[test]
    fn create_room_impl_rejects_unknown_member() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");

        let err =
            create_room_impl("Ghost Room", &["scout".to_string(), "ghost".to_string()])
                .unwrap_err();
        assert!(matches!(err, GroupChatError::MemberNotFound { .. }));
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 11 (G-3): write_room_preview strips reasoning blocks
    // BEFORE truncating, so the roster row's preview (rendered verbatim by
    // `bot_roster/group_row.rs`, no strip opportunity of its own) never
    // shows raw chain-of-thought.
    // -------------------------------------------------------------------

    #[test]
    fn write_room_preview_strips_a_reasoning_block_before_storing() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Member("zig".to_string()),
            text: "<think>internal musing</think>shipped the tracer".to_string(),
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
        }];
        append_room_messages_impl(&room.id, messages)
            .expect("append_room_messages_impl should succeed");

        let loaded = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(loaded.room.preview.as_deref(), Some("shipped the tracer"));
    }

    #[test]
    fn write_room_preview_reasoning_only_reply_stores_an_empty_preview() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Member("zig".to_string()),
            text: "<think>only reasoning, nothing else</think>".to_string(),
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
        }];
        append_room_messages_impl(&room.id, messages)
            .expect("append_room_messages_impl should succeed");

        let loaded = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(
            loaded.room.preview.as_deref(),
            Some(""),
            "a reasoning-only reply must store an empty preview line, never a raw tag fragment"
        );
    }

    #[test]
    fn write_room_preview_truncates_the_stripped_text_not_the_raw_text() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        // The reasoning block alone is longer than ROOM_PREVIEW_MAX_CHARS.
        // If stripping ran AFTER truncation, the cap would land inside the
        // reasoning block and the visible text would never survive.
        let reasoning = "x".repeat(ROOM_PREVIEW_MAX_CHARS + 40);
        let visible = "y".repeat(ROOM_PREVIEW_MAX_CHARS + 40);
        let text = format!("<think>{reasoning}</think>{visible}");

        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Member("zig".to_string()),
            text,
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
        }];
        append_room_messages_impl(&room.id, messages)
            .expect("append_room_messages_impl should succeed");

        let loaded = load_room_impl(&room.id).expect("load_room_impl should succeed");
        let preview = loaded.room.preview.expect("preview must be set");
        assert!(
            preview.starts_with('y'),
            "preview must be built from the STRIPPED visible text, got: {preview:?}"
        );
        assert!(
            !preview.contains('x'),
            "preview must never contain a character from the reasoning block, got: {preview:?}"
        );
        let cap_with_ellipsis = ROOM_PREVIEW_MAX_CHARS + 1; // truncated text + '…'
        assert_eq!(preview.chars().count(), cap_with_ellipsis);
    }

    // -------------------------------------------------------------------
    // Phase 50.2 Plan 15 (G-50.2-2a): update_room_members_impl /
    // validate_room_members — the rejection and preservation matrix.
    // -------------------------------------------------------------------

    #[test]
    fn update_room_members_impl_rejects_a_single_member() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let err = update_room_members_impl(&room.id, &["scout".to_string()]).unwrap_err();
        assert!(
            matches!(
                err,
                GroupChatError::MemberCountOutOfRange { count: 1, min: 2, max: 6 }
            ),
            "G-50.2-2a: single-member update must be rejected as count-out-of-range, got {err:?}"
        );

        let header_view = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(
            header_view.room.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the transcript copy unchanged"
        );
        let roster_view = list_rooms_impl().expect("list_rooms_impl should succeed");
        let roster_row = roster_view.iter().find(|r| r.id == room.id).unwrap();
        assert_eq!(
            roster_row.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the index copy unchanged"
        );
    }

    #[test]
    fn update_room_members_impl_rejects_seven_members() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");
        let extra: Vec<String> = (0..5).map(|i| format!("bot{i}")).collect();
        for m in &extra {
            scaffold_profile(m);
        }

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let mut seven = vec!["scout".to_string(), "zig".to_string()];
        seven.extend(extra);
        let err = update_room_members_impl(&room.id, &seven).unwrap_err();
        assert!(
            matches!(
                err,
                GroupChatError::MemberCountOutOfRange { count: 7, min: 2, max: 6 }
            ),
            "G-50.2-2a: seven-member update must be rejected as count-out-of-range, got {err:?}"
        );

        let header_view = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(
            header_view.room.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the transcript copy unchanged"
        );
        let roster_view = list_rooms_impl().expect("list_rooms_impl should succeed");
        let roster_row = roster_view.iter().find(|r| r.id == room.id).unwrap();
        assert_eq!(
            roster_row.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the index copy unchanged"
        );
    }

    #[test]
    fn update_room_members_impl_rejects_an_unknown_member() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let err = update_room_members_impl(
            &room.id,
            &["scout".to_string(), "ghost".to_string()],
        )
        .unwrap_err();
        assert!(
            matches!(err, GroupChatError::MemberNotFound { .. }),
            "G-50.2-2a: an unknown member must be rejected as member-not-found, got {err:?}"
        );

        let header_view = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(
            header_view.room.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the transcript copy unchanged"
        );
        let roster_view = list_rooms_impl().expect("list_rooms_impl should succeed");
        let roster_row = roster_view.iter().find(|r| r.id == room.id).unwrap();
        assert_eq!(
            roster_row.members,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: a rejected update must leave the index copy unchanged"
        );
    }

    #[test]
    fn update_room_members_impl_unknown_room_yields_room_not_found() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let err = update_room_members_impl(
            "never-created",
            &["scout".to_string(), "zig".to_string()],
        )
        .unwrap_err();
        assert!(
            matches!(err, GroupChatError::RoomNotFound { .. }),
            "G-50.2-2a: an unknown room id must be rejected as room-not-found, got {err:?}"
        );
    }

    #[test]
    fn update_room_members_impl_preserves_every_other_room_field() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");
        scaffold_profile("ada");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");
        set_room_needs_you_impl(&room.id, true).expect("set needs_you true");
        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Member("zig".to_string()),
            text: "shipped the tracer".to_string(),
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
        }];
        append_room_messages_impl(&room.id, messages)
            .expect("append_room_messages_impl should succeed");

        let before = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert!(before.room.needs_you, "fixture must have needs_you set");
        assert!(before.room.preview.is_some(), "fixture must have a preview");
        assert_eq!(before.messages.len(), 1, "fixture must have one message");

        let updated = update_room_members_impl(
            &room.id,
            &["scout".to_string(), "zig".to_string(), "ada".to_string()],
        )
        .expect("update_room_members_impl should succeed");

        assert_eq!(
            updated.members,
            vec!["scout".to_string(), "zig".to_string(), "ada".to_string()],
            "G-50.2-2a: members must reflect the new list"
        );
        assert_eq!(updated.needs_you, before.room.needs_you, "needs_you must survive untouched");
        assert_eq!(updated.preview, before.room.preview, "preview must survive untouched");
        assert_eq!(
            updated.preview_at_ms, before.room.preview_at_ms,
            "preview_at_ms must survive untouched"
        );
        assert_eq!(
            updated.created_at_ms, before.room.created_at_ms,
            "created_at_ms must survive untouched"
        );
        assert_eq!(updated.group, before.room.group, "group must survive untouched");
        assert_eq!(updated.id, before.room.id, "id must survive untouched");
        assert_eq!(updated.name, before.room.name, "name must survive untouched");
        assert!(
            updated.updated_at_ms >= before.room.updated_at_ms,
            "updated_at_ms must advance (or hold, on a fast clock)"
        );

        let after = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(after.messages.len(), 1, "message transcript must survive untouched");
    }

    #[test]
    fn validate_room_members_deduplicates_preserving_first_seen_order() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let validated = validate_room_members(&[
            "scout".to_string(),
            "zig".to_string(),
            "scout".to_string(),
        ])
        .expect("a six-entry list with one repeat must validate as five members");
        assert_eq!(
            validated,
            vec!["scout".to_string(), "zig".to_string()],
            "G-50.2-2a: de-duplication must preserve first-seen order"
        );
    }

    #[test]
    fn validate_room_members_is_the_only_bound_source() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let settings = GroupChatSettings::default();
        let members: Vec<String> = (0..(settings.max_members as usize + 1))
            .map(|i| format!("bot{i}"))
            .collect();
        for m in &members {
            scaffold_profile(m);
        }

        let err = validate_room_members(&members).unwrap_err();
        match err {
            GroupChatError::MemberCountOutOfRange { max, .. } => {
                assert_eq!(
                    max, settings.max_members,
                    "G-50.2-2a: the reported max must equal GroupChatSettings::default()'s value"
                );
            }
            other => panic!("expected MemberCountOutOfRange, got {other:?}"),
        }
    }
}
