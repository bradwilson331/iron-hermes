//! Phase 54.4 Plan 01 (D-04, D-05, D-06, D-07, D-26) — the persistent header.
//!
//! One `<header role="banner" class="ih-header">` is mounted inside `div.app`
//! (so the GAP-11 stacking-context fix is preserved) and OUTSIDE
//! `screen_router::ScreenRouter`, which is what puts the same banner on every
//! screen. Its grid has two columns:
//!
//! * column 1 — the wheel, docked: `wheel::Wheel { docked: true }`. The wheel
//!   box (`--ih-wheel-size`) is taller than the banner row (`--ih-header-h`)
//!   and straddles it; the header is `overflow: visible`, so the wheel hangs
//!   `--ih-overhang-h` below the banner by layout alone (D-26: no scale, no
//!   transform, no size transition).
//! * column 2 — two rows: the status strip (row 1, D-07) and the controls
//!   (row 2: the profile switch plus the screen shortcuts, D-06).
//!
//! This module declares NO context provider. Providers live only at the
//! `HermesApp` root; a provider declared in a child compiles and then panics
//! the consumers that sit outside its subtree at runtime. The header only
//! consumes `Signal<Screen>`, `Signal<WheelState>`, the tokens signal and the
//! profile contexts, and never reads or writes chat state or the WebSocket
//! (D-10).
//!
//! The shortcut buttons (>= 1100px) and the `MENU ▾` overflow button (below
//! 1100px) both always render; which one is visible is decided by CSS alone
//! (`assets/dashboard.css`), so there is no JS resize listener.
//!
//! `focus_element_by_id` and `menu_next_index` are `pub(crate)` on purpose:
//! later plans' chat-page and slide-over modules live outside `header/` and
//! reuse them without editing this file.

pub mod agent_dropdown;
pub mod env_chip;

use crate::components::hermes_app::agent_tabs::{
    live_agent_name, AgentNoticeCtx, AgentTabsCtx, LiveAgentCtx, DEFAULT_AGENT,
};
use crate::components::hermes_app::header::env_chip::env_options;
use crate::components::hermes_app::{sys_meta, wheel};
use crate::server::api::ContextWindowSource;
use crate::server::profile_api::{fetch_agent_header_state, AgentHeaderState};
use crate::state::Screen;
use dioxus::prelude::*;

/// The em-dash every unresolved or failed header value renders as.
const DASH: &str = "\u{2014}";

/// What the status strip and the env chip show for the focused agent.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct HeaderValues {
    pub provider: String,
    pub model: String,
    /// TOK is the live session's: shown only for the live agent.
    pub show_tokens: bool,
    /// `title` for PROVIDER and MODEL when the focused agent's config failed
    /// to load (UIC E1-error).
    pub error_title: Option<String>,
    /// The focused agent's terminal backend, or an em-dash.
    pub env_label: String,
}

/// Resolve the header values (D-07). The live agent shows the live
/// `get_config_summary` values; any other agent shows what
/// `fetch_agent_header_state` returned for it (an em-dash while that is
/// unresolved or failed). `live` is the live agent's name, `fetched` the
/// focused agent's fetch result (`None` while unresolved).
pub(crate) fn resolve_header_values(
    focused: &str,
    live: Option<&str>,
    live_provider: &str,
    live_model: &str,
    fetched: Option<&Result<AgentHeaderState, String>>,
) -> HeaderValues {
    let ok = fetched.and_then(|r| r.as_ref().ok());
    let env_label = ok
        .and_then(|s| s.terminal_backend.clone())
        .unwrap_or_else(|| DASH.to_string());
    if live == Some(focused) {
        return HeaderValues {
            provider: live_provider.to_string(),
            model: live_model.to_string(),
            show_tokens: true,
            error_title: None,
            env_label,
        };
    }
    let error_title = matches!(fetched, Some(Err(_))).then(|| {
        format!("Couldn't load {focused}'s config. Showing live values where available.")
    });
    HeaderValues {
        provider: ok
            .and_then(|s| s.provider.clone())
            .unwrap_or_else(|| DASH.to_string()),
        model: ok
            .and_then(|s| s.model.clone())
            .unwrap_or_else(|| DASH.to_string()),
        show_tokens: false,
        error_title,
        env_label,
    }
}

/// The screen shortcut buttons shown beside the wheel (D-06), in display
/// order. Plan 07 (D-08) appended `Logs`.
pub(crate) const HEADER_SHORTCUTS: &[(Screen, &str)] = &[
    (Screen::Kanban, "Kanban"),
    (Screen::Artifacts, "Artifacts"),
    (Screen::Schedules, "Schedules"),
    (Screen::Soul, "Soul"),
    (Screen::Logs, "Logs"),
];

/// Resolve a layout's header list to `(Screen, label)` pairs, in saved order
/// (Phase 54.9 D-02). The layout in `MenuCtx` is always validated, so an id
/// missing from the catalog is skipped rather than reported.
pub(crate) fn header_entries(layout: &crate::menu_layout::MenuLayout) -> Vec<(Screen, String)> {
    layout
        .header
        .iter()
        .filter_map(|e| {
            crate::menu_layout::catalog_entry(&e.screen).map(|c| (c.screen, e.label.clone()))
        })
        .collect()
}

/// At most this many shortcuts render inline at >= 1100px; the rest go in `MORE ▾`.
pub(crate) const HEADER_INLINE_MAX: usize = 5;

/// Split the header list into the inline part and the `MORE ▾` part.
pub(crate) fn split_inline<T>(entries: &[T]) -> (&[T], &[T]) {
    entries.split_at(HEADER_INLINE_MAX.min(entries.len()))
}

/// Keys the overflow menu reacts to for roving focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Up,
    Down,
    Home,
    End,
}

/// Next focus index for a keyboard-navigable menu of `len` items. Up/Down
/// wrap; Home/End jump; an empty menu has no target.
pub(crate) fn menu_next_index(current: Option<usize>, len: usize, key: MenuKey) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match key {
        MenuKey::Home => 0,
        MenuKey::End => len - 1,
        MenuKey::Down => match current {
            Some(i) => (i + 1) % len,
            None => 0,
        },
        MenuKey::Up => match current {
            Some(i) => (i + len - 1) % len,
            None => len - 1,
        },
    })
}

