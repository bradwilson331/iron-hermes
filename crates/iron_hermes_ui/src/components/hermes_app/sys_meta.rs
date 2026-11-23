//! Row 1 of the persistent header — the six-segment status strip
//! `MODULE | IH BUILD | UPTIME | PROVIDER | MODEL | TOK` (Phase 54.4 Plan 01,
//! D-07).
//!
//! History: Phase 46.9 Plan 03 (D-07, D-08, D-09) wired the old top-right
//! readout to real values; Phase 54.4 re-lays it out as the header's status
//! strip and drops the `OP READY` segment (not in the D-07 list). Sources:
//!   - MODULE: `breadcrumb::screen_short_name` of the active `Screen`
//!     (never empty).
//!   - IH BUILD: `env!("CARGO_PKG_VERSION")` (no git short-hash stamp
//!     mechanism exists in this workspace; process-wide).
//!   - UPTIME: real server uptime from `mod.rs`'s `get_config_summary`
//!     fetch, ticked at 1Hz client-side (Phase 46.9 Plan 10 GAP-2).
//!     `None` until the first summary resolves; renders `--:--:--`
//!     (UI-SPEC E1-loading).
//!   - PROVIDER + MODEL: the active provider and model id from the SAME
//!     fetch (D-09 single source). Before it resolves `mod.rs` passes an
//!     em-dash. The model keeps a fixed max-width + ellipsis (`.model-id`)
//!     so a long id cannot push TOK off-screen (UI-SPEC E1-long-text).
//!   - TOK: consumes the `tokens` `Signal<(u32, u32)>` context provided at
//!     the `mod.rs` root (already clamped to `[0, denominator]` at the write
//!     site — T-46.9-09). `show_tokens == false` (summary unresolved; plan 05
//!     also clears it for a non-live focused agent) renders an em-dash and
//!     no provenance marker.
//!
//! Phase 50.5 (D-12/D-13): the `TOK` segment's `{limit}` number gains a `~`
//! provenance marker when the window came from the global pin or the
//! 128,000 default — the two provenances that are NOT genuinely about the
//! selected model. `context_source: None` (the `ConfigSummary` resource
//! still resolving, or a fetch error) renders no marker either — a marker
//! that appeared before provenance was known would assert a pin/default
//! that may not be there.
//!
//! The outer `div.sys-meta` wrapper class is kept so existing selectors keep
//! matching. The strip ticks every second, so it deliberately has no live
//! region.

use dioxus::prelude::*;

use crate::components::hermes_app::breadcrumb::screen_short_name;
use crate::server::api::ContextWindowSource;

/// `HH:MM:SS` for a known uptime, `--:--:--` until the first fetch.
pub(crate) fn format_uptime(secs: Option<u64>) -> String {
    match secs {
        None => "--:--:--".to_string(),
        Some(secs) => {
            let h = secs / 3600;
            let m = (secs % 3600) / 60;
            let s = secs % 60;
            format!("{h:02}:{m:02}:{s:02}")
        }
    }
}

/// The TOK value text: an em-dash while `show` is false, otherwise
/// `{used} / {limit}` with `used` clamped to `limit`.
pub(crate) fn format_tok_value(show: bool, used: u32, limit: u32) -> String {
    if show {
        format!("{} / {limit}", used.min(limit))
    } else {
        "\u{2014}".to_string()
    }
}

/// `title` for the PROVIDER / MODEL values: the header-state fetch error
/// (UIC E1-error) when there is one, otherwise the value itself so a
/// truncated id stays readable on hover.
pub(crate) fn segment_title(value: &str, error_title: Option<&str>) -> String {
    error_title.unwrap_or(value).to_string()
}

