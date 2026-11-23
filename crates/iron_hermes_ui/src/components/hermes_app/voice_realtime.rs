//! Phase 54.7 Plan 08 (D-14) -- the Realtime voice family.
//!
//! `RealtimeForm` owns the five realtime form signals (provider, Base URL,
//! Model, Voice, Transcription model) that save into the top-level `realtime:`
//! config block; `RealtimeFamily` renders them inside
//! `div.voice-family[data-family=realtime]`. The group is ALWAYS mounted by
//! `VoiceSettings`: the non-selected service only toggles the `hidden`
//! attribute, so typed values survive flipping the service control (D-14).
//!
//! Every default and every curated list comes from `crate::voice_registry` or
//! the config snapshot (D-06); this file holds no voice default of its own. The
//! API key reference rides the shared `EndpointFields` row (D-02) and the
//! `Test connection` control closes the block (D-17).
//!
//! The family provides no context of its own (child providers panic ancestor
//! consumers at runtime).

// Web-only component tree: native dead-code analysis cannot reach the render
// sites (same rationale as voice_settings.rs). The wasm build lints normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::components::hermes_app::voice_fields::{EndpointFields, ListCache, OptionPickerField};
use crate::components::hermes_app::voice_settings::RealtimeInFlightCtx;
use crate::server::api::{VoiceConfigSnapshot, VoiceWritePayload};
use crate::server::voice_api::{test_realtime, RealtimeTestSettings, RealtimeTestTicket};
use crate::state::VoiceModeActiveContext;
use crate::voice_registry::{
    validate_base_url, validate_key_ref, validate_model_id, validate_transcription_model,
    validate_voice_id, FieldError, OptionKind, CATEGORY_BROWSER_UNREACHABLE, CATEGORY_KEY_REJECTED,
    CATEGORY_SERVER_UNREACHABLE, CATEGORY_TIMED_OUT, CATEGORY_UNEXPECTED_RESPONSE,
    ERROR_CATEGORIES, KEY_STATUS_NONE, REALTIME_DEFAULT_BASE_URL, REALTIME_PROVIDERS,
    REALTIME_TRANSCRIPTION_MODELS,
};

/// The Realtime family's form state: one `Signal<String>` per editable field,
/// plus the read-only key status the server reported for the saved reference.
#[derive(Clone, Copy, PartialEq)]
pub struct RealtimeForm {
    pub provider: Signal<String>,
    /// Empty means the registry's default address.
    pub base_url: Signal<String>,
    pub model: Signal<String>,
    pub voice: Signal<String>,
    pub transcription_model: Signal<String>,
    /// Name of an environment variable or vault entry holding the key (D-02).
    /// Never the key itself; empty means no key.
    pub key_ref: Signal<String>,
    /// What the server resolved the SAVED reference to (`found`, `not_found`,
    /// `none`). Loaded with the snapshot, never edited, not re-evaluated per
    /// keystroke.
    pub key_status: Signal<String>,
    /// Fetched option lists (D-06). Created inside [`use_realtime_form`], which
    /// runs in `VoiceSettings`' scope, so it lives as long as the panel and no
    /// context is involved.
    pub list_cache: ListCache,
    /// The Test connection state (D-17). Form-owned so any field change can
    /// clear it; never saved.
    pub conn: Signal<ConnState>,
    /// Attempt counter: bumped on every Start and every field change, so an
    /// answer that arrives for an older attempt is dropped (stale result).
    pub run: Signal<u64>,
}

/// Whether the Model picker may ask the server for a list: only a valid,
/// non-default Base URL has a server of its own to list (D-11); the
/// api.openai.com default keeps the curated registry list.
pub fn realtime_models_fetchable(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    if trimmed.is_empty() || validate_base_url(trimmed, true).is_err() {
        return false;
    }
    trimmed.trim_end_matches('/') != REALTIME_DEFAULT_BASE_URL
}

/// Create the family's signals. Call once, unconditionally, from `VoiceSettings`
/// (hook order); defaults come from the registry.
pub fn use_realtime_form() -> RealtimeForm {
    let spec = &REALTIME_PROVIDERS[0];
    RealtimeForm {
        provider: use_signal(|| spec.id.to_string()),
        base_url: use_signal(String::new),
        model: use_signal(|| spec.default_model.to_string()),
        voice: use_signal(|| spec.default_voice.to_string()),
        transcription_model: use_signal(|| REALTIME_TRANSCRIPTION_MODELS[0].to_string()),
        key_ref: use_signal(String::new),
        key_status: use_signal(|| KEY_STATUS_NONE.to_string()),
        list_cache: use_signal(HashMap::new),
        conn: use_signal(|| ConnState::Idle),
        run: use_signal(|| 0u64),
    }
}

/// The values a form shows for `snap`: snapshot values, with the registry
/// default standing in for an empty field (an older server may omit them).
/// Seeding and the dirty check share this so they cannot disagree.
fn effective(snap: &VoiceConfigSnapshot) -> [String; 6] {
    let spec = &REALTIME_PROVIDERS[0];
    let or_default = |value: &str, default: &str| {
        if value.is_empty() {
            default.to_string()
        } else {
            value.to_string()
        }
    };
    [
        or_default(&snap.realtime_provider, spec.id),
        // Empty is a real value here: it means the default address.
        snap.realtime_base_url.clone(),
        or_default(&snap.realtime_model, spec.default_model),
        or_default(&snap.realtime_voice, spec.default_voice),
        or_default(
            &snap.realtime_transcription_model,
            REALTIME_TRANSCRIPTION_MODELS[0],
        ),
        // Empty is a real value here too: it means no key.
        snap.realtime_key_ref.clone(),
    ]
}

impl RealtimeForm {
    /// Load every field from `snap` (one-shot load latch only).
    pub fn seed(&self, snap: &VoiceConfigSnapshot) {
        let [provider, base_url, model, voice, transcription, key_ref] = effective(snap);
        let (mut a, mut b, mut c, mut d, mut e, mut f) = (
            self.provider,
            self.base_url,
            self.model,
            self.voice,
            self.transcription_model,
            self.key_ref,
        );
        a.set(provider);
        b.set(base_url);
        c.set(model);
        d.set(voice);
        e.set(transcription);
        f.set(key_ref);
        // An older server omits the status: that reads as no key.
        let mut status = self.key_status;
        status.set(if snap.realtime_key_status.is_empty() {
            KEY_STATUS_NONE.to_string()
        } else {
            snap.realtime_key_status.clone()
        });
    }

    /// Write this form's fields into `p` (save path; `.peek()`). Values are
    /// trimmed: the server validates the trimmed form. The key reference is a
    /// reference only; the payload has no field that could hold a secret.
    pub fn apply_to(&self, p: &mut VoiceWritePayload) {
        p.realtime_provider = Some(self.provider.peek().trim().to_string());
        p.realtime_base_url = Some(self.base_url.peek().trim().to_string());
        p.realtime_model = Some(self.model.peek().trim().to_string());
        p.realtime_voice = Some(self.voice.peek().trim().to_string());
        p.realtime_transcription_model = Some(self.transcription_model.peek().trim().to_string());
        p.realtime_key_ref = Some(self.key_ref.peek().trim().to_string());
    }

    /// True when any field differs from `loaded` (render path: `.read()`).
    pub fn is_dirty(&self, loaded: &VoiceConfigSnapshot) -> bool {
        let [provider, base_url, model, voice, transcription, key_ref] = effective(loaded);
        *self.provider.read() != provider
            || *self.base_url.read() != base_url
            || *self.model.read() != model
            || *self.voice.read() != voice
            || *self.transcription_model.read() != transcription
            || *self.key_ref.read() != key_ref
    }

