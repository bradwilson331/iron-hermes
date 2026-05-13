//! Phase 26.2.1 — HermesApp root composer.
//!
//! This is the root component of the wheel-driven shell. It owns the four
//! root signals (`UiPrefs`, theme string, `WheelState`, active `Screen`),
//! provides them via `use_context_provider`, runs the localStorage
//! hydration gate exactly once on mount, and then keeps localStorage in
//! sync via three persistence effects (RESEARCH Pattern 5).
//!
//! Plan 04 mounted the wheel SVG + wheel-rail. Plan 05 inserted the
//! tweaks-panel + theme-effects. Plan 06 (this commit) HOISTS the chat
//! WebSocket here — `use_websocket` lives at the root component which
//! never unmounts, avoiding RESEARCH Pitfall 5 (per-screen unmount would
//! tear down the connection on every wheel-launch). The receive loop,
//! session bootstrap, and the user-input send-fn all live here; the
//! resulting `Signal<Vec<ChatBubble>>`, `Signal<Option<u64>>`,
//! `SessionIdContext`, `Signal<(u32, u32)>`, and `ChatSendHandler` are
//! provided via context so ScreenChat (and any future screen that wants
//! to peek at chat state) consumes them by lookup.

use crate::components::hermes_app::screens::chat::{ChatBubble, ChatSendHandler, ToolRow};
use crate::state::ThemeContext;
use crate::state::{Screen, SessionIdContext, WheelState};
use crate::ui_prefs::{self, UiPrefs};
use dioxus::prelude::*;

pub mod app_footer;
pub mod breadcrumb;
pub mod hud_chrome;
pub mod screen_router;
pub mod screens;
pub mod sys_meta;
pub mod theme_effects;
pub mod tweaks_panel;
pub mod wheel;
pub mod wheel_rail;

