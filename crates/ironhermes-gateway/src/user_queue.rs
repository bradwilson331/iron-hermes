use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use ironhermes_core::MessageEvent;
use crate::adapter::PlatformAdapter;
use tracing::{debug, warn};

/// A message dispatched to a per-chat worker.
pub struct QueuedMessage {
    pub event: MessageEvent,
    /// True if this message was queued behind an in-flight agent run.
    pub is_queued: bool,
    /// Text prefix from document extraction (prepended to user message content).
    pub text_prefix: Option<String>,
    /// Base64-encoded image data URI for vision LLM input.
    pub image_data_uri: Option<String>,
}

/// Manages per-chat message queues. Each unique chat_id gets a dedicated
/// mpsc channel. When a message arrives for a chat that already has an
/// in-flight agent run, the new message is queued and acknowledged with
/// an eye reaction (D-22).
pub struct UserQueueManager {
    senders: Mutex<HashMap<String, mpsc::Sender<QueuedMessage>>>,
    adapter: Arc<dyn PlatformAdapter>,
    queue_capacity: usize,
}

impl UserQueueManager {
    /// Create a new UserQueueManager. `queue_capacity` sets the per-chat buffer size.
    pub fn new(adapter: Arc<dyn PlatformAdapter>, queue_capacity: usize) -> Self {
        Self {
            senders: Mutex::new(HashMap::new()),
            adapter,
            queue_capacity,
        }
    }

    /// Dispatch a message to the appropriate per-chat worker.
    ///
    /// Returns `Some(receiver)` if a new worker must be spawned, `None` if the
    /// message was queued into an existing worker's channel.
    pub async fn dispatch(
        &self,
        event: MessageEvent,
        text_prefix: Option<String>,
        image_data_uri: Option<String>,
    ) -> Option<mpsc::Receiver<QueuedMessage>> {
        let chat_id = event.chat_id.clone();
        let message_id = event.message_id.clone();
        let mut senders = self.senders.lock().await;

        // Attempt to queue into an existing worker, if one is running.
        // We need to handle ownership carefully: try_send takes ownership of the message,
        // and returns it back on error via TrySendError::into_inner().
        if let Some(sender) = senders.get(&chat_id) {
            let msg = QueuedMessage {
                event: event.clone(),
                is_queued: true,
                text_prefix: text_prefix.clone(),
                image_data_uri: image_data_uri.clone(),
            };
            match sender.try_send(msg) {
                Ok(()) => {
                    // Successfully queued behind existing worker — add eye reaction
                    let adapter = self.adapter.clone();
                    let cid = chat_id.clone();
                    let mid = message_id.clone();
                    tokio::spawn(async move {
                        if let Err(e) = adapter.add_reaction(&cid, &mid, "\u{1f440}").await {
                            warn!("Failed to add eye reaction: {}", e);
                        }
                    });
                    debug!(chat_id = %chat_id, "Message queued behind in-flight agent run");
                    return None;
                }
                Err(e) => {
                    // Channel full or closed — reclaim the event and replace the worker
                    let original_msg = e.into_inner();
                    if original_msg.event.chat_id == chat_id {
                        warn!(chat_id = %chat_id, "Replacing stale/full channel");
                        senders.remove(&chat_id);
                        // Fall through to create new worker with the reclaimed event
                        let (tx, rx) = mpsc::channel(self.queue_capacity);
                        // Re-send as non-queued (it's becoming the first message for the new worker)
                        let _ = tx.try_send(QueuedMessage {
                            event: original_msg.event,
                            is_queued: false,
                            text_prefix: original_msg.text_prefix,
                            image_data_uri: original_msg.image_data_uri,
                        });
                        senders.insert(chat_id, tx);
                        return Some(rx);
                    }
                }
            }
        }

        // No active worker — create a fresh channel
        let (tx, rx) = mpsc::channel(self.queue_capacity);
        let _ = tx.try_send(QueuedMessage {
            event,
            is_queued: false,
            text_prefix,
            image_data_uri,
        });
        senders.insert(chat_id, tx);
        Some(rx)
    }

