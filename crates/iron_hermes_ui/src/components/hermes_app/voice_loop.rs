//! Phase 36.17.9 Plan 02 (D-07/D-09) — Hands-free energy-VAD voice loop.
//!
//! Implements the state machine:
//!
//! ```text
//! Idle → Listening → (speech confirmed) → Thinking → Speaking → Listening
//!                                     ↓ (3 no-speech cycles, wake-word off)
//!                                   Exit
//! ```
//!
//! # VAD parameters (mirrored from ironhermes-tools/src/vad.rs)
//!
//! | Parameter              | Value   | Notes                             |
//! |------------------------|---------|-----------------------------------|
//! | Poll interval          | 100 ms  | 10 Hz `getByteTimeDomainData`     |
//! | RMS threshold (bytes)  | 5.0     | byte-domain: samples in 0–255     |
//! | Speech confirm time    | 0.5 s   | 5 consecutive above-threshold     |
//! | End-of-speech silence  | 3.0 s   | 30 consecutive below-threshold    |
//! | Hard-stop              | 15.0 s  | absolute capture limit per turn   |
//! | No-speech auto-exit    | 3 cycles| cycles without any speech (D-09)  |
//!
//! # wasm32 gating
//!
//! All browser API calls (`web_sys`, `wasm_bindgen`) are inside
//! `#[cfg(target_arch = "wasm32")]` blocks. A `#[cfg(not(...))]` no-op path
//! suppresses unused-variable warnings on server/native builds.
//!
//! # Integration (voice_mode.rs seam)
//!
//! Call `start_voice_loop` from a `use_effect` in `VoiceModeScreen` when
//! `stt_available` is true. The loop drives `voice_state` and `transcript`
//! signals and calls `on_exit` after 3 no-speech cycles (D-09).

// The voice-loop functions, VAD constants, and analyser helpers are driven from
// wasm/web call sites (the energy-VAD loop runs only in the browser). Native
// builds compile the no-op stubs but never call them, so dead-code analysis flags
// them. Silence native-only dead_code without deleting web-live code; the wasm
// build still lints dead_code normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use crate::components::hermes_app::mic_button::AudioSendHandler;
use crate::components::hermes_app::screens::voice_mode::VoiceModeState;
use crate::components::hermes_app::voice_settings::{
    AudioPlaybackActiveCtx, BeepEnabledCtx, WakeSessionActiveCtx, WakeSessionStopCtx,
    WakeWordEnabledCtx, WakeWordPhraseCtx,
};
// AvatarModeCtx is only read inside the wasm-gated voice-session spawn (D-12 identity
// freeze); on native builds it is unused, so gate the import to match.
#[cfg(target_arch = "wasm32")]
use crate::components::hermes_app::voice_settings::AvatarModeCtx;
use crate::components::hermes_app::VoiceStatusState;
#[cfg(target_arch = "wasm32")]
use crate::state::SessionIdContext;

// Thread-local storage for the active AudioContext + AnalyserNode + stream.
// Mirrors RECORDER_SLOT in mic_button.rs (same isolation pattern).

#[cfg(target_arch = "wasm32")]
thread_local! {
    static VOICE_LOOP_SLOT: std::cell::RefCell<Option<VoiceLoopResources>> =
        const { std::cell::RefCell::new(None) };
    /// Idempotency guard for the realtime remote-stream tap. Both the WebRTC
    /// `ontrack` closure and the post-`set_remote_description` re-tap call
    /// `tap_realtime_stream_analyser` (belt-and-suspenders against the slot-population
    /// race). Without this guard, whichever ordering fires BOTH taps connects
    /// `analyser → destination` twice → the remote audio is summed to the output
    /// twice (the "stereo"/doubled playback UAT report). This flag ensures exactly
    /// one effective tap per session; it is reset when a fresh slot is populated
    /// and on teardown.
    static REALTIME_TAPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    static VOICE_LOOP_SLOT: std::cell::RefCell<Option<()>> =
        const { std::cell::RefCell::new(None) };
}

/// Browser resources held for the duration of one voice-mode session.
#[cfg(target_arch = "wasm32")]
pub struct VoiceLoopResources {
    pub audio_ctx: web_sys::AudioContext,
    pub analyser: web_sys::AnalyserNode,
    pub stream: web_sys::MediaStream,
}

/// Populate `VOICE_LOOP_SLOT` with a pre-built AudioContext + AnalyserNode on the
/// open-mic (realtime) path, so `tap_realtime_stream_analyser` and `read_fft_bins`
/// work without running the turn-based VAD loop (D-10 fix).
///
/// # Ownership contract (no double-free)
///
/// On the open-mic path, the AudioContext is owned by this slot and released by
/// `teardown_voice_loop` (via `use_drop` in VoiceModeScreen). The mic stream tracks
/// are owned by `RealtimeSessionHandle.stream` and stopped exactly once by
/// `teardown_realtime_session`. To keep these non-overlapping, pass an EMPTY
/// `MediaStream` (no tracks) as `placeholder_stream` — `teardown_voice_loop`
/// iterates `get_tracks()` over it and finds nothing to stop, so the realtime
/// mic tracks are stopped exclusively by `teardown_realtime_session`.
///
/// # Pitfall 5 (exclusivity)
///
/// This function does NOT start the turn-based VAD loop. Only `start_voice_loop`
/// does that. The open-mic path must never call `start_voice_loop` concurrently.
///
/// Two-arm cfg pattern mirrors `teardown_voice_loop`: wasm32 writes the slot;
/// non-wasm is a compile-time no-op (web-sys types are unavailable on native).
#[cfg(target_arch = "wasm32")]
pub fn populate_realtime_voice_loop_slot(
    audio_ctx: web_sys::AudioContext,
    analyser: web_sys::AnalyserNode,
    placeholder_stream: web_sys::MediaStream,
) {
    // Fresh session: allow exactly one tap against this new analyser.
    REALTIME_TAPPED.with(|f| f.set(false));
    VOICE_LOOP_SLOT.with(|slot| {
        *slot.borrow_mut() = Some(VoiceLoopResources {
            audio_ctx,
            analyser,
            stream: placeholder_stream,
        });
    });
}

