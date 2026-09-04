//! Chat platform cards — Phase 49.3 Plan 01 (tracer slice) built the live
//! Telegram card end to end. Plan 03 (D-06/D-07/D-08) extends this to all
//! four chat platforms this gateway hosts (Telegram/Discord/Slack/Buzz),
//! each owning its own `use_resource` over its own `get_*_platform_view`
//! DTO, and wires CONFIGURE on every card to the shared
//! [`crate::components::hermes_app::screens::gateway::chat_config_form::ChatConfigForm`].
//!
//! # Shared shell, per-platform data (Task 2)
//!
//! [`PlatformCardShell`] is the presentational shell — ghost skeleton,
//! error card, and the populated card markup, parameterized by primitive
//! display data — copied verbatim from Plan 01's Telegram-only inline
//! markup, now shared across all four platform components
//! ([`TelegramCard`], [`DiscordCard`], [`SlackCard`], [`BuzzCard`]) so the
//! `.plat-card`/`.plat-head`/`.plat-glyph`/`.plat-name`/`.plat-state`/
//! `.tgl`/`dl.kv` classes are never duplicated a second time. Each data
//! component owns its own resource + toggle-write state (Pattern E — ALL
//! hooks register unconditionally on every render — forbids sharing one
//! hook call across four different DTO types).
//!
//! # Read path
//!
//! `use_resource` over each platform's `get_*_platform_view(scope)`, keyed
//! on `refresh_tick` (47.4 Plan 12 GAP-2 sync-prefix idiom). While pending,
//! renders the E1 loading backstop dimmed ghost skeleton. No secret is ever
//! read in the browser — every DTO carries `token_present: bool` only
//! (plus `app_token_present` for Slack), never the token.
//!
//! # Write path
//!
//! The `.tgl` toggle wires each platform's `set_*_enabled` (gated instant
//! write, D-11) and renders the amber "SAVED — RESTART TO APPLY" staged-
//! apply pill on success (D-09 — no auto-restart). CONFIGURE opens
//! `ChatConfigForm` for the full primary+advanced form (Task 2/3).
//!
//! # Status source (Plan 06, D-08)
//!
//! `platform_status: Option<PlatformStatusMap>` (heartbeat-first,
//! pidfile-fallback, from `mod.rs`'s single `read_platform_status` fetch)
//! is looked up per card by platform key ("telegram"/"discord"/"slack"/
//! "buzz"). `session_count: Some(n)` renders the live heartbeat-driven
//! chat count; `None` (pidfile fallback — no count signal) omits the
//! counts segment entirely rather than rendering a fake zero or a dangling
//! dash (E7 partial).

use crate::server::gateway_platform_status_api::PlatformStatusMap;
use crate::server::platform_config_api::{
    get_buzz_platform_view, get_discord_platform_view, get_slack_platform_view,
    get_telegram_platform_view, set_buzz_enabled, set_discord_enabled, set_slack_enabled,
    set_telegram_enabled, BuzzPlatformView, DiscordPlatformView, SlackPlatformView,
    TelegramPlatformView,
};
use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

use super::chat_config_form::ChatConfigForm;

/// Which of the four chat platforms a card/form instance targets — shared
/// across `chat_platform_cards.rs`, `chat_config_form.rs`, and
/// `whitelist_editor.rs` (the per-platform ID-format hint) so no file
/// re-declares its own copy of this vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // consumed from cfg-gated UI call sites; dead_code fires under --all-features (mutually-exclusive renderer features)
pub enum ChatPlatformKind {
    Telegram,
    Discord,
    Slack,
    Buzz,
}

impl ChatPlatformKind {
    /// Display name used in card headers and form titles.
    pub fn display_name(self) -> &'static str {
        match self {
            ChatPlatformKind::Telegram => "Telegram",
            ChatPlatformKind::Discord => "Discord",
            ChatPlatformKind::Slack => "Slack",
            ChatPlatformKind::Buzz => "Buzz",
        }
    }
}