/// Move DOM focus to the element with `id` (wasm: `getElementById().focus()`).
/// `pub(crate)` so the chat tab strip and the settings slide-over can reuse it.
#[cfg(target_arch = "wasm32")]
pub(crate) fn focus_element_by_id(id: &str) {
    use wasm_bindgen::JsCast;
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        if let Ok(html_el) = el.dyn_into::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    }
}

/// Native twin of [`focus_element_by_id`]: there is no DOM to focus.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn focus_element_by_id(id: &str) {
    let _ = id;
}

/// The persistent banner: docked wheel (column 1) and the two header rows
/// (column 2). Every value is passed down from `HermesApp`'s single
/// `get_config_summary` resource; `uptime_secs` is `None` until that
/// resource first resolves.
#[component]
pub fn Header(
    model: String,
    provider: String,
    uptime_secs: Option<u64>,
    context_source: Option<ContextWindowSource>,
) -> Element {
    // D-23 notice slot (root-provided). The auto-dismiss is token-guarded
    // (chat.rs `fire_attach_toast` precedent): a newer notice restarts the 4 s
    // clock, so an earlier timer can never blank a later message. The effect
    // only PEEKS the token it writes, so it cannot retrigger itself.
    let mut notice = use_context::<AgentNoticeCtx>().0;
    let mut toast_token = use_signal(|| 0_u64);
    use_effect(move || {
        if notice.read().is_some() {
            let tok = *toast_token.peek() + 1;
            toast_token.set(tok);
            spawn(async move {
                crate::platform::timer::sleep(4000).await;
                if *toast_token.peek() == tok {
                    notice.set(None);
                }
            });
        }
    });
    let notice_text: Option<String> = notice.read().clone();

    // D-07: PROVIDER / MODEL / ENV follow the FOCUSED agent. The memo means
    // only a focus change refetches; the resource reads it with call syntax in
    // the SYNC prefix (outside the `async move`) so it subscribes. The result
    // is tagged with the name it was fetched for, so the previous agent's
    // values can never be shown under the new agent's name while a refetch is
    // in flight.
    let tabs_ctx = use_context::<AgentTabsCtx>();
    let live_ctx = use_context::<LiveAgentCtx>();
    let focused = use_memo(move || {
        tabs_ctx
            .0
            .read()
            .focused
            .clone()
            .unwrap_or_else(|| DEFAULT_AGENT.to_string())
    });
    // WR-04: the profile-refresh tick is also read in the sync prefix, so a
    // successful settings Save (which bumps it) refetches the focused agent's
    // provider / model / backend instead of leaving the old values on screen.
    let refresh =
        use_context::<crate::components::hermes_app::profile_topbar::TopbarProfileRefreshCtx>().0;
    let header_state = use_resource(move || {
        let name = focused();
        let _refresh_tick = refresh();
        async move {
            let result = fetch_agent_header_state(name.clone())
                .await
                .map_err(|e| e.to_string());
            (name, result)
        }
    });

    // Owned locals BEFORE rsx! so no signal borrow is held across the tree.
    let focused_name = focused();
    let live = live_agent_name(live_ctx.0());
    let fetched: Option<Result<AgentHeaderState, String>> = header_state()
        .filter(|(name, _)| *name == focused_name)
        .map(|(_, result)| result);
    let values = resolve_header_values(&focused_name, live.as_deref(), &provider, &model, fetched.as_ref());
    let env_opts = env_options(fetched.as_ref().and_then(|r| r.as_ref().ok()));
    let show_tokens = values.show_tokens && uptime_secs.is_some();
    let env_label = values.env_label.clone();

    rsx! {
        header { role: "banner", class: "ih-header",
            div { class: "ih-header-wheel",
                wheel::Wheel { docked: true }
            }
            div { class: "ih-header-rows",
                // Row 1 — status strip (D-07). TOK shows only for the live
                // agent once the first summary has resolved (which is exactly
                // when `uptime_secs` becomes `Some`); otherwise an em-dash.
                div { class: "ih-status-strip",
                    sys_meta::SysMeta {
                        model: values.model,
                        provider: values.provider,
                        uptime_secs,
                        context_source,
                        show_tokens,
                        error_title: values.error_title,
                    }
                }
                // Row 2 — controls.
                div { class: "ih-header-controls",
                    agent_dropdown::AgentDropdown {}
                    NavShortcuts {}
                    MenuOverflow { env_label }
                    env_chip::EnvChip { agent: focused_name, options: env_opts }
                }
            }
        }
        // D-23: rendered beside the banner, not inside it, so the fixed toast
        // is not confined to the banner's stacking context.
        if let Some(text) = notice_text {
            div { class: "ih-toast", role: "status", "aria-live": "polite",
                span { "{text}" }
                button {
                    class: "ih-toast-dismiss",
                    r#type: "button",
                    "aria-label": "Dismiss notice",
                    onclick: move |_| notice.set(None),
                    "\u{00D7}"
                }
            }
        }
    }
}

/// The inline screen shortcuts (visible at >= 1100px).
#[component]
pub fn NavShortcuts() -> Element {
    let mut active_screen = use_context::<Signal<Screen>>();
    // Owned local BEFORE rsx! so no signal borrow is held across the tree.
    let current = *active_screen.read();
    let menu = use_context::<crate::components::hermes_app::MenuCtx>();
    let entries = header_entries(&menu.layout.read());
    let (inline, overflow) = split_inline(&entries);
    let (inline, overflow) = (inline.to_vec(), overflow.to_vec());

    rsx! {
        nav { class: "ih-nav", "aria-label": "Shortcuts",
            for (screen, label) in inline {
                // Phase 54.4 Plan 11 (D-14): only the Logs entry is a drag
                // source (onto the Chat page to add the Logs panel).
                if screen == Screen::Logs {
                    LogsNavButton {
                        key: "{screen:?}",
                        label,
                        current: current == screen,
                    }
                } else {
                    button {
                        key: "{screen:?}",
                        class: "ih-nav-btn",
                        r#type: "button",
                        "aria-current": if current == screen { "page" },
                        onclick: move |_| active_screen.set(screen),
                        "{label}"
                    }
                }
            }
            if !overflow.is_empty() {
                MoreOverflow { entries: overflow }
            }
        }
    }
}

