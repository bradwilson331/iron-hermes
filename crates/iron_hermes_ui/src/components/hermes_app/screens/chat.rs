//! Phase 26.2.1 Plan 06 — Chat screen + chat-protocol UI types.
//!
//! This file owns the small data types consumed by the WebSocket receive
//! loop in `hermes_app/mod.rs` (`ChatBubble`, `ChatBubbleKind`, `ToolRow`)
//! plus the `ChatSendHandler` newtype that disambiguates the send-fn
//! context provider from any future `EventHandler<String>` in the tree.
//!
//! Plan 06 Task 1 (this commit) introduces the types so `mod.rs`'s hoisted
//! `use_websocket` + receive loop can reference them. Task 2 then
//! replaces the placeholder `ScreenChat` body with the full
//! `chat-mini` + `chat-stream` + `chat-input-pill` layout that consumes
//! these types via context.

#[cfg(target_arch = "wasm32")]
use dioxus::core::use_drop;
use dioxus::html::FileData;
use dioxus::html::HasFileData;
use dioxus::prelude::*;

// Phase 46.7 Plan 05 (D-06/D-09/D-11): the wire type for one uploaded
// chat-attachment row — shared, unconditional module (protocol.rs), so it is
// safe to use directly in this WASM-client-compiled file.
use crate::protocol::ChatAttachmentRow;

/// Assistant avatar — copper low-poly wings logo used on agent chat bubbles.
// The chat-screen types below (AVATAR_LOGO, ChatBubbleKind, ToolRow, ChatBubble +
// its constructors) are all consumed by the HermesApp subtree (mod.rs receive
// loop / send handler construct ChatBubble at 204/246/302/356/369, ToolRow at
// 212; ScreenChat renders AVATAR_LOGO). In a binary crate `pub` does not keep
// unreferenced items alive, so under `--all-features` — where `legacy-shell`
// mounts WarpHermes instead of HermesApp — this whole cluster reads as dead.
// Live in the default (non-legacy-shell) build; hence item-level allows below.
#[allow(dead_code)]
const AVATAR_LOGO: Asset = asset!("/assets/i_hermes_logo.png");

// ---------------------------------------------------------------------------
// Chat UI primitives (Plan 06)
// ---------------------------------------------------------------------------

/// Bubble role — drives the CSS class that selects user / assistant /
/// error styling. Maps 1:1 to `chat-msg.user`, `chat-msg.assistant`, and
/// an error variant rendered as `chat-bubble is-error`.
// dead under --all-features/legacy-shell; see AVATAR_LOGO note above.
#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug)]
pub enum ChatBubbleKind {
    User,
    Assistant,
    Error,
    // Phase 26.7.2 D-02: history/live boundary marker — renders as a section-label rule, no avatar, no bubble body
    Divider,
    // Phase 36.17.7 D-02-a/b/f: inline audio control bubble. `audio_src` holds the
    // Blob URL created from the binary WS frame; `audio_mime` is typically "audio/mpeg".
    Audio,
    // Phase 01-04 (DLV-03 web): inline generated-image bubble. `image_src` holds the
    // Blob URL created from the ImageOut binary WS frame; renders as an <img>.
    Image,
    // Phase 36.3.3 (D-08 web): inline generated-video bubble. `video_src` holds the
    // Blob URL created from the VideoOut binary WS frame; renders as a <video controls>.
    Video,
    // Phase 41.1 Plan 03 (SKILL-13 web / D-06, UI-SPEC §C): DIM run-turn meta
    // chip ("▶ Ran skill /<name>") preceding a one-shot skill run's reply.
    // Renders as a `.chat-meta` row — no avatar, no bubble background, DIM text
    // (metadata, not a message); modeled on the `Divider` no-bubble marker.
    Meta,
}

/// One tool-call progress row inside an assistant bubble.
///
/// `done` flips to `true` when the matching `ChatStreamEvent::ToolCallEnd`
/// arrives; `success` carries the server-reported outcome. Renders as
/// `.chat-progress-row.is-running` / `.is-done.is-success` / `.is-done.is-error`.
// dead under --all-features/legacy-shell; see AVATAR_LOGO note above.
#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug)]
pub struct ToolRow {
    pub name: String,
    pub args: String,
    pub done: bool,
    pub success: bool,
}

/// One bubble in the chat stream — user, assistant, or error.
///
/// `tool_rows` is mutated in-place by the receive loop in `mod.rs` when
/// `ChatStreamEvent::ToolCallStart` / `ToolCallEnd` arrive for the
/// currently-streaming assistant bubble.
// dead under --all-features/legacy-shell; see AVATAR_LOGO note above.
#[allow(dead_code)]
#[derive(Clone, PartialEq, Debug)]
pub struct ChatBubble {
    pub id: u64,
    pub kind: ChatBubbleKind,
    pub text: String,
    pub tool_rows: Vec<ToolRow>,
    // Phase 36.17.7 D-02-a: Blob URL produced by the WS Binary recv arm
    // (from create_object_url_with_blob) plus MIME type for the <audio> tag.
    pub audio_src: Option<String>,
    pub audio_mime: Option<String>,
    // Phase 01-04 (DLV-03 web): Blob URL produced by the ImageOut WS Binary
    // recv arm (from create_object_url_with_blob) for the inline <img> tag.
    pub image_src: Option<String>,
    // Phase 36.3.3 (D-08 web): Blob URL produced by the VideoOut WS Binary
    // recv arm (from create_object_url_with_blob) for the inline <video controls> tag.
    pub video_src: Option<String>,
    // Phase 46.7 Plan 05 (D-06/D-07/D-11): attachments carried by this bubble —
    // populated on the SEND side (composer's pending-attached rows at submit
    // time, via `ChatBubble::user_with_attachments`) and never mutated
    // afterward. Empty for every non-user bubble kind and for user bubbles
    // sent before this plan's UI existed.
    pub attachments: Vec<ChatAttachmentRow>,
}

// constructors dead under --all-features/legacy-shell; see AVATAR_LOGO note above.
#[allow(dead_code)]
impl ChatBubble {
    pub fn user(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::User,
            text,
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: None,
            attachments: vec![],
        }
    }
    /// Phase 46.7 Plan 05 (D-06/D-07): a user bubble carrying attachments sent
    /// alongside (or instead of) text. Constructed by `hermes_app/mod.rs`'s
    /// send handler at submit time from the composer's pending-attached rows.
    pub fn user_with_attachments(
        id: u64,
        text: String,
        attachments: Vec<ChatAttachmentRow>,
    ) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::User,
            text,
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: None,
            attachments,
        }
    }
    pub fn assistant(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Assistant,
            text,
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: None,
            attachments: vec![],
        }
    }
    pub fn error(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Error,
            text,
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: None,
            attachments: vec![],
        }
    }
    /// Phase 36.17.7 D-02-a/b: audio bubble carrying a Blob URL and MIME.
    /// Constructed by HermesApp's Binary recv arm after deserializing
    /// `ChatStreamEvent::AudioOut` and calling `create_object_url_with_blob`.
    pub fn audio(id: u64, src: String, mime: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Audio,
            text: String::new(),
            tool_rows: vec![],
            audio_src: Some(src),
            audio_mime: Some(mime),
            image_src: None,
            video_src: None,
            attachments: vec![],
        }
    }
    /// Phase 01-04 (DLV-03 web): image bubble carrying a Blob URL.
    /// Constructed by HermesApp's Binary recv arm after deserializing
    /// `ChatStreamEvent::ImageOut` and calling `create_object_url_with_blob`.
    pub fn image(id: u64, src: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Image,
            text: String::new(),
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: Some(src),
            video_src: None,
            attachments: vec![],
        }
    }
    /// Phase 36.3.3 (D-08 web): video bubble carrying a Blob URL.
    /// Constructed by HermesApp's Binary recv arm after deserializing
    /// `ChatStreamEvent::VideoOut` and calling `create_object_url_with_blob`.
    pub fn video(id: u64, src: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Video,
            text: String::new(),
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: Some(src),
            attachments: vec![],
        }
    }
    /// Phase 41.1 Plan 03 (SKILL-13 web / D-06, UI-SPEC §C): DIM run-turn meta
    /// chip. Constructed by HermesApp's `ChatStreamEvent::RunTurnMeta` arm from
    /// the server-formatted chip copy (`▶ Ran skill /<name>` …). Carries only
    /// `text`; no avatar, no bubble body, no attachments.
    pub fn meta(id: u64, text: String) -> Self {
        Self {
            id,
            kind: ChatBubbleKind::Meta,
            text,
            tool_rows: vec![],
            audio_src: None,
            audio_mime: None,
            image_src: None,
            video_src: None,
            attachments: vec![],
        }
    }
}

/// Newtype wrapper around the chat send handler so `use_context` lookup
/// stays unambiguous — any future `EventHandler<...>` provider would
/// otherwise collide.
///
/// `Copy` is required so consumers can `let send = use_context::<ChatSendHandler>();`
/// inside RSX closures without manual cloning.
///
/// Phase 46.7 Plan 05 (D-05/D-06/D-07): the handler now carries a second
/// tuple element — the sent attachment rows (settled `ChatAttachmentRow`s
/// from the composer's pending-attached list at submit time), alongside the
/// message text. `mod.rs`'s send closure maps `.id` for `ChatRequest.
/// attachment_ids` and uses the full rows for the sent bubble's D-06 render
/// (`ChatBubble::user_with_attachments`). An attachment-only send (D-07)
/// passes an empty text string with a non-empty attachment vec.
#[allow(dead_code)] // context-provider newtype; consumed via use_context::<ChatSendHandler>() in ScreenChat
#[derive(Clone, Copy)]
pub struct ChatSendHandler(pub EventHandler<(String, Vec<ChatAttachmentRow>)>);

