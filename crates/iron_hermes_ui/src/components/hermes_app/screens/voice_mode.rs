//! Phase 36.17.9 Plan 02 (D-04/D-06) — Full-screen voice-mode overlay.
//!
//! This is NOT a Screen enum variant — it is a conditional overlay rendered
//! on top of the normal app tree, driven by the `voice_mode_active` signal
//! provided by HermesApp (Plan 01).
//!
//! # State machine
//!
//! ```text
//! Idle → Listening → Thinking → Speaking → Listening (loop)
//!              ↓ (3 no-speech cycles, wake-word off)
//!            Exit
//! Armed → Listening (after wake phrase detected — Wave D)
//! Unavailable — STT not configured
//! ```
//!
//! # Integration seams
//!
//! - `voice_loop::start_voice_loop()` is called from the `use_effect` when the
//!   overlay activates; it owns the AudioContext + AnalyserNode lifecycle and
//!   drives the VoiceModeState signal (Task 2).
//! - `VoiceSettings` (Task 2 / voice_settings.rs) is conditionally rendered
//!   when `settings_open` is true.
//! - `on_exit` EventHandler is called by: Escape key, ✕ button, voice_loop
//!   3-cycle no-speech auto-exit.

// VoiceModeState variants and its render-helper methods are constructed/called by
// the wasm/web voice overlay. Native dead-code analysis cannot see those web-gated
// sites. Silence native-only dead_code without deleting web-live code; the wasm
// build still lints dead_code normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use dioxus::core::use_drop;
use dioxus::prelude::*;

use crate::components::hermes_app::orb_canvas::OrbCanvas;
use crate::components::hermes_app::voice_settings::{
    AudioPlaybackActiveCtx, AvatarErrorNoticeCtx, BargeInModeCtx, RealtimeApprovalCtx,
    RealtimeDegradedCtx, RealtimeInFlightCtx, WakeSessionActiveCtx, WakeSessionStopCtx,
    WakeWordEnabledCtx, WakeWordPhraseCtx,
};
use crate::components::hermes_app::VoiceStatusState;

/// State machine for the voice-mode overlay.
///
/// Drives the `.voice-state-label` modifier class and the ARIA live-region
/// announcement per UI-SPEC §ARIA and §Copywriting Contract.
#[derive(Clone, PartialEq, Default, Debug)]
pub enum VoiceModeState {
    #[default]
    Idle,
    /// Mic active — energy-VAD listening for speech.
    Listening,
    /// Turn audio sent — waiting for server STT + LLM response.
    Thinking,
    /// Agent AudioOut playback in progress.
    Speaking,
    /// Wake-word mode idle — waiting for the wake phrase (Wave D).
    Armed,
    /// STT not configured — cannot enter voice mode.
    Unavailable,
}

impl VoiceModeState {
    /// CSS modifier class applied to `.voice-state-label`.
    pub fn css_class(&self) -> &'static str {
        match self {
            VoiceModeState::Idle => "is-idle",
            VoiceModeState::Listening => "is-listening",
            VoiceModeState::Thinking => "is-thinking",
            VoiceModeState::Speaking => "is-speaking",
            VoiceModeState::Armed => "is-armed",
            VoiceModeState::Unavailable => "is-disabled",
        }
    }

    /// Label text per UI-SPEC §Copywriting Contract (exact strings).
    pub fn label_text(&self) -> &'static str {
        match self {
            VoiceModeState::Idle => "IDLE",
            VoiceModeState::Listening => "LISTENING",
            VoiceModeState::Thinking => "THINKING",
            VoiceModeState::Speaking => "SPEAKING",
            VoiceModeState::Armed => "WAKE WORD ARMED",
            VoiceModeState::Unavailable => "UNAVAILABLE",
        }
    }

    /// ARIA live-region announcement text per UI-SPEC §ARIA.
    pub fn aria_announcement(&self) -> &'static str {
        match self {
            VoiceModeState::Idle => "Voice mode idle",
            VoiceModeState::Listening => "Listening",
            VoiceModeState::Thinking => "Processing your request",
            VoiceModeState::Speaking => "Hermes is speaking",
            VoiceModeState::Armed => "Waiting for wake phrase",
            VoiceModeState::Unavailable => "Voice mode unavailable",
        }
    }
}

