//! Phase 49.4 Plan 07 (D-05/D-06/D-07): the Skills IMPORT wizard — a
//! two-step flow (source picker -> preview-before-install) wired to plan
//! 05's `preview_skill_import`/`install_previewed_skill` `#[server]` fns.
//!
//! The client performs no fetch of its own, no parsing of remote content,
//! and no filesystem write — every remote read and every mutation happens
//! through those two gated entry points. Mirrors
//! `profile_shared::create_dialog`'s step-state SHAPE (not its content) and
//! `kanban/drawer.rs`'s `ReadSignal<T>` prop / `EventHandler<T>` callback
//! convention: the parent (`skills.rs`) owns `open` and the shared refresh
//! signal; this component only reads `open` and emits `on_close`/
//! `on_installed`. This file must never call the sibling `skills_resource`'s
//! restart method — see `skills.rs`'s own module-header warning.

use dioxus::prelude::*;

use crate::server::skills_import_api::{
    install_previewed_skill, list_known_skill_dirs, preview_skill_import, stage_uploaded_skill,
    SkillImportPreview,
};

/// The wizard's two steps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportStep {
    Source,
    Preview,
}

/// The three source tabs D-05 requires, plus Phase 49.4's `Upload` — a file
/// picked from the operator's OWN machine and uploaded to the agent server
/// (`LocalPath` refers to a path on the SERVER, which is a different thing).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImportSourceTab {
    Url,
    Upload,
    LocalPath,
    Paste,
}

// Phase 49.4: the client-owned `IMPORT_READ_ERROR` copy was removed. It existed
// because "the server already normalizes every `preview_skill_import` failure to
// this identical message" — no longer true, and the reason a perfectly valid
// skill could fail to import with no way to tell why. The server still returns
// exactly that copy for a fetch/read failure (where naming the internal cause
// would leak probe detail), but a SKILL.md that was read and then failed to
// PARSE now comes back with the specific reason, naming the offending field.
// Relaying the server's message is what puts that reason in front of the
// operator.

/// D-06: a preview is complete enough to enable INSTALL SKILL once it has a
/// non-empty name and a non-empty runnable command block — every other
/// field (description, version, tags) is optional and renders as an
/// em-dash when absent (the partial-frontmatter rule).
pub(crate) fn preview_is_installable(preview: &SkillImportPreview) -> bool {
    !preview.name.trim().is_empty()
        && preview
            .command_block
            .as_deref()
            .map(|c| !c.trim().is_empty())
            .unwrap_or(false)
}

/// Render a preview field, substituting an em-dash for an absent optional
/// value (D-06 partial-frontmatter rule).
fn display_or_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "\u{2014}".to_string()
    } else {
        value.to_string()
    }
}

