//! Phase 54.7 Plan 12 (D-17) -- the generalized Test voice control.
//!
//! One `TestVoiceButton` serves the Global TTS block (auditioning the UNSAVED
//! form values) and the identity section (auditioning the identity's voice).
//! It calls `synth_voice_test`, which always synthesizes the server's fixed
//! phrase (no client text, so the button cannot drive arbitrary TTS spend) and
//! never writes config. Playback goes through the same Blob/Audio path the main
//! conversation uses, behind the shared `AudioPlaybackActiveCtx` half-duplex
//! guard.
//!
//! Plan 13 (D-17) adds `TestMicButton`: a 3-second browser recording
//! transcribed server-side (`test_stt`) with the UNSAVED STT settings. The
//! state machine (`mic_next`), the result copy (`mic_result_copy`) and the DOM
//! error mapping are pure and native-tested; only the recorder itself
//! (`record_clip`: getUserMedia + MediaRecorder) is wasm-only.
//!
//! Every string a result line can show is a fixed sentence plus one member of
//! the `ERROR_CATEGORIES` vocabulary -- raw server or provider text never
//! reaches the DOM. The one exception is the operator's own transcript, which
//! is rendered as a text node only. This file holds no voice default of its own
//! (D-06) and provides no context (a child provider panics ancestor consumers).

// Web-only component tree: native dead-code analysis cannot reach the render
// sites (same rationale as voice_settings.rs). The wasm build lints normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use dioxus::prelude::*;

use crate::components::hermes_app::voice_settings::{AudioPlaybackActiveCtx, RealtimeInFlightCtx};
use crate::server::api::{synth_voice_test, TtsTestSettings, VoiceSampleAudio};
#[cfg(target_arch = "wasm32")]
use crate::server::voice_api::{test_stt, SttTestRequest};
use crate::server::voice_api::{SttTestResult, SttTestSettings};
use crate::state::VoiceModeActiveContext;
use crate::voice_registry::{CATEGORY_UNEXPECTED_RESPONSE, ERROR_CATEGORIES};

/// The server's failure contract: `voice_test:{category}` (the server-fn error
/// wraps it in transport text, so the marker is searched for, not anchored).
const CONTRACT_PREFIX: &str = "voice_test:";

/// The result line for a failed Test voice. Only a member of
/// [`ERROR_CATEGORIES`] is ever echoed; anything else reads
/// `unexpected response`.
pub fn test_voice_error_copy(err: &str) -> String {
    let category = err
        .find(CONTRACT_PREFIX)
        .map(|at| err[at + CONTRACT_PREFIX.len()..].trim())
        .and_then(|rest| {
            ERROR_CATEGORIES
                .iter()
                .find(|known| rest.starts_with(**known))
                .copied()
        })
        .unwrap_or(CATEGORY_UNEXPECTED_RESPONSE);
    format!("Couldn't play sample: {category}. Try again.")
}

/// The result line after any TTS field changed: an old result must never
/// describe new values (UI-SPEC, Test controls), so it is always cleared.
pub fn test_result_after_edit(_previous: Option<String>) -> Option<String> {
    None
}

/// WR-02: whether unmounting the Test voice button must release the shared
/// [`AudioPlaybackActiveCtx`] latch. The button latches it for the whole
/// request + playback; if its scope drops mid-test the settle arms never run,
/// so the latch (which pauses the voice-loop mic) would stay true until reload.
/// Only a button that is itself latched may release it -- another surface's
/// playback must not be cut off by an idle button unmounting.
pub fn releases_playback_latch_on_unmount(was_playing: bool) -> bool {
    was_playing
}