// ---------------------------------------------------------------------------
// Phase 46.7 Plan 05 (D-05/D-06/D-08): composer attachment upload state
// ---------------------------------------------------------------------------
//
// Ports the 46.4 kanban-card attachment state machine (card.rs LocalUpload/
// UploadStatus/spawn_attach_call/spawn_attachment_reads) — only the CSS
// token family changes (screens.css, not kanban.css), per the UI-SPEC's
// "port, don't rebuild" directive. Two differences from the kanban original:
//   1. Chat's "settled" state stays CLIENT-SIDE until send (kanban's settled
//      state lives in the server-backed `fetch_attachments` resource because
//      a kanban attachment is immediately associated with its task; a chat
//      attachment is uploaded to the SESSION before the message that will
//      reference it even exists, so `pending_attached` — not a resource
//      restart — is the source of truth for "queued, not yet sent").
//   2. The cap is asymmetric by content type (image vs non-image), mirroring
//      chat_attachments_api.rs's server-side enforcement exactly.

/// Client-side pre-check mirror of `chat_attachments_api::IMAGE_ATTACHMENT_ACCEPT_CAP`
/// (20 MiB) — UX only; the server re-checks after base64 decode (D-09
/// authoritative enforcement, T-46.7-15 mitigation).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CLIENT_IMAGE_MAX_ATTACHMENT_BYTES: usize = 20 * 1024 * 1024;

/// Client-side pre-check mirror of `chat_attachments_api::NONIMAGE_ATTACHMENT_CAP`
/// (10 MiB, `ironhermes_gateway::multimodal::NONIMAGE_MAX_BYTES`) — UX only.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const CLIENT_NONIMAGE_MAX_ATTACHMENT_BYTES: usize = 10 * 1024 * 1024;

/// Phase 46.7 Plan 05: per-file upload state NOT yet reflected in
/// `pending_attached` — covers the `uploading` and `error` states from the
/// UI-SPEC Attachment states contract. The `attached` (settled) state moves
/// OUT of this list into `pending_attached: Signal<Vec<ChatAttachmentRow>>`
/// once `upload_attachment` returns `Ok`.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, PartialEq)]
struct LocalUpload {
    filename: String,
    status: UploadStatus,
    /// Cached base64 payload so the `↻` retry control can re-POST without
    /// re-reading the file (mirrors card.rs — the original FileData handle
    /// is not retained past the initial read).
    content_b64: String,
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Clone, PartialEq)]
enum UploadStatus {
    Uploading,
    Error(String),
}

/// Best-effort image-type sniff from the filename extension — mirrors
/// `chat_attachments_api::content_type_from_ext`'s image branch (deliberate
/// client-side copy, not a shared helper; this crate's WASM client cannot
/// see the server-only fn). Used only to pick the correct client cap
/// (D-09's asymmetric image/non-image ceiling).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn is_probably_image(filename: &str) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
}

/// `{filename} · {size}` chip copy — human-readable byte size (UI-SPEC
/// Copywriting Contract example: `screenshot.png · 1.4 MB`). A deliberate
/// copy of card.rs's `format_attachment_size` (porting-not-sharing
/// convention established by `chat_attachments_api::content_type_from_ext`).
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

/// Phase 46.7 Plan 05 (D-08): fire the composer's error toast for `msg`,
/// auto-dismissing after 4s unless a newer toast has already replaced it
/// (the `toast_token` guard — otherwise a slow-to-clear earlier toast could
/// blank out a toast fired moments later for a second failed file).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn fire_attach_toast(
    msg: String,
    mut toast_msg: Signal<Option<String>>,
    mut toast_token: Signal<u64>,
) {
    let tok = *toast_token.read() + 1;
    toast_token.set(tok);
    toast_msg.set(Some(msg));
    spawn(async move {
        crate::platform::timer::sleep(4000).await;
        if *toast_token.peek() == tok {
            toast_msg.set(None);
        }
    });
}

/// Phase 46.7 Plan 05: POST one already-encoded attachment to the SESSION
/// (not a task). Shared by the initial upload path and the `↻` retry
/// control. On success, the row moves from `local_uploads` (uploading) into
/// `pending_attached` (settled, awaiting send) — NOT into a restarted server
/// resource, per the file-level doc above. On failure, both the inline chip
/// error state AND the D-08 toast fire.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
fn spawn_chat_attach_call(
    filename: String,
    content_b64: String,
    session_id: String,
    mut local_uploads: Signal<Vec<LocalUpload>>,
    mut pending_attached: Signal<Vec<ChatAttachmentRow>>,
    toast_msg: Signal<Option<String>>,
    toast_token: Signal<u64>,
) {
    spawn(async move {
        match crate::server::chat_attachments_api::upload_attachment(
            session_id,
            filename.clone(),
            content_b64,
        )
        .await
        {
            Ok(row) => {
                // Settled — drop the local overlay entry and add the real
                // server row to the client-local pending-send list.
                local_uploads.write().retain(|u| u.filename != filename);
                pending_attached.write().push(row);
            }
            Err(e) => {
                {
                    let mut w = local_uploads.write();
                    if let Some(u) = w.iter_mut().find(|u| u.filename == filename) {
                        u.status = UploadStatus::Error(e.to_string());
                    }
                }
                fire_attach_toast(
                    format!("Couldn't attach {filename} — upload failed"),
                    toast_msg,
                    toast_token,
                );
            }
        }
    });
}

/// Phase 46.7 Plan 05 (D-05/D-08): read each picked/dropped/pasted
/// `FileData`'s bytes, enforce the client-side cap (asymmetric by content
/// type, D-09), base64-encode, and hand off to `spawn_chat_attach_call`.
/// Each file is an independent spawned task so one slow/failed read never
/// blocks the others. Fed by three entry points: the hidden file input
/// (picker), the `.chat-mini` drop target, and the textarea paste handler.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
fn spawn_chat_attachment_reads(
    files: Vec<FileData>,
    session_id: String,
    mut local_uploads: Signal<Vec<LocalUpload>>,
    pending_attached: Signal<Vec<ChatAttachmentRow>>,
    toast_msg: Signal<Option<String>>,
    toast_token: Signal<u64>,
) {
    for file in files {
        let filename = file.name();
        let session_id = session_id.clone();
        spawn(async move {
            let bytes = match file.read_bytes().await {
                Ok(b) => b,
                Err(e) => {
                    local_uploads.write().push(LocalUpload {
                        filename: filename.clone(),
                        status: UploadStatus::Error(format!("could not read file: {e}")),
                        content_b64: String::new(),
                    });
                    fire_attach_toast(
                        format!("Couldn't attach {filename} — upload failed"),
                        toast_msg,
                        toast_token,
                    );
                    return;
                }
            };
            // A failed/empty read (e.g. a Safari drag/drop File-API quirk)
            // must never queue an upload — surface an error chip locally
            // instead of POSTing a 0-byte payload (mirrors card.rs's UAT
            // gap fix).
            if bytes.is_empty() {
                local_uploads.write().push(LocalUpload {
                    filename: filename.clone(),
                    status: UploadStatus::Error("empty file — not uploaded".to_string()),
                    content_b64: String::new(),
                });
                fire_attach_toast(
                    format!("Couldn't attach {filename} — upload failed"),
                    toast_msg,
                    toast_token,
                );
                return;
            }
            let cap = if is_probably_image(&filename) {
                CLIENT_IMAGE_MAX_ATTACHMENT_BYTES
            } else {
                CLIENT_NONIMAGE_MAX_ATTACHMENT_BYTES
            };
            if bytes.len() > cap {
                // UI-SPEC Copywriting Contract — attachment size cap message.
                local_uploads.write().push(LocalUpload {
                    filename: filename.clone(),
                    status: UploadStatus::Error(format!(
                        "{filename} exceeds the {}MB attachment limit",
                        cap / (1024 * 1024)
                    )),
                    content_b64: String::new(),
                });
                fire_attach_toast(
                    format!("Couldn't attach {filename} — file too large"),
                    toast_msg,
                    toast_token,
                );
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
            spawn_chat_attach_call(
                filename,
                content_b64,
                session_id,
                local_uploads,
                pending_attached,
                toast_msg,
                toast_token,
            );
        });
    }
}

/// Phase 46.7 Plan 05 (D-16): split the D-16 wire-contract note
/// (`"\n[Artifact published: {title} — /artifacts/{id}]\n"`, appended to the
/// assistant bubble's streamed text by
/// `state.rs::capture_turn_deliverable_note` via the shared `stream_cb_arc`
/// out-of-band channel — mirrors the WR-01 context_warnings mechanism, see
/// 46.7-04-SUMMARY.md D-16 decision) out of the display text, returning
/// `(display_text_without_the_note, Some((title, id)))` when present. Manual
/// substring parse (no regex dep in this crate) — the note format is a
/// fixed, server-controlled literal, not user input.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn extract_artifact_chip(text: &str) -> (String, Option<(String, String)>) {
    const PREFIX: &str = "[Artifact published: ";
    const SEP: &str = " \u{2014} /artifacts/";
    if let Some(start) = text.find(PREFIX) {
        let after_prefix = &text[start + PREFIX.len()..];
        if let Some(sep_idx) = after_prefix.find(SEP) {
            let title = after_prefix[..sep_idx].to_string();
            let after_sep = &after_prefix[sep_idx + SEP.len()..];
            if let Some(close_idx) = after_sep.find(']') {
                let id = after_sep[..close_idx].to_string();
                let note_end = start + PREFIX.len() + sep_idx + SEP.len() + close_idx + 1;
                let mut cleaned = String::new();
                cleaned.push_str(text[..start].trim_end_matches('\n'));
                cleaned.push_str(text[note_end..].trim_start_matches('\n'));
                return (cleaned.trim().to_string(), Some((title, id)));
            }
        }
    }
    (text.to_string(), None)
}

