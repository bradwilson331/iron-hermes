//! Shared protocol types for WebSocket chat.
//!
//! These types are used on BOTH client and server — they are simple
//! serializable data structures with no server-only dependencies.
//! Kept in a separate unconditional module so the WASM client can
//! compile them without pulling in the `server` feature.

use serde::{Deserialize, Serialize};

/// Phase 36.17.8 Plan 06 (D-13): inbound audio frame — client → server.
///
/// A standalone struct (NOT a `ChatStreamEvent` variant — it is client→server,
/// mirroring how `ChatRequest` is its own type). The browser mic capture path
/// serializes this to JSON and sends it as a WebSocket Binary frame.
///
/// `mime` carries `"audio/webm;codecs=opus"` (Chrome/Firefox) or
/// `"audio/mp4"` (Safari). `bytes` is the raw captured audio from
/// `MediaRecorder`. JSON wire shape (external tagging from standalone struct,
/// no special serde attributes needed):
///   `{"session_id":"...","mime":"audio/webm;codecs=opus","bytes":[…]}`
///
/// D-14: the browser sends raw audio bytes — it NEVER holds an API key.
/// STT is server-side only.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AudioInFrame {
    pub session_id: String,
    pub mime: String,
    pub bytes: Vec<u8>,
    /// Phase 36.17.9 (D-12, Wave D consumer): when true, the server STT result
    /// is checked against the configured wake phrase rather than submitted as a
    /// full turn. Defaults to false for backward compatibility — legacy clients
    /// that omit this field trigger the normal full-turn path.
    #[serde(default)]
    pub wake_word_check: bool,
    /// Phase 36.17.9 (D-12/D-13, Wave D): the client-configured wake phrase,
    /// carried on the frame so the server can match against it without reading
    /// server config on the hot path.
    ///
    /// This is the ONE chosen mechanism for transporting the phrase (D-13):
    /// the phrase is a client-side setting (voice_settings.rs); it travels
    /// on the AudioInFrame, NOT from server config. Defaults to `None` —
    /// backward-compatible; when None, the server falls back to
    /// `app_state.config.voice.wake_word.phrase`.
    #[serde(default)]
    pub wake_phrase: Option<String>,
    /// Phase 40.5 Plan 08 (D-17): active communication-path identity slug.
    ///
    /// The browser stamps this from `AvatarPrefs.active_identity` (Plan 01
    /// localStorage pointer) so the server can resolve the per-identity TTS
    /// provider/voice in `auto_speak_reply` (Plan 08 D-11). Distinct from the
    /// Voice-Settings edit target (`VoiceEditTargetCtx`).
    ///
    /// Defaults to `None` — backward-compatible with legacy clients that omit
    /// this field; the server falls back to the global TTS config when absent.
    #[serde(default)]
    pub active_identity: Option<String>,
}

/// Client → server message (user input).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ChatRequest {
    pub session_id: String,
    pub message: String,
    /// Phase 40.5 Plan 08 (D-17): active communication-path identity slug.
    ///
    /// Stamped from `AvatarPrefs.active_identity` at the time the message is
    /// sent, so the server can resolve the per-identity TTS provider/voice for
    /// the free-mode reply (D-11). Defaults to `None` — backward-compatible
    /// with legacy clients/shells that do not set this field.
    #[serde(default)]
    pub active_identity: Option<String>,
    /// Phase 46.7 Plan 04 (D-09): ids of `chat_attachments` rows (from
    /// `upload_attachment`) to resolve into this turn's user message.
    /// `#[serde(default)]` mirrors the `active_identity` precedent above —
    /// legacy clients/shells that omit this field still deserialize (the WS
    /// text protocol stays backward-compatible). An attachment-only message
    /// (empty `message`, non-empty `attachment_ids`) is a valid turn (D-07).
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

/// Server → client streaming events.
///
/// Phase 39.1 Plan 02 (R39.1-08): streaming content variants carry a `turn_id`
/// (`uuid::Uuid`) so the browser can demultiplex concurrent turn streams.
/// Global/session-level variants (QueueUpdated, SubagentEvent, VoiceStatus,
/// WakeWordResult, AudioOut, UserTranscript) are not turn-scoped and remain unchanged.
///
/// `turn_id` serializes as a hyphenated UUID string via the `uuid` crate's built-in
/// serde support (`uuid = { features = ["serde"] }` in workspace Cargo.toml).
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ChatStreamEvent {
    /// Streaming text delta from the agent.
    ///
    /// Phase 39.1 Plan 02 (R39.1-08): `turn_id` identifies which concurrent turn
    /// this delta belongs to. Defaults to `Uuid::nil()` at legacy call sites —
    /// Task 2 wires the real per-turn id at each spawn site.
    Delta {
        /// Phase 39.1 Plan 02: per-turn id for client-side demultiplexing.
        #[serde(default)]
        turn_id: uuid::Uuid,
        text: String,
    },
    /// Agent started a tool call.
    ///
    /// Phase 39.1 Plan 02 (R39.1-08): carries `turn_id` for demultiplexing.
    ToolCallStart {
        #[serde(default)]
        turn_id: uuid::Uuid,
        name: String,
        args: String,
    },
    /// Tool call completed.
    ///
    /// Phase 39.1 Plan 02 (R39.1-08): carries `turn_id` for demultiplexing.
    ToolCallEnd {
        #[serde(default)]
        turn_id: uuid::Uuid,
        name: String,
        success: bool,
    },
    /// Agent response finished.
    ///
    /// Phase 39.1 Plan 02 (R39.1-08): carries `turn_id` so the client can close
    /// the per-turn stream on receipt of Finished.
    Finished {
        #[serde(default)]
        turn_id: uuid::Uuid,
        total_tokens: u32,
    },
    /// Error during agent execution.
    ///
    /// Phase 39.1 Plan 02 (R39.1-08): carries `turn_id` for demultiplexing.
    Error {
        #[serde(default)]
        turn_id: uuid::Uuid,
        message: String,
    },
    /// Phase 39.1 Plan 02 (R39.1-01/R39.1-08): a new concurrent turn has started.
    ///
    /// Emitted BEFORE the first Delta for the turn. `index` is the 0-based ordinal
    /// of this turn among concurrently active turns in the session (informational only;
    /// the client uses `turn_id` as the durable key).
    ///
    /// JSON wire shape (external tagging):
    ///   {"TurnStarted":{"turn_id":"<uuid>","session_id":"<sid>","index":0}}
    TurnStarted {
        turn_id: uuid::Uuid,
        session_id: String,
        index: u32,
    },
    /// Phase 39.1 Plan 02 (R39.1-08): a turn completed successfully.
    ///
    /// Emitted AFTER the Finished event for the same turn_id. The client can use
    /// this to clean up per-turn UI state (progress indicators, turn slots, etc.).
    ///
    /// JSON wire shape (external tagging):
    ///   {"TurnEnded":{"turn_id":"<uuid>","session_id":"<sid>"}}
    TurnEnded {
        turn_id: uuid::Uuid,
        session_id: String,
    },
    /// Phase 39.1 Plan 02 (R39.1-05/R39.1-08): a turn was cancelled (via /stop or
    /// /agents cancel <turn_id>).
    ///
    /// Emitted when the CancellationToken is triggered for this turn.
    ///
    /// JSON wire shape (external tagging):
    ///   {"TurnCancelled":{"turn_id":"<uuid>"}}
    TurnCancelled { turn_id: uuid::Uuid },
    /// Phase 26.7.1 Plan 02 (D-07): payload-free subagent-registry-changed signal.
    ///
    /// JSON shape (external tagging — no #[serde(...)] attribute on the enum):
    ///   {"SubagentEvent":{}}
    ///
    /// Client increments `subagent_events: Signal<u64>` on receipt and lets
    /// `ScreenAgents`' use_effect call `agents_resource.restart()` — same code
    /// path as the periodic poll, no divergent diff logic.
    SubagentEvent {},
    /// Phase 36.17.4 (D-03): queue depth + paused state snapshot. JSON shape
    /// (external tagging): {"QueueUpdated":{"depth":3,"paused":false}}. Emitted
    /// on every push, pop, pause toggle, unpause, and queue clear. Client
    /// updates Signal<(u32, bool)> for the status-bar Queue: N pill.
    QueueUpdated { depth: u32, paused: bool },
    /// Phase 36.17.9 (D-14): voice availability snapshot pushed on WS connect
    /// and on change. Replaces the hardcoded `stt_available: true` at
    /// `chat.rs:462`. Client stores this in `VoiceStatusState` and exposes
    /// it via context so all voice components read server-authoritative state.
    ///
    /// JSON wire shape (external tagging):
    ///   {"VoiceStatus":{"stt_available":true,"stt_provider":"groq",
    ///                   "tts_available":true,"tts_provider":"edge",
    ///                   "ffmpeg_present":true}}
    ///
    /// Server-driven only (T-36.17.9-01-01): client NEVER asserts availability
    /// back to server. `stt_provider` / `tts_provider` are `None` when the
    /// respective service is unavailable.
    VoiceStatus {
        stt_available: bool,
        stt_provider: Option<String>,
        /// Plan 03: active STT model name (derived from provider + per-provider model).
        #[serde(default)]
        stt_model: Option<String>,
        tts_available: bool,
        tts_provider: Option<String>,
        ffmpeg_present: bool,
        /// Plan 03 (VOICE-02): VAD silence duration in seconds (config.voice.silence_duration).
        /// `None` on old server builds — client falls back to vad_params::SILENCE_POLLS.
        #[serde(default)]
        silence_duration_secs: Option<f64>,
        /// Plan 03 (VOICE-02): Web Audio RMS threshold (config.voice.web_silence_threshold_rms).
        /// Distinct from silence_threshold: i32 (native PCM domain) — never aliased.
        #[serde(default)]
        web_silence_threshold_rms: Option<f32>,
        /// Plan 03 (VOICE-02): Speech-confirm window in milliseconds (hardcoded 500ms per RESEARCH Q4).
        #[serde(default)]
        speech_confirm_ms: Option<u32>,
        /// Plan 03 (VOICE-02): Auto-TTS flag (config.voice.auto_tts).
        #[serde(default)]
        auto_tts: Option<bool>,
    },
    /// Phase 36.17.9 (D-12, Wave D): wake-word STT-polling result.
    ///
    /// Emitted by the server after transcribing a wake-word-check clip
    /// (AudioInFrame { wake_word_check: true }). The match is a case-insensitive
    /// contains check — never a regex (T-36.17.9-04-01 ReDoS mitigation).
    ///
    /// JSON wire shape (external tagging):
    ///   {"WakeWordResult":{"matched":true}}
    ///
    /// On `matched: true` the client transitions from Armed to Listening for a
    /// full turn. On `matched: false` it returns to the Armed idle state.
    WakeWordResult { matched: bool },
    /// Phase 36.17.7 D-02-a: synthesized audio delivery to the web client.
    /// JSON wire shape (external tagging):
    ///   {"AudioOut":{"mime":"audio/mpeg","uuid":"<uuidv4>","bytes":[<u8>...]}}
    /// Transmitted as Message::Binary per D-02-a. The `bytes` field serializes
    /// as a JSON array of u8 values (no `serde_bytes` dep — plain Vec<u8>).
    AudioOut {
        mime: String,
        uuid: String,
        bytes: Vec<u8>,
    },
    /// Phase 01-04 (DLV-03 web): generated-image delivery to the web client.
    ///
    /// Mirrors `AudioOut` (binary-frame transport, the proven AudioOut path):
    /// the server extracts a `<MEDIA: ...>` photo tag off the WS stream via
    /// `MediaTagExtractor`, `WebImageDispatcher` reads the cached image bytes,
    /// and emits this variant. Transmitted as `Message::Binary`; the client
    /// builds a Blob URL from `bytes` and renders an inline `<img>`.
    ///
    /// JSON wire shape (external tagging):
    ///   {"ImageOut":{"mime":"image/png","uuid":"<uuidv4>","bytes":[<u8>...]}}
    ///
    /// `bytes` serializes as a JSON array of u8 (no `serde_bytes` dep — plain
    /// Vec<u8>, same as AudioOut). Photo bytes are size-gated server-side
    /// before framing (20 MB parity with the Telegram photo cap, T-04-03).
    ImageOut {
        mime: String,
        uuid: String,
        bytes: Vec<u8>,
    },
    /// Phase 36.3.3 (D-08 web): generated video delivery to the web client.
    ///
    /// Mirrors `ImageOut` (binary-frame transport): the server extracts a `<MEDIA: ...>`
    /// video tag via `MediaTagExtractor`, `WebVideoDispatcher` reads the cached video bytes,
    /// and emits this variant. Transmitted as `Message::Binary`; the client builds a Blob
    /// URL and renders an inline `<video controls>`.
    ///
    /// JSON wire shape (external tagging):
    ///   {"VideoOut":{"mime":"video/mp4","uuid":"<uuidv4>","bytes":[<u8>...]}}
    ///
    /// `bytes` serializes as a JSON array of u8 (no `serde_bytes` dep — plain Vec<u8>,
    /// same as AudioOut / ImageOut). Size-gated server-side before framing (50MB cap, D-07).
    VideoOut {
        mime: String,
        uuid: String,
        bytes: Vec<u8>,
    },
    /// Phase 36.17.9: server-transcribed voice input echoed to the client so
    /// it renders as a user bubble. Voice turns are transcribed server-side
    /// and fed straight into `run_web_turn`, so — unlike a typed message,
    /// where the client creates the bubble before sending — the client never
    /// had the text. This event supplies it for DISPLAY ONLY; the client must
    /// NOT re-submit it (the server already ran the turn).
    ///
    /// JSON wire shape (external tagging):
    ///   {"UserTranscript":{"text":"what the user said"}}
    UserTranscript { text: String },

    /// Phase 41.1 Plan 03 (SKILL-13 web / D-06, UI-SPEC §C): a DIM run-turn
    /// meta chip announcing a one-shot skill run. Emitted by the WS SKILL-13
    /// fallback IMMEDIATELY BEFORE the run turn's first Delta, so it renders
    /// ABOVE the streaming assistant reply. `text` is the fully-formatted chip
    /// copy: `▶ Ran skill /{name}` (bare) or
    /// `▶ Ran skill /{name} · "{args≤40}…"` (argued). The client renders it as
    /// its own DIM metadata row with NO bubble background — visually distinct
    /// from both user and assistant bubbles (it is metadata, not a message).
    /// The bare-invoke synthetic trigger text itself is never sent as a bubble.
    ///
    /// JSON wire shape (external tagging):
    ///   {"RunTurnMeta":{"text":"▶ Ran skill /gsd-config"}}
    RunTurnMeta { text: String },
}