    /// Fold the just-saved values into `loaded` so the form reads clean again.
    pub fn commit_to(&self, loaded: &mut VoiceConfigSnapshot) {
        loaded.realtime_provider = self.provider.peek().trim().to_string();
        loaded.realtime_base_url = self.base_url.peek().trim().to_string();
        loaded.realtime_model = self.model.peek().trim().to_string();
        loaded.realtime_voice = self.voice.peek().trim().to_string();
        loaded.realtime_transcription_model = self.transcription_model.peek().trim().to_string();
        loaded.realtime_key_ref = self.key_ref.peek().trim().to_string();
    }

    /// The unsaved values Test connection runs with (D-17). Blank fields are
    /// `None`: the server then uses the default address / the saved value.
    /// Render path: `.read()` so the caller re-renders when a field changes.
    pub fn realtime_test_settings(&self) -> RealtimeTestSettings {
        let non_empty = |sig: &Signal<String>| {
            let value = sig.read().trim().to_string();
            (!value.is_empty()).then_some(value)
        };
        RealtimeTestSettings {
            provider: non_empty(&self.provider),
            base_url: non_empty(&self.base_url),
            key_ref: non_empty(&self.key_ref),
            model: non_empty(&self.model),
            voice: non_empty(&self.voice),
            transcription_model: non_empty(&self.transcription_model),
        }
    }

    /// The Base URL rule: only an INVALID value blocks, never an empty one
    /// (empty means the default address).
    pub fn base_url_error(&self) -> Option<FieldError> {
        validate_base_url(self.base_url.read().trim(), false).err()
    }

    fn model_error(&self) -> Option<FieldError> {
        validate_model_id(self.model.read().trim()).err()
    }

    fn voice_error(&self) -> Option<FieldError> {
        validate_voice_id(self.voice.read().trim()).err()
    }

    fn transcription_error(&self) -> Option<FieldError> {
        validate_transcription_model(self.transcription_model.read().trim()).err()
    }

    /// The reference must be a namespaced name (`VOICE_KEY_*` or a vault entry
    /// under `secret/voice/`); a pasted secret or a foreign variable such as
    /// `OPENAI_API_KEY` blocks saving.
    fn key_ref_error(&self) -> Option<FieldError> {
        validate_key_ref(self.key_ref.read().trim()).err()
    }

    /// Fields that currently block saving (each fails the server's rule too).
    pub fn blocking_error_count(&self) -> usize {
        [
            self.base_url_error(),
            self.model_error(),
            self.voice_error(),
            self.transcription_error(),
            self.key_ref_error(),
        ]
        .iter()
        .filter(|e| e.is_some())
        .count()
    }
}

// ── Test connection (D-12, D-17) ────────────────────────────────────────────

/// Where a Test connection run stands. Only fixed copy and a category from the
/// registry vocabulary are ever shown (T-54.7-62).
#[derive(Clone, Debug, PartialEq)]
pub enum ConnState {
    Idle,
    Connecting,
    Connected,
    Failed(String),
}

/// What can happen to a [`ConnState`]. Always written `ConnEvent::Ok` -- never
/// glob-imported, because `Ok` would shadow `Result::Ok`.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnEvent {
    Start,
    Ok,
    Fail(String),
    FieldChanged,
}

/// The state machine. One attempt at a time: `Start` while `Connecting` is
/// ignored. A field change clears whatever is showing, so an old result never
/// describes new values. A completion with no attempt running changes nothing.
pub fn conn_next(state: ConnState, event: ConnEvent) -> ConnState {
    match (state, event) {
        (_, ConnEvent::FieldChanged) => ConnState::Idle,
        (_, ConnEvent::Start) => ConnState::Connecting,
        (ConnState::Connecting, ConnEvent::Ok) => ConnState::Connected,
        (ConnState::Connecting, ConnEvent::Fail(category)) => ConnState::Failed(category),
        (state, ConnEvent::Ok | ConnEvent::Fail(_)) => state,
    }
}

/// Why the browser-side probe failed, before it is reduced to a category.
#[derive(Clone, Debug, PartialEq)]
pub enum ProbeFailure {
    /// `fetch` rejected with a TypeError: CORS, mixed content or unreachable.
    FetchTypeError,
    /// The server answered the SDP offer with this non-2xx status.
    HttpStatus(u16),
    /// The 10 s budget ran out.
    Timeout,
    /// The offer/answer exchange itself failed.
    Sdp,
}

/// The fixed category shown for a probe failure.
pub fn probe_category(failure: &ProbeFailure) -> &'static str {
    match failure {
        ProbeFailure::FetchTypeError => CATEGORY_BROWSER_UNREACHABLE,
        ProbeFailure::HttpStatus(401 | 403) => CATEGORY_KEY_REJECTED,
        ProbeFailure::HttpStatus(_) | ProbeFailure::Sdp => CATEGORY_UNEXPECTED_RESPONSE,
        ProbeFailure::Timeout => CATEGORY_TIMED_OUT,
    }
}

/// The category the server put on a ticket, if any. A category outside the
/// fixed vocabulary reads as `unexpected response`, so server text can never
/// reach the DOM.
pub fn ticket_category(ticket: &RealtimeTestTicket) -> Option<String> {
    let category = ticket.category.as_deref()?;
    Some(if ERROR_CATEGORIES.contains(&category) {
        category.to_string()
    } else {
        CATEGORY_UNEXPECTED_RESPONSE.to_string()
    })
}

/// The result line for a state: fixed copy plus a category only.
pub fn connection_result_copy(state: &ConnState) -> String {
    match state {
        ConnState::Idle => String::new(),
        ConnState::Connecting => "Connecting…".to_string(),
        ConnState::Connected => "Connected. A session opened and closed.".to_string(),
        ConnState::Failed(category) => format!("Couldn't connect: {category}."),
    }
}

/// Why a Test button cannot run while voice mode owns the microphone.
const VOICE_RUNNING_REASON: &str = "Can't test while voice mode is running.";

/// The probe's whole budget.
#[cfg(target_arch = "wasm32")]
const PROBE_TIMEOUT_MS: u32 = 10_000;

/// Mint a ticket from the unsaved values, then let the browser prove it can
/// open (and close) a session. Returns the category on failure. Never saves.
async fn run_connection_test(settings: RealtimeTestSettings) -> Result<(), String> {
    let ticket = match test_realtime(Some(settings)).await {
        Ok(ticket) => ticket,
        Err(_) => return Err(CATEGORY_SERVER_UNREACHABLE.to_string()),
    };
    if let Some(category) = ticket_category(&ticket) {
        return Err(category);
    }
    probe_realtime_calls(&ticket)
        .await
        .map_err(|failure| probe_category(&failure).to_string())
}

/// The browser probe is wasm-only (WebRTC); the server build never runs it.
#[cfg(not(target_arch = "wasm32"))]
async fn probe_realtime_calls(_ticket: &RealtimeTestTicket) -> Result<(), ProbeFailure> {
    Err(ProbeFailure::FetchTypeError)
}