/// The token a Logs-shortcut drag carries as `text/plain`. Firefox starts no
/// drag unless `dragstart` sets some data (research A6, verified in UAT).
const LOGS_WIDGET_DRAG_TOKEN: &str = "logs-widget";

/// Put the Logs token on the drag's data transfer (wasm: raw web-sys event;
/// native: there is no DOM drag, so a no-op).
#[cfg(target_arch = "wasm32")]
fn set_logs_drag_data(evt: &DragEvent) {
    use dioxus::web::WebEventExt;
    if let Some(raw) = evt.try_as_web_event() {
        if let Some(transfer) = raw.data_transfer() {
            let _ = transfer.set_data("text/plain", LOGS_WIDGET_DRAG_TOKEN);
        }
    }
}

/// Native twin of [`set_logs_drag_data`].
#[cfg(not(target_arch = "wasm32"))]
fn set_logs_drag_data(evt: &DragEvent) {
    let _ = (evt, LOGS_WIDGET_DRAG_TOKEN);
}

/// The Logs shortcut button. A plain click still navigates to the Logs
/// screen; additionally it is a drag source: `dragstart` raises the transient
/// `dragging_logs_widget` flag on the root tab model (the Chat drop zone acts
/// only while it is set, T-54.4-37) and `dragend` clears it. The flag is
/// written only when it changes.
#[component]
fn LogsNavButton(label: String, current: bool) -> Element {
    let mut active_screen = use_context::<Signal<Screen>>();
    let mut tabs = use_context::<AgentTabsCtx>().0;

    rsx! {
        button {
            class: "ih-nav-btn",
            r#type: "button",
            draggable: "true",
            "aria-current": if current { "page" },
            onclick: move |_| active_screen.set(Screen::Logs),
            ondragstart: move |evt: DragEvent| {
                set_logs_drag_data(&evt);
                let mut next = tabs.peek().clone();
                if next.set_dragging(true) {
                    tabs.set(next);
                }
            },
            ondragend: move |_| {
                let mut next = tabs.peek().clone();
                if next.set_dragging(false) {
                    tabs.set(next);
                }
            },
            "{label}"
        }
    }
}

/// The `MENU ▾` overflow (visible below 1100px). A standalone dropdown
/// component, never an inline `<select>` (the Soul-dropdown revert 073d4e671
/// rendered blank under a green gate).
#[component]
pub fn MenuOverflow(env_label: String) -> Element {
    let mut active_screen = use_context::<Signal<Screen>>();
    let mut open = use_signal(|| false);

    // After the menu mounts, move focus to its first item. Elements exist by
    // the time an effect runs.
    use_effect(move || {
        if *open.read() {
            focus_element_by_id("ih-menu-item-0");
        }
    });

    let is_open = *open.read();
    let current = *active_screen.read();
    let expanded = if is_open { "true" } else { "false" };
    let menu = use_context::<crate::components::hermes_app::MenuCtx>();
    let entries = header_entries(&menu.layout.read());
    let len = entries.len();

    rsx! {
        div { class: "ih-menu-wrap",
            button {
                id: "ih-menu-trigger",
                class: "ih-menu-btn",
                r#type: "button",
                "aria-haspopup": "menu",
                "aria-expanded": "{expanded}",
                onclick: move |_| {
                    let now = *open.read();
                    open.set(!now);
                },
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::ArrowDown {
                        e.prevent_default();
                        open.set(true);
                    }
                },
                "MENU \u{25BE}"
            }
            if is_open {
                div {
                    class: "ih-menu-backdrop",
                    onclick: move |_| open.set(false),
                }
                div { class: "ih-menu", role: "menu",
                    for (i, (screen, label)) in entries.into_iter().enumerate() {
                        button {
                            key: "{screen:?}",
                            id: "ih-menu-item-{i}",
                            class: "ih-menu-item",
                            role: "menuitem",
                            r#type: "button",
                            "aria-current": if current == screen { "page" },
                            // Enter / Space activate a native button as a click.
                            onclick: move |_| {
                                active_screen.set(screen);
                                open.set(false);
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                let nav = match e.key() {
                                    Key::ArrowDown => Some(MenuKey::Down),
                                    Key::ArrowUp => Some(MenuKey::Up),
                                    Key::Home => Some(MenuKey::Home),
                                    Key::End => Some(MenuKey::End),
                                    _ => None,
                                };
                                if let Some(k) = nav {
                                    e.prevent_default();
                                    if let Some(n) =
                                        menu_next_index(Some(i), len, k)
                                    {
                                        focus_element_by_id(&format!("ih-menu-item-{n}"));
                                    }
                                } else if e.key() == Key::Escape {
                                    open.set(false);
                                    focus_element_by_id("ih-menu-trigger");
                                }
                            },
                            "{label}"
                        }
                    }
                    // The env chip folds in here below 900px (CSS). It is a
                    // non-interactive row: not a menuitem, not in the roving
                    // index above, and it has no handler (D-01).
                    div { class: "ih-menu-env", "ENV {env_label}" }
                }
            }
        }
    }
}

