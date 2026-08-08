//! Phase 36.17.8 Plan 06 (D-13 / D-14) — MicButton component.
//!
//! Push-to-talk mic button for the HermesApp chat composer. Captures audio
//! via `MediaRecorder`, serializes it as an `AudioInFrame` binary WS frame,
//! and fires `on_transcript` when the server sends the resulting transcript
//! back through the normal chat stream (D-15).
//!
//! # State machine
//!
//! ```text
//! Idle → [click] → RequestingPermission → [granted] → Recording
//!                                       → [denied]  → Error(PermissionDenied)
//! Recording → [click / Escape] → Transcribing → [server chat reply] → Idle
//!           → [Escape]          → Idle (cancel)
//! Transcribing → [2 s timeout]  → Idle  (5c: server-side error, auto-clear)
//! ```
//!
//! # WASM conventions
//!
//! - All `web_sys` calls are gated on `#[cfg(target_arch = "wasm32")]`.
//! - Signal borrows MUST NOT be held across `.await` (clippy.toml rule).
//! - No `cx`, `use_state`, `Scope` — Dioxus 0.7 only.
//!
//! # AudioSendHandler
//!
//! `AudioSendHandler` is a `Copy` newtype wrapping `EventHandler<Vec<u8>>`
//! provided by `HermesApp` via `use_context_provider`. The handler serializes
//! the `Vec<u8>` as an `AudioInFrame` JSON payload and sends it as a
//! `Message::Binary` WS frame.

use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// AudioSendHandler — context newtype (provided by HermesApp, consumed here).
// ---------------------------------------------------------------------------

/// Newtype wrapping the WS binary-send handler so it does not collide with
/// `ChatSendHandler` (both are `EventHandler` newtypes in the context tree).
///
/// `Copy` is required so consumers can call
/// `let audio_send = use_context::<AudioSendHandler>();` without moving it.
#[allow(dead_code)] // context-provider newtype; consumed via use_context::<AudioSendHandler>()
#[derive(Clone, Copy)]
pub struct AudioSendHandler(pub EventHandler<Vec<u8>>);

// ---------------------------------------------------------------------------
// MicState / MicError enums.
// ---------------------------------------------------------------------------

/// The six mic-button states defined in the UI-SPEC §Mic Button States.
///
/// Several variants are only constructed inside the `#[cfg(target_arch = "wasm32")]`
/// capture path; on the non-wasm host build (server/desktop/`--all-features` bin)
/// those constructors are compiled out, so allow dead_code here.
#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug)]
pub enum MicState {
    /// State 1 — idle, STT provider confirmed available.
    Idle,
    /// State 2 — waiting for `getUserMedia` to resolve.
    RequestingPermission,
    /// State 3 — `MediaRecorder` is active, capturing audio.
    Recording,
    /// State 4 — binary WS frame sent; waiting for agent reply.
    Transcribing,
    /// State 5 — error sub-states.
    Error(MicError),
    /// State 6 — STT provider not configured server-side.
    Disabled,
}

/// Error sub-states for `MicState::Error`.
///
/// Constructed only in the wasm32 capture path (see `MicState`); allow dead_code
/// on the non-wasm host build where those constructors are cfg'd out.
#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug)]
pub enum MicError {
    /// 5a — browser blocked the microphone.
    PermissionDenied,
    /// 5b — no input device found.
    NoDevice,
    /// 5c — server-side transcription failed.
    SttServerError,
}

// ---------------------------------------------------------------------------
// MicButton component.
// ---------------------------------------------------------------------------