// =============================================================================
// Phase 36.3.7.11 Plan 01 — kanban dashboard wire types
// =============================================================================

/// Phase 36.3.7.11 (D-08): kanban dashboard WebSocket envelope.
///
/// External tagging:
/// - `{"TaskEventBatch":{"events":[...],"last_event_id":42}}`
/// - `{"Error":{"message":"..."}}`
/// - `{"Ping":{}}`
///
/// Server → client only. The dashboard tail consumer pushes one
/// `TaskEventBatch` per polling cycle that observed new events.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum KanbanWsEvent {
    /// Batch of new `task_events` rows since the last broadcast.
    /// `last_event_id` is the highest `id` in `events` — used by the
    /// client as a reconnect cursor (Plan 02 behavior).
    TaskEventBatch {
        events: Vec<KanbanEventRow>,
        last_event_id: i64,
    },
    /// Server-side error encountered while streaming (rendered as a toast).
    Error { message: String },
    /// Payload-free liveness ping (mirrors ChatStreamEvent::SubagentEvent {}).
    Ping {},
}

/// Phase 36.3.7.11 (D-08): one row from the `task_events` table on the wire.
///
/// Plain Serde struct — no server-only deps. Mirrors
/// `ironhermes_kanban::events::KanbanEvent` but lives in protocol.rs so the
/// WASM client can compile it without pulling `ironhermes-kanban` into the
/// client build (Pattern A in PATTERNS.md).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct KanbanEventRow {
    pub id: i64,
    pub task_id: String,
    pub kind: String,
    pub payload: Option<String>,
    pub created_at: f64,
}

/// Phase 36.3.7.11 (D-13 / Q6): UI-layer transport wrapper for the
/// Complete / Block structured input. Lives here (not in `ironhermes-kanban`)
/// because it is a UI-layer concept — the dashboard's `patch_task_status`
/// `#[server]` fn carries this in lieu of separate `complete` / `block` write
/// fns. Plan 02 wires this up; Plan 01 just defines the type so the wire
/// contract is stable.
#[allow(dead_code)] // Plan 02 consumes this via the write-side `#[server]` fns.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PromptPayload {
    Complete {
        summary: String,
        metadata: Option<serde_json::Value>,
    },
    Block {
        reason: String,
    },
}

/// Phase 36.3.7.11 (D-19): board task on the wire. Mirrors
/// `ironhermes_kanban::types::Task` for read-side rendering. Plain Serde —
/// no server-only deps so the WASM client can render it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub body: Option<String>,
    pub assignee: String,
    pub status: String,
    pub priority: i64,
    pub tenant: Option<String>,
    pub workspace: Option<String>,
    pub created_at: f64,
    pub started_at: Option<f64>,
    pub ended_at: Option<f64>,
    /// Phase 46.4 Plan 06 (D-10): read-only output path set at completion
    /// time (CLI `--output-path` flag or a worker call). `None` when the
    /// task hasn't set one — the card renders no output-path row in that
    /// case (UI-SPEC "Empty state" row: absence is the empty state).
    pub output_path: Option<String>,
}

/// Phase 36.3.7.11 (Q5): the canonical worker_context contract returned by
/// `fetch_task`. Field set mirrors `kanban_show.rs` lines 218-232 exactly so
/// the drawer renders the same shape the LLM `kanban_show` tool produces.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct WorkerContextEnvelope {
    pub task_id: String,
    pub title: String,
    pub body: Option<String>,
    pub status: String,
    pub assignee: String,
    pub tenant: Option<String>,
    pub workspace: String,
    pub priority: i64,
    pub parent_handoffs: Vec<serde_json::Value>,
    pub prior_attempts: Vec<serde_json::Value>,
    pub comments: Vec<serde_json::Value>,
}

/// Phase 36.3.7.11 (D-20): one `task_runs` row on the wire. Used by the
/// drawer Run History section.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TaskRunRow {
    pub run_id: String,
    pub outcome: Option<String>,
    pub started_at: f64,
    pub ended_at: Option<f64>,
    pub elapsed_ms: Option<i64>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub worker: Option<String>,
}

/// Phase 36.3.7.11 (D-20): one `task_comments` row on the wire. Used by the
/// drawer comment thread.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CommentRow {
    pub author: String,
    pub body: String,
    pub created_at: f64,
}

/// Phase 46.4 Plan 06 (D-03/D-04): one `task_attachments` row on the wire.
/// Mirrors `ironhermes_kanban::types::AttachmentMeta` field-for-field — plain
/// Serde, no server-only deps, so the WASM client can render attachment
/// chips (Pattern A in PATTERNS.md). `stored_path` is intentionally omitted
/// from the wire: it is an absolute server-filesystem path with no meaning
/// to the browser and no UI-SPEC surface renders it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AttachmentRow {
    pub id: String,
    pub task_id: String,
    pub filename: String,
    pub size_bytes: i64,
    pub content_type: Option<String>,
    pub uploaded_by: Option<String>,
    pub created_at: f64,
}