/// The `MORE ▾` overflow for header shortcuts 6 and up (UI-SPEC A3). It sits
/// inside `nav.ih-nav`, so it is hidden below 1100px along with the inline
/// shortcuts (where `MENU ▾` lists everything). Same keyboard contract as
/// [`MenuOverflow`], with its own ids. Items are plain navigation targets,
/// never a drag source.
#[component]
pub fn MoreOverflow(entries: Vec<(Screen, String)>) -> Element {
    let mut active_screen = use_context::<Signal<Screen>>();
    let mut open = use_signal(|| false);

    use_effect(move || {
        if *open.read() {
            focus_element_by_id("ih-more-item-0");
        }
    });

    let is_open = *open.read();
    let current = *active_screen.read();
    let expanded = if is_open { "true" } else { "false" };
    let len = entries.len();

    rsx! {
        div { class: "ih-more-wrap",
            button {
                id: "ih-more-trigger",
                class: "ih-nav-btn ih-menu-btn",
                r#type: "button",
                "aria-haspopup": "menu",
                "aria-expanded": "{expanded}",
                onclick: move |_| {
                    let now = *open.read();
                    open.set(!now);
                },
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::ArrowDown {
                        e.prevent_default();
                        open.set(true);
                    }
                },
                "MORE \u{25BE}"
            }
            if is_open {
                div {
                    class: "ih-menu-backdrop",
                    onclick: move |_| open.set(false),
                }
                div { class: "ih-menu", role: "menu",
                    for (i, (screen, label)) in entries.into_iter().enumerate() {
                        button {
                            key: "{screen:?}",
                            id: "ih-more-item-{i}",
                            class: "ih-menu-item",
                            role: "menuitem",
                            r#type: "button",
                            "aria-current": if current == screen { "page" },
                            onclick: move |_| {
                                active_screen.set(screen);
                                open.set(false);
                            },
                            onkeydown: move |e: KeyboardEvent| {
                                let nav = match e.key() {
                                    Key::ArrowDown => Some(MenuKey::Down),
                                    Key::ArrowUp => Some(MenuKey::Up),
                                    Key::Home => Some(MenuKey::Home),
                                    Key::End => Some(MenuKey::End),
                                    _ => None,
                                };
                                if let Some(k) = nav {
                                    e.prevent_default();
                                    if let Some(n) = menu_next_index(Some(i), len, k) {
                                        focus_element_by_id(&format!("ih-more-item-{n}"));
                                    }
                                } else if e.key() == Key::Escape {
                                    open.set(false);
                                    focus_element_by_id("ih-more-trigger");
                                }
                            },
                            "{label}"
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
    use crate::components::hermes_app::agent_tabs::{
        AgentNoticeCtx, AgentTabsCtx, AgentTabsState, LiveAgentCtx,
    };
    use crate::components::hermes_app::profile_topbar::{
        SharedProfilesCtx, TopbarProfileRefreshCtx,
    };
    use crate::components::hermes_app::wheel::{wheel_shell_class, wheel_shell_style};
    use crate::protocol::ProfileRow;
    use crate::state::WheelState;

    const MOD_RS: &str = include_str!("mod.rs");
    const HEADER_RS: &str = include_str!("header.rs");

    fn fetched_ok(provider: &str, model: &str, backend: &str) -> Result<AgentHeaderState, String> {
        Ok(AgentHeaderState {
            provider: Some(provider.to_string()),
            model: Some(model.to_string()),
            terminal_backend: Some(backend.to_string()),
        })
    }

    /// D-14: only the Logs shortcut is a drag source (it carries the
    /// `logs-widget` token, and Firefox needs data set on dragstart); a plain
    /// click still navigates. Region-scoped to `NavShortcuts` and the Logs
    /// button component.
    #[test]
    fn header_logs_shortcut_is_draggable() {
        let code = &HEADER_RS[..HEADER_RS.find("#[cfg(test)]").unwrap_or(HEADER_RS.len())];
        assert_eq!(
            code.matches("draggable").count(),
            1,
            "exactly one drag source in the header"
        );
        let nav_at = code.find("pub fn NavShortcuts").expect("NavShortcuts");
        let menu_at = code.find("pub fn MenuOverflow").expect("MenuOverflow");
        let nav = &code[nav_at..menu_at];
        assert!(
            nav.contains("Screen::Logs") && nav.contains("LogsNavButton"),
            "NavShortcuts must route only the Logs entry to the draggable button"
        );
        let btn_at = code.find("fn LogsNavButton").expect("LogsNavButton component");
        let btn = &code[btn_at..];
        assert!(btn.contains(r#"draggable: "true""#));
        assert!(btn.contains("ondragstart"));
        assert!(btn.contains("ondragend"));
        assert!(
            btn.contains("set_logs_drag_data(&evt)"),
            "dragstart must put the token on the data transfer"
        );
        assert!(code.contains(r#"LOGS_WIDGET_DRAG_TOKEN: &str = "logs-widget""#));
        assert!(code.contains(r#"set_data("text/plain", LOGS_WIDGET_DRAG_TOKEN)"#));
        assert!(
            btn.contains("active_screen.set(Screen::Logs)"),
            "a plain click must still navigate to the Logs screen"
        );
        assert!(
            !code[..btn_at].contains("draggable"),
            "no other shortcut (or the MENU overflow) is draggable"
        );
    }

    #[test]
    fn header_shortcuts_include_logs() {
        // Phase 54.4 Plan 07 (D-06/D-08): Logs is the fifth, last shortcut.
        assert_eq!(HEADER_SHORTCUTS.len(), 5);
        assert_eq!(
            HEADER_SHORTCUTS.last().copied(),
            Some((Screen::Logs, "Logs"))
        );
        for want in [
            Screen::Kanban,
            Screen::Artifacts,
            Screen::Schedules,
            Screen::Soul,
        ] {
            assert!(
                HEADER_SHORTCUTS.iter().any(|(s, _)| *s == want),
                "{want:?} must stay in HEADER_SHORTCUTS"
            );
        }
    }

    #[test]
    fn header_state_live_agent_uses_live_values() {
        let v = resolve_header_values("default", Some("default"), "openrouter", "m1", None);
        assert_eq!(v.provider, "openrouter");
        assert_eq!(v.model, "m1");
        assert!(v.show_tokens);
        assert_eq!(v.error_title, None);
        // Live values win even when the focused agent's config also fetched.
        let fetched = fetched_ok("anthropic", "claude", "docker");
        let v = resolve_header_values("default", Some("default"), "openrouter", "m1", Some(&fetched));
        assert_eq!((v.provider.as_str(), v.model.as_str()), ("openrouter", "m1"));
        assert_eq!(v.env_label, "docker");
    }

    #[test]
    fn header_state_named_agent_uses_fetched_profile_values() {
        let fetched = fetched_ok("anthropic", "claude", "docker");
        let v = resolve_header_values("dev", Some("default"), "openrouter", "m1", Some(&fetched));
        assert_eq!(v.provider, "anthropic");
        assert_eq!(v.model, "claude");
        assert!(!v.show_tokens, "TOK is the live session's: dash for any other agent");
        assert_eq!(v.env_label, "docker");
        assert_eq!(v.error_title, None);
    }

    #[test]
    fn header_state_fetch_error_renders_dashes_with_title() {
        let fetched: Result<AgentHeaderState, String> = Err("boom".to_string());
        let v = resolve_header_values("dev", Some("default"), "openrouter", "m1", Some(&fetched));
        assert_eq!(v.provider, "\u{2014}");
        assert_eq!(v.model, "\u{2014}");
        assert_eq!(
            v.error_title.as_deref(),
            Some("Couldn't load dev's config. Showing live values where available.")
        );
        assert_eq!(v.env_label, "\u{2014}");
        // The live agent keeps its live values and raises no error title.
        let v = resolve_header_values("default", Some("default"), "openrouter", "m1", Some(&fetched));
        assert_eq!((v.provider.as_str(), v.model.as_str()), ("openrouter", "m1"));
        assert_eq!(v.error_title, None);
    }

    #[test]
    fn header_state_unresolved_fetch_is_dashes() {
        let v = resolve_header_values("dev", Some("default"), "openrouter", "m1", None);
        assert_eq!(v.provider, "\u{2014}");
        assert_eq!(v.model, "\u{2014}");
        assert_eq!(v.error_title, None);
        assert!(!v.show_tokens);
        // A fetched profile with no provider/model set also reads as dashes.
        let fetched = Ok(AgentHeaderState::default());
        let v = resolve_header_values("dev", None, "openrouter", "m1", Some(&fetched));
        assert_eq!((v.provider.as_str(), v.model.as_str()), ("\u{2014}", "\u{2014}"));
    }

    // CSS source-text group (Task 2). Region-scoped: negative checks run over
    // one extracted rule block, never over the whole file.
    const SITE_CSS: &str = include_str!("../../../assets/site.css");
    const SCREENS_CSS: &str = include_str!("../../../assets/screens.css");
    const WHEEL_CSS: &str = include_str!("../../../assets/wheel.css");
    const DASHBOARD_CSS: &str = include_str!("../../../assets/dashboard.css");
    const APP_RS: &str = include_str!("../../app.rs");

    /// Remove `/* ... */` comments so prose that names a selector or a value
    /// can neither satisfy nor trip a check.
    fn strip_css_comments(css: &str) -> String {
        let mut out = String::with_capacity(css.len());
        let mut rest = css;
        while let Some(start) = rest.find("/*") {
            out.push_str(&rest[..start]);
            match rest[start + 2..].find("*/") {
                Some(end) => rest = &rest[start + 2 + end + 2..],
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Every `{ ... }` body whose selector text ends with / contains
    /// `selector` (the text between the previous `}` / `{` / `;` boundary
    /// and the opening brace).
    fn blocks_for(css: &str, selector: &str) -> Vec<String> {
        let css = strip_css_comments(css);
        let mut blocks = Vec::new();
        for chunk in css.split('}') {
            if let Some((sel, body)) = chunk.rsplit_once('{') {
                if sel.contains(selector) {
                    blocks.push(body.to_string());
                }
            }
        }
        blocks
    }

    #[test]
    fn header_is_mounted_inside_app_and_outside_the_screen_router() {
        let app = MOD_RS
            .find("class: \"app\"")
            .expect("div.app must exist in the HermesApp render tree");
        let hdr = MOD_RS
            .find("header::Header {")
            .expect("header::Header must be mounted in mod.rs");
        let router = MOD_RS
            .find("screen_router::ScreenRouter {}")
            .expect("ScreenRouter must be mounted in mod.rs");
        assert!(app < hdr, "Header must come after div.app opens");
        assert!(
            hdr < router,
            "Header must be mounted BEFORE (outside) ScreenRouter"
        );
    }

    #[test]
    fn header_replaces_the_topbar_row_and_the_rail() {
        let old_row = ["topbar", "-row"].concat();
        let old_rail = ["wheel", "_rail"].concat();
        assert!(
            !MOD_RS.contains(&old_row),
            "mod.rs must no longer reference the old top bar row class"
        );
        assert!(
            !MOD_RS.contains(&old_rail),
            "mod.rs must no longer reference the wheel rail module"
        );
    }

    #[test]
    fn header_mounts_the_docked_wheel() {
        assert!(HEADER_RS.contains("wheel::Wheel { docked: true }"));
    }

    #[test]
    fn header_declares_no_context_provider() {
        let needle = ["use_context", "_provider"].concat();
        // The module doc is allowed to name the rule; code is not. The test
        // module (after `mod tests`) is excluded because it builds the
        // needle from fragments and a provider for the smoke app.
        let code_end = HEADER_RS.find("#[cfg(test)]").unwrap_or(HEADER_RS.len());
        assert!(
            !HEADER_RS[..code_end].contains(&needle),
            "header.rs must not provide any context (providers live at the HermesApp root)"
        );
    }

    #[test]
    fn wheel_shell_style_docked_drops_inline_position() {
        let docked = wheel_shell_style(true, 240.0, 10.0, 20.0);
        assert_eq!(docked, "--wheel-size: var(--ih-wheel-size);");
        assert!(!docked.contains("left:"));
        let floating = wheel_shell_style(false, 240.0, 10.0, 20.0);
        assert_eq!(floating, "--wheel-size: 240px; left: 10px; top: 20px;");
    }

    #[test]
    fn wheel_shell_class_docked_adds_modifier() {
        assert_eq!(wheel_shell_class(true), "wheel-shell wheel-shell--docked");
        assert_eq!(wheel_shell_class(false), "wheel-shell");
    }

    #[test]
    fn header_menu_next_index_wraps() {
        assert_eq!(menu_next_index(Some(0), 5, MenuKey::Up), Some(4));
        assert_eq!(menu_next_index(Some(4), 5, MenuKey::Down), Some(0));
        assert_eq!(menu_next_index(None, 5, MenuKey::Down), Some(0));
        assert_eq!(menu_next_index(Some(2), 5, MenuKey::Home), Some(0));
        assert_eq!(menu_next_index(Some(2), 5, MenuKey::End), Some(4));
        assert_eq!(menu_next_index(Some(1), 0, MenuKey::Down), None);
        assert_eq!(menu_next_index(None, 0, MenuKey::Home), None);
    }

    #[test]
    fn header_css_root_variables_declared() {
        let css = strip_css_comments(SITE_CSS);
        for needle in [
            "--ih-header-h: 112px",
            "--ih-wheel-size: 224px",
            "--ih-overhang-h: calc(var(--ih-wheel-size) - var(--ih-header-h))",
            "--ih-row-inset: calc(var(--ih-wheel-size) + 16px)",
            "--ih-tabstrip-h: 48px",
            "--ih-widget-col-w: 240px",
            "--ih-slideover-w: 480px",
        ] {
            assert!(css.contains(needle), "site.css :root must declare `{needle}`");
        }
        let narrow = blocks_for(SITE_CSS, ":root")
            .into_iter()
            .any(|b| b.contains("--ih-header-h: 96px") && b.contains("--ih-wheel-size: 192px"));
        assert!(
            narrow,
            "a :root block must re-declare --ih-header-h: 96px and --ih-wheel-size: 192px (< 900px)"
        );
    }

    #[test]
    fn header_css_screen_is_inset_below_the_banner() {
        let blocks: Vec<String> = blocks_for(SCREENS_CSS, ".screen")
            .into_iter()
            .filter(|b| b.contains("inset:"))
            .collect();
        assert!(
            blocks.iter().any(|b| {
                b.contains("inset: var(--ih-header-h) 0 0 0")
                    && b.contains("calc(var(--ih-overhang-h) + 16px)")
                    && !b.contains("380px")
            }),
            "the .screen block must be inset by the banner, pad by overhang + 16px, and drop the 380px column"
        );
    }

    #[test]
    fn header_css_screen_header_band_rule_uses_row_inset_and_overhang() {
        let css = strip_css_comments(SCREENS_CSS);
        let band = css
            .split('}')
            .map(|r| r.to_string())
            .find(|r| {
                r.contains(":has(> .screen-header:first-child)")
                    && r.contains("> .screen-header:first-child")
                    && r.contains("margin-left")
                    && !r.contains("density-dense")
            })
            .expect("screens.css must have a leading-.screen-header band rule");
        assert!(band.contains("var(--ih-row-inset)"), "band rule must start at --ih-row-inset");
        assert!(band.contains("var(--ih-overhang-h)"), "band rule must size to --ih-overhang-h");
        assert!(
            !band.contains("224px") && !band.contains("240px") && !band.contains("112px"),
            "band rule must not hard-code wheel dimensions"
        );
        assert!(
            css.contains(":has(> .screen-header:first-child)")
                && css.contains("padding-top: 16px;"),
            "header-first screens must drop the top padding to 16px"
        );
    }

    #[test]
    fn header_css_chat_header_in_band_and_tab_strip_not_inset() {
        let dash = strip_css_comments(DASHBOARD_CSS);
        let rule = |sel: &str| {
            dash.split('}')
                .find(|r| r.contains(sel))
                .unwrap_or_else(|| panic!("dashboard.css must have {sel}"))
                .to_string()
        };
        let hdr = rule("#screen-chat > .screen-header:first-child");
        assert!(hdr.contains("var(--ih-row-inset)"), "chat header starts at --ih-row-inset");
        assert!(hdr.contains("var(--ih-overhang-h)"), "chat header sizes to --ih-overhang-h");
        let row = rule(".ih-tabstrip-row {");
        assert!(!row.contains("--ih-row-inset"), "tab strip row must not be inset");
        assert!(!row.contains("margin-left"), "tab strip row must not be offset");
        assert!(!dash.contains(".ih-band-row"), "old band-row spacer must be gone");
        let screens = strip_css_comments(SCREENS_CSS);
        assert!(
            screens.contains(r#"[id="screen-chat"]"#),
            "generic band rule must still exclude Chat"
        );
    }

    #[test]
    fn header_css_old_wheel_column_rules_are_gone() {
        let site = strip_css_comments(SITE_CSS);
        for stale in [
            ".topbar-row",
            "#wheel-rail",
            "has-rail",
            "no-breadcrumb",
            ".breadcrumb",
        ] {
            assert!(
                !site.contains(stale),
                "site.css must no longer contain the retired rule `{stale}`"
            );
        }
        let screens = strip_css_comments(SCREENS_CSS);
        assert!(
            !screens.contains(".wheel-shell { position: fixed; top: 16px; left: 16px; }"),
            "the floating-wheel rule inside the 900px media block must be deleted"
        );
        assert!(
            !screens.contains(".wheel-rail"),
            "screens.css must no longer style the retired wheel rail"
        );
    }

    #[test]
    fn header_css_docked_wheel_has_no_magnify() {
        let blocks = blocks_for(WHEEL_CSS, ".wheel-shell--docked");
        assert!(
            !blocks.is_empty(),
            "wheel.css must define .wheel-shell--docked rules"
        );
        for b in blocks {
            assert!(!b.contains("scale("), "docked wheel must not scale: {b}");
            assert!(!b.contains("transform"), "docked wheel must not transform: {b}");
            assert!(!b.contains("transition"), "docked wheel must not animate size: {b}");
        }
    }

    #[test]
    fn header_css_dashboard_sheet_is_registered_unconditionally() {
        assert!(APP_RS.contains("asset!(\"/assets/dashboard.css\")"));
        assert!(APP_RS.contains("href: DASHBOARD_CSS"));
    }

    #[test]
    fn header_css_uses_theme_variables_only() {
        let css = strip_css_comments(DASHBOARD_CSS);
        assert!(css.contains(".ih-header"), "dashboard.css must define the header");
        let mut checked = 0;
        for chunk in css.split('}') {
            if let Some((sel, body)) = chunk.rsplit_once('{') {
                if sel.contains(".ih-") {
                    checked += 1;
                    assert!(
                        !body.contains('#'),
                        "`{}` must use theme variables, not a hex literal: {body}",
                        sel.trim()
                    );
                }
            }
        }
        assert!(checked >= 5, "expected several .ih- rules, found {checked}");
    }

    #[test]
    fn smoke_header_mounts_without_panic() {
        fn test_app() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let wheel = use_signal(WheelState::default);
            use_context_provider(|| wheel);
            let tokens = use_signal(|| (0_u32, 128_000_u32));
            use_context_provider(|| tokens);
            let tick = use_signal(|| 0_u32);
            use_context_provider(|| TopbarProfileRefreshCtx(tick));
            let profiles = use_resource(|| async {
                Ok::<Vec<ProfileRow>, ServerFnError>(Vec::new())
            });
            use_context_provider(|| SharedProfilesCtx(profiles));
            let tabs = use_signal(AgentTabsState::default_layout);
            use_context_provider(|| AgentTabsCtx(tabs));
            let live = use_resource(|| async { Ok::<String, ServerFnError>("default".to_string()) });
            use_context_provider(|| LiveAgentCtx(live));
            let notice = use_signal(|| None::<String>);
            use_context_provider(|| AgentNoticeCtx(notice));
            let menu_layout = use_signal(crate::menu_layout::default_layout);
            let menu_notice = use_signal(|| None::<String>);
            let menu_write = use_signal(|| false);
            let save = crate::components::hermes_app::screens::settings_menu::use_save_queue();
            use_context_provider(|| crate::components::hermes_app::MenuCtx {
                layout: menu_layout,
                notice: menu_notice,
                write_enabled: menu_write,
                save,
            });
            // Phase 57 R9b: `Header` mounts `Wheel`, which reads the approval-card map for
            // its Chat badge, so the root provides it here as `HermesApp` does.
            let cards = use_signal(
                crate::components::hermes_app::screens::chat::approval_card::ApprovalCards::default,
            );
            use_context_provider(|| {
                crate::components::hermes_app::screens::chat::approval_card::ApprovalCardsCtx(cards)
            });
            rsx! {
                Header {
                    model: "m".to_string(),
                    provider: "p".to_string(),
                    uptime_secs: Some(5_u64),
                    context_source: None,
                }
            }
        }

        let mut dom = VirtualDom::new(test_app);
        dom.rebuild_to_vec();
    }

    // Phase 54.9 Plan 02: data-driven header. NavShortcuts is rendered to HTML
    // inside a root that provides the contexts it (and LogsNavButton) consume.
    thread_local! {
        static NAV_LAYOUT: std::cell::RefCell<crate::menu_layout::MenuLayout> =
            std::cell::RefCell::new(crate::menu_layout::default_layout());
    }

    fn render_nav(layout: crate::menu_layout::MenuLayout) -> String {
        NAV_LAYOUT.with(|l| *l.borrow_mut() = layout);
        fn root() -> Element {
            let screen = use_signal(|| Screen::Chat);
            use_context_provider(|| screen);
            let tabs = use_signal(AgentTabsState::default_layout);
            use_context_provider(|| AgentTabsCtx(tabs));
            let layout = use_signal(|| NAV_LAYOUT.with(|l| l.borrow().clone()));
            let notice = use_signal(|| None::<String>);
            let write = use_signal(|| false);
            let save = crate::components::hermes_app::screens::settings_menu::use_save_queue();
            use_context_provider(|| crate::components::hermes_app::MenuCtx {
                layout,
                notice,
                write_enabled: write,
                save,
            });
            rsx! { NavShortcuts {} }
        }
        let mut dom = VirtualDom::new(root);
        dom.rebuild_in_place();
        dioxus::ssr::render(&dom)
    }

    #[test]
    fn header_entries_resolve_ids_to_screens() {
        // Phase 54.9 Plan 09 / 54.10 Plan 01: the shipped default ends with OpenUI then
        // Dashboard after the five literals.
        let mut want: Vec<(Screen, String)> = HEADER_SHORTCUTS
            .iter()
            .map(|(s, l)| (*s, l.to_string()))
            .collect();
        want.push((Screen::OpenUi, "OpenUI".to_string()));
        want.push((Screen::Dashboard, "Dashboard".to_string()));
        assert_eq!(header_entries(&crate::menu_layout::default_layout()), want);
    }

    #[cfg(feature = "server")]
    #[test]
    fn nav_shortcuts_follow_the_saved_header_order() {
        let mut layout = crate::menu_layout::default_layout();
        layout.header = [
            ("logs", "Logs"),
            ("kanban", "Board"),
            ("artifacts", "Artifacts"),
            ("schedules", "Schedules"),
            ("soul", "Soul"),
            ("openui", "OpenUI"),
            ("dashboard", "Dashboard"),
        ]
        .into_iter()
        .map(|(screen, label)| crate::menu_layout::MenuEntry {
            screen: screen.to_string(),
            label: label.to_string(),
        })
        .collect();
        // Round-trip through a real config.yaml, as the server does.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.yaml");
        std::fs::write(&path, "security:\n  web_config_write_enabled: true\n").unwrap();
        crate::server::menu_api::save_menu_layout_at(&path, &layout).unwrap();
        let snap = crate::server::menu_api::menu_snapshot_at(&path);
        assert_eq!(snap.notice, None);

        let html = render_nav(snap.layout);
        let logs = html.find("Logs").expect("Logs rendered");
        let board = html.find("Board").expect("Board rendered");
        assert!(logs < board, "saved order: Logs before Board: {html}");
        assert!(
            !html.contains("Kanban"),
            "renamed label replaces Kanban: {html}"
        );
    }

    /// A valid layout whose header list starts with `ids`, in order; every other
    /// catalog screen goes on the wheel. With 17 catalog entries a short header would leave
    /// the wheel above `WHEEL_MAX`, so surplus entries spill onto the END of the header.
    fn layout_with_header(ids: &[&str]) -> crate::menu_layout::MenuLayout {
        use crate::menu_layout::{MenuEntry, MENU_CATALOG};
        let entry = |c: &crate::menu_layout::CatalogEntry| MenuEntry {
            screen: c.id.to_string(),
            label: c.name.to_string(),
        };
        let mut header: Vec<MenuEntry> = ids
            .iter()
            .map(|id| entry(crate::menu_layout::catalog_entry(id).unwrap()))
            .collect();
        let mut wheel: Vec<MenuEntry> = MENU_CATALOG
            .iter()
            .filter(|c| !ids.contains(&c.id))
            .map(entry)
            .collect();
        while wheel.len() > crate::menu_layout::WHEEL_MAX {
            let at = wheel
                .iter()
                .rposition(|e| !crate::menu_layout::is_locked(&e.screen))
                .unwrap();
            header.push(wheel.remove(at));
        }
        let layout = crate::menu_layout::MenuLayout {
            version: crate::menu_layout::MENU_LAYOUT_VERSION,
            wheel,
            header,
        };
        assert_eq!(crate::menu_layout::validate(&layout), Ok(()));
        layout
    }

    const SEVEN: [&str; 7] = [
        "kanban",
        "artifacts",
        "schedules",
        "soul",
        "agents",
        "models",
        "logs",
    ];

    #[test]
    fn nav_renders_more_only_beyond_five() {
        // A 5-entry header cannot validate against the 17-entry catalog (the wheel would
        // exceed WHEEL_MAX), so the spilled entries are cut back for this render-only check.
        let mut five_layout =
            layout_with_header(&["kanban", "artifacts", "schedules", "soul", "logs"]);
        five_layout.header.truncate(5);
        let five = render_nav(five_layout);
        assert!(!five.contains("ih-more-trigger"), "{five}");
        assert_eq!(five.matches(r#"class="ih-nav-btn""#).count(), 5, "{five}");

        // Phase 54.10 Plan 01: the shipped default has seven header entries (OpenUI, Dashboard
        // last), so both sit inside MORE by default (UI-SPEC A3).
        let default = render_nav(crate::menu_layout::default_layout());
        assert_eq!(
            default.matches(r#"class="ih-nav-btn""#).count(),
            5,
            "{default}"
        );
        assert_eq!(default.matches("ih-more-trigger").count(), 1, "{default}");

        let seven = render_nav(layout_with_header(&SEVEN));
        assert_eq!(seven.matches(r#"class="ih-nav-btn""#).count(), 5, "{seven}");
        assert_eq!(seven.matches("ih-more-trigger").count(), 1, "{seven}");
        assert!(seven.contains("MORE \u{25BE}"), "{seven}");
    }

    #[test]
    fn logs_drag_source_only_when_inline() {
        let attr = ["drag", "gable"].concat();
        let inline = render_nav(layout_with_header(&[
            "logs",
            "kanban",
            "artifacts",
            "schedules",
            "soul",
            "agents",
            "models",
        ]));
        assert_eq!(inline.matches(&attr).count(), 1, "{inline}");
        // Logs is the 7th entry: it lands inside MORE, which is collapsed, and
        // nothing inline is a drag source.
        let overflowed = render_nav(layout_with_header(&SEVEN));
        assert!(!overflowed.contains(&attr), "{overflowed}");
    }

    #[test]
    fn menu_overflow_lists_every_configured_shortcut() {
        let seven: Vec<u8> = (0..7).collect();
        let (a, b) = split_inline(&seven);
        assert_eq!((a.len(), b.len()), (5, 2));
        let four: Vec<u8> = (0..4).collect();
        let (a, b) = split_inline(&four);
        assert_eq!((a.len(), b.len()), (4, 0));
        let code = &HEADER_RS[..HEADER_RS.find("#[cfg(test)]").unwrap_or(HEADER_RS.len())];
        let region = &code
            [code.find("pub fn MenuOverflow").unwrap()..code.find("pub fn MoreOverflow").unwrap()];
        assert!(region.contains("header_entries("));
        assert!(!region.contains("HEADER_SHORTCUTS"));
    }

    #[test]
    fn more_ids_do_not_collide_with_menu_ids() {
        let code = &HEADER_RS[..HEADER_RS.find("#[cfg(test)]").unwrap_or(HEADER_RS.len())];
        assert!(code.contains("ih-more-trigger") && code.contains("ih-more-item-"));
        let more = &code[code.find("pub fn MoreOverflow").unwrap()..];
        assert!(!more.contains("ih-menu-trigger"));
        assert!(!more.contains("ih-menu-item-"));
    }

    #[test]
    fn header_css_more_wrap_is_positioned() {
        let wrap = blocks_for(DASHBOARD_CSS, ".ih-more-wrap");
        assert!(
            wrap.iter().any(|b| b.contains("position: relative")),
            "`.ih-more-wrap` must be a positioning context"
        );
        let menu = blocks_for(DASHBOARD_CSS, ".ih-more-wrap .ih-menu");
        assert!(
            menu.iter().any(|b| b.contains("right: 0")),
            "the MORE popup must anchor to the right edge"
        );
    }

    /// WR-04: the header's per-agent read subscribes to the profile-refresh
    /// tick in its SYNC prefix (outside the `async move`), so a settings Save
    /// that bumps the tick refetches provider / model / backend.
    #[test]
    fn header_state_resource_subscribes_to_the_profile_refresh_tick() {
        let code = &HEADER_RS[..HEADER_RS.find("#[cfg(test)]").unwrap_or(HEADER_RS.len())];
        let start = code
            .find("let header_state = use_resource(move || {")
            .expect("the header_state resource must exist");
        let region = &code[start..];
        let async_at = region.find("async move").expect("the fetch is async");
        assert!(
            region[..async_at].contains("refresh()"),
            "the refresh tick must be read with call syntax before the async block so the resource subscribes"
        );
    }
}
