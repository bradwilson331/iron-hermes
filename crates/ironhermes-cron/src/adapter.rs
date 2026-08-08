use async_trait::async_trait;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Minimal send trait used by the cron-runner crate to dispatch
/// delivery payloads. Implemented by `TelegramAdapter`
/// (ironhermes-gateway) for production and `FakeTgClient` for tests.
///
/// Lives in `ironhermes-cron` (not `ironhermes-gateway`) so that
/// `ironhermes-cron-runner` can depend on the trait without forming
/// a dependency cycle with the gateway.
///
/// The four media methods (`send_voice`, `send_image_file`,
/// `send_video`, `send_document`) were added in Plan 32.1-07 Task 3
/// to support MEDIA tag routing in `dispatch_all_targets`.
#[async_trait]
pub trait TgSendApi: Send + Sync {
    async fn send_message(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a voice message (ogg/opus/mp3/wav/m4a).
    async fn send_voice(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a photo/image (png/jpg/jpeg/gif/webp).
    async fn send_image_file(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a video (mp4/mov/webm/mkv).
    async fn send_video(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;

    /// Send a document (any file not covered by the above types).
    async fn send_document(
        &self,
        chat_id: &str,
        path: &Path,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}

// ---------------------------------------------------------------------------
// DeliverySend / DeliveryRegistry (Phase 47.6 Plan 07)
// ---------------------------------------------------------------------------

/// Minimal, platform-agnostic text-send trait for the cron/notifier delivery
/// registry.
///
/// Lives in `ironhermes-cron` (not `ironhermes-gateway`), for the exact same
/// dependency-cycle reason [`TgSendApi`] above does: `ironhermes-cron-runner`
/// depends on this crate, and a dependency on `ironhermes-gateway` from
/// either cron crate would form a cycle (the gateway itself depends on
/// `ironhermes-cron`/`ironhermes-cron-runner`).
///
/// Deliberately narrower than [`TgSendApi`]: D-15 ships no media sender for
/// Buzz this phase (media arrives as a path-naming text notice instead — see
/// `ironhermes-cron-runner::delivery::buzz_media_notice`), so the general
/// delivery surface needs only a single text-send method. A narrow trait is
/// also what lets a future platform join the registry without having to
/// implement four media methods it cannot support.
#[async_trait]
pub trait DeliverySend: Send + Sync {
    async fn send_text(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> anyhow::Result<()>;
}

/// Platform-keyed registry of [`DeliverySend`] senders.
///
/// Lives in `ironhermes-cron` for the same dependency-cycle reason as
/// [`DeliverySend`] above — so `ironhermes-cron-runner` can depend on it
/// without forming a cycle with `ironhermes-gateway`.
///
/// Lookup is case-insensitive to match `build_notifier_send_fn`'s existing
/// case-insensitive linear search (in `ironhermes-gateway::runner`) over its
/// own adapter snapshot — the two lookups must never disagree, or a
/// subscription that resolves through the notifier could fail through cron,
/// or vice versa.
#[derive(Default)]
pub struct DeliveryRegistry {
    senders: HashMap<String, Arc<dyn DeliverySend>>,
}

impl DeliveryRegistry {
    /// Construct an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `sender` under `platform`. The key is normalized to
    /// lowercase for storage, matching [`Self::get`]'s case-insensitive
    /// lookup — inserting the same platform twice (in any casing) replaces
    /// the previously registered sender.
    pub fn insert(&mut self, platform: impl Into<String>, sender: Arc<dyn DeliverySend>) {
        self.senders.insert(platform.into().to_lowercase(), sender);
    }

    /// Case-insensitive lookup of the sender registered for `platform`.
    /// Returns `None` for an unregistered platform, including when the
    /// registry is empty.
    pub fn get(&self, platform: &str) -> Option<Arc<dyn DeliverySend>> {
        self.senders.get(&platform.to_lowercase()).cloned()
    }
}

#[cfg(test)]
mod delivery_registry_tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeSender {
        calls: Mutex<Vec<(String, String, Option<String>)>>,
    }

    #[async_trait]
    impl DeliverySend for FakeSender {
        async fn send_text(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push((
                chat_id.to_string(),
                content.to_string(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }
    }

    #[tokio::test]
    async fn registry_returns_the_sender_for_a_registered_platform() {
        let sender = Arc::new(FakeSender::default());
        let mut registry = DeliveryRegistry::new();
        registry.insert("telegram", sender.clone() as Arc<dyn DeliverySend>);

        let resolved = registry.get("telegram").expect("telegram must resolve");
        resolved.send_text("chat1", "hello", None).await.unwrap();

        assert_eq!(sender.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn registry_lookup_is_case_insensitive() {
        let sender = Arc::new(FakeSender::default());
        let mut registry = DeliveryRegistry::new();
        registry.insert("Telegram", sender.clone() as Arc<dyn DeliverySend>);

        assert!(registry.get("telegram").is_some());
        assert!(registry.get("TELEGRAM").is_some());
        assert!(registry.get("TeLeGrAm").is_some());
    }

    #[test]
    fn registry_returns_none_for_an_unregistered_platform() {
        let sender = Arc::new(FakeSender::default());
        let mut registry = DeliveryRegistry::new();
        registry.insert("telegram", sender as Arc<dyn DeliverySend>);

        assert!(registry.get("buzz").is_none());
    }

    #[test]
    fn empty_registry_returns_none_for_everything() {
        let registry = DeliveryRegistry::new();
        assert!(registry.get("telegram").is_none());
        assert!(registry.get("buzz").is_none());
        assert!(registry.get("").is_none());
    }

    #[tokio::test]
    async fn registry_holds_multiple_platforms_independently() {
        let tg_sender = Arc::new(FakeSender::default());
        let buzz_sender = Arc::new(FakeSender::default());
        let mut registry = DeliveryRegistry::new();
        registry.insert("telegram", tg_sender.clone() as Arc<dyn DeliverySend>);
        registry.insert("buzz", buzz_sender.clone() as Arc<dyn DeliverySend>);

        registry
            .get("telegram")
            .unwrap()
            .send_text("chat1", "for telegram", None)
            .await
            .unwrap();
        registry
            .get("buzz")
            .unwrap()
            .send_text("chat2", "for buzz", None)
            .await
            .unwrap();

        assert_eq!(tg_sender.calls.lock().unwrap().len(), 1);
        assert_eq!(tg_sender.calls.lock().unwrap()[0].0, "chat1");
        assert_eq!(buzz_sender.calls.lock().unwrap().len(), 1);
        assert_eq!(buzz_sender.calls.lock().unwrap()[0].0, "chat2");
    }
}