/// Push-to-talk mic button for the chat composer.
///
/// # Props
///
/// * `on_transcript` — fired with the transcript `String` when the server
///   returns a reply on the normal chat stream path (D-15: transcript is
///   injected into the composer's input signal and submitted).
///
/// STT availability is read REACTIVELY from the `Signal<VoiceStatusState>`
/// context (provided by HermesApp), NOT passed as a plain `bool` prop. The
/// server's `VoiceStatus` snapshot arrives asynchronously over the chat
/// WebSocket AFTER first paint, so a `bool` prop would be stale on mount and
/// — because `use_effect` tracks signal reads, not prop captures — the
/// reconcile effect below would never re-run to enable the button. Reading
/// the signal here subscribes both this render and the effect to it.
#[component]
pub fn MicButton(on_transcript: EventHandler<String>) -> Element {
    let voice_status = use_context::<Signal<super::VoiceStatusState>>();

    let mut state = use_signal(|| {
        if voice_status.read().stt_available {
            MicState::Idle
        } else {
            MicState::Disabled
        }
    });

    // Reconcile resting state (Idle ↔ Disabled) when the server's STT
    // availability changes. Reading `voice_status` subscribes this effect to
    // it, so the async `VoiceStatus` frame correctly flips the button from its
    // Disabled-on-mount default to Idle (the bug a plain `bool` prop caused).
    //
    // FREEZE FIX (36.17.9): this effect also READS `state` (subscribing to it)
    // AND writes it. Dioxus 0.7 signals do NOT deduplicate identical writes —
    // every `.set()` schedules a re-render and re-runs this effect (same fact
    // behind the agents.rs UAT-3 hotfix). An UNCONDITIONAL `set` looped forever
    // (set → re-run → set …), pegging the single-threaded WASM main thread and
    // locking up the page. BOTH branches MUST gate the write on an actual value
    // change so the effect converges after at most one write. The reconcile
    // never clobbers active states (Recording/Transcribing/RequestingPermission/
    // Error) because those are `!= Disabled` and only the resting pair is touched.
    use_effect(move || {
        let stt_available = voice_status.read().stt_available; // Copy bool; borrow ends at ;
        let current = state.read().clone();
        if stt_available {
            if current == MicState::Disabled {
                state.set(MicState::Idle);
            }
        } else if current != MicState::Disabled {
            state.set(MicState::Disabled);
        }
    });

    // Derive CSS class and label from current state.
    let (
        btn_class,
        label,
        aria_label,
        aria_busy,
        aria_disabled,
        aria_pressed,
        title,
        status_text,
        status_class,
    ) = {
        let s = state.read().clone();
        match &s {
            MicState::Idle => (
                "mic-btn",
                "◎ MIC",
                "Start voice input",
                "false",
                "false",
                "false",
                "Click to record — speak, then click again to stop",
                "",
                "mic-status",
            ),
            MicState::RequestingPermission => (
                "mic-btn is-requesting",
                "◌ MIC",
                "Requesting microphone permission\u{2026}",
                "true",
                "false",
                "false",
                "Click to record — speak, then click again to stop",
                "Requesting microphone access\u{2026}",
                "mic-status",
            ),
            MicState::Recording => (
                "mic-btn is-recording",
                "● REC",
                "Recording — click to stop",
                "false",
                "false",
                "true",
                "Click to record — speak, then click again to stop",
                "Recording\u{2026} click again to stop",
                "mic-status",
            ),
            MicState::Transcribing => (
                "mic-btn is-transcribing",
                "◐ STT",
                "Transcribing audio\u{2026}",
                "true",
                "true",
                "false",
                "Click to record — speak, then click again to stop",
                "Transcribing\u{2026}",
                "mic-status",
            ),
            MicState::Error(e) => match e {
                MicError::PermissionDenied => (
                    "mic-btn is-error",
                    "✕ MIC",
                    "Microphone access denied",
                    "false",
                    "false",
                    "false",
                    "Microphone access was denied. Allow microphone in browser settings and reload.",
                    "Mic permission denied — check browser settings",
                    "mic-status is-error",
                ),
                MicError::NoDevice => (
                    "mic-btn is-error",
                    "✕ MIC",
                    "No microphone found",
                    "false",
                    "false",
                    "false",
                    "No microphone detected. Connect a microphone and try again.",
                    "No microphone found",
                    "mic-status is-error",
                ),
                MicError::SttServerError => (
                    "mic-btn is-error",
                    "✕ MIC",
                    "Transcription failed",
                    "false",
                    "false",
                    "false",
                    "Click to record — speak, then click again to stop",
                    "Transcription failed — try again",
                    "mic-status is-error",
                ),
            },
            MicState::Disabled => (
                "mic-btn",
                "◎ MIC",
                "Voice input unavailable — STT not configured",
                "false",
                "true",
                "false",
                "Voice input requires a configured STT provider (GROQ_API_KEY or VOICE_TOOLS_OPENAI_KEY).",
                "",
                "mic-status",
            ),
        }
    };

    // Handle click — toggle between Idle↔Recording; ignore in non-interactive states.
    // `audio_send` is used inside the `#[cfg(target_arch = "wasm32")]` block
    // in `onclick`. On the server-side build path the wasm32 block is compiled
    // out, so suppress the unused-variable lint there.
    #[allow(unused_variables)]
    let audio_send = use_context::<AudioSendHandler>();
    let session_ctx = use_context::<crate::state::SessionIdContext>();
    // Phase 40.5 Plan 08 (D-17): AvatarModeCtx — read active_identity before
    // the async spawn (Pattern B). Provided at HermesApp root (mod.rs).
    #[allow(unused_variables)]
    let avatar_mode_ctx =
        use_context::<crate::components::hermes_app::voice_settings::AvatarModeCtx>();

    let onclick = move |_| {
        let current = state.read().clone();
        match current {
            MicState::Idle => {
                state.set(MicState::RequestingPermission);
                // Kick off getUserMedia + MediaRecorder in a spawn so we
                // don't block the event loop.  All web_sys calls are inside
                // the wasm32 cfg gate; the non-wasm path falls through
                // immediately (server-side render / unit tests).
                let session_id = session_ctx.0.read().clone();
                // Phase 40.5 Plan 08 (D-17): capture active identity into an owned local
                // BEFORE the spawn — no borrow may cross async boundaries (Pattern B).
                #[allow(unused_variables)]
                let active_identity_for_frame: Option<String> = {
                    let slug = avatar_mode_ctx.0.read().active_identity.clone();
                    if crate::components::hermes_app::avatar_logic::is_known_identity(&slug) {
                        Some(slug)
                    } else {
                        None
                    }
                };
                spawn(async move {
                    #[cfg(target_arch = "wasm32")]
                    {
                        use wasm_bindgen::JsCast;
                        use wasm_bindgen_futures::JsFuture;

                        let window = match web_sys::window() {
                            Some(w) => w,
                            None => {
                                state.set(MicState::Error(MicError::NoDevice));
                                return;
                            }
                        };
                        let navigator = window.navigator();
                        let media_devices = match navigator.media_devices() {
                            Ok(m) => m,
                            Err(_) => {
                                state.set(MicState::Error(MicError::NoDevice));
                                return;
                            }
                        };

                        // Build audio-only constraints.
                        let constraints = web_sys::MediaStreamConstraints::new();
                        constraints.set_audio(&wasm_bindgen::JsValue::TRUE);
                        constraints.set_video(&wasm_bindgen::JsValue::FALSE);

                        let stream_promise =
                            match media_devices.get_user_media_with_constraints(&constraints) {
                                Ok(p) => p,
                                Err(_) => {
                                    state.set(MicState::Error(MicError::NoDevice));
                                    return;
                                }
                            };

                        let stream_result = JsFuture::from(stream_promise).await;
                        let stream: web_sys::MediaStream = match stream_result {
                            Ok(s) => match s.dyn_into() {
                                Ok(ms) => ms,
                                Err(_) => {
                                    state.set(MicState::Error(MicError::NoDevice));
                                    return;
                                }
                            },
                            Err(err) => {
                                // Distinguish PermissionDenied (name == "NotAllowedError")
                                // from NoDevice (name == "NotFoundError" / others).
                                let name = js_sys::Reflect::get(&err, &"name".into())
                                    .ok()
                                    .and_then(|v| v.as_string())
                                    .unwrap_or_default();
                                if name == "NotAllowedError" || name == "PermissionDeniedError" {
                                    state.set(MicState::Error(MicError::PermissionDenied));
                                } else {
                                    state.set(MicState::Error(MicError::NoDevice));
                                }
                                return;
                            }
                        };

                        // Permission granted — start recording.
                        let recorder = match web_sys::MediaRecorder::new_with_media_stream(&stream)
                        {
                            Ok(r) => r,
                            Err(_) => {
                                state.set(MicState::Error(MicError::NoDevice));
                                return;
                            }
                        };

                        // ondataavailable fires when stop() is called; collect chunks.
                        // We use a shared Vec via Rc<RefCell> to accumulate chunks.
                        let chunks = std::rc::Rc::new(std::cell::RefCell::new(Vec::<u8>::new()));
                        let chunks_for_cb = chunks.clone();
                        let session_id_for_cb = session_id.clone();

                        let recorder_ref = recorder.clone();

                        let ondataavailable = wasm_bindgen::closure::Closure::<
                            dyn FnMut(web_sys::BlobEvent),
                        >::wrap(Box::new(
                            move |evt: web_sys::BlobEvent| {
                                let session_id_inner = session_id_for_cb.clone();
                                let chunks_inner = chunks_for_cb.clone();
                                // Phase 40.5 Plan 08 (D-17): clone per-invocation so the FnMut
                                // closure stays FnMut (active_identity_for_frame is not moved out).
                                let active_identity_inner = active_identity_for_frame.clone();
                                if let Some(blob) = evt.data() {
                                    let array_buf_promise =
                                        wasm_bindgen_futures::spawn_local(async move {
                                            use wasm_bindgen::JsCast;
                                            let promise: js_sys::Promise =
                                                blob.array_buffer().into();
                                            if let Ok(ab) = JsFuture::from(promise).await {
                                                let array = js_sys::Uint8Array::new(&ab);
                                                let mut buf = vec![0u8; array.length() as usize];
                                                array.copy_to(&mut buf);
                                                // Append to chunks accumulator.
                                                chunks_inner.borrow_mut().extend_from_slice(&buf);

                                                // Build AudioInFrame and send as binary WS frame.
                                                let frame = crate::protocol::AudioInFrame {
                                                    session_id: session_id_inner.clone(),
                                                    mime: "audio/webm;codecs=opus".to_string(),
                                                    bytes: chunks_inner.borrow().clone(),
                                                    // Phase 36.17.9 (D-12): push-to-talk always submits
                                                    // a full turn — wake-word-check path is Wave D only.
                                                    wake_word_check: false,
                                                    // Push-to-talk never carries a wake phrase.
                                                    wake_phrase: None,
                                                    // Phase 40.5 Plan 08 (D-17): active identity
                                                    // cloned per-invocation above the spawn_local.
                                                    active_identity: active_identity_inner,
                                                };
                                                if let Ok(json_bytes) = serde_json::to_vec(&frame) {
                                                    audio_send.0.call(json_bytes);
                                                }
                                                // Transition to Transcribing.
                                                state.set(MicState::Transcribing);

                                                // State 5c auto-clear: return to Idle after 4s if
                                                // the server hasn't responded (error guard).
                                                // The normal path: server reply comes in on the chat
                                                // stream → on_transcript fires → parent calls send
                                                // → we return to Idle via on_transcript handler.
                                                // The fallback timer handles the error case.
                                                let _ =
                                                    wasm_bindgen_futures::spawn_local(async move {
                                                        gloo_timers::future::sleep(
                                                            std::time::Duration::from_secs(4),
                                                        )
                                                        .await;
                                                        let s = state.read().clone();
                                                        if s == MicState::Transcribing {
                                                            state.set(MicState::Error(
                                                                MicError::SttServerError,
                                                            ));
                                                            // Auto-clear error after 2 more seconds.
                                                            wasm_bindgen_futures::spawn_local(
                                                                async move {
                                                                    gloo_timers::future::sleep(
                                                                std::time::Duration::from_secs(2),
                                                            )
                                                            .await;
                                                                    let s2 = state.read().clone();
                                                                    if s2 == MicState::Error(
                                                                        MicError::SttServerError,
                                                                    ) {
                                                                        state.set(MicState::Idle);
                                                                    }
                                                                },
                                                            );
                                                        }
                                                    });
                                            }
                                        });
                                    let _ = array_buf_promise;
                                }
                            },
                        ));

                        recorder_ref
                            .set_ondataavailable(Some(ondataavailable.as_ref().unchecked_ref()));
                        // Leak the closure so it lives for the recorder's lifetime.
                        ondataavailable.forget();

                        if recorder_ref.start().is_err() {
                            state.set(MicState::Error(MicError::NoDevice));
                            return;
                        }

                        state.set(MicState::Recording);

                        // Store recorder in a signal so the stop-click can reach it.
                        // We use a thread_local + Option pattern (no Arc needed on
                        // single-threaded WASM).
                        RECORDER_SLOT.with(|slot| {
                            *slot.borrow_mut() = Some(recorder_ref);
                        });
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Server-side render path: no MediaRecorder API.
                        let _ = session_id;
                        state.set(MicState::Error(MicError::NoDevice));
                    }
                });
            }
            MicState::Recording => {
                // Stop recording — ondataavailable fires synchronously on stop().
                #[cfg(target_arch = "wasm32")]
                RECORDER_SLOT.with(|slot| {
                    if let Some(recorder) = slot.borrow().as_ref() {
                        let _ = recorder.stop();
                    }
                    *slot.borrow_mut() = None;
                });
                // State transitions to Transcribing inside ondataavailable callback.
            }
            MicState::Error(_) => {
                // Click on error state: reset to Idle so user can retry.
                state.set(MicState::Idle);
            }
            // All other states are non-interactive.
            _ => {}
        }
    };

    // Keyboard handler: Escape cancels recording or transcribing.
    let onkeydown = move |evt: KeyboardEvent| {
        if evt.key() == Key::Escape {
            let current = state.read().clone();
            match current {
                MicState::Recording => {
                    #[cfg(target_arch = "wasm32")]
                    RECORDER_SLOT.with(|slot| {
                        if let Some(recorder) = slot.borrow().as_ref() {
                            let _ = recorder.stop();
                        }
                        *slot.borrow_mut() = None;
                    });
                    state.set(MicState::Idle);
                }
                MicState::Transcribing => {
                    state.set(MicState::Idle);
                }
                _ => {}
            }
        }
    };

    let is_disabled = {
        let s = state.read().clone();
        matches!(s, MicState::Disabled | MicState::Transcribing)
    };

    rsx! {
        button {
            r#type: "button",
            class: "{btn_class}",
            aria_label: "{aria_label}",
            aria_busy: "{aria_busy}",
            aria_disabled: "{aria_disabled}",
            aria_pressed: "{aria_pressed}",
            title: "{title}",
            disabled: is_disabled,
            onclick: onclick,
            onkeydown: onkeydown,
            "{label}"
        }
        // Inline status visible text (D-14: sighted users see state inline).
        if !status_text.is_empty() {
            div { class: "{status_class}", "{status_text}" }
        }
        // Screen-reader-only live region (aria-live="polite" for most states;
        // aria-live="assertive" for errors per UI-SPEC §Accessibility).
        div {
            role: "status",
            aria_live: if matches!(state.read().clone(), MicState::Error(_)) { "assertive" } else { "polite" },
            aria_atomic: "true",
            class: "mic-status-sr",
            "{status_text}"
        }
    }
}

// ---------------------------------------------------------------------------
// RECORDER_SLOT — thread_local storage for the active MediaRecorder.
// Single-threaded WASM only; the slot is always None on non-wasm targets.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
thread_local! {
    static RECORDER_SLOT: std::cell::RefCell<Option<web_sys::MediaRecorder>> =
        std::cell::RefCell::new(None);
}

#[cfg(not(target_arch = "wasm32"))]
thread_local! {
    /// No-op slot for server-side / unit-test compilation.
    static RECORDER_SLOT: std::cell::RefCell<Option<()>> =
        const { std::cell::RefCell::new(None) };
}