/// Open a receive-only WebRTC session against the ticket's calls URL, apply
/// the answer, and close it. No microphone is involved (D-12): the offer asks
/// only to receive audio, plus a data channel the server expects. The peer
/// connection is closed on every path, success, failure or timeout (T-54.7-61).
#[cfg(target_arch = "wasm32")]
async fn probe_realtime_calls(ticket: &RealtimeTestTicket) -> Result<(), ProbeFailure> {
    use std::future::Future;
    use std::task::Poll;

    let calls_url = ticket.calls_url.clone().ok_or(ProbeFailure::Sdp)?;
    let config = web_sys::RtcConfiguration::new();
    let pc = web_sys::RtcPeerConnection::new_with_configuration(&config)
        .map_err(|_| ProbeFailure::Sdp)?;

    let mut attempt = Box::pin(probe_exchange(
        pc.clone(),
        calls_url,
        ticket.token.clone(),
        ticket.session_json.clone(),
    ));
    let mut deadline = Box::pin(gloo_timers::future::TimeoutFuture::new(PROBE_TIMEOUT_MS));
    let outcome = std::future::poll_fn(|cx| {
        if let Poll::Ready(result) = attempt.as_mut().poll(cx) {
            return Poll::Ready(result);
        }
        if deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(ProbeFailure::Timeout));
        }
        Poll::Pending
    })
    .await;

    pc.close();
    outcome
}

/// The offer/answer exchange of [`probe_realtime_calls`]. The request shape
/// follows what the mint returned (`calls_request_kind`), exactly as the real
/// session does.
#[cfg(target_arch = "wasm32")]
async fn probe_exchange(
    pc: web_sys::RtcPeerConnection,
    calls_url: String,
    token: Option<String>,
    session_json: Option<String>,
) -> Result<(), ProbeFailure> {
    use crate::components::hermes_app::realtime_session::{calls_request_kind, CallsRequestKind};
    use wasm_bindgen::JsCast;
    use wasm_bindgen_futures::JsFuture;

    let transceiver = web_sys::RtcRtpTransceiverInit::new();
    transceiver.set_direction(web_sys::RtcRtpTransceiverDirection::Recvonly);
    pc.add_transceiver_with_str_and_init("audio", &transceiver);
    pc.create_data_channel("oai-events");

    let offer = JsFuture::from(pc.create_offer())
        .await
        .map_err(|_| ProbeFailure::Sdp)?;
    // create_offer resolves to a plain dictionary object, so a checked cast
    // (instanceof) always fails; reinterpret it like the real session does.
    let offer_desc: web_sys::RtcSessionDescriptionInit = offer.clone().unchecked_into();
    JsFuture::from(pc.set_local_description(&offer_desc))
        .await
        .map_err(|_| ProbeFailure::Sdp)?;
    let sdp = js_sys::Reflect::get(&offer, &wasm_bindgen::JsValue::from_str("sdp"))
        .ok()
        .and_then(|v| v.as_string())
        .ok_or(ProbeFailure::Sdp)?;

    let init = web_sys::RequestInit::new();
    init.set_method("POST");
    match calls_request_kind(&token, &session_json) {
        CallsRequestKind::MultipartSdpSession => {
            let form = web_sys::FormData::new().map_err(|_| ProbeFailure::Sdp)?;
            form.append_with_str("sdp", &sdp)
                .map_err(|_| ProbeFailure::Sdp)?;
            form.append_with_str("session", session_json.as_deref().unwrap_or("{}"))
                .map_err(|_| ProbeFailure::Sdp)?;
            init.set_body(form.as_ref());
        }
        kind => {
            let headers = web_sys::Headers::new().map_err(|_| ProbeFailure::Sdp)?;
            if kind == CallsRequestKind::BearerSdp {
                if let Some(token) = token.as_deref() {
                    headers
                        .set("Authorization", &format!("Bearer {token}"))
                        .map_err(|_| ProbeFailure::Sdp)?;
                }
            }
            headers
                .set("Content-Type", "application/sdp")
                .map_err(|_| ProbeFailure::Sdp)?;
            init.set_headers(&headers);
            init.set_body(&wasm_bindgen::JsValue::from_str(&sdp));
        }
    }
    let request = web_sys::Request::new_with_str_and_init(&calls_url, &init)
        .map_err(|_| ProbeFailure::Sdp)?;

    let window = web_sys::window().ok_or(ProbeFailure::Sdp)?;
    // A rejected fetch promise is a TypeError: CORS, mixed content or no route.
    let response: web_sys::Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| ProbeFailure::FetchTypeError)?
        .dyn_into()
        .map_err(|_| ProbeFailure::Sdp)?;
    if !response.ok() {
        return Err(ProbeFailure::HttpStatus(response.status()));
    }
    let answer_sdp = JsFuture::from(response.text().map_err(|_| ProbeFailure::Sdp)?)
        .await
        .map_err(|_| ProbeFailure::Sdp)?
        .as_string()
        .ok_or(ProbeFailure::Sdp)?;

    let answer = web_sys::RtcSessionDescriptionInit::new(web_sys::RtcSdpType::Answer);
    answer.set_sdp(&answer_sdp);
    JsFuture::from(pc.set_remote_description(&answer))
        .await
        .map_err(|_| ProbeFailure::Sdp)?;
    Ok(())
}

/// The `Test connection` control and its result line (D-17). Uses the current
/// unsaved values, never writes config, runs one attempt at a time.
///
/// `disabled_reason` is why the settings cannot be tested (an invalid Base
/// URL); voice mode running adds its own reason. A disabled button always
/// shows its reason.
#[component]
pub fn TestConnectionButton(
    settings: RealtimeTestSettings,
    disabled_reason: Option<String>,
    conn: Signal<ConnState>,
    run: Signal<u64>,
) -> Element {
    // Absent outside the full app (panel tests); then nothing is live.
    let voice_mode_running = try_use_context::<VoiceModeActiveContext>()
        .map(|ctx| *ctx.0.read())
        .unwrap_or(false);
    let session_live = try_use_context::<RealtimeInFlightCtx>()
        .map(|ctx| *ctx.0.read())
        .unwrap_or(false);

    let state = conn.read().clone();
    let connecting = state == ConnState::Connecting;
    let blocked_reason = if voice_mode_running || session_live {
        Some(VOICE_RUNNING_REASON.to_string())
    } else {
        disabled_reason
    };
    let disabled = blocked_reason.is_some() || connecting;
    let label = if connecting {
        "Connecting…"
    } else {
        "Test connection"
    };
    let (line, tone) = match (&blocked_reason, &state) {
        (Some(reason), _) => (reason.clone(), ""),
        (None, ConnState::Connected) => (connection_result_copy(&state), " is-ok"),
        (None, ConnState::Failed(_)) => (connection_result_copy(&state), " is-error"),
        (None, _) => (connection_result_copy(&state), ""),
    };

    rsx! {
        div { class: "voice-field",
            button {
                r#type: "button",
                class: "voice-test-btn",
                disabled,
                onclick: move |_| {
                    let mut conn = conn;
                    let mut run = run;
                    let current = conn.peek().clone();
                    if current == ConnState::Connecting {
                        return;
                    }
                    conn.set(conn_next(current, ConnEvent::Start));
                    let mine = run.peek().wrapping_add(1);
                    run.set(mine);
                    let settings = settings.clone();
                    spawn(async move {
                        let outcome = run_connection_test(settings).await;
                        // A field changed (or a newer attempt began) meanwhile:
                        // this answer describes values no longer on screen.
                        if *run.peek() != mine {
                            return;
                        }
                        let event = match outcome {
                            Ok(()) => ConnEvent::Ok,
                            Err(category) => ConnEvent::Fail(category),
                        };
                        let current = conn.peek().clone();
                        conn.set(conn_next(current, event));
                    });
                },
                "{label}"
            }
            p {
                class: "voice-test-result{tone}",
                role: "status",
                "aria-live": "polite",
                "{line}"
            }
        }
    }
}

