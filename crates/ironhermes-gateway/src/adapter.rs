use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Handler for incoming messages — connects gateway to the agent.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    /// Process an incoming message. The handler owns the adapter reference
    /// and drives edits/responses directly (enabling streaming).
    async fn handle(
        &self,
        event: &MessageEvent,
        adapter: Arc<dyn PlatformAdapter>,
        cancel: CancellationToken,
    ) -> Result<()>;
}

/// Trait for platform-specific messaging adapters.
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    /// The platform this adapter handles.
    fn platform(&self) -> Platform;

    /// Send a text message to a chat.
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send a text message using Telegram's `parse_mode: MarkdownV2`
    /// (Phase 36.17.2.2 D-01 — sibling of [`Self::send_message`] for the
    /// `stream_consumer.rs` overflow-chunk path per CONTEXT.md D-Discretion
    /// recommendation: "every overflow chunk uses `send_message_markdown_v2`
    /// so the entire response renders consistently").
    ///
    /// **Caller contract:** `content` MUST be pre-escaped via
    /// `crate::markdown_v2::escape_outside_code_blocks` before invocation.
    /// The trait surface does NOT auto-escape — the production call sites
    /// in `stream_consumer.rs::flush(final_edit=true)`'s overflow branch
    /// apply the escape themselves. Implementors that do not distinguish
    /// MarkdownV2 from plain text (Discord, Slack, test fixtures) MAY
    /// delegate to [`Self::send_message`]; only `TelegramAdapter` actually
    /// sets `parse_mode: MarkdownV2` and applies the D-02 fallback.
    ///
    /// **D-02 fallback (`TelegramAdapter` only):** when the Bot API returns a
    /// 400 whose description substring-matches a parse-mode failure, the
    /// implementation logs `warn!`, re-issues the same call once with
    /// `parse_mode` OMITTED, and returns the retry result. On second
    /// failure logs `error!` and returns the retry `Err`. Non-Telegram
    /// adapters need not implement the fallback.
    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Edit an existing message (plain text — for streaming edits).
    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()>;

    /// Edit an existing message using Telegram's `parse_mode: MarkdownV2`
    /// (Phase 36.17.2.2 D-01 — supersedes the legacy `Markdown` parse mode).
    ///
    /// **Caller contract:** `content` MUST be pre-escaped via
    /// `crate::markdown_v2::escape_outside_code_blocks` before invocation.
    /// The trait surface does NOT auto-escape — the only production call
    /// site (`stream_consumer.rs::flush(final_edit=true)` per plan 05)
    /// applies the escape. Implementors (notably `TelegramAdapter`) treat
    /// `content` as already-escaped and pass it through `parse_mode:
    /// MarkdownV2`.
    ///
    /// **D-02 fallback (`TelegramAdapter` only):** when the Bot API returns a
    /// 400 whose description substring-matches a parse-mode failure, the
    /// implementation logs `warn!`, re-issues the same call once with
    /// `parse_mode` OMITTED, and returns the retry result. On second
    /// failure logs `error!` and returns the retry `Err`. Non-Telegram
    /// adapters need not implement the fallback.
    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()>;

    /// Delete a message.
    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()>;

    /// Add a reaction to a message.
    async fn add_reaction(&self, _chat_id: &str, _message_id: &str, _emoji: &str) -> Result<()> {
        Ok(()) // Default no-op for platforms that don't support reactions
    }

    /// Send a chat action (e.g. "typing").
    async fn send_chat_action(&self, _chat_id: &str, _action: &str) -> Result<()> {
        Ok(()) // Default no-op
    }

    /// Check if the adapter is currently running.
    fn is_running(&self) -> bool;
}

// Phase 36.17.2.2 D-18: re-export the media-tag types so consumers can write
// `use ironhermes_gateway::adapter::{MediaSender, MediaRef, MediaSource, MediaKind};`
// with a single import statement. The canonical definitions still live in
// `crate::media_tag` (plan 02) — both paths resolve to the same types.
pub use crate::media_tag::{MediaKind, MediaRef, MediaSource};

