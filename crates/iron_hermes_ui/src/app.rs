use crate::components::WarpHermes;
use dioxus::prelude::*;

const FAVICON: Asset = asset!("/assets/favicon.ico");
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
const MAIN_CSS: Asset = asset!("/assets/main.css");
const DESIGN_TOKENS_CSS: Asset = asset!("/assets/design-tokens.css");
const WARP_IH_CSS: Asset = asset!("/assets/warp-ih.css");
const SCANNER_ANIM_CSS: Asset = asset!("/assets/scanner-anim.css");
const _IH_SHIELD: Asset = asset!("/assets/ih-shield.png");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Link { rel: "icon", href: FAVICON }
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }
        document::Link { rel: "stylesheet", href: WARP_IH_CSS }
        document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }
        WarpHermes {}
    }
}
