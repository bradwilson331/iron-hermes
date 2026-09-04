//! Phase 49.4.1 (D-04/D-05/D-07/D-10/D-11): the shared `SECRETS SOURCE`
//! picker — ONE component (D-05's "ONE component, TWO mounts" rule), mounted
//! from the profile detail drawer (Plan 01) and the create wizard's step 2
//! (Plan 02). This plan (Plan 03) fills the fourth row's body — a drop
//! target plus click-to-choose fallback and the multi-row manual-key list —
//! so "Provided keys" is fully usable at both mounts from this one
//! component.
//!
//! Row anatomy rhymes with `create_dialog.rs`'s `KeyModeKind` rows (dot +
//! title + helper + right-aligned pill), but the section eyebrow, helper
//! copy, and pill vocabulary are entirely distinct from that pre-existing
//! breadth control — that separation IS the D-07 fix (UI-SPEC "Copywriting
//! Contract" / "The four secrets-source labels").
//!
//! ALL hooks register unconditionally on every render — the
//! `GatewayScopeSelector` discipline (`screens/gateway/mod.rs:290`), which
//! this crate has already been bitten by twice (an inline dropdown that
//! rendered blank; a self-retriggering effect loop). Availability (including
//! the vault reason) ships with the same payload the caller already has —
//! per UI-SPEC E1/E2, "no in-flight state" — so this component registers no
//! fetch-backed hook of its own; the drag/read/error signals below are
//! purely local interaction state, not fetch-backed.

use crate::protocol::SecretSource;
use dioxus::html::{FileData, HasFileData};
use dioxus::prelude::*;

/// UI-SPEC Copywriting Contract eyebrow — deliberately NOT a restatement of
/// the pre-existing `KEY INHERITANCE — FROM ~/.IRONHERMES/.ENV` label (the
/// `KeyMode` breadth control). This separation is the D-07 fix.
pub(crate) const SECRETS_SOURCE_SECTION_LABEL: &str = "SECRETS SOURCE";

/// D-10 vault-disabled reason strings (UI-SPEC "D-10 vault-disabled reason
/// string(s)"). This plan (Task 2) passes `vault_available: false` with the
/// reason computed inline from `cfg!(feature = "rusty-vault")`; Plan 02
/// replaces that with the server-computed `SecretsSourceAvailability`
/// payload without changing this component's props.
pub(crate) const VAULT_REASON_BUILD_LACKS_FEATURE: &str =
    "This build was compiled without vault support.";
pub(crate) const VAULT_REASON_DISABLED_IN_CONFIG: &str =
    "Vault is disabled in config.yaml (vault.enabled: false).";

/// Phase 49.4.1 Plan 03 (D-04/D-11): the "Provided keys" row's drop target —
/// stable DOM id shared by the hidden `<input type="file">` and its paired
/// `label[for]`.
pub(crate) const PROVIDED_KEYS_INPUT_ID: &str = "kn-provided-keys-file-input";

/// UI-SPEC Copywriting Contract — the drop target's at-rest copy, declared
/// once as named constants so this one mount cannot drift from itself
/// across the two callers.
pub(crate) const DROPZONE_PROMPT_HEADING: &str = "Drop a .env file here";
pub(crate) const DROPZONE_PROMPT_BODY: &str =
    "or click to choose a file — KEY=value per line, .env format";

/// A browser-side file-read failure (permissions, an in-flight abort) —
/// distinct from [`crate::server::profile_api::UPLOADED_DOTENV_PARSE_ERROR`]
/// (a server-side parse rejection), which is native-only and unreachable
/// from this wasm-compiled component. Never interpolates anything from the
/// file.
const PROVIDED_FILE_READ_ERROR: &str = "Could not read that file. Try again.";

/// UI-SPEC E4: a blank-but-typed-into name, or two rows sharing a non-blank
/// name, both reject inline — never the value. A row where BOTH the name and
/// value are blank is not an error (UI-SPEC E4 "Partial/incomplete": blank
/// rows are dropped on submit rather than raised as errors).
pub(crate) fn manual_keys_inline_error(rows: &[(String, String)]) -> Option<String> {
    let mut seen_names = std::collections::HashSet::new();
    for (name, value) in rows {
        let name_trim = name.trim();
        let value_trim = value.trim();
        if name_trim.is_empty() {
            if !value_trim.is_empty() {
                return Some("Key name is required.".to_string());
            }
            continue;
        }
        if !seen_names.insert(name_trim.to_string()) {
            return Some(format!("Duplicate key name '{name_trim}'."));
        }
    }
    None
}

