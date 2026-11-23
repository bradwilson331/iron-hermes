//! Phase 54.7 Plan 18 (D-04/D-06/D-07/D-08) -- the voice panel's shared field
//! components.
//!
//! `OptionPickerField` is a model/voice combobox fed by the server: it asks
//! `list_voice_options` for the list (the server resolves any key reference and
//! calls the target -- the browser never contacts a voice server and never sees
//! a key, D-08), caches answers per session, drops stale answers by request
//! token, and degrades to the curated registry list on any failure (D-07).
//!
//! Caches are form-owned signals handed in as a prop; this file provides no
//! context (a child provider panics ancestor consumers at runtime).

// Web-only component tree: native dead-code analysis cannot reach the render
// sites (same rationale as voice_settings.rs). The wasm build lints normally.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]

use std::collections::HashMap;

use dioxus::prelude::*;

use crate::components::hermes_app::screens::model_picker::ModelPickerField;
use crate::server::voice_api::{list_voice_options, VoiceListRequest, VoiceListResult};
use crate::voice_registry::{
    key_pill, list_status_copy, realtime_provider, stt_provider, tts_provider, validate_base_url,
    validate_endpoint_path, validate_key_ref, ListStatus, OptionKind, ProviderSpec,
    CATEGORY_SERVER_UNREACHABLE, CATEGORY_UNEXPECTED_RESPONSE, ERROR_CATEGORIES, KEY_REF_HINT,
};

/// How long the Base URL / Path / key reference must sit still before a fetch
/// starts (UI-SPEC default).
pub const LIST_DEBOUNCE_MS: u32 = 500;

/// Everything that distinguishes one fetched list from another (UI-SPEC cache
/// key): two keys differing in any input never share an entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ListCacheKey {
    pub family: String,
    pub provider: String,
    pub base_url: String,
    pub path: String,
    pub key_ref: String,
    /// Wire form of the [`OptionKind`]: `models` or `voices`.
    pub kind: String,
    /// WR-04: the model a voice list was requested for (`GET .../audio/voices?model=`).
    /// Empty for every list that is not model-specific, so those keys are
    /// unchanged. Set only through [`ListCacheKey::with_model`].
    pub model: String,
}

impl ListCacheKey {
    pub fn new(
        family: &str,
        provider: &str,
        base_url: &str,
        path: &str,
        key_ref: &str,
        kind: OptionKind,
    ) -> Self {
        Self {
            family: family.trim().to_string(),
            provider: provider.trim().to_string(),
            base_url: base_url.trim().to_string(),
            path: path.trim().to_string(),
            key_ref: key_ref.trim().to_string(),
            kind: kind_wire(kind).to_string(),
            model: String::new(),
        }
    }

    /// Scope a VOICES key to the model it was listed for. A model-specific voice
    /// list (a custom endpoint's `/audio/voices?model=`) must never be served from
    /// another model's entry. Ignored for model lists and for providers whose
    /// voice list does not depend on the model, so they keep one shared entry.
    pub fn with_model(mut self, model: &str) -> Self {
        let model_specific = self.kind == kind_wire(OptionKind::Voices)
            && provider_voices_depend_on_model(&self.family, &self.provider);
        if model_specific {
            self.model = model.trim().to_string();
        }
        self
    }
}

/// Whether `provider`'s voice list is requested per model: only a custom
/// endpoint (the server forwards `?model=` for it alone).
fn provider_voices_depend_on_model(family: &str, provider: &str) -> bool {
    let spec = match family {
        "tts" => tts_provider(provider),
        "stt" => stt_provider(provider),
        "realtime" => realtime_provider(provider),
        _ => None,
    };
    spec.is_some_and(|spec| spec.endpoint_fields)
}

/// The wire `kind` of a picker.
fn kind_wire(kind: OptionKind) -> &'static str {
    match kind {
        OptionKind::Models => "models",
        OptionKind::Voices => "voices",
    }
}

/// Per-session list cache, owned by a family form (never a context).
pub type ListCache = Signal<HashMap<ListCacheKey, (ListStatus, Vec<String>)>>;

/// Map the server's answer onto a [`ListStatus`]. A failure category outside
/// the fixed vocabulary becomes `unexpected response`, so server-supplied text
/// can never reach the DOM through the status line (T-54.7-52).
pub fn list_result_to_status(result: &VoiceListResult) -> ListStatus {
    match result.status.as_str() {
        "loaded" if result.items.is_empty() => ListStatus::LoadedEmpty,
        "loaded" => ListStatus::Loaded(result.items.len()),
        "empty" => ListStatus::LoadedEmpty,
        "curated" => ListStatus::Curated,
        "failed" => {
            let category = result
                .category
                .as_deref()
                .and_then(|c| ERROR_CATEGORIES.iter().find(|known| **known == c).copied())
                .unwrap_or(CATEGORY_UNEXPECTED_RESPONSE);
            ListStatus::Failed(category)
        }
        _ => ListStatus::Failed(CATEGORY_UNEXPECTED_RESPONSE),
    }
}

/// A response is applied only when no newer request has started since.
pub fn accept_response(latest_token: u64, response_token: u64) -> bool {
    latest_token == response_token
}

