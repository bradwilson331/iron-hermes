//! REST API server card (E5, D-05) — enabled toggle, host/port,
//! `public_opt_in` with loud warning copy, and the write-only
//! `IRONHERMES_API_SERVER_KEY` field (persisted via `gateway_env_secret_api`).
//! Filled Plan 05 — reads `api_server_config_api.rs` (also filled this
//! plan) for the non-secret host/port/`public_opt_in`/`enabled` fields.
//!
//! # Everything is staged, nothing is instant (unlike the Telegram card)
//!
//! Unlike `chat_platform_cards.rs`'s Telegram toggle (an instant gated
//! write per D-11), `enabled`/`host`/`port`/`public_opt_in` are ALL staged
//! form fields here — `api_server_config_api.rs`'s Task 1 established only
//! `set_api_server_edit`, one combined commit, never a separate instant
//! `set_api_server_enabled`. Nothing here reaches disk until SAVE API
//! CONFIG is clicked.
//!
//! # The key never lands in `set_api_server_edit` (D-05/D-06)
//!
//! A non-blank key field routes through
//! [`crate::server::gateway_env_secret_api::set_gateway_secret`] directly —
//! never through the config-write payload, which has no `api_key` field at
//! all. A non-blank key first shows the REPLACE API KEY destructive
//! confirm (Copywriting Contract); confirming writes the key THEN the
//! non-secret fields, in that order, so an interrupted save never leaves
//! `enabled: true`/`public_opt_in: true` committed without the key that
//! posture depends on. A blank key field means "keep the existing key" —
//! SAVE API CONFIG commits only the non-secret fields, no confirm shown.

use crate::server::api_server_config_api::{
    get_api_server_config, set_api_server_edit, ApiServerConfigView, ApiServerEditPayload,
};
use crate::server::gateway_env_secret_api::set_gateway_secret;
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

/// The env var name the key is written under — never hardcoded a second
/// time below.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
const API_SERVER_KEY_ENV_NAME: &str = "IRONHERMES_API_SERVER_KEY";

