//! Phase 26.2.1 — ThemeEffects: render-free DOM mutation host.
//!
//! Bundles three `use_effect` blocks that translate the four root signals
//! (`UiPrefs`, `ThemeContext`, `Screen`) into DOM mutations on `<html>` and
//! `<body>` (RESEARCH Pattern 6). The component returns `rsx! {}` — its only
//! purpose is to subscribe to context signals and write to the DOM.
//!
//! Allowed mutations (D-03 / D-24):
//!   1. `<html data-theme="…">` attribute  (theme picker)
//!   2. `<html style="--teal: …; --teal-bright: …">`  (accent override; the
//!      pair is theme-aware — dark inks on dark themes, dark-enough AA inks on
//!      light themes, because the inline write beats every `[data-theme]`
//!      block, G-54.4-18. Values still come from closed enum matches.)
//!   3. `<body class="no-footer density-dense on-chat">` toggles
//!      (Phase 54.4 Plan 02 retired `no-breadcrumb` and `has-rail`; the
//!      density rule now pads by `calc(var(--ih-overhang-h) + 16px)`, per
//!      plan 01)
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

/// True exactly for the two light themes (a closed match, so an unknown or
/// tampered name is never light).
pub(crate) fn is_light_theme(theme: &str) -> bool {
    matches!(theme, "slate-light" | "parchment-light")
}

/// The `(--teal, --teal-bright)` pair Effect 3 writes inline on `<html>`.
///
/// The inline write beats every `[data-theme]` block, so on a light theme the
/// bright dark-theme accent would paint as illegible text. Light themes get the
/// dark AA inks; every other name (including unknown ones) keeps `hex_pair()`.
/// Values are `&'static str` from closed enum matches (T-26.2.1-16, T-54.4-46).
pub(crate) fn accent_inline_pair(
    theme: &str,
    accent: crate::ui_prefs::AccentColor,
) -> (&'static str, &'static str) {
    if is_light_theme(theme) {
        accent.light_hex_pair()
    } else {
        accent.hex_pair()
    }
}

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
    // Effect 2 — toggle three classes on `<body>` from UiPrefs + Screen.
    //
    // Per D-17 + RESEARCH Pattern 6 (Phase 54.4 Plan 02 dropped the
    // `no-breadcrumb` and `has-rail` toggles — their controls and CSS are gone):
    //   body.no-footer     .app-footer { display: none }
    //   body.density-dense .screen { padding-top: calc(var(--ih-overhang-h) + 16px); gap: 16px }
    //   body.on-chat       (chat-screen scoping hook)
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
                let _ = list.toggle_with_force("no-footer", !p.footer);
                let _ = list.toggle_with_force(
                    "density-dense",
                    p.density == crate::ui_prefs::Density::Dense,
                );
                let _ = list.toggle_with_force("on-chat", screen == crate::state::Screen::Chat);
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
    // The pair is theme-aware (G-54.4-18): dark inks on dark themes and
    // dark-enough AA inks on light themes, because this inline write beats
    // every `[data-theme]` block. It reads the theme as well as the accent so a
    // runtime light/dark switch rewrites `--teal` with no reload. The values
    // come from closed enum matches (`accent_inline_pair`) — no user-supplied
    // string reaches `set_property` (T-26.2.1-16 unchanged).
    // -----------------------------------------------------------------------
    use_effect(move || {
        let theme_name = theme.read().clone();
        let accent = prefs.read().accent;
        let (teal, teal_bright) = accent_inline_pair(&theme_name, accent);
        #[cfg(target_arch = "wasm32")]
        {
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
        // Silence "unused on non-WASM" — the borrows were dropped already.
        let _ = (teal, teal_bright);
    });

    rsx! {}
}
