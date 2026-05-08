use super::scanner::Scanner;
use crate::state::{ShellSettings, TokenBudget};
use dioxus::prelude::*;

/// Status bar — bottom of terminal column. Five `.wh-pill` spans
/// (mode/model/provider/tokens/personality) separated by `.wh-sep`
/// middots, plus the Scanner cells and right-aligned `.wh-hint`.
///
/// Phase 4 (per CONTEXT D-22): adds a NEW personality pill displaying
/// `settings.personality.label()` between the tokens pill and the
/// scanner. Read-only in Phase 4 (no click handler — TweaksPanel in
/// Phase 5 wires the click).
///
/// Phase 4 (per CONTEXT D-01): `tokens` and `scanner_active` are now
/// `ReadSignal<T>` so writes in WarpHermes (token pulse + scanner
/// pulse) trigger re-render of just this component. Other props stay
/// `String` (statics from WarpHermes).
#[component]
pub fn StatusBar(
    mode: String,
    model: String,
    provider: String,
    tokens: ReadSignal<TokenBudget>,
    scanner_active: ReadSignal<bool>,
    hint: String,
) -> Element {
    let t = tokens();
    let used_k = t.used as f32 / 1000.0;
    let max_k = t.max / 1000;
    let pct = if t.max > 0 {
        ((t.used as f32 / t.max as f32) * 100.0).round() as u32
    } else {
        0
    };

    let settings = use_context::<ShellSettings>();
    let pers_label = settings.personality.read().label();

    rsx! {
        div { class: "wh-status",
            span { class: "wh-pill", style: "color: var(--pill-0);", "{mode}" }
            span { class: "wh-sep", "·" }
            span { class: "wh-pill", style: "color: var(--pill-1);", "{model}" }
            span { class: "wh-sep", "·" }
            span { class: "wh-pill", style: "color: var(--pill-2);", "{provider}" }
            span { class: "wh-sep", "·" }
            span { class: "wh-pill", style: "color: var(--pill-3);", "{used_k:.1}K/{max_k}K ({pct}%)" }
            span { class: "wh-sep", "·" }
            span { class: "wh-pill", style: "color: var(--pill-4);", "/{pers_label}" }
            span { class: "wh-sep", "·" }
            Scanner { active: scanner_active() }
            span { class: "wh-hint", "{hint}" }
        }
    }
}
