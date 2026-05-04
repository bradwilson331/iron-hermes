use super::command_line::CommandLine;
use super::markdown::render_inline_code;
use super::tool_call::ToolCall;
use crate::state::{Block as BlockData, BlockEntry};
use dioxus::prelude::*;

/// Block — outer chrome for any of the six block kinds with a variant-
/// dispatched body and a hover-action button row.
///
/// Phase 4 (per CONTEXT D-07): prop type changed from `data: BlockData`
/// to `entry: BlockEntry` (BlockEntry wraps Block with stable `id`).
/// Adds `on_copy` and `on_rerun` EventHandler props (D-23 / D-24 +
/// KBD-05). is-ai body now invokes `render_inline_code(&markdown)` per
/// D-15 + D-16. Share button continues to render but is a click-no-op
/// per D-25 (Phase 3 visual stays intact).
///
/// Hover affordances are pure CSS per Phase 3 D-15: action buttons
/// render unconditionally; `assets/warp-ih.css` `.wh-block:hover
/// .wh-block-actions { opacity: 1 }` controls visibility.
#[component]
pub fn Block(entry: BlockEntry, on_copy: EventHandler<()>, on_rerun: EventHandler<()>) -> Element {
    let data = entry.block.clone();
    let kind_class = data.kind_class();

    let (author, time, exit_code) = match &data {
        BlockData::Cmd { command } => (None, command.time.clone(), None),
        BlockData::Out { author, time, .. } => (author.clone(), time.clone(), None),
        BlockData::Ai { author, time, .. } => (author.clone(), time.clone(), None),
        BlockData::Ok { author, time, .. } => (author.clone(), time.clone(), None),
        BlockData::Err {
            author,
            time,
            exit_code,
            ..
        } => (author.clone(), time.clone(), Some(*exit_code)),
        BlockData::Tool { .. } => (None, None, None),
    };
    let is_ok = matches!(data, BlockData::Ok { .. });
    let is_err = matches!(data, BlockData::Err { .. });
    let is_cmd = matches!(data, BlockData::Cmd { .. });

    rsx! {
        div { class: "wh-block {kind_class}",
            div { class: "wh-block-head",
                if let Some(author) = author { span { class: "wh-author", "{author}" } }
                if is_ok  { span { style: "color: var(--success); font-size: 10px;", "[OK]" } }
                if is_err {
                    span {
                        style: "color: var(--danger); font-size: 10px;",
                        if let Some(code) = exit_code { "exit {code}" } else { "exit 1" }
                    }
                }
                if let Some(t) = time { span { style: "margin-left: auto; font-size: 11px; color: var(--fg-dim);", "{t}" } }
            }
            // Variant-dispatched body
            match data.clone() {
                BlockData::Cmd { command } => rsx! {
                    CommandLine {
                        tokens: command.tokens,
                        time: command.time,
                        cwd: command.cwd,
                        glyph: command.glyph,
                    }
                },
                BlockData::Tool { call } => rsx! {
                    ToolCall {
                        name: call.name,
                        args_summary: call.args_summary,
                        status: call.status,
                    }
                },
                BlockData::Out { text, .. } => rsx! { div { class: "wh-block-body", "{text}" } },
                BlockData::Ai  { markdown, .. } => rsx! { div { class: "wh-block-body", {render_inline_code(&markdown)} } },
                BlockData::Ok  { message, .. } => rsx! { div { class: "wh-block-body", "{message}" } },
                BlockData::Err { message, .. } => rsx! { div { class: "wh-block-body", "{message}" } },
            }
            // Hover-action button row — always rendered.
            // Cmd: copy + rerun. Non-Cmd: copy + rerun (disabled) + share.
            div { class: "wh-block-actions",
                button {
                    class: "wh-icon-btn",
                    title: "copy",
                    onclick: move |_| on_copy.call(()),
                    "⎘"
                }
                button {
                    class: "wh-icon-btn",
                    title: "rerun",
                    style: if !is_cmd { "cursor: not-allowed; opacity: 0.5;" } else { "" },
                    onclick: move |_| { if is_cmd { on_rerun.call(()); } },
                    "↻"
                }
                if !is_cmd {
                    button {
                        class: "wh-icon-btn",
                        title: "share",
                        onclick: move |_| { /* D-25: share unwired in Phase 4; v2 needs real share-link backend. */ },
                        "↗"
                    }
                }
            }
        }
    }
}
