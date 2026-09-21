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

/// Recover the producing worker's name from a team artifact's composite
/// `source_ref` (`team_source_ref(room_id, drive_id, worker)` in
/// `server/group_team_api.rs` writes `"{room_id}:{drive_id}:{worker}"`;
/// Phase 52.1 Plan 05 D-09/D-10 keying).
///
/// Splits into exactly THREE pieces FROM THE LEFT and returns the third —
/// the opposite direction from `producing_session_id_from_source_ref` above,
/// which splits on the LAST `:` because a chat session id is itself
/// colon-bearing (`agent:main:web:dm:<uuid>`). The team reference's three
/// components are colon-free BY CONSTRUCTION (`team_source_ref`'s own doc
/// comment: a validated room-name slug, a version-4 UUID, and a validated
/// profile slug — none of which can itself contain a `:`), so a left-anchored
/// three-piece split is unambiguous and correctly preserves a worker name
/// even in the impossible case that one component contained a colon (it
/// would merge into the third piece rather than shift/truncate it, since
/// `splitn(3, ..)` caps the split count at two, leaving any remaining
/// colons in the final piece).
// Native builds never reach the HermesApp render tree (wasm-only), same
// reasoning as the chat helper directly above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn team_producer_from_source_ref(source_ref: &str) -> Option<String> {
    let mut parts = source_ref.splitn(3, ':');
    let _room_id = parts.next()?;
    let _drive_id = parts.next()?;
    let worker = parts.next()?.trim();
    if worker.is_empty() {
        None
    } else {
        Some(worker.to_string())
    }
}

/// Recover the producing group-room id from a team artifact's composite
/// `source_ref` (`team_source_ref(room_id, drive_id, worker)` in
/// `server/group_team_api.rs` writes `"{room_id}:{drive_id}:{worker}"`;
/// Phase 52.1 Plan 05 D-09/D-10 keying).
///
/// Splits into exactly THREE pieces FROM THE LEFT, same direction and same
/// `splitn(3, ..)` contract as `team_producer_from_source_ref` above, but
/// returns the FIRST component (the room id) instead of the third (the
/// worker name). Returns `None` when fewer than three parts exist or the
/// first is empty.
// Native builds never reach the HermesApp render tree (wasm-only), same
// reasoning as the sibling helpers above.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn team_room_from_source_ref(source_ref: &str) -> Option<String> {
    let mut parts = source_ref.splitn(3, ':');
    let room_id = parts.next()?;
    let _drive_id = parts.next()?;
    let _worker = parts.next()?;
    if room_id.is_empty() {
        None
    } else {
        Some(room_id.to_string())
    }
}

/// Which backlink button, if any, an artifact row's source kind and source
/// reference resolve to (D-13) — extracted purely so the gating logic for
/// all three source kinds is unit-testable in one place without mounting a
/// wasm component (`ArtifactRow` itself renders only under wasm). The
/// render path below uses its own `is_chat_source`/`is_kanban_source`/
/// `is_team_source` locals directly (the chat branch is a pre-existing,
/// shipped affordance, deliberately left untouched); this fn exists to pin
/// the SAME gating decisions under test, not to replace the render-path
/// wiring.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // native-test-only extraction; never called from wasm render code
enum RowBacklinkTarget {
    Chat(String),
    Kanban(String),
    Team(String),
}

#[allow(dead_code)] // native-test-only extraction; never called from wasm render code
fn resolve_row_backlink_target(
    source_kind: Option<&str>,
    source_ref: Option<&str>,
) -> Option<RowBacklinkTarget> {
    match source_kind {
        Some("chat") => source_ref
            .and_then(producing_session_id_from_source_ref)
            .map(RowBacklinkTarget::Chat),
        Some("kanban") => source_ref
            .filter(|s| !s.is_empty())
            .map(|s| RowBacklinkTarget::Kanban(s.to_string())),
        Some("team") => source_ref
            .and_then(team_room_from_source_ref)
            .map(RowBacklinkTarget::Team),
        _ => None,
    }
}

