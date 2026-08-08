//! Phase 36.17.12 Plan 03 — Browser-side OpenAI Realtime WebRTC session module.
//! Phase 39.3 Plan 04 — Function-call bridge (D-02/D-03) + transcript persistence (D-04).
//!
//! Implements the open-mic (Mode C) path: fetches an ephemeral token from the
//! server (Plan 02 `issue_realtime_token`), establishes a WebRTC peer connection
//! with echo-cancelled mic capture, maps provider DataChannel events to the binary
//! orb states, and taps the remote audio stream into the existing AnalyserNode.
//!
//! # Design decisions
//!
//! - D-02/D-03: On `response.done`, extracts each `function_call` item ({call_id, name,
//!   arguments}) and relays it to `realtime_tool_call`. On Output, calls
//!   `send_function_call_output` (the single shared D-05b send site). On ApprovalPending,
//!   raises the `approval_pending` signal for Plan 05's card (no premature send).
//! - D-04: On `conversation.item.input_audio_transcription.completed`, persists the user
//!   transcript via `realtime_save_transcript`.
//! - D-04/D-05: WebRTC-direct transport with ephemeral token (browser never sees the
//!   permanent key).
//! - D-08: Provider-driven barge-in via server_vad (`interrupt_response: true`).
//! - D-09: Maps provider events → existing Listening/Speaking states (no new variant).
//!   `response.done` returns Some(Listening) from `parse_realtime_event`; function_call
//!   extraction is handled separately (no extra orb state change introduced).
//! - D-10: Remote audio stream tapped into `VOICE_LOOP_SLOT.analyser` via
//!   `tap_realtime_stream_analyser` so the collapsed orb pulses with provider audio.
//!
//! # wasm32 gating
//!
//! The entire WebRTC body is inside `#[cfg(target_arch = "wasm32")]`. The non-wasm
//! arm is a no-op stub so the server/native builds compile without browser APIs.
//! `parse_realtime_event` is target-independent (pure Rust) and natively testable.
//!
//! # Exclusive mic gate (D-06 / Pitfall 5)
//!
//! The realtime mic is a FRESH `getUserMedia` capture — distinct from the
//! `VOICE_LOOP_SLOT.stream` used by the turn-based loop. Both paths are mutually
//! exclusive (voice_mode.rs mode gate never starts both), so there is never a
//! concurrent capture that could starve echo cancellation.

// The realtime WebRTC session functions (start/teardown/tap) run only in the
// browser; `parse_realtime_event` is target-independent and natively unit-tested.
// Native dead-code analysis cannot see the wasm call sites. Silence native-only
// dead_code without deleting web-live code; the wasm build still lints normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use dioxus::prelude::Signal;
// `WritableExt` provides `.set()` on signals; `ReadableExt` provides `.read()`.
// Both are used only inside the wasm DataChannel event handlers — gate to wasm
// so native builds don't see them as unused imports.
#[cfg(target_arch = "wasm32")]
use dioxus::prelude::{ReadableExt, WritableExt};

use crate::components::hermes_app::screens::voice_mode::VoiceModeState;

// ── Approval-pending state (D-03 / Plan 04 → Plan 05 contract) ──────────────
//
// When `realtime_tool_call` returns `ApprovalPending`, the browser raises this
// signal so Plan 05's orb overlay card can render the Approve / Deny UI.
// The signal carries `Some(info)` while a call is pending and `None` when
// cleared (after Approve/Deny or session teardown).
//
// This struct is target-independent so Plan 05 can use it without a wasm gate.
// The `send_function_call_output` helper (wasm-gated) is the SINGLE send site:
// Plan 05's Approve handler calls it after the user approves; this plan's Output
// branch calls it for auto-approved calls. Exactly one send per call_id.

/// Carries the information needed to render the approval card (Plan 05).
///
/// Populated on `ApprovalPending` from `realtime_tool_call`. Plan 05 reads this
/// via `RealtimeApprovalCtx` from `use_context`. The `turn_id` field is required
/// by the `realtime_approve(turn_id, call_id, approved)` server fn (Plan 02).
#[derive(Clone, Debug, PartialEq)]
pub struct ApprovalPendingInfo {
    pub call_id: String,
    /// Server-assigned turn_id — required by realtime_approve (Plan 02) so the
    /// Approve/Deny handler can re-validate turn liveness on the server.
    pub turn_id: String,
    pub tool_name: String,
    pub arguments: String,
}

// ── Single shared DataChannel send helper (D-05b) ────────────────────────────
//
// The ONLY site that emits function_call_output + response.create into the
// session. Both this plan's auto-approved Output branch and Plan 05's Approve
// handler call THIS helper — so exactly one send pair is emitted per call_id.
// Never inline a second copy.

/// Emit `conversation.item.create` (function_call_output) + `response.create`
/// into the DataChannel so the model voices the tool result.
///
/// - `call_id`: must be the call_id from the originating `function_call` item
///   (Pitfall 1 — use the item's own call_id, never response.id).
/// - `output`: the tool result string.
///
/// This is the D-05b single-send site. Both the auto-approved path (this module)
/// AND Plan 05's Approve handler call this helper. Never duplicate inline.
#[cfg(target_arch = "wasm32")]
pub fn send_function_call_output(dc: &web_sys::RtcDataChannel, call_id: &str, output: &str) {
    // Build the function_call_output item frame.
    // GA Realtime schema: conversation.item.create with type "function_call_output".
    let item_frame = serde_json::json!({
        "type": "conversation.item.create",
        "item": {
            "type": "function_call_output",
            "call_id": call_id,
            "output": output
        }
    })
    .to_string();

    if let Err(e) = dc.send_with_str(&item_frame) {
        web_sys::console::warn_1(
            &format!("[realtime_session] send_function_call_output item create failed: {e:?}")
                .into(),
        );
    }

    // Follow up immediately with response.create so the model voices the result (D-05b).
    let response_frame = serde_json::json!({ "type": "response.create" }).to_string();
    if let Err(e) = dc.send_with_str(&response_frame) {
        web_sys::console::warn_1(
            &format!("[realtime_session] send_function_call_output response.create failed: {e:?}")
                .into(),
        );
    }
}

/// Clone the live DataChannel from the thread-local session slot.
///
/// Returns `Some(dc)` while a realtime session is active; `None` otherwise.
/// Used by Plan 05's Approve/Deny handlers in voice_mode.rs so they can call
/// the shared `send_function_call_output` helper without holding a borrow on the
/// slot across an .await point (Pattern B — clone then drop borrow).
#[cfg(target_arch = "wasm32")]
pub fn get_realtime_dc() -> Option<web_sys::RtcDataChannel> {
    REALTIME_SESSION_SLOT.with(|slot| slot.borrow().as_ref().map(|h| h.dc.clone()))
}