/// Phase 49.4.1 Plan 03: the submit-time filter both mounts share — drops
/// fully- and partially-blank rows (UI-SPEC E4 "blank rows are dropped on
/// submit rather than raised as errors"; a name-blank row can never be a
/// valid entry regardless of its value, so it is dropped the same way).
/// Trims both sides; never rejects, only filters.
pub(crate) fn submit_ready_manual_keys(rows: &[(String, String)]) -> Vec<(String, String)> {
    rows.iter()
        .filter_map(|(name, value)| {
            let name_trim = name.trim();
            let value_trim = value.trim();
            if name_trim.is_empty() || value_trim.is_empty() {
                None
            } else {
                Some((name_trim.to_string(), value_trim.to_string()))
            }
        })
        .collect()
}

/// Best-effort client-side (wasm) `KEY=value` line split, used ONLY to
/// populate the manual-key rows for editing after
/// [`crate::server::profile_api::parse_provided_keys_file`] has ALREADY
/// confirmed the file parses cleanly with the real `dotenvy` reader. This is
/// NOT the source of truth for well-formedness — the server call above is —
/// so this fn only needs to recover the plaintext pairs the browser already
/// holds locally (never re-uploaded for display) well enough to seed
/// editable rows; any residual disagreement is caught again by
/// `validate_key_name`/`validate_key_value` at submit time, exactly as a
/// typed row already is.
fn split_dotenv_lines_for_display(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let (name, raw_value) = trimmed.split_once('=')?;
            let name = name.trim().to_string();
            let mut value = raw_value.trim();
            if value.len() >= 2
                && ((value.starts_with('\'') && value.ends_with('\''))
                    || (value.starts_with('"') && value.ends_with('"')))
            {
                value = &value[1..value.len() - 1];
            }
            Some((name, value.to_string()))
        })
        .collect()
}

/// Phase 49.4.1 Plan 03 (D-04/D-11, UI-SPEC E3 backstop): shared by the drop
/// handler and the click-to-choose change handler — both read bytes the same
/// way and feed this one fn, so there is exactly one upload code path, never
/// two. Ignores the call entirely (no-op) if a read is already in flight, so
/// two concurrent reads can never race to populate the manual-keys list.
fn spawn_provided_keys_upload(
    file: FileData,
    mut reading_file: Signal<bool>,
    mut upload_error: Signal<Option<String>>,
    mut manual_keys: Signal<Vec<(String, String)>>,
) {
    if *reading_file.peek() {
        return;
    }
    reading_file.set(true);
    upload_error.set(None);
    spawn(async move {
        let bytes = match file.read_bytes().await {
            Ok(b) => b,
            Err(_) => {
                reading_file.set(false);
                upload_error.set(Some(PROVIDED_FILE_READ_ERROR.to_string()));
                return;
            }
        };
        match crate::server::profile_api::parse_provided_keys_file(bytes.to_vec()).await {
            Ok(_rows) => {
                // The client already holds the plaintext it uploaded — the
                // server call above exists to CONFIRM the parse and drive
                // validation, not to hand plaintext back across the wire
                // (T-49.4.1-11). Populate the rows from what was read
                // locally.
                let text = String::from_utf8_lossy(&bytes).into_owned();
                manual_keys.set(split_dotenv_lines_for_display(&text));
                upload_error.set(None);
                reading_file.set(false);
            }
            Err(e) => {
                // `e`'s Display is exactly the server's returned message —
                // the locked UPLOADED_DOTENV_PARSE_ERROR string on a parse
                // failure, or a validation message naming only a key name.
                // Never a raw file line, never a value.
                upload_error.set(Some(format!("{e}")));
                reading_file.set(false);
            }
        }
    });
}

/// UI-SPEC "The four secrets-source labels" table — (title, helper, pill).
/// `Vault`'s helper here is the AVAILABLE-state copy only; the D-10
/// disabled-state reason is substituted into the same helper-line slot by
/// the caller/component, never appended alongside it.
pub(crate) fn secret_source_row_copy(
    source: SecretSource,
) -> (&'static str, &'static str, &'static str) {
    match source {
        SecretSource::RootEnv => (
            "Root .env",
            "Read from ~/.ironhermes/.env — what this wizard already inherits from today.",
            "default",
        ),
        SecretSource::ContainerEnv => (
            "Container environment",
            "Read from this process's own environment variables (filtered to *_API_KEY / *_KEY / *_TOKEN, per D-09).",
            "env",
        ),
        SecretSource::Vault => ("Vault", "Read from the RustyVault secret store.", "vault"),
        SecretSource::Provided => (
            "Provided keys",
            "Enter keys manually below, or drop a .env-shaped file.",
            "manual",
        ),
    }
}

