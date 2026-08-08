//! Phase 36.3.7.11 Plan 02 (D-01 / D-06) — Kanban card component.
//!
//! Visual contract = Models-page reference (CONTEXT canonical_refs):
//! cyan-glow border, monospace heading, thin separator, footer row of meta
//! chips. Per UI-SPEC §3.7: heading slot, separator, footer chips
//! (StatusChip / PriorityChip / AssigneeChip / AgeChip), action button
//! glyph `▶` on the right.
//!
//! Plan 02 wires the drag-source: `draggable="true"` plus real
//! `ondragstart`/`ondragend` handlers that update a shared
//! `dragged_task_id: Signal<Option<String>>`. Plan 03 will consume the
//! `kn-card-action-<id>` id for drawer-close focus restoration.

use crate::protocol::{AttachmentRow, DecomposeOrSpecify, TaskRow};
use dioxus::html::FileData;
use dioxus::html::HasFileData;
use dioxus::prelude::*;

// =============================================================================
// Phase 46.4 Plan 06 (D-03/D-04) — attachment upload state + helpers
// =============================================================================

/// Phase 46.4 Plan 06 (T-46.4-06): client-side pre-check mirror of the
/// server's `MAX_ATTACHMENT_BYTES` cap — UX only, so an obviously-oversized
/// file never queues a doomed upload. The server (kanban_api.rs) is the
/// authoritative enforcement point and re-checks after base64 decode.
// Web-live: the real caller (spawn_attachment_reads, itself consumed only
// in the wasm/FileEngine render path) is cfg'd out on native, so the native
// `bin test` build sees this as dead. Scope the allow to native only —
// wasm still enforces the live-usage check (repo pattern, mirrors
// format_age_secs / current_unix_time below).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CLIENT_MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// Phase 46.4 Plan 06: per-file upload state NOT yet reflected in the
/// persisted `fetch_attachments` list — covers the `uploading` and `error`
/// states from the UI-SPEC Attachment states contract. `attached` state
/// lives in the `fetch_attachments` resource itself (the source of truth
/// once the store write succeeds); this local overlay only tracks in-flight
/// work so the resource doesn't need optimistic-insert plumbing.
// Web-live overlay type — constructed only from the wasm/FileEngine render
// path; native `bin test` sees it as unconstructed. Native-scoped allow.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, PartialEq)]
struct LocalUpload {
    filename: String,
    status: UploadStatus,
    /// Cached base64 payload so the `↻` retry control can re-POST without
    /// re-reading the file from disk (the original `FileData` handle is not
    /// retained past the initial read).
    content_b64: String,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, PartialEq)]
enum UploadStatus {
    Uploading,
    Error(String),
}

/// Phase 46.4 Plan 06: POST one already-encoded attachment. Shared by the
/// initial upload path and the `↻` retry control so both go through the
/// exact same success/error handling.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn spawn_attach_call(
    filename: String,
    content_b64: String,
    task_id: String,
    mut local_uploads: Signal<Vec<LocalUpload>>,
    mut attachments: Resource<Result<Vec<AttachmentRow>, ServerFnError>>,
) {
    spawn(async move {
        match crate::server::kanban_api::attach_file(task_id, None, filename.clone(), content_b64)
            .await
        {
            Ok(_) => {
                // Settled — drop the local overlay entry; the restarted
                // `fetch_attachments` resource is now the source of truth
                // for this file's chip.
                local_uploads.write().retain(|u| u.filename != filename);
                attachments.restart();
            }
            Err(e) => {
                let mut w = local_uploads.write();
                if let Some(u) = w.iter_mut().find(|u| u.filename == filename) {
                    u.status = UploadStatus::Error(e.to_string());
                }
            }
        }
    });
}

