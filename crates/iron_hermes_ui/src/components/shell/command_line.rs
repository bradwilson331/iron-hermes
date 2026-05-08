use crate::state::Token;
use dioxus::prelude::*;

/// CommandLine row. Renders cwd + prompt glyph + ordered token spans + optional
/// time. Token kind ("bin"/"arg"/"flag"/"str") drives the per-span class
/// (`wh-cmd-bin`/`wh-cmd-arg`/`wh-cmd-flag`/`wh-cmd-str`); the bin token also
/// gets the `wh-cmd` modifier class for the stronger fg-strong + 700-weight
/// styling per `assets/warp-ih.css`.
///
/// Port of `warp2ironhermes/project/app/shell.jsx` lines 115-130 per CONTEXT D-01.
///
/// Note on naming: `crate::state::CommandLine` is the data struct;
/// `crate::components::shell::CommandLine` is this component function.
/// We import only `Token` here so the names don't collide.
#[component]
pub fn CommandLine(
    tokens: Vec<Token>,
    time: Option<String>,
    cwd: Option<String>,
    glyph: Option<String>,
) -> Element {
    let cwd_text = cwd.unwrap_or_else(|| "~".to_string());
    let glyph_char = glyph.unwrap_or_else(|| "❯".to_string());
    rsx! {
        div { class: "wh-cmdline",
            span { style: "color: var(--fg-dim); font-size: 11px;", "{cwd_text}" }
            span { class: "wh-prompt-glyph", "{glyph_char}" }
            span { class: "wh-cmd-tokens", style: "flex: 1;",
                for (i, t) in tokens.iter().enumerate() {
                    span {
                        key: "{i}",
                        class: "wh-cmd-{t.kind_class()}",
                        class: if matches!(t, Token::Bin(_)) { "wh-cmd" },
                        if i > 0 { " " }
                        "{t.text()}"
                    }
                }
            }
            if let Some(t) = time { span { class: "wh-cmd-time", "{t}" } }
        }
    }
}