#[component]
pub fn SysMeta(
    model: String,
    provider: String,
    uptime_secs: Option<u64>,
    context_source: Option<ContextWindowSource>,
    show_tokens: bool,
    error_title: Option<String>,
) -> Element {
    // Phase 26.7.1 Plan 01 tokens context (provided at the mod.rs root) —
    // already clamped to [0, denominator] at every write site (T-46.9-09).
    // Defensive re-clamp here costs nothing and guards any future write
    // site that forgets the clamp.
    let tokens = use_context::<Signal<(u32, u32)>>();
    let (used, limit) = *tokens.read();
    let used_display = used.min(limit);
    let tok_value = format_tok_value(show_tokens, used, limit);

    // Package version baked in at compile time — the accepted D-07 "build"
    // value in the absence of a git short-hash stamp mechanism.
    let build_version = env!("CARGO_PKG_VERSION");
    let uptime_display = format_uptime(uptime_secs);
    let provider_title = segment_title(&provider, error_title.as_deref());
    let model_title = segment_title(&model, error_title.as_deref());

    // Phase 50.5 (D-12): flag ONLY the two provenances that are not the
    // model's own — the global pin and the 128,000 default. `None` covers
    // both the resource-unresolved and resource-error cases; neither ever
    // renders a marker (UI-SPEC E1 loading/error).
    let marker_title = match context_source {
        Some(ContextWindowSource::GlobalPin) => Some(format!(
            "This window comes from your model.context_length setting, not \
             {model}'s own catalog value — click to review per-model overrides."
        )),
        Some(ContextWindowSource::Fallback) => Some(format!(
            "{model} has no known context window — showing the 128,000-token \
             default. Click to set the correct value."
        )),
        Some(ContextWindowSource::LocalDefault) => Some(format!(
            "{model} has no known context window — assuming 8,192 tokens because it \
             runs on a local server. Click to set the correct value."
        )),
        // Probed/Learned are the model's real window (Phase 58.3.1): no marker.
        Some(ContextWindowSource::PerModelConfig)
        | Some(ContextWindowSource::Metadata)
        | Some(ContextWindowSource::Probed)
        | Some(ContextWindowSource::Learned)
        | None => None,
    };

    // Phase 50.5 (D-13): the marker is the entry point to fixing the number,
    // not merely a warning about it — clicking it navigates to Models.
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    // Owned local BEFORE rsx! so no signal borrow is held across the tree.
    let module_name = screen_short_name(*active_screen.read());

    rsx! {
        div { class: "sys-meta",
            span { class: "ih-seg ih-seg-module",
                span { class: "ih-seg-label", "MODULE" }
                span { class: "v", "{module_name}" }
            }
            span { class: "sep", "aria-hidden": "true" }
            span { class: "ih-seg ih-seg-build",
                span { class: "ih-seg-label", "IH BUILD" }
                span { class: "v", "{build_version}" }
            }
            span { class: "sep", "aria-hidden": "true" }
            span { class: "ih-seg ih-seg-uptime",
                span { class: "ih-seg-label", "UPTIME" }
                span { class: "v", "{uptime_display}" }
            }
            span { class: "sep", "aria-hidden": "true" }
            span { class: "ih-seg ih-seg-provider",
                span { class: "ih-seg-label", "PROVIDER" }
                span { class: "v", title: "{provider_title}", "{provider}" }
            }
            span { class: "sep", "aria-hidden": "true" }
            span { class: "ih-seg ih-seg-model",
                span { class: "ih-seg-label", "MODEL" }
                span { class: "v model-id", title: "{model_title}", "{model}" }
            }
            span { class: "ih-seg ih-seg-tok",
                span { class: "ih-seg-label", "TOK" }
                if !show_tokens {
                    span { class: "v", "{tok_value}" }
                } else if let Some(t) = marker_title {
                    span { class: "v", "{used_display} / " }
                    button {
                        class: "tok-marker",
                        r#type: "button",
                        title: "{t}",
                        aria_label: "{t}",
                        onclick: move |_| active_screen.set(crate::state::Screen::Models),
                        "~"
                        span { class: "v", "{limit}" }
                    }
                } else {
                    span { class: "v", "{tok_value}" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYS_META_RS: &str = include_str!("sys_meta.rs");

    #[test]
    fn sys_meta_format_uptime_none_is_dashes() {
        assert_eq!(format_uptime(None), "--:--:--");
    }

    #[test]
    fn sys_meta_format_uptime_some_is_hms() {
        assert_eq!(format_uptime(Some(3723)), "01:02:03");
        assert_eq!(format_uptime(Some(0)), "00:00:00");
    }

    #[test]
    fn sys_meta_strip_has_six_ordered_segments() {
        // Search the component code only, never this test module, whose own
        // array would otherwise satisfy the check.
        let code_end = SYS_META_RS.find("#[cfg(test)]").unwrap_or(SYS_META_RS.len());
        let code = &SYS_META_RS[..code_end];
        let mut last = 0;
        for class in [
            "ih-seg-module",
            "ih-seg-build",
            "ih-seg-uptime",
            "ih-seg-provider",
            "ih-seg-model",
            "ih-seg-tok",
        ] {
            let at = code
                .find(class)
                .unwrap_or_else(|| panic!("segment class `{class}` missing"));
            assert!(at >= last, "segment `{class}` is out of order");
            last = at;
        }
    }

    #[test]
    fn sys_meta_module_segment_uses_screen_short_name() {
        let code_end = SYS_META_RS.find("#[cfg(test)]").unwrap_or(SYS_META_RS.len());
        assert!(SYS_META_RS[..code_end].contains("screen_short_name("));
    }

    #[test]
    fn sys_meta_segment_title_prefers_the_fetch_error() {
        assert_eq!(segment_title("claude", None), "claude");
        assert_eq!(
            segment_title("\u{2014}", Some("Couldn't load dev's config.")),
            "Couldn't load dev's config."
        );
    }

    #[test]
    fn sys_meta_tok_is_dash_until_the_summary_resolves() {
        assert_eq!(format_tok_value(false, 5, 10), "\u{2014}");
        assert_eq!(format_tok_value(true, 5, 10), "5 / 10");
        assert_eq!(format_tok_value(true, 12, 10), "10 / 10");
    }
}
