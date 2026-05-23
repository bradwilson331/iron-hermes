//! Gateway screen — ported from `app.html` `<section id="screen-gateway">`
//! (lines 1150-1263). Renders 6 platform cards from
//! `stub_data::gateway_platforms()` in the prototype's `.plat-card` grid.
//! Pure visual stub (D-04) — zero server calls.

use dioxus::prelude::*;

use crate::mocks::stub_data::{gateway_platforms, GatewayPlatformStub};

#[component]
pub fn ScreenGateway(is_active: bool) -> Element {
    let platforms = gateway_platforms();
    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-gateway",
            "data-screen-label": "10 Gateway",

            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 10" }
                    h1 { class: "screen-title", "Gateway" }
                    p { class: "screen-sub",
                        "Bridge Hermes to messaging platforms. Each integration runs as its own listener with credentials scoped per agent."
                    }
                }
                div { class: "screen-actions",
                    button { class: "btn btn--ghost btn--sm", "↻ RESTART ALL" }
                    button { class: "btn btn--sm", "+ ADD PLATFORM" }
                }
            }

            div { class: "grid wide",
                for g in platforms.iter() {
                    PlatformCard { key: "{g.name}", platform: g.clone() }
                }
            }
        }
    }
}

#[component]
fn PlatformCard(platform: GatewayPlatformStub) -> Element {
    let connected = platform.status == "CONNECTED";
    rsx! {
        div {
            class: "plat-card",
            class: if connected { "connected" },

            div { class: "plat-head",
                div { class: "plat-glyph", "▦" }
                div { style: "flex:1",
                    div { class: "plat-name", "{platform.name}" }
                    div { class: "plat-state",
                        if connected {
                            "{platform.status} · {platform.chats_connected} chats"
                        } else {
                            "{platform.status}"
                        }
                    }
                }
                div {
                    class: if connected { "tgl on" } else { "tgl" },
                    "data-tgl": "true",
                }
            }

            if connected {
                dl { class: "kv",
                    dt { "Chats" } dd { "{platform.chats_connected}" }
                    dt { "Status" } dd { "{platform.status}" }
                }
            } else {
                dl { class: "kv",
                    dt { "Host" } dd { "—" }
                    dt { "Agent" } dd { "—" }
                }
                button {
                    class: "btn btn--ghost btn--sm",
                    style: "align-self:flex-start;",
                    "CONFIGURE →"
                }
            }
        }
    }
}
