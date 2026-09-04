//! Add-route wizard (E3, D-03) — preset tiles (Twilio SMS / n8n generic_v2
//! / CRM deliver-only) plus a drop-in paste path, opening a full editable
//! route form. Reuses the 48.2 D-19 `.mcp-wizard-*` class set (`tools.css`,
//! loaded here via its own `document::Link` — Plan 01's SUMMARY documents
//! that `tools.css` is NOT globally loaded, so the Gateway screen must load
//! it itself to reuse those exact classes rather than paraphrasing them).
//!
//! `open: Signal<bool>` is the established contract `mod.rs`'s
//! `+ ADD PLATFORM` button already wires (Task 1) — toggled on click,
//! read here so a later reader cannot accidentally drop the modal
//! entirely. `scope`/`refresh_tick` are the same scope-write contract
//! every other writer in this phase takes.
//!
//! # `RouteEditorModal` is the ONE form both flows share (D-03)
//!
//! [`RouteEditorModal`] is the full editable route form — every
//! `WebhookRouteView` field. [`AddRouteWizard`]'s step 2 mounts it after a
//! preset/paste selection (`is_new: true`, `ADD ROUTE` CTA);
//! `webhook_route_cards.rs`'s CONFIGURE flow mounts the SAME component
//! directly for an existing route (`is_new: false`, `SAVE ROUTE` CTA) —
//! "existing routes open in the same form" is satisfied by sharing this
//! one component across both call sites, not by two parallel forms with
//! the same field list typed twice.
//!
//! # Presets/paste-parsing are server round trips, never client-built
//!
//! `webhook_route_api.rs`'s module doc explains why: `ironhermes-core`
//! (which owns `WebhookRoute`/`SignatureKind`/etc.) is declared ONLY under
//! this crate's wasm32-excluded dependency table — it does not exist on
//! this file's own compile target when built for the browser. Choosing a
//! preset tile or parsing a pasted snippet therefore calls
//! `webhook_route_api::preset_webhook_route`/`parse_pasted_route` (both
//! `#[server]` fns returning the wasm-safe `WebhookRouteView`) rather than
//! building a `WebhookRoute` in this file. The client-side REFUSAL check
//! (T-49.3-04-02) is the one thing that stays genuinely round-trip-free —
//! `webhook_route_api::route_would_refuse` operates on
//! `WebhookRouteView`'s plain `signature: String` field, so it needs no
//! native type and runs instantly as the operator edits the form.

use dioxus::prelude::*;

use crate::server::tools_config_api::ConfigScope;
use crate::server::webhook_route_api::{self, WebhookRouteView};

#[allow(dead_code)] // used in AddRouteWizard/RouteEditorModal rsx!; dead_code fires on the test target (tools.rs TOOLS_CSS precedent)
const TOOLS_CSS: Asset = asset!("/assets/tools.css");

// =============================================================================
// Pure helpers — outbound-auth selector <-> flattened DTO fields, unit-
// tested below. `signature`/`deliver`/`session` are already plain strings
// on `WebhookRouteView`, so no enum <-> value mapping is needed for them.
// =============================================================================

/// The outbound-auth SELECTOR value the wizard renders — matches
/// `WebhookRouteView::outbound_auth_kind` directly (`"none"`/`"bearer"`/
/// `"basic"`), so this is just a defensive normalize, never a re-derivation.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn normalized_outbound_auth_kind(kind: &str) -> &'static str {
    match kind {
        "bearer" => "bearer",
        "basic" => "basic",
        _ => "none",
    }
}

/// Reset a draft's outbound-auth env-NAME fields for a NEWLY selected
/// selector `kind` — env-NAME values from a DIFFERENT kind never leak into
/// the new kind's fields (switching `bearer` -> `basic` does not silently
/// reuse the bearer env name as a username env).
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn apply_outbound_auth_kind(draft: &mut WebhookRouteView, kind: &str) {
    draft.outbound_auth_kind = normalized_outbound_auth_kind(kind).to_string();
    draft.outbound_auth_env = None;
    draft.outbound_auth_user_env = None;
    draft.outbound_auth_pass_env = None;
}