/// The rows a picker offers: the server's list when one loaded, nothing when
/// the server listed nothing, the curated registry list otherwise.
pub fn displayed_options(
    status: &ListStatus,
    fetched: &[String],
    curated: &[String],
) -> Vec<String> {
    match status {
        ListStatus::Loaded(_) => fetched.to_vec(),
        ListStatus::LoadedEmpty => Vec::new(),
        ListStatus::Curated | ListStatus::Loading | ListStatus::Failed(_) => curated.to_vec(),
    }
}

/// What a picker shows right now: a cached answer, else `Loading` when a fetch
/// is due, else the curated list.
fn picker_state(
    entry: Option<(ListStatus, Vec<String>)>,
    eligible: bool,
) -> (ListStatus, Vec<String>) {
    match entry {
        Some(found) => found,
        None if eligible => (ListStatus::Loading, Vec::new()),
        None => (ListStatus::Curated, Vec::new()),
    }
}

/// A list may be fetched only while its family is on screen and, for a provider
/// with endpoint fields, only once the Base URL is usable (UI-SPEC: a hidden
/// family never polls a LAN server, an invalid address never leaves the
/// browser). A Refresh bypasses the cache but not this rule.
pub fn fetch_eligible(visible: bool, spec: &ProviderSpec, base_url: &str) -> bool {
    visible && (!spec.endpoint_fields || validate_base_url(base_url, true).is_ok())
}

/// [`fetch_eligible`] for a picker: also requires that the picker has a fetch
/// path at all and that its provider is a registry provider of its family.
fn picker_eligible(
    family: &str,
    provider: &str,
    fetchable: bool,
    visible: bool,
    base_url: &str,
) -> bool {
    if !visible || !fetchable {
        return false;
    }
    let spec = match family {
        "tts" => tts_provider(provider),
        "stt" => stt_provider(provider),
        "realtime" => realtime_provider(provider),
        _ => None,
    };
    spec.is_some_and(|spec| fetch_eligible(visible, spec, base_url))
}

fn non_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The server request for `key`; blank inputs go over the wire as `None`.
fn request_for(key: &ListCacheKey, model: &str) -> VoiceListRequest {
    VoiceListRequest {
        family: key.family.clone(),
        provider: key.provider.clone(),
        kind: key.kind.clone(),
        base_url: non_blank(&key.base_url),
        path: non_blank(&key.path),
        key_ref: non_blank(&key.key_ref),
        model: non_blank(model),
    }
}

/// Delay before a fetch: keystrokes in Base URL / Path / key reference are
/// debounced, every other trigger (mount, provider change, first visibility)
/// starts at once.
fn debounce_ms(previous: Option<&ListCacheKey>, next: &ListCacheKey) -> u32 {
    match previous {
        Some(prev)
            if prev.family == next.family
                && prev.provider == next.provider
                && prev.kind == next.kind
                && (prev.base_url != next.base_url
                    || prev.path != next.path
                    || prev.key_ref != next.key_ref
                    || prev.model != next.model) =>
        {
            LIST_DEBOUNCE_MS
        }
        _ => 0,
    }
}

/// Start one fetch under a fresh request token. Any earlier in-flight fetch of
/// this picker is superseded: its answer fails [`accept_response`].
fn start_fetch(
    mut cache: ListCache,
    mut token: Signal<u64>,
    key: ListCacheKey,
    request: VoiceListRequest,
    delay_ms: u32,
) {
    let mine = token.peek().wrapping_add(1);
    token.set(mine);
    spawn(async move {
        if delay_ms > 0 {
            gloo_timers::future::TimeoutFuture::new(delay_ms).await;
            let latest = *token.peek();
            if !accept_response(latest, mine) {
                return;
            }
        }
        let outcome = list_voice_options(request).await;
        let latest = *token.peek();
        if !accept_response(latest, mine) {
            return;
        }
        let entry = match outcome {
            Ok(result) => (list_result_to_status(&result), result.items),
            // The call to our own server failed: the same fixed category.
            Err(_) => (ListStatus::Failed(CATEGORY_SERVER_UNREACHABLE), Vec::new()),
        };
        cache.write().insert(key, entry);
    });
}

/// Supersede any in-flight fetch without starting another.
fn cancel_fetch(mut token: Signal<u64>) {
    let next = token.peek().wrapping_add(1);
    token.set(next);
}

