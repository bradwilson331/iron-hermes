use anyhow::{Context, Result};
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::adapter::{MessageHandler, PlatformAdapter};

const TELEGRAM_API_BASE: &str = "https://api.telegram.org";

/// Telegram Bot API adapter using long polling.
pub struct TelegramAdapter {
    token: String,
    http: Client,
    running: Arc<AtomicBool>,
    poll_handle: Option<tokio::task::JoinHandle<()>>,
}

impl TelegramAdapter {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: token.into(),
            http: Client::new(),
            running: Arc::new(AtomicBool::new(false)),
            poll_handle: None,
        }
    }

    fn api_url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", TELEGRAM_API_BASE, self.token, method)
    }

    async fn api_call<T: serde::de::DeserializeOwned>(
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
}

#[async_trait]
impl PlatformAdapter for TelegramAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn start(&mut self, handler: Box<dyn MessageHandler>) -> Result<()> {
        info!("Starting Telegram adapter (long polling)");
        self.running.store(true, Ordering::SeqCst);

        let token = self.token.clone();
        let http = self.http.clone();
        let running = self.running.clone();
        let handler = Arc::new(handler);

        // Verify bot token by calling getMe
        let me: TgUser = self
            .api_call("getMe", &serde_json::json!({}))
            .await
            .context("Failed to verify Telegram bot token")?;
        info!(
            bot_name = %me.first_name,
            username = ?me.username,
            "Telegram bot connected"
        );

        let handle = tokio::spawn(async move {
            let mut offset: Option<i64> = None;

            while running.load(Ordering::SeqCst) {
                let params = serde_json::json!({
                    "timeout": 30,
                    "offset": offset,
                    "allowed_updates": ["message"],
                });

                let url = format!("{}/bot{}/getUpdates", TELEGRAM_API_BASE, token);
                let result = http.post(&url).json(&params).send().await;

                match result {
                    Ok(response) => {
                        let body: std::result::Result<TelegramResponse<Vec<TgUpdate>>, _> =
                            response.json().await;
                        match body {
                            Ok(resp) if resp.ok => {
                                if let Some(updates) = resp.result {
                                    for update in updates {
                                        offset = Some(update.update_id + 1);

                                        if let Some(message) = update.message {
                                            let event = tg_message_to_event(&message);
                                            let handler = handler.clone();
                                            let http = http.clone();
                                            let token = token.clone();

                                            tokio::spawn(async move {
                                                match handler.handle(&event).await {
                                                    Ok(response_text) => {
                                                        if response_text.is_empty() {
                                                            return;
                                                        }
                                                        let params = serde_json::json!({
                                                            "chat_id": event.chat_id,
                                                            "text": response_text,
                                                            "parse_mode": "Markdown",
                                                        });
                                                        let url = format!(
                                                            "{}/bot{}/sendMessage",
                                                            TELEGRAM_API_BASE, token
                                                        );
                                                        if let Err(e) = http
                                                            .post(&url)
                                                            .json(&params)
                                                            .send()
                                                            .await
                                                        {
                                                            error!("Failed to send Telegram response: {}", e);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        error!("Handler error: {}", e);
                                                    }
                                                }
                                            });
                                        }
                                    }
                                }
                            }
                            Ok(resp) => {
                                warn!(
                                    "Telegram getUpdates failed: {}",
                                    resp.description.unwrap_or_default()
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
                            Err(e) => {
                                warn!("Failed to parse Telegram updates: {}", e);
                                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Telegram poll error: {}", e);
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
        });

        self.poll_handle = Some(handle);
        Ok(())
    }

    async fn stop(&mut self) -> Result<()> {
        info!("Stopping Telegram adapter");
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.poll_handle.take() {
            handle.abort();
        }
        Ok(())
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
            "parse_mode": "Markdown",
        });

        let result: TgMessage = self.api_call("sendMessage", &params).await?;

        Ok(MessageResponse {
            message_id: result.message_id.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn edit_message(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
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
        // Markdown parse mode on final edit only per D-03
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

    async fn add_reaction(
        &self,
        chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<()> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "message_id": message_id.parse::<i64>().unwrap_or(0),
            "reaction": [{"type": "emoji", "emoji": emoji}],
        });

        let _: serde_json::Value = self.api_call("setMessageReaction", &params).await?;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
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

#[derive(Debug, Deserialize)]
struct TgUpdate {
    update_id: i64,
    message: Option<TgMessage>,
}

#[derive(Debug, Deserialize)]
struct TgMessage {
    message_id: i64,
    from: Option<TgUser>,
    chat: TgChat,
    text: Option<String>,
    #[allow(dead_code)]
    date: i64,
}

#[derive(Debug, Deserialize)]
struct TgUser {
    id: i64,
    first_name: String,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TgChat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
    title: Option<String>,
}

fn tg_message_to_event(msg: &TgMessage) -> MessageEvent {
    MessageEvent {
        platform: Platform::Telegram,
        message_id: msg.message_id.to_string(),
        chat_id: msg.chat.id.to_string(),
        sender_id: msg
            .from
            .as_ref()
            .map(|u| u.id.to_string())
            .unwrap_or_default(),
        content: msg.text.clone().unwrap_or_default(),
        attachments: Vec::new(),
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
