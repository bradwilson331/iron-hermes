//! Bottom HUD strip — `.app-footer` with live UTC clock.
//!
//! Renders the persistent footer from `app.html` lines 1525-1542. The
//! rightmost SCREEN label tracks the context-provided `Signal<Screen>`,
//! and the clock value updates once per second via a `use_future` loop
//! that awaits `gloo_timers::future::TimeoutFuture::new(1000)` (RESEARCH
//! Example C).
//!
//! On non-WASM builds the clock is a static `"00:00:00 UTC"` literal —
//! the timer-driven update is wasm-only since `js_sys::Date` is
//! browser-only. Server / unit-test builds compile cleanly without
//! introducing chrono as a new dependency.

use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn AppFooter() -> Element {
    let active_screen = use_context::<Signal<Screen>>();
    let mut clock = use_signal(now_time_utc);

    // 1Hz ticker — `use_future` runs forever bound to this component's
    // lifetime. HermesApp never unmounts, so the timer never leaks.
    // The await sits at the loop boundary, not across any signal-borrow,
    // so the signal-borrow safety rule (clippy.toml) is respected.
    use_future(move || async move {
        loop {
            #[cfg(target_arch = "wasm32")]
            {
                gloo_timers::future::TimeoutFuture::new(1000).await;
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                tokio::time::sleep(std::time::Duration::from_millis(1000)).await;
            }
            clock.set(now_time_utc());
        }
    });

    let label = screen_label(*active_screen.read());
    let clock_str = clock.read().clone();

    rsx! {
        div { class: "app-footer",
            span { "NODE " span { class: "v", "HERMES-7" } }
            span { class: "sep" }
            span { "SCREEN " span { id: "ft-screen", class: "v", "{label}" } }
            span { class: "sep" }
            span { "AGENT " span { class: "v", "DEFAULT" } }
            div { class: "app-footer-right",
                span { "MEM " span { class: "v", "412 / ∞" } }
                span { class: "sep" }
                span { "SKILLS " span { class: "v", "57 / 142" } }
                span { class: "sep" }
                span { "P50 " span { class: "v", "218 MS" } }
                span { class: "sep" }
                span { id: "ft-clock", class: "v", "{clock_str}" }
            }
        }
    }
}

/// Short-name map for the footer's SCREEN field. Matches the breadcrumb
/// short-name vocabulary from the plan's `<interfaces>` table.
fn screen_label(s: Screen) -> &'static str {
    match s {
        Screen::Chat => "CHAT",
        Screen::Sessions => "SESSIONS",
        Screen::Agents => "AGENTS",
        Screen::Skills => "SKILLS",
        Screen::Models => "MODELS",
        Screen::Memory => "MEMORY",
        Screen::Soul => "SOUL",
        Screen::Tools => "TOOLS",
        Screen::Schedules => "SCHED",
        Screen::Gateway => "GATEWAY",
        Screen::Office => "OFFICE",
        Screen::Settings => "SYSTEM",
        Screen::Providers => "PROVIDERS",
    }
}

#[cfg(target_arch = "wasm32")]
fn now_time_utc() -> String {
    let d = js_sys::Date::new_0();
    // `js_sys::Date::get_utc_*` already returns u32 — no cast needed.
    format!(
        "{:02}:{:02}:{:02} UTC",
        d.get_utc_hours(),
        d.get_utc_minutes(),
        d.get_utc_seconds(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
fn now_time_utc() -> String {
    "00:00:00 UTC".to_string()
}
