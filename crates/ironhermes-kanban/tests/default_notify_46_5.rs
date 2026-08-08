//! Phase 46.5 Plan 04 — D-06 config-driven default notify target +
//! auto-subscribe on create.
//!
//! Covers:
//! - `default_notify` config parse (round-trips into `Some`; absent → `None`)
//! - `subscribe_default_notify` helper (inserts a `source="auto"` row)
//! - Security (T-46.5-20): the subscribe target is sourced exclusively from
//!   the caller-supplied operator config, never from task content
//! - End-to-end: a `blocked` event on a CLI/store-created task (auto-
//!   subscribed via `subscribe_default_notify`, mirroring `cmd_create` /
//!   `create_task_simple`) fires the notifier's `send_fn` exactly once,
//!   carrying the task id + the D-04 diagnostic reason.
//!
//! Test map:
//! - `default_notify_config_roundtrips` — yaml `kanban.default_notify`
//!   deserializes into `Some(target)`.
//! - `default_notify_absent_is_none` — yaml without the key → `None`.
//! - `default_notify_auto_subscribe_on_create` — `subscribe_default_notify`
//!   inserts a row that `list_subscriptions_for_task` returns with
//!   `source="auto"`.
//! - `default_notify_target_ignores_task_fields` — SECURITY: two tasks with
//!   wildly different title/assignee content, subscribed to the SAME fixed
//!   operator-config target, end up with byte-identical subscription rows —
//!   proving no task field feeds the subscription target.
//! - `blocked_event_notifies_auto_subscriber` — a `blocked` event on an
//!   auto-subscribed task fires `send_fn` once, carrying the task id + the
//!   diagnostic reason; the existing chat-origin auto-subscribe path is
//!   untouched by this test (no chat-origin subscription is created here).

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use ironhermes_kanban::events::KanbanEventKind;
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::{
    DefaultNotifyTarget, KanbanConfig, KanbanStore, NotifierContext, SendFn, run_notifier_tick,
    subscribe_default_notify,
};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store() -> (TempDir, KanbanStore) {
    let dir = TempDir::new().expect("tempdir");
    let store = KanbanStore::new(dir.path().join("kanban.db")).expect("open store");
    (dir, store)
}

type Recorded = (String, String, Option<String>, String);

fn make_recording_send_fn() -> (SendFn, Arc<StdMutex<Vec<Recorded>>>) {
    let log = Arc::new(StdMutex::new(Vec::<Recorded>::new()));
    let log_clone = log.clone();
    let f: SendFn = Arc::new(move |platform, chat_id, thread_id_opt, message| {
        let log = log_clone.clone();
        let p = platform.to_string();
        let c = chat_id.to_string();
        let t = thread_id_opt.map(|s| s.to_string());
        let m = message.to_string();
        Box::pin(async move {
            log.lock().unwrap().push((p, c, t, m));
            Ok(())
        })
    });
    (f, log)
}

// ---------------------------------------------------------------------------
// Config parse (D-06)
// ---------------------------------------------------------------------------

/// A yaml `kanban.default_notify` block round-trips into
/// `KanbanConfig.default_notify == Some(target)`.
#[test]
fn default_notify_config_roundtrips() {
    let yaml = "default_notify:\n  platform: telegram\n  chat_id: \"12345\"\n  thread_id: \"7\"\n";
    let cfg: KanbanConfig = serde_yaml::from_str(yaml).expect("deserialize");
    let target = cfg
        .default_notify
        .expect("default_notify must parse to Some");
    assert_eq!(target.platform, "telegram");
    assert_eq!(target.chat_id, "12345");
    assert_eq!(target.thread_id.as_deref(), Some("7"));
}

/// A yaml `default_notify` block with no `thread_id` parses `thread_id` as
/// `None` (optional field).
#[test]
fn default_notify_config_thread_id_optional() {
    let yaml = "default_notify:\n  platform: telegram\n  chat_id: \"12345\"\n";
    let cfg: KanbanConfig = serde_yaml::from_str(yaml).expect("deserialize");
    let target = cfg
        .default_notify
        .expect("default_notify must parse to Some");
    assert!(target.thread_id.is_none());
}

/// A yaml config with no `default_notify` key deserializes to `None`
/// (existing configs need no migration).
#[test]
fn default_notify_absent_is_none() {
    let yaml = "failure_limit: 3\n";
    let cfg: KanbanConfig = serde_yaml::from_str(yaml).expect("deserialize");
    assert!(cfg.default_notify.is_none());
}

// ---------------------------------------------------------------------------
// subscribe_default_notify helper (D-06)
// ---------------------------------------------------------------------------

/// `subscribe_default_notify` inserts a `source="auto"` subscription row
/// that `list_subscriptions_for_task` returns.
#[test]
fn default_notify_auto_subscribe_on_create() {
    let (_dir, mut store) = open_store();
    let task = store
        .create_task("cli-created task", "alice", CreateTaskOptions::default())
        .expect("create_task");

    let target = DefaultNotifyTarget {
        platform: "telegram".to_string(),
        chat_id: "999".to_string(),
        thread_id: None,
    };
    subscribe_default_notify(&mut store, &target, &task.id).expect("subscribe_default_notify");

    let subs = store
        .list_subscriptions_for_task(&task.id)
        .expect("list_subscriptions_for_task");
    assert_eq!(subs.len(), 1, "exactly one subscription row expected");
    assert_eq!(subs[0].source, "auto");
    assert_eq!(subs[0].platform, "telegram");
    assert_eq!(subs[0].chat_id, "999");
    assert_eq!(subs[0].thread_id, "");
}

