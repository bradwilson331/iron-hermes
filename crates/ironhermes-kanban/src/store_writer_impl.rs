//! Phase 36.3.7.5 BUG-36.3.7.5-06: production KanbanStoreWriter impl.
//!
//! Lives in ironhermes-kanban (NOT ironhermes-cli) because the gateway needs
//! to construct it at CommandContext build-time and ironhermes-cli already
//! depends on ironhermes-gateway (the reverse direction would be circular).
//!
//! Each method opens a fresh `KanbanStore::open_from_env()` and discards it
//! after the call. Stateless; safe to clone the trait-object Arc into multiple
//! contexts; no shared mutable state at the impl layer.
//!
//! Phase 36.3.7.13 D-A1: opens are env-bridged via `IRONIRONHERMES_KANBAN_DB` so
//! dashboard write paths share the same DB as the dispatcher when run under
//! a non-default profile.

use ironhermes_core::commands::context::{KanbanStoreWriter, SubscriptionView};

use crate::config::{DefaultNotifyTarget, subscribe_default_notify};
use crate::store::{CreateTaskOptions, KanbanStore};
use crate::types::Subscription;

/// Production impl that opens the default kanban DB per call.
/// Phase 36.3.7.5 BUG-36.3.7.5-06.
///
/// Phase 46.5-04 D-06: optionally carries an operator-config-sourced
/// `default_notify` target so `create_task_simple` (the programmatic,
/// non-chat-origin task-creation path) auto-subscribes the same way
/// `/kanban create`'s chat-origin hook does.
///
/// SECURITY (T-46.5-20): `default_notify` MUST be constructed by the caller
/// exclusively from `KanbanConfig.default_notify` (operator config) — never
/// from task-controlled data.
pub struct KanbanStoreWriterImpl {
    default_notify: Option<DefaultNotifyTarget>,
}

impl KanbanStoreWriterImpl {
    pub fn new() -> Self {
        Self {
            default_notify: None,
        }
    }

    /// Construct with an operator-config-sourced `default_notify` target
    /// (D-06). `target` must come from `KanbanConfig.default_notify` only —
    /// see the security note on the struct.
    pub fn with_default_notify(target: Option<DefaultNotifyTarget>) -> Self {
        Self {
            default_notify: target,
        }
    }
}

impl Default for KanbanStoreWriterImpl {
    fn default() -> Self {
        Self::new()
    }
}

fn open_store() -> Result<KanbanStore, String> {
    // Phase 36.3.7.13 D-A1: env-bridged open so the dashboard write paths
    // (which run in the same dispatcher-bridged context as the SPA reads)
    // honor IRONIRONHERMES_KANBAN_DB. Tracked in RESEARCH.md Risk 2.
    KanbanStore::open_from_env().map_err(|e| format!("open kanban.db: {}", e))
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
        let task = store
            .create_task(title, assignee, opts)
            .map_err(|e| format!("create_task: {}", e))?;

        // Phase 46.5-04 D-06: auto-subscribe the operator's configured
        // default_notify target (source="auto"), mirroring the chat-origin
        // hook, for this non-chat-origin creation path. Non-fatal — the
        // task was already created successfully; a subscribe failure must
        // not fail task creation.
        if let Some(ref target) = self.default_notify
            && let Err(e) = subscribe_default_notify(&mut store, target, &task.id)
        {
            tracing::warn!(
                task_id = %task.id,
                error = %e,
                "failed to auto-subscribe default_notify target"
            );
        }

        Ok(task.id)
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

    fn list_subscriptions_for_task(&self, task_id: &str) -> Result<Vec<SubscriptionView>, String> {
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
