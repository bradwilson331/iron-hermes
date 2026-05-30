//! Gateway notifier polling loop (Phase 36.3.7.5 BUG-36.3.7.5-03).
//!
//! # Architecture
//!
//! Mirrors `dispatcher.rs::run_dispatch_loop` exactly:
//! - `NotifierContext` carries the store handle + injected send closure + config.
//! - `run_notifier_loop` is the long-lived tokio task.
//! - `run_notifier_tick` is the testable inner step.
//!
//! # Locked architectural fence
//!
//! This module MUST NOT depend on `ironhermes-gateway`. The `send_fn` field is a
//! trait-object closure injected by the gateway at spawn time — same shape as the
//! dispatcher's `spawn_fn`.
//!
//! # The 4-step tick (locked CONTEXT.md decisions)
//!
//! 1. Read `task_events` WHERE id > watermark AND kind IN (terminal set) ORDER BY id ASC.
//! 2. For each event:
//!    a. List subscriptions for `event.task_id`.
//!    b. Format the per-kind plain-text message.
//!    c. Call `send_fn(platform, chat_id, thread_id_opt, message)` for each sub.
//!       On Err: log warning, continue (no retry; locked decision).
//!    d. Remove ALL subscriptions for the task after the send attempt loop
//!       (auto-remove on terminal delivery attempt; locked decision).
//! 3. Advance the watermark to MAX(event.id) processed.
//!
//! # Out of scope (locked CONTEXT.md fences)
//!
//! - Persistent watermark: in-memory `AtomicI64` only; gateway downtime can lose events.
//! - Retry on send_fn failure: log + drop; per-platform retry is the adapter's job.
//! - Reclaimed events: NOT a terminal kind; ignored by the SQL filter.
//! - Subscription filtering by event kind: v1 is all-or-nothing per subscription.
//! - Platform-specific rich formatting: plain text only.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use anyhow::Result as AnyResult;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::events::KanbanEvent;
use crate::store::KanbanStore;
use crate::types::Task;

// ---------------------------------------------------------------------------
// BoxFuture alias + SendFn type
// ---------------------------------------------------------------------------

type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Trait-object closure injected by the gateway at spawn time.
///
/// Signature: `(platform, chat_id, thread_id_opt, message) -> Result<()>`. The notifier
/// does NOT depend on `ironhermes-gateway`; the gateway captures its `Arc<dyn PlatformAdapter>`
/// set and fans out on the `platform` arg.
///
/// Returns `anyhow::Result<()>` (not the kanban-local Result type) so the gateway can
/// surface platform-specific error variants without the kanban crate needing to know
/// about them.
pub type SendFn = Arc<
    dyn Fn(&str, &str, Option<&str>, &str) -> BoxFuture<'static, AnyResult<()>> + Send + Sync,
>;

// ---------------------------------------------------------------------------
// NotifierContext
// ---------------------------------------------------------------------------

/// Shared context passed to each notifier tick.
///
/// Mirrors `DispatcherContext` shape — the `send_fn` field is injected by the
/// gateway at spawn time (same pattern as `DispatcherContext::spawn_fn`).
pub struct NotifierContext {
    pub store: Arc<TokioMutex<KanbanStore>>,
    pub poll_interval_seconds: u64,
    pub last_event_id: Arc<AtomicI64>,
    pub send_fn: SendFn,
}

impl NotifierContext {
    /// Construct a context. Watermark initialized to `0`; `run_notifier_loop`
    /// calls `init_watermark` at startup to advance it to `MAX(id) FROM task_events`.
    pub fn new(
        store: Arc<TokioMutex<KanbanStore>>,
        poll_interval_seconds: u64,
        send_fn: SendFn,
    ) -> Self {
        Self {
            store,
            poll_interval_seconds: poll_interval_seconds.max(1),
            last_event_id: Arc::new(AtomicI64::new(0)),
            send_fn,
        }
    }
}

// ---------------------------------------------------------------------------
// NotifierTickReport
// ---------------------------------------------------------------------------

/// Per-tick outcome — useful for tests + future metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifierTickReport {
    pub delivered: usize,
    pub delivery_failures: usize,
    pub events_processed: usize,
    pub subscriptions_removed: usize,
}

// ---------------------------------------------------------------------------
// run_notifier_loop
// ---------------------------------------------------------------------------

