//! Phase 50.1 Plan 04 (D-12): the three-source avatar picker — SHAPE /
//! UPLOAD / GENERATE. Mounted twice: inside the create wizard's Advanced
//! disclosure (`profile_shared/create_dialog.rs`) and inside the bot
//! detail drawer's Avatar section (`profile_shared/edit_dialog.rs`). Both
//! mounts pass the bot's own name and a working `descriptor` signal — this
//! component opens no shared context of its own and reads no ambient
//! state beyond its props.
//!
//! **Layout.** The SHAPE grid and colour row are always rendered — SHAPE
//! is "the default, always available, zero network dependency" source
//! (UI-SPEC §5) — beneath the 64px live preview and the 3-way segmented
//! toggle. Once an image is chosen (upload or generation succeeded), the
//! grid and colour row render `disabled` and visually greyed rather than
//! disappearing, so the operator can still see the geometric fallback
//! they would return to on clearing the image (UI-SPEC E5 partial). The
//! segmented toggle's UPLOAD/GENERATE selection controls only which
//! source-specific panel (dropzone or prompt) renders below the
//! always-visible SHAPE section.
//!
//! **Persistence.** Every selection — a shape cell, a colour swatch, a
//! completed upload, a completed generation — writes the working
//! `descriptor` signal immediately (so the 64px preview updates on every
//! change) AND calls `on_save`, so the caller can persist through the
//! bot-meta save path exactly when it is meaningful to that caller's own
//! lifecycle: the create wizard defers the actual `save_bot_meta` write
//! until the profile itself exists (reading the settled `descriptor` at
//! submit time), while the drawer's mount saves on every change since the
//! bot already exists.
//!
//! Ships exactly three sources. No pixel-companion subsystem, no fourth
//! tab — that decorative subsystem is explicitly out of this phase (D-12).

use crate::components::hermes_app::widgets::bot_face::{
    seeded_color_for, seeded_shape_for, shape_svg_path, BotFace, AVATAR_COLOR_TOKENS,
    AVATAR_PICKER_SHAPES,
};
use crate::protocol::{BotAvatarDescriptor, BotAvatarGenerateRequest, BotAvatarUploadRequest};
use dioxus::prelude::*;

/// UI-SPEC Copywriting Contract, locked verbatim.
// Under `--all-features`, `legacy-shell` swaps the root to WarpHermes,
// leaving HermesApp (and therefore AvatarPicker and everything it calls)
// unreferenced from `main()` — a binary crate's unreferenced item does not
// suppress dead_code, so the `-D warnings` clippy gate fires there even
// though this is live in the default (non-legacy-shell) build. Same
// pattern as `bot_face.rs`'s own `AVATAR_SHAPES` doc note.
#[allow(dead_code)]
const GENERATE_PROMPT_PLACEHOLDER: &str =
    r#"Describe your avatar — e.g. "a calm librarian owl, flat vector style""#;

/// Fixed DOM id for the hidden file input — this crate only ever mounts
/// one `AvatarPicker` at a time (the wizard and the drawer are mutually
/// exclusive surfaces), mirroring `chat.rs`'s own single fixed
/// `CHAT_ATTACH_INPUT_ID` constant rather than a per-instance id.
#[allow(dead_code)] // see GENERATE_PROMPT_PLACEHOLDER's doc note above (legacy-shell reachability)
const AVATAR_UPLOAD_INPUT_ID: &str = "kn-avatar-upload-input";

/// The three avatar sources — no fourth variant, ever (see module doc).
#[allow(dead_code)] // see GENERATE_PROMPT_PLACEHOLDER's doc note above (legacy-shell reachability)
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AvatarSource {
    #[default]
    Shape,
    Upload,
    Generate,
}

