//! Teaser cards (E10, D-01) — disabled "NOT AVAILABLE" cards for Email and
//! Voice, whose adapters are deferred to their own future phases. No
//! configuration path exists this phase — the CONFIGURE button is OMITTED
//! entirely (not rendered disabled), since there is nothing to configure.
//!
//! `scope` is accepted (not read) to keep this child's prop signature
//! consistent with its `ChatPlatformCards`/`WebhookRouteCards`/
//! `ApiServerCard` siblings (`mod.rs`'s established contract, Plan 01) even
//! though this purely static card needs no scope-scoped data.
//!
//! # Reuses `.plat-card` unmodified — no new CSS (D-10)
//!
//! `.plat-state`'s color rule in `screens.css` is `var(--gray)` by DEFAULT
//! and only becomes `--teal` under the `.plat-card.connected` modifier
//! (`screens.css:587-588`). A teaser card rendered WITHOUT `connected`
//! therefore already renders gray — exactly D-01's "NOT AVAILABLE, gray"
//! requirement — with no new class needed at all.

use crate::server::tools_config_api::ConfigScope;
use dioxus::prelude::*;

#[component]
pub fn TeaserCards(scope: ReadSignal<ConfigScope>) -> Element {
    let _ = scope;
    rsx! {
        TeaserCard { name: "Email" }
        TeaserCard { name: "Voice" }
    }
}

/// One disabled teaser card. Deliberately renders NO button of any kind —
/// D-01's "the button is omitted, not disabled" clause — so a structural
/// check for an absent `button` node in this component's output is a real
/// assertion, not an accident of styling.
#[component]
fn TeaserCard(name: &'static str) -> Element {
    rsx! {
        div { class: "plat-card",
            div { class: "plat-head",
                div { class: "plat-glyph", "▦" }
                div { style: "flex:1;",
                    div { class: "plat-name", "{name}" }
                    div { class: "plat-state", "NOT AVAILABLE" }
                }
            }
            dl { class: "kv",
                dt { "Host" }
                dd { "—" }
                dt { "Agent" }
                dd { "—" }
            }
        }
    }
}