/// Phase 46.7 Plan 04 (D-09/D-10/D-11): one `chat_attachments` row on the
/// wire for the web-chat upload surface. A deliberate wire-copy of
/// `ironhermes_state::ChatAttachmentRow`, NOT a re-export — `ironhermes-state`
/// is a native-only dep (`[target.'cfg(not(target_arch = "wasm32"))']` in
/// Cargo.toml), so this crate's WASM client build cannot see it; `protocol.rs`
/// must stay compilable on both targets (mirrors the `AttachmentRow` pattern
/// above). `stored_rel_path` and `created_at` are intentionally omitted from
/// the wire (server-filesystem detail / no UI-SPEC surface renders them yet).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ChatAttachmentRow {
    pub id: String,
    pub session_id: String,
    pub filename: String,
    pub content_type: Option<String>,
    pub size_bytes: i64,
    pub message_id: Option<String>,
}

// =============================================================================
// Phase 36.3.7.11 Plan 02 — kanban write-side wire types
// =============================================================================

/// Phase 36.3.7.11 Plan 02 (D-09 wire): wire-copy of
/// `ironhermes_kanban::types::KanbanStatus`. Plain Serde enum so the WASM
/// client can match on a status without pulling `ironhermes-kanban` into the
/// client build (Pattern A in PATTERNS.md). Variant set MUST match the
/// canonical seven variants byte-for-byte by `as_str` casing — the
/// `rename_all = "lowercase"` attribute combined with the special-case
/// `InProgress → "running"` mapping (via `#[serde(rename)]`) matches
/// `KanbanStatus::as_str` exactly (`triage`, `todo`, `ready`, `running`,
/// `blocked`, `done`, `archived`).
///
/// The client-side `kanban::transitions` module uses this enum; the
/// server-side `kanban_api::patch_task_status` parses the inbound value
/// then forwards to `ironhermes_kanban::KanbanStatus` for the actual store
/// call. The drift-risk surface is the variant set + the wire string for
/// `InProgress`/Running; both are locked by inline tests below.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum KanbanStatus {
    Triage,
    Todo,
    Ready,
    /// Wire form is "running" (matches `ironhermes_kanban::KanbanStatus::Running`).
    #[serde(rename = "running")]
    InProgress,
    Blocked,
    Done,
    Archived,
}

impl KanbanStatus {
    /// Canonical lowercase wire string (matches
    /// `ironhermes_kanban::KanbanStatus::as_str`).
    pub fn as_wire_str(self) -> &'static str {
        match self {
            KanbanStatus::Triage => "triage",
            KanbanStatus::Todo => "todo",
            KanbanStatus::Ready => "ready",
            KanbanStatus::InProgress => "running",
            KanbanStatus::Blocked => "blocked",
            KanbanStatus::Done => "done",
            KanbanStatus::Archived => "archived",
        }
    }

    /// Parse from the canonical lowercase wire string. Returns `None` for
    /// unknown values — callers should reject with a `ServerFnError`.
    pub fn from_wire_str(s: &str) -> Option<Self> {
        Some(match s {
            "triage" => KanbanStatus::Triage,
            "todo" => KanbanStatus::Todo,
            "ready" => KanbanStatus::Ready,
            "running" => KanbanStatus::InProgress,
            "blocked" => KanbanStatus::Blocked,
            "done" => KanbanStatus::Done,
            "archived" => KanbanStatus::Archived,
            _ => return None,
        })
    }
}

/// Phase 36.3.7.11 Plan 02 (D-13): payload for `create_task` `#[server]` fn.
/// Carries the structured form input from the dashboard's Create Task modal
/// to the server-side `KanbanStore::create_task` call.
///
/// Round-trips through serde — see inline tests below.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct CreateTaskPayload {
    pub title: String,
    pub assignee: Option<String>,
    pub parents: Vec<String>,
    pub priority: i64,
    pub tenant: Option<String>,
    pub body: Option<String>,
    /// When `true`, the new task starts in TRIAGE; when `false`, it starts
    /// in TODO (or READY if parents.is_empty(), per the store's
    /// `create_task` D-06 rule).
    pub start_in_triage: bool,
}

/// Phase 36.3.7.11 Plan 02 (D-13): which decomposer kernel action to invoke.
/// External-tag wire shape (default Rust enum Serde): bare strings
/// `"Decompose"` / `"Specify"`.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecomposeOrSpecify {
    Decompose,
    Specify,
}

impl DecomposeOrSpecify {
    /// Lowercase action slug for CLI hint copy ("decompose" / "specify").
    pub fn slug(self) -> &'static str {
        match self {
            DecomposeOrSpecify::Decompose => "decompose",
            DecomposeOrSpecify::Specify => "specify",
        }
    }
}

/// Phase 36.3.7.11 Plan 02 (D-13 / Q9): result of `run_decompose_or_specify`
/// `#[server]` fn. Two branches:
///
/// - `Ok` — the kernel ran. Returns the number of children spawned (0 for
///   the specify path) and a short human-readable summary.
/// - `NotWired` — the dashboard's AppState does not satisfy the kernel's
///   aux-client requirement. The UI surfaces the message as a tooltip and
///   the user runs the CLI command directly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum DecomposeResult {
    Ok {
        children_count: u32,
        summary: String,
    },
    NotWired {
        message: String,
    },
}

// ============================================================================
// Phase 47.4 — kanban profile management DTOs
// ============================================================================
//
// Declared in full now (Plan 01) so later plans in this phase never
// re-declare a profile type. Plain Serde, no server-only deps — compiled
// unconditionally on both targets (mirrors `TaskRow` above).

/// Phase 47.4 (D-11): the D-11 health vocabulary. `Configured` means dir +
/// `config.yaml` + at least one resolvable provider key, all read from
/// disk with zero network I/O. There is no `Reachable` variant and none
/// may be added — the dot claims CONFIGURED, never reachability
/// (T-47.4-01-R1).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ProfileHealth {
    Configured,
    Incomplete,
}

/// Phase 47.4 (D-11): the specific reason a profile is `Incomplete`. A
/// profile can carry more than one at once (e.g. missing dir AND no
/// resolvable key).
///
/// Phase 47.4 Plan 11 (GAP-1): `NoKeyForProvider(provider)` is the honest,
/// provider-aware replacement question — "does this profile have a key for
/// ITS OWN configured provider", not "does it have ANY key" — derived from
/// the same `ironhermes_core::dispatch_gate` predicate the CLI's hard
/// pre-spawn gate uses. `NoResolvableKey` is retained for the case where the
/// profile's provider identity itself is unknown (an unparseable
/// `config.yaml`) — never delete it, its label is locked.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ProfileGap {
    MissingDir,
    MissingConfigYaml,
    NoResolvableKey,
    NoKeyForProvider(String),
}

impl ProfileGap {
    /// UI-SPEC State Matrix copy — locked, not paraphrased. Returns
    /// `Cow<'static, str>` (Phase 47.4 Plan 11, GAP-1) rather than
    /// `&'static str` because `NoKeyForProvider` interpolates a provider
    /// name at runtime — the three original variants still return
    /// `Cow::Borrowed` over the byte-identical locked strings.
    #[allow(dead_code)] // called from ProfileSwitcher's rsx!; dead_code fires on test target
    pub fn meta_label(&self) -> std::borrow::Cow<'static, str> {
        match self {
            ProfileGap::MissingDir => std::borrow::Cow::Borrowed("profile dir missing"),
            ProfileGap::MissingConfigYaml => std::borrow::Cow::Borrowed("missing config.yaml"),
            ProfileGap::NoResolvableKey => std::borrow::Cow::Borrowed("no resolvable key"),
            ProfileGap::NoKeyForProvider(provider) => {
                std::borrow::Cow::Owned(format!("no key for provider {provider}"))
            }
        }
    }
}

/// Phase 47.4 (D-08 / D-11): one row in the board-header PROFILE dropdown,
/// returned by `list_profiles`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProfileRow {
    pub name: String,
    pub health: ProfileHealth,
    pub gaps: Vec<ProfileGap>,
    pub provider: Option<String>,
    pub model_default: Option<String>,
    pub key_count: usize,
}

/// Phase 47.4 (D-07 / D-13): whether a profile's key came from the root
/// `.env` (inheritance), was entered directly in the UI, or is absent.
#[allow(dead_code)] // Plan 07/08 consume this via KeyRow / the wizard + drawer key tables.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum KeyStatus {
    Inherited,
    Missing,
    ManuallySet,
}

/// Phase 47.4 (D-13): one key row on the wire. `masked` holds only
/// `sk-••••••••••••••••••` or `—` — there is no `value` field and none
/// may be added; key material never crosses the HTTP boundary readable.
#[allow(dead_code)] // Plan 07/08 consume this via ProfileDetail.keys.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KeyRow {
    pub name: String,
    pub status: KeyStatus,
    pub masked: String,
}

/// Phase 47.4 (D-04): the profile detail drawer's read model. Landed by
/// this plan for contract stability; `fetch_profile_detail` (a later plan)
/// is its first caller.
#[allow(dead_code)] // Plan 08 consumes this via fetch_profile_detail.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProfileDetail {
    pub name: String,
    pub dir: String,
    pub health: ProfileHealth,
    pub gaps: Vec<ProfileGap>,
    pub provider: Option<String>,
    pub model_default: Option<String>,
    pub keys: Vec<KeyRow>,
    pub web_config_write_enabled: bool,
    /// Phase 49.4.1 (D-01/D-05): the profile's remembered secrets-source
    /// selection, in [`SecretSource::config_str`] vocabulary — `None` means
    /// no remembered source (never synced, or an unparsed/hand-edited
    /// value). Lets the drawer pre-select the picker on mount without a
    /// second load effect.
    pub secrets_source: Option<String>,
}

/// Phase 47.4 (D-07): the wizard's key-inheritance mode. `Explicit` carries
/// the `--keys` allowlist for the `custom` mode.
#[allow(dead_code)] // Plan 07 consumes this via CreateProfileRequest.key_mode.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum KeyMode {
    LlmOnly,
    AllKeys,
    Explicit(Vec<String>),
}

