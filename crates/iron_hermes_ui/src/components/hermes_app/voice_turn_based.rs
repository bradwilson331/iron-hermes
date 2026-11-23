//! Phase 54.7 Plan 08 (D-14) -- the Turn-based voice family.
//!
//! `TurnBasedForm` owns the STT / TTS form signals that `VoiceSettings` used to
//! declare inline; `TurnBasedFamily` renders them (plus Auto-speak and Speak
//! mode) inside `div.voice-family[data-family=turn_based]`. The group is ALWAYS
//! mounted by `VoiceSettings`: the non-selected service only toggles the
//! `hidden` attribute, so the form's signals, its pending edits and any later
//! fetched lists survive a flip of the service control.
//!
//! Every default (provider ids, per-provider models, voices, formats) comes
//! from `crate::voice_registry` or from the config snapshot -- this file holds
//! no voice default of its own (D-06).
//!
//! Plan 12 (D-01/D-06/D-09/D-17): the TTS block is registry-driven. The
//! selected provider's registry entry decides which rows render (Base URL /
//! Path / API key reference, Model, Voice, Format), each provider keeps its OWN
//! model / voice / format signals, and the block ends in a Test voice button
//! that auditions the unsaved values.
//!
//! Plan 13 (D-01/D-06/D-17): the STT block is registry-driven too. The selected
//! STT provider decides which rows render (Base URL / Path / API key reference
//! for OpenAI-compatible, Model for Groq / OpenAI / OpenAI-compatible, nothing
//! but the auto-select hint for Auto) and the block ends in a Test mic button
//! that transcribes a 3 second recording with the unsaved values.
//!
//! The family consumes the root-provided voice status signal but provides no
//! context of its own (child providers panic ancestor consumers at runtime).

// Web-only component tree: native dead-code analysis cannot reach the render
// sites (same rationale as voice_settings.rs). The wasm build lints normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::components::hermes_app::voice_fields::{
    EndpointFields, ListCache, OptionPickerField, VoiceField,
};
use crate::components::hermes_app::voice_settings::stt_default_model;
use crate::components::hermes_app::voice_test_controls::{TestMicButton, TestVoiceButton};
use crate::components::hermes_app::VoiceStatusState;
use crate::server::api::{TtsTestSettings, VoiceConfigSnapshot, VoiceWritePayload};
use crate::server::voice_api::SttTestSettings;
use crate::voice_registry::{
    stt_provider, tts_provider, validate_base_url, validate_endpoint_path, validate_key_ref,
    validate_model_id, validate_voice_id, FieldError, OptionKind, ProviderSpec, STT_PROVIDERS,
    TTS_PROVIDERS,
};

/// The Turn-based family's form state: one `Signal<String>` per editable field.
///
/// `Copy` (signals are handles), so a copy moves freely into event closures.
/// TTS model / voice / format are kept PER PROVIDER (D-09): editing one
/// provider's voice never touches another's.
#[derive(Clone, Copy, PartialEq)]
pub struct TurnBasedForm {
    pub stt_provider: Signal<String>,
    pub stt_groq_model: Signal<String>,
    pub stt_openai_model: Signal<String>,
    /// The OpenAI-compatible STT block (`stt.openai_compatible`): its endpoint
    /// rows belong to the block, not to the provider switch, so they are kept.
    pub stt_compat_base_url: Signal<String>,
    pub stt_compat_path: Signal<String>,
    pub stt_compat_key_ref: Signal<String>,
    pub stt_compat_model: Signal<String>,
    /// `found` / `not_found` / `none` for the saved STT key reference (seeded
    /// from the snapshot; the form never holds a key value).
    pub stt_compat_key_status: Signal<String>,
    pub tts_provider: Signal<String>,
    /// Edge TTS voice (`config.tts.edge.voice`).
    pub tts_voice: Signal<String>,
    /// Edge TTS output format (`config.tts.edge.output_format`).
    pub tts_format: Signal<String>,
    pub tts_elevenlabs_model: Signal<String>,
    pub tts_elevenlabs_voice: Signal<String>,
    pub tts_openai_model: Signal<String>,
    pub tts_openai_voice: Signal<String>,
    pub tts_openai_format: Signal<String>,
    /// The OpenAI-compatible block (`tts.openai_compatible`): its endpoint rows
    /// belong to the block, not to the provider switch, so they are kept.
    pub tts_compat_base_url: Signal<String>,
    pub tts_compat_path: Signal<String>,
    pub tts_compat_key_ref: Signal<String>,
    pub tts_compat_model: Signal<String>,
    pub tts_compat_voice: Signal<String>,
    pub tts_compat_format: Signal<String>,
    /// Fetched option lists (D-06). Created inside [`use_turn_based_form`],
    /// which runs in `VoiceSettings`' scope, so it lives as long as the panel
    /// and no context is involved.
    pub list_cache: ListCache,
    /// `Switched to {provider}: ...` after a provider change (D-09); shown only
    /// while the TTS block still matches [`Self::tts_switch_fingerprint`].
    pub tts_switch_note: Signal<Option<String>>,
    /// The TTS block's values right after the switch. The note disappears the
    /// moment any TTS field differs from it, whichever control made the edit.
    tts_switch_fingerprint: Signal<String>,
    /// The STT counterpart of [`Self::tts_switch_note`] (D-09).
    pub stt_switch_note: Signal<Option<String>>,
    stt_switch_fingerprint: Signal<String>,
}

/// A registry default for `provider` (empty when the provider has none).
fn tts_default(provider: &str, pick: fn(&ProviderSpec) -> &'static str) -> String {
    tts_provider(provider)
        .map(pick)
        .unwrap_or_default()
        .to_string()
}

/// Create the family's signals. Call once, unconditionally, from `VoiceSettings`
/// (hook order); defaults come from the registry.
pub fn use_turn_based_form() -> TurnBasedForm {
    TurnBasedForm {
        stt_provider: use_signal(|| STT_PROVIDERS[0].id.to_string()),
        stt_groq_model: use_signal(|| stt_default_model("groq")),
        stt_openai_model: use_signal(|| stt_default_model("openai")),
        stt_compat_base_url: use_signal(String::new),
        stt_compat_path: use_signal(String::new),
        stt_compat_key_ref: use_signal(String::new),
        stt_compat_model: use_signal(|| stt_default_model("openai_compatible")),
        stt_compat_key_status: use_signal(String::new),
        tts_provider: use_signal(|| TTS_PROVIDERS[0].id.to_string()),
        tts_voice: use_signal(|| tts_default("edge", |s| s.default_voice)),
        tts_format: use_signal(|| tts_default("edge", |s| s.default_format)),
        tts_elevenlabs_model: use_signal(|| tts_default("elevenlabs", |s| s.default_model)),
        tts_elevenlabs_voice: use_signal(|| tts_default("elevenlabs", |s| s.default_voice)),
        tts_openai_model: use_signal(|| tts_default("openai", |s| s.default_model)),
        tts_openai_voice: use_signal(|| tts_default("openai", |s| s.default_voice)),
        tts_openai_format: use_signal(|| tts_default("openai", |s| s.default_format)),
        tts_compat_base_url: use_signal(String::new),
        tts_compat_path: use_signal(String::new),
        tts_compat_key_ref: use_signal(String::new),
        tts_compat_model: use_signal(|| tts_default("openai_compatible", |s| s.default_model)),
        tts_compat_voice: use_signal(|| tts_default("openai_compatible", |s| s.default_voice)),
        tts_compat_format: use_signal(|| tts_default("openai_compatible", |s| s.default_format)),
        list_cache: use_signal(HashMap::new),
        tts_switch_note: use_signal(|| None),
        tts_switch_fingerprint: use_signal(String::new),
        stt_switch_note: use_signal(|| None),
        stt_switch_fingerprint: use_signal(String::new),
    }
}

/// Every per-provider STT value of the form (or of a snapshot), as plain
/// strings, so seeding, the dirty check, the payload and the Test mic request
/// share one shape.
#[derive(Clone, Debug, PartialEq, Default)]
struct SttValues {
    groq_model: String,
    openai_model: String,
    c_base: String,
    c_path: String,
    c_key: String,
    c_model: String,
}

/// `value`, or the registry default model for STT `provider` when empty.
fn stt_or_default(value: &str, provider: &str) -> String {
    if value.trim().is_empty() {
        stt_default_model(provider)
    } else {
        value.to_string()
    }
}

/// The values a form shows for `snap`: snapshot values, with the registry
/// default model standing in for an empty one. Seeding and the dirty check
/// share this so they cannot disagree.
fn stt_effective(snap: &VoiceConfigSnapshot) -> SttValues {
    SttValues {
        groq_model: stt_or_default(&snap.stt_groq_model, "groq"),
        openai_model: stt_or_default(&snap.stt_openai_model, "openai"),
        c_base: snap.stt_compat_base_url.clone(),
        c_path: snap.stt_compat_path.clone(),
        c_key: snap.stt_compat_key_ref.clone(),
        c_model: stt_or_default(&snap.stt_compat_model, "openai_compatible"),
    }
}

impl SttValues {
    /// The values the server validates: every text field trimmed.
    fn normalized(&self) -> SttValues {
        let t = |v: &String| v.trim().to_string();
        SttValues {
            groq_model: t(&self.groq_model),
            openai_model: t(&self.openai_model),
            c_base: t(&self.c_base),
            c_path: t(&self.c_path),
            c_key: t(&self.c_key),
            c_model: t(&self.c_model),
        }
    }

    fn model_of(&self, provider: &str) -> &str {
        match provider {
            "groq" => &self.groq_model,
            "openai" => &self.openai_model,
            "openai_compatible" => &self.c_model,
            _ => "",
        }
    }
}

/// The Test mic request for `provider` from already-normalized `values`. Only
/// the fields the provider's registry entry uses are sent; Auto sends the
/// provider alone (the server keeps its saved model).
fn stt_test_settings_from(provider: &str, values: &SttValues) -> SttTestSettings {
    let mut settings = SttTestSettings {
        provider: Some(provider.to_string()),
        ..Default::default()
    };
    let Some(spec) = stt_provider(provider) else {
        return settings;
    };
    if spec.endpoint_fields {
        settings.base_url = non_empty(&values.c_base);
        settings.path = non_empty(&values.c_path);
        settings.key_ref = non_empty(&values.c_key);
    }
    if spec.model_field {
        settings.model = non_empty(values.model_of(provider));
    }
    settings
}

