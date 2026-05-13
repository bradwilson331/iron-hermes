//! Phase 26.2.1 — HermesApp root composer.
//!
//! This is the root component of the wheel-driven shell. It owns the four
//! root signals (`UiPrefs`, theme string, `WheelState`, active `Screen`),
//! provides them via `use_context_provider`, runs the localStorage
//! hydration gate exactly once on mount, and then keeps localStorage in
//! sync via three persistence effects (RESEARCH Pattern 5).
//!
//! Plan 04 (this commit) mounts the wheel SVG + wheel-rail; Plan 05 will
//! insert the tweaks-panel + theme-effects at the remaining marker comment
//! inside the rsx! body so wave-2 work can land cleanly without re-discovery.

use crate::state::ThemeContext;
use crate::state::{Screen, WheelState};
use crate::ui_prefs::{self, UiPrefs};
use dioxus::prelude::*;

pub mod app_footer;
pub mod breadcrumb;
pub mod hud_chrome;
pub mod screen_router;
pub mod screens;
pub mod sys_meta;
pub mod theme_effects;
pub mod wheel;
pub mod wheel_rail;

/// Root component of the Phase 26.2.1 wheel-driven shell.
///
/// Owns the four root signals and gates persistence through a one-shot
/// hydration effect — defaults never clobber stored values (RESEARCH
/// Pitfall 5 / Pattern 5).
#[component]
pub fn HermesApp() -> Element {
    let mut prefs = use_signal(UiPrefs::default);
    let mut theme = use_signal(|| "slate-dark".to_string());
    let mut wheel_state = use_signal(WheelState::default);
    let active_screen = use_signal(|| Screen::Chat);
    let mut hydrated = use_signal(|| false);

    // Context providers — four root signals exposed to descendants.
    //
    // The theme signal is wrapped in `ThemeContext(theme)` (B-03 newtype,
    // D-26) so it does not type-collide with Plan 06's `SessionIdContext`
    // (also a `Signal<String>`). The other three signals have unique
    // types in the tree and are provided as bare `Signal<T>`.
    use_context_provider(|| prefs);
    use_context_provider(|| crate::state::ThemeContext(theme));
    use_context_provider(|| wheel_state);
    use_context_provider(|| active_screen);

    // Suppress unused-variable warnings on the wrapper read path — the
    // ThemeContext newtype constructor uses the `theme` signal by move,
    // and Plan 05's theme-effects consumer reads it via context.
    let _ = ThemeContext;

    // Hydration gate: read localStorage exactly once. On non-WASM hosts
    // (server / unit-test builds) the read helpers return `None`, so
    // `hydrated` simply flips to `true` without overwriting any defaults.
    use_effect(move || {
        if *hydrated.read() {
            return;
        }
        if let Some(p) = ui_prefs::read_json::<UiPrefs>(ui_prefs::KEY_TWEAKS) {
            prefs.set(p);
        }
        if let Some(t) = ui_prefs::read_string(ui_prefs::KEY_THEME) {
            theme.set(t);
        }
        if let Some(ws) = ui_prefs::read_json::<WheelState>(ui_prefs::KEY_WHEEL) {
            wheel_state.set(ws);
        }
        hydrated.set(true);
    });

    // Persist-on-change effects — gated on `hydrated` so the initial
    // `UiPrefs::default()` never overwrites a stored blob.
    //
    // Signal-borrow safety (clippy.toml): read into a local, drop the
    // borrow at the `;`, then call the side-effecting write helper.
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let p = prefs.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_TWEAKS, &p);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let t = theme.read().clone();
        ui_prefs::write_string(ui_prefs::KEY_THEME, &t);
    });
    use_effect(move || {
        if !*hydrated.read() {
            return;
        }
        let ws = wheel_state.read().clone();
        ui_prefs::write_json(ui_prefs::KEY_WHEEL, &ws);
    });

    rsx! {
        hud_chrome::HudChrome {}
        breadcrumb::Breadcrumb {}
        sys_meta::SysMeta {}
        div { class: "app", id: "app",
            screen_router::ScreenRouter {}
        }
        app_footer::AppFooter {}
        wheel_rail::WheelRail {}
        wheel::Wheel {}

        // Plan 05 inserts: tweaks_panel::TweaksPanel {}
    }
}
