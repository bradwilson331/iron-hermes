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
//! # The multi-board tick (Phase 36.3.7.9 D-03 extension)
//!
//! `run_notifier_tick` resolves the board set from `subscribe_boards`, then for
//! each board:
//! 1. Opens a per-board store (or uses `ctx.store` for "default" back-compat).
//! 2. Initialises a per-board watermark on first visit (watermark == 0).
//! 3. Reads `task_events` past the per-board watermark.
//! 4. Delivers to subscriptions, advances the per-board watermark.
//! 5. On per-board open failure: logs warn + skips (INV-36.3.7-08-05 extension).
//!
//! # Out of scope (locked CONTEXT.md fences)
//!
//! - Persistent watermark: in-memory `AtomicI64` only; gateway downtime can lose events.
//! - Retry on send_fn failure: log + drop; per-platform retry is the adapter's job.
//! - Reclaimed events: NOT a terminal kind; ignored by the SQL filter.
//! - Subscription filtering by event kind: v1 is all-or-nothing per subscription.
//! - Platform-specific rich formatting: plain text only.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex as StdMutex;

use anyhow::Result as AnyResult;
use tokio::sync::Mutex as TokioMutex;
use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::events::KanbanEvent;
use crate::notifier_config::{SubscribeBoardsConfig, resolve_subscribe_boards};
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
///
/// # Phase 36.3.7.9 extension (D-03)
///
/// `subscribe_boards` configures which boards are swept each tick.
/// `board_watermarks` holds a per-board `AtomicI64` watermark so each
/// board's `task_events` stream is replayed independently (T-36.3.7.9-05-03).
///
/// `last_event_id` is kept for back-compat with Phase 36.3.7.5 tests that
/// access it directly. For the "default" board it stays in sync with
/// `board_watermarks["default"]`.
pub struct NotifierContext {
    pub store: Arc<TokioMutex<KanbanStore>>,
    pub poll_interval_seconds: u64,
    /// Phase 36.3.7.5 back-compat watermark for the "default" board.
    /// Kept in sync with `board_watermarks["default"]` on every tick.
    pub last_event_id: Arc<AtomicI64>,
    pub send_fn: SendFn,
    /// Per-board watermark map (D-03). Keyed by board slug.
    /// The "default" entry shares the same `Arc<AtomicI64>` as `last_event_id`
    /// so back-compat reads via `ctx.last_event_id` see the same value.
    pub board_watermarks: Arc<StdMutex<HashMap<String, Arc<AtomicI64>>>>,
    /// Which boards to sweep each tick (D-03).
    pub subscribe_boards: SubscribeBoardsConfig,
}

impl NotifierContext {
    /// Construct a single-board default context (Phase 36.3.7.5 back-compat).
    ///
    /// Creates a context scoped to `Explicit(["default"])`. The `store` field
    /// is the pre-opened default board store. Existing callers need no change.
    pub fn new(
        store: Arc<TokioMutex<KanbanStore>>,
        poll_interval_seconds: u64,
        send_fn: SendFn,
    ) -> Self {
        let last_event_id = Arc::new(AtomicI64::new(0));
        let board_watermarks = Arc::new(StdMutex::new(HashMap::new()));
        // The "default" entry shares the same Arc so ctx.last_event_id stays
        // in sync with board_watermarks["default"] automatically.
        board_watermarks
            .lock()
            .unwrap()
            .insert("default".to_string(), last_event_id.clone());
        Self {
            store,
            poll_interval_seconds: poll_interval_seconds.max(1),
            last_event_id,
            send_fn,
            board_watermarks,
            subscribe_boards: SubscribeBoardsConfig::Explicit(vec!["default".to_string()]),
        }
    }

    /// Construct a multi-board context with a custom `subscribe_boards` config.
    ///
    /// The `store` field is used for the "default" board (back-compat). Named
    /// boards are opened via `KanbanStore::open_for_board` per tick.
    pub fn new_multi_board(
        store: Arc<TokioMutex<KanbanStore>>,
        poll_interval_seconds: u64,
        send_fn: SendFn,
        subscribe_boards: SubscribeBoardsConfig,
    ) -> Self {
        let last_event_id = Arc::new(AtomicI64::new(0));
        let board_watermarks = Arc::new(StdMutex::new(HashMap::new()));
        board_watermarks
            .lock()
            .unwrap()
            .insert("default".to_string(), last_event_id.clone());
        Self {
            store,
            poll_interval_seconds: poll_interval_seconds.max(1),
            last_event_id,
            send_fn,
            board_watermarks,
            subscribe_boards,
        }
    }
}

// ---------------------------------------------------------------------------
// NotifierTickReport
// ---------------------------------------------------------------------------

/// Per-tick outcome — useful for tests + future metrics.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotifierTickReport {
    /// Total events delivered across all boards this tick (summed).
    pub delivered: usize,
    /// Total send_fn failures across all boards this tick (summed).
    pub delivery_failures: usize,
    /// Total terminal events observed across all boards this tick (summed).
    pub events_processed: usize,
    /// Total subscriptions removed across all boards this tick (summed).
    pub subscriptions_removed: usize,
    /// Per-board breakdown (D-03). One entry per successfully-swept board.
    pub per_board: Vec<BoardTickStats>,
}

