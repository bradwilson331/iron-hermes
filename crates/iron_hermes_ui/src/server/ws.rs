//! WebSocket endpoint for streaming agent chat responses.

use dioxus::prelude::*;
#[cfg(feature = "server")]
use dioxus_fullstack::{body::Bytes, CloseCode, Message, TypedWebsocket};
use dioxus_fullstack::{WebSocketOptions, Websocket};
#[cfg(feature = "server")]
use std::time::Duration;
#[cfg(feature = "server")]
use tokio::sync::mpsc;
#[cfg(feature = "server")]
use tokio::task::JoinHandle;
#[cfg(feature = "server")]
use tracing::{info, warn};

pub use crate::protocol::{ChatRequest, ChatStreamEvent};

/// Phase 54.9 Plan 10 (D-09): true only when the request is an OpenUI turn AND asked for the
/// opt-in Thesys hosted Gateway. A bare `openui_gateway` never reaches the Gateway.
#[cfg(feature = "server")]
pub(crate) const OPENUI_GATEWAY_DISABLED: &str =
    "The Thesys Gateway is disabled. Set ui.openui_gateway_enabled: true in config.yaml to use it.";

#[cfg(feature = "server")]
pub(crate) fn openui_gateway_requested(req: &ChatRequest) -> bool {
    req.openui && req.openui_gateway
}

/// Phase 54.9 Plan 09 (D-09, T-54.9-24): the one-shot skill overlay an OpenUI turn carries.
/// `Some(("openui", body))` only when the client set `openui: true`; the body is the server's
/// compiled-in prompt plus the host restrictions, never client text.
#[cfg(feature = "server")]
pub(crate) fn openui_overlay_for(req: &ChatRequest) -> Option<(String, String)> {
    req.openui.then(|| {
        (
            "openui".to_string(),
            super::openui_route::openui_overlay_body(),
        )
    })
}

/// CR-02 / RR-04: an OpenUI turn that is NOT the operator's own typed prompt (a generated
/// button's action; the text is model-controlled) must never reach the slash-command router.
#[cfg(feature = "server")]
pub(crate) fn openui_action_slash_blocked(req: &ChatRequest) -> bool {
    req.openui && !req.openui_prompt && req.message.trim_start().starts_with('/')
}

/// RR-02: the web queue is a shared `String` FIFO, so an OpenUI turn that has to wait carries
/// this marker in front of its text. The drain strips it and applies the overlay after the
/// inbound gate, exactly like a live turn (WR-01 ordering). Control chars are not typeable into
/// the chat input; a hand-crafted `/queue` argument that includes the marker can only add the
/// same one-shot OpenUI overlay to its own session's turn.
#[cfg(feature = "server")]
const OPENUI_QUEUE_TAG: &str = "\u{1}ih-openui\u{1}";

/// The text to push onto the web queue for `message`.
#[cfg(feature = "server")]
pub(crate) fn tag_queued_message(message: &str, openui: bool) -> String {
    if openui {
        format!("{OPENUI_QUEUE_TAG}{message}")
    } else {
        message.to_string()
    }
}

/// Inverse of [`tag_queued_message`]: the text to run and whether to apply the OpenUI overlay.
#[cfg(feature = "server")]
pub(crate) fn split_queued_message(raw: String) -> (String, bool) {
    match raw.strip_prefix(OPENUI_QUEUE_TAG) {
        Some(rest) => (rest.to_string(), true),
        None => (raw, false),
    }
}

// Phase 36.1 D-04/D-05/D-07: slash interception imports.
// Phase 39.1: is_bypass removed (slash commands never rejected mid-turn per D-06).
#[cfg(feature = "server")]
use ironhermes_core::commands::{CommandResult, ResolveResult};

// Phase 39.1 Plan 02 (R39.1-01/R39.1-09): concurrent turn tracking.
// StreamMap drains N in-flight channels without extra spawns.
// TurnId, TurnEntry, Surface from ironhermes-core concurrency module.
#[cfg(feature = "server")]
use ironhermes_core::{Surface, TurnEntry, TurnId};
#[cfg(feature = "server")]
use std::collections::HashMap;
#[cfg(feature = "server")]
use tokio_stream::wrappers::UnboundedReceiverStream;
#[cfg(feature = "server")]
use tokio_stream::{StreamExt as _, StreamMap};

// Phase 36.17.9 (D-14): STT/TTS availability probes for VoiceStatus snapshot.
#[cfg(feature = "server")]
use ironhermes_tools::stt::select_stt_provider;
#[cfg(feature = "server")]
use ironhermes_tools::tts::ffmpeg_available;

/// Phase 26.7.1 Plan 02 (D-06 / Path A): RAII guard that clears the per-turn
/// callback slot on drop. Ensures the slot is reset to None even if
/// `run_web_turn` panics — the tokio task's drop machinery runs Drop before
/// the JoinHandle's error propagates.
#[cfg(feature = "server")]
struct SubagentCallbackSlotGuard {
    slot: std::sync::Arc<
        tokio::sync::Mutex<
            Option<tokio::sync::mpsc::UnboundedSender<crate::protocol::ChatStreamEvent>>,
        >,
    >,
}

#[cfg(feature = "server")]
impl Drop for SubagentCallbackSlotGuard {
    fn drop(&mut self) {
        // Best-effort clear. Use try_lock since Drop cannot await.
        // The slot is held only across very short windows; contention is
        // not expected outside of pathological teardown cases.
        if let Ok(mut guard) = self.slot.try_lock() {
            *guard = None;
        }
        // If try_lock fails (extremely unlikely — only the callback's
        // try_lock contends, and it doesn't hold the lock across .send),
        // we leak a stale Some(tx) until the next turn overwrites it. The
        // closed channel makes any further send a silent no-op. Acceptable.
    }
}

/// Server-side application-level WebSocket keepalive interval.
///
/// Application-level Ping frames keep intermediate proxy idle timers
/// reset and detect half-broken sockets promptly. Browsers automatically
/// respond to Ping with Pong at the WebSocket protocol level, so the
/// client requires no changes. Pong frames are skipped in the recv_raw
/// match arm.
///
/// 5 seconds is well below the ~9s idle-close threshold observed with
/// the dx serve proxy and matches the low end of common reverse-proxy
/// keepalive intervals.
#[cfg(feature = "server")]
const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Best-effort WebSocket close-frame emit before dropping the socket.
///
/// Ensures every teardown branch completes the WebSocket close handshake
/// so upstream proxies do not observe a raw transport reset.
/// Errors are intentionally swallowed — if the send fails the transport
/// is already broken and we must not block teardown.
#[cfg(feature = "server")]
async fn send_close_frame(
    socket: &mut TypedWebsocket<String, String>,
    code: CloseCode,
    reason: &str,
) {
    let _ = socket
        .send_raw(Message::Close {
            code,
            reason: reason.to_string(),
        })
        .await;
}

/// Phase 36.17.9 (D-14): build a VoiceStatus snapshot from the server's live config.
///
/// Probes STT availability via `select_stt_provider` (returns `Some(name)` when a
/// configured provider has credentials; `None` when unavailable — RESEARCH Pitfall 7).
/// TTS availability is derived from `config.tts.provider != "none"`. ffmpeg presence
/// is checked once at call time via `ffmpeg_available()`.
///
/// This function is server-only (`#[cfg(feature = "server")]`) — it must not be
/// compiled into the WASM client build (Pattern F in PATTERNS.md).
#[cfg(feature = "server")]
fn build_voice_status(
    app_state: &crate::server::state::AppState,
) -> crate::protocol::ChatStreamEvent {
    // Plan 03 (VOICE-02): re-read config from disk on each WS connect so web-written
    // VAD changes (silence_duration, web_silence_threshold_rms) take effect without a
    // process restart. Cost: one YAML parse per connect — acceptable for a local app.
    // Precedent: toggle_skill (api.rs:412) uses Config::load() fresh.
    // T-36.17.10-03-02 mitigation: fall back to startup snapshot on parse error.
    voice_status_from(&fresh_voice_config(&app_state.config()))
}

/// Pure builder behind [`build_voice_status`]: the VoiceStatus snapshot for `cfg`.
#[cfg(feature = "server")]
fn voice_status_from(cfg: &ironhermes_core::config::Config) -> crate::protocol::ChatStreamEvent {
    let stt_provider = select_stt_provider(&cfg.stt);
    // Pitfall 15: "selected" is not "usable". Available only when the selected
    // provider is registered AND reports itself available (an openai_compatible
    // STT with an empty base_url is not). Availability needs no key, so no vault
    // access is needed here.
    let stt_available = stt_provider.as_deref().is_some_and(|name| {
        ironhermes_tools::stt::build_stt_registry_with_vault(&cfg.stt, None)
            .get(name)
            .is_some_and(|p| p.is_available())
    });
    let tts_available = cfg.tts.provider != "none";
    let tts_provider = if tts_available {
        Some(cfg.tts.provider.clone())
    } else {
        None
    };

    // Derive active STT model from provider + per-provider config.
    let stt_model: Option<String> = match cfg.stt.provider.as_str() {
        "groq" => Some(cfg.stt.groq.model.clone()),
        "openai" => Some(cfg.stt.openai.model.clone()),
        "openai_compatible" => Some(cfg.stt.openai_compatible.model.clone()),
        // "auto": select_stt_provider picks the available provider; derive model from it.
        _ => stt_provider.as_deref().map(|p| match p {
            "groq" => cfg.stt.groq.model.clone(),
            "openai" => cfg.stt.openai.model.clone(),
            "openai_compatible" => cfg.stt.openai_compatible.model.clone(),
            _ => String::new(),
        }),
    };

    crate::protocol::ChatStreamEvent::VoiceStatus {
        stt_available,
        stt_provider,
        stt_model,
        tts_available,
        tts_provider,
        ffmpeg_present: ffmpeg_available(),
        // Plan 03 VAD fields — populated from freshly-read config.
        silence_duration_secs: Some(cfg.voice.silence_duration),
        web_silence_threshold_rms: Some(cfg.voice.web_silence_threshold_rms),
        speech_confirm_ms: Some(500u32), // hardcoded per RESEARCH Open Q4
        auto_tts: Some(cfg.voice.auto_tts),
    }
}

/// Phase 54.7 (D-18): the voice config a turn should run on.
///
/// Re-reads `config.yaml` so a web save applies to the next turn without a
/// restart, and falls back to the boot `snapshot` only when the file fails to
/// load (T-54.7-43). Same precedent as [`build_voice_status`]. Callers read it
/// ONCE at the start of a turn and hold the value, so an in-flight turn
/// finishes on the settings it started with. Deliberately does NOT call
/// `AppState::apply_config_now` (it rebuilds the main LLM client and would
/// publish unrelated unsaved edits).
#[cfg(feature = "server")]
pub(crate) fn fresh_voice_config(
    snapshot: &ironhermes_core::config::Config,
) -> ironhermes_core::config::Config {
    ironhermes_core::config::Config::load().unwrap_or_else(|_| snapshot.clone())
}

/// IN-05: [`fresh_voice_config`] for async callers. `Config::load()` is blocking
/// disk + YAML work, and the per-clip paths run it on the async executor (once per
/// clip, including every wake-word probe), so it is moved to the blocking pool. A
/// failed join falls back to the boot snapshot, like a failed load.
#[cfg(feature = "server")]
pub(crate) async fn fresh_voice_config_async(
    snapshot: &ironhermes_core::config::Config,
) -> ironhermes_core::config::Config {
    let owned = snapshot.clone();
    tokio::task::spawn_blocking(move || fresh_voice_config(&owned))
        .await
        .unwrap_or_else(|_| snapshot.clone())
}

/// Phase 54.7 (D-18/D-19): transcribe one full-turn clip with `cfg`'s STT
/// settings and apply the hallucination filter.
///
/// `None` means "send nothing": no provider, a provider failure (logged with
/// the category only, T-54.7-45) or a transcript the filter rejected.
#[cfg(feature = "server")]
pub(crate) async fn transcribe_turn_audio(
    cfg: &ironhermes_core::config::Config,
    bytes: &[u8],
    mime: &str,
    cache_dir: &std::path::Path,
) -> Option<String> {
    use ironhermes_tools::hallucination_filter::HallucinationFilter;

    let vault = ironhermes_core::voice_http::voice_vault_config(cfg);
    let raw = match ironhermes_tools::stt::clip::transcribe_clip(
        bytes,
        mime,
        &cfg.stt,
        vault.as_ref(),
        cache_dir,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => {
            // ClipError's Display carries the category / provider name only.
            warn!(reason = %e, "audio-in: STT transcription failed; dropping");
            return None;
        }
    };

    // D-12: HallucinationFilter.
    match HallucinationFilter::filter(&raw) {
        Some(t) => Some(t.to_string()),
        None => {
            info!("audio-in: transcript rejected by hallucination filter; no message sent");
            None
        }
    }
}

/// Phase 54.7 (D-18/D-19): transcribe one short wake-word clip with `cfg`'s STT
/// settings. Returns the raw transcript (the phrase match is the gate, so no
/// hallucination filter); `None` on any failure, which the caller reports as
/// "no match". Logs the category / provider name only (T-54.7-45).
#[cfg(feature = "server")]
pub(crate) async fn wake_word_transcript(
    cfg: &ironhermes_core::config::Config,
    bytes: &[u8],
    mime: &str,
    cache_dir: &std::path::Path,
) -> Option<String> {
    let vault = ironhermes_core::voice_http::voice_vault_config(cfg);
    match ironhermes_tools::stt::clip::transcribe_clip(
        bytes,
        mime,
        &cfg.stt,
        vault.as_ref(),
        cache_dir,
    )
    .await
    {
        Ok(t) => Some(t),
        Err(e) => {
            warn!(reason = %e, "wake-word-check: STT failed; emitting no-match");
            None
        }
    }
}

/// Phase 36.17.9 (D-12, Wave D): ReDoS-safe wake-word match predicate.
///
/// Returns `true` when the lowercased transcript contains the lowercased phrase.
/// Returns `false` for an empty or whitespace-only phrase — an unset phrase MUST
/// NOT trivially arm every clip (T-36.17.9-04-03 empty-phrase guard).
///
/// NEVER uses a regex (T-36.17.9-04-01: to_lowercase().contains() only — no
/// Regex::new / regex:: usage). Phrase length is enforced by the caller via the
/// 64-char truncation guard before this predicate is invoked.
#[cfg(feature = "server")]
fn wake_word_matches(transcript: &str, phrase: &str) -> bool {
    // T-36.17.9-04-03: empty or whitespace-only phrase → no match (never trivially true).
    if phrase.trim().is_empty() {
        return false;
    }
    // T-36.17.9-04-01 ReDoS mitigation: to_lowercase().contains() — NOT a regex.
    transcript.to_lowercase().contains(&phrase.to_lowercase())
}

/// Phase 41.3 Plan 04 (D-11/D-12): collect Web's nine core `CommandContext`
/// handles from real values on `AppState`/`AgentRuntime`. This is the D-12
/// enumeration — each handle is sourced explicitly rather than approximated —
/// and is the direct fix for `/agents` returning the "Subagent registry not
/// wired." fallback (`handlers.rs:219`): Web previously wired only 2 of the 9
/// (`state_store`, `skill_registry`); this brings it to 9-of-9.
#[cfg(feature = "server")]
fn web_core_handles(
    app_state: &crate::server::state::AppState,
    session_id: &str,
) -> ironhermes_core::commands::context::CoreContextHandles {
    use ironhermes_core::commands::context::{
        McpReloader, ProcessRegistrySnapshotHandle, StateStoreHandle, SubagentListSnapshot,
        ToolsetSessionHandle,
    };

    let state_store: std::sync::Arc<dyn StateStoreHandle> = std::sync::Arc::new(
        ironhermes_state::StateStoreHandleAdapter(app_state.state_store.clone()),
    );

    // Phase 32.3 Plan 04 style wiring, ported to Web: AppState has held the
    // subagent registry since state.rs:99 — it was simply never handed to
    // CommandContext. This is the fix for the /agents symptom.
    let subagent_registry: std::sync::Arc<dyn SubagentListSnapshot> = std::sync::Arc::new(
        ironhermes_agent::subagent_registry::SubagentRegistryHandle::new(
            app_state.subagent_registry.clone(),
        ),
    );

    // Phase 41.3 Plan 04 correction: `AgentRuntime` has no public
    // `process_registry` accessor (the handle is consumed into the tool
    // registry's terminal/execute_code instances at construction and not
    // retained) — `AppState.process_registry` (state.rs, added this plan)
    // holds the same Arc threaded into `AgentRuntimeInput` at init, mirroring
    // why `subagent_registry` is likewise held on `AppState` directly.
    let process_registry: std::sync::Arc<dyn ProcessRegistrySnapshotHandle> = std::sync::Arc::new(
        ironhermes_exec::process_registry::ProcessRegistryHandle::new(
            app_state.process_registry.clone(),
        ),
    );

    let mcp_reloader: Option<std::sync::Arc<dyn McpReloader>> = app_state
        .runtime
        .mcp_manager()
        .map(|mgr| mgr.clone() as std::sync::Arc<dyn McpReloader>);

    // toolset_session — mirrors crates/ironhermes-cli/src/main.rs:4771-4776's
    // RegistryToolsetSession construction from the runtime's live tool registry.
    let toolset_session: std::sync::Arc<dyn ToolsetSessionHandle> = std::sync::Arc::new(
        ironhermes_tools::RegistryToolsetSession::new(
            app_state.runtime.registry().clone(),
            app_state.runtime.merged_tools().clone(),
        ),
    );

    // workspace — resolved the same way `AppState::init` itself sourced the
    // cwd it passed into `AgentRuntimeInput.cwd` (state.rs), and the same way
    // the CLI resolves the value it passes into build_cmd_ctx (main.rs).
    // `AgentRuntime.cwd` is private with no accessor, so this reads
    // `std::env::current_dir()` directly rather than adding a second AppState
    // field for a value AppState::init already computes from the same source.
    let workspace = std::env::current_dir()
        .ok()
        .and_then(|cwd| ironhermes_core::workspace::resolve_from_cwd(&cwd))
        .map(std::sync::Arc::new);

    // trajectory_writer — realtime_trajectory_writer is populated only for
    // realtime-voice sessions (state.rs:233, config-gated). For the ordinary
    // text WS path it is None, and a None here fails the D-12 gate, so open
    // (or reuse) a per-session writer the same way the CLI/gateway do.
    let trajectory_writer = match app_state.realtime_trajectory_writer.clone() {
        Some(handle) => Some(handle),
        None => open_web_trajectory_writer(&workspace, session_id),
    };

    assemble_web_core_handles(
        state_store,
        subagent_registry,
        process_registry,
        app_state.runtime.skill_registry().clone(),
        toolset_session,
        app_state.turn_registry.clone(),
        workspace,
        mcp_reloader,
        trajectory_writer,
    )
}

/// D-12 assembly seam for [`web_core_handles`], decoupled from `AppState` /
/// `AgentRuntime` so it can be exercised in a unit test against lightweight
/// fakes (`web_core_handles_are_complete` below) instead of a full `AppState`
/// — which requires `AgentRuntime::from_config` (config/network-dependent at
/// construction time, unsuitable for a unit test). This function IS the D-12
/// enumeration: every one of the nine core handles is threaded through as a
/// named parameter, so a caller that forgets one is a compile error, not a
/// silent gap.
#[cfg(feature = "server")]
#[allow(clippy::too_many_arguments)]
fn assemble_web_core_handles(
    state_store: std::sync::Arc<dyn ironhermes_core::commands::context::StateStoreHandle>,
    subagent_registry: std::sync::Arc<dyn ironhermes_core::commands::context::SubagentListSnapshot>,
    process_registry: std::sync::Arc<
        dyn ironhermes_core::commands::context::ProcessRegistrySnapshotHandle,
    >,
    skill_registry: std::sync::Arc<ironhermes_core::SkillRegistry>,
    toolset_session: std::sync::Arc<dyn ironhermes_core::commands::context::ToolsetSessionHandle>,
    turn_registry: std::sync::Arc<ironhermes_core::TurnRegistry>,
    workspace: Option<std::sync::Arc<ironhermes_core::workspace::Workspace>>,
    mcp_reloader: Option<std::sync::Arc<dyn ironhermes_core::commands::context::McpReloader>>,
    trajectory_writer: Option<
        std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>,
    >,
) -> ironhermes_core::commands::context::CoreContextHandles {
    ironhermes_core::commands::context::CoreContextHandles {
        subagent_registry: Some(subagent_registry),
        process_registry: Some(process_registry),
        skill_registry: Some(skill_registry),
        state_store: Some(state_store),
        toolset_session: Some(toolset_session),
        turn_registry: Some(turn_registry),
        workspace,
        mcp_reloader,
        trajectory_writer,
    }
}

/// Phase 41.3 UAT finding F-1: attach the `provider_resolver` handle, which is
/// **not** one of the nine core handles and therefore is not set by
/// `build_core_context`.
///
/// Without this, `cmd_model` / `cmd_provider` / `cmd_fast` hit their
/// `ctx.provider_resolver == None` guards (`handlers.rs:999,1033,1050`) and Web
/// answered `"Provider resolver not configured."` for `/model`, `/provider` and
/// `/fast` — despite `AppState.resolver` (`state.rs:58`) having held a real
/// `ProviderResolver` since init. Exactly the shape of the `/agents` fallback
/// Plan 04 fixed: the value was present, it was simply never handed to
/// `CommandContext`.
///
/// Split out from the ws handler so it is reachable from a unit test without
/// standing up a full `AppState` (which needs `AgentRuntime::from_config`) —
/// same rationale as `assemble_web_core_handles` above.
#[cfg(feature = "server")]
fn attach_web_provider_resolver(
    ctx: ironhermes_core::commands::context::CommandContext,
    resolver: std::sync::Arc<ironhermes_core::ProviderResolver>,
) -> ironhermes_core::commands::context::CommandContext {
    ctx.with_provider_resolver(std::sync::Arc::new(
        ironhermes_core::commands::context::ProviderResolverAdapter::new(resolver),
    ))
}

/// Phase 41.3 Plan 04 (D-12): open a per-session `TrajectoryWriter` for the
/// Web slash-dispatch path, at `<workspace>/.ironhermes/sessions/<id>/trajectories.jsonl`
/// when a workspace is resolved, else `~/.ironhermes/sessions/<id>/trajectories.jsonl`.
/// Identical path scheme to the CLI's `build_cmd_ctx` construction (main.rs).
/// Best-effort: an open failure logs and returns `None` rather than failing
/// the turn — matches the CLI's own error handling for the same open call.
#[cfg(feature = "server")]
fn open_web_trajectory_writer(
    workspace: &Option<std::sync::Arc<ironhermes_core::workspace::Workspace>>,
    session_id: &str,
) -> Option<std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle>> {
    let traj_dir = match workspace {
        Some(ws) => ws.root.join(".ironhermes").join("sessions").join(session_id),
        None => ironhermes_core::get_hermes_home()
            .join("sessions")
            .join(session_id),
    };
    let traj_path = traj_dir.join("trajectories.jsonl");
    match ironhermes_trajectory::TrajectoryWriter::open(&traj_path) {
        Ok(w) => {
            let arc_writer = std::sync::Arc::new(std::sync::Mutex::new(w));
            let handle: std::sync::Arc<dyn ironhermes_core::commands::context::TrajectoryWriterHandle> =
                std::sync::Arc::new(ironhermes_trajectory::TrajectoryWriterHandleImpl::new(
                    arc_writer,
                ));
            Some(handle)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %traj_path.display(),
                session_id = %session_id,
                "Phase 41.3 Plan 04: failed to open Web trajectory writer; \
                 per-tool-call ledger disabled for this session"
            );
            None
        }
    }
}

/// Phase 49.7 Plan 05 (T-49.7-05-01, accepted web gap): renders
/// `CommandResult::StartGoalLoop` as an honest not-configured sentence.
///
/// Deliberately takes `objective`/`budget` as parameters (rather than
/// closing over nothing) so a test can assert the returned text never
/// contains the caller's objective — this function's body is the ENTIRE
/// arm; it constructs and returns one string, makes no call at all (no
/// task spawn, no goal-loop entry point, no judge construction), matching
/// `/cron`'s and `/blueprint run`'s already-shipped behavior on this
/// surface (`iron_hermes_ui` wires no `CronJobReader`/`CronJobWriter`, so
/// there is no goal-loop executor here to hand off to).
fn goal_loop_not_available_on_web(objective: &str, budget: Option<u32>) -> String {
    let _ = objective; // intentionally unused: never echoed back to the browser
    let _ = budget;
    "`/goal` isn't available on the web terminal yet — run it from the CLI or \
     a connected chat platform (this server has no goal-loop executor wired)."
        .to_string()
}

/// Phase 57 D-14: map one [`WebGateEvent`] onto the wire event for the turn `turn_id`.
///
/// `Security` events return `None` here: they map to several wire events (a dot severity
/// and at most one critical row), so [`web_notice_sink`] sends them through
/// [`security_event_to_stream`] instead. Neither approval event carries the raw arguments
/// (the gate never forwards them), only the tool name and the redacted reason.
#[cfg(feature = "server")]
pub(crate) fn web_gate_event_to_stream_event(
    turn_id: uuid::Uuid,
    event: &crate::server::web_approval_gate::WebGateEvent,
) -> Option<ChatStreamEvent> {
    use crate::server::web_approval_gate::WebGateEvent;
    match event {
        WebGateEvent::YoloBypass(s) => Some(ChatStreamEvent::YoloBypassNotice {
            turn_id,
            session_id: s.session_id.clone(),
            count: s.count(),
            tools: s.by_tool(),
            reasons: s.reasons_capped(20),
        }),
        WebGateEvent::ApprovalRequested {
            session_id,
            approval_id,
            tool,
            reason,
        } => Some(ChatStreamEvent::ApprovalRequested {
            turn_id,
            session_id: session_id.clone(),
            approval_id: approval_id.clone(),
            tool: tool.clone(),
            reason: reason.clone(),
        }),
        WebGateEvent::ApprovalResolved {
            session_id,
            approval_id,
            tool,
            outcome,
        } => Some(ChatStreamEvent::ApprovalResolved {
            turn_id,
            session_id: session_id.clone(),
            approval_id: approval_id.clone(),
            tool: tool.clone(),
            outcome: outcome.clone(),
        }),
        WebGateEvent::Security(event) => {
            // One-to-many: handled by `web_notice_sink` via `security_event_to_stream`.
            tracing::debug!(
                target: "ironhermes::security::web_gate",
                kind = ?event.kind,
                tool = %event.tool,
                level = event.level,
                "security event delivered to the web gate"
            );
            None
        }
    }
}

/// The typed notice sink for one turn, built from the turn's own event sender. Used
/// by all three `run_web_turn` call sites (typed message, STT transcript, queue drain).
/// Never routed through the assistant-token `StreamCallback` (Phase 54.3 D-01).
#[cfg(feature = "server")]
fn web_notice_sink(
    tx: mpsc::UnboundedSender<ChatStreamEvent>,
    turn_id: uuid::Uuid,
    session_id: String,
) -> crate::server::web_approval_gate::NoticeSink {
    std::sync::Arc::new(
        move |event: &crate::server::web_approval_gate::WebGateEvent| {
            // Phase 57 D-16/D-17: a security event becomes the turn's dot severity and, for
            // the C2 kinds, one critical transcript row (see `security_event_to_stream`).
            if let crate::server::web_approval_gate::WebGateEvent::Security(ev) = event {
                for out in security_event_to_stream(turn_id, &session_id, ev) {
                    let _ = tx.send(out);
                }
                return;
            }
            if let Some(ev) = web_gate_event_to_stream_event(turn_id, event) {
                let _ = tx.send(ev);
            }
        },
    )
}

