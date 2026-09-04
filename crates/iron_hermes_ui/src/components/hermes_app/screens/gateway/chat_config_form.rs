//! Chat-platform CONFIGURE form (E2, D-06) — Slack/Discord/Telegram/Buzz
//! full form with a collapsed ADVANCED tier (bot token, whitelist editor,
//! home channel, `session_timeout_hours`, `max_concurrent_runs`). Filled
//! starting Plan 03 — mounted from `chat_platform_cards.rs`'s CONFIGURE
//! button as a fixed overlay modal (`.gw-form-overlay`/`.gw-form-modal`,
//! screens.css). No `#[server]` fn is added by this file; it renders over
//! the existing per-platform read/write surfaces in `platform_config_api.rs`
//! plus the write-only secret writer in `gateway_env_secret_api.rs`.
//!
//! # One form, four platforms (Task 2)
//!
//! [`ChatConfigForm`] is parameterized by [`ChatPlatformKind`] rather than
//! being four near-duplicate components — it fetches through whichever
//! platform's `get_*_platform_view` matches `kind` inside ONE
//! `use_resource` (Pattern E: a single hook call, branching happens inside
//! the async block, not around the hook), normalizes the result into
//! [`ChatFormData`] (a DTO-shape-independent staged-form snapshot), and
//! saves through whichever `set_*_edit`/`set_gateway_secret` calls `kind`
//! names.
//!
//! # Bot token is write-only (D-06, 48.2 D-06 carried forward)
//!
//! The token field always starts BLANK (never prefilled from
//! `token_present` — there is no value to prefill, only a presence flag).
//! A blank field on SAVE means "keep the existing secret" — the token
//! write is SKIPPED entirely, not sent as an empty string. A non-blank
//! field routes through [`crate::server::gateway_env_secret_api::set_gateway_secret`]
//! — never through `set_*_edit`, and never echoed back into any DTO.
//!
//! # Buzz is the one platform without a bot-token field
//!
//! `PlatformGatewayConfig`'s Buzz identity (`BUZZ_NSEC`) is resolved by a
//! COMPLETELY different mechanism (`ironhermes_gateway::buzz_identity`,
//! surfaced via the EXISTING `tools/buzz_section.rs` `ToolCredentialForm`
//! on the Tools page — out of this file's scope). `BuzzPlatformView` has
//! no `token_present` field and `set_buzz_edit` has no token parameter, so
//! [`kind_has_bot_token`] gates the token field's very existence for this
//! form — never a dead/no-op control. The Buzz variant instead renders
//! `relay_url`/`channels`/`channel_trust`, reusing the SAME `channel_trust:
//! Open` warning copy `buzz_section.rs` already uses (never restated —
//! module-local [`buzz_open_trust_warning`] returns the identical string).
//!
//! # No ADVANCED tier for Buzz
//!
//! `session_timeout_hours`/`max_concurrent_runs` are not part of
//! `BuzzPlatformView`/`set_buzz_edit` (48.2's existing surface, out of this
//! plan's `files_modified` scope) — [`kind_has_advanced_tier`] gates the
//! ADVANCED tier the same way `kind_has_bot_token` gates the token field,
//! so this form never renders a control that would silently do nothing on
//! save.

use dioxus::prelude::*;

use crate::server::gateway_env_secret_api::set_gateway_secret;
use crate::server::platform_config_api::{
    set_buzz_edit, set_discord_edit, set_slack_edit, set_telegram_edit, whitelist_denies_all_senders,
    BuzzEditPayload, ChatEditPayload,
};
use crate::server::tools_config_api::ConfigScope;

use super::chat_platform_cards::ChatPlatformKind;
use super::whitelist_editor::WhitelistEditor;

/// Whether `kind` has a write-only bot-token field on this form at all
/// (module doc's "Buzz is the one platform without a bot-token field").
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn kind_has_bot_token(kind: ChatPlatformKind) -> bool {
    !matches!(kind, ChatPlatformKind::Buzz)
}