/// Phase 47.4 (D-08 / D-13 / T-47.4-01-I1): the wizard's create-profile
/// request. Deliberately does NOT derive `Debug` — `manual_keys` carries
/// plaintext key material that must never reach a log line via an
/// accidental `tracing::debug!(?request)`. See the hand-written `Debug`
/// impl below.
#[allow(dead_code)] // Plan 07 consumes this via create_profile.
#[derive(Serialize, Deserialize, Clone)]
pub struct CreateProfileRequest {
    pub name: String,
    pub key_mode: KeyMode,
    pub force: bool,
    pub manual_keys: Vec<(String, String)>,
    /// Phase 49.4.1 Plan 02 (D-04): which [`SecretSource`] the create path
    /// resolves keys/registry connection fields from. `#[serde(default)]`
    /// so a payload serialized before this field existed still deserializes
    /// to the pre-phase behaviour (D-04's costly-reversibility mitigation)
    /// rather than failing.
    #[serde(default = "default_create_secret_source")]
    pub secret_source: SecretSource,
}

/// [`CreateProfileRequest::secret_source`]'s serde default — the pre-phase
/// behaviour (root `.env` only).
fn default_create_secret_source() -> SecretSource {
    SecretSource::RootEnv
}

/// D-13: redacts `manual_keys` unconditionally — printing this value can
/// never leak key material, even under `{:#?}`.
impl std::fmt::Debug for CreateProfileRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateProfileRequest")
            .field("name", &self.name)
            .field("key_mode", &self.key_mode)
            .field("force", &self.force)
            .field("manual_keys", &"<redacted>")
            .field("secret_source", &self.secret_source)
            .finish()
    }
}

/// Phase 49.4.1 (D-04/D-07): the secrets-provenance axis — WHERE a
/// profile's provider connection fields and key material are sourced from
/// on create or re-sync. Orthogonal to [`KeyMode`] (which key NAMES);
/// neither replaces the other and a profile carries one of each (D-07). A
/// closed set of four, no fifth variant — wire contract locked by the
/// 49.4.1-01 Task 1 checkpoint (variant names, serde representation,
/// persisted `config_str` vocabulary, and operator-facing labels all
/// approved verbatim).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretSource {
    RootEnv,
    ContainerEnv,
    Vault,
    Provided,
}

impl SecretSource {
    /// Operator-facing label — UI-SPEC Copywriting Contract, locked.
    pub fn label(&self) -> &'static str {
        match self {
            SecretSource::RootEnv => "Root .env",
            SecretSource::ContainerEnv => "Container environment",
            SecretSource::Vault => "Vault",
            SecretSource::Provided => "Provided keys",
        }
    }

    /// Persisted `config.yaml` vocabulary — deliberately a SEPARATE
    /// snake_case string set from the serde wire representation above, so
    /// the on-disk file reads as configuration rather than as Rust. Pinned
    /// round-trip: [`SecretSource::from_config_str`].
    pub fn config_str(&self) -> &'static str {
        match self {
            SecretSource::RootEnv => "root_env",
            SecretSource::ContainerEnv => "container_env",
            SecretSource::Vault => "vault",
            SecretSource::Provided => "provided",
        }
    }

    /// Total over the four [`SecretSource::config_str`] values; `None` for
    /// anything else — an unrecognised or hand-edited value degrades to "no
    /// remembered source" rather than to a wrong one.
    pub fn from_config_str(s: &str) -> Option<SecretSource> {
        match s {
            "root_env" => Some(SecretSource::RootEnv),
            "container_env" => Some(SecretSource::ContainerEnv),
            "vault" => Some(SecretSource::Vault),
            "provided" => Some(SecretSource::Provided),
            _ => None,
        }
    }
}

/// Phase 49.4.1 Plan 02 (D-10): server-computed answer to "is the Vault
/// source selectable, and if not, why" — shipped in the SAME payload that
/// renders the `SECRETS SOURCE` section (UI-SPEC E1/E2: no separate
/// in-flight state for the row itself). Carries no key material, so a
/// derived `Debug` is correct here (unlike [`CreateProfileRequest`], which
/// can carry plaintext).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SecretsSourceAvailability {
    pub vault_available: bool,
    /// One of the two UI-SPEC "D-10 vault-disabled reason string(s)" when
    /// `vault_available` is `false`; `None` when it is `true`.
    pub vault_reason: Option<String>,
}

/// Phase 49.4.1 (D-01/D-05/D-06): re-sync an EXISTING profile's provider
/// registry and keys from a chosen [`SecretSource`]. Deliberately does NOT
/// derive `Debug` — `manual_keys` can carry plaintext key material (Plan
/// 03's provided-keys path). See the hand-written `Debug` impl below,
/// structurally identical to [`CreateProfileRequest`]'s.
#[derive(Serialize, Deserialize, Clone)]
pub struct SyncProfileSecretsRequest {
    pub name: String,
    pub source: SecretSource,
    pub key_mode: KeyMode,
    pub manual_keys: Vec<(String, String)>,
}

/// D-11: redacts `manual_keys` unconditionally — printing this value can
/// never leak key material, even under `{:#?}`.
impl std::fmt::Debug for SyncProfileSecretsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncProfileSecretsRequest")
            .field("name", &self.name)
            .field("source", &self.source)
            .field("key_mode", &self.key_mode)
            .field("manual_keys", &"<redacted>")
            .finish()
    }
}

/// Phase 49.4.1 (D-06): the sync's success payload — the profile's
/// refreshed key rows, the source that was used, and how many `providers:`
/// entries the registry mirror touched. A sync that fails D-06's loud-
/// failure check never produces this value; the caller sees `Err` instead.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SyncProfileSecretsResponse {
    pub keys: Vec<KeyRow>,
    pub source: SecretSource,
    pub providers_mirrored: usize,
}

/// Phase 47.4 (D-04): `Option`-per-field merge payload for the profile
/// detail drawer's provider/model save — mirrors `ProviderWritePayload`
/// (`provider_config_api.rs:59-78`).
///
/// Phase 50.1 Plan 05 (D-15): `skills_disabled` extends the same
/// option-per-field merge discipline — `None` leaves the target profile's
/// current opt-out list alone, `Some(vec![])` clears it (every skill on).
#[allow(dead_code)] // Plan 08 consumes this via update_profile_config.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProfileConfigWritePayload {
    pub name: String,
    pub provider: Option<String>,
    pub model_default: Option<String>,
    pub skills_disabled: Option<Vec<String>>,
}

/// Phase 50.1 Plan 05 (D-15): one row of a profile-scoped skills catalog —
/// the process-global skill registry's catalog joined against one target
/// profile's own `config.yaml` opt-out list. `enabled_for_profile` is the
/// inverse of catalog membership in that profile's `skills.disabled`
/// ("everything not named is on").
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProfileSkillRow {
    pub name: String,
    pub category: String,
    pub enabled_for_profile: bool,
}

/// Phase 50.1 Plan 05 (D-15/D-16): a bot's persona (SOUL.md) body, read
/// from / written to that bot's own workspace subdirectory — the
/// directory its CLI-handoff subprocess runs in, so the workspace
/// resolver (`ironhermes_core::workspace::resolve_from_cwd`) picks it up
/// at turn time. An empty `body` means "no persona file yet" (inherits
/// the default identity), never an error.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ProfilePersona {
    pub name: String,
    pub body: String,
}

/// Phase 49.4 Plan 08 (D-14): the two-level scope an activation record
/// covers — "chat only" (the web UI chat/voice surface alone) or
/// "everywhere" (chat plus editor defaults plus the gateway/worker
/// default). Plain-tag enum serde, matching `ProfileHealth`/`KeyStatus`
/// above.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationScope {
    ChatOnly,
    Everywhere,
}

/// Phase 49.4 Plan 08 (D-14): the persisted activation-with-scope record on
/// the wire — which profile is "active" and how far that activation
/// reaches. The absence of a record (`get_active_profile` returning `None`)
/// means every surface keeps resolving to the pre-existing
/// environment-derived active profile; this DTO exists only once an
/// operator has explicitly activated something.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ActiveProfileRecord {
    pub name: String,
    pub scope: ActivationScope,
    pub updated_at_ms: i64,
}

/// Phase 49.4 Plan 12 (D-16): a bot's identity for the profile-to-bot
/// binding. Checkpoint decision (resolved by the human operator,
/// `gate="blocking-human"`, before this plan's Task 2 ran — full text
/// recorded in `49.4-12-SUMMARY.md`'s Decisions section): a "bot" for this
/// binding is a gateway PLATFORM ADAPTER instance (Telegram, Discord,
/// Slack, Buzz, the webhook listener, or the REST API server listener) —
/// the fixed, closed key set already enumerated as `GatewayConfig.platforms`'s
/// map keys (`gateway_platform_status_api.rs::PLATFORM_KEYS`: `"telegram"`,
/// `"discord"`, `"slack"`, `"buzz"`, `"webhook"`, `"api_server"`). NOT a
/// profile (the roster's existing rows already ARE profiles — binding
/// "which profile" onto a profile row is circular) and NOT a kanban worker
/// (already bound via its `--profile <assignee>` spawn argument per the
/// operator's own second decision — no second store is built for that
/// case; see `server::bot_binding_api`'s module doc for the full record). A
/// plain string newtype: a platform adapter has a single natural key, never
/// a composite one.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BotKey(pub String);

/// Phase 49.4 Plan 12 (D-16): one persisted profile-to-bot binding record.
/// `profile_name` is always an explicit, real profile name — including the
/// literal `"default"` (the always-present profile), which both the roster
/// and Soul-page UIs use to represent "no meaningful override" rather than
/// an unlabelled/blank state (UI-SPEC E14 partial). `updated_at_ms` mirrors
/// `ActiveProfileRecord`'s own field for the same reason (an audit/debug
/// timestamp, never displayed to the operator).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotBinding {
    pub bot_key: BotKey,
    pub profile_name: String,
    pub updated_at_ms: i64,
}