/// Phase 57 D-17: the severity event sent next to a failed `ToolCallEnd`, so the client
/// turns that turn's dot red without inferring it. `None` for a successful call.
#[cfg(feature = "server")]
fn tool_failure_severity(
    turn_id: uuid::Uuid,
    session_id: &str,
    success: bool,
) -> Option<ChatStreamEvent> {
    (!success).then(|| ChatStreamEvent::TurnSeverity {
        turn_id,
        session_id: session_id.to_string(),
        worst: crate::protocol::TurnWorst::Red,
    })
}

/// Phase 57 UI-SPEC R9c: the events to push after a slash command ran, given the
/// session's level `before` and `after` it. Empty when nothing changed (a refused or
/// same-level `/yolo`), else one `DefconLevel` carrying the server-computed `bypass`.
#[cfg(feature = "server")]
pub(crate) fn web_level_change_events(
    session_id: &str,
    before: ironhermes_core::config::DefconLevel,
    after: ironhermes_core::config::DefconLevel,
) -> Vec<ChatStreamEvent> {
    if before == after {
        return Vec::new();
    }
    vec![ChatStreamEvent::DefconLevel {
        session_id: session_id.to_string(),
        level: after.level(),
        bypass: after.yolo_bypass_active(),
    }]
}

/// The surface name a web level change is attributed to in its `defcon_changed` row.
#[cfg(feature = "server")]
const WEB_DEFCON_SURFACE: &str = "web";

/// Phase 57 D-22: the per-message facts the web surface supplies to `/yolo <N>`.
/// `auth_enabled` is whether the auth layer is on; the WebSocket upgrade only completes
/// after `require_auth` validated the session, so with auth on every message here comes
/// from an authenticated operator, and with it off no message can claim to.
#[cfg(feature = "server")]
pub(crate) fn web_defcon_auth_facts(
    auth_enabled: bool,
) -> ironhermes_core::commands::context::DefconAuthFacts {
    ironhermes_core::commands::context::DefconAuthFacts {
        web_authenticated: auth_enabled,
        actor: "web".to_string(),
        ..Default::default()
    }
}

/// The level a session's inbound text is screened at: its live level, or the process
/// default for an id that is not a safe session id (so a hostile id creates no registry
/// entry).
#[cfg(feature = "server")]
fn web_session_level(session_id: &str) -> ironhermes_core::config::DefconLevel {
    use ironhermes_core::defcon::session_defcon;
    if ironhermes_core::safe_session_id(session_id).is_some() {
        session_defcon().handle_for(session_id).get()
    } else {
        session_defcon().process_default()
    }
}

/// Phase 57 D-04(b): screen one inbound chat message (as pasted text) before the model
/// sees it. `true` lets the turn proceed. A Block sends one `Error` event naming the rule
/// ids on `tx`, writes a `security_verdict` row (tool `inbound_message`; rule ids and the
/// level, never the text) and returns `false`; a Warn writes the row and proceeds.
///
/// A Block or Warn that carries `unavailable` is a message that could not be screened
/// (CR-01). The Error then reads `Message not processed: it could not be screened
/// (<reason>).` and the row is `sidecar_unavailable` (reason, tool, level, verdict).
#[cfg(feature = "server")]
pub(crate) async fn web_inbound_gate_with(
    tx: &mpsc::UnboundedSender<ChatStreamEvent>,
    turn_id: uuid::Uuid,
    session_id: &str,
    text: &str,
    level: ironhermes_core::config::DefconLevel,
    audit: &ironhermes_core::AuditLog,
) -> bool {
    use ironhermes_core::content_screen::{ContentKind, ContentScreenRequest, ScreenVerdict};

    // No screen installed means no host booted the sidecar: nothing to ask. (The server
    // main boots it before serving; the one-time warning lives in `GatePolicy`.)
    let Some(screen) = ironhermes_core::content_screen::content_screen() else {
        return true;
    };

    // The screen is synchronous, so run it off the connection's task. It must not use
    // `block_in_place` (the per-connection LocalSet forbids it); `spawn_blocking` is fine.
    let owned = text.to_string();
    let outcome = tokio::task::spawn_blocking(move || {
        screen.screen(&ContentScreenRequest {
            kind: ContentKind::Paste,
            text: &owned,
            name: None,
            level,
            warn_class: false,
        })
    })
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            // The screen itself panicked: it cannot vouch for the text, so refuse.
            tracing::warn!(
                target: "ironhermes::security::web_inbound",
                session_id = %session_id,
                error = %e,
                "inbound message screen did not complete; refusing the message"
            );
            let _ = tx.send(ChatStreamEvent::Error {
                turn_id,
                message: "Message not processed: the security screen could not run.".to_string(),
            });
            return false;
        }
    };

    if outcome.verdict == ScreenVerdict::Allow {
        return true;
    }
    let decision = if outcome.verdict == ScreenVerdict::Block {
        "blocked"
    } else {
        "warn"
    };
    let rule_ids: Vec<&str> = outcome.rules.iter().map(String::as_str).collect();
    // CR-01 / D-12: a Block or Warn that carries `unavailable` is a message the sidecar
    // could not screen. It is audited as `sidecar_unavailable` with the reason, not as a
    // reasonless `security_verdict` row.
    let unscreened = outcome.unscreened_reason();
    // Rule ids, the reason and the level only: never the text (T-57-27).
    let (kind, reason, args) = match (&unscreened, outcome.unavailable.as_deref()) {
        (Some(why), Some(raw)) => (
            ironhermes_core::context_scanner::SIDECAR_UNAVAILABLE_KIND,
            format!(
                "inbound web message {decision} at DEFCON {}: {why}",
                level.level()
            ),
            serde_json::json!({
                "reason": raw,
                "tool": "inbound_message",
                "level": level.level(),
                "verdict": if outcome.verdict == ScreenVerdict::Block { "block" } else { "warn" },
            }),
        ),
        _ => (
            ironhermes_core::context_scanner::SECURITY_VERDICT_KIND,
            format!(
                "inbound web message {decision} by the security screen at DEFCON {}",
                level.level()
            ),
            serde_json::json!({
                "rule": rule_ids.join(","),
                "rules": rule_ids,
                "level": level.level(),
            }),
        ),
    };
    let entry = audit.make_entry(
        kind,
        "inbound_message",
        None,
        session_id,
        "web",
        session_id,
        reason,
        decision,
        "verdict",
        &args,
    );
    if let Err(e) = audit.append(&entry).await {
        tracing::warn!(
            target: "ironhermes::security::web_inbound",
            error = %e,
            "failed to append the inbound-message security_verdict audit row"
        );
    }

    if outcome.verdict == ScreenVerdict::Warn {
        return true;
    }
    let message = if let Some(why) = unscreened {
        format!("Message not processed: it {why}.")
    } else {
        let why = if !rule_ids.is_empty() {
            rule_ids.join(", ")
        } else {
            "security policy".to_string()
        };
        format!("Message not processed: the security screen flagged it ({why}).")
    };
    let _ = tx.send(ChatStreamEvent::Error { turn_id, message });
    false
}

/// [`web_inbound_gate_with`] for a running server: the session's live level and the
/// configured audit log.
#[cfg(feature = "server")]
async fn web_inbound_gate(
    app_state: &crate::server::state::AppState,
    tx: &mpsc::UnboundedSender<ChatStreamEvent>,
    turn_id: uuid::Uuid,
    session_id: &str,
    text: &str,
) -> bool {
    let audit = ironhermes_core::AuditLog::load(app_state.config().audit.clone());
    let level = web_session_level(session_id);
    web_inbound_gate_with(tx, turn_id, session_id, text, level, &audit).await
}

