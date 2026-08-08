//! Providers screen — Phase 46.9 Plan 01 (D-01/D-02/D-10): live-wired to
//! `config.yaml` via `provider_config_api::get_provider_config` /
//! `update_provider_config`, replacing the pure visual stub that rendered
//! mock provider rows.
//!
//! Four distinct list states (UI-SPEC 46.9 provider-list): EMPTY (centered
//! `.card` panel), LOADING (2 ghost `.card` blocks at 50% opacity — never
//! collapsed into empty, unlike `models.rs`'s `None`-is-empty conflation),
//! ERROR (`var(--red)` / `12px`, never the `--danger`/`--fs-12` design-token
//! bucket this phase must not pull in), POPULATED (unchanged `.grid.wide`
//! of `ProviderCard`).
//!
//! `P50` renders an em dash "—" (never `0ms`) until real latency tracking
//! exists. The visual vocabulary (`.card`/`.grid.wide`/`.pill`/`.btn`) is
//! preserved pixel-for-pixel per the UI-SPEC Scope & Vocabulary Lock.
//!
//! ── Write side (Task 3, D-01/D-10) ──────────────────────────────────────
//! `EDIT` / `+ NEW PROVIDER` open an inline `provider-editor-form` panel;
//! `SAVE PROVIDER` calls `update_provider_config` and disables + reads
//! "SAVING…" while in flight. After a successful save the local
//! `provider_list_sig` working copy is refreshed by calling
//! `get_provider_config()` directly (a plain async fn call) — deliberately
//! NOT `providers_resource.restart()`. `use_server_future(...)?` early-
//! returns while the resource is in its loading state (see `agents.rs:41-60`
//! for the documented UAT-2 hotfix), which would break hook ordering for
//! every signal declared after the resource line on the render where
//! `.restart()` flips it back to loading. `skills.rs`'s optimistic
//! `toggle_states` seeding is the established precedent this file follows
//! instead: seed a local `Signal<Vec<ProviderSnapshot>>` once from the
//! resource, then mutate that local copy directly from write handlers.
//!
//! `⇣ EXPORT` serializes only non-secret fields (name/base_url/enabled/
//! default_model/api_mode/fallback_providers — never a secret credential)
//! to a downloaded JSON file via a base64-encoded `document::eval` Blob
//! trigger (base64's alphabet cannot break out of the JS string literal it
//! is interpolated into, so this never round-trips operator-entered text
//! directly into the script).
//!
//! ── Secret row (Task 2, D-03/D-04) ──────────────────────────────────────
//! A separate write path over `server::provider_secrets_api`'s vault-backed
//! `set`/`rotate`/`clear_provider_secret` — independent of `SAVE PROVIDER`.
//! Absent -> `NOT CONFIGURED` (dim) + `SET KEY`; present -> `••••••••` +
//! `ROTATE`/`CLEAR`; hard-blocked (vault disabled / read-only `env-var`
//! backend / backend unavailable in this build) -> the row renders the
//! server-reported actionable copy and offers NO action (UAT fix — never
//! present `SET KEY` for a store that cannot persist). Opening the editor
//! for an EXISTING provider fires one presence+blocked
//! `get_provider_secret_status` read (not a page fetch — the rest of the
//! form is pre-filled from already-loaded card data); `+ NEW PROVIDER`
//! seeds presence `false` directly (nothing can be stored yet for a name
//! that does not exist) but still fetches the name-independent blocked
//! state. The real key value
//! is never held in a signal after a write confirms — only the boolean
//! presence flag is. `CLEAR` requires the UI-SPEC confirmation panel
//! (mirrors `schedules.rs`'s `delete_confirm` panel) before calling
//! `clear_provider_secret`.

use dioxus::prelude::*;

use base64::Engine as _;

use crate::server::provider_config_api::{
    get_provider_config, update_provider_config, ProviderSnapshot, ProviderWritePayload,
};
use crate::server::provider_secrets_api::{
    clear_provider_secret, get_provider_secret_status, rotate_provider_secret, set_provider_secret,
};

