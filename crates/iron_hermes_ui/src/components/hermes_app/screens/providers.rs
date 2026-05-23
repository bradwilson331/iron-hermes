//! Providers screen — D-05: there is no `<section id="screen-providers">`
//! in app.html. Plan 02 derived the data from `project/screens/Providers.tsx`
//! (which exposes provider env keys + model picker + credential pool),
//! but the TSX reference is React-form-heavy with hook-driven state and
//! `window.hermesAPI` calls — none of which fit the visual-stub contract
//! (D-04). Per the plan's anti-pattern list (no React hook ports —
//! replace with plain Rust expressions), this screen surfaces the
//! `stub_data::providers()` rows in the same `.card` / `.grid wide`
//! visual vocabulary used by the other 9 screens — keeping it coherent
//! with the rest of the design while leaving room for 26.2.10 to wire
//! the real provider editor surface.
//!
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{providers, ProviderStub};

#[component]
pub fn ScreenProviders(is_active: bool) -> Element {
    let provider_list = providers();
    let provider_count = provider_list.len();
    let active_count = provider_list.iter().filter(|p| p.status == "ACTIVE").count();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-providers",
            "data-screen-label": "13 Providers",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 13" }
                    h1 { class: "screen-title", "Providers" }
                    p { class: "screen-sub",
                        "API providers and credential pools. "
                        code { style: "color:var(--teal)", "{active_count}" }
                        " active of "
                        code { style: "color:var(--teal)", "{provider_count}" }
                        " configured."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "⇣ EXPORT" }
                    button { class: "btn btn--sm", "+ NEW PROVIDER" }
                }
            }

            div { class: "grid wide",
                for p in provider_list.iter() {
                    ProviderCard { key: "{p.id}", provider: p.clone() }
                }
            }
        }
    }
}

#[component]
fn ProviderCard(provider: ProviderStub) -> Element {
    let is_active_card = provider.status == "ACTIVE";
    let pill_class = match provider.status {
        "ACTIVE" => "pill teal",
        "IDLE" => "pill amber",
        _ => "pill",
    };
    rsx! {
        div {
            class: "card",
            class: if is_active_card { "is-active" },
            "data-provider-id": "{provider.id}",

            div { class: "card-head",
                div { class: "card-icon", "◉" }
                div { style: "flex:1",
                    div { class: "card-title", "{provider.label}" }
                    div { class: "card-meta", "{provider.model_count} models" }
                }
                span { class: "{pill_class}", "{provider.status}" }
            }
            div { class: "card-footer",
                div { style: "display:flex;gap:14px;font-size:10px;color:var(--gray);letter-spacing:0.06em;",
                    span {
                        "MODELS "
                        span { style: "color:var(--teal);font-weight:700",
                            "{provider.model_count}"
                        }
                    }
                    span {
                        "P50 "
                        span { style: "color:var(--teal);font-weight:700",
                            "{provider.latency_p50_ms}ms"
                        }
                    }
                }
                button { class: "btn btn--ghost btn--sm", "EDIT" }
            }
        }
    }
}