/// The Test voice button and its result line.
///
/// `identity_slug` is `None` for the Global target and `Some(slug)` for an
/// identity; `settings` are the unsaved TTS values to audition (`None` = the
/// saved config); `disabled_reason` disables the button and is shown as its
/// visible reason (a disabled button is never silently inert).
#[component]
pub fn TestVoiceButton(
    identity_slug: Option<String>,
    settings: Option<TtsTestSettings>,
    disabled_reason: Option<String>,
) -> Element {
    let mut playback_active = use_context::<AudioPlaybackActiveCtx>().0;
    let mut playing = use_signal(|| false);
    let mut current_audio: Signal<Option<web_sys::HtmlAudioElement>> = use_signal(|| None);
    let mut result_error: Signal<Option<String>> = use_signal(|| None);
    // Request token: stopping while the synth call is in flight supersedes it,
    // so a late answer never starts playback the operator already cancelled.
    let mut request = use_signal(|| 0u64);

    // The result line clears the moment the audited values change. The plain
    // prop is declared as a reactive dependency (a bare effect would run once).
    use_effect(use_reactive!(|settings| {
        let _ = &settings;
        let previous = result_error.peek().clone();
        if previous.is_some() {
            result_error.set(test_result_after_edit(previous));
        }
    }));

    // Pattern G: .read() in render subscribes (NOT .peek()).
    let is_playing = *playing.read();
    // WR-02: plain-cell mirror of `playing`, refreshed every render, so the
    // unmount hook never reads a panel-local signal whose owner scope is already
    // being torn down. Releases the root-owned playback latch if this button is
    // dropped while latched (request in flight or sample playing).
    let latched_mirror = use_hook(|| std::rc::Rc::new(std::cell::Cell::new(false)));
    latched_mirror.set(is_playing);
    {
        let mirror = latched_mirror.clone();
        dioxus::core::use_drop(move || {
            if releases_playback_latch_on_unmount(mirror.get()) {
                // Supersede any in-flight request so a late answer is inert.
                if let Ok(mut r) = request.try_write() {
                    *r = r.wrapping_add(1);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    if let Ok(slot) = current_audio.try_peek() {
                        if let Some(el) = slot.clone() {
                            let _ = el.pause();
                        }
                    }
                }
                if let Ok(mut pb) = playback_active.try_write() {
                    *pb = false;
                }
            }
        });
    }
    // True only when SOME OTHER voice audio is playing -- never for this
    // button's own playback.
    let other_playing = *playback_active.read() && !is_playing;
    let error_msg = result_error.read().clone();
    let blocked = other_playing || disabled_reason.is_some();
    let label = if is_playing {
        "■ Playing…"
    } else {
        "▶ Test voice"
    };

    rsx! {
        div { class: "voice-field",
            button {
                class: "voice-test-btn",
                r#type: "button",
                disabled: blocked,
                onclick: move |_| {
                    // Belt-and-suspenders: `disabled` already stops the click.
                    if *playback_active.peek() && !*playing.peek() {
                        return;
                    }
                    result_error.set(None);
                    if *playing.peek() {
                        // Clicking while playing (or while the request is in
                        // flight) stops at once and returns to idle.
                        let superseded = request.peek().wrapping_add(1);
                        request.set(superseded);
                        #[cfg(target_arch = "wasm32")]
                        {
                            if let Some(el) = current_audio.peek().clone() {
                                let _ = el.pause();
                            }
                        }
                        current_audio.set(None);
                        playing.set(false);
                        playback_active.set(false);
                        return;
                    }
                    // Owned locals: no signal borrow spans the .await below.
                    let slug_for_call = identity_slug.clone();
                    let settings_for_call = settings.clone();
                    let mine = request.peek().wrapping_add(1);
                    request.set(mine);
                    playing.set(true);
                    playback_active.set(true);
                    spawn(async move {
                        let outcome = synth_voice_test(slug_for_call, settings_for_call).await;
                        if *request.peek() != mine {
                            return;
                        }
                        match outcome {
                            Ok(VoiceSampleAudio { audio_b64, mime }) => {
                                #[cfg(target_arch = "wasm32")]
                                {
                                    let played = play_sample(
                                        &audio_b64,
                                        &mime,
                                        playback_active,
                                        playing,
                                        current_audio,
                                        result_error,
                                    );
                                    if !played {
                                        playback_active.set(false);
                                        playing.set(false);
                                        result_error.set(Some(test_voice_error_copy("")));
                                    }
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    // Server-side render path: no Blob/audio API.
                                    let _ = (audio_b64, mime);
                                    playback_active.set(false);
                                    playing.set(false);
                                }
                            }
                            Err(e) => {
                                // The synth request itself failed: back to idle,
                                // retriable, with a category-only line.
                                playback_active.set(false);
                                playing.set(false);
                                result_error.set(Some(test_voice_error_copy(&e.to_string())));
                            }
                        }
                    });
                },
                "{label}"
            }
            // A disabled button always carries a visible reason; a failure
            // always carries a retriable one.
            if other_playing {
                p {
                    class: "voice-test-result",
                    role: "status",
                    "aria-live": "polite",
                    "Can't test while voice is playing."
                }
            } else if let Some(reason) = disabled_reason {
                p {
                    class: "voice-test-result",
                    role: "status",
                    "aria-live": "polite",
                    "{reason}"
                }
            } else if let Some(msg) = error_msg {
                p {
                    class: "voice-test-result is-error",
                    role: "status",
                    "aria-live": "polite",
                    "{msg}"
                }
            }
        }
    }
}

// ── Test mic (Plan 13, D-17) ────────────────────────────────────────────────

/// Seconds the clip is recorded for (UI-SPEC: counts 3 to 0).
const RECORD_SECONDS: u8 = 3;

/// Longest transcript shown in the result line, in characters.
const TRANSCRIPT_CLAMP: usize = 240;

/// Why Test mic is disabled while voice mode owns the microphone.
const VOICE_RUNNING_REASON: &str = "Can't test while voice mode is running.";

/// The pseudo DOM error name for "this browser has no MediaRecorder".
pub const MEDIA_RECORDER_MISSING: &str = "MediaRecorderUnsupported";

/// Where one Test mic run is.
#[derive(Clone, Debug, PartialEq)]
pub enum MicState {
    Idle,
    /// Seconds left, counting 3 down to 0.
    Recording(u8),
    Transcribing,
    Heard(String),
    Empty,
    /// A category from the fixed `ERROR_CATEGORIES` vocabulary.
    Failed(String),
    PermissionDenied,
    Unsupported,
}

/// What can happen to a Test mic run.
#[derive(Clone, Debug, PartialEq)]
pub enum MicEvent {
    Start,
    Tick,
    Result(SttTestResult),
    /// A DOM exception name (or [`MEDIA_RECORDER_MISSING`]).
    DomError(String),
    /// An STT field changed: a finished result no longer describes the values.
    FieldChanged,
}

/// True while a recording or transcription is in flight (one at a time).
pub fn mic_is_busy(state: &MicState) -> bool {
    matches!(state, MicState::Recording(_) | MicState::Transcribing)
}

/// A category from the fixed vocabulary; anything else reads
/// `unexpected response` so server text never reaches the DOM.
fn known_category(raw: &str) -> String {
    ERROR_CATEGORIES
        .iter()
        .find(|known| **known == raw.trim())
        .copied()
        .unwrap_or(CATEGORY_UNEXPECTED_RESPONSE)
        .to_string()
}

/// The state a finished transcription leads to.
fn mic_result_state(result: SttTestResult) -> MicState {
    let heard = result
        .transcript
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let Some(text) = heard {
        return MicState::Heard(text.to_string());
    }
    match result.category {
        Some(category) => MicState::Failed(known_category(&category)),
        None => MicState::Empty,
    }
}

