//! Phase 36.3.7.5 BUG-36.3.7.5-06: production KanbanStoreWriter impl.
//!
//! Lives in ironhermes-kanban (NOT ironhermes-cli) because the gateway needs
//! to construct it at CommandContext build-time and ironhermes-cli already
//! depends on ironhermes-gateway (the reverse direction would be circular).
//!
//! Each method opens a fresh `KanbanStore::open_default()` and discards it
//! after the call. Stateless; safe to clone the trait-object Arc into multiple
//! contexts; no shared mutable state at the impl layer.

use ironhermes_core::commands::context::{KanbanStoreWriter, SubscriptionView};

use crate::store::{CreateTaskOptions, KanbanStore};
use crate::types::Subscription;

/// Production impl that opens the default kanban DB per call.
/// Phase 36.3.7.5 BUG-36.3.7.5-06.
pub struct KanbanStoreWriterImpl;

impl KanbanStoreWriterImpl {
    pub fn new() -> Self {
        Self
    }
}

impl Default for KanbanStoreWriterImpl {
    fn default() -> Self {
        Self::new()
    }
}

fn open_store() -> Result<KanbanStore, String> {
    KanbanStore::open_default().map_err(|e| format!("open kanban.db: {}", e))
}

fn to_view(s: Subscription) -> SubscriptionView {
    SubscriptionView {
        id: s.id,
        task_id: s.task_id,
        platform: s.platform,
        chat_id: s.chat_id,
        thread_id: s.thread_id,
        source: s.source,
        created_at: s.created_at,
    }
}

impl KanbanStoreWriter for KanbanStoreWriterImpl {
    fn create_task_simple(
        &self,
        title: &str,
        assignee: &str,
        _json: bool,
    ) -> Result<String, String> {
        let mut store = open_store()?;
        let opts = CreateTaskOptions::default();
        store
            .create_task(title, assignee, opts)
            .map(|t| t.id)
            .map_err(|e| format!("create_task: {}", e))
    }

    fn append_subscription(
        &self,
        task_id: &str,
        platform: &str,
        chat_id: &str,
        thread_id: Option<&str>,
        source: &str,
    ) -> Result<i64, String> {
        let mut store = open_store()?;
        store
            .append_subscription(task_id, platform, chat_id, thread_id, source)
            .map_err(|e| format!("append_subscription: {}", e))
    }

    fn list_subscriptions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<SubscriptionView>, String> {
        let store = open_store()?;
        store
            .list_subscriptions_for_task(task_id)
            .map(|v| v.into_iter().map(to_view).collect())
            .map_err(|e| format!("list_subscriptions_for_task: {}", e))
    }

    fn list_all_subscriptions(&self) -> Result<Vec<SubscriptionView>, String> {
        let store = open_store()?;
        store
            .list_all_subscriptions()
            .map(|v| v.into_iter().map(to_view).collect())
            .map_err(|e| format!("list_all_subscriptions: {}", e))
    }

    fn remove_subscriptions(
        &self,
        task_id: &str,
        platform: Option<&str>,
        chat_id: Option<&str>,
    ) -> Result<usize, String> {
        let mut store = open_store()?;
        store
            .remove_subscriptions(task_id, platform, chat_id)
            .map_err(|e| format!("remove_subscriptions: {}", e))
    }
}