/// No-op stub for non-wasm targets (native/server builds do not have web-sys types).
#[cfg(not(target_arch = "wasm32"))]
pub fn populate_realtime_voice_loop_slot() {
    // No-op — VOICE_LOOP_SLOT holds `Option<()>` on non-wasm and is never written here.
}

/// Release all held browser resources (mic stream tracks + AudioContext).
///
/// Called on overlay exit via `use_drop` in VoiceModeScreen (Task 2 seam).
pub fn teardown_voice_loop() {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        // Clear the tap guard so the next session can tap its own analyser.
        REALTIME_TAPPED.with(|f| f.set(false));
        VOICE_LOOP_SLOT.with(|slot| {
            if let Some(res) = slot.borrow_mut().take() {
                // Stop all mic tracks so the browser indicator goes away.
                let tracks = res.stream.get_tracks();
                for i in 0..tracks.length() {
                    if let Some(track) = tracks.get(i).dyn_ref::<web_sys::MediaStreamTrack>() {
                        track.stop();
                    }
                }
                // Close AudioContext (ignore errors — already closing is fine).
                let _ = res.audio_ctx.close();
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        VOICE_LOOP_SLOT.with(|slot| {
            *slot.borrow_mut() = None;
        });
    }
}

/// Compute RMS on byte-domain `getByteTimeDomainData` output.
///
/// The Web Audio AnalyserNode fills a `Uint8Array` where silence is 128
/// (midpoint of 0–255 unsigned). RMS is computed as:
///
///   rms = sqrt(mean((b - 128)^2))
///
/// Silence → ~0.0; loud speech → ~40–60 in practice.
#[cfg(target_arch = "wasm32")]
pub fn compute_rms(buf: &[u8]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = buf
        .iter()
        .map(|&b| {
            let x = b as f64 - 128.0;
            x * x
        })
        .sum();
    (sum_sq / buf.len() as f64).sqrt() as f32
}

/// VAD parameters — mirrored from ironhermes-tools/src/vad.rs.
pub mod vad_params {
    /// Poll interval in milliseconds (10 Hz).
    pub const POLL_MS: u32 = 100;
    /// Byte-domain RMS threshold; below = silence, above = speech.
    pub const RMS_THRESHOLD: f32 = 5.0;
    /// Consecutive polls above threshold to confirm speech start (~0.5 s).
    pub const SPEECH_CONFIRM_POLLS: u32 = 5;
    /// Consecutive polls below threshold to confirm end-of-speech (~3.0 s).
    pub const SILENCE_POLLS: u32 = 30;
    /// Absolute per-turn capture limit in polls (~15 s).
    pub const HARD_STOP_POLLS: u32 = 150;
    /// Consecutive no-speech cycles before auto-exit (D-09).
    pub const NO_SPEECH_CYCLE_LIMIT: u32 = 3;
    /// FFT size for AnalyserNode (power of 2; 256 → 128-sample buffer).
    pub const FFT_SIZE: u32 = 256;
    /// Smoothing for AnalyserNode.
    pub const SMOOTHING: f64 = 0.8;
}

/// Returns `true` if the voice state indicates TTS playback is in progress.
///
/// Used as a half-duplex gate: while `Speaking`, the VAD capture loop skips
/// RMS polling to avoid the agent capturing its own TTS audio (D-22).
///
/// # Unit-tested gate
///
/// The pure predicate is unit-tested; the LIVE pause is driven by
/// `AudioPlaybackActiveCtx` (set by the AudioOut handler in Task 2).
pub fn should_pause_for_tts(state: VoiceModeState) -> bool {
    matches!(state, VoiceModeState::Speaking)
}

/// Returns `true` when the wake session has been idle long enough to auto-exit.
///
/// Threshold: 150 polls × 100 ms/poll ≈ 15 s without confirmed speech (D-19).
pub fn session_idle_timeout_reached(idle_polls: u32) -> bool {
    idle_polls >= 150
}

/// Play a short chime on wake match (D-21).
///
/// Uses `document::eval` to invoke the Web Audio API inline because
/// `OscillatorNode` and `GainNode` are not listed in this crate's `Cargo.toml`
/// `web_sys` features. Gated on `beep_enabled`; no-op when disabled or on
/// non-wasm targets.
fn play_wake_chime(beep_enabled: bool) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = beep_enabled;
    #[cfg(target_arch = "wasm32")]
    {
        if !beep_enabled {
            return;
        }
        let js = "(function(){ try { \
            var ctx=new AudioContext(); \
            var osc=ctx.createOscillator(); \
            var g=ctx.createGain(); \
            osc.connect(g); \
            g.connect(ctx.destination); \
            osc.frequency.value=880; \
            g.gain.value=0.3; \
            osc.start(); \
            osc.stop(ctx.currentTime+0.15); \
        } catch(e){} })();";
        let _ = dioxus::prelude::document::eval(js);
    }
}

/// Start the hands-free energy-VAD loop.
///
/// This is the integration point called from VoiceModeScreen's `use_effect`.
/// It spawns an async task that:
/// 1. Acquires the mic via `getUserMedia` (reuses mic_button.rs pattern).
/// 2. Builds `AudioContext → MediaStreamSource → AnalyserNode` graph.
/// 3. Polls at 10 Hz, computes RMS, drives the VAD state machine.
/// 4. On end-of-speech: sends `AudioInFrame` via `AudioSendHandler` context.
/// 5. After 3 no-speech cycles (and wake-word off): calls `on_exit`.
///
/// # Wake-word mode (Wave D, D-12)
///
/// When the wake-word context (`WakeWordEnabledCtx`) signals enabled:
/// - The loop idles in `Armed` state with NO 3-cycle silence timeout (D-09 exception).
/// - Short VAD-gated clips (< ~2 s) are sent with `wake_word_check: true` and
///   `wake_phrase: Some(phrase)` — NOT submitted as full turns.
/// - On `WakeWordResult { matched: true }`, the loop arms a full turn (Armed → Listening).
/// - On `WakeWordResult { matched: false }`, it returns to Armed waiting.
///
/// # Arguments
/// - `voice_state`:       writable signal driving VoiceModeScreen's UI state.
/// - `transcript`:        writable signal for the live transcript display.
/// - `on_exit`:           EventHandler called on auto-exit (3-cycle no-speech, wake off only).
/// - `wake_word_off`:     true when wake-word is disabled (D-09 exit applies).
// `mut voice_state` is required by the wasm32 loop (many `.set()` calls); the
// non-wasm stub only discards it, so allow the otherwise-unused mut there.
#[cfg_attr(not(target_arch = "wasm32"), allow(unused_mut))]
pub fn start_voice_loop(
    mut voice_state: Signal<VoiceModeState>,
    transcript: Signal<String>,
    on_exit: EventHandler<()>,
    wake_word_off: bool,
) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;
        use wasm_bindgen_futures::JsFuture;

        // Pattern B: read ALL context values into locals before the async spawn.
        // No signal borrow may be held across any .await boundary (clippy.toml rule).
        let audio_send = use_context::<AudioSendHandler>();
        let session_id = use_context::<SessionIdContext>().0;

        // Phase 36.17.9 (D-12, Wave D): consume wake-word context newtypes
        // provided by VoiceSettings via use_context_provider (Plan 02 Task 2).
        // Read both into Copy locals before the spawn — Pattern B borrow safety.
        let wake_enabled_ctx = use_context::<WakeWordEnabledCtx>();
        let wake_phrase_ctx = use_context::<WakeWordPhraseCtx>();
        let wake_word_enabled: bool = *wake_enabled_ctx.0.read();
        let wake_phrase_local: String = wake_phrase_ctx.0.read().clone();

        // WakeWordResult signal provided by mod.rs (Task 1 seam).
        // voice_loop polls this to detect Armed → Listening transition.
        let mut wake_word_matched_ctx = use_context::<crate::state::WakeWordMatchedContext>().0;

        // Plan 03 (VOICE-02): VAD params come from VoiceStatusState context.
        // VoiceStatusState is seeded by the WS VoiceStatus event (pushed on connect)
        // from build_voice_status (re-reads config from disk each connect), and is
        // ALSO written live by VoiceSettings' "live" threshold/silence inputs.
        //
        // Phase 36.17.10 (UAT live-tuning fix): we keep only the Copy Signal HANDLE
        // here and read the values LIVE inside the poll loop below — NOT a snapshot.
        // Snapshotting here meant a panel edit mid-session never reached the running
        // loop (the captured local was frozen at spawn time), so the "live" pill lied.
        // The handle is Copy and moves into the spawn; each poll reads + drops the
        // guard within one iteration, so no borrow ever crosses an `.await`.
        let voice_status_ctx = use_context::<Signal<VoiceStatusState>>();

        // Plan 04 (D-19/D-21/D-22): wake session + half-duplex contexts.
        // Extract the inner Signal<bool> immediately — Pattern B: no borrow across await.
        let mut wake_session_active = use_context::<WakeSessionActiveCtx>().0;
        let mut wake_session_stop = use_context::<WakeSessionStopCtx>().0;
        let audio_playback_active = use_context::<AudioPlaybackActiveCtx>().0;
        let beep_enabled_ctx = use_context::<BeepEnabledCtx>();
        let beep_enabled_local: bool = *beep_enabled_ctx.0.read();

        // Phase 40.5 Plan 08 (D-12, D-17): freeze the active identity ONCE at voice-session
        // start. Read AvatarModeCtx into an owned Option<String> BEFORE the async spawn
        // (Pattern B — no borrow across .await). The same slug is stamped on EVERY
        // AudioInFrame emitted during this session (wake-word-check + full-turn frames),
        // so mid-session avatar switches never reach the running loop (D-12 parity with
        // Plan 03 realtime token path). Falls back to None if the slug is unknown.
        let frozen_active_identity: Option<String> = {
            let avatar_ctx = use_context::<AvatarModeCtx>();
            let slug = avatar_ctx.0.read().active_identity.clone();
            if crate::components::hermes_app::avatar_logic::is_known_identity(&slug) {
                Some(slug)
            } else {
                None
            }
        };

        dioxus::prelude::spawn(async move {
            // ── Acquire mic stream ────────────────────────────────────────
            let window = match web_sys::window() {
                Some(w) => w,
                None => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            let media_devices = match window.navigator().media_devices() {
                Ok(m) => m,
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            let constraints = web_sys::MediaStreamConstraints::new();
            constraints.set_audio(&wasm_bindgen::JsValue::TRUE);
            constraints.set_video(&wasm_bindgen::JsValue::FALSE);
            let stream_promise = match media_devices.get_user_media_with_constraints(&constraints) {
                Ok(p) => p,
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            let stream: web_sys::MediaStream = match JsFuture::from(stream_promise).await {
                Ok(s) => match s.dyn_into() {
                    Ok(ms) => ms,
                    Err(_) => {
                        voice_state.set(VoiceModeState::Unavailable);
                        return;
                    }
                },
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };

            // ── Build Web Audio graph ─────────────────────────────────────
            let audio_ctx = match web_sys::AudioContext::new() {
                Ok(ctx) => ctx,
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            let source = match audio_ctx.create_media_stream_source(&stream) {
                Ok(s) => s,
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            let analyser = match audio_ctx.create_analyser() {
                Ok(a) => a,
                Err(_) => {
                    voice_state.set(VoiceModeState::Unavailable);
                    return;
                }
            };
            analyser.set_fft_size(vad_params::FFT_SIZE);
            analyser.set_smoothing_time_constant(vad_params::SMOOTHING);
            if source.connect_with_audio_node(&analyser).is_err() {
                voice_state.set(VoiceModeState::Unavailable);
                return;
            }

            // Phase 36.17.10 UAT fix: resume the AudioContext. Browsers create it
            // SUSPENDED under the autoplay policy; a suspended context feeds the
            // AnalyserNode pure silence, so compute_rms stays ~0, speech is never
            // confirmed, and end-of-speech detection never fires (recording never
            // auto-stops — the symptom reported in UAT test 7, independent of the
            // configured threshold/duration). Voice mode is entered via a user
            // gesture, so resume() is permitted. Await it so the graph is running
            // before the first RMS poll. (mic_button.rs is unaffected — it uses
            // MediaRecorder directly, with no AudioContext.)
            if let Ok(resume_promise) = audio_ctx.resume() {
                let _ = JsFuture::from(resume_promise).await;
            }

            // Store resources in thread_local for teardown.
            VOICE_LOOP_SLOT.with(|slot| {
                *slot.borrow_mut() = Some(VoiceLoopResources {
                    audio_ctx: audio_ctx.clone(),
                    analyser: analyser.clone(),
                    stream: stream.clone(),
                });
            });

            // ── Common buffer shared by armed + session + outer loops ─────────
            let buf_len = analyser.frequency_bin_count() as usize;
            let mut buf = vec![0u8; buf_len];

            if wake_word_enabled {
                // ── Continuous wake-session re-arm loop (D-19) ───────────────
                //
                // Each iteration: Armed (detect phrase) → session (VAD turns until
                // idle-timeout or Stop Listening) → back to Armed.
                // Loops indefinitely; the only exit is teardown_voice_loop() from
                // the component's use_drop when the user leaves voice mode.
                'rearm: loop {
                    voice_state.set(VoiceModeState::Armed);

                    // ── Wake-word Armed idle state (D-09/D-11/D-12) ──────────
                    //
                    // Keep the mic AnalyserNode + VAD running. Send short speech
                    // clips as wake-word-check frames (wake_word_check=true).
                    // Stay Armed until WakeWordResult { matched: true } arrives.
                    // D-09 exception: NO 3-cycle silence timeout in Armed state.
                    'armed: loop {
                        // Pattern B: read into bool local before any await.
                        let matched_now: bool = *wake_word_matched_ctx.read();
                        if matched_now {
                            // Consume the match — reset to false before transitioning.
                            wake_word_matched_ctx.set(false);
                            break 'armed;
                        }

                        // VAD poll — accumulate a short speech segment.
                        let mut above_polls_ww: u32 = 0;
                        let mut speech_confirmed_ww = false;
                        let mut hard_stop_ww: u32 = 0;

                        let recorder_ww =
                            match web_sys::MediaRecorder::new_with_media_stream(&stream) {
                                Ok(r) => r,
                                Err(_) => break 'armed,
                            };
                        let chunks_rc_ww =
                            std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
                        let chunks_for_cb_ww = chunks_rc_ww.clone();
                        let ondataavailable_ww = wasm_bindgen::closure::Closure::<
                            dyn FnMut(web_sys::BlobEvent),
                        >::wrap(Box::new(
                            move |evt: web_sys::BlobEvent| {
                                let chunks_inner_ww = chunks_for_cb_ww.clone();
                                if let Some(blob) = evt.data() {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        use wasm_bindgen::JsCast;
                                        let promise_ww: js_sys::Promise =
                                            blob.array_buffer().into();
                                        if let Ok(ab) = JsFuture::from(promise_ww).await {
                                            let arr = js_sys::Uint8Array::new(&ab);
                                            let mut tmp = vec![0u8; arr.length() as usize];
                                            arr.copy_to(&mut tmp);
                                            chunks_inner_ww.borrow_mut().extend_from_slice(&tmp);
                                        }
                                    });
                                }
                            },
                        ));
                        recorder_ww
                            .set_ondataavailable(Some(ondataavailable_ww.as_ref().unchecked_ref()));
                        ondataavailable_ww.forget();
                        let _ = recorder_ww.start();

                        // ~2 s capture limit for wake-word clips (D-12: short VAD-gated clip).
                        // 20 polls × 100 ms = 2 s hard cap.
                        const WW_HARD_STOP: u32 = 20;
                        let mut silence_polls_ww: u32 = 0;

                        'listen_ww: loop {
                            gloo_timers::future::sleep(std::time::Duration::from_millis(
                                vad_params::POLL_MS as u64,
                            ))
                            .await;

                            analyser.get_byte_time_domain_data(&mut buf);
                            let rms_ww = compute_rms(&buf);
                            hard_stop_ww += 1;

                            if rms_ww >= vad_params::RMS_THRESHOLD {
                                above_polls_ww += 1;
                                silence_polls_ww = 0;
                                if !speech_confirmed_ww
                                    && above_polls_ww >= vad_params::SPEECH_CONFIRM_POLLS
                                {
                                    speech_confirmed_ww = true;
                                }
                            } else {
                                above_polls_ww = 0;
                                if speech_confirmed_ww {
                                    silence_polls_ww += 1;
                                    if silence_polls_ww >= vad_params::SILENCE_POLLS {
                                        break 'listen_ww;
                                    }
                                }
                            }
                            if hard_stop_ww >= WW_HARD_STOP {
                                break 'listen_ww;
                            }
                        }

                        let _ = recorder_ww.stop();

                        if speech_confirmed_ww {
                            // Brief yield so ondataavailable can fire.
                            gloo_timers::future::sleep(std::time::Duration::from_millis(200)).await;
                            let ww_audio = chunks_rc_ww.borrow().clone();

                            if !ww_audio.is_empty() {
                                // D-12: send as wake-word-check clip — NOT a full turn.
                                // D-13: wake phrase travels on the frame (client-controlled).
                                let frame_ww = crate::protocol::AudioInFrame {
                                    session_id: session_id(),
                                    mime: "audio/webm;codecs=opus".to_string(),
                                    bytes: ww_audio,
                                    wake_word_check: true,
                                    wake_phrase: Some(wake_phrase_local.clone()),
                                    // Phase 40.5 Plan 08 (D-12): session-frozen slug
                                    // (captured once at session start above the spawn).
                                    active_identity: frozen_active_identity.clone(),
                                };
                                if let Ok(json_bytes) = serde_json::to_vec(&frame_ww) {
                                    audio_send.0.call(json_bytes);
                                }
                            }
                        }

                        // Brief pause before polling wake_word_matched again.
                        gloo_timers::future::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    // Armed loop exited — WakeWordResult { matched: true } received.

                    // D-21: play a real chime on wake match (gated by beep_enabled).
                    // SAFETY: js string is a hardcoded literal with no user input.
                    play_wake_chime(beep_enabled_local);

                    // Transition Armed → Listening and start the continuous session.
                    voice_state.set(VoiceModeState::Listening);

                    // Defensively re-ensure the AudioContext is running after the
                    // wake-word STT round-trip (MediaRecorder churn can suspend it).
                    if audio_ctx.state() != web_sys::AudioContextState::Running {
                        if let Ok(resume_promise) = audio_ctx.resume() {
                            let _ = JsFuture::from(resume_promise).await;
                        }
                    }

                    // Signal that a wake session is active (WakeSessionIndicator reads this).
                    wake_session_active.set(true);
                    let mut idle_polls: u32 = 0;

                    // ── Continuous wake session (D-19) ────────────────────────
                    //
                    // Loops VAD-segmented turns without re-arming the wake word
                    // between them. Ends on idle-timeout (~15 s without confirmed
                    // speech) or an explicit "Stop Listening" signal from the UI.
                    'session: loop {
                        // Pattern B: read ALL signals into owned locals BEFORE any
                        // await — no borrow may cross an .await boundary.
                        let vs: VoiceModeState = voice_state.read().clone();
                        let playback_active: bool = *audio_playback_active.read();
                        let stop_requested: bool = *wake_session_stop.read();

                        // D-22: skip capture while the agent is speaking. The REAL
                        // AudioPlaybackActiveCtx signal (set by AudioOut handler in
                        // Task 2 on play-start, cleared on ended/error) drives the
                        // pause — NOT a fixed timer.
                        if should_pause_for_tts(vs) || playback_active {
                            gloo_timers::future::sleep(std::time::Duration::from_millis(
                                vad_params::POLL_MS as u64,
                            ))
                            .await;
                            continue 'session;
                        }

                        // Explicit stop from the "Stop Listening" button.
                        if stop_requested {
                            wake_session_stop.set(false);
                            break 'session;
                        }

                        // Idle-timeout: ~15 s with no confirmed speech (D-19 end condition).
                        if session_idle_timeout_reached(idle_polls) {
                            break 'session;
                        }

                        // ── One VAD turn within the session ───────────────────
                        let mut above_polls: u32 = 0;
                        let mut silence_polls_count: u32 = 0;
                        let mut speech_confirmed = false;
                        let mut hard_stop: u32 = 0;

                        let recorder = match web_sys::MediaRecorder::new_with_media_stream(&stream)
                        {
                            Ok(r) => r,
                            Err(_) => break 'session,
                        };
                        let chunks_rc = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
                        let chunks_for_cb = chunks_rc.clone();
                        let ondataavailable = wasm_bindgen::closure::Closure::<
                            dyn FnMut(web_sys::BlobEvent),
                        >::wrap(Box::new(
                            move |evt: web_sys::BlobEvent| {
                                let chunks_inner = chunks_for_cb.clone();
                                if let Some(blob) = evt.data() {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        use wasm_bindgen::JsCast;
                                        let promise: js_sys::Promise = blob.array_buffer().into();
                                        if let Ok(ab) = JsFuture::from(promise).await {
                                            let arr = js_sys::Uint8Array::new(&ab);
                                            let mut tmp = vec![0u8; arr.length() as usize];
                                            arr.copy_to(&mut tmp);
                                            chunks_inner.borrow_mut().extend_from_slice(&tmp);
                                        }
                                    });
                                }
                            },
                        ));
                        recorder
                            .set_ondataavailable(Some(ondataavailable.as_ref().unchecked_ref()));
                        ondataavailable.forget();
                        let _ = recorder.start();

                        'listen_s: loop {
                            gloo_timers::future::sleep(std::time::Duration::from_millis(
                                vad_params::POLL_MS as u64,
                            ))
                            .await;

                            analyser.get_byte_time_domain_data(&mut buf);
                            let rms = compute_rms(&buf);
                            hard_stop += 1;

                            // Live VAD params — same pattern as the non-wake 'outer loop.
                            let (rms_threshold, silence_polls): (f32, u32) = {
                                let vs_live = voice_status_ctx.read();
                                let t = vs_live
                                    .web_silence_threshold_rms
                                    .unwrap_or(vad_params::RMS_THRESHOLD);
                                let s = vs_live
                                    .silence_duration_secs
                                    .map(|d| (d * 1000.0 / vad_params::POLL_MS as f64) as u32)
                                    .unwrap_or(vad_params::SILENCE_POLLS);
                                (t, s)
                            };

                            if rms >= rms_threshold {
                                above_polls += 1;
                                silence_polls_count = 0;
                                if !speech_confirmed
                                    && above_polls >= vad_params::SPEECH_CONFIRM_POLLS
                                {
                                    speech_confirmed = true;
                                }
                            } else {
                                above_polls = 0;
                                if speech_confirmed {
                                    silence_polls_count += 1;
                                    if silence_polls_count >= silence_polls {
                                        break 'listen_s;
                                    }
                                }
                            }
                            if hard_stop >= vad_params::HARD_STOP_POLLS {
                                break 'listen_s;
                            }
                        }

                        let _ = recorder.stop();

                        if !speech_confirmed {
                            // No speech — increment idle counter and stay Listening.
                            idle_polls += 1;
                            voice_state.set(VoiceModeState::Listening);
                            continue 'session;
                        }

                        // Speech confirmed — send turn, then wait for TTS (D-22).
                        idle_polls = 0;
                        voice_state.set(VoiceModeState::Thinking);

                        // Brief yield so ondataavailable can fire.
                        gloo_timers::future::sleep(std::time::Duration::from_millis(200)).await;
                        let audio_chunks = chunks_rc.borrow().clone();

                        if !audio_chunks.is_empty() {
                            let frame = crate::protocol::AudioInFrame {
                                session_id: session_id(),
                                mime: "audio/webm;codecs=opus".to_string(),
                                bytes: audio_chunks,
                                wake_word_check: false,
                                wake_phrase: None,
                                // Phase 40.5 Plan 08 (D-12): session-frozen slug.
                                active_identity: frozen_active_identity.clone(),
                            };
                            if let Ok(json_bytes) = serde_json::to_vec(&frame) {
                                audio_send.0.call(json_bytes);
                            }
                        }

                        // D-22: half-duplex — wait for the REAL AudioPlaybackActiveCtx
                        // signal to clear (set by AudioOut handler in Task 2 on play-start,
                        // cleared on ended/error). Resumes exactly when TTS playback ends.
                        voice_state.set(VoiceModeState::Speaking);
                        'wait_pb_s: loop {
                            // Pattern B: read into owned local before await.
                            let pb: bool = *audio_playback_active.read();
                            if !pb {
                                break 'wait_pb_s;
                            }
                            gloo_timers::future::sleep(std::time::Duration::from_millis(
                                vad_params::POLL_MS as u64,
                            ))
                            .await;
                        }
                        voice_state.set(VoiceModeState::Listening);
                    }
                    // Session ended (idle-timeout or Stop Listening) — return to Armed.
                    wake_session_active.set(false);
                    // 'rearm continues: voice_state → Armed at top of next iteration.
                }
                // 'rearm never exits naturally — wake-word mode loops until
                // teardown_voice_loop() is called via use_drop in VoiceModeScreen.
            } else {
                // ── Non-wake VAD loop (D-09: 3-cycle auto-exit) ──────────────
                voice_state.set(VoiceModeState::Listening);

                // Phase 36.17.10: defensively re-ensure the AudioContext is running on
                // entry to the full-turn loop. It is resumed once before this point,
                // but the wake-word STT round-trip + MediaRecorder churn can leave it
                // suspended, and a suspended context feeds the AnalyserNode pure silence
                // (RMS ~0, speech never confirmed). Resuming a running context is a no-op.
                if audio_ctx.state() != web_sys::AudioContextState::Running {
                    if let Ok(resume_promise) = audio_ctx.resume() {
                        let _ = JsFuture::from(resume_promise).await;
                    }
                }

                let mut no_speech_cycles: u32 = 0;
                'outer: loop {
                    // ── Listen phase ──────────────────────────────────────────
                    let mut above_polls: u32 = 0;
                    // Counter for consecutive below-threshold polls (renamed to avoid conflict
                    // with derived `silence_polls` local from Plan 03 VAD param derivation).
                    let mut silence_polls_count: u32 = 0;
                    let mut speech_confirmed = false;
                    let mut hard_stop: u32 = 0;

                    // Set up MediaRecorder for this turn's capture.
                    let recorder = match web_sys::MediaRecorder::new_with_media_stream(&stream) {
                        Ok(r) => r,
                        Err(_) => break 'outer,
                    };
                    let chunks_rc = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
                    let chunks_for_cb = chunks_rc.clone();
                    let ondataavailable =
                        wasm_bindgen::closure::Closure::<dyn FnMut(web_sys::BlobEvent)>::wrap(
                            Box::new(move |evt: web_sys::BlobEvent| {
                                let chunks_inner = chunks_for_cb.clone();
                                if let Some(blob) = evt.data() {
                                    wasm_bindgen_futures::spawn_local(async move {
                                        use wasm_bindgen::JsCast;
                                        let promise: js_sys::Promise = blob.array_buffer().into();
                                        if let Ok(ab) = JsFuture::from(promise).await {
                                            let arr = js_sys::Uint8Array::new(&ab);
                                            let mut tmp = vec![0u8; arr.length() as usize];
                                            arr.copy_to(&mut tmp);
                                            chunks_inner.borrow_mut().extend_from_slice(&tmp);
                                        }
                                    });
                                }
                            }),
                        );
                    recorder.set_ondataavailable(Some(ondataavailable.as_ref().unchecked_ref()));
                    ondataavailable.forget();
                    let _ = recorder.start();

                    'listen: loop {
                        // Poll RMS at 10 Hz.
                        gloo_timers::future::sleep(std::time::Duration::from_millis(
                            vad_params::POLL_MS as u64,
                        ))
                        .await;

                        analyser.get_byte_time_domain_data(&mut buf);
                        let rms = compute_rms(&buf);
                        hard_stop += 1;

                        // Phase 36.17.10 (UAT live-tuning): read VAD params LIVE each poll
                        // from VoiceStatusState (written by VoiceSettings' "live" inputs and
                        // seeded from config on connect). The read guard is dropped at the
                        // end of this block — it never spans the `.await` above. This is what
                        // makes a threshold/silence edit take effect mid-session.
                        let (rms_threshold, silence_polls): (f32, u32) = {
                            let vs = voice_status_ctx.read();
                            let t = vs
                                .web_silence_threshold_rms
                                .unwrap_or(vad_params::RMS_THRESHOLD);
                            // Derive silence_polls from silence_duration_secs (s → poll count at POLL_MS Hz).
                            // HARD_STOP_POLLS stays hardcoded (RESEARCH Open Q3).
                            let s = vs
                                .silence_duration_secs
                                .map(|d| (d * 1000.0 / vad_params::POLL_MS as f64) as u32)
                                .unwrap_or(vad_params::SILENCE_POLLS);
                            (t, s)
                        };

                        // Plan 03 (VOICE-02): use config-derived rms_threshold / silence_polls locals.
                        // Constants remain as the fallback defaults when VoiceStatusState is unset.
                        if rms >= rms_threshold {
                            above_polls += 1;
                            silence_polls_count = 0;
                            if !speech_confirmed && above_polls >= vad_params::SPEECH_CONFIRM_POLLS
                            {
                                speech_confirmed = true;
                            }
                        } else {
                            above_polls = 0;
                            if speech_confirmed {
                                silence_polls_count += 1;
                                if silence_polls_count >= silence_polls {
                                    // End-of-speech detected.
                                    break 'listen;
                                }
                            }
                        }

                        // Hard-stop: absolute capture limit (RESEARCH Open Q3 — hardcoded).
                        if hard_stop >= vad_params::HARD_STOP_POLLS {
                            break 'listen;
                        }
                    }

                    // Stop recorder — triggers ondataavailable with all chunks.
                    let _ = recorder.stop();

                    if !speech_confirmed {
                        no_speech_cycles += 1;
                        if wake_word_off && no_speech_cycles >= vad_params::NO_SPEECH_CYCLE_LIMIT {
                            // D-09: auto-exit after 3 consecutive no-speech cycles.
                            break 'outer;
                        }
                        // No speech — go back to Listening.
                        voice_state.set(VoiceModeState::Listening);
                        continue 'outer;
                    }

                    // Speech detected — reset counter and transition to Thinking.
                    no_speech_cycles = 0;
                    voice_state.set(VoiceModeState::Thinking);

                    // Collect chunks (brief yield to let ondataavailable run).
                    gloo_timers::future::sleep(std::time::Duration::from_millis(200)).await;
                    let audio_chunks = chunks_rc.borrow().clone();

                    // Send AudioInFrame via AudioSendHandler → tx_stt → run_web_turn.
                    if !audio_chunks.is_empty() {
                        let frame = crate::protocol::AudioInFrame {
                            session_id: session_id(),
                            mime: "audio/webm;codecs=opus".to_string(),
                            bytes: audio_chunks,
                            // D-12: full turn — wake_word_check is false.
                            wake_word_check: false,
                            // Full turns don't carry a wake phrase.
                            wake_phrase: None,
                            // Phase 40.5 Plan 08 (D-12): session-frozen slug.
                            active_identity: frozen_active_identity.clone(),
                        };
                        if let Ok(json_bytes) = serde_json::to_vec(&frame) {
                            audio_send.0.call(json_bytes);
                        }
                    }

                    // D-22: half-duplex pause — real AudioPlaybackActiveCtx signal
                    // replaces the former fixed 1 000 ms sleep ("TODO Wave D").
                    // Resumes exactly when the AudioOut element fires ended/error.
                    voice_state.set(VoiceModeState::Speaking);
                    'wait_pb: loop {
                        // Pattern B: read into owned local before await.
                        let pb: bool = *audio_playback_active.read();
                        if !pb {
                            break 'wait_pb;
                        }
                        gloo_timers::future::sleep(std::time::Duration::from_millis(
                            vad_params::POLL_MS as u64,
                        ))
                        .await;
                    }
                    voice_state.set(VoiceModeState::Listening);
                }

                // Auto-exit: 3 no-speech cycles (D-09).
                teardown_voice_loop();
                on_exit.call(());
            }
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        // Server / native build: no-op to suppress unused variable warnings.
        let _ = voice_state;
        let _ = transcript;
        let _ = on_exit;
        let _ = wake_word_off;
        // Suppress context lookup unused warnings on non-wasm builds.
        let _: WakeWordEnabledCtx;
        let _: WakeWordPhraseCtx;
        let _: VoiceStatusState; // Plan 03: suppress unused import on non-wasm builds.
                                 // Plan 04: suppress new context type imports on non-wasm builds.
        let _: AudioPlaybackActiveCtx;
        let _: BeepEnabledCtx;
        let _: WakeSessionActiveCtx;
        let _: WakeSessionStopCtx;
    }
}