/// The client-side refusal mirror's UI copy (T-49.3-04-02, exact string
/// from `49.3-UI-SPEC.md`'s Copywriting Contract "Error state" row).
/// `bind_host` is `None` when the webhook listener has no configured bind
/// host yet — WEBHOOK-AND-REST-API.md documents no default, so there is
/// nothing to refuse against yet, and this fn returns `None` rather than
/// guessing.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn client_refusal_message(route: &WebhookRouteView, bind_host: Option<&str>) -> Option<String> {
    let host = bind_host?;
    if webhook_route_api::route_would_refuse(route, host) {
        Some(
            "This route would refuse to start: \"signature: none\" requires a loopback bind. \
             Remove verification or bind to 127.0.0.1."
                .to_string(),
        )
    } else {
        None
    }
}

/// CR-02: derive the identity assertion the save path sends as
/// `editing_name` — `None` when a new route is being created, `Some(the
/// name the modal was opened under)` when editing an existing one. ONE
/// small helper so there is a SINGLE definition of which route is being
/// edited, referenced both by [`save_intent`]'s direct-send branch and by
/// the CONFIRM REPLACE button — never a second, independently-written
/// expression (a second producer encoding a different contract is exactly
/// what CR-02 was).
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn editing_name_for(is_new: bool, initial_name: &str) -> Option<String> {
    if is_new {
        None
    } else {
        Some(initial_name.to_string())
    }
}

/// [`save_intent`]'s return type — the save path's arguments, and nothing
/// else. Every caller of `upsert_webhook_route` derives its arguments from
/// ONE of these two variants; there is no second, independently-computed
/// expression anywhere else in this file (CR-02: the server's collision
/// guard and this file's OLD `overwrite_collision` predicate disagreed
/// about the in-place-edit case, and the disagreement shipped past two
/// individually-green test suites because nothing forced the two halves
/// through one producer).
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
pub(crate) enum SaveIntent {
    /// The REPLACE ROUTE confirm must be shown before any server call —
    /// carries the colliding route's name for the dialog's copy.
    Confirm { colliding_name: String },
    /// Send directly, no confirm — the exact two arguments
    /// `upsert_webhook_route` needs beyond `scope`/`payload`.
    DirectSend {
        allow_overwrite: bool,
        editing_name: Option<String>,
    },
}

/// CR-01/CR-02 (client half): the ONLY producer of the arguments the save
/// path sends to `upsert_webhook_route` — both the overwrite flag and the
/// editing identity. Fires the REPLACE ROUTE confirm whenever a NEW route
/// (`is_new: true`) collides with an existing name, or an EDIT renames onto
/// a DIFFERENT existing route's name — never on the ordinary in-place edit
/// (`!is_new && draft_name == initial_name`), which must stay a one-click
/// save (49.3-07-PLAN.md's Task 2 `<behavior>` spec, D-03). A blank draft
/// name is a validation concern ([`validate_route_fields`], server-side),
/// not an overwrite concern, so it is deliberately never treated as a
/// collision here even if it happens to equal an existing (also-blank,
/// which cannot exist) name. This is the UX-only pre-check —
/// [`upsert_webhook_route_impl`]'s server-side collision guard, narrowed by
/// the SAME `editing_name` this predicate computes via
/// [`editing_name_for`], is the actual authority, reachable by any client
/// regardless of whether this predicate ran.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
pub(crate) fn save_intent(
    is_new: bool,
    initial_name: &str,
    draft_name: &str,
    existing_names: &[String],
) -> SaveIntent {
    let direct_send = || SaveIntent::DirectSend {
        allow_overwrite: false,
        editing_name: editing_name_for(is_new, initial_name),
    };
    if draft_name.trim().is_empty() {
        return direct_send();
    }
    if !is_new && draft_name == initial_name {
        return direct_send();
    }
    if existing_names.iter().any(|n| n == draft_name) {
        return SaveIntent::Confirm {
            colliding_name: draft_name.to_string(),
        };
    }
    direct_send()
}

