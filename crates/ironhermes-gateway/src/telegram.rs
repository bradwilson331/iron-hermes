use crate::adapter::{MediaSender, MediaSource, PlatformAdapter};
use anyhow::{Context, Result};
use async_trait::async_trait;
use ironhermes_core::{Attachment, MessageEvent, MessageResponse, Platform};
// TgSendApi trait now lives in ironhermes-cron::adapter (Phase 32.1 Plan 06)
pub use ironhermes_cron::TgSendApi;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tracing::{error, warn};

#[async_trait]
impl TgSendApi for TelegramAdapter {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Delegate to PlatformAdapter::send_message; discard MessageResponse.
        <Self as PlatformAdapter>::send_message(self, chat_id, content, thread_id)
            .await
            .map(|_| ())
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Phase 36.17.2.2-04: `send_file_multipart` now returns
        // `Result<MessageResponse>` per Pitfall 4 / D-18. `TgSendApi`
        // contract is preserved by discarding the projected response
        // with `.map(|_| ())` — `ironhermes-cron` callers continue to
        // see `Result<()>` (RESEARCH FLAGGED RISK A8: zero external
        // callers of the gateway-side helper).
        self.send_file_multipart("sendVoice", "voice", chat_id, path, thread_id)
            .await
            .map(|_| ())
    }

    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Phase 36.17.2.2-04: discard the new `MessageResponse` projection to
        // preserve the `TgSendApi::Result<()>` contract (Pitfall 4 option ii).
        self.send_file_multipart("sendPhoto", "photo", chat_id, path, thread_id)
            .await
            .map(|_| ())
    }

    async fn send_video(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Phase 36.17.2.2-04: discard MessageResponse — preserve TgSendApi contract.
        self.send_file_multipart("sendVideo", "video", chat_id, path, thread_id)
            .await
            .map(|_| ())
    }

    async fn send_document(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Phase 36.17.2.2-04: discard MessageResponse — preserve TgSendApi contract.
        self.send_file_multipart("sendDocument", "document", chat_id, path, thread_id)
            .await
            .map(|_| ())
    }
}

// Re-export CancellationToken so polling modules in later plans can import it from here.
pub use tokio_util::sync::CancellationToken;

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram Bot API adapter using long polling.
pub struct TelegramAdapter {
    token: String,
    http: Client,
    pub bot_username: Option<String>,
}