#[get("/api/ws/chat")]
pub async fn ws_chat(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    #[cfg(feature = "server")]
    let app_state = crate::server::state::global_app_state().clone();

    Ok(ws.on_upgrade(
        move |mut socket: dioxus_fullstack::TypedWebsocket<String, String>| {
            #[cfg(feature = "server")]
            let app_state = app_state.clone();
            async move {
                #[cfg(feature = "server")]
                {
                struct InFlightTurn {
                    // `turn_id` mirrors the HashMap key and `rx` is a vestigial
                    // sentinel — the live receiver is drained via `stream_map`
                    // after the Plan-02 concurrent-turn refactor. Both are retained
                    // for struct clarity but intentionally never read (cf. the
                    // `_per_permit` / `_global_permit` dummy-placeholder fields).
                    #[allow(dead_code)]
                    turn_id: TurnId,
                    session_id: String,
                    #[allow(dead_code)]
                    rx: mpsc::UnboundedReceiver<ChatStreamEvent>,
                    handle: JoinHandle<()>,
                    /// Semaphore permits held for the lifetime of this turn (RAII release on drop).
                    _per_permit: tokio::sync::OwnedSemaphorePermit,
                    _global_permit: tokio::sync::OwnedSemaphorePermit,
                    /// Cancellation token — signalled by /stop.
                    cancel: tokio_util::sync::CancellationToken,
                }

                info!("websocket chat connection established");

                // Phase 36.17.9 (D-14): push VoiceStatus snapshot immediately on connect.
                // Server-asserted — client never sends availability back (T-36.17.9-01-01).
                // Emit before starting the select! loop so the client receives the snapshot
                // before any user interaction. Errors are swallowed (transport may have dropped).
                {
                    let vs = build_voice_status(&app_state);
                    if let Ok(vs_json) = serde_json::to_string(&vs) {
                        let _ = socket.send_raw(Message::Text(vs_json)).await;
                    }
                }

                // Phase 39.1 Plan 02 (R39.1-01/R39.1-06): replace single Option<InFlightTurn>
                // with a HashMap keyed by TurnId plus a StreamMap for concurrent drain.
                // Multiple turns per session can be in flight simultaneously up to
                // concurrency.session_turn_cap; overflow falls back to the FIFO queue.
                let mut in_flight_turns: HashMap<TurnId, InFlightTurn> = HashMap::new();
                let mut stream_map: StreamMap<TurnId, UnboundedReceiverStream<ChatStreamEvent>> =
                    StreamMap::new();

                // Phase 36.17.8 Plan 06 (D-13/D-14): STT transcript injection channel.
                // Binary audio frames are processed asynchronously (ffmpeg + STT API).
                // On success the spawned task sends (session_id, transcript) here; the
                // third select arm below routes the transcript through the same
                // run_web_turn + InFlightTurn path as a user-typed message.
                // Phase 40.5 Plan 08 (D-12/D-17): channel carries (session_id, transcript,
                // active_identity) so the frozen identity from the binary audio frame is
                // threaded through to auto_speak_reply for per-identity TTS selection.
                let (tx_stt, mut rx_stt) =
                    tokio::sync::mpsc::unbounded_channel::<(String, String, Option<String>)>();

                // Phase 36.17.4 (D-01): canonical SessionKey for every queue
                // call site in this connection. `web_key(session_id)` returns
                // a key with platform=Web, chat_id=session_id, user_id="web"
                // per the must_have invariant. Used at 6+ call sites below.
                fn web_key(session_id: &str) -> ironhermes_core::session::SessionKey {
                    ironhermes_core::session::SessionKey {
                        platform: ironhermes_core::types::Platform::Web,
                        chat_id: session_id.to_string(),
                        user_id: Some("web".into()),
                    }
                }

                let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
                keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // Skip first tick so we don't Ping immediately on connect.
                keepalive.tick().await;

                // Phase 57 D-16: every chat socket subscribes ONCE to core's process-local
                // security bus. A cron prompt or kanban task blocked in this server process
                // (plan 11) is published there, outside any turn; the select arm below turns
                // each `JobBlocked` into a nil-turn `job_blocked` critical row. A closed bus
                // disables only that arm.
                let mut security_bus = ironhermes_core::subscribe_security_events();
                let mut security_bus_open = true;

                loop {
                    tokio::select! {
                        // ── Incoming frames from the client ──────────────────────
                        //
                        // Use recv_raw so we handle each frame type explicitly.
                        // TypedWebsocket::recv() (the typed/Stream path) tries to
                        // JSON-decode the text frame as type String, which fails for
                        // raw JSON object payloads like {"session_id":...,"message":...}
                        // because a JSON object is not a JSON string literal. Using
                        // recv_raw bypasses that decode layer entirely — we read the
                        // raw text and parse it ourselves as ChatRequest.
                        raw = socket.recv_raw() => {
                            let text = match raw {
                                Ok(Message::Text(t)) => {
                                    info!("websocket chat message received (len={})", t.len());
                                    t
                                }
                                Ok(Message::Close { code, reason }) => {
                                    let in_flight = !in_flight_turns.is_empty();
                                    let session_id = in_flight_turns
                                        .values()
                                        .next()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");
                                    warn!(
                                        session_id = %session_id,
                                        code = ?code,
                                        reason = %reason,
                                        in_flight,
                                        "websocket close frame received; exiting connection"
                                    );
                                    for (_, turn) in in_flight_turns.drain() {
                                        turn.cancel.cancel();
                                        let _ = turn.handle.await;
                                    }
                                    send_close_frame(
                                        &mut socket,
                                        CloseCode::Normal,
                                        "recv closed cleanly",
                                    )
                                    .await;
                                    break;
                                }
                                // Phase 36.17.8 Plan 06 (D-13/D-14): inbound Binary frame
                                // from the browser mic. Deserialize as AudioInFrame, apply
                                // size cap (T-36.17.8-web-dos), transcode → WAV via ffmpeg
                                // (passthrough when absent), call SttProvider::transcribe,
                                // apply HallucinationFilter, submit surviving transcript on
                                // the existing turn path.
                                Ok(Message::Binary(raw_bytes)) => {
                                    // T-36.17.8-web-dos: size cap BEFORE any work.
                                    // 10 MB cap — well above 60s × 16 kHz × 2 B but below
                                    // the Whisper 25 MB API hard limit.
                                    const AUDIO_IN_MAX_BYTES: usize = 10 * 1024 * 1024;
                                    if raw_bytes.len() > AUDIO_IN_MAX_BYTES {
                                        warn!(
                                            bytes = raw_bytes.len(),
                                            "audio-in frame exceeds size cap; dropping (T-36.17.8-web-dos)"
                                        );
                                        continue;
                                    }

                                    // Deserialize the JSON-encoded AudioInFrame payload.
                                    // Parse failure → drop silently (T-36.17.8-frame-confusion).
                                    let frame: crate::protocol::AudioInFrame =
                                        match serde_json::from_slice(&raw_bytes) {
                                            Ok(f) => f,
                                            Err(e) => {
                                                warn!(
                                                    reason = %e,
                                                    "audio-in frame failed to deserialize as AudioInFrame; dropping"
                                                );
                                                continue;
                                            }
                                        };

                                    // Second size cap on the inner bytes field (client
                                    // could have embedded oversized bytes inside the JSON).
                                    if frame.bytes.len() > AUDIO_IN_MAX_BYTES {
                                        warn!(
                                            bytes = frame.bytes.len(),
                                            "audio-in frame.bytes exceeds size cap; dropping (T-36.17.8-web-dos)"
                                        );
                                        continue;
                                    }

                                    // Phase 36.17.9 (D-12, Wave D): wake-word-check fork.
                                    //
                                    // When `frame.wake_word_check` is true, the client
                                    // sent a short VAD-gated clip for phrase detection —
                                    // NOT a full turn. Transcribe via the same STT
                                    // provider as the full-turn path, match against the
                                    // wake phrase using `wake_word_matches` (ReDoS-safe
                                    // to_lowercase().contains(), T-36.17.9-04-01), then
                                    // emit WakeWordResult to the client. The full-turn
                                    // submission path (below) is entirely skipped.
                                    if frame.wake_word_check {
                                        // Clone all values needed inside the spawn.
                                        let ww_session_id = frame.session_id.clone();
                                        let ww_bytes     = frame.bytes.clone();
                                        let ww_mime      = frame.mime.clone();
                                        // D-13: phrase travels on the frame (client-controlled).
                                        // Fall back to server config only when frame.wake_phrase is None.
                                        let ww_phrase_from_frame = frame.wake_phrase.clone();
                                        let ww_app_state = app_state.clone();
                                        // We need a way to send the result back to the client.
                                        // Reuse the tx_stt channel with a sentinel to route
                                        // WakeWordResult — but that channel only carries transcripts.
                                        // Instead, capture a clone of the socket sender by routing
                                        // through a dedicated one-shot channel.
                                        let (tx_ww, mut rx_ww) =
                                            tokio::sync::mpsc::unbounded_channel::<crate::protocol::ChatStreamEvent>();
                                        tokio::spawn(async move {
                                            // D-18: read config ONCE for this clip and hold it.
                                            let cfg = fresh_voice_config_async(&ww_app_state.config()).await;

                                            // D-19: the same shared clip transcriber as the
                                            // full-turn path (no hallucination filter here: the
                                            // phrase match is the gate).
                                            let audio_cache_dir =
                                                ironhermes_core::constants::get_hermes_home()
                                                    .join("audio_cache");
                                            let transcript_raw = match wake_word_transcript(
                                                &cfg,
                                                &ww_bytes,
                                                &ww_mime,
                                                &audio_cache_dir,
                                            )
                                            .await
                                            {
                                                Some(t) => t,
                                                None => {
                                                    let _ = tx_ww.send(
                                                        crate::protocol::ChatStreamEvent::WakeWordResult { matched: false },
                                                    );
                                                    return;
                                                }
                                            };

                                            // D-13: resolve phrase from frame first,
                                            // fall back to server config when not set.
                                            let raw_phrase = ww_phrase_from_frame
                                                .unwrap_or_else(|| cfg.voice.wake_word.phrase.clone());
                                            // T-36.17.9-04-02: length-guard phrase to 64 chars server-side.
                                            let phrase: String =
                                                raw_phrase.chars().take(64).collect();

                                            let matched = wake_word_matches(&transcript_raw, &phrase);
                                            info!(
                                                session_id = %ww_session_id,
                                                matched,
                                                phrase_len = phrase.len(),
                                                "wake-word-check: STT result evaluated"
                                            );
                                            let _ = tx_ww.send(
                                                crate::protocol::ChatStreamEvent::WakeWordResult { matched },
                                            );
                                        });

                                        // Forward the WakeWordResult to the client on this socket.
                                        if let Some(result_ev) = rx_ww.recv().await {
                                            if let Ok(json) = serde_json::to_string(&result_ev) {
                                                let _ = socket
                                                    .send_raw(Message::Text(json))
                                                    .await;
                                            }
                                        }
                                        continue;
                                    }

                                    // Clone what the spawn needs; avoid holding any borrows
                                    // across the spawn boundary.
                                    let stt_session_id = frame.session_id.clone();
                                    let audio_bytes = frame.bytes.clone();
                                    let audio_mime = frame.mime.clone();
                                    let app_state_stt = app_state.clone();
                                    let tx_stt_inner = tx_stt.clone();
                                    // Phase 40.5 Plan 08 (D-12/D-17): capture frozen identity
                                    // from the audio frame; threaded to auto_speak_reply via channel.
                                    let stt_active_identity_frame = frame.active_identity.clone();

                                    // Spawn the STT pipeline so the select! loop remains
                                    // responsive while ffmpeg/STT run.
                                    tokio::spawn(async move {
                                        // D-18: read config ONCE at the start of the turn and
                                        // hold it, so an in-flight turn finishes on the settings
                                        // it started with and a save between turns applies to the
                                        // next one without a restart.
                                        let cfg = fresh_voice_config_async(&app_state_stt.config()).await;

                                        // D-19: one shared clip transcriber (provider selection,
                                        // UUID temp files, ffmpeg, transcribe, cleanup) plus the
                                        // D-12 HallucinationFilter. No client-controlled path
                                        // reaches the cache directory (T-36.17.8-web-path).
                                        let audio_cache_dir =
                                            ironhermes_core::constants::get_hermes_home()
                                                .join("audio_cache");
                                        let transcript = match transcribe_turn_audio(
                                            &cfg,
                                            &audio_bytes,
                                            &audio_mime,
                                            &audio_cache_dir,
                                        )
                                        .await
                                        {
                                            Some(t) => t,
                                            // Failure or filtered out: already logged, and no
                                            // empty bubble per UI-SPEC.
                                            None => return,
                                        };

                                        // Inject the transcript into the main select loop
                                        // via tx_stt so it flows through the same
                                        // run_web_turn + InFlightTurn path as a user-typed
                                        // message (D-13 / D-15: transcript = normal user turn).
                                        // Phase 40.5 Plan 08 (D-17): also carry the frozen identity.
                                        if tx_stt_inner
                                            .send((stt_session_id.clone(), transcript, stt_active_identity_frame))
                                            .is_err()
                                        {
                                            // Connection closed while STT was in flight.
                                            warn!(
                                                session_id = %stt_session_id,
                                                "audio-in: transcript channel closed; dropping transcript"
                                            );
                                        }
                                    });
                                    continue;
                                }
                                // Ping/Pong — skip silently.
                                Ok(_) => continue,
                                Err(err) => {
                                    let reason = err.to_string();
                                    let in_flight = !in_flight_turns.is_empty();
                                    let session_id = in_flight_turns
                                        .values()
                                        .next()
                                        .map(|t| t.session_id.as_str())
                                        .unwrap_or("unknown");
                                    warn!(
                                        session_id = %session_id,
                                        reason = %reason,
                                        in_flight,
                                        "websocket recv failed; closing connection"
                                    );
                                    for (_, turn) in in_flight_turns.drain() {
                                        turn.cancel.cancel();
                                        turn.handle.abort();
                                    }
                                    send_close_frame(&mut socket, CloseCode::Away, "recv failed")
                                        .await;
                                    break;
                                }
                            };

                            let req: ChatRequest = match serde_json::from_str(&text) {
                                Ok(r) => r,
                                Err(e) => {
                                    let err_event = ChatStreamEvent::Error { turn_id: uuid::Uuid::nil(),
                                        message: format!("Invalid request: {e}"),
                                    };
                                    let _ = socket
                                        .send_raw(Message::Text(
                                            serde_json::to_string(&err_event)
                                                .unwrap_or_default(),
                                        ))
                                        .await;
                                    continue;
                                }
                            };

                            // WR-04: the Gateway is off unless `ui.openui_gateway_enabled` is set
                            // server-side; the client toggle alone never reaches api.thesys.dev.
                            if openui_gateway_requested(&req)
                                && !super::openui_gateway::gateway_enabled()
                            {
                                let err_event = ChatStreamEvent::Error {
                                    turn_id: uuid::Uuid::nil(),
                                    message: OPENUI_GATEWAY_DISABLED.to_string(),
                                };
                                let _ = socket
                                    .send_raw(Message::Text(
                                        serde_json::to_string(&err_event).unwrap_or_default(),
                                    ))
                                    .await;
                                continue;
                            }

                            // CR-02: a generated view's action text is model-controlled; it must
                            // never reach the slash-command router (`/yolo`, `/new`, skills).
                            // RR-04: the operator's own typed Generate prompt is exempt.
                            if openui_action_slash_blocked(&req) {
                                let err_event = ChatStreamEvent::Error {
                                    turn_id: uuid::Uuid::nil(),
                                    message: "Generated views cannot run commands.".to_string(),
                                };
                                let _ = socket
                                    .send_raw(Message::Text(
                                        serde_json::to_string(&err_event).unwrap_or_default(),
                                    ))
                                    .await;
                                continue;
                            }

                            let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                            let app_state = app_state.clone();
                            // Phase 54.9 Plan 09 (D-09): resolved before `req` is partially moved.
                            // Phase 54.9 Plan 10 (D-09): the Gateway path sends the prompt itself, so
                            // it must not leave a one-shot overlay behind for a later normal turn.
                            let openui_gateway = openui_gateway_requested(&req);
                            let openui_overlay = if openui_gateway {
                                None
                            } else {
                                openui_overlay_for(&req)
                            };
                            let session_id = req.session_id;
                            let session_id_for_turn = session_id.clone();
                            let message = req.message;
                            // Phase 41.1 Plan 03 (SKILL-13 web / D-06): the text actually RUN
                            // as this turn. Defaults to the user's message; the SKILL-13
                            // NotFound fallback below overrides it with the resolved skill's
                            // trigger_text (bare → run-now instruction, argued → verbatim
                            // trailing text). `message` itself stays the ORIGINAL slash input
                            // so the rare capacity-race requeue re-resolves it cleanly.
                            let mut turn_input = message.clone();
                            // Phase 41.1 Plan 03 (D-06, UI-SPEC §C): Some(chip) when this turn
                            // is a one-shot skill run — emitted as a DIM `RunTurnMeta` event
                            // immediately before the reply streams.
                            let mut skill_run_chip: Option<String> = None;
                            // Phase 40.5 Plan 08 (D-17): freeze identity from the ChatRequest
                            // (already validated by the client's is_known_identity gate).
                            let req_active_identity = req.active_identity;
                            // Phase 46.7 Plan 04 (D-09): ids of chat_attachments rows to
                            // resolve into this turn's user message. No empty-message guard
                            // exists on this path today, so an attachment-only ChatRequest
                            // (empty message, non-empty attachment_ids) already dispatches a
                            // turn unconditionally (D-07) — this just threads the ids through.
                            let attachment_ids = req.attachment_ids;

                            // Phase 39.1 Plan 02 (R39.1-03/D-03): FIFO gate via semaphore
                            // try_acquire instead of the old Option<InFlightTurn>.is_some() flag.
                            // Slash commands fall through so bypass-listed ones (/stop, /new, etc.)
                            // still dispatch mid-turn (D-06). Non-slash messages that cannot acquire
                            // a permit are queued instead of rejected (R39.1-06).
                            if app_state.concurrency.try_acquire().is_none() && !message.starts_with('/') {
                                let key = web_key(&session_id);
                                let paused_flag =
                                    app_state.get_or_create_paused_flag(&session_id);
                                let paused_snapshot = paused_flag
                                    .load(std::sync::atomic::Ordering::SeqCst);
                                let (tx_q, rx_q) =
                                    mpsc::unbounded_channel::<ChatStreamEvent>();
                                match app_state.queue.try_push(
                                    &key,
                                    tag_queued_message(&message, openui_overlay.is_some()),
                                ) {
                                    Ok(()) => {
                                        let depth = app_state.queue.len(&key) as u32;
                                        let _ = tx_q.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                            text: format!(
                                                "Queued: \"{}\" ({} in queue)\n",
                                                message, depth
                                            ),
                                        });
                                        let _ = tx_q.send(ChatStreamEvent::QueueUpdated {
                                            depth,
                                            paused: paused_snapshot,
                                        });
                                    }
                                    Err(
                                        ironhermes_core::queue::QueueError::CapacityReached {
                                            max,
                                            ..
                                        },
                                    ) => {
                                        let _ = tx_q.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                            text: format!(
                                                "Queue is full ({max}/{max}). /stop or /flush to drain.\n"
                                            ),
                                        });
                                    }
                                }
                                let _ = tx_q.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                    total_tokens: 0,
                                });
                                drop(tx_q);
                                let mut qrx = rx_q;
                                while let Some(ev) = qrx.recv().await {
                                    let json = serde_json::to_string(&ev)
                                        .unwrap_or_default();
                                    let _ = socket
                                        .send_raw(Message::Text(json))
                                        .await;
                                }
                                continue;
                            }

                            // Phase 36.1 D-03/D-04/D-05 / Phase 39.1 D-06:
                            // Slash-command interception BEFORE run_web_turn.
                            //
                            // Resolution uses the canonical def.name (post-alias)
                            // so /reset → "new" correctly bypasses the guard
                            // (Pitfall 4 mitigation: never call is_bypass on raw input).
                            //
                            // Phase 39.1 D-06: slash commands are NEVER rejected mid-turn.
                            // The old "non-bypass slash rejected while turn in flight" block
                            // has been deleted — all slash commands now dispatch unconditionally.
                            // /new and /reset emit an in_flight_warning (warn, not block).
                            // /stop cancels in-flight turns via turn_registry.cancel_session.
                            //
                            // Slash dispatch does NOT set in_flight_turns (Pitfall 7):
                            // slash responses are synchronous single-turn outputs.
                            if message.starts_with('/') {
                                let platform = ironhermes_core::types::Platform::Web;
                                match app_state.command_router.resolve(&message, &platform) {
                                    ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
                                        // Phase 39.1 D-06: all slashes dispatch — dispatch normally.
                                        let parts: Vec<&str> =
                                            message.split_whitespace().collect();
                                        let args: Vec<&str> = if parts.len() > 1 {
                                            parts[1..].to_vec()
                                        } else {
                                            vec![]
                                        };
                                        // Phase 41.3 Plan 04 (D-11/D-12): Web builds its
                                        // CommandContext through the shared build_core_context
                                        // factory — previously Web wired only 2 of the 9 core
                                        // handles (state_store, skill_registry), which is why
                                        // `/agents` returned "Subagent registry not wired." even
                                        // though AppState has held the subagent registry since
                                        // state.rs:99. web_core_handles() sources all nine from
                                        // real values on AppState/AgentRuntime.
                                        // Phase 39.1 (R39.1-06 / D-06): the running-agent gate
                                        // flag was removed from CommandContext.
                                        let ctx = ironhermes_core::commands::context::build_core_context(
                                            platform,
                                            session_id.clone(),
                                            web_core_handles(&app_state, &session_id),
                                        );
                                        let ctx = attach_web_provider_resolver(
                                            ctx,
                                            app_state.resolver().clone(),
                                        );
                                        // Phase 57 D-22: `/yolo <N>` from the web is authorized
                                        // only when the auth layer is on (this socket's upgrade
                                        // then passed `require_auth`); with it off the facts
                                        // refuse, so the web surface cannot change the level.
                                        let defcon_facts = web_defcon_auth_facts(
                                            crate::server::state::web_auth_enabled(),
                                        );
                                        // The actor the level change is attributed to in the row.
                                        let defcon_actor = defcon_facts.actor.clone();
                                        let ctx = ctx.with_defcon_auth(defcon_facts);
                                        // Phase 57 UI-SPEC R9c / R8: the session's level just
                                        // before the command runs, to push a `DefconLevel` event
                                        // and a `defcon_changed` critical row if it changed it.
                                        let level_before = ironhermes_core::defcon::session_defcon()
                                            .handle_for(&session_id)
                                            .get();

                                        // Phase 39.1 Plan 02 (R39.1-08 / D-06): /new and /reset
                                        // warn (do not block) if turns are in flight for this
                                        // session. Warning is prepended to the normal response.
                                        if def.name == "new" || def.name == "reset" {
                                            if let Some(warn_msg) = ironhermes_core::commands::handlers::in_flight_warning(
                                                &app_state.turn_registry,
                                                &session_id,
                                            ).await {
                                                let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                    text: warn_msg,
                                                });
                                            }
                                        }

                                        // Phase 36.17.4 (D-04a / D-05) / Phase 39.1 (R39.1-08):
                                        // /stop early-intercept BEFORE dispatch.
                                        // Phase 39.1: also cancel in-flight turns via registry.
                                        // Sequence: queue.clear → cancel_session →
                                        // paused.store(false) → QueueUpdated → Delta → Finished → drain.
                                        if def.name == "stop" {
                                            let key = web_key(&session_id);
                                            app_state.queue.clear(&key);
                                            // Phase 39.1: signal all in-flight turns for this
                                            // session to stop via their CancellationTokens.
                                            let cancelled = app_state
                                                .turn_registry
                                                .cancel_session(&session_id)
                                                .await;
                                            if cancelled > 0 {
                                                info!(
                                                    session_id = %session_id,
                                                    count = cancelled,
                                                    "turn_registry: /stop cancelled in-flight turns"
                                                );
                                            }
                                            // Phase 57 D-14: a /stop also denies this session's
                                            // parked approval now, instead of leaving the cancelled
                                            // turn waiting out `approvals.timeout_secs`.
                                            app_state.web_approvals.cancel(&session_id);
                                            app_state
                                                .get_or_create_paused_flag(&session_id)
                                                .store(
                                                    false,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                            let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                depth: 0,
                                                paused: false,
                                            });
                                            let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                text:
                                                    "Queue cleared. In-flight turns cancelled.\n"
                                                        .to_string(),
                                            });
                                            let _ = tx.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                                total_tokens: 0,
                                            });
                                            drop(tx);
                                            let mut slash_rx = rx;
                                            while let Some(ev) = slash_rx.recv().await {
                                                let json = serde_json::to_string(&ev)
                                                    .unwrap_or_default();
                                                let _ = socket
                                                    .send_raw(Message::Text(json))
                                                    .await;
                                            }
                                            continue;
                                        }

                                        let result = ironhermes_core::commands::handlers::dispatch(
                                            def,
                                            &args,
                                            &ctx,
                                            &app_state.command_router,
                                        );

                                        // Phase 36.17.4 (D-01 / D-03 / D-06):
                                        // dedicated arms for Queued /
                                        // PauseQueue / UnpauseQueue. Each
                                        // performs its own complete emit
                                        // sequence + drain + continue (bypasses
                                        // the shared Delta/Finished delivery
                                        // below) so the QueueUpdated event can
                                        // be interleaved between Delta and
                                        // Finished per the must_have invariant.
                                        match result {
                                            CommandResult::Queued { message: queued_msg } => {
                                                let key = web_key(&session_id);
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let paused_snapshot = paused_flag.load(
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                match app_state
                                                    .queue
                                                    .try_push(&key, queued_msg.clone())
                                                {
                                                    Ok(()) => {
                                                        let depth =
                                                            app_state.queue.len(&key) as u32;
                                                        let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                            text: format!(
                                                                "Queued: \"{}\" ({} in queue)\n",
                                                                queued_msg, depth
                                                            ),
                                                        });
                                                        let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                            depth,
                                                            paused: paused_snapshot,
                                                        });
                                                    }
                                                    Err(
                                                        ironhermes_core::queue::QueueError::CapacityReached {
                                                            max,
                                                            ..
                                                        },
                                                    ) => {
                                                        let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                            text: format!(
                                                                "Queue is full ({max}/{max}). /stop or /flush to drain.\n"
                                                            ),
                                                        });
                                                    }
                                                }
                                                let _ = tx.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            CommandResult::PauseQueue => {
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let was_paused = paused_flag.fetch_xor(
                                                    true,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                let new_paused = !was_paused;
                                                let key = web_key(&session_id);
                                                let depth =
                                                    app_state.queue.len(&key) as u32;
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth,
                                                    paused: new_paused,
                                                });
                                                let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                    text: if new_paused {
                                                        format!(
                                                            "Queue paused. ({} queued)\n",
                                                            depth
                                                        )
                                                    } else {
                                                        format!(
                                                            "Queue resumed. ({} queued)\n",
                                                            depth
                                                        )
                                                    },
                                                });
                                                let _ = tx.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            CommandResult::UnpauseQueue => {
                                                let paused_flag = app_state
                                                    .get_or_create_paused_flag(&session_id);
                                                let was_paused = paused_flag.swap(
                                                    false,
                                                    std::sync::atomic::Ordering::SeqCst,
                                                );
                                                let key = web_key(&session_id);
                                                let depth =
                                                    app_state.queue.len(&key) as u32;
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth,
                                                    paused: false,
                                                });
                                                let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                    text: if was_paused {
                                                        "Queue resumed.\n".to_string()
                                                    } else {
                                                        "Queue was not paused.\n".to_string()
                                                    },
                                                });
                                                let _ = tx.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                                    total_tokens: 0,
                                                });
                                                drop(tx);
                                                let mut slash_rx = rx;
                                                while let Some(ev) = slash_rx.recv().await {
                                                    let json = serde_json::to_string(&ev)
                                                        .unwrap_or_default();
                                                    let _ = socket
                                                        .send_raw(Message::Text(json))
                                                        .await;
                                                }
                                                continue;
                                            }
                                            _ => {}
                                        }

                                        let text = match result {
                                            CommandResult::Output(t) => t,
                                            CommandResult::Error(e) => {
                                                format!("Command error: {e}")
                                            }
                                            CommandResult::NewSession { message: m } => {
                                                // Phase 36.17.4 (D-04): ordering
                                                // invariant — queue.clear →
                                                // paused.store(false) →
                                                // QueueUpdated → reset_web_session
                                                // → emit message. QueueUpdated
                                                // pushed into the same `tx`
                                                // (mpsc FIFO) BEFORE the shared
                                                // delivery's Delta(text), so
                                                // the client sees the pill
                                                // reset before the
                                                // confirmation Delta.
                                                let key = web_key(&session_id);
                                                app_state.queue.clear(&key);
                                                app_state
                                                    .get_or_create_paused_flag(&session_id)
                                                    .store(
                                                        false,
                                                        std::sync::atomic::Ordering::SeqCst,
                                                    );
                                                let _ = tx.send(ChatStreamEvent::QueueUpdated {
                                                    depth: 0,
                                                    paused: false,
                                                });
                                                app_state.reset_web_session(&session_id);
                                                m
                                            }
                                            CommandResult::Handled | CommandResult::Quit => {
                                                String::new()
                                            }
                                            // Phase 36.6.3 Plan 03 (TUI-INPUT-02, D-06):
                                            // bare `/model`/`/provider` open an
                                            // interactive picker only in tui_rata — the
                                            // web chat has no such overlay surface. Fall
                                            // back to the pre-existing plain-text output
                                            // (model_list_text()/status_text()) so
                                            // nothing regresses (mirrors the gateway/CLI
                                            // fallback).
                                            CommandResult::OpenModelPicker { fallback_text }
                                            | CommandResult::OpenProviderPicker {
                                                fallback_text,
                                            } => fallback_text,
                                            // Phase 41.1 Plan 03 (Pitfall 3 / prior 36.6.3
                                            // incident): EXPLICIT non-leaking arms for the skill
                                            // variants, placed BEFORE the terminal Debug wildcard
                                            // below. Without these, `/skills reload` (now that the
                                            // registry is attached, `cmd_skills` returns
                                            // `SkillsReload`) would fall into `other => {"{other:?}"}`
                                            // and Debug-format an internal enum into chat; a future
                                            // dispatch path returning `SkillActivated` would leak
                                            // the FULL SKILL.md `body` verbatim to the browser.
                                            CommandResult::SkillsReload => {
                                                // Web loads its SkillRegistry into the AgentRuntime
                                                // at server start and exposes no hot-swap of that
                                                // Arc (`AgentRuntime::skill_registry` returns
                                                // `&Arc<_>`), so a true reload+diff (as the
                                                // gateway/CLI perform) needs runtime registry-swap
                                                // plumbing that is out of this plan's scope
                                                // (architectural). Return an honest, non-leaking
                                                // message instead of Debug-leaking the variant.
                                                "Skills are loaded when the server starts. \
                                                 `/skills reload` isn't available on the web \
                                                 surface yet — restart the server to pick up \
                                                 skill changes."
                                                    .to_string()
                                            }
                                            CommandResult::SkillActivated { name, .. } => {
                                                // Defensive: on this surface skill activation
                                                // flows through the SKILL-13 NotFound fallback
                                                // (below), not through `dispatch`, so this arm is
                                                // not reached today. It exists so the SKILL.md
                                                // `body` can NEVER Debug-leak into chat if a future
                                                // dispatch path ever returns this variant.
                                                format!("Skill '{name}' activated.")
                                            }
                                            // Phase 49.7 Plan 05 (T-49.7-05-01): EXPLICIT arm,
                                            // placed BEFORE the terminal Debug wildcard below —
                                            // same reasoning as `SkillsReload`/`SkillActivated`
                                            // above. `iron_hermes_ui` wires no goal-loop executor
                                            // in this phase (accepted web gap, Phase 49.7 Plan
                                            // 05: no CronJobReader/CronJobWriter here either, the
                                            // same pre-existing limitation `/cron` and
                                            // `/blueprint run` already have on this surface). This
                                            // arm's only job is to render an honest sentence
                                            // instead of letting the wildcard Debug-format the
                                            // variant (and the user's objective) into the browser.
                                            CommandResult::StartGoalLoop { objective, budget } => {
                                                goal_loop_not_available_on_web(&objective, budget)
                                            }
                                            other => {
                                                format!("{other:?}")
                                            }
                                        };
                                        if !text.is_empty() {
                                            let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(), text });
                                        }
                                        // Phase 57 UI-SPEC R9c: right after the command's own
                                        // output, tell the client the level changed. A refused,
                                        // invalid or same-level `/yolo` pushes nothing.
                                        let level_after = ironhermes_core::defcon::session_defcon()
                                            .handle_for(&session_id)
                                            .get();
                                        for ev in web_level_change_events(&session_id, level_before, level_after) {
                                            let _ = tx.send(ev);
                                        }
                                        // Phase 57 R8: one `defcon_changed` row next to that push,
                                        // from the same before/after values; a refused or
                                        // same-level `/yolo` stays ordinary command output.
                                        if let Some(row) = defcon_changed_event(
                                            &session_id,
                                            level_before,
                                            level_after,
                                            WEB_DEFCON_SURFACE,
                                            &defcon_actor,
                                        ) {
                                            let _ = tx.send(row);
                                        }
                                        let _ =
                                            tx.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(), total_tokens: 0 });
                                        drop(tx);
                                        let mut slash_rx = rx;
                                        while let Some(ev) = slash_rx.recv().await {
                                            let json =
                                                serde_json::to_string(&ev).unwrap_or_default();
                                            let _ = socket.send_raw(Message::Text(json)).await;
                                        }
                                        continue;
                                    }
                                    ResolveResult::Ambiguous(_) | ResolveResult::NotFound => {
                                        // Phase 41.1 Plan 03 (SKILL-13 / D-06): dynamic-skill
                                        // fallback BEFORE agent passthrough — mirrors the gateway
                                        // NotFound arm (handler.rs:1454) and the TUI tracer
                                        // (Plan 02). Registered commands already won above (the
                                        // 3-stage resolution ran first). On a match we activate
                                        // the skill body into THIS session's one-shot overlay AND
                                        // fire a real agent turn through the SAME turn-spawn block
                                        // below — the synthesized turn inherits its session/turn
                                        // identity from the WS connection + TurnRegistry, never
                                        // reconstructed from the user-controlled slash args
                                        // (mirrors the anti-impersonation discipline at
                                        // handler.rs:1229-1230). On no match, keep today's
                                        // chat-passthrough.
                                        if let Some(run) = plan_web_skill_run(
                                            &app_state.runtime.skill_registry(),
                                            &message,
                                        ) {
                                            app_state.push_web_skill_overlay(
                                                &session_id,
                                                run.skill_name,
                                                run.skill_body,
                                            );
                                            skill_run_chip = Some(run.meta_chip);
                                            turn_input = run.turn_input;
                                            // Fall through to the shared turn-spawn block below.
                                        }
                                        // else: not a skill — fall through to run_web_turn as a
                                        // plain-text message (unchanged behavior).
                                    }
                                }
                            }

                            // Phase 39.1 Plan 02: plain-text guard removed. The semaphore-based
                            // FIFO at the top of this arm already gates messages that cannot
                            // acquire a permit into the queue. Messages that reach here have
                            // successfully acquired a permit (implicit in the FIFO check not
                            // triggering `continue`), so spawning is always safe.

                            // Phase 39.1 Plan 02 (R39.1-01/R39.1-03): acquire semaphore
                            // permits before spawning. try_acquire was already checked for
                            // non-slash messages above; slash commands that reach here never
                            // set in_flight_turns so they don't consume a permit. For the
                            // run_web_turn path we do a real acquire here — if it fails
                            // (extremely rare race: FIFO check passed but capacity exhausted
                            // between check and here) we fall back to queue and continue.
                            let (per_permit, global_permit) = match app_state.concurrency.try_acquire() {
                                Some(p) => p,
                                None => {
                                    // Rare race — fall back to queue same as FIFO trigger.
                                    let key = web_key(&session_id);
                                    let _ = app_state.queue.try_push(
                                        &key,
                                        tag_queued_message(&message, openui_overlay.is_some()),
                                    );
                                    let (tx_fb, rx_fb) = mpsc::unbounded_channel::<ChatStreamEvent>();
                                    let _ = tx_fb.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                        text: "Capacity full; message queued.\n".to_string(),
                                    });
                                    let _ = tx_fb.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(), total_tokens: 0 });
                                    drop(tx_fb);
                                    let mut fbrx = rx_fb;
                                    while let Some(ev) = fbrx.recv().await {
                                        let json = serde_json::to_string(&ev).unwrap_or_default();
                                        let _ = socket.send_raw(Message::Text(json)).await;
                                    }
                                    continue;
                                }
                            };

                            // Register-before-spawn discipline (R39.1-09).
                            let turn_id = TurnId::new_v4();
                            let cancel_token = tokio_util::sync::CancellationToken::new();
                            app_state.turn_registry.register(TurnEntry {
                                turn_id,
                                session_id: session_id.clone(),
                                surface: Surface::Web,
                                started_at: std::time::Instant::now(),
                                cancel: cancel_token.clone(),
                            }).await;

                            // Emit TurnStarted BEFORE spawning (R39.1-08).
                            let turn_index = in_flight_turns.len() as u32;
                            if let Ok(json) = serde_json::to_string(&ChatStreamEvent::TurnStarted {
                                turn_id,
                                session_id: session_id.clone(),
                                index: turn_index,
                            }) {
                                let _ = socket.send_raw(Message::Text(json)).await;
                            }

                            // Phase 41.1 Plan 03 (D-06, UI-SPEC §C): emit the DIM run-turn
                            // meta chip for a one-shot skill run, on the MAIN task (not the
                            // spawned turn) so it is ordered strictly BEFORE the turn's first
                            // Delta and renders ABOVE the streaming reply. Only the chip is
                            // user-visible for a bare invoke — the synthetic trigger text is
                            // never emitted as a user bubble.
                            if let Some(chip) = skill_run_chip.take() {
                                if let Ok(json) =
                                    serde_json::to_string(&ChatStreamEvent::RunTurnMeta { text: chip })
                                {
                                    let _ = socket.send_raw(Message::Text(json)).await;
                                }
                            }

                            let turn_id_spawn = turn_id;
                            let cancel_token_spawn = cancel_token.clone();
                            let registry_spawn = app_state.turn_registry.clone();
                            // Phase 57 D-04(b): the user's own text (never a skill run's
                            // synthesized trigger text) is what the inbound screen sees.
                            let inbound_text = message.clone();
                            let handle = tokio::spawn(async move {
                                // RAII: holds semaphore permits for the lifetime of this task.
                                let _per = per_permit;
                                let _global = global_permit;

                                // Phase 57 D-04(b): screen the message before the model sees it.
                                // A Block sends the Error event and ends the turn here.
                                if !web_inbound_gate(
                                    &app_state,
                                    &tx,
                                    turn_id_spawn,
                                    &session_id_for_turn,
                                    &inbound_text,
                                )
                                .await
                                {
                                    registry_spawn.deregister(turn_id_spawn).await;
                                    return;
                                }

                                // Phase 54.9 Plan 10 (D-09, T-54.9-27/28): the opt-in Thesys Gateway
                                // runs INSTEAD of run_web_turn (no tools, no session history). The
                                // key is resolved here, server-side; with the toggle off this block
                                // is never entered, so nothing contacts api.thesys.dev.
                                if openui_gateway {
                                    match crate::server::openui_gateway::resolve_thesys_key().await {
                                        Some(key) => {
                                            tokio::select! {
                                                _ = cancel_token_spawn.cancelled() => {
                                                    let _ = tx.send(ChatStreamEvent::TurnCancelled {
                                                        turn_id: turn_id_spawn,
                                                    });
                                                }
                                                usage = crate::server::openui_gateway::stream_gateway_generation(
                                                    crate::server::openui_gateway::GATEWAY_BASE_URL,
                                                    &key,
                                                    &inbound_text,
                                                    turn_id_spawn,
                                                    &tx,
                                                ) => {
                                                    if let Some(usage) = usage {
                                                        crate::server::openui_gateway::record_gateway_usage(
                                                            &app_state.state_store,
                                                            &session_id_for_turn,
                                                            usage,
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        None => {
                                            let _ = tx.send(ChatStreamEvent::Error {
                                                turn_id: turn_id_spawn,
                                                message: crate::server::openui_gateway::GATEWAY_MISSING_KEY
                                                    .to_string(),
                                            });
                                        }
                                    }
                                    registry_spawn.deregister(turn_id_spawn).await;
                                    return;
                                }

                                // WR-01: queue the OpenUI overlay only now that this turn passed the
                                // inbound gate and will run `run_web_turn`, which drains it. Queued at
                                // receipt, an early exit (queue, slash command, gate block) left it
                                // behind for the session's next turn.
                                if let Some((name, body)) = openui_overlay {
                                    app_state.push_web_skill_overlay(&session_id_for_turn, name, body);
                                }

                                // Phase 34a MEM-READ-05: scrub <memory-context> fence tags.
                                let scrubber_ws = std::sync::Arc::new(std::sync::Mutex::new(
                                    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
                                ));
                                // Phase 01-04 (DLV-03 web): extract `<MEDIA: ...>` photo tags
                                // off the stream so they are stripped from Delta text (not
                                // rendered as literal `<MEDIA: ...>`) and accumulated for
                                // dispatch as ImageOut frames after the turn completes.
                                // Mirrors the scrubber wire-up (and handler.rs's Telegram path).
                                let media_extractor_ws = std::sync::Arc::new(std::sync::Mutex::new(
                                    ironhermes_gateway::media_tag::MediaTagExtractor::new(),
                                ));
                                let scrubber_ws_cb = std::sync::Arc::clone(&scrubber_ws);
                                let media_extractor_cb = std::sync::Arc::clone(&media_extractor_ws);
                                let tx_stream = tx.clone();
                                let tid = turn_id_spawn;
                                let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                                    Box::new(move |delta: &str| {
                                        // Scrub memory-context fences first, then strip MEDIA
                                        // tags from the scrubbed text before emitting Delta.
                                        let scrubbed = scrubber_ws_cb.lock().unwrap().feed(delta);
                                        if scrubbed.is_empty() {
                                            return;
                                        }
                                        let visible =
                                            media_extractor_cb.lock().unwrap().feed(&scrubbed);
                                        if !visible.is_empty() {
                                            let _ = tx_stream.send(ChatStreamEvent::Delta { turn_id: tid,
                                                text: visible,
                                            });
                                        }
                                    });

                                let tx_tool = tx.clone();
                                let tool_progress_callback: ironhermes_agent::agent_loop::ToolProgressCallback =
                                    // Phase 57 A3: the arguments are never sent to the browser.
                                    Box::new(move |name: &str, _args: &str| {
                                        let _ = tx_tool.send(ChatStreamEvent::ToolCallStart { turn_id: tid,
                                            name: name.to_string(),
                                        });
                                    });

                                let tx_tool_result = tx.clone();
                                let sid_tool_result = session_id_for_turn.clone();
                                let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                    Box::new(move |name: &str, success: bool, _output: &str| {
                                        let _ = tx_tool_result.send(ChatStreamEvent::ToolCallEnd { turn_id: tid,
                                            name: name.to_string(),
                                            success,
                                        });
                                        // Phase 57 D-17: a failed call turns this turn's dot red.
                                        if let Some(sev) = tool_failure_severity(tid, &sid_tool_result, success) {
                                            let _ = tx_tool_result.send(sev);
                                        }
                                    });

                                // Phase 26.7.1 Plan 02 (D-06 / Path A): install this turn's tx into the
                                // callback slot so the singleton SubagentProgressCallback baked into
                                // AppRuntimeBundle can forward SubagentEvent {} to this client.
                                let tx_subagent = tx.clone();
                                {
                                    let mut guard = app_state.subagent_callback_slot.lock().await;
                                    *guard = Some(tx_subagent);
                                }
                                let _slot_guard = SubagentCallbackSlotGuard {
                                    slot: app_state.subagent_callback_slot.clone(),
                                };
                                // _slot_guard is dropped at end-of-block (after run_web_turn returns or
                                // panics), restoring slot to None.

                                // Phase 36.17.10 Plan 05: read auto_tts fresh per turn
                                // for hot-reload (a web write takes effect without restart).
                                // Falls back to startup snapshot on load error.
                                // Precedent: toggle_skill (api.rs) uses Config::load() fresh.
                                let auto_tts = ironhermes_core::config::Config::load()
                                    .map(|c| c.voice.auto_tts)
                                    .unwrap_or_else(|_| app_state.config().voice.auto_tts);

                                // Phase 36.17.7 D-02-a: construct per-turn WebAudioDispatcher
                                // and TTS wiring so TextToSpeechTool emits AudioOut WS frames.
                                // audio_dispatcher: Some(...) for BOTH Mode A and Mode B —
                                // Mode A's tool-driven TTS must keep working regardless.
                                let audio_tx = tx.clone();
                                let audio_cache_dir = ironhermes_core::constants::get_hermes_home()
                                    .join("audio_cache");
                                let web_audio_dispatcher = std::sync::Arc::new(
                                    crate::server::web_audio_dispatcher::WebAudioDispatcher::new(
                                        audio_tx,
                                        audio_cache_dir,
                                    ),
                                );
                                // Phase 01-04 (DLV-03 web): per-turn WebImageDispatcher so
                                // extracted photo `<MEDIA:>` refs are delivered as ImageOut
                                // binary frames. Mirrors the audio dispatcher above; reads
                                // from get_hermes_home()/cache/images (written by image_gen).
                                let image_tx = tx.clone();
                                let images_cache_dir = ironhermes_core::constants::get_hermes_home()
                                    .join("cache")
                                    .join("images");
                                let web_image_dispatcher =
                                    crate::server::web_image_dispatcher::WebImageDispatcher::new(
                                        image_tx,
                                        images_cache_dir,
                                    );
                                // Phase 36.3.3 (D-08 web): per-turn WebVideoDispatcher so
                                // extracted video `<MEDIA:>` refs are delivered as VideoOut
                                // binary frames. Mirrors WebImageDispatcher above; reads
                                // from get_hermes_home()/cache/videos (written by video_gen).
                                // Cap: configurable via config.video_gen.max_inline_bytes
                                // (WR-01), default 50MB (D-07), not 20MB (Pitfall 4).
                                let video_tx = tx.clone();
                                let videos_cache_dir = ironhermes_core::constants::get_hermes_home()
                                    .join("cache")
                                    .join("videos");
                                // WR-01: use the configured inline cap; fall back to the
                                // dispatcher's default const if the config value is 0 (unset).
                                let video_size_cap = match app_state.config().video_gen.max_inline_bytes {
                                    0 => crate::server::web_video_dispatcher::VIDEO_SIZE_CAP,
                                    cap => cap,
                                };
                                let web_video_dispatcher =
                                    crate::server::web_video_dispatcher::WebVideoDispatcher::new(
                                        video_tx,
                                        videos_cache_dir,
                                        video_size_cap,
                                    );
                                let tts_wiring = Some(ironhermes_agent::TtsPerTurnWiring {
                                    session_key: web_key(&session_id_for_turn), // D-05 session key
                                    audio_dispatcher: Some(
                                        web_audio_dispatcher.clone()
                                            as std::sync::Arc<
                                                dyn ironhermes_tools::AudioDispatcher,
                                            >,
                                    ),
                                });
                                // Phase 36.3.8 D-04: web v1 clarify path — dispatcher=None
                                // (ClarifyTool falls back to stdout numbered list); shared
                                // clarify_registry Arc threads through AppState so a future
                                // WS round-trip resolver can reuse the same awaiter map.
                                let messaging_wiring =
                                    Some(ironhermes_agent::MessagingPerTurnWiring {
                                        session_key: web_key(&session_id_for_turn),
                                        message_dispatcher: None,
                                        clarify_dispatcher: None,
                                        clarify_registry: app_state
                                            .web_clarify_registry
                                            .clone(),
                                        cancel_token: Some(cancel_token_spawn.clone()),
                                    });

                                // Phase 54.3 (D-07): typed yolo-bypass notice channel for THIS
                                // turn. Built from the turn's own sender exactly like the tool
                                // callbacks above; one summary per bypassing turn becomes one
                                // YoloBypassNotice event. Never routed through `stream_callback`.
                                let notice_callback = web_notice_sink(tx.clone(), tid, session_id_for_turn.clone());

                                let result = app_state
                                    .run_web_turn(
                                        &session_id_for_turn,
                                        // Phase 41.1 Plan 03 (D-06): `turn_input` == the user's
                                        // message for normal chat, or the resolved skill's
                                        // trigger_text for a one-shot `/<skill>` run.
                                        &turn_input,
                                        stream_callback,
                                        Some(tool_progress_callback),
                                        Some(tool_result_callback),
                                        tts_wiring,
                                        // Phase 39.2 Plan 04: pass TurnRegistry UUID for bb correlation.
                                        Some(turn_id_spawn),
                                        messaging_wiring,
                                        // Phase 46.7 Plan 04 (D-09): this is the only call site fed by
                                        // a live ChatRequest, so it is the only one that ever carries
                                        // non-empty attachment_ids.
                                        attachment_ids,
                                        Some(notice_callback),
                                    )
                                    .await;

                                // Phase 34a MEM-READ-05: flush scrubber tail after stream ends.
                                let tail = scrubber_ws.lock().unwrap().flush();
                                if !tail.is_empty() {
                                    // Tail may still carry a (partial) MEDIA tag — feed it
                                    // through the extractor so it is stripped, not emitted.
                                    let visible = media_extractor_ws.lock().unwrap().feed(&tail);
                                    if !visible.is_empty() {
                                        let _ = tx.send(ChatStreamEvent::Delta { turn_id: tid, text: visible });
                                    }
                                }

                                // Phase 01-04 (DLV-03 web): flush the MEDIA extractor tail and
                                // dispatch each extracted photo as an ImageOut frame. Only
                                // Photo-kind refs with a local Path source are dispatched
                                // (image_gen always emits a local cache path). Non-photo or
                                // URL refs are ignored on the web path (T-04-02: only
                                // server-extracted, dispatcher-issued frames render as <img>).
                                //
                                // fix(47): track every local media path dispatched from the
                                // model's own stream so the deterministic post-turn tool-result
                                // scan (in the Ok arm below) never double-delivers a path the
                                // model already echoed as a bare <MEDIA:> tag.
                                let mut dispatched_media: std::collections::HashSet<
                                    std::path::PathBuf,
                                > = std::collections::HashSet::new();
                                {
                                    let media_tail = media_extractor_ws.lock().unwrap().flush_tail();
                                    if !media_tail.is_empty() {
                                        let _ = tx.send(ChatStreamEvent::Delta {
                                            turn_id: tid,
                                            text: media_tail,
                                        });
                                    }
                                    let attachments =
                                        media_extractor_ws.lock().unwrap().take_attachments();
                                    for media_ref in &attachments {
                                        // Phase 01-04 (DLV-03 web): dispatch Photo refs as ImageOut.
                                        if media_ref.kind
                                            == ironhermes_gateway::media_tag::MediaKind::Photo
                                        {
                                            if let ironhermes_gateway::media_tag::MediaSource::Path(p) =
                                                &media_ref.source
                                            {
                                                // fix(47): mark as delivered so the post-turn
                                                // tool-result scan does not re-dispatch it.
                                                dispatched_media.insert(p.clone());
                                                if let Err(e) =
                                                    web_image_dispatcher.send_image_file(p).await
                                                {
                                                    tracing::warn!(
                                                        error = %e,
                                                        "WebImageDispatcher: failed to dispatch ImageOut frame"
                                                    );
                                                }
                                            }
                                        }
                                        // Phase 36.3.3 (D-08 web): dispatch Video refs as VideoOut.
                                        // ADDITIVE — does not replace the Photo arm above (Pitfall 5).
                                        if media_ref.kind
                                            == ironhermes_gateway::media_tag::MediaKind::Video
                                        {
                                            if let ironhermes_gateway::media_tag::MediaSource::Path(p) =
                                                &media_ref.source
                                            {
                                                // fix(47): mark as delivered (see Photo arm).
                                                dispatched_media.insert(p.clone());
                                                if let Err(e) =
                                                    web_video_dispatcher.send_video_file(p).await
                                                {
                                                    tracing::warn!(
                                                        error = %e,
                                                        "WebVideoDispatcher: failed to dispatch VideoOut frame"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }

                                match result {
                                    Ok(agent_result) => {
                                        // fix(47): deterministic media delivery. Some chat models
                                        // wrap the <MEDIA:> tag in a code fence (which the stream
                                        // extractor intentionally passes through as literal text)
                                        // or reword/drop it, so the image/video never renders. The
                                        // image_gen / video tools ALWAYS emit a bare <MEDIA: /path>
                                        // in their tool-result text, so dispatch any local media
                                        // this turn's tool results reference that the model's own
                                        // stream did not already deliver (deduped by path).
                                        for (p, kind) in undelivered_tool_result_media(
                                            &agent_result.appended,
                                            &dispatched_media,
                                        ) {
                                            dispatched_media.insert(p.clone());
                                            match kind {
                                                ironhermes_gateway::media_tag::MediaKind::Photo => {
                                                    if let Err(e) =
                                                        web_image_dispatcher.send_image_file(&p).await
                                                    {
                                                        tracing::warn!(
                                                            error = %e,
                                                            "WebImageDispatcher: failed to dispatch tool-result ImageOut frame"
                                                        );
                                                    }
                                                }
                                                ironhermes_gateway::media_tag::MediaKind::Video => {
                                                    if let Err(e) =
                                                        web_video_dispatcher.send_video_file(&p).await
                                                    {
                                                        tracing::warn!(
                                                            error = %e,
                                                            "WebVideoDispatcher: failed to dispatch tool-result VideoOut frame"
                                                        );
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                        // Phase 36.17.10 Plan 05 — Mode B: auto-speak the
                                        // assistant's final reply via WebAudioDispatcher/AudioOut.
                                        // Fires BEFORE Finished so the client can play audio
                                        // while the turn is still "in flight". Note: if the
                                        // agent already called the TTS tool this turn, Mode B
                                        // produces one additional auto-speak of the final reply
                                        // text — acceptable per spec (Mode B = speak every reply).
                                        if should_auto_speak(auto_tts) {
                                            let reply_text =
                                                assistant_reply_text(&agent_result);
                                            if !reply_text.is_empty() {
                                                // Phase 40.5 Plan 08 (D-11): pass frozen identity
                                                // so auto_speak_reply selects the per-identity voice.
                                                auto_speak_reply(
                                                    &reply_text,
                                                    &web_audio_dispatcher,
                                                    req_active_identity.as_deref(),
                                                )
                                                .await;
                                            }
                                        }
                                        let _ = tx.send(ChatStreamEvent::Finished { turn_id: tid,
                                            total_tokens: agent_result.total_usage.total_tokens
                                                as u32,
                                        });
                                    }
                                    Err(e) => {
                                        // fix: surface the FULL error chain (provider status +
                                        // response body), not just the top-level context, so a
                                        // provider 4xx (e.g. a rejected vision request) is
                                        // diagnosable instead of an opaque "Streaming LLM call
                                        // failed". Also log server-side for web.log capture.
                                        tracing::error!(turn_id = %tid, error = ?e, "web chat turn failed");
                                        let _ = tx.send(ChatStreamEvent::Error { turn_id: tid,
                                            message: format!("Agent error: {e:#}"),
                                        });
                                    }
                                }

                                // Deregister from the process-wide registry (register-before-spawn).
                                registry_spawn.deregister(turn_id_spawn).await;
                                // Suppress unused-variable warning for cancel token (held for RAII cancel).
                                drop(cancel_token_spawn);
                            });

                            stream_map.insert(turn_id, UnboundedReceiverStream::new(rx));
                            in_flight_turns.insert(turn_id, InFlightTurn {
                                turn_id,
                                session_id,
                                rx: {
                                    // rx was moved into stream_map above; store a sentinel placeholder.
                                    // The actual drain is via stream_map in the maybe_event arm.
                                    // We need the field only for session_id lookup + abort.
                                    // Use a dummy channel that is immediately closed.
                                    let (_, dummy_rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                                    dummy_rx
                                },
                                handle,
                                _per_permit: {
                                    // Permit was moved into the spawned task for RAII.
                                    // Create a dummy placeholder — the real permit lives in the task.
                                    // Safety: we use a fresh semaphore with 1 permit so the drop
                                    // of this placeholder never affects the real semaphores.
                                    let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
                                    s.try_acquire_owned().unwrap()
                                },
                                _global_permit: {
                                    let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
                                    s.try_acquire_owned().unwrap()
                                },
                                cancel: cancel_token,
                            });
                        }

                        // ── STT transcript injection (Plan 06 D-13/D-14) ─────────
                        // Received when the async STT pipeline finishes for a binary
                        // audio frame. Process the transcript exactly like a user-typed
                        // ChatRequest — uses the same semaphore gate + register-before-spawn.
                        stt_turn = rx_stt.recv() => {
                            // Phase 40.5 Plan 08 (D-17): destructure identity from channel tuple.
                            if let Some((stt_session_id, stt_transcript, stt_active_identity)) = stt_turn {
                                // Phase 36.17.9: echo the transcript to the client BEFORE the
                                // turn runs so it renders as a user bubble.
                                if let Ok(json) = serde_json::to_string(
                                    &ChatStreamEvent::UserTranscript {
                                        text: stt_transcript.clone(),
                                    },
                                ) {
                                    let _ = socket.send_raw(Message::Text(json)).await;
                                }

                                let app_state_s = app_state.clone();
                                let session_id_s = stt_session_id.clone();
                                let session_id_for_turn_s = stt_session_id.clone();
                                let message_s = stt_transcript.clone();

                                // Phase 39.1 Plan 02: semaphore gate for STT turn.
                                match app_state.concurrency.try_acquire() {
                                    None => {
                                        // At capacity — enqueue the transcript.
                                        let key = web_key(&session_id_s);
                                        let paused_flag =
                                            app_state.get_or_create_paused_flag(&session_id_s);
                                        let paused_snapshot =
                                            paused_flag.load(std::sync::atomic::Ordering::SeqCst);
                                        let (tx_q, rx_q) =
                                            tokio::sync::mpsc::unbounded_channel::<ChatStreamEvent>();
                                        match app_state.queue.try_push(&key, message_s.clone()) {
                                            Ok(()) => {
                                                let depth =
                                                    app_state.queue.len(&key) as u32;
                                                let _ = tx_q.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                    text: format!(
                                                        "Queued: \"{}\" ({} in queue)\n",
                                                        message_s, depth
                                                    ),
                                                });
                                                let _ = tx_q.send(ChatStreamEvent::QueueUpdated {
                                                    depth,
                                                    paused: paused_snapshot,
                                                });
                                            }
                                            Err(
                                                ironhermes_core::queue::QueueError::CapacityReached {
                                                    max,
                                                    ..
                                                },
                                            ) => {
                                                let _ = tx_q.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(),
                                                    text: format!(
                                                        "Queue is full ({max}/{max}). /stop or /flush to drain.\n"
                                                    ),
                                                });
                                            }
                                        }
                                        let _ = tx_q.send(ChatStreamEvent::Finished { turn_id: uuid::Uuid::nil(),
                                            total_tokens: 0,
                                        });
                                        drop(tx_q);
                                        let mut qrx = rx_q;
                                        while let Some(ev) = qrx.recv().await {
                                            let json =
                                                serde_json::to_string(&ev).unwrap_or_default();
                                            let _ =
                                                socket.send_raw(Message::Text(json)).await;
                                        }
                                    }
                                    Some((per_permit_s, global_permit_s)) => {
                                        // Capacity available — register-before-spawn.
                                        let (tx_s, rx_s) =
                                            tokio::sync::mpsc::unbounded_channel::<ChatStreamEvent>();
                                        let turn_id_s = TurnId::new_v4();
                                        let cancel_s = tokio_util::sync::CancellationToken::new();
                                        app_state.turn_registry.register(TurnEntry {
                                            turn_id: turn_id_s,
                                            session_id: session_id_s.clone(),
                                            surface: Surface::Web,
                                            started_at: std::time::Instant::now(),
                                            cancel: cancel_s.clone(),
                                        }).await;
                                        let stt_index = in_flight_turns.len() as u32;
                                        if let Ok(json) = serde_json::to_string(&ChatStreamEvent::TurnStarted {
                                            turn_id: turn_id_s,
                                            session_id: session_id_s.clone(),
                                            index: stt_index,
                                        }) {
                                            let _ = socket.send_raw(Message::Text(json)).await;
                                        }
                                        let registry_s = app_state.turn_registry.clone();
                                        let cancel_s_spawn = cancel_s.clone();
                                        let handle_s = tokio::spawn(async move {
                                            let _per = per_permit_s;
                                            let _global = global_permit_s;
                                            let tid_s = turn_id_s;
                                            // Phase 57 D-04(b): the transcript is user text too.
                                            if !web_inbound_gate(
                                                &app_state_s,
                                                &tx_s,
                                                tid_s,
                                                &session_id_for_turn_s,
                                                &message_s,
                                            )
                                            .await
                                            {
                                                registry_s.deregister(tid_s).await;
                                                return;
                                            }
                                            let scrubber_ws =
                                                std::sync::Arc::new(std::sync::Mutex::new(
                                                    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
                                                ));
                                            let scrubber_ws_cb = std::sync::Arc::clone(&scrubber_ws);
                                            let tx_stream = tx_s.clone();
                                            let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                                                Box::new(move |delta: &str| {
                                                    let visible =
                                                        scrubber_ws_cb.lock().unwrap().feed(delta);
                                                    if !visible.is_empty() {
                                                        let _ = tx_stream.send(
                                                            ChatStreamEvent::Delta { turn_id: tid_s, text: visible },
                                                        );
                                                    }
                                                });
                                            let tx_tool = tx_s.clone();
                                            let tool_progress_callback: ironhermes_agent::agent_loop::ToolProgressCallback =
                                                // Phase 57 A3: the arguments are never sent to the browser.
                                                Box::new(move |name: &str, _args: &str| {
                                                    let _ = tx_tool.send(
                                                        ChatStreamEvent::ToolCallStart { turn_id: tid_s,
                                                            name: name.to_string(),
                                                        },
                                                    );
                                                });
                                            let tx_tool_result = tx_s.clone();
                                            let sid_tool_result = session_id_for_turn_s.clone();
                                            let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                                Box::new(move |name: &str, success: bool, _output: &str| {
                                                    let _ = tx_tool_result.send(
                                                        ChatStreamEvent::ToolCallEnd { turn_id: tid_s,
                                                            name: name.to_string(),
                                                            success,
                                                        },
                                                    );
                                                    // Phase 57 D-17: a failed call turns this turn's dot red.
                                                    if let Some(sev) = tool_failure_severity(tid_s, &sid_tool_result, success) {
                                                        let _ = tx_tool_result.send(sev);
                                                    }
                                                });
                                            let tx_subagent = tx_s.clone();
                                            {
                                                let mut guard =
                                                    app_state_s.subagent_callback_slot.lock().await;
                                                *guard = Some(tx_subagent);
                                            }
                                            let _slot_guard = SubagentCallbackSlotGuard {
                                                slot: app_state_s.subagent_callback_slot.clone(),
                                            };
                                            let auto_tts_s =
                                                ironhermes_core::config::Config::load()
                                                    .map(|c| c.voice.auto_tts)
                                                    .unwrap_or_else(|_| {
                                                        app_state_s.config().voice.auto_tts
                                                    });

                                            let audio_tx = tx_s.clone();
                                            let audio_cache_dir =
                                                ironhermes_core::constants::get_hermes_home()
                                                    .join("audio_cache");
                                            let web_audio_dispatcher = std::sync::Arc::new(
                                                crate::server::web_audio_dispatcher::WebAudioDispatcher::new(
                                                    audio_tx,
                                                    audio_cache_dir,
                                                ),
                                            );
                                            let tts_wiring =
                                                Some(ironhermes_agent::TtsPerTurnWiring {
                                                    session_key: web_key(&session_id_for_turn_s),
                                                    audio_dispatcher: Some(
                                                        web_audio_dispatcher.clone()
                                                            as std::sync::Arc<
                                                                dyn ironhermes_tools::AudioDispatcher,
                                                            >,
                                                    ),
                                                });
                                            // Phase 36.3.8 D-04: web v1 clarify path.
                                            let messaging_wiring_s =
                                                Some(ironhermes_agent::MessagingPerTurnWiring {
                                                    session_key: web_key(&session_id_for_turn_s),
                                                    message_dispatcher: None,
                                                    clarify_dispatcher: None,
                                                    clarify_registry: app_state_s
                                                        .web_clarify_registry
                                                        .clone(),
                                                    cancel_token: Some(cancel_s_spawn.clone()),
                                                });
                                            // Phase 54.3 (D-07): typed yolo-bypass notice channel for
                                            // this STT turn, built from this site's own sender.
                                            let notice_callback = web_notice_sink(tx_s.clone(), tid_s, session_id_for_turn_s.clone());
                                            let result = app_state_s
                                                .run_web_turn(
                                                    &session_id_for_turn_s,
                                                    &message_s,
                                                    stream_callback,
                                                    Some(tool_progress_callback),
                                                    Some(tool_result_callback),
                                                    tts_wiring,
                                                    // Phase 39.2 Plan 04: TurnRegistry UUID for bb correlation.
                                                    Some(tid_s),
                                                    messaging_wiring_s,
                                                    // Phase 46.7 Plan 04 (D-09): STT-transcript turns never
                                                    // carry attachments (queue stores text only).
                                                    Vec::new(),
                                                    Some(notice_callback),
                                                )
                                                .await;
                                            let tail = scrubber_ws.lock().unwrap().flush();
                                            if !tail.is_empty() {
                                                let _ = tx_s.send(ChatStreamEvent::Delta { turn_id: tid_s,
                                                    text: tail,
                                                });
                                            }
                                            match result {
                                                Ok(agent_result) => {
                                                    if should_auto_speak(auto_tts_s) {
                                                        let reply_text =
                                                            assistant_reply_text(&agent_result);
                                                        if !reply_text.is_empty() {
                                                            // Phase 40.5 Plan 08 (D-11): pass frozen identity
                                                            // from the STT frame via tx_stt channel (D-12).
                                                            auto_speak_reply(
                                                                &reply_text,
                                                                &web_audio_dispatcher,
                                                                stt_active_identity.as_deref(),
                                                            )
                                                            .await;
                                                        }
                                                    }
                                                    let _ = tx_s.send(ChatStreamEvent::Finished { turn_id: tid_s,
                                                        total_tokens: agent_result
                                                            .total_usage
                                                            .total_tokens
                                                            as u32,
                                                    });
                                                }
                                                Err(e) => {
                                                    let _ = tx_s.send(ChatStreamEvent::Error { turn_id: tid_s,
                                                        message: format!("Agent error: {e:#}"),
                                                    });
                                                }
                                            }
                                            registry_s.deregister(tid_s).await;
                                            drop(cancel_s_spawn);
                                        });
                                        stream_map.insert(turn_id_s, UnboundedReceiverStream::new(rx_s));
                                        in_flight_turns.insert(turn_id_s, InFlightTurn {
                                            turn_id: turn_id_s,
                                            session_id: session_id_s,
                                            rx: { let (_, d) = mpsc::unbounded_channel::<ChatStreamEvent>(); d },
                                            handle: handle_s,
                                            _per_permit: { let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1)); s.try_acquire_owned().unwrap() },
                                            _global_permit: { let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1)); s.try_acquire_owned().unwrap() },
                                            cancel: cancel_s,
                                        });
                                    }
                                }
                            }
                        }

                        // ── Agent stream events → client (StreamMap drain) ────────
                        // Phase 39.1 Plan 02 (R39.1-01): StreamMap polls all in-flight
                        // turn channels concurrently. Each item is (TurnId, ChatStreamEvent).
                        // When a channel closes (task dropped its tx), StreamMap removes it
                        // and returns None for that key — we deregister + emit TurnEnded.
                        Some((done_turn_id, event)) = stream_map.next(), if !stream_map.is_empty() => {
                            // Phase 36.17.7 D-02-b: AudioOut → Binary frame; everything else → Text.
                            // Phase 01-04 (DLV-03 web): ImageOut also rides a Binary frame
                            // (mirrors AudioOut); the client builds a Blob URL + <img>.
                            // Phase 36.3.3 (D-08 web): VideoOut rides a Binary frame too
                            // (mirrors ImageOut); the client builds a Blob URL + <video controls>.
                            let ws_msg = match &event {
                                ChatStreamEvent::AudioOut { .. }
                                | ChatStreamEvent::ImageOut { .. }
                                | ChatStreamEvent::VideoOut { .. } => {
                                    Message::Binary(serde_json::to_vec(&event).unwrap_or_default().into())
                                }
                                _ => {
                                    Message::Text(serde_json::to_string(&event).unwrap_or_default())
                                }
                            };
                            if let Err(err) = socket.send_raw(ws_msg).await {
                                // Send failed — abort all in-flight turns and close.
                                warn!(
                                    reason = %err,
                                    in_flight = in_flight_turns.len(),
                                    "websocket send failed; aborting all in-flight turns"
                                );
                                for (_, turn) in in_flight_turns.drain() {
                                    turn.cancel.cancel();
                                    turn.handle.abort();
                                }
                                send_close_frame(&mut socket, CloseCode::Away, "send failed").await;
                                break;
                            }
                            // Check if this turn's channel has been exhausted (task completed).
                            // StreamMap::next() returns None when the inner stream ends — but
                            // we receive events one at a time, so we detect completion by checking
                            // whether the turn's JoinHandle is finished after each event.
                            if let Some(turn) = in_flight_turns.get(&done_turn_id) {
                                if turn.handle.is_finished() {
                                    if let Some(finished_turn) = in_flight_turns.remove(&done_turn_id) {
                                        let sid = finished_turn.session_id.clone();
                                        // Await the handle to surface any panic.
                                        if let Err(err) = finished_turn.handle.await {
                                            warn!(
                                                session_id = %sid,
                                                reason = %err,
                                                "turn task join failed"
                                            );
                                        }
                                        // Emit TurnEnded (R39.1-08).
                                        if let Ok(json) = serde_json::to_string(&ChatStreamEvent::TurnEnded {
                                            turn_id: done_turn_id,
                                            session_id: sid.clone(),
                                        }) {
                                            let _ = socket.send_raw(Message::Text(json)).await;
                                        }
                                        // Phase 36.17.4 (D-02): queue drain — same logic as before.
                                        let key = web_key(&sid);
                                        let paused_now = app_state
                                            .get_or_create_paused_flag(&sid)
                                            .load(std::sync::atomic::Ordering::SeqCst);
                                        if !paused_now {
                                            if let Some(next_text) = app_state.queue.pop(&key) {
                                                let depth_after = app_state.queue.len(&key) as u32;
                                                let qu_event = ChatStreamEvent::QueueUpdated {
                                                    depth: depth_after,
                                                    paused: false,
                                                };
                                                let _ = socket.send_raw(Message::Text(
                                                    serde_json::to_string(&qu_event).unwrap_or_default(),
                                                )).await;
                                                // Re-acquire permits for the queue-drain turn.
                                                if let Some((per_drain, global_drain)) = app_state.concurrency.try_acquire() {
                                                    let (tx_drain, rx_drain) = mpsc::unbounded_channel::<ChatStreamEvent>();
                                                    let app_state_drain = app_state.clone();
                                                    let session_id_spawn = sid.clone();
                                                    let (next_text_owned, drain_openui) =
                                                        split_queued_message(next_text);
                                                    let drain_turn_id = TurnId::new_v4();
                                                    let cancel_drain = tokio_util::sync::CancellationToken::new();
                                                    app_state.turn_registry.register(TurnEntry {
                                                        turn_id: drain_turn_id,
                                                        session_id: sid.clone(),
                                                        surface: Surface::Web,
                                                        started_at: std::time::Instant::now(),
                                                        cancel: cancel_drain.clone(),
                                                    }).await;
                                                    let drain_index = in_flight_turns.len() as u32;
                                                    if let Ok(json) = serde_json::to_string(&ChatStreamEvent::TurnStarted {
                                                        turn_id: drain_turn_id,
                                                        session_id: sid.clone(),
                                                        index: drain_index,
                                                    }) {
                                                        let _ = socket.send_raw(Message::Text(json)).await;
                                                    }
                                                    let registry_drain = app_state.turn_registry.clone();
                                                    let cancel_drain_spawn = cancel_drain.clone();
                                                    let drain_handle = tokio::spawn(async move {
                                                        let _per = per_drain;
                                                        let _global = global_drain;
                                                        let tid_drain = drain_turn_id;
                                                        // Phase 57 D-04(b): a queued message (a `/queue`
                                                        // argument included) is screened when it RUNS.
                                                        if !web_inbound_gate(
                                                            &app_state_drain,
                                                            &tx_drain,
                                                            tid_drain,
                                                            &session_id_spawn,
                                                            &next_text_owned,
                                                        )
                                                        .await
                                                        {
                                                            registry_drain.deregister(tid_drain).await;
                                                            return;
                                                        }
                                                        // RR-02: a queued OpenUI turn gets its overlay here,
                                                        // after the gate, like a live one.
                                                        if drain_openui {
                                                            app_state_drain.push_web_skill_overlay(
                                                                &session_id_spawn,
                                                                "openui".to_string(),
                                                                super::openui_route::openui_overlay_body(),
                                                            );
                                                        }
                                                        let scrubber_ws = std::sync::Arc::new(std::sync::Mutex::new(
                                                            ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new(),
                                                        ));
                                                        let scrubber_ws_cb = std::sync::Arc::clone(&scrubber_ws);
                                                        let tx_stream = tx_drain.clone();
                                                        let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
                                                            Box::new(move |delta: &str| {
                                                                let visible = scrubber_ws_cb.lock().unwrap().feed(delta);
                                                                if !visible.is_empty() {
                                                                    let _ = tx_stream.send(ChatStreamEvent::Delta { turn_id: tid_drain, text: visible });
                                                                }
                                                            });
                                                        let tx_tool = tx_drain.clone();
                                                        let tool_progress_callback: ironhermes_agent::agent_loop::ToolProgressCallback =
                                                            // Phase 57 A3: the arguments are never sent to the browser.
                                                            Box::new(move |name: &str, _args: &str| {
                                                                let _ = tx_tool.send(ChatStreamEvent::ToolCallStart { turn_id: tid_drain, name: name.to_string() });
                                                            });
                                                        let tx_tool_result = tx_drain.clone();
                                                        let sid_tool_result = session_id_spawn.clone();
                                                        let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                                            Box::new(move |name: &str, success: bool, _output: &str| {
                                                                let _ = tx_tool_result.send(ChatStreamEvent::ToolCallEnd { turn_id: tid_drain, name: name.to_string(), success });
                                                                // Phase 57 D-17: a failed call turns this turn's dot red.
                                                                if let Some(sev) = tool_failure_severity(tid_drain, &sid_tool_result, success) {
                                                                    let _ = tx_tool_result.send(sev);
                                                                }
                                                            });
                                                        let tx_subagent = tx_drain.clone();
                                                        { let mut guard = app_state_drain.subagent_callback_slot.lock().await; *guard = Some(tx_subagent); }
                                                        let _slot_guard = SubagentCallbackSlotGuard { slot: app_state_drain.subagent_callback_slot.clone() };
                                                        let auto_tts_drain = ironhermes_core::config::Config::load()
                                                            .map(|c| c.voice.auto_tts)
                                                            .unwrap_or_else(|_| app_state_drain.config().voice.auto_tts);
                                                        let audio_tx_drain = tx_drain.clone();
                                                        let audio_cache_dir_drain = ironhermes_core::constants::get_hermes_home().join("audio_cache");
                                                        let web_audio_dispatcher_drain = std::sync::Arc::new(
                                                            crate::server::web_audio_dispatcher::WebAudioDispatcher::new(audio_tx_drain, audio_cache_dir_drain),
                                                        );
                                                        let tts_wiring_drain = Some(ironhermes_agent::TtsPerTurnWiring {
                                                            session_key: web_key(&session_id_spawn),
                                                            audio_dispatcher: Some(web_audio_dispatcher_drain.clone() as std::sync::Arc<dyn ironhermes_tools::AudioDispatcher>),
                                                        });
                                                        // Phase 36.3.8 D-04: web v1 clarify path.
                                                        let messaging_wiring_drain =
                                                            Some(ironhermes_agent::MessagingPerTurnWiring {
                                                                session_key: web_key(&session_id_spawn),
                                                                message_dispatcher: None,
                                                                clarify_dispatcher: None,
                                                                clarify_registry: app_state_drain
                                                                    .web_clarify_registry
                                                                    .clone(),
                                                                cancel_token: Some(cancel_drain_spawn.clone()),
                                                            });
                                                        // Phase 54.3 (D-07): typed yolo-bypass notice channel for
                                                        // this queue-drain turn, built from this site's own sender.
                                                        let notice_callback = web_notice_sink(tx_drain.clone(), tid_drain, session_id_spawn.clone());
                                                        let result = app_state_drain.run_web_turn(
                                                            &session_id_spawn, &next_text_owned,
                                                            stream_callback, Some(tool_progress_callback), Some(tool_result_callback), tts_wiring_drain,
                                                            // Phase 39.2 Plan 04: TurnRegistry UUID for bb correlation.
                                                            Some(tid_drain),
                                                            messaging_wiring_drain,
                                                            // Phase 46.7 Plan 04 (D-09): queue-drain turns never carry
                                                            // attachments (queue stores text only).
                                                            Vec::new(),
                                                            Some(notice_callback),
                                                        ).await;
                                                        let tail = scrubber_ws.lock().unwrap().flush();
                                                        if !tail.is_empty() {
                                                            let _ = tx_drain.send(ChatStreamEvent::Delta { turn_id: tid_drain, text: tail });
                                                        }
                                                        match result {
                                                            Ok(agent_result) => {
                                                                if should_auto_speak(auto_tts_drain) {
                                                                    let reply_text = assistant_reply_text(&agent_result);
                                                                    // Phase 40.5 Plan 08 (D-11): queue-drain turns have no
                                                                    // identity (queue stores text only) — fall back to global TTS.
                                                                    if !reply_text.is_empty() { auto_speak_reply(&reply_text, &web_audio_dispatcher_drain, None).await; }
                                                                }
                                                                let _ = tx_drain.send(ChatStreamEvent::Finished { turn_id: tid_drain, total_tokens: agent_result.total_usage.total_tokens as u32 });
                                                            }
                                                            Err(e) => {
                                                                let _ = tx_drain.send(ChatStreamEvent::Error { turn_id: tid_drain, message: format!("Agent error: {e:#}") });
                                                            }
                                                        }
                                                        registry_drain.deregister(tid_drain).await;
                                                        drop(cancel_drain_spawn);
                                                    });
                                                    stream_map.insert(drain_turn_id, UnboundedReceiverStream::new(rx_drain));
                                                    in_flight_turns.insert(drain_turn_id, InFlightTurn {
                                                        turn_id: drain_turn_id,
                                                        session_id: sid,
                                                        rx: { let (_, d) = mpsc::unbounded_channel::<ChatStreamEvent>(); d },
                                                        handle: drain_handle,
                                                        _per_permit: { let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1)); s.try_acquire_owned().unwrap() },
                                                        _global_permit: { let s = std::sync::Arc::new(tokio::sync::Semaphore::new(1)); s.try_acquire_owned().unwrap() },
                                                        cancel: cancel_drain,
                                                    });
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // ── Process-local security bus (Phase 57 D-16 JobBlocked) ──
                        // Cancel-safe `recv`. Only `JobBlocked` maps to a row; every other
                        // kind is dropped here because in-turn kinds arrive through the
                        // turn's own gate (forwarding them would show them twice).
                        bus_event = security_bus.recv(), if security_bus_open => {
                            match bus_event {
                                Ok(sec) => {
                                    if let Some(out) = process_event_to_stream(&sec) {
                                        let json = serde_json::to_string(&out).unwrap_or_default();
                                        let _ = socket.send_raw(Message::Text(json)).await;
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                    warn!(
                                        skipped,
                                        "security bus receiver lagged; some blocked-job rows were skipped"
                                    );
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                    security_bus_open = false;
                                }
                            }
                        }

                        // ── Keepalive Ping ────────────────────────────────────────
                        _ = keepalive.tick() => {
                            if let Err(err) = socket
                                .send_raw(Message::Ping(Bytes::new()))
                                .await
                            {
                                let in_flight = !in_flight_turns.is_empty();
                                let session_id = in_flight_turns
                                    .values()
                                    .next()
                                    .map(|t| t.session_id.as_str())
                                    .unwrap_or("unknown");
                                warn!(
                                    session_id = %session_id,
                                    reason = %err,
                                    in_flight,
                                    "websocket keepalive ping failed; closing connection"
                                );
                                for (_, turn) in in_flight_turns.drain() {
                                    turn.cancel.cancel();
                                    turn.handle.abort();
                                }
                                send_close_frame(
                                    &mut socket,
                                    CloseCode::Away,
                                    "keepalive failed",
                                )
                                .await;
                                break;
                            }
                        }
                    }
                }

                // 49.8-08 (D-10/D-16): every break path above (Close frame,
                // recv error, keepalive failure) falls through to here, so a
                // closed browser tab or dropped connection is a real session
                // boundary — stop the robot and cancel any goals. Awaited,
                // and bounded by end_session_stop's own session-end timeout
                // (see ironhermes_tools::ros2::session), so an unreachable
                // bridge can never wedge the socket's own teardown.
                let ros2_report =
                    ironhermes_tools::ros2::end_session_stop(app_state.runtime.registry()).await;
                if ros2_report != "nothing to stop" {
                    info!(report = %ros2_report, "web chat ws teardown: ros2 session-end stop");
                }
                }

                #[cfg(not(feature = "server"))]
                {
                    let unavailable = ChatStreamEvent::Error { turn_id: uuid::Uuid::nil(),
                        message: "Websocket chat route is unavailable without `server` feature"
                            .to_string(),
                    };
                    let _ = socket
                        .send_raw(Message::Text(
                            serde_json::to_string(&unavailable).unwrap_or_default(),
                        ))
                        .await;
                }
            }
        },
    ))
}

// =============================================================================
// Phase 36.17.10 Plan 05 — TTS Mode A/B helpers
//
// These helpers implement the per-turn auto-speak gate (Mode B) for the web
// surface. They are `#[cfg(not(target_arch = "wasm32"))]`-gated because they
// use server-side TTS synthesis (disk I/O, network calls to TTS providers).
//
// should_auto_speak: thin decision fn — one documented control point.
// assistant_reply_text: extract the assistant's final reply from AgentResult.
// auto_speak_reply: synthesize text and emit AudioOut via WebAudioDispatcher.
// =============================================================================

/// Phase 36.17.10 Plan 05 (VOICE-02): decide whether Mode B auto-speak fires.
///
/// Thin wrapper around the boolean so there is a single documented decision
/// point and the unit test expresses intent, not just a bool comparison.
/// Mode A (auto_tts=false): returns false — tool-driven TTS only.
/// Mode B (auto_tts=true):  returns true  — auto-speak every reply.
#[cfg(not(target_arch = "wasm32"))]
fn should_auto_speak(auto_tts: bool) -> bool {
    auto_tts
}

/// Phase 36.17.10 Plan 05 (VOICE-02): extract the assistant's final reply text.
///
/// Concatenates all `Role::Assistant` messages from `result.appended` in
/// insertion order, using `MessageContent::as_text()` to handle both plain-text
/// and multi-part content. Returns an empty string when there are no assistant
/// messages (never panics). The caller guards against empty text before synthesizing.
#[cfg(not(target_arch = "wasm32"))]
fn assistant_reply_text(result: &ironhermes_agent::AgentResult) -> String {
    result
        .appended
        .iter()
        .filter(|m| m.role == ironhermes_core::types::Role::Assistant)
        .filter_map(|m| m.content.as_ref().and_then(|c| c.as_text()))
        .collect::<Vec<_>>()
        .join(" ")
}

/// fix(47): deterministic web media delivery. Scans this turn's tool-result
/// messages for the bare `<MEDIA: /path>` tags that `image_gen` and the video
/// tools always emit, returning the local Photo/Video paths that were NOT
/// already delivered from the model's own stream (`already`).
///
/// This decouples rendering from whether the model echoes the tag: some chat
/// models wrap it in a code fence (which the stream `MediaTagExtractor`
/// intentionally passes through as literal text) or reword/drop it, so the
/// image never renders. The tool-result text is authored by the tool, never
/// fenced, so it always yields the path. Only `Role::Tool` messages are trusted
/// (an assistant message that fenced the tag is ignored here); URL-form refs are
/// skipped, matching the stream path — only local cache paths become binary
/// frames. Dedup is by path, both against `already` and within this scan.
#[cfg(not(target_arch = "wasm32"))]
fn undelivered_tool_result_media(
    appended: &[ironhermes_core::types::ChatMessage],
    already: &std::collections::HashSet<std::path::PathBuf>,
) -> Vec<(
    std::path::PathBuf,
    ironhermes_gateway::media_tag::MediaKind,
)> {
    let mut out = Vec::new();
    let mut seen = already.clone();
    for msg in appended {
        if msg.role != ironhermes_core::types::Role::Tool {
            continue;
        }
        let Some(text) = msg.content_text() else {
            continue;
        };
        let mut extractor = ironhermes_gateway::media_tag::MediaTagExtractor::new();
        let _ = extractor.feed(text);
        let _ = extractor.flush_tail();
        for media_ref in extractor.take_attachments() {
            if let ironhermes_gateway::media_tag::MediaSource::Path(p) = media_ref.source {
                // `insert` returns false when the path was already delivered
                // (from the stream this turn, or an earlier tool result).
                if seen.insert(p.clone()) {
                    out.push((p, media_ref.kind));
                }
            }
        }
    }
    out
}

/// Phase 54.7 (D-09/D-18/D-19): synthesize `text` with `cfg`'s TTS settings into
/// `audio_dir` and return the written path.
///
/// The file extension comes from [`ironhermes_tools::tts::audio_format::extension_for`]
/// (what the provider actually returns), so the dispatcher can serve a matching
/// MIME. `active_identity` is validated against the known presets before it can
/// influence provider selection (T-54.7-46); an unknown slug uses the global
/// config. `None` on any failure; failures log the category only (T-54.7-45).
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
pub(crate) async fn synthesize_reply_audio(
    cfg: &ironhermes_core::config::Config,
    text: &str,
    active_identity: Option<&str>,
    audio_dir: &std::path::Path,
) -> Option<std::path::PathBuf> {
    use ironhermes_core::voice_http::{VoiceErrorCategory, VoiceHttpError};
    use ironhermes_tools::tts::{audio_format, build_tts_registry_with_vault};

    // Phase 40.5 Plan 08 (T-40.5-08-01): validate the wire-supplied slug server-side.
    // A crafted/unknown slug must not reach provider selection — apply the same
    // is_known_identity gate used on the client write path (defense in depth).
    let validated_identity = active_identity
        .filter(|slug| crate::components::hermes_app::avatar_logic::is_known_identity(slug));

    // Phase 40.5 Plan 08 (D-11): resolve per-identity TTS config; falls back to
    // the global TTS config when the identity is None or unknown.
    let effective = cfg.effective_tts_config_for_identity(validated_identity);

    let registry = build_tts_registry_with_vault(
        &effective,
        ironhermes_core::voice_http::voice_vault_config(cfg),
    );
    let provider = match registry.get(&effective.provider) {
        Some(p) => p,
        None => {
            tracing::debug!(
                provider = %effective.provider,
                "auto_speak_reply: TTS provider not registered; skipping Mode B synthesis"
            );
            return None;
        }
    };

    if let Err(e) = std::fs::create_dir_all(audio_dir) {
        tracing::warn!(
            error = %e,
            "auto_speak_reply: failed to create audio_cache dir; skipping synthesis"
        );
        return None;
    }
    let output_path = audio_dir.join(format!(
        "{}.{}",
        uuid::Uuid::new_v4(),
        audio_format::extension_for(&effective)
    ));

    // Synthesize. Errors (network failure, provider unavailable) are non-fatal;
    // only the category is logged (a provider error text may carry a body).
    match provider.synthesize(text, &output_path).await {
        Ok(p) => Some(p),
        Err(e) => {
            let category = e
                .downcast_ref::<VoiceHttpError>()
                .map(|v| v.category)
                .unwrap_or(VoiceErrorCategory::UnexpectedResponse);
            tracing::warn!(
                category = category.as_str(),
                "auto_speak_reply: TTS synthesis failed; skipping Mode B AudioOut"
            );
            None
        }
    }
}

/// Phase 36.17.10 Plan 05 (VOICE-02): synthesize `text` and dispatch AudioOut.
///
/// Uses the current on-disk config to select the TTS provider (hot-reload:
/// a config write between turns is picked up here). Writes to a temporary path
/// in `$IRONHERMES_HOME/audio_cache/` then calls `WebAudioDispatcher::send_audio_file`
/// which emits a `ChatStreamEvent::AudioOut` binary WS frame to the browser.
///
/// Errors are logged and silently swallowed — a TTS failure must never prevent
/// the `Finished` event from being sent (the turn completed successfully).
/// Phase 40.5 Plan 08 (D-11): `active_identity` selects the per-identity TTS provider/voice
/// via `Config::effective_tts_config_for_identity`. Pass `None` to use the global TTS config.
///
/// Gated on `feature = "server"` as well as the target: the
/// `crate::server::web_audio_dispatcher` module in this signature is
/// `#[cfg(feature = "server")]`, so a target-only gate left this function
/// compiled without its dependency under native + default features.
#[cfg(all(not(target_arch = "wasm32"), feature = "server"))]
async fn auto_speak_reply(
    text: &str,
    dispatcher: &crate::server::web_audio_dispatcher::WebAudioDispatcher,
    active_identity: Option<&str>,
) {
    use ironhermes_core::constants::get_hermes_home;
    use ironhermes_tools::AudioDispatcher as _;

    // Re-read config fresh so the provider choice reflects the latest web write.
    let config = match ironhermes_core::config::Config::load() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "auto_speak_reply: config load failed; skipping Mode B synthesis"
            );
            return;
        }
    };

    let audio_dir = get_hermes_home().join("audio_cache");
    let Some(written_path) =
        synthesize_reply_audio(&config, text, active_identity, &audio_dir).await
    else {
        return;
    };

    // Dispatch as AudioOut WS frame via the already-constructed WebAudioDispatcher.
    if let Err(e) = dispatcher.send_audio_file("web", &written_path, None).await {
        tracing::warn!(
            error = %e,
            "auto_speak_reply: AudioDispatcher dispatch failed"
        );
    }
}

/// Phase 41.1 Plan 03 (SKILL-13 web / D-06): the plan for a one-shot web skill
/// run, produced by [`plan_web_skill_run`] and consumed by the WS SKILL-13
/// NotFound fallback. Pure, identity-free data — the WS handler owns turn
/// identity (session_id / TurnRegistry), never this struct (T-41.1-03-01).
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct WebSkillRun {
    /// The text submitted as the run turn: the resolved skill's `trigger_text`
    /// (bare → run-now instruction, argued → the user's verbatim trailing text).
    pub turn_input: String,
    /// Normalized (registry) skill name — activated into the session overlay.
    pub skill_name: String,
    /// SKILL.md body from `SkillRegistry::read_content` (SKILL-07-scanned) —
    /// prepended to the run turn's system prompt via `PromptBuilder::activate_skill`.
    pub skill_body: String,
    /// Phase 41.1 Plan 03 (UI-SPEC §C): the DIM run-turn meta chip copy —
    /// `▶ Ran skill /{name}` (bare) or `▶ Ran skill /{name} · "{args≤40}…"`
    /// (argued). Emitted as a `RunTurnMeta` event before the reply streams.
    pub meta_chip: String,
}

/// Phase 41.1 Plan 03 (UI-SPEC §C / Copywriting Contract): build the DIM
/// run-turn meta chip copy. Bare invoke → `▶ Ran skill /{name}`; argued invoke
/// → `▶ Ran skill /{name} · "{args}"`, with `args` truncated to 40 chars
/// (char-safe) and an inner `…` appended only when truncated. Mirrors the TUI
/// tracer's `run_turn_meta_chip` (tui_rata/app.rs) verbatim so both surfaces
/// render identical copy.
#[cfg(not(target_arch = "wasm32"))]
fn run_turn_meta_chip(name: &str, args_display: Option<&str>) -> String {
    match args_display {
        None => format!("▶ Ran skill /{name}"),
        Some(args) => {
            const MAX: usize = 40;
            let mut chars = args.chars();
            let head: String = chars.by_ref().take(MAX).collect();
            let truncated = chars.next().is_some();
            if truncated {
                format!("▶ Ran skill /{name} · \"{head}…\"")
            } else {
                format!("▶ Ran skill /{name} · \"{head}\"")
            }
        }
    }
}

/// Phase 41.1 Plan 03 (SKILL-13 web / D-06): decide whether a slash `input`
/// that fell through command resolution is a dynamic-skill invocation, and if
/// so what turn to run. Thin wrapper over the shared, pure
/// [`resolve_skill_invocation`] resolver (Plan 01) — the same resolver every
/// surface uses — so the Web fallback stays behavior-identical to the TUI
/// tracer and the gateway. Returns `None` for a non-skill token (caller keeps
/// today's chat-passthrough).
///
/// This is the testable seam that proves Web fires a REAL turn: a `Some` result
/// carries the exact `turn_input` submitted to `run_web_turn` (a genuine agent
/// turn), never a Debug-formatted string.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn plan_web_skill_run(
    registry: &ironhermes_core::skills::SkillRegistry,
    input: &str,
) -> Option<WebSkillRun> {
    let inv = ironhermes_core::commands::skill_dispatch::resolve_skill_invocation(registry, input)?;
    // Bare vs argued for the meta chip: the resolver sets `trigger_text` to the
    // bare run-now instruction for a bare invoke, else the user's verbatim
    // trailing text. Reuse that (robust to aliases — no re-derivation from the
    // raw token) to pick the chip's argued form.
    let bare_instruction =
        format!("Run the {} skill now: carry out its instructions immediately.", inv.name);
    let args_display = if inv.trigger_text == bare_instruction {
        None
    } else {
        Some(inv.trigger_text.as_str())
    };
    let meta_chip = run_turn_meta_chip(&inv.name, args_display);
    Some(WebSkillRun {
        turn_input: inv.trigger_text,
        skill_name: inv.name,
        skill_body: inv.body,
        meta_chip,
    })
}