/// The four chat platform cards, plus the (at most one) open CONFIGURE
/// form. `open_configure` is `None` when no form is open — clicking
/// CONFIGURE on any card sets it to that platform, closing whichever
/// other form (if any) was open (only one form is ever mounted at a time).
#[component]
pub fn ChatPlatformCards(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let open_configure: Signal<Option<ChatPlatformKind>> = use_signal(|| None);

    rsx! {
        TelegramCard {
            scope,
            refresh_tick,
            platform_status: platform_status.clone(),
            open_configure,
        }
        DiscordCard {
            scope,
            refresh_tick,
            platform_status: platform_status.clone(),
            open_configure,
        }
        SlackCard {
            scope,
            refresh_tick,
            platform_status: platform_status.clone(),
            open_configure,
        }
        BuzzCard {
            scope,
            refresh_tick,
            platform_status: platform_status.clone(),
            open_configure,
        }
        if let Some(kind) = *open_configure.read() {
            ChatConfigForm {
                kind,
                scope,
                refresh_tick,
                on_close: {
                    let mut open_configure_sig = open_configure;
                    move |_: ()| {
                        open_configure_sig.set(None);
                    }
                },
            }
        }
    }
}

// =============================================================================
// Per-platform data components — each owns its own resource + toggle-write
// state, then renders through the shared `PlatformCardShell`.
// =============================================================================

#[component]
fn TelegramCard(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
    mut open_configure: Signal<Option<ChatPlatformKind>>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { get_telegram_platform_view(scope_value).await }
    });

    let toggling: Signal<bool> = use_signal(|| false);
    let toggle_error: Signal<Option<String>> = use_signal(|| None);
    let mut staged: Signal<bool> = use_signal(|| false);
    use_effect(move || {
        let _ = view_resource();
        staged.set(false);
    });

    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<TelegramPlatformView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let toggling_val = *toggling.read();
    let toggle_error_val = toggle_error.read().clone();
    let staged_val = *staged.read();
    let enabled_val = view.as_ref().map(|v| v.enabled).unwrap_or(false);
    let configured_val = view.as_ref().map(|v| v.configured).unwrap_or(false);
    let platform_entry = platform_status.as_ref().and_then(|m| m.get("telegram"));
    let count_val: Option<usize> = platform_entry.and_then(|e| e.session_count);
    let connected_val = enabled_val && platform_entry.map(|e| e.connected).unwrap_or(false);

    rsx! {
        PlatformCardShell {
            name: "Telegram".to_string(),
            is_loading,
            load_error,
            configured: configured_val,
            enabled: enabled_val,
            connected: connected_val,
            count: count_val,
            toggling: toggling_val,
            staged: staged_val,
            error: toggle_error_val,
            on_toggle: move |_: ()| {
                if *toggling.peek() {
                    return;
                }
                let next_enabled = !enabled_val;
                let scope_value = scope();
                let mut toggling_sig = toggling;
                let mut toggle_error_sig = toggle_error;
                let mut staged_sig = staged;
                let mut refresh_tick_sig = refresh_tick;
                toggling_sig.set(true);
                spawn(async move {
                    match set_telegram_enabled(scope_value, next_enabled).await {
                        Ok(_new_view) => {
                            toggle_error_sig.set(None);
                            toggling_sig.set(false);
                            staged_sig.set(true);
                            let cur = *refresh_tick_sig.read();
                            refresh_tick_sig.set(cur + 1);
                        }
                        Err(e) => {
                            toggling_sig.set(false);
                            toggle_error_sig.set(Some(e.to_string()));
                        }
                    }
                });
            },
            on_configure: move |_: ()| {
                open_configure.set(Some(ChatPlatformKind::Telegram));
            },
        }
    }
}

