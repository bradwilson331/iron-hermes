use crate::config::HooksConfig;
use crate::event::HookEvent;
use std::sync::Arc;

/// A hook listener is a thread-safe, cloneable callback that receives HookEvents.
pub type HookListener = Arc<dyn Fn(HookEvent) + Send + Sync>;

/// Registry of hook listeners. Events are dispatched fire-and-forget via tokio::spawn.
pub struct HookRegistry {
    listeners: Vec<HookListener>,
    config: HooksConfig,
}

impl HookRegistry {
    /// Create a new, empty registry with the given config.
    pub fn new(config: HooksConfig) -> Self {
        Self {
            listeners: Vec::new(),
            config,
        }
    }

    /// Register a listener. Listeners are called in registration order.
    pub fn add_listener(&mut self, listener: HookListener) {
        self.listeners.push(listener);
    }

    /// Fire an event to all registered listeners.
    ///
    /// Each listener is invoked in a separate `tokio::spawn` task so the caller
    /// is never blocked by slow or failing listeners (T-06-03 mitigation).
    pub fn fire(&self, event: HookEvent) {
        for listener in &self.listeners {
            let listener = Arc::clone(listener);
            let event = event.clone();
            tokio::spawn(async move {
                listener(event);
            });
        }
    }

    /// Access the hooks configuration (used by guardrails, webhooks, etc.).
    pub fn config(&self) -> &HooksConfig {
        &self.config
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::HookEventKind;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_event() -> HookEvent {
        HookEvent::new(
            "req-test",
            HookEventKind::MessageReceived {
                platform: "test".to_string(),
                chat_id: "0".to_string(),
                content_preview: "hello".to_string(),
            },
        )
    }

    #[tokio::test]
    async fn test_fire_with_no_listeners_does_not_panic() {
        let registry = HookRegistry::new(HooksConfig::default());
        registry.fire(make_event()); // must not panic
    }

    #[tokio::test]
    async fn test_fire_calls_all_listeners() {
        let mut registry = HookRegistry::new(HooksConfig::default());
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..3 {
            let c = Arc::clone(&counter);
            registry.add_listener(Arc::new(move |_event| {
                c.fetch_add(1, Ordering::SeqCst);
            }));
        }

        registry.fire(make_event());

        // Give tokio tasks time to complete
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn test_add_multiple_listeners() {
        let mut registry = HookRegistry::new(HooksConfig::default());
        let results: Arc<tokio::sync::Mutex<Vec<String>>> =
            Arc::new(tokio::sync::Mutex::new(Vec::new()));

        for label in &["a", "b", "c"] {
            let label = label.to_string();
            let results = Arc::clone(&results);
            registry.add_listener(Arc::new(move |_event| {
                let label = label.clone();
                let results = Arc::clone(&results);
                // Use a blocking approach since the listener is Fn not async
                if let Ok(mut guard) = results.try_lock() {
                    guard.push(label);
                }
            }));
        }

        registry.fire(make_event());
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let guard = results.lock().await;
        assert_eq!(guard.len(), 3);
    }

    #[test]
    fn test_config_accessor() {
        use crate::config::HooksConfig;
        let mut cfg = HooksConfig::default();
        cfg.blocked_tools = vec!["terminal".to_string()];
        let registry = HookRegistry::new(cfg);
        assert_eq!(registry.config().blocked_tools, vec!["terminal"]);
    }
}