impl TelegramAdapter {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            http: Client::new(),
            bot_username: None,
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", TELEGRAM_API_BASE, self.token, method)
    }

    pub async fn api_call<T: serde::de::DeserializeOwned>(
        &self,
        method: &str,
        params: &impl Serialize,
    ) -> Result<T> {
        let url = self.api_url(method);
        let response = self
            .http
            .post(&url)
            .json(params)
            .send()
            .await
            .context("Telegram API request failed")?;

        let status = response.status();
        let body: TelegramResponse<T> = response
            .json()
            .await
            .context("Failed to parse Telegram response")?;

        if !body.ok {
            anyhow::bail!(
                "Telegram API error ({}): {}",
                body.error_code.unwrap_or(status.as_u16() as i32),
                body.description.unwrap_or_default()
            );
        }

        body.result.context("Telegram API returned no result")
    }

    /// Get bot information.
    pub async fn get_me(&self) -> Result<TgUser> {
        self.api_call("getMe", &serde_json::json!({})).await
    }

    /// Register bot commands with Telegram.
    pub async fn set_my_commands(&self, commands: &[TgBotCommand]) -> Result<()> {
        let params = serde_json::json!({ "commands": commands });
        let _: serde_json::Value = self.api_call("setMyCommands", &params).await?;
        Ok(())
    }

    /// Get file metadata by file_id.
    pub async fn get_file(&self, file_id: &str) -> Result<TgFile> {
        let params = serde_json::json!({ "file_id": file_id });
        self.api_call("getFile", &params).await
    }

    /// Download a file by its file_path from getFile response.
    pub async fn download_file(&self, file_path: &str) -> Result<Vec<u8>> {
        let url = format!("{}/file/bot{}/{}", TELEGRAM_API_BASE, self.token, file_path);
        let bytes = self.http.get(&url).send().await?.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Long-poll for updates with a 30-second timeout.
    /// `offset` should be `last_update_id + 1` to avoid redelivery.
    pub async fn get_updates(&self, offset: Option<i64>) -> Result<Vec<TgUpdate>> {
        let mut params = serde_json::json!({ "timeout": 30 });
        if let Some(off) = offset {
            params["offset"] = serde_json::Value::Number(serde_json::Number::from(off));
        }
        self.api_call("getUpdates", &params).await
    }

    /// Upload a file to Telegram using multipart/form-data.
    ///
    /// `method`     — Telegram Bot API method name (e.g. "sendPhoto")
    /// `field_name` — multipart field name for the file (e.g. "photo")
    ///
    /// Phase 36.17.2.2-04 (Pitfall 4 / D-18): return type was widened from
    /// `Result<()>` to `Result<MessageResponse>` so `MediaSender::send_*`
    /// implementations can project a real `MessageResponse` (mirrors the
    /// `TgMessage` → `MessageResponse` projection used by `send_message` at
    /// the bottom of `impl PlatformAdapter for TelegramAdapter`). The four
    /// existing `TgSendApi` callers (`send_voice`, `send_image_file`,
    /// `send_video`, `send_document`) discard the new value via
    /// `.map(|_| ())` to preserve their `Result<()>` contract.
    async fn send_file_multipart(
        &self,
        method: &str,
        field_name: &str,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        let file_bytes = tokio::fs::read(path)
            .await
            .with_context(|| format!("Failed to read media file: {}", path.display()))?;

        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();

        let part = reqwest::multipart::Part::bytes(file_bytes).file_name(filename);
        let mut form = reqwest::multipart::Form::new()
            .text("chat_id", chat_id.to_string())
            .part(field_name.to_string(), part);

        if let Some(tid) = thread_id {
            form = form.text("message_thread_id", tid.to_string());
        }

        let url = self.api_url(method);
        let response = self
            .http
            .post(&url)
            .multipart(form)
            .send()
            .await
            .with_context(|| format!("Telegram {} request failed", method))?;

        let status = response.status();
        let body: TelegramResponse<TgMessage> = response
            .json()
            .await
            .with_context(|| format!("Failed to parse Telegram {} response", method))?;

        if !body.ok {
            anyhow::bail!(
                "Telegram {} error ({}): {}",
                method,
                body.error_code.unwrap_or(status.as_u16() as i32),
                body.description.unwrap_or_default()
            );
        }

        let result = body
            .result
            .with_context(|| format!("Telegram {} returned no result", method))?;
        Ok(MessageResponse {
            message_id: result.message_id.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    /// Phase 36.17.2.2-04 D-13: send a music-player audio attachment
    /// (`.mp3`, `.m4a`, `.flac`, `.wav`) via Telegram Bot API `sendAudio`.
    ///
    /// Sibling of the existing `TgSendApi::send_voice` / `send_image_file`
    /// / `send_video` / `send_document` helpers — mechanically symmetric,
    /// mirrors the `send_file_multipart` call shape per RESEARCH §Telegram
    /// Bot API Confirmations Assumption A2 (pattern symmetry).
    ///
    /// Added as an inherent method (NOT on `TgSendApi`) because the cron
    /// crate has no audio caller today; adding it to the cross-crate trait
    /// would expand the public surface area for a single in-crate
    /// consumer (`impl MediaSender for TelegramAdapter`, task 4 below).
    pub async fn send_audio(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.send_file_multipart("sendAudio", "audio", chat_id, path, thread_id).await
    }
}

#[async_trait]
impl PlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "text": content,
        });

        let result: TgMessage = self.api_call("sendMessage", &params).await?;

        Ok(MessageResponse {
            message_id: result.message_id.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        // Phase 36.17.2.2-05 D-01 + D-Discretion: route the overflow-chunk
        // path through MarkdownV2 so the entire final response renders
        // consistently. `content` is assumed to have been pre-escaped by
        // the caller via `crate::markdown_v2::escape_outside_code_blocks`.
        // D-02 single-retry-as-plain-text fallback mirrors the shape used
        // by `edit_message_markdown_v2` above.
        let mut params = serde_json::json!({
            "chat_id": chat_id,
            "text": content,
            "parse_mode": "MarkdownV2",
        });
        if let Some(tid) = thread_id {
            params["message_thread_id"] = serde_json::Value::String(tid.to_string());
        }

        let result: Result<TgMessage> = self.api_call("sendMessage", &params).await;

        match result {
            Ok(msg) => Ok(MessageResponse {
                message_id: msg.message_id.to_string(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            }),
            Err(e) => {
                // D-02 fallback: detect parse-mode failures via substring match
                // on the error description (same keyword set as
                // `edit_message_markdown_v2`).
                let desc = format!("{e}");
                let is_parse_failure = desc.contains("can't parse")
                    || desc.contains("parse entities")
                    || desc.contains("parse_mode");
                if !is_parse_failure {
                    return Err(e);
                }

                warn!(
                    chat_id = chat_id,
                    error = %desc,
                    "MarkdownV2 parse failed on send_message_markdown_v2; retrying as plain text (D-02 fallback)"
                );

                let mut plain_params = serde_json::json!({
                    "chat_id": chat_id,
                    "text": content,
                });
                if let Some(tid) = thread_id {
                    plain_params["message_thread_id"] =
                        serde_json::Value::String(tid.to_string());
                }

                match self.api_call::<TgMessage>("sendMessage", &plain_params).await {
                    Ok(msg) => Ok(MessageResponse {
                        message_id: msg.message_id.to_string(),
                        chat_id: chat_id.to_string(),
                        platform: Platform::Telegram,
                    }),
                    Err(retry_err) => {
                        error!(
                            chat_id = chat_id,
                            original_error = %desc,
                            retry_error = %retry_err,
                            "D-02 plain-text retry also failed on send_message_markdown_v2"
                        );
                        Err(retry_err)
                    }
                }
            }
        }
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> {
        // Plain text during streaming edits per D-03 (no parse_mode)
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
            "text": content,
        });

        let _: serde_json::Value = self.api_call("editMessageText", &params).await?;
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        // Phase 36.17.2.2-04 D-01: switch parse_mode from legacy "Markdown"
        // to "MarkdownV2" and apply the D-02 single-retry-as-plain-text
        // fallback. `content` is assumed to have been pre-escaped by the
        // caller via `crate::markdown_v2::escape_outside_code_blocks`
        // (call site wired in plan 05; the trait-level doc-comment on
        // `PlatformAdapter::edit_message_markdown_v2` records the contract).
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
            "text": content,
            "parse_mode": "MarkdownV2",
        });

        let result: Result<serde_json::Value> =
            self.api_call("editMessageText", &params).await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => {
                // D-02 fallback: detect parse-mode failures via substring match
                // on the error description. Telegram's exact wording for parse
                // errors has shifted across Bot API versions, so we match a
                // small substring set rather than a fixed string. Keywords
                // sourced from RESEARCH §Telegram Bot API Confirmations row 8
                // ("can't parse", "parse entities", "parse_mode").
                let desc = format!("{e}");
                let is_parse_failure = desc.contains("can't parse")
                    || desc.contains("parse entities")
                    || desc.contains("parse_mode");
                if !is_parse_failure {
                    // Not a parse-mode failure — propagate unchanged.
                    return Err(e);
                }

                warn!(
                    chat_id = chat_id,
                    message_id = message_id,
                    error = %desc,
                    "MarkdownV2 parse failed; retrying as plain text (D-02 fallback)"
                );

                let plain_params = serde_json::json!({
                    "chat_id": chat_id,
                    "message_id": message_id.parse::<i64>().unwrap_or(0),
                    "text": content,
                });

                match self
                    .api_call::<serde_json::Value>("editMessageText", &plain_params)
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(retry_err) => {
                        error!(
                            chat_id = chat_id,
                            message_id = message_id,
                            original_error = %desc,
                            retry_error = %retry_err,
                            "D-02 plain-text retry also failed"
                        );
                        Err(retry_err)
                    }
                }
            }
        }
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
        });

        let _: serde_json::Value = self.api_call("deleteMessage", &params).await?;
        Ok(())
    }

    async fn add_reaction(&self, chat_id: &str, message_id: &str, emoji: &str) -> Result<()> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
            "reaction": [{"type": "emoji", "emoji": emoji}],
        });

        let _: serde_json::Value = self.api_call("setMessageReaction", &params).await?;
        Ok(())
    }

    async fn send_chat_action(&self, chat_id: &str, action: &str) -> Result<()> {
        let params = serde_json::json!({ "chat_id": chat_id, "action": action });
        let _: serde_json::Value = self.api_call("sendChatAction", &params).await?;
        Ok(())
    }

    fn is_running(&self) -> bool {
        // Lifecycle is managed by GatewayRunner; adapter itself has no running state
        false
    }
}