#[component]
fn DiscordCard(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
    mut open_configure: Signal<Option<ChatPlatformKind>>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { get_discord_platform_view(scope_value).await }
    });

    let toggling: Signal<bool> = use_signal(|| false);
    let toggle_error: Signal<Option<String>> = use_signal(|| None);
    let mut staged: Signal<bool> = use_signal(|| false);
    use_effect(move || {
        let _ = view_resource();
        staged.set(false);
    });

    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<DiscordPlatformView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let toggling_val = *toggling.read();
    let toggle_error_val = toggle_error.read().clone();
    let staged_val = *staged.read();
    let enabled_val = view.as_ref().map(|v| v.enabled).unwrap_or(false);
    let configured_val = view.as_ref().map(|v| v.configured).unwrap_or(false);
    let platform_entry = platform_status.as_ref().and_then(|m| m.get("discord"));
    let count_val: Option<usize> = platform_entry.and_then(|e| e.session_count);
    let connected_val = enabled_val && platform_entry.map(|e| e.connected).unwrap_or(false);

    rsx! {
        PlatformCardShell {
            name: "Discord".to_string(),
            is_loading,
            load_error,
            configured: configured_val,
            enabled: enabled_val,
            connected: connected_val,
            count: count_val,
            toggling: toggling_val,
            staged: staged_val,
            error: toggle_error_val,
            on_toggle: move |_: ()| {
                if *toggling.peek() {
                    return;
                }
                let next_enabled = !enabled_val;
                let scope_value = scope();
                let mut toggling_sig = toggling;
                let mut toggle_error_sig = toggle_error;
                let mut staged_sig = staged;
                let mut refresh_tick_sig = refresh_tick;
                toggling_sig.set(true);
                spawn(async move {
                    match set_discord_enabled(scope_value, next_enabled).await {
                        Ok(_new_view) => {
                            toggle_error_sig.set(None);
                            toggling_sig.set(false);
                            staged_sig.set(true);
                            let cur = *refresh_tick_sig.read();
                            refresh_tick_sig.set(cur + 1);
                        }
                        Err(e) => {
                            toggling_sig.set(false);
                            toggle_error_sig.set(Some(e.to_string()));
                        }
                    }
                });
            },
            on_configure: move |_: ()| {
                open_configure.set(Some(ChatPlatformKind::Discord));
            },
        }
    }
}

#[component]
fn SlackCard(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
    mut open_configure: Signal<Option<ChatPlatformKind>>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { get_slack_platform_view(scope_value).await }
    });

    let toggling: Signal<bool> = use_signal(|| false);
    let toggle_error: Signal<Option<String>> = use_signal(|| None);
    let mut staged: Signal<bool> = use_signal(|| false);
    use_effect(move || {
        let _ = view_resource();
        staged.set(false);
    });

    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<SlackPlatformView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let toggling_val = *toggling.read();
    let toggle_error_val = toggle_error.read().clone();
    let staged_val = *staged.read();
    let enabled_val = view.as_ref().map(|v| v.enabled).unwrap_or(false);
    let configured_val = view.as_ref().map(|v| v.configured).unwrap_or(false);
    let platform_entry = platform_status.as_ref().and_then(|m| m.get("slack"));
    let count_val: Option<usize> = platform_entry.and_then(|e| e.session_count);
    let connected_val = enabled_val && platform_entry.map(|e| e.connected).unwrap_or(false);

    rsx! {
        PlatformCardShell {
            name: "Slack".to_string(),
            is_loading,
            load_error,
            configured: configured_val,
            enabled: enabled_val,
            connected: connected_val,
            count: count_val,
            toggling: toggling_val,
            staged: staged_val,
            error: toggle_error_val,
            on_toggle: move |_: ()| {
                if *toggling.peek() {
                    return;
                }
                let next_enabled = !enabled_val;
                let scope_value = scope();
                let mut toggling_sig = toggling;
                let mut toggle_error_sig = toggle_error;
                let mut staged_sig = staged;
                let mut refresh_tick_sig = refresh_tick;
                toggling_sig.set(true);
                spawn(async move {
                    match set_slack_enabled(scope_value, next_enabled).await {
                        Ok(_new_view) => {
                            toggle_error_sig.set(None);
                            toggling_sig.set(false);
                            staged_sig.set(true);
                            let cur = *refresh_tick_sig.read();
                            refresh_tick_sig.set(cur + 1);
                        }
                        Err(e) => {
                            toggling_sig.set(false);
                            toggle_error_sig.set(Some(e.to_string()));
                        }
                    }
                });
            },
            on_configure: move |_: ()| {
                open_configure.set(Some(ChatPlatformKind::Slack));
            },
        }
    }
}