/// Phase 57 UI-SPEC C2: what a scanner outage did to the call it affected, stated in the
/// `sidecar_unavailable` row so the operator reads what actually happened.
#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarOutcome {
    /// The call was blocked (levels 1-3 for a Warn-class call, or a floor Critical at any
    /// level, D-07).
    Blocked,
    /// The call was parked for operator approval.
    HeldForApproval,
    /// The call ran without approval at a bypass-active level (4-5), audited.
    AuditedBypass,
    /// The outage changed nothing for this tool class; only the built-in rules applied.
    BuiltInRulesOnly,
}

#[cfg(feature = "server")]
impl SidecarOutcome {
    /// The clause that follows the tool name in the row summary.
    pub(crate) fn phrase(self) -> &'static str {
        match self {
            Self::Blocked => "was blocked",
            Self::HeldForApproval => "was held for approval",
            Self::AuditedBypass => "ran without approval (audited bypass)",
            Self::BuiltInRulesOnly => "ran with the built-in rules only",
        }
    }

    /// The severity this outcome gives the turn's dot (R5: a resulting block is red).
    pub(crate) fn worst(self) -> crate::protocol::TurnWorst {
        match self {
            Self::Blocked => crate::protocol::TurnWorst::Red,
            _ => crate::protocol::TurnWorst::Amber,
        }
    }
}

