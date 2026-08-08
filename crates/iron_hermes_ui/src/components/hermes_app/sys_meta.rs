//! Top-right `.sys-meta` chrome — BUILD / UPTIME / PROVIDER / MODEL / TOK / OP fields.
//!
//! Phase 46.9 Plan 03 (D-07, D-08, D-09): previously static placeholders
//! (BUILD "1.0.0" / UPTIME "00:00:00", no model, no token gauge). Now wired
//! to real values:
//!   - BUILD: `env!("CARGO_PKG_VERSION")` (no git short-hash stamp mechanism
//!     exists in this workspace — CARGO version is the accepted "build" value
//!     per the plan's flagged assumption; either version or commit satisfies
//!     D-07's "version/commit" wording).
//!   - UPTIME: real server uptime, passed down from `mod.rs`'s
//!     `get_config_summary` fetch (`uptime_secs`, new field — Phase 46.9
//!     Plan 03 addition to `server/api.rs`'s `ConfigSummary`). Phase 46.9
//!     Plan 10 (GAP-2): `mod.rs` now ticks this value forward client-side
//!     at 1Hz, so this prop advances live instead of freezing at the mount
//!     value until reload.
//!   - PROVIDER + MODEL: the active provider and model id, from the SAME
//!     `get_config_summary` fetch (D-09 single source — the identical value
//!     `mod.rs` uses to seed the tokens denominator). Phase 46.9 Plan 10
//!     (GAP-3): provider now renders adjacent to the model segment. The
//!     model segment keeps its fixed max-width + ellipsis (`.model-id` in
//!     `site.css`) so a long model id cannot push `TOK … READY` off-screen.
//!   - TOK: consumes the `tokens` `Signal<(u32, u32)>` context provided at
//!     `mod.rs` root (already clamped to `[0, denominator]` at the write
//!     site — T-46.9-09).
//!   - OP: unchanged (`READY`).
//!
//! Markup mirrors `app.html` lines 348-354 (the `.sys-meta` block); layout
//! and vocabulary are otherwise unchanged (D-plan prohibition — no redesign)
//! — the provider segment reuses the existing separator/segment vocabulary
//! rather than introducing new chrome.

use dioxus::prelude::*;

#[component]
pub fn SysMeta(model: String, provider: String, uptime_secs: u64) -> Element {
    // Phase 26.7.1 Plan 01 tokens context (provided at mod.rs:787-ish) —
    // already clamped to [0, denominator] at every write site (T-46.9-09).
    // Defensive re-clamp here costs nothing and guards any future write
    // site that forgets the clamp.
    let tokens = use_context::<Signal<(u32, u32)>>();
    let (used, limit) = *tokens.read();
    let used_display = used.min(limit);

    // Package version baked in at compile time — the accepted D-07 "build"
    // value in the absence of a git short-hash stamp mechanism.
    let build_version = env!("CARGO_PKG_VERSION");

    // Format uptime_secs as `HH:MM:SS`, matching the original stub's
    // `00:00:00` shape so the layout width is unchanged. Kept as a local
    // binding (not a module-level fn) so `--all-features` dead-code
    // reachability analysis on this `bin` crate ties its liveness directly
    // to this render body, not to a separately-declared item.
    let uptime_display = {
        let h = uptime_secs / 3600;
        let m = (uptime_secs % 3600) / 60;
        let s = uptime_secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    };

    rsx! {
        div { class: "sys-meta",
            span { "BUILD " span { class: "v", "{build_version}" } }
            span { "·" }
            span { "UPTIME " span { class: "v", "{uptime_display}" } }
            span { "·" }
            span { class: "v", title: "{provider}", "{provider}" }
            span { "·" }
            span { class: "v model-id", title: "{model}", "{model}" }
            span { "·" }
            span { "TOK " span { class: "v", "{used_display} / {limit}" } }
            span { "·" }
            span { "OP " span { class: "v", "READY" } }
        }
    }
}