/// The pure Test mic reducer. `Start` is accepted from `Idle` only (one test at
/// a time); a ticking countdown ends in `Transcribing`; a result lands only
/// while transcribing; an edit to any STT field returns a finished run to
/// `Idle` and leaves a busy one alone.
pub fn mic_next(state: &MicState, event: MicEvent) -> MicState {
    match (state, event) {
        (MicState::Idle, MicEvent::Start) => MicState::Recording(RECORD_SECONDS),
        (MicState::Recording(left), MicEvent::Tick) if *left > 0 => MicState::Recording(left - 1),
        (MicState::Recording(_), MicEvent::Tick) => MicState::Transcribing,
        (MicState::Transcribing, MicEvent::Result(result)) => mic_result_state(result),
        (MicState::Recording(_) | MicState::Transcribing, MicEvent::DomError(name)) => {
            mic_error_from_dom_name(&name)
        }
        (other, MicEvent::FieldChanged) if !mic_is_busy(other) => MicState::Idle,
        (other, _) => other.clone(),
    }
}

/// Map a DOM exception name onto the state it means. A denied permission and a
/// missing device / recorder have their own copy; anything else is a failure
/// with the generic category.
pub fn mic_error_from_dom_name(name: &str) -> MicState {
    match name {
        "NotAllowedError" | "SecurityError" => MicState::PermissionDenied,
        "NotFoundError" | MEDIA_RECORDER_MISSING => MicState::Unsupported,
        _ => MicState::Failed(CATEGORY_UNEXPECTED_RESPONSE.to_string()),
    }
}

/// `text` limited to [`TRANSCRIPT_CLAMP`] characters (counted as characters,
/// never bytes); a longer text ends in an ellipsis inside the limit.
fn clamp_transcript(text: &str) -> String {
    if text.chars().count() <= TRANSCRIPT_CLAMP {
        return text.to_string();
    }
    let mut clamped: String = text.chars().take(TRANSCRIPT_CLAMP - 1).collect();
    clamped.push('…');
    clamped
}

/// The text a state shows (the button label while busy, the result line
/// otherwise) and the `title` carrying the full transcript. Empty for `Idle`.
pub fn mic_result_copy(state: &MicState) -> (String, Option<String>) {
    match state {
        MicState::Idle => (String::new(), None),
        MicState::Recording(left) => (format!("Recording… {left}s"), None),
        MicState::Transcribing => ("Transcribing…".to_string(), None),
        MicState::Heard(text) => (
            format!("Heard: \"{}\"", clamp_transcript(text)),
            Some(text.clone()),
        ),
        MicState::Empty => (
            "Nothing was transcribed. Check your microphone and try again.".to_string(),
            None,
        ),
        MicState::Failed(category) => (format!("Couldn't transcribe: {category}."), None),
        MicState::PermissionDenied => (
            "Microphone access is blocked. Allow it in your browser's site settings and try again."
                .to_string(),
            None,
        ),
        MicState::Unsupported => ("This browser can't record audio.".to_string(), None),
    }
}

/// True while a result computed from `started` settings still describes the
/// form (no STT field changed since the click).
fn result_is_current(started_epoch: u64, now_epoch: u64) -> bool {
    started_epoch == now_epoch
}

/// The Test mic button and its result line (D-17).
///
/// `settings` are the unsaved STT values to transcribe with (`None` = the saved
/// config); `disabled_reason` disables the button and is its visible reason
/// (an invalid Base URL). Voice mode or a live realtime session owns the
/// microphone, so both disable it with their own reason.
#[component]
pub fn TestMicButton(
    settings: Option<SttTestSettings>,
    disabled_reason: Option<String>,
) -> Element {
    // Absent outside the full app (panel tests); then nothing is live.
    let voice_mode_running = try_use_context::<VoiceModeActiveContext>()
        .map(|ctx| *ctx.0.read())
        .unwrap_or(false);
    let session_live = try_use_context::<RealtimeInFlightCtx>()
        .map(|ctx| *ctx.0.read())
        .unwrap_or(false);

    let mut mic = use_signal(|| MicState::Idle);
    // Bumped whenever the audited settings change: a transcript that arrives
    // for an older epoch is dropped (an old result never describes new values).
    let mut epoch = use_signal(|| 0u64);

    // The plain prop is declared as a reactive dependency (a bare effect would
    // run once). Covers every STT control, pickers included.
    use_effect(use_reactive!(|settings| {
        let _ = &settings;
        let current = mic.peek().clone();
        let next = mic_next(&current, MicEvent::FieldChanged);
        if next != current {
            mic.set(next);
        }
        let bumped = epoch.peek().wrapping_add(1);
        epoch.set(bumped);
    }));

    // Pattern G: .read() in render subscribes (NOT .peek()).
    let state = mic.read().clone();
    let busy = mic_is_busy(&state);
    let blocked_reason = if voice_mode_running || session_live {
        Some(VOICE_RUNNING_REASON.to_string())
    } else {
        disabled_reason
    };
    let disabled = busy || blocked_reason.is_some();
    let label = if busy {
        mic_result_copy(&state).0
    } else {
        "● Test mic".to_string()
    };
    // A disabled button always carries a visible reason; a finished run its result.
    let (line, title, tone) = match (&blocked_reason, busy) {
        (Some(reason), _) => (reason.clone(), None, ""),
        (None, true) => (String::new(), None, ""),
        (None, false) => {
            let (text, title) = mic_result_copy(&state);
            let tone = match state {
                MicState::Heard(_) => " is-ok",
                MicState::Idle => "",
                _ => " is-error",
            };
            (text, title, tone)
        }
    };

    rsx! {
        div { class: "voice-field",
            button {
                class: "voice-test-btn",
                r#type: "button",
                disabled,
                onclick: move |_| {
                    // Belt-and-suspenders: `disabled` already stops the click.
                    if mic_is_busy(&mic.peek()) {
                        return;
                    }
                    // A finished result is dismissed by the next run.
                    mic.set(mic_next(&MicState::Idle, MicEvent::Start));
                    #[cfg(target_arch = "wasm32")]
                    {
                        let settings_for_call = settings.clone();
                        let started_epoch = *epoch.peek();
                        spawn(run_mic_test(mic, epoch, started_epoch, settings_for_call));
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        // Server-side render path: there is no browser to record.
                        mic.set(mic_error_from_dom_name(MEDIA_RECORDER_MISSING));
                    }
                },
                "{label}"
            }
            if !line.is_empty() {
                p {
                    class: "voice-test-result{tone}",
                    role: "status",
                    "aria-live": "polite",
                    title: title,
                    "{line}"
                }
            }
        }
    }
}

