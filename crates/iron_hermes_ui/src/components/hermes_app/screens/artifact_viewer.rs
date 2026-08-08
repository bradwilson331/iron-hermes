//! Phase 46.6 Plan 05 (D-02 / D-07) — sandboxed artifact viewer chrome.
//!
//! The D-02 client boundary: embeds the artifact via an opaque-origin
//! `<iframe src="/artifacts/{id}" sandbox="allow-scripts allow-forms allow-modals">` (server-set CSP
//! comes from Plan 03's `serve_artifact` route). The same-origin sandbox
//! allowance is NEVER granted alongside `allow-scripts` — per RESEARCH
//! Pitfall 1 / MDN, that combination lets the framed content call
//! `document.documentElement.removeAttribute('sandbox')` and fully escape
//! same-origin isolation. This module never passes operator session data,
//! tokens, or a bidirectional `postMessage` bridge into the iframe — it is
//! strictly one-way display (UI-SPEC §3).
//!
//! Reads the selected artifact from `SelectedArtifactCtx` (Plan 05 Task 1,
//! `hermes_app/mod.rs`), written by the gallery (`artifacts.rs`) on row
//! click.

use dioxus::prelude::*;

/// The D-02 client-boundary sandbox token list. Grants exactly three
/// allowances so artifacts are genuinely interactive without escaping
/// isolation: `allow-scripts` (JS runs), `allow-forms` (a `<form>`'s submit
/// event fires — CSP `form-action 'none'` still blocks any navigation/exfil),
/// and `allow-modals` (`alert`/`confirm`/`prompt` work; without it the JS
/// engine still runs but modal calls are silently dropped — the Phase 46.6
/// UAT "buttons do nothing" symptom).
///
/// NEVER add `allow-same-origin`: combined with `allow-scripts` it lets the
/// framed content call `document.documentElement.removeAttribute('sandbox')`
/// and fully escape (RESEARCH Pitfall 1 / MDN). Omitting it is the opaque-origin
/// guarantee — the artifact is treated as a unique origin that always fails the
/// same-origin policy, isolating it from the operator's `iron_hermes_ui`
/// session/cookies/DOM. Network is independently blocked by the server-set CSP
/// (`default-src 'none'; connect-src 'none'`). The `sandbox_never_grants_same_origin`
/// test enforces the invariant.
//
// Used only inside ArtifactViewer's rsx! iframe attribute below. Under
// `--all-features`, `legacy-shell` swaps the root to WarpHermes, leaving
// HermesApp (and therefore ArtifactViewer) unreferenced — a binary crate's
// unreferenced `pub fn`/`const` does not suppress dead_code, so the
// `-D warnings` clippy gate fires there even though this is live in the
// default (non-legacy-shell) build. Same pattern as `KANBAN_CSS` in
// `screens/kanban.rs` and `dispatch_slash` in `hermes_app/mod.rs`.
#[allow(dead_code)]
const ARTIFACT_SANDBOX: &str = "allow-scripts allow-forms allow-modals";