// ── Speaking-state AnalyserNode tap ──────────────────────────────────────────
//
// Called from mod.rs AudioOut arm (Phase 36.17.9 Plan 03) BEFORE the audio
// element is pushed to bubbles. Creates a MediaElementSource → AnalyserNode
// → destination chain so the orb can react to TTS playback amplitude.
//
// CRITICAL (Pitfall 2): MediaElementSource permanently re-routes the element.
// MUST connect to destination or audio goes completely silent.
// Guard: only runs when VOICE_LOOP_SLOT holds an active VoiceLoopResources.

/// Tap the speaking-state AnalyserNode for an `<audio>` element created by the
/// AudioOut binary-frame arm in mod.rs.
///
/// - Wires: `audio_el → MediaElementSource → analyser → destination`
/// - No-op when voice loop is not active (voice mode not open).
/// - No-op on non-wasm targets (server/native builds).
#[allow(unused_variables)]
pub fn tap_speaking_analyser(audio_el: &web_sys::HtmlAudioElement) {
    #[cfg(target_arch = "wasm32")]
    {
        VOICE_LOOP_SLOT.with(|slot| {
            if let Some(resources) = slot.borrow().as_ref() {
                let ctx = &resources.audio_ctx;
                let analyser = &resources.analyser;
                // createMediaElementSource throws InvalidStateError if called twice
                // on the same element. Since each AudioOut creates a fresh element
                // with a new Blob URL, this is always a first call per element.
                match ctx.create_media_element_source(audio_el) {
                    Ok(source) => {
                        // Connect source → analyser → destination
                        // (source.connect_with_audio_node returns Result, ignore err gracefully)
                        let _ = source.connect_with_audio_node(analyser);
                        let dest = ctx.destination();
                        let _ = analyser.connect_with_audio_node(&dest);
                    }
                    Err(e) => {
                        // Log but do NOT panic — audio still plays because the
                        // element is pushed to bubbles regardless of tap success.
                        #[cfg(target_arch = "wasm32")]
                        web_sys::console::warn_1(
                            &format!("[voice_loop] tap_speaking_analyser failed: {:?}", e).into(),
                        );
                    }
                }
            }
            // If slot is None, voice mode is inactive — skip silently.
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = audio_el;
    }
}

/// Tap the speaking-state AnalyserNode for a remote `MediaStream` from the
/// OpenAI Realtime WebRTC session.
///
/// Called from `realtime_session.rs` `ontrack` when the provider's remote
/// audio track arrives. Routes it through the existing Web Audio graph:
///   `MediaStreamSource → analyser → destination`
///
/// - CRITICAL (Pitfall 3): MUST connect `analyser → destination` or the remote
///   audio is completely silent in the browser.
/// - No-op when `VOICE_LOOP_SLOT` is empty (voice mode not open).
/// - No-op on non-wasm targets (server/native builds).
#[allow(unused_variables)]
pub fn tap_realtime_stream_analyser(stream: &web_sys::MediaStream) {
    #[cfg(target_arch = "wasm32")]
    {
        // Idempotency guard: a session taps the remote stream exactly once. Both
        // the ontrack closure and the post-SRD re-tap call this; the first to run
        // with a populated slot wins, the rest no-op — otherwise analyser→destination
        // is connected twice and playback doubles ("stereo" UAT report).
        if REALTIME_TAPPED.with(|f| f.get()) {
            return;
        }
        VOICE_LOOP_SLOT.with(|slot| {
            if let Some(resources) = slot.borrow().as_ref() {
                let ctx = &resources.audio_ctx;
                let analyser = &resources.analyser;
                match ctx.create_media_stream_source(stream) {
                    Ok(source) => {
                        // Connect source → analyser → destination.
                        // MUST connect analyser to destination or audio is silent (Pitfall 3).
                        let _ = source.connect_with_audio_node(analyser);
                        let dest = ctx.destination();
                        let _ = analyser.connect_with_audio_node(&dest);
                        // Mark tapped only on success so a failed attempt can be retried.
                        REALTIME_TAPPED.with(|f| f.set(true));
                    }
                    Err(e) => {
                        // Warn-not-panic: remote audio still flows via WebRTC speaker output;
                        // the orb just won't pulse reactively if this tap fails.
                        web_sys::console::warn_1(
                            &format!("[voice_loop] tap_realtime_stream_analyser failed: {e:?}")
                                .into(),
                        );
                    }
                }
            }
            // If slot is None, voice mode is inactive — skip silently.
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = stream;
    }
}

/// Read the current byte-frequency data from the live AnalyserNode in VOICE_LOOP_SLOT.
///
/// Returns a `Vec<u8>` of length `frequency_bin_count()` (128 bins with FFT_SIZE=256,
/// values 0–255) suitable for passing to OrbCanvas as the `fft_bins` prop.
///
/// Returns an empty `Vec::new()` when:
/// - Voice mode is inactive (VOICE_LOOP_SLOT is None), or
/// - Running on a non-wasm target (server/native build).
///
/// This is the public seam that lets voice_mode.rs read FFT bins without touching
/// the private VOICE_LOOP_SLOT thread_local directly.
pub fn read_fft_bins() -> Vec<u8> {
    #[cfg(target_arch = "wasm32")]
    {
        let mut result = Vec::new();
        VOICE_LOOP_SLOT.with(|slot| {
            if let Some(resources) = slot.borrow().as_ref() {
                let analyser = &resources.analyser;
                let bin_count = analyser.frequency_bin_count() as usize;
                let mut buf = vec![0u8; bin_count];
                analyser.get_byte_frequency_data(&mut buf);
                result = buf;
            }
        });
        result
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::hermes_app::screens::voice_mode::VoiceModeState;

    #[test]
    fn should_pause_for_tts_when_speaking() {
        assert!(should_pause_for_tts(VoiceModeState::Speaking));
    }

    #[test]
    fn should_pause_for_tts_when_listening() {
        assert!(!should_pause_for_tts(VoiceModeState::Listening));
    }

    #[test]
    fn should_pause_for_tts_when_armed() {
        assert!(!should_pause_for_tts(VoiceModeState::Armed));
    }

    #[test]
    fn should_pause_for_tts_when_thinking() {
        assert!(!should_pause_for_tts(VoiceModeState::Thinking));
    }

    #[test]
    fn session_idle_below_threshold() {
        assert!(!session_idle_timeout_reached(149));
    }

    #[test]
    fn session_idle_at_threshold() {
        assert!(session_idle_timeout_reached(150));
    }

    #[test]
    fn session_idle_above_threshold() {
        assert!(session_idle_timeout_reached(200));
    }
}