// =============================================================================
// MediaSender — Phase 36.17.2.2-04 D-12/D-13/D-14/D-15/D-17/D-18
// =============================================================================

// Per-method size caps (D-15). All five Bot API methods reject local
// multipart uploads above their respective caps; pre-checking against the
// metadata length avoids a wasted multipart attempt and gives the D-19
// dispatch loop in plan 06 a typed Err to detect for the reinsert path.
const SIZE_CAP_20_MB: u64 = 20 * 1024 * 1024;
const SIZE_CAP_50_MB: u64 = 50 * 1024 * 1024;

/// Implements outbound media-attachment delivery on Telegram per the
/// `MediaSender` trait surface (D-18).
///
/// Per-method handling:
/// - **`MediaSource::Path(p)`** — runs the D-15 size pre-check
///   (`tokio::fs::metadata`), then the T-INPUT-MEDIA-PATH canonicalization
///   security gate (canonicalize + allow-list against `HERMES_HOME` and
///   `/tmp`), then forwards to `send_file_multipart` for multipart upload.
///   Rejected paths log the FILENAME ONLY via `tracing::warn!` per the
///   phase threat model's T-LOG-LEAK row (no parent directories leaked).
/// - **`MediaSource::Url(u)`** — D-12 passthrough: pass the URL string
///   straight to Telegram as the per-method field (`photo`/`voice`/etc.)
///   so Telegram's servers fetch it. Pre-validates the URL scheme: only
///   `http://` and `https://` are accepted (T-INPUT-MEDIA-URL gate).
///
/// **D-17 — no captions.** None of the five methods set a `caption`
/// parameter. The surrounding text body has already been rendered into
/// the placeholder message via `edit_message_markdown_v2` (D-01) before
/// attachments are dispatched (D-07), so a caption would duplicate the
/// final body.
///
/// **FLAGGED RISK D-05 — name sibling, not rename.** `send_photo` here is
/// INTENTIONALLY a sibling of the existing `TgSendApi::send_image_file`
/// method on this same struct (telegram.rs:34). `send_image_file` has
/// `ironhermes-cron` callers and renaming it is OUT OF SCOPE for this
/// plan. The two paths share `send_file_multipart` so behavior is
/// identical for the Path arm; the new method exists so the
/// `MediaSender` trait surface can be naturally named.
#[async_trait]
impl MediaSender for TelegramAdapter {
    async fn send_photo(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match source {
            MediaSource::Path(p) => {
                check_size_cap(p, SIZE_CAP_20_MB).await?;
                let validated = canonicalize_under_allowed_roots(p)?;
                self.send_file_multipart("sendPhoto", "photo", chat_id, &validated, thread_id)
                    .await
            }
            MediaSource::Url(u) => {
                validate_url_scheme(u)?;
                send_url_form(self, "sendPhoto", "photo", chat_id, u, thread_id).await
            }
        }
    }