// ── Cooperative realtime cancel (R39.1-10) ───────────────────────────────────
//
// DECISION 1 — CONFIRM: The realtime wasm client has NO persistent server
// connection today.  Its only server contact is the one-shot
// `issue_realtime_token()` server fn (Step 1 in `start_realtime_session`).
// The live media+event path (`RtcDataChannel "oai-events"`) terminates at
// OpenAI directly — NOT at our server.  Therefore a server-initiated push to
// the browser client is not available, and cooperative cancel must be
// CLIENT-PULLED (the client polls us).
//
// DECISION 2 — CHOSEN TRANSPORT: A periodic `realtime_heartbeat(turn_id)`
// server fn that the client calls on a ~2-3 s interval using
// `wasm_bindgen_futures::spawn_local` + `gloo_timers::future::sleep` (both
// already used in this crate — no new deps added).  The heartbeat returns
// `RealtimeTurnSignal::{Continue, Cancelled}`.  On `Cancelled`, the client
// calls `teardown_realtime_session()` (existing, line ~96) and stops the loop.
// The heartbeat also updates a `last_seen` Instant stored on the process-wide
// AppState so `realtime_prune` can reap sessions whose client stopped
// heartbeating (browser closed / crashed) without ever calling
// `realtime_session_end`.  Rationale for poll over the classic chat WS: the
// realtime surface does not share that WS, and the existing `TurnCancelled`
// ChatStreamEvent (Plan 02) only reaches classic-WS clients — reusing it
// would require the realtime client to also hold the chat WS, which it does
// not.
//
// DECISION 3 — CONFIRM: Cancel flows through the SHARED `TurnRegistry`.
// A `/agents cancel <id>` (or agents-page cancel button) on ANY surface calls
// `cancel_one(turn_id)` → the realtime `TurnEntry`'s `CancellationToken` is
// cancelled → the next `realtime_heartbeat` poll observes `token.is_cancelled()`
// → returns `Cancelled`.  This keeps `TurnRegistry` as the single control point
// (D-09) while respecting that the realtime transport is browser-managed WebRTC.
// No media relay is involved — audio stays browser↔OpenAI direct throughout.
//
// Task 3 implements against these decisions.
// ─────────────────────────────────────────────────────────────────────────────

// ── Thread-local slot (mirrors VOICE_LOOP_SLOT in voice_loop.rs) ────────────

#[cfg(target_arch = "wasm32")]
thread_local! {
    static REALTIME_SESSION_SLOT: std::cell::RefCell<Option<RealtimeSessionHandle>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static REALTIME_SESSION_SLOT: std::cell::RefCell<Option<()>> =
        const { std::cell::RefCell::new(None) };
}

/// Browser resources held for one realtime WebRTC session.
#[cfg(target_arch = "wasm32")]
pub struct RealtimeSessionHandle {
    pub pc: web_sys::RtcPeerConnection,
    pub dc: web_sys::RtcDataChannel,
    pub stream: web_sys::MediaStream,
    /// Hidden `<audio>` element that sinks the remote MediaStream so the
    /// agent voice is audible (BUG 2 fix). Chromium WebRTC pitfall: a remote
    /// track routed only through Web Audio is silent — the stream must also
    /// be attached to a media element. Retained here so teardown can clear
    /// srcObject and prevent resource leaks.
    pub audio_sink: web_sys::HtmlAudioElement,
    /// Phase 39.1 Plan 07 (R39.1-10): server-assigned turn_id returned by
    /// `issue_realtime_token`. Echoed back to `realtime_heartbeat` /
    /// `realtime_session_end` to identify this session in the TurnRegistry.
    pub turn_id: String,
}

// ── Pure, target-independent event parser (natively testable seam) ───────────

/// Map a provider DataChannel event `type` string to a `VoiceModeState` change.
///
/// Returns `Some(state)` when the event should trigger an orb state transition;
/// returns `None` when the event should be silently ignored (no state change).
/// Never panics on unknown or malformed type strings.
///
/// Mapping (D-09):
/// - `input_audio_buffer.speech_started` → `Listening`  (user started speaking)
/// - `response.audio.delta`              → `Speaking`   (agent audio chunk)
/// - `response.audio.done`              → `Listening`  (agent audio finished)
/// - `response.done`                     → `Listening`  (full response complete)
/// - `response.cancelled`               → `Listening`  (barge-in: agent interrupted)
/// - everything else                     → `None`       (no state change)
pub fn parse_realtime_event(event_type: &str) -> Option<VoiceModeState> {
    match event_type {
        "input_audio_buffer.speech_started" => Some(VoiceModeState::Listening),
        "response.audio.delta" => Some(VoiceModeState::Speaking),
        "response.audio.done" => Some(VoiceModeState::Listening),
        "response.done" => Some(VoiceModeState::Listening),
        "response.cancelled" => Some(VoiceModeState::Listening),
        // All other events (speech_stopped, session.created, unknown, empty) → no change.
        _ => None,
    }
}

// ── Teardown (idempotent — None slot is a no-op) ─────────────────────────────

/// Release all held browser resources for the realtime session.
///
/// Closes the RTCPeerConnection and stops the mic tracks. Idempotent —
/// calling when the slot is empty is a no-op. Called via `use_drop` in
/// `VoiceModeScreen` alongside `teardown_voice_loop`.
///
/// Phase 39.1 Plan 07 (R39.1-10): on user-initiated end, also fires
/// `realtime_session_end(turn_id)` as a fire-and-forget `spawn_local` so the
/// TurnRegistry deregisters promptly (not only via stale-prune). Idempotent
/// server-side if the cancel-heartbeat path already deregistered.
pub fn teardown_realtime_session() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        REALTIME_SESSION_SLOT.with(|slot| {
            if let Some(handle) = slot.borrow_mut().take() {
                // Stop mic tracks so the browser recording indicator goes away.
                let tracks = handle.stream.get_tracks();
                for i in 0..tracks.length() {
                    if let Some(track) = tracks.get(i).dyn_ref::<web_sys::MediaStreamTrack>() {
                        track.stop();
                    }
                }
                // Detach the remote audio sink (BUG 2 fix): clear srcObject so
                // the browser releases the MediaStream reference and stops playback.
                use web_sys::HtmlMediaElement;
                let media_el: &HtmlMediaElement = handle.audio_sink.as_ref();
                media_el.set_src_object(None);
                // Phase 01 Plan 04 (Pitfall 1): clear the avatar's realtime-stream
                // global so a reconnect re-publishes a FRESH stream and avatar.js's
                // late-attach picks it up (a stale stream would otherwise feed
                // wawa a dead MediaStream). Mirrors the src_object detach above —
                // no new transport, just nulling the re-exposed reference (D-12).
                if let Some(win) = web_sys::window() {
                    let _ = js_sys::Reflect::set(
                        &win,
                        &wasm_bindgen::JsValue::from_str("__ihRealtimeStream"),
                        &wasm_bindgen::JsValue::NULL,
                    );
                }
                // Close the peer connection (ignore errors — already closing is fine).
                handle.pc.close();

                // Phase 39.1 Plan 07 (R39.1-10): deregister from TurnRegistry
                // promptly. Fire-and-forget spawn_local — we are in a sync drop
                // context (use_drop), cannot .await here. Server-side
                // realtime_session_end is idempotent (no-op if already deregistered
                // by a cancel-heartbeat path).
                let tid = handle.turn_id.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = crate::server::api::realtime_session_end(tid).await;
                });
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        REALTIME_SESSION_SLOT.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}

// ── Main wasm32-only session start ───────────────────────────────────────────