// =============================================================================
// AddRouteWizard — the always-mounted modal `mod.rs` toggles via `open`.
// =============================================================================

#[derive(Clone, PartialEq)]
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
enum WizardStep {
    /// D-03 preset step: CHOOSE A STARTING POINT.
    ChooseStart,
    /// Full editor over a preset/parsed draft, before the first save.
    /// Boxed — `WebhookRouteView` is >400 bytes, which would otherwise make
    /// every `WizardStep` (including the zero-data `ChooseStart` variant)
    /// pay for the largest variant's size (`clippy::large_enum_variant`).
    Editing(Box<WebhookRouteView>),
}

#[component]
pub fn AddRouteWizard(
    open: Signal<bool>,
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let mut step: Signal<WizardStep> = use_signal(|| WizardStep::ChooseStart);
    let mut paste_text: Signal<String> = use_signal(String::new);
    let mut paste_error: Signal<Option<String>> = use_signal(|| None);
    let preset_loading: Signal<bool> = use_signal(|| false);

    let mut close_and_reset = move || {
        open.set(false);
        step.set(WizardStep::ChooseStart);
        paste_text.set(String::new());
        paste_error.set(None);
    };

    let select_preset = move |kind: &'static str| {
        let mut step_sig = step;
        let mut loading_sig = preset_loading;
        let scope_value = scope();
        loading_sig.set(true);
        spawn(async move {
            let result = webhook_route_api::preset_webhook_route(kind.to_string(), scope_value).await;
            loading_sig.set(false);
            if let Ok(view) = result {
                step_sig.set(WizardStep::Editing(Box::new(view)));
            }
        });
    };

    let step_val = step.read().clone();
    let preset_loading_val = *preset_loading.read();

    rsx! {
        document::Link { rel: "stylesheet", href: TOOLS_CSS }
        if *open.read() {
            match step_val {
                WizardStep::Editing(draft) => rsx! {
                    RouteEditorModal {
                        initial: *draft,
                        is_new: true,
                        scope,
                        refresh_tick,
                        on_close: move |_| close_and_reset(),
                    }
                },
                WizardStep::ChooseStart => rsx! {
                    div { class: "mcp-wizard-overlay", role: "presentation",
                        div {
                            class: "mcp-wizard",
                            role: "dialog",
                            aria_modal: "true",
                            "aria-labelledby": "webhook-wizard-title",
                            onkeydown: move |event| {
                                if event.key() == Key::Escape {
                                    close_and_reset();
                                }
                            },
                            div { class: "mcp-wizard-header",
                                h3 { class: "mcp-wizard-title", id: "webhook-wizard-title", "CHOOSE A STARTING POINT" }
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    "aria-label": "Close add-route wizard",
                                    onclick: move |_| close_and_reset(),
                                    "✕"
                                }
                            }
                            div { class: "mcp-wizard-body",
                                div { class: "grid wide",
                                    button {
                                        class: "btn",
                                        disabled: preset_loading_val,
                                        onclick: move |_| select_preset("twilio"),
                                        "TWILIO SMS"
                                    }
                                    button {
                                        class: "btn",
                                        disabled: preset_loading_val,
                                        onclick: move |_| select_preset("n8n"),
                                        "N8N / GENERIC"
                                    }
                                    button {
                                        class: "btn",
                                        disabled: preset_loading_val,
                                        onclick: move |_| select_preset("crm"),
                                        "CRM DELIVER-ONLY"
                                    }
                                }
                                label { class: "tools-settings-label", "PASTE ROUTE CONFIG — JSON OR YAML" }
                                textarea {
                                    class: "mcp-wizard-textarea",
                                    placeholder: "Paste a webhook route snippet — a bare {{}} is valid and fills in server defaults",
                                    value: "{paste_text}",
                                    oninput: move |evt| paste_text.set(evt.value()),
                                }
                                if let Some(err) = paste_error.read().clone() {
                                    div { class: "mcp-wizard-probe-error", "{err}" }
                                }
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    disabled: paste_text.read().trim().is_empty() || preset_loading_val,
                                    onclick: move |_| {
                                        let text = paste_text.read().clone();
                                        let mut step_sig = step;
                                        let mut paste_error_sig = paste_error;
                                        let mut loading_sig = preset_loading;
                                        loading_sig.set(true);
                                        spawn(async move {
                                            let result = webhook_route_api::parse_pasted_route(text).await;
                                            loading_sig.set(false);
                                            match result {
                                                Ok(view) => {
                                                    paste_error_sig.set(None);
                                                    step_sig.set(WizardStep::Editing(Box::new(view)));
                                                }
                                                Err(e) => paste_error_sig.set(Some(e.to_string())),
                                            }
                                        });
                                    },
                                    if preset_loading_val { "PARSING…" } else { "USE PASTED CONFIG" }
                                }
                            }
                            div { class: "mcp-wizard-footer",
                                button {
                                    class: "btn btn--ghost btn--sm",
                                    onclick: move |_| close_and_reset(),
                                    "CANCEL"
                                }
                            }
                        }
                    }
                },
            }
        }
    }
}

