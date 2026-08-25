//! Phase 48.2 Plan 12 (G-48.2-7): the BUZZ section — the operator's only
//! Tools-page surface for `gateway.platforms["buzz"]`.
//!
//! Follows `mcp_section.rs`'s non-tool-section pattern: module doc stating
//! the honesty contract, `mut refresh_tick: Signal<u32>` taken as the
//! page-level signal, section-level error surfacing, `writable` honored
//! throughout so a closed write gate renders every control inert-by-
//! declaration rather than live-but-silent.
//!
//! # Honesty contract (Task 1)
//!
//! Buzz registers no `Tool`, so nothing buzz-shaped ever reaches the
//! catalog `ToolsetSection` renders — this section is the entire reason a
//! BUZZ card exists at all (G-48.2-7's root cause). When no `buzz:` block
//! exists in `gateway.platforms`, this section renders that as its own
//! honest state (not a disabled-but-configured card) and states that
//! enabling it is exactly what adds the block.
//!
//! # Staged-write discipline (Task 2, D-11)
//!
//! Whitelist/relay_url/channels are edited as a LOCAL staged copy — nothing
//! reaches `config.yaml` until SAVE calls
//! [`crate::server::platform_config_api::set_buzz_edit`]. Mirrors
//! `settings_panel.rs`'s staged-form pattern: a `dirty` flag, an explicit
//! SAVE, a DISCARD path back to the last-loaded values, and outcome
//! surfacing on both success and failure. The seed effect re-seeds the
//! staged copy from a fresh successful load ONLY when `!dirty` — an
//! unrelated `refresh_tick` bump (the ENABLED toggle shares this section's
//! resource) must never discard an in-progress edit here.
//!
//! `channel_trust` has NO staged field and NO write path anywhere in this
//! file — it is rendered read-only with its meaning stated and the
//! config.yaml key path named. Moving a Buzz channel from `Closed` to
//! `Open` removes the whitelist requirement for that channel entirely; that
//! downgrade stays a deliberate, file-level edit on purpose (Task 2
//! prohibition) — this section surfaces the CURRENT value so it is never
//! invisible, but a later reader must not "finish the job" and wire a web
//! toggle for it.
//!
//! # The identity key — profile-scoped, or honestly refused (Task 3)
//!
//! `BUZZ_NSEC` is an nsec: whoever holds it IS the bot on the Nostr
//! network. At `ConfigScope::Profile`, this section mounts the EXISTING
//! `ToolCredentialForm` (48.2-07's hardened profile `.env` write —
//! single-quoted, 0600, atomic, provenance-stamped) — no `.env` write is
//! implemented in this file. At `ConfigScope::Root`, this section renders
//! NO write control at all: `ironhermes_gateway::buzz_identity::resolve_buzz_nsec`
//! reads the `BUZZ_NSEC` env var then the home `.env`, while this page's
//! root credential writes go to the vault or plaintext `config.yaml` —
//! neither of which the resolver reads. A root-scope write here would
//! report success and change nothing the gateway can see (a rejected
//! "silent no-op control" outcome, 47.3 D-03), so [`buzz_identity_form_applies`]
//! decides which of the two treatments renders and is directly tested so a
//! later refactor cannot silently re-enable that no-op.
//!
//! Only the DERIVED public key (`buzz_npub_api::fetch_bot_npub`) is ever
//! displayed — never the secret, never a server fn that could return it.

use crate::server::gateway_status_api::GatewayRuntimeStatus;
use crate::server::platform_config_api::{
    whitelist_denies_all_senders, BuzzEditPayload, BuzzModelDisclosure, BuzzPlatformView,
};
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

use super::credential_form::ToolCredentialForm;

/// The `.env` key name `ironhermes_gateway::buzz_identity::BUZZ_NSEC_ENV`
/// declares — re-declared as a local literal so this file does not take a
/// dependency on the gateway crate just to name a string (mirrors
/// `buzz_npub_api.rs`'s own `IDENTITY_ENV_KEY` re-declaration, same
/// rationale in that file's doc comment).
const BUZZ_NSEC_KEY: &str = "BUZZ_NSEC";