/// Run the notifier tick loop until `cancel` is signalled.
///
/// Ticks on `ctx.poll_interval_seconds` (min 1s). Each tick error is logged
/// and the loop continues — mirrors the dispatcher loop's error-isolation
/// pattern.
pub async fn run_notifier_loop(ctx: Arc<NotifierContext>, cancel: CancellationToken) {
    if let Err(e) = init_watermark(&ctx).await {
        tracing::warn!(
            error = %e,
            "notifier: failed to init watermark; loop will not start"
        );
        return;
    }
    let interval_secs = ctx.poll_interval_seconds.max(1);
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    tracing::info!(
        interval_seconds = interval_secs,
        "kanban notifier loop started"
    );
    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                tracing::info!("kanban notifier loop cancelled");
                break;
            }
            _ = interval.tick() => {
                match run_notifier_tick(&ctx).await {
                    Ok(report) if report.events_processed > 0 => {
                        tracing::debug!(?report, "notifier tick");
                    }
                    Ok(_) => {} // quiet path — no terminal events this tick
                    Err(e) => {
                        tracing::warn!(error = %e, "notifier tick failed");
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// run_notifier_tick
// ---------------------------------------------------------------------------

/// Execute one notifier tick — the testable inner step.
///
/// Steps:
/// 1. Load terminal events past the watermark.
/// 2. For each event, list subscriptions, fetch task, format message,
///    call `send_fn` for each subscription (log + drop on Err per locked policy),
///    then remove ALL subscriptions for the task (auto-remove after attempt).
/// 3. Advance the watermark to the MAX `id` observed via `AtomicI64::fetch_max`
///    (monotonic — never goes backwards).
pub async fn run_notifier_tick(ctx: &NotifierContext) -> Result<NotifierTickReport> {
    let watermark = ctx.last_event_id.load(Ordering::SeqCst);
    let events = {
        let store = ctx.store.lock().await;
        store.list_terminal_events_after(watermark)?
    };
    if events.is_empty() {
        return Ok(NotifierTickReport::default());
    }
    let mut report = NotifierTickReport::default();
    let mut highest_id = watermark;
    for ev in &events {
        highest_id = highest_id.max(ev.id);
        report.events_processed += 1;

        // 1. list subscriptions
        let subs = {
            let store = ctx.store.lock().await;
            store.list_subscriptions_for_task(&ev.task_id)?
        };
        if subs.is_empty() {
            continue;
        }

        // 2. fetch task for the assignee + format the per-kind message
        let task = {
            let store = ctx.store.lock().await;
            store.get_task(&ev.task_id)?
        };
        let message = format_terminal_message(&task, ev);

        // 3. send to each subscription
        for sub in &subs {
            let tid = if sub.thread_id.is_empty() {
                None
            } else {
                Some(sub.thread_id.as_str())
            };
            match (ctx.send_fn)(sub.platform.as_str(), sub.chat_id.as_str(), tid, &message)
                .await
            {
                Ok(_) => report.delivered += 1,
                Err(e) => {
                    report.delivery_failures += 1;
                    tracing::warn!(
                        event = "notification_failed",
                        task_id = %ev.task_id,
                        platform = %sub.platform,
                        chat_id = %sub.chat_id,
                        error = %e,
                        "notifier send_fn returned error; logging + dropping per locked policy"
                    );
                }
            }
        }

        // 4. auto-remove subscriptions for this task
        let removed = {
            let mut store = ctx.store.lock().await;
            store.remove_all_subscriptions_for_task(&ev.task_id)?
        };
        report.subscriptions_removed += removed;
    }
    ctx.last_event_id.fetch_max(highest_id, Ordering::SeqCst);
    Ok(report)
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

async fn init_watermark(ctx: &NotifierContext) -> Result<()> {
    let max_id = {
        let store = ctx.store.lock().await;
        store.max_event_id()?
    };
    ctx.last_event_id.store(max_id, Ordering::SeqCst);
    tracing::info!(watermark = max_id, "notifier watermark initialized");
    Ok(())
}

fn format_terminal_message(task: &Task, ev: &KanbanEvent) -> String {
    // Parse the event's payload as JSON if present (stored as a JSON string).
    let payload: Option<serde_json::Value> = ev
        .payload
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    let summary_first_line = payload
        .as_ref()
        .and_then(|p| p.get("summary"))
        .and_then(|s| s.as_str())
        .map(|s| s.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();
    let reason = payload
        .as_ref()
        .and_then(|p| p.get("reason"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let pid = payload
        .as_ref()
        .and_then(|p| p.get("pid"))
        .and_then(|s| s.as_u64())
        .unwrap_or(0);
    match ev.kind.as_str() {
        "completed" => {
            if summary_first_line.is_empty() {
                format!("\u{2713} {} completed by {}", task.id, task.assignee)
            } else {
                format!(
                    "\u{2713} {} completed by {}\n{}",
                    task.id, task.assignee, summary_first_line
                )
            }
        }
        "blocked" => format!("\u{26a0} {} blocked: {}", task.id, reason),
        "gave_up" => format!("\u{26a0} {} gave up: {}", task.id, reason),
        "crashed" => format!("\u{2717} {} crashed (pid={})", task.id, pid),
        "timed_out" => format!("\u{23f1} {} timed out", task.id),
        other => format!("{} {}", task.id, other),
    }
}