/// Whether `kind` additionally has a Socket-Mode app-level token field
/// (Slack only, D-06).
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn kind_has_app_token(kind: ChatPlatformKind) -> bool {
    matches!(kind, ChatPlatformKind::Slack)
}

/// Whether `kind` has the D-06 ADVANCED tier (`session_timeout_hours`/
/// `max_concurrent_runs`) — false for Buzz (module doc's "No ADVANCED tier
/// for Buzz").
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn kind_has_advanced_tier(kind: ChatPlatformKind) -> bool {
    !matches!(kind, ChatPlatformKind::Buzz)
}

/// The `.env` key name `set_gateway_secret` writes the bot token to — the
/// SAME names `ironhermes-gateway/src/runner.rs`'s `resolve_token_with_env`
/// call sites already read (`DISCORD_BOT_TOKEN`, `SLACK_BOT_TOKEN`); the
/// bare `TELEGRAM_BOT_TOKEN` name is `resolve_token`'s own fallback.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn bot_token_env_name(kind: ChatPlatformKind) -> &'static str {
    match kind {
        ChatPlatformKind::Telegram => "TELEGRAM_BOT_TOKEN",
        ChatPlatformKind::Discord => "DISCORD_BOT_TOKEN",
        ChatPlatformKind::Slack => "SLACK_BOT_TOKEN",
        ChatPlatformKind::Buzz => unreachable!("kind_has_bot_token gates this — Buzz has no bot token field"),
    }
}

/// The `.env` key name for Slack's Socket Mode app-level token — matches
/// `runner.rs`'s `resolve_token_with_env(&slack_config.app_token,
/// "SLACK_APP_TOKEN")` call site.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
const SLACK_APP_TOKEN_ENV_NAME: &str = "SLACK_APP_TOKEN";

/// IN-01: which write-only field(s) a SAVE would replace, or `None` when
/// neither applicable field was typed (a blank-field save commits
/// immediately with no confirm — the existing "blank keeps the existing
/// value" contract is unchanged). Consults [`kind_has_bot_token`]/
/// [`kind_has_app_token`] so a platform without a given field can never
/// produce a heading naming it (Buzz has neither, so it always returns
/// `None` regardless of the booleans passed in).
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn replace_confirm_heading(
    kind: ChatPlatformKind,
    token_typed: bool,
    app_token_typed: bool,
) -> Option<&'static str> {
    let token_typed = token_typed && kind_has_bot_token(kind);
    let app_token_typed = app_token_typed && kind_has_app_token(kind);
    match (token_typed, app_token_typed) {
        (true, true) => Some("REPLACE BOT TOKEN AND APP TOKEN"),
        (true, false) => Some("REPLACE BOT TOKEN"),
        (false, true) => Some("REPLACE APP TOKEN"),
        (false, false) => None,
    }
}

/// Buzz's `channel_trust: Open` loud warning — the IDENTICAL string
/// `tools/buzz_section.rs`'s `channel_trust_explanation("open")` arm
/// returns (module doc's "reused, not restated" requirement; duplicated as
/// a literal here rather than imported since that fn is private to the
/// Tools screen's `buzz_section` submodule — the STRING is what must
/// match, not the code path).
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn buzz_open_trust_warning() -> &'static str {
    "OPEN treats channel membership alone as sufficient — the whitelist requirement above \
     is removed for this channel. Change this by editing gateway.platforms.buzz.channel_trust \
     in config.yaml."
}

/// Buzz's `channel_trust: Closed` explanation — the IDENTICAL default-arm
/// string `channel_trust_explanation`'s `_` branch returns.
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
fn buzz_closed_trust_explanation() -> &'static str {
    "CLOSED (default) requires the sender's key to also be in the whitelist above. Change \
     this by editing gateway.platforms.buzz.channel_trust in config.yaml."
}