/// Outbound media-attachment sender. Sibling of [`PlatformAdapter`] —
/// declared in this module so the natural import surface for callers is
/// `use ironhermes_gateway::adapter::{PlatformAdapter, MediaSender, ...};`.
///
/// Implementors (today): `TelegramAdapter` (plan 04). Discord, Slack, and
/// the web adapter intentionally do NOT implement `MediaSender`; on those
/// platforms `GatewayMessageHandler` holds `Option<Arc<dyn MediaSender>>`
/// set to `None`, and the plan 06 D-19 dispatch loop logs a `warn!` and
/// drops any extracted `<MEDIA: ...>` refs (CONTEXT.md D-18).
///
/// **D-17 — uniform-no-caption divergence from hermes-agent §Audio Captions.**
/// NONE of the six methods carry a `caption` parameter. The text body has
/// already been rendered into the placeholder message via
/// `edit_message_markdown_v2` (D-01) BEFORE attachments are dispatched
/// — see [`PlatformAdapter::edit_message_markdown_v2`].
//
// (doc-link no-op: pull `edit_message_markdown_v2` into rustdoc index)
/// (D-07), so a per-attachment caption would be redundant — and a partial
/// duplicate of the final-text body. This deviates from hermes-agent's
/// behaviour, which attaches plain-text captions on `sendVoice`; the
/// rationale and follow-up are tracked in CONTEXT.md D-17.
///
/// **Construction pattern (RESEARCH §`#[async_trait]` Verification Open Q4 /
/// Assumption A7).** Trait upcasting between `Arc<dyn PlatformAdapter>` and
/// `Arc<dyn MediaSender>` is unstable on stable Rust at the time this trait
/// was added. Callers MUST construct the concrete adapter once
/// (`Arc<TelegramAdapter>`) and then `clone()`-cast separately into each
/// trait object — never `Arc<dyn PlatformAdapter>` -> `Arc<dyn MediaSender>`
/// via upcast. Example wiring (runner.rs in plan 06):
///
/// ```ignore
/// let telegram: Arc<TelegramAdapter> = Arc::new(TelegramAdapter::new(token));
/// let adapter_for_platform: Arc<dyn PlatformAdapter> = telegram.clone();
/// let adapter_for_media:    Arc<dyn MediaSender>     = telegram.clone();
/// ```
#[async_trait]
pub trait MediaSender: Send + Sync {
    /// Send a still image (`.png`, `.jpg`, `.jpeg`, `.webp`, `.gif`).
    /// Path variants are uploaded multipart; URL variants are passed
    /// through for Telegram's servers to fetch (D-12).
    async fn send_photo(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send an inline voice bubble (`.ogg` / `.opus`) — D-14.
    async fn send_voice(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send a music-player audio attachment (`.mp3`, `.m4a`, `.flac`,
    /// `.wav`) — D-14.
    async fn send_audio(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send a playable video (`.mp4`, `.mov`, `.webm`).
    async fn send_video(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Send an arbitrary file as a downloadable document (everything that
    /// doesn't match the four media kinds above).
    async fn send_document(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse>;

    /// Dispatch on [`MediaKind`] and forward to the corresponding per-type
    /// method. Default impl provided so concrete adapters need only
    /// implement the five per-type methods (D-18).
    async fn send_media(
        &self,
        chat_id: &str,
        media_ref: &MediaRef,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match media_ref.kind {
            MediaKind::Photo => self.send_photo(chat_id, &media_ref.source, thread_id).await,
            MediaKind::Voice => self.send_voice(chat_id, &media_ref.source, thread_id).await,
            MediaKind::Audio => self.send_audio(chat_id, &media_ref.source, thread_id).await,
            MediaKind::Video => self.send_video(chat_id, &media_ref.source, thread_id).await,
            MediaKind::Document => {
                self.send_document(chat_id, &media_ref.source, thread_id).await
            }
        }
    }
}