/// Every per-provider TTS value of the form (or of a snapshot), as plain
/// strings, so seeding, the dirty check, the payload and the Test voice request
/// share one shape.
#[derive(Clone, Debug, PartialEq, Default)]
struct TtsValues {
    edge_voice: String,
    edge_format: String,
    el_model: String,
    el_voice: String,
    oa_model: String,
    oa_voice: String,
    oa_format: String,
    c_base: String,
    c_path: String,
    c_key: String,
    c_model: String,
    c_voice: String,
    c_format: String,
}

/// `value`, or the registry default for `provider` when `value` is empty (an
/// older server or a hand-edited config may omit it).
fn or_default(value: &str, provider: &str, pick: fn(&ProviderSpec) -> &'static str) -> String {
    if value.trim().is_empty() {
        tts_default(provider, pick)
    } else {
        value.to_string()
    }
}

/// The values a form shows for `snap`: snapshot values, with the registry
/// default standing in for an empty model / voice / format. Seeding and the
/// dirty check share this so they cannot disagree. A stored format that is not
/// in the registry list is kept RAW (only the Format select normalizes its
/// display), so an untouched legacy value round-trips unchanged.
fn tts_effective(snap: &VoiceConfigSnapshot) -> TtsValues {
    TtsValues {
        edge_voice: or_default(&snap.tts_edge_voice, "edge", |s| s.default_voice),
        edge_format: or_default(&snap.tts_edge_format, "edge", |s| s.default_format),
        el_model: or_default(&snap.tts_elevenlabs_model, "elevenlabs", |s| {
            s.default_model
        }),
        el_voice: or_default(&snap.tts_elevenlabs_voice, "elevenlabs", |s| {
            s.default_voice
        }),
        oa_model: or_default(&snap.tts_openai_model, "openai", |s| s.default_model),
        oa_voice: or_default(&snap.tts_openai_voice, "openai", |s| s.default_voice),
        oa_format: or_default(&snap.tts_openai_format, "openai", |s| s.default_format),
        c_base: snap.tts_compat_base_url.clone(),
        c_path: snap.tts_compat_path.clone(),
        c_key: snap.tts_compat_key_ref.clone(),
        c_model: or_default(&snap.tts_compat_model, "openai_compatible", |s| {
            s.default_model
        }),
        c_voice: or_default(&snap.tts_compat_voice, "openai_compatible", |s| {
            s.default_voice
        }),
        c_format: or_default(&snap.tts_compat_format, "openai_compatible", |s| {
            s.default_format
        }),
    }
}

impl TtsValues {
    /// The values the server validates: every text field trimmed. Formats are
    /// registry tokens, so trimming them is harmless.
    fn normalized(&self) -> TtsValues {
        let t = |v: &String| v.trim().to_string();
        TtsValues {
            edge_voice: t(&self.edge_voice),
            edge_format: t(&self.edge_format),
            el_model: t(&self.el_model),
            el_voice: t(&self.el_voice),
            oa_model: t(&self.oa_model),
            oa_voice: t(&self.oa_voice),
            oa_format: t(&self.oa_format),
            c_base: t(&self.c_base),
            c_path: t(&self.c_path),
            c_key: t(&self.c_key),
            c_model: t(&self.c_model),
            c_voice: t(&self.c_voice),
            c_format: t(&self.c_format),
        }
    }

    fn model_of(&self, provider: &str) -> &str {
        match provider {
            "elevenlabs" => &self.el_model,
            "openai" => &self.oa_model,
            "openai_compatible" => &self.c_model,
            _ => "",
        }
    }

    fn voice_of(&self, provider: &str) -> &str {
        match provider {
            "edge" => &self.edge_voice,
            "elevenlabs" => &self.el_voice,
            "openai" => &self.oa_voice,
            "openai_compatible" => &self.c_voice,
            _ => "",
        }
    }

    /// The stored format of `provider`; empty for a provider with no format
    /// setting of its own (ElevenLabs is MP3-only and persists none).
    fn format_of(&self, provider: &str) -> &str {
        match provider {
            "edge" => &self.edge_format,
            "openai" => &self.oa_format,
            "openai_compatible" => &self.c_format,
            _ => "",
        }
    }
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

/// The Test voice request for `provider` from already-normalized `values`.
/// Only the fields the provider's registry entry uses are sent; the format is
/// the one the Format select shows (a stored value outside the provider's list
/// would be refused by the server's save-style validation).
fn test_settings_from(provider: &str, values: &TtsValues) -> TtsTestSettings {
    let mut settings = TtsTestSettings {
        provider: Some(provider.to_string()),
        ..Default::default()
    };
    let Some(spec) = tts_provider(provider) else {
        return settings;
    };
    if spec.endpoint_fields {
        settings.base_url = non_empty(&values.c_base);
        settings.path = non_empty(&values.c_path);
        settings.key_ref = non_empty(&values.c_key);
    }
    if spec.model_field {
        settings.model = Some(values.model_of(provider).to_string());
    }
    if spec.voice_field {
        settings.voice = Some(values.voice_of(provider).to_string());
    }
    if !spec.formats.is_empty() {
        settings.format = Some(displayed_format(values.format_of(provider), spec).to_string());
    }
    settings
}

/// The format the Format select shows as selected: the stored value when the
/// provider's registry lists it, else the provider's default format. This is a
/// DISPLAY rule only -- the form keeps the raw stored value until the operator
/// picks an option, so a legacy value outside the list (an Edge
/// `audio-24khz-...` string, a hand-typed `flac`) round-trips unchanged and an
/// unrelated save is never blocked by it (plan 06 accepts an unchanged value).
pub fn displayed_format(stored: &str, spec: &ProviderSpec) -> &'static str {
    spec.formats
        .iter()
        .copied()
        .find(|f| *f == stored)
        .unwrap_or(spec.default_format)
}

/// The STT fields of `provider` that fail the server's rule, from
/// already-normalized `values`. Only the selected provider's fields count: a
/// field a provider does not use does not exist for it (Auto has none).
fn stt_field_errors(provider: &str, values: &SttValues) -> Vec<FieldError> {
    let Some(spec) = stt_provider(provider) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    if spec.endpoint_fields {
        errors.extend(validate_base_url(&values.c_base, true).err());
        errors.extend(validate_endpoint_path(&values.c_path).err());
        errors.extend(validate_key_ref(&values.c_key).err());
    }
    if spec.model_field {
        errors.extend(validate_model_id(values.model_of(provider)).err());
    }
    errors
}

/// A fingerprint of the whole STT block (selected provider + every value).
fn stt_fingerprint(provider: &str, values: &SttValues) -> String {
    format!("{provider}|{values:?}")
}

/// Change the STT provider (D-09): the new provider's model resets to its
/// registry default, its cached option lists are dropped so they refetch, and
/// the `Switched to {provider}` note appears until the next edit in the block.
/// Base URL, Path and key reference belong to the OpenAI-compatible block and
/// are left untouched. Auto has no model, so nothing is reset and no note is
/// shown. Nothing is saved until SAVE SETTINGS.
pub fn on_stt_provider_change(form: &TurnBasedForm, new_provider: &str) {
    let mut provider = form.stt_provider;
    if provider.peek().as_str() == new_provider {
        return;
    }
    provider.set(new_provider.to_string());
    let (mut note, mut at_switch) = (form.stt_switch_note, form.stt_switch_fingerprint);
    let Some(spec) = stt_provider(new_provider) else {
        note.set(None);
        return;
    };
    let Some(mut model) = form.stt_model_signal(spec.id) else {
        note.set(None);
        return;
    };
    model.set(spec.default_model.to_string());
    let mut cache = form.list_cache;
    cache
        .write()
        .retain(|key, _| !(key.family == "stt" && key.provider == spec.id));
    at_switch.set(stt_fingerprint(
        new_provider,
        &form.stt_values(|s| s.peek().clone()).normalized(),
    ));
    note.set(Some(format!(
        "Switched to {}: model, voice and format reset to its defaults.",
        spec.label
    )));
}

/// The TTS fields of `provider` that fail the server's rule, from
/// already-normalized `values`. Only the selected provider's fields count: a
/// field a provider does not use does not exist for it.
fn tts_field_errors(provider: &str, values: &TtsValues) -> Vec<FieldError> {
    let Some(spec) = tts_provider(provider) else {
        return Vec::new();
    };
    let mut errors = Vec::new();
    if spec.endpoint_fields {
        errors.extend(validate_base_url(&values.c_base, true).err());
        errors.extend(validate_endpoint_path(&values.c_path).err());
        errors.extend(validate_key_ref(&values.c_key).err());
    }
    if spec.model_field {
        errors.extend(validate_model_id(values.model_of(provider)).err());
    }
    if spec.voice_field {
        errors.extend(validate_voice_id(values.voice_of(provider)).err());
    }
    errors
}

/// The switch note while it still describes the block: `note` is shown only if
/// the TTS values are exactly those right after the switch.
fn visible_switch_note(note: Option<String>, at_switch: &str, now: &str) -> Option<String> {
    note.filter(|_| at_switch == now)
}

/// A fingerprint of the whole TTS block (selected provider + every value).
fn tts_fingerprint(provider: &str, values: &TtsValues) -> String {
    format!("{provider}|{values:?}")
}

/// Change the TTS provider (D-09): the new provider's model, voice and format
/// reset to its registry defaults (a provider with no setting of a kind has
/// nothing to reset), its cached option lists are dropped so they refetch, and
/// the `Switched to {provider}` note appears until the next edit in the block.
/// Base URL, Path and key reference belong to the OpenAI-compatible block and
/// are left untouched. Nothing is saved until SAVE SETTINGS.
pub fn on_tts_provider_change(form: &TurnBasedForm, new_provider: &str) {
    let mut provider = form.tts_provider;
    if provider.peek().as_str() == new_provider {
        return;
    }
    provider.set(new_provider.to_string());
    let Some(spec) = tts_provider(new_provider) else {
        return;
    };
    if let Some(mut model) = form.model_signal(spec.id) {
        model.set(spec.default_model.to_string());
    }
    if let Some(mut voice) = form.voice_signal(spec.id) {
        voice.set(spec.default_voice.to_string());
    }
    if let Some(mut format) = form.format_signal(spec.id) {
        format.set(spec.default_format.to_string());
    }
    let mut cache = form.list_cache;
    cache
        .write()
        .retain(|key, _| !(key.family == "tts" && key.provider == spec.id));
    let fingerprint = tts_fingerprint(
        new_provider,
        &form.tts_values(|s| s.peek().clone()).normalized(),
    );
    let (mut note, mut at_switch) = (form.tts_switch_note, form.tts_switch_fingerprint);
    at_switch.set(fingerprint);
    note.set(Some(format!(
        "Switched to {}: model, voice and format reset to its defaults.",
        spec.label
    )));
}