/// Start an OpenAI Realtime WebRTC session (wasm32 only).
///
/// Steps:
/// 1. Fetch ephemeral token + session_id via `issue_realtime_token` server fn.
/// 2. Guard: bail if a session is already in `REALTIME_SESSION_SLOT`.
/// 3. Create `RTCPeerConnection`.
/// 4. Set `ontrack` — taps the remote `MediaStream` into `VOICE_LOOP_SLOT.analyser`.
/// 5. getUserMedia with `echoCancellation: true` + `noiseSuppression: true`; add mic track.
/// 6. Create the `"oai-events"` DataChannel BEFORE creating the offer (Pitfall 6).
///    Wire `onmessage` to:
///    - parse events → update `voice_state` (D-09);
///    - on `response.done`, extract each `function_call` item and relay to
///      `realtime_tool_call` (D-02/D-03); on Output call `send_function_call_output`;
///      on ApprovalPending raise `approval_pending` signal for Plan 05's card;
///    - on `conversation.item.input_audio_transcription.completed`, persist via
///      `realtime_save_transcript` (D-04).
///    Wire `onopen` to send the `session.update` VAD config (D-08 provider-driven barge-in).
/// 7. SDP: create_offer → set_local_description → POST SDP to OpenAI → set_remote_description.
/// 8. Store the handle in `REALTIME_SESSION_SLOT` and return `Ok(())`.
///
/// The `approval_pending` signal is set to `Some(ApprovalPendingInfo{...})` when a
/// tool call requires operator approval (D-03). Plan 05's card reads this signal.
/// The signal stays `None` for auto-approved calls (Output branch calls
/// `send_function_call_output` directly) and on Rejected (warn + send error output).
///
/// `in_flight` is set true when any tool call is dispatched and cleared (false) when
/// Output/Rejected is received or on error — drives the D-05a in-flight badge in
/// Plan 05's orb overlay.
///
/// Returns `Err(JsValue)` on any failure so the caller (`voice_mode.rs`) can
/// trigger the D-07 fallback (degraded path + `RealtimeDegradedCtx` signal).
///
/// All error paths warn-not-panic via `web_sys::console::warn_1`.
#[cfg(target_arch = "wasm32")]
pub async fn start_realtime_session(
    mut voice_state: Signal<VoiceModeState>,
    mut transcript: Signal<String>,
    approval_pending: Signal<Option<ApprovalPendingInfo>>,
    mut in_flight: Signal<bool>,
    // D-04 RC-4: the chat bubble list from HermesApp root — realtime turns are
    // pushed here for live in-session display (mirrors the WS UserTranscript path).
    mut bubbles: Signal<Vec<crate::components::hermes_app::screens::chat::ChatBubble>>,
    // D-04 RC-4: monotonic bubble id counter from HermesApp root.
    mut next_id: Signal<u64>,
    // D-04 RC-1: the user's real chat session key ("agent:main:web:dm:{uuid}"),
    // read from SessionIdContext by the caller (voice_mode.rs) BEFORE the spawn
    // so it is available as an owned String here without a use_context call
    // inside an async task (Dioxus pattern B — context reads must happen in
    // component scope, not in spawned async blocks).
    chat_session_id: String,
    // Phase 40.5 Plan 03 (D-12): active identity slug, read from AvatarModeCtx by
    // voice_mode.rs BEFORE the spawn (Pattern B). Passed to issue_realtime_token
    // so the server can apply per-identity realtime voice override. None = global.
    active_identity: Option<String>,
    // G-41.2-8: AudioPlaybackActiveCtx signal from the HermesApp root. The realtime
    // path has no turn-based AudioOut handler (mod.rs:592 covers only WS-frame TTS),
    // so we mirror agent-speaking into it here so the half-duplex guard (Test Voice
    // disable + VAD pause) engages during realtime agent speech.
    mut audio_playback_active: Signal<bool>,
) -> Result<(), wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    // ── Step 1: Fetch ephemeral token + server turn_id ────────────────────────
    // D-04 RC-1: pass the real chat session id to issue_realtime_token so the
    // server validates it, calls ensure_web_session (satisfying FK), and echoes
    // it back as chat_session_id in IssueRealtimeTokenResult.
    // Phase 40.5 Plan 03 (D-12): also pass active_identity for per-identity voice.
    let token_result = crate::server::api::issue_realtime_token(chat_session_id, active_identity)
        .await
        .map_err(|e| {
            let msg = format!("[realtime_session] token fetch failed: {e}");
            web_sys::console::warn_1(&msg.into());
            wasm_bindgen::JsValue::from_str("token_fetch_failed")
        })?;
    // Extract owned values before any await (Pattern B — no borrow across .await).
    let token = token_result.token;
    let turn_id_str = token_result.turn_id;
    // session_id is the turn-derived realtime id (internal use only post RC-1).
    let session_id_str = token_result.session_id;
    // D-04 RC-1: use the real chat session key as the WRITE key for transcripts.
    let chat_session_id_str = token_result.chat_session_id;

    // ── Step 2: Guard — do not start a second session ─────────────────────────
    let already_active = REALTIME_SESSION_SLOT.with(|slot| slot.borrow().is_some());
    if already_active {
        web_sys::console::warn_1(&"[realtime_session] session already active, skipping".into());
        return Ok(());
    }

    // ── Step 3: Create RTCPeerConnection ─────────────────────────────────────
    let pc_config = web_sys::RtcConfiguration::new();
    let pc = web_sys::RtcPeerConnection::new_with_configuration(&pc_config).map_err(|e| {
        web_sys::console::warn_1(
            &format!("[realtime_session] RTCPeerConnection failed: {e:?}").into(),
        );
        e
    })?;

    // ── Step 4: ontrack — sink remote MediaStream to audio element + analyser ──
    // BUG 2 fix: Chromium WebRTC pitfall — a `MediaStreamAudioSourceNode` built
    // from a remote WebRTC track yields SILENCE in Web Audio unless the track is
    // also consumed by a media element that forces the browser to decode it
    // (Chrome bug 121673). So we attach the stream to a hidden `<audio>` element
    // purely as that decode "kickstart".
    //
    // AUDIBLE-PATH OWNERSHIP (Phase 41.2 "stereo"/double-playback fix): the single
    // audible path is `analyser → AudioContext.destination` (wired in
    // `tap_realtime_stream_analyser`; that destination routing is also what pulls
    // the analyser so `read_fft_bins` gets data and the orb pulses). Therefore the
    // `<audio>` element MUST be **muted** — an unmuted element is a SECOND audible
    // sink playing the same stream, which is exactly the doubled/"stereo" playback
    // reported in UAT. A muted element still decodes the track (satisfying the
    // Chrome workaround) without emitting sound. Do NOT unmute it.
    //   (a) Create a hidden, muted <audio autoplay> element → Chrome decode kickstart.
    //   (b) Tap the same stream into VOICE_LOOP_SLOT.analyser → orb pulses (D-10)
    //       and provides the sole audible output via AudioContext.destination.
    //
    // We create the audio element outside the closure so we can store it in
    // the session handle for teardown. The closure gets a clone of the element
    // reference (via JsCast from the window document).
    let audio_sink = {
        let window = web_sys::window()
            .ok_or_else(|| wasm_bindgen::JsValue::from_str("no window for audio sink"))?;
        let document = window
            .document()
            .ok_or_else(|| wasm_bindgen::JsValue::from_str("no document for audio sink"))?;
        let el = document.create_element("audio").map_err(|e| {
            web_sys::console::warn_1(
                &format!("[realtime_session] create audio element: {e:?}").into(),
            );
            e
        })?;
        // Cast to HtmlAudioElement; set autoplay so playback starts automatically
        // when srcObject is set (required: autoplay must be set before srcObject
        // on some browsers).
        let audio_el: web_sys::HtmlAudioElement = el
            .dyn_into()
            .map_err(|_| wasm_bindgen::JsValue::from_str("audio element cast failed"))?;
        {
            use web_sys::HtmlMediaElement;
            let media_el: &HtmlMediaElement = audio_el.as_ref();
            media_el.set_autoplay(true);
            // Muted: this element exists only as the Chrome WebRTC→WebAudio decode
            // kickstart (bug 121673). The audible path is analyser→destination, so
            // an unmuted element would double the playback ("stereo" UAT report).
            // Muted autoplay is also always permitted (no user-gesture rejection).
            media_el.set_muted(true);
        }
        audio_el
    };

    {
        let audio_sink_for_ontrack = audio_sink.clone();
        let ontrack = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::RtcTrackEvent)>::wrap(
            Box::new(move |evt: web_sys::RtcTrackEvent| {
                let streams = evt.streams();
                if streams.length() > 0 {
                    if let Some(stream_val) = streams.get(0).dyn_ref::<web_sys::MediaStream>() {
                        // (a) Sink remote stream to audio element → audible agent voice.
                        {
                            use web_sys::HtmlMediaElement;
                            let media_el: &HtmlMediaElement = audio_sink_for_ontrack.as_ref();
                            media_el.set_src_object(Some(stream_val));
                            // Fire play() — ignore the returned Promise (autoplay handles
                            // it; the Promise rejection for "user gesture required" is
                            // swallowed intentionally — autoplay policy is handled via
                            // the autoplay attribute set above during getUserMedia, which
                            // counts as a user activation context in modern browsers).
                            let _ = media_el.play();
                        }
                        // (b) Also tap into analyser for orb pulse (D-10).
                        crate::components::hermes_app::voice_loop::tap_realtime_stream_analyser(
                            stream_val,
                        );
                        // (c) Phase 01 Plan 04 (D-04/D-12, Pitfall 1): re-expose
                        // the EXISTING remote OpenAI MediaStream to JS so
                        // avatar.js can `createMediaStreamSource(...)` for wawa's
                        // own analyser. This is a single Reflect::set on the same
                        // stream we already sink + tap — NOT a new
                        // RtcPeerConnection / track / DataChannel / WebSocket /
                        // sidecar (D-12). The orb tap and audio sink above are
                        // untouched. avatar.js late-attaches to this global
                        // (Pitfall 7 resume on user gesture), so publishing it on
                        // every ontrack is sufficient.
                        if let Some(win) = web_sys::window() {
                            if let Err(e) = js_sys::Reflect::set(
                                &win,
                                &wasm_bindgen::JsValue::from_str("__ihRealtimeStream"),
                                stream_val,
                            ) {
                                web_sys::console::warn_1(&e);
                            }
                        }
                    }
                }
            }),
        );
        pc.set_ontrack(Some(ontrack.as_ref().unchecked_ref()));
        ontrack.forget();
    }

    // ── Step 5: getUserMedia with echo cancellation (NOT VOICE_LOOP_SLOT.stream) ─
    // Fresh capture — must not reuse the turn-based mic stream (Pitfall 5).
    // Use js_sys::Reflect::set to build constraints with echoCancellation + noiseSuppression
    // (Pitfall 4 mitigation; plain JsValue::TRUE does not set audio constraint options).
    let window = web_sys::window().ok_or_else(|| wasm_bindgen::JsValue::from_str("no window"))?;
    let media_devices = window.navigator().media_devices().map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] media_devices: {e:?}").into());
        e
    })?;

    // Build audio constraints object: { echoCancellation: true, noiseSuppression: true }
    let audio_constraints = js_sys::Object::new();
    js_sys::Reflect::set(
        &audio_constraints,
        &wasm_bindgen::JsValue::from_str("echoCancellation"),
        &wasm_bindgen::JsValue::TRUE,
    )
    .map_err(|e| {
        web_sys::console::warn_1(
            &format!("[realtime_session] reflect echoCancellation: {e:?}").into(),
        );
        e
    })?;
    js_sys::Reflect::set(
        &audio_constraints,
        &wasm_bindgen::JsValue::from_str("noiseSuppression"),
        &wasm_bindgen::JsValue::TRUE,
    )
    .map_err(|e| {
        web_sys::console::warn_1(
            &format!("[realtime_session] reflect noiseSuppression: {e:?}").into(),
        );
        e
    })?;

    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&audio_constraints);
    constraints.set_video(&wasm_bindgen::JsValue::FALSE);

    let stream_promise = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|e| {
            web_sys::console::warn_1(&format!("[realtime_session] getUserMedia: {e:?}").into());
            e
        })?;

    let stream_val = JsFuture::from(stream_promise).await.map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] getUserMedia await: {e:?}").into());
        e
    })?;

    let mic_stream: web_sys::MediaStream = stream_val.dyn_into().map_err(|e| {
        web_sys::console::warn_1(&"[realtime_session] stream cast failed".into());
        e
    })?;

    // Add mic track to peer connection so the provider receives our audio.
    let audio_tracks = mic_stream.get_audio_tracks();
    for i in 0..audio_tracks.length() {
        if let Some(track) = audio_tracks.get(i).dyn_ref::<web_sys::MediaStreamTrack>() {
            let _ = pc.add_track_0(track, &mic_stream);
        }
    }

    // ── Step 5b: Fetch the GA-shaped session config (BUG 5 fix) ──────────────
    // The session.update VAD/noise schema is built SERVER-side from config
    // (voice.realtime_* — noise_reduction, vad_mode, threshold). The browser no
    // longer hardcodes it. On error, fall back to a safe GA-shape default
    // (far_field + semantic_vad) so realtime still works.
    let session_cfg = crate::server::api::realtime_session_config()
        .await
        .unwrap_or_else(|e| {
            web_sys::console::warn_1(
                &format!("[realtime_session] session config fetch failed: {e}; using default").into(),
            );
            // Include tools/tool_choice keys for GA-schema consistency. NOTE: tools is
            // empty here because the real ToolRegistry is only available server-side; if
            // this fallback is hit the agent has NO tools (a degraded session) — the warn
            // above is the signal. This is a safety net, not a substitute for the server cfg.
            r#"{"type":"realtime","tools":[],"tool_choice":"auto","audio":{"input":{"noise_reduction":{"type":"far_field"},"turn_detection":{"type":"semantic_vad","create_response":true,"interrupt_response":true},"transcription":null}}}"#.to_string()
        });

    // ── Step 6: Create DataChannel BEFORE offer (Pitfall 6) ──────────────────
    // Creating the DataChannel first ensures it is included in the SDP offer.
    // If created after the offer, the channel is not negotiated and will not open.
    let dc_init = web_sys::RtcDataChannelInit::new();
    let dc = pc.create_data_channel_with_data_channel_dict("oai-events", &dc_init);

    // Wire onmessage: parse event type → update voice_state (D-09);
    // on response.done extract function_call items and relay (D-02/D-03);
    // on transcription.completed persist via realtime_save_transcript (D-04).
    {
        // Clone owned values into the closure (Pitfall 6 — no Signal borrow across .await).
        let dc_for_msg = dc.clone();
        let turn_id_for_msg = turn_id_str.clone();
        // D-04 RC-1: use the real chat session key as the transcript write key.
        let chat_session_id_for_msg = chat_session_id_str.clone();
        // session_id_str (turn-derived) is no longer used as a write key after RC-1.
        // Retain the binding with _ prefix so the value is still captured (the
        // REALTIME_SESSION_SLOT stores it for teardown / heartbeat).
        let _session_id_for_msg = session_id_str.clone();
        let onmessage = wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::MessageEvent)>::wrap(
            Box::new(move |evt: web_sys::MessageEvent| {
                let data = evt.data();
                if let Some(text) = data.as_string() {
                    // Parse the JSON frame — extract the "type" field.
                    if let Ok(parsed) = js_sys::JSON::parse(&text) {
                        let type_val =
                            js_sys::Reflect::get(&parsed, &wasm_bindgen::JsValue::from_str("type"))
                                .unwrap_or(wasm_bindgen::JsValue::UNDEFINED);
                        if let Some(event_type) = type_val.as_string() {
                            // CR-03 / D-07: surface provider "error" events so a
                            // rejected session.update or runtime error is no longer
                            // silent. parse_realtime_event returns None for "error"
                            // (no orb state change — D-09 contract preserved).
                            if event_type == "error" {
                                // Extract the nested error message/code if present.
                                let err_msg = js_sys::Reflect::get(
                                    &parsed,
                                    &wasm_bindgen::JsValue::from_str("error"),
                                )
                                .ok()
                                .and_then(|v| {
                                    js_sys::Reflect::get(
                                        &v,
                                        &wasm_bindgen::JsValue::from_str("message"),
                                    )
                                    .ok()
                                    .and_then(|m| m.as_string())
                                })
                                .unwrap_or_else(|| text.clone());
                                web_sys::console::warn_1(
                                    &format!("[realtime_session] provider error event: {err_msg}")
                                        .into(),
                                );
                            }

                            // ── D-04: User transcript persistence ─────────────────
                            // Input transcription: populate the transcript card with
                            // the USER's words, then persist to StateStore so the turn
                            // appears in history (D-04 parity with text turns).
                            // Provider sends the final text on:
                            // conversation.item.input_audio_transcription.completed
                            // (enabled via session.audio.input.transcription).
                            if event_type == "conversation.item.input_audio_transcription.completed"
                            {
                                if let Ok(t) = js_sys::Reflect::get(
                                    &parsed,
                                    &wasm_bindgen::JsValue::from_str("transcript"),
                                ) {
                                    if let Some(transcript_text) = t.as_string() {
                                        // Update the live transcript signal.
                                        transcript.set(transcript_text.clone());
                                        // D-04 RC-4: push user bubble for live in-session display.
                                        {
                                            let id = {
                                                let n = *next_id.read();
                                                next_id.set(n + 1);
                                                n
                                            };
                                            bubbles.write().push(
                                                    crate::components::hermes_app::screens::chat::ChatBubble::user(
                                                        id,
                                                        transcript_text.clone(),
                                                    ),
                                                );
                                        }
                                        // Persist: clone chat_session_id + turn_id + text into spawn_local
                                        // (Pitfall 6 — no Signal borrow across .await).
                                        // D-04 RC-1: write key is chat_session_id_for_msg, not session_id.
                                        // D-04 RC-2: turn_id_for_msg is for authz only; FK is the write guard.
                                        let csid = chat_session_id_for_msg.clone();
                                        let tid = turn_id_for_msg.clone();
                                        let txt = transcript_text.clone();
                                        wasm_bindgen_futures::spawn_local(async move {
                                            if let Err(e) =
                                                crate::server::api::realtime_save_transcript(
                                                    csid,
                                                    tid,
                                                    "user".to_string(),
                                                    txt,
                                                )
                                                .await
                                            {
                                                web_sys::console::warn_1(
                                                        &format!(
                                                            "[realtime_session] realtime_save_transcript (user) failed: {e}"
                                                        )
                                                        .into(),
                                                    );
                                            }
                                        });
                                    }
                                }
                            }

                            // ── D-04 RC-3: Assistant transcript persistence ────────
                            // Persist the assistant's spoken text so it appears in the
                            // chat window + history alongside the user's turns (D-04).
                            //
                            // EVENT NAME (beta vs GA): the beta Realtime API emits
                            //   "response.audio_transcript.done"
                            // but the GA API (gpt-realtime, session.type "realtime" —
                            // what we use) renamed assistant OUTPUT events with an
                            // `output_` prefix:
                            //   "response.output_audio_transcript.done"
                            // Listening only for the beta name meant this handler never
                            // fired under GA, so AI replies never reached the chat window
                            // (the orb still animated because it's driven by the audio
                            // stream tap, not this event). Match the shared suffix so
                            // BOTH names fire. The input transcript uses
                            // "...input_audio_transcription.completed" (a different
                            // suffix), so there is no collision. In both schemas the
                            // full text is in the top-level `transcript` string field.
                            if event_type.ends_with("audio_transcript.done") {
                                if let Ok(t) = js_sys::Reflect::get(
                                    &parsed,
                                    &wasm_bindgen::JsValue::from_str("transcript"),
                                ) {
                                    if let Some(asst_text) = t.as_string() {
                                        if !asst_text.is_empty() {
                                            // D-04 RC-4: push assistant bubble for live display.
                                            {
                                                let id = {
                                                    let n = *next_id.read();
                                                    next_id.set(n + 1);
                                                    n
                                                };
                                                bubbles.write().push(
                                                        crate::components::hermes_app::screens::chat::ChatBubble::assistant(
                                                            id,
                                                            asst_text.clone(),
                                                        ),
                                                    );
                                            }
                                            // Persist assistant transcript.
                                            // RC-1: write under chat_session_id.
                                            // RC-2: turn_id for authz only; FK is write guard.
                                            let csid = chat_session_id_for_msg.clone();
                                            let tid = turn_id_for_msg.clone();
                                            let txt = asst_text.clone();
                                            wasm_bindgen_futures::spawn_local(async move {
                                                if let Err(e) =
                                                    crate::server::api::realtime_save_transcript(
                                                        csid,
                                                        tid,
                                                        "assistant".to_string(),
                                                        txt,
                                                    )
                                                    .await
                                                {
                                                    web_sys::console::warn_1(
                                                            &format!(
                                                                "[realtime_session] realtime_save_transcript (assistant) failed: {e}"
                                                            )
                                                            .into(),
                                                        );
                                                }
                                            });
                                        }
                                    }
                                }
                            }

                            // ── D-02/D-03: response.done → function_call extraction ─
                            // On response.done, iterate response.output[] looking for
                            // function_call items (type == "function_call").
                            // For each one: relay to realtime_tool_call, then:
                            //   - Output{output}: call send_function_call_output (D-05b
                            //     single-send site — auto-approved path).
                            //   - ApprovalPending{...}: raise approval_pending signal for
                            //     Plan 05's card (no send here — exactly one send per call_id).
                            //   - Rejected{reason}: log + send error output so the model
                            //     is not left hung in Thinking (T-39.3-04-01 mitigation).
                            //
                            // NOTE: parse_realtime_event("response.done") returns
                            // Some(Listening) — the orb state update below still fires.
                            // Function-call handling is ADDITIONAL to (not replacing) that.
                            if event_type == "response.done" {
                                // Extract response.output as a JS array.
                                if let Ok(response_obj) = js_sys::Reflect::get(
                                    &parsed,
                                    &wasm_bindgen::JsValue::from_str("response"),
                                ) {
                                    if let Ok(output_val) = js_sys::Reflect::get(
                                        &response_obj,
                                        &wasm_bindgen::JsValue::from_str("output"),
                                    ) {
                                        if let Ok(output_arr) =
                                            output_val.dyn_into::<js_sys::Array>()
                                        {
                                            for i in 0..output_arr.length() {
                                                let item = output_arr.get(i);
                                                // Check if this item is a function_call.
                                                let item_type = js_sys::Reflect::get(
                                                    &item,
                                                    &wasm_bindgen::JsValue::from_str("type"),
                                                )
                                                .ok()
                                                .and_then(|v| v.as_string());
                                                if item_type.as_deref() != Some("function_call") {
                                                    continue;
                                                }
                                                // Extract call_id, name, arguments from
                                                // this function_call item.
                                                // Pitfall 1: use item's own call_id —
                                                // NEVER response.id or item.id.
                                                let call_id = js_sys::Reflect::get(
                                                    &item,
                                                    &wasm_bindgen::JsValue::from_str("call_id"),
                                                )
                                                .ok()
                                                .and_then(|v| v.as_string());
                                                let fn_name = js_sys::Reflect::get(
                                                    &item,
                                                    &wasm_bindgen::JsValue::from_str("name"),
                                                )
                                                .ok()
                                                .and_then(|v| v.as_string());
                                                let arguments = js_sys::Reflect::get(
                                                    &item,
                                                    &wasm_bindgen::JsValue::from_str("arguments"),
                                                )
                                                .ok()
                                                .and_then(|v| v.as_string());

                                                // All three fields required; skip if absent.
                                                let (call_id, fn_name, arguments) = match (
                                                    call_id, fn_name, arguments,
                                                ) {
                                                    (Some(c), Some(n), Some(a)) => (c, n, a),
                                                    _ => {
                                                        web_sys::console::warn_1(
                                                                    &"[realtime_session] function_call item missing call_id/name/arguments — skipping".into(),
                                                                );
                                                        continue;
                                                    }
                                                };

                                                // Clone all owned values into the async block
                                                // (Pitfall 6 — no Signal borrow across .await).
                                                let tid = turn_id_for_msg.clone();
                                                let cid = call_id.clone();
                                                let name_c = fn_name.clone();
                                                let args_c = arguments.clone();
                                                let dc_relay = dc_for_msg.clone();
                                                // Clone the signal values (Copy of the pointer)
                                                // — do not hold a borrow across .await.
                                                let mut ap_signal = approval_pending;
                                                // D-05a: set in-flight true when relay starts.
                                                in_flight.set(true);

                                                wasm_bindgen_futures::spawn_local(async move {
                                                    match crate::server::api::realtime_tool_call(
                                                            tid.clone(),
                                                            cid.clone(),
                                                            name_c,
                                                            args_c,
                                                        )
                                                        .await
                                                        {
                                                            Ok(crate::server::api::RealtimeToolResult::Output { output }) => {
                                                                // Auto-approved: send function_call_output + response.create.
                                                                // D-05b single-send site for the auto-approved path.
                                                                // call_id is from the function_call item (Pitfall 1).
                                                                send_function_call_output(&dc_relay, &cid, &output);
                                                                // D-05a: clear in-flight badge on completion.
                                                                in_flight.set(false);
                                                            }
                                                            Ok(crate::server::api::RealtimeToolResult::ApprovalPending {
                                                                call_id: pending_cid,
                                                                tool_name,
                                                                arguments,
                                                            }) => {
                                                                // Approval required: raise the pending signal for
                                                                // Plan 05's card. Do NOT send function_call_output
                                                                // here — Plan 05's Approve handler calls the shared
                                                                // helper after the user approves.
                                                                // Exactly one send per call_id (D-05b contract).
                                                                // Include turn_id so the Approve/Deny handler can
                                                                // call realtime_approve(turn_id, call_id, approved).
                                                                ap_signal.set(Some(ApprovalPendingInfo {
                                                                    call_id: pending_cid,
                                                                    turn_id: tid,
                                                                    tool_name,
                                                                    arguments,
                                                                }));
                                                                // D-05a: keep in-flight true while approval is pending —
                                                                // the badge stays visible until Approve/Deny resolves.
                                                            }
                                                            Ok(crate::server::api::RealtimeToolResult::Rejected { reason }) => {
                                                                // Rejected: log and send an error output so the
                                                                // model is not left stuck in Thinking
                                                                // (T-39.3-04-01 mitigation).
                                                                web_sys::console::warn_1(
                                                                    &format!(
                                                                        "[realtime_session] tool call rejected (call_id={cid}): {reason}"
                                                                    )
                                                                    .into(),
                                                                );
                                                                let error_output = serde_json::json!({
                                                                    "error": reason
                                                                })
                                                                .to_string();
                                                                // Reuse shared helper — exactly one send even on error.
                                                                send_function_call_output(&dc_relay, &cid, &error_output);
                                                                // D-05a: clear in-flight badge on rejection.
                                                                in_flight.set(false);
                                                            }
                                                            Err(e) => {
                                                                web_sys::console::warn_1(
                                                                    &format!(
                                                                        "[realtime_session] realtime_tool_call server fn error (call_id={cid}): {e}"
                                                                    )
                                                                    .into(),
                                                                );
                                                                // Send error output so model is not left hung.
                                                                let error_output = serde_json::json!({
                                                                    "error": "server_error"
                                                                })
                                                                .to_string();
                                                                send_function_call_output(&dc_relay, &cid, &error_output);
                                                                // D-05a: clear in-flight badge on server error.
                                                                in_flight.set(false);
                                                            }
                                                        }
                                                });
                                            }
                                        }
                                    }
                                }
                            }

                            // Orb state machine update (D-09). response.done still maps to
                            // Listening here — the function_call handling above is additive.
                            if let Some(new_state) = parse_realtime_event(&event_type) {
                                // G-41.2-8: mirror agent-speaking into the shared
                                // half-duplex flag so "Test voice" disables (and the
                                // VAD loop pauses) while the realtime agent is talking.
                                let speaking = new_state == VoiceModeState::Speaking;
                                audio_playback_active.set(speaking);
                                voice_state.set(new_state);
                            }
                        }
                    }
                }
            }),
        );
        dc.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();
    }

    // Wire onopen: send session.update to enable server_vad + provider barge-in (D-08).
    // This is the provider-side VAD configuration that enables the user talking over
    // the agent (barge-in / interrupt_response).
    {
        let dc_for_open = dc.clone();
        // BUG 5 fix: the `session` object is built server-side (GA schema, from
        // config.voice.realtime_*) and fetched above. We wrap it in a
        // session.update envelope and send on channel open. The GA schema nests
        // turn_detection / noise_reduction / transcription under audio.input, with
        // create_response + interrupt_response INSIDE turn_detection (D-08 barge-in).
        // The previous beta-shaped literal (top-level turn_detection) was silently
        // ignored by the GA server, so VAD ran on defaults with no noise reduction
        // — the cause of the background-noise over-sensitivity.
        let session_update = format!(r#"{{"type":"session.update","session":{session_cfg}}}"#);
        let onopen = wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(Box::new(move || {
            if let Err(e) = dc_for_open.send_with_str(&session_update) {
                web_sys::console::warn_1(
                    &format!("[realtime_session] session.update send failed: {e:?}").into(),
                );
            }
        }));
        dc.set_onopen(Some(onopen.as_ref().unchecked_ref()));
        onopen.forget();
    }

    // ── Step 7: SDP exchange ──────────────────────────────────────────────────
    // create_offer → set_local_description → POST SDP to OpenAI → set_remote_description.

    // create_offer
    let offer_promise = pc.create_offer();
    let offer_val = JsFuture::from(offer_promise).await.map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] create_offer: {e:?}").into());
        e
    })?;

    // set_local_description.
    // BUG 4 fix: create_offer() resolves to an RTCSessionDescriptionInit *dictionary*
    // (a plain JS object, NOT a class instance). A checked `dyn_into()` uses JS
    // `instanceof`, which is ALWAYS false for WebIDL dictionaries — so the old cast
    // failed every time ("offer desc cast failed"), aborting the SDP exchange and
    // forcing the silent D-07 fallback. `unchecked_into()` reinterprets without the
    // instanceof check; create_offer always returns a valid init (the answer path
    // below likewise builds a fresh RtcSessionDescriptionInit rather than casting).
    let offer_desc: web_sys::RtcSessionDescriptionInit = offer_val.clone().unchecked_into();
    JsFuture::from(pc.set_local_description(&offer_desc))
        .await
        .map_err(|e| {
            web_sys::console::warn_1(
                &format!("[realtime_session] set_local_description: {e:?}").into(),
            );
            e
        })?;

    // Extract the SDP string from the offer.
    let sdp_str = js_sys::Reflect::get(&offer_val, &wasm_bindgen::JsValue::from_str("sdp"))
        .ok()
        .and_then(|v| v.as_string())
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no sdp in offer"))?;

    // POST SDP to OpenAI Realtime calls endpoint.
    // Per docs: POST https://api.openai.com/v1/realtime/calls
    // Authorization: Bearer {ephemeral_token}, Content-Type: application/sdp
    let headers = web_sys::Headers::new().map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] Headers::new: {e:?}").into());
        e
    })?;
    let auth_header = format!("Bearer {token}");
    headers.set("Authorization", &auth_header).map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] auth header: {e:?}").into());
        e
    })?;
    headers
        .set("Content-Type", "application/sdp")
        .map_err(|e| {
            web_sys::console::warn_1(
                &format!("[realtime_session] content-type header: {e:?}").into(),
            );
            e
        })?;

    let request_init = web_sys::RequestInit::new();
    request_init.set_method("POST");
    request_init.set_headers(&headers);
    request_init.set_body(&wasm_bindgen::JsValue::from_str(&sdp_str));

    let request = web_sys::Request::new_with_str_and_init(
        "https://api.openai.com/v1/realtime/calls",
        &request_init,
    )
    .map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] Request::new: {e:?}").into());
        e
    })?;

    let response_val = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| {
            web_sys::console::warn_1(&format!("[realtime_session] fetch: {e:?}").into());
            e
        })?;

    let response: web_sys::Response = response_val.dyn_into().map_err(|e| {
        web_sys::console::warn_1(&"[realtime_session] Response cast failed".into());
        e
    })?;

    if !response.ok() {
        let status = response.status();
        web_sys::console::warn_1(
            &format!("[realtime_session] SDP POST returned HTTP {status}").into(),
        );
        return Err(wasm_bindgen::JsValue::from_str("sdp_post_failed"));
    }

    // Extract the answer SDP from the response body.
    let answer_sdp_val = JsFuture::from(response.text().map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] response.text(): {e:?}").into());
        e
    })?)
    .await
    .map_err(|e| {
        web_sys::console::warn_1(&format!("[realtime_session] answer text await: {e:?}").into());
        e
    })?;

    let answer_sdp = answer_sdp_val
        .as_string()
        .ok_or_else(|| wasm_bindgen::JsValue::from_str("no answer sdp text"))?;

    // set_remote_description with the provider's SDP answer.
    let answer_desc = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Answer);
    answer_desc.set_sdp(&answer_sdp);
    JsFuture::from(pc.set_remote_description(&answer_desc))
        .await
        .map_err(|e| {
            web_sys::console::warn_1(
                &format!("[realtime_session] set_remote_description: {e:?}").into(),
            );
            e
        })?;

    // ── Step 8a: Populate VOICE_LOOP_SLOT for orb pulse (D-10) ──────────────
    // On the open-mic path the turn-based VAD loop never runs, so VOICE_LOOP_SLOT
    // is never populated via start_voice_loop. Populate it here so that:
    //   - tap_realtime_stream_analyser (called from the ontrack closure above)
    //     connects the remote audio track into a live AnalyserNode, and
    //   - read_fft_bins() returns live bins, making the collapsed orb pulse.
    //
    // Ownership: AudioContext is released by teardown_voice_loop (use_drop in
    // VoiceModeScreen). Mic tracks are released by teardown_realtime_session.
    // We pass an EMPTY MediaStream as the placeholder so teardown_voice_loop
    // iterates 0 tracks — the real mic tracks are stopped exactly once by
    // teardown_realtime_session (no double-stop).
    //
    // IMPORTANT: do NOT call start_voice_loop here — Pitfall 5 exclusivity.
    match web_sys::AudioContext::new() {
        Ok(orb_ctx) => {
            match orb_ctx.create_analyser() {
                Ok(orb_analyser) => {
                    orb_analyser.set_fft_size(
                        crate::components::hermes_app::voice_loop::vad_params::FFT_SIZE,
                    );
                    orb_analyser.set_smoothing_time_constant(
                        crate::components::hermes_app::voice_loop::vad_params::SMOOTHING,
                    );
                    // Unlock autoplay suspension — must resume before any audio graph data flows.
                    if let Ok(resume_promise) = orb_ctx.resume() {
                        let _ = JsFuture::from(resume_promise).await;
                    }
                    // Empty stream placeholder — teardown_voice_loop iterates its tracks (none).
                    let placeholder = web_sys::MediaStream::new().unwrap_or_else(|_| {
                        // Fallback: use a JS-evaluated empty MediaStream if constructor fails.
                        // (This is essentially unreachable on any modern browser.)
                        wasm_bindgen::JsValue::from_str("")
                            .dyn_into::<web_sys::MediaStream>()
                            .unwrap_or_else(|_| web_sys::MediaStream::new().unwrap())
                    });
                    crate::components::hermes_app::voice_loop::populate_realtime_voice_loop_slot(
                        orb_ctx,
                        orb_analyser,
                        placeholder,
                    );
                    // ── Phase 41.2 pulse race fix ────────────────────────────
                    // The `ontrack` closure (Step 4) is wired BEFORE this slot is
                    // populated and fires as soon as `set_remote_description`
                    // resolves — and the `resume().await` above yields control, so
                    // `ontrack` typically runs FIRST. Its `tap_realtime_stream_analyser`
                    // then no-ops (VOICE_LOOP_SLOT is still empty), so the remote
                    // agent audio is never connected to the AnalyserNode that
                    // read_fft_bins reads: the orb sees `slot=true len=128 max=0`
                    // and stays on the idle breath during speech (the reported
                    // "idle state while speaking"). `ontrack` stashes the remote
                    // stream on `window.__ihRealtimeStream`, so if it already
                    // arrived, re-tap it now that the slot holds the analyser. If
                    // `ontrack` has NOT fired yet, the global is unset and this is
                    // a no-op — its own tap will succeed later against the now-
                    // populated slot. Either firing order yields exactly one tap.
                    if let Some(win) = web_sys::window() {
                        if let Ok(stream_val) = js_sys::Reflect::get(
                            &win,
                            &wasm_bindgen::JsValue::from_str("__ihRealtimeStream"),
                        ) {
                            if let Some(stream) = stream_val.dyn_ref::<web_sys::MediaStream>() {
                                crate::components::hermes_app::voice_loop::tap_realtime_stream_analyser(
                                    stream,
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    web_sys::console::warn_1(
                        &format!("[realtime_session] create_analyser for orb failed: {e:?}; orb will not pulse").into(),
                    );
                }
            }
        }
        Err(e) => {
            web_sys::console::warn_1(
                &format!(
                    "[realtime_session] AudioContext for orb failed: {e:?}; orb will not pulse"
                )
                .into(),
            );
        }
    }

    // ── Step 8b: Store handle (incl. turn_id for heartbeat / session-end) ───
    REALTIME_SESSION_SLOT.with(|slot| {
        *slot.borrow_mut() = Some(RealtimeSessionHandle {
            pc,
            dc,
            stream: mic_stream,
            audio_sink,
            turn_id: turn_id_str.clone(),
        });
    });

    // ── Step 9: Start cooperative-cancel heartbeat loop (R39.1-10) ────────────
    //
    // Polls `realtime_heartbeat(turn_id)` every ~2.5 s while a session is active.
    // Uses existing `gloo_timers::future::sleep` + `wasm_bindgen_futures::spawn_local`
    // (both already used in this crate — no new deps).
    //
    // Signal-borrow discipline (MEMORY.md / clippy.toml): `turn_id_str` is an
    // owned String moved into the closure — no GenerationalRef held across .await.
    // The REALTIME_SESSION_SLOT check (is_some) reads a bool (Copy) from the
    // thread_local inside a borrow block that ends before the .await.
    wasm_bindgen_futures::spawn_local(async move {
        const HEARTBEAT_INTERVAL_MS: u32 = 2_500;
        const MAX_CONSECUTIVE_ERRORS: u32 = 3;

        let mut consecutive_errors: u32 = 0;
        let tid = turn_id_str; // owned — moved into this async block

        loop {
            // Check if session is still active (borrow ends before .await).
            let still_active = REALTIME_SESSION_SLOT.with(|slot| slot.borrow().is_some());
            if !still_active {
                // Session was torn down externally (e.g. VoiceModeScreen use_drop).
                break;
            }

            // Sleep BEFORE polling so we don't poll immediately on start.
            gloo_timers::future::sleep(std::time::Duration::from_millis(
                HEARTBEAT_INTERVAL_MS as u64,
            ))
            .await;

            // Re-check after sleep — might have torn down while we slept.
            let still_active = REALTIME_SESSION_SLOT.with(|slot| slot.borrow().is_some());
            if !still_active {
                break;
            }

            // Poll the server for the cooperative-cancel signal.
            match crate::server::api::realtime_heartbeat(tid.clone()).await {
                Ok(crate::server::api::RealtimeTurnSignal::Cancelled) => {
                    // Server cancelled this turn — tear down the WebRTC peer.
                    web_sys::console::warn_1(
                        &"[realtime_session] heartbeat Cancelled — tearing down WebRTC peer".into(),
                    );
                    teardown_realtime_session();
                    break;
                }
                Ok(crate::server::api::RealtimeTurnSignal::Continue) => {
                    // Server acknowledges the turn is still running.
                    consecutive_errors = 0;
                }
                Err(e) => {
                    // Transient error — tolerate up to MAX_CONSECUTIVE_ERRORS before
                    // tearing down. The server may have restarted; in-memory registry
                    // is lost so a lost entry is treated as Cancelled.
                    consecutive_errors += 1;
                    web_sys::console::warn_1(
                        &format!(
                            "[realtime_session] heartbeat error ({consecutive_errors}/{MAX_CONSECUTIVE_ERRORS}): {e}"
                        )
                        .into(),
                    );
                    if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                        web_sys::console::warn_1(
                            &"[realtime_session] too many heartbeat errors — tearing down".into(),
                        );
                        teardown_realtime_session();
                        break;
                    }
                }
            }
        }
    });

    Ok(())
}

/// No-op stub so the server/native build compiles without browser APIs.
#[cfg(not(target_arch = "wasm32"))]
#[allow(clippy::too_many_arguments)]
pub async fn start_realtime_session(
    _voice_state: Signal<VoiceModeState>,
    _transcript: Signal<String>,
    _approval_pending: Signal<Option<ApprovalPendingInfo>>,
    _in_flight: Signal<bool>,
    // D-04 RC-4: bubble list + id counter (wasm-only use; ignored in native stub).
    _bubbles: Signal<Vec<crate::components::hermes_app::screens::chat::ChatBubble>>,
    _next_id: Signal<u64>,
    // D-04 RC-1: real chat session key (wasm-only use; ignored in native stub).
    _chat_session_id: String,
    // Phase 40.5 Plan 03 (D-12): active identity slug (wasm-only use; ignored in native stub).
    _active_identity: Option<String>,
    // G-41.2-8: half-duplex playback flag (wasm-only use; ignored in native stub).
    _audio_playback_active: Signal<bool>,
) -> Result<(), String> {
    Ok(())
}

// ── Native unit tests for parse_realtime_event (all 9 behavior rows) ─────────

#[cfg(test)]
mod tests {
    use super::parse_realtime_event;
    use crate::components::hermes_app::screens::voice_mode::VoiceModeState;

    #[test]
    fn speech_started_maps_to_listening() {
        assert_eq!(
            parse_realtime_event("input_audio_buffer.speech_started"),
            Some(VoiceModeState::Listening),
        );
    }

    #[test]
    fn response_audio_delta_maps_to_speaking() {
        assert_eq!(
            parse_realtime_event("response.audio.delta"),
            Some(VoiceModeState::Speaking),
        );
    }

    #[test]
    fn response_audio_done_maps_to_listening() {
        assert_eq!(
            parse_realtime_event("response.audio.done"),
            Some(VoiceModeState::Listening),
        );
    }

    #[test]
    fn response_done_maps_to_listening() {
        assert_eq!(
            parse_realtime_event("response.done"),
            Some(VoiceModeState::Listening),
        );
    }

    #[test]
    fn response_cancelled_maps_to_listening() {
        // Barge-in: user spoke while agent was speaking; response truncated.
        assert_eq!(
            parse_realtime_event("response.cancelled"),
            Some(VoiceModeState::Listening),
        );
    }

    #[test]
    fn speech_stopped_returns_none() {
        // Stay Listening — no state change needed for speech_stopped.
        assert_eq!(
            parse_realtime_event("input_audio_buffer.speech_stopped"),
            None,
        );
    }

    #[test]
    fn session_created_returns_none() {
        assert_eq!(parse_realtime_event("session.created"), None);
    }

    #[test]
    fn unknown_event_returns_none() {
        assert_eq!(parse_realtime_event("unknown.event"), None);
    }

    #[test]
    fn malformed_empty_type_returns_none_no_panic() {
        // Must not panic on empty or weird type strings.
        assert_eq!(parse_realtime_event(""), None);
        assert_eq!(parse_realtime_event("   "), None);
        assert_eq!(parse_realtime_event("null"), None);
    }

    #[test]
    fn error_event_returns_none_no_orb_transition() {
        // CR-03 / D-07: a provider "error" event must NOT trigger an orb state
        // change (D-09 orb-state contract: only binary Listening/Speaking).
        // The error is logged in onmessage but parse_realtime_event stays pure.
        assert_eq!(parse_realtime_event("error"), None);
    }

    // ── Phase 39.3 Plan 04 (Task 2): extended tests locking response.done orb ──
    // mapping and confirming function-call extraction does not introduce new orb
    // state changes (D-09 contract preserved after Plan 04 additions).

    #[test]
    fn response_done_still_maps_to_listening_after_plan04() {
        // REGRESSION LOCK (Plan 04 / D-09): function-call extraction is added to
        // the response.done branch in onmessage, but parse_realtime_event must
        // continue to return Some(Listening) — the orb transitions to Listening,
        // not to some new "function-calling" state. This test locks that invariant
        // so any future edit to parse_realtime_event that changes this mapping
        // is caught immediately.
        assert_eq!(
            parse_realtime_event("response.done"),
            Some(VoiceModeState::Listening),
            "response.done must still map to Listening after Plan 04 additions"
        );
    }

    #[test]
    fn function_call_specific_events_return_none() {
        // D-09 orb contract: no function-call-specific event type produces an
        // orb state change. Function-call items arrive inside response.output[],
        // not as separate top-level event types — so no new branch in
        // parse_realtime_event was introduced. Assert common related strings
        // all return None to lock this absence.
        assert_eq!(
            parse_realtime_event("response.function_call_arguments.delta"),
            None,
            "function_call_arguments.delta must not change orb state"
        );
        assert_eq!(
            parse_realtime_event("response.function_call_arguments.done"),
            None,
            "function_call_arguments.done must not change orb state"
        );
        assert_eq!(
            parse_realtime_event("conversation.item.created"),
            None,
            "conversation.item.created must not change orb state"
        );
        assert_eq!(
            parse_realtime_event("conversation.item.input_audio_transcription.completed"),
            None,
            "transcription.completed must not change orb state (handled separately in onmessage)"
        );
    }

    #[test]
    fn transcription_completed_returns_none_no_orb_change() {
        // D-04 / D-09: the transcript persistence call on
        // conversation.item.input_audio_transcription.completed is a side-effect
        // in the onmessage branch — it does NOT trigger an orb state change.
        // parse_realtime_event returns None for this event type.
        assert_eq!(
            parse_realtime_event("conversation.item.input_audio_transcription.completed"),
            None,
            "transcript persistence must not cause an orb state transition"
        );
    }
}