/// Phase 50.1 Plan 01 (D-11): UI-owned bot display metadata. Persisted under
/// a UI-owned store keyed by profile name — never the profile `config.yaml`
/// (a malformed one silently degrades to defaults, D-11), never the agent
/// database (schema migration + lifecycle coupling, D-11).
///
/// Persistence shape (operator checkpoint decision, resolved at plan
/// execution time — recorded in full in `50.1-01-SUMMARY.md`): a hybrid of
/// the checkpoint's option-c and option-a rather than either pure option.
/// The CANONICAL copy of one bot's `BotMeta` lives at a per-profile sidecar
/// file (`~/.ironhermes/profiles/<name>/ui-meta.json`) — deleting the
/// profile directory removes it with no separate cleanup path needed. A
/// central JSON map (`~/.ironhermes/bot-meta.json`) caches every bot's
/// `BotMeta` for the roster's single whole-map read per paint (D-13 fences
/// the roster away from N-per-profile-directory fan-out); see
/// `server/bot_meta_api.rs`'s module doc for the full read/write protocol.
///
/// No secret-adjacent field lives here, so — unlike `CreateProfileRequest`
/// below — no hand-written redacting `Debug` impl is required.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct BotMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<BotAvatarDescriptor>,
    pub group: Option<String>,
    pub preview: Option<String>,
    pub preview_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

/// Phase 50.1 Plan 01 (D-11): `Option`-per-field merge payload for
/// `save_bot_meta` — mirrors `ProfileConfigWritePayload` below. `None` on a
/// field means "leave unchanged"; `Some(v)` sets that field to `v`. There is
/// no way to clear a field back to `None` once set (matches
/// `ProfileConfigWritePayload`'s own precedent) — a future plan can add an
/// explicit clear verb if that need ever surfaces.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotMetaPatch {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub avatar: Option<BotAvatarDescriptor>,
    pub group: Option<String>,
    pub preview: Option<String>,
    pub preview_at_ms: Option<i64>,
}

/// Phase 50.1 Plan 01: a bot's avatar descriptor. `image_id` references a
/// separately stored asset (plan 50.1-04) — inline data-URL image bytes are
/// rejected by `save_bot_meta` (T-50.1-01-05) so this metadata payload can
/// never grow unbounded with binary blobs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotAvatarDescriptor {
    pub kind: String,
    pub shape: Option<String>,
    pub color: Option<String>,
    pub image_id: Option<String>,
}

/// Phase 50.1 Plan 04 (D-11/D-12): avatar upload request — the arg-bearing
/// POST body for `bot_avatar_api::upload_bot_avatar`. `content_b64` is the
/// raw image bytes, standard base64 (mirrors `chat_attachments_api`'s
/// `upload_attachment` transport shape).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotAvatarUploadRequest {
    pub bot_name: String,
    pub content_b64: String,
}

/// Phase 50.1 Plan 04 (D-12): AI-portrait generation request — the
/// arg-bearing POST body for `bot_avatar_api::generate_bot_avatar`.
/// `prompt` is the operator's free-text description; the positive step
/// count and safety flag are resolved server-side from config, never
/// client-supplied (see `bot_avatar_api::resolve_generation_args`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotAvatarGenerateRequest {
    pub bot_name: String,
    pub prompt: String,
}

/// Phase 50.1 Plan 04 (D-11): the result of a successful upload or
/// generation — an opaque identifier only. Image bytes never travel inside
/// the bot-meta metadata payload OR this response; the bytes are served
/// separately from `serve_bot_avatar`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotAvatarSaved {
    pub image_id: String,
}

/// Phase 50.1 Plan 01 (D-09): one roster row — a join of the existing
/// `ProfileRow` enumeration with this plan's optional `BotMeta` and a
/// server-derived `is_live` flag (D-04: the LIVE badge is never
/// client-computed).
///
/// `is_kanban_worker` (OF-4, gap task) is joined in from
/// [`BotMetaListResponse::kanban_worker_names`] — a SEPARATE set, not a
/// field on `BotMeta` — because most kanban worker profiles carry no
/// `BotMeta` sidecar at all (OF-4: absence of bot-meta is NOT the badge
/// signal, only board-assignee membership is).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotRosterEntry {
    pub row: ProfileRow,
    pub meta: Option<BotMeta>,
    pub is_live: bool,
    pub is_kanban_worker: bool,
}

/// Phase 50.1 OF-4 (gap task): `list_bot_meta`'s response, extended with
/// the kanban-worker badge join. `kanban_worker_names` is a SEPARATE set
/// from `meta` — folding the flag onto `BotMeta` itself would silently drop
/// the badge for every kanban worker profile that has no `BotMeta` sidecar
/// (the common case per OF-4's operator report). Computed server-side with
/// ONE `fetch_board` read per roster load (D-13 fence: never a per-profile
/// fan-out — see `server/bot_meta_api.rs`'s `list_bot_meta`); a board-read
/// failure degrades to an empty set so the roster still renders, badges
/// simply absent.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct BotMetaListResponse {
    pub meta: std::collections::BTreeMap<String, BotMeta>,
    pub kanban_worker_names: std::collections::BTreeSet<String>,
}

/// Phase 50.1 Plan 06 (D-17): the `duplicate_profile` request — an explicit
/// source and target, never merged into `CreateProfileRequest`. The two
/// contracts are materially different (scaffold-from-root-config-and-
/// resolve-inherited-keys vs. copy-a-specific-allowlist-of-the-source's-own-
/// artifacts-and-deliberately-omit-one) and folding them into one payload
/// would produce a conditional-field footgun where the `.env` omission is
/// invisible at the call site.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DuplicateProfileRequest {
    pub source: String,
    pub target: String,
}

/// Phase 50.1 Plan 06 (UI-SPEC Component Inventory §3): the create wizard's
/// clone-from control state. `Import` is present in the control per the
/// Copywriting Contract but has no backing operation in this phase — the
/// wizard renders it disabled with an inline note rather than silently
/// accepting a selection that does nothing.
///
/// Under `--all-features`, `legacy-shell` swaps the reachable app root to
/// `WarpHermes`, leaving this enum's only construction site
/// (`profile_shared/create_dialog.rs`'s bot-context wizard) unreferenced
/// from `main()` for dead-code-lint purposes — even though it is live in
/// the default (non-legacy-shell) build. Same pre-existing pattern as
/// `KeyMode` a few lines above.
#[allow(dead_code)]
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CloneFromChoice {
    #[default]
    Empty,
    CloneExisting,
    Import,
}

/// Phase 47.4 (D-09): the real judge-probe outcome. `Success`/`Failure`/
/// `Timeout` are earned claims — a configuration-presence check alone
/// cannot report `Success` here (that would be exactly the
/// false-confidence failure D-09 exists to prevent).
#[allow(dead_code)] // Plan 09 consumes this via VerifyReport::outcome.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum VerifyOutcome {
    Success,
    Failure { summary: String },
    Timeout { seconds: u64 },
}

/// Phase 47.4 (D-09): the wizard VERIFY step / drawer VERIFY action report.
/// The static disk-only checks (`dir_ok`/`config_ok`/`env_ok`) are
/// synchronous; `outcome` alone carries the real network judge-probe
/// result.
#[allow(dead_code)] // Plan 09 consumes this via verify_profile.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VerifyReport {
    pub dir_ok: bool,
    pub config_ok: bool,
    pub model_default: Option<String>,
    pub env_ok: bool,
    pub key_count: usize,
    pub first_key: Option<String>,
    pub outcome: VerifyOutcome,
}

/// Phase 50.1 Plan 03 (D-05/D-06/D-20): the CLI-handoff dispatch request — a
/// non-live bot's name and a single message to send it via an isolated
/// `ironhermes` subprocess (`server/cli_handoff.rs`). Carries no key
/// material, so no hand-written redacting `Debug` impl is required (unlike
/// `CreateProfileRequest` above, which redacts `manual_keys`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotHandoffRequest {
    pub name: String,
    pub message: String,
}

/// Phase 50.1 Plan 03: the CLI-handoff dispatch result. `dispatch_bot_message`
/// (this plan) always returns failure as `Err(ServerFnError)` — every
/// `BotHandoffError` maps to one, per its module doc — so `error` is never
/// populated by that call site; it exists on this DTO because D-20 commits
/// `run_bot_handoff`'s spawn-wait-return shape to Phase 50.2's group-chat
/// round driver, which needs to represent one member's failed turn as DATA
/// (not an early `Err` that aborts the whole round) without a second result
/// type. Neither field carries key material.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotHandoffResult {
    pub reply: String,
    pub exit_ok: bool,
    pub error: Option<String>,
    pub elapsed_ms: u64,
}

/// Phase 50.2 Plan 16 (G-50.2-2c): `dispatch_bot_message`'s return shape —
/// either the bot had no turn in flight and ran now (`Replied`, carrying the
/// same [`BotHandoffResult`] shape as before this plan), or the bot was busy
/// and the message was accepted and QUEUED rather than racing a second
/// concurrent subprocess. `Queued` means the message will be folded into
/// that bot's NEXT handoff prompt, whichever surface starts it next; `depth`
/// is how many messages are now waiting for that bot (this one included).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum BotDispatchOutcome {
    Replied(BotHandoffResult),
    Queued { depth: u32 },
}

// ---------------------------------------------------------------------
// Phase 50.2 Plan 07 (D-20): the @mention-handoff middleware's DTO set.
// Placed adjacent to the `BotHandoff*` block above, following its own
// documentation style. Consumed by `server/mention_handoff_api.rs` and the
// shared `bot_roster/mention_handoff.rs` component's three call sites
// (Bot Chat window, room transcript, live Chat screen).
// ---------------------------------------------------------------------

/// Phase 50.2 Plan 07 (D-20): one `@mention` handoff dispatch — the target
/// bot's name and the delegated message text (the mentioning message,
/// unmodified — the mention is detected, never consumed or edited).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MentionHandoffRequest {
    pub target: String,
    pub message: String,
}