#[component]
fn BuzzCard(
    scope: ReadSignal<ConfigScope>,
    refresh_tick: Signal<u32>,
    platform_status: Option<PlatformStatusMap>,
    mut open_configure: Signal<Option<ChatPlatformKind>>,
) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E).
    let view_resource = use_resource(move || {
        let scope_value = scope();
        let _tick = refresh_tick();
        async move { get_buzz_platform_view(scope_value).await }
    });

    let toggling: Signal<bool> = use_signal(|| false);
    let toggle_error: Signal<Option<String>> = use_signal(|| None);
    let mut staged: Signal<bool> = use_signal(|| false);
    use_effect(move || {
        let _ = view_resource();
        staged.set(false);
    });

    let is_loading = view_resource().is_none();
    let load_error: Option<String> = match view_resource() {
        Some(Err(e)) => Some(e.to_string()),
        _ => None,
    };
    let view: Option<BuzzPlatformView> = match view_resource() {
        Some(Ok(v)) => Some(v),
        _ => None,
    };
    let toggling_val = *toggling.read();
    let toggle_error_val = toggle_error.read().clone();
    let staged_val = *staged.read();
    let enabled_val = view.as_ref().map(|v| v.enabled).unwrap_or(false);
    let configured_val = view.as_ref().map(|v| v.configured).unwrap_or(false);
    let platform_entry = platform_status.as_ref().and_then(|m| m.get("buzz"));
    let count_val: Option<usize> = platform_entry.and_then(|e| e.session_count);
    let connected_val = enabled_val && platform_entry.map(|e| e.connected).unwrap_or(false);

    rsx! {
        PlatformCardShell {
            name: "Buzz".to_string(),
            is_loading,
            load_error,
            configured: configured_val,
            enabled: enabled_val,
            connected: connected_val,
            count: count_val,
            toggling: toggling_val,
            staged: staged_val,
            error: toggle_error_val,
            on_toggle: move |_: ()| {
                if *toggling.peek() {
                    return;
                }
                let next_enabled = !enabled_val;
                let scope_value = scope();
                let mut toggling_sig = toggling;
                let mut toggle_error_sig = toggle_error;
                let mut staged_sig = staged;
                let mut refresh_tick_sig = refresh_tick;
                toggling_sig.set(true);
                spawn(async move {
                    match set_buzz_enabled(scope_value, next_enabled).await {
                        Ok(_new_view) => {
                            toggle_error_sig.set(None);
                            toggling_sig.set(false);
                            staged_sig.set(true);
                            let cur = *refresh_tick_sig.read();
                            refresh_tick_sig.set(cur + 1);
                        }
                        Err(e) => {
                            toggling_sig.set(false);
                            toggle_error_sig.set(Some(e.to_string()));
                        }
                    }
                });
            },
            on_configure: move |_: ()| {
                open_configure.set(Some(ChatPlatformKind::Buzz));
            },
        }
    }
}

// =============================================================================
// Shared presentational shell — ghost skeleton / error card / populated
// card, copied verbatim from Plan 01's Telegram-only inline markup. Takes
// only primitive display data — no DTO type dependency — so all four
// platform components above render through the SAME markup rather than
// four copies of it.
// =============================================================================

