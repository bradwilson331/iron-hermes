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
// ScreenChat component (Plan 06 Task 2 replaces this body)
// ---------------------------------------------------------------------------

#[component]
pub fn ScreenChat(is_active: bool) -> Element {
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-chat",
            "data-screen-label": "01 Chat",
            div { class: "screen-placeholder",
                "Chat screen — wired by Plan 06"
            }
        }
    }
}
