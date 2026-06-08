//! Phase 36.3.7.5 Plan 02 (BUG-36.3.7.5-03 + part of BUG-36.3.7.5-07).
//! Receiver-end tests for the notifier polling loop.
//!
//! Tests drive `run_notifier_tick` directly with a mock `send_fn` that records
//! every call. No real adapter, no real network — the closure-injection pattern
//! makes the polling logic fully testable without touching gateway code.
//!
//! Test map:
//! - `polling_loop_delivers_once_and_removes_subscription` — happy path:
//!   Completed event + 1 sub → mock called once → sub removed.
//! - `reclaimed_event_is_ignored` — Reclaimed is NOT in the terminal SQL filter.
//! - `watermark_advances_past_processed_event` — monotonic watermark across
//!   3 ticks; the second event has no subs, the third does.
//! - `send_fn_failure_still_removes_subscription` — locked policy: log + drop
//!   the subscription even when send fails.
//! - `no_subscriptions_means_no_send` — early skip when `subs.is_empty()`.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use ironhermes_kanban::events::KanbanEventKind;
use ironhermes_kanban::store::CreateTaskOptions;
use ironhermes_kanban::{KanbanStore, NotifierContext, SendFn, run_notifier_tick};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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

fn make_failing_send_fn() -> (SendFn, Arc<StdMutex<usize>>) {
    let counter = Arc::new(StdMutex::new(0usize));
    let counter_clone = counter.clone();
    let f: SendFn = Arc::new(move |_p, _c, _t, _m| {
        let counter = counter_clone.clone();
        Box::pin(async move {
            *counter.lock().unwrap() += 1;
            Err(anyhow::anyhow!("transport down — receiver test"))
        })
    });
    (f, counter)
}

fn open_store() -> (TempDir, Arc<TokioMutex<KanbanStore>>) {
    let dir = TempDir::new().expect("tempdir");
    let store = KanbanStore::new(dir.path().join("kanban.db")).expect("open");
    (dir, Arc::new(TokioMutex::new(store)))
}

/// Create a task and append a Completed event with the given summary in the
/// payload. Returns the new task id.
async fn seed_task_with_completed_event(
    store: &Arc<TokioMutex<KanbanStore>>,
    title: &str,
    summary: &str,
) -> String {
    let task_id = {
        let mut s = store.lock().await;
        s.create_task(title, "alice", CreateTaskOptions::default())
            .expect("create_task")
            .id
    };
    {
        let mut s = store.lock().await;
        let payload = serde_json::json!({"summary": summary});
        s.append_event(&task_id, None, KanbanEventKind::Completed, Some(&payload))
            .expect("append_event");
    }
    task_id
}

/// Append a Reclaimed event for an existing task (used by the
/// `reclaimed_event_is_ignored` test).
async fn append_reclaimed_event(store: &Arc<TokioMutex<KanbanStore>>, task_id: &str) {
    let mut s = store.lock().await;
    let payload = serde_json::json!({"reason": "ttl expired"});
    s.append_event(task_id, None, KanbanEventKind::Reclaimed, Some(&payload))
        .expect("append_event reclaimed");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// BUG-36.3.7.5-03 happy path: a Completed event + 1 subscription → mock
/// recorded exactly 1 call with the expected message + subscription removed
/// after delivery.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn polling_loop_delivers_once_and_removes_subscription() {
    let (_dir, store) = open_store();
    let task_id = seed_task_with_completed_event(&store, "ship it", "wrote 42 lines").await;
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_id, "telegram", "42", None, "auto")
            .expect("append_subscription");
    }

    let (send_fn, log) = make_recording_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "exactly one send_fn call expected");
    let (platform, chat_id, thread_id, message) = &calls[0];
    assert_eq!(platform, "telegram");
    assert_eq!(chat_id, "42");
    assert!(
        thread_id.is_none(),
        "None thread_id passed when sub.thread_id is empty"
    );
    assert!(
        message.starts_with("\u{2713}"),
        "BUG-36.3.7.5-03: completed message must start with check-icon, got {message:?}"
    );
    assert!(
        message.contains(&task_id),
        "BUG-36.3.7.5-03: completed message must include task_id, got {message:?}"
    );
    assert!(
        message.contains("alice"),
        "BUG-36.3.7.5-03: completed message must include assignee, got {message:?}"
    );
    assert!(
        message.contains("wrote 42 lines"),
        "BUG-36.3.7.5-03: completed message must include summary first line, got {message:?}"
    );

    let subs = {
        let s = store.lock().await;
        s.list_subscriptions_for_task(&task_id)
            .expect("list_subscriptions_for_task")
    };
    assert!(
        subs.is_empty(),
        "BUG-36.3.7.5-03: subscription must be removed after terminal delivery"
    );

    assert_eq!(report.delivered, 1);
    assert_eq!(report.delivery_failures, 0);
    assert_eq!(report.events_processed, 1);
    assert_eq!(report.subscriptions_removed, 1);
}

/// BUG-36.3.7.5-03 negative: Reclaimed is NOT in the terminal kind set; the
/// notifier must skip the event entirely (SQL filter excludes it) so the
/// subscription stays in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn reclaimed_event_is_ignored() {
    let (_dir, store) = open_store();
    // create_task only — no Completed event; we attach Reclaimed instead.
    let task_id = {
        let mut s = store.lock().await;
        s.create_task("orphan", "alice", CreateTaskOptions::default())
            .expect("create_task")
            .id
    };
    append_reclaimed_event(&store, &task_id).await;
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_id, "telegram", "42", None, "auto")
            .expect("append_subscription");
    }

    let (send_fn, log) = make_recording_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert!(
        log.lock().unwrap().is_empty(),
        "BUG-36.3.7.5-03: Reclaimed must NOT trigger send_fn (not a terminal kind)"
    );
    let subs = {
        let s = store.lock().await;
        s.list_subscriptions_for_task(&task_id)
            .expect("list_subscriptions_for_task")
    };
    assert_eq!(
        subs.len(),
        1,
        "BUG-36.3.7.5-03: subscription must remain when no terminal event fired"
    );
    assert_eq!(report.events_processed, 0);
    assert_eq!(report.delivered, 0);
    assert_eq!(report.subscriptions_removed, 0);
}

