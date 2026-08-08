//! Bottom HUD strip — `.app-footer` with a timezone-aware live clock.
//!
//! Renders the persistent footer from `app.html` lines 1525-1542. The
//! rightmost SCREEN label tracks the context-provided `Signal<Screen>`,
//! and the clock value updates once per second via a `use_future` loop
//! that awaits `gloo_timers::future::TimeoutFuture::new(1000)` (RESEARCH
//! Example C).
//!
//! Phase 46.9 Plan 11 (GAP-4/GAP-5): the clock used to hardcode
//! `js_sys::Date::get_utc_*` (always UTC, regardless of any configured
//! `agent.timezone`/`display.timezone`). It now fetches
//! `get_display_timezones()` (`server/display_tz_api.rs`) once via
//! `use_resource` + a `loaded` latch (the `voice_settings.rs`/
//! `kanban_config.rs` template — NOT `use_server_future(...)?`, which would
//! early-return before this component's `use_future` ticker hook ever
//! registers on a loading render; see agents.rs UAT-2 for the same trap) and
//! formats the PRIMARY clock in the resolved zone (`display.timezone` if
//! set, else `agent.timezone`, else the browser's host-local zone — GAP-4),
//! plus up to 4 EXTRA labeled clocks from `DisplayTimezones.extras` (GAP-5).
//!
//! On non-WASM builds the clock is a static `"00:00:00"` literal per zone —
//! the timer-driven Intl formatting is wasm-only since `js_sys::Date` /
//! `Intl.DateTimeFormat` are browser-only. Server / unit-test builds compile
//! cleanly without introducing chrono as a new dependency.

use crate::server::display_tz_api::{get_display_timezones, DisplayTimezones};
use crate::state::Screen;
use dioxus::prelude::*;

#[component]
pub fn AppFooter() -> Element {
    let active_screen = use_context::<Signal<Screen>>();
    // Phase 36.17.4 (D-03a): queue pill state provided by HermesApp.
    let queue_state = use_context::<Signal<(u32, bool)>>();

    // Phase 46.9 Plan 11 (GAP-4/GAP-5): resolve the primary + extra display
    // timezones once via `use_resource` + a `loaded` latch (Pitfall 3 loop
    // avoidance — never re-seed after the first successful resolve; the
    // config value cannot change out from under an open tab, config never
    // hot-reloads). `use_resource` (not `use_server_future(...)?`) so this
    // hook — and every hook declared below it — registers unconditionally on
    // every render, including the loading render (hook-order-safe).
    let tz_resource = use_resource(move || async move { get_display_timezones().await });
    let mut resolved_tz = use_signal(DisplayTimezones::default);
    let mut tz_loaded = use_signal(|| false);
    if !*tz_loaded.read() {
        if let Some(Ok(dt)) = tz_resource.read().as_ref() {
            resolved_tz.set(dt.clone());
            tz_loaded.set(true);
        }
    }

    let mut clock = use_signal(|| format_clock(None, false));
    let mut extra_clocks = use_signal(Vec::<(String, String)>::new);

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
            // `.peek()` (not `.read()`): this runs inside an effect/loop, not
            // a render path, so a plain read is correct here (Dioxus
            // peek-vs-read reactivity — `.read()` in this position would
            // subscribe the ticker future to `resolved_tz`, which is neither
            // necessary nor desired).
            let dt = resolved_tz.peek().clone();
            let hour12 = dt.hour12;
            clock.set(format_clock(dt.primary.as_deref(), hour12));
            let extras: Vec<(String, String)> = dt
                .extras
                .iter()
                .take(4)
                .map(|zone| (zone_short_label(zone), format_clock(Some(zone), hour12)))
                .collect();
            extra_clocks.set(extras);
        }
    });

    let label = screen_label(*active_screen.read());
    let clock_str = clock.read().clone();
    let extra_clocks_snapshot = extra_clocks.read().clone();
    // Phase 36.17.4: read into Copy locals before RSX (clippy.toml: signal borrow must not span RSX).
    let (queue_depth, queue_paused) = *queue_state.read();

    rsx! {
        div { class: "app-footer",
            span { "NODE " span { class: "v", "HERMES-7" } }
            span { class: "sep" }
            span { "SCREEN " span { id: "ft-screen", class: "v", "{label}" } }
            span { class: "sep" }
            span { "AGENT " span { class: "v", "DEFAULT" } }
            // Phase 36.17.4 (D-03a / D-03b): queue pill — hide-when-zero. Placed in
            // the left run after AGENT for visual parity with the TUI status_line.rs.
            if queue_depth > 0 {
                span { class: "sep" }
                span {
                    "QUEUE "
                    if queue_paused {
                        span { class: "v", "{queue_depth} (paused)" }
                    } else {
                        span { class: "v", "{queue_depth}" }
                    }
                }
            }
            div { class: "app-footer-right",
                span { "MEM " span { class: "v", "412 / ∞" } }
                span { class: "sep" }
                span { "SKILLS " span { class: "v", "57 / 142" } }
                span { class: "sep" }
                span { "P50 " span { class: "v", "218 MS" } }
                // Phase 46.9 Plan 11 (GAP-5): up to 4 extra labeled zone clocks,
                // rendered before the primary clock (same .sep-separated
                // vocabulary as the existing MEM/SKILLS/P50 pills).
                for (zone_label , zone_clock) in extra_clocks_snapshot {
                    span { class: "sep" }
                    span { "{zone_label} " span { class: "v", "{zone_clock}" } }
                }
                span { class: "sep" }
                span { id: "ft-clock", class: "v", "{clock_str}" }
            }
        }
    }
}