/// The Realtime family group.
///
/// `hidden` toggles the `hidden` attribute only -- the group is never
/// conditionally mounted.
#[component]
pub fn RealtimeFamily(
    form: RealtimeForm,
    hidden: bool,
    disabled: bool,
    #[props(default)] on_edit: Option<EventHandler<()>>,
) -> Element {
    let spec = &REALTIME_PROVIDERS[0];
    let mut provider = form.provider;
    let base_url = form.base_url;

    // Pattern B: owned locals; every borrow ends at its `;`.
    let provider_val = provider.read().clone();
    let base_url_val = base_url.read().clone();
    let key_status_val = form.key_status.read().clone();
    // WR-03: the typed key reference rides the Model list request. Without it a
    // key-protected custom server is asked unauthenticated and answers 401.
    let key_ref_val = form.key_ref.read().trim().to_string();
    // The keyless hint is about the EFFECTIVE key: shown only when none
    // resolves (a default-address server whose provider key is set has one).
    let no_key_hint = (key_status_val == KEY_STATUS_NONE)
        .then(|| "With no key, the browser connects without a token.".to_string());
    let model_error = form.model_error();
    let voice_error = form.voice_error();
    let transcription_error = form.transcription_error();

    let models: Vec<String> = spec.curated_models.iter().map(|m| m.to_string()).collect();
    let voices: Vec<String> = spec.curated_voices.iter().map(|v| v.to_string()).collect();
    let transcription_models: Vec<String> = REALTIME_TRANSCRIPTION_MODELS
        .iter()
        .map(|m| m.to_string())
        .collect();

    // Test connection: the unsaved values, and why it cannot run (an invalid
    // Base URL). A result never outlives the values it was run with: any
    // change to the settings clears it, and bumps the attempt counter so an
    // answer still in flight is dropped.
    let test_settings = form.realtime_test_settings();
    let base_url_reason = form
        .base_url_error()
        .or_else(|| form.key_ref_error())
        .map(|e| e.copy().to_string());
    use_effect(use_reactive!(|test_settings| {
        let _ = &test_settings;
        let mut conn = form.conn;
        let mut run = form.run;
        let current = conn.peek().clone();
        if current != ConnState::Idle {
            conn.set(conn_next(current, ConnEvent::FieldChanged));
            let next = run.peek().wrapping_add(1);
            run.set(next);
        }
    }));

    let base_edit = on_edit;
    let notify = move || {
        if let Some(handler) = on_edit.as_ref() {
            handler.call(());
        }
    };

    rsx! {
        div {
            class: "voice-family",
            "data-family": "realtime",
            hidden: if hidden { "true" },

            p {
                class: "voice-settings-section-header",
                role: "heading",
                "aria-level": "3",
                "REALTIME"
            }
            hr { class: "voice-settings-separator" }

            // Provider (one registry option)
            div { class: "voice-field",
                div { class: "voice-field-head",
                    label {
                        r#for: "realtime-provider-select",
                        class: "voice-settings-label",
                        "Provider"
                    }
                }
                select {
                    id: "realtime-provider-select",
                    class: "voice-settings-select",
                    disabled,
                    value: "{provider_val}",
                    onchange: move |evt| {
                        notify();
                        provider.set(evt.value());
                    },
                    for p in REALTIME_PROVIDERS.iter() {
                        option { key: "{p.id}", value: "{p.id}", "{p.label}" }
                    }
                }
            }

            // Base URL (empty means the default address, so it is optional)
            // and the API key reference with its status pill (D-02, D-12).
            EndpointFields {
                base_url: form.base_url,
                key_ref: Some(form.key_ref),
                key_status: key_status_val,
                no_key_hint,
                default_path: String::new(),
                base_placeholder: REALTIME_DEFAULT_BASE_URL.to_string(),
                base_hint: Some(
                    "Your browser connects to this address directly, so it must be reachable from this device."
                        .to_string(),
                ),
                required: false,
                disabled,
                id_prefix: "realtime".to_string(),
                on_edit: base_edit,
            }

            // No address yet: say what a Base URL unlocks. The pickers below
            // still work from the curated lists.
            if base_url_val.trim().is_empty() {
                div { class: "voice-field",
                    p {
                        class: "voice-settings-label",
                        role: "heading",
                        "aria-level": "4",
                        "Add your server's address"
                    }
                    p { class: "voice-settings-hint voice-field-hint",
                        "Enter a Base URL above to load this server's models and voices, or type a model and voice by hand."
                    }
                }
            }

            // Model: listed from the operator's own server for a custom Base
            // URL, the curated registry list otherwise (D-11).
            OptionPickerField {
                label: "Model".to_string(),
                field_id: "realtime-model".to_string(),
                kind: OptionKind::Models,
                family: "realtime".to_string(),
                provider: provider_val.clone(),
                provider_label: spec.label.to_string(),
                value: form.model,
                curated: models,
                base_url: base_url_val.clone(),
                path: String::new(),
                key_ref: key_ref_val.clone(),
                model_for_voices: String::new(),
                fetchable: realtime_models_fetchable(&base_url_val),
                visible: !hidden,
                disabled,
                cache: form.list_cache,
                seeded: spec.default_model.to_string(),
                placeholder: "Pick or type a model".to_string(),
                title: "Realtime model".to_string(),
                error: model_error.map(|e| e.copy().to_string()),
            }

            // Voice: curated registry list only (D-11); the realtime API has
            // no voice-listing endpoint.
            OptionPickerField {
                label: "Voice".to_string(),
                field_id: "realtime-voice".to_string(),
                kind: OptionKind::Voices,
                family: "realtime".to_string(),
                provider: provider_val.clone(),
                provider_label: spec.label.to_string(),
                value: form.voice,
                curated: voices,
                base_url: base_url_val.clone(),
                path: String::new(),
                key_ref: String::new(),
                model_for_voices: String::new(),
                fetchable: false,
                visible: !hidden,
                disabled,
                cache: form.list_cache,
                seeded: spec.default_voice.to_string(),
                placeholder: "Pick or type a voice".to_string(),
                title: "Realtime voice".to_string(),
                noun: "voice".to_string(),
                error: voice_error.map(|e| e.copy().to_string()),
            }

            // Transcription model: curated registry list only (D-11).
            OptionPickerField {
                label: "Transcription model".to_string(),
                field_id: "realtime-transcription".to_string(),
                kind: OptionKind::Models,
                family: "realtime".to_string(),
                provider: provider_val.clone(),
                provider_label: spec.label.to_string(),
                value: form.transcription_model,
                curated: transcription_models,
                base_url: base_url_val.clone(),
                path: String::new(),
                key_ref: String::new(),
                model_for_voices: String::new(),
                fetchable: false,
                visible: !hidden,
                disabled,
                cache: form.list_cache,
                seeded: REALTIME_TRANSCRIPTION_MODELS[0].to_string(),
                placeholder: "Pick or type a model".to_string(),
                title: "Realtime transcription model".to_string(),
                noun: "model id".to_string(),
                error: transcription_error.map(|e| e.copy().to_string()),
            }

            // Test connection: proves the browser can open a session to this
            // server with the values above, saved or not (D-17).
            TestConnectionButton {
                settings: test_settings,
                disabled_reason: base_url_reason,
                conn: form.conn,
                run: form.run,
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::server::api::{VoiceConfigSnapshot, VoiceWritePayload};
    use crate::voice_registry::{REALTIME_DEFAULT_BASE_URL, REALTIME_PROVIDERS};
    use std::cell::RefCell;

    fn snapshot() -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            realtime_provider: REALTIME_PROVIDERS[0].id.to_string(),
            realtime_base_url: "http://rt.example:9000/v1".to_string(),
            realtime_model: "model-a".to_string(),
            realtime_voice: "voice-a".to_string(),
            realtime_transcription_model: "trans-a".to_string(),
            realtime_key_ref: "VOICE_KEY_IH_TEST_547_RT".to_string(),
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct Observed {
        payload: VoiceWritePayload,
        clean_after_seed: bool,
        /// One entry per field: dirty after editing only that field.
        dirty_per_field: Vec<(&'static str, bool)>,
        clean_after_revert: bool,
        blocking_both_bad: usize,
        blocking_empty_base_url: usize,
        blocking_valid: usize,
    }

    thread_local! {
        static SNAP: RefCell<VoiceConfigSnapshot> = RefCell::new(VoiceConfigSnapshot::default());
        static OUT: RefCell<Option<Observed>> = const { RefCell::new(None) };
    }

    fn observe(snap: VoiceConfigSnapshot) -> Observed {
        SNAP.with(|s| *s.borrow_mut() = snap);
        OUT.with(|o| *o.borrow_mut() = None);

        fn app() -> Element {
            let snap = SNAP.with(|s| s.borrow().clone());
            let form = use_realtime_form();
            form.seed(&snap);
            let mut obs = Observed {
                clean_after_seed: !form.is_dirty(&snap),
                ..Default::default()
            };
            form.apply_to(&mut obs.payload);

            let fields: [(&'static str, Signal<String>); 6] = [
                ("provider", form.provider),
                ("base_url", form.base_url),
                ("model", form.model),
                ("voice", form.voice),
                ("transcription_model", form.transcription_model),
                ("key_ref", form.key_ref),
            ];
            for (name, sig) in fields {
                let mut sig = sig;
                let original = sig.peek().clone();
                sig.set(format!("{original}-edited"));
                obs.dirty_per_field.push((name, form.is_dirty(&snap)));
                sig.set(original);
            }
            obs.clean_after_revert = !form.is_dirty(&snap);

            let mut base_url = form.base_url;
            let mut model = form.model;
            base_url.set("ftp://x".to_string());
            model.set("a\"b".to_string());
            obs.blocking_both_bad = form.blocking_error_count();
            base_url.set(String::new());
            model.set("model-a".to_string());
            obs.blocking_empty_base_url = form.blocking_error_count();
            base_url.set("https://rt.example/v1".to_string());
            obs.blocking_valid = form.blocking_error_count();

            OUT.with(|o| *o.borrow_mut() = Some(obs));
            rsx! {}
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        OUT.with(|o| o.borrow_mut().take())
            .expect("the harness ran")
    }

    #[test]
    fn realtime_form_seed_apply_and_dirty() {
        let obs = observe(snapshot());
        let p = &obs.payload;
        assert_eq!(
            p.realtime_provider.as_deref(),
            Some(REALTIME_PROVIDERS[0].id)
        );
        assert_eq!(
            p.realtime_base_url.as_deref(),
            Some("http://rt.example:9000/v1")
        );
        assert_eq!(p.realtime_model.as_deref(), Some("model-a"));
        assert_eq!(p.realtime_voice.as_deref(), Some("voice-a"));
        assert_eq!(p.realtime_transcription_model.as_deref(), Some("trans-a"));
        assert_eq!(
            p.realtime_key_ref.as_deref(),
            Some("VOICE_KEY_IH_TEST_547_RT")
        );
        assert_eq!(p.voice_service, None);
        assert!(obs.clean_after_seed, "a freshly seeded form is not dirty");
        assert_eq!(obs.dirty_per_field.len(), 6);
        for (name, dirty) in &obs.dirty_per_field {
            assert!(*dirty, "editing {name} alone must mark the form dirty");
        }
        assert!(
            obs.clean_after_revert,
            "reverting every edit is clean again"
        );
    }

    #[test]
    fn realtime_form_blocking_errors_count() {
        let obs = observe(snapshot());
        assert_eq!(obs.blocking_both_bad, 2, "bad base url + bad model id");
        assert_eq!(
            obs.blocking_empty_base_url, 0,
            "an empty base url means the default and never blocks"
        );
        assert_eq!(obs.blocking_valid, 0);
    }

    #[test]
    fn realtime_form_seeds_registry_defaults_for_an_empty_snapshot() {
        // An older server (empty snapshot fields) still yields a usable form.
        let obs = observe(VoiceConfigSnapshot::default());
        let p = &obs.payload;
        assert_eq!(
            p.realtime_provider.as_deref(),
            Some(REALTIME_PROVIDERS[0].id)
        );
        assert_eq!(
            p.realtime_model.as_deref(),
            Some(REALTIME_PROVIDERS[0].default_model)
        );
        assert_eq!(
            p.realtime_voice.as_deref(),
            Some(REALTIME_PROVIDERS[0].default_voice)
        );
        assert!(!p.realtime_model.as_deref().unwrap_or("").is_empty());
        // The default address is not hard-coded into the form: empty means default.
        assert_eq!(p.realtime_base_url.as_deref(), Some(""));
        assert!(REALTIME_DEFAULT_BASE_URL.starts_with("https://"));
    }

    fn render_family() -> String {
        fn app() -> Element {
            let form = use_realtime_form();
            rsx! { RealtimeFamily { form, hidden: false, disabled: false } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn realtime_family_renders_fields_from_registry() {
        let html = render_family();
        assert!(html.contains("data-family=\"realtime\""), "{html}");
        // Provider: one registry option, labelled from the registry.
        assert!(html.contains(REALTIME_PROVIDERS[0].label), "{html}");
        assert!(html.contains("OpenAI Realtime-compatible"), "{html}");
        // Base URL placeholder and the always-shown reachability hint.
        assert!(
            html.contains("placeholder=\"https://api.openai.com/v1\""),
            "{html}"
        );
        assert!(
            html.contains(
                "Your browser connects to this address directly, so it must be reachable from this device."
            ),
            "{html}"
        );
        // Model / Voice / Transcription model are comboboxes seeded from the registry.
        assert_eq!(html.matches("role=\"combobox\"").count(), 3, "{html}");
        assert!(
            html.contains(&format!(
                "value=\"{}\"",
                REALTIME_PROVIDERS[0].default_model
            )),
            "model combobox seeded from the registry: {html}"
        );
        assert!(
            html.contains(&format!(
                "value=\"{}\"",
                REALTIME_PROVIDERS[0].default_voice
            )),
            "voice combobox seeded from the registry: {html}"
        );
        for label in [
            "Provider",
            "Base URL",
            "Model",
            "Voice",
            "Transcription model",
        ] {
            assert!(html.contains(label), "label {label} missing: {html}");
        }
    }

    // ── Plan 18 Task 1: the custom-server model list ────────────────────────

    use crate::components::hermes_app::voice_fields::{
        displayed_options, list_result_to_status, ListCache, ListCacheKey, OptionPickerField,
    };
    use crate::server::voice_api::{list_voice_options_impl, VoiceListRequest, VoiceListResult};
    use crate::voice_registry::{ListStatus, OptionKind};
    use ironhermes_core::config::Config;
    use std::collections::HashMap;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn realtime_model_fetch_only_for_custom_base() {
        assert!(!realtime_models_fetchable(""));
        assert!(!realtime_models_fetchable("https://api.openai.com/v1"));
        assert!(!realtime_models_fetchable("https://api.openai.com/v1/"));
        assert!(realtime_models_fetchable("http://rt.lan:9000/v1"));
        assert!(!realtime_models_fetchable("ftp://x"));
        assert!(!realtime_models_fetchable("not a url"));
    }

    thread_local! {
        static SERVED: RefCell<Option<(String, ListStatus, Vec<String>)>> =
            const { RefCell::new(None) };
    }

    fn render_picker_with_cache_entry() -> String {
        fn app() -> Element {
            let (base, status, items) = SERVED.with(|s| s.borrow().clone().expect("seeded"));
            let spec = &REALTIME_PROVIDERS[0];
            let key = ListCacheKey::new("realtime", spec.id, &base, "", "", OptionKind::Models);
            let cache: ListCache = use_signal(move || HashMap::from([(key, (status, items))]));
            let value = use_signal(|| spec.default_model.to_string());
            rsx! {
                OptionPickerField {
                    label: "Model".to_string(),
                    field_id: "realtime-model".to_string(),
                    kind: OptionKind::Models,
                    family: "realtime".to_string(),
                    provider: spec.id.to_string(),
                    provider_label: spec.label.to_string(),
                    value,
                    curated: spec.curated_models.iter().map(|m| m.to_string()).collect::<Vec<_>>(),
                    base_url: base,
                    path: String::new(),
                    key_ref: String::new(),
                    model_for_voices: String::new(),
                    fetchable: true,
                    visible: true,
                    disabled: false,
                    cache,
                    seeded: spec.default_model.to_string(),
                    placeholder: "Pick or type a model".to_string(),
                    title: "Realtime model".to_string(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn custom_realtime_base_lists_server_models_end_to_end() {
        let _guard = crate::server::test_support::env_lock();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let (base, result): (String, VoiceListResult) = runtime.block_on(async {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "object": "list",
                    "data": [{"id": "rt-model-a"}]
                })))
                .mount(&server)
                .await;
            let base = format!("{}/v1", server.uri());
            let req = VoiceListRequest {
                family: "realtime".into(),
                provider: REALTIME_PROVIDERS[0].id.into(),
                kind: "models".into(),
                base_url: Some(base.clone()),
                ..Default::default()
            };
            let res = list_voice_options_impl(&req, &Config::default(), true)
                .await
                .expect("list");
            (base, res)
        });
        assert!(realtime_models_fetchable(&base));
        let status = list_result_to_status(&result);
        assert_eq!(status, ListStatus::Loaded(1));

        // The picker lists the server's models, not the curated ones.
        let curated: Vec<String> = REALTIME_PROVIDERS[0]
            .curated_models
            .iter()
            .map(|m| m.to_string())
            .collect();
        assert_eq!(
            displayed_options(&status, &result.items, &curated),
            vec!["rt-model-a".to_string()]
        );

        SERVED.with(|s| *s.borrow_mut() = Some((base, status, result.items.clone())));
        let html = render_picker_with_cache_entry();
        assert!(html.contains("1 model loaded from your server."), "{html}");
        assert!(!html.contains("aria-busy=\"true\""), "{html}");
    }

    // ── Plan 18 Task 3: curated pickers, empty state, blocking values ────────

    /// SSR escapes quotes and apostrophes in text nodes; match any spelling.
    fn has_text(html: &str, text: &str) -> bool {
        ["'", "&#39;", "&#x27;"]
            .iter()
            .any(|a| html.contains(text.replace('\'', a).as_str()))
    }

    thread_local! {
        static BASE: RefCell<String> = const { RefCell::new(String::new()) };
    }

    fn render_family_with_base(base_url: &str) -> String {
        BASE.with(|b| *b.borrow_mut() = base_url.to_string());
        fn app() -> Element {
            let form = use_realtime_form();
            use_hook(|| {
                let mut base = form.base_url;
                base.set(BASE.with(|b| b.borrow().clone()));
            });
            rsx! { RealtimeFamily { form, hidden: false, disabled: false } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    const EMPTY_HEADING: &str = "Add your server's address";
    const EMPTY_BODY: &str =
        "Enter a Base URL above to load this server's models and voices, or type a model and voice by hand.";

    #[test]
    fn realtime_pickers_use_curated_on_default() {
        let html = render_family_with_base(REALTIME_DEFAULT_BASE_URL);
        let label = REALTIME_PROVIDERS[0].label;
        // Model and Transcription model are model pickers, Voice a voice picker;
        // none fetches on the api.openai.com default.
        assert!(
            has_text(
                &html,
                &format!("Built-in models for {label}. Type any value to use it.")
            ),
            "{html}"
        );
        assert_eq!(
            html.matches("Built-in models for ").count(),
            2,
            "Model and Transcription model: {html}"
        );
        assert_eq!(
            html.matches(&format!(
                "Built-in voices for {label}. Type any value to use it."
            ))
            .count(),
            1,
            "Voice: {html}"
        );
        assert_eq!(html.matches("role=\"combobox\"").count(), 3, "{html}");
        // The default address needs no empty-state prompt.
        assert!(!has_text(&html, EMPTY_HEADING), "{html}");
        // Nothing to refresh: the curated lists have no server behind them.
        assert!(!html.contains("voice-refresh-btn"), "{html}");
    }

    #[test]
    fn realtime_empty_base_url_shows_empty_state() {
        let html = render_family_with_base("");
        assert!(has_text(&html, EMPTY_HEADING), "{html}");
        assert!(has_text(&html, EMPTY_BODY), "{html}");
        // The pickers fall back to the curated lists.
        assert!(html.contains("Built-in models for "), "{html}");
        // An empty address means the default and never blocks saving.
        let obs = observe(snapshot());
        assert_eq!(obs.blocking_empty_base_url, 0);
    }

    #[test]
    fn realtime_invalid_values_block_save() {
        fn app() -> Element {
            let form = use_realtime_form();
            let mut base = form.base_url;
            let mut voice = form.voice;
            base.set("ftp://x".to_string());
            let after_base = form.blocking_error_count();
            voice.set("a b".to_string());
            let after_voice = form.blocking_error_count();
            base.set(String::new());
            voice.set("shimmer".to_string());
            let after_clear = form.blocking_error_count();
            COUNTS.with(|c| *c.borrow_mut() = Some((after_base, after_voice, after_clear)));
            rsx! {}
        }
        thread_local! {
            static COUNTS: RefCell<Option<(usize, usize, usize)>> = const { RefCell::new(None) };
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let counts = COUNTS.with(|c| c.borrow_mut().take()).expect("ran");
        assert_eq!(counts, (1, 2, 0));
    }

    #[test]
    fn voice_realtime_has_no_hardcoded_voice_defaults() {
        let src = include_str!("voice_realtime.rs");
        for needle in crate::voice_registry::hardcoded_voice_default_needles() {
            assert!(
                !src.contains(needle.as_str()),
                "voice_realtime.rs still holds a hard-coded voice default: {needle}"
            );
        }
    }

    // ── Plan 14 Task 1: Test connection ─────────────────────────────────────

    use crate::server::voice_api::{RealtimeTestSettings, RealtimeTestTicket};

    thread_local! {
        static SETTINGS_OUT: RefCell<Option<(RealtimeTestSettings, RealtimeTestSettings)>> =
            const { RefCell::new(None) };
    }

    #[test]
    fn realtime_test_settings_from_form() {
        fn app() -> Element {
            let form = use_realtime_form();
            form.seed(&snapshot());
            let filled = form.realtime_test_settings();
            let (mut base, mut model, mut voice, mut transcription) = (
                form.base_url,
                form.model,
                form.voice,
                form.transcription_model,
            );
            base.set("   ".to_string());
            model.set(String::new());
            voice.set(String::new());
            transcription.set(String::new());
            let empty = form.realtime_test_settings();
            SETTINGS_OUT.with(|o| *o.borrow_mut() = Some((filled, empty)));
            rsx! {}
        }
        SETTINGS_OUT.with(|o| *o.borrow_mut() = None);
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let (filled, empty) = SETTINGS_OUT
            .with(|o| o.borrow_mut().take())
            .expect("the harness ran");
        assert_eq!(
            filled.provider.as_deref(),
            Some(REALTIME_PROVIDERS[0].id),
            "the provider rides along"
        );
        assert_eq!(
            filled.base_url.as_deref(),
            Some("http://rt.example:9000/v1")
        );
        assert_eq!(filled.model.as_deref(), Some("model-a"));
        assert_eq!(filled.voice.as_deref(), Some("voice-a"));
        assert_eq!(filled.transcription_model.as_deref(), Some("trans-a"));
        // Blank means "the default / keep the saved one" on the server (None).
        assert_eq!(empty.base_url, None);
        assert_eq!(empty.model, None);
        assert_eq!(empty.voice, None);
        assert_eq!(empty.transcription_model, None);
    }

    #[test]
    fn connection_state_machine() {
        use ConnEvent::*;
        use ConnState::*;
        assert_eq!(conn_next(Idle, Start), Connecting);
        assert_eq!(conn_next(Connecting, Ok), Connected);
        assert_eq!(
            conn_next(Connecting, Fail("key rejected".to_string())),
            Failed("key rejected".to_string())
        );
        // One attempt at a time: Start while Connecting is ignored.
        assert_eq!(conn_next(Connecting, Start), Connecting);
        // A finished test can be run again.
        assert_eq!(conn_next(Connected, Start), Connecting);
        assert_eq!(
            conn_next(Failed("timed out".to_string()), Start),
            Connecting
        );
        // Any field change clears whatever is on screen.
        for state in [Idle, Connecting, Connected, Failed("timed out".to_string())] {
            assert_eq!(conn_next(state, FieldChanged), Idle);
        }
        // A completion that arrives with no attempt running changes nothing.
        assert_eq!(conn_next(Idle, Ok), Idle);
        assert_eq!(conn_next(Idle, Fail("timed out".to_string())), Idle);
    }

    #[test]
    fn probe_error_category_mapping() {
        use crate::voice_registry::{
            CATEGORY_BROWSER_UNREACHABLE, CATEGORY_KEY_REJECTED, CATEGORY_TIMED_OUT,
            CATEGORY_UNEXPECTED_RESPONSE, ERROR_CATEGORIES,
        };
        assert_eq!(
            probe_category(&ProbeFailure::FetchTypeError),
            "this browser can't reach the address"
        );
        assert_eq!(
            probe_category(&ProbeFailure::FetchTypeError),
            CATEGORY_BROWSER_UNREACHABLE
        );
        assert_eq!(
            probe_category(&ProbeFailure::HttpStatus(401)),
            CATEGORY_KEY_REJECTED
        );
        assert_eq!(
            probe_category(&ProbeFailure::HttpStatus(403)),
            "key rejected"
        );
        assert_eq!(
            probe_category(&ProbeFailure::HttpStatus(500)),
            CATEGORY_UNEXPECTED_RESPONSE
        );
        assert_eq!(probe_category(&ProbeFailure::Timeout), CATEGORY_TIMED_OUT);
        assert_eq!(probe_category(&ProbeFailure::Sdp), "unexpected response");
        // Whatever the probe says is inside the fixed vocabulary.
        for f in [
            ProbeFailure::FetchTypeError,
            ProbeFailure::HttpStatus(404),
            ProbeFailure::Timeout,
            ProbeFailure::Sdp,
        ] {
            assert!(ERROR_CATEGORIES.contains(&probe_category(&f)));
        }
        // A ticket category passes through unchanged ...
        for category in ERROR_CATEGORIES {
            let ticket = RealtimeTestTicket {
                category: Some(category.to_string()),
                ..Default::default()
            };
            assert_eq!(ticket_category(&ticket).as_deref(), Some(category));
        }
        // ... a clean ticket has none ...
        assert_eq!(ticket_category(&RealtimeTestTicket::default()), None);
        // ... and text outside the vocabulary never reaches the screen.
        let rogue = RealtimeTestTicket {
            category: Some("<img src=x onerror=alert(1)>".to_string()),
            ..Default::default()
        };
        assert_eq!(
            ticket_category(&rogue).as_deref(),
            Some("unexpected response")
        );
    }

    #[test]
    fn connection_result_copy() {
        assert_eq!(
            super::connection_result_copy(&ConnState::Connected),
            "Connected. A session opened and closed."
        );
        assert_eq!(
            super::connection_result_copy(&ConnState::Failed("key rejected".to_string())),
            "Couldn't connect: key rejected."
        );
        assert_eq!(
            super::connection_result_copy(&ConnState::Connecting),
            "Connecting…"
        );
        assert_eq!(super::connection_result_copy(&ConnState::Idle), "");
    }

    thread_local! {
        static BTN: RefCell<(bool, Option<String>)> = const { RefCell::new((false, None)) };
    }

    fn render_button(voice_active: bool, reason: Option<&str>) -> String {
        BTN.with(|b| *b.borrow_mut() = (voice_active, reason.map(str::to_string)));
        fn app() -> Element {
            let (active, reason) = BTN.with(|b| b.borrow().clone());
            let sig = use_signal(move || active);
            use_context_provider(|| crate::state::VoiceModeActiveContext(sig));
            let form = use_realtime_form();
            rsx! {
                TestConnectionButton {
                    settings: RealtimeTestSettings::default(),
                    disabled_reason: reason,
                    conn: form.conn,
                    run: form.run,
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The attribute text of the first `<button ...>` tag.
    fn button_tag(html: &str) -> &str {
        let start = html.find("<button").expect("a button renders");
        let end = html[start..].find('>').expect("tag closes") + start;
        &html[start..end]
    }

    #[test]
    fn test_connection_button_states() {
        let idle = render_button(false, None);
        assert!(idle.contains("Test connection"), "{idle}");
        assert!(idle.contains("voice-test-btn"), "{idle}");
        assert!(!button_tag(&idle).contains("disabled"), "{idle}");
        assert!(
            !has_text(&idle, "Can't test while voice mode is running."),
            "{idle}"
        );
        assert!(idle.contains("voice-test-result"), "{idle}");
        assert!(idle.contains("role=\"status\""), "{idle}");
        assert!(idle.contains("aria-live=\"polite\""), "{idle}");

        let running = render_button(true, None);
        assert!(button_tag(&running).contains("disabled"), "{running}");
        assert!(
            has_text(&running, "Can't test while voice mode is running."),
            "{running}"
        );

        let invalid = render_button(false, Some("Enter a full address."));
        assert!(button_tag(&invalid).contains("disabled"), "{invalid}");
        assert!(has_text(&invalid, "Enter a full address."), "{invalid}");
        assert!(
            !has_text(&invalid, "Can't test while voice mode is running."),
            "{invalid}"
        );
    }

    #[test]
    fn invalid_base_url_disables_test_connection_in_the_family() {
        let html = render_family_with_base("ftp://x");
        assert!(
            button_tag_after(&html, "Test connection").contains("disabled"),
            "{html}"
        );
        let copy = crate::voice_registry::validate_base_url("ftp://x", false)
            .expect_err("ftp is not an allowed scheme")
            .copy();
        // Once under the Base URL field, once as the Test connection reason.
        assert_eq!(
            ["'", "&#39;", "&#x27;"]
                .iter()
                .map(|a| html.matches(copy.replace('\'', a).as_str()).count())
                .max(),
            Some(2),
            "{html}"
        );
    }

    /// The tag of the `<button` that precedes the first `needle`.
    fn button_tag_after<'a>(html: &'a str, needle: &str) -> &'a str {
        let at = html.find(needle).expect("needle renders");
        let start = html[..at].rfind("<button").expect("a button precedes it");
        let end = html[start..].find('>').expect("tag closes") + start;
        &html[start..end]
    }

    #[test]
    fn test_connection_opens_no_microphone() {
        let src = include_str!("voice_realtime.rs");
        let needle = ["get_user", "_media"].concat();
        assert!(
            !src.contains(&needle),
            "Test connection must never open the microphone (D-12)"
        );
        // The probe listens with a receive-only transceiver instead.
        let recvonly = ["Recv", "only"].concat();
        assert!(
            src.contains(&recvonly),
            "the probe uses a recvonly transceiver"
        );
    }
    // ── Plan 14 Task 2: key reference, pill, keyless hint, result clearing ──

    const KEYLESS_HINT: &str = "With no key, the browser connects without a token.";

    thread_local! {
        static KEY_STATUS: RefCell<String> = const { RefCell::new(String::new()) };
    }

    fn render_family_with_key_status(status: &str) -> String {
        KEY_STATUS.with(|k| *k.borrow_mut() = status.to_string());
        fn app() -> Element {
            let form = use_realtime_form();
            use_hook(|| {
                let snap = VoiceConfigSnapshot {
                    realtime_key_status: KEY_STATUS.with(|k| k.borrow().clone()),
                    ..Default::default()
                };
                form.seed(&snap);
            });
            rsx! { RealtimeFamily { form, hidden: false, disabled: false } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    thread_local! {
        static KEYED_MODELS: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    }

    /// WR-03: the Model picker's list request carries the typed key reference.
    /// The cache is keyed by (provider, base, key_ref); an entry stored under the
    /// typed reference is only found when the picker sends that same reference.
    #[test]
    fn model_picker_sends_typed_key_ref() {
        KEYED_MODELS.with(|k| {
            *k.borrow_mut() = Some((
                "http://rt.lan:9000/v1".to_string(),
                "VOICE_KEY_RT".to_string(),
            ))
        });
        fn app() -> Element {
            let (base, key_ref) = KEYED_MODELS.with(|k| k.borrow().clone().expect("seeded"));
            let form = use_realtime_form();
            use_hook(|| {
                let mut b = form.base_url;
                b.set(base.clone());
                let mut k = form.key_ref;
                k.set(key_ref.clone());
                let key = ListCacheKey::new(
                    "realtime",
                    REALTIME_PROVIDERS[0].id,
                    &base,
                    "",
                    &key_ref,
                    OptionKind::Models,
                );
                let mut cache = form.list_cache;
                cache.write().insert(
                    key,
                    (ListStatus::Loaded(1), vec!["rt-model-a".to_string()]),
                );
            });
            rsx! { RealtimeFamily { form, hidden: false, disabled: false } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(
            html.contains("1 model loaded from your server."),
            "the Model picker must look up its list under the typed key reference: {html}"
        );
    }

    #[test]
    fn key_ref_field_and_keyless_hint() {
        let none = render_family_with_key_status(KEY_STATUS_NONE);
        assert!(none.contains("API key reference"), "{none}");
        assert!(none.contains("realtime-key-ref"), "{none}");
        assert!(none.contains("NO KEY"), "{none}");
        assert!(has_text(&none, KEYLESS_HINT), "{none}");
        assert!(
            none.contains(crate::voice_registry::KEY_REF_HINT),
            "the shared reference hint is shown: {none}"
        );

        let found = render_family_with_key_status("found");
        assert!(found.contains("FOUND"), "{found}");
        assert!(!found.contains("NO KEY"), "{found}");
        assert!(!has_text(&found, KEYLESS_HINT), "{found}");

        let missing = render_family_with_key_status("not_found");
        assert!(missing.contains("NOT FOUND"), "{missing}");
        assert!(!has_text(&missing, KEYLESS_HINT), "{missing}");
    }

    thread_local! {
        static KEY_COUNTS: RefCell<Option<(usize, usize, usize)>> = const { RefCell::new(None) };
    }

    #[test]
    fn realtime_key_ref_round_trips_and_blocks() {
        let obs = observe(snapshot());
        assert_eq!(
            obs.payload.realtime_key_ref.as_deref(),
            Some("VOICE_KEY_IH_TEST_547_RT"),
            "apply_to writes the reference"
        );
        assert!(obs.clean_after_seed, "a seeded reference is not dirty");

        fn app() -> Element {
            let form = use_realtime_form();
            form.seed(&VoiceConfigSnapshot {
                realtime_key_ref: "VOICE_KEY_IH_TEST_547_RT".to_string(),
                ..Default::default()
            });
            let carried = form.realtime_test_settings().key_ref;
            assert_eq!(carried.as_deref(), Some("VOICE_KEY_IH_TEST_547_RT"));
            let mut key_ref = form.key_ref;
            key_ref.set("sk-abc".to_string());
            let secret = form.blocking_error_count();
            key_ref.set("OPENAI_API_KEY".to_string());
            let out_of_namespace = form.blocking_error_count();
            key_ref.set("VOICE_KEY_RT".to_string());
            let valid = form.blocking_error_count();
            key_ref.set(String::new());
            assert_eq!(form.realtime_test_settings().key_ref, None);
            KEY_COUNTS.with(|c| *c.borrow_mut() = Some((secret, out_of_namespace, valid)));
            rsx! {}
        }
        KEY_COUNTS.with(|c| *c.borrow_mut() = None);
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let counts = KEY_COUNTS.with(|c| c.borrow_mut().take()).expect("ran");
        assert_eq!(counts, (1, 1, 0));
    }

    thread_local! {
        static LIVE_FORM: RefCell<Option<RealtimeForm>> = const { RefCell::new(None) };
    }

    #[test]
    fn realtime_result_clears_on_field_change() {
        // The pure rule: every state returns to Idle.
        assert_eq!(
            conn_next(ConnState::Connected, ConnEvent::FieldChanged),
            ConnState::Idle
        );

        // The wiring: a mounted family clears a finished result when a field
        // changes, and leaves it alone otherwise.
        fn app() -> Element {
            let form = use_realtime_form();
            LIVE_FORM.with(|f| *f.borrow_mut() = Some(form));
            rsx! { RealtimeFamily { form, hidden: false, disabled: false } }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        // Let the mount-time effect run before the scenario starts.
        dom.render_immediate_to_vec();
        let form = LIVE_FORM
            .with(|f| f.borrow_mut().take())
            .expect("the family mounted");
        let conn_now = |dom: &VirtualDom| dom.in_scope(ScopeId::APP, || form.conn.peek().clone());

        dom.in_scope(ScopeId::APP, || {
            let mut conn = form.conn;
            conn.set(ConnState::Connected);
        });
        dom.render_immediate_to_vec();
        assert_eq!(
            conn_now(&dom),
            ConnState::Connected,
            "nothing changed, so the result stays"
        );

        dom.in_scope(ScopeId::APP, || {
            let mut model = form.model;
            model.set("another-model".to_string());
        });
        // Render the changed family, then let the effect it queued run.
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(
            conn_now(&dom),
            ConnState::Idle,
            "a field change clears the result"
        );

        // The key reference is a field too.
        dom.in_scope(ScopeId::APP, || {
            let mut conn = form.conn;
            conn.set(ConnState::Failed("timed out".to_string()));
        });
        dom.render_immediate_to_vec();
        dom.in_scope(ScopeId::APP, || {
            let mut key_ref = form.key_ref;
            key_ref.set("VOICE_KEY_IH_TEST_547_OTHER".to_string());
        });
        dom.render_immediate_to_vec();
        dom.render_immediate_to_vec();
        assert_eq!(conn_now(&dom), ConnState::Idle);
    }
}