/// Phase 50.2 Plan 07 (D-20): the successful result of one `@mention`
/// handoff dispatch. `dispatch_mention_handoff` always returns failure as
/// `Err(ServerFnError)` — this DTO carries only the success shape.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MentionHandoffResult {
    pub target: String,
    pub reply: String,
    pub elapsed_ms: u64,
}

/// Phase 50.2 Plan 07 (D-20, UI-SPEC State Matrix): the client-side render
/// state one `@mention` handoff block carries. `Failed.reason` carries only
/// a `BotHandoffError` `Display` string — never raw child output, never an
/// env value, never a raw `.env` line (the CR-05/CR-06 discipline
/// `server/cli_handoff.rs`'s module doc records).
// Phase 50.2 Plan 07 Task 1: `MentionHandoffState`'s first production
// caller lands in Task 2/3 (the shared `MentionHandoffBlock` component and
// its three call sites); this intra-plan staging `#[allow(dead_code)]`
// follows the crate's own precedent (`group_chat_api.rs`'s Plan 04 pure
// fns carried the same allow before Task 3 wired their driver).
#[allow(dead_code)]
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MentionHandoffState {
    Pending,
    Resolved,
    Failed { reason: String },
}

// ---------------------------------------------------------------------
// Phase 50.2 Plan 01 (D-20/D-21/D-04/D-05/D-06/D-03): the group-chat social
// layer's DTO set. Placed adjacent to `BotMeta`/`BotHandoffResult` above and
// following the same documentation style; consumed by
// `server/group_chat_store.rs`, `server/group_chat_api.rs`, and the
// `bot_roster/{create_room_modal,group_row,group_chat_workspace}.rs`
// components. Introduced by this plan's tracer task — later 50.2 plans
// consume these without editing this file.
// ---------------------------------------------------------------------

/// Phase 50.2 Plan 01: one group-chat room's persisted record — the
/// canonical value stored at `<hermes_home>/group-rooms.json` (index) and
/// mirrored per-room at `<hermes_home>/group-rooms/<id>.json` (see
/// `server/group_chat_store.rs`'s module doc for the full persistence
/// shape). `id` is a slug derived from `name` at creation time
/// (`slugify_room_name`) and never changes after that — renaming a room is
/// out of scope for this plan.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupRoom {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    /// Roster-section label this room is filed under — unused until plan
    /// 06's roster-sectioning work; carried here now so the DTO shape never
    /// needs to change when that plan lands.
    pub group: Option<String>,
    pub needs_you: bool,
    pub preview: Option<String>,
    pub preview_at_ms: Option<i64>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Phase 50.2 Plan 01: the roster-row projection of a [`GroupRoom`] —
/// exactly the fields `group_row.rs` needs to paint one `.kn-room-row`,
/// without shipping the full transcript over the wire on every roster
/// paint.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupRoomSummary {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    /// Phase 50.2 Plan 03 (D-11, reachability fix): passed through from
    /// `GroupRoom.group` — `list_rooms_impl` (`group_chat_store.rs`)
    /// previously dropped this field when projecting the summary, which
    /// would have made a room's group assignment permanently invisible to
    /// the roster's sectioning join no matter what the store held. Mirrors
    /// `BotMeta.group`'s shape exactly.
    pub group: Option<String>,
    pub needs_you: bool,
    pub preview: Option<String>,
    pub preview_at_ms: Option<i64>,
    /// `Some(round)` while a round is actively dispatching for this room;
    /// `None` when idle. This plan's tracer never sets it to `Some` in the
    /// summary response — round-in-flight roster-row reflection is plan
    /// 04's job — but the field is shipped now so that plan is additive.
    pub active_round: Option<u32>,
}

/// Phase 50.2 Plan 01: who produced a [`GroupRoomMessage`]. `System` is
/// reserved for a future round-divider/settled marker persisted as a real
/// transcript row (unused by this plan's tracer, which renders those
/// client-side from `round`/`GroupRoundOutcome` instead).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GroupRoomSpeaker {
    Operator,
    Member(String),
    System,
}

/// Phase 50.2 Plan 01: the outcome of one member's turn within a round.
/// `Failed.reason` carries only a [`BotHandoffError`]'s `Display` string —
/// never raw child stdout, never an env value, never a raw `.env` line (the
/// CR-05/CR-06 discipline `server/cli_handoff.rs`'s module doc records).
///
/// [`BotHandoffError`]: crate::server::cli_handoff::BotHandoffError
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MemberTurnStatus {
    Replied,
    Passed,
    Failed { reason: String },
}

/// Phase 50.2 Plan 01: one row in a room's persisted transcript.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupRoomMessage {
    pub from: GroupRoomSpeaker,
    pub text: String,
    pub at_ms: i64,
    pub round: u32,
    pub status: MemberTurnStatus,
}

/// Phase 50.2 Plan 01: a room's full persisted transcript — the per-room
/// sibling file's on-disk shape
/// (`<hermes_home>/group-rooms/<id>.json`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupRoomTranscript {
    pub room: GroupRoom,
    pub messages: Vec<GroupRoomMessage>,
}

/// Phase 50.2 Plan 01: one member's failed turn within a completed round —
/// the summary shape [`GroupRoundOutcome::failures`] carries back to the
/// caller (the full detail also lands in the transcript as a
/// [`GroupRoomMessage`] with `status: MemberTurnStatus::Failed`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupMemberFailure {
    pub member: String,
    pub reason: String,
}

/// Phase 50.2 Plan 01: the result of one `dispatch_group_round` call. This
/// plan's tracer always returns `rounds_run: 1`, `settled: false`,
/// `needs_you: false` — the real multi-round settle/needs-you computation
/// is plan 03's job.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupRoundOutcome {
    pub rounds_run: u32,
    pub messages_appended: u32,
    pub settled: bool,
    pub needs_you: bool,
    pub failures: Vec<GroupMemberFailure>,
}

/// Phase 50.2 Plan 18 (G-50.2-2c): `dispatch_group_round`'s return shape —
/// either the room had no drive in flight and ran now (`Ran`, carrying the
/// same [`GroupRoundOutcome`] shape as before this plan), or a drive was
/// already running for this room and the operator's message was accepted
/// and QUEUED rather than racing a second concurrent drive on the same
/// room. `Queued` means the message will be injected as an operator
/// transcript row at the next round boundary of the drive already running;
/// `depth` is how many messages are now waiting for that room (this one
/// included). The room-level twin of [`BotDispatchOutcome`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GroupRoundDispatch {
    Ran(GroupRoundOutcome),
    Queued { depth: u32 },
}

/// Phase 50.2 Plan 01 (D-21): the app-wide group-chat orchestration
/// tunables. `Default` returns the D-21 constants quoted verbatim from
/// upstream `plugin.js:3295-3298` — this plan consumes only
/// `Default::default()`; plan 05 gives this struct real persistence and a
/// settings UI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupChatSettings {
    pub max_rounds: u32,
    pub max_messages: u32,
    pub history_limit: u32,
    pub min_members: u32,
    pub max_members: u32,
}

impl Default for GroupChatSettings {
    fn default() -> Self {
        Self {
            max_rounds: 3,
            max_messages: 10,
            history_limit: 24,
            min_members: 2,
            max_members: 6,
        }
    }
}

/// Phase 50.2 Plan 01: the `create_group_room` request payload.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CreateGroupRoomRequest {
    pub name: String,
    pub members: Vec<String>,
}

/// Phase 50.2 Plan 15 (G-50.2-2a): the `update_group_room_members` request
/// payload. `room_id` is the room's creation-time slug
/// (`GroupRoom::id`/`slugify_room_name`) and is never itself changed by this
/// request — this DTO changes membership only, never a room's name or id
/// (`protocol.rs:1035-1041` records `id` as frozen after creation).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UpdateGroupRoomMembersRequest {
    pub room_id: String,
    pub members: Vec<String>,
}

/// Phase 50.2 Plan 01: the `dispatch_group_round` request payload.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DispatchGroupRoundRequest {
    pub room_id: String,
    pub message: String,
}

// ---------------------------------------------------------------------
// Phase 50.2 Plan 08 (D-13/D-23/D-11/D-04/D-21): the persisted, operator-
// side Bot Chat thread — the OPERATOR half of "every bot has a persistent
// Bot Chat." Plan 02 gave the BOT half (`--session "Bot Chat"` resume +
// history seeding over the CLI-handoff transport); this DTO set is what
// `server/bot_chat_store.rs` persists and `bot_roster/chat_window.rs`
// renders. `BotChatEntryKind` mirrors `chat_window.rs`'s `ChatWindowRole`
// (`Operator`/`Bot`/`Error`) one variant each, so the persisted shape and
// the client render shape can never drift apart.
// ---------------------------------------------------------------------

/// Phase 50.2 Plan 08: which side authored a persisted Bot Chat row —
/// mirrors `chat_window.rs`'s `ChatWindowRole` exactly.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum BotChatEntryKind {
    Operator,
    Bot,
    Error,
}

/// Phase 50.2 Plan 08: one persisted row in a bot's Bot Chat thread.
/// `at_ms` is always assigned server-side (`server/bot_chat_store.rs`'s
/// `append_bot_chat_entries_impl`), never trusted from the client, so a
/// thread's row order is never dependent on the browser's clock.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotChatEntry {
    pub kind: BotChatEntryKind,
    pub text: String,
    pub at_ms: i64,
}

