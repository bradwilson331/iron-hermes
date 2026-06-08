//! Multi-board notifier integration tests (Phase 36.3.7.9 Plan 05, D-03).
//!
//! Tests drive `run_notifier_tick` with `new_multi_board` contexts and verify:
//! - Board sweep scope (All vs Explicit)
//! - Per-board watermark independence (T-36.3.7.9-05-03)
//! - Corrupt-board skip + continue (T-36.3.7.9-05-02, INV-36.3.7-08-05 extension)
//! - Inert Explicit codepath (D-03 proof — filters correctly even before activation)
//!
//! Test map:
//! - `tick_sweeps_default_only_when_no_named_boards`
//! - `tick_sweeps_default_plus_named_under_subscribe_all`
//! - `tick_skips_corrupt_board_logs_warn_continues_sweeping`
//! - `per_board_watermark_advances_independently`
//! - `subscribe_explicit_filters_out_unlisted_boards`
//! - `subscribe_all_with_zero_boards_returns_default_only_tick`

use std::sync::Arc;
use std::sync::Mutex as StdMutex;

use ironhermes_kanban::events::KanbanEventKind;
use ironhermes_kanban::notifier_config::SubscribeBoardsConfig;
use ironhermes_kanban::store::{CreateTaskOptions, KanbanStore};
use ironhermes_kanban::{NotifierContext, SendFn, run_notifier_tick};
use tempfile::TempDir;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Serialise env-mutating tests
// ---------------------------------------------------------------------------

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---------------------------------------------------------------------------
// RAII env guard
// ---------------------------------------------------------------------------

struct ScopedEnv {
    key: String,
    prev: Option<String>,
}

impl ScopedEnv {
    fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: guarded by ENV_LOCK in all callers.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.to_string(),
            prev,
        }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => unsafe { std::env::set_var(&self.key, v) },
            None => unsafe { std::env::remove_var(&self.key) },
        }
    }
}

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

/// Open the default-board store at a tempdir-specific path (not IRONHERMES_HOME-resolved).
/// Returns the TempDir (caller must hold it) and the store Arc.
fn open_store_at(dir: &TempDir, name: &str) -> Arc<TokioMutex<KanbanStore>> {
    let store = KanbanStore::new(dir.path().join(name)).expect("open store");
    Arc::new(TokioMutex::new(store))
}

/// Seed a task + Completed event + subscription in a store. Returns task_id.
async fn seed_completed_with_sub(
    store: &Arc<TokioMutex<KanbanStore>>,
    title: &str,
    platform: &str,
    chat_id: &str,
) -> String {
    let task_id = {
        let mut s = store.lock().await;
        s.create_task(title, "bot", CreateTaskOptions::default())
            .expect("create_task")
            .id
    };
    {
        let mut s = store.lock().await;
        let payload = serde_json::json!({"summary": format!("{title} done")});
        s.append_event(&task_id, None, KanbanEventKind::Completed, Some(&payload))
            .expect("append_event");
    }
    {
        let mut s = store.lock().await;
        s.append_subscription(&task_id, platform, chat_id, None, "auto")
            .expect("append_subscription");
    }
    task_id
}

// ---------------------------------------------------------------------------
// Test 1: tick_sweeps_default_only_when_no_named_boards
// ---------------------------------------------------------------------------

/// When subscribe_boards = All and no named boards exist, the tick sweeps only "default".
/// Events on the default board are delivered; tick.per_board has exactly one entry.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn tick_sweeps_default_only_when_no_named_boards() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();
    // Set IRONHERMES_HOME to a dir with no boards/ subdirectory.
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    let default_store = open_store_at(&dir, "kanban.db");
    let _task = seed_completed_with_sub(&default_store, "task-A", "telegram", "100").await;

    let (send_fn, log) = make_recording_send_fn();
    let ctx =
        NotifierContext::new_multi_board(default_store, 1, send_fn, SubscribeBoardsConfig::All);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "exactly one send for default board event"
    );
    assert_eq!(report.delivered, 1);
    assert_eq!(report.events_processed, 1);
    assert_eq!(report.per_board.len(), 1, "only one board swept");
    assert_eq!(report.per_board[0].slug, "default");
}

// ---------------------------------------------------------------------------
// Test 2: tick_sweeps_default_plus_named_under_subscribe_all
// ---------------------------------------------------------------------------

