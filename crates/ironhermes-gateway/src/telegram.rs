use crate::adapter::PlatformAdapter;
use anyhow::{Context, Result};
use async_trait::async_trait;
use ironhermes_core::{Attachment, MessageEvent, MessageResponse, Platform};
// TgSendApi trait now lives in ironhermes-cron::adapter (Phase 32.1 Plan 06)
pub use ironhermes_cron::TgSendApi;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::Path;

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
        self.send_file_multipart("sendVoice", "voice", chat_id, path, thread_id).await
    }

    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.send_file_multipart("sendPhoto", "photo", chat_id, path, thread_id).await
    }

    async fn send_video(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.send_file_multipart("sendVideo", "video", chat_id, path, thread_id).await
    }

    async fn send_document(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()> {
        self.send_file_multipart("sendDocument", "document", chat_id, path, thread_id).await
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
    async fn send_file_multipart(
        &self,
        method: &str,
        field_name: &str,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> Result<()> {
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
        let body: TelegramResponse<serde_json::Value> = response
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

        Ok(())
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

    async fn edit_message_markdown(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
            "text": content,
            "parse_mode": "Markdown",
        });

        let _: serde_json::Value = self.api_call("editMessageText", &params).await?;
        Ok(())
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
