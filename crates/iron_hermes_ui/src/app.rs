use dioxus::prelude::*;

// ---------------------------------------------------------------------------
// Default-build assets (Phase 26.2.1 new shell)
// ---------------------------------------------------------------------------
//
// Load order matches CONTEXT D-07 / RESEARCH Pattern 1:
//   tokens → site → wheel → screens → components
// `tokens.css` MUST come first because it declares the CSS custom properties
// the other four sheets consume.

const FAVICON: Asset = asset!("/assets/favicon.ico");
const TOKENS_CSS: Asset = asset!("/assets/tokens.css");
const SITE_CSS: Asset = asset!("/assets/site.css");
const WHEEL_CSS: Asset = asset!("/assets/wheel.css");
const SCREENS_CSS: Asset = asset!("/assets/screens.css");
const COMPONENTS_CSS: Asset = asset!("/assets/components.css");

// ---------------------------------------------------------------------------
// Legacy-shell assets (only compiled when the `legacy-shell` feature is on)
// ---------------------------------------------------------------------------
//
// Kept compiling as a UAT fallback per D-25/D-26. Both the asset constants
// and the Link tags below are gated so the default WASM bundle never
// references them.

#[cfg(feature = "legacy-shell")]
const TAILWIND_CSS: Asset = asset!("/assets/tailwind.css");
#[cfg(feature = "legacy-shell")]
const MAIN_CSS: Asset = asset!("/assets/main.css");
#[cfg(feature = "legacy-shell")]
const DESIGN_TOKENS_CSS: Asset = asset!("/assets/design-tokens.css");
#[cfg(feature = "legacy-shell")]
const WARP_IH_CSS: Asset = asset!("/assets/warp-ih.css");
#[cfg(feature = "legacy-shell")]
const SCANNER_ANIM_CSS: Asset = asset!("/assets/scanner-anim.css");

#[component]
pub fn App() -> Element {
    rsx! {
        document::Meta { name: "viewport", content: "width=device-width, initial-scale=1" }
        document::Link { rel: "icon", href: FAVICON }

        // Optional fonts hosted on Google for parity with the React prototype
        // (Ioskeley Mono webfonts live under /assets/fonts/ and are loaded by
        // `tokens.css` / `site.css` via @font-face — no Link tag needed for them).
        document::Link { rel: "preconnect", href: "https://fonts.googleapis.com" }
        document::Link { rel: "preconnect", href: "https://fonts.gstatic.com", crossorigin: "" }
        document::Link {
            rel: "stylesheet",
            href: "https://fonts.googleapis.com/css2?family=JetBrains+Mono:ital,wght@0,400;0,500;0,700;1,400&display=swap",
        }

        // New bundle CSS — load order is significant (tokens first).
        document::Link { rel: "stylesheet", href: TOKENS_CSS }
        document::Link { rel: "stylesheet", href: SITE_CSS }
        document::Link { rel: "stylesheet", href: WHEEL_CSS }
        document::Link { rel: "stylesheet", href: SCREENS_CSS }
        document::Link { rel: "stylesheet", href: COMPONENTS_CSS }

        // Legacy bundle CSS — only emitted when the legacy shell is mounted.
        // Use a nested `legacy_links()` helper so the rsx! parser sees one
        // expression slot; the helper itself is cfg-branched at item scope.
        {legacy_links()}

        // Root child — compile-time branch (not runtime) so the OFF shell is
        // not pulled into the WASM binary. Same helper-fn pattern as above.
        {root_shell()}
    }
}

// ---------------------------------------------------------------------------
// Cfg-branched rsx fragments (compile-time selected — RESEARCH Pattern 1).
// ---------------------------------------------------------------------------

#[cfg(feature = "legacy-shell")]
fn legacy_links() -> Element {
    rsx! {
        document::Link { rel: "stylesheet", href: TAILWIND_CSS }
        document::Link { rel: "stylesheet", href: MAIN_CSS }
        document::Link { rel: "stylesheet", href: DESIGN_TOKENS_CSS }
        document::Link { rel: "stylesheet", href: WARP_IH_CSS }
        document::Link { rel: "stylesheet", href: SCANNER_ANIM_CSS }
    }
}

#[cfg(not(feature = "legacy-shell"))]
fn legacy_links() -> Element {
    rsx! {}
}

#[cfg(feature = "legacy-shell")]
fn root_shell() -> Element {
    rsx! { crate::components::warp_hermes::WarpHermes {} }
}

#[cfg(not(feature = "legacy-shell"))]
fn root_shell() -> Element {
    rsx! { crate::components::hermes_app::HermesApp {} }
}
