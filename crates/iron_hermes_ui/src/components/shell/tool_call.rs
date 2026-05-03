use dioxus::prelude::*;
use crate::state::ToolStatus;

/// Tool-call card. Displays the tool name, optional args summary, and
/// status text in a yellow-bordered card (`.wh-toolcall` styling already in
/// `assets/warp-ih.css`).
///
/// Port of `warp2ironhermes/project/app/shell.jsx` lines 132-147 per CONTEXT D-01.
///
/// Note on naming: `crate::state::ToolCall` is the data struct;
/// `crate::components::shell::ToolCall` is this component function.
/// We import only `ToolStatus` here so the names don't collide.
///
/// `match &status { ... }` borrows the enum without requiring `Copy` — keeps
/// `state.rs` unchanged (ToolStatus there derives only Clone+PartialEq+Debug).
#[component]
pub fn ToolCall(name: String, args_summary: String, status: ToolStatus) -> Element {
    let status_color = match &status {
        ToolStatus::Done => "var(--success)",
        ToolStatus::Failed => "var(--danger)",
        ToolStatus::Pending | ToolStatus::Running => "var(--warn)",
    };
    let status_text = match &status {
        ToolStatus::Done => "[OK]",
        ToolStatus::Pending => "pending…",
        ToolStatus::Running => "running…",
        ToolStatus::Failed => "failed",
    };
    rsx! {
        div { class: "wh-toolcall",
            div { style: "display: flex; gap: 8px; align-items: baseline;",
                span { style: "color: var(--fg-dim);", "Tool:" }
                b { "{name}" }
                span {
                    style: "margin-left: auto; font-size: 10px; color: {status_color};",
                    "{status_text}"
                }
            }
            if !args_summary.is_empty() {
                pre {
                    style: "margin: 4px 0 0; color: var(--fg-dim); font-size: 11px; white-space: pre-wrap;",
                    "{args_summary}"
                }
            }
        }
    }
}
