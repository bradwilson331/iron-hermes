//! Phase 26.2.1 Plan 06 — Chat screen + chat-protocol UI types.
//!
//! This file owns the small data types consumed by the WebSocket receive
//! loop in `hermes_app/mod.rs` (`ChatBubble`, `ChatBubbleKind`, `ToolRow`)
//! plus the `ChatSendHandler` newtype that disambiguates the send-fn
//! context provider from any future `EventHandler<String>` in the tree.
//!
//! Plan 06 Task 1 (this commit) introduces the types so `mod.rs`'s hoisted
//! `use_websocket` + receive loop can reference them. Task 2 then
//! replaces the placeholder `ScreenChat` body with the full
//! `chat-mini` + `chat-stream` + `chat-input-pill` layout that consumes
//! these types via context.

use dioxus::prelude::*;

/// Assistant avatar — copper low-poly wings logo used on agent chat bubbles.
const AVATAR_LOGO: Asset = asset!("/assets/i_hermes_logo.png");

// ---------------------------------------------------------------------------
// Chat UI primitives (Plan 06)
// ---------------------------------------------------------------------------

/// Bubble role — drives the CSS class that selects user / assistant /
/// error styling. Maps 1:1 to `chat-msg.user`, `chat-msg.assistant`, and
/// an error variant rendered as `chat-bubble is-error`.
#[derive(Clone, PartialEq, Debug)]
pub enum ChatBubbleKind {
    User,
    Assistant,
    Error,
    // Phase 26.7.2 D-02: history/live boundary marker — renders as a section-label rule, no avatar, no bubble body
    Divider,
}

/// One tool-call progress row inside an assistant bubble.
///
/// `done` flips to `true` when the matching `ChatStreamEvent::ToolCallEnd`
/// arrives; `success` carries the server-reported outcome. Renders as
/// `.chat-progress-row.is-running` / `.is-done.is-success` / `.is-done.is-error`.
#[derive(Clone, PartialEq, Debug)]
pub struct ToolRow {
    pub name: String,
    pub args: String,
    pub done: bool,
    pub success: bool,
}

/// One bubble in the chat stream — user, assistant, or error.
///
/// `tool_rows` is mutated in-place by the receive loop in `mod.rs` when
/// `ChatStreamEvent::ToolCallStart` / `ToolCallEnd` arrive for the
/// currently-streaming assistant bubble.
#[derive(Clone, PartialEq, Debug)]
pub struct ChatBubble {
    pub id: u64,
    pub kind: ChatBubbleKind,
    pub text: String,
    pub tool_rows: Vec<ToolRow>,
}

impl ChatBubble {
    pub fn user(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::User,
            text,
            tool_rows: vec![],
        }
    }
    pub fn assistant(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Assistant,
            text,
            tool_rows: vec![],
        }
    }
    pub fn error(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Error,
            text,
            tool_rows: vec![],
        }
    }
}

/// Newtype wrapper around the chat send handler so `use_context` lookup
/// stays unambiguous — any future `EventHandler<String>` provider would
/// otherwise collide.
///
/// `Copy` is required so consumers can `let send = use_context::<ChatSendHandler>();`
/// inside RSX closures without manual cloning.
#[derive(Clone, Copy)]
pub struct ChatSendHandler(pub EventHandler<String>);