    async fn send_voice(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match source {
            MediaSource::Path(p) => {
                check_size_cap(p, SIZE_CAP_20_MB).await?;
                let validated = canonicalize_under_allowed_roots(p)?;
                self.send_file_multipart("sendVoice", "voice", chat_id, &validated, thread_id)
                    .await
            }
            MediaSource::Url(u) => {
                validate_url_scheme(u)?;
                send_url_form(self, "sendVoice", "voice", chat_id, u, thread_id).await
            }
        }
    }

    async fn send_audio(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match source {
            MediaSource::Path(p) => {
                check_size_cap(p, SIZE_CAP_20_MB).await?;
                let validated = canonicalize_under_allowed_roots(p)?;
                self.send_file_multipart("sendAudio", "audio", chat_id, &validated, thread_id)
                    .await
            }
            MediaSource::Url(u) => {
                validate_url_scheme(u)?;
                send_url_form(self, "sendAudio", "audio", chat_id, u, thread_id).await
            }
        }
    }

    async fn send_video(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match source {
            MediaSource::Path(p) => {
                check_size_cap(p, SIZE_CAP_20_MB).await?;
                let validated = canonicalize_under_allowed_roots(p)?;
                self.send_file_multipart("sendVideo", "video", chat_id, &validated, thread_id)
                    .await
            }
            MediaSource::Url(u) => {
                validate_url_scheme(u)?;
                send_url_form(self, "sendVideo", "video", chat_id, u, thread_id).await
            }
        }
    }