#[component]
fn PlatformCardShell(
    name: String,
    is_loading: bool,
    load_error: Option<String>,
    configured: bool,
    enabled: bool,
    connected: bool,
    // Plan 06 (D-08, E7 partial): `None` when the status source has no
    // count signal (pidfile fallback) — the counts segment is omitted
    // entirely below rather than rendering a fake zero or a dangling dash.
    count: Option<usize>,
    toggling: bool,
    staged: bool,
    error: Option<String>,
    on_toggle: EventHandler<()>,
    on_configure: EventHandler<()>,
) -> Element {
    rsx! {
        if is_loading {
            // E1 loading backstop: dimmed ghost skeleton, --border-faint
            // border (inherited from .plat-card's own rule), reduced
            // opacity via .plat-card--ghost.
            div { class: "plat-card plat-card--ghost", "aria-hidden": "true",
                div { class: "plat-head",
                    div { class: "plat-glyph", "▦" }
                    div { style: "flex:1",
                        div { class: "plat-name", "{name}" }
                        div { class: "plat-state", "···" }
                    }
                    div { class: "tgl" }
                }
                dl { class: "kv", dt { "Host" } dd { "—" } dt { "Agent" } dd { "—" } }
            }
        } else if let Some(reason) = load_error {
            // E1 error: never a blank/broken card.
            div { class: "plat-card",
                div { class: "plat-head",
                    div { class: "plat-glyph", "▦" }
                    div { style: "flex:1",
                        div { class: "plat-name", "{name}" }
                        div { class: "plat-state", "STATUS UNKNOWN" }
                    }
                    div { class: "tgl" }
                }
                dl { class: "kv", dt { "Host" } dd { "—" } dt { "Agent" } dd { "—" } }
                p { class: "plat-card-help", "Could not load {name} configuration — {reason}." }
            }
        } else {
            {
                let status_line = if connected {
                    "CONNECTED"
                } else if enabled {
                    "DISCONNECTED"
                } else {
                    "DISABLED"
                };
                rsx! {
                    div {
                        class: "plat-card",
                        class: if connected { "connected" },
                        div { class: "plat-head",
                            div { class: "plat-glyph", "▦" }
                            div { style: "flex:1",
                                div { class: "plat-name", "{name}" }
                                div { class: "plat-state", "{status_line}" }
                            }
                            div {
                                class: if enabled { "tgl on" } else { "tgl" },
                                class: if toggling { "tgl--disabled" },
                                "data-tgl": "true",
                                "aria-label": "Toggle {name} enabled",
                                "aria-disabled": if toggling { "true" },
                                onclick: move |_| {
                                    on_toggle.call(());
                                },
                            }
                        }
                        // E1 partial: a card with missing status fields
                        // renders the dash Host/Agent rows. E7 partial
                        // (Plan 06, D-08): when `count` is `None` (pidfile
                        // fallback has no count signal), the "Chats" row is
                        // omitted entirely — never a fake zero, never a
                        // dangling separator.
                        if configured && enabled {
                            dl { class: "kv",
                                if let Some(c) = count {
                                    dt { "Chats" } dd { "{c}" }
                                }
                                dt { "Status" } dd { "{status_line}" }
                            }
                        } else {
                            dl { class: "kv", dt { "Host" } dd { "—" } dt { "Agent" } dd { "—" } }
                        }
                        // D-06: CONFIGURE is always available — a chat
                        // platform's full form is editable whether or not
                        // it is currently enabled/connected.
                        button {
                            class: "btn btn--ghost btn--sm",
                            style: "align-self:flex-start;",
                            "aria-label": "Configure {name}",
                            onclick: move |_| {
                                on_configure.call(());
                            },
                            "CONFIGURE →"
                        }
                        // D-09: staged-apply pill — a write never itself
                        // restarts the gateway; the operator restarts via
                        // RESTART ALL in mod.rs.
                        if staged {
                            span { class: "pill amber", "SAVED — RESTART TO APPLY" }
                        }
                        if let Some(err) = error.clone() {
                            span { class: "pill red", "SAVE FAILED — {err}." }
                        }
                    }
                }
            }
        }
    }
}