/// Resolve the (shape, colour-token) pair the SHAPE section currently
/// shows selected — the descriptor's own stored values when present, else
/// the deterministic seeded default for `name` (UI-SPEC E5 empty: a new
/// bot with no prior choice renders a seeded default, never one
/// hardcoded face for every bot). Pure and disk/DOM-I/O-free.
#[allow(dead_code)] // see GENERATE_PROMPT_PLACEHOLDER's doc note above (legacy-shell reachability)
fn effective_shape_and_color(name: &str, descriptor: Option<&BotAvatarDescriptor>) -> (String, String) {
    let shape = descriptor
        .and_then(|d| d.shape.clone())
        .unwrap_or_else(|| seeded_shape_for(name).to_string());
    let color = descriptor
        .and_then(|d| d.color.clone())
        .unwrap_or_else(|| seeded_color_for(name).to_string());
    (shape, color)
}

/// Whether the descriptor currently carries an image — the SHAPE section's
/// inert/greyed trigger (UI-SPEC E5 partial). Pure and disk/DOM-I/O-free.
#[allow(dead_code)] // see GENERATE_PROMPT_PLACEHOLDER's doc note above (legacy-shell reachability)
fn has_image(descriptor: Option<&BotAvatarDescriptor>) -> bool {
    descriptor.and_then(|d| d.image_id.as_ref()).is_some()
}