/// Unified staged-form snapshot — built from whichever platform's
/// `get_*_platform_view` DTO `kind` names (module doc's "One form, four
/// platforms" section). Buzz-only fields are empty/`None` on
/// Telegram/Discord/Slack; the advanced-tier fields are `0` on Buzz (never
/// rendered there — [`kind_has_advanced_tier`] gates that).
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
struct ChatFormData {
    enabled: bool,
    whitelist: Vec<String>,
    home_channel_id: Option<String>,
    session_timeout_hours: u64,
    max_concurrent_runs: usize,
    token_present: bool,
    app_token_present: bool,
    relay_url: Option<String>,
    channels: Vec<String>,
    channel_trust: Option<String>,
}

#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
impl ChatFormData {
    fn from_telegram(v: crate::server::platform_config_api::TelegramPlatformView) -> Self {
        Self {
            enabled: v.enabled,
            whitelist: v.whitelist,
            home_channel_id: v.home_channel_id,
            session_timeout_hours: v.session_timeout_hours,
            max_concurrent_runs: v.max_concurrent_runs,
            token_present: v.token_present,
            app_token_present: false,
            relay_url: None,
            channels: Vec::new(),
            channel_trust: None,
        }
    }

    fn from_discord(v: crate::server::platform_config_api::DiscordPlatformView) -> Self {
        Self {
            enabled: v.enabled,
            whitelist: v.whitelist,
            home_channel_id: v.home_channel_id,
            session_timeout_hours: v.session_timeout_hours,
            max_concurrent_runs: v.max_concurrent_runs,
            token_present: v.token_present,
            app_token_present: false,
            relay_url: None,
            channels: Vec::new(),
            channel_trust: None,
        }
    }

    fn from_slack(v: crate::server::platform_config_api::SlackPlatformView) -> Self {
        Self {
            enabled: v.enabled,
            whitelist: v.whitelist,
            home_channel_id: v.home_channel_id,
            session_timeout_hours: v.session_timeout_hours,
            max_concurrent_runs: v.max_concurrent_runs,
            token_present: v.token_present,
            app_token_present: v.app_token_present,
            relay_url: None,
            channels: Vec::new(),
            channel_trust: None,
        }
    }

    fn from_buzz(v: crate::server::platform_config_api::BuzzPlatformView) -> Self {
        Self {
            enabled: v.enabled,
            whitelist: v.whitelist,
            home_channel_id: None,
            session_timeout_hours: 0,
            max_concurrent_runs: 0,
            token_present: false,
            app_token_present: false,
            relay_url: v.relay_url,
            channels: v.channels,
            channel_trust: Some(v.channel_trust),
        }
    }
}