/// Task 3b: the predicate deciding which of the two `BUZZ_NSEC` treatments
/// renders — `true` for profile scope (the credential form, the only
/// scope 48.2-07's write path actually reaches), `false` for root scope
/// (explanation only, no control). A dedicated, tested predicate so a
/// later refactor cannot silently re-enable a root-scope write that would
/// report success while changing nothing the gateway resolver reads
/// (T-48.2-12-06).
fn buzz_identity_form_applies(scope: &ConfigScope) -> bool {
    matches!(scope, ConfigScope::Profile(_))
}

/// Plan 14 (G-48.2-8) Task 2: the MODEL RESOLUTION block's headline
/// caption — states plainly that this IS the resolution that answers a
/// Buzz message, distinct from the sub-job role list rendered below it
/// (the `role_override_note` requirement: one label must not stand in for
/// the whole truth). Pure and directly unit-tested, mirroring
/// `channel_trust_explanation`'s convention below.
#[allow(dead_code)] // used inside BuzzSection's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn buzz_main_resolution_caption() -> &'static str {
    "This is the model that replies to a Buzz message. Sub-job role overrides listed below apply \
     to specific jobs inside a turn, not to the reply itself."
}

/// Plan 14 (G-48.2-8) Task 2: explanatory text for a sub-job role
/// resolution whose model is absent — the role names a provider this
/// scope's config.yaml does not define. Never an empty string, never a
/// bare placeholder dash on its own.
#[allow(dead_code)] // used inside BuzzSection's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn buzz_role_absent_model_text(provider: &str) -> String {
    format!(
        "\"{provider}\" is not defined in this scope's config.yaml — no model to show for this role."
    )
}

/// `channel_trust`'s meaning, for the read-only row (Task 2c). Pure and
/// directly unit-testable — matches this crate's convention of pulling
/// display logic for a fixed small vocabulary out into a standalone fn
/// (`mcp_section.rs`'s `mcp_status_label`/`mcp_status_title` precedent).
#[allow(dead_code)] // used inside BuzzSection's rsx!; dead_code fires on test target (tools.rs TOOLS_CSS precedent)
fn channel_trust_explanation(value: &str) -> &'static str {
    match value {
        "open" => {
            "OPEN treats channel membership alone as sufficient — the whitelist requirement above \
             is removed for this channel. Change this by editing gateway.platforms.buzz.channel_trust \
             in config.yaml."
        }
        _ => {
            "CLOSED (default) requires the sender's key to also be in the whitelist above. Change \
             this by editing gateway.platforms.buzz.channel_trust in config.yaml."
        }
    }
}

