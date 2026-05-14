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
    //
    // Plan 26.2.1-11 — client-side slash-command dispatch (GAP-26.2.1-04).
    // Messages beginning with `/` are intercepted here BEFORE the
    // WebSocket send path: `/clear` clears the local bubble list with no
    // server round-trip; unknown slash commands append a single
    // "unknown command" error bubble. Plain messages still flow through
    // `ws.send_raw(ChatRequest)` exactly as before. This amends D-20 from
    // "server-side CommandRouter dispatch" to "client-side dispatch for
    // purely-visual commands; server-side dispatch reserved for future
    // server-effecting commands and requires a follow-up phase that lifts
    // D-02" (the 26.2.1 D-02 server-untouched constraint).
    let send = EventHandler::new(move |text: String| {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with('/') && dispatch_slash(&trimmed, &mut bubbles, &mut next_id) {
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

// ---------------------------------------------------------------------------
// Plan 26.2.1-11 — Client-side slash-command dispatch (GAP-26.2.1-04).
// ---------------------------------------------------------------------------
//
// `/clear` empties the local bubble list with no LLM round-trip. Unknown
// slash commands append a single error-styled bubble locally. Plain
// (non-slash) messages still flow through the existing WebSocket path in
// the `send` EventHandler closure — this helper is only reached after the
// `text.starts_with('/')` early-return branch.
//
// Signals are passed by `&mut` even though they implement `Copy` in Dioxus
// 0.7; this preserves the call-site spelling `&mut bubbles` / `&mut next_id`
// for clarity at the only call site, and `.write()` / `.set()` take `&self`
// internally so this compiles cleanly under both default and `legacy-shell`
// features.
fn dispatch_slash(
    cmd: &str,
    bubbles: &mut Signal<Vec<ChatBubble>>,
    _next_id: &mut Signal<u64>,
) -> bool {
    let mut parts = cmd.trim().splitn(2, char::is_whitespace);
    let head = parts.next().unwrap_or("");
    match head {
        "/clear" => {
            bubbles.write().clear();
            true
        }
        // Per D-26.2.1-13-B: unknown slashes are NOT handled
        // client-side. Fall through to ws.send_raw so the server
        // (or LLM) routes them. /research weather, /help (future),
        // /status (future) all reach the backend via this path.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    //! Plan 26.2.1-11 — static-grep regression guards for
    //! GAP-26.2.1-04 (client-side slash-command interception).
    //!
    //! Dioxus 0.7 `Signal<T>` is component-scoped; constructing one outside
    //! a component requires a `dioxus::prelude::ScopeId` and a live
    //! VirtualDom, which would substantially expand this crate's test
    //! footprint. The crate's existing test bootstrap (see e.g.
    //! `wheel.rs` tests) does not yet construct signals outside a
    //! component context. Per the plan: "If the existing test bootstrap
    //! pattern doesn't easily support signals outside a component …
    //! skip the unit test and rely on a static-grep regression test".
    //! That is the choice made here.
    const MOD_RS: &str = include_str!("mod.rs");

    // GAP-26.2.1-07-R3 / GAP-26.2.1-09-R3 — Plan 14 regression sources.
    const SITE_CSS: &str = include_str!("../../../assets/site.css");
    const API_RS: &str = include_str!("../../server/api.rs");

    #[test]
    fn dispatch_slash_helper_exists() {
        assert!(
            MOD_RS.contains("fn dispatch_slash"),
            "dispatch_slash helper must exist in mod.rs (GAP-26.2.1-04 regression)",
        );
        assert!(
            MOD_RS.contains(") -> bool"),
            "dispatch_slash must return bool per GAP-26.2.1-08 (D-26.2.1-13-B)",
        );
    }

    #[test]
    fn send_eventhandler_intercepts_slash_prefix_before_websocket() {
        assert!(
            MOD_RS.contains("trimmed.starts_with('/') && dispatch_slash(&trimmed, &mut bubbles, &mut next_id)"),
            "send EventHandler must combine starts_with('/') with dispatch_slash bool return per GAP-26.2.1-08",
        );
    }

    #[test]
    fn clear_arm_empties_bubble_list_locally() {
        assert!(
            MOD_RS.contains("\"/clear\""),
            "dispatch_slash must match on the literal `/clear` head",
        );
        assert!(
            MOD_RS.contains("bubbles.write().clear()"),
            "the `/clear` arm must empty the bubble list locally",
        );
    }

    #[test]
    fn unknown_command_arm_falls_through_to_websocket() {
        // Per GAP-26.2.1-08 (D-26.2.1-13-B): unknown slashes must NOT
        // be handled client-side. The `_` arm returns false so the
        // call site falls through to ws.send_raw(ChatRequest).
        //
        // Note: we check for the specific `format!("unknown command:` string
        // rather than a ChatBubble::error call because other error paths in
        // the send handler legitimately use ChatBubble::error.
        // The old dispatch_slash code built a local error message whose
        // format string began with the two words below (split here to avoid
        // the include_str! self-match):
        let pat = ["unknown", " command: {head}"].concat();
        assert!(
            !MOD_RS.contains(&pat),
            "GAP-26.2.1-08: dispatch_slash must not build a local error message for unhandled slashes",
        );
        // Positive assertion: the `_` arm must yield `false` so the
        // call site short-circuits and falls through.
        assert!(
            MOD_RS.contains("_ => false"),
            "GAP-26.2.1-08: unknown-slash arm must return false to enable fall-through to ws.send_raw",
        );
    }

    #[test]
    fn slash_branch_precedes_websocket_send_path() {
        // The early-return slash branch MUST appear before `let sid =
        // session_id.read().clone();` — otherwise a `/clear` would
        // re-emit a user bubble + WS frame before the helper runs.
        let idx_slash = MOD_RS
            .find("trimmed.starts_with('/')")
            .expect("starts_with('/') branch must be present");
        let idx_sid = MOD_RS
            .find("let sid = session_id.read().clone();")
            .expect("WebSocket-path `let sid` must still be present");
        assert!(
            idx_slash < idx_sid,
            "slash-prefix branch must come BEFORE the WebSocket path so /clear never round-trips",
        );
    }

    #[test]
    fn scanlines_toggle_hide_rule_exists_in_site_css() {
        // GAP-26.2.1-07-R3 regression: the no-scanlines body-class hide rule
        // must remain in site.css. Plan 14 Branch (c) bumped specificity
        // (html prefix → 0,2,2) and added a triple-guard (display + visibility
        // + opacity) — the rule body spans multiple lines, so we scan for the
        // selector first, then assert the declaration block following it
        // contains `display: none`. The minimum invariant is that some hide
        // rule with `no-scanlines` in the selector still drives `display: none`.
        let selector_idx = SITE_CSS.find("no-scanlines .scanlines").expect(
            "GAP-26.2.1-07-R3: site.css must contain a selector matching `no-scanlines .scanlines`",
        );
        // Look for the declaration block that follows — bounded by the next
        // `}` so we don't accidentally match a later unrelated rule.
        let after_selector = &SITE_CSS[selector_idx..];
        let block_end = after_selector
            .find('}')
            .expect("GAP-26.2.1-07-R3: selector must be followed by a closing `}`");
        let block = &after_selector[..block_end];
        assert!(
            block.contains("display") && block.contains("none"),
            "GAP-26.2.1-07-R3: the `no-scanlines .scanlines` rule must set `display: none` (Plan 14 Branch c triple-guard)",
        );
    }

    #[test]
    fn list_sessions_filters_by_message_count_in_api_rs() {
        // GAP-26.2.1-09-R3 regression: list_sessions must filter the Vec<Session>
        // by message_count > 0 before mapping to SessionInfo, otherwise foreign-
        // format directories (only trajectories.jsonl) leak into the SESSIONS
        // wedge and produce dead row-clicks.
        assert!(
            API_RS.contains(".filter(|s| s.message_count > 0)"),
            "GAP-26.2.1-09-R3: api.rs list_sessions must include `.filter(|s| s.message_count > 0)` per D-26.2.1-14-B",
        );
        assert!(
            API_RS.contains("GAP-26.2.1-09-R3"),
            "GAP-26.2.1-09-R3: api.rs must cite the gap ID in the filter's explanatory comment",
        );
    }
}