    async fn send_document(
        &self,
        chat_id: &str,
        source: &MediaSource,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        match source {
            MediaSource::Path(p) => {
                // D-15: documents have a higher cap (50 MiB vs 20 MiB).
                check_size_cap(p, SIZE_CAP_50_MB).await?;
                let validated = canonicalize_under_allowed_roots(p)?;
                self.send_file_multipart(
                    "sendDocument",
                    "document",
                    chat_id,
                    &validated,
                    thread_id,
                )
                .await
            }
            MediaSource::Url(u) => {
                validate_url_scheme(u)?;
                send_url_form(self, "sendDocument", "document", chat_id, u, thread_id).await
            }
        }
    }
}

/// D-15 size pre-check helper. Reads `tokio::fs::metadata` on `path` and
/// returns `Err` if the file exceeds `cap` bytes. The Err message is
/// typed so the D-19 dispatch loop (plan 06) can substring-match the
/// "media exceeds size cap" prefix for the reinsert path.
async fn check_size_cap(path: &Path, cap: u64) -> Result<()> {
    let meta = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("Failed to stat media file: {}", path.display()))?;
    let len = meta.len();
    if len > cap {
        anyhow::bail!(
            "media exceeds size cap: {} bytes > {} bytes cap (path {})",
            len,
            cap,
            path.display()
        );
    }
    Ok(())
}

/// T-INPUT-MEDIA-PATH security gate (HIGH, phase threat model).
///
/// Canonicalize `path` (resolves symlinks and `..` components), then assert
/// the resolved absolute path is contained under one of the allowed roots:
/// - `$HERMES_HOME` if the env var is set, AND
/// - `/tmp` (literal).
///
/// On rejection, log a `warn!` with the FILENAME ONLY (no parent dirs) per
/// the T-LOG-LEAK mitigation row, then return a typed `Err` that the D-19
/// reinsert path in plan 06 can detect via substring match.
fn canonicalize_under_allowed_roots(path: &Path) -> Result<PathBuf> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("Failed to canonicalize media path: {}", path.display()))?;

    let mut allowed_roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HERMES_HOME") {
        if let Ok(canon_home) = PathBuf::from(&home).canonicalize() {
            allowed_roots.push(canon_home);
        }
    }
    if let Ok(canon_tmp) = PathBuf::from("/tmp").canonicalize() {
        allowed_roots.push(canon_tmp);
    }

    let allowed = allowed_roots
        .iter()
        .any(|root| canonical.starts_with(root));

    if !allowed {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?");
        warn!(
            filename = %filename,
            "rejected media path outside allowed roots (T-INPUT-MEDIA-PATH)"
        );
        anyhow::bail!("media path outside allowed roots: {}", path.display());
    }

    Ok(canonical)
}

/// T-INPUT-MEDIA-URL security gate. Accept only `http://` and `https://`
/// schemes — reject `file://`, `gopher://`, `ftp://`, etc.
fn validate_url_scheme(u: &str) -> Result<()> {
    if !(u.starts_with("http://") || u.starts_with("https://")) {
        anyhow::bail!("media URL scheme not allowed: {}", u);
    }
    Ok(())
}

/// D-12 URL-form sender. Builds the JSON payload that Telegram's
/// `sendPhoto`/`sendVoice`/... methods accept when the file is referenced
/// by URL rather than uploaded as multipart, calls `api_call`, and
/// projects the resulting `TgMessage` to `MessageResponse` (mirrors
/// `impl PlatformAdapter for TelegramAdapter::send_message`).
async fn send_url_form(
    adapter: &TelegramAdapter,
    method: &str,
    field_name: &str,
    chat_id: &str,
    url: &str,
    thread_id: Option<&str>,
) -> Result<MessageResponse> {
    let mut params = serde_json::json!({
        "chat_id": chat_id,
        field_name: url,
    });
    if let Some(tid) = thread_id {
        params["message_thread_id"] = serde_json::Value::String(tid.to_string());
    }
    let result: TgMessage = adapter.api_call(method, &params).await?;
    Ok(MessageResponse {
        message_id: result.message_id.to_string(),
        chat_id: chat_id.to_string(),
        platform: Platform::Telegram,
    })
}