impl TurnBasedForm {
    /// The signal holding `provider`'s model, when it has a model setting.
    pub fn model_signal(&self, provider: &str) -> Option<Signal<String>> {
        match provider {
            "elevenlabs" => Some(self.tts_elevenlabs_model),
            "openai" => Some(self.tts_openai_model),
            "openai_compatible" => Some(self.tts_compat_model),
            _ => None,
        }
    }

    /// The signal holding `provider`'s voice.
    pub fn voice_signal(&self, provider: &str) -> Option<Signal<String>> {
        match provider {
            "edge" => Some(self.tts_voice),
            "elevenlabs" => Some(self.tts_elevenlabs_voice),
            "openai" => Some(self.tts_openai_voice),
            "openai_compatible" => Some(self.tts_compat_voice),
            _ => None,
        }
    }

    /// The signal holding `provider`'s format, when it persists one.
    pub fn format_signal(&self, provider: &str) -> Option<Signal<String>> {
        match provider {
            "edge" => Some(self.tts_format),
            "openai" => Some(self.tts_openai_format),
            "openai_compatible" => Some(self.tts_compat_format),
            _ => None,
        }
    }

    /// The signal holding STT `provider`'s model, when it has a model setting.
    pub fn stt_model_signal(&self, provider: &str) -> Option<Signal<String>> {
        match provider {
            "groq" => Some(self.stt_groq_model),
            "openai" => Some(self.stt_openai_model),
            "openai_compatible" => Some(self.stt_compat_model),
            _ => None,
        }
    }

    /// Every STT value of the form, read through `get` (`.read()` in render,
    /// `.peek()` in handlers).
    fn stt_values(&self, get: impl Fn(&Signal<String>) -> String) -> SttValues {
        SttValues {
            groq_model: get(&self.stt_groq_model),
            openai_model: get(&self.stt_openai_model),
            c_base: get(&self.stt_compat_base_url),
            c_path: get(&self.stt_compat_path),
            c_key: get(&self.stt_compat_key_ref),
            c_model: get(&self.stt_compat_model),
        }
    }

    /// Every TTS value of the form, read through `get` (`.read()` in render,
    /// `.peek()` in handlers).
    fn tts_values(&self, get: impl Fn(&Signal<String>) -> String) -> TtsValues {
        TtsValues {
            edge_voice: get(&self.tts_voice),
            edge_format: get(&self.tts_format),
            el_model: get(&self.tts_elevenlabs_model),
            el_voice: get(&self.tts_elevenlabs_voice),
            oa_model: get(&self.tts_openai_model),
            oa_voice: get(&self.tts_openai_voice),
            oa_format: get(&self.tts_openai_format),
            c_base: get(&self.tts_compat_base_url),
            c_path: get(&self.tts_compat_path),
            c_key: get(&self.tts_compat_key_ref),
            c_model: get(&self.tts_compat_model),
            c_voice: get(&self.tts_compat_voice),
            c_format: get(&self.tts_compat_format),
        }
    }

    /// Load every field from `snap`. Called from the panel's one-shot load latch
    /// only (never from an effect that also reads these signals).
    pub fn seed(&self, snap: &VoiceConfigSnapshot) {
        let (mut a, mut d, mut status) = (
            self.stt_provider,
            self.tts_provider,
            self.stt_compat_key_status,
        );
        a.set(snap.stt_provider.clone());
        d.set(snap.tts_provider.clone());
        status.set(snap.stt_compat_key_status.clone());
        let stt = stt_effective(snap);
        let stt_pairs: [(Signal<String>, String); 6] = [
            (self.stt_groq_model, stt.groq_model),
            (self.stt_openai_model, stt.openai_model),
            (self.stt_compat_base_url, stt.c_base),
            (self.stt_compat_path, stt.c_path),
            (self.stt_compat_key_ref, stt.c_key),
            (self.stt_compat_model, stt.c_model),
        ];
        for (mut signal, value) in stt_pairs {
            signal.set(value);
        }
        let v = tts_effective(snap);
        let pairs: [(Signal<String>, String); 13] = [
            (self.tts_voice, v.edge_voice),
            (self.tts_format, v.edge_format),
            (self.tts_elevenlabs_model, v.el_model),
            (self.tts_elevenlabs_voice, v.el_voice),
            (self.tts_openai_model, v.oa_model),
            (self.tts_openai_voice, v.oa_voice),
            (self.tts_openai_format, v.oa_format),
            (self.tts_compat_base_url, v.c_base),
            (self.tts_compat_path, v.c_path),
            (self.tts_compat_key_ref, v.c_key),
            (self.tts_compat_model, v.c_model),
            (self.tts_compat_voice, v.c_voice),
            (self.tts_compat_format, v.c_format),
        ];
        for (mut signal, value) in pairs {
            signal.set(value);
        }
    }

    /// Write this form's fields into `p` (save path; `.peek()` -- it runs in an
    /// event handler, never in render). Every provider's block rides each save
    /// (like the realtime block), from its own signals. Fields this form does
    /// not own are left untouched.
    pub fn apply_to(&self, p: &mut VoiceWritePayload) {
        p.stt_provider = Some(self.stt_provider.peek().clone());
        let stt = self.stt_values(|s| s.peek().clone()).normalized();
        p.stt_groq_model = Some(stt.groq_model);
        p.stt_openai_model = Some(stt.openai_model);
        // An empty path / key reference clears the field (plan 06).
        p.stt_compat_base_url = Some(stt.c_base);
        p.stt_compat_path = Some(stt.c_path);
        p.stt_compat_key_ref = Some(stt.c_key);
        p.stt_compat_model = Some(stt.c_model);
        p.tts_provider = Some(self.tts_provider.peek().clone());
        let v = self.tts_values(|s| s.peek().clone()).normalized();
        p.tts_edge_voice = Some(v.edge_voice);
        p.tts_edge_format = Some(v.edge_format);
        p.tts_elevenlabs_model = Some(v.el_model);
        p.tts_elevenlabs_voice = Some(v.el_voice);
        p.tts_openai_model = Some(v.oa_model);
        p.tts_openai_voice = Some(v.oa_voice);
        p.tts_openai_format = Some(v.oa_format);
        // An empty path / key reference clears the field (plan 06).
        p.tts_compat_base_url = Some(v.c_base);
        p.tts_compat_path = Some(v.c_path);
        p.tts_compat_key_ref = Some(v.c_key);
        p.tts_compat_model = Some(v.c_model);
        p.tts_compat_voice = Some(v.c_voice);
        p.tts_compat_format = Some(v.c_format);
    }

    /// True when any field differs from `loaded` (render path: `.read()`, so the
    /// Save button re-evaluates when a field changes).
    pub fn is_dirty(&self, loaded: &VoiceConfigSnapshot) -> bool {
        *self.stt_provider.read() != loaded.stt_provider
            || self.stt_values(|s| s.read().clone()).normalized()
                != stt_effective(loaded).normalized()
            || *self.tts_provider.read() != loaded.tts_provider
            || self.tts_values(|s| s.read().clone()).normalized()
                != tts_effective(loaded).normalized()
    }

    /// Fold the just-saved values into `loaded` so the form reads clean again.
    pub fn commit_to(&self, loaded: &mut VoiceConfigSnapshot) {
        loaded.stt_provider = self.stt_provider.peek().clone();
        let stt = self.stt_values(|s| s.peek().clone()).normalized();
        loaded.stt_groq_model = stt.groq_model;
        loaded.stt_openai_model = stt.openai_model;
        loaded.stt_compat_base_url = stt.c_base;
        loaded.stt_compat_path = stt.c_path;
        loaded.stt_compat_key_ref = stt.c_key;
        loaded.stt_compat_model = stt.c_model;
        loaded.tts_provider = self.tts_provider.peek().clone();
        let v = self.tts_values(|s| s.peek().clone()).normalized();
        loaded.tts_edge_voice = v.edge_voice;
        loaded.tts_edge_format = v.edge_format;
        loaded.tts_elevenlabs_model = v.el_model;
        loaded.tts_elevenlabs_voice = v.el_voice;
        loaded.tts_openai_model = v.oa_model;
        loaded.tts_openai_voice = v.oa_voice;
        loaded.tts_openai_format = v.oa_format;
        loaded.tts_compat_base_url = v.c_base;
        loaded.tts_compat_path = v.c_path;
        loaded.tts_compat_key_ref = v.c_key;
        loaded.tts_compat_model = v.c_model;
        loaded.tts_compat_voice = v.c_voice;
        loaded.tts_compat_format = v.c_format;
    }

    /// The unsaved TTS values a Test voice request auditions (D-17). Read with
    /// `.read()` so the render that builds the button's props re-renders when
    /// any TTS field changes -- the button must never audition stale values.
    pub fn tts_test_settings(&self) -> TtsTestSettings {
        let provider = self.tts_provider.read().clone();
        test_settings_from(
            &provider,
            &self.tts_values(|s| s.read().clone()).normalized(),
        )
    }

    /// The unsaved STT values a Test mic request transcribes with (D-17). Read
    /// with `.read()` so the render that builds the button's props re-renders
    /// when any STT field changes -- the button must never test stale values.
    pub fn stt_test_settings(&self) -> SttTestSettings {
        let provider = self.stt_provider.read().clone();
        stt_test_settings_from(
            &provider,
            &self.stt_values(|s| s.read().clone()).normalized(),
        )
    }

    /// The `Switched to {provider}` note, while it still describes the block.
    /// Render path (`.read()`), so editing any TTS field re-renders and clears it.
    pub fn switch_note(&self) -> Option<String> {
        let provider = self.tts_provider.read().clone();
        let now = tts_fingerprint(
            &provider,
            &self.tts_values(|s| s.read().clone()).normalized(),
        );
        let at_switch = self.tts_switch_fingerprint.read().clone();
        visible_switch_note(self.tts_switch_note.read().clone(), &at_switch, &now)
    }

    /// The STT `Switched to {provider}` note, while it still describes the
    /// block. Render path (`.read()`), so editing any STT field clears it.
    pub fn stt_switch_note(&self) -> Option<String> {
        let provider = self.stt_provider.read().clone();
        let now = stt_fingerprint(
            &provider,
            &self.stt_values(|s| s.read().clone()).normalized(),
        );
        let at_switch = self.stt_switch_fingerprint.read().clone();
        visible_switch_note(self.stt_switch_note.read().clone(), &at_switch, &now)
    }