/// Short-name map for the footer's SCREEN field. Matches the breadcrumb
/// short-name vocabulary from the plan's `<interfaces>` table.
// Called by AppFooter, which is only mounted via HermesApp. In a binary crate
// `pub fn` does not suppress dead_code, so when `--all-features` enables
// `legacy-shell` (app.rs mounts WarpHermes instead of HermesApp) the HermesApp
// subtree is unreferenced and this helper reads as dead. Live in the default
// (non-legacy-shell) build.
#[allow(dead_code)]
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
        // Phase 36.3.7.11 D-02: Kanban wheel-nav + dashboard short-name.
        Screen::Kanban => "KANBAN",
        // Phase 46.6 Plan 05 (D-07): Artifacts gallery + sandboxed viewer.
        Screen::Artifacts => "ARTIFACTS",
        Screen::ArtifactViewer => "ARTIFACT",
    }
}

/// Phase 46.9 Plan 11 (GAP-5): short label for an extra footer clock —
/// the IANA zone's final segment with underscores replaced by spaces
/// (e.g. `"America/New_York"` -> `"New York"`, `"Europe/London"` -> `"London"`).
/// Pure string logic, unconditional (no wasm/native split needed).
// Called only by AppFooter's wasm ticker branch; reads as dead under the
// `--all-features` legacy-shell build, same rationale as `screen_label` above.
#[allow(dead_code)]
fn zone_short_label(iana: &str) -> String {
    iana.rsplit('/').next().unwrap_or(iana).replace('_', " ")
}

// Phase 46.9 Plan 11 (GAP-4): a JS-namespace-scoped `Intl.DateTimeFormat`
// binding with `catch` so a construction failure (e.g. an unknown/invalid
// IANA zone name in `config.display.timezone`/`extra_timezones`) surfaces
// as a Rust `Err` instead of an uncaught JS exception crashing the wasm
// module (T-46.9-28). `Intl.DateTimeFormat` is spec'd to behave identically
// whether called with or without `new`, so a plain function-call binding is
// sufficient — no `constructor` attribute needed.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(catch, js_namespace = Intl, js_name = DateTimeFormat)]
    fn try_construct_date_time_format(
        locale: &str,
        options: &wasm_bindgen::JsValue,
    ) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

/// Build a fixed HH:MM:SS + short zone-name `Intl.DateTimeFormat` options
/// object, optionally pinned to `zone`. `None` leaves `timeZone` unset,
/// which resolves to the browser's host-local zone. `hour12` is the
/// caller-supplied QDG-01 clock format (`false` = 24-hour, `true` = 12-hour
/// AM/PM), resolved server-side from `config.display.hour12`.
#[cfg(target_arch = "wasm32")]
fn build_clock_options(zone: Option<&str>, hour12: bool) -> js_sys::Intl::DateTimeFormatOptions {
    use js_sys::Intl::{DateTimeFormatOptions, NumericFormat, TimeZoneNameFormat};

    let opts = DateTimeFormatOptions::new();
    opts.set_hour12(hour12);
    opts.set_hour(NumericFormat::TwoDigit);
    opts.set_minute(NumericFormat::TwoDigit);
    opts.set_second(NumericFormat::TwoDigit);
    opts.set_time_zone_name(TimeZoneNameFormat::Short);
    if let Some(z) = zone {
        opts.set_time_zone(z);
    }
    opts
}

// Called only by AppFooter (HermesApp subtree); dead under `--all-features` when
// `legacy-shell` swaps the root to WarpHermes (see screen_label note). Live in the
// default build.
#[allow(dead_code)]
#[cfg(target_arch = "wasm32")]
fn format_clock(zone: Option<&str>, hour12: bool) -> String {
    let date = js_sys::Date::new_0();

    if let Some(z) = zone {
        let candidate = build_clock_options(Some(z), hour12);
        // Guard (T-46.9-28): validate the zone constructs cleanly before
        // formatting with it; an unknown/invalid IANA name falls back to
        // host-local formatting instead of panicking (mirrors
        // prompt_builder.rs's tolerant IANA resolution).
        if try_construct_date_time_format("en-US", candidate.as_ref()).is_ok() {
            return date
                .to_locale_time_string_with_options("en-US", candidate.as_ref())
                .into();
        }
    }

    let host_options = build_clock_options(None, hour12);
    date.to_locale_time_string_with_options("en-US", host_options.as_ref())
        .into()
}

// Host stub for non-wasm builds; same dead-under-legacy-shell rationale as the
// wasm arm above. Live in the default (non-legacy-shell) host build. The
// timer-driven Intl formatting is wasm-only (js_sys::Date is browser-only),
// so the zone argument is intentionally unused here — matches the pre-46.9-11
// `now_time_utc` native stub.
#[allow(dead_code)]
#[cfg(not(target_arch = "wasm32"))]
fn format_clock(_zone: Option<&str>, _hour12: bool) -> String {
    "00:00:00".to_string()
}