// =============================================================================
// Telegram API types
// =============================================================================

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    error_code: Option<i32>,
    description: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgUpdate {
    pub update_id: i64,
    pub message: Option<TgMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgMessage {
    pub message_id: i64,
    pub from: Option<TgUser>,
    pub chat: TgChat,
    pub text: Option<String>,
    pub caption: Option<String>,
    pub photo: Option<Vec<TgPhotoSize>>,
    pub document: Option<TgDocument>,
    #[allow(dead_code)]
    pub date: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgUser {
    pub id: i64,
    pub first_name: String,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgChat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TgBotCommand {
    pub command: String,
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgFile {
    pub file_id: String,
    pub file_unique_id: Option<String>,
    pub file_size: Option<i64>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgPhotoSize {
    pub file_id: String,
    pub file_unique_id: String,
    pub width: i32,
    pub height: i32,
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TgDocument {
    pub file_id: String,
    pub file_unique_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    pub file_size: Option<i64>,
}

pub fn tg_message_to_event(msg: &TgMessage) -> MessageEvent {
    let mut attachments: Vec<Attachment> = Vec::new();

    // Include photo attachment metadata (actual download deferred to multimodal.rs)
    if let Some(ref photos) = msg.photo
        && let Some(largest) = photos.last()
    {
        attachments.push(Attachment {
            filename: Some(format!("photo_{}.jpg", largest.file_id)),
            mime_type: Some("image/jpeg".to_string()),
            data: None,
            url: None,
            file_id: Some(largest.file_id.clone()),
        });
    }

    // Include document attachment metadata
    if let Some(ref doc) = msg.document {
        attachments.push(Attachment {
            filename: doc.file_name.clone(),
            mime_type: doc.mime_type.clone(),
            data: None,
            url: None,
            file_id: Some(doc.file_id.clone()),
        });
    }

    MessageEvent {
        platform: Platform::Telegram,
        message_id: msg.message_id.to_string(),
        chat_id: msg.chat.id.to_string(),
        sender_id: msg
            .from
            .as_ref()
            .map(|u| u.id.to_string())
            .unwrap_or_default(),
        content: msg
            .text
            .clone()
            .or_else(|| msg.caption.clone())
            .unwrap_or_default(),
        attachments,
        thread_id: None,
        chat_type: match msg.chat.chat_type.as_str() {
            "private" => "dm".to_string(),
            "group" | "supergroup" => "group".to_string(),
            "channel" => "channel".to_string(),
            other => other.to_string(),
        },
        chat_name: msg.chat.title.clone(),
        sender_name: msg.from.as_ref().map(|u| u.first_name.clone()),
        replied_to_id: None,
    }
}

// Phase 36.17.5 D-14 / D-15: implement AudioDispatcher for TelegramAdapter.
//
// The trait lives in ironhermes-tools (so tools don't depend on gateway).
// The impl lives here (gateway) because of Rust's orphan rule: both the trait
// and the type must be in scope; neither can cross crate boundaries as a blanket
// impl. Method names (send_voice_file / send_audio_file) are distinct from the
// inherent TelegramAdapter methods (send_voice / send_audio) to avoid ambiguity.
#[async_trait::async_trait]
impl ironhermes_tools::AudioDispatcher for TelegramAdapter {
    async fn send_voice_file(
        &self,
        chat_id: &str,
        path: &std::path::PathBuf,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Use TgSendApi::send_voice (takes &Path) not MediaSender::send_voice (takes MediaSource)
        TgSendApi::send_voice(self, chat_id, path.as_path(), thread_id).await
    }

    async fn send_audio_file(
        &self,
        chat_id: &str,
        path: &std::path::PathBuf,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        // Use inherent send_audio (takes &Path, returns Result<MessageResponse>)
        self.send_audio(chat_id, path.as_path(), thread_id)
            .await
            .map(|_| ())
    }
}