/// Why a `SidecarUnavailable` row exists, read from the producer's `<reason>; outcome ...`
/// summary (GC-WR-03). Only a real outage says the scanner is down: a reason where the
/// sidecar answered but refused the subject (`timeout`, `killed_for_timeouts`, `too_large`,
/// `protocol_error`, `unsupported_subject`) is a subject that could not be screened.
#[cfg(feature = "server")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarCause {
    /// The sidecar binary is not installed.
    Missing,
    /// The sidecar process is gone or not answering. Also the fallback for a reason word
    /// that does not parse.
    NotResponding,
    /// The sidecar refused this subject; it did not go down.
    NotScreened(ironhermes_security::UnavailableReason),
}

/// Classify the reason word before the first `;` in `summary`, with
/// `UnavailableReason::from_word`. The producer's text itself is never copied into a row.
#[cfg(feature = "server")]
pub(crate) fn sidecar_reason_phrase(summary: &str) -> SidecarCause {
    use ironhermes_security::UnavailableReason;

    let word = summary.split(';').next().unwrap_or("").trim();
    match UnavailableReason::from_word(word) {
        Some(UnavailableReason::Missing) => SidecarCause::Missing,
        Some(reason) if !reason.is_outage() => SidecarCause::NotScreened(reason),
        _ => SidecarCause::NotResponding,
    }
}