/// A model/voice combobox backed by the server-fetched list (D-06/D-07/D-08).
///
/// `fetchable` says whether this picker has a fetch path at all (a realtime
/// voice list does not); `visible` is whether its family is on screen -- a
/// hidden family never fetches. Custom typed values always commit.
#[component]
pub fn OptionPickerField(
    label: String,
    field_id: String,
    kind: OptionKind,
    family: String,
    provider: String,
    provider_label: String,
    value: Signal<String>,
    curated: Vec<String>,
    base_url: String,
    path: String,
    key_ref: String,
    model_for_voices: String,
    fetchable: bool,
    visible: bool,
    disabled: bool,
    cache: ListCache,
    seeded: String,
    placeholder: String,
    title: String,
    #[props(default = "model id".to_string())] noun: String,
    // A typed-value validation error, shown beside the list status.
    #[props(default)] error: Option<String>,
) -> Element {
    let token = use_signal(|| 0u64);
    let prev_key = use_signal(|| None::<ListCacheKey>);

    let key = ListCacheKey::new(&family, &provider, &base_url, &path, &key_ref, kind)
        .with_model(&model_for_voices);
    let eligible = picker_eligible(&family, &provider, fetchable, visible, &base_url);
    // Pattern B: the cache borrow ends at this statement's `;`.
    let entry = cache.read().get(&key).cloned();
    let (status, fetched) = picker_state(entry, eligible);
    let options = displayed_options(&status, &fetched, &curated);
    let busy = status == ListStatus::Loading;
    let status_text = list_status_copy(&status, kind, &provider_label);
    let plural = match kind {
        OptionKind::Models => "models",
        OptionKind::Voices => "voices",
    };

    // Mount, provider change, Base URL / Path / key reference change and the
    // family becoming visible all land here: the plain props are declared as
    // reactive deps (a bare effect would run once).
    use_effect(use_reactive!(|key, eligible, model_for_voices| {
        let previous = prev_key.peek().clone();
        let mut remember = prev_key;
        remember.set(Some(key.clone()));
        if !eligible || cache.peek().contains_key(&key) {
            cancel_fetch(token);
            return;
        }
        let delay = debounce_ms(previous.as_ref(), &key);
        let request = request_for(&key, &model_for_voices);
        start_fetch(cache, token, key, request, delay);
    }));

    // Refresh bypasses the cache and the debounce, never the eligibility rule.
    let refresh_key = key.clone();
    let refresh_model = model_for_voices.clone();
    let on_refresh = move |_| {
        if !eligible {
            return;
        }
        let mut entries = cache;
        entries.write().remove(&refresh_key);
        let request = request_for(&refresh_key, &refresh_model);
        start_fetch(cache, token, refresh_key.clone(), request, 0);
    };

    rsx! {
        VoiceField {
            label,
            field_id: field_id.clone(),
            group: true,
            busy,
            status: Some((status_text, false)),
            head_extra: rsx! {
                if fetchable {
                    button {
                        class: "voice-refresh-btn",
                        r#type: "button",
                        disabled: disabled || !eligible,
                        "aria-label": "Refresh {plural} from your server",
                        onclick: on_refresh,
                        "↻ Refresh {plural}"
                    }
                }
            },
            ModelPickerField {
                value,
                all_options: options,
                seeded,
                disabled,
                placeholder,
                title,
                noun,
            }
            if let Some(message) = error {
                p {
                    id: "{field_id}-error",
                    class: "voice-field-status is-error",
                    role: "status",
                    "aria-live": "polite",
                    "{message}"
                }
            }
        }
    }
}

/// The UI-SPEC stacked field row: label (with an optional extra in the head),
/// the control, an optional one-line hint and an optional status line
/// (`.is-error` when the flag is set). `group` switches the label from `for=`
/// to `role="group"` + `aria-labelledby` for composite controls.
#[component]
pub fn VoiceField(
    label: String,
    field_id: String,
    #[props(default = VNode::empty())] head_extra: Element,
    #[props(default)] hint: Option<String>,
    #[props(default)] status: Option<(String, bool)>,
    #[props(default)] group: bool,
    #[props(default)] busy: bool,
    children: Element,
) -> Element {
    let labelled_by = group.then(|| format!("{field_id}-label"));
    let for_id = (!group).then(|| field_id.clone());
    rsx! {
        div {
            class: "voice-field",
            role: if group { "group" },
            "aria-labelledby": labelled_by,
            "aria-busy": if busy { "true" },
            div { class: "voice-field-head",
                label {
                    id: "{field_id}-label",
                    r#for: for_id,
                    class: "voice-settings-label",
                    "{label}"
                }
                {head_extra}
            }
            {children}
            if let Some(text) = hint {
                p {
                    id: "{field_id}-hint",
                    class: "voice-settings-hint voice-field-hint",
                    "{text}"
                }
            }
            if let Some((text, is_error)) = status {
                p {
                    id: "{field_id}-status",
                    class: if is_error { "voice-field-status is-error" } else { "voice-field-status" },
                    role: "status",
                    "aria-live": "polite",
                    "{text}"
                }
            }
        }
    }
}

/// The key-reference status pill: FOUND / NOT FOUND / NO KEY from the server's
/// status string. It never renders the reference or a key (D-02).
#[component]
pub fn KeyRefStatusPill(status: String) -> Element {
    let (text, modifier) = key_pill(&status);
    rsx! {
        span { class: "voice-status-pill {modifier}", "{text}" }
    }
}

/// `aria-describedby` for a field: its hint and/or its status line.
fn described_by(field_id: &str, has_hint: bool, has_status: bool) -> Option<String> {
    let mut ids = Vec::new();
    if has_hint {
        ids.push(format!("{field_id}-hint"));
    }
    if has_status {
        ids.push(format!("{field_id}-status"));
    }
    (!ids.is_empty()).then(|| ids.join(" "))
}

/// The `--danger` border of an invalid input (UI-SPEC), as an inline style so
/// no stylesheet outside this plan has to know about it.
const INVALID_BORDER: &str = "border-color: var(--danger);";