/// The chat-platform CONFIGURE form — fixed overlay modal, one instance
/// mounted at a time by `chat_platform_cards.rs`'s `open_configure` signal.
#[component]
pub fn ChatConfigForm(
    kind: ChatPlatformKind,
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    on_close: EventHandler<()>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move {
            match kind {
                ChatPlatformKind::Telegram => {
                    crate::server::platform_config_api::get_telegram_platform_view(scope_value)
                        .await
                        .map(ChatFormData::from_telegram)
                }
                ChatPlatformKind::Discord => {
                    crate::server::platform_config_api::get_discord_platform_view(scope_value)
                        .await
                        .map(ChatFormData::from_discord)
                }
                ChatPlatformKind::Slack => {
                    crate::server::platform_config_api::get_slack_platform_view(scope_value)
                        .await
                        .map(ChatFormData::from_slack)
                }
                ChatPlatformKind::Buzz => {
                    crate::server::platform_config_api::get_buzz_platform_view(scope_value)
                        .await
                        .map(ChatFormData::from_buzz)
                }
            }
        }
    });

    // Staged working copy (D-11) — nothing reaches config.yaml/.env until
    // SAVE PLATFORM CONFIG. Mirrors `buzz_section.rs`'s staged-form
    // discipline: a `dirty` flag, an explicit SAVE, outcome surfacing.
    let mut staged_whitelist: Signal<Vec<String>> = use_signal(Vec::new);
    let mut staged_home_channel: Signal<String> = use_signal(String::new);
    let mut staged_session_timeout: Signal<String> = use_signal(String::new);
    let mut staged_max_concurrent: Signal<String> = use_signal(String::new);
    // Token fields ALWAYS start blank (write-only — module doc). Never
    // seeded from the resource; only cleared on a fresh load or after a
    // successful save (so a stale value cannot linger in the DOM).
    let mut staged_token: Signal<String> = use_signal(String::new);
    let mut staged_app_token: Signal<String> = use_signal(String::new);
    let mut staged_relay_url: Signal<String> = use_signal(String::new);
    let mut staged_channels: Signal<Vec<String>> = use_signal(Vec::new);
    let mut dirty: Signal<bool> = use_signal(|| false);
    let mut advanced_open: Signal<bool> = use_signal(|| false);
    let saving: Signal<bool> = use_signal(|| false);
    let mut save_errors: Signal<Vec<String>> = use_signal(Vec::new);
    let staged_pill: Signal<bool> = use_signal(|| false);
    // IN-01: shown when a non-blank token/app-token would be replaced —
    // mirrors api_server_card.rs's REPLACE API KEY confirm gate.
    let mut show_replace_confirm: Signal<bool> = use_signal(|| false);

    // Seed/re-seed from a fresh successful load — never while the operator
    // has an in-progress edit (buzz_section.rs precedent). Token fields are
    // explicitly cleared here too (never seeded from data — there is no
    // value in the DTO to seed from).
    use_effect(move || {
        if let Some(Ok(v)) = view_resource() {
            if !*dirty.peek() {
                staged_whitelist.set(v.whitelist.clone());
                staged_home_channel.set(v.home_channel_id.clone().unwrap_or_default());
                staged_session_timeout.set(v.session_timeout_hours.to_string());
                staged_max_concurrent.set(v.max_concurrent_runs.to_string());
                staged_token.set(String::new());
                staged_app_token.set(String::new());
                staged_relay_url.set(v.relay_url.clone().unwrap_or_default());
                staged_channels.set(v.channels.clone());
                save_errors.set(Vec::new());
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
    let data: Option<ChatFormData> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let staged_whitelist_val = staged_whitelist.read().clone();
    let staged_home_channel_val = staged_home_channel.read().clone();
    let staged_session_timeout_val = staged_session_timeout.read().clone();
    let staged_max_concurrent_val = staged_max_concurrent.read().clone();
    let staged_token_val = staged_token.read().clone();
    let staged_app_token_val = staged_app_token.read().clone();
    let staged_relay_url_val = staged_relay_url.read().clone();
    let dirty_val = *dirty.read();
    let advanced_open_val = *advanced_open.read();
    let saving_val = *saving.read();
    let save_errors_val = save_errors.read().clone();
    let staged_pill_val = *staged_pill.read();
    let show_replace_confirm_val = *show_replace_confirm.read();
    let whitelist_warns = whitelist_denies_all_senders(&staged_whitelist_val);
    let has_token_field = kind_has_bot_token(kind);
    let has_app_token_field = kind_has_app_token(kind);
    let has_advanced_tier = kind_has_advanced_tier(kind);
    let is_buzz = matches!(kind, ChatPlatformKind::Buzz);
    let channel_trust_val = data.as_ref().and_then(|d| d.channel_trust.clone());
    // IN-01: the heading names exactly which write-only field(s) a
    // non-blank save would replace; `None` while both fields are blank —
    // the confirm never shows and the SAVE button commits immediately.
    let replace_confirm_heading_val = replace_confirm_heading(
        kind,
        !staged_token_val.trim().is_empty(),
        !staged_app_token_val.trim().is_empty(),
    );
    let replace_confirm_body_val = format!(
        "{} — Any connection using the old value stops authenticating after restart. The old value cannot be recovered.",
        replace_confirm_heading_val.unwrap_or("REPLACE TOKEN"),
    );

    // Extracted verbatim from the SAVE button's former inline `onclick`
    // spawn body (IN-01) — shared by the direct-save path (no token typed)
    // and the confirmed-save path (CONFIRM REPLACE). `Copy` — captures
    // only `Signal`s, a `ReadSignal` (`scope`), and the `Copy` `kind`.
    let commit_platform_config = move || {
        if *saving.peek() {
            return;
        }
        let scope_value = scope();
        let token_to_write = staged_token.peek().clone();
        let app_token_to_write = staged_app_token.peek().clone();
        let whitelist_payload = staged_whitelist.peek().clone();
        let home_channel_payload = {
            let raw = staged_home_channel.peek().clone();
            if raw.trim().is_empty() { None } else { Some(raw) }
        };
        let session_timeout_payload = staged_session_timeout
            .peek()
            .parse::<u64>()
            .unwrap_or(24);
        let max_concurrent_payload = staged_max_concurrent
            .peek()
            .parse::<usize>()
            .unwrap_or(8);
        let relay_url_payload = {
            let raw = staged_relay_url.peek().clone();
            if raw.trim().is_empty() { None } else { Some(raw) }
        };
        let channels_payload = staged_channels.peek().clone();

        let mut saving_sig = saving;
        let mut save_errors_sig = save_errors;
        let mut staged_pill_sig = staged_pill;
        let mut dirty_sig = dirty;
        let mut refresh_tick_sig = refresh_tick;
        let mut staged_token_sig = staged_token;
        let mut staged_app_token_sig = staged_app_token;

        saving_sig.set(true);
        save_errors_sig.set(Vec::new());

        spawn(async move {
            // Secret writes FIRST — a failed token
            // write aborts before the non-secret
            // edit is even attempted, so the
            // operator sees the real reason rather
            // than a partial success.
            if kind_has_bot_token(kind) && !token_to_write.trim().is_empty() {
                if let Err(e) = set_gateway_secret(
                    scope_value.clone(),
                    bot_token_env_name(kind).to_string(),
                    token_to_write,
                )
                    .await
                {
                    saving_sig.set(false);
                    save_errors_sig.set(vec![e.to_string()]);
                    return;
                }
            }
            if kind_has_app_token(kind) && !app_token_to_write.trim().is_empty() {
                if let Err(e) = set_gateway_secret(
                    scope_value.clone(),
                    SLACK_APP_TOKEN_ENV_NAME.to_string(),
                    app_token_to_write,
                )
                    .await
                {
                    saving_sig.set(false);
                    save_errors_sig.set(vec![e.to_string()]);
                    return;
                }
            }

            let edit_result: Result<(), ServerFnError> = match kind {
                ChatPlatformKind::Telegram => {
                    set_telegram_edit(
                        scope_value.clone(),
                        ChatEditPayload {
                            whitelist: whitelist_payload,
                            home_channel_id: home_channel_payload,
                            session_timeout_hours: session_timeout_payload,
                            max_concurrent_runs: max_concurrent_payload,
                        },
                    )
                        .await
                        .map(|_| ())
                }
                ChatPlatformKind::Discord => {
                    set_discord_edit(
                        scope_value.clone(),
                        ChatEditPayload {
                            whitelist: whitelist_payload,
                            home_channel_id: home_channel_payload,
                            session_timeout_hours: session_timeout_payload,
                            max_concurrent_runs: max_concurrent_payload,
                        },
                    )
                        .await
                        .map(|_| ())
                }
                ChatPlatformKind::Slack => {
                    set_slack_edit(
                        scope_value.clone(),
                        ChatEditPayload {
                            whitelist: whitelist_payload,
                            home_channel_id: home_channel_payload,
                            session_timeout_hours: session_timeout_payload,
                            max_concurrent_runs: max_concurrent_payload,
                        },
                    )
                        .await
                        .map(|_| ())
                }
                ChatPlatformKind::Buzz => {
                    set_buzz_edit(
                        scope_value.clone(),
                        BuzzEditPayload {
                            whitelist: whitelist_payload,
                            relay_url: relay_url_payload,
                            channels: channels_payload,
                        },
                    )
                        .await
                        .map(|_| ())
                }
            };

            match edit_result {
                Ok(()) => {
                    saving_sig.set(false);
                    dirty_sig.set(false);
                    staged_pill_sig.set(true);
                    staged_token_sig.set(String::new());
                    staged_app_token_sig.set(String::new());
                    let cur = *refresh_tick_sig.read();
                    refresh_tick_sig.set(cur + 1);
                }
                Err(e) => {
                    saving_sig.set(false);
                    save_errors_sig.set(vec![e.to_string()]);
                }
            }
        });
    };

    // IN-01: SAVE either commits immediately (no token typed) or reveals
    // the replace-confirm and performs no server call until CONFIRM
    // REPLACE — mirrors api_server_card.rs's `on_save_click`.
    let commit_platform_config_for_click = commit_platform_config;
    let on_save_click = move |_| {
        if *saving.peek() {
            return;
        }
        let token_typed = !staged_token.peek().trim().is_empty();
        let app_token_typed = !staged_app_token.peek().trim().is_empty();
        match replace_confirm_heading(kind, token_typed, app_token_typed) {
            Some(_heading) => {
                show_replace_confirm.set(true);
            }
            None => {
                commit_platform_config_for_click();
            }
        }
    };

    rsx! {
        div {
            class: "gw-form-overlay",
            "aria-label": "Configure {kind.display_name()}",
            onclick: move |_| {
                on_close.call(());
            },
            div {
                class: "gw-form-modal",
                // Stop propagation so a click inside the modal never
                // bubbles up to the overlay's close-on-click-outside
                // handler.
                onclick: move |evt| {
                    evt.stop_propagation();
                },
                div { class: "gw-form-header",
                    span { class: "gw-form-title", "Configure {kind.display_name()}" }
                    button {
                        r#type: "button",
                        class: "btn btn--ghost btn--sm",
                        "aria-label": "Close configure form",
                        onclick: move |_| {
                            on_close.call(());
                        },
                        "×"
                    }
                }

                if is_loading {
                    // E2 loading: a single loading row until the
                    // scope-scoped GET resolves.
                    p { class: "gw-field-help", "···" }
                } else if let Some(reason) = load_error {
                    div { class: "gw-form-errors",
                        span { class: "pill red", "SAVE FAILED — {reason}. Check your connection and retry." }
                    }
                } else if let Some(_d) = data {
                    // ---------------------------------------------------
                    // Primary tier — whitelist, home channel, bot token
                    // (or Buzz's relay_url/channels/channel_trust).
                    // ---------------------------------------------------
                    if has_token_field {
                        div { class: "gw-field-group",
                            label { class: "gw-field-label", r#for: "gw-bot-token-input", "BOT TOKEN" }
                            p { class: "gw-field-help",
                                "Write-only — this field is always blank. Leave it blank to keep the existing token; type a new one to replace it."
                            }
                            input {
                                id: "gw-bot-token-input",
                                class: "gw-input",
                                "aria-label": "{kind.display_name()} bot token",
                                r#type: "password",
                                placeholder: "leave blank to keep the existing token",
                                value: "{staged_token_val}",
                                oninput: move |evt| {
                                    staged_token.set(evt.value());
                                    dirty.set(true);
                                },
                            }
                            if has_app_token_field {
                                label {
                                    class: "gw-field-label",
                                    style: "margin-top:var(--sp-2);",
                                    r#for: "gw-app-token-input",
                                    "APP TOKEN"
                                }
                                p { class: "gw-field-help", "Socket Mode app-level token (xapp-). Same write-only rule as BOT TOKEN." }
                                input {
                                    id: "gw-app-token-input",
                                    class: "gw-input",
                                    "aria-label": "Slack app token",
                                    r#type: "password",
                                    placeholder: "leave blank to keep the existing token",
                                    value: "{staged_app_token_val}",
                                    oninput: move |evt| {
                                        staged_app_token.set(evt.value());
                                        dirty.set(true);
                                    },
                                }
                            }
                        }
                    }

                    if is_buzz {
                        div { class: "gw-field-group",
                            label { class: "gw-field-label", r#for: "gw-relay-url-input", "RELAY URL" }
                            input {
                                id: "gw-relay-url-input",
                                class: "gw-input",
                                "aria-label": "Buzz relay URL",
                                placeholder: "wss://relay.example.com",
                                value: "{staged_relay_url_val}",
                                oninput: move |evt| {
                                    staged_relay_url.set(evt.value());
                                    dirty.set(true);
                                },
                            }
                        }
                        div { class: "gw-field-group",
                            span { class: "gw-field-label", "CHANNELS" }
                            p { class: "gw-field-help", "NIP-29 channel identifiers this Buzz platform subscribes to." }
                            WhitelistEditor {
                                items: staged_channels,
                                kind,
                                writable: true,
                                dirty,
                            }
                        }
                        if let Some(trust) = channel_trust_val.clone() {
                            div { class: "gw-field-group",
                                span { class: "gw-field-label", "CHANNEL TRUST" }
                                span { class: "pill", "{trust.to_uppercase()}" }
                                p { class: "gw-field-help",
                                    if trust == "open" { "{buzz_open_trust_warning()}" } else { "{buzz_closed_trust_explanation()}" }
                                }
                            }
                        }
                    } else {
                        div { class: "gw-field-group",
                            label { class: "gw-field-label", r#for: "gw-home-channel-input", "HOME CHANNEL" }
                            input {
                                id: "gw-home-channel-input",
                                class: "gw-input",
                                "aria-label": "{kind.display_name()} home channel",
                                value: "{staged_home_channel_val}",
                                oninput: move |evt| {
                                    staged_home_channel.set(evt.value());
                                    dirty.set(true);
                                },
                            }
                        }
                    }

                    if !is_buzz {
                        if whitelist_warns {
                            div { class: "gw-whitelist-empty", role: "note", "Whitelist is empty — every sender will be denied until an entry is added." }
                        }
                        WhitelistEditor {
                            items: staged_whitelist,
                            kind,
                            writable: true,
                            dirty,
                        }
                    }

                    // ---------------------------------------------------
                    // Collapsed ADVANCED tier — session_timeout_hours,
                    // max_concurrent_runs. Not rendered for Buzz
                    // (module doc's "No ADVANCED tier for Buzz").
                    // ---------------------------------------------------
                    if has_advanced_tier {
                        button {
                            r#type: "button",
                            class: "btn btn--ghost btn--sm gw-advanced-toggle",
                            "aria-expanded": if advanced_open_val { "true" } else { "false" },
                            onclick: move |_| {
                                let cur = *advanced_open.read();
                                advanced_open.set(!cur);
                            },
                            if advanced_open_val { "▾ ADVANCED" } else { "▸ ADVANCED" }
                        }
                        if advanced_open_val {
                            div { class: "gw-field-group",
                                label { class: "gw-field-label", r#for: "gw-session-timeout-input", "SESSION TIMEOUT HOURS" }
                                input {
                                    id: "gw-session-timeout-input",
                                    class: "gw-input",
                                    "aria-label": "Session inactivity timeout in hours",
                                    r#type: "number",
                                    value: "{staged_session_timeout_val}",
                                    oninput: move |evt| {
                                        staged_session_timeout.set(evt.value());
                                        dirty.set(true);
                                    },
                                }
                            }
                            div { class: "gw-field-group",
                                label { class: "gw-field-label", r#for: "gw-max-concurrent-input", "MAX CONCURRENT RUNS" }
                                input {
                                    id: "gw-max-concurrent-input",
                                    class: "gw-input",
                                    "aria-label": "Maximum concurrent agent runs",
                                    r#type: "number",
                                    value: "{staged_max_concurrent_val}",
                                    oninput: move |evt| {
                                        staged_max_concurrent.set(evt.value());
                                        dirty.set(true);
                                    },
                                }
                            }
                        }
                    }

                    if !save_errors_val.is_empty() {
                        div { class: "gw-form-errors",
                            for (i , msg) in save_errors_val.iter().enumerate() {
                                span { key: "{i}", class: "pill red", "SAVE FAILED — {msg}. Check your connection and retry." }
                            }
                        }
                    }
                    if staged_pill_val {
                        span { class: "pill amber", "SAVED — RESTART TO APPLY" }
                    }

                    // IN-01: shown only when a non-blank token/app-token
                    // was typed — mirrors api_server_card.rs's REPLACE API
                    // KEY confirm. CANCEL performs zero server calls;
                    // CONFIRM REPLACE runs the same commit_platform_config
                    // the direct-save path uses.
                    if show_replace_confirm_val {
                        div { class: "gw-confirm",
                            p {
                                "{replace_confirm_body_val}"
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
                                    onclick: move |_| {
                                        show_replace_confirm.set(false);
                                        commit_platform_config();
                                    },
                                    "CONFIRM REPLACE"
                                }
                            }
                        }
                    }

                    div { class: "gw-form-actions",
                        button {
                            r#type: "button",
                            class: "btn btn--ghost btn--sm",
                            disabled: saving_val || !dirty_val,
                            onclick: {
                                let d_for_discard = _d.clone();
                                move |_| {
                                    staged_whitelist.set(d_for_discard.whitelist.clone());
                                    staged_home_channel.set(d_for_discard.home_channel_id.clone().unwrap_or_default());
                                    staged_session_timeout.set(d_for_discard.session_timeout_hours.to_string());
                                    staged_max_concurrent.set(d_for_discard.max_concurrent_runs.to_string());
                                    staged_token.set(String::new());
                                    staged_app_token.set(String::new());
                                    staged_relay_url.set(d_for_discard.relay_url.clone().unwrap_or_default());
                                    staged_channels.set(d_for_discard.channels.clone());
                                    dirty.set(false);
                                    save_errors.set(Vec::new());
                                    // WR-04: the confirm's heading/body are
                                    // recomputed each render from the staged
                                    // token fields, so once those are reset
                                    // above a shown confirm would silently
                                    // degrade to generic "REPLACE TOKEN"
                                    // wording for an action that no longer
                                    // replaces anything. Never leave a shown
                                    // confirm describing a replacement the
                                    // discard already made impossible.
                                    show_replace_confirm.set(false);
                                }
                            },
                            "DISCARD"
                        }
                        button {
                            r#type: "button",
                            class: "btn btn--primary btn--sm",
                            // Disabled while the replace confirm is showing
                            // so an unconfirmed save cannot fire behind it
                            // (IN-01).
                            disabled: saving_val || !dirty_val || show_replace_confirm_val,
                            "aria-label": "Save {kind.display_name()} platform config",
                            onclick: on_save_click,
                            if saving_val { "SAVING…" } else { "SAVE PLATFORM CONFIG" }
                        }
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
    fn replace_confirm_heading_is_none_when_nothing_typed() {
        assert_eq!(
            replace_confirm_heading(ChatPlatformKind::Telegram, false, false),
            None
        );
    }

    #[test]
    fn replace_confirm_heading_names_the_bot_token() {
        assert_eq!(
            replace_confirm_heading(ChatPlatformKind::Telegram, true, false),
            Some("REPLACE BOT TOKEN")
        );
    }

    #[test]
    fn replace_confirm_heading_names_both_slack_tokens() {
        assert_eq!(
            replace_confirm_heading(ChatPlatformKind::Slack, true, true),
            Some("REPLACE BOT TOKEN AND APP TOKEN")
        );
    }

    #[test]
    fn replace_confirm_heading_names_the_app_token_alone() {
        assert_eq!(
            replace_confirm_heading(ChatPlatformKind::Slack, false, true),
            Some("REPLACE APP TOKEN")
        );
    }

    #[test]
    fn replace_confirm_heading_is_none_for_a_kind_without_token_fields() {
        assert_eq!(
            replace_confirm_heading(ChatPlatformKind::Buzz, true, true),
            None
        );
    }
}