/// Phase 46.4 Plan 06 (D-03/D-04): read each picked/dropped `FileData`'s
/// bytes, enforce the client-side cap, base64-encode, and hand off to
/// `spawn_attach_call`. Each file is an independent spawned task so one
/// slow/failed read never blocks the others.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn spawn_attachment_reads(
    files: Vec<FileData>,
    task_id: String,
    mut local_uploads: Signal<Vec<LocalUpload>>,
    attachments: Resource<Result<Vec<AttachmentRow>, ServerFnError>>,
) {
    for file in files {
        let filename = file.name();
        let task_id = task_id.clone();
        spawn(async move {
            let bytes = match file.read_bytes().await {
                Ok(b) => b,
                Err(e) => {
                    local_uploads.write().push(LocalUpload {
                        filename: filename.clone(),
                        status: UploadStatus::Error(format!("could not read file: {e}")),
                        content_b64: String::new(),
                    });
                    return;
                }
            };
            // UAT gap fix: a failed/empty read (e.g. a Safari drag/drop
            // File-API quirk) must never queue an upload — surface an
            // error chip locally instead of POSTing a 0-byte payload.
            if bytes.is_empty() {
                local_uploads.write().push(LocalUpload {
                    filename: filename.clone(),
                    status: UploadStatus::Error("empty file — not uploaded".to_string()),
                    content_b64: String::new(),
                });
                return;
            }
            if bytes.len() > CLIENT_MAX_ATTACHMENT_BYTES {
                // UI-SPEC Copywriting Contract — attachment size cap message.
                local_uploads.write().push(LocalUpload {
                    filename: filename.clone(),
                    status: UploadStatus::Error(format!(
                        "{filename} exceeds the 10MB attachment limit"
                    )),
                    content_b64: String::new(),
                });
                return;
            }
            let content_b64 = {
                use base64::Engine;
                base64::engine::general_purpose::STANDARD.encode(&bytes)
            };
            local_uploads.write().push(LocalUpload {
                filename: filename.clone(),
                status: UploadStatus::Uploading,
                content_b64: content_b64.clone(),
            });
            spawn_attach_call(filename, content_b64, task_id, local_uploads, attachments);
        });
    }
}

/// Phase 46.4 Plan 06: `{filename} · {size}` chip copy — human-readable
/// byte size (UI-SPEC Copywriting Contract example: `poem_draft.txt · 4.2 KB`).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn format_attachment_size(bytes: i64) -> String {
    let b = bytes.max(0) as f64;
    if b < 1024.0 {
        format!("{b:.0} B")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB", b / 1024.0)
    } else {
        format!("{:.1} MB", b / (1024.0 * 1024.0))
    }
}

