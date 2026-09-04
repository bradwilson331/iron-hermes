//! Phase 46.6 (D-07 / D-04) — Artifacts gallery with management controls.
//!
//! Fetches `list_artifacts(include_archived)` (`server/api.rs`) and renders one
//! row per artifact (`.row-list` / `.row` / `.row-main` / `.row-title` /
//! `.row-sub`). Each row carries a management cluster: **download** (raw HTML),
//! **publish** (a stub for a future Postiz CI/CD pipeline), **archive/unarchive**
//! (soft hide, with a "show archived" toggle), and **delete** (irreversible,
//! two-click confirm). Clicking a row (outside the cluster) writes the artifact
//! into `SelectedArtifactCtx` and switches to `Screen::ArtifactViewer`.

use dioxus::prelude::*;

/// Phase 26.7.2 (D-05) relative-timestamp helper, duplicated here (private,
/// mirrors `sessions.rs`'s `format_relative`). `updated_at` is a unix-seconds
/// string, same shape as `SessionInfo.created_at` (`server/api.rs`).
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

/// Recover the producing session id from a chat artifact's composite
/// `source_ref` (`chat_capture.rs` writes `"<session_id>:<filename>"`, Plan 03
/// D-25 keying).
///
/// Splits on the LAST `:` — web session ids are themselves colon-delimited
/// (`agent:main:web:dm:<uuid>`), so splitting on the first `:` truncates the id
/// to `"agent"` and navigates to a session that does not exist. The filename is
/// the only colon-free component, so it is the reliable anchor.
// Native builds never reach the HermesApp render tree (wasm-only), so the sole
// non-test caller is invisible to native dead-code analysis — same reason the
// `format_relative` stub above carries an allow.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn producing_session_id_from_source_ref(source_ref: &str) -> Option<String> {
    let (session_id, filename) = source_ref.rsplit_once(':')?;
    if session_id.is_empty() || filename.is_empty() {
        return None;
    }
    Some(session_id.to_string())
}

