use super::block::Block;
use crate::state::{Block as BlockData, BlockEntry, Token};
use dioxus::prelude::*;

/// Block stream — owns the `wh-stream` / `wh-stream-scroll` chrome and
/// iterates over a reactive `ReadSignal<Vec<BlockEntry>>` per
/// CONTEXT D-01.
///
/// Per CONTEXT D-07: each iterated child uses `key: "{entry.id}"` for
/// stable identity across `/clear` (Vec emptied) + append cycles. The
/// old `key: "{i}"` index-based key would collide on append.
///
/// Copy/rerun handlers live here (not in Block) so Block stays free of
/// `web_sys` imports. Block fires `on_copy.call(())` and BlockStream
/// resolves the entry id → text mapping per CONTEXT D-23, then writes
/// to the browser clipboard fire-and-forget. `on_rerun: EventHandler<u64>`
/// forwards the entry id up to WarpHermes which dispatches `run_shell`.
#[component]
pub fn BlockStream(blocks: ReadSignal<Vec<BlockEntry>>, on_rerun: EventHandler<u64>) -> Element {
    rsx! {
        div { class: "wh-stream",
            div { class: "wh-stream-scroll",
                for entry in blocks.read().iter().cloned() {
                    Block {
                        key: "{entry.id}",
                        entry: entry.clone(),
                        on_copy: {
                            let entry_for_copy = entry.clone();
                            move |_| {
                                let text = block_text_for_copy(&entry_for_copy);
                                write_to_clipboard(&text);
                            }
                        },
                        on_rerun: move |_| on_rerun.call(entry.id),
                    }
                }
            }
        }
    }
}

/// Assemble the copy-text for a block per CONTEXT D-23.
///
/// Cmd → tokens joined by space; Out/Ai/Ok/Err → message/text/markdown;
/// Tool → "{name} {args_summary}".
fn block_text_for_copy(entry: &BlockEntry) -> String {
    match &entry.block {
        BlockData::Cmd { command } => command
            .tokens
            .iter()
            .map(token_text)
            .collect::<Vec<_>>()
            .join(" "),
        BlockData::Out { text, .. } => text.clone(),
        BlockData::Ai { markdown, .. } => markdown.clone(),
        BlockData::Ok { message, .. } => message.clone(),
        BlockData::Err { message, .. } => message.clone(),
        BlockData::Tool { call } => format!("{} {}", call.name, call.args_summary),
    }
}

fn token_text(t: &Token) -> String {
    match t {
        Token::Bin(s) | Token::Arg(s) | Token::Flag(s) | Token::Str(s) => s.clone(),
    }
}

/// Fire-and-forget clipboard write per CONTEXT D-23.
///
/// Failures (no clipboard, permission denied) are silent — Phase 4 has no
/// toast/feedback UI per D-23. The returned Promise is dropped; JS engine
/// still executes per RESEARCH Common Op 2 + Assumptions Log A4.
fn write_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Native build: no-op (clipboard wiring is web-only in Phase 4).
        let _ = text;
    }
}