// ---------------------------------------------------------------------------
// ScreenChat component (Plan 06 Task 2)
// ---------------------------------------------------------------------------
//
// Renders the full `<section id="screen-chat">` layout from `app.html`:
// a screen-header (with session-id label + READY/STREAMING status + token
// budget), a `.chat-mini` container that holds the `.chat-stream` (per-
// bubble `chat-msg.{user|assistant|error}` rows with embedded
// `.chat-progress` tool-call rows), and the `.chat-input-pill` composer.
//
// All chat state is consumed from the context providers established by
// HermesApp in `hermes_app/mod.rs`:
//
//   - `Signal<Vec<ChatBubble>>`   — the bubble list (bare signal)
//   - `Signal<Option<u64>>`       — id of the currently-streaming bubble
//   - `Signal<(u32, u32)>`        — (used, max) token budget
//   - `SessionIdContext`          — B-03 newtype wrapping the session id
//   - `ChatSendHandler`           — newtype wrapping the send EventHandler
//
// Submit semantics mirror Phase 22.3 D-15 (the shell_legacy `input_box.rs`
// rule, per D-20): Enter submits, Shift+Enter inserts a literal newline.
// Slash commands (`/help`, `/clear`, `/research …`) are NOT parsed client-
// side — they flow over the WebSocket as plain text frames and the
// server's existing CommandRouter resolves them (D-20).
#[component]
pub fn ScreenChat(is_active: bool) -> Element {
    // Context lookups — every read drops its borrow before the rsx tree
    // is constructed (clippy.toml signal-borrow safety).
    let mut bubbles = use_context::<Signal<Vec<ChatBubble>>>();
    let mut streaming_id = use_context::<Signal<Option<u64>>>();
    let tokens = use_context::<Signal<(u32, u32)>>();
    let session_id = use_context::<crate::state::SessionIdContext>().0;
    let send = use_context::<ChatSendHandler>();
    // Phase 26.7.2 D-06: next_id for monotonic bubble ID allocation in history load.
    // Provided via context by HermesApp so IDs don't collide with the WS receive loop.
    let mut next_id = use_context::<Signal<u64>>();

    // Local composer state — text being typed in the `chat-input-pill`
    // textarea. Cleared on submit (Enter or the SEND button).
    let mut input = use_signal(String::new);

    // Auto-scroll on new bubbles — port of the warp_hermes.rs pattern.
    // Read the length into a local, drop the borrow, then poke the DOM.
    use_effect(move || {
        let len = bubbles.read().len();
        if len > 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Some(doc) = window.document() {
                        if let Ok(Some(el)) =
                            doc.query_selector(".chat-stream .chat-msg:last-child")
                        {
                            el.scroll_into_view_with_bool(false);
                        }
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = len; // suppress unused-variable on host builds
            }
        }
    });

    // Phase 26.7.2 D-06: History load — fires on every session_id change.
    // Signal-borrow discipline (clippy.toml): session_id borrow cloned into
    // owned String before spawn; no GenerationalRef held across await.
    // Subscribes only to session_id — writing bubbles/next_id inside spawn
    // does NOT retrigger this effect (Pitfall 2: no loop).
    use_effect(move || {
        // Clone into owned String; borrow ends at ; (RESEARCH Pitfall 1 / Q8).
        let sid = session_id.read().clone();
        // D-03/Q3: skip if session bootstrap has not resolved yet.
        if sid == "pending" {
            return;
        }

        // D-08: always clear on session_id change — no stale history leaks.
        bubbles.write().clear();
        // Cancel any stale STREAMING indicator from a previous session (Q8 / T-26.7.2-11).
        streaming_id.set(None);

        #[cfg(target_arch = "wasm32")]
        web_sys::console::log_1(&format!("[26.7.2] history effect fired — sid={sid:?}").into());

        spawn(async move {
            match crate::server::api::get_session_messages(sid.clone()).await {
                Ok(msgs) if !msgs.is_empty() => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("[26.7.2] got {} msgs for {sid:?}", msgs.len()).into());
                    // Map ChatMessage → ChatBubble. Read next_id as Copy u64
                    // before any .await; borrow ends at ; (CLAUDE.md signal rules).
                    let mut id_val = *next_id.read();
                    let mut history: Vec<ChatBubble> = msgs
                        .into_iter()
                        .filter_map(|m| match m.role.as_str() {
                            "user" => {
                                let b = ChatBubble::user(id_val, m.content);
                                id_val += 1;
                                Some(b)
                            }
                            "assistant" => {
                                let b = ChatBubble::assistant(id_val, m.content);
                                id_val += 1;
                                Some(b)
                            }
                            _ => None,
                        })
                        .collect();

                    // D-02 / Pitfall 3: insert divider only when history non-empty.
                    // Guarded by the outer Ok(msgs) if !msgs.is_empty() arm.
                    history.push(ChatBubble {
                        id: id_val,
                        kind: ChatBubbleKind::Divider,
                        text: String::new(),
                        tool_rows: vec![],
                    });
                    id_val += 1;

                    // Single write — all history + divider atomically appended.
                    // WriteLock acquired and released before next .await (none follows).
                    bubbles.write().extend(history);
                    next_id.set(id_val);
                }
                Ok(_) => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("[26.7.2] empty response for {sid:?}").into());
                }
                Err(e) => {
                    #[cfg(target_arch = "wasm32")]
                    web_sys::console::log_1(&format!("[26.7.2] error for {sid:?}: {e}").into());
                }
            }
        });
    });

    // Header-bar reads — pull into locals so the rsx tree never holds a
    // live signal borrow across an attribute-evaluation boundary.
    let streaming = streaming_id.read().is_some();
    let (used, max) = *tokens.read();
    let sid_full = session_id.read().clone();
    let sid_short: String = if sid_full.len() <= 8 {
        sid_full.clone()
    } else {
        sid_full[sid_full.len() - 8..].to_string()
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-chat",
            "data-screen-label": "01 Chat",

            // Screen header — mirrors app.html lines ~362-374. Carries the
            // module tag, title, sub-copy, and session/token affordances.
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 01" }
                    h1 { class: "screen-title", "Chat" }
                    p { class: "screen-sub",
                        "Streaming conversation with slash commands, live tool progress, and per-message token accounting."
                    }
                }
                div { class: "screen-actions",
                    span { class: "screen-meta",
                        "SID "
                        code { "{sid_short}" }
                    }
                    span { class: "screen-meta",
                        "TOK {used} / {max}"
                    }
                    if streaming {
                        span { class: "screen-meta is-streaming", "STREAMING" }
                    } else {
                        span { class: "screen-meta", "READY" }
                    }
                }
            }

            // Chat container — `.chat-mini > .chat-main > (chat-stream + chat-input-pill)`
            // per app.html. Class strings match the screens.css selectors
            // verbatim so the design renders pixel-faithful.
            div { class: "chat-mini",
                div { class: "chat-main",

                    // Scrollable bubble stream.
                    div { class: "chat-stream",
                        for b in bubbles.read().iter() {
                            div {
                                key: "{b.id}",
                                class: match b.kind {
                                    ChatBubbleKind::User      => "chat-msg user",
                                    ChatBubbleKind::Assistant => "chat-msg assistant",
                                    ChatBubbleKind::Error     => "chat-msg error",
                                    ChatBubbleKind::Divider   => "chat-divider",
                                },

                                // Avatar — the shield-caduceus PNG for the
                                // assistant (D-09), a textual badge for user
                                // and error bubbles.
                                match b.kind {
                                    ChatBubbleKind::Assistant => rsx! {
                                        div { class: "avatar logo",
                                            img { src: AVATAR_LOGO, alt: "" }
                                        }
                                    },
                                    ChatBubbleKind::User => rsx! {
                                        div { class: "avatar amber", "OP" }
                                    },
                                    ChatBubbleKind::Error => rsx! {
                                        div { class: "avatar error", "!" }
                                    },
                                    ChatBubbleKind::Divider => rsx! {},
                                }

                                // Bubble body + embedded tool-call progress
                                // rows for assistant bubbles (D-19).
                                // Phase 26.7.2 D-02: Divider variant renders a
                                // section-label rule instead of a bubble body.
                                if b.kind != ChatBubbleKind::Divider {
                                    div { class: "chat-bubble-wrap",
                                        div {
                                            class: match b.kind {
                                                ChatBubbleKind::User      => "chat-bubble is-user",
                                                ChatBubbleKind::Assistant => "chat-bubble is-assistant",
                                                ChatBubbleKind::Error     => "chat-bubble is-error",
                                                ChatBubbleKind::Divider   => "chat-bubble is-divider",
                                            },
                                            // `{b.text}` is rendered as a plain
                                            // Dioxus text node — auto-escaped,
                                            // no `dangerous_inner_html` (T-26.2.1-19).
                                            "{b.text}"

                                            if !b.tool_rows.is_empty() {
                                                div { class: "chat-progress",
                                                    for tr in b.tool_rows.iter() {
                                                        {
                                                            let state_class = if tr.done {
                                                                if tr.success { "is-done is-success" } else { "is-done is-error" }
                                                            } else {
                                                                "is-running"
                                                            };
                                                            let icon_class = if tr.done { "icon done" } else { "icon spin" };
                                                            let icon_glyph = if tr.done { "●" } else { "◐" };
                                                            rsx! {
                                                                div {
                                                                    class: "chat-progress-row {state_class}",
                                                                    span {
                                                                        class: "{icon_class}",
                                                                        "{icon_glyph}"
                                                                    }
                                                                    span { class: "tp-name", "{tr.name}" }
                                                                    if !tr.args.is_empty() {
                                                                        span { class: "tp-args", " · {tr.args}" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    // Phase 26.7.2 D-02: history/live boundary marker.
                                    // Matches the `.section-label` convention from sessions.rs.
                                    div { class: "chat-divider-label",
                                        span { class: "section-label", "─── Earlier ───" }
                                    }
                                }
                            }
                        }
                    }

                    // Composer pill — textarea + SEND button. Per D-20 /
                    // Phase 22.3 D-15: Enter submits, Shift+Enter inserts
                    // a literal newline. Slash commands are NOT parsed
                    // here — the server's CommandRouter handles `/help`,
                    // `/clear`, `/research …` etc.
                    form {
                        class: "chat-input-pill",
                        onsubmit: move |evt| {
                            evt.prevent_default();
                            let text = { let s = input.read(); s.clone() };
                            send.0.call(text);
                            input.set(String::new());
                        },
                        textarea {
                            rows: "1",
                            placeholder: "/ slash, or just message…",
                            value: "{input}",
                            oninput: move |evt| input.set(evt.value()),
                            onkeydown: move |evt| {
                                if evt.key() == Key::Enter && !evt.modifiers().shift() {
                                    evt.prevent_default();
                                    let text = { let s = input.read(); s.clone() };
                                    send.0.call(text);
                                    input.set(String::new());
                                }
                            }
                        }
                        button {
                            r#type: "submit",
                            "▓ SEND"
                        }
                    }
                }
            }
        }
    }
}