#[component]
pub fn SkillImportWizard(
    open: ReadSignal<bool>,
    on_close: EventHandler<()>,
    on_installed: EventHandler<()>,
) -> Element {
    let mut step: Signal<ImportStep> = use_signal(|| ImportStep::Source);
    let mut source_tab: Signal<ImportSourceTab> = use_signal(|| ImportSourceTab::Url);
    let mut url_value: Signal<String> = use_signal(String::new);
    let mut path_value: Signal<String> = use_signal(String::new);
    let mut paste_value: Signal<String> = use_signal(String::new);
    let mut preview: Signal<Option<SkillImportPreview>> = use_signal(|| None);
    let mut preview_loading: Signal<bool> = use_signal(|| false);
    let mut preview_error: Signal<Option<String>> = use_signal(|| None);
    let mut installing: Signal<bool> = use_signal(|| false);
    let mut install_error: Signal<Option<String>> = use_signal(|| None);
    // The exact source string handed to preview_skill_import, captured at
    // PREVIEW time — a tab switch mid-flight cannot silently change what
    // INSTALL SKILL later installs.
    let mut resolved_source: Signal<String> = use_signal(String::new);
    // Phase 49.4 upload tab: the server-side staged path returned by
    // `stage_uploaded_skill`, the name of the picked file (shown back to the
    // operator), and the in-flight / error state for the upload itself.
    let mut upload_staged_path: Signal<String> = use_signal(String::new);
    let mut upload_filename: Signal<String> = use_signal(String::new);
    let mut uploading: Signal<bool> = use_signal(|| false);
    let mut upload_error: Signal<Option<String>> = use_signal(|| None);

    // Phase 49.4: known local skill root dirs for the Local Path quick-pick.
    //
    // Gated on `open` so a CLOSED wizard makes no server call at all. This
    // component is mounted unconditionally by the Skills screen (it renders
    // nothing until opened), so an ungated fetch here ran on every visit to the
    // Skills page — and, because `list_known_skill_dirs` is a NEW server fn, it
    // ran against a server binary that may not have it yet (hot reload updates
    // the wasm client but never recompiles `#[server]` fns). Same lazy-when-
    // closed treatment the profile drawer got. An empty or errored result simply
    // hides the quick-pick; the operator types the path as before.
    let known_dirs_resource = use_resource(move || {
        let is_open = open();
        async move {
            if !is_open {
                return Ok(Vec::new());
            }
            list_known_skill_dirs().await
        }
    });
    let known_dirs: Vec<String> = match known_dirs_resource() {
        Some(Ok(dirs)) => dirs,
        _ => Vec::new(),
    };

    // Read every signal into an owned local BEFORE rsx! (Pattern B) — no
    // GenerationalRef crosses the macro boundary.
    let open_val = open();
    let step_val = *step.read();
    let tab_val = *source_tab.read();
    let url_value_val = url_value.read().clone();
    let path_value_val = path_value.read().clone();
    let paste_value_val = paste_value.read().clone();
    let preview_val = preview.read().clone();
    let preview_loading_val = *preview_loading.read();
    let preview_error_val = preview_error.read().clone();
    let installing_val = *installing.read();
    let install_error_val = install_error.read().clone();
    let upload_staged_path_val = upload_staged_path.read().clone();
    let upload_filename_val = upload_filename.read().clone();
    let uploading_val = *uploading.read();
    let upload_error_val = upload_error.read().clone();

    if !open_val {
        return rsx! {};
    }

    let current_input = match tab_val {
        ImportSourceTab::Url => url_value_val.clone(),
        // An upload is staged server-side first; `upload_staged_path` holds the
        // path that staging returned, so from here on it behaves like any other
        // local path and flows through the same preview/install calls.
        ImportSourceTab::Upload => upload_staged_path_val.clone(),
        ImportSourceTab::LocalPath => path_value_val.clone(),
        ImportSourceTab::Paste => paste_value_val.clone(),
    };
    let can_preview = !current_input.trim().is_empty();
    let can_install = preview_val
        .as_ref()
        .map(preview_is_installable)
        .unwrap_or(false)
        && !installing_val;

    let mut reset_and_close = move || {
        step.set(ImportStep::Source);
        url_value.set(String::new());
        path_value.set(String::new());
        paste_value.set(String::new());
        preview.set(None);
        preview_error.set(None);
        install_error.set(None);
        upload_staged_path.set(String::new());
        upload_filename.set(String::new());
        uploading.set(false);
        upload_error.set(None);
        on_close.call(());
    };

    rsx! {
        div {
            class: "kn-modal-overlay",
            onclick: move |_| reset_and_close(),
            div {
                class: "skill-wizard",
                onclick: move |e| e.stop_propagation(),
                div { class: "kn-modal-header",
                    h2 { class: "kn-modal-title", "Import skill" }
                }
                div { class: "skill-wizard-steps",
                    span {
                        class: if step_val == ImportStep::Source { "skill-wizard-step is-active" } else { "skill-wizard-step" },
                        "1. SOURCE"
                    }
                    span {
                        class: if step_val == ImportStep::Preview { "skill-wizard-step is-active" } else { "skill-wizard-step" },
                        "2. PREVIEW"
                    }
                }
                div { class: "kn-modal-body",
                    if step_val == ImportStep::Source {
                        div { class: "kn-modal-segmented",
                            button {
                                class: if tab_val == ImportSourceTab::Url { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                onclick: move |_| source_tab.set(ImportSourceTab::Url),
                                "URL"
                            }
                            button {
                                class: if tab_val == ImportSourceTab::Upload { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                onclick: move |_| source_tab.set(ImportSourceTab::Upload),
                                "UPLOAD"
                            }
                            button {
                                class: if tab_val == ImportSourceTab::LocalPath { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                onclick: move |_| source_tab.set(ImportSourceTab::LocalPath),
                                "LOCAL PATH"
                            }
                            button {
                                class: if tab_val == ImportSourceTab::Paste { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                                onclick: move |_| source_tab.set(ImportSourceTab::Paste),
                                "PASTE"
                            }
                        }
                        {match tab_val {
                            ImportSourceTab::Paste => rsx! {
                                textarea {
                                    class: "kn-modal-textarea kn-modal-textarea--mono",
                                    placeholder: "Paste a full SKILL.md, frontmatter and all…",
                                    value: "{paste_value_val}",
                                    oninput: move |e| paste_value.set(e.value()),
                                }
                            },
                            ImportSourceTab::Url => rsx! {
                                input {
                                    class: "kn-modal-input skill-source-input",
                                    placeholder: "https://github.com/owner/repo or a raw SKILL.md / .zip URL",
                                    value: "{url_value_val}",
                                    oninput: move |e| url_value.set(e.value()),
                                }
                            },
                            // Phase 49.4: a real file picker on the operator's
                            // OWN machine. The bytes are read in the browser and
                            // staged server-side; from there the normal
                            // preview/install flow takes over unchanged.
                            ImportSourceTab::Upload => rsx! {
                                input {
                                    r#type: "file",
                                    class: "kn-modal-input skill-source-input",
                                    accept: ".zip,.md",
                                    disabled: uploading_val,
                                    onchange: move |evt| {
                                        // Dioxus 0.7: `files()` yields `Vec<FileData>`;
                                        // one file is all this flow accepts.
                                        let files = evt.files();
                                        let Some(file) = files.first().cloned() else { return };
                                        let name = file.name();
                                        upload_error.set(None);
                                        upload_staged_path.set(String::new());
                                        upload_filename.set(name.clone());
                                        uploading.set(true);
                                        spawn(async move {
                                            let Ok(bytes) = file.read_bytes().await else {
                                                uploading.set(false);
                                                upload_error.set(Some(
                                                    "Could not read that file from your machine.".to_string(),
                                                ));
                                                return;
                                            };
                                            match stage_uploaded_skill(name.clone(), bytes.to_vec()).await {
                                                Ok(path) => {
                                                    uploading.set(false);
                                                    upload_staged_path.set(path);
                                                }
                                                Err(e) => {
                                                    uploading.set(false);
                                                    upload_error.set(Some(e.to_string()));
                                                }
                                            }
                                        });
                                    },
                                }
                                div { class: "kn-modal-hint--info",
                                    "Choose a .zip skill bundle or a SKILL.md from this computer. It uploads to the agent server, then PREVIEW shows what will be installed."
                                }
                                if uploading_val {
                                    div { class: "kn-drawer-loading", "Uploading {upload_filename_val}…" }
                                } else if let Some(ref reason) = upload_error_val {
                                    div { class: "kn-modal-error", "{reason}" }
                                } else if !upload_staged_path_val.is_empty() {
                                    div { class: "kn-modal-hint--info",
                                        "Uploaded {upload_filename_val} — ready to preview."
                                    }
                                }
                            },
                            ImportSourceTab::LocalPath => rsx! {
                                // Phase 49.4: quick-pick of known skill root dirs
                                // (browsers can't open a native file dialog).
                                // Selecting one prefills the path with a trailing
                                // slash; the operator appends the skill folder name.
                                if !known_dirs.is_empty() {
                                    select {
                                        class: "kn-modal-input skill-source-input",
                                        style: "margin-bottom: 6px;",
                                        onchange: move |e| {
                                            let v = e.value();
                                            if !v.is_empty() {
                                                path_value.set(format!("{v}/"));
                                            }
                                        },
                                        option { value: "", "— quick-pick a known skills dir —" }
                                        for d in known_dirs.iter() {
                                            option { key: "{d}", value: "{d}", "{d}" }
                                        }
                                    }
                                }
                                input {
                                    class: "kn-modal-input skill-source-input",
                                    placeholder: "/srv/skills/my-skill or ~/skills/my-skill",
                                    value: "{path_value_val}",
                                    oninput: move |e| path_value.set(e.value()),
                                }
                            },
                        }}
                        div { class: "kn-modal-actions",
                            button {
                                class: "kn-modal-btn",
                                onclick: move |_| reset_and_close(),
                                "CANCEL"
                            }
                            button {
                                class: "kn-modal-btn kn-modal-btn--submit",
                                disabled: !can_preview,
                                onclick: move |_| {
                                    let source = current_input.clone();
                                    resolved_source.set(source.clone());
                                    step.set(ImportStep::Preview);
                                    preview.set(None);
                                    preview_error.set(None);
                                    preview_loading.set(true);
                                    spawn(async move {
                                        match preview_skill_import(source).await {
                                            Ok(p) => {
                                                preview_loading.set(false);
                                                preview.set(Some(p));
                                            }
                                            Err(e) => {
                                                preview_loading.set(false);
                                                preview_error.set(Some(e.to_string()));
                                            }
                                        }
                                    });
                                },
                                "PREVIEW"
                            }
                        }
                    } else {
                        h3 { class: "kn-modal-title", style: "font-size:14px;", "Review before install" }
                        p { class: "kn-modal-desc",
                            "Skills can execute code on this machine. Confirm this is what you expect."
                        }
                        if preview_loading_val {
                            div { class: "skill-preview-panel",
                                span { class: "icon spin", "◐" }
                                span { " Fetching and parsing…" }
                            }
                        } else if let Some(ref err) = preview_error_val {
                            div { class: "kn-modal-error", "{err}" }
                        } else if let Some(ref p) = preview_val {
                            div { class: "skill-preview-panel",
                                div { "Name: {display_or_dash(&p.name)}" }
                                div { "Description: {display_or_dash(&p.description)}" }
                                div { "Version: {display_or_dash(p.version.as_deref().unwrap_or_default())}" }
                                div { "Tags: {display_or_dash(&p.tags.join(\", \"))}" }
                                div { "Command:" }
                                pre { "{display_or_dash(p.command_block.as_deref().unwrap_or_default())}" }
                            }
                        }
                        if let Some(ref err) = install_error_val {
                            div { class: "kn-modal-error", "{err}" }
                        }
                        div { class: "kn-modal-actions",
                            button {
                                class: "kn-modal-btn",
                                disabled: installing_val,
                                onclick: move |_| step.set(ImportStep::Source),
                                "BACK"
                            }
                            button {
                                class: "kn-modal-btn kn-modal-btn--submit",
                                disabled: !can_install,
                                onclick: move |_| {
                                    let source = resolved_source.read().clone();
                                    installing.set(true);
                                    install_error.set(None);
                                    spawn(async move {
                                        match install_previewed_skill(source).await {
                                            Ok(_) => {
                                                installing.set(false);
                                                step.set(ImportStep::Source);
                                                preview.set(None);
                                                on_installed.call(());
                                                on_close.call(());
                                            }
                                            Err(e) => {
                                                installing.set(false);
                                                install_error.set(Some(format!("Couldn't install skill — {e}.")));
                                            }
                                        }
                                    });
                                },
                                if installing_val { "INSTALLING…" } else { "INSTALL SKILL" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod preview_is_installable_tests {
    use super::*;

    fn preview(name: &str, command_block: Option<&str>) -> SkillImportPreview {
        SkillImportPreview {
            name: name.to_string(),
            description: String::new(),
            version: None,
            tags: vec![],
            command_block: command_block.map(|s| s.to_string()),
            source_label: String::new(),
            trust_tier: String::new(),
        }
    }

    #[test]
    fn name_and_command_block_enables_install() {
        assert!(preview_is_installable(&preview("x", Some("echo hi"))));
    }

    #[test]
    fn missing_command_block_disables_install() {
        assert!(!preview_is_installable(&preview("x", None)));
    }

    #[test]
    fn empty_command_block_disables_install() {
        assert!(!preview_is_installable(&preview("x", Some("   "))));
    }

    #[test]
    fn empty_name_disables_install() {
        assert!(!preview_is_installable(&preview("  ", Some("echo hi"))));
    }

    #[test]
    fn missing_optional_fields_still_installable() {
        let p = preview("x", Some("echo hi"));
        assert!(p.description.is_empty() && p.version.is_none() && p.tags.is_empty());
        assert!(preview_is_installable(&p));
    }
}