/// Apply one event to the run's state.
#[cfg(target_arch = "wasm32")]
fn advance(mut mic: Signal<MicState>, event: MicEvent) {
    let current = mic.peek().clone();
    let next = mic_next(&current, event);
    mic.set(next);
}

/// One whole Test mic run: record, transcribe with the unsaved settings, show
/// the result. Owned values only; no signal borrow spans an `.await`.
#[cfg(target_arch = "wasm32")]
async fn run_mic_test(
    mut mic: Signal<MicState>,
    epoch: Signal<u64>,
    started_epoch: u64,
    settings: Option<SttTestSettings>,
) {
    use base64::Engine as _;

    let clip = record_clip(|| advance(mic, MicEvent::Tick)).await;
    let outcome = match clip {
        Err(name) => MicEvent::DomError(name),
        Ok(clip) if clip.bytes.is_empty() => MicEvent::Result(SttTestResult {
            transcript: None,
            empty: true,
            category: None,
        }),
        Ok(clip) => {
            let request = SttTestRequest {
                clip_b64: base64::engine::general_purpose::STANDARD.encode(&clip.bytes),
                mime: clip.mime,
                settings,
            };
            let result = match test_stt(request).await {
                Ok(result) => result,
                // The call to our own server failed: the same fixed category.
                Err(_) => SttTestResult {
                    transcript: None,
                    empty: false,
                    category: Some(crate::voice_registry::CATEGORY_SERVER_UNREACHABLE.to_string()),
                },
            };
            MicEvent::Result(result)
        }
    };
    if !result_is_current(started_epoch, *epoch.peek()) {
        // A field changed while the test ran: its answer describes old values.
        mic.set(MicState::Idle);
        return;
    }
    advance(mic, outcome);
}

/// One recorded clip: the container bytes and the recorder's MIME type.
#[cfg(target_arch = "wasm32")]
struct RecordedClip {
    bytes: Vec<u8>,
    mime: String,
}

/// Stops every track of the captured stream when dropped, on every exit path
/// (error, early return, the future being dropped with its component), so the
/// browser's recording indicator always goes away (T-54.7-57).
#[cfg(target_arch = "wasm32")]
struct StreamGuard(web_sys::MediaStream);

#[cfg(target_arch = "wasm32")]
impl Drop for StreamGuard {
    fn drop(&mut self) {
        use wasm_bindgen::JsCast as _;
        let tracks = self.0.get_tracks();
        for index in 0..tracks.length() {
            if let Some(track) = tracks.get(index).dyn_ref::<web_sys::MediaStreamTrack>() {
                track.stop();
            }
        }
    }
}

/// Detaches the recorder's handlers and stops it when dropped, so no browser
/// event can reach a dropped closure.
#[cfg(target_arch = "wasm32")]
struct RecorderGuard(web_sys::MediaRecorder);

#[cfg(target_arch = "wasm32")]
impl Drop for RecorderGuard {
    fn drop(&mut self) {
        self.0.set_ondataavailable(None);
        self.0.set_onstop(None);
        // Stopping an inactive recorder is an error we do not care about.
        let _ = self.0.stop();
    }
}

/// The `name` of a thrown DOM exception (empty when it has none).
#[cfg(target_arch = "wasm32")]
fn js_error_name(error: &wasm_bindgen::JsValue) -> String {
    js_sys::Reflect::get(error, &wasm_bindgen::JsValue::from_str("name"))
        .ok()
        .and_then(|name| name.as_string())
        .unwrap_or_default()
}