    /// Fields that currently block saving (each fails the server's rule too):
    /// the selected TTS and STT providers' Base URL, Path, key reference, model
    /// and (TTS) voice.
    pub fn blocking_error_count(&self) -> usize {
        let tts = self.tts_provider.read().clone();
        let tts_values = self.tts_values(|s| s.read().clone()).normalized();
        let stt = self.stt_provider.read().clone();
        let stt_values = self.stt_values(|s| s.read().clone()).normalized();
        tts_field_errors(&tts, &tts_values).len() + stt_field_errors(&stt, &stt_values).len()
    }
}

/// The TTS provider-specific rows: the selected provider's registry entry
/// decides which render (a field the provider does not use does not exist for
/// it), ending in the Test voice button.
#[component]
fn TtsFields(
    form: TurnBasedForm,
    provider: String,
    visible: bool,
    disabled: bool,
    key_status: String,
    #[props(default)] on_edit: Option<EventHandler<()>>,
) -> Element {
    let Some(spec) = tts_provider(&provider) else {
        return rsx! {};
    };
    let notify = move || {
        if let Some(handler) = on_edit.as_ref() {
            handler.call(());
        }
    };

    // Pattern B: owned locals; every borrow ends at its `;`.
    let values = form.tts_values(|s| s.read().clone()).normalized();
    let (base_url, path, key_ref) = if spec.endpoint_fields {
        (
            values.c_base.clone(),
            values.c_path.clone(),
            values.c_key.clone(),
        )
    } else {
        (String::new(), String::new(), String::new())
    };
    let model_val = values.model_of(spec.id).to_string();
    let voice_val = values.voice_of(spec.id).to_string();
    // The select SHOWS the provider's default when the stored value is not in
    // its list; the form keeps the raw stored value until an option is picked.
    let format_shown = displayed_format(values.format_of(spec.id), spec);
    let model_error = spec
        .model_field
        .then(|| validate_model_id(&model_val).err())
        .flatten()
        .map(|e| e.copy().to_string());
    let voice_error = spec
        .voice_field
        .then(|| validate_voice_id(&voice_val).err())
        .flatten()
        .map(|e| e.copy().to_string());
    // An invalid Base URL disables Test voice with the validation error as the
    // visible reason (UI-SPEC, Test controls).
    let test_reason = spec
        .endpoint_fields
        .then(|| validate_base_url(&base_url, true).err())
        .flatten()
        .map(|e| e.copy().to_string());
    let model_curated: Vec<String> = spec.curated_models.iter().map(|m| m.to_string()).collect();
    let voice_curated: Vec<String> = spec.curated_voices.iter().map(|v| v.to_string()).collect();
    let format_sig = form.format_signal(spec.id);
    let model_sig = form.model_signal(spec.id);
    let voice_sig = form.voice_signal(spec.id);
    let test_settings = form.tts_test_settings();

    rsx! {
        if spec.endpoint_fields {
            EndpointFields {
                base_url: form.tts_compat_base_url,
                path: Some(form.tts_compat_path),
                key_ref: Some(form.tts_compat_key_ref),
                key_status,
                default_path: spec.default_path.to_string(),
                base_placeholder: "http://host:8880/v1".to_string(),
                required: true,
                disabled,
                id_prefix: "tts-compat".to_string(),
                on_edit,
            }
        }
        if let Some(model) = model_sig {
            OptionPickerField {
                label: "Model".to_string(),
                field_id: "tts-model".to_string(),
                kind: OptionKind::Models,
                family: "tts".to_string(),
                provider: spec.id.to_string(),
                provider_label: spec.label.to_string(),
                value: model,
                curated: model_curated,
                base_url: base_url.clone(),
                path: path.clone(),
                key_ref: key_ref.clone(),
                model_for_voices: String::new(),
                fetchable: spec.discovers_models,
                visible,
                disabled,
                cache: form.list_cache,
                seeded: spec.default_model.to_string(),
                placeholder: "Pick or type a model".to_string(),
                title: "TTS model".to_string(),
                error: model_error,
            }
        }
        if let Some(voice) = voice_sig {
            OptionPickerField {
                label: "Voice".to_string(),
                field_id: "tts-voice".to_string(),
                kind: OptionKind::Voices,
                family: "tts".to_string(),
                provider: spec.id.to_string(),
                provider_label: spec.label.to_string(),
                value: voice,
                curated: voice_curated,
                base_url: base_url.clone(),
                path: path.clone(),
                key_ref: key_ref.clone(),
                model_for_voices: model_val.clone(),
                fetchable: spec.discovers_voices,
                visible,
                disabled,
                cache: form.list_cache,
                seeded: spec.default_voice.to_string(),
                placeholder: "Pick or type a voice".to_string(),
                title: "TTS voice".to_string(),
                noun: "voice".to_string(),
                error: voice_error,
            }
        }
        // Format: a select over the provider's registry formats only.
        VoiceField {
            label: "Format".to_string(),
            field_id: "tts-format-select".to_string(),
            select {
                id: "tts-format-select",
                class: "voice-settings-select",
                disabled,
                value: "{format_shown}",
                "aria-label": "TTS output format",
                onchange: move |evt| {
                    notify();
                    if let Some(mut sig) = format_sig {
                        sig.set(evt.value());
                    }
                },
                for f in spec.formats.iter() {
                    option {
                        key: "{f}",
                        value: "{f}",
                        selected: *f == format_shown,
                        "{f}"
                    }
                }
            }
        }
        TestVoiceButton {
            identity_slug: None,
            settings: Some(test_settings),
            disabled_reason: test_reason,
        }
    }
}

/// The STT provider-specific rows: the selected provider's registry entry
/// decides which render (a field the provider does not use does not exist for
/// it), ending in the Test mic button.
#[component]
fn SttFields(
    form: TurnBasedForm,
    provider: String,
    visible: bool,
    disabled: bool,
    #[props(default)] on_edit: Option<EventHandler<()>>,
) -> Element {
    // An unknown provider id (an unseeded form) reads as Auto: nothing to edit.
    let Some(spec) = stt_provider(&provider).or_else(|| stt_provider(STT_PROVIDERS[0].id)) else {
        return rsx! {};
    };

    // Pattern B: owned locals; every borrow ends at its `;`.
    let values = form.stt_values(|s| s.read().clone()).normalized();
    let key_status = form.stt_compat_key_status.read().clone();
    let (base_url, path, key_ref) = if spec.endpoint_fields {
        (
            values.c_base.clone(),
            values.c_path.clone(),
            values.c_key.clone(),
        )
    } else {
        (String::new(), String::new(), String::new())
    };
    let model_val = values.model_of(spec.id).to_string();
    let model_error = spec
        .model_field
        .then(|| validate_model_id(&model_val).err())
        .flatten()
        .map(|e| e.copy().to_string());
    // An invalid Base URL disables Test mic with the validation error as the
    // visible reason (UI-SPEC, Test controls).
    let test_reason = spec
        .endpoint_fields
        .then(|| validate_base_url(&base_url, true).err())
        .flatten()
        .map(|e| e.copy().to_string());
    let model_curated: Vec<String> = spec.curated_models.iter().map(|m| m.to_string()).collect();
    let model_sig = form.stt_model_signal(spec.id);
    let test_settings = form.stt_test_settings();

    rsx! {
        if spec.endpoint_fields {
            EndpointFields {
                base_url: form.stt_compat_base_url,
                path: Some(form.stt_compat_path),
                key_ref: Some(form.stt_compat_key_ref),
                key_status,
                default_path: spec.default_path.to_string(),
                base_placeholder: "http://host:8880/v1".to_string(),
                required: true,
                disabled,
                id_prefix: "stt-compat".to_string(),
                on_edit,
            }
        }
        if let Some(model) = model_sig {
            OptionPickerField {
                label: "Model".to_string(),
                field_id: "stt-model".to_string(),
                kind: OptionKind::Models,
                family: "stt".to_string(),
                provider: spec.id.to_string(),
                provider_label: spec.label.to_string(),
                value: model,
                curated: model_curated,
                base_url: base_url.clone(),
                path: path.clone(),
                key_ref: key_ref.clone(),
                model_for_voices: String::new(),
                // Only the OpenAI-compatible server can list its models (D-06);
                // Groq and OpenAI stay on the curated registry list.
                fetchable: spec.discovers_models,
                visible,
                disabled,
                cache: form.list_cache,
                seeded: spec.default_model.to_string(),
                placeholder: "Pick or type a model".to_string(),
                title: "STT model".to_string(),
                error: model_error,
            }
        }
        TestMicButton {
            settings: Some(test_settings),
            disabled_reason: test_reason,
        }
    }
}

