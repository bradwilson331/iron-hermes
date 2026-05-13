//! Phase 26.2.1 — ThemeEffects: render-free DOM mutation host.
//!
//! Bundles three `use_effect` blocks that translate the four root signals
//! (`UiPrefs`, `ThemeContext`, `Screen`) into DOM mutations on `<html>` and
//! `<body>` (RESEARCH Pattern 6). The component returns `rsx! {}` — its only
//! purpose is to subscribe to context signals and write to the DOM.
//!
//! Allowed mutations (D-03 / D-24):
//!   1. `<html data-theme="…">` attribute  (theme picker)
//!   2. `<html style="--teal: …; --teal-bright: …">`  (accent override)
//!   3. `<body class="no-scanlines no-breadcrumb no-footer has-rail
//!                   density-dense on-chat">` toggles
//!
//! NOTHING ELSE in the DOM is touched here. Persistence to localStorage is
//! owned by Plan 03's hydration-gated effects in `hermes_app/mod.rs`.
//!
//! Signal-borrow safety (clippy.toml): every effect reads context signals
//! into Copy / cloned locals and drops the read borrow before the `#[cfg]`
//! block that runs web_sys calls.

use dioxus::prelude::*;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::JsCast;

/// Render-free DOM mutation host — see module docs.
///
/// Mount this as a sibling under `HermesApp` so its three effects fire
/// under the same context graph as the other shell children.
#[component]
pub fn ThemeEffects() -> Element {
    let prefs = use_context::<Signal<crate::ui_prefs::UiPrefs>>();
    let theme = use_context::<crate::state::ThemeContext>().0;
    let active_screen = use_context::<Signal<crate::state::Screen>>();

    // -----------------------------------------------------------------------
    // Effect 1 — write `<html data-theme="…">`.
    //
    // Plan 03's hydration effect owns persistence; this effect ONLY writes
    // the DOM attribute. The value originates from a closed Signal<String>
    // driven by 5 hardcoded picker buttons (T-26.2.1-15 accepts XSS) so no
    // sanitiser is needed.
    // -----------------------------------------------------------------------
    use_effect(move || {
        // Read into a local, then drop the borrow.
        let theme_name = theme.read().clone();
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.document_element())
            {
                let _ = el.set_attribute("data-theme", &theme_name);
            }
        }
        // Silence "unused on non-WASM" warning without cfg-gating the read.
        let _ = theme_name;
    });

    // -----------------------------------------------------------------------
    // Effect 2 — toggle six classes on `<body>` from UiPrefs + Screen.
    //
    // Per D-17 + RESEARCH Pattern 6 + app.html lines 102–112:
    //   body.no-scanlines  .scanlines  { display: none }
    //   body.no-breadcrumb .breadcrumb { display: none }
    //   body.no-footer     .app-footer { display: none }
    //   body.has-rail.on-chat #wheel-rail { display: flex !important }
    //   body.density-dense .screen { padding-top: 70px; gap: 16px }
    // -----------------------------------------------------------------------
    use_effect(move || {
        let p = prefs.read().clone();
        let screen = *active_screen.read();
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(body) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.body())
            {
                let list = body.class_list();
                let _ = list.toggle_with_force("no-scanlines", !p.scanlines);
                let _ = list.toggle_with_force("no-breadcrumb", !p.breadcrumb);
                let _ = list.toggle_with_force("no-footer", !p.footer);
                let _ = list.toggle_with_force("has-rail", p.rail);
                let _ = list.toggle_with_force(
                    "density-dense",
                    p.density == crate::ui_prefs::Density::Dense,
                );
                let _ = list
                    .toggle_with_force("on-chat", screen == crate::state::Screen::Chat);
            }
        }
        // Silence "unused on non-WASM" — the borrows are already dropped.
        let _ = p;
        let _ = screen;
    });

    // -----------------------------------------------------------------------
    // Effect 3 — write `--teal` / `--teal-bright` CSS custom properties on
    // `<html>` so accent-color rotation is a single DOM op.
    //
    // The hex pair is sourced from a closed `AccentColor` enum match (Plan
    // 02's `hex_pair()`) — no user-supplied string reaches `set_property`
    // (T-26.2.1-16 disposition: accept).
    // -----------------------------------------------------------------------
    use_effect(move || {
        let accent = prefs.read().accent;
        #[cfg(target_arch = "wasm32")]
        {
            let (teal, teal_bright) = accent.hex_pair();
            if let Some(html) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.document_element())
                .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
            {
                let style = html.style();
                let _ = style.set_property("--teal", teal);
                let _ = style.set_property("--teal-bright", teal_bright);
            }
        }
        // Silence "unused on non-WASM" — the borrow was dropped already.
        let _ = accent;
    });

    rsx! {}
}