/// Phase 46.7 Plan 05 (D-24): truncate a long absolute path for the compact
/// workspace-path chip — keeps the filename-bearing tail, prefixes an
/// ellipsis when truncated. The full path is always available via the
/// native `title` tooltip. A deliberate copy of card.rs's
/// `truncate_output_path` (porting-not-sharing convention).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn truncate_workspace_path(path: &str, max_chars: usize) -> String {
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

/// Phase 46.7 Plan 05 (D-24): fire-and-forget clipboard write, mirroring
/// card.rs's `write_output_path_to_clipboard` (no toast/modal feedback
/// beyond the ✓ icon flip the UI-SPEC specifies).
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn write_workspace_path_to_clipboard(text: &str) {
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

// ---------------------------------------------------------------------------
// ScreenChat component (Plan 06 Task 2)
// ---------------------------------------------------------------------------
//
// Renders the full `<section id="screen-chat">` layout from `app.html`:
// a screen-header (with session-id label + READY/STREAMING status + token
// budget), a `.chat-mini` container that holds the `.chat-stream` (per-
// bubble `chat-msg.{user|assistant|error}` rows with embedded
// `.chat-progress` tool-call rows), and the `.chat-input-pill` composer.
//
// All chat state is consumed from the context providers established by
// HermesApp in `hermes_app/mod.rs`:
//
//   - `Signal<Vec<ChatBubble>>`   — the bubble list (bare signal)
//   - `Signal<Option<u64>>`       — id of the currently-streaming bubble
//   - `Signal<(u32, u32)>`        — (used, max) token budget
//   - `SessionIdContext`          — B-03 newtype wrapping the session id
//   - `ChatSendHandler`           — newtype wrapping the send EventHandler
//
// Submit semantics mirror Phase 22.3 D-15 (the shell_legacy `input_box.rs`
// rule, per D-20): Enter submits, Shift+Enter inserts a literal newline.
// Slash commands (`/help`, `/clear`, `/research …`) are NOT parsed client-
// side — they flow over the WebSocket as plain text frames and the
// server's existing CommandRouter resolves them (D-20).
// Phase 47.3 Plan 06 (D-01, Correction 9): fires the `/logout` palette
// action. POSTs `/auth/logout` (the server does the actual session
// revocation, Plan 01/02 — this client never reads or writes the session
// cookie name) and hard-navigates to `/`, where the server re-renders the
// login page because the cookie is now invalid.
//
// Uses the SAME `web_sys::window()` + `js_sys::Reflect` idiom as
// `ui_prefs.rs`'s localStorage helpers, rather than adding `gloo-net` as a
// direct wasm dependency (RESEARCH Assumption A4, resolved) or the
// `Request`/`RequestInit` web-sys features (not currently enabled and out
// of this plan's `files_modified` — Cargo.toml is not ours to touch here).
// `Window::fetch_with_str` needs only the already-enabled `Window` feature;
// the POST method + same-origin credentials are set via a plain
// `js_sys::Object` built through `Reflect::set`, mirroring the untyped
// get/set-item calls `ui_prefs::storage` already uses.
#[cfg(target_arch = "wasm32")]
async fn perform_logout() {
    use wasm_bindgen::{JsCast, JsValue};

    let Some(window) = web_sys::window() else {
        return;
    };

    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(
        &init,
        &JsValue::from_str("method"),
        &JsValue::from_str("POST"),
    );
    let _ = js_sys::Reflect::set(
        &init,
        &JsValue::from_str("credentials"),
        &JsValue::from_str("same-origin"),
    );

    if let Ok(fetch_fn) = js_sys::Reflect::get(&window, &JsValue::from_str("fetch")) {
        if let Ok(f) = fetch_fn.dyn_into::<js_sys::Function>() {
            if let Ok(promise) = f.call2(&window, &JsValue::from_str("/auth/logout"), &init) {
                let promise: js_sys::Promise = promise.unchecked_into();
                // Best-effort: the redirect below must proceed even if this
                // fetch fails (e.g. the session was already dead) — a stuck
                // operator on a dead session is worse than a missed revoke.
                let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
            }
        }
    }

    hard_navigate_to_login();
}

/// Non-WASM stub so `cargo test` on the host target links cleanly (mirrors
/// `ui_prefs.rs`'s stub convention). `--all-features` native builds don't
/// reach `HermesApp`'s render tree (same reachability gap the existing
/// `SessionIdContext`/`SelectedArtifactCtx`/etc. context newtypes already
/// carry `#[allow(dead_code)]` for), so the call site inside `on_pick`
/// doesn't register as "used" to native dead-code analysis either.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
async fn perform_logout() {}

/// Hard-navigates the browser to `/` (D-14/D-07 precedent: a real
/// successful auth transition — login OR logout OR session death — is
/// always a full reload, never a client-side animation, so the server can
/// re-render the correct page for the new auth state). Shared by
/// `perform_logout` above and `trigger_session_expiry_redirect` below —
/// same `js_sys::Reflect` idiom as `ui_prefs.rs`, no new web-sys feature.
#[cfg(target_arch = "wasm32")]
fn hard_navigate_to_login() {
    use wasm_bindgen::JsValue;
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Ok(location) = js_sys::Reflect::get(&window, &JsValue::from_str("location")) {
        let _ = js_sys::Reflect::set(
            &location,
            &JsValue::from_str("href"),
            &JsValue::from_str("/"),
        );
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
fn hard_navigate_to_login() {}

/// Phase 47.3 Plan 06 (D-17): the ONE code path for all three session-death
/// causes (7-day absolute TTL, 24h idle timeout, and — most commonly — a
/// server restart wiping the in-memory session store, since the session
/// store is in-memory). Called from BOTH D-17 trigger points this plan
/// wires: a 401 detected from a server fn call (`palette_data` below) and a
/// WebSocket close (the `is_ws_connected` watcher below, fed by the
/// receive loop hoisted at the `HermesApp` root in `mod.rs`).
///
/// Stashes the composer's current unsent text (no-op if empty — "nothing
/// to stash" per must_haves), flips `AuthedContext` to unauthenticated so
/// any in-flight UI stops assuming a session, then hard-navigates to `/`.
/// The stash is best-effort by construction (see `stash_composer_draft`'s
/// doc comment) — this function always proceeds to the redirect regardless
/// of whether the localStorage write actually landed.
#[allow(dead_code)] // see hard_navigate_to_login's native-reachability note above
fn trigger_session_expiry_redirect(mut authed: Signal<bool>, composer_text: &str) {
    crate::ui_prefs::stash_composer_draft(composer_text);
    authed.set(false);
    hard_navigate_to_login();
}

/// Phase 47.3 Plan 06 (D-17): does this server-fn call's error indicate the
/// session died (a 401 from the `require_auth` middleware, Plan 01/02)?
///
/// `list_slash_commands`/`list_skills` (api.rs) declare the bare
/// `dioxus::Result<T>` return type, i.e. `Result<T, CapturedError>` — the
/// underlying `ServerFnError` (with its HTTP status code) is still
/// downcastable out of the `anyhow::Error` `CapturedError` wraps
/// (`impl<E: Into<anyhow::Error>> From<E> for CapturedError`, dioxus-core's
/// blanket impl — `ServerFnError` implements `std::error::Error` via
/// `thiserror`, so this conversion never loses the original error type).
///
/// Checks both `ServerFnError` shapes that can carry an HTTP status code —
/// `ServerError { code, .. }` (the server fn's own structured error path)
/// and `Request(RequestError::Status(_, code))` (the client-side codec's
/// view of a non-2xx response, which is what actually happens here: the
/// auth middleware's raw 401 short-circuits the server fn handler entirely,
/// so the client only ever sees this as a transport-level status error).
#[allow(dead_code)] // see hard_navigate_to_login's native-reachability note above
fn is_unauthorized_error<T>(result: &dioxus::Result<T>) -> bool {
    let Err(captured) = result else {
        return false;
    };
    match captured.downcast_ref::<ServerFnError>() {
        Some(ServerFnError::ServerError { code, .. }) => *code == 401,
        Some(ServerFnError::Request(req_err)) => req_err.status_code() == Some(401),
        _ => false,
    }
}

#[component]
pub fn ScreenChat(is_active: bool) -> Element {
    // Context lookups — every read drops its borrow before the rsx tree
    // is constructed (clippy.toml signal-borrow safety).
    let mut bubbles = use_context::<Signal<Vec<ChatBubble>>>();
    let mut streaming_id = use_context::<Signal<Option<u64>>>();
    let tokens = use_context::<Signal<(u32, u32)>>();
    let session_id = use_context::<crate::state::SessionIdContext>().0;
    let send = use_context::<ChatSendHandler>();
    // Phase 26.7.2 D-06: next_id for monotonic bubble ID allocation in history load.
    // Provided via context by HermesApp so IDs don't collide with the WS receive loop.
    let mut next_id = use_context::<crate::state::NextIdContext>().0;
    // Phase 36.17.9 (D-14): MicButton now reads STT availability reactively from
    // the Signal<VoiceStatusState> context itself (see mic_button.rs) — no stale
    // bool prop is threaded through here anymore.

    // Phase 36.17.10 UAT redesign: the composer action oval's middle "Free"
    // segment launches hands-free voice mode (the orb overlay). Read STT
    // availability (to gate the button) and the voice-mode-active signal (to
    // toggle the overlay) here. Reading voice_status subscribes this component
    // so the async VoiceStatus frame re-enables the button after first paint.
    let voice_status = use_context::<Signal<crate::components::hermes_app::VoiceStatusState>>();
    let mut voice_mode_active = use_context::<crate::state::VoiceModeActiveContext>().0;
    let stt_avail = voice_status.read().stt_available; // Copy bool; borrow ends here.

    // Phase 46.7 Plan 05 (D-16): the assistant-bubble artifact chip opens the
    // 46.6 sandboxed viewer via the SAME context primitives artifacts.rs /
    // card.rs already use — no new provider, already available at HermesApp
    // root (state.rs:850-852 / mod.rs).
    let mut active_screen = use_context::<Signal<crate::state::Screen>>();
    let mut selected_artifact = use_context::<crate::state::SelectedArtifactCtx>().0;

    // Phase 47.3 Plan 06 (D-17): AuthedContext — flipped to `false` by
    // `/logout` (below) and by the session-death redirect. Provided at the
    // HermesApp root (mod.rs) per the Dioxus context-panic rule.
    let mut authed = use_context::<crate::components::hermes_app::AuthedContext>().0;

    // Local composer state — text being typed in the `chat-input-pill`
    // textarea. Cleared on submit (Enter or the SEND button).
    let mut input = use_signal(String::new);

    // Phase 47.3 Plan 06 (D-17): the SECOND D-17 trigger point — the
    // WebSocket closing. `is_ws_connected` is set false by the receive loop
    // hoisted at the HermesApp root (mod.rs) on both `Message::Close` and a
    // hard `Err` (mod.rs's WS receive loop). A TRUE->FALSE transition here
    // (not merely "currently false", which is also true for one render
    // before the very first connect succeeds) is treated as a possible dead
    // session — most commonly a server restart, since the session store is
    // in-memory. `was_ws_connected` tracks the previous value so only a
    // genuine drop fires the redirect, never the initial mount state.
    let is_ws_connected = use_context::<crate::state::WsConnectedContext>().0;
    let mut was_ws_connected = use_signal(|| false);
    use_effect(move || {
        let now_connected = *is_ws_connected.read();
        if *was_ws_connected.peek() && !now_connected {
            let composer_text = input.peek().clone();
            trigger_session_expiry_redirect(authed, &composer_text);
        }
        was_ws_connected.set(now_connected);
    });

    // Phase 47.3 Plan 06 (D-17): restore a stashed composer draft exactly
    // once on mount (after re-authentication). `restore_composer_draft`
    // consumes (clears) the draft on read, so a later reload never
    // resurrects it. `draft_restored` is the same one-shot-effect idiom the
    // hydration effect in mod.rs uses.
    let mut draft_restored = use_signal(|| false);
    use_effect(move || {
        if *draft_restored.peek() {
            return;
        }
        draft_restored.set(true);
        if let Some(draft) = crate::ui_prefs::restore_composer_draft() {
            input.set(draft);
        }
    });

    // Phase 46.7 Plan 05 (D-05/D-06/D-08): composer attachment state.
    // `local_uploads` — in-flight/failed uploads (uploading/error, not yet
    // settled). `pending_attached` — settled, queued-to-send rows (cleared
    // on submit). `file_drag_active` — the OS-file drop-target overlay.
    // `toast_msg`/`toast_token` — the D-08 error toast (token guards a
    // stale delayed-clear from clobbering a newer toast).
    let mut local_uploads: Signal<Vec<LocalUpload>> = use_signal(Vec::new);
    let mut pending_attached: Signal<Vec<ChatAttachmentRow>> = use_signal(Vec::new);
    let mut file_drag_active: Signal<bool> = use_signal(|| false);
    let mut toast_msg: Signal<Option<String>> = use_signal(|| None);
    let toast_token: Signal<u64> = use_signal(|| 0_u64);

    // Fixed, single-instance id — ScreenChat is always-mounted exactly once
    // (unlike kanban's per-card ids, which need per-task uniqueness).
    const CHAT_ATTACH_INPUT_ID: &str = "chat-attach-input";

    // D-01 accept list: broad type set per the UI-SPEC's discretion note —
    // images plus the common text/code/doc extensions
    // `content_type_from_ext` (server) and the chat_capture pdf/text
    // pipeline (Plan 02) already recognize.
    const CHAT_ATTACH_ACCEPT: &str =
        "image/*,.pdf,.txt,.md,.rs,.py,.js,.ts,.json,.yaml,.yml,.toml,.csv,.zip,.log,.sh";

    // Auto-scroll on new bubbles — port of the warp_hermes.rs pattern.
    // Read the length into a local, drop the borrow, then poke the DOM.
    use_effect(move || {
        let len = bubbles.read().len();
        if len > 0 {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(window) = web_sys::window() {
                    if let Some(doc) = window.document() {
                        if let Ok(Some(el)) =
                            doc.query_selector(".chat-stream .chat-msg:last-child")
                        {
                            el.scroll_into_view_with_bool(false);
                        }
                    }
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = len; // suppress unused-variable on host builds
            }
        }
    });

    // Phase 26.7.2 D-06: History load — fires on every session_id change.
    // Signal-borrow discipline (clippy.toml): session_id borrow cloned into
    // owned String before spawn; no GenerationalRef held across await.
    // Subscribes only to session_id — writing bubbles/next_id inside spawn
    // does NOT retrigger this effect (Pitfall 2: no loop).
    use_effect(move || {
        // Clone into owned String; borrow ends at ; (RESEARCH Pitfall 1 / Q8).
        let sid = session_id.read().clone();
        // D-03/Q3: skip if session bootstrap has not resolved yet.
        if sid == "pending" {
            return;
        }

        // D-08: always clear on session_id change — no stale history leaks.
        bubbles.write().clear();
        // Cancel any stale STREAMING indicator from a previous session (Q8 / T-26.7.2-11).
        streaming_id.set(None);

        spawn(async move {
            // Phase 46.7 Plan 05 (D-11): clone before get_session_messages
            // consumes `sid` — needed again below for fetch_session_attachments.
            let sid_for_attachments = sid.clone();
            match crate::server::api::get_session_messages(sid).await {
                Ok(msgs) if !msgs.is_empty() => {
                    // Map ChatMessage → ChatBubble. Read next_id as Copy u64
                    // before any .await; borrow ends at ; (CLAUDE.md signal rules).
                    let mut id_val = *next_id.read();
                    let mut history: Vec<ChatBubble> = msgs
                        .into_iter()
                        .filter_map(|m| match m.role.as_str() {
                            "user" => {
                                let b = ChatBubble::user(id_val, m.content);
                                id_val += 1;
                                Some(b)
                            }
                            "assistant" => {
                                let b = ChatBubble::assistant(id_val, m.content);
                                id_val += 1;
                                Some(b)
                            }
                            _ => None,
                        })
                        .collect();

                    // Phase 46.7 Plan 05 (D-11): fetch the session's persisted
                    // attachment rows and hang them off the divider bubble —
                    // chat_attachments rows are never linked to a specific
                    // message_id today (Plan 04 left that field always None,
                    // documented gap; see SUMMARY), so per-message placement
                    // is not reconstructible. Surfacing the full persisted set
                    // right at the history/live boundary is the best-effort
                    // rendering that satisfies "a resumed session renders its
                    // persisted attachment chips/thumbnails" without claiming
                    // per-message precision this data model cannot provide.
                    let session_attachments: Vec<ChatAttachmentRow> =
                        crate::server::chat_attachments_api::fetch_session_attachments(
                            sid_for_attachments,
                        )
                        .await
                        .unwrap_or_default();

                    // D-02 / Pitfall 3: insert divider only when history non-empty.
                    // Guarded by the outer Ok(msgs) if !msgs.is_empty() arm.
                    history.push(ChatBubble {
                        id: id_val,
                        kind: ChatBubbleKind::Divider,
                        text: String::new(),
                        tool_rows: vec![],
                        // Phase 36.17.7 D-02-a/b/f: dividers don't carry audio.
                        audio_src: None,
                        audio_mime: None,
                        // Phase 01-04 (DLV-03 web): dividers don't carry an image.
                        image_src: None,
                        // Phase 36.3.3 (D-08 web): dividers don't carry a video.
                        video_src: None,
                        // Phase 46.7 Plan 05 (D-11): persisted session attachments,
                        // rendered as a strip below the divider label.
                        attachments: session_attachments,
                    });
                    id_val += 1;

                    // Single write — all history + divider atomically appended.
                    // WriteLock acquired and released before next .await (none follows).
                    bubbles.write().extend(history);
                    next_id.set(id_val);
                }
                _ => {
                    // D-07: empty or error → empty stream, no indicator, no divider.
                }
            }
        });
    });

    // Phase 36.17.10 Plan 04 (D-14): consume the EXISTING queue-depth signal
    // provided by HermesApp mod.rs:509 (`use_context_provider(|| queue_state)`).
    // This is a plain Signal<(u32, bool)> — NOT a newtype — so use_context
    // resolves to the same (u32, bool) provider from AppFooter / mod.rs.
    // We do NOT create a second queue_state signal (Dioxus v0.3.0 context-
    // collision bug: duplicate same-typed providers).
    let queue_state = use_context::<Signal<(u32, bool)>>();
    // Transient cancel-error bubble ID: set when cancel_turn fails so the
    // correct bubble can display a retry label. Stored as Signal<Option<u64>>
    // at component scope (cannot use per-bubble hooks inside `for` loops).
    let cancel_error_id: Signal<Option<u64>> = use_signal(|| None);

    // Phase 46.7 Plan 05 (D-24): the copyable workspace-path chip. Fetches
    // once per session_id change (use_resource re-runs when a signal read in
    // its synchronous prefix changes, mirroring card.rs's card_artifact
    // resource) — session_workspace_dir is a server-side path resolver
    // (ironhermes_core, native-only), so a #[server] fn round-trip is
    // required to surface it to the WASM client.
    let workspace_path_resource = use_resource(move || {
        let sid = session_id.read().clone();
        async move {
            if sid == "pending" {
                return String::new();
            }
            crate::server::api::session_workspace_path(sid)
                .await
                .unwrap_or_default()
        }
    });
    let workspace_path_full: String = workspace_path_resource.value()().unwrap_or_default();
    let workspace_path_truncated = truncate_workspace_path(&workspace_path_full, 32);
    // Separate clone for the onclick closure below (mirrors card.rs's
    // output_path_for_copy — the closure moves its capture, so it must not
    // alias the same binding the title/text interpolations read).
    let workspace_path_for_copy = workspace_path_full.clone();
    let mut workspace_path_copied: Signal<bool> = use_signal(|| false);

    // Phase 41.1 Plan 06 (D-05): the `/` command palette. `pal_open` is toggled
    // by a leading `/` in the composer textarea (oninput below); `pal_query`
    // backs the palette's own search box; `pal_state` stays `Browse` (this
    // surface has no personality-pick substate). Esc-to-close is handled by the
    // wrapper `onkeydown` around the palette in the rsx tree (the palette
    // component does not own the open signal — legacy split preserved).
    let mut pal_open = use_signal(|| false);
    let mut pal_query = use_signal(String::new);
    let pal_state = use_signal(|| crate::state::PaletteState::Browse);

    // G-41.1-3: Cmd+K / Ctrl+K opens the palette from anywhere in the chat
    // screen, not just while the composer textarea has focus. Ported from the
    // disabled `legacy-shell` WarpHermes handler (warp_hermes.rs:575-619) and
    // rewired to THIS screen's own `pal_open`/`pal_query` signals — the ONLY
    // Cmd+K binding in the active (non-legacy-shell) build. A global
    // `window`-level listener (not a per-element `onkeydown`) so the chord is
    // captured regardless of where focus currently sits; mirrors the
    // `listener_slot: Signal<Option<Closure>>` + `use_drop` idiom already
    // blessed in this tree (wheel.rs:182-291).
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;
        use web_sys::KeyboardEvent as WebKeyboardEvent;

        let mut cmdk_slot: Signal<Option<Closure<dyn FnMut(WebKeyboardEvent)>>> =
            use_signal(|| None);

        use_effect(move || {
            let Some(window) = web_sys::window() else {
                return;
            };
            let cb = Closure::<dyn FnMut(WebKeyboardEvent)>::new(move |ev: WebKeyboardEvent| {
                // Physical key code — independent of Option-key æöü remaps.
                if (ev.meta_key() || ev.ctrl_key()) && ev.code() == "KeyK" {
                    ev.prevent_default();
                    pal_open.set(true);
                    pal_query.set(String::new());
                }
            });
            let _ = window
                .add_event_listener_with_callback("keydown", cb.as_ref().unchecked_ref());
            cmdk_slot.set(Some(cb));
        });

        use_drop(move || {
            if let Some(cb) = cmdk_slot.write().take() {
                if let Some(window) = web_sys::window() {
                    let _ = window.remove_event_listener_with_callback(
                        "keydown",
                        cb.as_ref().unchecked_ref(),
                    );
                }
                // cb drops here, releasing the JS-side reference.
            }
        });
    }

    // Palette data source — a LIVE union of Web-available slash commands and
    // SkillRegistry entries (UI-SPEC §A). Built from the EXISTING server reads
    // `list_slash_commands` (`GET /api/commands`) + `list_skills`
    // (`GET /api/skills`) — no new #[server] execution fn (RESEARCH Pitfall 6).
    // The synchronous prefix reads no signals, so this resource runs once.
    // Phase 47.3 Plan 06 (D-17): also this resource's first D-17 trigger
    // point — a 401 from either of the two server fn calls below means the
    // session has died. Checked BEFORE `.unwrap_or_default()` swallows the
    // error, since that's the only place the underlying Result is still
    // visible. `input.peek()` (not `.read()`) is correct here — this is an
    // async resource body, not the render path.
    let palette_data = use_resource(move || async move {
        let cmds_result = crate::server::api::list_slash_commands().await;
        let skills_result = crate::server::api::list_skills().await;

        if is_unauthorized_error(&cmds_result) || is_unauthorized_error(&skills_result) {
            let composer_text = input.peek().clone();
            trigger_session_expiry_redirect(authed, &composer_text);
        }

        let cmds = cmds_result.unwrap_or_default();
        let skills = skills_result.unwrap_or_default();
        (cmds, skills)
    });

    // Map server data → PaletteItem union BEFORE rsx (no signal borrow held
    // across the macro). Skill rows carry section="skill" → the `◆ skill` tag.
    let mut palette_items: Vec<crate::state::PaletteItem> = match palette_data.value()() {
        Some((cmds, skills)) => {
            let mut v: Vec<crate::state::PaletteItem> = cmds
                .into_iter()
                .map(|c| crate::state::PaletteItem {
                    section: "slash".into(),
                    cmd: format!("/{}", c.name),
                    label: c.description,
                    kbd: vec![],
                })
                .collect();
            v.extend(skills.into_iter().map(|s| crate::state::PaletteItem {
                section: "skill".into(),
                cmd: format!("/{}", s.name),
                label: s.description,
                kbd: vec![],
            }));
            v
        }
        None => vec![],
    };
    // Phase 47.3 Plan 06 (D-01/WIRING §7): `/logout` unioned into the SAME
    // live palette_data feed as list_slash_commands()/list_skills() above —
    // NOT the command_palette.rs demo fixtures. Deliberately NOT registered
    // in the shared command_router registry that also feeds the TUI/CLI/
    // gateway surfaces (logging out of a web session is meaningless there);
    // `on_pick` below intercepts this exact cmd string before it reaches the
    // WS chat-input path any other palette item takes.
    palette_items.push(crate::state::PaletteItem {
        section: "slash".into(),
        cmd: "/logout".into(),
        label: "Log out and return to the login page".into(),
        kbd: vec![],
    });

    // Header-bar reads — pull into locals so the rsx tree never holds a
    // live signal borrow across an attribute-evaluation boundary.
    let streaming = streaming_id.read().is_some();
    let (used, max) = *tokens.read();
    let sid_full = session_id.read().clone();
    let sid_short: String = if sid_full.len() <= 8 {
        sid_full.clone()
    } else {
        sid_full[sid_full.len() - 8..].to_string()
    };
    // Read queue depth into a Copy u32 local before the rsx tree.
    // Pattern B: no Signal borrow is held across any closure or async boundary.
    let queue_depth = queue_state.read().0;

    // Phase 46.7 Plan 05: snapshot the attachment signals into owned Vecs
    // before the rsx tree (mirrors card.rs's persisted_attachments /
    // local_uploads_snapshot pattern) — the `for` loops below must not hold
    // a live Signal Read guard across closure construction in the render tree.
    let pending_attached_snapshot: Vec<ChatAttachmentRow> = pending_attached.read().clone();
    let local_uploads_snapshot: Vec<LocalUpload> = local_uploads.read().clone();
    let has_pending_attachments =
        !pending_attached_snapshot.is_empty() || !local_uploads_snapshot.is_empty();

    rsx! {
        section {
            class: "screen",
            class: if is_active { "is-active" },
            id: "screen-chat",
            "data-screen-label": "01 Chat",

            // Screen header — mirrors app.html lines ~362-374. Carries the
            // module tag, title, sub-copy, and session/token affordances.
            div { class: "screen-header",
                div { class: "screen-header-left",
                    div { class: "screen-tag", "// MODULE 01" }
                    h1 { class: "screen-title", "Chat" }
                    p { class: "screen-sub",
                        "Streaming conversation with slash commands, live tool progress, and per-message token accounting."
                    }
                }
                div { class: "screen-actions",
                    // Phase 46.7 Plan 05 (D-24): copyable workspace-path chip.
                    // Path-display only — NO file-browser panel (D-24
                    // prohibition, satisfied by absence). Absence of a path
                    // (empty string — session not yet resolved) renders
                    // nothing, matching the 46.4 output-path empty-state
                    // convention.
                    if !workspace_path_full.is_empty() {
                        span {
                            class: "chat-workspace-chip",
                            title: "{workspace_path_full}",
                            span { class: "hint", "WORKSPACE" }
                            span { class: "chat-workspace-chip-path", "{workspace_path_truncated}" }
                            button {
                                class: "chat-workspace-copy",
                                r#type: "button",
                                "aria-label": "Copy workspace path",
                                "data-copied": if *workspace_path_copied.read() { "true" },
                                onclick: move |_| {
                                    write_workspace_path_to_clipboard(&workspace_path_for_copy);
                                    workspace_path_copied.set(true);
                                    spawn(async move {
                                        crate::platform::timer::sleep(1200).await;
                                        workspace_path_copied.set(false);
                                    });
                                },
                                if *workspace_path_copied.read() { "\u{2713}" } else { "\u{29c9}" }
                            }
                        }
                    }
                    span { class: "screen-meta",
                        "SID "
                        code { "{sid_short}" }
                    }
                    span { class: "screen-meta",
                        "TOK {used} / {max}"
                    }
                    if streaming {
                        span { class: "screen-meta is-streaming", "STREAMING" }
                    } else {
                        span { class: "screen-meta", "READY" }
                    }
                }
            }

            // Chat container — `.chat-mini > .chat-main > (chat-stream + chat-input-pill)`
            // per app.html. Class strings match the screens.css selectors
            // verbatim so the design renders pixel-faithful.
            //
            // Phase 46.7 Plan 05 (D-05): `.chat-mini` doubles as the OS-file
            // drop target. Unlike the kanban card, chat has NO competing
            // internal drag source, so no `dragged_task_id`-style suppression
            // is needed (UI-SPEC "Drop-target scope and collision avoidance").
            div {
                class: "chat-mini",
                "data-drop-active": if *file_drag_active.read() { "true" },
                ondragover: move |evt: DragEvent| {
                    evt.prevent_default();
                    file_drag_active.set(true);
                },
                ondragleave: move |_| {
                    file_drag_active.set(false);
                },
                ondrop: move |evt: DragEvent| {
                    evt.prevent_default();
                    file_drag_active.set(false);
                    // UAT gap fix precedent (card.rs): `evt.files()` (the
                    // FileEngine accessor), NOT `evt.data_transfer().files()`
                    // — the latter's FileList does not survive the async
                    // read_bytes() call in some browsers.
                    let files = evt.files();
                    if !files.is_empty() {
                        let sid = session_id.read().clone();
                        spawn_chat_attachment_reads(
                            files,
                            sid,
                            local_uploads,
                            pending_attached,
                            toast_msg,
                            toast_token,
                        );
                    }
                },
                if *file_drag_active.read() {
                    div { class: "chat-drop-overlay",
                        span { "Drop to attach" }
                    }
                }
                div { class: "chat-main",

                    // Scrollable bubble stream.
                    div { class: "chat-stream",
                        for b in bubbles.read().iter() {
                            div {
                                key: "{b.id}",
                                class: match b.kind {
                                    ChatBubbleKind::User      => "chat-msg user",
                                    ChatBubbleKind::Assistant => "chat-msg assistant",
                                    ChatBubbleKind::Error     => "chat-msg error",
                                    ChatBubbleKind::Divider   => "chat-divider",
                                    ChatBubbleKind::Audio     => "chat-msg assistant audio",
                                    ChatBubbleKind::Image     => "chat-msg assistant image",
                                    ChatBubbleKind::Video     => "chat-msg assistant video",
                                    // Phase 41.1 Plan 03 (UI-SPEC §C): DIM meta row, no bubble.
                                    ChatBubbleKind::Meta      => "chat-meta",
                                },

                                // Avatar — the shield-caduceus PNG for the
                                // assistant (D-09), a textual badge for user
                                // and error bubbles.
                                match b.kind {
                                    ChatBubbleKind::Assistant => rsx! {
                                        div { class: "avatar logo",
                                            img { src: AVATAR_LOGO, alt: "" }
                                        }
                                    },
                                    ChatBubbleKind::User => rsx! {
                                        div { class: "avatar amber", "OP" }
                                    },
                                    ChatBubbleKind::Error => rsx! {
                                        div { class: "avatar error", "!" }
                                    },
                                    ChatBubbleKind::Divider => rsx! {},
                                    // Phase 41.1 Plan 03 (UI-SPEC §C): meta chip has no avatar.
                                    ChatBubbleKind::Meta => rsx! {},
                                    ChatBubbleKind::Audio => rsx! {
                                        div { class: "avatar logo",
                                            img { src: AVATAR_LOGO, alt: "" }
                                        }
                                    },
                                    // Phase 01-04 (DLV-03 web): generated-image bubble uses
                                    // the assistant avatar (the image is an assistant output).
                                    ChatBubbleKind::Image => rsx! {
                                        div { class: "avatar logo",
                                            img { src: AVATAR_LOGO, alt: "" }
                                        }
                                    },
                                    // Phase 36.3.3 (D-08 web): generated-video bubble uses
                                    // the assistant avatar (the video is an assistant output).
                                    ChatBubbleKind::Video => rsx! {
                                        div { class: "avatar logo",
                                            img { src: AVATAR_LOGO, alt: "" }
                                        }
                                    },
                                }

                                // Phase 46.7 Plan 05 (D-16): split the D-16
                                // artifact-published note out of assistant text
                                // before rendering (see extract_artifact_chip doc
                                // — the wire-contract note appended by
                                // state.rs::capture_turn_deliverable_note via the
                                // shared stream_cb_arc channel). Non-assistant
                                // kinds pass through unchanged.
                                {
                                    let (display_text, artifact_chip) = if b.kind == ChatBubbleKind::Assistant {
                                        extract_artifact_chip(&b.text)
                                    } else {
                                        (b.text.clone(), None)
                                    };

                                // Bubble body + embedded tool-call progress
                                // rows for assistant bubbles (D-19).
                                // Phase 26.7.2 D-02: Divider variant renders a
                                // section-label rule instead of a bubble body.
                                //
                                // NOTE: this whole if/else is itself an
                                // "expression child" node (the enclosing `{`
                                // above), so BOTH arms must produce an
                                // `Element` via an explicit `rsx! {}` call —
                                // bare `div { ... }` is only valid directly
                                // inside an `rsx!` macro invocation.
                                if !matches!(b.kind, ChatBubbleKind::Divider | ChatBubbleKind::Meta) {
                                    rsx! {
                                    div { class: "chat-bubble-wrap",
                                        // Phase 36.17.10 Plan 04: conditional cancel/stop button.
                                        // State A (in-flight): this bubble is the currently-streaming
                                        //   assistant bubble → red ■ STOP button.
                                        // State B (queued): not in-flight but queue depth > 0 →
                                        //   neutral ✕ REMOVE button.
                                        // Hidden for User/Error/Divider/Audio bubbles and for
                                        // assistant bubbles that are already finished (not streaming,
                                        // queue depth == 0).
                                        // Signal-borrow safety (Pattern B): session_id, streaming_id,
                                        // and queue_depth are all Copy locals read before this closure.
                                        {
                                            let bubble_id = b.id;
                                            let is_inflight = matches!(b.kind, ChatBubbleKind::Assistant)
                                                && *streaming_id.read() == Some(bubble_id);
                                            let is_queued = matches!(b.kind, ChatBubbleKind::Assistant)
                                                && !is_inflight
                                                && queue_depth > 0;
                                            let show_cancel_error = *cancel_error_id.read() == Some(bubble_id);
                                            let sid_for_cancel = session_id.read().clone();

                                            // Phase 36.17.10 UAT: NO button on the in-flight bubble. The
                                            // contract (D-05) is that the streaming response finishes
                                            // naturally — there is no hard abort — so a button there is
                                            // misleading. The remove control lives only on the QUEUED bubble,
                                            // where the action (drop the pending message) is real. `is_inflight`
                                            // is still computed above to exclude the streaming bubble from `is_queued`.
                                            if is_queued {
                                                rsx! {
                                                    button {
                                                        class: "wh-cancel-btn is-stop",
                                                        r#type: "button",
                                                        "aria-label": "Remove queued message",
                                                        title: "Remove this queued message",
                                                        onclick: move |_| {
                                                            let sid = sid_for_cancel.clone();
                                                            let mut err_sig = cancel_error_id;
                                                            // Clear the server queue, then remove this queued bubble
                                                            // locally so it actually disappears (cancel_turn clears
                                                            // the whole queue + emits QueueUpdated; the bubble itself
                                                            // is client state).
                                                            let mut bubbles_sig = bubbles;
                                                            spawn(async move {
                                                                if crate::server::api::cancel_turn(sid).await.is_err() {
                                                                    err_sig.set(Some(bubble_id));
                                                                } else {
                                                                    bubbles_sig.write().retain(|x| x.id != bubble_id);
                                                                }
                                                            });
                                                        },
                                                        if show_cancel_error {
                                                            "Could not remove. Try again."
                                                        } else {
                                                            "✕ REMOVE"
                                                        }
                                                    }
                                                }
                                            } else {
                                                rsx! {}
                                            }
                                        }
                                        div {
                                            class: match b.kind {
                                                ChatBubbleKind::User      => "chat-bubble is-user",
                                                ChatBubbleKind::Assistant => "chat-bubble is-assistant",
                                                ChatBubbleKind::Error     => "chat-bubble is-error",
                                                ChatBubbleKind::Divider   => "chat-bubble is-divider",
                                                ChatBubbleKind::Audio     => "chat-bubble is-assistant is-audio",
                                                ChatBubbleKind::Image     => "chat-bubble is-assistant is-image",
                                                ChatBubbleKind::Video     => "chat-bubble is-assistant is-video",
                                                // Unreachable — Meta/Divider skip the bubble-wrap
                                                // above; present only for exhaustive-match compliance.
                                                ChatBubbleKind::Meta      => "chat-bubble is-meta",
                                            },
                                            // Phase 36.17.7 D-02-a/b/f: render inline <audio controls>
                                            // when this bubble carries a Blob URL. In Dioxus RSX, the
                                            // {audio_src} expression is RSX interpolation of the local
                                            // `audio_src` String — NOT a literal "{audio_src}" string.
                                            // Equivalent in non-RSX code: format!("{}", audio_src).
                                            // RSX BRACE INTERPOLATION note (MED 10).
                                            if let (Some(audio_src), Some(audio_mime)) = (b.audio_src.clone(), b.audio_mime.clone()) {
                                                audio {
                                                    controls: true,
                                                    src: "{audio_src}",
                                                    "type": "{audio_mime}",
                                                }
                                            }
                                            // Phase 01-04 (DLV-03 web): render the generated
                                            // image inline as an <img> when this bubble carries
                                            // a Blob URL. {image_src} is RSX interpolation of
                                            // the local `image_src` String (not a literal).
                                            // The Blob URL is built client-side from the
                                            // ImageOut binary frame (mod.rs Binary recv arm),
                                            // so no raw `<MEDIA:>` text reaches the DOM (T-04-02).
                                            if let Some(image_src) = b.image_src.clone() {
                                                img {
                                                    class: "chat-image",
                                                    src: "{image_src}",
                                                    alt: "generated image",
                                                }
                                            }
                                            // Phase 36.3.3 (D-08 web): render the generated
                                            // video inline as a <video controls> when this
                                            // bubble carries a Blob URL. {video_src} is RSX
                                            // interpolation of the local `video_src` String.
                                            // The Blob URL is built client-side from the
                                            // VideoOut binary frame (mod.rs Binary recv arm),
                                            // so no raw `<MEDIA:>` text reaches the DOM.
                                            // Dioxus reactivity: use .read() in the render path,
                                            // .peek() only in effects/closures (MEMORY.md).
                                            if let Some(video_src) = b.video_src.clone() {
                                                video {
                                                    class: "chat-video",
                                                    controls: true,
                                                    src: "{video_src}",
                                                }
                                            }
                                            // Phase 46.7 Plan 05 (D-06/D-07): sent-bubble
                                            // attachment render — image attachments as an
                                            // inline thumbnail (reuses the existing
                                            // .chat-image class), non-image as a compact
                                            // download-only .chat-file-chip. An
                                            // attachment-only message (D-07) renders no
                                            // placeholder text — the {display_text} node
                                            // below is simply empty and produces nothing
                                            // visible, so the chip/thumbnail row IS the
                                            // bubble body.
                                            if b.kind == ChatBubbleKind::User && !b.attachments.is_empty() {
                                                for att in b.attachments.iter() {
                                                    {
                                                        let is_image = att
                                                            .content_type
                                                            .as_deref()
                                                            .is_some_and(|c| c.starts_with("image/"));
                                                        let href = format!(
                                                            "/chat-attachments/{}/{}",
                                                            att.session_id, att.id
                                                        );
                                                        if is_image {
                                                            rsx! {
                                                                img {
                                                                    key: "{att.id}",
                                                                    class: "chat-image",
                                                                    src: "{href}",
                                                                    alt: "{att.filename}",
                                                                }
                                                            }
                                                        } else {
                                                            rsx! {
                                                                a {
                                                                    key: "{att.id}",
                                                                    class: "chat-file-chip",
                                                                    href: "{href}",
                                                                    "download": "{att.filename}",
                                                                    "\u{1f4c4} {att.filename} \u{00b7} {format_attachment_size(att.size_bytes)}"
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            // `{display_text}` is rendered as a plain
                                            // Dioxus text node — auto-escaped, no
                                            // `dangerous_inner_html` (T-26.2.1-19). For
                                            // an assistant bubble this is `b.text` with
                                            // the D-16 artifact-published note stripped
                                            // out (rendered as the chip below instead).
                                            "{display_text}"

                                            if !b.tool_rows.is_empty() {
                                                div { class: "chat-progress",
                                                    for tr in b.tool_rows.iter() {
                                                        {
                                                            let state_class = if tr.done {
                                                                if tr.success { "is-done is-success" } else { "is-done is-error" }
                                                            } else {
                                                                "is-running"
                                                            };
                                                            let icon_class = if tr.done { "icon done" } else { "icon spin" };
                                                            let icon_glyph = if tr.done { "●" } else { "◐" };
                                                            rsx! {
                                                                div {
                                                                    class: "chat-progress-row {state_class}",
                                                                    span {
                                                                        class: "{icon_class}",
                                                                        "{icon_glyph}"
                                                                    }
                                                                    span { class: "tp-name", "{tr.name}" }
                                                                    if !tr.args.is_empty() {
                                                                        span { class: "tp-args", " · {tr.args}" }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }

                                            // Phase 46.7 Plan 05 (D-16): assistant-bubble
                                            // artifact chip — the LAST element inside
                                            // `.chat-bubble.is-assistant`, after text and
                                            // any `.chat-progress` rows (UI-SPEC placement
                                            // contract), opening the 46.6 sandboxed viewer
                                            // via SelectedArtifactCtx.
                                            if let Some((artifact_title, artifact_id)) = artifact_chip {
                                                button {
                                                    class: "chat-artifact-chip",
                                                    r#type: "button",
                                                    "aria-label": "Open artifact {artifact_title}",
                                                    onclick: move |_| {
                                                        selected_artifact.set(Some(crate::server::api::ArtifactInfo {
                                                            id: artifact_id.clone(),
                                                            title: artifact_title.clone(),
                                                            icon: None,
                                                            source_kind: Some("chat".to_string()),
                                                            source_ref: None,
                                                            updated_at: String::new(),
                                                            archived: false,
                                                        }));
                                                        active_screen.set(crate::state::Screen::ArtifactViewer);
                                                    },
                                                    "\u{25a4} {artifact_title}"
                                                }
                                            }
                                        }
                                    }
                                    }
                                } else if b.kind == ChatBubbleKind::Meta {
                                    // Phase 41.1 Plan 03 (SKILL-13 web / D-06, UI-SPEC §C):
                                    // DIM run-turn meta chip — a single left-aligned metadata
                                    // row with NO bubble background, visually distinct from
                                    // user/assistant bubbles. The `▶` glyph + text are the
                                    // server-formatted chip copy (`b.text`).
                                    rsx! {
                                        div { class: "chat-meta-chip", "{b.text}" }
                                    }
                                } else {
                                    rsx! {
                                    // Phase 26.7.2 D-02: history/live boundary marker.
                                    // Matches the `.section-label` convention from sessions.rs.
                                    div { class: "chat-divider-label",
                                        span { class: "section-label", "─── Earlier ───" }
                                        // Phase 46.7 Plan 05 (D-11): persisted session
                                        // attachments, rendered as a best-effort strip at
                                        // the history/live boundary (see the history-load
                                        // use_effect doc comment for why per-message
                                        // placement is not reconstructible today).
                                        if !b.attachments.is_empty() {
                                            div { class: "chat-attach-history-strip",
                                                span { class: "hint", "Session files" }
                                                for att in b.attachments.iter() {
                                                    {
                                                        let href = format!(
                                                            "/chat-attachments/{}/{}",
                                                            att.session_id, att.id
                                                        );
                                                        rsx! {
                                                            a {
                                                                key: "{att.id}",
                                                                class: "chat-file-chip",
                                                                href: "{href}",
                                                                "download": "{att.filename}",
                                                                "\u{1f4c4} {att.filename} \u{00b7} {format_attachment_size(att.size_bytes)}"
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
                        }
                    }

                    // Phase 46.7 Plan 05 (D-06): pending-attach-chips row —
                    // renders ABOVE the pill as its own row when non-empty
                    // (collapses to zero height when empty; UI-SPEC
                    // "Composer layout order"). Shows uploading/error
                    // (`local_uploads`) and settled/queued-to-send
                    // (`pending_attached`) chips together.
                    if has_pending_attachments {
                        div { class: "chat-attach-chips",
                            for row in pending_attached_snapshot.iter() {
                                span {
                                    key: "{row.id}",
                                    class: "chat-attach-chip",
                                    "{row.filename} \u{00b7} {format_attachment_size(row.size_bytes)}"
                                    {
                                        let id_for_remove = row.id.clone();
                                        rsx! {
                                            button {
                                                class: "chat-attach-remove",
                                                r#type: "button",
                                                "aria-label": "Remove {row.filename}",
                                                onclick: move |_| {
                                                    pending_attached.write().retain(|r| r.id != id_for_remove);
                                                },
                                                "\u{2715}"
                                            }
                                        }
                                    }
                                }
                            }
                            for up in local_uploads_snapshot.iter() {
                                {
                                    let filename_for_retry = up.filename.clone();
                                    let content_b64_for_retry = up.content_b64.clone();
                                    let filename_for_remove = up.filename.clone();
                                    match &up.status {
                                        UploadStatus::Uploading => rsx! {
                                            span {
                                                key: "{up.filename}",
                                                class: "chat-attach-chip is-uploading",
                                                "{up.filename} \u{00b7} uploading\u{2026}"
                                            }
                                        },
                                        UploadStatus::Error(reason) => rsx! {
                                            span {
                                                key: "{up.filename}",
                                                class: "chat-attach-chip is-error",
                                                "{reason}"
                                                button {
                                                    class: "chat-attach-retry",
                                                    r#type: "button",
                                                    "aria-label": "Retry upload for {up.filename}",
                                                    onclick: move |_| {
                                                        let sid = session_id.read().clone();
                                                        local_uploads
                                                            .write()
                                                            .retain(|u| u.filename != filename_for_retry);
                                                        spawn_chat_attach_call(
                                                            filename_for_retry.clone(),
                                                            content_b64_for_retry.clone(),
                                                            sid,
                                                            local_uploads,
                                                            pending_attached,
                                                            toast_msg,
                                                            toast_token,
                                                        );
                                                    },
                                                    "\u{21bb}"
                                                }
                                                button {
                                                    class: "chat-attach-remove",
                                                    r#type: "button",
                                                    "aria-label": "Remove {up.filename}",
                                                    onclick: move |_| {
                                                        local_uploads
                                                            .write()
                                                            .retain(|u| u.filename != filename_for_remove);
                                                    },
                                                    "\u{2715}"
                                                }
                                            }
                                        },
                                    }
                                }
                            }
                        }
                    }

                    // Composer pill — attach button + textarea + SEND button.
                    // Per D-20 / Phase 22.3 D-15: Enter submits, Shift+Enter
                    // inserts a literal newline. Slash commands are NOT
                    // parsed here — the server's CommandRouter handles
                    // `/help`, `/clear`, `/research …` etc.
                    form {
                        class: "chat-input-pill",
                        onsubmit: move |evt| {
                            evt.prevent_default();
                            let text = { let s = input.read(); s.clone() };
                            let attachments: Vec<ChatAttachmentRow> = pending_attached.read().clone();
                            send.0.call((text, attachments));
                            input.set(String::new());
                            pending_attached.write().clear();
                        },
                        // Phase 46.7 Plan 05 (D-05): attach button — a SEPARATE
                        // 4th element to the LEFT of the existing 3-segment
                        // `.composer-oval` (UI-SPEC "Existing precedent this
                        // phase must NOT collide with" — does not join the oval).
                        // Opens the hidden file input via `label[for=...]`
                        // (mirrors card.rs's kn-attach-add trigger pair).
                        label {
                            class: "chat-attach-btn",
                            r#for: "{CHAT_ATTACH_INPUT_ID}",
                            "aria-label": "Attach files",
                            title: "Attach files",
                            "\u{1f4ce}"
                        }
                        input {
                            id: "{CHAT_ATTACH_INPUT_ID}",
                            class: "chat-attach-input-hidden",
                            r#type: "file",
                            multiple: true,
                            accept: "{CHAT_ATTACH_ACCEPT}",
                            onchange: move |evt: FormEvent| {
                                let files = evt.files();
                                if !files.is_empty() {
                                    let sid = session_id.read().clone();
                                    spawn_chat_attachment_reads(
                                        files,
                                        sid,
                                        local_uploads,
                                        pending_attached,
                                        toast_msg,
                                        toast_token,
                                    );
                                }
                            },
                        }
                        textarea {
                            rows: "1",
                            placeholder: "/ slash, or just message…",
                            value: "{input}",
                            oninput: move |evt| {
                                let v = evt.value();
                                // Phase 41.1 Plan 06 (D-05): a leading `/` opens the
                                // command palette (⌘K-style overlay with its own search
                                // box). Seed that search with the text typed after the
                                // slash and clear the composer — the palette takes over.
                                if v.starts_with('/') && !pal_open() {
                                    pal_query.set(v.trim_start_matches('/').to_string());
                                    pal_open.set(true);
                                    input.set(String::new());
                                } else {
                                    input.set(v);
                                }
                            },
                            onkeydown: move |evt| {
                                if evt.key() == Key::Enter && !evt.modifiers().shift() {
                                    evt.prevent_default();
                                    let text = { let s = input.read(); s.clone() };
                                    let attachments: Vec<ChatAttachmentRow> = pending_attached.read().clone();
                                    send.0.call((text, attachments));
                                    input.set(String::new());
                                    pending_attached.write().clear();
                                }
                            },
                            // Phase 46.7 Plan 05 (D-05): paste-from-clipboard
                            // (images only). Scoped to this textarea, NOT
                            // document-global, so paste on other screens is
                            // unaffected. Dioxus's typed ClipboardData carries
                            // no file/MIME accessor on web (dioxus-html 0.7.7
                            // — verified: HasClipboardData exposes only
                            // as_any()), so the real clipboardData.files read
                            // goes through the raw web_sys escape hatch
                            // (dioxus::web::WebEventExt), cfg-gated to wasm32
                            // like every other raw-JS block in this file.
                            onpaste: move |evt| {
                                #[cfg(target_arch = "wasm32")]
                                {
                                    use dioxus::web::WebEventExt;
                                    use wasm_bindgen::JsCast;
                                    if let Some(raw) = evt.try_as_web_event() {
                                        if let Ok(clipboard_evt) = raw.dyn_into::<web_sys::ClipboardEvent>() {
                                            if let Some(dt) = clipboard_evt.clipboard_data() {
                                                if let Some(file_list) = dt.files() {
                                                    let mut image_file: Option<web_sys::File> = None;
                                                    for i in 0..file_list.length() {
                                                        if let Some(f) = file_list.item(i) {
                                                            if f.type_().starts_with("image/") {
                                                                image_file = Some(f);
                                                                break;
                                                            }
                                                        }
                                                    }
                                                    if let Some(file) = image_file {
                                                        evt.prevent_default();
                                                        if let Ok(reader) = web_sys::FileReader::new() {
                                                            let file_data = FileData::new(
                                                                dioxus::web::WebFileData::new(file, reader),
                                                            );
                                                            let sid = session_id.read().clone();
                                                            spawn_chat_attachment_reads(
                                                                vec![file_data],
                                                                sid,
                                                                local_uploads,
                                                                pending_attached,
                                                                toast_msg,
                                                                toast_token,
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                                #[cfg(not(target_arch = "wasm32"))]
                                {
                                    let _ = evt;
                                }
                            },
                        }
                        // Phase 36.17.10 UAT redesign — segmented action oval:
                        //   Mic (push-to-talk) | Free (hands-free orb) | Send.
                        // Rounded ends only on the outer edges; vertical dividers
                        // between segments. Replaces the inline mic+send pair and
                        // the former global floating voice-entry-btn (removed in mod.rs).
                        div { class: "composer-oval",
                            // ── Mic — push-to-talk (unchanged behavior; D-13/D-14) ──
                            // on_transcript: inject transcript into the composer and
                            // submit, exactly as if the user had typed it (D-15).
                            crate::components::hermes_app::mic_button::MicButton {
                                on_transcript: move |transcript: String| {
                                    input.set(transcript.clone());
                                    let attachments: Vec<ChatAttachmentRow> = pending_attached.read().clone();
                                    send.0.call((transcript, attachments));
                                    input.set(String::new());
                                    pending_attached.write().clear();
                                },
                            }
                            span { class: "composer-divider", "aria-hidden": "true" }
                            // ── Free — launch hands-free voice mode (orb overlay) ──
                            // Mirrors the AudioContext-resume user-gesture unlock from
                            // the former floating voice-entry-btn (browser autoplay
                            // policy — the loop's own AudioContext inherits the
                            // user-activated audio state from this gesture).
                            button {
                                r#type: "button",
                                class: "composer-free-btn",
                                "aria-label": "Launch hands-free voice mode",
                                // Always enabled (UAT regression fix): open-mic / free
                                // voice uses the OpenAI Realtime API, NOT the whisper STT
                                // path, so a false or not-yet-arrived `stt_available`
                                // (e.g. the VoiceStatus frame hasn't landed, or whisper is
                                // unconfigured) must never disable free-voice entry. The
                                // VoiceModeScreen overlay shows its own Unavailable state
                                // if voice genuinely cannot run. `stt_avail` is surfaced in
                                // the tooltip only.
                                "aria-disabled": "false",
                                title: if stt_avail {
                                    "Hands-free voice mode with the orb"
                                } else {
                                    "Hands-free voice mode with the orb (open-mic realtime)"
                                },
                                onclick: move |_| {
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        if let Some(window) = web_sys::window() {
                                            let _ = window; // confirm gesture context
                                            if let Ok(ctx) = web_sys::AudioContext::new() {
                                                let _ = ctx.resume();
                                            }
                                        }
                                    }
                                    voice_mode_active.set(true);
                                },
                                "✦ FREE"
                            }
                            span { class: "composer-divider", "aria-hidden": "true" }
                            // ── Send — submit the composer (unchanged) ──
                            button {
                                r#type: "submit",
                                class: "composer-send-btn",
                                "▓ SEND"
                            }
                        }
                    }
                }

                // Phase 46.7 Plan 05 (D-08): attachment-error toast. Fires
                // alongside the inline chip error state (both surfaces per
                // D-08); auto-dismisses after 4s (fire_attach_toast) or on
                // manual "×" dismiss.
                if let Some(msg) = toast_msg.read().clone() {
                    div { class: "chat-toast",
                        span { "{msg}" }
                        button {
                            class: "chat-toast-dismiss",
                            r#type: "button",
                            "aria-label": "Dismiss",
                            onclick: move |_| toast_msg.set(None),
                            "\u{d7}"
                        }
                    }
                }
            }

            // Phase 41.1 Plan 06 (D-05): the `/` command palette overlay. The
            // wrapper `onkeydown` closes it on Esc (the palette component does
            // not own the open signal — legacy split preserved). On pick,
            // `/<name>` is submitted through the EXISTING WS chat-input path
            // (the ChatSendHandler wired at HermesApp root — the SAME path a
            // typed message takes), firing the one-shot skill run Plan 03 wired.
            // No new server execution entry point (RESEARCH Pitfall 6).
            div {
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Escape && pal_open() {
                        pal_open.set(false);
                        pal_query.set(String::new());
                    }
                },
                crate::components::hermes_app::command_palette::CommandPalette {
                    items: palette_items,
                    query: pal_query,
                    open: pal_open,
                    state: pal_state,
                    on_pick: move |item: crate::state::PaletteItem| {
                        // Phase 47.3 Plan 06 (D-01): `/logout` is intercepted
                        // here, BEFORE the normal WS chat-input path below —
                        // logging out is a real auth action, not a chat
                        // message. Set AuthedContext false first so any
                        // in-flight UI stops assuming a session, then let the
                        // server do the actual revocation.
                        if item.cmd == "/logout" {
                            pal_open.set(false);
                            pal_query.set(String::new());
                            authed.set(false);
                            spawn(async move {
                                perform_logout().await;
                            });
                            return;
                        }
                        let attachments: Vec<ChatAttachmentRow> = pending_attached.read().clone();
                        // Reattach any args typed after the command token in the
                        // palette search (e.g. `/gsd-phase 49.1 fix the search`).
                        // `item.cmd` is the canonical `/<name>`; the palette
                        // filters on the first token only, so `pal_query` still
                        // holds the full "<cmd> <args…>" the user typed. Split off
                        // the trailing text and append it — the backend resolver
                        // (`ws.rs::resolve_skill_invocation`) splits command from
                        // trailing text and runs the skill argued. Without this the
                        // args are dropped and only the bare command runs.
                        let q = pal_query.read().clone();
                        let args = q
                            .split_once(char::is_whitespace)
                            .map(|(_, rest)| rest.trim())
                            .unwrap_or("");
                        let cmd = if args.is_empty() {
                            item.cmd.clone()
                        } else {
                            format!("{} {}", item.cmd, args)
                        };
                        send.0.call((cmd, attachments));
                        pending_attached.write().clear();
                        pal_open.set(false);
                        pal_query.set(String::new());
                        input.set(String::new());
                    },
                }
            }
        }
    }
}