/// The four-row exclusive single-select (D-04) — never a cascade. `source`
/// is the caller's working-copy signal; this component never owns the
/// selection, only renders it and calls `.set()` on click (mirrors
/// `GatewayScopeSelector` taking a `Signal<T>` rather than owning global
/// state). `vault_available`/`vault_reason` arrive with the same payload
/// that renders the rest of the section (E1/E2: no separate loading state).
/// `disabled` covers the whole picker (e.g. while a sync/create is in
/// flight, or writes are disabled) — the Vault row is additionally disabled
/// whenever `!vault_available`, independent of this prop.
///
/// Phase 49.4.1 Plan 03 (D-04/D-05/D-11): `manual_keys` is the Provided-keys
/// row's working copy — the caller's `Signal<Vec<(String, String)>>`,
/// threaded out so `create_dialog.rs` can put it on
/// `CreateProfileRequest.manual_keys` and `edit_dialog.rs` on
/// `SyncProfileSecretsRequest.manual_keys`. Typed rows and a dropped/chosen
/// file both populate this SAME signal — one key-carrying path, not two.
#[component]
pub fn SecretsSourcePicker(
    source: Signal<SecretSource>,
    vault_available: bool,
    vault_reason: Option<String>,
    disabled: bool,
    manual_keys: Signal<Vec<(String, String)>>,
) -> Element {
    // Registered unconditionally on every render (Pattern E) — purely local
    // interaction state, never fetch-backed (E1/E2/E3: availability and the
    // rows themselves carry no separate loading state of their own).
    let mut file_drag_active: Signal<bool> = use_signal(|| false);
    let reading_file: Signal<bool> = use_signal(|| false);
    let upload_error: Signal<Option<String>> = use_signal(|| None);

    let current = *source.read();

    rsx! {
        label { class: "kn-modal-label", "{SECRETS_SOURCE_SECTION_LABEL}" }
        for row_source in [
            SecretSource::RootEnv,
            SecretSource::ContainerEnv,
            SecretSource::Vault,
            SecretSource::Provided,
        ] {
            {
                let (title, helper, pill) = secret_source_row_copy(row_source);
                let is_selected = current == row_source;
                let is_vault_row = row_source == SecretSource::Vault;
                let vault_disabled = is_vault_row && !vault_available;
                let row_disabled = disabled || vault_disabled;
                let helper_text = if vault_disabled {
                    vault_reason.clone().unwrap_or_default()
                } else {
                    helper.to_string()
                };
                let dot_style = if is_selected && !row_disabled {
                    "color: var(--accent-primary);"
                } else {
                    "color: var(--fg-dim);"
                };
                rsx! {
                    div {
                        key: "{pill}",
                        style: "display: flex; align-items: center; gap: var(--sp-2); padding: var(--sp-2) 0; cursor: pointer;",
                        onclick: move |_| {
                            if !row_disabled {
                                source.set(row_source);
                            }
                        },
                        span {
                            "aria-hidden": "true",
                            style: "{dot_style}",
                            if is_selected && !row_disabled { "\u{25cf}" } else { "\u{25cb}" }
                        }
                        div { style: "flex: 1 1 auto;",
                            div { style: "font-size: var(--fs-13); color: var(--fg);", "{title}" }
                            // D-10: the vault-disabled reason renders via the
                            // exact `.kn-modal-hint--info` shape DELETE BOT
                            // already establishes (`edit_dialog.rs:986-998`)
                            // — never a tooltip, never silently omitted.
                            // Every other row's helper line stays the plain
                            // `KeyModeKind`-anatomy inline style.
                            if vault_disabled {
                                div { class: "kn-modal-hint--info", "{helper_text}" }
                            } else {
                                div { style: "font-size: var(--fs-11); color: var(--fg-dim);", "{helper_text}" }
                            }
                        }
                        if !vault_disabled {
                            span { style: "font-size: var(--fs-11); color: var(--accent-primary);", "{pill}" }
                        }
                    }
                }
            }
        }
        // Phase 49.4.1 Plan 03 (D-04/D-05/D-11): the Provided-keys row body —
        // drop target + click-to-choose + manual key rows — renders ONLY
        // while that source is the selected one, so both mounts get it from
        // this one component and neither carries its own copy.
        if current == SecretSource::Provided {
            div { style: "padding-top: var(--sp-2); display: flex; flex-direction: column; gap: var(--sp-2);",
                div {
                    class: "kn-dropzone",
                    "data-drop-active": if *file_drag_active.read() { "true" },
                    ondragover: move |evt: DragEvent| {
                        evt.prevent_default();
                        if !disabled {
                            file_drag_active.set(true);
                        }
                    },
                    ondragleave: move |_| {
                        file_drag_active.set(false);
                    },
                    ondrop: move |evt: DragEvent| {
                        evt.prevent_default();
                        file_drag_active.set(false);
                        if disabled {
                            return;
                        }
                        // `evt.files()` — the FileEngine accessor, NOT
                        // `evt.data_transfer().files()`, which does not
                        // survive the async `read_bytes()` call in some
                        // browsers (chat.rs's own precedent, verbatim).
                        let files = evt.files();
                        if let Some(file) = files.into_iter().next() {
                            spawn_provided_keys_upload(file, reading_file, upload_error, manual_keys);
                        }
                    },
                    if *reading_file.read() {
                        div { style: "font-size: var(--fs-13); color: var(--fg);", "Reading\u{2026}" }
                    } else {
                        div { style: "font-size: var(--fs-13); color: var(--fg);", "{DROPZONE_PROMPT_HEADING}" }
                        div { style: "font-size: var(--fs-11); color: var(--fg-dim);", "{DROPZONE_PROMPT_BODY}" }
                    }
                    label {
                        class: "kn-action-btn",
                        r#for: "{PROVIDED_KEYS_INPUT_ID}",
                        "aria-label": "Choose a .env file",
                        if *reading_file.read() { "READING\u{2026}" } else { "CHOOSE FILE" }
                    }
                    input {
                        id: "{PROVIDED_KEYS_INPUT_ID}",
                        style: "position: absolute; width: 1px; height: 1px; padding: 0; margin: -1px; overflow: hidden; clip: rect(0, 0, 0, 0); white-space: nowrap;",
                        r#type: "file",
                        accept: ".env,text/plain",
                        disabled: disabled || *reading_file.read(),
                        onchange: move |evt: FormEvent| {
                            let files = evt.files();
                            if let Some(file) = files.into_iter().next() {
                                spawn_provided_keys_upload(file, reading_file, upload_error, manual_keys);
                            }
                        },
                    }
                    if *file_drag_active.read() {
                        div { class: "kn-drop-overlay", span { "Drop to import" } }
                    }
                }
                if let Some(err) = upload_error.read().clone() {
                    div { class: "kn-modal-error", "{err}" }
                }
                label { class: "kn-modal-label", "MANUAL KEYS" }
                {
                    let rows = manual_keys.read().clone();
                    rsx! {
                        for (idx , (row_name , row_value)) in rows.iter().cloned().enumerate() {
                            div {
                                key: "{idx}",
                                style: "display: flex; align-items: center; gap: var(--sp-2);",
                                input {
                                    class: "kn-key-input",
                                    style: "flex: 1 1 auto; min-width: 0;",
                                    placeholder: "KEY_NAME",
                                    title: "{row_name}",
                                    value: "{row_name}",
                                    disabled,
                                    oninput: move |evt| {
                                        let mut list = manual_keys.write();
                                        if let Some(entry) = list.get_mut(idx) {
                                            entry.0 = evt.value();
                                        }
                                    },
                                }
                                input {
                                    class: "kn-key-input",
                                    style: "flex: 1 1 auto; min-width: 0;",
                                    r#type: "password",
                                    placeholder: "value",
                                    value: "{row_value}",
                                    disabled,
                                    oninput: move |evt| {
                                        let mut list = manual_keys.write();
                                        if let Some(entry) = list.get_mut(idx) {
                                            entry.1 = evt.value();
                                        }
                                    },
                                }
                                button {
                                    class: "kn-action-btn",
                                    r#type: "button",
                                    disabled,
                                    "aria-label": "Remove this key row",
                                    onclick: move |_| {
                                        let mut list = manual_keys.write();
                                        if idx < list.len() {
                                            list.remove(idx);
                                        }
                                    },
                                    "\u{2715}"
                                }
                            }
                        }
                    }
                }
                button {
                    class: "kn-action-btn",
                    r#type: "button",
                    disabled,
                    onclick: move |_| {
                        manual_keys.write().push((String::new(), String::new()));
                    },
                    "+ ADD KEY"
                }
                if let Some(err) = manual_keys_inline_error(&manual_keys.read()) {
                    div { class: "kn-modal-error", "{err}" }
                }
            }
        }
    }
}