/// Artifacts gallery screen. Always mounted (RESEARCH Pattern 7); `is_active`
/// drives the `.is-active` CSS class, mirroring every other screen.
#[component]
pub fn ScreenArtifacts(is_active: bool) -> Element {
    // "Show archived" toggle — read INSIDE the resource so flipping it re-fetches
    // with the new `include_archived` value (mirrors the kanban board pattern).
    let mut show_archived = use_signal(|| false);

    let mut artifacts_resource = use_resource(move || {
        let include_archived = *show_archived.read();
        async move { crate::server::api::list_artifacts(include_archived).await }
    });

    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let mut selected_artifact = use_context::<crate::state::SelectedArtifactCtx>().0;

    // Publish (Postiz) is a stub for a future CI/CD pipeline — clicking it posts
    // a "coming soon" notice into the header rather than pretending to work.
    let mut notice = use_signal(|| Option::<String>::None);

    // Live-refresh: every screen is always-mounted, so the mount-time fetch never
    // re-runs — a just-published artifact wouldn't appear until an app reload.
    // Re-fetch whenever the operator navigates ONTO the Artifacts screen.
    use_effect(move || {
        if *active_screen.read() == crate::state::Screen::Artifacts {
            artifacts_resource.restart();
        }
    });

    // Row click: stash the artifact and jump to the sandboxed viewer.
    let on_select = move |artifact: crate::server::api::ArtifactInfo| {
        selected_artifact.set(Some(artifact));
        active_screen.set(crate::state::Screen::ArtifactViewer);
    };
    // Re-fetch after a delete/archive mutation so the row list stays in sync.
    let on_changed = move |_: ()| {
        artifacts_resource.restart();
    };
    // Publish stub → surface the "coming soon" notice.
    let on_publish = move |_: ()| {
        notice.set(Some(
            "Publish → Postiz pipeline isn't wired up yet — coming soon.".to_string(),
        ));
    };

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-artifacts",
            "data-screen-label": "15 Artifacts",

            {
                match &*artifacts_resource.value().read() {
                    None => rsx! {
                        div { class: "screen-header",
                            div { class: "screen-header-left",
                                div { class: "screen-tag", "// MODULE 15" }
                                h1 { class: "screen-title", "Artifacts" }
                                span { class: "screen-status", "· Loading…" }
                            }
                        }
                    },
                    Some(Err(_)) => rsx! {
                        div { class: "screen-header",
                            div { class: "screen-header-left",
                                div { class: "screen-tag", "// MODULE 15" }
                                h1 { class: "screen-title", "Artifacts" }
                            }
                        }
                        div { class: "panel",
                            div { class: "panel-title", "Couldn't load artifacts." }
                            p { class: "panel-sub",
                                "Something went wrong reaching the artifact store. Try again, or check the gateway logs."
                            }
                        }
                    },
                    Some(Ok(artifacts)) => {
                        let count = artifacts.len();
                        rsx! {
                            div { class: "screen-header",
                                div { class: "screen-header-left",
                                    div { class: "screen-tag", "// MODULE 15" }
                                    h1 { class: "screen-title", "Artifacts" }
                                    span { class: "screen-status",
                                        "· {count} artifact",
                                        if count == 1 { "" } else { "s" },
                                    }
                                }
                                div { class: "screen-actions",
                                    if let Some(msg) = notice.read().clone() {
                                        span {
                                            class: "row-sub",
                                            style: "opacity:0.7; margin-right:8px",
                                            "{msg}"
                                        }
                                    }
                                    button {
                                        class: "btn btn--ghost btn--sm",
                                        onclick: move |_| {
                                            let cur = *show_archived.read();
                                            show_archived.set(!cur);
                                        },
                                        if *show_archived.read() { "HIDE ARCHIVED" } else { "SHOW ARCHIVED" }
                                    }
                                }
                            }
                            div { class: "section-label",
                                "Artifacts "
                                span { class: "count", "· {count}" }
                            }
                            if count == 0 {
                                div { class: "panel",
                                    div { class: "panel-title", "No artifacts yet" }
                                    p { class: "panel-sub",
                                        "Ask Hermes to capture a diff walkthrough, a dashboard, a comparison, or a live checklist as an artifact — it will show up here."
                                    }
                                }
                            } else {
                                div { class: "row-list", style: "grid-template-columns: 1fr;",
                                    for artifact in artifacts.iter().cloned() {
                                        ArtifactRow {
                                            key: "{artifact.id}",
                                            artifact: artifact.clone(),
                                            on_select,
                                            on_changed,
                                            on_publish,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One row in the artifacts gallery: leading icon, title + source pill +
/// timestamp, and a trailing management cluster. A click anywhere OUTSIDE the
/// cluster opens the viewer; the cluster's own clicks `stop_propagation`.
#[component]
fn ArtifactRow(
    artifact: crate::server::api::ArtifactInfo,
    on_select: EventHandler<crate::server::api::ArtifactInfo>,
    on_changed: EventHandler<()>,
    on_publish: EventHandler<()>,
) -> Element {
    let for_select = artifact.clone();
    let icon = artifact.icon.clone().unwrap_or_else(|| "▤".to_string());
    let title = artifact.title.clone();
    let ts = format_relative(&artifact.updated_at);
    let source_label = artifact.source_kind.clone().map(|s| s.to_uppercase());
    let archived = artifact.archived;

    // Phase 46.7 Plan 05 (D-26): gallery CHAT backlink — navigates to the
    // producing session via the SAME primitive sessions.rs's row-click
    // uses (SessionIdContext.set + Screen::Chat).
    let mut session_id_ctx = use_context::<crate::state::SessionIdContext>().0;
    let mut active_screen_ctx = use_context::<Signal<crate::state::Screen>>();
    // `is_chat_source` gates the backlink button; `producing_session_id` is
    // `None` if source_kind is "chat" but source_ref is malformed/absent
    // (defensive — the button simply doesn't render in that case).
    let is_chat_source = artifact.source_kind.as_deref() == Some("chat");
    let producing_session_id: Option<String> = artifact
        .source_ref
        .as_deref()
        .and_then(producing_session_id_from_source_ref);

    // Two-click delete confirmation (avoids a browser confirm() dialog).
    let mut confirming = use_signal(|| false);

    let id_arch = artifact.id.clone();
    let id_del = artifact.id.clone();
    let (archive_title, archive_glyph) = if archived {
        ("Unarchive", "\u{21ba}") // ↺
    } else {
        ("Archive", "\u{1f4e6}") // 📦
    };

    // Sanitize the title into a download filename, falling back to the id.
    let download_name = {
        let base: String = title
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let base = if base.trim_matches('_').is_empty() {
            artifact.id.clone()
        } else {
            base
        };
        format!("{base}.html")
    };

    rsx! {
        div {
            class: "row",
            style: "grid-template-columns: auto 1fr auto;",
            onclick: move |_| on_select.call(for_select.clone()),
            span { "{icon}" }
            div { class: "row-main",
                span { class: "row-title", "{title}" }
                if let Some(label) = &source_label {
                    span { class: "row-sub",
                        span { class: "pill", "{label}" }
                    }
                }
                span { class: "row-sub", style: "opacity:0.5",
                    "{ts}"
                    if archived {
                        span { class: "pill", style: "margin-left:6px", "ARCHIVED" }
                    }
                }
            }
            // Management cluster — stop_propagation so its clicks never open the viewer.
            div {
                style: "display:flex; gap:4px; align-items:center;",
                onclick: move |e| e.stop_propagation(),
                // Phase 46.7 Plan 05 (D-26): CHAT backlink — placed in the
                // management cluster (already stop_propagations above) so it
                // never fights the row's click-opens-viewer behavior.
                if is_chat_source {
                    if let Some(sid) = producing_session_id.clone() {
                        button {
                            class: "btn btn--icon btn--ghost",
                            title: "Open producing session",
                            "aria-label": "Open the session that produced this artifact",
                            onclick: move |_| {
                                session_id_ctx.set(sid.clone());
                                active_screen_ctx.set(crate::state::Screen::Chat);
                            },
                            "\u{2197}" // ↗
                        }
                    }
                }
                a {
                    class: "btn btn--icon btn--ghost",
                    href: "/artifacts/{artifact.id}",
                    "download": "{download_name}",
                    title: "Download HTML",
                    "\u{2b07}" // ⬇
                }
                button {
                    class: "btn btn--icon btn--ghost",
                    title: "Publish to Postiz (pipeline coming soon)",
                    onclick: move |_| on_publish.call(()),
                    "\u{1f680}" // 🚀
                }
                button {
                    class: "btn btn--icon btn--ghost",
                    title: archive_title,
                    onclick: move |_| {
                        let id = id_arch.clone();
                        spawn(async move {
                            let _ = crate::server::api::set_artifact_archived(id, !archived).await;
                            on_changed.call(());
                        });
                    },
                    "{archive_glyph}"
                }
                if *confirming.read() {
                    button {
                        class: "btn btn--icon",
                        style: "color: var(--danger)",
                        title: "Confirm delete (irreversible)",
                        onclick: move |_| {
                            let id = id_del.clone();
                            spawn(async move {
                                let _ = crate::server::api::delete_artifact(id).await;
                                on_changed.call(());
                            });
                        },
                        "\u{2713}" // ✓
                    }
                    button {
                        class: "btn btn--icon btn--ghost",
                        title: "Cancel",
                        onclick: move |_| confirming.set(false),
                        "\u{2715}" // ✕
                    }
                } else {
                    button {
                        class: "btn btn--icon btn--ghost",
                        title: "Delete",
                        onclick: move |_| confirming.set(true),
                        "\u{1f5d1}" // 🗑
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::producing_session_id_from_source_ref;

    /// Phase 46.7 UAT test 6 regression: the canonical web session id is itself
    /// colon-delimited, so a first-colon split truncated it to `"agent"` and the
    /// backlink navigated to a nonexistent session (blank chat).
    #[test]
    fn recovers_colon_bearing_web_session_id() {
        let session = "agent:main:web:dm:af60c4b6-97c3-49a0-9017-85324469236f";
        let source_ref = format!("{session}:index.html");
        assert_eq!(
            producing_session_id_from_source_ref(&source_ref).as_deref(),
            Some(session),
            "must recover the FULL colon-bearing session id, not the first segment"
        );
    }

    #[test]
    fn recovers_colon_free_session_id() {
        assert_eq!(
            producing_session_id_from_source_ref("plain-session:report.html").as_deref(),
            Some("plain-session")
        );
    }

    #[test]
    fn rejects_malformed_source_refs() {
        assert_eq!(
            producing_session_id_from_source_ref("no-colon-at-all"),
            None
        );
        assert_eq!(producing_session_id_from_source_ref(":index.html"), None);
        assert_eq!(producing_session_id_from_source_ref("session-id:"), None);
        assert_eq!(producing_session_id_from_source_ref(""), None);
    }
}