/// Root component of the Phase 26.2.1 wheel-driven shell.
///
/// Owns the four root signals and gates persistence through a one-shot
/// hydration effect — defaults never clobber stored values (RESEARCH
/// Pitfall 5 / Pattern 5). Plan 06 adds five more chat-state signals
/// (bubbles / streaming_id / session_id / tokens / send-handler) and the
/// hoisted `use_websocket` + receive loop.
#[component]
pub fn HermesApp() -> Element {
    let mut prefs = use_signal(UiPrefs::default);
    let mut theme = use_signal(|| "slate-dark".to_string());
    let mut wheel_state = use_signal(WheelState::default);
    let active_screen = use_signal(|| Screen::Chat);
    let mut hydrated = use_signal(|| false);

    // Context providers — four root signals exposed to descendants.
    //
    // The theme signal is wrapped in `ThemeContext(theme)` (B-03 newtype,
    // D-26) so it does not type-collide with Plan 06's `SessionIdContext`
    // (also a `Signal<String>`). The other three signals have unique
    // types in the tree and are provided as bare `Signal<T>`.
    use_context_provider(|| prefs);
    use_context_provider(|| crate::state::ThemeContext(theme));
    use_context_provider(|| wheel_state);
    use_context_provider(|| active_screen);

    // Suppress unused-variable warnings on the wrapper read path — the
    // ThemeContext newtype constructor uses the `theme` signal by move,
    // and Plan 05's theme-effects consumer reads it via context.
    let _ = ThemeContext;

    // Hydration gate: read localStorage exactly once. On non-WASM hosts
    // (server / unit-test builds) the read helpers return `None`, so
    // `hydrated` simply flips to `true` without overwriting any defaults.
    use_effect(move || {
        if *hydrated.read() {
            return;
        }
        if let Some(p) = ui_prefs::read_json::<UiPrefs>(ui_prefs::KEY_TWEAKS) {
            prefs.set(p);
        }
        if let Some(t) = ui_prefs::read_string(ui_prefs::KEY_THEME) {
            theme.set(t);
        }
        if let Some(ws) = ui_prefs::read_json::<WheelState>(ui_prefs::KEY_WHEEL) {
            wheel_state.set(ws);
        }
        hydrated.set(true);
    });

    // Persist-on-change effects — gated on `hydrated` so the initial
    // `UiPrefs::default()` never overwrites a stored blob.
    //
    // Signal-borrow safety (clippy.toml): read into a local, drop the
    // borrow at the `;`, then call the side-effecting write helper.
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let p = prefs.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_TWEAKS, &p);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let t = theme.read().clone();
        ui_prefs::write_string(ui_prefs::KEY_THEME, &t);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let ws = wheel_state.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_WHEEL, &ws);
    });

    // -----------------------------------------------------------------
    // Plan 06 — Chat signal hub (HOISTED at HermesApp root per
    // RESEARCH Pitfall 5: `use_websocket` MUST live in a component that
    // never unmounts. HermesApp is the root, so it's the safe placement.
    // -----------------------------------------------------------------

    let mut bubbles = use_signal(Vec::<ChatBubble>::new);
    let mut streaming_id = use_signal(|| Option::<u64>::None);
    let mut next_id = use_signal(|| 1u64);
    let mut session_id = use_signal(|| "pending".to_string());
    let mut tokens = use_signal(|| (0u32, 128_000u32));

    // Bootstrap the chat session via the existing server fn from
    // Phase 25.5 (D-02 — no edits to the server file). Mirrors
    // warp_hermes.rs:104-129 adapted for the new bubble shape.
    use_effect(move || {
        spawn(async move {
            match crate::server::api::create_session().await {
                Ok(sid) => {
                    session_id.set(sid);
                }
                Err(e) => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("Session creation failed: {e}").into());
                    let _ = e;
                }
            }
        });
    });

    // Open the WebSocket with automatic reconnect — preserves the
    // Phase 26.1 keepalive + close-frame contract byte-for-byte (D-02 /
    // D-18). `ws_chat` itself is unchanged.
    let mut ws = dioxus_fullstack::use_websocket(move || {
        crate::server::ws::ws_chat(
            dioxus_fullstack::WebSocketOptions::new().with_automatic_reconnect(),
        )
    });

    // Receive loop — ports the warp_hermes.rs:139-410 structure adapted
    // for the new ChatBubble shape. Every signal read is dropped before
    // any `.await` (clippy.toml signal-borrow-across-await guard).
    use_future(move || async move {
        loop {
            let _state = ws.connect().await;
            if ws.is_err() {
                streaming_id.set(None);
                continue;
            }

            loop {
                match ws.recv_raw().await {
                    Ok(dioxus_fullstack::Message::Text(t)) => {
                        let event: crate::protocol::ChatStreamEvent =
                            match serde_json::from_str(&t) {
                                Ok(e) => e,
                                Err(_) => continue, // Skip malformed frames silently.
                            };
                        match event {
                            crate::protocol::ChatStreamEvent::Delta { text } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        b.text.push_str(&text);
                                    }
                                } else {
                                    let id = {
                                        let n = *next_id.read();
                                        next_id.set(n + 1);
                                        n
                                    };
                                    streaming_id.set(Some(id));
                                    bubbles.write().push(ChatBubble::assistant(id, text));
                                }
                            }
                            crate::protocol::ChatStreamEvent::ToolCallStart { name, args } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        b.tool_rows.push(ToolRow {
                                            name,
                                            args,
                                            done: false,
                                            success: false,
                                        });
                                    }
                                }
                            }
                            crate::protocol::ChatStreamEvent::ToolCallEnd { name, success } => {
                                let sid = *streaming_id.read();
                                if let Some(id) = sid {
                                    let mut bs = bubbles.write();
                                    if let Some(b) = bs.iter_mut().find(|b| b.id == id) {
                                        for row in b.tool_rows.iter_mut().rev() {
                                            if row.name == name && !row.done {
                                                row.done = true;
                                                row.success = success;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            crate::protocol::ChatStreamEvent::Finished { total_tokens } => {
                                tokens.set((total_tokens, 128_000));
                                streaming_id.set(None);
                            }
                            crate::protocol::ChatStreamEvent::Error { message } => {
                                let id = {
                                    let n = *next_id.read();
                                    next_id.set(n + 1);
                                    n
                                };
                                bubbles.write().push(ChatBubble::error(id, message));
                                streaming_id.set(None);
                            }
                        }
                    }
                    Ok(dioxus_fullstack::Message::Close { .. }) => {
                        streaming_id.set(None);
                        break; // Outer loop will reconnect via with_automatic_reconnect.
                    }
                    Err(_) => break,
                    _ => continue, // Skip ping/pong/binary.
                }
            }
        }
    });

    // Send-handler — invoked by ScreenChat's input box on Enter (or the
    // submit button). Serialization failure does NOT silently drop — it
    // surfaces as an error bubble so the user always gets feedback (B-05).
    let send = EventHandler::new(move |text: String| {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        let sid = session_id.read().clone();
        let id = {
            let n = *next_id.read();
            next_id.set(n + 1);
            n
        };
        bubbles.write().push(ChatBubble::user(id, trimmed.clone()));
        let req = crate::protocol::ChatRequest {
            session_id: sid,
            message: trimmed,
        };
        // Let-else early-return on serialization failure — no silent
        // unwrap_or_default(); surface the failure as an error bubble.
        let Ok(json) = serde_json::to_string(&req) else {
            let err_id = {
                let n = *next_id.read();
                next_id.set(n + 1);
                n
            };
            bubbles.write().push(ChatBubble::error(
                err_id,
                "Failed to send message: serialization error".to_string(),
            ));
            return;
        };
        spawn(async move {
            let _ = ws.send_raw(dioxus_fullstack::Message::Text(json)).await;
        });
    });
    let send_handler = ChatSendHandler(send);

    // Plan 06 context providers — five new signals reachable from any
    // descendant screen. `session_id` is wrapped in the B-03
    // `SessionIdContext` newtype so it does not collide with Plan 03's
    // `ThemeContext(theme)` (also `Signal<String>`).
    use_context_provider(|| bubbles);
    use_context_provider(|| streaming_id);
    use_context_provider(|| SessionIdContext(session_id));
    use_context_provider(|| tokens);
    use_context_provider(|| send_handler);

    rsx! {
        hud_chrome::HudChrome {}
        breadcrumb::Breadcrumb {}
        sys_meta::SysMeta {}
        div { class: "app", id: "app",
            screen_router::ScreenRouter {}
        }
        app_footer::AppFooter {}
        wheel_rail::WheelRail {}
        wheel::Wheel {}
        theme_effects::ThemeEffects {}
        tweaks_panel::TweaksPanel {}
    }
}