// =============================================================================
// RouteEditorModal — the ONE full route-field form (D-03).
// =============================================================================

#[component]
pub(crate) fn RouteEditorModal(
    initial: WebhookRouteView,
    is_new: bool,
    scope: ReadSignal<ConfigScope>,
    mut refresh_tick: Signal<u32>,
    on_close: EventHandler<()>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    // Captured BEFORE `initial` is moved into the draft signal's
    // initializer below — the draft's name changes as the operator types,
    // so `save_intent`'s comparison baseline must be the name the modal
    // was opened under, never the current (possibly-edited) draft.
    let initial_name = initial.name.clone();
    let mut draft: Signal<WebhookRouteView> = use_signal(move || initial.clone());
    let submitting: Signal<bool> = use_signal(|| false);
    let save_error: Signal<Option<String>> = use_signal(|| None);
    // The colliding route's name, or `None` — CR-01 client half.
    let pending_overwrite: Signal<Option<String>> = use_signal(|| None);

    // The webhook listener's OWN configured bind host — the client-side
    // refusal mirror's real input (T-49.3-04-02), re-fetched on scope
    // change.
    let bind_host_resource = use_resource(move || {
        let scope_value = scope();
        async move { webhook_route_api::get_webhook_bind_host(scope_value).await }
    });
    let bind_host: Option<String> = match bind_host_resource() {
        Some(Ok(host)) => host,
        _ => None,
    };

    // The already-available route list this modal's save-path pre-check
    // reads (`49.3-VERIFICATION.md`'s `missing:` list) — re-read on scope
    // change or after a save, exactly like `bind_host_resource` above.
    let existing_routes_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { webhook_route_api::list_webhook_routes(scope_value).await }
    });
    let existing_routes: Vec<WebhookRouteView> = match existing_routes_resource() {
        Some(Ok(list)) => list,
        _ => Vec::new(),
    };
    let existing_names: Vec<String> = existing_routes.iter().map(|r| r.name.clone()).collect();

    let draft_val = draft.read().clone();
    let refusal_message = client_refusal_message(&draft_val, bind_host.as_deref());
    let title = if is_new {
        "ADD ROUTE".to_string()
    } else {
        format!("EDIT ROUTE — {}", draft_val.name)
    };
    let save_label = if is_new { "ADD ROUTE" } else { "SAVE ROUTE" };
    let pending_overwrite_val = pending_overwrite.read().clone();
    let confirm_showing = pending_overwrite_val.is_some();
    let submit_disabled = *submitting.read() || refusal_message.is_some() || confirm_showing;
    let outbound_kind = draft_val.outbound_auth_kind.clone();
    // CR-02: computed once, from the SAME `editing_name_for` helper
    // `save_intent` uses internally — the CONFIRM REPLACE button below
    // needs its own copy (it does not go through `save_intent`, since the
    // confirm branch is already decided), and there must still be only one
    // definition of which route is being edited.
    let editing_name_for_confirm = editing_name_for(is_new, &initial_name);

    // Extract the save spawn body into one Copy closure taking the
    // `allow_overwrite` bool AND the editing identity — the confirmed and
    // unconfirmed paths share this ONE implementation rather than
    // duplicating it. Captures only `Signal`/`ReadSignal`/`EventHandler`
    // values, all already `Copy`; `editing_name` is a per-call PARAMETER,
    // not a capture, so it does not affect `commit_save`'s own `Copy`-ness
    // (its comment above the doc explains why the identity is passed in
    // per call site rather than captured as a `String`). `commit_save`
    // itself stays `Copy` and can be referenced from both the ordinary save
    // button and the confirm's CONFIRM REPLACE button below.
    let commit_save = move |allow_overwrite: bool, editing_name: Option<String>| {
        let mut pending_overwrite_sig = pending_overwrite;
        pending_overwrite_sig.set(None);
        let scope_value = scope();
        let payload = draft.read().clone();
        let mut submitting_sig = submitting;
        let mut save_error_sig = save_error;
        let mut refresh_tick_sig = refresh_tick;
        submitting_sig.set(true);
        spawn(async move {
            let result = webhook_route_api::upsert_webhook_route(
                scope_value,
                payload,
                allow_overwrite,
                editing_name,
            )
            .await;
            submitting_sig.set(false);
            match result {
                Ok(_) => {
                    save_error_sig.set(None);
                    let cur = *refresh_tick_sig.read();
                    refresh_tick_sig.set(cur + 1);
                    on_close.call(());
                }
                Err(e) => save_error_sig.set(Some(e.to_string())),
            }
        });
    };

    // The confirm's copy — resolved once per render so the rsx below stays
    // a plain `{msg}` interpolation. `colliding_path` is empty only in the
    // (unreachable in practice) case the collision fired against a name no
    // longer present in `existing_routes` between fetches.
    let confirm_message: Option<String> = pending_overwrite_val.as_ref().map(|name| {
        let path = existing_routes
            .iter()
            .find(|r| &r.name == name)
            .map(|r| r.path.clone())
            .unwrap_or_default();
        format!(
            "REPLACE ROUTE — This overwrites the existing route \"{name}\" ({path}) with the \
             configuration in this form. Its signature, secret env names, delivery target and \
             prompt template are all replaced. This can't be undone."
        )
    });

    rsx! {
        div { class: "mcp-wizard-overlay", role: "presentation",
            div {
                class: "mcp-wizard",
                role: "dialog",
                aria_modal: "true",
                "aria-labelledby": "route-editor-title",
                onkeydown: move |event| {
                    if event.key() == Key::Escape {
                        on_close.call(());
                    }
                },
                div { class: "mcp-wizard-header",
                    h3 { class: "mcp-wizard-title", id: "route-editor-title", "{title}" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Close route editor",
                        onclick: move |_| on_close.call(()),
                        "✕"
                    }
                }
                div { class: "mcp-wizard-body",
                    label { class: "tools-settings-label", "NAME" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.name}",
                        oninput: move |evt| draft.write().name = evt.value(),
                    }
                    label { class: "tools-settings-label", "PATH" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.path}",
                        oninput: move |evt| draft.write().path = evt.value(),
                    }
                    label { class: "tools-settings-label", "SIGNATURE" }
                    select {
                        class: "tools-settings-input",
                        value: "{draft_val.signature}",
                        onchange: move |evt| draft.write().signature = evt.value(),
                        option { value: "generic_v2", "generic_v2" }
                        option { value: "none", "none" }
                        option { value: "twilio", "twilio" }
                        option { value: "telnyx", "telnyx" }
                    }
                    if let Some(msg) = refusal_message.clone() {
                        div { class: "mcp-wizard-probe-error", "{msg}" }
                    }
                    label { class: "tools-settings-label", "SECRET_ENV (generic_v2 HMAC secret var name)" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.secret_env.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().secret_env = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label { class: "tools-settings-label", "AUTH_TOKEN_ENV (Twilio auth token var name)" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.auth_token_env.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().auth_token_env = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label { class: "tools-settings-label", "PUBLIC_KEY_ENV (Telnyx Ed25519 public key var name)" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.public_key_env.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().public_key_env = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label { class: "tools-settings-label", "TIMESTAMP_SKEW_SECS" }
                    input {
                        class: "tools-settings-input",
                        r#type: "number",
                        value: "{draft_val.timestamp_skew_secs}",
                        oninput: move |evt| {
                            if let Ok(v) = evt.value().parse::<u64>() {
                                draft.write().timestamp_skew_secs = v;
                            }
                        },
                    }
                    label { class: "tools-settings-label", "PROMPT_TEMPLATE" }
                    textarea {
                        class: "mcp-wizard-textarea mcp-wizard-textarea--sm",
                        value: "{draft_val.prompt_template}",
                        oninput: move |evt| draft.write().prompt_template = evt.value(),
                    }
                    label { class: "tools-settings-label", "DELIVER" }
                    select {
                        class: "tools-settings-input",
                        value: "{draft_val.deliver}",
                        onchange: move |evt| draft.write().deliver = evt.value(),
                        option { value: "url", "url" }
                        option { value: "origin", "origin" }
                        option { value: "platform", "platform" }
                    }
                    label { class: "tools-settings-label", "DELIVER_URL" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.deliver_url.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().deliver_url = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label { class: "tools-settings-label", "DELIVER_PLATFORM" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.deliver_platform.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().deliver_platform = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label { class: "tools-settings-label", "DELIVER_CHAT_ID" }
                    input {
                        class: "tools-settings-input",
                        value: "{draft_val.deliver_chat_id.clone().unwrap_or_default()}",
                        oninput: move |evt| {
                            let v = evt.value();
                            draft.write().deliver_chat_id = if v.trim().is_empty() { None } else { Some(v) };
                        },
                    }
                    label {
                        input {
                            r#type: "checkbox",
                            checked: draft_val.deliver_only,
                            onchange: move |evt| draft.write().deliver_only = evt.checked(),
                        }
                        " DELIVER_ONLY (skip the agent turn entirely)"
                    }
                    label { class: "tools-settings-label", "OUTBOUND_AUTH" }
                    select {
                        class: "tools-settings-input",
                        value: "{outbound_kind}",
                        onchange: move |evt| apply_outbound_auth_kind(&mut draft.write(), &evt.value()),
                        option { value: "none", "none" }
                        option { value: "bearer", "bearer" }
                        option { value: "basic", "basic" }
                    }
                    if outbound_kind == "bearer" {
                        label { class: "tools-settings-label", "OUTBOUND_AUTH.ENV" }
                        input {
                            class: "tools-settings-input",
                            value: "{draft_val.outbound_auth_env.clone().unwrap_or_default()}",
                            oninput: move |evt| draft.write().outbound_auth_env = Some(evt.value()),
                        }
                    } else if outbound_kind == "basic" {
                        label { class: "tools-settings-label", "OUTBOUND_AUTH.USER_ENV" }
                        input {
                            class: "tools-settings-input",
                            value: "{draft_val.outbound_auth_user_env.clone().unwrap_or_default()}",
                            oninput: move |evt| draft.write().outbound_auth_user_env = Some(evt.value()),
                        }
                        label { class: "tools-settings-label", "OUTBOUND_AUTH.PASS_ENV" }
                        input {
                            class: "tools-settings-input",
                            value: "{draft_val.outbound_auth_pass_env.clone().unwrap_or_default()}",
                            oninput: move |evt| draft.write().outbound_auth_pass_env = Some(evt.value()),
                        }
                    }
                    label { class: "tools-settings-label", "SESSION" }
                    select {
                        class: "tools-settings-input",
                        value: "{draft_val.session}",
                        onchange: move |evt| draft.write().session = evt.value(),
                        option { value: "ephemeral", "ephemeral" }
                        option { value: "persistent", "persistent" }
                    }
                    if let Some(err) = save_error.read().clone() {
                        div { class: "mcp-wizard-probe-error", "SAVE FAILED — {err}. Check your connection and retry." }
                    }
                    if let Some(msg) = confirm_message.clone() {
                        div { class: "gw-confirm",
                            p { "{msg}" }
                            div { class: "gw-confirm-actions",
                                button {
                                    r#type: "button",
                                    class: "btn btn--ghost btn--sm",
                                    disabled: *submitting.read(),
                                    onclick: move |_| {
                                        let mut pending_overwrite_sig = pending_overwrite;
                                        pending_overwrite_sig.set(None);
                                    },
                                    "CANCEL"
                                }
                                button {
                                    r#type: "button",
                                    class: "btn btn--danger btn--sm",
                                    disabled: *submitting.read(),
                                    // CR-02: the editing identity comes from
                                    // `editing_name_for_confirm` — the SAME
                                    // `editing_name_for` helper `save_intent`
                                    // uses, never a second inline expression.
                                    onclick: move |_| commit_save(true, editing_name_for_confirm.clone()),
                                    "CONFIRM REPLACE"
                                }
                            }
                        }
                    }
                }
                div { class: "mcp-wizard-footer",
                    button {
                        class: "btn btn--ghost btn--sm",
                        disabled: *submitting.read(),
                        onclick: move |_| on_close.call(()),
                        "CANCEL"
                    }
                    button {
                        class: "btn",
                        disabled: submit_disabled,
                        onclick: move |_| {
                            if *submitting.peek() {
                                return;
                            }
                            let draft_name = draft.peek().name.clone();
                            // CR-02: `save_intent` is the ONLY producer of
                            // these arguments — no second, independently
                            // derived overwrite-flag/editing-identity pair.
                            match save_intent(is_new, &initial_name, &draft_name, &existing_names) {
                                SaveIntent::Confirm { colliding_name } => {
                                    let mut pending_overwrite_sig = pending_overwrite;
                                    pending_overwrite_sig.set(Some(colliding_name));
                                }
                                SaveIntent::DirectSend { allow_overwrite, editing_name } => {
                                    commit_save(allow_overwrite, editing_name)
                                }
                            }
                        },
                        if *submitting.read() { "SAVING…" } else { "{save_label}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_outbound_auth_kind_defaults_unknown_to_none() {
        assert_eq!(normalized_outbound_auth_kind("bearer"), "bearer");
        assert_eq!(normalized_outbound_auth_kind("basic"), "basic");
        assert_eq!(normalized_outbound_auth_kind("garbage"), "none");
    }

    fn empty_draft() -> WebhookRouteView {
        WebhookRouteView {
            name: String::new(),
            path: String::new(),
            signature: "generic_v2".to_string(),
            secret_env: None,
            auth_token_env: None,
            public_key_env: None,
            timestamp_skew_secs: 300,
            prompt_template: String::new(),
            deliver: "url".to_string(),
            deliver_url: None,
            deliver_platform: None,
            deliver_chat_id: None,
            deliver_only: false,
            outbound_auth_kind: "none".to_string(),
            outbound_auth_env: None,
            outbound_auth_user_env: None,
            outbound_auth_pass_env: None,
            session: "ephemeral".to_string(),
            rails_max_body_bytes: 1024 * 1024,
            rails_rate_limit_per_minute: 30,
            rails_idempotency_ttl_secs: 3600,
        }
    }

    #[test]
    fn apply_outbound_auth_kind_clears_stale_env_names_on_switch() {
        let mut draft = empty_draft();
        draft.outbound_auth_kind = "bearer".to_string();
        draft.outbound_auth_env = Some("OLD_BEARER_ENV".to_string());

        apply_outbound_auth_kind(&mut draft, "basic");

        assert_eq!(draft.outbound_auth_kind, "basic");
        assert_eq!(draft.outbound_auth_env, None, "stale bearer env must not leak into basic");
        assert_eq!(draft.outbound_auth_user_env, None);
        assert_eq!(draft.outbound_auth_pass_env, None);
    }

    #[test]
    fn client_refusal_message_matches_ui_spec_copy_for_none_on_public_bind() {
        let mut route = empty_draft();
        route.signature = "none".to_string();
        let msg = client_refusal_message(&route, Some("0.0.0.0"));
        assert!(msg.is_some());
        assert!(msg.unwrap().starts_with("This route would refuse to start"));
    }

    #[test]
    fn client_refusal_message_is_none_when_bind_host_unset() {
        let mut route = empty_draft();
        route.signature = "none".to_string();
        assert_eq!(client_refusal_message(&route, None), None);
    }

    #[test]
    fn client_refusal_message_is_none_for_verified_signature() {
        let route = empty_draft();
        assert_eq!(client_refusal_message(&route, Some("0.0.0.0")), None);
    }

    // -------------------------------------------------------------------
    // save_intent / editing_name_for — CR-02 client-half truth table.
    // Renamed from `overwrite_collision_*` (CR-01) and re-expressed against
    // `SaveIntent`; the two branches `overwrite_collision` had no way to
    // express (a rename to a free name, a new route with a free name) are
    // included below, each asserting the editing identity the direct send
    // now carries.
    // -------------------------------------------------------------------

    #[test]
    fn editing_name_for_is_none_for_a_new_route() {
        assert_eq!(editing_name_for(true, ""), None);
    }

    #[test]
    fn editing_name_for_is_the_opened_under_name_for_an_edit() {
        assert_eq!(
            editing_name_for(false, "n8n-trigger"),
            Some("n8n-trigger".to_string())
        );
    }

    #[test]
    fn save_intent_fires_confirm_for_new_route_colliding_with_existing() {
        assert_eq!(
            save_intent(true, "", "n8n-trigger", &["n8n-trigger".to_string()]),
            SaveIntent::Confirm {
                colliding_name: "n8n-trigger".to_string()
            }
        );
    }

    #[test]
    fn save_intent_direct_sends_for_in_place_edit_carrying_the_editing_identity() {
        assert_eq!(
            save_intent(
                false,
                "n8n-trigger",
                "n8n-trigger",
                &["n8n-trigger".to_string()]
            ),
            SaveIntent::DirectSend {
                allow_overwrite: false,
                editing_name: Some("n8n-trigger".to_string()),
            },
            "editing a route in place under its own name is the ordinary save, not an overwrite"
        );
    }

    #[test]
    fn save_intent_fires_confirm_when_an_edit_renames_onto_another_route() {
        assert_eq!(
            save_intent(
                false,
                "crm-update",
                "n8n-trigger",
                &["n8n-trigger".to_string(), "crm-update".to_string()]
            ),
            SaveIntent::Confirm {
                colliding_name: "n8n-trigger".to_string()
            }
        );
    }

    #[test]
    fn save_intent_direct_sends_for_a_new_route_with_a_free_name() {
        assert_eq!(
            save_intent(true, "", "brand-new", &["n8n-trigger".to_string()]),
            SaveIntent::DirectSend {
                allow_overwrite: false,
                editing_name: None,
            },
            "a new route with a free name carries no editing identity"
        );
    }

    #[test]
    fn save_intent_direct_sends_for_an_edit_renaming_to_a_free_name_carrying_the_editing_identity()
    {
        assert_eq!(
            save_intent(
                false,
                "n8n-trigger",
                "n8n-trigger-renamed",
                &["n8n-trigger".to_string()]
            ),
            SaveIntent::DirectSend {
                allow_overwrite: false,
                editing_name: Some("n8n-trigger".to_string()),
            },
            "a rename to a free name is a direct send carrying the opened-under name as the \
             editing identity — CR-02's branch `overwrite_collision` had no way to express"
        );
    }

    #[test]
    fn save_intent_direct_sends_for_a_blank_draft_name() {
        assert_eq!(
            save_intent(true, "", "   ", &["n8n-trigger".to_string()]),
            SaveIntent::DirectSend {
                allow_overwrite: false,
                editing_name: None,
            },
            "a blank draft name is a validation concern, not an overwrite concern"
        );
    }
}