/// The BUZZ section — mounted on the Tools screen after MCP SERVERS. Owns
/// its own `get_buzz_platform_view` resource; `scope` and `refresh_tick`
/// are read in the sync prefix so either changing re-fetches (mirrors
/// `McpSection`).
///
/// `runtime_status` is `ScreenTools`'s already-fetched 48.2-11 gateway
/// liveness resource, threaded through as a prop (never a second fetch of
/// the same fact) so Task 3's live-apply honesty statement can render it
/// without a second edit to `tools.rs` — `tools.rs`'s mount call is
/// finalized in Task 1 (`files_modified` scopes Task 3 to this file alone).
/// Task 1/2 accept the prop but do not render it yet.
#[component]
pub fn BuzzSection(
    scope: ConfigScope,
    writable: bool,
    mut refresh_tick: Signal<u32>,
    runtime_status: Option<GatewayRuntimeStatus>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let save_error: Signal<Option<String>> = use_signal(|| None);
    let toggling: Signal<bool> = use_signal(|| false);

    let scope_for_resource = scope.clone();
    let view_resource = use_resource(move || {
        let scope_value = scope_for_resource.clone();
        let _tick = refresh_tick();
        async move { crate::server::platform_config_api::get_buzz_platform_view(scope_value).await }
    });

    // Task 3: the derived-npub fetch. Only reachable at profile scope — at
    // root scope the resource resolves to a content-free "not available"
    // answer without ever calling the server fn, since root has no single
    // profile identity this control could name (module doc's "identity
    // key" section). Registered unconditionally regardless of scope
    // (Pattern E); the branch lives INSIDE the async block, not around the
    // hook call.
    let profile_name_for_npub = match &scope {
        ConfigScope::Profile(name) => Some(name.clone()),
        ConfigScope::Root => None,
    };
    let npub_resource = use_resource(move || {
        let name = profile_name_for_npub.clone();
        let _tick = refresh_tick();
        async move {
            match name {
                Some(n) => crate::server::buzz_npub_api::fetch_bot_npub(n).await,
                None => Ok(crate::server::buzz_npub_api::BotNpubResult {
                    npub: None,
                    inherited: false,
                    own_env_unreadable: false,
                }),
            }
        }
    });

    // Task 2 staged working copy (D-11) — every field here is local until
    // SAVE. `staged_relay_url` uses `""` for "no relay configured" (the
    // text input's own natural empty state); `set_buzz_edit`/its payload
    // normalize an empty-after-trim string to `None`.
    let mut staged_whitelist: Signal<Vec<String>> = use_signal(Vec::new);
    let mut staged_relay_url: Signal<String> = use_signal(String::new);
    let mut staged_channels: Signal<Vec<String>> = use_signal(Vec::new);
    let mut dirty: Signal<bool> = use_signal(|| false);
    let mut edit_saving: Signal<bool> = use_signal(|| false);
    let mut edit_save_errors: Signal<Vec<String>> = use_signal(Vec::new);

    // Seed/re-seed from a fresh successful load — never while the operator
    // has an in-progress edit (module doc's staged-write discipline
    // section). `dirty` is read via `.peek()` so toggling it does not
    // itself retrigger this effect — only a genuine resource change does
    // (`settings_panel.rs` precedent).
    use_effect(move || {
        if let Some(Ok(v)) = view_resource() {
            if !*dirty.peek() {
                staged_whitelist.set(v.whitelist.clone());
                staged_relay_url.set(v.relay_url.clone().unwrap_or_default());
                staged_channels.set(v.channels.clone());
                edit_save_errors.set(Vec::new());
            }
        }
    });

    // Extract every value out of the resource BEFORE rsx! — no signal
    // borrow held across the macro (iron_hermes_ui/clippy.toml).
    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<BuzzPlatformView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let save_error_val = save_error();
    let toggling_val = *toggling.read();
    let staged_whitelist_val = staged_whitelist.read().clone();
    let staged_relay_url_val = staged_relay_url.read().clone();
    let dirty_val = *dirty.read();
    let edit_saving_val = *edit_saving.read();
    let edit_save_errors_val = edit_save_errors.read().clone();
    let whitelist_warns = whitelist_denies_all_senders(&staged_whitelist_val);
    let npub_result: Option<crate::server::buzz_npub_api::BotNpubResult> = match npub_resource() {
        Some(Ok(r)) => Some(r),
        _ => None,
    };
    let (runtime_pill_class, runtime_pill_label, runtime_pill_detail) =
        super::runtime_section::runtime_pill(runtime_status.as_ref());
    let identity_form_applies = buzz_identity_form_applies(&scope);

    let scope_for_toggle = scope.clone();
    let scope_for_save = scope.clone();
    let scope_for_identity = scope.clone();

    rsx! {
        div { class: "buzz-section", id: "buzz-section",
            div { class: "buzz-section-header",
                div { class: "section-label", "BUZZ" }
            }
            // Task 3d (D-12): stated ONCE for the whole section — Buzz runs
            // in the separate gateway process, so every edit above (and
            // below) takes effect only when that process restarts. Shown
            // beside the live gateway state from 48.2-11's status surface
            // (never a second fetch of the same fact — `runtime_status` is
            // a prop) so the operator can see whether the process they need
            // to restart is even running, rather than implying an instant
            // apply.
            div { class: "runtime-status-row",
                span { class: "{runtime_pill_class}", "{runtime_pill_label}" }
                if let Some(detail) = runtime_pill_detail {
                    span { class: "runtime-status-detail", "{detail}" }
                }
            }
            p { class: "buzz-channel-trust-explain",
                "Buzz runs inside the separate gateway process — every edit in this section takes effect when that process restarts, not instantly."
            }
            if is_loading {
                div { class: "buzz-section-skeleton",
                    span { class: "pill", "···" }
                }
            } else if let Some(reason) = load_error {
                div { class: "tools-error-banner",
                    span { "COULD NOT LOAD BUZZ CONFIGURATION — {reason}" }
                    button {
                        class: "btn btn--ghost btn--sm",
                        onclick: move |_| {
                            let cur = *refresh_tick.read();
                            refresh_tick.set(cur + 1);
                        },
                        "RETRY"
                    }
                }
            } else if let Some(v) = view {
                if !v.configured {
                    div { class: "buzz-not-configured",
                        p { class: "buzz-not-configured-body",
                            "No buzz: block exists in this scope's config.yaml yet. Turning this on adds one."
                        }
                    }
                }
                div { class: "buzz-enabled-row",
                    span { class: "buzz-field-label", "ENABLED" }
                    div {
                        class: if v.enabled { "tgl on" } else { "tgl" },
                        class: if !writable || toggling_val { "tgl--disabled" },
                        "aria-label": "Toggle Buzz enabled",
                        "aria-disabled": if !writable || toggling_val { "true" },
                        onclick: move |_| {
                            if !writable || *toggling.peek() {
                                return;
                            }
                            let next_enabled = !v.enabled;
                            let scope_value = scope_for_toggle.clone();
                            let mut toggling_sig = toggling;
                            let mut save_error_sig = save_error;
                            let mut refresh_tick_sig = refresh_tick;
                            toggling_sig.set(true);
                            spawn(async move {
                                match crate::server::platform_config_api::set_buzz_enabled(
                                    scope_value,
                                    next_enabled,
                                )
                                    .await
                                {
                                    Ok(_new_view) => {
                                        save_error_sig.set(None);
                                        toggling_sig.set(false);
                                        let cur = *refresh_tick_sig.read();
                                        refresh_tick_sig.set(cur + 1);
                                    }
                                    Err(e) => {
                                        toggling_sig.set(false);
                                        save_error_sig.set(Some(e.to_string()));
                                    }
                                }
                            });
                        },
                    }
                }
                if let Some(err) = save_error_val {
                    span { class: "pill red tools-save-failed-chip", "SAVE FAILED — {err}." }
                }

                // ---------------------------------------------------------
                // Task 2 — staged whitelist / relay_url / channels editor
                // ---------------------------------------------------------
                div { class: "buzz-field-group",
                    span { class: "buzz-field-label", "SENDER WHITELIST" }
                    p { class: "buzz-field-help",
                        "An empty whitelist denies every sender — this is the field's own contract, not a bug."
                    }
                    if whitelist_warns {
                        div { class: "credential-notice", role: "note",
                            "Whitelist is empty — every sender will be denied until an entry is added."
                        }
                    }
                    BuzzListEditor {
                        items: staged_whitelist,
                        writable,
                        add_label: "ADD SENDER".to_string(),
                        row_aria_prefix: "Whitelist".to_string(),
                        dirty,
                    }
                }

                div { class: "buzz-field-group",
                    label {
                        class: "buzz-field-label",
                        r#for: "buzz-relay-url-input",
                        "RELAY URL"
                    }
                    input {
                        id: "buzz-relay-url-input",
                        class: "buzz-relay-url-input",
                        "aria-label": "Buzz relay URL",
                        disabled: !writable,
                        placeholder: "wss://relay.example.com",
                        value: "{staged_relay_url_val}",
                        oninput: move |evt| {
                            if !writable {
                                return;
                            }
                            staged_relay_url.set(evt.value());
                            dirty.set(true);
                        },
                    }
                }

                div { class: "buzz-field-group",
                    span { class: "buzz-field-label", "CHANNELS" }
                    p { class: "buzz-field-help", "NIP-29 channel identifiers this Buzz platform subscribes to." }
                    BuzzListEditor {
                        items: staged_channels,
                        writable,
                        add_label: "ADD CHANNEL".to_string(),
                        row_aria_prefix: "Channel".to_string(),
                        dirty,
                    }
                }

                // ---------------------------------------------------------
                // Task 2 (Plan 14, G-48.2-8) — MODEL RESOLUTION: read-only
                // disclosure of the provider/model that will serve a Buzz
                // reply for this scope, plus configured sub-job role
                // overrides. No staged field, no write path anywhere in
                // this file — PlatformGatewayConfig has no buzz-scoped
                // model key, so a write control here would be a silent
                // no-op (module doc's rejected-pattern precedent).
                // ---------------------------------------------------------
                {
                    let model_disclosure = &v.model_disclosure;
                    rsx! {
                        div { class: "buzz-field-group",
                            span { class: "buzz-field-label", "MODEL RESOLUTION" }
                            if let BuzzModelDisclosure::Resolved(resolved) = model_disclosure {
                                div { class: "buzz-model-resolution-row",
                                    span { class: "pill", "{resolved.main.provider.to_uppercase()}" }
                                    span { class: "buzz-model-value",
                                        "{resolved.main.model.clone().unwrap_or_default()}"
                                    }
                                }
                                p { class: "buzz-field-help", "{buzz_main_resolution_caption()}" }
                                p { class: "buzz-field-help",
                                    "Read from {resolved.config_source}."
                                }
                                if !resolved.roles.is_empty() {
                                    div { class: "buzz-model-roles",
                                        span { class: "buzz-field-label", "SUB-JOB ROLE OVERRIDES" }
                                        p { class: "buzz-field-help",
                                            "These apply to specific jobs inside a turn — summarization, vision, kanban judgment, and similar — not to the reply itself."
                                        }
                                        for role in resolved.roles.iter() {
                                            div { key: "{role.label}", class: "buzz-model-role-row",
                                                span { class: "buzz-model-role-label", "{role.label}" }
                                                span { class: "pill", "{role.provider.to_uppercase()}" }
                                                if let Some(model) = role.model.clone() {
                                                    span { class: "buzz-model-value", "{model}" }
                                                } else {
                                                    span { class: "buzz-model-value buzz-model-value--unset",
                                                        "{buzz_role_absent_model_text(&role.provider)}"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if let BuzzModelDisclosure::Unresolvable { reason } = model_disclosure {
                                span { class: "pill red tools-save-failed-chip",
                                    "MODEL RESOLUTION UNAVAILABLE — {reason}"
                                }
                            }
                            p { class: "buzz-channel-trust-explain",
                                "These values come from the config on disk for the displayed scope and take effect on the gateway's next restart, the same as every other field in this section. The buzz-acp bridge is a SEPARATE path — Buzz.app spawns IronHermes directly and resolves its model from whatever profile that spawn names, not from this page. See docs/ACP.md for how that profile is configured."
                            }
                        }
                    }
                }

                // Task 2c — channel_trust is DISPLAYED, never edited. No
                // staged field, no write path; see module doc.
                div { class: "buzz-channel-trust-row",
                    span { class: "buzz-field-label", "CHANNEL TRUST" }
                    span { class: "pill", "{v.channel_trust.to_uppercase()}" }
                }
                p { class: "buzz-channel-trust-explain",
                    "{channel_trust_explanation(&v.channel_trust)}"
                }

                if !edit_save_errors_val.is_empty() {
                    div { class: "tools-settings-errors",
                        for (i , msg) in edit_save_errors_val.iter().enumerate() {
                            div {
                                key: "{i}",
                                class: "pill red tools-save-failed-chip",
                                "SAVE FAILED — {msg}."
                            }
                        }
                    }
                }

                if writable {
                    div { class: "buzz-edit-actions",
                        button {
                            r#type: "button",
                            class: "btn btn--ghost btn--sm",
                            disabled: edit_saving_val || !dirty_val,
                            "aria-label": "Discard Buzz whitelist/relay/channel edits",
                            onclick: {
                                let v_for_discard = v.clone();
                                move |_| {
                                    staged_whitelist.set(v_for_discard.whitelist.clone());
                                    staged_relay_url.set(v_for_discard.relay_url.clone().unwrap_or_default());
                                    staged_channels.set(v_for_discard.channels.clone());
                                    dirty.set(false);
                                    edit_save_errors.set(Vec::new());
                                }
                            },
                            "DISCARD"
                        }
                        button {
                            r#type: "button",
                            class: "btn btn--primary btn--sm",
                            disabled: edit_saving_val || !dirty_val,
                            "aria-label": "Save Buzz whitelist/relay/channel edits",
                            onclick: move |_| {
                                if *edit_saving.peek() {
                                    return;
                                }
                                let payload = BuzzEditPayload {
                                    whitelist: staged_whitelist.peek().clone(),
                                    relay_url: {
                                        let raw = staged_relay_url.peek().clone();
                                        if raw.trim().is_empty() { None } else { Some(raw) }
                                    },
                                    channels: staged_channels.peek().clone(),
                                };
                                let scope_value = scope_for_save.clone();
                                edit_saving.set(true);
                                edit_save_errors.set(Vec::new());
                                spawn(async move {
                                    match crate::server::platform_config_api::set_buzz_edit(
                                        scope_value,
                                        payload,
                                    )
                                        .await
                                    {
                                        Ok(_new_view) => {
                                            edit_saving.set(false);
                                            dirty.set(false);
                                            edit_save_errors.set(Vec::new());
                                            let cur = *refresh_tick.read();
                                            refresh_tick.set(cur + 1);
                                        }
                                        Err(e) => {
                                            edit_saving.set(false);
                                            edit_save_errors.set(vec![e.to_string()]);
                                        }
                                    }
                                });
                            },
                            if edit_saving_val { "SAVING…" } else { "SAVE" }
                        }
                    }
                }

                // ---------------------------------------------------------
                // Task 3 — the identity key: profile-scoped, or honestly
                // refused (module doc's "The identity key" section).
                // ---------------------------------------------------------
                div { class: "buzz-field-group",
                    span { class: "buzz-field-label", "IDENTITY (BUZZ_NSEC)" }
                    if identity_form_applies {
                        ToolCredentialForm {
                            scope: scope_for_identity.clone(),
                            key_name: BUZZ_NSEC_KEY.to_string(),
                            writable,
                            on_saved: move |_| {
                                let cur = *refresh_tick.read();
                                refresh_tick.set(cur + 1);
                            },
                        }
                        div { class: "credential-row",
                            span { class: "buzz-field-label", "PUBLIC KEY" }
                            if let Some(result) = npub_result.clone() {
                                if let Some(npub) = result.npub {
                                    span { class: "credential-row-value", "{npub}" }
                                    if result.inherited {
                                        span { class: "credential-row-tier", "INHERITED FROM MAIN PROFILE" }
                                    }
                                } else if result.own_env_unreadable {
                                    span { class: "credential-row-value credential-row-value--unset",
                                        "Not available — this profile's .env could not be read."
                                    }
                                } else {
                                    span { class: "credential-row-value credential-row-value--unset",
                                        "Not available yet."
                                    }
                                }
                            } else {
                                span { class: "credential-row-value credential-row-value--unset", "···" }
                            }
                        }
                        p { class: "buzz-channel-trust-explain",
                            "If this key has ever been disclosed, remediate by rotating it (ROTATE above) — not by deleting the file."
                        }
                    } else {
                        div { class: "buzz-not-configured",
                            p { class: "buzz-not-configured-body",
                                "BUZZ_NSEC has no control here at ROOT scope. This page's root credential writes go to the vault or config.yaml, but the Buzz identity resolver reads the BUZZ_NSEC environment variable first, then the home .env directly — neither of which a root-scope write here would reach. A write from this page at root scope would report success and change nothing the gateway can see."
                            }
                            p { class: "buzz-not-configured-body",
                                "Set the key by exporting the BUZZ_NSEC environment variable in the gateway's own environment, or by editing the home .env directly. To manage this key from this page, switch to a PROFILE — the profile-scope route above is the one this page supports."
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Shared add/edit/remove row list editor for `whitelist`/`channels` — takes
/// the staged `Signal<Vec<String>>` directly (`FavoritesRow`'s `pinned:
/// Signal<ToolFavorites>` precedent in `tools.rs`), so a row edit writes
/// straight into the parent's staged signal without an `EventHandler`
/// round trip. `dirty` is bumped on every edit/add/remove.
#[component]
fn BuzzListEditor(
    mut items: Signal<Vec<String>>,
    writable: bool,
    add_label: String,
    row_aria_prefix: String,
    mut dirty: Signal<bool>,
) -> Element {
    let items_val = items.read().clone();

    rsx! {
        div { class: "buzz-list-editor",
            for (i , item) in items_val.iter().cloned().enumerate() {
                div { key: "{i}", class: "buzz-list-editor-row",
                    input {
                        class: "buzz-list-editor-input",
                        "aria-label": "{row_aria_prefix} entry {i}",
                        disabled: !writable,
                        value: "{item}",
                        oninput: move |evt| {
                            if !writable {
                                return;
                            }
                            if let Some(slot) = items.write().get_mut(i) {
                                *slot = evt.value();
                            }
                            dirty.set(true);
                        },
                    }
                    if writable {
                        button {
                            r#type: "button",
                            class: "btn btn--ghost btn--sm",
                            "aria-label": "Remove {row_aria_prefix} entry {i}",
                            onclick: move |_| {
                                {
                                    let mut list = items.write();
                                    if i < list.len() {
                                        list.remove(i);
                                    }
                                }
                                dirty.set(true);
                            },
                            "×"
                        }
                    }
                }
            }
            if writable {
                button {
                    r#type: "button",
                    class: "btn btn--ghost btn--sm",
                    onclick: move |_| {
                        items.write().push(String::new());
                        dirty.set(true);
                    },
                    "{add_label}"
                }
            }
        }
    }
}

// =============================================================================
// Pure-fn tests
// =============================================================================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_scope_gets_the_explanation_not_the_form() {
        assert!(
            !buzz_identity_form_applies(&ConfigScope::Root),
            "root scope must render the explanation, never the credential form (T-48.2-12-06)"
        );
    }

    #[test]
    fn profile_scope_gets_the_form() {
        assert!(buzz_identity_form_applies(&ConfigScope::Profile(
            "default".to_string()
        )));
    }

    #[test]
    fn channel_trust_explanation_covers_both_values() {
        assert!(channel_trust_explanation("open").contains("OPEN"));
        assert!(channel_trust_explanation("closed").contains("CLOSED"));
        // Any other/unexpected string falls into the same safe CLOSED-shaped
        // explanation rather than panicking (defensive default).
        assert!(channel_trust_explanation("unexpected").contains("CLOSED"));
    }

    // -------------------------------------------------------------------
    // Plan 14 (G-48.2-8) Task 2 — MODEL RESOLUTION pure display helpers
    // -------------------------------------------------------------------

    #[test]
    fn main_resolution_caption_reads_as_the_reply_serving_model_distinct_from_roles() {
        let caption = buzz_main_resolution_caption();
        assert!(
            caption.contains("replies to a Buzz message"),
            "caption must read as \"this is the model that answers a Buzz message\""
        );
        assert!(
            caption.to_lowercase().contains("role"),
            "caption must distinguish itself from the sub-job role label"
        );
    }

    #[test]
    fn role_absent_model_text_names_the_undefined_provider_never_empty_or_bare_dash() {
        let text = buzz_role_absent_model_text("not-a-configured-provider");
        assert!(
            text.contains("not-a-configured-provider"),
            "text must name the undefined provider"
        );
        assert!(!text.is_empty());
        assert_ne!(
            text.trim(),
            "-",
            "must never be a bare placeholder dash on its own"
        );
    }
}