/// Phase 50.2 Plan 08: one bot's full persisted Bot Chat thread — the
/// on-disk shape at `<hermes_home>/profiles/<bot>/bot-chat.json` (see
/// `server/bot_chat_store.rs`'s module doc for the full persistence-shape
/// record). Bounded to `BOT_CHAT_THREAD_MAX_ENTRIES` (200) most-recent
/// entries on every append — the OPERATOR'S VIEW only. The bot's own
/// recall is bounded separately by `ironhermes-cli`'s
/// `BOT_SESSION_HISTORY_LIMIT` (24, D-21's `GROUP_CHAT_HISTORY_LIMIT`) —
/// the two bounds are deliberately different because they answer different
/// questions: how much the operator can scroll back, versus how much
/// context each turn carries.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BotChatThread {
    pub bot: String,
    pub entries: Vec<BotChatEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 46.7 Plan 04 (D-09): a legacy `ChatRequest` JSON payload that
    /// omits `attachment_ids` entirely (pre-Plan-04 client/shell) must still
    /// deserialize — `#[serde(default)]` backward-compatibility lock.
    #[test]
    fn chat_request_deserializes_without_attachment_ids_field() {
        let legacy_json = r#"{"session_id":"s1","message":"hi"}"#;
        let parsed: ChatRequest =
            serde_json::from_str(legacy_json).expect("legacy ChatRequest must deserialize");
        assert_eq!(parsed.session_id, "s1");
        assert_eq!(parsed.message, "hi");
        assert!(
            parsed.attachment_ids.is_empty(),
            "attachment_ids must default to an empty Vec when omitted"
        );
        assert_eq!(parsed.active_identity, None);
    }

    /// Phase 46.7 Plan 04 (D-09/D-07): a ChatRequest carrying attachment_ids
    /// with an empty message (attachment-only turn) must round-trip.
    #[test]
    fn chat_request_with_attachment_ids_and_empty_message_round_trips() {
        let json = r#"{"session_id":"s2","message":"","attachment_ids":["att-1","att-2"]}"#;
        let parsed: ChatRequest =
            serde_json::from_str(json).expect("attachment-only ChatRequest must deserialize");
        assert_eq!(parsed.message, "");
        assert_eq!(
            parsed.attachment_ids,
            vec!["att-1".to_string(), "att-2".to_string()]
        );
    }

    /// Phase 26.7.1 Plan 02 (Wave 0): D-07 serde shape verification.
    /// SubagentEvent {} must serialize to {"SubagentEvent":{}} (external tagging).
    #[test]
    fn test_subagent_event_json_shape() {
        let ev = ChatStreamEvent::SubagentEvent {};
        let json = serde_json::to_string(&ev).expect("serialize SubagentEvent");
        assert_eq!(json, r#"{"SubagentEvent":{}}"#);

        // Round-trip: deserialize back into the variant.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(parsed, ChatStreamEvent::SubagentEvent {}),
            "round-trip must reconstruct SubagentEvent variant"
        );
    }

    // =========================================================================
    // Phase 36.17.9 Plan 01 — Wave A protocol tests (D-14 / D-12)
    // =========================================================================

    /// Phase 36.17.9 (D-12, Wave D): WakeWordResult must use external tagging and round-trip.
    ///
    /// Wire shape: {"WakeWordResult":{"matched":true}}
    ///
    /// T-36.17.9-04-01 ReDoS mitigation: match is contains(), never regex.
    #[test]
    fn test_wake_word_result_json_shape() {
        let ev_matched = ChatStreamEvent::WakeWordResult { matched: true };
        let json = serde_json::to_string(&ev_matched).expect("serialize WakeWordResult matched");
        assert!(
            json.starts_with(r#"{"WakeWordResult":"#),
            "D-12: WakeWordResult must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""matched":true"#),
            "D-12: WakeWordResult matched=true must serialize (got {json})"
        );
        // Round-trip matched=true.
        let parsed: ChatStreamEvent =
            serde_json::from_str(&json).expect("deserialize WakeWordResult");
        assert!(
            matches!(parsed, ChatStreamEvent::WakeWordResult { matched: true }),
            "D-12: WakeWordResult {{ matched: true }} must round-trip via serde_json"
        );

        // matched=false case.
        let ev_no = ChatStreamEvent::WakeWordResult { matched: false };
        let json_no = serde_json::to_string(&ev_no).expect("serialize WakeWordResult no-match");
        assert!(
            json_no.contains(r#""matched":false"#),
            "D-12: WakeWordResult matched=false must serialize (got {json_no})"
        );
        let parsed_no: ChatStreamEvent =
            serde_json::from_str(&json_no).expect("deserialize no-match");
        assert!(
            matches!(
                parsed_no,
                ChatStreamEvent::WakeWordResult { matched: false }
            ),
            "D-12: WakeWordResult {{ matched: false }} must round-trip"
        );
    }

    /// Phase 36.17.9 (D-14): VoiceStatus must use external tagging and round-trip.
    ///
    /// Wire shape: {"VoiceStatus":{"stt_available":true,"stt_provider":"groq",
    ///              "tts_available":true,"tts_provider":"edge","ffmpeg_present":true}}
    #[test]
    fn test_voice_status_json_shape() {
        let ev = ChatStreamEvent::VoiceStatus {
            stt_available: true,
            stt_provider: Some("groq".to_string()),
            stt_model: None,
            tts_available: true,
            tts_provider: Some("edge".to_string()),
            ffmpeg_present: true,
            silence_duration_secs: None,
            web_silence_threshold_rms: None,
            speech_confirm_ms: None,
            auto_tts: None,
        };
        let json = serde_json::to_string(&ev).expect("serialize VoiceStatus");
        assert!(
            json.starts_with(r#"{"VoiceStatus":"#),
            "D-14: VoiceStatus must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""stt_available":true"#),
            "D-14: stt_available must serialize (got {json})"
        );
        // Round-trip: deserialize back into the variant.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(parsed, ChatStreamEvent::VoiceStatus { .. }),
            "D-14: VoiceStatus must round-trip via serde_json"
        );
    }

    /// Phase 36.17.9 (D-12): AudioInFrame deserialized without wake_word_check
    /// must default that field to false (back-compat).
    #[test]
    fn test_audio_in_frame_wake_word_check_defaults_false() {
        // Simulate a legacy frame that has no wake_word_check field.
        let json = r#"{"session_id":"abc","mime":"audio/webm;codecs=opus","bytes":[1,2,3]}"#;
        let frame: super::AudioInFrame = serde_json::from_str(json)
            .expect("must parse legacy AudioInFrame lacking wake_word_check");
        assert!(
            !frame.wake_word_check,
            "D-12: wake_word_check must default to false for back-compat"
        );
    }

    /// Phase 36.17.8 Plan 06 (D-13): AudioInFrame serde round-trip.
    ///
    /// Plain struct (not a ChatStreamEvent variant) — three fields survive a
    /// serde_json round-trip: session_id, mime, bytes. bytes is a plain Vec<u8>
    /// (no serde_bytes), matching AudioOut.
    #[test]
    fn test_audio_in_frame_round_trip() {
        let frame = super::AudioInFrame {
            session_id: "test-session-abc".to_string(),
            mime: "audio/webm;codecs=opus".to_string(),
            bytes: vec![0x1A, 0x45, 0xDF, 0xA3],
            wake_word_check: false,
            wake_phrase: None,
            // Phase 40.5 Plan 08 (D-17): test site uses None (no identity context in unit tests).
            active_identity: None,
        };

        let json = serde_json::to_string(&frame).expect("AudioInFrame must serialize");

        // All three fields must appear in the JSON string.
        assert!(
            json.contains(r#""session_id":"test-session-abc""#),
            "D-13: session_id must serialize (got {json})"
        );
        assert!(
            json.contains(r#""mime":"audio/webm;codecs=opus""#),
            "D-13: mime must serialize (got {json})"
        );
        assert!(
            json.contains("\"bytes\""),
            "D-13: bytes field must serialize (got {json})"
        );

        // Round-trip: all three fields preserved.
        let parsed: super::AudioInFrame =
            serde_json::from_str(&json).expect("AudioInFrame must deserialize");
        assert_eq!(
            parsed.session_id, frame.session_id,
            "D-13: session_id round-trip"
        );
        assert_eq!(parsed.mime, frame.mime, "D-13: mime round-trip");
        assert_eq!(parsed.bytes, frame.bytes, "D-13: bytes round-trip");
    }

    /// Phase 36.17.7 D-02-a: AudioOut wire-format lock.
    /// External-tagged struct variant must serialize to
    /// {"AudioOut":{"mime":"audio/mpeg","uuid":"test-uuid","bytes":[255,251]}}.
    /// Round-trip preserved via serde_json.
    #[test]
    fn test_audio_out_json_shape() {
        let ev = ChatStreamEvent::AudioOut {
            mime: "audio/mpeg".to_string(),
            uuid: "test-uuid".to_string(),
            bytes: vec![0xFF, 0xFB],
        };
        let json = serde_json::to_string(&ev).expect("serialize AudioOut");
        assert!(
            json.starts_with(r#"{"AudioOut":"#),
            "D-02-a: AudioOut must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""mime":"audio/mpeg""#),
            "D-02-a: AudioOut must serialize mime field (got {json})"
        );
        assert!(
            json.contains(r#""uuid":"test-uuid""#),
            "D-02-a: AudioOut must serialize uuid field (got {json})"
        );
        // Round-trip.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize AudioOut");
        assert!(
            matches!(parsed, ChatStreamEvent::AudioOut { .. }),
            "D-02-a: AudioOut must round-trip via serde_json"
        );
    }

    /// Phase 36.3.3 (D-08 web): VideoOut wire-format lock.
    /// External-tagged struct variant must serialize to
    /// {"VideoOut":{"mime":"video/mp4","uuid":"u","bytes":[1,2,3]}}.
    /// Round-trip preserved via serde_json.
    #[test]
    fn test_video_out_json_shape() {
        let ev = ChatStreamEvent::VideoOut {
            mime: "video/mp4".to_string(),
            uuid: "u".to_string(),
            bytes: vec![1, 2, 3],
        };
        let json = serde_json::to_string(&ev).expect("serialize VideoOut");
        assert!(
            json.starts_with(r#"{"VideoOut":"#),
            "D-08: VideoOut must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""mime":"video/mp4""#),
            "D-08: VideoOut must serialize mime field (got {json})"
        );
        assert!(
            json.contains(r#""uuid":"u""#),
            "D-08: VideoOut must serialize uuid field (got {json})"
        );
        assert!(
            json.contains(r#""bytes":[1,2,3]"#),
            "D-08: VideoOut must serialize bytes field (got {json})"
        );
        // Round-trip.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize VideoOut");
        assert!(
            matches!(parsed, ChatStreamEvent::VideoOut { .. }),
            "D-08: VideoOut must round-trip via serde_json"
        );
    }

    /// Phase 36.17.4 Plan 02 (D-11): QueueUpdated wire-format lock.
    /// External-tagged struct variant must serialize to
    /// {"QueueUpdated":{"depth":3,"paused":false}}.
    /// Both paused=false and paused=true cases asserted; round-trip preserved.
    #[test]
    fn test_queue_updated_json_shape() {
        // depth=3, paused=false: literal wire-format lock.
        let ev = ChatStreamEvent::QueueUpdated {
            depth: 3,
            paused: false,
        };
        let json = serde_json::to_string(&ev).expect("serialize QueueUpdated");
        assert_eq!(
            json, r#"{"QueueUpdated":{"depth":3,"paused":false}}"#,
            "D-11: QueueUpdated must serialize to external-tagged struct shape"
        );

        // Round-trip: deserialize back into the variant.
        let parsed: ChatStreamEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(
                parsed,
                ChatStreamEvent::QueueUpdated {
                    depth: 3,
                    paused: false,
                }
            ),
            "round-trip must reconstruct QueueUpdated {{ depth: 3, paused: false }}"
        );

        // paused=true variant: separate shape lock.
        let ev_paused = ChatStreamEvent::QueueUpdated {
            depth: 7,
            paused: true,
        };
        let json_paused = serde_json::to_string(&ev_paused).expect("serialize paused QueueUpdated");
        assert_eq!(
            json_paused, r#"{"QueueUpdated":{"depth":7,"paused":true}}"#,
            "D-11: paused=true variant must serialize correctly"
        );
    }

    // =========================================================================
    // Phase 36.3.7.11 Plan 01 (D-08 / D-13 / Q6) — Kanban wire-format locks
    // =========================================================================

    /// Phase 36.3.7.11 (D-08): TaskEventBatch must serialize as
    /// `{"TaskEventBatch":{"events":[...],"last_event_id":N}}`.
    /// Round-trip preserves field shape.
    #[test]
    fn test_kanban_ws_event_task_event_batch_json_shape() {
        let ev = KanbanWsEvent::TaskEventBatch {
            events: vec![],
            last_event_id: 42,
        };
        let json = serde_json::to_string(&ev).expect("serialize TaskEventBatch");
        assert!(
            json.starts_with(r#"{"TaskEventBatch":"#),
            "D-08: TaskEventBatch must use external tagging (got {json})"
        );
        assert!(
            json.contains(r#""last_event_id":42"#),
            "D-08: TaskEventBatch must serialize last_event_id (got {json})"
        );
        // Round-trip.
        let parsed: KanbanWsEvent = serde_json::from_str(&json).expect("deserialize");
        assert!(
            matches!(
                parsed,
                KanbanWsEvent::TaskEventBatch {
                    last_event_id: 42,
                    ..
                }
            ),
            "round-trip must reconstruct TaskEventBatch variant"
        );
    }

    /// Phase 36.3.7.11 (D-08): Ping is the payload-free liveness variant.
    /// External-tag struct-variant serialization: `{"Ping":{}}`.
    #[test]
    fn test_kanban_ws_event_ping_json_shape() {
        let ev = KanbanWsEvent::Ping {};
        let json = serde_json::to_string(&ev).expect("serialize Ping");
        assert_eq!(
            json, r#"{"Ping":{}}"#,
            "D-08: Ping must serialize to {{\"Ping\":{{}}}}"
        );
    }

    /// Phase 36.3.7.11 (D-13 / Q6): PromptPayload::Complete round-trip.
    #[test]
    fn test_prompt_payload_complete_round_trip() {
        let ev = PromptPayload::Complete {
            summary: "shipped".to_string(),
            metadata: Some(serde_json::json!({ "pr": 123 })),
        };
        let json = serde_json::to_string(&ev).expect("serialize Complete");
        let parsed: PromptPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ev, "D-13: Complete must round-trip");
    }

    /// Phase 36.3.7.11 (D-13 / Q6): PromptPayload::Block round-trip.
    #[test]
    fn test_prompt_payload_block_round_trip() {
        let ev = PromptPayload::Block {
            reason: "waiting on dependency".to_string(),
        };
        let json = serde_json::to_string(&ev).expect("serialize Block");
        let parsed: PromptPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, ev, "D-13: Block must round-trip");
    }

    // =========================================================================
    // Phase 36.3.7.11 Plan 02 (D-09 wire / D-13) — kanban write-side types
    // =========================================================================

    /// D-09 wire: every `KanbanStatus` variant serializes to the canonical
    /// lowercase string (matches `ironhermes_kanban::KanbanStatus::as_str`).
    /// InProgress maps to "running".
    #[test]
    fn test_kanban_status_serializes_lowercase() {
        let cases = [
            (KanbanStatus::Triage, "\"triage\""),
            (KanbanStatus::Todo, "\"todo\""),
            (KanbanStatus::Ready, "\"ready\""),
            (KanbanStatus::InProgress, "\"running\""),
            (KanbanStatus::Blocked, "\"blocked\""),
            (KanbanStatus::Done, "\"done\""),
            (KanbanStatus::Archived, "\"archived\""),
        ];
        for (variant, expected) in cases {
            let json = serde_json::to_string(&variant).expect("serialize KanbanStatus");
            assert_eq!(
                json, expected,
                "D-09 wire: {:?} must serialize as {}",
                variant, expected
            );
            let parsed: KanbanStatus = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, variant, "D-09 wire: round-trip preserves variant");
        }
    }

    /// D-09 wire: as_wire_str helper agrees with serde shape.
    #[test]
    fn test_kanban_status_as_wire_str_matches_serde() {
        for v in [
            KanbanStatus::Triage,
            KanbanStatus::Todo,
            KanbanStatus::Ready,
            KanbanStatus::InProgress,
            KanbanStatus::Blocked,
            KanbanStatus::Done,
            KanbanStatus::Archived,
        ] {
            let serde_str = serde_json::to_string(&v).unwrap();
            assert_eq!(
                serde_str,
                format!("\"{}\"", v.as_wire_str()),
                "as_wire_str must match serde for {:?}",
                v
            );
        }
    }

    /// D-09 wire: from_wire_str inverse-of as_wire_str.
    #[test]
    fn test_kanban_status_from_wire_str_round_trip() {
        for v in [
            KanbanStatus::Triage,
            KanbanStatus::Todo,
            KanbanStatus::Ready,
            KanbanStatus::InProgress,
            KanbanStatus::Blocked,
            KanbanStatus::Done,
            KanbanStatus::Archived,
        ] {
            let parsed = KanbanStatus::from_wire_str(v.as_wire_str()).expect("parse");
            assert_eq!(parsed, v);
        }
        assert!(KanbanStatus::from_wire_str("nonsense").is_none());
    }

    /// D-13: `CreateTaskPayload` round-trips through serde.
    #[test]
    fn test_create_task_payload_round_trip() {
        let payload = CreateTaskPayload {
            title: "wire up dashboard".to_string(),
            assignee: Some("frontend-dev".to_string()),
            parents: vec!["t_a".to_string(), "t_b".to_string()],
            priority: 2,
            tenant: Some("dashboard".to_string()),
            body: Some("acceptance: drag works".to_string()),
            start_in_triage: true,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let parsed: CreateTaskPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, payload, "D-13: CreateTaskPayload must round-trip");
    }

    /// D-13: `DecomposeOrSpecify` round-trips + slug helper.
    #[test]
    fn test_decompose_or_specify_round_trip() {
        for v in [DecomposeOrSpecify::Decompose, DecomposeOrSpecify::Specify] {
            let json = serde_json::to_string(&v).expect("serialize");
            let parsed: DecomposeOrSpecify = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, v, "D-13: DecomposeOrSpecify must round-trip");
        }
        // External-tag bare-string shape.
        assert_eq!(
            serde_json::to_string(&DecomposeOrSpecify::Decompose).unwrap(),
            r#""Decompose""#
        );
        assert_eq!(DecomposeOrSpecify::Decompose.slug(), "decompose");
        assert_eq!(DecomposeOrSpecify::Specify.slug(), "specify");
    }

    /// D-13 / Q9: `DecomposeResult` round-trips for both branches.
    #[test]
    fn test_decompose_result_round_trip() {
        let ok = DecomposeResult::Ok {
            children_count: 3,
            summary: "decomposed into 3 children".to_string(),
        };
        let json = serde_json::to_string(&ok).expect("serialize Ok");
        let parsed: DecomposeResult = serde_json::from_str(&json).expect("deserialize Ok");
        assert_eq!(parsed, ok, "D-13: DecomposeResult::Ok must round-trip");

        let nw = DecomposeResult::NotWired {
            message: "Use: hermes kanban decompose t_abc".to_string(),
        };
        let json = serde_json::to_string(&nw).expect("serialize NotWired");
        let parsed: DecomposeResult = serde_json::from_str(&json).expect("deserialize NotWired");
        assert_eq!(
            parsed, nw,
            "D-13: DecomposeResult::NotWired must round-trip"
        );
    }

    /// Phase 50.2 Plan 01: `GroupChatSettings::default()` must return exactly
    /// the D-21 constants quoted verbatim from upstream `plugin.js:3295-3298`.
    #[test]
    fn group_chat_settings_default_matches_d21_constants() {
        let defaults = GroupChatSettings::default();
        assert_eq!(defaults.max_rounds, 3);
        assert_eq!(defaults.max_messages, 10);
        assert_eq!(defaults.history_limit, 24);
        assert_eq!(defaults.min_members, 2);
        assert_eq!(defaults.max_members, 6);
    }
}