/// When subscribe_boards = All and a named board "alpha" exists, the tick sweeps both.
/// Events on both boards are delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn tick_sweeps_default_plus_named_under_subscribe_all() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();

    // Create the boards/alpha/ directory so list_boards() discovers it.
    let alpha_board_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_board_dir).unwrap();
    let alpha_db_path = alpha_board_dir.join("kanban.db");

    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Open and seed the default board (passed via ctx.store).
    let default_store = open_store_at(&dir, "kanban.db");
    let _default_task =
        seed_completed_with_sub(&default_store, "default-task", "telegram", "10").await;

    // Open and seed the alpha board (will be opened via open_for_board("alpha")).
    {
        let alpha_store = KanbanStore::new(&alpha_db_path).expect("open alpha store");
        let alpha_arc = Arc::new(TokioMutex::new(alpha_store));
        let _alpha_task = seed_completed_with_sub(&alpha_arc, "alpha-task", "telegram", "20").await;
        // Drop to release the connection before open_for_board opens it.
        drop(alpha_arc);
    }

    let (send_fn, log) = make_recording_send_fn();
    let ctx =
        NotifierContext::new_multi_board(default_store, 1, send_fn, SubscribeBoardsConfig::All);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert_eq!(
        log.lock().unwrap().len(),
        2,
        "two sends: one for default, one for alpha"
    );
    assert_eq!(report.delivered, 2);
    assert_eq!(report.events_processed, 2);
    assert_eq!(report.per_board.len(), 2);
    let slugs: Vec<&str> = report.per_board.iter().map(|s| s.slug.as_str()).collect();
    assert!(slugs.contains(&"default"), "per_board must include default");
    assert!(slugs.contains(&"alpha"), "per_board must include alpha");
}

// ---------------------------------------------------------------------------
// Test 3: tick_skips_corrupt_board_logs_warn_continues_sweeping
// ---------------------------------------------------------------------------

/// When a board's DB is a non-SQLite file, open_for_board returns Err.
/// The tick logs warn + skips that board but continues sweeping others.
/// The default board's events are still delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn tick_skips_corrupt_board_logs_warn_continues_sweeping() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();

    // Create boards/broken/ with non-SQLite bytes.
    let broken_dir = dir.path().join("kanban").join("boards").join("broken");
    std::fs::create_dir_all(&broken_dir).unwrap();
    std::fs::write(broken_dir.join("kanban.db"), b"not valid sqlite data!!!").unwrap();

    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Seed default board with a Completed event + subscription.
    let default_store = open_store_at(&dir, "kanban.db");
    let _task = seed_completed_with_sub(&default_store, "default-task", "telegram", "42").await;

    let (send_fn, log) = make_recording_send_fn();
    // Sweep All: default + broken (broken will fail to open).
    let ctx =
        NotifierContext::new_multi_board(default_store, 1, send_fn, SubscribeBoardsConfig::All);
    let report = run_notifier_tick(&ctx)
        .await
        .expect("tick should not return Err even with corrupt board");

    // Corrupt board was skipped — not in per_board.
    let swept_slugs: Vec<&str> = report.per_board.iter().map(|s| s.slug.as_str()).collect();
    assert!(
        !swept_slugs.contains(&"broken"),
        "corrupt board must be skipped from per_board, got: {:?}",
        swept_slugs
    );

    // Default board was still swept and delivered.
    assert!(
        swept_slugs.contains(&"default"),
        "default board must still be swept"
    );
    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "default board event must still be delivered despite broken board"
    );
    assert_eq!(report.delivered, 1);
}

// ---------------------------------------------------------------------------
// Test 4: per_board_watermark_advances_independently
// ---------------------------------------------------------------------------

/// Events on default and alpha have different rowids. After a tick, both board
/// watermarks advance to their own MAX(id), not each other's.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn per_board_watermark_advances_independently() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();

    let alpha_board_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_board_dir).unwrap();
    let alpha_db_path = alpha_board_dir.join("kanban.db");

    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Seed default board with 2 Completed events.
    let default_store = open_store_at(&dir, "kanban.db");
    let _d1 = seed_completed_with_sub(&default_store, "d-task-1", "telegram", "10").await;
    let _d2 = seed_completed_with_sub(&default_store, "d-task-2", "telegram", "11").await;

    // Seed alpha board with 1 Completed event.
    let alpha_max_id = {
        let alpha_store = KanbanStore::new(&alpha_db_path).expect("open alpha store");
        let alpha_arc = Arc::new(TokioMutex::new(alpha_store));
        let _a1 = seed_completed_with_sub(&alpha_arc, "a-task-1", "telegram", "20").await;
        let max = {
            let s = alpha_arc.lock().await;
            s.max_event_id().expect("max_event_id")
        };
        drop(alpha_arc);
        max
    };

    let default_max_id = {
        let s = default_store.lock().await;
        s.max_event_id().expect("max_event_id")
    };

    let (send_fn, _log) = make_recording_send_fn();
    let ctx =
        NotifierContext::new_multi_board(default_store, 1, send_fn, SubscribeBoardsConfig::All);
    let _report = run_notifier_tick(&ctx).await.expect("tick");

    // Check watermarks advanced independently.
    let map = ctx.board_watermarks.lock().unwrap();
    let wm_default = map
        .get("default")
        .expect("default watermark must exist")
        .load(std::sync::atomic::Ordering::SeqCst);
    let wm_alpha = map
        .get("alpha")
        .expect("alpha watermark must exist")
        .load(std::sync::atomic::Ordering::SeqCst);

    assert_eq!(
        wm_default, default_max_id,
        "default watermark must advance to default board MAX(id)"
    );
    assert_eq!(
        wm_alpha, alpha_max_id,
        "alpha watermark must advance to alpha board MAX(id)"
    );
    assert_ne!(
        wm_default, wm_alpha,
        "per-board watermarks must be independent (T-36.3.7.9-05-03)"
    );
}

