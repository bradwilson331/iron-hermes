// Phase 34 Plan 04: SlackAdapter — PlatformAdapter impl + Socket Mode runner
//
// Implements D-11: SlackAdapter routes Slack Socket Mode events through
// GatewayMessageHandler.handle_with_multimodal, inheriting the Learning Loop
// (nudge + skill_manage) structurally.
//
// Two-token shape (Pitfall 2):
//   - app_token (xapp-...) — Socket Mode WebSocket connection
//   - bot_token (xoxb-...) — chat.postMessage / chat.update / chat.delete API calls
//
// T-34-01 mitigation: tokens are NEVER passed to tracing macros.
// T-34-02 mitigation: canonical whitelist empty = deny-all (mirrors runner.rs:601-611, D-12).
// T-34-04 mitigation: callback returns Ok(()) immediately; handler dispatched via tokio::spawn.
//
// API deviation (WARNING 4 closure — supply-chain confirmed):
// UserCallbackFunction is a plain fn-pointer type in slack-morphism 2.22.0, NOT a closure type.
// Captured state is threaded through SlackClientEventsListenerEnvironment::with_user_state<T>()
// and read from the SlackClientEventsUserState parameter inside the callback.

use crate::adapter::PlatformAdapter;
use crate::handler::GatewayMessageHandler;
use crate::multimodal::ProcessedAttachments;
use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{MessageEvent, MessageResponse, Platform};
use slack_morphism::prelude::*;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// =============================================================================
// SlackAdapter — PlatformAdapter impl
// =============================================================================

/// Platform adapter for Slack using the slack-morphism Web API client.
///
/// The `client` field is reused across API calls (send/edit/delete).
/// The `bot_token` authorizes Web API calls (xoxb-...).
/// The Socket Mode connection uses the app_token (xapp-...) in `run_slack_adapter`.
pub struct SlackAdapter {
    client: Arc<SlackHyperClient>,
    bot_token: SlackApiToken,
}

impl SlackAdapter {
    pub fn new(client: Arc<SlackHyperClient>, bot_token: SlackApiToken) -> Self {
        Self { client, bot_token }
    }
}