/// Map a server error into the two-line copy the UI-SPEC specifies. The
/// gate-closed case uses the exact shared copy; anything else falls back to
/// a generic line plus the raw server message.
#[allow(dead_code)] // called from the SAVE PROVIDER onclick closure in
                    // ScreenProviders; dead_code fires under `--all-features --all-targets`
                    // (test target) — same known false-positive as skills.rs's
                    // tab_predicate/search_matches helpers in this crate.
fn map_save_error(e: &ServerFnError) -> (String, String) {
    let msg = e.to_string();
    if msg.contains("Provider already exists") {
        // Gap 3 (D-01, CR-03): the server-side expect_new collision guard
        // rejected a create — surface the same copy the client-side
        // pre-submit check shows, so a bypassed client check (or a race
        // against a concurrent create of the same name) still shows a
        // clear message instead of a silent success.
        (
            "Provider already exists.".to_string(),
            "Rename it or EDIT the existing provider instead.".to_string(),
        )
    } else if msg.contains("disabled") {
        (
            "Config writes are disabled.".to_string(),
            "Set security.web_config_write_enabled: true in config.yaml, then retry.".to_string(),
        )
    } else {
        ("Save failed.".to_string(), msg)
    }
}

/// Map a secret-write server error into the two-line copy the UI-SPEC
/// specifies for `set`/`rotate`/`clear_provider_secret` (D-03/D-04). Order
/// matters: check the vault-disabled hard-block BEFORE the generic
/// write-gate check, since neither error string is a substring of the
/// other but a future copy tweak should not accidentally make one shadow
/// the other.
#[allow(dead_code)] // called from the SET KEY/ROTATE/CLEAR onclick closures
                    // in ScreenProviders; dead_code fires under
                    // `--all-features --all-targets` (test target) — same
                    // known false positive as this file's own
                    // `map_save_error` and skills.rs's `tab_predicate`/
                    // `search_matches` helpers.
fn map_secret_error(e: &ServerFnError) -> (String, String) {
    let msg = e.to_string();
    if msg.contains("read-only (env-var)") {
        // UAT fix: the read-only-backend hard-block (server-side
        // check_backend_writable) — actionable copy, never the raw
        // EnvVarStore backend diagnostic.
        (
            "Vault backend is read-only (env-var).".to_string(),
            "Set vault.backend: \"rusty-vault\" in config.yaml and serve a rusty-vault build \
             (--features rusty-vault), then retry."
                .to_string(),
        )
    } else if msg.contains("Vault is not enabled") {
        (
            "Vault is not enabled.".to_string(),
            "Set vault.enabled: true in config.yaml to store provider secrets, then retry."
                .to_string(),
        )
    } else if msg.contains("Config writes are disabled") {
        (
            "Config writes are disabled.".to_string(),
            "Set security.web_config_write_enabled: true in config.yaml, then retry.".to_string(),
        )
    } else {
        ("Action failed.".to_string(), msg)
    }
}