/// The Turn-based family group: STT, TTS, Auto-speak, Speak mode.
///
/// `hidden` toggles the `hidden` attribute only -- the group is never
/// conditionally mounted. `tts_key_status` is the saved OpenAI-compatible TTS
/// key reference's status (`found` / `not_found` / `none`, from the snapshot).
#[component]
pub fn TurnBasedFamily(
    form: TurnBasedForm,
    auto_tts: Signal<bool>,
    hidden: bool,
    disabled: bool,
    #[props(default)] on_edit: Option<EventHandler<()>>,
    #[props(default)] tts_key_status: String,
) -> Element {
    let status = use_context::<Signal<VoiceStatusState>>();
    let stt_available = status.read().stt_available;
    let tts_available = status.read().tts_available;

    let mut auto_tts = auto_tts;

    // Pattern B: owned locals; every borrow ends at its `;`.
    let stt_provider_val = form.stt_provider.read().clone();
    let tts_provider_val = form.tts_provider.read().clone();
    let tts_switch_note = form.switch_note();
    let stt_switch_note = form.stt_switch_note();
    let auto_tts_on = *auto_tts.read();

    // Auto has no fields of its own: the select carries the auto-select hint.
    let stt_auto_hint = (stt_provider_val == STT_PROVIDERS[0].id)
        .then(|| "Hermes selects the provider automatically.".to_string());

    let notify = move || {
        if let Some(handler) = on_edit.as_ref() {
            handler.call(());
        }
    };

    rsx! {
        div {
            class: "voice-family",
            "data-family": "turn_based",
            hidden: if hidden { "true" },

            // ── STT ───────────────────────────────────────────────────────
            div { class: "voice-field-head",
                p {
                    class: "voice-settings-section-header",
                    role: "heading",
                    "aria-level": "3",
                    "STT"
                }
                span {
                    class: if stt_available { "voice-status-pill is-active" } else { "voice-status-pill is-unavailable" },
                    if stt_available { "ACTIVE" } else { "UNAVAILABLE" }
                }
            }
            hr { class: "voice-settings-separator" }

            // STT Provider select (a registry-driven list)
            VoiceField {
                label: "Provider".to_string(),
                field_id: "stt-provider-select".to_string(),
                hint: stt_auto_hint,
                status: stt_switch_note.map(|text| (text, false)),
                select {
                    id: "stt-provider-select",
                    class: "voice-settings-select",
                    disabled,
                    value: "{stt_provider_val}",
                    onchange: move |evt| {
                        notify();
                        on_stt_provider_change(&form, &evt.value());
                    },
                    for spec in STT_PROVIDERS.iter() {
                        option { key: "{spec.id}", value: "{spec.id}", "{spec.label}" }
                    }
                }
            }

            // The selected provider's rows and the Test mic button.
            SttFields {
                form,
                provider: stt_provider_val.clone(),
                visible: !hidden,
                disabled,
                on_edit,
            }

            hr { class: "voice-settings-separator" }

            // ── TTS ───────────────────────────────────────────────────────
            div { class: "voice-field-head",
                p {
                    class: "voice-settings-section-header",
                    role: "heading",
                    "aria-level": "3",
                    "TTS"
                }
                span {
                    class: if tts_available { "voice-status-pill is-active" } else { "voice-status-pill is-unavailable" },
                    if tts_available { "ACTIVE" } else { "UNAVAILABLE" }
                }
            }
            hr { class: "voice-settings-separator" }

            // TTS Provider select (a registry-driven list)
            VoiceField {
                label: "Provider".to_string(),
                field_id: "tts-provider-select".to_string(),
                status: tts_switch_note.map(|text| (text, false)),
                select {
                    id: "tts-provider-select",
                    class: "voice-settings-select",
                    disabled,
                    value: "{tts_provider_val}",
                    onchange: move |evt| {
                        notify();
                        on_tts_provider_change(&form, &evt.value());
                    },
                    for spec in TTS_PROVIDERS.iter() {
                        option { key: "{spec.id}", value: "{spec.id}", "{spec.label}" }
                    }
                }
            }

            // The selected provider's rows and the Test voice button.
            TtsFields {
                form,
                provider: tts_provider_val.clone(),
                visible: !hidden,
                disabled,
                key_status: tts_key_status.clone(),
                on_edit,
            }

            hr { class: "voice-settings-separator" }

            // Auto-speak toggle (auto_tts)
            div { class: "voice-settings-row",
                label {
                    r#for: "auto-tts-toggle",
                    class: "voice-settings-label",
                    "Auto-speak"
                }
                div { style: "display:flex; align-items:center; gap:var(--space-1);",
                    button {
                        id: "auto-tts-toggle",
                        class: if auto_tts_on { "voice-toggle is-on" } else { "voice-toggle" },
                        r#type: "button",
                        role: "switch",
                        "aria-checked": if auto_tts_on { "true" } else { "false" },
                        "aria-label": "Auto-speak",
                        disabled,
                        onclick: move |_| {
                            notify();
                            auto_tts.set(!auto_tts_on);
                        },
                        span { class: "voice-toggle-thumb" }
                    }
                    // "live" pill — auto_tts takes effect without restart
                    span { class: "voice-pill-live", "live" }
                }
            }

            // Speak mode segmented control (maps to auto_tts — two presentations of same field)
            div { class: "voice-settings-row", style: "flex-direction: column; align-items: flex-start;",
                label { class: "voice-settings-label", style: "margin-bottom:var(--space-1);",
                    "Speak mode"
                }
                div {
                    class: "voice-seg-control",
                    role: "radiogroup",
                    "aria-label": "Speak mode",
                    button {
                        class: if !auto_tts_on { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if !auto_tts_on { "true" } else { "false" },
                        disabled,
                        onclick: move |_| {
                            notify();
                            auto_tts.set(false);
                        },
                        "Tool-driven"
                    }
                    button {
                        class: if auto_tts_on { "voice-seg is-active" } else { "voice-seg" },
                        r#type: "button",
                        role: "radio",
                        "aria-checked": if auto_tts_on { "true" } else { "false" },
                        disabled,
                        onclick: move |_| {
                            notify();
                            auto_tts.set(true);
                        },
                        "Auto (every reply)"
                    }
                }
                // Helper text — changes with mode
                p { class: "voice-settings-hint", style: "margin-top:var(--space-1);",
                    if auto_tts_on {
                        "Hermes speaks every reply automatically."
                    } else {
                        "Hermes speaks only when it calls the audio tool."
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::components::hermes_app::voice_settings::AudioPlaybackActiveCtx;
    use crate::components::hermes_app::VoiceStatusState;
    use crate::server::api::{VoiceConfigSnapshot, VoiceWritePayload};
    use std::any::Any;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    fn snapshot() -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            stt_provider: "openai".to_string(),
            stt_groq_model: "groq-model-a".to_string(),
            stt_openai_model: "openai-model-b".to_string(),
            tts_provider: "elevenlabs".to_string(),
            tts_edge_voice: "voice-a".to_string(),
            tts_edge_format: "format-a".to_string(),
            ..Default::default()
        }
    }

    struct RoundTrip {
        payload: VoiceWritePayload,
        clean_after_seed: bool,
        dirty_after_edit: bool,
        blocking: usize,
    }

    thread_local! {
        static SNAP: RefCell<VoiceConfigSnapshot> = RefCell::new(VoiceConfigSnapshot::default());
        static OUT: RefCell<Option<RoundTrip>> = const { RefCell::new(None) };
        static STATUS: RefCell<(bool, bool)> = const { RefCell::new((false, false)) };
    }

    fn run_round_trip(snap: VoiceConfigSnapshot) -> RoundTrip {
        SNAP.with(|s| *s.borrow_mut() = snap);
        OUT.with(|o| *o.borrow_mut() = None);

        fn app() -> Element {
            let snap = SNAP.with(|s| s.borrow().clone());
            let form = use_turn_based_form();
            form.seed(&snap);
            let clean_after_seed = !form.is_dirty(&snap);
            let mut payload = VoiceWritePayload::default();
            form.apply_to(&mut payload);
            let mut voice = form.tts_voice;
            voice.set("voice-changed".to_string());
            let dirty_after_edit = form.is_dirty(&snap);
            let blocking = form.blocking_error_count();
            OUT.with(|o| {
                *o.borrow_mut() = Some(RoundTrip {
                    payload,
                    clean_after_seed,
                    dirty_after_edit,
                    blocking,
                })
            });
            rsx! {}
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        OUT.with(|o| o.borrow_mut().take())
            .expect("the harness ran")
    }

    #[test]
    fn turn_based_form_round_trips_payload() {
        let rt = run_round_trip(snapshot());
        let p = &rt.payload;
        assert_eq!(p.stt_provider.as_deref(), Some("openai"));
        assert_eq!(p.stt_groq_model.as_deref(), Some("groq-model-a"));
        assert_eq!(p.stt_openai_model.as_deref(), Some("openai-model-b"));
        assert_eq!(p.tts_provider.as_deref(), Some("elevenlabs"));
        assert_eq!(p.tts_edge_voice.as_deref(), Some("voice-a"));
        assert_eq!(p.tts_edge_format.as_deref(), Some("format-a"));
        // Only the fields this form owns are named; the rest stays untouched.
        assert_eq!(p.auto_tts, None);
        assert_eq!(p.voice_service, None);
        assert_eq!(p.realtime_model, None);
        assert!(rt.clean_after_seed, "a freshly seeded form is not dirty");
        assert!(rt.dirty_after_edit, "one signal change marks it dirty");
        assert_eq!(rt.blocking, 0, "no endpoint fields yet (plans 12/13)");
    }

    fn render_family(stt_available: bool, tts_available: bool) -> String {
        STATUS.with(|s| *s.borrow_mut() = (stt_available, tts_available));

        fn app() -> Element {
            let (stt_available, tts_available) = STATUS.with(|s| *s.borrow());
            let status = use_signal(|| VoiceStatusState {
                stt_available,
                tts_available,
                ..Default::default()
            });
            use_context_provider(|| status);
            let playing = use_signal(|| false);
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            let form = use_turn_based_form();
            let auto_tts = use_signal(|| false);
            rsx! {
                TurnBasedFamily { form, auto_tts, hidden: false, disabled: false }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// The text between the heading `>{heading}<` and the next `</div>`: the
    /// heading row, which must carry its own status pill.
    fn heading_row<'a>(html: &'a str, heading: &str) -> &'a str {
        let marker = format!(">{heading}<");
        let at = html
            .find(&marker)
            .unwrap_or_else(|| panic!("heading {heading} missing: {html}"));
        let rest = &html[at..];
        let end = rest.find("</div>").expect("heading row closes");
        &rest[..end]
    }

    #[test]
    fn status_pills_live_in_family_headings() {
        let html = render_family(true, false);
        assert!(
            html.contains("data-family=\"turn_based\""),
            "positive control: the family renders: {html}"
        );
        let stt = heading_row(&html, "STT");
        assert!(stt.contains("voice-status-pill is-active"), "{stt}");
        assert!(stt.contains("ACTIVE"), "{stt}");
        let tts = heading_row(&html, "TTS");
        assert!(tts.contains("voice-status-pill is-unavailable"), "{tts}");
        assert!(tts.contains("UNAVAILABLE"), "{tts}");
        // Auto-speak and Speak mode moved into this family.
        assert!(html.contains("Auto-speak"), "{html}");
        assert!(html.contains("Speak mode"), "{html}");
    }

    #[test]
    fn voice_turn_based_has_no_hardcoded_voice_defaults() {
        let src = include_str!("voice_turn_based.rs");
        for needle in crate::voice_registry::hardcoded_voice_default_needles() {
            assert!(
                !src.contains(needle.as_str()),
                "voice_turn_based.rs still holds a hard-coded voice default: {needle}"
            );
        }
    }

    // ── Plan 12 Task 1: registry-driven TTS block ────────────────────────────

    type Probe = Rc<dyn Fn(&TurnBasedForm) -> Box<dyn Any>>;

    thread_local! {
        static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
        static PROBE_OUT: RefCell<Option<Box<dyn Any>>> = const { RefCell::new(None) };
        static ACT: Cell<Option<fn(&TurnBasedForm)>> = const { Cell::new(None) };
    }

    /// Seed a form from `snap` inside a VirtualDom (hooks need one) and run `f`
    /// on it.
    fn observe<R: 'static>(
        snap: VoiceConfigSnapshot,
        f: impl Fn(&TurnBasedForm) -> R + 'static,
    ) -> R {
        SNAP.with(|s| *s.borrow_mut() = snap);
        PROBE.with(|p| *p.borrow_mut() = Some(Rc::new(move |form| Box::new(f(form)))));
        PROBE_OUT.with(|o| *o.borrow_mut() = None);

        fn app() -> Element {
            let snap = SNAP.with(|s| s.borrow().clone());
            let form = use_turn_based_form();
            form.seed(&snap);
            let probe = PROBE
                .with(|p| p.borrow().clone())
                .expect("a probe is staged");
            let out = probe(&form);
            PROBE_OUT.with(|o| *o.borrow_mut() = Some(out));
            rsx! {}
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        let out = PROBE_OUT
            .with(|o| o.borrow_mut().take())
            .expect("the probe ran");
        *out.downcast::<R>().expect("the probe result type")
    }

    /// SSR of the family with `snap` seeded; `act` (if any) mutates the form
    /// before it renders.
    fn render_with(snap: &VoiceConfigSnapshot, act: Option<fn(&TurnBasedForm)>) -> String {
        SNAP.with(|s| *s.borrow_mut() = snap.clone());
        ACT.with(|a| a.set(act));

        fn app() -> Element {
            let snap = SNAP.with(|s| s.borrow().clone());
            let status = use_signal(VoiceStatusState::default);
            use_context_provider(|| status);
            let playing = use_signal(|| false);
            use_context_provider(|| AudioPlaybackActiveCtx(playing));
            let form = use_turn_based_form();
            form.seed(&snap);
            if let Some(act) = ACT.with(|a| a.get()) {
                act(&form);
            }
            let auto_tts = use_signal(|| false);
            rsx! {
                TurnBasedFamily {
                    form,
                    auto_tts,
                    hidden: false,
                    disabled: false,
                    tts_key_status: snap.tts_compat_key_status.clone(),
                }
            }
        }

        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    /// SSR escapes quotes and apostrophes in text nodes; match any spelling.
    fn has_text(html: &str, text: &str) -> bool {
        ["'", "&#39;", "&#x27;"]
            .iter()
            .any(|a| html.contains(text.replace('\'', a).as_str()))
    }

    fn tts_snapshot(provider: &str) -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            tts_provider: provider.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn tts_block_fields_per_provider() {
        // Edge: Voice + Format, no Model, no endpoint rows.
        let edge = render_with(&tts_snapshot("edge"), None);
        assert!(edge.contains("id=\"tts-voice-label\""), "{edge}");
        assert!(edge.contains("id=\"tts-format-select\""), "{edge}");
        assert!(
            !edge.contains("id=\"tts-model-label\""),
            "no Model row: {edge}"
        );
        assert!(!edge.contains("tts-compat-base-url"), "no Base URL: {edge}");
        assert!(edge.contains("▶ Test voice"), "{edge}");

        // ElevenLabs and OpenAI: Model + Voice + Format, no endpoint rows.
        for provider in ["elevenlabs", "openai"] {
            let html = render_with(&tts_snapshot(provider), None);
            for id in ["tts-model-label", "tts-voice-label", "tts-format-select"] {
                assert!(
                    html.contains(&format!("id=\"{id}\"")),
                    "{provider} {id}: {html}"
                );
            }
            assert!(!html.contains("tts-compat-base-url"), "{provider}: {html}");
            assert!(!html.contains("tts-compat-path"), "{provider}: {html}");
            assert!(!html.contains("tts-compat-key-ref"), "{provider}: {html}");
        }

        // OpenAI-compatible: the endpoint rows too.
        let compat = render_with(&tts_snapshot("openai_compatible"), None);
        for id in [
            "tts-compat-base-url",
            "tts-compat-path",
            "tts-compat-key-ref",
            "tts-model-label",
            "tts-voice-label",
            "tts-format-select",
        ] {
            assert!(compat.contains(&format!("id=\"{id}\"")), "{id}: {compat}");
        }
        assert!(
            compat.contains("placeholder=\"http://host:8880/v1\""),
            "{compat}"
        );
        assert!(compat.contains("placeholder=\"/audio/speech\""), "{compat}");
        assert!(
            compat.contains("NO KEY"),
            "empty reference reads NO KEY: {compat}"
        );
        assert!(
            has_text(&compat, "Enter the server's address to use this provider."),
            "an empty Base URL is an error: {compat}"
        );
        // The provider select still lists every registry provider.
        for spec in TTS_PROVIDERS {
            assert!(
                compat.contains(&format!("value=\"{}\"", spec.id)),
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn tts_form_payload_writes_compat_fields() {
        let snap = VoiceConfigSnapshot {
            tts_provider: "openai_compatible".to_string(),
            tts_compat_base_url: "http://host:8880/v1".to_string(),
            tts_compat_path: "/custom/speech".to_string(),
            tts_compat_key_ref: "VOICE_KEY_KOKORO".to_string(),
            tts_compat_model: "model-x".to_string(),
            tts_compat_voice: "voice-x".to_string(),
            tts_compat_format: "wav".to_string(),
            tts_edge_voice: "edge-voice-x".to_string(),
            tts_edge_format: "mp3".to_string(),
            tts_openai_model: "openai-model-x".to_string(),
            tts_openai_voice: "openai-voice-x".to_string(),
            tts_openai_format: "opus".to_string(),
            tts_elevenlabs_model: "eleven-model-x".to_string(),
            tts_elevenlabs_voice: "eleven-voice-x".to_string(),
            ..Default::default()
        };
        let rt = run_round_trip(snap);
        let p = &rt.payload;
        assert_eq!(p.tts_provider.as_deref(), Some("openai_compatible"));
        assert_eq!(
            p.tts_compat_base_url.as_deref(),
            Some("http://host:8880/v1")
        );
        assert_eq!(p.tts_compat_path.as_deref(), Some("/custom/speech"));
        assert_eq!(p.tts_compat_key_ref.as_deref(), Some("VOICE_KEY_KOKORO"));
        assert_eq!(p.tts_compat_model.as_deref(), Some("model-x"));
        assert_eq!(p.tts_compat_voice.as_deref(), Some("voice-x"));
        assert_eq!(p.tts_compat_format.as_deref(), Some("wav"));
        // Every other block rides from its OWN signals (D-09).
        assert_eq!(p.tts_edge_voice.as_deref(), Some("edge-voice-x"));
        assert_eq!(p.tts_edge_format.as_deref(), Some("mp3"));
        assert_eq!(p.tts_openai_model.as_deref(), Some("openai-model-x"));
        assert_eq!(p.tts_openai_voice.as_deref(), Some("openai-voice-x"));
        assert_eq!(p.tts_openai_format.as_deref(), Some("opus"));
        assert_eq!(p.tts_elevenlabs_model.as_deref(), Some("eleven-model-x"));
        assert_eq!(p.tts_elevenlabs_voice.as_deref(), Some("eleven-voice-x"));
        assert!(rt.clean_after_seed, "a freshly seeded form is not dirty");
        assert_eq!(
            rt.blocking, 0,
            "a complete compat block does not block saving"
        );
    }

    #[test]
    fn tts_test_settings_from_form() {
        let compat = VoiceConfigSnapshot {
            tts_provider: "openai_compatible".to_string(),
            tts_compat_base_url: "http://host:8880/v1".to_string(),
            tts_compat_model: "model-x".to_string(),
            tts_compat_voice: "voice-x".to_string(),
            tts_compat_format: "wav".to_string(),
            ..Default::default()
        };
        let s = observe(compat.clone(), |f| f.tts_test_settings());
        assert_eq!(s.provider.as_deref(), Some("openai_compatible"));
        assert_eq!(s.base_url.as_deref(), Some("http://host:8880/v1"));
        assert_eq!(s.path, None, "an empty path is None");
        assert_eq!(s.key_ref, None, "an empty key reference is None");
        assert_eq!(s.model.as_deref(), Some("model-x"));
        assert_eq!(s.voice.as_deref(), Some("voice-x"));
        assert_eq!(s.format.as_deref(), Some("wav"));

        let with_path = VoiceConfigSnapshot {
            tts_compat_path: "/custom/speech".to_string(),
            tts_compat_key_ref: "VOICE_KEY_KOKORO".to_string(),
            ..compat
        };
        let s = observe(with_path, |f| f.tts_test_settings());
        assert_eq!(s.path.as_deref(), Some("/custom/speech"));
        assert_eq!(s.key_ref.as_deref(), Some("VOICE_KEY_KOKORO"));

        // A provider without endpoint fields sends none, and Edge sends no model.
        let edge = VoiceConfigSnapshot {
            tts_provider: "edge".to_string(),
            tts_edge_voice: "edge-voice-x".to_string(),
            tts_compat_base_url: "http://host:8880/v1".to_string(),
            ..Default::default()
        };
        let s = observe(edge, |f| f.tts_test_settings());
        assert_eq!(s.provider.as_deref(), Some("edge"));
        assert_eq!(s.base_url, None);
        assert_eq!(s.model, None);
        assert_eq!(s.voice.as_deref(), Some("edge-voice-x"));
    }

    // ── Plan 12 Task 3: provider switch, legacy-tolerant Format, blocking ────

    use crate::components::hermes_app::voice_fields::ListCacheKey;
    use crate::voice_registry::ListStatus;

    /// A compat-provider snapshot whose values differ from every registry
    /// default, so a reset is observable.
    fn custom_compat_snapshot() -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            tts_provider: "openai_compatible".to_string(),
            tts_compat_base_url: "http://host:8880/v1".to_string(),
            tts_compat_path: "/custom/speech".to_string(),
            tts_compat_key_ref: "VOICE_KEY_KOKORO".to_string(),
            tts_compat_model: "model-x".to_string(),
            tts_compat_voice: "voice-x".to_string(),
            tts_compat_format: "wav".to_string(),
            tts_openai_model: "openai-model-x".to_string(),
            tts_openai_voice: "openai-voice-x".to_string(),
            tts_openai_format: "opus".to_string(),
            ..Default::default()
        }
    }

    fn registry_defaults(provider: &str) -> (String, String, String) {
        let spec = tts_provider(provider).expect("registry provider");
        (
            spec.default_model.to_string(),
            spec.default_voice.to_string(),
            spec.default_format.to_string(),
        )
    }

    #[test]
    fn tts_provider_switch_resets_to_registry_defaults() {
        type Snap = (
            String,
            (String, String, String),
            (String, String, String),
            [String; 3],
        );
        let triple = |m: Signal<String>, v: Signal<String>, f: Signal<String>| {
            (m.peek().clone(), v.peek().clone(), f.peek().clone())
        };
        let (after_openai, openai_vals, compat_vals, endpoint): Snap =
            observe(custom_compat_snapshot(), move |f| {
                // A cached list for the target provider must not survive the switch.
                let key = ListCacheKey::new("tts", "openai", "", "", "", OptionKind::Models);
                let mut cache = f.list_cache;
                cache
                    .write()
                    .insert(key.clone(), (ListStatus::Curated, Vec::new()));
                on_tts_provider_change(f, "openai");
                assert!(
                    !cache.peek().contains_key(&key),
                    "the switched-to provider's cached lists are cleared"
                );
                let openai = triple(f.tts_openai_model, f.tts_openai_voice, f.tts_openai_format);
                let provider = f.tts_provider.peek().clone();
                on_tts_provider_change(f, "openai_compatible");
                let compat = triple(f.tts_compat_model, f.tts_compat_voice, f.tts_compat_format);
                let endpoint = [
                    f.tts_compat_base_url.peek().clone(),
                    f.tts_compat_path.peek().clone(),
                    f.tts_compat_key_ref.peek().clone(),
                ];
                (provider, openai, compat, endpoint)
            });
        assert_eq!(after_openai, "openai");
        assert_eq!(openai_vals, registry_defaults("openai"));
        assert_eq!(
            compat_vals,
            registry_defaults("openai_compatible"),
            "switching back resets to the registry defaults, not the old values"
        );
        assert_eq!(
            endpoint,
            [
                "http://host:8880/v1".to_string(),
                "/custom/speech".to_string(),
                "VOICE_KEY_KOKORO".to_string()
            ],
            "Base URL, Path and key reference are kept across switches"
        );
    }

    #[test]
    fn tts_switch_note_shows_and_clears() {
        let note = "Switched to OpenAI: model, voice and format reset to its defaults.";
        let before = render_with(&custom_compat_snapshot(), None);
        assert!(!has_text(&before, "Switched to"), "{before}");

        let switched = render_with(
            &custom_compat_snapshot(),
            Some(|f| on_tts_provider_change(f, "openai")),
        );
        assert!(has_text(&switched, note), "{switched}");

        // The next edit in the TTS block clears it.
        let edited = render_with(
            &custom_compat_snapshot(),
            Some(|f| {
                on_tts_provider_change(f, "openai");
                let mut voice = f.tts_openai_voice;
                voice.set("another-voice".to_string());
            }),
        );
        assert!(!has_text(&edited, "Switched to"), "{edited}");
        assert!(
            edited.contains("another-voice"),
            "positive control: {edited}"
        );
    }

    /// The `<select id="tts-format-select">` element's markup.
    fn format_select(html: &str) -> &str {
        let at = html
            .find("id=\"tts-format-select\"")
            .unwrap_or_else(|| panic!("the Format select must render: {html}"));
        let start = html[..at].rfind("<select").expect("a select element");
        let end = at + html[at..].find("</select>").expect("a closed select");
        &html[start..end]
    }

    fn option_values(select: &str) -> Vec<String> {
        select
            .split("<option")
            .skip(1)
            .filter_map(|o| {
                let from = o.find("value=\"")? + "value=\"".len();
                Some(o[from..from + o[from..].find('"')?].to_string())
            })
            .collect()
    }

    /// The `<option ...>` markup whose value is `value`.
    fn option_markup<'a>(select: &'a str, value: &str) -> &'a str {
        let marker = format!("<option value=\"{value}\"");
        let at = select
            .find(&marker)
            .unwrap_or_else(|| panic!("option {value} missing: {select}"));
        let end = at + select[at..].find("</option>").expect("a closed option");
        &select[at..end]
    }

    #[test]
    fn tts_format_select_lists_only_provider_formats() {
        let edge = render_with(&tts_snapshot("edge"), None);
        assert_eq!(option_values(format_select(&edge)), vec!["mp3"]);
        let compat = render_with(&tts_snapshot("openai_compatible"), None);
        assert_eq!(
            option_values(format_select(&compat)),
            vec!["mp3", "opus", "wav"]
        );
        let eleven = render_with(&tts_snapshot("elevenlabs"), None);
        assert_eq!(option_values(format_select(&eleven)), vec!["mp3"]);
    }

    #[test]
    fn format_select_normalizes_legacy_value() {
        // Edge: the pre-54.7 free-text value (assembled so this file never
        // contains the placeholder literal).
        let legacy_edge = ["audio-24khz-48kbitrate", "-mono-mp3"].concat();
        let snap = VoiceConfigSnapshot {
            tts_provider: "edge".to_string(),
            tts_edge_format: legacy_edge.clone(),
            ..Default::default()
        };
        let html = render_with(&snap, None);
        let select = format_select(&html);
        assert!(
            option_markup(select, "mp3").contains("selected"),
            "the select shows the provider's default format: {select}"
        );
        let expected = legacy_edge.clone();
        let (dirty, payload) = observe(snap.clone(), move |f| {
            let mut payload = VoiceWritePayload::default();
            f.apply_to(&mut payload);
            (f.is_dirty(&snap), payload)
        });
        assert!(!dirty, "an untouched legacy value is not a change");
        assert_eq!(
            payload.tts_edge_format.as_deref(),
            Some(expected.as_str()),
            "the raw stored value round-trips unchanged"
        );

        // OpenAI: a hand-typed value outside the provider's list.
        let snap = VoiceConfigSnapshot {
            tts_provider: "openai".to_string(),
            tts_openai_format: "flac".to_string(),
            ..Default::default()
        };
        let html = render_with(&snap, None);
        let select = format_select(&html);
        assert!(
            option_markup(select, "mp3").contains("selected"),
            "{select}"
        );
        assert!(
            !option_markup(select, "wav").contains("selected"),
            "{select}"
        );
        let (dirty, payload) = observe(snap.clone(), move |f| {
            let mut payload = VoiceWritePayload::default();
            f.apply_to(&mut payload);
            (f.is_dirty(&snap), payload)
        });
        assert!(!dirty);
        assert_eq!(payload.tts_openai_format.as_deref(), Some("flac"));

        // A listed value is shown as stored.
        let snap = VoiceConfigSnapshot {
            tts_provider: "openai_compatible".to_string(),
            tts_compat_format: "wav".to_string(),
            ..Default::default()
        };
        let html = render_with(&snap, None);
        let select = format_select(&html);
        assert!(
            option_markup(select, "wav").contains("selected"),
            "{select}"
        );
        assert!(
            !option_markup(select, "mp3").contains("selected"),
            "{select}"
        );
    }

    #[test]
    fn edge_format_free_text_placeholder_is_gone() {
        let src = include_str!("voice_turn_based.rs");
        let needle = ["audio-24khz-48kbitrate", "-mono-mp3"].concat();
        assert!(
            !src.contains(needle.as_str()),
            "the Edge free-text format placeholder must not come back"
        );
    }

    #[test]
    fn tts_blocking_errors_counted() {
        let valid = custom_compat_snapshot();
        let count = |snap: VoiceConfigSnapshot| observe(snap, |f| f.blocking_error_count());
        assert_eq!(count(valid.clone()), 0, "a complete compat block is clean");
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_compat_base_url: String::new(),
                ..valid.clone()
            }),
            1,
            "an empty Base URL on openai_compatible blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_compat_model: "a\"b".to_string(),
                ..valid.clone()
            }),
            1,
            "a model outside the charset blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_compat_key_ref: "OPENAI_API_KEY".to_string(),
                ..valid.clone()
            }),
            1,
            "a key reference outside the voice namespace blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_compat_voice: "bad voice".to_string(),
                tts_compat_path: "no-slash".to_string(),
                ..valid.clone()
            }),
            2,
            "voice and path errors count separately"
        );
        // Only the selected provider's fields count: the same bad compat Base
        // URL does not block while OpenAI is selected.
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_provider: "openai".to_string(),
                tts_compat_base_url: String::new(),
                ..valid
            }),
            0
        );
    }

    // ── Plan 13 Task 1: registry-driven STT block + Test mic ─────────────────

    fn stt_snapshot(provider: &str) -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            stt_provider: provider.to_string(),
            tts_provider: "edge".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn stt_block_fields_per_provider() {
        // Auto: Provider + the auto-select hint + Test mic; no Model, no endpoint rows.
        let auto = render_with(&stt_snapshot("auto"), None);
        assert!(
            has_text(&auto, "Hermes selects the provider automatically."),
            "{auto}"
        );
        assert!(auto.contains("● Test mic"), "{auto}");
        assert!(auto.contains("id=\"stt-provider-select\""), "{auto}");
        assert!(
            !auto.contains("id=\"stt-model-label\""),
            "no Model row for Auto: {auto}"
        );
        assert!(!auto.contains("stt-compat-base-url"), "{auto}");

        // Groq and OpenAI: Model + Test mic, no endpoint rows, no auto hint.
        for provider in ["groq", "openai"] {
            let html = render_with(&stt_snapshot(provider), None);
            assert!(
                html.contains("id=\"stt-model-label\""),
                "{provider}: {html}"
            );
            assert!(html.contains("● Test mic"), "{provider}: {html}");
            assert!(!html.contains("stt-compat-base-url"), "{provider}: {html}");
            assert!(!html.contains("stt-compat-path"), "{provider}: {html}");
            assert!(!html.contains("stt-compat-key-ref"), "{provider}: {html}");
            assert!(
                !has_text(&html, "Hermes selects the provider automatically."),
                "{provider}: {html}"
            );
        }

        // OpenAI-compatible: the endpoint rows, Model and Test mic.
        let compat = render_with(&stt_snapshot("openai_compatible"), None);
        for id in [
            "stt-compat-base-url",
            "stt-compat-path",
            "stt-compat-key-ref",
            "stt-model-label",
        ] {
            assert!(compat.contains(&format!("id=\"{id}\"")), "{id}: {compat}");
        }
        assert!(
            compat.contains("placeholder=\"http://host:8880/v1\""),
            "{compat}"
        );
        assert!(
            compat.contains("placeholder=\"/audio/transcriptions\""),
            "{compat}"
        );
        assert!(compat.contains("NO KEY"), "{compat}");
        assert!(compat.contains("● Test mic"), "{compat}");
        assert!(
            has_text(&compat, "Enter the server's address to use this provider."),
            "an empty Base URL is an error: {compat}"
        );
        // The provider select lists every registry provider.
        for spec in STT_PROVIDERS {
            assert!(
                compat.contains(&format!("value=\"{}\"", spec.id)),
                "{}",
                spec.id
            );
        }
    }

    fn whisper_cpp_snapshot() -> VoiceConfigSnapshot {
        VoiceConfigSnapshot {
            stt_provider: "openai_compatible".to_string(),
            stt_compat_base_url: "http://host:8178".to_string(),
            stt_compat_path: "/inference".to_string(),
            stt_compat_key_ref: "VOICE_KEY_WHISPER".to_string(),
            stt_compat_model: "whisper-large-v3".to_string(),
            stt_groq_model: "groq-model-x".to_string(),
            stt_openai_model: "openai-model-x".to_string(),
            tts_provider: "edge".to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn stt_form_payload_writes_compat_fields() {
        let rt = run_round_trip(whisper_cpp_snapshot());
        let p = &rt.payload;
        assert_eq!(p.stt_provider.as_deref(), Some("openai_compatible"));
        assert_eq!(p.stt_compat_base_url.as_deref(), Some("http://host:8178"));
        assert_eq!(p.stt_compat_path.as_deref(), Some("/inference"));
        assert_eq!(p.stt_compat_key_ref.as_deref(), Some("VOICE_KEY_WHISPER"));
        assert_eq!(p.stt_compat_model.as_deref(), Some("whisper-large-v3"));
        // Every other STT block rides from its OWN signal (D-09).
        assert_eq!(p.stt_groq_model.as_deref(), Some("groq-model-x"));
        assert_eq!(p.stt_openai_model.as_deref(), Some("openai-model-x"));
        assert!(rt.clean_after_seed, "a freshly seeded form is not dirty");
        assert_eq!(rt.blocking, 0, "a complete STT compat block is clean");

        // Test mic auditions the UNSAVED values: the form mirrors them.
        let s = observe(whisper_cpp_snapshot(), |f| f.stt_test_settings());
        assert_eq!(s.provider.as_deref(), Some("openai_compatible"));
        assert_eq!(s.base_url.as_deref(), Some("http://host:8178"));
        assert_eq!(s.path.as_deref(), Some("/inference"));
        assert_eq!(s.key_ref.as_deref(), Some("VOICE_KEY_WHISPER"));
        assert_eq!(s.model.as_deref(), Some("whisper-large-v3"));

        // An empty path / key reference is None; Groq sends its own model and
        // no endpoint fields; Auto sends the provider only.
        let bare = VoiceConfigSnapshot {
            stt_compat_path: String::new(),
            stt_compat_key_ref: String::new(),
            ..whisper_cpp_snapshot()
        };
        let s = observe(bare, |f| f.stt_test_settings());
        assert_eq!(s.path, None);
        assert_eq!(s.key_ref, None);
        let groq = VoiceConfigSnapshot {
            stt_provider: "groq".to_string(),
            ..whisper_cpp_snapshot()
        };
        let s = observe(groq, |f| f.stt_test_settings());
        assert_eq!(s.provider.as_deref(), Some("groq"));
        assert_eq!(s.model.as_deref(), Some("groq-model-x"));
        assert_eq!(s.base_url, None, "Groq has no endpoint fields");
        let auto = VoiceConfigSnapshot {
            stt_provider: "auto".to_string(),
            ..whisper_cpp_snapshot()
        };
        let s = observe(auto, |f| f.stt_test_settings());
        assert_eq!(s.provider.as_deref(), Some("auto"));
        assert_eq!(s.model, None);
        assert_eq!(s.base_url, None);
    }

    // ── Plan 13 Task 2: provider switch, model fetch, blocking errors ────────

    fn stt_registry_default(provider: &str) -> String {
        stt_provider(provider)
            .expect("registry STT provider")
            .default_model
            .to_string()
    }

    #[test]
    fn stt_provider_switch_resets_model() {
        type Seen = (String, String, String, [String; 3]);
        let (provider, groq_model, compat_model, endpoint): Seen =
            observe(whisper_cpp_snapshot(), move |f| {
                // A cached list for the target provider must not survive the switch.
                let key = ListCacheKey::new("stt", "groq", "", "", "", OptionKind::Models);
                let mut cache = f.list_cache;
                cache
                    .write()
                    .insert(key.clone(), (ListStatus::Curated, Vec::new()));
                on_stt_provider_change(f, "groq");
                assert!(
                    !cache.peek().contains_key(&key),
                    "the switched-to provider's cached lists are cleared"
                );
                let provider = f.stt_provider.peek().clone();
                let groq = f.stt_groq_model.peek().clone();
                on_stt_provider_change(f, "openai_compatible");
                let compat = f.stt_compat_model.peek().clone();
                let endpoint = [
                    f.stt_compat_base_url.peek().clone(),
                    f.stt_compat_path.peek().clone(),
                    f.stt_compat_key_ref.peek().clone(),
                ];
                (provider, groq, compat, endpoint)
            });
        assert_eq!(provider, "groq");
        assert_eq!(groq_model, stt_registry_default("groq"));
        assert_ne!(groq_model, "groq-model-x", "the stored model was replaced");
        assert_eq!(
            compat_model,
            stt_registry_default("openai_compatible"),
            "switching back resets to the registry default, not whisper-large-v3"
        );
        assert_eq!(
            endpoint,
            [
                "http://host:8178".to_string(),
                "/inference".to_string(),
                "VOICE_KEY_WHISPER".to_string()
            ],
            "Base URL, Path and key reference are kept across switches"
        );

        // The switch note shows, and the next STT edit clears it.
        let note = "Switched to Groq: model, voice and format reset to its defaults.";
        let before = render_with(&whisper_cpp_snapshot(), None);
        assert!(!has_text(&before, "Switched to"), "{before}");
        let switched = render_with(
            &whisper_cpp_snapshot(),
            Some(|f| on_stt_provider_change(f, "groq")),
        );
        assert!(has_text(&switched, note), "{switched}");
        let edited = render_with(
            &whisper_cpp_snapshot(),
            Some(|f| {
                on_stt_provider_change(f, "groq");
                let mut model = f.stt_groq_model;
                model.set("another-model".to_string());
            }),
        );
        assert!(!has_text(&edited, "Switched to"), "{edited}");
        assert!(
            edited.contains("another-model"),
            "positive control: {edited}"
        );
        // Auto has no model, so nothing was reset and no note is shown.
        let auto = render_with(
            &whisper_cpp_snapshot(),
            Some(|f| on_stt_provider_change(f, "auto")),
        );
        assert!(!has_text(&auto, "Switched to"), "{auto}");
        assert!(
            has_text(&auto, "Hermes selects the provider automatically."),
            "positive control: {auto}"
        );
    }

    #[test]
    fn stt_model_picker_fetches_for_compat_only() {
        // OpenAI-compatible with a valid Base URL and nothing cached: a fetch is
        // due, so the picker shows Loading and offers Refresh.
        let compat = render_with(&whisper_cpp_snapshot(), None);
        assert!(has_text(&compat, "Loading models…"), "{compat}");
        assert!(compat.contains("aria-busy=\"true\""), "{compat}");
        assert!(compat.contains("↻ Refresh models"), "{compat}");

        // Groq stays on the curated list: no fetch, no Refresh.
        let groq = render_with(
            &VoiceConfigSnapshot {
                stt_provider: "groq".to_string(),
                ..whisper_cpp_snapshot()
            },
            None,
        );
        assert!(!has_text(&groq, "Loading models…"), "{groq}");
        assert!(!groq.contains("Refresh models"), "{groq}");
        assert!(
            has_text(&groq, "Built-in models for Groq. Type any value to use it."),
            "{groq}"
        );

        // An unusable Base URL never fetches.
        let no_base = render_with(
            &VoiceConfigSnapshot {
                stt_compat_base_url: String::new(),
                ..whisper_cpp_snapshot()
            },
            None,
        );
        assert!(!has_text(&no_base, "Loading models…"), "{no_base}");
        assert!(
            has_text(
                &no_base,
                "Built-in models for OpenAI-compatible. Type any value to use it."
            ),
            "{no_base}"
        );

        // whisper.cpp answers 404 on /models: the loaded-empty line shows and
        // the typed model is accepted as it is.
        let listed_empty = render_with(
            &whisper_cpp_snapshot(),
            Some(|f| {
                let key = ListCacheKey::new(
                    "stt",
                    "openai_compatible",
                    "http://host:8178",
                    "/inference",
                    "VOICE_KEY_WHISPER",
                    OptionKind::Models,
                );
                let mut cache = f.list_cache;
                cache
                    .write()
                    .insert(key, (ListStatus::LoadedEmpty, Vec::new()));
            }),
        );
        assert!(
            has_text(
                &listed_empty,
                "This server didn't list any models. Type one to use it."
            ),
            "{listed_empty}"
        );
        assert!(listed_empty.contains("whisper-large-v3"), "{listed_empty}");
    }

    #[test]
    fn stt_blocking_errors_counted() {
        let valid = whisper_cpp_snapshot();
        let count = |snap: VoiceConfigSnapshot| observe(snap, |f| f.blocking_error_count());
        assert_eq!(count(valid.clone()), 0, "a complete compat block is clean");
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_compat_base_url: String::new(),
                ..valid.clone()
            }),
            1,
            "an empty Base URL on openai_compatible blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_compat_path: "inference".to_string(),
                ..valid.clone()
            }),
            1,
            "a path without a leading slash blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_compat_key_ref: "GROQ_API_KEY".to_string(),
                ..valid.clone()
            }),
            1,
            "a key reference outside the voice namespace blocks"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_compat_model: "a\"b".to_string(),
                ..valid.clone()
            }),
            1,
            "a model outside the charset blocks"
        );
        // Only the selected provider's fields count.
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_provider: "groq".to_string(),
                stt_compat_base_url: String::new(),
                ..valid.clone()
            }),
            0,
            "the compat Base URL does not block while Groq is selected"
        );
        assert_eq!(
            count(VoiceConfigSnapshot {
                stt_provider: "auto".to_string(),
                stt_compat_base_url: String::new(),
                ..valid.clone()
            }),
            0
        );
        let bad_groq_model = observe(
            VoiceConfigSnapshot {
                stt_provider: "groq".to_string(),
                ..valid.clone()
            },
            |f| {
                let mut model = f.stt_groq_model;
                model.set("a\"b".to_string());
                f.blocking_error_count()
            },
        );
        assert_eq!(
            bad_groq_model, 1,
            "the selected provider's model is checked"
        );
        // The save bar's count sums the TTS and the STT block.
        assert_eq!(
            count(VoiceConfigSnapshot {
                tts_provider: "openai_compatible".to_string(),
                tts_compat_base_url: String::new(),
                stt_compat_base_url: String::new(),
                ..valid
            }),
            2
        );
    }
}