/// Record [`RECORD_SECONDS`] of microphone audio (getUserMedia + MediaRecorder),
/// calling `on_tick` once per elapsed second and once more as the recorder
/// stops. `Err` carries a DOM exception name for [`mic_error_from_dom_name`].
#[cfg(target_arch = "wasm32")]
async fn record_clip(mut on_tick: impl FnMut()) -> Result<RecordedClip, String> {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast as _, JsValue};
    use wasm_bindgen_futures::JsFuture;

    let has_recorder = js_sys::Reflect::has(&js_sys::global(), &JsValue::from_str("MediaRecorder"))
        .unwrap_or(false);
    if !has_recorder {
        return Err(MEDIA_RECORDER_MISSING.to_string());
    }
    let media_devices = web_sys::window()
        .ok_or_else(|| "NotFoundError".to_string())?
        .navigator()
        .media_devices()
        .map_err(|error| js_error_name(&error))?;
    let constraints = web_sys::MediaStreamConstraints::new();
    constraints.set_audio(&JsValue::TRUE);
    constraints.set_video(&JsValue::FALSE);
    let permission = media_devices
        .get_user_media_with_constraints(&constraints)
        .map_err(|error| js_error_name(&error))?;
    let stream: web_sys::MediaStream = JsFuture::from(permission)
        .await
        .map_err(|error| js_error_name(&error))?
        .dyn_into()
        .map_err(|_| "NotFoundError".to_string())?;
    // Declared first, dropped last: the tracks stop whatever happens below.
    let stream_guard = StreamGuard(stream);

    let recorder = web_sys::MediaRecorder::new_with_media_stream(&stream_guard.0)
        .map_err(|_| MEDIA_RECORDER_MISSING.to_string())?;
    let blobs: Rc<RefCell<Vec<web_sys::Blob>>> = Rc::default();
    let stopped = Rc::new(Cell::new(false));
    let on_data = {
        let blobs = blobs.clone();
        Closure::<dyn FnMut(web_sys::BlobEvent)>::wrap(Box::new(
            move |event: web_sys::BlobEvent| {
                if let Some(blob) = event.data() {
                    blobs.borrow_mut().push(blob);
                }
            },
        ))
    };
    let on_stop = {
        let stopped = stopped.clone();
        Closure::<dyn FnMut()>::wrap(Box::new(move || stopped.set(true)))
    };
    // Declared after the closures, so it drops (and detaches them) first.
    let recorder_guard = RecorderGuard(recorder);
    recorder_guard
        .0
        .set_ondataavailable(Some(on_data.as_ref().unchecked_ref()));
    recorder_guard
        .0
        .set_onstop(Some(on_stop.as_ref().unchecked_ref()));
    recorder_guard
        .0
        .start()
        .map_err(|_| MEDIA_RECORDER_MISSING.to_string())?;

    for _ in 0..RECORD_SECONDS {
        gloo_timers::future::TimeoutFuture::new(1000).await;
        on_tick();
    }
    // The countdown reached 0: show Transcribing while the recorder flushes.
    on_tick();
    let _ = recorder_guard.0.stop();
    // `stop` fires the final dataavailable and then the stop event, so once the
    // flag is set every blob is collected. Bounded so a stuck recorder fails.
    for _ in 0..40 {
        if stopped.get() {
            break;
        }
        gloo_timers::future::TimeoutFuture::new(50).await;
    }
    let mime = match recorder_guard.0.mime_type() {
        mime if mime.is_empty() => "audio/webm".to_string(),
        mime => mime,
    };
    let collected: Vec<web_sys::Blob> = blobs.borrow().clone();
    let mut bytes = Vec::new();
    for blob in collected {
        let promise: js_sys::Promise = blob.array_buffer().into();
        let buffer = JsFuture::from(promise)
            .await
            .map_err(|error| js_error_name(&error))?;
        let view = js_sys::Uint8Array::new(&buffer);
        let mut chunk = vec![0u8; view.length() as usize];
        view.copy_to(&mut chunk);
        bytes.extend_from_slice(&chunk);
    }
    Ok(RecordedClip { bytes, mime })
}

