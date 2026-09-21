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
    GroupChatSettings, GroupRoom, GroupRoomMessage, GroupRoomSummary, GroupRoomTeamSetup,
    GroupRoomTranscript, MemberRole, MemberTurnStatus,
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
    /// Phase 52 (Round 1 codex MEDIUM): the landing point for
    /// `impl From<TeamDriveError> for GroupChatError` — `code` is a
    /// host-owned discriminant string (`"leader-contract"`, `"no-leader"`,
    /// `"task-targets"`, …), never a formatted error. `run_team_drive` and
    /// its callers must never route a `TeamDriveError` through `StoreIo`'s
    /// `format!("{e}")` shape — that would reopen the T-52-02 leak the
    /// moment `TeamDriveError` carries a parse failure.
    TeamDriveFailed { code: &'static str },
    /// Phase 52 (D-06/D-07/D-09): `validate_team_room_shape`'s shape-rule
    /// rejection — `reason` names WHICH shape rule failed
    /// (`pattern.is_some()` without a Leader entry, a Leader entry without
    /// `pattern`, or more than one Leader entry) and carries no filesystem
    /// path or model-produced byte (CR-05/CR-06 discipline).
    TeamShapeInvalid { reason: String },
    /// Phase 52 (D-07, Round 1 codex MEDIUM): a team setup designates a
    /// leader who is not present in the incoming member list — distinct
    /// from D-07's demotion (removing the CURRENTLY PERSISTED leader from
    /// membership), which `normalize_team_setup_for_write` handles before
    /// this validator ever runs.
    LeaderNotAMember { name: String },
    /// Phase 52 (D-14, Round 1 codex HIGH): a per-room `max_cycles`
    /// override outside `protocol::TEAM_CYCLE_MIN..=TEAM_CYCLE_MAX`.
    TeamCyclesOutOfRange { value: u32 },
    /// Phase 52 Plan 05 (D-02, Round 1 codex HIGH): a conversation reset was
    /// requested while a round drive is currently in flight for this room.
    /// Retryable — the drive that holds the room's slot will eventually
    /// finish and release it, and a reset that never mutated anything is
    /// safe to retry as many times as needed.
    ResetRefusedDriveInFlight,
    /// Phase 52 Plan 05 (D-02, Round 1 codex HIGH): the reset's transcript
    /// write (the AUTHORITATIVE copy — `run_group_rounds_with_settings`
    /// reads `transcript.room.conversation_epoch`, never the index copy)
    /// succeeded, but the subsequent index projection write failed. The
    /// reset HAS LANDED; only the roster's index copy is stale. Distinct
    /// from `StoreIo` deliberately — a caller must NOT retry this outcome,
    /// because a retry would bump the epoch a second time and orphan
    /// another session for nothing. The next successful write through any
    /// store path repairs the stale projection on its own.
    IndexProjectionStale,
    /// Phase 52 Plan 05 (D-02, Round 1 codex suggestion): the room's
    /// `conversation_epoch` is already `u32::MAX` — advancing it further
    /// would require a saturating add, which would silently report success
    /// while producing the SAME title (and therefore reusing the SAME
    /// child session) as the un-reset room, a false clean slate. The epoch
    /// and transcript are left completely unchanged when this is returned.
    ConversationEpochExhausted,
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
            Self::TeamDriveFailed { code } => write!(f, "team drive failed: {code}"),
            Self::TeamShapeInvalid { reason } => write!(f, "team room shape invalid: {reason}"),
            Self::LeaderNotAMember { name } => {
                write!(f, "\"{name}\" is designated leader but is not a room member")
            }
            Self::TeamCyclesOutOfRange { value } => write!(
                f,
                "max_cycles {value} is out of range [{}, {}]",
                crate::protocol::TEAM_CYCLE_MIN,
                crate::protocol::TEAM_CYCLE_MAX
            ),
            Self::ResetRefusedDriveInFlight => write!(
                f,
                "a round drive is currently in flight for this room — the reset was refused, retry once it finishes"
            ),
            Self::IndexProjectionStale => write!(
                f,
                "the conversation reset landed, but the room list's cached copy is stale — it will refresh on the next reload"
            ),
            Self::ConversationEpochExhausted => write!(
                f,
                "this room has already reset the maximum number of times and cannot reset again"
            ),
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
/// against the `[min_members, max_members]` bound (see below),
/// returning [`GroupChatError::MemberCountOutOfRange`]. Then validates each
/// name through `ironhermes_core::profile::validate_profile_name` plus a
/// `crate::server::profile_api::profile_dir_for(...).is_dir()` existence
/// check, returning [`GroupChatError::MemberNotFound`] for either failure.
///
/// Phase 52 (D-18): the `[min_members, max_members]` bound is read through
/// [`crate::server::group_settings_api::load_group_settings_impl`] — the
/// OPERATOR-PERSISTED record, the same one
/// [`crate::server::group_chat_api::group_chat_settings_for_drive`] reads
/// for the round driver's own bounds. Before this fix the bound was
/// hardcoded to [`GroupChatSettings::default`], so a room could be created
/// against a bound the driver was not actually enforcing. A settings-file
/// read failure (missing OR corrupt) falls back to
/// [`GroupChatSettings::default`] — the same "a corrupted settings file
/// must never prevent a room from running" policy
/// `group_chat_settings_for_drive`'s own doc comment records, cited here by
/// name so the two fallbacks are visibly the same policy, not two
/// independently-invented ones.
///
/// Performs ONE settings-file read and NO locking of its own —
/// `load_group_settings_impl` reads `group-chat-settings.json` directly, it
/// does not take the settings store's own mutex for a read. The read
/// happens BEFORE either caller (`create_room_impl`/
/// `create_room_with_team_impl` and [`update_room_members_impl`]) acquires
/// [`GROUP_CHAT_LOCK`] — both call this fn above their own
/// `let _guard = GROUP_CHAT_LOCK.lock()` line — so this store's chat lock
/// is never nested inside (or around) the settings store's own mutex,
/// which only ever guards a settings SAVE.
#[cfg(feature = "server")]
pub(crate) fn validate_room_members(members: &[String]) -> Result<Vec<String>, GroupChatError> {
    let mut deduped: Vec<String> = Vec::with_capacity(members.len());
    for member in members {
        if !deduped.contains(member) {
            deduped.push(member.clone());
        }
    }

    // Phase 52 (D-18): read the operator-persisted record rather than the
    // hardcoded default. See this fn's doc comment for the fallback policy.
    let settings = crate::server::group_settings_api::load_group_settings_impl()
        .unwrap_or_else(|_| GroupChatSettings::default());
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

/// Phase 50.2 Plan 01: create a plain peer room. Validates the name (own
/// validator, never `validate_profile_name`) and the members through
/// [`validate_room_members`] — the count against the operator-persisted
/// `[min_members, max_members]` bound (D-18), and every member name against
/// `ironhermes_core::profile::validate_profile_name` plus an on-disk
/// existence check (a room member must be a real bot).
///
/// Phase 52 (D-09): a thin wrapper over [`create_room_with_team_impl`] with
/// `team: None` — kept as its own fn, rather than folding its ~35 existing
/// call sites across this crate into the team-aware signature, because
/// every one of those call sites creates a plain peer room and a bare
/// `None` third argument at each would carry no information. The two fns
/// share one write path; this one is byte-for-byte what it was before this
/// phase. `#[allow(dead_code)]`: every remaining call site is this crate's
/// own test suite (`create_group_room`, the one production caller, now
/// calls `create_room_with_team_impl` directly to forward `req.team`) —
/// same precedent `VerifyOutcome`/`CloneFromChoice::Import`
/// (`protocol.rs`) and this file's own `set_room_needs_you_impl` already
/// set for a fn with no production call site yet.
#[cfg(feature = "server")]
#[allow(dead_code)]
pub(crate) fn create_room_impl(name: &str, members: &[String]) -> Result<GroupRoom, GroupChatError> {
    create_room_with_team_impl(name, members, None)
}

/// Phase 52 (D-09): create a room, optionally AS a team room, in the same
/// write path [`create_room_impl`] uses for a plain peer room — D-09's
/// "same persisted write path, no schema migration" requirement. When
/// `team` is `Some`, the setup is validated through
/// [`validate_team_room_shape`] (against the validated member list) BEFORE
/// the lock is taken — at creation there is no CURRENT room, so
/// [`normalize_team_setup_for_write`]'s demotion step does not apply; a
/// setup that designates a leader absent from the member list is rejected
/// outright, never silently dropped. Locks once for the whole create
/// sequence: index-duplicate check, transcript write, index write.
#[cfg(feature = "server")]
pub(crate) fn create_room_with_team_impl(
    name: &str,
    members: &[String],
    team: Option<&GroupRoomTeamSetup>,
) -> Result<GroupRoom, GroupChatError> {
    let validated_name = validate_group_room_name(name)?;
    let validated_members = validate_room_members(members)?;

    let (pattern, roles, max_cycles, leader_prompt_override, worker_prompt_override) = match team {
        Some(setup) => {
            validate_team_room_shape(
                &setup.pattern,
                &setup.roles,
                &validated_members,
                setup.max_cycles,
            )?;
            (
                setup.pattern.clone(),
                setup.roles.clone(),
                setup.max_cycles,
                setup.leader_prompt_override.clone(),
                setup.worker_prompt_override.clone(),
            )
        }
        // Phase 52: no team setup submitted — a freshly created room is a
        // plain peer room, byte-for-byte Plan 02's hardcoded literal.
        None => (None, BTreeMap::new(), None, None, None),
    };

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
        needs_you_reason: None,
        preview: None,
        preview_at_ms: None,
        created_at_ms: now,
        updated_at_ms: now,
        pattern,
        roles,
        max_cycles,
        leader_prompt_override,
        worker_prompt_override,
        conversation_epoch: 1,
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

/// Phase 52: test-only room constructor that also sets `pattern`/`roles` —
/// the `#[server]` surface for enabling a team pattern on an existing room
/// is Plan 03's job, so this tracer's own tests need a direct way to get a
/// team room onto disk. `#[cfg(test)]` (not gated inside `mod tests`) so it
/// is crate-visible to `group_team_api.rs`'s own test module during
/// `cargo test`, mirroring Wave 0's `stub_script_fixture` precedent for
/// crate-visible test-only infra. Follows the same
/// lock→load→mutate→write-detail→write-index sequence every other mutator
/// in this module uses.
#[cfg(all(test, feature = "server"))]
pub(crate) fn create_team_room_for_test(
    name: &str,
    members: &[String],
    pattern: crate::protocol::TeamPattern,
    roles: BTreeMap<String, crate::protocol::MemberRole>,
) -> Result<GroupRoom, GroupChatError> {
    let mut room = create_room_impl(name, members)?;

    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    room.pattern = Some(pattern);
    room.roles = roles;

    write_transcript(
        &room.id,
        &GroupRoomTranscript {
            room: room.clone(),
            messages: Vec::new(),
        },
    )?;
    let mut index = load_room_index()?;
    index.insert(room.id.clone(), room.clone());
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
            // Phase 52 (D-06/D-01, same reachability class as `group`
            // above): without this the roster row can never see a room's
            // team status no matter what the store holds.
            pattern: room.pattern,
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
///
/// Phase 52 (Round 1 codex MEDIUM): widened to carry an optional `reason`
/// alongside the flag — raising the flag stores the reason, clearing it
/// clears the reason, in the same write. Today `needs_you` had nowhere to
/// say WHY it was raised, and UI-SPEC contracts two distinct advisory
/// sentences (cycle exhaustion, leader-contract failure) the badge alone
/// cannot carry. Every EXISTING call site passes `None`, preserving current
/// behaviour exactly — the two production call sites in
/// `group_chat_api.rs` (the operator-message clear and the mention-
/// triggered raise, `score.needs_you && !needs_you`) have no contracted
/// sentence to offer, and inventing one here would put un-contracted copy
/// on a peer-room path. Plan 04 is the only future caller that ever passes
/// `Some`.
///
/// **Caller audit (re-run, do not trust a stated count):**
/// `grep -rn 'set_room_needs_you_impl(' crates/ --include='*.rs'` finds
/// this definition plus 7 call sites: 2 production in `group_chat_api.rs`
/// (the clear and the raise) and 5 in this crate's own tests
/// (`group_chat_api.rs` x2, `group_chat_store.rs` x3, including this fn's
/// own `set_room_needs_you_impl_toggles_the_flag`). Every one of them
/// passes `None`.
#[cfg(feature = "server")]
pub(crate) fn set_room_needs_you_impl(
    room_id: &str,
    needs_you: bool,
    reason: Option<String>,
) -> Result<(), GroupChatError> {
    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;
    transcript.room.needs_you = needs_you;
    transcript.room.needs_you_reason = reason;
    transcript.room.updated_at_ms = now_ms();
    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(())
}

/// Phase 52 (D-06/D-07/D-09, RESEARCH Pitfall 5): the SINGLE enforcement
/// point for the team-room shape invariant — creation
/// ([`create_room_with_team_impl`]), conversion and demotion
/// ([`update_room_team_impl`], [`update_room_members_impl`]) all call this
/// before persisting, so a room the team driver cannot dispatch from can
/// never be WRITTEN (reading an already-violating record must still
/// succeed — see those two fns' doc comments). Pure: takes no lock and
/// performs no I/O, so it is safe to call from inside `GROUP_CHAT_LOCK`
/// once a caller has already loaded the current room.
///
/// Two independent rules:
/// - **Shape.** `pattern.is_some()` must equal "`roles` contains exactly
///   one `Leader` entry." A `Leader` entry naming a name absent from
///   `members` is [`GroupChatError::LeaderNotAMember`]. Two `Leader`
///   entries, a `Leader` with no `pattern`, or a `pattern` with no
///   `Leader`, is [`GroupChatError::TeamShapeInvalid`].
/// - **Budget (D-14, Round 1 codex HIGH).** `max_cycles` must be `None`,
///   or `Some(n)` with `n` inside
///   `protocol::TEAM_CYCLE_MIN..=protocol::TEAM_CYCLE_MAX`, otherwise
///   [`GroupChatError::TeamCyclesOutOfRange`]. This is NOT redundant with
///   [`crate::server::group_settings_api::clamp_group_settings`] — that fn
///   has exactly one caller, `save_group_settings_impl`, on the APP-WIDE
///   settings save path; no room write path reaches it, so without this
///   check a per-room `max_cycles` override — D-14's primary cost lever —
///   would have no authoritative validation anywhere.
#[cfg(feature = "server")]
pub(crate) fn validate_team_room_shape(
    pattern: &Option<crate::protocol::TeamPattern>,
    roles: &BTreeMap<String, MemberRole>,
    members: &[String],
    max_cycles: Option<u32>,
) -> Result<(), GroupChatError> {
    let leaders: Vec<&String> = roles
        .iter()
        .filter(|(_, role)| matches!(role, MemberRole::Leader))
        .map(|(name, _)| name)
        .collect();

    match (pattern.is_some(), leaders.len()) {
        (true, 1) => {
            let leader_name = leaders[0];
            if !members.iter().any(|m| m == leader_name) {
                return Err(GroupChatError::LeaderNotAMember {
                    name: leader_name.clone(),
                });
            }
        }
        (true, 0) => {
            return Err(GroupChatError::TeamShapeInvalid {
                reason: "pattern is set but roles has no Leader entry".to_string(),
            });
        }
        (false, 0) => {}
        // (true, 2+) and (false, 1+) both land here: either more than one
        // Leader entry, or a Leader entry with no pattern set.
        _ => {
            return Err(GroupChatError::TeamShapeInvalid {
                reason: "roles' Leader entries and pattern disagree: exactly one Leader \
                         entry is required when, and only when, pattern is set"
                    .to_string(),
            });
        }
    }

    if let Some(n) = max_cycles {
        if !(crate::protocol::TEAM_CYCLE_MIN..=crate::protocol::TEAM_CYCLE_MAX).contains(&n) {
            return Err(GroupChatError::TeamCyclesOutOfRange { value: n });
        }
    }

    Ok(())
}

/// Phase 52 (D-07, Round 1 codex MEDIUM): the state-dependent half of D-07's
/// demotion rule, kept separate from the pure [`validate_team_room_shape`]
/// because it needs the CURRENT persisted room — only available once a
/// caller has loaded the transcript under [`GROUP_CHAT_LOCK`]. Applies
/// demotion in exactly ONE case: the room currently HAS a persisted
/// `Leader` entry, and that persisted leader's name is absent from the
/// incoming `members` list. In that case, it returns a setup with `roles`
/// cleared and `pattern` set to `None` (D-07's "unrepresentable state the
/// driver hits at dispatch time" cannot be persisted). In every OTHER
/// case — including when `setup` itself DESIGNATES a leader absent from
/// `members`, while the room's CURRENT leader is still present — `setup`
/// is returned unchanged, so [`validate_team_room_shape`] rejects that case
/// as [`GroupChatError::LeaderNotAMember`] rather than it being silently
/// normalized into a demotion. Without this split, a setup naming a
/// never-a-member leader would be indistinguishable from an operator who
/// removed the real leader from membership, turning an explicit validation
/// error into a silent scope change. Pure: takes no lock and performs no
/// I/O.
#[cfg(feature = "server")]
pub(crate) fn normalize_team_setup_for_write(
    current: &GroupRoom,
    members: &[String],
    setup: &GroupRoomTeamSetup,
) -> GroupRoomTeamSetup {
    let current_leader = current
        .roles
        .iter()
        .find(|(_, role)| matches!(role, MemberRole::Leader))
        .map(|(name, _)| name.clone());

    if let Some(leader) = current_leader {
        if !members.iter().any(|m| m == &leader) {
            return GroupRoomTeamSetup {
                pattern: None,
                roles: BTreeMap::new(),
                max_cycles: setup.max_cycles,
                leader_prompt_override: setup.leader_prompt_override.clone(),
                worker_prompt_override: setup.worker_prompt_override.clone(),
            };
        }
    }

    setup.clone()
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
/// persisted copies stay byte-for-byte as they were.
///
/// Phase 52 (D-07): also runs [`normalize_team_setup_for_write`] then
/// [`validate_team_room_shape`], INSIDE the lock, against a setup derived
/// from the room's OWN current team fields (this call never carries a
/// submitted team setup — [`update_room_team_impl`] is that surface) — so a
/// membership edit that drops the persisted leader demotes the room to a
/// peer room in the SAME write, and a caller cannot hand-craft a way past
/// the invariant by using this fn instead of `update_room_team_impl`.
/// Leaving `pattern` `Some` with no leader would be the unrepresentable
/// state the driver hits at dispatch time — D-07's stated reason. A
/// rejection returns before any mutation, so `members` stays unchanged too.
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

    let current_setup = GroupRoomTeamSetup {
        pattern: transcript.room.pattern.clone(),
        roles: transcript.room.roles.clone(),
        max_cycles: transcript.room.max_cycles,
        leader_prompt_override: transcript.room.leader_prompt_override.clone(),
        worker_prompt_override: transcript.room.worker_prompt_override.clone(),
    };
    let normalized =
        normalize_team_setup_for_write(&transcript.room, &validated_members, &current_setup);
    validate_team_room_shape(
        &normalized.pattern,
        &normalized.roles,
        &validated_members,
        normalized.max_cycles,
    )?;

    transcript.room.members = validated_members;
    transcript.room.pattern = normalized.pattern;
    transcript.room.roles = normalized.roles;
    transcript.room.max_cycles = normalized.max_cycles;
    transcript.room.leader_prompt_override = normalized.leader_prompt_override;
    transcript.room.worker_prompt_override = normalized.worker_prompt_override;
    transcript.room.updated_at_ms = now_ms();
    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(transcript.room)
}

/// Phase 52 (D-06/D-07/D-09): change an EXISTING room's membership AND team
/// composition in ONE write — the `#[server]` counterpart of
/// [`crate::protocol::UpdateGroupRoomTeamRequest`]'s doc comment: membership
/// and team composition ride together so the edit modal cannot half-save.
/// Mirrors [`update_room_members_impl`]'s structure exactly: validates
/// through [`validate_room_members`] BEFORE the lock, then takes
/// [`GROUP_CHAT_LOCK`] once, loads the transcript, runs
/// [`normalize_team_setup_for_write`] against the loaded room (applying
/// D-07's demotion if the room's CURRENT persisted leader is absent from
/// the incoming member list) then [`validate_team_room_shape`] on the
/// result — and only if that passes, applies members AND the team fields
/// together, bumps `updated_at_ms`, writes the transcript, then the index.
/// A rejection returns before any mutation, so persisted members are
/// unchanged too. The normalization and validation run inside the lock
/// because they depend on the loaded room; both are pure and lock-free, so
/// no second lock is taken and no lock ordering is introduced.
#[cfg(feature = "server")]
pub(crate) fn update_room_team_impl(
    room_id: &str,
    members: &[String],
    setup: &GroupRoomTeamSetup,
) -> Result<GroupRoom, GroupChatError> {
    let validated_members = validate_room_members(members)?;

    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;

    let normalized = normalize_team_setup_for_write(&transcript.room, &validated_members, setup);
    validate_team_room_shape(
        &normalized.pattern,
        &normalized.roles,
        &validated_members,
        normalized.max_cycles,
    )?;

    transcript.room.members = validated_members;
    transcript.room.pattern = normalized.pattern;
    transcript.room.roles = normalized.roles;
    transcript.room.max_cycles = normalized.max_cycles;
    transcript.room.leader_prompt_override = normalized.leader_prompt_override;
    transcript.room.worker_prompt_override = normalized.worker_prompt_override;
    transcript.room.updated_at_ms = now_ms();
    write_transcript(room_id, &transcript)?;

    let mut index = load_room_index()?;
    index.insert(room_id.to_string(), transcript.room.clone());
    write_room_index(&index)?;

    Ok(transcript.room)
}

/// Phase 52 Plan 05 (D-02, Round 1 codex HIGH): the fixed, host-owned text
/// of the marker row a "New conversation" reset appends. This is the FIRST
/// writer of `GroupRoomSpeaker::System` in the codebase — that variant was
/// reserved with no writer since Phase 50.2 Plan 01.
#[cfg(feature = "server")]
pub(crate) const CONVERSATION_RESET_MARKER_TEXT: &str =
    "Conversation reset — earlier messages are no longer replayed to members.";

/// Phase 52 Plan 05 (D-02/D-17): the "New conversation" action's store
/// implementation — resets a room's conversational continuity WITHOUT
/// touching `pattern`, `roles`, membership, or any other room field (D-15).
/// The room-side and child-side breaks D-02 exists to fix are NOT two
/// independent mechanisms that can diverge: this fn bumps
/// `conversation_epoch` and appends one marker row in a SINGLE
/// `write_transcript` call, and the child-side break — every member's own
/// CLI subprocess resuming a DIFFERENT session — is a DERIVED consequence,
/// computed at the NEXT round drive when `group_chat_api::group_session_title`
/// renders a different title from the newly persisted epoch. Applies to
/// EVERY room, peer and team alike (D-17).
///
/// **Coordinates with drive ownership BEFORE anything else (Round 1 codex
/// HIGH).** A drive holds its `transcript` and `session_title` as locals
/// across every await and persists rows by re-loading under
/// `GROUP_CHAT_LOCK` and appending — so a reset that only took that lock
/// could not stop an in-flight drive from landing rows AFTER this reset's
/// own marker row, which `group_chat_api::conversation_start_index` would
/// then read as being INSIDE the new conversation, silently
/// re-contaminating it. `handoff_steering::try_acquire_room_drive` is
/// acquired FIRST, before `GROUP_CHAT_LOCK`, and held for the WHOLE reset —
/// a busy room returns [`GroupChatError::ResetRefusedDriveInFlight`] with
/// NOTHING mutated, and holding the guard across the whole reset also means
/// a drive cannot start mid-reset.
///
/// Then the room's queued pre-reset steering messages are drained and
/// DISCARDED (`handoff_steering::drain_room_steering`) — those messages
/// were queued against the conversation being ended, so carrying them into
/// the new one would be the exact carryover D-02 exists to break, and
/// leaving them queued would replay them into a conversation they were
/// never written for.
///
/// **The epoch advances via `checked_add`, never `saturating_add` (Round 1
/// codex suggestion).** A saturating add at `u32::MAX` is a silent no-op —
/// it would report a successful clean slate while producing the SAME title
/// and reusing the SAME child session. `None` returns
/// [`GroupChatError::ConversationEpochExhausted`] with the transcript
/// completely unchanged.
///
/// **Names the two-file write window honestly (Round 1 codex HIGH) rather
/// than asserting it away.** `write_transcript` and the index refresh below
/// are two separate `write_json_atomic` calls, atomic individually and NOT
/// jointly — mirroring [`update_room_members_impl`]'s own lock→load→
/// mutate→write-transcript→write-index sequence. The transcript is the
/// AUTHORITATIVE copy: `run_group_rounds_with_settings` reads
/// `transcript.room.conversation_epoch`, never the index copy. If
/// `write_transcript` fails, nothing has changed and the ordinary
/// [`GroupChatError::StoreIo`] is correct and retryable. If it SUCCEEDS and
/// the index load or write fails afterward, the reset HAS LANDED and only
/// the roster's projection is stale — [`GroupChatError::IndexProjectionStale`]
/// is returned instead of `StoreIo`, so a caller never retries a landed
/// reset into a second, orphaning epoch bump. The next successful write
/// through any store path repairs the stale index projection on its own.
#[cfg(feature = "server")]
pub(crate) fn reset_room_conversation_impl(room_id: &str) -> Result<GroupRoom, GroupChatError> {
    let Some(_drive_guard) = crate::server::handoff_steering::try_acquire_room_drive(room_id)
    else {
        return Err(GroupChatError::ResetRefusedDriveInFlight);
    };

    // Those messages were queued against the conversation being ended;
    // carrying them into the new one is the same carryover D-02 exists to
    // break. Discarded, not replayed.
    let _ = crate::server::handoff_steering::drain_room_steering(room_id);

    let _guard = GROUP_CHAT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let mut transcript = load_transcript(room_id)?.ok_or_else(|| GroupChatError::RoomNotFound {
        id: room_id.to_string(),
    })?;

    let Some(next_epoch) = transcript.room.conversation_epoch.checked_add(1) else {
        return Err(GroupChatError::ConversationEpochExhausted);
    };
    transcript.room.conversation_epoch = next_epoch;
    transcript.messages.push(GroupRoomMessage {
        from: crate::protocol::GroupRoomSpeaker::System,
        text: CONVERSATION_RESET_MARKER_TEXT.to_string(),
        at_ms: now_ms(),
        round: 0,
        status: MemberTurnStatus::Replied,
        team_row: None,
    });
    transcript.room.updated_at_ms = now_ms();

    // The transcript write is the reset's ONE authoritative commit point —
    // a failure here means nothing changed at all.
    write_transcript(room_id, &transcript)?;

    // From here on, the reset has LANDED. A failure in either of these two
    // steps is reported as IndexProjectionStale, never StoreIo, precisely
    // so a caller never retries into a second epoch bump.
    let mut index = match load_room_index() {
        Ok(index) => index,
        Err(_) => return Err(GroupChatError::IndexProjectionStale),
    };
    index.insert(room_id.to_string(), transcript.room.clone());
    if write_room_index(&index).is_err() {
        return Err(GroupChatError::IndexProjectionStale);
    }

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
    // Phase 52 Plan 01 (Wave 0, RESEARCH Pitfall 2): pre-Phase-52 JSON
    // fixtures, captured under the CURRENT struct definitions before any
    // serde-default field is added. `include_str!` follows the static-asset
    // precedent already used in `chat_attachments_api.rs`/`mcp_admin_api.rs`
    // (self-source scans, not fixture data, but the same "compile a tracked
    // file into the test binary" idiom).
    // -------------------------------------------------------------------

    /// Phase 52 Plan 01: the room INDEX MAP shape `load_room_index`
    /// deserializes (`BTreeMap<String, GroupRoom>`) — the on-disk
    /// `group-rooms.json` shape, captured before any Phase 52 field exists.
    const PRE52_GROUP_ROOMS_FIXTURE: &str =
        include_str!("fixtures/pre52_group_rooms.json");

    /// Phase 52 Plan 01: one full `GroupRoomTranscript` (`room` plus a
    /// non-empty `messages` array) — the per-room sidecar shape
    /// `load_transcript` deserializes. Carries an Operator row, a `Member`
    /// row, and a `Failed { reason }` row so Plan 02's `GroupRoomMessage`
    /// extension has real old-data input for every persisted variant shape.
    const PRE52_GROUP_ROOM_TRANSCRIPT_FIXTURE: &str =
        include_str!("fixtures/pre52_group_room_transcript.json");

    /// Phase 52 Plan 01: one `GroupChatSettings` at its pre-Phase-52
    /// `Default` values, captured before D-14's `max_cycles`/
    /// `max_workers_per_delegation` fields exist.
    const PRE52_GROUP_CHAT_SETTINGS_FIXTURE: &str =
        include_str!("fixtures/pre52_group_chat_settings.json");

    /// Phase 52 Plan 02 Task 3: this test's pre-schema form
    /// (`pre52_group_rooms_index_fixture_is_field_for_field_todays_index_shape`)
    /// was green at Plan 01's base commit, BEFORE any Phase 52 field
    /// existed. Task 2 added six new `GroupRoom` fields, which broke that
    /// field-for-field-equality assertion by construction (today's struct
    /// now serializes MORE keys than the untouched fixture carries) — this
    /// is RESEARCH Pitfall 2's "old data, new code" case: the fixture must
    /// still DESERIALIZE (not round-trip byte-identically) and every new
    /// field must land on its documented serde default. Every pre-existing
    /// field assertion is kept so a field that silently changed meaning
    /// would also be caught.
    #[test]
    fn pre52_group_rooms_index_fixture_still_deserializes_after_the_phase_52_fields() {
        let parsed: BTreeMap<String, GroupRoom> =
            serde_json::from_str(PRE52_GROUP_ROOMS_FIXTURE)
                .expect("pre52 group-rooms fixture must still deserialize under today's GroupRoom shape");
        let room = parsed.get("standup").expect("fixture must carry a \"standup\" room");

        // Pre-existing fields must keep their captured values.
        assert_eq!(room.id, "standup");
        assert_eq!(room.name, "standup");
        assert_eq!(room.members, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(room.group, None);
        assert!(!room.needs_you);
        assert_eq!(room.preview.as_deref(), Some("kickoff"));
        assert_eq!(room.preview_at_ms, Some(1757000000000));
        assert_eq!(room.created_at_ms, 1756900000000);
        assert_eq!(room.updated_at_ms, 1757000000000);

        // Every Phase 52 field must land on its documented serde default.
        assert_eq!(room.pattern, None);
        assert!(room.roles.is_empty());
        assert_eq!(room.max_cycles, None);
        assert_eq!(room.leader_prompt_override, None);
        assert_eq!(room.worker_prompt_override, None);
        assert_eq!(room.conversation_epoch, 1);
    }

    /// Phase 52 Plan 02 Task 3: post-schema twin of
    /// `pre52_group_room_transcript_fixture_is_field_for_field_todays_transcript_shape`
    /// — same "still deserializes, defaults land" contract as the index
    /// test above, plus the specific old-data case `GroupRoomMessage.team_row`
    /// creates: all three pre-Phase-52 messages must still load, each with
    /// `team_row: None`.
    #[test]
    fn pre52_group_room_transcript_fixture_still_deserializes_after_the_phase_52_fields() {
        let parsed: GroupRoomTranscript =
            serde_json::from_str(PRE52_GROUP_ROOM_TRANSCRIPT_FIXTURE).expect(
                "pre52 group-room-transcript fixture must still deserialize under today's GroupRoomTranscript shape",
            );

        let room = &parsed.room;
        assert_eq!(room.id, "standup");
        assert_eq!(room.members, vec!["alpha".to_string(), "beta".to_string()]);
        assert_eq!(room.group, None);
        assert!(!room.needs_you);
        assert_eq!(room.preview.as_deref(), Some("kickoff"));
        assert_eq!(room.preview_at_ms, Some(1757000000000));
        assert_eq!(room.created_at_ms, 1756900000000);
        assert_eq!(room.updated_at_ms, 1757000000000);
        assert_eq!(room.pattern, None);
        assert!(room.roles.is_empty());
        assert_eq!(room.max_cycles, None);
        assert_eq!(room.leader_prompt_override, None);
        assert_eq!(room.worker_prompt_override, None);
        assert_eq!(room.conversation_epoch, 1);

        assert_eq!(parsed.messages.len(), 3, "all three pre-Phase-52 messages must still load");
        for msg in &parsed.messages {
            assert_eq!(msg.team_row, None, "an old-data message must default team_row to None");
        }
        assert_eq!(parsed.messages[0].from, crate::protocol::GroupRoomSpeaker::Operator);
        assert_eq!(parsed.messages[0].text, "morning, what's the status?");
        assert_eq!(
            parsed.messages[1].from,
            crate::protocol::GroupRoomSpeaker::Member("alpha".to_string())
        );
        assert_eq!(parsed.messages[1].status, MemberTurnStatus::Passed);
        assert_eq!(
            parsed.messages[2].from,
            crate::protocol::GroupRoomSpeaker::Member("beta".to_string())
        );
        assert_eq!(
            parsed.messages[2].status,
            MemberTurnStatus::Failed {
                reason: "timeout after 180s".to_string()
            }
        );
    }

    /// Phase 52 Plan 02 Task 3: post-schema twin of
    /// `pre52_group_chat_settings_fixture_is_field_for_field_todays_settings_shape`
    /// — same "still deserializes, defaults land" contract. The five
    /// pre-existing tunables keep their captured (D-21 default) values; the
    /// two new Phase 52 tunables must land on their documented serde
    /// defaults, `TEAM_CYCLE_MIN` and `TEAM_WORKERS_MAX`.
    #[test]
    fn pre52_group_chat_settings_fixture_still_deserializes_after_the_phase_52_fields() {
        let parsed: GroupChatSettings =
            serde_json::from_str(PRE52_GROUP_CHAT_SETTINGS_FIXTURE).expect(
                "pre52 group-chat-settings fixture must still deserialize under today's GroupChatSettings shape",
            );

        assert_eq!(parsed.max_rounds, 3);
        assert_eq!(parsed.max_messages, 10);
        assert_eq!(parsed.history_limit, 24);
        assert_eq!(parsed.min_members, 2);
        assert_eq!(parsed.max_members, 6);

        assert_eq!(parsed.max_cycles, crate::protocol::TEAM_CYCLE_MIN);
        assert_eq!(parsed.max_workers_per_delegation, crate::protocol::TEAM_WORKERS_MAX);
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

    #[test]
    fn the_epoch_separator_is_rejected_by_the_room_name_validator() {
        // Phase 52 Plan 05 (D-02, Round 1 codex HIGH): the paired half of
        // the whole collision-freedom argument — a room name must NEVER be
        // able to contain `group_chat_api::CONVERSATION_EPOCH_SEPARATOR`
        // (`#`), because an epoch-discriminated session title's uniqueness
        // depends entirely on a room name containing zero separators. This
        // is the one test in the crate enforcing that disjointness; nothing
        // else does.
        let err = validate_group_room_name("standup#2").unwrap_err();
        assert!(
            matches!(err, GroupChatError::RoomNameRejected { .. }),
            "a room name containing the reserved epoch separator must be rejected"
        );
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
            team_row: None,
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

        set_room_needs_you_impl(&room.id, true, None).expect("set needs_you true");
        let loaded = load_room_impl(&room.id).expect("load after set");
        assert!(loaded.room.needs_you);

        set_room_needs_you_impl(&room.id, false, None).expect("set needs_you false");
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
            team_row: None,
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
            team_row: None,
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
            team_row: None,
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
        set_room_needs_you_impl(&room.id, true, None).expect("set needs_you true");
        let messages = vec![GroupRoomMessage {
            from: crate::protocol::GroupRoomSpeaker::Member("zig".to_string()),
            text: "shipped the tracer".to_string(),
            at_ms: 1,
            round: 1,
            status: MemberTurnStatus::Replied,
            team_row: None,
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
        // Phase 52 (D-18): no settings file is persisted in this test, so
        // `validate_room_members` reads through to the fallback default —
        // this is now the FALLBACK path, not a hardcoded one; see the two
        // tests below for the persisted-record path this fn now also
        // exercises.
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
                    "G-50.2-2a/D-18: with no settings file present, the reported max must \
                     equal the fallback GroupChatSettings::default()'s value — this fn \
                     remains the only bound source"
                );
            }
            other => panic!("expected MemberCountOutOfRange, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 03, Task 1 (D-18): validate_room_members reads the
    // persisted settings record.
    // -------------------------------------------------------------------

    fn write_group_chat_settings(dir: &tempfile::TempDir, settings: &GroupChatSettings) {
        let path = crate::server::group_settings_api::group_settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir settings parent");
        }
        std::fs::write(
            &path,
            serde_json::to_string_pretty(settings).expect("serialize settings"),
        )
        .expect("write group-chat-settings.json");
        let _ = dir;
    }

    #[test]
    fn validate_room_members_honours_a_persisted_non_default_max_members() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let persisted = GroupChatSettings {
            min_members: 2,
            max_members: 3,
            ..GroupChatSettings::default()
        };
        write_group_chat_settings(&dir, &persisted);

        let members: Vec<String> = ["bot0", "bot1", "bot2", "bot3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for m in &members {
            scaffold_profile(m);
        }

        let err = validate_room_members(&members).unwrap_err();
        match err {
            GroupChatError::MemberCountOutOfRange { max, .. } => {
                assert_eq!(
                    max, 3,
                    "D-18: the reported max must be the PERSISTED record's max_members (3), \
                     not GroupChatSettings::default()'s (6)"
                );
            }
            other => panic!("expected MemberCountOutOfRange, got {other:?}"),
        }
    }

    #[test]
    fn validate_room_members_falls_back_to_defaults_when_the_settings_file_is_corrupt() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        let path = crate::server::group_settings_api::group_settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir settings parent");
        }
        std::fs::write(&path, b"not valid json {{{").expect("write corrupt settings file");

        let two_members: Vec<String> = ["bot0", "bot1"].iter().map(|s| s.to_string()).collect();
        for m in &two_members {
            scaffold_profile(m);
        }
        assert!(
            validate_room_members(&two_members).is_ok(),
            "D-18: a corrupted settings file must never prevent a room from running — a \
             2-member list must be accepted under the fallback default's min_members (2)"
        );

        let seven_members: Vec<String> = (0..7).map(|i| format!("bot{i}")).collect();
        for m in &seven_members {
            scaffold_profile(m);
        }
        let err = validate_room_members(&seven_members).unwrap_err();
        match err {
            GroupChatError::MemberCountOutOfRange { max, .. } => {
                assert_eq!(
                    max, 6,
                    "D-18: a corrupted settings file falls back to the shipped default's \
                     max_members (6)"
                );
            }
            other => panic!("expected MemberCountOutOfRange, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // Phase 52 Plan 03, Task 2 (D-06/D-07/D-09): the team-shape invariant
    // and the store write paths that enforce it.
    // -------------------------------------------------------------------

    fn leader_role(name: &str) -> BTreeMap<String, MemberRole> {
        let mut roles = BTreeMap::new();
        roles.insert(name.to_string(), MemberRole::Leader);
        roles
    }

    fn team_setup(
        pattern: Option<crate::protocol::TeamPattern>,
        roles: BTreeMap<String, MemberRole>,
        max_cycles: Option<u32>,
    ) -> GroupRoomTeamSetup {
        GroupRoomTeamSetup {
            pattern,
            roles,
            max_cycles,
            leader_prompt_override: None,
            worker_prompt_override: None,
        }
    }

    #[test]
    fn a_team_room_cannot_be_persisted_with_pattern_set_and_no_leader() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let room = create_room_impl("Ops Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");

        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            BTreeMap::new(),
            None,
        );
        let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
        assert!(
            matches!(err, GroupChatError::TeamShapeInvalid { .. }),
            "pattern set with no Leader entry must be TeamShapeInvalid, got {err:?}"
        );

        let reloaded = load_room_impl(&room.id).expect("load after rejected update");
        assert_eq!(
            reloaded.room.pattern, None,
            "a rejected shape update must leave the persisted room untouched"
        );
    }

    #[test]
    fn a_team_room_cannot_be_persisted_with_a_leader_who_is_not_a_member() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        scaffold_profile("zulu");

        let room = create_room_impl("Ops Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");

        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            leader_role("zulu"),
            None,
        );
        let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
        assert!(
            matches!(err, GroupChatError::LeaderNotAMember { ref name } if name == "zulu"),
            "a Leader entry naming a non-member must be LeaderNotAMember, got {err:?}"
        );

        let reloaded = load_room_impl(&room.id).expect("load after rejected update");
        assert_eq!(
            reloaded.room.pattern, None,
            "a rejected shape update must leave the persisted room untouched"
        );
    }

    #[test]
    fn a_team_room_setup_with_a_leader_role_but_no_pattern_or_two_leaders_is_rejected() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let room = create_room_impl("Ops Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");

        // Mirror case: a Leader entry with no pattern set is dead role
        // metadata that must not be allowed to accumulate.
        let setup = team_setup(None, leader_role("alpha"), None);
        let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
        assert!(
            matches!(err, GroupChatError::TeamShapeInvalid { .. }),
            "a Leader entry with no pattern must be TeamShapeInvalid, got {err:?}"
        );

        // Two Leader entries: D-06 ships exactly one Leader arm this phase.
        let mut two_leaders = BTreeMap::new();
        two_leaders.insert("alpha".to_string(), MemberRole::Leader);
        two_leaders.insert("beta".to_string(), MemberRole::Leader);
        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            two_leaders,
            None,
        );
        let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
        assert!(
            matches!(err, GroupChatError::TeamShapeInvalid { .. }),
            "two Leader entries must be TeamShapeInvalid, got {err:?}"
        );
    }

    #[test]
    fn removing_the_designated_leader_clears_both_the_role_entry_and_the_pattern() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        scaffold_profile("gamma");

        let room = create_team_room_for_test(
            "Ops Room",
            &["alpha".to_string(), "beta".to_string()],
            crate::protocol::TeamPattern::OrchestratorWorkers,
            leader_role("alpha"),
        )
        .expect("create_team_room_for_test should succeed");

        let updated =
            update_room_members_impl(&room.id, &["beta".to_string(), "gamma".to_string()])
                .expect("dropping the leader from membership must be accepted, not rejected");

        assert_eq!(
            updated.pattern, None,
            "D-07: removing the persisted leader from membership must demote pattern to None"
        );
        assert!(
            updated.roles.is_empty(),
            "D-07: removing the persisted leader from membership must clear the roles map"
        );
        assert_eq!(
            updated.members,
            vec!["beta".to_string(), "gamma".to_string()]
        );

        let reloaded = load_room_impl(&room.id).expect("load after demotion");
        assert_eq!(reloaded.room.pattern, None);
        assert!(reloaded.room.roles.is_empty());
    }

    #[test]
    fn submitting_a_newly_invalid_leader_is_rejected_rather_than_silently_demoted() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        scaffold_profile("zulu");

        let room = create_team_room_for_test(
            "Ops Room",
            &["alpha".to_string(), "beta".to_string()],
            crate::protocol::TeamPattern::OrchestratorWorkers,
            leader_role("alpha"),
        )
        .expect("create_team_room_for_test should succeed");

        // The persisted leader (alpha) is STILL in the incoming member
        // list, but the submitted setup designates zulu — never a member —
        // as leader. This must be rejected, not normalized into a
        // demotion: normalize_team_setup_for_write only reacts to the
        // CURRENT persisted leader's absence from `members`, and alpha is
        // present.
        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            leader_role("zulu"),
            None,
        );
        let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
        assert!(
            matches!(err, GroupChatError::LeaderNotAMember { ref name } if name == "zulu"),
            "a newly-invalid leader must be rejected as LeaderNotAMember, got {err:?}"
        );

        let reloaded = load_room_impl(&room.id).expect("load after rejected update");
        assert_eq!(
            reloaded.room.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            "a rejected leader change must leave the room a team room, not demote it"
        );
        assert_eq!(
            reloaded.room.roles.get("alpha"),
            Some(&MemberRole::Leader),
            "the original leader (alpha) must still be in place"
        );
    }

    #[test]
    fn converting_a_peer_room_to_a_team_room_persists_pattern_and_leader_together() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let room = create_room_impl("Ops Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");
        assert_eq!(room.pattern, None, "must start as a plain peer room");

        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            leader_role("alpha"),
            None,
        );
        let updated = update_room_team_impl(&room.id, &room.members, &setup)
            .expect("converting a valid peer room to a team room must succeed");

        assert_eq!(
            updated.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers)
        );
        assert_eq!(updated.roles.get("alpha"), Some(&MemberRole::Leader));
        assert_eq!(
            updated.members, room.members,
            "D-09: conversion must leave membership untouched"
        );
    }

    #[test]
    fn a_room_max_cycles_override_outside_the_team_cycle_range_is_rejected_on_create() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        let members = vec!["alpha".to_string(), "beta".to_string()];

        for bad in [0u32, 6u32] {
            let setup = team_setup(
                Some(crate::protocol::TeamPattern::OrchestratorWorkers),
                leader_role("alpha"),
                Some(bad),
            );
            let err = create_room_with_team_impl("Cycle Room", &members, Some(&setup)).unwrap_err();
            assert!(
                matches!(err, GroupChatError::TeamCyclesOutOfRange { value } if value == bad),
                "max_cycles {bad} must be rejected on create, got {err:?}"
            );
        }

        for good in [1u32, 5u32] {
            let setup = team_setup(
                Some(crate::protocol::TeamPattern::OrchestratorWorkers),
                leader_role("alpha"),
                Some(good),
            );
            create_room_with_team_impl(&format!("Cycle Room {good}"), &members, Some(&setup))
                .unwrap_or_else(|e| panic!("max_cycles {good} must be accepted on create: {e:?}"));
        }
    }

    #[test]
    fn a_room_max_cycles_override_outside_the_team_cycle_range_is_rejected_on_update() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let room = create_team_room_for_test(
            "Ops Room",
            &["alpha".to_string(), "beta".to_string()],
            crate::protocol::TeamPattern::OrchestratorWorkers,
            leader_role("alpha"),
        )
        .expect("create_team_room_for_test should succeed");

        for bad in [0u32, 6u32] {
            let setup = team_setup(
                Some(crate::protocol::TeamPattern::OrchestratorWorkers),
                leader_role("alpha"),
                Some(bad),
            );
            let err = update_room_team_impl(&room.id, &room.members, &setup).unwrap_err();
            assert!(
                matches!(err, GroupChatError::TeamCyclesOutOfRange { value } if value == bad),
                "max_cycles {bad} must be rejected on update, got {err:?}"
            );
        }

        let reloaded = load_room_impl(&room.id).expect("load after rejected updates");
        assert_eq!(
            reloaded.room.max_cycles, None,
            "a rejected max_cycles update must leave the persisted room unchanged"
        );
    }

    #[test]
    fn a_room_max_cycles_override_of_none_is_accepted_and_means_inherit() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        let members = vec!["alpha".to_string(), "beta".to_string()];

        let setup = team_setup(
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            leader_role("alpha"),
            None,
        );
        let room = create_room_with_team_impl("Inherit Room", &members, Some(&setup))
            .expect("max_cycles None must be accepted on create");
        assert_eq!(
            room.max_cycles, None,
            "None must not be confused with 0 — it means inherit the app-wide default"
        );
    }

    #[test]
    fn creating_a_room_with_a_team_setup_persists_pattern_roles_and_max_cycles() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");
        let members = vec!["alpha".to_string(), "beta".to_string()];

        let setup = GroupRoomTeamSetup {
            pattern: Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            roles: leader_role("alpha"),
            max_cycles: Some(3),
            leader_prompt_override: Some("custom leader prompt".to_string()),
            worker_prompt_override: Some("custom worker prompt".to_string()),
        };
        let room = create_room_with_team_impl("Team Room", &members, Some(&setup))
            .expect("a valid team setup must be accepted on create");

        assert_eq!(
            room.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers)
        );
        assert_eq!(room.roles.get("alpha"), Some(&MemberRole::Leader));
        assert_eq!(room.max_cycles, Some(3));
        assert_eq!(
            room.leader_prompt_override.as_deref(),
            Some("custom leader prompt")
        );
        assert_eq!(
            room.worker_prompt_override.as_deref(),
            Some("custom worker prompt")
        );

        let reloaded = load_room_impl(&room.id).expect("load transcript copy");
        assert_eq!(reloaded.room.pattern, room.pattern);
        assert_eq!(reloaded.room.roles, room.roles);
        assert_eq!(reloaded.room.max_cycles, room.max_cycles);

        let index = list_rooms_impl().expect("list_rooms_impl should succeed");
        let summary = index
            .iter()
            .find(|r| r.id == room.id)
            .expect("the new room must be in the index copy");
        assert_eq!(summary.pattern, room.pattern, "index copy must agree too");
    }

    #[test]
    fn list_rooms_impl_projects_pattern_onto_the_room_summary() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let team_room = create_team_room_for_test(
            "Team Room",
            &["alpha".to_string(), "beta".to_string()],
            crate::protocol::TeamPattern::OrchestratorWorkers,
            leader_role("alpha"),
        )
        .expect("create_team_room_for_test should succeed");
        let peer_room = create_room_impl("Peer Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");

        let summaries = list_rooms_impl().expect("list_rooms_impl should succeed");
        let team_summary = summaries
            .iter()
            .find(|r| r.id == team_room.id)
            .expect("team room must be listed");
        let peer_summary = summaries
            .iter()
            .find(|r| r.id == peer_room.id)
            .expect("peer room must be listed");

        assert_eq!(
            team_summary.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers),
            "a team room's summary must carry pattern Some"
        );
        assert_eq!(
            peer_summary.pattern, None,
            "a peer room's summary must carry pattern None"
        );
    }

    #[test]
    fn a_room_index_record_with_pattern_and_no_leader_still_loads() {
        let _lock = crate::server::test_support::env_lock();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("alpha");
        scaffold_profile("beta");

        let room = create_room_impl("Ops Room", &["alpha".to_string(), "beta".to_string()])
            .expect("create_room_impl should succeed");

        // Hand-corrupt the persisted record the way a pre-demotion write or
        // a hand edit could: pattern set, roles empty — the exact shape
        // validate_team_room_shape refuses to WRITE. Reads must still
        // succeed; the invariant guards writes only.
        let mut transcript = load_transcript(&room.id)
            .expect("load_transcript should succeed")
            .expect("transcript must exist");
        transcript.room.pattern = Some(crate::protocol::TeamPattern::OrchestratorWorkers);
        write_transcript(&room.id, &transcript).expect("write corrupted transcript");

        let mut index = load_room_index().expect("load_room_index should succeed");
        index.insert(room.id.clone(), transcript.room.clone());
        write_room_index(&index).expect("write corrupted index");

        let loaded = load_room_impl(&room.id).expect("a violating record must still load, not error");
        assert_eq!(
            loaded.room.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers)
        );
        assert!(loaded.room.roles.is_empty());

        let summaries = list_rooms_impl().expect("a violating record must still list, not panic");
        let summary = summaries
            .iter()
            .find(|r| r.id == room.id)
            .expect("the violating room must still be listed");
        assert_eq!(
            summary.pattern,
            Some(crate::protocol::TeamPattern::OrchestratorWorkers)
        );
    }

    // -------------------------------------------------------------------
    // reset_room_conversation_impl (Phase 52 Plan 05, D-02/D-17)
    // -------------------------------------------------------------------

    #[test]
    fn a_reset_bumps_the_epoch_and_appends_the_marker_row_in_one_transcript_write() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");
        assert_eq!(room.conversation_epoch, 1);

        let reset = reset_room_conversation_impl(&room.id).expect("reset must succeed");
        assert_eq!(reset.conversation_epoch, 2, "epoch must bump by exactly one");

        let loaded = load_room_impl(&room.id).expect("load_room_impl should succeed");
        assert_eq!(
            loaded.room.conversation_epoch, 2,
            "one reload must show both the incremented epoch and the appended row"
        );
        let last = loaded.messages.last().expect("transcript must carry the marker row");
        assert_eq!(last.from, crate::protocol::GroupRoomSpeaker::System);
        assert_eq!(last.text, CONVERSATION_RESET_MARKER_TEXT);
    }

    #[test]
    fn a_reset_leaves_pattern_roles_and_membership_untouched() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let mut roles = BTreeMap::new();
        roles.insert("scout".to_string(), MemberRole::Leader);
        let room = create_team_room_for_test(
            "Ops Room",
            &["scout".to_string(), "zig".to_string()],
            crate::protocol::TeamPattern::OrchestratorWorkers,
            roles,
        )
        .expect("create_team_room_for_test should succeed");

        let before = load_room_impl(&room.id).expect("load before reset");
        let reset = reset_room_conversation_impl(&room.id).expect("reset must succeed");

        assert_eq!(reset.pattern, before.room.pattern, "pattern must survive untouched");
        assert_eq!(reset.roles, before.room.roles, "roles must survive untouched");
        assert_eq!(reset.max_cycles, before.room.max_cycles, "max_cycles must survive untouched");
        assert_eq!(reset.members, before.room.members, "members must survive untouched");
        assert_eq!(
            reset.leader_prompt_override, before.room.leader_prompt_override,
            "leader_prompt_override must survive untouched"
        );
        assert_eq!(
            reset.worker_prompt_override, before.room.worker_prompt_override,
            "worker_prompt_override must survive untouched"
        );
    }

    #[test]
    fn a_reset_is_refused_while_a_drive_is_in_flight_for_that_room() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        let drive_guard = crate::server::handoff_steering::try_acquire_room_drive(&room.id)
            .expect("room must be idle at the start");

        let err = reset_room_conversation_impl(&room.id).expect_err("must refuse while busy");
        assert_eq!(err, GroupChatError::ResetRefusedDriveInFlight);

        let unchanged = load_room_impl(&room.id).expect("load after refused reset");
        assert_eq!(
            unchanged.room.conversation_epoch, 1,
            "a refused reset must not mutate the epoch"
        );
        assert!(
            unchanged.messages.is_empty(),
            "a refused reset must not append a marker row"
        );

        drop(drive_guard);
        let reset = reset_room_conversation_impl(&room.id)
            .expect("reset must succeed once the drive guard is dropped");
        assert_eq!(reset.conversation_epoch, 2);
    }

    #[test]
    fn a_reset_discards_the_rooms_queued_pre_reset_steering_messages() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        crate::server::handoff_steering::enqueue_room_steering(&room.id, "also check the logs")
            .expect("queue first steering message");
        crate::server::handoff_steering::enqueue_room_steering(&room.id, "and ping ops")
            .expect("queue second steering message");
        assert_eq!(crate::server::handoff_steering::room_steering_depth(&room.id), 2);

        reset_room_conversation_impl(&room.id).expect("reset must succeed");

        assert_eq!(
            crate::server::handoff_steering::room_steering_depth(&room.id),
            0,
            "a reset must discard, not carry forward, the room's queued steering messages"
        );
        let loaded = load_room_impl(&room.id).expect("load after reset");
        assert!(
            !loaded
                .messages
                .iter()
                .any(|m| matches!(m.from, crate::protocol::GroupRoomSpeaker::Operator)),
            "no Operator row must be appended for the discarded steering messages"
        );
    }

    #[test]
    fn an_epoch_at_u32_max_is_refused_rather_than_saturating_into_a_false_clean_slate() {
        let _lock = crate::server::test_support::env_lock();
        crate::server::handoff_steering::reset_steering_for_test();
        let dir = tempfile::tempdir().expect("tempdir");
        let _guard = home(&dir);
        scaffold_profile("scout");
        scaffold_profile("zig");

        let room = create_room_impl("Ops Room", &["scout".to_string(), "zig".to_string()])
            .expect("create_room_impl should succeed");

        // Hand-set the room to the maximum epoch — the one state
        // reset_room_conversation_impl must refuse to advance past, same
        // "hand-corrupt the persisted record" technique this module's own
        // `a_room_index_record_with_pattern_and_no_leader_still_loads` uses.
        let mut transcript = load_transcript(&room.id)
            .expect("load_transcript should succeed")
            .expect("transcript must exist");
        transcript.room.conversation_epoch = u32::MAX;
        write_transcript(&room.id, &transcript).expect("write max-epoch transcript");
        let mut index = load_room_index().expect("load_room_index should succeed");
        index.insert(room.id.clone(), transcript.room.clone());
        write_room_index(&index).expect("write max-epoch index");

        let err = reset_room_conversation_impl(&room.id).expect_err("must refuse at u32::MAX");
        assert_eq!(err, GroupChatError::ConversationEpochExhausted);

        let unchanged = load_room_impl(&room.id).expect("load after refused reset");
        assert_eq!(
            unchanged.room.conversation_epoch,
            u32::MAX,
            "a saturating add would silently report success while reusing the same title \
             and child session — refusal must leave the epoch exactly where it was"
        );
        assert!(
            unchanged.messages.is_empty(),
            "no marker row must be appended when the reset is refused"
        );
    }
}