/// Phase 50.1 Plan 04: the three-source avatar picker. `descriptor` is the
/// working copy the caller owns; `on_save` fires on every persist-worthy
/// change (see module doc's Persistence note).
#[component]
pub fn AvatarPicker(
    bot_name: String,
    descriptor: Signal<Option<BotAvatarDescriptor>>,
    on_save: EventHandler<BotAvatarDescriptor>,
) -> Element {
    let mut source: Signal<AvatarSource> = use_signal(AvatarSource::default);
    let uploading: Signal<bool> = use_signal(|| false);
    let upload_error: Signal<Option<String>> = use_signal(|| None);
    let generating: Signal<bool> = use_signal(|| false);
    let generate_error: Signal<Option<String>> = use_signal(|| None);
    let mut prompt: Signal<String> = use_signal(String::new);

    let current = descriptor.read().clone();
    let (current_shape, current_color) = effective_shape_and_color(&bot_name, current.as_ref());
    let image_present = has_image(current.as_ref());
    let current_source = *source.read();
    let is_uploading = *uploading.read();
    let is_generating = *generating.read();

    let preview_image_id = current.as_ref().and_then(|d| d.image_id.clone());
    let (preview_shape, preview_color) = if image_present {
        (None, None)
    } else {
        (Some(current_shape.clone()), Some(current_color.clone()))
    };

    rsx! {
        div { class: "kn-avatar-picker",
            div { class: "kn-avatar-picker-head",
                BotFace {
                    name: bot_name.clone(),
                    size: 64u32,
                    shape: preview_shape,
                    color_token: preview_color,
                    image_id: preview_image_id,
                }
                div { class: "kn-modal-segmented", role: "tablist", "aria-label": "Avatar source",
                    button {
                        r#type: "button",
                        class: if current_source == AvatarSource::Shape { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                        onclick: move |_| source.set(AvatarSource::Shape),
                        "SHAPE"
                    }
                    button {
                        r#type: "button",
                        class: if current_source == AvatarSource::Upload { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                        onclick: move |_| source.set(AvatarSource::Upload),
                        "UPLOAD"
                    }
                    button {
                        r#type: "button",
                        class: if current_source == AvatarSource::Generate { "kn-modal-seg kn-modal-seg--active" } else { "kn-modal-seg" },
                        onclick: move |_| source.set(AvatarSource::Generate),
                        "GENERATE"
                    }
                }
            }

            // SHAPE section — always rendered (the always-available, zero-
            // network-dependency source), inert-greyed rather than hidden
            // once an image is chosen (UI-SPEC E5 partial).
            div {
                class: if image_present { "kn-avatar-shape-grid kn-avatar-shape-grid--inert" } else { "kn-avatar-shape-grid" },
                "aria-disabled": if image_present { "true" } else { "false" },
                for shape in AVATAR_PICKER_SHAPES {
                    {
                        let shape_owned = shape.to_string();
                        let color_for_render = current_color.clone();
                        let color_for_click = current_color.clone();
                        let is_selected = !image_present && current_shape == shape;
                        let mut descriptor_sig = descriptor;
                        let path_d = shape_svg_path(shape);
                        rsx! {
                            button {
                                key: "{shape}",
                                r#type: "button",
                                class: if is_selected { "kn-avatar-shape-cell kn-avatar-shape-cell--selected" } else { "kn-avatar-shape-cell" },
                                "aria-pressed": if is_selected { "true" } else { "false" },
                                "aria-label": "{shape} shape",
                                disabled: image_present,
                                onclick: move |_| {
                                    let new_descriptor = BotAvatarDescriptor {
                                        kind: "generated".to_string(),
                                        shape: Some(shape_owned.clone()),
                                        color: Some(color_for_click.clone()),
                                        image_id: None,
                                    };
                                    descriptor_sig.set(Some(new_descriptor.clone()));
                                    on_save.call(new_descriptor);
                                },
                                svg {
                                    "viewBox": "0 0 40 40",
                                    width: "44",
                                    height: "44",
                                    "aria-hidden": "true",
                                    path { d: "{path_d}", fill: "var({color_for_render})" }
                                }
                            }
                        }
                    }
                }
            }
            div {
                class: if image_present { "kn-avatar-color-row kn-avatar-color-row--inert" } else { "kn-avatar-color-row" },
                "aria-disabled": if image_present { "true" } else { "false" },
                for token in AVATAR_COLOR_TOKENS {
                    {
                        let token_owned = token.to_string();
                        let shape_for_click = current_shape.clone();
                        let is_selected = !image_present && current_color == token;
                        let mut descriptor_sig = descriptor;
                        let swatch_style = format!("background: var({token});");
                        rsx! {
                            button {
                                key: "{token}",
                                r#type: "button",
                                class: if is_selected { "kn-avatar-color-swatch kn-avatar-color-swatch--selected" } else { "kn-avatar-color-swatch" },
                                "aria-pressed": if is_selected { "true" } else { "false" },
                                "aria-label": "identity colour {token}",
                                disabled: image_present,
                                style: "{swatch_style}",
                                onclick: move |_| {
                                    let new_descriptor = BotAvatarDescriptor {
                                        kind: "generated".to_string(),
                                        shape: Some(shape_for_click.clone()),
                                        color: Some(token_owned.clone()),
                                        image_id: None,
                                    };
                                    descriptor_sig.set(Some(new_descriptor.clone()));
                                    on_save.call(new_descriptor);
                                },
                            }
                        }
                    }
                }
            }

            if current_source == AvatarSource::Upload {
                div { class: "kn-avatar-upload-dropzone",
                    label {
                        class: "kn-action-btn",
                        r#for: "{AVATAR_UPLOAD_INPUT_ID}",
                        "aria-label": "Choose an image",
                        if is_uploading { "UPLOADING…" } else { "CHOOSE IMAGE" }
                    }
                    input {
                        id: "{AVATAR_UPLOAD_INPUT_ID}",
                        class: "kn-avatar-upload-input-hidden",
                        r#type: "file",
                        accept: "image/png,image/jpeg,image/gif,image/webp",
                        disabled: is_uploading,
                        onchange: move |evt: FormEvent| {
                            let files = evt.files();
                            let Some(file) = files.into_iter().next() else {
                                return;
                            };
                            let bot_name_owned = bot_name.clone();
                            let mut uploading_sig = uploading;
                            let mut upload_error_sig = upload_error;
                            let mut descriptor_sig = descriptor;
                            let save = on_save;
                            uploading_sig.set(true);
                            upload_error_sig.set(None);
                            spawn(async move {
                                let bytes = match file.read_bytes().await {
                                    Ok(b) => b,
                                    Err(_) => {
                                        uploading_sig.set(false);
                                        upload_error_sig.set(Some(
                                            "Could not read that image. Try a PNG or JPG under 5MB.".to_string(),
                                        ));
                                        return;
                                    }
                                };
                                use base64::Engine as _;
                                let content_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                                let req = BotAvatarUploadRequest { bot_name: bot_name_owned, content_b64 };
                                match crate::server::bot_avatar_api::upload_bot_avatar(req).await {
                                    Ok(saved) => {
                                        uploading_sig.set(false);
                                        let new_descriptor = BotAvatarDescriptor {
                                            kind: "upload".to_string(),
                                            shape: None,
                                            color: None,
                                            image_id: Some(saved.image_id),
                                        };
                                        descriptor_sig.set(Some(new_descriptor.clone()));
                                        save.call(new_descriptor);
                                    }
                                    Err(e) => {
                                        uploading_sig.set(false);
                                        upload_error_sig.set(Some(format!("{e}")));
                                    }
                                }
                            });
                        },
                    }
                    if let Some(err) = upload_error.read().clone() {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
            } else if current_source == AvatarSource::Generate {
                div { class: "kn-avatar-generate",
                    input {
                        class: "kn-modal-input",
                        placeholder: GENERATE_PROMPT_PLACEHOLDER,
                        value: "{prompt.read()}",
                        disabled: is_generating,
                        oninput: move |evt| prompt.set(evt.value()),
                    }
                    button {
                        class: "kn-modal-btn",
                        disabled: is_generating || prompt.read().trim().is_empty(),
                        onclick: move |_| {
                            let bot_name_owned = bot_name.clone();
                            let prompt_owned = prompt.read().clone();
                            let mut generating_sig = generating;
                            let mut generate_error_sig = generate_error;
                            let mut descriptor_sig = descriptor;
                            let save = on_save;
                            generating_sig.set(true);
                            generate_error_sig.set(None);
                            spawn(async move {
                                let req = BotAvatarGenerateRequest { bot_name: bot_name_owned, prompt: prompt_owned };
                                match crate::server::bot_avatar_api::generate_bot_avatar(req).await {
                                    Ok(saved) => {
                                        generating_sig.set(false);
                                        let new_descriptor = BotAvatarDescriptor {
                                            kind: "generated-image".to_string(),
                                            shape: None,
                                            color: None,
                                            image_id: Some(saved.image_id),
                                        };
                                        descriptor_sig.set(Some(new_descriptor.clone()));
                                        save.call(new_descriptor);
                                    }
                                    Err(e) => {
                                        generating_sig.set(false);
                                        generate_error_sig.set(Some(format!("{e}")));
                                    }
                                }
                            });
                        },
                        if is_generating { "GENERATING…" } else { "GENERATE" }
                    }
                    if let Some(err) = generate_error.read().clone() {
                        div { class: "kn-modal-error", "{err}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_shape_and_color_with_no_descriptor_falls_back_to_seeded_default() {
        let (shape, color) = effective_shape_and_color("scout", None);
        assert_eq!(shape, seeded_shape_for("scout"));
        assert_eq!(color, seeded_color_for("scout"));
    }

    #[test]
    fn effective_shape_and_color_prefers_the_stored_descriptor() {
        let descriptor = BotAvatarDescriptor {
            kind: "generated".to_string(),
            shape: Some("triangle".to_string()),
            color: Some("--danger".to_string()),
            image_id: None,
        };
        let (shape, color) = effective_shape_and_color("scout", Some(&descriptor));
        assert_eq!(shape, "triangle");
        assert_eq!(color, "--danger");
    }

    #[test]
    fn has_image_true_only_when_image_id_is_some() {
        assert!(!has_image(None));
        let no_image = BotAvatarDescriptor {
            kind: "generated".to_string(),
            shape: Some("circle".to_string()),
            color: Some("--accent-primary".to_string()),
            image_id: None,
        };
        assert!(!has_image(Some(&no_image)));
        let with_image = BotAvatarDescriptor {
            kind: "upload".to_string(),
            shape: None,
            color: None,
            image_id: Some("abc123".to_string()),
        };
        assert!(has_image(Some(&with_image)));
    }
}