/// BUG-36.3.7.5-03 watermark: three ticks across three Completed events.
/// Tick 1 — event id 1 has a sub: delivered + removed, watermark advances.
/// Tick 2 — event id 2 has NO sub: skipped but watermark advances.
/// Tick 3 — event id 3 has a sub: delivered.
/// Re-running tick should be a no-op (idempotent past the watermark).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn watermark_advances_past_processed_event() {
    let (_dir, store) = open_store();

    // -- seed task A + Completed event + subscription --
    let task_a = seed_task_with_completed_event(&store, "A", "a-summary").await;
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_a, "telegram", "42", None, "auto")
            .expect("sub A");
    }

    let (send_fn, log) = make_recording_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);

    // Tick 1: deliver A.
    let r1 = run_notifier_tick(&ctx).await.expect("tick 1");
    assert_eq!(log.lock().unwrap().len(), 1, "tick 1: one send");
    assert_eq!(r1.delivered, 1);
    assert_eq!(r1.events_processed, 1);
    let watermark_after_1 = ctx.last_event_id.load(std::sync::atomic::Ordering::SeqCst);
    assert!(watermark_after_1 >= 1);

    // -- seed task B + Completed event, no subscription --
    let _task_b = seed_task_with_completed_event(&store, "B", "b-summary").await;

    // Tick 2: B's event is processed but no subs → no send; watermark advances.
    let r2 = run_notifier_tick(&ctx).await.expect("tick 2");
    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "tick 2: still one send total (B has no sub)"
    );
    assert_eq!(r2.events_processed, 1);
    assert_eq!(r2.delivered, 0);
    let watermark_after_2 = ctx.last_event_id.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        watermark_after_2 > watermark_after_1,
        "BUG-36.3.7.5-03: watermark must advance past unsubscribed events too"
    );

    // -- seed task C + Completed event + subscription --
    let task_c = seed_task_with_completed_event(&store, "C", "c-summary").await;
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_c, "discord", "99", Some("thread-7"), "explicit")
            .expect("sub C");
    }

    // Tick 3: deliver C only (A's event id < watermark, B has no sub).
    let r3 = run_notifier_tick(&ctx).await.expect("tick 3");
    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.len(), 2, "tick 3: now two sends total");
    let (platform, chat_id, thread_id, _msg) = &calls[1];
    assert_eq!(platform, "discord");
    assert_eq!(chat_id, "99");
    assert_eq!(thread_id.as_deref(), Some("thread-7"));
    assert_eq!(r3.delivered, 1);

    let watermark_after_3 = ctx.last_event_id.load(std::sync::atomic::Ordering::SeqCst);
    assert!(watermark_after_3 > watermark_after_2);

    // Tick 4: nothing new — no-op.
    let r4 = run_notifier_tick(&ctx).await.expect("tick 4");
    assert_eq!(
        log.lock().unwrap().len(),
        2,
        "tick 4: no new sends past watermark"
    );
    assert_eq!(r4.events_processed, 0);
}

/// BUG-36.3.7.5-03 locked policy: when `send_fn` returns Err, the notifier
/// logs the failure AND still removes the subscription (no retry).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn send_fn_failure_still_removes_subscription() {
    let (_dir, store) = open_store();
    let task_id = seed_task_with_completed_event(&store, "doomed", "won't deliver").await;
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_id, "telegram", "42", None, "auto")
            .expect("append_subscription");
    }

    let (send_fn, counter) = make_failing_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);
    let watermark_before = ctx.last_event_id.load(std::sync::atomic::Ordering::SeqCst);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert_eq!(
        *counter.lock().unwrap(),
        1,
        "send_fn was attempted exactly once"
    );
    assert_eq!(report.delivered, 0);
    assert_eq!(report.delivery_failures, 1);

    let subs = {
        let s = store.lock().await;
        s.list_subscriptions_for_task(&task_id)
            .expect("list_subscriptions_for_task")
    };
    assert!(
        subs.is_empty(),
        "BUG-36.3.7.5-03: locked log+drop policy — subscription removed even after send failure"
    );
    let watermark_after = ctx.last_event_id.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        watermark_after > watermark_before,
        "BUG-36.3.7.5-03: watermark must advance so next tick does NOT replay"
    );
}

/// BUG-36.3.7.5-03 early-skip: terminal event present but no subscriptions
/// for the task → mock not called, but events_processed counts the event
/// (it WAS observed by the SQL filter; the inner `if subs.is_empty() { continue; }`
/// guard fires after).
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn no_subscriptions_means_no_send() {
    let (_dir, store) = open_store();
    let _task_id = seed_task_with_completed_event(&store, "solo", "no audience").await;

    let (send_fn, log) = make_recording_send_fn();
    let ctx = NotifierContext::new(store.clone(), 1, send_fn);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert!(
        log.lock().unwrap().is_empty(),
        "BUG-36.3.7.5-03: no subs means no send_fn call"
    );
    assert_eq!(
        report.events_processed, 1,
        "BUG-36.3.7.5-03: event WAS observed even when no subs"
    );
    assert_eq!(report.delivered, 0);
    assert_eq!(report.subscriptions_removed, 0);
}
