//! Inline-code markdown renderer per CONTEXT D-15 + D-16.
//!
//! Plain `pub fn` (NOT a `#[component]`) returning Element. Splits text on
//! backticks; even-indexed segments render as `<span>`, odd-indexed as
//! `<code>`. Wrapped in `<div style="white-space: pre-wrap;">` so newlines
//! in personality replies (e.g., Hype) render correctly. ~20 LOC, no new
//! crate (full markdown deferred to v2).
//!
//! Caller pattern:  rsx! { div { class: "wh-block-body", {render_inline_code(&markdown)} } }

use dioxus::prelude::*;

/// Render text with inline `<code>` spans for backtick-delimited segments.
pub fn render_inline_code(text: &str) -> Element {
    let parts: Vec<&str> = text.split('`').collect();
    rsx! {
        div { style: "white-space: pre-wrap;",
            for (i, seg) in parts.iter().enumerate() {
                if i % 2 == 0 {
                    span { key: "p{i}", "{seg}" }
                } else {
                    code { key: "c{i}", "{seg}" }
                }
            }
        }
    }
}