#[async_trait]
impl PlatformAdapter for SlackAdapter {
    fn platform(&self) -> Platform {
        Platform::Slack
    }

    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        let session = self.client.open_session(&self.bot_token);
        let req = SlackApiChatPostMessageRequest::new(
            chat_id.into(),
            SlackMessageContent::new().with_text(content.to_string()),
        );
        let resp = session
            .chat_post_message(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Slack send_message failed: {e}"))?;
        Ok(MessageResponse {
            message_id: resp.ts.to_string(),
            chat_id: chat_id.to_string(),
            platform: Platform::Slack,
        })
    }

    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()> {
        let session = self.client.open_session(&self.bot_token);
        let req = SlackApiChatUpdateRequest::new(
            chat_id.into(),
            SlackMessageContent::new().with_text(content.to_string()),
            message_id.into(),
        );
        session
            .chat_update(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Slack edit_message failed: {e}"))?;
        Ok(())
    }

    async fn edit_message_markdown(
        &self,
        chat_id: &str,
        message_id: &str,
        content: &str,
    ) -> Result<()> {
        // Slack auto-formats mrkdwn natively — same code path as plain edit.
        self.edit_message(chat_id, message_id, content).await
    }

    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()> {
        let session = self.client.open_session(&self.bot_token);
        let req = SlackApiChatDeleteRequest::new(chat_id.into(), message_id.into());
        session
            .chat_delete(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Slack delete_message failed: {e}"))?;
        Ok(())
    }

    // add_reaction and send_chat_action: use trait default no-ops.
    // Slack supports both, but neither is required for Phase 34 Learning Loop parity.

    fn is_running(&self) -> bool {
        // Lifecycle managed by GatewayRunner; adapter itself has no running state.
        false
    }
}

// =============================================================================
// slack_event_to_message_event — event converter
// =============================================================================

/// Classifies Slack channel type from channel ID string.
///
/// Slack channel-ID convention:
///   D... = DM (direct message)
///   C... = public channel
///   G... = private channel / group DM
///
/// Extracted as pub(crate) fn for unit-testability without constructing slack-morphism types.
pub(crate) fn classify_slack_channel_type(channel: &str) -> &'static str {
    if channel.starts_with('D') {
        "dm"
    } else {
        "group"
    }
}

/// Convert a `SlackMessageEvent` into a platform-agnostic `MessageEvent`.
///
/// Mirrors `tg_message_to_event` in telegram.rs (lines 378-431).
/// Phase 34 is text-only; attachment processing is deferred.
pub fn slack_event_to_message_event(event: &SlackMessageEvent) -> MessageEvent {
    let chat_id = event
        .origin
        .channel
        .as_ref()
        .map(|c| c.to_string())
        .unwrap_or_default();

    MessageEvent {
        platform: Platform::Slack,
        message_id: event.origin.ts.to_string(),
        chat_type: classify_slack_channel_type(&chat_id).to_string(),
        chat_id,
        sender_id: event
            .sender
            .user
            .as_ref()
            .map(|u| u.to_string())
            .unwrap_or_default(),
        content: event
            .content
            .as_ref()
            .and_then(|c| c.text.clone())
            .unwrap_or_default(),
        attachments: vec![], // Phase 34: text-only; attachment support deferred
        thread_id: event.origin.thread_ts.as_ref().map(|t| t.to_string()),
        chat_name: None,     // channel name lookup is a separate Web API call — deferred
        sender_name: None,   // user name lookup is a separate Web API call — deferred
        replied_to_id: None, // thread parent captured in thread_id above
    }
}

// =============================================================================
// SlackAdapterState — shared state threaded through UserState
// =============================================================================

/// State shared between the Socket Mode callback (fn pointer) and the adapter.
///
/// slack-morphism 2.22 `UserCallbackFunction` is a bare fn pointer type — closures
/// that capture variables cannot be coerced to fn pointers. State is instead stored
/// in `SlackClientEventsListenerEnvironment::with_user_state<T>()` and retrieved
/// from the `SlackClientEventsUserState` RwLock in the callback body.
///
/// This struct bundles all state that the push-events callback needs.
struct SlackAdapterState {
    whitelist: Vec<String>,
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
    adapter: Arc<dyn PlatformAdapter>,
}

/// Socket Mode push-events callback — must be a fn pointer (not a closure).
///
/// State (whitelist, handler, cancel, adapter) is retrieved from `state_storage`.
async fn on_push_event(
    callback: SlackPushEventCallback,
    _client: Arc<SlackHyperClient>,
    state_storage: SlackClientEventsUserState,
) -> UserCallbackResult<()> {
    // Extract SlackMessageEvent — skip non-message events with an immediate Ok.
    let msg_event = match callback.event {
        SlackEventCallbackBody::Message(ref msg) => msg.clone(),
        _ => return Ok(()),
    };

    // Skip bot messages to avoid feedback loops.
    if msg_event.sender.bot_id.is_some() {
        return Ok(());
    }

    // Skip message-changed / message-deleted subtypes — only process new messages.
    if let Some(ref subtype) = msg_event.subtype {
        match subtype {
            SlackMessageEventType::BotMessage
            | SlackMessageEventType::MessageChanged
            | SlackMessageEventType::MessageDeleted => return Ok(()),
            _ => {}
        }
    }

    let sender_id = msg_event
        .sender
        .user
        .as_ref()
        .map(|u| u.to_string())
        .unwrap_or_default();

    // Read shared state from the RwLock. Read access is non-exclusive and fast.
    let state_read = state_storage.read().await;
    let state = match state_read.get_user_state::<SlackAdapterState>() {
        Some(s) => s,
        None => {
            tracing::error!("SlackAdapterState not found in user state — this is a bug");
            return Ok(());
        }
    };

    // CANONICAL whitelist check — mirrors runner.rs:601-611 + config.rs:731 (D-12).
    // Empty whitelist = deny all messages (D-12).
    if !state.whitelist.is_empty() {
        if !state.whitelist.contains(&sender_id) {
            tracing::warn!(
                sender_id = %sender_id,
                "Sender not in whitelist, ignoring"
            );
            return Ok(());
        }
    } else {
        tracing::warn!("Whitelist is empty — denying all messages (D-12)");
        return Ok(());
    }

    // Build MessageEvent + ProcessedAttachments BEFORE spawning (borrow-checker safe).
    // This ensures the callback returns Ok(()) within ~3s (T-34-04 ACK timing mitigation).
    let event_for_handler = slack_event_to_message_event(&msg_event);
    let processed = ProcessedAttachments { text_prefix: None, image_data_uri: None };
    let h = state.handler.clone();
    let a = state.adapter.clone();
    let c = state.cancel.child_token();

    // Drop the state read guard BEFORE spawning to avoid holding it across task boundaries.
    drop(state_read);

    // T-34-04: Non-blocking ACK — spawn handler in background, return Ok(()) immediately.
    tokio::spawn(async move {
        if let Err(e) = h.handle_with_multimodal(&event_for_handler, a, c, processed).await {
            tracing::error!("Slack handler error: {e:#}");
        }
    });

    Ok(())
}

/// Socket Mode error handler — must be a fn pointer (not a closure).
fn on_socket_mode_error(
    err: Box<dyn std::error::Error + Send + Sync>,
    _client: Arc<SlackHyperClient>,
    _state: SlackClientEventsUserState,
) -> HttpStatusCode {
    tracing::warn!("Slack socket mode error: {err}");
    HttpStatusCode::OK
}

// =============================================================================
// run_slack_adapter — Socket Mode listener + cancellation + whitelist
// =============================================================================

/// Start the Slack adapter in Socket Mode.
///
/// Blocks until either:
/// - `listener.listen_for()` returns (error or Slack disconnect), or
/// - `cancel` is signalled, which returns Ok(()) for clean shutdown.
///
/// # Security
/// - T-34-01: Both tokens are NEVER logged (only "...redacted" appears in logs).
/// - T-34-02: Canonical whitelist — empty = deny-all (mirrors runner.rs:601-611, D-12).
/// - T-34-04: Callback returns Ok(()) immediately; handler is dispatched via
///   `tokio::spawn(async move { ... })` with owned values, satisfying the 3-second
///   Slack Socket Mode ACK deadline.
pub async fn run_slack_adapter(
    app_token: &str,
    bot_token: &str,
    whitelist: Vec<String>,
    handler: Arc<GatewayMessageHandler>,
    cancel: CancellationToken,
) -> Result<()> {
    // T-34-01: Log startup with redacted token prefixes only — never the actual token strings.
    tracing::info!(
        "Slack Socket Mode adapter starting (app_token: xapp-...redacted, bot_token: xoxb-...redacted)"
    );

    let client: Arc<SlackHyperClient> = Arc::new(SlackClient::new(
        SlackClientHyperConnector::new()
            .map_err(|e| anyhow::anyhow!("Slack connector init failed: {e}"))?,
    ));

    let bot_token_obj = SlackApiToken::new(bot_token.into());
    let app_token_obj = SlackApiToken::new(app_token.into());

    let adapter: Arc<dyn PlatformAdapter> =
        Arc::new(SlackAdapter::new(client.clone(), bot_token_obj));

    let socket_mode_callbacks =
        SlackSocketModeListenerCallbacks::new().with_push_events(on_push_event);

    // Store shared state in the listener environment's user state store.
    // This is the only way to thread captured state into fn-pointer callbacks
    // in slack-morphism 2.22 (UserCallbackFunction is a bare fn pointer type).
    let listener_environment = Arc::new(
        SlackClientEventsListenerEnvironment::new(client.clone())
            .with_error_handler(on_socket_mode_error)
            .with_user_state(SlackAdapterState {
                whitelist,
                handler,
                cancel: cancel.clone(),
                adapter,
            }),
    );

    let socket_mode_listener = SlackClientSocketModeListener::new(
        &SlackClientSocketModeConfig::new(),
        listener_environment,
        socket_mode_callbacks,
    );

    // CancellationToken integration: select between listener and cancel signal.
    tokio::select! {
        result = socket_mode_listener.listen_for(&app_token_obj) => {
            result.map_err(|e| anyhow::anyhow!("Slack Socket Mode listener error: {e}"))
        }
        _ = cancel.cancelled() => {
            // Clean shutdown — listener is dropped when this arm is taken.
            Ok(())
        }
    }
}

// =============================================================================
// Unit tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_slack_channel_type_dm_starts_with_d() {
        assert_eq!(classify_slack_channel_type("D0123ABC"), "dm");
    }

    #[test]
    fn classify_slack_channel_type_channel_starts_with_c() {
        assert_eq!(classify_slack_channel_type("C0123ABC"), "group");
    }

    #[test]
    fn classify_slack_channel_type_group_starts_with_g() {
        assert_eq!(classify_slack_channel_type("G0123ABC"), "group");
    }
}