/// Base URL, optional Path and optional API key reference rows with instant
/// validation (no debounce; the server re-validates, D-04). LAN, private and
/// loopback hosts get no warning. `id_prefix` keeps ids unique per block. The
/// key reference is a plain text input: it names a secret, never holds one.
#[component]
pub fn EndpointFields(
    base_url: Signal<String>,
    #[props(default)] path: Option<Signal<String>>,
    #[props(default)] key_ref: Option<Signal<String>>,
    key_status: String,
    default_path: String,
    base_placeholder: String,
    #[props(default)] base_hint: Option<String>,
    required: bool,
    disabled: bool,
    #[props(default)] no_key_hint: Option<String>,
    id_prefix: String,
    #[props(default)] on_edit: Option<EventHandler<()>>,
) -> Element {
    // Pattern B: owned locals; every borrow ends at its `;`.
    let base_val = base_url.read().clone();
    let base_err = validate_base_url(base_val.trim(), required).err();
    let base_id = format!("{id_prefix}-base-url");
    let base_describedby = described_by(&base_id, base_hint.is_some(), base_err.is_some());

    let path_row = path.map(|sig| {
        let val = sig.read().clone();
        let err = validate_endpoint_path(&val).err();
        (sig, val, err)
    });
    let key_row = key_ref.map(|sig| {
        let val = sig.read().clone();
        let err = validate_key_ref(val.trim()).err();
        (sig, val, err)
    });

    let notify = move || {
        if let Some(handler) = on_edit.as_ref() {
            handler.call(());
        }
    };

    rsx! {
        VoiceField {
            label: "Base URL".to_string(),
            field_id: base_id.clone(),
            hint: base_hint,
            status: base_err.map(|e| (e.copy().to_string(), true)),
            input {
                id: "{base_id}",
                r#type: "text",
                class: "voice-wake-phrase-input",
                style: if base_err.is_some() { INVALID_BORDER },
                disabled,
                placeholder: "{base_placeholder}",
                value: "{base_val}",
                "aria-invalid": if base_err.is_some() { "true" } else { "false" },
                "aria-describedby": base_describedby,
                autocomplete: "off",
                spellcheck: "false",
                oninput: move |evt| {
                    notify();
                    let mut sig = base_url;
                    sig.set(evt.value());
                },
            }
        }
        if let Some((sig, val, err)) = path_row {
            {
                let path_id = format!("{id_prefix}-path");
                let describedby = described_by(&path_id, true, err.is_some());
                rsx! {
                    VoiceField {
                        label: "Path".to_string(),
                        field_id: path_id.clone(),
                        hint: Some("Added to the base URL. Leave empty to use the standard path.".to_string()),
                        status: err.map(|e| (e.copy().to_string(), true)),
                        input {
                            id: "{path_id}",
                            r#type: "text",
                            class: "voice-wake-phrase-input",
                            style: if err.is_some() { INVALID_BORDER },
                            disabled,
                            placeholder: "{default_path}",
                            value: "{val}",
                            "aria-invalid": if err.is_some() { "true" } else { "false" },
                            "aria-describedby": describedby,
                            autocomplete: "off",
                            spellcheck: "false",
                            oninput: move |evt| {
                                notify();
                                let mut sig = sig;
                                sig.set(evt.value());
                            },
                        }
                    }
                }
            }
        }
        if let Some((sig, val, err)) = key_row {
            {
                let key_id = format!("{id_prefix}-key-ref");
                let describedby = described_by(&key_id, true, err.is_some());
                let hint = match (&no_key_hint, val.trim().is_empty()) {
                    (Some(extra), true) => format!("{KEY_REF_HINT} {extra}"),
                    _ => KEY_REF_HINT.to_string(),
                };
                rsx! {
                    VoiceField {
                        label: "API key reference".to_string(),
                        field_id: key_id.clone(),
                        head_extra: rsx! { KeyRefStatusPill { status: key_status.clone() } },
                        hint: Some(hint),
                        status: err.map(|e| (e.copy().to_string(), true)),
                        input {
                            id: "{key_id}",
                            r#type: "text",
                            class: "voice-wake-phrase-input",
                            style: if err.is_some() { INVALID_BORDER },
                            disabled,
                            value: "{val}",
                            "aria-invalid": if err.is_some() { "true" } else { "false" },
                            "aria-describedby": describedby,
                            autocomplete: "off",
                            spellcheck: "false",
                            oninput: move |evt| {
                                notify();
                                let mut sig = sig;
                                sig.set(evt.value());
                            },
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::server::voice_api::VoiceListResult;
    use crate::voice_registry::{ListStatus, OptionKind};

    fn key() -> ListCacheKey {
        ListCacheKey::new(
            "tts",
            "openai_compatible",
            "http://h:1/v1",
            "/p",
            "VOICE_KEY_A",
            OptionKind::Voices,
        )
    }

    #[test]
    fn list_cache_key_includes_all_inputs() {
        let base = key();
        assert_eq!(base, key());
        let variants = [
            ListCacheKey::new(
                "stt",
                "openai_compatible",
                "http://h:1/v1",
                "/p",
                "VOICE_KEY_A",
                OptionKind::Voices,
            ),
            ListCacheKey::new(
                "tts",
                "openai",
                "http://h:1/v1",
                "/p",
                "VOICE_KEY_A",
                OptionKind::Voices,
            ),
            ListCacheKey::new(
                "tts",
                "openai_compatible",
                "http://h:2/v1",
                "/p",
                "VOICE_KEY_A",
                OptionKind::Voices,
            ),
            ListCacheKey::new(
                "tts",
                "openai_compatible",
                "http://h:1/v1",
                "/q",
                "VOICE_KEY_A",
                OptionKind::Voices,
            ),
            ListCacheKey::new(
                "tts",
                "openai_compatible",
                "http://h:1/v1",
                "/p",
                "VOICE_KEY_B",
                OptionKind::Voices,
            ),
            ListCacheKey::new(
                "tts",
                "openai_compatible",
                "http://h:1/v1",
                "/p",
                "VOICE_KEY_A",
                OptionKind::Models,
            ),
        ];
        for v in &variants {
            assert_ne!(&base, v, "every key input must distinguish cache entries");
        }
    }

    /// WR-04: a model-specific voice list is keyed by the model, so changing the
    /// model never serves the previous model's voices from the cache.
    #[test]
    fn voice_list_cache_key_includes_the_model() {
        let a = key().with_model("kokoro");
        let b = key().with_model("other-tts");
        assert_ne!(a, b, "a different model is a different cache entry");
        assert_eq!(a, key().with_model("  kokoro "), "the model is trimmed");
        assert_ne!(a, key(), "a model-scoped key differs from the unscoped one");
        // A debounced re-fetch: model edits are keystrokes like Base URL edits.
        assert_eq!(debounce_ms(Some(&a), &b), LIST_DEBOUNCE_MS);
        assert_eq!(debounce_ms(Some(&a), &a), 0);

        // Not model-specific: model lists, and providers without a custom endpoint.
        let models = ListCacheKey::new(
            "tts",
            "openai_compatible",
            "http://h:1/v1",
            "/p",
            "VOICE_KEY_A",
            OptionKind::Models,
        );
        assert_eq!(models.clone().with_model("kokoro"), models);
        let eleven = ListCacheKey::new("tts", "elevenlabs", "", "", "", OptionKind::Voices);
        assert_eq!(eleven.clone().with_model("eleven_v3"), eleven);
    }

    thread_local! {
        static STALE_MODEL: RefCell<Option<(String, String)>> = const { RefCell::new(None) };
    }

    /// WR-04 end to end through the picker: voices cached for model A are not
    /// shown once the picker's model is B.
    #[test]
    fn picker_does_not_serve_another_models_voices() {
        fn render(listed_for: &str, shown_for: &str) -> String {
            STALE_MODEL.with(|c| {
                *c.borrow_mut() = Some((listed_for.to_string(), shown_for.to_string()))
            });
            fn app() -> Element {
                let (listed_for, shown_for) = STALE_MODEL.with(|c| c.borrow().clone().expect("case"));
                let spec = tts_provider("openai_compatible").expect("registry provider");
                let key = ListCacheKey::new("tts", spec.id, "http://h:1/v1", "", "", OptionKind::Voices)
                    .with_model(&listed_for);
                let cache: ListCache = use_signal(move || {
                    HashMap::from([(key, (ListStatus::Loaded(1), vec!["v-a".to_string()]))])
                });
                let value = use_signal(String::new);
                rsx! {
                    OptionPickerField {
                        label: "Voice".to_string(),
                        field_id: "tts-voice".to_string(),
                        kind: OptionKind::Voices,
                        family: "tts".to_string(),
                        provider: spec.id.to_string(),
                        provider_label: spec.label.to_string(),
                        value,
                        curated: vec!["curated-one".to_string()],
                        base_url: "http://h:1/v1".to_string(),
                        path: String::new(),
                        key_ref: String::new(),
                        model_for_voices: shown_for,
                        fetchable: true,
                        visible: true,
                        disabled: false,
                        cache,
                        seeded: String::new(),
                        placeholder: "Pick or type a voice".to_string(),
                        title: "Voice".to_string(),
                        noun: "voice".to_string(),
                    }
                }
            }
            let mut dom = VirtualDom::new(app);
            dom.rebuild_in_place();
            dioxus::ssr::render(&dom)
        }
        let same = render("model-a", "model-a");
        assert!(same.contains("1 voice loaded from your server."), "{same}");
        let changed = render("model-a", "model-b");
        assert!(
            !changed.contains("1 voice loaded from your server."),
            "model B must not show model A's voices: {changed}"
        );
    }

    fn result(status: &str, items: &[&str], category: Option<&str>) -> VoiceListResult {
        VoiceListResult {
            status: status.to_string(),
            items: items.iter().map(|s| s.to_string()).collect(),
            category: category.map(str::to_string),
        }
    }

    #[test]
    fn list_result_maps_to_status() {
        assert_eq!(
            list_result_to_status(&result("loaded", &["a", "b", "c"], None)),
            ListStatus::Loaded(3)
        );
        assert_eq!(
            list_result_to_status(&result("empty", &[], None)),
            ListStatus::LoadedEmpty
        );
        assert_eq!(
            list_result_to_status(&result("failed", &[], Some("timed out"))),
            ListStatus::Failed("timed out")
        );
        assert_eq!(
            list_result_to_status(&result("curated", &["x"], None)),
            ListStatus::Curated
        );
        // A category outside the fixed vocabulary never reaches the DOM.
        assert_eq!(
            list_result_to_status(&result("failed", &[], Some("<b>boom</b> http://u:p@h"))),
            ListStatus::Failed("unexpected response")
        );
        assert_eq!(
            list_result_to_status(&result("failed", &[], None)),
            ListStatus::Failed("unexpected response")
        );
        assert_eq!(
            list_result_to_status(&result("surprise", &[], None)),
            ListStatus::Failed("unexpected response")
        );
    }

    #[test]
    fn debounce_applies_only_to_endpoint_edits() {
        let base = key();
        // First fetch (mount / first visibility) starts at once.
        assert_eq!(debounce_ms(None, &base), 0);
        // Typing in Base URL, Path or key reference waits for a quiet moment.
        let mut typed = base.clone();
        typed.base_url.push('x');
        assert_eq!(debounce_ms(Some(&base), &typed), LIST_DEBOUNCE_MS);
        let mut typed = base.clone();
        typed.path.push('x');
        assert_eq!(debounce_ms(Some(&base), &typed), LIST_DEBOUNCE_MS);
        let mut typed = base.clone();
        typed.key_ref.push('X');
        assert_eq!(debounce_ms(Some(&base), &typed), LIST_DEBOUNCE_MS);
        // A provider change is immediate even though the endpoint text is kept.
        let mut switched = base.clone();
        switched.provider = "openai".to_string();
        switched.base_url.push('x');
        assert_eq!(debounce_ms(Some(&base), &switched), 0);
    }

    #[test]
    fn request_carries_the_unsaved_inputs_as_a_reference() {
        let req = request_for(&key(), "kokoro");
        assert_eq!(req.family, "tts");
        assert_eq!(req.provider, "openai_compatible");
        assert_eq!(req.kind, "voices");
        assert_eq!(req.base_url.as_deref(), Some("http://h:1/v1"));
        assert_eq!(req.path.as_deref(), Some("/p"));
        assert_eq!(req.key_ref.as_deref(), Some("VOICE_KEY_A"));
        assert_eq!(req.model.as_deref(), Some("kokoro"));
        let blank = ListCacheKey::new(
            "realtime",
            "openai_compatible",
            "",
            " ",
            "",
            OptionKind::Models,
        );
        let req = request_for(&blank, "  ");
        assert_eq!(
            (req.base_url, req.path, req.key_ref, req.model),
            (None, None, None, None)
        );
    }

    #[test]
    fn displayed_options_follow_the_status() {
        let curated = vec!["c1".to_string(), "c2".to_string()];
        let fetched = vec!["f1".to_string()];
        assert_eq!(
            displayed_options(&ListStatus::Loaded(1), &fetched, &curated),
            fetched
        );
        assert!(displayed_options(&ListStatus::LoadedEmpty, &[], &curated).is_empty());
        for status in [
            ListStatus::Curated,
            ListStatus::Loading,
            ListStatus::Failed("timed out"),
        ] {
            assert_eq!(displayed_options(&status, &fetched, &curated), curated);
        }
    }

    // ── Plan 18 Task 2: field components and every picker state ──────────────

    use crate::voice_registry::{
        list_status_copy, stt_provider, tts_provider, FieldError, KEY_REF_HINT,
    };
    use std::cell::RefCell;

    /// SSR escapes quotes and apostrophes in text nodes; match any spelling.
    fn has_text(html: &str, text: &str) -> bool {
        let apos = ["'", "&#39;", "&#x27;"];
        let quote = ["\"", "&quot;", "&#34;", "&#x22;"];
        apos.iter().any(|a| {
            quote.iter().any(|q| {
                let candidate = text.replace('\'', a).replace('"', q);
                html.contains(candidate.as_str())
            })
        })
    }

    /// The Refresh button's markup, from `<button` to `</button>`.
    fn refresh_button(html: &str) -> String {
        let at = html.find("voice-refresh-btn").expect("a Refresh button");
        let start = html[..at].rfind("<button").expect("a button element");
        let end = at + html[at..].find("</button>").expect("a closed button");
        html[start..end].to_string()
    }

    type PickerEntry = Option<(ListStatus, Vec<String>)>;

    thread_local! {
        static PICKER: RefCell<Option<(String, PickerEntry)>> = const { RefCell::new(None) };
    }

    fn render_voices_picker(base_url: &str, entry: PickerEntry) -> String {
        PICKER.with(|c| *c.borrow_mut() = Some((base_url.to_string(), entry)));
        fn app() -> Element {
            let (base, entry) = PICKER.with(|c| c.borrow().clone().expect("case"));
            let spec = tts_provider("openai_compatible").expect("registry provider");
            let key = ListCacheKey::new("tts", spec.id, &base, "", "", OptionKind::Voices)
                .with_model("model-x");
            let cache: ListCache = use_signal(move || {
                let mut seeded = HashMap::new();
                if let Some(found) = entry {
                    seeded.insert(key, found);
                }
                seeded
            });
            let value = use_signal(String::new);
            rsx! {
                OptionPickerField {
                    label: "Voice".to_string(),
                    field_id: "tts-voice".to_string(),
                    kind: OptionKind::Voices,
                    family: "tts".to_string(),
                    provider: spec.id.to_string(),
                    provider_label: spec.label.to_string(),
                    value,
                    curated: vec!["curated-one".to_string()],
                    base_url: base,
                    path: String::new(),
                    key_ref: String::new(),
                    model_for_voices: "model-x".to_string(),
                    fetchable: true,
                    visible: true,
                    disabled: false,
                    cache,
                    seeded: String::new(),
                    placeholder: "Pick or type a voice".to_string(),
                    title: "Voice".to_string(),
                    noun: "voice".to_string(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn option_picker_renders_each_state() {
        let base = "http://h:1/v1";
        let three: Vec<String> = ["v1", "v2", "v3"].iter().map(|s| s.to_string()).collect();
        let cases: Vec<(&str, PickerEntry, ListStatus)> = vec![
            ("", None, ListStatus::Curated),
            (base, None, ListStatus::Loading),
            (
                base,
                Some((ListStatus::Loaded(1), vec!["v1".to_string()])),
                ListStatus::Loaded(1),
            ),
            (
                base,
                Some((ListStatus::Loaded(3), three)),
                ListStatus::Loaded(3),
            ),
            (
                base,
                Some((ListStatus::LoadedEmpty, Vec::new())),
                ListStatus::LoadedEmpty,
            ),
            (
                base,
                Some((ListStatus::Failed("timed out"), Vec::new())),
                ListStatus::Failed("timed out"),
            ),
        ];
        for (url, entry, expected) in cases {
            let html = render_voices_picker(url, entry);
            let copy = list_status_copy(&expected, OptionKind::Voices, "OpenAI-compatible");
            assert!(has_text(&html, &copy), "{expected:?}: {html}");
            assert!(html.contains("↻ Refresh voices"), "{expected:?}: {html}");
            assert!(
                html.contains("aria-label=\"Refresh voices from your server\""),
                "{expected:?}: {html}"
            );
            assert_eq!(
                html.contains("aria-busy=\"true\""),
                expected == ListStatus::Loading,
                "{expected:?}: {html}"
            );
            // A degraded hint is never styled as a form error.
            assert!(!html.contains("is-error"), "{expected:?}: {html}");
        }
        // Literal copy, independent of list_status_copy.
        let one = render_voices_picker(base, Some((ListStatus::Loaded(1), vec!["v1".into()])));
        assert!(has_text(&one, "1 voice loaded from your server."), "{one}");
        let loading = render_voices_picker(base, None);
        assert!(has_text(&loading, "Loading voices…"), "{loading}");
        let failed =
            render_voices_picker(base, Some((ListStatus::Failed("key rejected"), Vec::new())));
        assert!(
            has_text(
                &failed,
                "Couldn't load voices: key rejected. Showing built-in defaults. You can still type any value."
            ),
            "{failed}"
        );
        // Refresh needs a usable address: disabled without one, enabled with one.
        assert!(refresh_button(&render_voices_picker("", None)).contains("disabled"));
        assert!(!refresh_button(&loading).contains("disabled"));
    }

    #[test]
    fn fetch_eligibility_rules() {
        let compat = tts_provider("openai_compatible").expect("registry provider");
        let edge = tts_provider("edge").expect("registry provider");
        let stt_compat = stt_provider("openai_compatible").expect("registry provider");
        // Endpoint providers need the family on screen AND a usable Base URL.
        assert!(fetch_eligible(true, compat, "http://h:1/v1"));
        assert!(fetch_eligible(true, stt_compat, "https://h/v1"));
        assert!(!fetch_eligible(false, compat, "http://h:1/v1"));
        assert!(!fetch_eligible(true, compat, ""));
        assert!(!fetch_eligible(true, compat, "ftp://h"));
        assert!(!fetch_eligible(true, compat, "http://u:p@h"));
        // Providers without endpoint fields only need to be visible.
        assert!(fetch_eligible(true, edge, ""));
        assert!(!fetch_eligible(false, edge, ""));
    }

    struct EndpointCase {
        base: String,
        path: Option<String>,
        key_ref: Option<String>,
        key_status: String,
    }

    thread_local! {
        static ENDPOINT: RefCell<Option<EndpointCase>> = const { RefCell::new(None) };
    }

    fn render_endpoint(case: EndpointCase) -> String {
        ENDPOINT.with(|c| *c.borrow_mut() = Some(case));
        fn app() -> Element {
            let case = ENDPOINT.with(|c| c.borrow_mut().take().expect("case"));
            let base_url = use_signal(|| case.base.clone());
            let path_sig = use_signal(|| case.path.clone().unwrap_or_default());
            let key_sig = use_signal(|| case.key_ref.clone().unwrap_or_default());
            rsx! {
                EndpointFields {
                    base_url,
                    path: case.path.as_ref().map(|_| path_sig),
                    key_ref: case.key_ref.as_ref().map(|_| key_sig),
                    key_status: case.key_status.clone(),
                    default_path: "/audio/speech".to_string(),
                    base_placeholder: "http://host:8880/v1".to_string(),
                    base_hint: None,
                    required: true,
                    disabled: false,
                    no_key_hint: Some("With no key, the browser connects without a token.".to_string()),
                    id_prefix: "tts".to_string(),
                }
            }
        }
        let mut dom = VirtualDom::new(app);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    fn endpoint(base: &str, path: Option<&str>, key_ref: Option<&str>, status: &str) -> String {
        render_endpoint(EndpointCase {
            base: base.to_string(),
            path: path.map(str::to_string),
            key_ref: key_ref.map(str::to_string),
            key_status: status.to_string(),
        })
    }

    #[test]
    fn endpoint_fields_validate_instantly() {
        let bad = endpoint("http://u:p@h", None, None, "none");
        assert!(has_text(&bad, FieldError::BaseUrlUserinfo.copy()), "{bad}");
        assert!(bad.contains("voice-field-status is-error"), "{bad}");
        assert!(bad.contains("aria-invalid=\"true\""), "{bad}");
        assert!(bad.contains("id=\"tts-base-url-status\""), "{bad}");
        assert!(
            bad.contains("aria-describedby=\"tts-base-url-hint tts-base-url-status\"")
                || bad.contains("aria-describedby=\"tts-base-url-status\""),
            "{bad}"
        );
        // Only the Base URL row: no Path and no key reference.
        // (the userinfo copy itself mentions the field, so match the label.)
        assert!(!bad.contains(">API key reference<"), "{bad}");
        assert!(!bad.contains("tts-key-ref"), "{bad}");
        assert!(!bad.contains(">Path<"), "{bad}");
        assert!(!bad.contains("tts-path"), "{bad}");

        // A LAN / private host is fine and gets no warning (D-04).
        let lan = endpoint("http://10.0.0.5:8880/v1", None, None, "none");
        assert!(!lan.contains("is-error"), "{lan}");
        assert!(!lan.contains("is-warn"), "{lan}");
        assert!(lan.contains("aria-invalid=\"false\""), "{lan}");

        // Empty is a blocking error only where the provider is selected.
        let empty = endpoint("", None, None, "none");
        assert!(has_text(&empty, FieldError::BaseUrlEmpty.copy()), "{empty}");

        // Path validates on input too.
        let path = endpoint("http://h/v1", Some("no-slash"), None, "none");
        assert!(has_text(&path, FieldError::PathShape.copy()), "{path}");
        assert!(path.contains("placeholder=\"/audio/speech\""), "{path}");
    }

    #[test]
    fn key_ref_field_is_plain_text() {
        let html = endpoint("http://h/v1", Some(""), Some(""), "none");
        assert!(html.contains("id=\"tts-key-ref\""), "{html}");
        assert!(html.contains("type=\"text\""), "{html}");
        assert!(!html.contains("type=\"password\""), "{html}");
        assert!(has_text(&html, KEY_REF_HINT), "{html}");
        assert!(
            has_text(&html, "With no key, the browser connects without a token."),
            "{html}"
        );
        assert!(html.contains("NO KEY"), "{html}");

        // An unrelated secret's name is refused with the namespace copy.
        let ns = endpoint("http://h/v1", Some(""), Some("OPENAI_API_KEY"), "none");
        assert!(has_text(&ns, FieldError::KeyRefNamespace.copy()), "{ns}");
        // A key-shaped value gets the "looks like the key itself" copy.
        let secret = endpoint("http://h/v1", Some(""), Some("sk-abc"), "none");
        assert!(
            has_text(&secret, FieldError::KeyRefLooksSecret.copy()),
            "{secret}"
        );

        // The pill is one of three fixed strings and never the reference.
        for (status, text) in [
            ("found", "FOUND"),
            ("not_found", "NOT FOUND"),
            ("none", "NO KEY"),
        ] {
            let pill = endpoint("http://h/v1", None, Some("VOICE_KEY_PILL"), status);
            assert!(pill.contains(text), "{status}: {pill}");
        }
        fn pill_app() -> Element {
            rsx! { KeyRefStatusPill { status: "VOICE_KEY_LEAK".to_string() } }
        }
        let mut dom = VirtualDom::new(pill_app);
        dom.rebuild_in_place();
        let html = dioxus::ssr::render(&dom);
        assert!(html.contains("NO KEY"), "{html}");
        assert!(!html.contains("VOICE_KEY_LEAK"), "{html}");
    }

    #[test]
    fn stale_response_is_discarded() {
        assert!(!accept_response(5, 4));
        assert!(accept_response(5, 5));
    }

    #[test]
    fn voice_fields_has_no_hardcoded_voice_defaults() {
        let src = include_str!("voice_fields.rs");
        for needle in crate::voice_registry::hardcoded_voice_default_needles() {
            assert!(
                !src.contains(needle.as_str()),
                "voice_fields.rs still holds a hard-coded voice default: {needle}"
            );
        }
    }

    #[test]
    fn option_picker_calls_only_the_server_fn() {
        let src = include_str!("voice_fields.rs");
        assert!(
            src.contains("list_voice_options("),
            "the server fn is the only path"
        );
        for needle in [
            ["web_sys::Req", "uest"].concat(),
            ["fetch", "_with_request"].concat(),
        ] {
            assert!(
                !src.contains(needle.as_str()),
                "voice_fields.rs must not contact a voice server from the browser: {needle}"
            );
        }
    }
}