/// What the outage did, from plan 07's `<reason>; outcome <token>` summary. Without a
/// recognised token the D-12 Warn-class default applies: blocked at levels 1-3, an audited
/// bypass at 4-5.
#[cfg(feature = "server")]
pub(crate) fn sidecar_outcome(summary: &str, level: u8) -> SidecarOutcome {
    let token = summary
        .split("outcome ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map(|t| t.trim_matches(|c: char| !c.is_ascii_alphanumeric()));
    match token {
        Some("blocked") => SidecarOutcome::Blocked,
        Some("approval") => SidecarOutcome::HeldForApproval,
        Some("bypass") => SidecarOutcome::AuditedBypass,
        Some("allowed") => SidecarOutcome::BuiltInRulesOnly,
        _ if level <= 3 => SidecarOutcome::Blocked,
        _ => SidecarOutcome::AuditedBypass,
    }
}

/// The text that follows the word "blocked" in a Block row, naming the cause (G-2).
///
/// Precedence: named rule ids (` by rule {ids}`), then the regex floor
/// (` by the built-in rules`), then a subject the sidecar could not screen
/// (`: it could not be screened ({reason})`), then a scanner outage
/// (`: the security scanner is {missing|not responding}`), and last, for a producer that
/// sent no ids at all, ` by the security policy`. Cause markers are core constants. A
/// reason word is rendered only after it parses as an `UnavailableReason`, and named ids
/// pass `sanitize_notice_field`, so no producer text reaches the row unchecked.
#[cfg(feature = "server")]
fn blocked_clause(rule_ids: &[String]) -> String {
    use ironhermes_core::content_screen::{
        REGEX_FLOOR_RULE_ID, SIDECAR_OUTAGE_RULE_PREFIX, UNSCREENED_RULE_PREFIX,
    };
    use ironhermes_security::UnavailableReason;

    let is_marker = |r: &str| {
        r == REGEX_FLOOR_RULE_ID
            || r.starts_with(UNSCREENED_RULE_PREFIX)
            || r.starts_with(SIDECAR_OUTAGE_RULE_PREFIX)
    };
    let named = rule_ids
        .iter()
        .filter(|r| !is_marker(r))
        .map(|r| ironhermes_core::sanitize_notice_field(r))
        .filter(|r| !r.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    if !named.is_empty() {
        return format!(" by rule {named}");
    }
    if rule_ids.iter().any(|r| r == REGEX_FLOOR_RULE_ID) {
        return " by the built-in rules".to_string();
    }
    if let Some(word) = rule_ids
        .iter()
        .find_map(|r| r.strip_prefix(UNSCREENED_RULE_PREFIX))
    {
        return match UnavailableReason::from_word(word) {
            Some(reason) => format!(": it could not be screened ({reason})"),
            None => ": it could not be screened".to_string(),
        };
    }
    if let Some(word) = rule_ids
        .iter()
        .find_map(|r| r.strip_prefix(SIDECAR_OUTAGE_RULE_PREFIX))
    {
        return match UnavailableReason::from_word(word) {
            Some(UnavailableReason::Missing) => ": the security scanner is missing".to_string(),
            _ => ": the security scanner is not responding".to_string(),
        };
    }
    " by the security policy".to_string()
}

/// Phase 57 D-16/D-17, UI-SPEC C2 and R4/R5: map one in-turn [`SecurityEvent`] to the wire
/// events for turn `turn_id`: a `TurnSeverity` for the dot, plus a transcript
/// `CriticalEvent` only for the kinds that render a row. Approval events (the card), a
/// turn-ending tool failure (the Error bubble), `DefconChanged` (the web slash path) and
/// `JobBlocked` (the process bus) never yield a row here, so nothing is shown twice. The
/// producer's `summary` is only scanned for a reason word and an outcome token; command
/// text never reaches a row.
#[cfg(feature = "server")]
pub(crate) fn security_event_to_stream(
    turn_id: uuid::Uuid,
    session_id: &str,
    ev: &ironhermes_core::SecurityEvent,
) -> Vec<ChatStreamEvent> {
    use crate::protocol::TurnWorst;
    use ironhermes_core::SecurityEventKind as K;

    let sev = |worst: TurnWorst| ChatStreamEvent::TurnSeverity {
        turn_id,
        session_id: session_id.to_string(),
        worst,
    };
    let crit = |kind: &str, summary: String| ChatStreamEvent::CriticalEvent {
        turn_id,
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        summary,
    };
    let tool = ironhermes_core::sanitize_notice_field(&ev.tool);
    match ev.kind {
        K::Blocked => vec![
            sev(TurnWorst::Red),
            crit(
                "blocked",
                format!(
                    "{tool} was blocked{}. It did not run.",
                    blocked_clause(&ev.rule_ids)
                ),
            ),
        ],
        K::SidecarUnavailable => {
            let outcome = sidecar_outcome(&ev.summary, ev.level);
            let text = match sidecar_reason_phrase(&ev.summary) {
                SidecarCause::Missing => {
                    format!(
                        "The security scanner is missing. {tool} {}.",
                        outcome.phrase()
                    )
                }
                SidecarCause::NotResponding => format!(
                    "The security scanner is not responding. {tool} {}.",
                    outcome.phrase()
                ),
                SidecarCause::NotScreened(reason) => {
                    format!(
                        "{tool} could not be screened ({reason}) and {}.",
                        outcome.phrase()
                    )
                }
            };
            vec![sev(outcome.worst()), crit("sidecar_unavailable", text)]
        }
        K::CredentialDetected => vec![
            sev(TurnWorst::Amber),
            crit(
                "credential_detected",
                format!("A credential was found in {tool}. The value is hidden."),
            ),
        ],
        K::WarnRan | K::BypassRan => vec![sev(TurnWorst::Amber)],
        K::ToolFailedTurnEnded => vec![sev(TurnWorst::Red)],
        K::ApprovalRequested | K::ApprovalResolved | K::DefconChanged | K::JobBlocked => Vec::new(),
    }
}

/// Phase 57 D-16 / UI-SPEC C2, R6: map an event from the process-local security bus to its
/// wire event. Only `JobBlocked` (a cron prompt or kanban task blocked outside any turn,
/// raised in this process by plan 11) is forwarded: an in-turn kind arrives through the
/// turn's own gate, so forwarding it here would show it twice. The row is built from the
/// job label (`ev.summary`: a cron job's name or id, a kanban task's id; plan 11 never
/// puts prompt or task text there) and the rule ids only. It is a nil-turn row with no
/// session, attached to no dot.
#[cfg(feature = "server")]
pub(crate) fn process_event_to_stream(
    ev: &ironhermes_core::SecurityEvent,
) -> Option<ChatStreamEvent> {
    if ev.kind != ironhermes_core::SecurityEventKind::JobBlocked {
        return None;
    }
    let label = ironhermes_core::sanitize_notice_field(ev.summary.trim());
    let label = label.trim();
    let label = if label.is_empty() { "(unnamed)" } else { label };
    Some(ChatStreamEvent::CriticalEvent {
        turn_id: uuid::Uuid::nil(),
        session_id: String::new(),
        kind: "job_blocked".to_string(),
        summary: format!(
            "Scheduled job {label} was blocked{}. It did not run.",
            blocked_clause(&ev.rule_ids)
        ),
    })
}

/// Phase 57 R8 / D-10 / D-16: the critical row for a web `/yolo <N>` that changed the
/// session's level, `DEFCON {old} to {new}, set from {surface} by {actor}.`. `None` when the
/// level did not change. The slash dispatch passes the registry level read before and after
/// the command, so a refused (auth off, flag off), invalid or same-level `/yolo` never
/// produces a row (T-57-79).
#[cfg(feature = "server")]
pub(crate) fn defcon_changed_event(
    session_id: &str,
    old: ironhermes_core::config::DefconLevel,
    new: ironhermes_core::config::DefconLevel,
    surface: &str,
    actor: &str,
) -> Option<ChatStreamEvent> {
    if old == new {
        return None;
    }
    Some(ChatStreamEvent::CriticalEvent {
        turn_id: uuid::Uuid::nil(),
        session_id: session_id.to_string(),
        kind: "defcon_changed".to_string(),
        summary: format!(
            "DEFCON {} to {}, set from {} by {}.",
            old.level(),
            new.level(),
            ironhermes_core::sanitize_notice_field(surface),
            ironhermes_core::sanitize_notice_field(actor),
        ),
    })
}

#[cfg(test)]
#[cfg(feature = "server")]
mod tts_mode_tests {
    //! Phase 36.17.10 Plan 05 (VOICE-02) — unit tests for TTS Mode A/B gate.
    //!
    //! Tests are `#[cfg(feature = "server")]`-gated so they only run in the
    //! server build, matching the `#[cfg(not(target_arch = "wasm32"))]` helpers.

    use super::{assistant_reply_text, should_auto_speak};
    use ironhermes_agent::agent_loop::StopReason;
    use ironhermes_agent::{AgentResult, AggregatedUsage};
    use ironhermes_core::types::{ChatMessage, MessageContent, Role};

    /// Construct a minimal AgentResult for testing assistant_reply_text.
    fn make_result(messages: Vec<ChatMessage>) -> AgentResult {
        AgentResult {
            messages: vec![],
            appended: messages,
            turns_used: 1,
            finished_naturally: true,
            final_response: None,
            total_usage: AggregatedUsage::default(),
            compression_count_after: 0,
            stop_reason: StopReason::Natural,
            context_warnings: vec![],
        }
    }

    fn assistant_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Assistant,
            content: Some(MessageContent::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }
    }

    fn user_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: Some(MessageContent::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        }
    }

    /// Mode A (auto_tts=false): should_auto_speak returns false.
    #[test]
    fn mode_a_does_not_auto_speak() {
        assert!(
            !should_auto_speak(false),
            "Mode A (auto_tts=false) must never auto-speak"
        );
    }

    /// Mode B (auto_tts=true): should_auto_speak returns true.
    #[test]
    fn mode_b_does_auto_speak() {
        assert!(
            should_auto_speak(true),
            "Mode B (auto_tts=true) must always auto-speak"
        );
    }

    /// assistant_reply_text extracts only assistant messages.
    #[test]
    fn reply_text_returns_assistant_content() {
        let result = make_result(vec![
            user_msg("hello"),
            assistant_msg("Hi there!"),
            assistant_msg("How can I help?"),
        ]);
        let text = assistant_reply_text(&result);
        assert!(
            text.contains("Hi there!"),
            "must include first assistant message"
        );
        assert!(
            text.contains("How can I help?"),
            "must include second assistant message"
        );
        assert!(!text.contains("hello"), "must not include user message");
    }

    /// assistant_reply_text returns empty string when no assistant messages.
    #[test]
    fn reply_text_empty_when_no_assistant_messages() {
        let result = make_result(vec![user_msg("hello")]);
        let text = assistant_reply_text(&result);
        assert!(
            text.is_empty(),
            "must return empty string when no assistant messages (no panic)"
        );
    }

    /// assistant_reply_text returns empty string for empty appended.
    #[test]
    fn reply_text_empty_on_empty_result() {
        let result = make_result(vec![]);
        let text = assistant_reply_text(&result);
        assert!(
            text.is_empty(),
            "must return empty string for empty appended (no panic)"
        );
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod plan_26_7_1_02_tests {
    use crate::protocol::ChatStreamEvent;
    use ironhermes_tools::delegate_task::{SubagentProgress, SubagentProgressCallback};
    use std::sync::Arc;
    use tokio::sync::{mpsc, Mutex};

    /// Phase 26.7.1 Plan 02 (Wave 0): D-06 callback wiring shape.
    /// Mirrors the callback constructed in state.rs Task 2: lock the slot,
    /// read Some(tx), send ChatStreamEvent::SubagentEvent {}.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_subagent_callback_emits_event() {
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
        let slot: Arc<Mutex<Option<mpsc::UnboundedSender<ChatStreamEvent>>>> =
            Arc::new(Mutex::new(Some(tx)));
        let cb_slot = slot.clone();
        let cb: SubagentProgressCallback =
            Arc::new(move |_index: usize, _event: SubagentProgress| {
                if let Ok(guard) = cb_slot.try_lock() {
                    if let Some(s) = guard.as_ref() {
                        let _ = s.send(ChatStreamEvent::SubagentEvent {});
                    }
                }
            });

        // Invoke the callback as the delegate-task runner would.
        cb(0, SubagentProgress::Completed);

        let received = rx.recv().await.expect("expected SubagentEvent");
        assert!(
            matches!(received, ChatStreamEvent::SubagentEvent {}),
            "callback must send the SubagentEvent variant"
        );

        // After clearing the slot, the callback becomes a silent no-op.
        {
            let mut g = slot.lock().await;
            *g = None;
        }
        cb(1, SubagentProgress::Completed);
        // Nothing should arrive — give the runtime a moment to surface anything.
        // Accept either: Err(Elapsed) = timeout (slot None, channel still open),
        // or Ok(None) = channel closed (all senders dropped when slot cleared).
        // Both mean no SubagentEvent was sent by the second cb invocation.
        let timed = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
        let no_spurious_event = match timed {
            Err(_) => true,       // timeout — nothing in channel
            Ok(None) => true,     // channel closed — all senders dropped
            Ok(Some(_)) => false, // unexpected event sent after slot was cleared
        };
        assert!(
            no_spurious_event,
            "no events should be received after slot is cleared"
        );
    }
}

// =============================================================================
// Phase 36.17.9 Plan 04 (D-12 / T-36.17.9-04-01 / T-36.17.9-04-03)
// wake_word_matches predicate — ReDoS-safe contains() match, no regex.
// =============================================================================

#[cfg(test)]
#[cfg(feature = "server")]
mod wake_word_tests {
    use super::wake_word_matches;

    /// Phase 36.17.9 (D-12): case-insensitive contains match.
    ///
    /// T-36.17.9-04-01 ReDoS: predicate uses to_lowercase().contains() — no Regex.
    /// T-36.17.9-04-03 empty-phrase guard: empty/whitespace phrase → false.
    #[test]
    fn test_wake_word_match() {
        // Match: transcript contains phrase (case-insensitive).
        assert!(
            wake_word_matches("Hey Hermes, what's up", "hey hermes"),
            "D-12: phrase found case-insensitively must return true"
        );

        // Match: all-uppercase transcript vs lower-case phrase.
        assert!(
            wake_word_matches("HEY HERMES NOW PLEASE", "hey hermes"),
            "D-12: upper-case transcript must match lower-case phrase"
        );

        // No match: transcript does not contain phrase.
        assert!(
            !wake_word_matches("hello there", "hey hermes"),
            "D-12: unrelated transcript must return false"
        );

        // No match: empty phrase (T-36.17.9-04-03 guard).
        assert!(
            !wake_word_matches("hey hermes", ""),
            "T-36.17.9-04-03: empty phrase must return false (never trivially true)"
        );

        // No match: whitespace-only phrase (T-36.17.9-04-03 guard).
        assert!(
            !wake_word_matches("hey hermes", "   "),
            "T-36.17.9-04-03: whitespace-only phrase must return false"
        );

        // Match: mixed-case phrase vs mixed-case transcript.
        assert!(
            wake_word_matches("Ok Hermes go ahead", "ok hermes"),
            "D-12: mixed-case transcript must match lower-case phrase"
        );
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod plan_39_1_02_tests {
    //! Phase 39.1 Plan 02 (R39.1-01/R39.1-03/R39.1-06): concurrent turn tests.
    //!
    //! These tests exercise the ConcurrencyLayer + TurnRegistry primitives
    //! directly without requiring a live AppState (which needs a full DB and
    //! config). The WS handler logic is integration-tested via the native and
    //! wasm32 build gates above; these unit tests cover the concurrency
    //! primitives that the handler delegates to.

    use ironhermes_core::{ConcurrencyLayer, Surface, TurnEntry, TurnId, TurnRegistry};
    use std::sync::Arc;
    use std::time::Instant;
    use tokio_util::sync::CancellationToken;

    /// Helper: build a TurnEntry for Surface::Web.
    fn web_entry(session_id: &str) -> TurnEntry {
        TurnEntry {
            turn_id: TurnId::new_v4(),
            session_id: session_id.to_string(),
            surface: Surface::Web,
            started_at: Instant::now(),
            cancel: CancellationToken::new(),
        }
    }

    /// R39.1-03: try_acquire returns Some when capacity is available.
    #[test]
    fn concurrency_layer_try_acquire_succeeds_when_capacity_available() {
        let layer = ConcurrencyLayer::new(3, 32);
        let result = layer.try_acquire();
        assert!(result.is_some(), "must succeed when semaphore has capacity");
    }

    /// R39.1-03: try_acquire returns None when per-session cap is exhausted.
    #[test]
    fn concurrency_layer_try_acquire_fails_when_session_cap_exhausted() {
        let layer = ConcurrencyLayer::new(2, 32);
        // Exhaust per-session cap (hold permits in scope).
        let _p1 = layer.try_acquire().expect("first acquire must succeed");
        let _p2 = layer.try_acquire().expect("second acquire must succeed");
        // Cap exhausted — third must fail.
        let result = layer.try_acquire();
        assert!(
            result.is_none(),
            "must fail when per-session cap is exhausted"
        );
    }

    /// R39.1-03: permits release when dropped, restoring capacity.
    #[test]
    fn concurrency_layer_permits_release_on_drop() {
        let layer = ConcurrencyLayer::new(1, 32);
        {
            let _p = layer.try_acquire().expect("first acquire must succeed");
            // Capacity exhausted inside this block.
            assert!(
                layer.try_acquire().is_none(),
                "must be exhausted while permit held"
            );
        }
        // Permit dropped — capacity restored.
        assert!(
            layer.try_acquire().is_some(),
            "must succeed after permit is dropped"
        );
    }

    /// R39.1-09: TurnRegistry register + deregister is reflected in count_session.
    #[tokio::test]
    async fn registry_register_deregister_updates_session_count() {
        let registry = Arc::new(TurnRegistry::new());
        let entry = web_entry("sess-1");
        let turn_id = entry.turn_id;

        assert_eq!(registry.count_session("sess-1").await, 0, "initially empty");
        registry.register(entry).await;
        assert_eq!(registry.count_session("sess-1").await, 1, "after register");
        registry.deregister(turn_id).await;
        assert_eq!(
            registry.count_session("sess-1").await,
            0,
            "after deregister"
        );
    }

    /// R39.1-06: concurrent turns from different sessions do not interfere.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_turns_two_sessions_independent() {
        let layer = ConcurrencyLayer::new(3, 32);
        let registry = Arc::new(TurnRegistry::new());

        // Acquire permits for two concurrent turns in different logical sessions.
        let (per1, glob1) = layer.try_acquire().expect("session A turn 1 must succeed");
        let (per2, glob2) = layer.try_acquire().expect("session B turn 1 must succeed");

        let entry_a = web_entry("sess-A");
        let entry_b = web_entry("sess-B");
        let id_a = entry_a.turn_id;
        let id_b = entry_b.turn_id;

        registry.register(entry_a).await;
        registry.register(entry_b).await;

        assert_eq!(registry.count_session("sess-A").await, 1);
        assert_eq!(registry.count_session("sess-B").await, 1);
        assert_eq!(registry.list_all().await.len(), 2);

        // Simulate completion: release permits + deregister.
        drop(per1);
        drop(glob1);
        drop(per2);
        drop(glob2);
        registry.deregister(id_a).await;
        registry.deregister(id_b).await;

        assert_eq!(
            registry.list_all().await.len(),
            0,
            "registry must be empty after both deregister"
        );
    }

    /// R39.1-08: cancel_session signals all turns for that session only.
    #[tokio::test]
    async fn cancel_session_signals_only_target_session() {
        let registry = Arc::new(TurnRegistry::new());

        let entry_a = web_entry("sess-cancel");
        let entry_b = web_entry("sess-keep");
        let cancel_a = entry_a.cancel.clone();
        let cancel_b = entry_b.cancel.clone();

        registry.register(entry_a).await;
        registry.register(entry_b).await;

        let cancelled = registry.cancel_session("sess-cancel").await;
        assert_eq!(cancelled, 1, "only 1 turn in sess-cancel");
        assert!(
            cancel_a.is_cancelled(),
            "sess-cancel turn must be cancelled"
        );
        assert!(
            !cancel_b.is_cancelled(),
            "sess-keep turn must NOT be cancelled"
        );
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod media_delivery_tests {
    //! fix(47) — deterministic web media delivery from tool-result text.
    //!
    //! `undelivered_tool_result_media` is the pure core of the ws turn loop's
    //! post-turn media dispatch: it decides WHICH local paths to render as
    //! ImageOut/VideoOut frames, independent of whether the model echoed (or
    //! fenced, or dropped) the `<MEDIA:>` tag in its visible reply.
    use super::undelivered_tool_result_media;
    use ironhermes_core::types::{ChatMessage, MessageContent, Role};
    use ironhermes_gateway::media_tag::MediaKind;
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn tool_msg(text: &str) -> ChatMessage {
        ChatMessage {
            role: Role::Tool,
            content: Some(MessageContent::Text(text.to_string())),
            tool_calls: None,
            tool_call_id: Some("call_1".into()),
            name: None,
            is_recall_context: false,
        }
    }

    /// The image_gen tool result (`Generated your image.\n<MEDIA: /path>`) yields
    /// the local photo path — this is the deterministic path that renders even
    /// when the model never echoes the tag.
    #[test]
    fn extracts_bare_media_path_from_tool_result() {
        let msgs = vec![tool_msg("Generated your image.\n<MEDIA: /cache/a.webp>")];
        let out = undelivered_tool_result_media(&msgs, &HashSet::new());
        assert_eq!(out, vec![(PathBuf::from("/cache/a.webp"), MediaKind::Photo)]);
    }

    /// A path the model echoed bare (already dispatched from the stream) must not
    /// be delivered a second time from the tool-result scan.
    #[test]
    fn dedupes_against_already_delivered() {
        let msgs = vec![tool_msg("<MEDIA: /cache/a.webp>")];
        let mut already = HashSet::new();
        already.insert(PathBuf::from("/cache/a.webp"));
        let out = undelivered_tool_result_media(&msgs, &already);
        assert!(out.is_empty(), "stream-delivered path must not repeat");
    }

    /// Only tool-authored text is trusted. An assistant message that fenced the
    /// tag (the exact bug this fixes) is NOT re-scanned here — deterministic
    /// delivery comes from the tool result, not the model's prose.
    #[test]
    fn ignores_assistant_messages_even_when_fenced() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: Some(MessageContent::Text(
                "```\n<MEDIA: /cache/a.webp>\n```".to_string(),
            )),
            tool_calls: None,
            tool_call_id: None,
            name: None,
            is_recall_context: false,
        };
        let out = undelivered_tool_result_media(&[msg], &HashSet::new());
        assert!(out.is_empty(), "assistant text is not a media source here");
    }

    /// Video tool results classify as Video (dispatched via WebVideoDispatcher).
    #[test]
    fn classifies_video_paths() {
        let msgs = vec![tool_msg("<MEDIA: /cache/v.mp4>")];
        let out = undelivered_tool_result_media(&msgs, &HashSet::new());
        assert_eq!(out, vec![(PathBuf::from("/cache/v.mp4"), MediaKind::Video)]);
    }

    /// The same path appearing in two tool results this turn delivers once.
    #[test]
    fn dedupes_repeat_within_same_turn() {
        let msgs = vec![
            tool_msg("<MEDIA: /cache/a.webp>"),
            tool_msg("<MEDIA: /cache/a.webp>"),
        ];
        let out = undelivered_tool_result_media(&msgs, &HashSet::new());
        assert_eq!(out.len(), 1, "duplicate path across tool results delivers once");
    }

    /// URL-form refs are skipped (parity with the stream path — only local cache
    /// files become binary frames).
    #[test]
    fn skips_url_form_refs() {
        let msgs = vec![tool_msg("<MEDIA: https://example.com/a.webp>")];
        let out = undelivered_tool_result_media(&msgs, &HashSet::new());
        assert!(out.is_empty(), "URL refs are not dispatched as local frames");
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod skill_run_web_tests {
    //! Phase 41.1 Plan 03 (SKILL-13 web / D-06) — unit tests for the Web
    //! one-shot activate+run DECISION seam (`plan_web_skill_run`).
    //!
    //! These mirror the Plan 02 TUI tracer's discipline: they assert a REAL
    //! turn is submitted (the `turn_input` that `run_web_turn` runs), never a
    //! produced/Debug-formatted string. The full end-to-end WS turn-spawn is
    //! covered by manual UAT (no ws-socket harness exists) — see SUMMARY.

    use super::{plan_web_skill_run, run_turn_meta_chip};
    use ironhermes_core::skills::SkillRegistry;
    use std::fs;
    use tempfile::tempdir;

    /// Build an isolated registry with a single `gsd-config` skill. The TempDir
    /// MUST outlive use: `read_content` reads the SKILL.md body from disk.
    fn test_registry() -> (tempfile::TempDir, SkillRegistry) {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("skills");
        let skill_dir = skills_dir.join("gsd-config");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: gsd-config\ndescription: Configure GSD\n---\nSKILL BODY CONTENT",
        )
        .unwrap();
        let registry = SkillRegistry::load_with_paths(&[skills_dir]);
        (dir, registry)
    }

    /// A bare `/<skill>` produces a REAL run turn: `turn_input` is the run-now
    /// instruction (a genuine agent turn), and the body/name are carried for the
    /// one-shot overlay — proving Web fires a turn, not a Debug string.
    #[test]
    fn bare_invoke_plans_a_real_run_turn() {
        let (_dir, registry) = test_registry();
        let run = plan_web_skill_run(&registry, "/gsd-config").expect("registered skill");
        assert_eq!(run.skill_name, "gsd-config");
        assert_eq!(
            run.turn_input, "Run the gsd-config skill now: carry out its instructions immediately.",
            "bare invoke submits the run-now instruction as the real turn"
        );
        assert!(
            run.skill_body.contains("SKILL BODY CONTENT"),
            "body must come from SkillRegistry::read_content, not a second path"
        );
        // The turn_input is a genuine submittable turn, NOT a Debug-formatted
        // CommandResult — it never contains the leak-shape `SkillActivated {`.
        assert!(!run.turn_input.contains("SkillActivated"));
        // Bare invoke → chip with no argued clause.
        assert_eq!(run.meta_chip, "▶ Ran skill /gsd-config");
    }

    /// An argued `/<skill> <text>` submits the user's verbatim trailing text as
    /// the run turn (D-02) and the chip shows the argued form.
    #[test]
    fn argued_invoke_uses_trailing_text_as_turn() {
        let (_dir, registry) = test_registry();
        let run = plan_web_skill_run(&registry, "/gsd-config show me the config")
            .expect("registered skill");
        assert_eq!(run.skill_name, "gsd-config");
        assert_eq!(run.turn_input, "show me the config");
        assert_eq!(
            run.meta_chip,
            "▶ Ran skill /gsd-config · \"show me the config\""
        );
    }

    /// Run-turn meta chip copy + 40-char (char-safe) argued truncation with an
    /// inner ellipsis — identical contract to the TUI tracer's chip.
    #[test]
    fn meta_chip_copy_and_truncation() {
        assert_eq!(run_turn_meta_chip("gsd-config", None), "▶ Ran skill /gsd-config");
        assert_eq!(
            run_turn_meta_chip("gsd-config", Some("show me the config")),
            "▶ Ran skill /gsd-config · \"show me the config\"",
            "short argued text renders in full, no ellipsis"
        );
        let long = "x".repeat(50);
        assert_eq!(
            run_turn_meta_chip("gsd-config", Some(&long)),
            format!("▶ Ran skill /gsd-config · \"{}…\"", "x".repeat(40)),
            "argued text longer than 40 chars truncates to 40 + inner ellipsis"
        );
    }

    /// A non-skill token yields `None` — the caller keeps chat-passthrough.
    #[test]
    fn unknown_token_is_passthrough() {
        let (_dir, registry) = test_registry();
        assert!(plan_web_skill_run(&registry, "/no-such-skill").is_none());
        assert!(plan_web_skill_run(&registry, "/").is_none());
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod web_core_handles_tests {
    //! Phase 41.3 Plan 04 (D-11/D-12) — the runtime half of the divergence
    //! gate: proves `assemble_web_core_handles` (the D-12 assembly logic
    //! `web_core_handles` calls) never leaves a core handle unwired, and that
    //! `/agents` reaches a real registry through it rather than the
    //! `handlers.rs:219` "Subagent registry not wired." fallback.
    //!
    //! `assemble_web_core_handles` is tested directly (not `web_core_handles`
    //! itself) because the latter takes `&AppState`, whose `runtime` field is
    //! only constructible via `AgentRuntime::from_config` — config/network-
    //! dependent at construction time, unsuitable for a unit test. Every
    //! fake here mirrors `tests/command_context_parity.rs`'s fixture style
    //! (ironhermes-core) rather than inventing a new mock idiom.

    use super::{assemble_web_core_handles, attach_web_provider_resolver};
    use ironhermes_core::commands::context::{
        McpReloadResult, McpReloader, ProcessRegistrySnapshotHandle, StateStoreHandle,
        SubagentListSnapshot, ToolsetSessionHandle, TrajectoryWriterHandle,
    };
    use ironhermes_core::commands::handlers::dispatch;
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandResult, CommandRouter};
    use ironhermes_core::skills::SkillRegistry;
    use ironhermes_core::types::Platform;
    use ironhermes_core::workspace::Workspace;
    use std::path::PathBuf;
    use std::sync::Arc;

    struct FakeSubagents {
        entries: Vec<(String, String, std::time::Duration)>,
    }
    impl SubagentListSnapshot for FakeSubagents {
        fn active_count(&self) -> usize {
            self.entries.len()
        }
        fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
            self.entries.clone()
        }
        fn kill(&self, _id: &str) -> bool {
            false
        }
        fn transcript_path(&self, _id: &str) -> Option<PathBuf> {
            None
        }
    }

    struct FakeProc;
    impl ProcessRegistrySnapshotHandle for FakeProc {
        fn tracked(&self) -> usize {
            0
        }
        fn snapshot_json(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn drain_and_kill<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
            Box::pin(async move {})
        }
    }

    struct FakeStateStore;
    impl StateStoreHandle for FakeStateStore {
        fn list_sessions_text(&self, _limit: usize) -> String {
            String::new()
        }
        fn list_sessions_text_filtered(
            &self,
            _limit: usize,
            _workspace_root: Option<&str>,
        ) -> String {
            String::new()
        }
        fn history_text(&self, _session_id: &str) -> String {
            String::new()
        }
        fn export_session_text(&self, _session_id: &str) -> String {
            String::new()
        }
        fn update_title(&self, _session_id: &str, _title: &str) -> Result<(), String> {
            Ok(())
        }
        fn get_session_id(&self, _name_or_id: &str) -> Option<String> {
            None
        }
    }

    struct FakeToolsetSession;
    impl ToolsetSessionHandle for FakeToolsetSession {
        fn enable_toolset(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
        fn disable_toolset(&self, _name: &str) -> Result<(), String> {
            Ok(())
        }
        fn render_list(&self) -> String {
            String::new()
        }
        fn render_show(&self, _name: &str) -> Result<String, String> {
            Ok(String::new())
        }
    }

    struct FakeTrajectoryWriter;
    impl TrajectoryWriterHandle for FakeTrajectoryWriter {
        fn append_json_line(&self, _line: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }

    struct FakeMcpReloader;
    #[async_trait::async_trait]
    impl McpReloader for FakeMcpReloader {
        async fn reload(&self) -> McpReloadResult {
            McpReloadResult {
                connected: vec![],
                failed: vec![],
                tool_count: 0,
            }
        }
        fn connected_server_names(&self) -> Vec<String> {
            vec![]
        }
        async fn registered_tool_count(&self) -> usize {
            0
        }
    }

    fn fake_workspace() -> Workspace {
        Workspace {
            root: PathBuf::from("/tmp/fake-web-core-handles-root"),
            soul_path: None,
            agents_chain: vec![],
            memory_dir: PathBuf::from("/tmp/fake-web-core-handles-root/.ironhermes/memory"),
            skills_dir: PathBuf::from("/tmp/fake-web-core-handles-root/skills"),
            tools_config: None,
        }
    }

    /// `web_core_handles_are_complete`: build the D-12 assembly against a
    /// fully-populated set of (fake) handles and assert `missing_core_handles()`
    /// is empty — the runtime half of the gate, catching a surface that calls
    /// the factory with a half-populated struct (the source-grep half in
    /// `command_context_parity.rs` alone cannot detect that failure mode).
    #[test]
    fn web_core_handles_are_complete() {
        let handles = assemble_web_core_handles(
            Arc::new(FakeStateStore),
            Arc::new(FakeSubagents {
                entries: vec![(
                    "sub_webcore01".to_string(),
                    "fixture".to_string(),
                    std::time::Duration::from_secs(1),
                )],
            }),
            Arc::new(FakeProc),
            Arc::new(SkillRegistry::load(&PathBuf::from(
                "/tmp/fake-web-core-handles-skills-nonexistent",
            ))),
            Arc::new(FakeToolsetSession),
            Arc::new(ironhermes_core::concurrency::TurnRegistry::default()),
            Some(Arc::new(fake_workspace())),
            Some(Arc::new(FakeMcpReloader)),
            Some(Arc::new(FakeTrajectoryWriter)),
        );
        let ctx = ironhermes_core::commands::context::build_core_context(
            Platform::Web,
            "web-core-handles-test".to_string(),
            handles,
        );
        assert!(
            ctx.missing_core_handles().is_empty(),
            "fully-populated Web core handles reported missing: {:?}",
            ctx.missing_core_handles()
        );
    }

    /// `web_agents_command_does_not_return_the_not_wired_fallback`: dispatch
    /// `/agents` through a Web-shaped context built by the D-12 assembly and
    /// assert the response does not contain the `handlers.rs:219` fallback
    /// text — this is the direct symptom fix this plan closes.
    #[test]
    fn web_agents_command_does_not_return_the_not_wired_fallback() {
        let handles = assemble_web_core_handles(
            Arc::new(FakeStateStore),
            Arc::new(FakeSubagents {
                entries: vec![(
                    "sub_webagents01".to_string(),
                    "fixture".to_string(),
                    std::time::Duration::from_secs(1),
                )],
            }),
            Arc::new(FakeProc),
            Arc::new(SkillRegistry::load(&PathBuf::from(
                "/tmp/fake-web-core-handles-skills-nonexistent",
            ))),
            Arc::new(FakeToolsetSession),
            Arc::new(ironhermes_core::concurrency::TurnRegistry::default()),
            Some(Arc::new(fake_workspace())),
            Some(Arc::new(FakeMcpReloader)),
            Some(Arc::new(FakeTrajectoryWriter)),
        );
        let ctx = ironhermes_core::commands::context::build_core_context(
            Platform::Web,
            "web-agents-test".to_string(),
            handles,
        );
        let cmd = build_registry()
            .into_iter()
            .find(|c| c.name == "agents")
            .expect("agents command must be registered");
        let router = CommandRouter::new(build_registry());
        let res = dispatch(&cmd, &[], &ctx, &router);
        match res {
            CommandResult::Output(s) => {
                assert!(
                    !s.contains("Subagent registry not wired"),
                    "Web-built context must reach the wired fake, not the \
                     handlers.rs:219 fallback; got: {s}"
                );
                assert!(
                    s.contains("sub_webagents01"),
                    "expected the fake's fixture id in the /agents output; got: {s}"
                );
            }
            other => panic!("expected Output, got {other:?}"),
        }
    }

    /// Phase 41.3 UAT finding F-1. `/model`, `/provider` and `/fast` answered
    /// `"Provider resolver not configured."` on Web because `provider_resolver`
    /// is not one of the nine core handles and nothing attached it.
    ///
    /// Builds a Web-shaped context exactly as the ws handler does — core
    /// handles, then `attach_web_provider_resolver` — and asserts none of the
    /// three commands reaches its `None` guard. A real `ProviderResolver` is
    /// used (`build` is sync and local, no network), so this exercises the
    /// shipped adapter rather than a fake.
    #[test]
    fn web_provider_commands_do_not_return_the_not_configured_fallback() {
        const NOT_CONFIGURED: &str = "Provider resolver not configured.";

        let handles = assemble_web_core_handles(
            Arc::new(FakeStateStore),
            Arc::new(FakeSubagents { entries: vec![] }),
            Arc::new(FakeProc),
            Arc::new(SkillRegistry::load(&PathBuf::from(
                "/tmp/fake-web-provider-skills-nonexistent",
            ))),
            Arc::new(FakeToolsetSession),
            Arc::new(ironhermes_core::concurrency::TurnRegistry::default()),
            Some(Arc::new(fake_workspace())),
            Some(Arc::new(FakeMcpReloader)),
            Some(Arc::new(FakeTrajectoryWriter)),
        );
        let ctx = ironhermes_core::commands::context::build_core_context(
            Platform::Web,
            "web-provider-test".to_string(),
            handles,
        );
        let router = CommandRouter::new(build_registry());
        let model_cmd = build_registry()
            .into_iter()
            .find(|c| c.name == "model")
            .expect("/model must be registered");

        // Negative control: the core handles ALONE must still hit the guard.
        // Without this the positive assertions below could pass vacuously — for
        // example if `build_core_context` ever started wiring the resolver
        // itself, this test would no longer be testing `attach_web_provider_resolver`.
        match dispatch(&model_cmd, &["kimi-k3"], &ctx, &router) {
            CommandResult::Output(s) => assert!(
                s.contains(NOT_CONFIGURED),
                "core handles alone must NOT wire provider_resolver — otherwise \
                 the assertions below prove nothing; got: {s}"
            ),
            other => panic!("expected the None-guard Output, got {other:?}"),
        }

        let config = ironhermes_core::Config::default();
        let resolver = Arc::new(
            ironhermes_core::ProviderResolver::build(&config)
                .expect("resolver must build from a default config"),
        );
        let ctx = attach_web_provider_resolver(ctx, resolver);

        // `/model <name>` — the exact command the operator ran. `/provider` and
        // `/fast` share the same guard, so all three are checked.
        for (name, args) in [
            ("model", vec!["kimi-k3"]),
            ("provider", vec![]),
            ("fast", vec![]),
        ] {
            let cmd = if name == "model" {
                model_cmd.clone()
            } else {
                build_registry()
                    .into_iter()
                    .find(|c| c.name == name)
                    .unwrap_or_else(|| panic!("/{name} must be registered"))
            };
            let rendered = match dispatch(&cmd, &args, &ctx, &router) {
                CommandResult::Output(s) => s,
                CommandResult::Error(e) => e,
                // `/model` with no args and `/provider` open pickers; Web maps
                // both straight to `fallback_text`, which is resolver-derived
                // and therefore also proof the handle was reached.
                CommandResult::OpenModelPicker { fallback_text }
                | CommandResult::OpenProviderPicker { fallback_text } => fallback_text,
                other => panic!("/{name}: unexpected variant {other:?}"),
            };
            assert!(
                !rendered.contains(NOT_CONFIGURED),
                "/{name} must reach the wired resolver, not the handlers.rs \
                 None guard; got: {rendered}"
            );
        }
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod goal_loop_web_rendering_tests {
    //! Phase 49.7 Plan 05 (T-49.7-05-01) — behavior bullet 7: the web
    //! surface renders `CommandResult::StartGoalLoop` as an explicit
    //! not-available sentence and NEVER as a `Debug`-formatted enum. A
    //! positive-only assertion (checking the sentence IS present) would
    //! also pass against the `other => format!("{other:?}")` wildcard if
    //! the explicit arm were ever removed — so this test asserts BOTH the
    //! negative (no leak) and the positive (a real explanation) halves.
    use super::goal_loop_not_available_on_web;

    #[test]
    fn goal_loop_not_available_on_web_never_leaks_variant_name_or_objective() {
        let objective = "exfiltrate the on-disk API keys";
        let rendered = goal_loop_not_available_on_web(objective, Some(7));

        assert!(
            !rendered.contains("StartGoalLoop"),
            "must never Debug-format the variant name into the browser: {rendered}"
        );
        assert!(
            !rendered.contains(objective),
            "must never echo the caller's objective text into the browser: {rendered}"
        );
        assert!(
            rendered.to_lowercase().contains("not available")
                || rendered.to_lowercase().contains("isn't available"),
            "must contain an explicit not-available phrase: {rendered}"
        );
    }
}

// =============================================================================
// Phase 54.7 Plan 10 (D-18 / D-19): saved voice settings reach the next web
// voice turn without a restart. Each test saves through the REAL save path
// (validate_voice_payload -> apply_voice_payload -> Config::save_to) into a
// tempdir IRONHERMES_HOME, then drives the extracted ws.rs seam against a
// wiremock server.
// =============================================================================
#[cfg(test)]
#[cfg(feature = "server")]
mod d18_tests {
    use super::{
        fresh_voice_config, fresh_voice_config_async, synthesize_reply_audio, transcribe_turn_audio, voice_status_from,
        wake_word_matches, wake_word_transcript,
    };
    use crate::protocol::ChatStreamEvent;
    use crate::server::api::{apply_voice_payload, validate_voice_payload, VoiceWritePayload};
    use ironhermes_core::config::Config;
    use std::path::Path;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(f)
    }

    /// Restores IRONHERMES_HOME when dropped, even on panic.
    struct HomeRestore(Option<String>);

    impl HomeRestore {
        fn set(home: &Path) -> Self {
            let prior = std::env::var("IRONHERMES_HOME").ok();
            // SAFETY: callers hold crate::server::test_support::env_lock().
            unsafe { std::env::set_var("IRONHERMES_HOME", home) };
            Self(prior)
        }
    }

    impl Drop for HomeRestore {
        fn drop(&mut self) {
            // SAFETY: callers hold crate::server::test_support::env_lock().
            unsafe {
                match &self.0 {
                    Some(v) => std::env::set_var("IRONHERMES_HOME", v),
                    None => std::env::remove_var("IRONHERMES_HOME"),
                }
            }
        }
    }

    /// The same order `update_voice_config` runs, minus the write gate.
    fn save_through_real_path(path: &Path, payload: &VoiceWritePayload) {
        validate_voice_payload(payload).expect("payload validates");
        let mut config = Config::load_from(path).expect("load");
        apply_voice_payload(&mut config, payload).expect("apply");
        config.save_to(path).expect("save");
    }

    fn whisper_cpp_text(body: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_string(body.to_string())
    }

    fn stt_compat_payload(base_url: &str) -> VoiceWritePayload {
        VoiceWritePayload {
            stt_provider: Some("openai_compatible".into()),
            stt_compat_base_url: Some(base_url.to_string()),
            stt_compat_path: Some("/inference".into()),
            stt_compat_model: Some("whisper-large-v3".into()),
            ..Default::default()
        }
    }

    #[test]
    fn d18_saved_stt_settings_apply_to_next_turn() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        let cfg_path = home.path().join("config.yaml");

        // Config A: the boot snapshot, STT on groq.
        let mut a = Config::default();
        a.stt.provider = "groq".into();
        a.save_to(&cfg_path).expect("write config A");
        let snapshot = Config::load_from(&cfg_path).expect("snapshot");
        assert_eq!(snapshot.stt.provider, "groq");

        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/inference"))
                .respond_with(whisper_cpp_text("{\"text\":\" Hi.\\n\"}"))
                .expect(1)
                .mount(&server)
                .await;

            // The operator saves openai_compatible STT from the panel.
            save_through_real_path(&cfg_path, &stt_compat_payload(&server.uri()));

            // The next turn reads fresh config, not the boot snapshot.
            let fresh = fresh_voice_config(&snapshot);
            assert_eq!(fresh.stt.provider, "openai_compatible");

            let cache = home.path().join("audio_cache");
            let text =
                transcribe_turn_audio(&fresh, b"fake webm bytes", "audio/webm", &cache).await;
            assert_eq!(text.as_deref(), Some("Hi."));

            let requests = server.received_requests().await.expect("requests");
            assert_eq!(requests.len(), 1, "the mock must receive exactly one clip");
        });
    }

    /// IN-05: the async variant loads on the blocking pool and returns the same
    /// config; a broken file falls back to the snapshot.
    #[test]
    fn fresh_voice_config_async_matches_the_sync_load() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let mut snapshot = Config::default();
        snapshot.stt.provider = "openai".into();
        std::fs::write(
            home.path().join("config.yaml"),
            "stt:\n  provider: groq\n",
        )
        .expect("write config");
        let fresh = runtime.block_on(fresh_voice_config_async(&snapshot));
        assert_eq!(fresh.stt.provider, "groq", "the saved file wins over the snapshot");

        std::fs::write(home.path().join("config.yaml"), "stt: [broken: yaml\n  - {")
            .expect("write broken yaml");
        let fallback = runtime.block_on(fresh_voice_config_async(&snapshot));
        assert_eq!(fallback.stt.provider, "openai");

        // The per-clip call sites use the async variant (no blocking load on the
        // executor); only the sync WS-connect status path keeps the sync one.
        let src = include_str!("ws.rs");
        assert!(src.contains("fresh_voice_config_async(&ww_app_state.config()).await"));
        assert!(src.contains("fresh_voice_config_async(&app_state_stt.config()).await"));
    }

    #[test]
    fn fresh_voice_config_falls_back_to_snapshot_on_load_error() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        std::fs::write(
            home.path().join("config.yaml"),
            "stt: [this is: not: valid yaml\n  - {",
        )
        .expect("write broken yaml");

        let mut snapshot = Config::default();
        snapshot.stt.provider = "openai".into();
        let fresh = fresh_voice_config(&snapshot);
        assert_eq!(
            fresh.stt.provider, "openai",
            "a config that fails to load must fall back to the boot snapshot"
        );
    }

    #[test]
    fn turn_audio_filters_hallucinations() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        let cfg_path = home.path().join("config.yaml");
        Config::default().save_to(&cfg_path).expect("write config");

        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/inference"))
                .respond_with(whisper_cpp_text("{\"text\":\" Thank you.\\n\"}"))
                .mount(&server)
                .await;
            save_through_real_path(&cfg_path, &stt_compat_payload(&server.uri()));
            let fresh = fresh_voice_config(&Config::default());

            let cache = home.path().join("audio_cache");
            let text =
                transcribe_turn_audio(&fresh, b"fake webm bytes", "audio/webm", &cache).await;
            assert_eq!(text, None, "HallucinationFilter must reject the phrase");
        });
    }

    #[test]
    fn wake_word_transcript_uses_fresh_compat_config() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        let cfg_path = home.path().join("config.yaml");
        Config::default().save_to(&cfg_path).expect("write config");

        block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/inference"))
                .respond_with(whisper_cpp_text("{\"text\":\" hey hermes open\\n\"}"))
                .expect(1)
                .mount(&server)
                .await;
            save_through_real_path(&cfg_path, &stt_compat_payload(&server.uri()));
            let fresh = fresh_voice_config(&Config::default());

            let cache = home.path().join("audio_cache");
            let transcript =
                wake_word_transcript(&fresh, b"fake webm bytes", "audio/webm", &cache).await;
            assert_eq!(transcript.as_deref(), Some("hey hermes open"));
            assert!(wake_word_matches(
                transcript.as_deref().unwrap_or(""),
                "hey hermes"
            ));
        });
    }

    fn status_fields(ev: ChatStreamEvent) -> (bool, Option<String>, Option<String>) {
        match ev {
            ChatStreamEvent::VoiceStatus {
                stt_available,
                stt_provider,
                stt_model,
                ..
            } => (stt_available, stt_provider, stt_model),
            other => panic!("expected VoiceStatus, got {other:?}"),
        }
    }

    #[test]
    fn voice_status_reports_openai_compatible() {
        let mut cfg = Config::default();
        cfg.stt.provider = "openai_compatible".into();
        cfg.stt.openai_compatible.base_url = "http://stt.lan:8178".into();
        cfg.stt.openai_compatible.model = "whisper-large-v3".into();
        let (available, provider, model) = status_fields(voice_status_from(&cfg));
        assert!(available, "a configured openai_compatible STT is available");
        assert_eq!(provider.as_deref(), Some("openai_compatible"));
        assert_eq!(model.as_deref(), Some("whisper-large-v3"));

        // Pitfall 15: selected but not usable (empty base_url) is NOT available.
        cfg.stt.openai_compatible.base_url = String::new();
        let (available, _, _) = status_fields(voice_status_from(&cfg));
        assert!(
            !available,
            "an openai_compatible STT with an empty base_url must report unavailable"
        );
    }

    #[test]
    fn ws_stt_sites_read_fresh_config() {
        let src = include_str!("ws.rs");
        // Needles are assembled from fragments so this test's own text never matches.
        for needle in [
            [".config()", ".stt"].concat(),
            ["config().voice.wake", "_word"].concat(),
        ] {
            assert!(
                !src.contains(&needle),
                "ws.rs still reads the boot snapshot via `{needle}` (D-18)"
            );
        }
    }

    const FAKE_AUDIO: &[u8] = b"ID3 fake synthesized audio body";

    /// A mock OpenAI-compatible speech server answering `POST /v1/audio/speech`.
    async fn speech_server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/audio/speech"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(FAKE_AUDIO.to_vec()))
            .mount(&server)
            .await;
        server
    }

    fn tts_compat_config(base_url: &str, format: &str) -> Config {
        let mut cfg = Config::default();
        cfg.tts.provider = "openai_compatible".into();
        cfg.tts.openai_compatible.base_url = base_url.to_string();
        cfg.tts.openai_compatible.format = format.to_string();
        cfg
    }

    #[test]
    fn d18_saved_tts_settings_apply_to_next_reply() {
        let _guard = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().expect("tempdir");
        let _restore = HomeRestore::set(home.path());
        let cfg_path = home.path().join("config.yaml");
        Config::default()
            .save_to(&cfg_path)
            .expect("write config A");
        let snapshot = Config::load_from(&cfg_path).expect("snapshot");
        assert_ne!(snapshot.tts.provider, "openai_compatible");

        block_on(async {
            let server = speech_server().await;
            let payload = VoiceWritePayload {
                tts_provider: Some("openai_compatible".into()),
                tts_compat_base_url: Some(format!("{}/v1", server.uri())),
                tts_compat_format: Some("mp3".into()),
                ..Default::default()
            };
            save_through_real_path(&cfg_path, &payload);

            let fresh = fresh_voice_config(&snapshot);
            let audio_dir = home.path().join("audio_cache");
            let written = synthesize_reply_audio(&fresh, "hi", None, &audio_dir)
                .await
                .expect("the new TTS server must synthesize");
            assert_eq!(written.extension().and_then(|e| e.to_str()), Some("mp3"));
            assert_eq!(std::fs::read(&written).expect("read audio"), FAKE_AUDIO);
            assert_eq!(
                server.received_requests().await.expect("requests").len(),
                1,
                "the mock must receive exactly one synthesis request"
            );
        });
    }

    #[test]
    fn reply_audio_extension_follows_format() {
        let home = tempfile::tempdir().expect("tempdir");
        block_on(async {
            let server = speech_server().await;
            let cfg = tts_compat_config(&format!("{}/v1", server.uri()), "wav");
            let audio_dir = home.path().join("audio_cache");
            let written = synthesize_reply_audio(&cfg, "hi", None, &audio_dir)
                .await
                .expect("synthesize");
            assert_eq!(written.extension().and_then(|e| e.to_str()), Some("wav"));
        });
    }

    #[test]
    fn reply_audio_rejects_unknown_identity_for_selection() {
        let home = tempfile::tempdir().expect("tempdir");
        block_on(async {
            let server = speech_server().await;
            let mut cfg = tts_compat_config(&format!("{}/v1", server.uri()), "mp3");
            // Both slugs carry a format override; only a known preset may apply it.
            for slug in ["rogue_slug_not_a_preset", "orb_classic"] {
                let mut record = ironhermes_core::config::IdentityRecord::default();
                record.voice.free_mode_tts_format = Some("opus".into());
                cfg.identities.insert(slug.to_string(), record);
            }
            let audio_dir = home.path().join("audio_cache");

            let unknown =
                synthesize_reply_audio(&cfg, "hi", Some("rogue_slug_not_a_preset"), &audio_dir)
                    .await
                    .expect("synthesize with an unknown identity");
            assert_eq!(
                unknown.extension().and_then(|e| e.to_str()),
                Some("mp3"),
                "an unknown identity slug must be ignored (global config used)"
            );

            let known = synthesize_reply_audio(&cfg, "hi", Some("orb_classic"), &audio_dir)
                .await
                .expect("synthesize with a known identity");
            assert_eq!(
                known.extension().and_then(|e| e.to_str()),
                Some("opus"),
                "a known identity applies its override"
            );
        });
    }

    #[test]
    fn ws_voice_paths_do_not_call_apply_config_now() {
        // Scan the production code only: this test fn's own name contains the needle.
        let src = include_str!("ws.rs")
            .split("mod d18_tests")
            .next()
            .unwrap_or_default();
        let needle = ["apply_config", "_now("].concat();
        assert!(
            !src.contains(&needle),
            "voice paths must not call apply_config_now (T-54.7-44)"
        );
    }
}