/// Decode the sample and play it through an `<audio>` element built from a Blob
/// URL (the same steps the main conversation's AudioOut handler runs, fed by a
/// server-fn return instead of a WS binary frame). Returns whether playback
/// started; the `ended` / `error` listeners return the control to idle.
#[cfg(target_arch = "wasm32")]
fn play_sample(
    audio_b64: &str,
    mime: &str,
    playback_active: Signal<bool>,
    playing: Signal<bool>,
    current_audio: Signal<Option<web_sys::HtmlAudioElement>>,
    result_error: Signal<Option<String>>,
) -> bool {
    use base64::Engine as _;
    use wasm_bindgen::JsCast as _;

    let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(audio_b64) else {
        return false;
    };
    let uint8_array = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&uint8_array);
    let opts = web_sys::BlobPropertyBag::new();
    opts.set_type(mime);
    let Ok(blob) = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts) else {
        return false;
    };
    let Ok(url) = web_sys::Url::create_object_url_with_blob(&blob) else {
        return false;
    };
    let Ok(audio_el) = web_sys::HtmlAudioElement::new_with_src(&url) else {
        // IN-07: no element will ever play it, so release the decoded sample now.
        let _ = web_sys::Url::revoke_object_url(&url);
        return false;
    };
    crate::components::hermes_app::voice_loop::tap_speaking_analyser(&audio_el);
    let mut audio_slot = current_audio;
    audio_slot.set(Some(audio_el.clone()));

    // ended: playback completed normally -> idle.
    let ended_cb = {
        let mut pb = playback_active;
        let mut tv = playing;
        let mut cur = current_audio;
        let url = url.clone();
        wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(Box::new(move || {
            // IN-07: playback is over; release the Blob the object URL retains.
            let _ = web_sys::Url::revoke_object_url(&url);
            // WR-02: the button may have unmounted since; its own signals are then
            // gone and a plain `.set` would panic inside this wasm callback.
            pb.set(false);
            if let Ok(mut w) = tv.try_write() {
                *w = false;
            }
            if let Ok(mut w) = cur.try_write() {
                *w = None;
            }
        }))
    };
    // error: playback failed -> idle + the retriable category line.
    let error_cb = {
        let mut pb = playback_active;
        let mut tv = playing;
        let mut cur = current_audio;
        let mut err = result_error;
        let url = url.clone();
        wasm_bindgen::closure::Closure::<dyn FnMut()>::wrap(Box::new(move || {
            // IN-07: playback failed; release the Blob the object URL retains.
            let _ = web_sys::Url::revoke_object_url(&url);
            pb.set(false);
            if let Ok(mut w) = tv.try_write() {
                *w = false;
            }
            if let Ok(mut w) = cur.try_write() {
                *w = None;
            }
            if let Ok(mut w) = err.try_write() {
                *w = Some(test_voice_error_copy(""));
            }
        }))
    };
    let _ = audio_el.add_event_listener_with_callback("ended", ended_cb.as_ref().unchecked_ref());
    let _ = audio_el.add_event_listener_with_callback("error", error_cb.as_ref().unchecked_ref());
    // Keep the closures alive until they fire.
    ended_cb.forget();
    error_cb.forget();
    let _ = audio_el.play();
    true
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::components::hermes_app::voice_settings::AudioPlaybackActiveCtx;
    use crate::voice_registry::ERROR_CATEGORIES;
    use std::cell::RefCell;

    /// SSR escapes quotes and apostrophes in text nodes; match any spelling.
    fn has_text(html: &str, text: &str) -> bool {
        let apos = ["'", "&#39;", "&#x27;"];
        apos.iter()
            .any(|a| html.contains(text.replace('\'', a).as_str()))
    }

    thread_local! {
        /// (other audio playing, disabled reason)
        static CASE: RefCell<(bool, Option<String>)> = const { RefCell::new((false, None)) };
    }

    fn render_button(other_playing: bool, reason: Option<&str>) -> String {
        CASE.with(|c| *c.borrow_mut() = (other_playing, reason.map(str::to_string)));

        fn app() -> Element {
            let (other_playing, reason) = CASE.with(|c| c.borrow().clone());
            let playing = use_signal(|| other_playing);
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            rsx! {
                TestVoiceButton {
                    identity_slug: None,
                    settings: None,
                    disabled_reason: reason,
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The `<button ...>` element of the rendered control.
    fn button_markup(html: &str) -> &str {
        let at = html.find("voice-test-btn").expect("a Test voice button");
        let start = html[..at].rfind("<button").expect("a button element");
        let end = at + html[at..].find("</button>").expect("a closed button");
        &html[start..end]
    }

    #[test]
    fn test_voice_button_states() {
        // Idle.
        let idle = render_button(false, None);
        let button = button_markup(&idle);
        assert!(button.contains("▶ Test voice"), "{idle}");
        assert!(button.contains("class=\"voice-test-btn\""), "{idle}");
        assert!(!button.contains("disabled"), "idle is enabled: {idle}");
        assert!(
            !has_text(&idle, "Can't test while voice is playing."),
            "{idle}"
        );
        // The shared style replaces the old inline literal.
        assert!(!button.contains("style="), "no inline style: {idle}");

        // Other voice audio is playing: disabled, with the visible reason.
        let busy = render_button(true, None);
        assert!(button_markup(&busy).contains("disabled"), "{busy}");
        assert!(
            has_text(&busy, "Can't test while voice is playing."),
            "{busy}"
        );

        // A caller-supplied reason disables the button and is shown.
        let reasoned = render_button(
            false,
            Some("Enter the server's address to use this provider."),
        );
        assert!(button_markup(&reasoned).contains("disabled"), "{reasoned}");
        assert!(
            has_text(
                &reasoned,
                "Enter the server's address to use this provider."
            ),
            "{reasoned}"
        );
    }

    thread_local! {
        static UNMOUNT_SHOW: std::cell::Cell<bool> = const { std::cell::Cell::new(true) };
        static UNMOUNT_LATCH: RefCell<Option<Signal<bool>>> = const { RefCell::new(None) };
    }

    /// WR-02: only a button that is itself latched releases the shared playback
    /// latch on unmount; an idle button unmounting must not cut off some other
    /// surface's playback. (The latched path needs a live click + wasm Audio, so
    /// its decision is the pure helper; the wiring is guarded below.)
    #[test]
    fn unmounting_an_idle_button_leaves_the_playback_latch_alone() {
        assert!(releases_playback_latch_on_unmount(true));
        assert!(!releases_playback_latch_on_unmount(false));

        fn app() -> Element {
            // Another surface (the main conversation) holds the latch.
            let latch = use_signal(|| true);
            use_context_provider(|| AudioPlaybackActiveCtx(latch));
            UNMOUNT_LATCH.with(|l| *l.borrow_mut() = Some(latch));
            if UNMOUNT_SHOW.with(|s| s.get()) {
                rsx! { TestVoiceButton { identity_slug: None, settings: None, disabled_reason: None } }
            } else {
                rsx! { div {} }
            }
        }
        UNMOUNT_SHOW.with(|s| s.set(true));
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        UNMOUNT_SHOW.with(|s| s.set(false));
        dom.mark_dirty(ScopeId::APP);
        dom.render_immediate(&mut dioxus::core::NoOpMutations);
        let latch = UNMOUNT_LATCH.with(|l| l.borrow().expect("latch signal captured"));
        assert!(
            dom.in_runtime(|| *latch.peek()),
            "an idle button unmounting must not release another surface's latch"
        );

        // The unmount hook and the panic-safe settle callbacks stay wired.
        let src = include_str!("voice_test_controls.rs");
        let cut = src.find("#[cfg(all(test, not(target_arch").expect("test marker");
        let prod = &src[..cut];
        assert!(prod.contains("dioxus::core::use_drop("), "unmount hook present");
        assert!(
            prod.contains("releases_playback_latch_on_unmount(mirror.get())"),
            "unmount hook consults the latched mirror"
        );
        assert!(
            prod.contains("tv.try_write()"),
            "ended/error callbacks must tolerate an unmounted button"
        );
    }

    /// IN-07: `play_sample` is wasm-only (Blob/Audio), so the revoke is guarded at
    /// the source level: every create_object_url is paired with revokes in the
    /// ended and error callbacks and on the early-return path.
    #[test]
    fn play_sample_revokes_its_object_url() {
        let src = include_str!("voice_test_controls.rs");
        let start = src.find("fn play_sample(").expect("play_sample present");
        let end = src[start..]
            .find("#[cfg(all(test, not(target_arch")
            .map(|n| start + n)
            .expect("test marker");
        let body = &src[start..end];
        assert_eq!(body.matches("create_object_url_with_blob").count(), 1);
        assert_eq!(
            body.matches("revoke_object_url(&url)").count(),
            3,
            "early return + ended + error must each revoke"
        );
    }

    #[test]
    fn test_voice_error_copy() {
        assert_eq!(
            super::test_voice_error_copy("voice_test:key rejected"),
            "Couldn't play sample: key rejected. Try again."
        );
        // The server-fn error wraps the message; the contract is found inside it.
        assert_eq!(
            super::test_voice_error_copy("error running server function: voice_test:timed out"),
            "Couldn't play sample: timed out. Try again."
        );
        for category in ERROR_CATEGORIES {
            assert_eq!(
                super::test_voice_error_copy(&format!("voice_test:{category}")),
                format!("Couldn't play sample: {category}. Try again.")
            );
        }
        // Unrecognised text never reaches the DOM.
        for raw in [
            "boom: secret-token-123",
            "voice_test:<script>alert(1)</script>",
            "voice_test:",
            "",
        ] {
            let copy = super::test_voice_error_copy(raw);
            assert_eq!(
                copy, "Couldn't play sample: unexpected response. Try again.",
                "{raw}"
            );
        }
    }

    /// An old result must never describe new values: any recorded result is
    /// dropped once a TTS field changes.
    #[test]
    fn tts_test_result_clears_on_edit() {
        let recorded = super::test_voice_error_copy("voice_test:key rejected");
        assert_eq!(test_result_after_edit(Some(recorded)), None);
        assert_eq!(test_result_after_edit(None), None);
    }

    #[test]
    fn voice_test_controls_has_no_hardcoded_voice_defaults() {
        let src = include_str!("voice_test_controls.rs");
        for needle in crate::voice_registry::hardcoded_voice_default_needles() {
            assert!(
                !src.contains(needle.as_str()),
                "voice_test_controls.rs still holds a hard-coded voice default: {needle}"
            );
        }
    }

    // ── Plan 13 Task 1: Test mic ─────────────────────────────────────────────

    fn transcript(text: &str) -> SttTestResult {
        SttTestResult {
            transcript: Some(text.to_string()),
            empty: false,
            category: None,
        }
    }

    fn category(name: &str) -> SttTestResult {
        SttTestResult {
            transcript: None,
            empty: false,
            category: Some(name.to_string()),
        }
    }

    #[test]
    fn test_mic_state_machine() {
        // Start from idle begins a 3 s recording.
        let mut state = mic_next(&MicState::Idle, MicEvent::Start);
        assert_eq!(state, MicState::Recording(3));
        // The countdown runs 3 -> 0, then the next tick moves to Transcribing.
        for expected in [2u8, 1, 0] {
            state = mic_next(&state, MicEvent::Tick);
            assert_eq!(state, MicState::Recording(expected));
        }
        state = mic_next(&state, MicEvent::Tick);
        assert_eq!(state, MicState::Transcribing);

        // Results.
        assert_eq!(
            mic_next(&MicState::Transcribing, MicEvent::Result(transcript("Hi."))),
            MicState::Heard("Hi.".to_string())
        );
        assert_eq!(
            mic_next(
                &MicState::Transcribing,
                MicEvent::Result(SttTestResult {
                    transcript: None,
                    empty: true,
                    category: None,
                })
            ),
            MicState::Empty
        );
        assert_eq!(
            mic_next(
                &MicState::Transcribing,
                MicEvent::Result(category("timed out"))
            ),
            MicState::Failed("timed out".to_string())
        );
        // A category outside the fixed vocabulary never reaches the DOM.
        assert_eq!(
            mic_next(
                &MicState::Transcribing,
                MicEvent::Result(category("secret-token-123"))
            ),
            MicState::Failed("unexpected response".to_string())
        );
        // A blank transcript is "nothing transcribed".
        assert_eq!(
            mic_next(&MicState::Transcribing, MicEvent::Result(transcript("  "))),
            MicState::Empty
        );

        // One test at a time: Start outside idle is ignored.
        for busy in [
            MicState::Recording(2),
            MicState::Transcribing,
            MicState::Heard("Hi.".to_string()),
            MicState::Empty,
            MicState::Failed("timed out".to_string()),
            MicState::PermissionDenied,
            MicState::Unsupported,
        ] {
            assert_eq!(mic_next(&busy, MicEvent::Start), busy, "{busy:?}");
        }
        // A result with no request in flight changes nothing.
        assert_eq!(
            mic_next(&MicState::Idle, MicEvent::Result(transcript("Hi."))),
            MicState::Idle
        );

        // A DOM error while recording or transcribing ends the test.
        assert_eq!(
            mic_next(
                &MicState::Recording(3),
                MicEvent::DomError("NotAllowedError".to_string())
            ),
            MicState::PermissionDenied
        );
        assert_eq!(
            mic_next(
                &MicState::Transcribing,
                MicEvent::DomError("NotFoundError".to_string())
            ),
            MicState::Unsupported
        );
    }

    #[test]
    fn test_mic_result_copy() {
        // A long transcript is clamped to 240 characters; the title holds all of it.
        let long: String = "word ".repeat(60);
        assert_eq!(long.chars().count(), 300);
        let (text, title) = mic_result_copy(&MicState::Heard(long.clone()));
        let heard = text
            .strip_prefix("Heard: \"")
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or_else(|| panic!("Heard line shape: {text}"));
        assert_eq!(heard.chars().count(), 240, "{text}");
        assert!(heard.ends_with('…'), "{text}");
        assert_eq!(title.as_deref(), Some(long.as_str()));

        // A short transcript is shown whole.
        let (text, title) = mic_result_copy(&MicState::Heard("Hi.".to_string()));
        assert_eq!(text, "Heard: \"Hi.\"");
        assert_eq!(title.as_deref(), Some("Hi."));

        // Clamping counts characters, never bytes (no split code point).
        let wide = "é".repeat(300);
        let (text, _) = mic_result_copy(&MicState::Heard(wide));
        assert!(text.contains('é'));

        let copy = |state: MicState| mic_result_copy(&state).0;
        assert_eq!(
            copy(MicState::Empty),
            "Nothing was transcribed. Check your microphone and try again."
        );
        assert_eq!(
            copy(MicState::Failed("timed out".to_string())),
            "Couldn't transcribe: timed out."
        );
        assert_eq!(
            copy(MicState::PermissionDenied),
            "Microphone access is blocked. Allow it in your browser's site settings and try again."
        );
        assert_eq!(
            copy(MicState::Unsupported),
            "This browser can't record audio."
        );
        assert_eq!(copy(MicState::Recording(2)), "Recording… 2s");
        assert_eq!(copy(MicState::Transcribing), "Transcribing…");
        assert_eq!(copy(MicState::Idle), "");
    }

    #[test]
    fn mic_error_from_dom_name_maps_permission_and_support() {
        assert_eq!(
            mic_error_from_dom_name("NotAllowedError"),
            MicState::PermissionDenied
        );
        assert_eq!(
            mic_error_from_dom_name("SecurityError"),
            MicState::PermissionDenied
        );
        assert_eq!(
            mic_error_from_dom_name("NotFoundError"),
            MicState::Unsupported
        );
        assert_eq!(
            mic_error_from_dom_name(MEDIA_RECORDER_MISSING),
            MicState::Unsupported
        );
        assert_eq!(
            mic_error_from_dom_name("SomethingElse"),
            MicState::Failed("unexpected response".to_string())
        );
        assert_eq!(
            mic_error_from_dom_name(""),
            MicState::Failed("unexpected response".to_string())
        );
    }

    thread_local! {
        /// (voice mode running, realtime in flight, disabled reason)
        static MIC_CASE: RefCell<(bool, bool, Option<String>)> =
            const { RefCell::new((false, false, None)) };
    }

    fn render_mic(voice_mode: bool, in_flight: bool, reason: Option<&str>) -> String {
        MIC_CASE.with(|c| *c.borrow_mut() = (voice_mode, in_flight, reason.map(str::to_string)));

        fn app() -> Element {
            use crate::components::hermes_app::voice_settings::RealtimeInFlightCtx;
            use crate::state::VoiceModeActiveContext;
            let (voice_mode, in_flight, reason) = MIC_CASE.with(|c| c.borrow().clone());
            let voice_sig = use_signal(|| voice_mode);
            use_context_provider(|| VoiceModeActiveContext(voice_sig));
            let flight_sig = use_signal(|| in_flight);
            use_context_provider(|| RealtimeInFlightCtx(flight_sig));
            rsx! {
                TestMicButton { settings: None, disabled_reason: reason }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn test_mic_disabled_while_voice_mode_running() {
        // Idle: enabled, with the idle label and no reason line.
        let idle = render_mic(false, false, None);
        let button = button_markup(&idle);
        assert!(button.contains("● Test mic"), "{idle}");
        assert!(button.contains("class=\"voice-test-btn\""), "{idle}");
        assert!(!button.contains("disabled"), "idle is enabled: {idle}");
        assert!(
            !has_text(&idle, "Can't test while voice mode is running."),
            "{idle}"
        );

        // Voice mode owns the microphone: disabled with the visible reason.
        for (voice_mode, in_flight) in [(true, false), (false, true)] {
            let html = render_mic(voice_mode, in_flight, None);
            assert!(
                button_markup(&html).contains("disabled"),
                "voice_mode={voice_mode} in_flight={in_flight}: {html}"
            );
            assert!(
                has_text(&html, "Can't test while voice mode is running."),
                "{html}"
            );
        }

        // A caller reason (an invalid Base URL) disables it and is visible.
        let reasoned = render_mic(
            false,
            false,
            Some("Enter the server's address to use this provider."),
        );
        assert!(button_markup(&reasoned).contains("disabled"), "{reasoned}");
        assert!(
            has_text(
                &reasoned,
                "Enter the server's address to use this provider."
            ),
            "{reasoned}"
        );
        assert!(reasoned.contains("role=\"status\""), "{reasoned}");
        assert!(reasoned.contains("aria-live=\"polite\""), "{reasoned}");
    }

    #[test]
    fn test_mic_transcript_is_a_text_node() {
        let src = include_str!("voice_test_controls.rs");
        let needle = ["dangerous_inner", "_html"].concat();
        assert!(
            !src.contains(needle.as_str()),
            "the transcript must never be rendered as HTML"
        );
    }

    #[test]
    fn mic_result_clears_on_stt_edit() {
        // A finished result never describes edited values.
        for finished in [
            MicState::Heard("Hi.".to_string()),
            MicState::Empty,
            MicState::Failed("timed out".to_string()),
            MicState::PermissionDenied,
            MicState::Unsupported,
        ] {
            assert_eq!(
                mic_next(&finished, MicEvent::FieldChanged),
                MicState::Idle,
                "{finished:?}"
            );
        }
        assert_eq!(
            mic_next(&MicState::Idle, MicEvent::FieldChanged),
            MicState::Idle
        );
        // A run in flight is left alone: its answer is dropped by the epoch check.
        for busy in [MicState::Recording(2), MicState::Transcribing] {
            assert_eq!(mic_next(&busy, MicEvent::FieldChanged), busy);
        }
        assert!(result_is_current(3, 3));
        assert!(
            !result_is_current(3, 4),
            "an answer for older settings is stale"
        );
    }
}