/// Full-screen voice-mode overlay (D-04).
///
/// Rendered conditionally by HermesApp when `voice_mode_active` is true.
/// Consumes `voice_status` from context to determine initial availability.
///
/// # Props
/// - `on_exit`: called when the user closes the overlay (Escape, ✕ button,
///   or 3-cycle no-speech auto-exit from voice_loop).
#[component]
pub fn VoiceModeScreen(on_exit: EventHandler<()>) -> Element {
    // Context lookups — borrows dropped into locals before rsx! tree.
    let voice_status = use_context::<Signal<VoiceStatusState>>();
    let stt_available = voice_status.read().stt_available;

    // Phase 36.17.9 (D-12, Wave D): read wake phrase from context for the Armed
    // transcript placeholder. WakeWordPhraseCtx is provided by VoiceSettings
    // (voice_settings.rs use_context_provider — Plan 02 Task 2).
    // Pattern B: read into local String before rsx!, no borrow held across tree.
    let wake_phrase_ctx = use_context::<WakeWordPhraseCtx>();
    let wake_phrase_str = wake_phrase_ctx.0.read().clone();

    // Local overlay state.
    let mut voice_state = use_signal(|| {
        if stt_available {
            VoiceModeState::Idle
        } else {
            VoiceModeState::Unavailable
        }
    });
    let mut settings_open = use_signal(|| false);
    // Phase 39.3 Plan 05 (D-03/D-05a): approval-pending + in-flight contexts.
    // RealtimeApprovalCtx and RealtimeInFlightCtx are provided at HermesApp root
    // (mod.rs) per the Dioxus context-panic rule (MEMORY.md). Consume them here
    // to (a) pass the underlying signals to start_realtime_session and (b) render
    // the approval card and in-flight badge in the orb overlay (Task 2).
    let approval_ctx = use_context::<RealtimeApprovalCtx>();
    let approval_pending = approval_ctx.0;
    let in_flight_ctx = use_context::<RealtimeInFlightCtx>();
    let in_flight = in_flight_ctx.0;
    // G-41.2-8: AudioPlaybackActiveCtx (provided at HermesApp root). Passed to
    // start_realtime_session so realtime agent-speaking drives the half-duplex
    // guard (Test Voice disable + VAD pause) — the realtime path otherwise never
    // sets it (only the turn-based AudioOut handler does).
    let audio_playback_active = use_context::<AudioPlaybackActiveCtx>().0;
    // Phase 36.17.10 (UAT): collapse the full-screen overlay to a compact corner
    // widget so the chat transcript behind it stays visible while voice mode runs.
    let mut minimized = use_signal(|| false);
    let transcript = use_signal(String::new);

    // Read state fields into locals before rsx — Pattern B (no borrow across rsx).
    let state_class = voice_state.read().css_class();
    let state_label = voice_state.read().label_text();
    let current_state = voice_state.read().clone();
    let transcript_text = transcript.read().clone();
    // Pre-compute transcript card content to avoid nested rsx!/match in RSX tree.
    // UI-SPEC: Armed placeholder is `Say "${phrase}" to begin`.
    let armed_placeholder = format!("Say \"{wake_phrase_str}\" to begin");
    let transcript_placeholder: &str = match &current_state {
        VoiceModeState::Armed => &armed_placeholder,
        _ => "Waiting\u{2026}",
    };
    // UI-SPEC §ARIA: Armed aria-live is `Say "${phrase}" to begin speaking.`
    let armed_aria = format!("Say \"{wake_phrase_str}\" to begin speaking.");
    let state_aria: &str = match &current_state {
        VoiceModeState::Armed => &armed_aria,
        _ => voice_state.read().aria_announcement(),
    };
    let transcript_card_class = if transcript_text.is_empty() {
        "voice-transcript-card is-placeholder"
    } else {
        "voice-transcript-card"
    };

    // Phase 39.3 Plan 05 (D-03/D-05a): pre-compute approval card locals before rsx!
    // Pattern B: clone out of Signal borrow before tree — no GenerationalRef in closures.
    // `in_flight_active` is Copy; approval locals are owned Strings for move into handlers.
    // D-03 fix: use .read() (not .peek()) so this component SUBSCRIBES to the signals —
    // .peek() reads without subscribing, so set() from the realtime ApprovalPending branch
    // never triggered a re-render and the card/badge never appeared.
    let in_flight_active = *in_flight.read();
    let approval_snapshot = approval_pending.read().clone(); // Option<ApprovalPendingInfo>
    let show_approval = approval_snapshot.is_some();
    let approval_tool_name = approval_snapshot
        .as_ref()
        .map(|p| p.tool_name.clone())
        .unwrap_or_default();
    let approval_args = approval_snapshot
        .as_ref()
        .map(|p| p.arguments.clone())
        .unwrap_or_default();
    let approval_call_id = approval_snapshot
        .as_ref()
        .map(|p| p.call_id.clone())
        .unwrap_or_default();
    let approval_turn_id = approval_snapshot
        .as_ref()
        .map(|p| p.turn_id.clone())
        .unwrap_or_default();

    // Phase 36.17.9 Plan 05 (gap closure): voice_loop wiring.
    // Pattern B: read wake-word-enabled into a Copy local BEFORE any hook, so no
    // borrow is held across the use_effect closure boundary.
    let wake_word_off = !*use_context::<WakeWordEnabledCtx>().0.read();

    // FFT bins signal — polled every ~100ms from the live AnalyserNode via read_fft_bins().
    // Non-wasm builds see an always-empty Vec (read_fft_bins() returns Vec::new() there).
    // `mut` is required on wasm32 (the ~100ms pump calls `fft_bins.set(...)`);
    // on native that pump is cfg'd out, so allow the otherwise-unused mut there.
    #[allow(unused_mut)]
    let mut fft_bins = use_signal(Vec::<u8>::new);

    // Phase 36.17.12 Plan 03: read barge_in_mode into an owned local BEFORE any
    // spawn/await (Pattern B — no borrow held across async boundaries).
    // Phase 36.17.12 Plan 04 (CR-01 gap closure): BargeInModeCtx and RealtimeDegradedCtx
    // are provided at the HermesApp root (mod.rs) — NOT by VoiceSettings. Providing them
    // in a child of this component caused a "Could not find context BargeInModeCtx" panic
    // on every voice-mode entry. The fix mirrors the WakeWordEnabledCtx/WakeWordPhraseCtx
    // root-provider pattern (mod.rs lines immediately after the wake-word providers).
    let barge_in_mode_val = use_context::<BargeInModeCtx>().0.read().clone();
    let mut realtime_degraded = use_context::<RealtimeDegradedCtx>().0;

    // Phase 40.2 Plan 04 (FE-05): one-time per-session avatar error notice.
    // .read() in render path subscribes for re-render when the flag flips true
    // (Dioxus trap 1 — .peek() would not re-render). Pattern B: read into owned
    // Copy bool so the GenerationalRef borrow is released before rsx!.
    let avatar_error_notice_shown = *use_context::<AvatarErrorNoticeCtx>().0.read();

    // Start the hands-free VAD loop (or realtime session) when stt is available.
    // start_voice_loop internally calls use_context — must run inside component scope
    // (use_effect closures run in scope; detached spawns do NOT).
    //
    // Phase 36.17.10 (UAT zombie-loop fix): spawn EXACTLY ONCE per voice-mode entry.
    // start_voice_loop reads the wake_enabled / wake_phrase signals synchronously, so
    // those reads register as use_effect dependencies — meaning every wake-phrase
    // keystroke or wake-word toggle re-ran this effect and spawned ANOTHER loop, each
    // with its own getUserMedia capture. N concurrent captures contend for the mic
    // (echo-cancellation starves them), so every analyser reads ~0 RMS, no speech is
    // ever confirmed, and the turn never fires. The `started` latch (read via .peek()
    // so it is NOT itself a dependency) guarantees a single spawn; the loop reads the
    // VAD threshold/silence live, and wake-word changes apply on the next voice-mode
    // entry (teardown on unmount resets the latch).
    //
    // Phase 36.17.12 Plan 03 (D-01/D-06/D-07): STRICTLY EXCLUSIVE mode gate.
    // - open_mic → spawn start_realtime_session; on Err (D-07 fallback) set
    //   RealtimeDegradedCtx true and start the existing turn-based loop.
    //   start_voice_loop is NOT called on the open_mic success path (Pitfall 5).
    // - push_to_interrupt / half_duplex → start_voice_loop unchanged.
    //
    // D-04 RC-4: read bubbles + next_id from context so realtime turns can be
    // pushed into the live chat surface.  Pattern B: read into Copy/Clone locals
    // before use_effect so no borrow is held across the closure boundary.
    // bubbles is Signal<Vec<ChatBubble>> (provided bare by HermesApp).
    // next_id is wrapped in NextIdContext.
    let bubbles_for_rt =
        use_context::<Signal<Vec<crate::components::hermes_app::screens::chat::ChatBubble>>>();
    let next_id_for_rt = use_context::<crate::state::NextIdContext>().0;
    // D-04 RC-1: read the real chat session key from context BEFORE the spawn
    // (Dioxus Pattern B — use_context must be called in component scope, not in
    // async tasks).  The owned String is moved into the spawn closure.
    // D-04 RC-1: read the real chat session key from context BEFORE the spawn
    // (Dioxus Pattern B — use_context must be called in component scope, not in
    // async tasks).  Two-statement form required: the GenerationalRef temp from
    // `.read()` must be dropped before the block ends (E0597 — borrow outlives ctx.0).
    let chat_session_id_for_rt = {
        let ctx = use_context::<crate::state::SessionIdContext>();
        let val = ctx.0.read().clone();
        val
    };
    // Phase 40.5 Plan 03 (D-12): read active_identity BEFORE spawn (Pattern B).
    // D-12: identity is frozen at session start — the slug is captured once here
    // and passed as active_identity to start_realtime_session. Mid-session identity
    // changes in the UI do NOT re-resolve the realtime voice (single token, no update).
    // GenerationalRef from .read() is dropped at the end of the block (E0597 guard).
    let active_identity_for_rt = {
        let ctx = use_context::<crate::components::hermes_app::voice_settings::AvatarModeCtx>();
        // Two-statement form: the GenerationalRef temp from .read() must be dropped before
        // the block ends (E0597 — borrow outlives ctx). Mirrors chat_session_id_for_rt pattern.
        let val = ctx.0.read().active_identity.clone();
        val
    };

    let mut started = use_signal(|| false);
    use_effect(move || {
        if stt_available && !*started.peek() {
            started.set(true);
            if barge_in_mode_val == "open_mic" {
                // Open-mic path: attempt the realtime WebRTC session.
                // On failure, degrade to the turn-based loop (D-07).
                // D-04 RC-1: clone chat_session_id before the async move so the
                // FnMut closure retains ownership (E0507 — String is not Copy).
                let chat_session_id_rt_clone = chat_session_id_for_rt.clone();
                // Phase 40.5 Plan 03 (D-12): clone active_identity before async move
                // so the FnMut closure retains ownership (String is not Copy).
                let active_identity_rt = Some(active_identity_for_rt.clone());
                dioxus::prelude::spawn(async move {
                    match crate::components::hermes_app::realtime_session::start_realtime_session(
                        voice_state,
                        transcript,
                        approval_pending,
                        in_flight,
                        bubbles_for_rt,
                        next_id_for_rt,
                        chat_session_id_rt_clone,
                        active_identity_rt,
                        audio_playback_active,
                    )
                    .await
                    {
                        Ok(()) => {
                            // Realtime session started — do NOT also start the turn-based loop.
                            // The two mic captures are mutually exclusive (Pitfall 5 / D-06).
                        }
                        Err(_) => {
                            // D-07 fallback: realtime unavailable — degrade to turn-based path.
                            // Surface the degrade indicator so VoiceSettings can show the hint.
                            realtime_degraded.set(true);
                            crate::components::hermes_app::voice_loop::start_voice_loop(
                                voice_state,
                                transcript,
                                on_exit,
                                wake_word_off,
                            );
                        }
                    }
                });
            } else {
                // push_to_interrupt / half_duplex: existing turn-based loop, unchanged.
                crate::components::hermes_app::voice_loop::start_voice_loop(
                    voice_state,
                    transcript,
                    on_exit,
                    wake_word_off,
                );
            }
        }
    });

    // Teardown: release mic stream + AudioContext when the overlay unmounts (D-06/D-07).
    // Both teardown functions are idempotent — a None slot is a no-op, so calling both
    // is always safe regardless of which path was taken above.
    use_drop(crate::components::hermes_app::voice_loop::teardown_voice_loop);
    // Phase 36.17.12 Plan 03: second use_drop for the realtime session (D-05/T-36.17.12-03-05).
    use_drop(crate::components::hermes_app::realtime_session::teardown_realtime_session);

    // FFT polling loop (~100ms cadence, wasm32 only).
    // Drives the fft_bins signal so OrbCanvas receives live byte-frequency data (D-03).
    use_effect(move || {
        #[cfg(target_arch = "wasm32")]
        {
            dioxus::prelude::spawn(async move {
                loop {
                    gloo_timers::future::sleep(std::time::Duration::from_millis(100)).await;
                    // Pattern B: read_fft_bins() returns an owned Vec — no borrow across await.
                    fft_bins.set(crate::components::hermes_app::voice_loop::read_fft_bins());
                }
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = fft_bins;
        }
    });

    // Focus-trap / focus-return: on mount, move focus into the overlay.
    // On exit (Escape or ✕), focus returns to the voice-entry-btn in the header.
    // The focus-return is handled by the caller (mod.rs) restoring focus after
    // conditional render removes the overlay from the DOM.
    use_effect(move || {
        // Announce initial state to screen readers on mount.
        // Actual focus management requires web_sys and is gated below.
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            if let Some(window) = web_sys::window() {
                if let Some(document) = window.document() {
                    // Move focus into the overlay's close button so keyboard
                    // users have an immediate focus target (UI-SPEC §Keyboard).
                    if let Ok(Some(el)) = document.query_selector(".voice-header-btn--close") {
                        if let Ok(btn) = el.dyn_into::<web_sys::HtmlElement>() {
                            let _ = btn.focus();
                        }
                    }
                }
            }
        }
    });

    let is_minimized = *minimized.read();

    rsx! {
        div {
            class: if is_minimized { "voice-mode-overlay voice-mode-overlay--min" } else { "voice-mode-overlay" },
            role: "dialog",
            "aria-modal": "true",
            "aria-label": "Voice mode",
            // Keyboard: Escape closes the overlay (UI-SPEC §Keyboard).
            onkeydown: move |evt: KeyboardEvent| {
                if evt.key() == Key::Escape {
                    on_exit.call(());
                }
            },

            // ── Header buttons ────────────────────────────────────────────
            button {
                class: "voice-header-btn voice-header-btn--close",
                "aria-label": "Exit voice mode",
                onclick: move |_| on_exit.call(()),
                "✕ EXIT"
            }
            button {
                class: "voice-header-btn voice-header-btn--minimize",
                "aria-label": if is_minimized { "Expand voice mode to full screen" } else { "Minimize voice mode to corner" },
                onclick: move |_| {
                    let m = *minimized.read();
                    minimized.set(!m);
                },
                if is_minimized { "▢ EXPAND" } else { "▤ MIN" }
            }
            button {
                class: "voice-header-btn voice-header-btn--settings",
                "aria-label": "Voice settings",
                "aria-expanded": if *settings_open.read() { "true" } else { "false" },
                onclick: move |_| {
                    let open = *settings_open.read();
                    settings_open.set(!open);
                },
                "⚙ SETTINGS"
            }

            // ── Orb region — OrbCanvas (Wave C: Three.js audio-reactive orb) ──
            div { class: "orb-region",
                OrbCanvas {
                    state: current_state.clone(),
                    // Pattern B: clone into prop — no borrow held across rsx (D-03).
                    fft_bins: fft_bins.read().clone(),
                }
            }

            // ── State label pill ──────────────────────────────────────────
            div {
                class: "voice-state-label {state_class}",
                "{state_label}"
            }

            // ── Wake session indicator (D-19/D-21): shown while a hands-free ──
            // session is active. Renders nothing when WakeSessionActiveCtx is false.
            WakeSessionIndicator {}

            // ── Transcript card ───────────────────────────────────────────
            div {
                class: "{transcript_card_class}",
                if transcript_text.is_empty() {
                    "{transcript_placeholder}"
                } else {
                    "{transcript_text}"
                }
            }

            // ── D-05a: In-flight badge — visible while a background tool turn runs ──
            // `in_flight_active` is a Copy bool computed before rsx! (Pattern B).
            // RealtimeInFlightCtx is set true by realtime_session.rs on relay-start
            // and cleared on Output/Rejected/error.
            if in_flight_active {
                div {
                    class: "voice-badge-inflight",
                    role: "status",
                    "aria-live": "polite",
                    "aria-label": "Tool call in progress",
                    "working\u{2026}"
                }
            }

            // ── Phase 40.2 Plan 04 (FE-05): one-time avatar error notice ──────
            // Shown when avatar.js emits window.__ihAvatarError and orb_canvas.rs
            // sets AvatarErrorNoticeCtx=true. Not dismissible — one-time per session.
            // AvatarPrefs.enabled is NOT changed (D-11: user preference preserved).
            if avatar_error_notice_shown {
                div {
                    class: "voice-badge-inflight",
                    role: "status",
                    "aria-live": "polite",
                    "Avatar unavailable — using orb"
                }
            }

            // ── D-03: Approval card — shown when a tool call awaits user approval ──
            // All approval data was cloned into owned locals before rsx! (Pattern B).
            // The card is an overlay element and does NOT gate the orb/audio loop
            // (D-03 keep-conversing — card presence does not affect voice session state).
            if show_approval {
                div {
                    class: "voice-approval-card",
                    role: "dialog",
                    "aria-label": "Tool call approval",
                    "aria-modal": "false",

                    div { class: "voice-approval-card__tool",
                        span { class: "voice-approval-card__label", "Tool:" }
                        span { class: "voice-approval-card__value", "{approval_tool_name}" }
                    }
                    div { class: "voice-approval-card__args",
                        span { class: "voice-approval-card__label", "Args:" }
                        pre  { class: "voice-approval-card__value", "{approval_args}" }
                    }

                    div { class: "voice-approval-card__actions",
                        // ── Approve ──────────────────────────────────────────────────
                        // Single-send contract (D-05b): this is the ONE send site for
                        // the user-approved path. Plan 04's ApprovalPending branch did
                        // NOT send; we call the shared send_function_call_output helper
                        // here so exactly one function_call_output + response.create is
                        // emitted per call_id. Never inline a fresh send pair.
                        button {
                            class: "voice-approval-card__btn voice-approval-card__btn--approve",
                            "aria-label": "Approve tool call",
                            onclick: {
                                let call_id  = approval_call_id.clone();
                                let turn_id  = approval_turn_id.clone();
                                move |_| {
                                    let call_id  = call_id.clone();
                                    let turn_id  = turn_id.clone();
                                    #[cfg(target_arch = "wasm32")]
                                    let mut approval_sig = approval_pending;
                                    #[cfg(target_arch = "wasm32")]
                                    let mut inflight_sig = in_flight;
                                    #[cfg(target_arch = "wasm32")]
                                    wasm_bindgen_futures::spawn_local(async move {
                                        // Clone dc BEFORE .await — Pattern B (no borrow across await).
                                        let dc_opt = crate::components::hermes_app::realtime_session::get_realtime_dc();
                                        match crate::server::api::realtime_approve(
                                            turn_id,
                                            call_id.clone(),
                                            true,
                                        ).await {
                                            Ok(crate::server::api::RealtimeToolResult::Output { output }) => {
                                                if let Some(dc) = dc_opt {
                                                    crate::components::hermes_app::realtime_session::send_function_call_output(
                                                        &dc, &call_id, &output,
                                                    );
                                                }
                                            }
                                            Ok(crate::server::api::RealtimeToolResult::Rejected { reason }) => {
                                                // Turn ended or cancelled while card was showing.
                                                // Send an error payload so the model is not left hanging.
                                                if let Some(dc) = dc_opt {
                                                    let err = format!("{{\"error\":\"{reason}\"}}");
                                                    crate::components::hermes_app::realtime_session::send_function_call_output(
                                                        &dc, &call_id, &err,
                                                    );
                                                }
                                            }
                                            Ok(_) | Err(_) => {
                                                // Unexpected result — clear card, let session recover.
                                            }
                                        }
                                        approval_sig.set(None);
                                        inflight_sig.set(false);
                                    });
                                    #[cfg(not(target_arch = "wasm32"))]
                                    { let _ = (call_id, turn_id, approval_pending, in_flight); }
                                }
                            },
                            "APPROVE"
                        }

                        // ── Deny ─────────────────────────────────────────────────────
                        // Calls realtime_approve(approved=false) so the server removes
                        // the pending entry (T-39.3-05-02 — denied call must not execute).
                        // Sends an error payload via the shared helper so the model is
                        // not left waiting for a function_call_output (D-05b).
                        button {
                            class: "voice-approval-card__btn voice-approval-card__btn--deny",
                            "aria-label": "Deny tool call",
                            onclick: {
                                let call_id  = approval_call_id.clone();
                                let turn_id  = approval_turn_id.clone();
                                move |_| {
                                    let call_id  = call_id.clone();
                                    let turn_id  = turn_id.clone();
                                    #[cfg(target_arch = "wasm32")]
                                    let mut approval_sig = approval_pending;
                                    #[cfg(target_arch = "wasm32")]
                                    let mut inflight_sig = in_flight;
                                    #[cfg(target_arch = "wasm32")]
                                    wasm_bindgen_futures::spawn_local(async move {
                                        let dc_opt = crate::components::hermes_app::realtime_session::get_realtime_dc();
                                        let _ = crate::server::api::realtime_approve(
                                            turn_id,
                                            call_id.clone(),
                                            false,
                                        ).await;
                                        // Notify the model the tool was denied — send error
                                        // payload so model is not left waiting for output.
                                        if let Some(dc) = dc_opt {
                                            let err = "{\"error\":\"Tool call denied by user.\"}";
                                            crate::components::hermes_app::realtime_session::send_function_call_output(
                                                &dc, &call_id, err,
                                            );
                                        }
                                        approval_sig.set(None);
                                        inflight_sig.set(false);
                                    });
                                    #[cfg(not(target_arch = "wasm32"))]
                                    { let _ = (call_id, turn_id, approval_pending, in_flight); }
                                }
                            },
                            "DENY"
                        }
                    }
                }
            }

            // ── Controls row (per-state controls) ────────────────────────
            div { class: "voice-controls-row",
                match &current_state {
                    VoiceModeState::Speaking => {
                        // INTERRUPT is single-tap immediate — NO confirmation dialog
                        // (UI-SPEC §Controls / threat model T-36.17.9-02-01).
                        rsx! {
                            button {
                                class: "voice-interrupt-btn",
                                "aria-label": "Interrupt — stop playback",
                                onclick: move |_| {
                                    // voice_loop will handle actual audio stop (Task 2 seam).
                                    voice_state.set(VoiceModeState::Listening);
                                },
                                "⊘ INTERRUPT"
                            }
                        }
                    }
                    VoiceModeState::Thinking => {
                        rsx! {
                            span { class: "voice-timeout-hint", "PROCESSING\u{2026}" }
                        }
                    }
                    VoiceModeState::Unavailable => {
                        rsx! {
                            span { class: "voice-timeout-hint",
                                "STT not configured — check server settings."
                            }
                        }
                    }
                    _ => {
                        rsx! { span { class: "voice-timeout-hint", "\u{00a0}" } }
                    }
                }
            }

            // ── Settings panel (conditional) ──────────────────────────────
            if *settings_open.read() {
                // VoiceSettings is created in Task 2 (voice_settings.rs).
                // Import seam: the module is declared in hermes_app/mod.rs Task 2.
                crate::components::hermes_app::voice_settings::VoiceSettings {
                    on_close: move |_| settings_open.set(false),
                }
            }

            // ── ARIA live region (screen reader state announcements) ──────
            div {
                role: "status",
                "aria-live": "polite",
                "aria-atomic": "true",
                class: "voice-mode-sr",
                "{state_aria}"
            }
        }
    }
}

/// Phase 40.5 Plan 04 (D-19): visual cue + stop affordance for an active wake session.
///
/// Consumes `WakeSessionActiveCtx` and `WakeSessionStopCtx` from the HermesApp root.
///
/// - When inactive: renders nothing (returns `None`).
/// - When active: renders a dim "Wake session active" label below the orb state pill,
///   and a "Stop Listening" button (styled as `.voice-interrupt-btn`) that drives
///   `WakeSessionStopCtx = true`, signalling the voice_loop 'session loop to exit.
///
/// Pattern G: uses `.read()` (not `.peek()`) in the render path so the component
/// subscribes and re-renders when `WakeSessionActiveCtx` changes.
#[component]
pub fn WakeSessionIndicator() -> Element {
    // Pattern G: .read() subscribes — component re-renders on session state change.
    let wake_session_active_ctx = use_context::<WakeSessionActiveCtx>();
    let mut wake_session_stop_ctx = use_context::<WakeSessionStopCtx>();

    let session_active: bool = *wake_session_active_ctx.0.read();

    if !session_active {
        return rsx! {};
    }

    rsx! {
        div { class: "wake-session-indicator",
            span {
                class: "voice-state-label is-listening wake-session-label",
                "aria-live": "polite",
                "Wake session active"
            }
            button {
                class: "voice-interrupt-btn",
                "aria-label": "Stop listening — end the hands-free wake session",
                onclick: move |_| {
                    wake_session_stop_ctx.0.set(true);
                },
                "Stop Listening"
            }
        }
    }
}