// =============================================================================
// Phase 57 plan 12 (D-04b, D-22, UI-SPEC R9c): web surface wiring tests
// =============================================================================
#[cfg(test)]
#[cfg(feature = "server")]
mod web_level_tests {
    use super::*;
    use ironhermes_core::commands::context::CommandContext;
    use ironhermes_core::commands::{CommandResult, CommandRouter, ResolveResult};
    use ironhermes_core::config::DefconLevel;
    use ironhermes_core::defcon::session_defcon;
    use ironhermes_core::types::Platform;
    use ironhermes_core::{AuditConfig, AuditLog};
    use ironhermes_security::policy::Severity;
    use ironhermes_security::testing::{
        FakeBackend, clear_installed_for_tests, install_fake_for_tests,
    };

    fn level(n: u8) -> DefconLevel {
        DefconLevel::try_from(n).expect("valid level")
    }

    fn as_json(events: &[ChatStreamEvent]) -> Vec<serde_json::Value> {
        events
            .iter()
            .map(|e| serde_json::to_value(e).expect("serialize"))
            .collect()
    }

    fn rows(path: &std::path::Path) -> Vec<serde_json::Value> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// R9c: a level change pushes exactly one `DefconLevel` (with the server's own
    /// `bypass`); a refused or same-level `/yolo` pushes nothing.
    #[test]
    fn web_level_change_pushes_defcon_level() {
        assert_eq!(
            as_json(&web_level_change_events("s1", level(3), level(1))),
            vec![serde_json::json!(
                {"DefconLevel":{"session_id":"s1","level":1,"bypass":false}}
            )]
        );
        assert_eq!(
            as_json(&web_level_change_events("s1", level(3), level(5))),
            vec![serde_json::json!(
                {"DefconLevel":{"session_id":"s1","level":5,"bypass":true}}
            )]
        );
        assert_eq!(
            as_json(&web_level_change_events("s1", level(4), level(4))),
            Vec::<serde_json::Value>::new(),
            "an unchanged level pushes nothing"
        );
    }

    /// D-14: each gate event reaches the turn's channel as the matching wire event, with
    /// the turn id, the tool name, and no arguments anywhere.
    #[test]
    fn gate_events_map_to_wire_events() {
        use crate::server::web_approval_gate::WebGateEvent;
        let turn = uuid::Uuid::new_v4();

        let requested = web_gate_event_to_stream_event(
            turn,
            &WebGateEvent::ApprovalRequested {
                session_id: "s".into(),
                approval_id: "a".into(),
                tool: "terminal".into(),
                reason: "needs approval".into(),
            },
        )
        .expect("ApprovalRequested maps");
        assert_eq!(
            serde_json::to_value(&requested).unwrap(),
            serde_json::json!({"ApprovalRequested":{
                "turn_id": turn.to_string(), "session_id":"s", "approval_id":"a",
                "tool":"terminal", "reason":"needs approval"}})
        );

        let resolved = web_gate_event_to_stream_event(
            turn,
            &WebGateEvent::ApprovalResolved {
                session_id: "s".into(),
                approval_id: "a".into(),
                tool: "terminal".into(),
                outcome: "timed_out".into(),
            },
        )
        .expect("ApprovalResolved maps");
        assert_eq!(
            serde_json::to_value(&resolved).unwrap(),
            serde_json::json!({"ApprovalResolved":{
                "turn_id": turn.to_string(), "session_id":"s", "approval_id":"a",
                "tool":"terminal", "outcome":"timed_out"}})
        );

        let bypass = web_gate_event_to_stream_event(
            turn,
            &WebGateEvent::YoloBypass(ironhermes_core::YoloBypassSummary::single(
                "s", "web", "", "terminal", "reason",
            )),
        )
        .expect("YoloBypass maps");
        assert!(matches!(
            bypass,
            ChatStreamEvent::YoloBypassNotice { count: 1, .. }
        ));

        let security = web_gate_event_to_stream_event(
            turn,
            &WebGateEvent::Security(ironhermes_core::SecurityEvent {
                kind: ironhermes_core::SecurityEventKind::Blocked,
                tool: "terminal".into(),
                rule_ids: vec![],
                level: 3,
                summary: "blocked".into(),
            }),
        );
        assert!(
            security.is_none(),
            "Security is one-to-many: web_notice_sink maps it through security_event_to_stream"
        );
    }