#[component]
pub fn ScreenProviders(is_active: bool) -> Element {
    let providers_resource = use_server_future(get_provider_config)?;

    // Extract data BEFORE rsx! — signal borrow discipline per
    // iron_hermes_ui/clippy.toml (no GenerationalRef held across RSX).
    let is_loading = providers_resource().is_none();
    let load_error = matches!(providers_resource(), Some(Err(_)));

    // Optimistic local working copy — see module doc for why this is seeded
    // once from the resource rather than driven by `.restart()`.
    let mut provider_list_sig: Signal<Vec<ProviderSnapshot>> = use_signal(Vec::new);
    let mut seeded = use_signal(|| false);
    {
        let loaded = match providers_resource() {
            Some(Ok(ref snap)) => Some(snap.providers.clone()),
            _ => None,
        };
        use_effect(move || {
            if let Some(ref list) = loaded {
                if !*seeded.read() {
                    provider_list_sig.set(list.clone());
                    seeded.set(true);
                }
            }
        });
    }

    let provider_list = provider_list_sig.read().clone();
    let provider_count = provider_list.len();
    let active_count = provider_list.iter().filter(|p| p.enabled).count();

    // ── Editor form state (Task 3) ────────────────────────────────────────
    let mut editor_open = use_signal(|| false);
    let mut editor_is_new = use_signal(|| false);
    let mut editor_name = use_signal(String::new);
    let mut editor_base_url = use_signal(String::new);
    let mut editor_enabled = use_signal(|| true);
    let mut editor_default_model = use_signal(String::new);
    let mut editor_api_mode = use_signal(|| "chat_completions".to_string());
    let mut editor_fallback_providers: Signal<Vec<String>> = use_signal(Vec::new);
    let mut editor_has_secret = use_signal(|| false);
    let mut saving = use_signal(|| false);
    let mut save_error: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut show_restart_banner = use_signal(|| false);

    // ── Secret row state (Task 2, D-03/D-04) ────────────────────────────
    // `None` = presence unknown (initial-fetch in flight for an EDIT open);
    // `Some(true/false)` = vault-secret presence resolved. `+ NEW PROVIDER`
    // seeds `Some(false)` directly — nothing can be stored under a name
    // that does not exist yet, so no read is fired for it.
    let mut secret_status: Signal<Option<bool>> = use_signal(|| None);
    // UAT fix: `Some((line1, line2))` when the server reports secret writes
    // hard-blocked (vault disabled / read-only env-var backend / backend
    // unavailable in this build) — the row renders the actionable copy and
    // hides SET KEY/ROTATE/CLEAR entirely instead of letting the user type
    // a key that can never be stored.
    let mut secret_blocked: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut secret_input_open = use_signal(|| false);
    let mut secret_input_mode = use_signal(|| "set".to_string());
    let mut secret_input_value = use_signal(String::new);
    let mut secret_saving = use_signal(|| false);
    let mut secret_error: Signal<Option<(String, String)>> = use_signal(|| None);
    let mut secret_clear_confirm = use_signal(|| false);

    // Read all signal values into owned locals BEFORE rsx! (Pattern B).
    let editor_open_val = *editor_open.read();
    let editor_is_new_val = *editor_is_new.read();
    let editor_name_val = editor_name.read().clone();
    let editor_base_url_val = editor_base_url.read().clone();
    let editor_enabled_val = *editor_enabled.read();
    let editor_default_model_val = editor_default_model.read().clone();
    let editor_api_mode_val = editor_api_mode.read().clone();
    let editor_fallback_val = editor_fallback_providers.read().clone();
    let editor_has_secret_val = *editor_has_secret.read();
    let saving_val = *saving.read();
    let save_error_val = save_error.read().clone();
    let show_restart_banner_val = *show_restart_banner.read();
    let secret_status_val = *secret_status.read();
    let secret_blocked_val = secret_blocked.read().clone();
    let secret_input_open_val = *secret_input_open.read();
    let secret_input_mode_val = secret_input_mode.read().clone();
    let secret_input_value_val = secret_input_value.read().clone();
    let secret_saving_val = *secret_saving.read();
    let secret_error_val = secret_error.read().clone();
    let secret_clear_confirm_val = *secret_clear_confirm.read();

    // Fallback-provider chip choices: every other known provider name.
    let other_provider_names: Vec<String> = provider_list
        .iter()
        .map(|p| p.name.clone())
        .filter(|n| editor_is_new_val || n != &editor_name_val)
        .collect();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-providers",
            "data-screen-label": "13 Providers",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 13" }
                    h1 { class: "screen-title", "Providers" }
                    p { class: "screen-sub",
                        "API providers and credential pools. "
                        code { style: "color:var(--teal)", "{active_count}" }
                        " active of "
                        code { style: "color:var(--teal)", "{provider_count}" }
                        " configured."
                    }
                }
                div { class: "screen-actions",
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            let list = provider_list_sig.read().clone();
                            let export_rows: Vec<serde_json::Value> = list
                                .iter()
                                .map(|p| serde_json::json!({
                                    "name": p.name,
                                    "base_url": p.base_url,
                                    "enabled": p.enabled,
                                    "default_model": p.default_model,
                                    "api_mode": p.api_mode,
                                    "fallback_providers": p.fallback_providers,
                                }))
                                .collect();
                            let json_str = serde_json::to_string_pretty(&export_rows)
                                .unwrap_or_else(|_| "[]".to_string());
                            let b64 = base64::engine::general_purpose::STANDARD
                                .encode(json_str.as_bytes());
                            spawn(async move {
                                // `document::eval` is Dioxus's client-side JS-bridge
                                // primitive (established pattern: orb_canvas.rs,
                                // voice_loop.rs) — NOT arbitrary user-controlled
                                // code execution. The only interpolated value is
                                // `b64`, a base64 encoding of the export JSON;
                                // base64's alphabet ([A-Za-z0-9+/=]) cannot contain
                                // `"`, `\`, or any character that could break out
                                // of the JS string literal it is placed inside, so
                                // operator-entered provider text (names/URLs) can
                                // never inject into this script regardless of its
                                // contents.
                                let script = format!(
                                    r#"(function(){{const b=atob("{b64}");const bytes=new Uint8Array(b.length);for(let i=0;i<b.length;i++){{bytes[i]=b.charCodeAt(i);}}const blob=new Blob([bytes],{{type:"application/json"}});const url=URL.createObjectURL(blob);const a=document.createElement("a");a.href=url;a.download="providers-export.json";document.body.appendChild(a);a.click();document.body.removeChild(a);URL.revokeObjectURL(url);}})();"#
                                );
                                let _ = document::eval(&script).await;
                            });
                        },
                        "⇣ EXPORT"
                    }
                    button {
                        class: "btn btn--sm",
                        onclick: move |_| {
                            editor_is_new.set(true);
                            editor_name.set(String::new());
                            editor_base_url.set(String::new());
                            editor_enabled.set(true);
                            editor_default_model.set(String::new());
                            editor_api_mode.set("chat_completions".to_string());
                            editor_fallback_providers.set(Vec::new());
                            editor_has_secret.set(false);
                            save_error.set(None);
                            editor_open.set(true);
                            // Task 2: a brand-new provider name cannot have a
                            // vault secret yet — seed presence directly, no
                            // per-name read. UAT fix: the backend BLOCKED
                            // state is name-independent, so still fetch it
                            // (empty name) so a read-only/disabled vault
                            // blocks the row before the user types a key.
                            secret_status.set(Some(false));
                            secret_blocked.set(None);
                            secret_input_open.set(false);
                            secret_input_value.set(String::new());
                            secret_error.set(None);
                            spawn(async move {
                                if let Ok(status) =
                                    get_provider_secret_status(String::new()).await
                                {
                                    secret_blocked.set(status.blocked);
                                }
                            });
                            secret_clear_confirm.set(false);
                        },
                        "+ NEW PROVIDER"
                    }
                }
            }

            if show_restart_banner_val {
                div {
                    class: "panel",
                    style: "border-color:rgba(210,153,34,0.45);background:rgba(210,153,34,0.06);flex-direction:row;align-items:center;justify-content:space-between;gap:12px;flex-wrap:wrap;padding:10px 16px;",
                    span { style: "color:var(--amber);font-size:11px;line-height:1.5;",
                        "Restart required — provider and model changes take effect after restart. Schedule changes apply immediately."
                    }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| show_restart_banner.set(false),
                        "DISMISS"
                    }
                }
            }

            if editor_open_val {
                div { class: "panel", style: "margin-top:14px;",
                    div { class: "panel-title",
                        if editor_is_new_val { "New Provider" } else { "Edit Provider" }
                    }

                    if let Some((ref line1, ref line2)) = save_error_val {
                        div { style: "color:var(--red);font-size:12px;",
                            div { "{line1}" }
                            div { style: "margin-top:2px;", "{line2}" }
                        }
                    }

                    if editor_has_secret_val {
                        div {
                            style: "color:var(--amber);font-size:11px;line-height:1.5;background:rgba(210,153,34,0.06);border:1px solid rgba(210,153,34,0.3);border-radius:8px;padding:10px 12px;",
                            "This provider already has a key from config.yaml or an environment variable — a vault key will be saved but ignored until that is cleared."
                        }
                    }

                    div { class: "field-row",
                        div { class: "field-label", "Provider name" }
                        input {
                            class: "field-input",
                            readonly: !editor_is_new_val,
                            disabled: !editor_is_new_val,
                            placeholder: "e.g. openrouter",
                            value: "{editor_name_val}",
                            oninput: move |e| editor_name.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label",
                            "Base URL"
                            span { class: "help", "e.g. https://api.example.com/v1" }
                        }
                        input {
                            class: "field-input",
                            placeholder: "https://api.example.com/v1",
                            value: "{editor_base_url_val}",
                            oninput: move |e| editor_base_url.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label", "Enabled" }
                        div {
                            class: if editor_enabled_val { "tgl on" } else { "tgl" },
                            role: "switch",
                            aria_checked: "{editor_enabled_val}",
                            onclick: move |_| editor_enabled.set(!editor_enabled_val),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label", "Default model" }
                        input {
                            class: "field-input",
                            placeholder: "e.g. anthropic/claude-3.5-sonnet",
                            value: "{editor_default_model_val}",
                            oninput: move |e| editor_default_model.set(e.value()),
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label", "API mode" }
                        select {
                            value: "{editor_api_mode_val}",
                            onchange: move |e| editor_api_mode.set(e.value()),
                            option { value: "chat_completions", "chat_completions" }
                            option { value: "anthropic_messages", "anthropic_messages" }
                            option { value: "codex_responses", "codex_responses" }
                        }
                    }
                    div { class: "field-row",
                        div { class: "field-label", "Fallback providers" }
                        div { style: "display:flex;flex-wrap:wrap;gap:6px;",
                            for other_name in other_provider_names.iter() {
                                {
                                    let name_c = other_name.clone();
                                    let is_selected = editor_fallback_val.contains(other_name);
                                    rsx! {
                                        button {
                                            key: "{other_name}",
                                            r#type: "button",
                                            class: if is_selected { "pill teal" } else { "pill" },
                                            onclick: move |_| {
                                                let mut list = editor_fallback_providers.write();
                                                if let Some(pos) = list.iter().position(|x| x == &name_c) {
                                                    list.remove(pos);
                                                } else {
                                                    list.push(name_c.clone());
                                                }
                                            },
                                            "{other_name}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    div { class: "field-row",
                        div { class: "field-label", "API key" }
                        div { style: "display:flex;flex-direction:column;gap:8px;flex:1;",

                            if let Some((ref line1, ref line2)) = secret_error_val {
                                div { style: "color:var(--red);font-size:12px;",
                                    div { "{line1}" }
                                    div { style: "margin-top:2px;", "{line2}" }
                                }
                            }

                            if let Some((ref blocked_line1, ref blocked_line2)) = secret_blocked_val {
                                // UAT fix: secret writes hard-blocked in this
                                // config/build (vault disabled, read-only
                                // env-var backend, or backend unavailable) —
                                // render the actionable copy in place of any
                                // action, same visual treatment as the
                                // hard-block errors. No SET KEY/ROTATE/CLEAR
                                // is offered for a store that cannot persist.
                                div { style: "color:var(--red);font-size:12px;",
                                    div { "{blocked_line1}" }
                                    div { style: "margin-top:2px;", "{blocked_line2}" }
                                }
                            } else if secret_clear_confirm_val {
                                div {
                                    class: "panel",
                                    style: "border-color:rgba(248,81,73,0.45);background:rgba(248,81,73,0.06);padding:12px;",
                                    div { class: "panel-title", style: "color:var(--red);", "Clear API key" }
                                    p { style: "color:var(--text);font-size:12px;margin:0 0 12px 0;",
                                        "This removes the stored key from the vault immediately. The provider stops resolving credentials from the vault until a new key is set. Continue?"
                                    }
                                    div { style: "display:flex;gap:10px;",
                                        button {
                                            class: "btn btn--sm",
                                            style: "background:var(--red);border-color:var(--red);",
                                            disabled: secret_saving_val,
                                            onclick: move |_| {
                                                // Pattern B: no borrow across .await.
                                                let name_local = editor_name.read().clone();
                                                secret_saving.set(true);
                                                secret_error.set(None);
                                                spawn(async move {
                                                    match clear_provider_secret(name_local).await {
                                                        Ok(()) => {
                                                            secret_status.set(Some(false));
                                                            secret_saving.set(false);
                                                            secret_clear_confirm.set(false);
                                                        }
                                                        Err(e) => {
                                                            secret_saving.set(false);
                                                            secret_clear_confirm.set(false);
                                                            secret_error.set(Some(map_secret_error(&e)));
                                                        }
                                                    }
                                                });
                                            },
                                            if secret_saving_val { "SAVING…" } else { "CLEAR" }
                                        }
                                        button {
                                            class: "btn btn--ghost btn--sm",
                                            disabled: secret_saving_val,
                                            onclick: move |_| secret_clear_confirm.set(false),
                                            "CANCEL"
                                        }
                                    }
                                }
                            } else if secret_input_open_val {
                                div { style: "display:flex;flex-direction:column;gap:8px;",
                                    input {
                                        r#type: "password",
                                        class: "field-input",
                                        placeholder: "Paste API key",
                                        value: "{secret_input_value_val}",
                                        oninput: move |e| secret_input_value.set(e.value()),
                                    }
                                    div { style: "display:flex;gap:10px;",
                                        button {
                                            class: "btn btn--sm",
                                            disabled: secret_saving_val || secret_input_value_val.trim().is_empty(),
                                            onclick: move |_| {
                                                // Pattern B: no borrow across .await.
                                                let name_local = editor_name.read().clone();
                                                let key_local = secret_input_value.read().clone();
                                                let mode_local = secret_input_mode.read().clone();
                                                secret_saving.set(true);
                                                secret_error.set(None);
                                                spawn(async move {
                                                    let result = if mode_local == "rotate" {
                                                        rotate_provider_secret(name_local, key_local).await
                                                    } else {
                                                        set_provider_secret(name_local, key_local).await
                                                    };
                                                    match result {
                                                        Ok(()) => {
                                                            // No optimistic mask flip — only
                                                            // set after the write confirms.
                                                            secret_status.set(Some(true));
                                                            secret_saving.set(false);
                                                            secret_input_open.set(false);
                                                            secret_input_value.set(String::new());
                                                        }
                                                        Err(e) => {
                                                            secret_saving.set(false);
                                                            secret_error.set(Some(map_secret_error(&e)));
                                                        }
                                                    }
                                                });
                                            },
                                            if secret_saving_val {
                                                "SAVING…"
                                            } else if secret_input_mode_val == "rotate" {
                                                "SAVE KEY"
                                            } else {
                                                "SET KEY"
                                            }
                                        }
                                        button {
                                            class: "btn btn--ghost btn--sm",
                                            disabled: secret_saving_val,
                                            onclick: move |_| {
                                                secret_input_open.set(false);
                                                secret_input_value.set(String::new());
                                            },
                                            "CANCEL"
                                        }
                                    }
                                }
                            } else if secret_status_val == Some(true) {
                                div { style: "display:flex;align-items:center;gap:12px;",
                                    span {
                                        style: "color:var(--text-dim);letter-spacing:0.08em;",
                                        "••••••••"
                                    }
                                    button {
                                        class: "btn btn--ghost btn--sm",
                                        disabled: editor_name_val.trim().is_empty(),
                                        onclick: move |_| {
                                            secret_input_mode.set("rotate".to_string());
                                            secret_input_value.set(String::new());
                                            secret_input_open.set(true);
                                        },
                                        "ROTATE"
                                    }
                                    button {
                                        class: "btn btn--ghost btn--sm",
                                        onclick: move |_| secret_clear_confirm.set(true),
                                        "CLEAR"
                                    }
                                }
                            } else if secret_status_val == Some(false) {
                                div { style: "display:flex;align-items:center;gap:12px;",
                                    span { style: "color:var(--gray);", "NOT CONFIGURED" }
                                    button {
                                        class: "btn btn--sm",
                                        disabled: editor_name_val.trim().is_empty(),
                                        onclick: move |_| {
                                            secret_input_mode.set("set".to_string());
                                            secret_input_value.set(String::new());
                                            secret_input_open.set(true);
                                        },
                                        "SET KEY"
                                    }
                                }
                            } else {
                                // secret_status_val == None: initial presence
                                // read still in flight (EDIT on an existing
                                // provider) — never renders SET KEY/ROTATE/CLEAR
                                // until the real vault state is known.
                                span { style: "color:var(--gray);", "…" }
                            }
                        }
                    }

                    div { style: "display:flex;gap:10px;margin-top:6px;",
                        button {
                            class: "btn btn--sm",
                            disabled: saving_val,
                            onclick: move |_| {
                                // Pattern B: read all signal values into owned
                                // locals BEFORE spawn — no borrow across .await.
                                let name_local = editor_name.read().clone();
                                let base_url_local = editor_base_url.read().clone();
                                let enabled_local = *editor_enabled.read();
                                let default_model_local = editor_default_model.read().clone();
                                let api_mode_local = editor_api_mode.read().clone();
                                let fallback_local = editor_fallback_providers.read().clone();
                                let is_new_local = *editor_is_new.read();

                                // Gap 3 (D-01, CR-03): client-side collision
                                // guard — a + NEW PROVIDER save must never
                                // silently overwrite a live provider on a
                                // name collision. Defense-in-depth only; the
                                // server's expect_new guard in
                                // update_provider_config is the integrity
                                // boundary (T-46.9-17/18).
                                if is_new_local {
                                    let collides = provider_list_sig
                                        .read()
                                        .iter()
                                        .any(|p| p.name == name_local);
                                    if collides {
                                        save_error.set(Some((
                                            "Provider already exists.".to_string(),
                                            "Rename it or EDIT the existing provider instead."
                                                .to_string(),
                                        )));
                                        return;
                                    }
                                }

                                saving.set(true);
                                save_error.set(None);

                                spawn(async move {
                                    let payload = ProviderWritePayload {
                                        name: name_local,
                                        base_url: if base_url_local.trim().is_empty() {
                                            None
                                        } else {
                                            Some(base_url_local)
                                        },
                                        enabled: Some(enabled_local),
                                        default_model: if default_model_local.trim().is_empty() {
                                            None
                                        } else {
                                            Some(default_model_local)
                                        },
                                        api_mode: Some(api_mode_local),
                                        fallback_providers: Some(fallback_local),
                                        expect_new: is_new_local,
                                    };
                                    match update_provider_config(payload).await {
                                        Ok(()) => {
                                            // Re-fetch authoritative state directly
                                            // (NOT providers_resource.restart() —
                                            // see module doc).
                                            if let Ok(fresh) = get_provider_config().await {
                                                provider_list_sig.set(fresh.providers);
                                            }
                                            saving.set(false);
                                            editor_open.set(false);
                                            show_restart_banner.set(true);
                                        }
                                        Err(e) => {
                                            saving.set(false);
                                            save_error.set(Some(map_save_error(&e)));
                                        }
                                    }
                                });
                            },
                            if saving_val { "SAVING…" } else { "SAVE PROVIDER" }
                        }
                        button {
                            class: "btn btn--ghost btn--sm",
                            disabled: saving_val,
                            onclick: move |_| editor_open.set(false),
                            "CANCEL"
                        }
                    }
                }
            }

            if is_loading {
                div { class: "grid wide",
                    ProviderGhostCard {}
                    ProviderGhostCard {}
                }
            } else if load_error {
                div {
                    class: "card",
                    style: "align-items:center;text-align:center;padding:32px 18px;",
                    div { style: "color:var(--red);font-size:12px;font-weight:700;",
                        "Could not load providers."
                    }
                    div { style: "color:var(--red);font-size:12px;margin-top:4px;",
                        "Check the server connection and retry."
                    }
                }
            } else if provider_list.is_empty() {
                div {
                    class: "card",
                    style: "align-items:center;text-align:center;padding:32px 18px;",
                    div { class: "card-title", "No providers configured." }
                    div { class: "card-meta", style: "margin-top:4px;",
                        "Add a provider to start routing chat, kanban, and role-based model calls."
                    }
                }
            } else {
                div { class: "grid wide",
                    for p in provider_list.iter() {
                        ProviderCard {
                            key: "{p.name}",
                            provider: p.clone(),
                            on_edit: move |p: ProviderSnapshot| {
                                editor_is_new.set(false);
                                editor_name.set(p.name.clone());
                                editor_base_url.set(p.base_url.clone().unwrap_or_default());
                                editor_enabled.set(p.enabled);
                                editor_default_model.set(p.default_model.clone().unwrap_or_default());
                                editor_api_mode.set(p.api_mode.clone());
                                editor_fallback_providers.set(p.fallback_providers.clone());
                                editor_has_secret.set(p.has_secret);
                                save_error.set(None);
                                editor_open.set(true);
                                // Task 2: one presence+blocked vault-secret
                                // read for this existing provider — not a
                                // page fetch/skeleton, the rest of the form
                                // is already pre-filled from `p` above. UAT
                                // fix: the response also carries the blocked
                                // state (vault disabled / read-only backend)
                                // so the row blocks BEFORE a key is typed.
                                secret_status.set(None);
                                secret_blocked.set(None);
                                secret_input_open.set(false);
                                secret_input_value.set(String::new());
                                secret_error.set(None);
                                secret_clear_confirm.set(false);
                                let name_for_status = p.name.clone();
                                spawn(async move {
                                    match get_provider_secret_status(name_for_status).await {
                                        Ok(status) => {
                                            secret_blocked.set(status.blocked);
                                            secret_status.set(Some(status.has_secret));
                                        }
                                        Err(_) => {
                                            // Transport failure: fall back to
                                            // the absent state; a write would
                                            // still be server-gated.
                                            secret_blocked.set(None);
                                            secret_status.set(Some(false));
                                        }
                                    }
                                });
                            },
                        }
                    }
                }
            }
        }
    }
}

/// Ghost placeholder card for the loading state — visually distinct from
/// both the empty panel and a populated card (50% opacity, no data).
#[component]
fn ProviderGhostCard() -> Element {
    rsx! {
        div {
            class: "card",
            style: "opacity:0.5;",
            "aria-hidden": "true",
            div { class: "card-head",
                div { class: "card-icon gray", "◉" }
                div { style: "flex:1",
                    div {
                        class: "card-title",
                        style: "background:var(--border-faint);border-radius:4px;color:transparent;width:60%;",
                        "…"
                    }
                    div {
                        class: "card-meta",
                        style: "background:var(--border-faint);border-radius:4px;color:transparent;width:40%;margin-top:4px;",
                        "…"
                    }
                }
            }
        }
    }
}

#[component]
fn ProviderCard(provider: ProviderSnapshot, on_edit: EventHandler<ProviderSnapshot>) -> Element {
    let is_active_card = provider.enabled;
    let pill_class = if is_active_card {
        "pill teal"
    } else {
        "pill amber"
    };
    let pill_label = if is_active_card { "ACTIVE" } else { "IDLE" };
    let has_default = provider.default_model.is_some();
    let base_url_display = provider
        .base_url
        .clone()
        .unwrap_or_else(|| "(no base_url set)".to_string());
    let provider_for_edit = provider.clone();

    rsx! {
        div {
            class: "card",
            class: if is_active_card { "is-active" },
            "data-provider-name": "{provider.name}",

            div { class: "card-head",
                div { class: "card-icon", "◉" }
                div { style: "flex:1;min-width:0;",
                    div {
                        class: "card-title",
                        style: "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                        "{provider.name}"
                    }
                    div {
                        class: "card-meta",
                        style: "overflow:hidden;text-overflow:ellipsis;white-space:nowrap;",
                        "{base_url_display}"
                    }
                }
                div { style: "display:flex;flex-direction:column;gap:4px;align-items:flex-end;",
                    span { class: "{pill_class}", "{pill_label}" }
                    if has_default {
                        span { class: "pill teal", "DEFAULT" }
                    }
                }
            }
            div { class: "card-footer",
                div { style: "display:flex;gap:14px;font-size:10px;color:var(--gray);letter-spacing:0.06em;",
                    span {
                        "MODELS "
                        span { style: "color:var(--teal);font-weight:700",
                            "{provider.model_count}"
                        }
                    }
                    span {
                        "P50 "
                        span { style: "color:var(--teal);font-weight:700", "—" }
                    }
                    if provider.has_secret {
                        span { style: "color:var(--gray);", "KEY SET" }
                    }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| on_edit.call(provider_for_edit.clone()),
                    "EDIT"
                }
            }
        }
    }
}