/// The default filename extension for a source_format wire string
/// ("html" | "md" | "code"), falling back to "html" for an empty or
/// unrecognized value.
///
/// Mirrors `ironhermes_artifacts::render::SourceFormat::{parse,
/// default_extension}` (`crates/ironhermes-artifacts/src/render.rs`) rather
/// than calling into that crate directly: `ironhermes-artifacts` depends on
/// `rusqlite`, which does not compile for the wasm32 target this component
/// actually renders under (see the CLAUDE.md wasm-gate constraint and
/// `Cargo.toml`'s `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`
/// placement of `ironhermes-artifacts`). Keep this in sync with
/// `SourceFormat`'s wire strings if that enum ever changes.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn source_format_default_extension(wire: &str) -> &'static str {
    match wire {
        "md" => "md",
        "code" => "txt",
        _ => "html", // "html", empty, and any unrecognized wire string (D-03)
    }
}

/// Build the download filename for an artifact row (D-03).
///
/// `raw = false` (the rendered-HTML download) always ends in `.html`,
/// regardless of `source_format` — the rendered route renders every format to
/// HTML. `raw = true` (the raw-source download) ends in the extension for
/// `source_format` (`.md` / `.html` / `.txt`), UNLESS `title` already ends in
/// a recognizable extension — a final `.` followed by one to eight ASCII
/// alphanumeric characters — in which case that extension is preserved
/// instead (a captured `report.py` downloads as `report.py`, not
/// `report.txt`). The stem is sanitized the same way in both cases
/// (alphanumeric/hyphen/underscore survive, everything else becomes an
/// underscore), falling back to `id` when sanitizing leaves nothing.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn artifact_download_name(title: &str, id: &str, source_format: &str, raw: bool) -> String {
    let recognized_extension = title.rfind('.').and_then(|dot| {
        let ext = &title[dot + 1..];
        let looks_like_extension = !ext.is_empty()
            && ext.len() <= 8
            && ext.chars().all(|c| c.is_ascii_alphanumeric());
        if looks_like_extension {
            Some((dot, ext.to_string()))
        } else {
            None
        }
    });

    let (stem_source, extension): (&str, String) = if raw {
        match recognized_extension {
            Some((dot, ext)) => (&title[..dot], ext),
            None => (title, source_format_default_extension(source_format).to_string()),
        }
    } else {
        (title, "html".to_string())
    };

    let base: String = stem_source
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
        id.to_string()
    } else {
        base
    };
    format!("{base}.{extension}")
}