    /// D-04(b): a Block never reaches the model (the gate returns false, so the caller
    /// never starts the turn), names the rule id, and is audited without the text.
    // The std lock is held across awaits ON PURPOSE: it serializes this test's use of the
    // process-global content-screen slot against other tests that install one.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn web_inbound_message_blocked() {
        let _env = crate::server::test_support::env_lock();
        install_fake_for_tests(
            FakeBackend::new()
                .block_if_contains("CANARY-BLOCK", Severity::High)
                .warn_if_contains("CANARY-WARN", Severity::Medium),
        );
        let dir = tempfile::tempdir().unwrap();
        let audit_path = dir.path().join("audit.jsonl");
        let audit = AuditLog::with_path(audit_path.clone(), AuditConfig::default());
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        // The test seam: `turns` is what `run_web_turn` would have been called for.
        let mut turns = 0;
        if web_inbound_gate_with(
            &tx,
            uuid::Uuid::nil(),
            "s-in",
            "please CANARY-BLOCK this",
            level(3),
            &audit,
        )
        .await
        {
            turns += 1;
        }
        assert_eq!(turns, 0, "a blocked message must never start a turn");
        match rx.try_recv().expect("an Error event") {
            ChatStreamEvent::Error { message, .. } => {
                assert!(
                    message.contains("FakeBlock"),
                    "names the rule id: {message}"
                );
                assert!(
                    !message.contains("CANARY-BLOCK"),
                    "never echoes the text: {message}"
                );
            }
            other => panic!("expected an Error event, got {other:?}"),
        }
        let written = rows(&audit_path);
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0]["kind"], "security_verdict");
        assert_eq!(written[0]["tool"], "inbound_message");
        assert_eq!(written[0]["decision"], "blocked");
        assert!(
            !std::fs::read_to_string(&audit_path)
                .unwrap()
                .contains("CANARY"),
            "the audit row must carry rule ids, not the text"
        );

        // A benign message proceeds and writes nothing.
        assert!(
            web_inbound_gate_with(&tx, uuid::Uuid::nil(), "s-in", "hello", level(3), &audit).await
        );
        assert!(rx.try_recv().is_err(), "no event for a clean message");
        assert_eq!(rows(&audit_path).len(), 1);

        // A Warn is audited and proceeds.
        assert!(
            web_inbound_gate_with(
                &tx,
                uuid::Uuid::nil(),
                "s-in",
                "some CANARY-WARN text",
                level(3),
                &audit
            )
            .await,
            "a Warn does not stop the message"
        );
        assert!(rx.try_recv().is_err(), "a Warn sends no Error");
        let written = rows(&audit_path);
        assert_eq!(written.len(), 2, "{written:?}");
        assert_eq!(written[1]["decision"], "warn");

        clear_installed_for_tests();
    }

    /// CR-01 / D-04(b) / D-12: a message the sidecar could not screen never starts a
    /// turn at DEFCON 3, names the reason without echoing the text, and is audited as a
    /// `sidecar_unavailable` row. At DEFCON 5 it proceeds as a Warn with the same row.
    // Held across awaits on purpose, as in `web_inbound_message_blocked`.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn web_inbound_unscreenable_refused_at_three() {
        let _env = crate::server::test_support::env_lock();
        install_fake_for_tests(FakeBackend::unavailable(
            ironhermes_security::UnavailableReason::TooLarge,
        ));
        let dir = tempfile::tempdir().unwrap();
        let audit_path = dir.path().join("audit.jsonl");
        let audit = AuditLog::with_path(audit_path.clone(), AuditConfig::default());
        let (tx, mut rx) = mpsc::unbounded_channel::<ChatStreamEvent>();

        assert!(
            !web_inbound_gate_with(
                &tx,
                uuid::Uuid::nil(),
                "s-unscreened",
                "please CANARY-UNSCREENED",
                level(3),
                &audit,
            )
            .await,
            "an unscreened message must never start a turn at DEFCON 3"
        );
        match rx.try_recv().expect("an Error event") {
            ChatStreamEvent::Error { message, .. } => {
                assert_eq!(
                    message,
                    "Message not processed: it could not be screened (too_large)."
                );
            }
            other => panic!("expected an Error event, got {other:?}"),
        }
        assert!(rx.try_recv().is_err(), "exactly one Error event");
        let written = rows(&audit_path);
        assert_eq!(written.len(), 1, "{written:?}");
        assert_eq!(written[0]["kind"], "sidecar_unavailable");
        assert_eq!(written[0]["tool"], "inbound_message");
        assert_eq!(written[0]["decision"], "blocked");
        let text = std::fs::read_to_string(&audit_path).unwrap();
        assert!(!text.contains("CANARY"), "the row must not carry the text");
        assert!(text.contains("too_large"), "the row carries the reason");

        // DEFCON 5: a Warn. The message proceeds, no Error, one more row.
        assert!(
            web_inbound_gate_with(
                &tx,
                uuid::Uuid::nil(),
                "s-unscreened",
                "please CANARY-UNSCREENED",
                level(5),
                &audit,
            )
            .await,
            "an unscreened message is a Warn at DEFCON 5"
        );
        assert!(rx.try_recv().is_err(), "a Warn sends no Error");
        let written = rows(&audit_path);
        assert_eq!(written.len(), 2, "{written:?}");
        assert_eq!(written[1]["kind"], "sidecar_unavailable");
        assert_eq!(written[1]["tool"], "inbound_message");
        assert_eq!(written[1]["decision"], "warn");
        assert!(
            !std::fs::read_to_string(&audit_path)
                .unwrap()
                .contains("CANARY")
        );

        clear_installed_for_tests();
    }

    /// D-22: `/yolo <N>` from the web changes the level only when the auth layer is on
    /// (and `defcon_remote_change_enabled` is set); with auth off it is refused.
    #[test]
    fn web_yolo_needs_auth_enabled() {
        let _env = crate::server::test_support::env_lock();
        let home = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.yaml"),
            "security:\n  defcon_remote_change_enabled: true\n",
        )
        .unwrap();
        unsafe { std::env::set_var("IRONHERMES_HOME", home.path()) };

        let router = CommandRouter::new(ironhermes_core::commands::registry::build_registry());
        let run = |auth_enabled: bool, session: &str| -> CommandResult {
            let (ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def)) =
                router.resolve("/yolo 5", &Platform::Web)
            else {
                panic!("/yolo must resolve on the web surface");
            };
            let ctx = CommandContext::new(Platform::Web, session.to_string())
                .with_defcon_auth(web_defcon_auth_facts(auth_enabled));
            ironhermes_core::commands::handlers::dispatch(def, &["5"], &ctx, &router)
        };

        session_defcon().set_session_level("s-yolo-off", level(3));
        assert!(
            matches!(run(false, "s-yolo-off"), CommandResult::Error(_)),
            "auth disabled: the web surface cannot change the level"
        );
        assert_eq!(session_defcon().handle_for("s-yolo-off").get(), level(3));

        session_defcon().set_session_level("s-yolo-on", level(3));
        assert!(
            matches!(run(true, "s-yolo-on"), CommandResult::Output(_)),
            "auth enabled and the remote flag on: the level changes"
        );
        assert_eq!(session_defcon().handle_for("s-yolo-on").get(), level(5));

        let audit = rows(&home.path().join("logs").join("audit.jsonl"));
        assert!(
            audit.iter().any(|r| r["kind"] == "defcon_change"
                && r["session"] == "s-yolo-on"
                && r["decision"] == "changed"),
            "a defcon_change row was written: {audit:?}"
        );
        assert!(
            audit.iter().any(|r| r["kind"] == "defcon_change"
                && r["session"] == "s-yolo-off"
                && r["decision"] == "refused"),
            "the refusal is audited too: {audit:?}"
        );

        unsafe { std::env::remove_var("IRONHERMES_HOME") };
        session_defcon().forget("s-yolo-off");
        session_defcon().forget("s-yolo-on");
    }
}

#[cfg(test)]
#[cfg(feature = "server")]
mod security_stream_tests {
    //! Phase 57 plan 14: the server half of the critical-only chat. An in-turn
    //! `SecurityEvent` becomes at most one `TurnSeverity` plus at most one transcript
    //! `CriticalEvent`, built from the tool name and rule ids only.
    use super::*;
    use crate::protocol::TurnWorst;
    use ironhermes_core::{SecurityEvent, SecurityEventKind};

    fn ev(
        kind: SecurityEventKind,
        tool: &str,
        rules: &[&str],
        level: u8,
        summary: &str,
    ) -> SecurityEvent {
        SecurityEvent {
            kind,
            tool: tool.to_string(),
            rule_ids: rules.iter().map(|r| r.to_string()).collect(),
            level,
            summary: summary.to_string(),
        }
    }

    fn turn() -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    /// `(worst, kind, summary)` of the `[TurnSeverity, CriticalEvent]` pair, or panic.
    fn sev_and_crit(out: &[ChatStreamEvent]) -> (TurnWorst, String, String) {
        assert_eq!(
            out.len(),
            2,
            "expected [TurnSeverity, CriticalEvent], got {out:?}"
        );
        let worst = match &out[0] {
            ChatStreamEvent::TurnSeverity { worst, .. } => *worst,
            other => panic!("first event is not TurnSeverity: {other:?}"),
        };
        match &out[1] {
            ChatStreamEvent::CriticalEvent { kind, summary, .. } => {
                (worst, kind.clone(), summary.clone())
            }
            other => panic!("second event is not CriticalEvent: {other:?}"),
        }
    }

    fn only_severity(out: &[ChatStreamEvent]) -> TurnWorst {
        assert_eq!(out.len(), 1, "expected exactly [TurnSeverity], got {out:?}");
        match &out[0] {
            ChatStreamEvent::TurnSeverity { worst, .. } => *worst,
            other => panic!("not TurnSeverity: {other:?}"),
        }
    }

    #[test]
    fn security_block_maps_to_red_and_critical() {
        let t = turn();
        let out = security_event_to_stream(
            t,
            "s",
            &ev(
                SecurityEventKind::Blocked,
                "terminal",
                &["guardrail.rm_rf"],
                3,
                "CANARY-CMD",
            ),
        );
        let (worst, kind, summary) = sev_and_crit(&out);
        assert_eq!(worst, TurnWorst::Red);
        assert_eq!(kind, "blocked");
        assert_eq!(
            summary,
            "terminal was blocked by rule guardrail.rm_rf. It did not run."
        );
        for e in &out {
            match e {
                ChatStreamEvent::TurnSeverity {
                    turn_id,
                    session_id,
                    ..
                }
                | ChatStreamEvent::CriticalEvent {
                    turn_id,
                    session_id,
                    ..
                } => {
                    assert_eq!(*turn_id, t);
                    assert_eq!(session_id, "s");
                }
                _ => unreachable!(),
            }
        }
        assert!(!serde_json::to_string(&out).unwrap().contains("CANARY-CMD"));

        let out = security_event_to_stream(
            t,
            "s",
            &ev(SecurityEventKind::Blocked, "terminal", &[], 3, "CANARY-CMD"),
        );
        let (_, _, summary) = sev_and_crit(&out);
        assert_eq!(
            summary,
            "terminal was blocked by the security policy. It did not run."
        );

        let out = security_event_to_stream(
            t,
            "s",
            &ev(
                SecurityEventKind::Blocked,
                "terminal",
                &["a.one", "b.two"],
                3,
                "x",
            ),
        );
        let (_, _, summary) = sev_and_crit(&out);
        assert_eq!(
            summary,
            "terminal was blocked by rule a.one, b.two. It did not run."
        );

        let out = security_event_to_stream(
            t,
            "s",
            &ev(
                SecurityEventKind::CredentialDetected,
                "terminal",
                &[],
                3,
                "CANARY-CMD",
            ),
        );
        let (worst, kind, summary) = sev_and_crit(&out);
        assert_eq!(worst, TurnWorst::Amber);
        assert_eq!(kind, "credential_detected");
        assert_eq!(
            summary,
            "A credential was found in terminal. The value is hidden."
        );

        for k in [SecurityEventKind::WarnRan, SecurityEventKind::BypassRan] {
            let out = security_event_to_stream(t, "s", &ev(k, "terminal", &[], 3, "CANARY-CMD"));
            assert_eq!(only_severity(&out), TurnWorst::Amber);
            assert!(!serde_json::to_string(&out).unwrap().contains("CANARY-CMD"));
        }
    }

    #[test]
    fn sidecar_unavailable_outcome_branches() {
        let t = turn();
        let run = |tool: &str, level: u8, summary: &str| {
            sev_and_crit(&security_event_to_stream(
                t,
                "s",
                &ev(
                    SecurityEventKind::SidecarUnavailable,
                    tool,
                    &[],
                    level,
                    summary,
                ),
            ))
        };

        let (w, k, s) = run("terminal", 3, "timeout; outcome blocked");
        assert_eq!((w, k.as_str()), (TurnWorst::Red, "sidecar_unavailable"));
        assert_eq!(
            s,
            "terminal could not be screened (timeout) and was blocked."
        );

        let (w, _, s) = run("write_file", 3, "too_large; outcome approval");
        assert_eq!(w, TurnWorst::Amber);
        assert_eq!(
            s,
            "write_file could not be screened (too_large) and was held for approval."
        );

        let (w, _, s) = run("terminal", 5, "killed_for_timeouts; outcome bypass");
        assert_eq!(w, TurnWorst::Amber);
        assert_eq!(
            s,
            "terminal could not be screened (killed_for_timeouts) and ran without approval (audited bypass)."
        );

        let (w, _, s) = run("write_file", 2, "crash; outcome approval");
        assert_eq!(w, TurnWorst::Amber);
        assert_eq!(
            s,
            "The security scanner is not responding. write_file was held for approval."
        );

        let (w, _, s) = run("terminal", 5, "missing; outcome bypass");
        assert_eq!(w, TurnWorst::Amber);
        assert_eq!(
            s,
            "The security scanner is missing. terminal ran without approval (audited bypass)."
        );

        // A floor Critical blocks at every level (D-07).
        let (w, _, s) = run("terminal", 5, "crash; outcome blocked");
        assert_eq!(w, TurnWorst::Red);
        assert_eq!(
            s,
            "The security scanner is not responding. terminal was blocked."
        );

        let (w, _, s) = run("read_file", 3, "crash; outcome allowed");
        assert_eq!(w, TurnWorst::Amber);
        assert_eq!(
            s,
            "The security scanner is not responding. read_file ran with the built-in rules only."
        );

        // No token: the D-12 Warn-class default.
        let (w, _, s) = run("terminal", 3, "crash");
        assert_eq!(w, TurnWorst::Red);
        assert!(s.ends_with("terminal was blocked."), "{s}");
        let (w, _, s) = run("terminal", 4, "crash");
        assert_eq!(w, TurnWorst::Amber);
        assert!(
            s.ends_with("terminal ran without approval (audited bypass)."),
            "{s}"
        );

        for summary in [
            "timeout; outcome blocked",
            "crash; outcome approval",
            "missing; outcome bypass",
            "crash; outcome allowed",
            "crash",
        ] {
            let (_, _, s) = run("terminal", 3, summary);
            assert!(!s.contains("outcome"), "token leaked: {s}");
            assert!(!s.contains('{'), "raw placeholder: {s}");
        }
    }

    /// Drive a terminal call at DEFCON 3 through the real `execute_gated_command`, a
    /// `WebApprovalGate` capture sink and `security_event_to_stream` with a sidecar that
    /// is unavailable for `reason`. Returns the `(kind, summary)` of every `CriticalEvent`
    /// the web would render, in order.
    async fn gated_block_rows(
        reason: ironhermes_security::UnavailableReason,
    ) -> Vec<(String, String)> {
        use crate::server::web_approval_gate::{NoticeSink, WebApprovalGate, WebGateEvent};
        use ironhermes_core::config::DefconLevel;
        use ironhermes_core::{AuditConfig, AuditLog};
        use ironhermes_hooks::{
            execute_gated_command, DangerousCommandGuardrail, GatePolicy, GatedOutcome,
        };
        use ironhermes_security::service::SecurityService;
        use ironhermes_security::testing::FakeBackend;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Vec<WebGateEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_events = captured.clone();
        let sink: NoticeSink = Arc::new(move |e: &WebGateEvent| {
            sink_events.lock().unwrap().push(e.clone());
        });
        let gate = WebApprovalGate::with_notice_sink(sink);
        let dir = tempfile::tempdir().unwrap();
        let audit = AuditLog::with_path(dir.path().join("audit.jsonl"), AuditConfig::default());
        let policy = GatePolicy::fixed(
            DefconLevel::THREE,
            Some(Arc::new(SecurityService::new(Arc::new(
                FakeBackend::unavailable(reason),
            )))),
        );
        let ran = Arc::new(AtomicUsize::new(0));
        let ran_in = ran.clone();
        let outcome = execute_gated_command(
            "terminal",
            "ls",
            &DangerousCommandGuardrail::new(),
            Some(&gate),
            &audit,
            "s-g2",
            "web",
            "s-g2",
            policy,
            false,
            false,
            || async move {
                ran_in.fetch_add(1, Ordering::SeqCst);
                Ok("ran".to_string())
            },
        )
        .await;
        assert!(matches!(outcome, GatedOutcome::Blocked(_)), "{outcome:?}");
        assert_eq!(ran.load(Ordering::SeqCst), 0, "the call must not run");

        let t = turn();
        let events = captured.lock().unwrap().clone();
        let mut rows = Vec::new();
        for e in events {
            let WebGateEvent::Security(sev) = e else {
                continue;
            };
            for out in security_event_to_stream(t, "s-g2", &sev) {
                if let ChatStreamEvent::CriticalEvent { kind, summary, .. } = out {
                    rows.push((kind, summary));
                }
            }
        }
        rows
    }

    /// G-2 (D-16, D-12): a terminal call Blocked at DEFCON 3 because the sidecar crashed
    /// shows a web row that names the scanner, through the real chokepoint.
    #[tokio::test]
    async fn outage_block_card_names_the_scanner_e2e() {
        let rows = gated_block_rows(ironhermes_security::UnavailableReason::Crash).await;
        assert_eq!(
            rows,
            vec![
                (
                    "sidecar_unavailable".to_string(),
                    "The security scanner is not responding. terminal was blocked.".to_string()
                ),
                (
                    "blocked".to_string(),
                    "terminal was blocked: the security scanner is not responding. It did not run."
                        .to_string()
                ),
            ]
        );
    }

    /// G-2: every cause renders its own clause, in the documented precedence, and an
    /// unparseable marker suffix renders no suffix text.
    /// GC-WR-03: a refused subject reads as "could not be screened" on both rows.
    #[tokio::test]
    async fn unscreened_block_card_names_the_reason_e2e() {
        let rows = gated_block_rows(ironhermes_security::UnavailableReason::TooLarge).await;
        assert_eq!(
            rows,
            vec![
                (
                    "sidecar_unavailable".to_string(),
                    "terminal could not be screened (too_large) and was blocked.".to_string()
                ),
                (
                    "blocked".to_string(),
                    "terminal was blocked: it could not be screened (too_large). It did not run."
                        .to_string()
                ),
            ]
        );
    }

    #[test]
    fn blocked_clause_names_every_cause() {
        let clause = |ids: &[&str]| {
            let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
            blocked_clause(&ids)
        };
        assert_eq!(clause(&[]), " by the security policy");
        assert_eq!(clause(&["regex_floor"]), " by the built-in rules");
        assert_eq!(
            clause(&["unscreened:too_large"]),
            ": it could not be screened (too_large)"
        );
        assert_eq!(
            clause(&["unscreened:killed_for_timeouts"]),
            ": it could not be screened (killed_for_timeouts)"
        );
        assert_eq!(
            clause(&["unscreened:<script>"]),
            ": it could not be screened"
        );
        assert_eq!(
            clause(&["sidecar_outage:crash"]),
            ": the security scanner is not responding"
        );
        assert_eq!(
            clause(&["sidecar_outage:missing"]),
            ": the security scanner is missing"
        );
        assert_eq!(
            clause(&["guardrail.rm_rf", "regex_floor"]),
            " by rule guardrail.rm_rf"
        );
        assert_eq!(
            clause(&["regex_floor", "unscreened:too_large"]),
            " by the built-in rules"
        );
    }

    #[test]
    fn approval_and_turn_failure_render_once() {
        let t = turn();
        for k in [
            SecurityEventKind::ApprovalRequested,
            SecurityEventKind::ApprovalResolved,
            SecurityEventKind::DefconChanged,
            SecurityEventKind::JobBlocked,
        ] {
            let out = security_event_to_stream(t, "s", &ev(k.clone(), "terminal", &[], 3, "x"));
            assert!(
                out.is_empty(),
                "{k:?} must yield no transcript event: {out:?}"
            );
        }
        let out = security_event_to_stream(
            t,
            "s",
            &ev(
                SecurityEventKind::ToolFailedTurnEnded,
                "terminal",
                &[],
                3,
                "x",
            ),
        );
        assert_eq!(only_severity(&out), TurnWorst::Red);
        assert!(!out
            .iter()
            .any(|e| matches!(e, ChatStreamEvent::CriticalEvent { .. })));
    }

    fn crit_summary(ev: Option<ChatStreamEvent>) -> String {
        match ev {
            Some(ChatStreamEvent::CriticalEvent {
                turn_id,
                session_id,
                kind,
                summary,
            }) => {
                assert_eq!(
                    turn_id,
                    uuid::Uuid::nil(),
                    "an out-of-turn row has a nil turn"
                );
                assert_eq!(session_id, "");
                assert_eq!(kind, "job_blocked");
                summary
            }
            other => panic!("expected a job_blocked CriticalEvent, got {other:?}"),
        }
    }

    /// D-16 / R6: a blocked scheduled job becomes one nil-turn row built from the job
    /// label and rule ids only; every other kind stays off this path.
    #[test]
    fn process_job_blocked_maps_to_critical_event() {
        let job = |tool: &str, rules: &[&str], label: &str| {
            process_event_to_stream(&ev(SecurityEventKind::JobBlocked, tool, rules, 3, label))
        };
        assert_eq!(
            crit_summary(job("cron_prompt", &["cron.exfil"], "clawdi-fork-parity-daily")),
            "Scheduled job clawdi-fork-parity-daily was blocked by rule cron.exfil. It did not run."
        );
        assert_eq!(
            crit_summary(job("cron_prompt", &["cron.exfil"], "job-7")),
            "Scheduled job job-7 was blocked by rule cron.exfil. It did not run."
        );
        assert_eq!(
            crit_summary(job("cron_prompt", &["a.one", "b.two"], "nightly")),
            "Scheduled job nightly was blocked by rule a.one, b.two. It did not run."
        );
        assert_eq!(
            crit_summary(job("cron_prompt", &[], "nightly")),
            "Scheduled job nightly was blocked by the security policy. It did not run."
        );
        assert_eq!(
            crit_summary(job("cron_prompt", &["unscreened:too_large"], "nightly")),
            "Scheduled job nightly was blocked: it could not be screened (too_large). It did not run."
        );
        assert_eq!(
            crit_summary(job("kanban_task", &["kanban.exfil"], "42")),
            "Scheduled job 42 was blocked by rule kanban.exfil. It did not run."
        );
        let unnamed = crit_summary(job("cron_prompt", &["r"], "   "));
        assert_eq!(
            unnamed,
            "Scheduled job (unnamed) was blocked by rule r. It did not run."
        );
        assert!(!unnamed.contains('{'), "never a raw placeholder: {unnamed}");

        for k in [
            SecurityEventKind::Blocked,
            SecurityEventKind::WarnRan,
            SecurityEventKind::BypassRan,
            SecurityEventKind::SidecarUnavailable,
        ] {
            assert!(
                process_event_to_stream(&ev(k.clone(), "terminal", &["r"], 3, "x")).is_none(),
                "{k:?} must not be forwarded from the bus"
            );
        }

        // Structural: each chat socket subscribes to the bus and uses the mapper.
        const WS_RS: &str = include_str!("ws.rs");
        let prod = &WS_RS[..WS_RS.find(concat!("#[cfg(", "test)]")).unwrap()];
        assert!(prod.contains(concat!("subscribe_security", "_events()")));
        assert!(
            prod.matches(concat!("process_event_to", "_stream("))
                .count()
                >= 2,
            "the mapper is defined and used in production code"
        );
    }

    fn level(n: u8) -> ironhermes_core::config::DefconLevel {
        ironhermes_core::config::DefconLevel::try_from(n).expect("valid level")
    }

    /// R8: a web `/yolo <N>` that changes the level adds exactly one row.
    #[test]
    fn defcon_changed_maps_to_critical_event() {
        match defcon_changed_event("s1", level(3), level(1), "web", "web") {
            Some(ChatStreamEvent::CriticalEvent {
                turn_id,
                session_id,
                kind,
                summary,
            }) => {
                assert_eq!(turn_id, uuid::Uuid::nil());
                assert_eq!(session_id, "s1");
                assert_eq!(kind, "defcon_changed");
                assert_eq!(summary, "DEFCON 3 to 1, set from web by web.");
            }
            other => panic!("expected a defcon_changed CriticalEvent, got {other:?}"),
        }
        const WS_RS: &str = include_str!("ws.rs");
        let prod = &WS_RS[..WS_RS.find(concat!("#[cfg(", "test)]")).unwrap()];
        assert!(
            prod.matches(concat!("defcon_changed", "_event(")).count() >= 2,
            "the builder is defined and called from the slash dispatch"
        );
    }

    /// R8 / T-57-79: a refused or same-level `/yolo` leaves the level unchanged, so the
    /// before/after pair the dispatch passes is equal and no row is built.
    #[test]
    fn refused_yolo_adds_no_critical_row() {
        assert!(defcon_changed_event("s1", level(3), level(3), "web", "web").is_none());
        assert!(defcon_changed_event("s1", level(5), level(5), "web", "web").is_none());
    }
}

/// Phase 54.9 Plan 09 (D-09, T-54.9-24): the OpenUI overlay is server-owned and opt-in.
#[cfg(all(test, feature = "server"))]
mod openui_overlay_tests {
    use super::*;

    fn req(openui: bool) -> ChatRequest {
        ChatRequest {
            session_id: "s".into(),
            message: "show a table".into(),
            active_identity: None,
            attachment_ids: Vec::new(),
            openui,
            openui_gateway: false,
            openui_prompt: false,
        }
    }

    #[test]
    fn slash_guard_applies_to_actions_not_typed_prompts() {
        // RR-04
        let mut r = req(true);
        r.message = "  /yolo".into();
        assert!(openui_action_slash_blocked(&r), "an action with a slash is blocked");
        r.openui_prompt = true;
        assert!(!openui_action_slash_blocked(&r), "the operator's own prompt is not");
        r.openui_prompt = false;
        r.message = "do the thing".into();
        assert!(!openui_action_slash_blocked(&r));
        r.openui = false;
        r.message = "/new".into();
        assert!(!openui_action_slash_blocked(&r), "plain Chat is untouched");
    }

    #[test]
    fn gateway_branch_only_when_flag_and_openui() {
        let mut r = req(false);
        assert!(!openui_gateway_requested(&r));
        r.openui_gateway = true;
        assert!(
            !openui_gateway_requested(&r),
            "the gateway flag alone is not enough"
        );
        r.openui = true;
        assert!(openui_gateway_requested(&r));
        r.openui_gateway = false;
        assert!(!openui_gateway_requested(&r));
        // The Gateway branch sits behind the flag and runs instead of run_web_turn. Anchored on
        // the key resolution (unique to the branch); `if openui_gateway {` also matches the
        // overlay selection earlier in the handler (WR-07).
        let src = include_str!("ws.rs");
        let branch = src
            .find(concat!("openui_gateway::resolve_thesys_key", "().await"))
            .expect("gateway branch");
        let normal = src
            .find("let scrubber_ws = std::sync::Arc::new")
            .expect("normal turn path");
        assert!(branch < normal);
        let guard = src[..branch].rfind("if openui_gateway {").expect("flag guard");
        assert!(branch - guard < 200, "the key resolution sits directly under the flag guard");
    }

    #[test]
    fn openui_overlay_only_when_flagged() {
        assert!(openui_overlay_for(&req(false)).is_none());
        let (name, body) =
            openui_overlay_for(&req(true)).expect("flagged request gets the overlay");
        assert_eq!(name, "openui");
        assert!(body.starts_with(super::super::openui_route::OPENUI_SYSTEM_PROMPT));
        // ADR-0007 "Overlay content": the restrictions ride after the unmodified prompt.
        for needle in ["images", "@OpenUrl", "Query()", "Mutation()"] {
            assert!(body.contains(needle), "overlay must mention {needle}");
        }
        // WR-01: the handler selects the overlay at receipt but pushes it only inside the
        // spawned turn, after the inbound gate and the Gateway branch, right before the normal
        // `run_web_turn` path.
        // RR-08: whitespace-insensitive anchors, so a rustfmt reflow cannot break the ordering check.
        let src: String = include_str!("ws.rs")
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let select = src
            .find("openui_overlay_for(&req)")
            .expect("handler calls it");
        let gate = src
            .find("if!web_inbound_gate(&app_state,&tx,turn_id_spawn")
            .expect("spawned gate");
        let push = src
            .find("push_web_skill_overlay(&session_id_for_turn,name,body)")
            .expect("overlay push");
        let normal = src
            .find("letscrubber_ws=std::sync::Arc::new")
            .expect("normal turn path");
        assert!(select < gate && gate < push && push < normal);
    }

    #[test]
    fn queued_openui_message_round_trips_its_marker() {
        // RR-02: the queue path keeps the OpenUI flag so the drain can apply the overlay.
        let tagged = tag_queued_message("make a form", true);
        assert_ne!(tagged, "make a form");
        assert_eq!(split_queued_message(tagged), ("make a form".to_string(), true));
        let plain = tag_queued_message("hello", false);
        assert_eq!(plain, "hello");
        assert_eq!(split_queued_message(plain), ("hello".to_string(), false));
    }

    #[test]
    fn queue_drain_applies_the_openui_overlay_after_the_gate() {
        let src = include_str!("ws.rs");
        let start = src.find("split_queued_message(next_text)").expect("drain split");
        let rest = &src[start..];
        let gate = rest.find("web_inbound_gate(").expect("drain gate");
        let push = rest.find("push_web_skill_overlay(").expect("drain overlay");
        let run = rest.find("run_web_turn(").expect("drain run");
        assert!(gate < push && push < run, "gate, then overlay, then run");
        // Both queue-on-busy paths tag the pushed text.
        let prod = &src[..src.find("mod openui_overlay_tests").expect("test mod")];
        assert_eq!(prod.matches("tag_queued_message(&message,").count(), 2);
    }
}