#[component]
pub fn ApiServerCard(scope: ReadSignal<ConfigScope>, refresh_tick: Signal<u32>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { get_api_server_config(scope_value).await }
    });

    // Staged form fields — seeded from the loaded view, never sent to the
    // server until SAVE API CONFIG. `port_input` is a String proxy for the
    // numeric field (native text input value), parsed on save.
    let mut host_input: Signal<String> = use_signal(String::new);
    let mut port_input: Signal<String> = use_signal(String::new);
    let mut enabled_input: Signal<bool> = use_signal(|| false);
    let mut public_opt_in_input: Signal<bool> = use_signal(|| false);
    let mut key_input: Signal<String> = use_signal(String::new);

    let saving: Signal<bool> = use_signal(|| false);
    let save_error: Signal<Option<String>> = use_signal(|| None);
    let mut staged: Signal<bool> = use_signal(|| false);
    let mut show_replace_confirm: Signal<bool> = use_signal(|| false);

    // Re-seed the staged fields every time the resource resolves fresh
    // (initial load, a scope change, or the refresh_tick bump this card's
    // own successful save performs) — mirrors chat_platform_cards.rs's
    // "re-run whenever the resource resolves fresh" effect discipline.
    use_effect(move || {
        if let Some(Ok(view)) = view_resource() {
            host_input.set(view.host.clone().unwrap_or_default());
            port_input.set(view.port.map(|p| p.to_string()).unwrap_or_default());
            enabled_input.set(view.enabled);
            public_opt_in_input.set(view.public_opt_in);
            // Never prefilled — write-only field (D-06).
            key_input.set(String::new());
        }
        staged.set(false);
        show_replace_confirm.set(false);
    });

    // Extract every value out of the resource BEFORE the rsx! block — no
    // signal borrow held across the macro (iron_hermes_ui/clippy.toml).
    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<ApiServerConfigView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };

    let host_val = host_input.read().clone();
    let port_val = port_input.read().clone();
    let enabled_val = *enabled_input.read();
    let public_opt_in_val = *public_opt_in_input.read();
    let key_val = key_input.read().clone();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let staged_val = *staged.read();
    let show_replace_confirm_val = *show_replace_confirm.read();
    let key_present_val = view.as_ref().map(|v| v.key_present).unwrap_or(false);

    let status_line = if enabled_val { "ENABLED" } else { "DISABLED" };

    // Commits enabled/host/port/public_opt_in only — never the key. Used
    // both for a blank-key save and as the second step after a
    // successful key write.
    let commit_config = move || {
        let scope_value = scope();
        let host = host_input.peek().clone();
        let port_str = port_input.peek().clone();
        let enabled = *enabled_input.peek();
        let public_opt_in = *public_opt_in_input.peek();
        let mut saving_sig = saving;
        let mut save_error_sig = save_error;
        let mut staged_sig = staged;
        let mut refresh_tick_sig = refresh_tick;

        let Ok(port) = port_str.trim().parse::<u16>() else {
            save_error_sig.set(Some(
                "port must be a number between 1 and 65535".to_string(),
            ));
            return;
        };

        saving_sig.set(true);
        save_error_sig.set(None);
        spawn(async move {
            let payload = ApiServerEditPayload {
                enabled,
                host,
                port,
                public_opt_in,
            };
            match set_api_server_edit(scope_value, payload).await {
                Ok(_new_view) => {
                    saving_sig.set(false);
                    staged_sig.set(true);
                    let cur = *refresh_tick_sig.read();
                    refresh_tick_sig.set(cur + 1);
                }
                Err(e) => {
                    saving_sig.set(false);
                    save_error_sig.set(Some(e.to_string()));
                }
            }
        });
    };

    // Writes the key first, then the non-secret fields, in that order —
    // module doc's "the key never lands in set_api_server_edit" section.
    let commit_config_after_key = commit_config;
    let save_key_then_config = move || {
        let scope_value = scope();
        let value = key_input.peek().clone();
        let mut saving_sig = saving;
        let mut save_error_sig = save_error;
        let mut key_input_sig = key_input;
        let mut show_replace_confirm_sig = show_replace_confirm;

        saving_sig.set(true);
        save_error_sig.set(None);
        show_replace_confirm_sig.set(false);
        spawn(async move {
            match set_gateway_secret(
                scope_value,
                API_SERVER_KEY_ENV_NAME.to_string(),
                value,
            )
            .await
            {
                Ok(_ack) => {
                    key_input_sig.set(String::new());
                    commit_config_after_key();
                }
                Err(e) => {
                    saving_sig.set(false);
                    save_error_sig.set(Some(e.to_string()));
                }
            }
        });
    };

    let commit_config_for_click = commit_config;
    let on_save_click = move |_| {
        if *saving.peek() {
            return;
        }
        if key_input.peek().trim().is_empty() {
            commit_config_for_click();
        } else {
            show_replace_confirm.set(true);
        }
    };

    rsx! {
        div { class: "plat-card",
            div { class: "plat-head",
                div { class: "plat-glyph", "◈" }
                div { style: "flex:1",
                    div { class: "plat-name", "REST API Server" }
                    div { class: "plat-state", "{status_line}" }
                }
                div {
                    class: if enabled_val { "tgl on" } else { "tgl" },
                    class: if saving_val { "tgl--disabled" },
                    "data-tgl": "true",
                    "aria-label": "Toggle REST API server enabled",
                    "aria-disabled": if saving_val { "true" },
                    onclick: move |_| {
                        if *saving.peek() {
                            return;
                        }
                        let next = !*enabled_input.peek();
                        enabled_input.set(next);
                    },
                }
            }
            if is_loading {
                // E5 loading: single loading row until the scope-scoped
                // GET resolves (E2 pattern).
                dl { class: "kv", dt { "Host" } dd { "···" } }
            } else if let Some(reason) = load_error {
                dl { class: "kv", dt { "Host" } dd { "—" } }
                p { class: "plat-card-help", "Could not load REST API server configuration — {reason}." }
            } else if view.is_some() {
                div { class: "gw-form",
                    div { class: "gw-field",
                        label { r#for: "gw-api-host", "Host" }
                        input {
                            id: "gw-api-host",
                            r#type: "text",
                            class: "gw-input gw-input--mono",
                            value: "{host_val}",
                            oninput: move |evt| host_input.set(evt.value()),
                        }
                    }
                    div { class: "gw-field",
                        label { r#for: "gw-api-port", "Port" }
                        input {
                            id: "gw-api-port",
                            r#type: "text",
                            class: "gw-input gw-input--mono",
                            value: "{port_val}",
                            oninput: move |evt| port_input.set(evt.value()),
                        }
                    }
                    div { class: "gw-checkbox-row",
                        input {
                            id: "gw-api-public-opt-in",
                            r#type: "checkbox",
                            checked: public_opt_in_val,
                            onchange: move |evt| public_opt_in_input.set(evt.checked()),
                        }
                        label { r#for: "gw-api-public-opt-in", "PUBLIC OPT-IN — expose beyond loopback" }
                    }
                    if public_opt_in_val {
                        p { class: "gw-warn",
                            "PUBLIC — bound to {host_val}:{port_val}. Anyone reaching this address with the key can drive the agent."
                        }
                    }
                    p { class: "gw-static-note",
                        "FAIL-CLOSED: this listener refuses to start without a key. A non-loopback host additionally requires BOTH the key AND the PUBLIC OPT-IN above."
                    }
                    div { class: "gw-field",
                        label { r#for: "gw-api-key",
                            if key_present_val { "API Key (set — leave blank to keep it)" } else { "API Key (not set)" }
                        }
                        input {
                            id: "gw-api-key",
                            r#type: "password",
                            class: "gw-input gw-input--mono",
                            placeholder: "blank keeps the existing key",
                            value: "{key_val}",
                            oninput: move |evt| key_input.set(evt.value()),
                        }
                    }
                    if show_replace_confirm_val {
                        div { class: "gw-confirm",
                            p {
                                "REPLACE API KEY — Any client using the old key stops authenticating after restart. The old key cannot be recovered."
                            }
                            div { class: "gw-confirm-actions",
                                button {
                                    r#type: "button",
                                    class: "btn btn--ghost btn--sm",
                                    onclick: move |_| show_replace_confirm.set(false),
                                    "CANCEL"
                                }
                                button {
                                    r#type: "button",
                                    class: "btn btn--danger btn--sm",
                                    onclick: move |_| save_key_then_config(),
                                    "CONFIRM REPLACE"
                                }
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "btn btn--sm",
                        style: "align-self:flex-start;",
                        disabled: saving_val,
                        onclick: on_save_click,
                        if saving_val { "SAVING…" } else { "SAVE API CONFIG" }
                    }
                    if staged_val {
                        span { class: "pill amber", "SAVED — RESTART TO APPLY" }
                    }
                    if let Some(err) = save_error_val {
                        span { class: "pill red", "SAVE FAILED — {err}. Check your connection and retry." }
                    }
                }
            }
        }
    }
}