/// Resolve the display name for a gallery row's producer (D-13/D-15), pure
/// and store-free — the actual resolution happens elsewhere by design:
/// kanban's name is resolved SERVER-SIDE (`ArtifactInfo.producer`, a
/// cross-crate kanban-store lookup a wasm component cannot perform itself);
/// team's name is a pure client-side parse of `source_ref`
/// (`team_producer_from_source_ref`, no I/O). Every other source kind — and
/// a kanban row whose DTO producer is `None` — yields `None`, which the
/// caller must render as pill-only with no placeholder text (D-15's explicit
/// degrade).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn resolve_row_producer_name(
    source_kind: Option<&str>,
    dto_producer: Option<&str>,
    source_ref: Option<&str>,
) -> Option<String> {
    match source_kind {
        Some("kanban") => dto_producer.map(str::to_string),
        Some("team") => source_ref.and_then(team_producer_from_source_ref),
        _ => None,
    }
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
    // D-13/D-15: resolved once here — kanban reads the server-resolved DTO
    // field, team parses source_ref client-side, everything else is None.
    let producer_name: Option<String> = resolve_row_producer_name(
        artifact.source_kind.as_deref(),
        artifact.producer.as_deref(),
        artifact.source_ref.as_deref(),
    );

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

    // Phase 52.1 Plan 07 (D-13): gallery KANBAN/TEAM backlinks — the same
    // idiom as the chat backlink just above, applied to two more source
    // kinds. Each hands its target identifier to the receiving screen via
    // the pending deep-link contexts (state.rs) rather than a direct prop
    // chain, then flips `active_screen_ctx` in the same click.
    let mut pending_kanban_task_ctx = use_context::<crate::state::PendingKanbanTaskCtx>().0;
    let mut pending_room_open_ctx = use_context::<crate::state::PendingRoomOpenCtx>().0;
    let is_kanban_source = artifact.source_kind.as_deref() == Some("kanban");
    let is_team_source = artifact.source_kind.as_deref() == Some("team");
    // Per D-15, source_ref for a kanban artifact IS the bare task id — no
    // parsing needed, only an empty-reference guard.
    let producing_kanban_task_id: Option<String> = artifact
        .source_ref
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let producing_team_room_id: Option<String> = artifact
        .source_ref
        .as_deref()
        .and_then(team_room_from_source_ref);

    // Two-click delete confirmation (avoids a browser confirm() dialog).
    let mut confirming = use_signal(|| false);

    let id_arch = artifact.id.clone();
    let id_del = artifact.id.clone();
    let (archive_title, archive_glyph) = if archived {
        ("Unarchive", "\u{21ba}") // ↺
    } else {
        ("Archive", "\u{1f4e6}") // 📦
    };

    // D-03: one download-name helper for both the rendered and raw anchors.
    let download_name = artifact_download_name(&title, &artifact.id, &artifact.source_format, false);
    let raw_download_name =
        artifact_download_name(&title, &artifact.id, &artifact.source_format, true);

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
                        // D-13/D-15: the producing bot's name, plain text
                        // (not a second pill) immediately after the pill.
                        // Absent entirely when unresolvable — no placeholder,
                        // no dash, no "unknown" string.
                        if let Some(name) = &producer_name {
                            span { style: "margin-left:6px; opacity:0.8;", "{name}" }
                        }
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
                // Phase 52.1 Plan 07 (D-13): KANBAN backlink — sibling of the
                // chat backlink just above, same cluster, same glyph.
                if is_kanban_source {
                    if let Some(task_id) = producing_kanban_task_id.clone() {
                        button {
                            class: "btn btn--icon btn--ghost",
                            title: "Open producing task",
                            "aria-label": "Open the kanban task that produced this artifact",
                            onclick: move |_| {
                                pending_kanban_task_ctx.set(Some(task_id.clone()));
                                active_screen_ctx.set(crate::state::Screen::Kanban);
                            },
                            "\u{2197}" // ↗
                        }
                    }
                }
                // Phase 52.1 Plan 07 (D-13): TEAM backlink — sibling of the
                // chat backlink above; `Screen::Agents` is where the bot
                // roster and its group-room workspace live.
                if is_team_source {
                    if let Some(room_id) = producing_team_room_id.clone() {
                        button {
                            class: "btn btn--icon btn--ghost",
                            title: "Open producing room",
                            "aria-label": "Open the team room that produced this artifact",
                            onclick: move |_| {
                                pending_room_open_ctx.set(Some(room_id.clone()));
                                active_screen_ctx.set(crate::state::Screen::Agents);
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
                a {
                    class: "btn btn--icon btn--ghost",
                    href: "/artifacts/{artifact.id}/raw",
                    "download": "{raw_download_name}",
                    title: "Download raw source",
                    "aria-label": "Download the raw, unrendered source for this artifact",
                    "\u{1f4c4}" // 📄
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
    use super::{
        artifact_download_name, producing_session_id_from_source_ref,
        resolve_row_backlink_target, resolve_row_producer_name, team_producer_from_source_ref,
        team_room_from_source_ref, RowBacklinkTarget,
    };

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

    // -- team_producer_from_source_ref (Plan 06, D-13) -----------------------

    #[test]
    fn team_producer_from_source_ref_three_part_returns_third_component() {
        assert_eq!(
            team_producer_from_source_ref("my-room:11111111-1111-1111-1111-111111111111:hand"),
            Some("hand".to_string())
        );
    }

    #[test]
    fn team_producer_from_source_ref_two_part_returns_none_not_partial() {
        assert_eq!(
            team_producer_from_source_ref("my-room:11111111-1111-1111-1111-111111111111"),
            None,
            "a reference with only two components must not yield a partial value"
        );
    }

    #[test]
    fn team_producer_from_source_ref_more_than_three_segments_keeps_everything_after_second_colon() {
        // A worker name is validated colon-free in practice, but this pins the
        // left-anchored splitn(3, ..) contract: any extra colons land in the
        // third piece rather than shifting/truncating it.
        assert_eq!(
            team_producer_from_source_ref("room:drive-id:worker:extra:colon"),
            Some("worker:extra:colon".to_string())
        );
    }

    #[test]
    fn team_producer_from_source_ref_empty_third_component_is_none() {
        assert_eq!(
            team_producer_from_source_ref("my-room:11111111-1111-1111-1111-111111111111:"),
            None
        );
    }

    #[test]
    fn team_producer_from_source_ref_empty_reference_is_none() {
        assert_eq!(team_producer_from_source_ref(""), None);
    }

    #[test]
    fn team_producer_from_source_ref_regression_chat_helper_unaffected() {
        // Regression pin (plan's own requirement): the chat helper's
        // colon-bearing-session-id behavior is untouched by this addition.
        let session = "agent:main:web:dm:af60c4b6-97c3-49a0-9017-85324469236f";
        let source_ref = format!("{session}:index.html");
        assert_eq!(
            producing_session_id_from_source_ref(&source_ref).as_deref(),
            Some(session)
        );
    }

    // -- resolve_row_producer_name (Plan 06, D-13/D-15) ----------------------

    #[test]
    fn resolve_row_producer_name_kanban_with_producer_shows_it() {
        assert_eq!(
            resolve_row_producer_name(Some("kanban"), Some("worker-bot"), Some("task-1")),
            Some("worker-bot".to_string())
        );
    }

    #[test]
    fn resolve_row_producer_name_kanban_without_producer_is_none() {
        assert_eq!(
            resolve_row_producer_name(Some("kanban"), None, Some("task-1")),
            None,
            "an unresolvable kanban board must degrade to no name, never an error or placeholder"
        );
    }

    #[test]
    fn resolve_row_producer_name_team_parses_source_ref_ignoring_dto_producer() {
        // dto_producer is deliberately Some(..) here to prove team never
        // depends on it — the resolved name must come from source_ref alone.
        assert_eq!(
            resolve_row_producer_name(
                Some("team"),
                Some("should-be-ignored"),
                Some("my-room:11111111-1111-1111-1111-111111111111:hand")
            ),
            Some("hand".to_string())
        );
    }

    #[test]
    fn resolve_row_producer_name_chat_is_always_none() {
        assert_eq!(
            resolve_row_producer_name(Some("chat"), Some("irrelevant"), Some("session:file.html")),
            None
        );
    }

    #[test]
    fn resolve_row_producer_name_no_source_kind_is_none() {
        assert_eq!(resolve_row_producer_name(None, None, None), None);
    }

    #[test]
    fn resolve_row_producer_name_delegate_is_none() {
        assert_eq!(
            resolve_row_producer_name(Some("delegate"), Some("irrelevant"), Some("task-1")),
            None
        );
    }

    // -- artifact_download_name (Plan 02, D-03) -----------------------------

    #[test]
    fn artifact_download_name_rendered_always_ends_in_html() {
        for format in ["html", "md", "code", "", "bogus"] {
            assert_eq!(
                artifact_download_name("Report", "abc-123", format, false),
                "Report.html",
                "rendered download must end in .html for format {format:?}"
            );
        }
    }

    #[test]
    fn artifact_download_name_raw_uses_format_extension() {
        assert_eq!(
            artifact_download_name("Report", "abc-123", "md", true),
            "Report.md"
        );
        assert_eq!(
            artifact_download_name("Report", "abc-123", "html", true),
            "Report.html"
        );
        assert_eq!(
            artifact_download_name("Report", "abc-123", "code", true),
            "Report.txt"
        );
    }

    #[test]
    fn artifact_download_name_raw_preserves_recognized_title_extension() {
        assert_eq!(
            artifact_download_name("report.py", "abc-123", "code", true),
            "report.py",
            "a title ending in a recognizable extension must keep that extension, not the format default"
        );
    }

    #[test]
    fn artifact_download_name_raw_falls_back_to_format_extension_for_unrecognizable_title_suffix() {
        // "v1.2.3" — the final segment "3" is a valid 1-8 char alnum extension per
        // the recognizer, so this actually keeps ".3"; verify a suffix that does NOT
        // qualify (too long, non-alnum) falls through to the format default instead.
        assert_eq!(
            artifact_download_name("notes.toolongext", "abc-123", "md", true),
            "notes_toolongext.md",
            "a 'trailing extension' longer than 8 chars must not be treated as an extension"
        );
        assert_eq!(
            artifact_download_name("weird.na me", "abc-123", "md", true),
            "weird_na_me.md",
            "a 'trailing extension' containing non-alphanumeric characters must not be preserved"
        );
    }

    #[test]
    fn artifact_download_name_unrecognized_source_format_produces_html_extension_not_a_panic() {
        assert_eq!(
            artifact_download_name("Report", "abc-123", "", true),
            "Report.html"
        );
        assert_eq!(
            artifact_download_name("Report", "abc-123", "totally-bogus", true),
            "Report.html"
        );
    }

    #[test]
    fn artifact_download_name_empty_sanitized_stem_falls_back_to_id() {
        assert_eq!(
            artifact_download_name("!!!", "abc-123", "html", false),
            "abc-123.html"
        );
        assert_eq!(
            artifact_download_name("###", "abc-123", "code", true),
            "abc-123.txt"
        );
    }

    #[test]
    fn artifact_download_name_non_extension_characters_in_stem_are_sanitized() {
        assert_eq!(
            artifact_download_name("My Report v2!", "abc-123", "html", false),
            "My_Report_v2_.html"
        );
    }

    // -- team_room_from_source_ref (Plan 07, D-13) --------------------------

    #[test]
    fn team_room_from_source_ref_three_part_returns_first_component() {
        assert_eq!(
            team_room_from_source_ref("my-room:11111111-1111-1111-1111-111111111111:hand"),
            Some("my-room".to_string())
        );
    }

    #[test]
    fn team_room_from_source_ref_two_part_returns_none_not_partial() {
        assert_eq!(
            team_room_from_source_ref("my-room:11111111-1111-1111-1111-111111111111"),
            None,
            "a reference with only two components must not yield a partial value"
        );
    }

    #[test]
    fn team_room_from_source_ref_empty_room_component_is_none() {
        assert_eq!(
            team_room_from_source_ref(":11111111-1111-1111-1111-111111111111:hand"),
            None
        );
    }

    #[test]
    fn team_room_from_source_ref_empty_reference_is_none() {
        assert_eq!(team_room_from_source_ref(""), None);
    }

    // -- resolve_row_backlink_target (Plan 07, D-13) — the six <behavior> ---
    // -- cases from 52.1-07-PLAN.md Task 2, pinned individually ------------

    #[test]
    fn backlink_kanban_nonempty_source_ref_resolves_to_kanban_target() {
        assert_eq!(
            resolve_row_backlink_target(Some("kanban"), Some("task-42")),
            Some(RowBacklinkTarget::Kanban("task-42".to_string()))
        );
    }

    #[test]
    fn backlink_kanban_absent_or_empty_source_ref_is_none() {
        assert_eq!(resolve_row_backlink_target(Some("kanban"), None), None);
        assert_eq!(resolve_row_backlink_target(Some("kanban"), Some("")), None);
    }

    #[test]
    fn backlink_team_three_part_source_ref_resolves_to_team_target() {
        assert_eq!(
            resolve_row_backlink_target(
                Some("team"),
                Some("my-room:11111111-1111-1111-1111-111111111111:hand")
            ),
            Some(RowBacklinkTarget::Team("my-room".to_string()))
        );
    }

    #[test]
    fn backlink_team_fewer_than_three_parts_is_none() {
        assert_eq!(
            resolve_row_backlink_target(Some("team"), Some("my-room:only-two-parts")),
            None
        );
    }

    #[test]
    fn backlink_chat_resolves_to_chat_target_only_never_kanban_or_team() {
        let session = "agent:main:web:dm:af60c4b6-97c3-49a0-9017-85324469236f";
        let source_ref = format!("{session}:index.html");
        assert_eq!(
            resolve_row_backlink_target(Some("chat"), Some(&source_ref)),
            Some(RowBacklinkTarget::Chat(session.to_string())),
            "a chat artifact resolves to exactly its existing backlink, no new one"
        );
    }

    #[test]
    fn backlink_resolution_yields_at_most_one_target_per_row() {
        // Structural pin: the fn returns a single Option<RowBacklinkTarget>,
        // never a collection, so "at most one button renders per row" holds
        // by construction — pinned here by asserting the EXACT resolved
        // variant (never more than one) for every source kind, including
        // unrecognized ones.
        assert_eq!(
            resolve_row_backlink_target(Some("chat"), Some("session:file.html")),
            Some(RowBacklinkTarget::Chat("session".to_string()))
        );
        assert_eq!(
            resolve_row_backlink_target(Some("kanban"), Some("task-1")),
            Some(RowBacklinkTarget::Kanban("task-1".to_string()))
        );
        assert_eq!(
            resolve_row_backlink_target(
                Some("team"),
                Some("room:11111111-1111-1111-1111-111111111111:worker")
            ),
            Some(RowBacklinkTarget::Team("room".to_string()))
        );
        assert_eq!(
            resolve_row_backlink_target(Some("delegate"), Some("irrelevant")),
            None,
            "an unrecognized source kind must never fall through to another kind's target"
        );
        assert_eq!(resolve_row_backlink_target(None, None), None);
    }
}