/// Phase 46.4 Plan 06 (D-10): truncate a long absolute path for the compact
/// card row — keeps the filename-bearing tail, prefixes an ellipsis when
/// truncated. The full path is always available via the native `title`
/// tooltip (UI-SPEC Copywriting Contract).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn truncate_output_path(path: &str, max_chars: usize) -> String {
    if path.chars().count() <= max_chars {
        return path.to_string();
    }
    let tail: String = path
        .chars()
        .rev()
        .take(max_chars.saturating_sub(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("\u{2026}{tail}")
}

/// Phase 46.4 Plan 06 (D-10): fire-and-forget clipboard write, mirroring
/// `shell_legacy/block_stream.rs`'s `write_to_clipboard` (no toast/modal
/// feedback beyond the ✓ icon flip UI-SPEC specifies).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn write_output_path_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(window) = web_sys::window() {
            let _ = window.navigator().clipboard().write_text(text);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

/// Phase 36.3.7.11 Plan 02: a single kanban task card.
///
/// Props:
///
/// - `task`: read-only signal carrying the task row.
/// - `on_open_drawer`: handler invoked when the user clicks the action
///   button. Plan 03 wires the drawer.
/// - `dragged_task_id`: shared signal tracking the currently-dragged
///   task. `ondragstart` writes `Some(task_id)`; `ondragend` clears it.
/// - `is_pending`: true while an optimistic update is in flight for this
///   card (UI-SPEC §4.2 `kn-pending-pulse` animation).
/// - `lens_dimmed`: Phase 47.4 Plan 09 (D-05) — true when the board
///   PROFILE lens is active and this card's assignee does not match the
///   selected profile. Pure presentational flag computed by the caller
///   (`KanbanColumn`); this component only renders the resulting DOM
///   attribute below. Never removes the card from the DOM, never filters
///   it out of the column's iteration — dimming only.
#[component]
pub fn KanbanCard(
    task: ReadSignal<TaskRow>,
    on_open_drawer: EventHandler<String>,
    dragged_task_id: Signal<Option<String>>,
    is_pending: bool,
    lens_dimmed: bool,
    // Plan 03 (D-12): emitted when the user clicks ⚗ Decompose or ✨
    // Specify on a TRIAGE card. ScreenKanban dispatches the actual
    // `run_decompose_or_specify` server fn. Only the TRIAGE arm renders
    // these buttons (see below).
    on_triage_action: EventHandler<(String, DecomposeOrSpecify)>,
) -> Element {
    let t = task.read().clone();
    let is_triage = t.status == "triage";
    let task_id_for_decompose = t.id.clone();
    let task_id_for_specify = t.id.clone();
    // ARIA label per UI-SPEC §6.1.
    let aria = format!(
        "{} — {}, priority {}, assigned to {}",
        t.title, t.status, t.priority, t.assignee
    );
    let task_id_for_click = t.id.clone();
    let task_id_for_start = t.id.clone();
    let task_title_for_aria = t.title.clone();

    // Plan 03 dependency: stable id `kn-card-action-<task_id>` so the
    // drawer-close focus restore can target the originating card. Same id
    // is used by the drawer's `on_close` handler in Plan 03.
    let action_id = format!("kn-card-action-{}", t.id);

    // is_dragging is true when THIS card is the active drag source — driven
    // by the shared dragged_task_id signal at board scope. Reading the
    // signal here causes Dioxus to subscribe; the read is value-copy so no
    // lock is held.
    let is_dragging = dragged_task_id
        .read()
        .as_deref()
        .map(|id| id == t.id)
        .unwrap_or(false);

    // ------------------------------------------------------------------
    // Phase 46.4 Plan 06 (D-03/D-04/D-10): attachment + output-path state.
    // All hooks register unconditionally (Pattern E — PATTERNS.md).
    // ------------------------------------------------------------------

    // Persisted attachments — read side, keyed on this card's task id.
    // Reads `t.id` (a plain String snapshot, not the reactive `task`
    // signal) inside the async closure per the drawer.rs precedent; the
    // resource restarts on `.restart()` after a successful upload.
    let task_id_for_resource = t.id.clone();
    let attachments = use_resource(move || {
        let tid = task_id_for_resource.clone();
        async move { crate::server::kanban_api::fetch_attachments(tid, None).await }
    });

    // Phase 46.6 gap-closure: a completed card links to the artifact it produced.
    // Reactive to `task` (reads status inside the closure), so the link appears
    // as soon as the card reaches DONE — no board reload. Only DONE cards hit
    // the server; others resolve to None without a request.
    let card_artifact = use_resource(move || {
        let snap = task.read();
        let tid = snap.id.clone();
        let done = snap.status == "done";
        drop(snap);
        async move {
            if done {
                crate::server::api::artifact_for_task(tid)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            }
        }
    });
    let mut nav_screen = use_context::<Signal<crate::state::Screen>>();
    let mut selected_artifact = use_context::<crate::state::SelectedArtifactCtx>().0;

    // Local overlay for in-flight/failed uploads not yet in the persisted
    // list (or that never made it there).
    let mut local_uploads: Signal<Vec<LocalUpload>> = use_signal(Vec::new);

    // Active file-drop-target highlight (UI-SPEC Interaction Contract —
    // Drop-target visual states). Distinct from `dragged_task_id`, which
    // tracks the INTERNAL card-reorder drag channel.
    let mut file_drag_active: Signal<bool> = use_signal(|| false);

    // Output-path copy-button success flip (UI-SPEC: `⧉` → `✓` for ~1200ms).
    let mut output_path_copied: Signal<bool> = use_signal(|| false);

    let task_id_for_attach = t.id.clone();
    let task_id_for_attach_drop = t.id.clone();
    // Stable per-card id so the hidden file input + its `label[for=...]`
    // trigger pair correctly identify each other across every card on the
    // board (ids must be unique document-wide).
    let attach_input_id = format!("kn-attach-input-{}", t.id);
    let attach_input_id_for_label = attach_input_id.clone();
    let attach_input_id_for_label2 = attach_input_id.clone();

    // Snapshot the resource + local-overlay state once per render (avoids
    // re-reading the signal mid-conditional inside the rsx! tree below).
    let attachments_state = attachments.value()();
    let persisted_attachments: Vec<AttachmentRow> = match &attachments_state {
        Some(Ok(rows)) => rows.clone(),
        _ => Vec::new(),
    };
    let local_uploads_snapshot = local_uploads.read().clone();
    let has_no_attachments = persisted_attachments.is_empty() && local_uploads_snapshot.is_empty();

    // Phase 46.6 gap-closure: the artifact this (completed) card produced, if any.
    let linked_artifact: Option<crate::server::api::ArtifactInfo> =
        card_artifact.value()().flatten();

    // Output-path row (D-10): done tasks only; absence of output_path
    // renders nothing at all per the UI-SPEC empty-state row.
    let show_output_path = t.status == "done" && t.output_path.is_some();
    let output_path_full = t.output_path.clone().unwrap_or_default();
    let output_path_truncated = truncate_output_path(&output_path_full, 28);
    let output_path_for_copy = output_path_full.clone();

    rsx! {
        div {
            class: "kn-card",
            role: "listitem",
            tabindex: "0",
            "aria-label": "{aria}",
            // D-06: drag source flag — required for ondragstart/ondrop to
            // fire in HTML5 native DnD.
            draggable: "true",
            "data-task-id": "{t.id}",
            // UI-SPEC §4.2: visual state attribute toggles for the
            // .kn-card[data-dragging] / .kn-card[data-pending] CSS rules.
            "data-dragging": if is_dragging { "true" },
            "data-pending": if is_pending { "true" },
            // Phase 47.4 Plan 09 (D-05): omit the attribute entirely when
            // false so the existing CSS selector (plan 01's kanban.css
            // rule targeting this attribute) stays cheap and the default
            // DOM is unchanged.
            "data-lens-dimmed": if lens_dimmed { "true" },
            // D-06: drag-start — write Some(task_id) into the shared signal
            // so the drop target can read it. Signal `.write()` is a
            // value-copy operation; no borrow held across `.await` (there
            // is no await here — this is a synchronous handler).
            ondragstart: move |_| {
                *dragged_task_id.write() = Some(task_id_for_start.clone());
            },
            // D-06: drag-end (covers both successful drops and cancels) —
            // clear the shared signal as a safety net even though
            // KanbanColumn's ondrop also clears it.
            ondragend: move |_| {
                *dragged_task_id.write() = None;
            },
            // Heading slot: monospace title.
            div { class: "kn-card-heading", "{t.title}" }
            // Thin internal separator (per UI-SPEC §3.7).
            div { class: "kn-card-separator" }
            // Footer chip row.
            div { class: "kn-card-footer",
                span {
                    class: "kn-chip",
                    "data-kind": "status",
                    "data-status": "{t.status}",
                    "{t.status}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "priority",
                    "P{t.priority}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "assignee",
                    "{t.assignee}"
                }
                span {
                    class: "kn-chip",
                    "data-kind": "age",
                    "{format_age_secs(t.created_at)}"
                }
                // Phase 46.6 gap-closure: a completed card links to the artifact
                // it produced, opening the sandboxed viewer. stop_propagation so
                // the click doesn't bubble to the card (drag/focus).
                if let Some(art) = linked_artifact.clone() {
                    button {
                        class: "kn-chip",
                        "data-kind": "artifact",
                        style: "cursor: pointer;",
                        "aria-label": "Open the artifact this task produced",
                        onclick: move |e| {
                            e.stop_propagation();
                            selected_artifact.set(Some(art.clone()));
                            nav_screen.set(crate::state::Screen::ArtifactViewer);
                        },
                        "\u{25a4} artifact"
                    }
                }
                // Action button — Plan 03 opens the drawer. The stable
                // id `kn-card-action-<task_id>` enables drawer-close focus
                // restoration per UI-SPEC §6.2.
                button {
                    class: "kn-card-action",
                    id: "{action_id}",
                    "aria-label": "Open {task_title_for_aria} detail",
                    onclick: move |_| {
                        on_open_drawer.call(task_id_for_click.clone());
                    },
                    "▶"
                }
            }
            // Plan 03 (D-12 / UI-SPEC §3.7): TRIAGE-card-only action row
            // — ⚗ Decompose / ✨ Specify buttons that call the screen's
            // run_decompose_or_specify dispatcher. The drawer's
            // TriageActionRow does the same for the drawer surface.
            if is_triage {
                div { class: "kn-card-triage-actions",
                    button {
                        class: "kn-action-btn kn-action-btn--sm",
                        "aria-label": "Decompose triage task",
                        onclick: move |_| {
                            on_triage_action
                                .call((task_id_for_decompose.clone(), DecomposeOrSpecify::Decompose));
                        },
                        "\u{2697} Decompose"
                    }
                    button {
                        class: "kn-action-btn kn-action-btn--sm",
                        "aria-label": "Specify triage task",
                        onclick: move |_| {
                            on_triage_action
                                .call((task_id_for_specify.clone(), DecomposeOrSpecify::Specify));
                        },
                        "\u{2728} Specify"
                    }
                }
            }
            // ------------------------------------------------------------
            // Phase 46.4 Plan 06 (D-03/D-04 / UI-SPEC "Card file
            // attachments" PRIMARY surface): attachments drop-zone + chip
            // strip. Net-new row, sibling of kn-card-triage-actions —
            // KanbanCard is extended, not rebuilt.
            // ------------------------------------------------------------
            div {
                class: "kn-attach-strip",
                "data-drop-active": if *file_drag_active.read() && dragged_task_id.read().is_none() { "true" },
                // UI-SPEC Drag-and-drop collision avoidance: only arm the
                // file-drop highlight when no INTERNAL card-reorder drag is
                // active (dragged_task_id is Some during a reorder).
                ondragover: move |evt: DragEvent| {
                    evt.prevent_default();
                    if dragged_task_id.read().is_none() {
                        file_drag_active.set(true);
                    }
                },
                ondragleave: move |_| {
                    file_drag_active.set(false);
                },
                ondrop: move |evt: DragEvent| {
                    evt.prevent_default();
                    let was_internal_drag = dragged_task_id.read().is_some();
                    file_drag_active.set(false);
                    if !was_internal_drag {
                        // UAT gap fix: `evt.data_transfer().files()` reads
                        // the DragEvent's FileList, which does not survive
                        // the async `read_bytes()` call in some browsers —
                        // the cross-browser drop failure. `evt.files()`
                        // matches the working picker path (onchange below)
                        // and is the correct/stable FileEngine accessor for
                        // both drag-drop and click-to-pick.
                        let files = evt.files();
                        if !files.is_empty() {
                            spawn_attachment_reads(
                                files,
                                task_id_for_attach_drop.clone(),
                                local_uploads,
                                attachments,
                            );
                        }
                    }
                },
                if has_no_attachments {
                    label {
                        class: "kn-attach-idle",
                        r#for: "{attach_input_id_for_label}",
                        "Drop files or \u{1F4CE} Attach"
                    }
                } else {
                    div { class: "kn-attach-chips",
                        for row in persisted_attachments.iter() {
                            span {
                                key: "{row.id}",
                                class: "kn-attach-chip",
                                "{row.filename} \u{00B7} {format_attachment_size(row.size_bytes)}"
                            }
                        }
                        for up in local_uploads_snapshot.iter() {
                            {
                                let filename_for_retry = up.filename.clone();
                                let content_b64_for_retry = up.content_b64.clone();
                                let filename_for_remove = up.filename.clone();
                                let task_id_for_retry = task_id_for_attach.clone();
                                match &up.status {
                                    UploadStatus::Uploading => rsx! {
                                        span {
                                            key: "{up.filename}",
                                            class: "kn-attach-chip kn-attach-chip--uploading",
                                            "{up.filename} \u{00B7} uploading\u{2026}"
                                        }
                                    },
                                    UploadStatus::Error(_) => rsx! {
                                        span {
                                            key: "{up.filename}",
                                            class: "kn-attach-chip kn-attach-chip--error",
                                            "{up.filename} \u{00B7} upload failed"
                                            button {
                                                class: "kn-attach-retry",
                                                "aria-label": "Retry {up.filename}",
                                                onclick: move |_| {
                                                    local_uploads
                                                        .write()
                                                        .retain(|u| u.filename != filename_for_retry);
                                                    spawn_attach_call(
                                                        filename_for_retry.clone(),
                                                        content_b64_for_retry.clone(),
                                                        task_id_for_retry.clone(),
                                                        local_uploads,
                                                        attachments,
                                                    );
                                                },
                                                "\u{21BB}"
                                            }
                                            button {
                                                class: "kn-attach-remove",
                                                "aria-label": "Remove {up.filename}",
                                                onclick: move |_| {
                                                    local_uploads
                                                        .write()
                                                        .retain(|u| u.filename != filename_for_remove);
                                                },
                                                "\u{00D7}"
                                            }
                                        }
                                    },
                                }
                            }
                        }
                        label {
                            class: "kn-attach-add",
                            r#for: "{attach_input_id_for_label2}",
                            "aria-label": "Attach files",
                            "\u{1F4CE}"
                        }
                    }
                }
                input {
                    id: "{attach_input_id}",
                    class: "kn-card-input-hidden",
                    r#type: "file",
                    multiple: true,
                    onchange: move |evt: FormEvent| {
                        let files = evt.files();
                        if !files.is_empty() {
                            spawn_attachment_reads(
                                files,
                                task_id_for_attach.clone(),
                                local_uploads,
                                attachments,
                            );
                        }
                    },
                }
            }
            // ------------------------------------------------------------
            // Phase 46.4 Plan 06 (D-10 / UI-SPEC "Output-path display"
            // SECONDARY surface): read-only 📂 path + ⧉ copy, done tasks
            // only. Absence of output_path renders nothing (empty state).
            // ------------------------------------------------------------
            if show_output_path {
                div { class: "kn-card-output-path",
                    span {
                        class: "kn-card-output-path-text",
                        title: "{output_path_full}",
                        "\u{1F4C2} {output_path_truncated}"
                    }
                    button {
                        class: "kn-card-output-copy",
                        "data-copied": if *output_path_copied.read() { "true" },
                        "aria-label": "Copy output path",
                        onclick: move |_| {
                            write_output_path_to_clipboard(&output_path_for_copy);
                            output_path_copied.set(true);
                            spawn(async move {
                                crate::platform::timer::sleep(1200).await;
                                output_path_copied.set(false);
                            });
                        },
                        if *output_path_copied.read() { "\u{2713}" } else { "\u{29C9}" }
                    }
                }
            }
        }
    }
}

/// Phase 36.3.7.11 Plan 01: format a task's `created_at` (Unix epoch
/// seconds, float) as a short relative age string for the card AGE chip.
/// Plan 01 uses a coarse seconds/minutes/hours/days bucket — Plan 04 polish
/// may add a richer format. Falls back to `"--"` on any clock error.
#[allow(dead_code)] // called in KanbanCard rsx! format string; dead_code fires on test target
fn format_age_secs(created_at: f64) -> String {
    let now = current_unix_time();
    if now <= 0.0 {
        return "--".to_string();
    }
    let age_secs = (now - created_at).max(0.0) as u64;
    if age_secs < 60 {
        format!("{}s", age_secs)
    } else if age_secs < 60 * 60 {
        format!("{}m", age_secs / 60)
    } else if age_secs < 24 * 60 * 60 {
        format!("{}h", age_secs / (60 * 60))
    } else {
        format!("{}d", age_secs / (24 * 60 * 60))
    }
}

/// Phase 36.3.7.11 Plan 01: current time in Unix seconds (float). On
/// WASM uses `js_sys::Date::now()`; on native uses `SystemTime`. Returns
/// `0.0` on error so the caller can fall back to a placeholder.
#[allow(dead_code)] // called from format_age_secs; dead_code fires on test target
fn current_unix_time() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() / 1000.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0)
    }
}