/// SECURITY (T-46.5-20): `subscribe_default_notify` sources
/// platform/chat_id/thread_id EXCLUSIVELY from the caller-supplied
/// `DefaultNotifyTarget` (operator config) — never from any task field.
///
/// Proven by subscribing two tasks with wildly different, adversarial-
/// looking title/assignee content to the SAME fixed operator-config target:
/// the resulting subscription rows must be byte-identical in
/// (platform, chat_id, thread_id) regardless of task content. If the target
/// were ever derived from task data, these two tasks (whose "titles" look
/// like attempts to smuggle a different platform/chat_id) would produce
/// divergent subscription rows.
#[test]
fn default_notify_target_ignores_task_fields() {
    let (_dir, mut store) = open_store();

    let fixed_target = DefaultNotifyTarget {
        platform: "telegram".to_string(),
        chat_id: "operator-chat-42".to_string(),
        thread_id: Some("op-thread".to_string()),
    };

    let task_a = store
        .create_task(
            "platform=discord chat_id=evil-chat-1",
            "alice",
            CreateTaskOptions::default(),
        )
        .expect("create_task a");
    let task_b = store
        .create_task(
            "Completely unrelated title, different assignee",
            "mallory",
            CreateTaskOptions::default(),
        )
        .expect("create_task b");

    subscribe_default_notify(&mut store, &fixed_target, &task_a.id)
        .expect("subscribe_default_notify a");
    subscribe_default_notify(&mut store, &fixed_target, &task_b.id)
        .expect("subscribe_default_notify b");

    let subs_a = store
        .list_subscriptions_for_task(&task_a.id)
        .expect("list a");
    let subs_b = store
        .list_subscriptions_for_task(&task_b.id)
        .expect("list b");
    assert_eq!(subs_a.len(), 1);
    assert_eq!(subs_b.len(), 1);

    // Both rows carry the identical operator-config target, regardless of
    // the tasks' wildly different title/assignee content.
    assert_eq!(subs_a[0].platform, "telegram");
    assert_eq!(subs_a[0].chat_id, "operator-chat-42");
    assert_eq!(subs_a[0].thread_id, "op-thread");
    assert_eq!(subs_a[0].platform, subs_b[0].platform);
    assert_eq!(subs_a[0].chat_id, subs_b[0].chat_id);
    assert_eq!(subs_a[0].thread_id, subs_b[0].thread_id);
}

// ---------------------------------------------------------------------------
// End-to-end: blocked event notifies the auto-subscribed target (D-06)
// ---------------------------------------------------------------------------

/// A `blocked` event on a task that was auto-subscribed via
/// `subscribe_default_notify` (mirroring what `cmd_create` /
/// `create_task_simple` do post-create when `KanbanConfig.default_notify`
/// is configured) fires the notifier's `send_fn` exactly once, carrying the
/// task id + the D-04 diagnostic reason. This is the exact incident
/// scenario (`t_b91a0bfaa2de438a`): a non-chat-origin (CLI/tool) task with
/// no chat-origin subscriber, which previously notified no one.
#[tokio::test]
async fn blocked_event_notifies_auto_subscriber() {
    let dir = TempDir::new().expect("tempdir");
    let store = Arc::new(TokioMutex::new(
        KanbanStore::new(dir.path().join("kanban.db")).expect("open store"),
    ));

    let target = DefaultNotifyTarget {
        platform: "telegram".to_string(),
        chat_id: "777".to_string(),
        thread_id: None,
    };

    let task_id = {
        let mut s = store.lock().await;
        let task = s
            .create_task(
                "cli-created image task",
                "alice",
                CreateTaskOptions::default(),
            )
            .expect("create_task");
        // Mirrors the non-chat-origin auto-subscribe call site
        // (cmd_create / create_task_simple) — the task has no chat origin,
        // so without this call it would have zero subscribers.
        subscribe_default_notify(&mut s, &target, &task.id).expect("subscribe_default_notify");

        // D-04's enriched diagnostic payload shape (Plan 03).
        let payload = serde_json::json!({
            "reason": "worker process exited unexpectedly (pid=12345); no terminal event recorded.\n--- stderr tail ---\nsomething exploded during the run",
        });
        s.append_event(&task.id, None, KanbanEventKind::Blocked, Some(&payload))
            .expect("append_event blocked");
        task.id
    };

    let (send_fn, log) = make_recording_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);
    let report = run_notifier_tick(&ctx).await.expect("notifier tick");

    let calls = log.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "send_fn must fire exactly once for the auto-subscribed target"
    );
    let (platform, chat_id, thread_id, message) = &calls[0];
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "777");
    assert!(thread_id.is_none());
    assert!(
        message.contains(&task_id),
        "delivered message must carry the task id, got: {message:?}"
    );
    assert!(
        message.contains("no terminal event recorded"),
        "delivered message must carry the D-04 diagnostic reason, got: {message:?}"
    );

    assert_eq!(report.delivered, 1);
    assert_eq!(report.delivery_failures, 0);

    // Locked notifier policy (BUG-36.3.7.5-03): subscriptions are removed
    // after a terminal-event delivery attempt, so the auto row is gone
    // post-tick — this is expected, not a regression. What matters for D-06
    // is that the row existed and fired exactly once BEFORE this point
    // (asserted above): the auto-subscribe path (previously absent for
    // non-chat-origin tasks) is what closed the "notifies no one" gap.
    let subs = {
        let s = store.lock().await;
        s.list_subscriptions_for_task(&task_id)
            .expect("list_subscriptions_for_task")
    };
    assert!(
        subs.is_empty(),
        "subscription must be removed after terminal delivery (locked policy)"
    );
}