/// Phase 26.7.2 (D-05) relative-timestamp helper, duplicated here (private,
/// mirrors `sessions.rs` / `artifacts.rs` — not `pub` in either, so each
/// screen carries its own copy).
#[cfg(target_arch = "wasm32")]
fn format_relative(unix_secs_str: &str) -> String {
    let unix_secs: f64 = unix_secs_str.parse().unwrap_or(0.0);
    let now_ms = js_sys::Date::now();
    let then_ms = unix_secs * 1000.0; // seconds → milliseconds
    let diff_secs = ((now_ms - then_ms) / 1000.0) as i64;
    match diff_secs {
        s if s < 5 => "just now".to_string(),
        s if s < 60 => format!("{}s ago", s),
        s if s < 3_600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3_600),
        s if s < 604_800 => format!("{}d ago", s / 86_400),
        _ => format!("{}w ago", diff_secs / 604_800),
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // stub for non-wasm builds; wasm32 caller is gated behind #[cfg(target_arch = "wasm32")]
fn format_relative(_: &str) -> String {
    "\u{2014}".to_string()
}

/// Sandboxed artifact viewer screen. Always mounted (RESEARCH Pattern 7);
/// `is_active` drives the `.is-active` CSS class, mirroring every other
/// screen. Reads the selected artifact from context rather than fetching —
/// the gallery already has the full `ArtifactInfo` row and passes it along.
#[component]
pub fn ArtifactViewer(is_active: bool) -> Element {
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let selected_artifact = use_context::<crate::state::SelectedArtifactCtx>().0;
    // No-flash-on-load overlay: shown until the iframe's own `onload` fires.
    // This does NOT read the iframe's contents (no bridge into the opaque
    // origin) — it only observes the browser-native iframe load event, the
    // same signal a plain `<img onload>` would give, and carries no data
    // back from the sandboxed content (UI-SPEC §3 one-way display).
    let mut iframe_loaded = use_signal(|| false);

    let current = selected_artifact.read().clone();

    let back_to_gallery = move |_| {
        active_screen.set(crate::state::Screen::Artifacts);
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-artifact-viewer",
            "data-screen-label": "16 Artifact Viewer",

            {
                match &current {
                    // Nothing selected (e.g. reached before a gallery row was
                    // clicked, or the id came back empty/malformed) — treat as
                    // the not-selected/failed path per UI-SPEC's viewer
                    // load-error copy, never a blank iframe.
                    None => rsx! {
                        div { class: "panel",
                            div { class: "panel-title", "Couldn't load this artifact." }
                            p { class: "panel-sub",
                                "The content may have been removed, or blocked by the sandbox. Go back and reopen it from the gallery."
                            }
                            button {
                                class: "btn btn--ghost btn--sm",
                                onclick: back_to_gallery,
                                "‹ BACK TO GALLERY"
                            }
                        }
                    },
                    Some(artifact) if artifact.id.trim().is_empty() => rsx! {
                        div { class: "panel",
                            div { class: "panel-title", "Couldn't load this artifact." }
                            p { class: "panel-sub",
                                "The content may have been removed, or blocked by the sandbox. Go back and reopen it from the gallery."
                            }
                            button {
                                class: "btn btn--ghost btn--sm",
                                onclick: back_to_gallery,
                                "‹ BACK TO GALLERY"
                            }
                        }
                    },
                    Some(artifact) => {
                        let icon = artifact.icon.clone().unwrap_or_else(|| "▤".to_string());
                        let title = artifact.title.clone();
                        let ts = format_relative(&artifact.updated_at);
                        let source_label = artifact.source_kind.clone().map(|s| s.to_uppercase());
                        let id = artifact.id.clone();
                        let loaded = *iframe_loaded.read();
                        rsx! {
                            div { class: "screen-header",
                                div {
                                    class: "screen-header-left",
                                    style: "flex-direction: row; align-items: center; gap: 10px;",
                                    button {
                                        class: "btn btn--ghost btn--sm",
                                        onclick: back_to_gallery,
                                        "‹ ARTIFACTS"
                                    }
                                    span { "{icon}" }
                                    span { class: "row-title", "{title}" }
                                    if let Some(label) = &source_label {
                                        span { class: "pill", "{label}" }
                                    }
                                    span { class: "row-sub", style: "opacity:0.5", "{ts}" }
                                }
                                div { class: "screen-actions",
                                    button {
                                        class: "btn btn--icon btn--ghost",
                                        "aria-label": "Close artifact viewer",
                                        title: "Close",
                                        onclick: back_to_gallery,
                                        "×"
                                    }
                                }
                            }
                            div {
                                class: "panel",
                                style: "flex: 1; position: relative; padding: 0; overflow: hidden; background: var(--bg-deep); border-radius: 12px;",
                                if !loaded {
                                    span {
                                        class: "row-sub",
                                        style: "position: absolute; top: 50%; left: 50%; transform: translate(-50%, -50%);",
                                        "Loading artifact…"
                                    }
                                }
                                // D-02 client boundary: opaque-origin sandbox. NEVER grant
                                // the same-origin sandbox allowance alongside allow-scripts
                                // (RESEARCH Pitfall 1) — no operator session/token/query-credential
                                // is interpolated into `src`, and no postMessage bridge
                                // is wired to this iframe (UI-SPEC §3 one-way display).
                                // `sandbox` is quoted here (Dioxus "custom attribute"
                                // syntax, matching `"aria-label"` / `"data-screen-label"`
                                // elsewhere in this file/crate) because dioxus-html
                                // 0.7.7's `iframe` element definition comments out the
                                // typed `sandbox` attribute (`// sandbox:
                                // SpacedSet<Sandbox>,` in dioxus-html/src/elements.rs) —
                                // the bare-identifier form does not compile against the
                                // pinned version. The rendered DOM attribute is
                                // unaffected: `sandbox="allow-scripts"` either way. Value
                                // is still driven exclusively by `ARTIFACT_SANDBOX`.
                                iframe {
                                    src: "/artifacts/{id}",
                                    "sandbox": ARTIFACT_SANDBOX,
                                    style: "width: 100%; height: 100%; border: none;",
                                    onload: move |_| iframe_loaded.set(true),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The load-bearing D-02 invariant: `allow-same-origin` combined with
    /// `allow-scripts` is a full sandbox escape (RESEARCH Pitfall 1). Interactive
    /// tokens (forms/modals) are fine — same-origin is never permitted.
    #[test]
    fn sandbox_never_grants_same_origin() {
        assert!(
            !ARTIFACT_SANDBOX.contains("allow-same-origin"),
            "ARTIFACT_SANDBOX must never grant allow-same-origin (sandbox escape)"
        );
        assert!(
            ARTIFACT_SANDBOX.contains("allow-scripts"),
            "artifacts must still be able to run JS"
        );
    }
}
