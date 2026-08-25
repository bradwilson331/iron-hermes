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
    let cfg =
        ironhermes_core::config::Config::load().unwrap_or_else(|_| (*app_state.config).clone());

    let stt_provider = select_stt_provider(&cfg.stt);
    let stt_available = stt_provider.is_some();
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
        // "auto": select_stt_provider picks the available provider; derive model from it.
        _ => stt_provider.as_deref().map(|p| match p {
            "groq" => cfg.stt.groq.model.clone(),
            "openai" => cfg.stt.openai.model.clone(),
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
                                            use ironhermes_tools::stt::{
                                                build_stt_registry, select_stt_provider,
                                            };

                                            let stt_registry = build_stt_registry(
                                                &ww_app_state.config.stt,
                                            );
                                            let provider_name =
                                                match select_stt_provider(&ww_app_state.config.stt) {
                                                    Some(n) => n,
                                                    None => {
                                                        warn!("wake-word-check: no STT provider; emitting no-match");
                                                        let _ = tx_ww.send(
                                                            crate::protocol::ChatStreamEvent::WakeWordResult { matched: false },
                                                        );
                                                        return;
                                                    }
                                                };
                                            let provider = match stt_registry.get(&provider_name) {
                                                Some(p) => p,
                                                None => {
                                                    warn!(provider = %provider_name, "wake-word-check: provider not in registry; emitting no-match");
                                                    let _ = tx_ww.send(
                                                        crate::protocol::ChatStreamEvent::WakeWordResult { matched: false },
                                                    );
                                                    return;
                                                }
                                            };

                                            // Write clip to temp file (mirrors full-turn path).
                                            let audio_cache_dir =
                                                ironhermes_core::constants::get_hermes_home()
                                                    .join("audio_cache");
                                            let _ = tokio::fs::create_dir_all(&audio_cache_dir).await;
                                            let file_uuid = uuid::Uuid::new_v4().to_string();
                                            let input_ext = if ww_mime.contains("mp4") { "mp4" } else { "webm" };
                                            let input_path = audio_cache_dir
                                                .join(format!("{file_uuid}-ww-in.{input_ext}"));
                                            let wav_path = audio_cache_dir
                                                .join(format!("{file_uuid}-ww.wav"));

                                            if let Err(e) =
                                                tokio::fs::write(&input_path, &ww_bytes).await
                                            {
                                                warn!(reason = %e, "wake-word-check: failed to write temp file; no-match");
                                                let _ = tx_ww.send(
                                                    crate::protocol::ChatStreamEvent::WakeWordResult { matched: false },
                                                );
                                                return;
                                            }

                                            // Transcode to WAV when ffmpeg available (same as full-turn path).
                                            let transcribe_path =
                                                if ironhermes_tools::tts::ffmpeg_available() {
                                                    let ffmpeg_status = tokio::process::Command::new("ffmpeg")
                                                        .args([
                                                            "-y", "-i",
                                                            input_path.to_str().unwrap_or(""),
                                                            "-ar", "16000", "-ac", "1", "-f", "wav",
                                                            wav_path.to_str().unwrap_or(""),
                                                        ])
                                                        .output()
                                                        .await;
                                                    match ffmpeg_status {
                                                        Ok(out) if out.status.success() => wav_path.clone(),
                                                        _ => input_path.clone(),
                                                    }
                                                } else {
                                                    input_path.clone()
                                                };

                                            let transcript_raw =
                                                match provider.transcribe(&transcribe_path).await {
                                                    Ok(t) => t,
                                                    Err(e) => {
                                                        warn!(reason = %e, "wake-word-check: STT failed; emitting no-match");
                                                        let _ = tokio::fs::remove_file(&input_path).await;
                                                        let _ = tokio::fs::remove_file(&wav_path).await;
                                                        let _ = tx_ww.send(
                                                            crate::protocol::ChatStreamEvent::WakeWordResult { matched: false },
                                                        );
                                                        return;
                                                    }
                                                };

                                            // Clean up temp files.
                                            let _ = tokio::fs::remove_file(&input_path).await;
                                            let _ = tokio::fs::remove_file(&wav_path).await;

                                            // D-13: resolve phrase from frame first,
                                            // fall back to server config when not set.
                                            let raw_phrase = ww_phrase_from_frame
                                                .unwrap_or_else(|| {
                                                    ww_app_state.config.voice.wake_word.phrase.clone()
                                                });
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
                                        use ironhermes_tools::hallucination_filter::HallucinationFilter;
                                        use ironhermes_tools::stt::{
                                            build_stt_registry, select_stt_provider,
                                        };

                                        // Build a per-call SttRegistry from the server config.
                                        // This is cheap (two Arc::new) and avoids the need for
                                        // a separate stt_registry field on AppState.
                                        let stt_registry = build_stt_registry(
                                            &app_state_stt.config.stt,
                                        );

                                        // D-06: select provider (explicit > groq > openai > None).
                                        let provider_name =
                                            match select_stt_provider(&app_state_stt.config.stt) {
                                                Some(n) => n,
                                                None => {
                                                    warn!("audio-in: no STT provider configured; dropping frame");
                                                    return;
                                                }
                                            };
                                        let provider = match stt_registry.get(&provider_name) {
                                            Some(p) => p,
                                            None => {
                                                warn!(
                                                    provider = %provider_name,
                                                    "audio-in: STT provider not found in registry; dropping"
                                                );
                                                return;
                                            }
                                        };

                                        // Build a UUID-named temp file under audio_cache.
                                        // T-36.17.8-web-path: no client-controlled path.
                                        let audio_cache_dir =
                                            ironhermes_core::constants::get_hermes_home()
                                                .join("audio_cache");
                                        let _ = tokio::fs::create_dir_all(&audio_cache_dir).await;
                                        let file_uuid =
                                            uuid::Uuid::new_v4().to_string();

                                        // Determine the input extension for ffmpeg from mime.
                                        let input_ext = if audio_mime.contains("mp4") {
                                            "mp4"
                                        } else {
                                            "webm"
                                        };
                                        let input_path = audio_cache_dir
                                            .join(format!("{file_uuid}-in.{input_ext}"));
                                        let wav_path = audio_cache_dir
                                            .join(format!("{file_uuid}.wav"));

                                        // Write the raw captured audio to the input temp file.
                                        if let Err(e) =
                                            tokio::fs::write(&input_path, &audio_bytes).await
                                        {
                                            warn!(
                                                reason = %e,
                                                "audio-in: failed to write input temp file; dropping"
                                            );
                                            return;
                                        }

                                        // Transcode to 16 kHz mono WAV when ffmpeg is available
                                        // (RESEARCH Pitfall 5: both Groq and OpenAI accept webm
                                        // directly — pass through when ffmpeg is absent).
                                        let transcribe_path =
                                            if ironhermes_tools::tts::ffmpeg_available() {
                                                // ffmpeg -i <input> -ar 16000 -ac 1 -f wav <output>
                                                let ffmpeg_status = tokio::process::Command::new(
                                                    "ffmpeg",
                                                )
                                                .args([
                                                    "-y",
                                                    "-i",
                                                    input_path.to_str().unwrap_or(""),
                                                    "-ar",
                                                    "16000",
                                                    "-ac",
                                                    "1",
                                                    "-f",
                                                    "wav",
                                                    wav_path.to_str().unwrap_or(""),
                                                ])
                                                .output()
                                                .await;
                                                match ffmpeg_status {
                                                    Ok(out) if out.status.success() => {
                                                        wav_path.clone()
                                                    }
                                                    Ok(out) => {
                                                        warn!(
                                                            stderr = %String::from_utf8_lossy(&out.stderr),
                                                            "audio-in: ffmpeg transcode failed; falling back to passthrough"
                                                        );
                                                        input_path.clone()
                                                    }
                                                    Err(e) => {
                                                        warn!(
                                                            reason = %e,
                                                            "audio-in: ffmpeg spawn failed; falling back to passthrough"
                                                        );
                                                        input_path.clone()
                                                    }
                                                }
                                            } else {
                                                // ffmpeg absent — pass webm/mp4 directly.
                                                // Both Groq and OpenAI Whisper accept these.
                                                input_path.clone()
                                            };

                                        // Call SttProvider::transcribe.
                                        let transcript_raw =
                                            match provider.transcribe(&transcribe_path).await {
                                                Ok(t) => t,
                                                Err(e) => {
                                                    warn!(
                                                        reason = %e,
                                                        "audio-in: STT transcription failed; dropping"
                                                    );
                                                    // Clean up temp files before returning.
                                                    let _ = tokio::fs::remove_file(&input_path).await;
                                                    let _ = tokio::fs::remove_file(&wav_path).await;
                                                    return;
                                                }
                                            };

                                        // Clean up temp files after successful transcription.
                                        let _ = tokio::fs::remove_file(&input_path).await;
                                        let _ = tokio::fs::remove_file(&wav_path).await;

                                        // D-12: apply HallucinationFilter.
                                        let transcript = match HallucinationFilter::filter(
                                            &transcript_raw,
                                        ) {
                                            Some(t) => t.to_string(),
                                            None => {
                                                // Filtered out — no empty bubble per UI-SPEC.
                                                info!(
                                                    "audio-in: transcript rejected by hallucination filter; no message sent"
                                                );
                                                return;
                                            }
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

                            let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>();
                            let app_state = app_state.clone();
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
                                match app_state.queue.try_push(&key, message.clone()) {
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
                                            app_state.resolver.clone(),
                                        );

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
                                            other => {
                                                format!("{other:?}")
                                            }
                                        };
                                        if !text.is_empty() {
                                            let _ = tx.send(ChatStreamEvent::Delta { turn_id: uuid::Uuid::nil(), text });
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
                                            app_state.runtime.skill_registry(),
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
                                    let _ = app_state.queue.try_push(&key, message.clone());
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
                            let handle = tokio::spawn(async move {
                                // RAII: holds semaphore permits for the lifetime of this task.
                                let _per = per_permit;
                                let _global = global_permit;

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
                                    Box::new(move |name: &str, args: &str| {
                                        let _ = tx_tool.send(ChatStreamEvent::ToolCallStart { turn_id: tid,
                                            name: name.to_string(),
                                            args: args.to_string(),
                                        });
                                    });

                                let tx_tool_result = tx.clone();
                                let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                    Box::new(move |name: &str, success: bool, _output: &str| {
                                        let _ = tx_tool_result.send(ChatStreamEvent::ToolCallEnd { turn_id: tid,
                                            name: name.to_string(),
                                            success,
                                        });
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
                                    .unwrap_or_else(|_| app_state.config.voice.auto_tts);

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
                                let video_size_cap = match app_state.config.video_gen.max_inline_bytes {
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
                                                Box::new(move |name: &str, args: &str| {
                                                    let _ = tx_tool.send(
                                                        ChatStreamEvent::ToolCallStart { turn_id: tid_s,
                                                            name: name.to_string(),
                                                            args: args.to_string(),
                                                        },
                                                    );
                                                });
                                            let tx_tool_result = tx_s.clone();
                                            let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                                Box::new(move |name: &str, success: bool, _output: &str| {
                                                    let _ = tx_tool_result.send(
                                                        ChatStreamEvent::ToolCallEnd { turn_id: tid_s,
                                                            name: name.to_string(),
                                                            success,
                                                        },
                                                    );
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
                                                        app_state_s.config.voice.auto_tts
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
                                                    let next_text_owned = next_text;
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
                                                            Box::new(move |name: &str, args: &str| {
                                                                let _ = tx_tool.send(ChatStreamEvent::ToolCallStart { turn_id: tid_drain, name: name.to_string(), args: args.to_string() });
                                                            });
                                                        let tx_tool_result = tx_drain.clone();
                                                        let tool_result_callback: ironhermes_agent::agent_loop::ToolResultCallback =
                                                            Box::new(move |name: &str, success: bool, _output: &str| {
                                                                let _ = tx_tool_result.send(ChatStreamEvent::ToolCallEnd { turn_id: tid_drain, name: name.to_string(), success });
                                                            });
                                                        let tx_subagent = tx_drain.clone();
                                                        { let mut guard = app_state_drain.subagent_callback_slot.lock().await; *guard = Some(tx_subagent); }
                                                        let _slot_guard = SubagentCallbackSlotGuard { slot: app_state_drain.subagent_callback_slot.clone() };
                                                        let auto_tts_drain = ironhermes_core::config::Config::load()
                                                            .map(|c| c.voice.auto_tts)
                                                            .unwrap_or_else(|_| app_state_drain.config.voice.auto_tts);
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
                                                        let result = app_state_drain.run_web_turn(
                                                            &session_id_spawn, &next_text_owned,
                                                            stream_callback, Some(tool_progress_callback), Some(tool_result_callback), tts_wiring_drain,
                                                            // Phase 39.2 Plan 04: TurnRegistry UUID for bb correlation.
                                                            Some(tid_drain),
                                                            messaging_wiring_drain,
                                                            // Phase 46.7 Plan 04 (D-09): queue-drain turns never carry
                                                            // attachments (queue stores text only).
                                                            Vec::new(),
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

    // Phase 40.5 Plan 08 (T-40.5-08-01): validate wire-supplied slug server-side.
    // A crafted/unknown slug must not reach provider selection — apply the same
    // is_known_identity gate used on the client write path (defense in depth).
    let validated_identity = active_identity
        .filter(|slug| crate::components::hermes_app::avatar_logic::is_known_identity(slug));

    // Phase 40.5 Plan 08 (D-11): resolve per-identity TTS config.
    // Falls back to global TTS config when active_identity is None or unknown.
    let effective_tts = config.effective_tts_config_for_identity(validated_identity);

    // Build TTS registry from effective config (same factory used by the TTS tool).
    let tts_registry = ironhermes_tools::tts::build_tts_registry(&effective_tts);
    let provider = match tts_registry.get(&effective_tts.provider) {
        Some(p) => p,
        None => {
            tracing::debug!(
                provider = %effective_tts.provider,
                "auto_speak_reply: TTS provider not registered; skipping Mode B synthesis"
            );
            return;
        }
    };

    // Resolve output path inside audio_cache/ (same directory as TTS tool).
    let audio_dir = get_hermes_home().join("audio_cache");
    if let Err(e) = std::fs::create_dir_all(&audio_dir) {
        tracing::warn!(
            error = %e,
            "auto_speak_reply: failed to create audio_cache dir; skipping synthesis"
        );
        return;
    }
    let ext = if effective_tts.provider == "elevenlabs"
        && effective_tts.elevenlabs.output_format == "opus"
    {
        "opus"
    } else {
        "mp3"
    };
    let output_path = audio_dir.join(format!("{}.{}", uuid::Uuid::new_v4(), ext));

    // Synthesize. Errors (e.g. network failure, provider unavailable) are non-fatal.
    let written_path = match provider.synthesize(text, &output_path).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "auto_speak_reply: TTS synthesis failed; skipping Mode B AudioOut"
            );
            return;
        }
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
