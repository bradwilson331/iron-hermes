// Phase 18 Plan 04: default `context:pre_compress` handler that flushes the
// active MemoryProvider via `sync_turn` before destructive pruning runs
// (D-20 / PRMT-16). Handler failures are logged via `tracing::warn!` and
// swallowed so the engine proceeds with compression (D-22, T-18-07).

use ironhermes_core::MemoryProvider;
use ironhermes_hooks::{AsyncHookListener, HookEvent, HookEventKind};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Build an async hook listener that calls `MemoryProvider::sync_turn` when a
/// `ContextPreCompress` event fires. All other event kinds are ignored.
///
/// The returned listener is `Arc`-cheap-to-clone and can be registered via
/// [`ironhermes_hooks::HookRegistry::add_async_listener`].
pub fn build_memory_flush_listener(
    provider: Arc<Mutex<dyn MemoryProvider + Send>>,
) -> AsyncHookListener {
    Arc::new(move |event: HookEvent| {
        let provider = Arc::clone(&provider);
        Box::pin(async move {
            if let HookEventKind::ContextPreCompress { session_id, .. } = &event.kind {
                let sid = session_id.clone();
                let guard = provider.lock().await;
                // Snapshot current MemoryEntries from the provider and hand to sync_turn.
                let entries = guard.to_memory_entries();
                if let Err(e) = guard.sync_turn(&sid, &entries).await {
                    tracing::warn!(
                        error = ?e,
                        session_id = %sid,
                        "memory flush failed during context:pre_compress"
                    );
                }
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ironhermes_core::memory_store::MemoryResult;
    use ironhermes_core::{MemoryEntries, MemoryProviderConfig, MemoryTarget};
    use ironhermes_hooks::{HookRegistry, HooksConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Minimal MemoryProvider that records sync_turn invocations.
    struct MockProvider {
        sync_calls: Arc<AtomicUsize>,
        last_session: Arc<Mutex<Option<String>>>,
    }

    impl MockProvider {
        fn new() -> (Self, Arc<AtomicUsize>, Arc<Mutex<Option<String>>>) {
            let sync_calls = Arc::new(AtomicUsize::new(0));
            let last_session = Arc::new(Mutex::new(None));
            let provider = Self {
                sync_calls: sync_calls.clone(),
                last_session: last_session.clone(),
            };
            (provider, sync_calls, last_session)
        }
    }

    #[async_trait]
    impl MemoryProvider for MockProvider {
        async fn initialize(&mut self, _cfg: &MemoryProviderConfig) -> anyhow::Result<()> {
            Ok(())
        }
        async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
            Ok(MemoryEntries::default())
        }
        async fn sync_turn(
            &self,
            session_id: &str,
            _entries: &MemoryEntries,
        ) -> anyhow::Result<()> {
            self.sync_calls.fetch_add(1, Ordering::SeqCst);
            *self.last_session.lock().await = Some(session_id.to_string());
            Ok(())
        }
        async fn on_session_end(
            &self,
            _session_id: &str,
            _entries: &MemoryEntries,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn shutdown(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn load_from_disk(&mut self) -> anyhow::Result<()> {
            Ok(())
        }
        fn add(&mut self, _target: MemoryTarget, _content: &str) -> MemoryResult {
            Err("not supported".into())
        }
        fn replace(
            &mut self,
            _target: MemoryTarget,
            _old_text: &str,
            _new_content: &str,
        ) -> MemoryResult {
            Err("not supported".into())
        }
        fn remove(&mut self, _target: MemoryTarget, _old_text: &str) -> MemoryResult {
            Err("not supported".into())
        }
        fn format_for_system_prompt(&self, _target: MemoryTarget) -> Option<String> {
            None
        }
        fn to_memory_entries(&self) -> MemoryEntries {
            MemoryEntries::default()
        }
    }

    #[tokio::test]
    async fn build_memory_flush_listener_calls_sync_turn() {
        let (provider, sync_calls, last_session) = MockProvider::new();
        let provider: Arc<Mutex<dyn MemoryProvider + Send>> = Arc::new(Mutex::new(provider));

        let listener = build_memory_flush_listener(provider);
        let mut registry = HookRegistry::new(HooksConfig::default());
        registry.add_async_listener(listener);

        let event = HookEvent::new(
            "req-1",
            HookEventKind::ContextPreCompress {
                session_id: "sess-42".into(),
                estimated_tokens: 100,
                threshold: 0.5,
                mode: "hard".into(),
                pruned_range: None,
            },
        );
        registry.fire_awaitable(event).await;

        assert_eq!(sync_calls.load(Ordering::SeqCst), 1);
        assert_eq!(last_session.lock().await.as_deref(), Some("sess-42"));
    }

    #[tokio::test]
    async fn listener_ignores_non_pre_compress_events() {
        let (provider, sync_calls, _) = MockProvider::new();
        let provider: Arc<Mutex<dyn MemoryProvider + Send>> = Arc::new(Mutex::new(provider));

        let listener = build_memory_flush_listener(provider);
        let mut registry = HookRegistry::new(HooksConfig::default());
        registry.add_async_listener(listener);

        let event = HookEvent::new(
            "req-1",
            HookEventKind::MessageReceived {
                platform: "x".into(),
                chat_id: "1".into(),
                content_preview: "hi".into(),
            },
        );
        registry.fire_awaitable(event).await;
        assert_eq!(sync_calls.load(Ordering::SeqCst), 0);
    }
}
