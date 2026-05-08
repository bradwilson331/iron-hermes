use crate::state::Mode;
use dioxus::prelude::*;

/// InputBox — bottom-row form chrome with mode pill, prompt glyph,
/// auto-grow textarea, and right-side action buttons.
///
/// Phase 4 (per CONTEXT D-19 + KBD-04): textarea is controlled
/// (`value: "{value}"` + `oninput` → `value.set(...)`). Plain Enter calls
/// `on_submit.call(())` and prevents default; Shift+Enter inserts a
/// newline (browser-default textarea behavior — no special handling).
///
/// Per CONTEXT D-17: `focused` is a `Signal<bool>` written by `onfocus`
/// (true) and `onblur` (false). The global keydown listener in
/// `WarpHermes` reads this same signal to gate ⌥M mode toggle (so ⌥M
/// only fires when input is focused — avoids intercepting macOS
/// Option-letter combos that produce special characters).
///
/// Mode-driven pill/glyph/placeholder copy unchanged from Phase 3 UI-SPEC
/// lines 220-227. The run-button accent color preserved per UI-SPEC line 224.
#[component]
pub fn InputBox(
    value: Signal<String>,
    mode: ReadSignal<Mode>,
    focused: Signal<bool>,
    on_submit: EventHandler<()>,
) -> Element {
    let is_agent = matches!(mode(), Mode::Agent);
    let pill_label = if is_agent { "Agent" } else { "Shell" };
    let prompt_glyph = if is_agent { "✦" } else { "❯" };
    let placeholder = if is_agent {
        "message hermes…"
    } else {
        "shell command…"
    };
    rsx! {
        div {
            class: "wh-input-wrap",
            class: if focused() { "is-focus" },
            div { class: "wh-input-mode",
                span {
                    class: "wh-mode-pill",
                    class: if is_agent { "is-agent" },
                    "{pill_label}"
                }
                span { "⌥M mode" }
                span { style: "margin-left: auto;", "↵ run · ⇧↵ newline · ⌃C cancel" }
            }
            div { class: "wh-input-row",
                span { class: "wh-prompt-glyph", "{prompt_glyph}" }
                textarea {
                    class: "wh-textarea",
                    rows: "1",
                    placeholder: "{placeholder}",
                    "aria-label": if is_agent { "Message Hermes" } else { "Shell command" },
                    value: "{value}",
                    oninput: move |e| value.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && !e.modifiers().shift() {
                            e.prevent_default();
                            on_submit.call(());
                        }
                    },
                    onfocus: move |_| focused.set(true),
                    onblur:  move |_| focused.set(false),
                }
                div { class: "wh-input-actions",
                    button {
                        class: "wh-icon-btn",
                        title: "attach",
                        onclick: move |_| {},
                        "@"
                    }
                    button {
                        class: "wh-icon-btn",
                        title: "voice",
                        onclick: move |_| {},
                        "●"
                    }
                    button {
                        class: "wh-icon-btn",
                        title: "run",
                        style: "color: var(--accent-primary);",
                        onclick: move |_| on_submit.call(()),
                        "↵"
                    }
                }
            }
        }
    }
}
