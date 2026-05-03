//! 3-stage agent reply chain per CONTEXT D-10 + MOCK-03.
//!
//! Port of `warp2ironhermes/project/app/app.jsx` `runAgent` (lines 176-185).
//! Stages: append user message → sleep(400) → append hermes tool-call
//! message → sleep(1000) → append hermes reply (via personality table).
//!
//! Borrow-then-await discipline (CONTEXT D-06 + clippy.toml
//! `await-holding-invalid-types`): every `.write()` borrow MUST drop at
//! the semicolon BEFORE any `.await`. Reads are cloned into owned locals
//! before `.await`. See RESEARCH.md Pattern 2 for the canonical GOOD/BAD
//! comparison; this file follows the GOOD pattern in every stage.
//!
//! Module-level `#![allow(dead_code)]`: `run_agent_steps` is consumed by
//! Wave 4 (Plan 04-04a/04-04b WarpHermes rewire). Until then, it is
//! "ready but unwired" — clippy `-D warnings` would otherwise reject the
//! Wave 3 gate.
#![allow(dead_code)]

use crate::mocks::personalities::pick_reply;
use crate::platform::timer::sleep;
use crate::state::{now_time, Message, Personality, ToolCall, ToolStatus};
use dioxus::prelude::*;

/// Append `prompt` as a user message, then after 400ms append a hermes
/// tool-call message (search), then after 1000ms append the personality-
/// matched reply. v2 swap target: same signature backed by a
/// `dioxus_fullstack` server-fn into `ironhermes-agent` (CONTEXT D-36).
pub async fn run_agent_steps(
    prompt: String,
    personality: Personality,
    mut messages: Signal<Vec<Message>>,
) {
    // ── Stage 1: user message. .write() borrow drops at the semicolon. ──
    messages.write().push(Message {
        who: "user".into(),
        time: now_time(),
        body: prompt.clone(),
        tool: None,
    });

    // No live signal borrows → safe to await.
    sleep(400).await;

    // ── Stage 2: hermes tool-call message. ──
    // `summary` is owned String (no signal borrow); safe to construct
    // before the await chain.
    let summary: String = prompt.chars().take(40).collect();
    messages.write().push(Message {
        who: "hermes".into(),
        time: now_time(),
        body: String::new(),
        tool: Some(ToolCall {
            name: "search".into(),
            args_summary: format!("{{\"q\":\"{summary}\"}}"),
            status: ToolStatus::Done,
        }),
    });

    sleep(1000).await;

    // ── Stage 3: hermes reply pulled from personality table. ──
    // pick_reply returns &'static str; .to_string() owns it. No signal borrow.
    let reply = pick_reply(personality).to_string();
    messages.write().push(Message {
        who: "hermes".into(),
        time: now_time(),
        body: reply,
        tool: None,
    });
}