// ---------------------------------------------------------------------------
// Test 5: subscribe_explicit_filters_out_unlisted_boards
// ---------------------------------------------------------------------------

/// D-03 inert-codepath proof: Explicit(["alpha"]) sweeps ONLY alpha.
/// Default board events are NOT processed even though they exist.
/// This test proves the explicit codepath ships and filters correctly.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn subscribe_explicit_filters_out_unlisted_boards() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();

    let alpha_board_dir = dir.path().join("kanban").join("boards").join("alpha");
    std::fs::create_dir_all(&alpha_board_dir).unwrap();
    let alpha_db_path = alpha_board_dir.join("kanban.db");

    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    // Seed default board with a Completed event + sub.
    let default_store = open_store_at(&dir, "kanban.db");
    let _default_task =
        seed_completed_with_sub(&default_store, "default-task", "telegram", "99").await;

    // Seed alpha board with a Completed event + sub.
    {
        let alpha_store = KanbanStore::new(&alpha_db_path).expect("open alpha");
        let alpha_arc = Arc::new(TokioMutex::new(alpha_store));
        let _a1 = seed_completed_with_sub(&alpha_arc, "alpha-task", "telegram", "77").await;
        drop(alpha_arc);
    }

    let (send_fn, log) = make_recording_send_fn();
    // Only sweep "alpha" — default must NOT be processed.
    let ctx = NotifierContext::new_multi_board(
        default_store,
        1,
        send_fn,
        SubscribeBoardsConfig::Explicit(vec!["alpha".to_string()]),
    );
    let report = run_notifier_tick(&ctx).await.expect("tick");

    let calls = log.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "only alpha's event must be delivered (D-03 inert Explicit codepath)"
    );
    // Verify it came from chat_id "77" (alpha's sub), not "99" (default's sub).
    assert_eq!(
        calls[0].1, "77",
        "delivered event must be from alpha (chat_id=77), not default (chat_id=99)"
    );

    // Default must not appear in per_board sweep.
    let swept_slugs: Vec<&str> = report.per_board.iter().map(|s| s.slug.as_str()).collect();
    assert!(
        !swept_slugs.contains(&"default"),
        "default must NOT be swept when Explicit([alpha])"
    );
    assert!(swept_slugs.contains(&"alpha"), "alpha must be swept");
}

// ---------------------------------------------------------------------------
// Test 6: subscribe_all_with_zero_boards_returns_default_only_tick
// ---------------------------------------------------------------------------

/// When subscribe_boards = All and no boards/ directory exists, the tick sweeps
/// only "default" and delivers its events.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn subscribe_all_with_zero_boards_returns_default_only_tick() {
    let _lock = ENV_LOCK.lock().await;
    let dir = TempDir::new().unwrap();
    // No boards/ directory — list_boards() returns empty.
    let _env = ScopedEnv::set("IRONHERMES_HOME", dir.path().to_str().unwrap());

    let default_store = open_store_at(&dir, "kanban.db");
    let _task = seed_completed_with_sub(&default_store, "only-task", "discord", "55").await;

    let (send_fn, log) = make_recording_send_fn();
    let ctx =
        NotifierContext::new_multi_board(default_store, 1, send_fn, SubscribeBoardsConfig::All);
    let report = run_notifier_tick(&ctx).await.expect("tick");

    assert_eq!(
        log.lock().unwrap().len(),
        1,
        "default board event delivered even with no named boards"
    );
    assert_eq!(report.per_board.len(), 1, "only default swept");
    assert_eq!(report.per_board[0].slug, "default");
    assert_eq!(report.delivered, 1);
}