/// Per-board stats within a single tick.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BoardTickStats {
    pub slug: String,
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
/// Phase 36.3.7.9 extension: resolves the board set from `ctx.subscribe_boards`,
/// then sweeps each board independently with its own watermark.
///
/// Per-board open failures log a warning and skip the board
/// (INV-36.3.7-08-05 extension, T-36.3.7.9-05-02) — other boards continue.
pub async fn run_notifier_tick(ctx: &NotifierContext) -> Result<NotifierTickReport> {
    let slugs = resolve_subscribe_boards(&ctx.subscribe_boards)
        .map_err(|e| crate::error::KanbanError::Other(anyhow::anyhow!("{}", e)))?;
    let mut total = NotifierTickReport::default();

    for slug in &slugs {
        // Get-or-create per-board watermark.
        let wm: Arc<AtomicI64> = {
            let mut map = ctx.board_watermarks.lock().unwrap();
            map.entry(slug.clone())
                .or_insert_with(|| Arc::new(AtomicI64::new(0)))
                .clone()
        };

        // Open the board store. For "default", use ctx.store directly (back-compat
        // with Phase 36.3.7.5 tests that pre-seed a store at an arbitrary path).
        // For named boards, open via open_for_board (D-01 path resolution).
        let board_stats = if slug == "default" {
            sweep_board_with_store_ref(ctx, slug, &ctx.store, wm).await
        } else {
            match KanbanStore::open_for_board(slug) {
                Ok(store) => {
                    let store_arc = Arc::new(TokioMutex::new(store));
                    sweep_board_with_store_ref(ctx, slug, &store_arc, wm).await
                }
                Err(e) => {
                    // INV-36.3.7-08-05 extension: log + skip corrupt/missing boards.
                    tracing::warn!(
                        slug = %slug,
                        error = %e,
                        "[kanban] skipping board '{slug}': {e}"
                    );
                    continue;
                }
            }
        };

        // Accumulate per-board stats into totals (back-compat fields summed).
        total.delivered += board_stats.delivered;
        total.delivery_failures += board_stats.delivery_failures;
        total.events_processed += board_stats.events_processed;
        total.subscriptions_removed += board_stats.subscriptions_removed;
        total.per_board.push(board_stats);
    }

    Ok(total)
}

/// Sweep one board's `task_events` and deliver notifications.
///
/// Returns per-board stats that the caller accumulates into the tick report.
async fn sweep_board_with_store_ref(
    ctx: &NotifierContext,
    slug: &str,
    store: &Arc<TokioMutex<KanbanStore>>,
    wm: Arc<AtomicI64>,
) -> BoardTickStats {
    let mut stats = BoardTickStats {
        slug: slug.to_string(),
        ..Default::default()
    };

    let watermark = wm.load(Ordering::SeqCst);

    // Load terminal events past the watermark.
    let events = {
        let s = store.lock().await;
        match s.list_terminal_events_after(watermark) {
            Ok(evs) => evs,
            Err(e) => {
                tracing::warn!(
                    slug = %slug,
                    error = %e,
                    "notifier: failed to read events for board"
                );
                return stats;
            }
        }
    };

    if events.is_empty() {
        return stats;
    }

    let mut highest_id = watermark;

    for ev in &events {
        highest_id = highest_id.max(ev.id);
        stats.events_processed += 1;

        // List subscriptions for this event's task.
        let subs = {
            let s = store.lock().await;
            match s.list_subscriptions_for_task(&ev.task_id) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        slug = %slug,
                        task_id = %ev.task_id,
                        error = %e,
                        "notifier: failed to list subscriptions"
                    );
                    continue;
                }
            }
        };
        if subs.is_empty() {
            continue;
        }

        // Fetch task for message formatting.
        let task = {
            let s = store.lock().await;
            match s.get_task(&ev.task_id) {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(
                        slug = %slug,
                        task_id = %ev.task_id,
                        error = %e,
                        "notifier: failed to fetch task"
                    );
                    continue;
                }
            }
        };
        let message = format_terminal_message(&task, ev);

        // Send to each subscription (log + drop on failure per locked policy).
        for sub in &subs {
            let tid = if sub.thread_id.is_empty() {
                None
            } else {
                Some(sub.thread_id.as_str())
            };
            match (ctx.send_fn)(sub.platform.as_str(), sub.chat_id.as_str(), tid, &message).await
            {
                Ok(_) => stats.delivered += 1,
                Err(e) => {
                    stats.delivery_failures += 1;
                    tracing::warn!(
                        event = "notification_failed",
                        slug = %slug,
                        task_id = %ev.task_id,
                        platform = %sub.platform,
                        chat_id = %sub.chat_id,
                        error = %e,
                        "notifier send_fn returned error; logging + dropping per locked policy"
                    );
                }
            }
        }

        // Auto-remove subscriptions after attempt (locked decision).
        {
            let mut s = store.lock().await;
            match s.remove_all_subscriptions_for_task(&ev.task_id) {
                Ok(removed) => stats.subscriptions_removed += removed,
                Err(e) => {
                    tracing::warn!(
                        slug = %slug,
                        task_id = %ev.task_id,
                        error = %e,
                        "notifier: failed to remove subscriptions"
                    );
                }
            }
        }
    }

    // Advance per-board watermark monotonically (never goes backwards).
    wm.fetch_max(highest_id, Ordering::SeqCst);
    // Sync last_event_id for back-compat (default board only).
    if slug == "default" {
        ctx.last_event_id.fetch_max(highest_id, Ordering::SeqCst);
    }

    stats
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

async fn init_watermark(ctx: &NotifierContext) -> Result<()> {
    let max_id = {
        let store = ctx.store.lock().await;
        store.max_event_id()?
    };
    // Update last_event_id (back-compat) and board_watermarks["default"].
    ctx.last_event_id.store(max_id, Ordering::SeqCst);
    {
        let mut map = ctx.board_watermarks.lock().unwrap();
        // The "default" entry is the same Arc as last_event_id (set in new()), so
        // storing via last_event_id above already updated it. Insert if absent.
        map.entry("default".to_string())
            .or_insert_with(|| Arc::new(AtomicI64::new(max_id)));
    }
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