    /// Remove the sender for a chat when its worker exits.
    pub async fn remove(&self, chat_id: &str) {
        let mut senders = self.senders.lock().await;
        senders.remove(chat_id);
        debug!(chat_id = %chat_id, "Per-chat worker exited, removed queue entry");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::{MessageResponse, Platform};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct MockAdapter {
        reaction_count: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl PlatformAdapter for MockAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }

        async fn send_message(
            &self,
            chat_id: &str,
            _content: &str,
            _thread_id: Option<&str>,
        ) -> anyhow::Result<MessageResponse> {
            Ok(MessageResponse {
                message_id: "1".into(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }

        async fn edit_message(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn edit_message_markdown(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn delete_message(
            &self,
            _chat_id: &str,
            _message_id: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn add_reaction(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _emoji: &str,
        ) -> anyhow::Result<()> {
            self.reaction_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn is_running(&self) -> bool {
            true
        }
    }

    fn make_event(chat_id: &str, message_id: &str) -> MessageEvent {
        MessageEvent {
            platform: Platform::Telegram,
            message_id: message_id.to_string(),
            chat_id: chat_id.to_string(),
            sender_id: "user1".to_string(),
            content: "hello".to_string(),
            attachments: vec![],
            thread_id: None,
            chat_type: "dm".to_string(),
            chat_name: None,
            sender_name: None,
            replied_to_id: None,
        }
    }

    #[tokio::test]
    async fn test_first_dispatch_returns_some_receiver() {
        let reaction_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(MockAdapter {
            reaction_count: reaction_count.clone(),
        });
        let manager = UserQueueManager::new(adapter, 16);

        let rx = manager.dispatch(make_event("chat1", "msg1"), None, None).await;
        assert!(rx.is_some(), "First dispatch should return Some(receiver)");
    }

    #[tokio::test]
    async fn test_second_dispatch_returns_none_and_adds_reaction() {
        let reaction_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(MockAdapter {
            reaction_count: reaction_count.clone(),
        });
        let manager = UserQueueManager::new(adapter, 16);

        // First dispatch — creates worker channel (don't consume rx so channel stays alive)
        let _rx = manager.dispatch(make_event("chat1", "msg1"), None, None).await;

        // Second dispatch for same chat — should queue and return None
        let rx2 = manager.dispatch(make_event("chat1", "msg2"), None, None).await;
        assert!(
            rx2.is_none(),
            "Second dispatch should return None (worker already running)"
        );

        // Give the spawned reaction task time to run
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(
            reaction_count.load(Ordering::SeqCst),
            1,
            "Should add one eye reaction for queued message"
        );
    }

    #[tokio::test]
    async fn test_different_chats_get_independent_workers() {
        let reaction_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(MockAdapter {
            reaction_count: reaction_count.clone(),
        });
        let manager = UserQueueManager::new(adapter, 16);

        let rx1 = manager.dispatch(make_event("chat1", "msg1"), None, None).await;
        let rx2 = manager.dispatch(make_event("chat2", "msg2"), None, None).await;
        assert!(rx1.is_some(), "chat1 should get a worker");
        assert!(rx2.is_some(), "chat2 should get independent worker");
        assert_eq!(
            reaction_count.load(Ordering::SeqCst),
            0,
            "No reactions for first messages of each chat"
        );
    }

    #[tokio::test]
    async fn test_remove_clears_entry_and_new_dispatch_creates_worker() {
        let reaction_count = Arc::new(AtomicUsize::new(0));
        let adapter = Arc::new(MockAdapter { reaction_count });
        let manager = UserQueueManager::new(adapter, 16);

        let _rx = manager.dispatch(make_event("chat1", "msg1"), None, None).await;
        manager.remove("chat1").await;

        // After remove, next dispatch should return Some (fresh worker)
        let rx2 = manager.dispatch(make_event("chat1", "msg3"), None, None).await;
        assert!(
            rx2.is_some(),
            "After remove, dispatch should create a new worker"
        );
    }
}
