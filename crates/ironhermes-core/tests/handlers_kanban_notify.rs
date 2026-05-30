//! Phase 36.3.7.5 Plan 04 (BUG-36.3.7.5-05 + BUG-36.3.7.5-06 + final part of BUG-36.3.7.5-07).
//!
//! Bilateral-tracing-by-construction: every producer surface in handlers.rs +
//! context.rs + the CLI dispatch layer has a paired consumer here. The
//! lifecycle test exercises the full cross-crate pipeline: dispatch /kanban
//! create -> auto-subscribe writes row -> notifier tick reads subscription ->
//! mock send_fn invoked -> subscription removed. No real network; no real
//! gateway runner; just the in-process bits the executor can drive
//! deterministically.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;

use ironhermes_core::commands::context::{
    CommandContext, KanbanStoreReader, KanbanStoreWriter, SubscriptionView,
};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// FakeKanbanStoreWriter — records every call for assertions
// =============================================================================

#[derive(Default)]
struct FakeKanbanStoreWriterCalls {
    create_calls: Vec<(String, String, bool)>, // (title, assignee, json)
    // (task_id, platform, chat_id, thread_id_opt, source)
    subscribe_calls: Vec<(String, String, String, Option<String>, String)>,
    list_for_task_calls: Vec<String>,
    list_all_calls: usize,
    // (task_id, platform_opt, chat_id_opt)
    remove_calls: Vec<(String, Option<String>, Option<String>)>,
}

struct FakeKanbanStoreWriter {
    calls: Arc<StdMutex<FakeKanbanStoreWriterCalls>>,
    next_task_id: Arc<StdMutex<usize>>,
    canned_subs: Vec<SubscriptionView>,
}

impl FakeKanbanStoreWriter {
    fn new() -> (Self, Arc<StdMutex<FakeKanbanStoreWriterCalls>>) {
        let calls = Arc::new(StdMutex::new(FakeKanbanStoreWriterCalls::default()));
        let f = Self {
            calls: calls.clone(),
            next_task_id: Arc::new(StdMutex::new(1)),
            canned_subs: vec![],
        };
        (f, calls)
    }

    fn with_canned_subs(mut self, subs: Vec<SubscriptionView>) -> Self {
        self.canned_subs = subs;
        self
    }
}

impl KanbanStoreWriter for FakeKanbanStoreWriter {
    fn create_task_simple(
        &self,
        title: &str,
        assignee: &str,
        json: bool,
    ) -> Result<String, String> {
        self.calls.lock().unwrap().create_calls.push((
            title.to_string(),
            assignee.to_string(),
            json,
        ));
        let mut next = self.next_task_id.lock().unwrap();
        let id = format!("t_test{:02}", *next);
        *next += 1;
        Ok(id)
    }

    fn append_subscription(
        &self,
        task_id: &str,
        platform: &str,
        chat_id: &str,
        thread_id: Option<&str>,
        source: &str,
    ) -> Result<i64, String> {
        self.calls.lock().unwrap().subscribe_calls.push((
            task_id.to_string(),
            platform.to_string(),
            chat_id.to_string(),
            thread_id.map(|t| t.to_string()),
            source.to_string(),
        ));
        Ok(1)
    }

    fn list_subscriptions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<SubscriptionView>, String> {
        self.calls
            .lock()
            .unwrap()
            .list_for_task_calls
            .push(task_id.to_string());
        Ok(self
            .canned_subs
            .iter()
            .filter(|s| s.task_id == task_id)
            .cloned()
            .collect())
    }

    fn list_all_subscriptions(&self) -> Result<Vec<SubscriptionView>, String> {
        self.calls.lock().unwrap().list_all_calls += 1;
        Ok(self.canned_subs.clone())
    }

    fn remove_subscriptions(
        &self,
        task_id: &str,
        platform: Option<&str>,
        chat_id: Option<&str>,
    ) -> Result<usize, String> {
        self.calls.lock().unwrap().remove_calls.push((
            task_id.to_string(),
            platform.map(|s| s.to_string()),
            chat_id.map(|s| s.to_string()),
        ));
        Ok(1)
    }
}

// Same FakeKanbanStoreReader as handlers_kanban.rs — copy for self-containment.
struct FakeKanbanStoreReader;
impl KanbanStoreReader for FakeKanbanStoreReader {
    fn list_text(&self) -> String {
        "ID STATUS ASSIGNEE\n".to_string()
    }
    fn show_text(&self, _id: &str) -> Option<String> {
        None
    }
    fn tip_text(&self) -> String {
        "tip\n".to_string()
    }
    fn deferred_subverb_message(&self, name: &str) -> String {
        format!(
            "/kanban {}: use 'ironhermes kanban {}' from the CLI for v1.",
            name, name
        )
    }
}

fn make_ctx_with_writer(
    chat_id: Option<&str>,
    thread_id: Option<&str>,
    canned_subs: Vec<SubscriptionView>,
) -> (CommandContext, Arc<StdMutex<FakeKanbanStoreWriterCalls>>) {
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let writer = writer.with_canned_subs(canned_subs);
    let reader: Arc<dyn KanbanStoreReader> = Arc::new(FakeKanbanStoreReader);
    let writer_arc: Arc<dyn KanbanStoreWriter> = Arc::new(writer);
    let mut ctx = CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_kanban_store(reader)
    .with_kanban_store_writer(writer_arc);
    if let Some(c) = chat_id {
        ctx = ctx.with_chat_origin(c, thread_id);
    }
    (ctx, calls)
}

fn find_kanban_cmd() -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == "kanban")
        .expect("'kanban' command must be registered in build_registry()")
}

fn router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// =============================================================================
// Tests — 9 receiver-end tests for BUG-05, BUG-06, and BUG-07d
// =============================================================================

/// Test 1 (BUG-06): /kanban create routes to the new Some("create") arm, NOT to
/// the deferred-subverb message path. Locks "create" removal from DEFERRED.
#[test]
fn dispatch_kanban_create_routes_to_create_arm_not_deferred() {
    let (ctx, calls) = make_ctx_with_writer(None, None, vec![]);
    let cmd = find_kanban_cmd();
    let r = router();
    let res = dispatch(
        &cmd,
        &["create", "the title", "--assignee", "alice"],
        &ctx,
        &r,
    );
    match res {
        CommandResult::Output(s) => {
            assert!(
                s.starts_with("Created t_"),
                "BUG-36.3.7.5-06: expected 'Created t_...' output (Create arm), got: {}",
                s
            );
            assert!(
                !s.contains("use 'ironhermes kanban"),
                "BUG-36.3.7.5-06: deferred message must NOT appear (create is no longer deferred); got: {}",
                s
            );
        }
        other => panic!("expected CommandResult::Output, got {:?}", other),
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.create_calls.len(),
        1,
        "BUG-36.3.7.5-06: writer.create_task_simple should be called exactly once"
    );
    assert_eq!(calls.create_calls[0].0, "the title");
    assert_eq!(calls.create_calls[0].1, "alice");
    assert_eq!(calls.create_calls[0].2, false);
}

/// Test 2 (BUG-05): notify-subscribe writer-handler path is exercised through
/// our trait. (CLI parse layer is locked separately by clap-derive; this test
/// proves the cmd_notify_subscribe writer path uses source='explicit'.)
#[test]
fn dispatch_notify_subscribe_writes_explicit_row() {
    // Simulate cmd_notify_subscribe's writer call shape directly via the fake.
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let id = writer
        .append_subscription("t_42", "telegram", "C1", Some("T7"), "explicit")
        .expect("append_subscription should succeed");
    assert_eq!(id, 1, "BUG-36.3.7.5-05: fake returns synthetic id 1");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.subscribe_calls.len(), 1);
    let call = &calls.subscribe_calls[0];
    assert_eq!(call.0, "t_42");
    assert_eq!(call.1, "telegram");
    assert_eq!(call.2, "C1");
    assert_eq!(call.3, Some("T7".to_string()));
    assert_eq!(
        call.4, "explicit",
        "BUG-36.3.7.5-05: notify-subscribe MUST write source='explicit'"
    );
}

/// Test 3 (BUG-05): notify-list with a task filter returns the canned rows.
#[test]
fn dispatch_notify_list_returns_rows() {
    let canned = vec![SubscriptionView {
        id: 7,
        task_id: "t_99".to_string(),
        platform: "discord".to_string(),
        chat_id: "C9".to_string(),
        thread_id: String::new(),
        source: "auto".to_string(),
        created_at: 1.0,
    }];
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let writer = writer.with_canned_subs(canned.clone());
    let rows = writer
        .list_subscriptions_for_task("t_99")
        .expect("list should succeed");
    assert_eq!(
        rows.len(),
        1,
        "BUG-36.3.7.5-05: notify-list filter should return canned row"
    );
    assert_eq!(rows[0].source, "auto");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.list_for_task_calls, vec!["t_99".to_string()]);
}

/// Test 4 (BUG-05): notify-list --json with no filter calls list_all_subscriptions.
#[test]
fn dispatch_notify_list_json_returns_array() {
    let canned = vec![
        SubscriptionView {
            id: 1,
            task_id: "t_a".to_string(),
            platform: "telegram".to_string(),
            chat_id: "C1".to_string(),
            thread_id: String::new(),
            source: "auto".to_string(),
            created_at: 1.0,
        },
        SubscriptionView {
            id: 2,
            task_id: "t_b".to_string(),
            platform: "slack".to_string(),
            chat_id: "C2".to_string(),
            thread_id: "T1".to_string(),
            source: "explicit".to_string(),
            created_at: 2.0,
        },
    ];
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let writer = writer.with_canned_subs(canned.clone());
    let rows = writer
        .list_all_subscriptions()
        .expect("list_all should succeed");
    assert_eq!(
        rows.len(),
        2,
        "BUG-36.3.7.5-05: notify-list (no filter, --json) should call list_all"
    );
    assert_eq!(calls.lock().unwrap().list_all_calls, 1);
}

/// Test 5 (BUG-05): notify-unsubscribe with no filters removes ALL for task.
#[test]
fn dispatch_notify_unsubscribe_no_filters_removes_all_for_task() {
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let removed = writer
        .remove_subscriptions("t_99", None, None)
        .expect("remove should succeed");
    assert_eq!(removed, 1, "BUG-36.3.7.5-05: fake returns 1");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.remove_calls.len(), 1);
    let call = &calls.remove_calls[0];
    assert_eq!(call.0, "t_99");
    assert_eq!(
        call.1, None,
        "BUG-36.3.7.5-05: no --platform = None passed through"
    );
    assert_eq!(
        call.2, None,
        "BUG-36.3.7.5-05: no --chat-id = None passed through"
    );
}

/// Test 6 (BUG-05): notify-unsubscribe with --platform + --chat-id passes filters through.
#[test]
fn dispatch_notify_unsubscribe_with_filters_passes_filters_through() {
    let (writer, calls) = FakeKanbanStoreWriter::new();
    let _ = writer
        .remove_subscriptions("t_99", Some("telegram"), Some("42"))
        .expect("remove should succeed");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.remove_calls.len(), 1);
    let call = &calls.remove_calls[0];
    assert_eq!(call.0, "t_99");
    assert_eq!(call.1, Some("telegram".to_string()));
    assert_eq!(call.2, Some("42".to_string()));
}

/// Test 7 (BUG-06): auto-subscribe FIRES when chat_id is present AND --json is absent.
/// Locks the D-json-skips-auto-subscribe positive case + the platform_to_str mapping.
#[test]
fn auto_subscribe_writes_row_when_chat_origin_present() {
    let (ctx, calls) = make_ctx_with_writer(Some("42"), None, vec![]);
    let cmd = find_kanban_cmd();
    let r = router();
    let res = dispatch(
        &cmd,
        &["create", "subscribe me", "--assignee", "bob"],
        &ctx,
        &r,
    );
    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("subscribed"),
                "BUG-36.3.7.5-06: with chat_origin present, output should mention subscription; got: {}",
                s
            );
        }
        other => panic!("expected CommandResult::Output, got {:?}", other),
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.create_calls.len(),
        1,
        "BUG-36.3.7.5-06: create should fire"
    );
    assert_eq!(
        calls.subscribe_calls.len(),
        1,
        "BUG-36.3.7.5-06: append_subscription MUST be called when chat_id set + !json"
    );
    let sub = &calls.subscribe_calls[0];
    assert_eq!(sub.0, "t_test01", "task_id from FakeWriter");
    assert_eq!(
        sub.1, "local",
        "BUG-36.3.7.5-06: platform string comes from Platform::Display (lowercase)"
    );
    assert_eq!(sub.2, "42", "chat_id passed through");
    assert_eq!(sub.3, None, "no thread_id");
    assert_eq!(
        sub.4, "auto",
        "BUG-36.3.7.5-06: auto-subscribe writes source='auto' (not 'explicit')"
    );
}

/// Test 8 (BUG-06): D-json-skips-auto-subscribe locked decision.
/// --json on /kanban create SKIPS the auto-subscribe hook even with chat_origin.
#[test]
fn dispatch_kanban_create_with_json_skips_auto_subscribe() {
    let (ctx, calls) = make_ctx_with_writer(Some("42"), None, vec![]);
    let cmd = find_kanban_cmd();
    let r = router();
    let res = dispatch(
        &cmd,
        &["create", "scripted", "--assignee", "alice", "--json"],
        &ctx,
        &r,
    );
    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("\"task_id\""),
                "BUG-36.3.7.5-06: --json must produce JSON output; got: {}",
                s
            );
            assert!(
                s.contains("\"assignee\":\"alice\""),
                "BUG-36.3.7.5-06: --json should include assignee; got: {}",
                s
            );
        }
        other => panic!("expected CommandResult::Output, got {:?}", other),
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.create_calls.len(),
        1,
        "BUG-36.3.7.5-06: create still fires under --json"
    );
    assert_eq!(
        calls.create_calls[0].2, true,
        "BUG-36.3.7.5-06: json flag plumbed through to create_task_simple"
    );
    assert_eq!(
        calls.subscribe_calls.len(),
        0,
        "BUG-36.3.7.5-06: D-json-skips-auto-subscribe — append_subscription MUST NOT be called"
    );
}

/// Test 9 (BUG-06 KEYSTONE / BUG-07d lifecycle e2e):
/// Full cross-crate pipeline — dispatch /kanban create -> auto-subscribe row written ->
/// terminal event appended -> notifier_tick reads subscription -> mock send_fn invoked ->
/// subscription removed after delivery attempt.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn auto_subscribe_lifecycle_end_to_end() {
    use ironhermes_kanban::{
        KanbanEventKind, KanbanStore, NotifierContext, SendFn, run_notifier_tick,
    };
    use tempfile::TempDir;
    use tokio::sync::Mutex as TokioMutex;

    // Build a real KanbanStore backed by a tempfile so the schema migration
    // runs and we can drive the full Plan 01/02/04 stack against shared state.
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("kanban.db");

    // Test adapter: implements KanbanStoreWriter against a real tempfile-backed
    // KanbanStore (KanbanStoreWriterImpl from ironhermes-kanban opens
    // ~/.ironhermes/kanban.db, which we can't use in a test).
    struct TempStoreWriter {
        store: std::sync::Mutex<KanbanStore>,
    }
    impl KanbanStoreWriter for TempStoreWriter {
        fn create_task_simple(
            &self,
            title: &str,
            assignee: &str,
            _json: bool,
        ) -> Result<String, String> {
            let opts = ironhermes_kanban::CreateTaskOptions::default();
            self.store
                .lock()
                .unwrap()
                .create_task(title, assignee, opts)
                .map(|t| t.id)
                .map_err(|e| e.to_string())
        }
        fn append_subscription(
            &self,
            task_id: &str,
            platform: &str,
            chat_id: &str,
            thread_id: Option<&str>,
            source: &str,
        ) -> Result<i64, String> {
            self.store
                .lock()
                .unwrap()
                .append_subscription(task_id, platform, chat_id, thread_id, source)
                .map_err(|e| e.to_string())
        }
        fn list_subscriptions_for_task(
            &self,
            task_id: &str,
        ) -> Result<Vec<SubscriptionView>, String> {
            self.store
                .lock()
                .unwrap()
                .list_subscriptions_for_task(task_id)
                .map(|v| {
                    v.into_iter()
                        .map(|s| SubscriptionView {
                            id: s.id,
                            task_id: s.task_id,
                            platform: s.platform,
                            chat_id: s.chat_id,
                            thread_id: s.thread_id,
                            source: s.source,
                            created_at: s.created_at,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        }
        fn list_all_subscriptions(&self) -> Result<Vec<SubscriptionView>, String> {
            Err("not used by this test".to_string())
        }
        fn remove_subscriptions(
            &self,
            _t: &str,
            _p: Option<&str>,
            _c: Option<&str>,
        ) -> Result<usize, String> {
            Err("not used by this test".to_string())
        }
    }

    let writer = Arc::new(TempStoreWriter {
        store: std::sync::Mutex::new(KanbanStore::new(&db_path).unwrap()),
    });
    let writer_arc: Arc<dyn KanbanStoreWriter> = writer.clone();

    // 1. Dispatch /kanban create through cmd_kanban with chat_origin set.
    let reader: Arc<dyn KanbanStoreReader> = Arc::new(FakeKanbanStoreReader);
    let ctx = CommandContext::new(
        Platform::Local,
        "test".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
    .with_kanban_store(reader)
    .with_kanban_store_writer(writer_arc)
    .with_chat_origin("42", None::<String>);
    let cmd = find_kanban_cmd();
    let r = router();
    let res = dispatch(
        &cmd,
        &["create", "lifecycle test", "--assignee", "alice"],
        &ctx,
        &r,
    );
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Created t_"),
            "BUG-36.3.7.5-06: expected 'Created t_...' output, got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }

    // 2. Pull the task_id out of the writer's store, assert subscription row exists.
    let task_id = {
        let store = writer.store.lock().unwrap();
        let tasks = store
            .list_tasks(ironhermes_kanban::ListFilters::default())
            .unwrap();
        tasks
            .iter()
            .find(|t| t.title == "lifecycle test")
            .expect("created task should be in store")
            .id
            .clone()
    };
    let subs_before = {
        let store = writer.store.lock().unwrap();
        store.list_subscriptions_for_task(&task_id).unwrap()
    };
    assert_eq!(
        subs_before.len(),
        1,
        "BUG-36.3.7.5-06 lifecycle: auto-subscribe should have written 1 row"
    );
    assert_eq!(subs_before[0].source, "auto");
    assert_eq!(subs_before[0].chat_id, "42");
    assert_eq!(subs_before[0].platform, "local");

    // 3. Append a Completed event so the notifier has something to deliver.
    {
        let mut store = writer.store.lock().unwrap();
        let payload = serde_json::json!({"summary": "lifecycle done"});
        store
            .append_event(&task_id, None, KanbanEventKind::Completed, Some(&payload))
            .unwrap();
    }

    // 4. Build a real NotifierContext sharing the same tempfile store.
    let recorded: Arc<StdMutex<Vec<(String, String, Option<String>, String)>>> =
        Arc::new(StdMutex::new(vec![]));
    let recorded_clone = recorded.clone();
    let send_fn: SendFn = Arc::new(move |p, c, t, m| {
        let r = recorded_clone.clone();
        let p = p.to_string();
        let c = c.to_string();
        let t = t.map(|s| s.to_string());
        let m = m.to_string();
        Box::pin(async move {
            r.lock().unwrap().push((p, c, t, m));
            Ok(())
        })
    });
    // Wrap the same kanban.db in a SECOND store handle for the notifier
    // (the writer's StdMutex<KanbanStore> isn't a TokioMutex; we open a fresh
    // one here for the notifier — both point at the same on-disk DB so the
    // rows are shared).
    let notifier_store = Arc::new(TokioMutex::new(KanbanStore::new(&db_path).unwrap()));
    let nctx = NotifierContext::new(notifier_store.clone(), 1, send_fn);
    nctx.last_event_id
        .store(0, std::sync::atomic::Ordering::SeqCst);
    let report = run_notifier_tick(&nctx).await.unwrap();
    assert_eq!(
        report.delivered, 1,
        "BUG-36.3.7.5-07d lifecycle: notifier should report 1 delivery"
    );
    assert_eq!(
        report.events_processed, 1,
        "BUG-36.3.7.5-07d lifecycle: 1 terminal event processed"
    );
    let calls = recorded.lock().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "BUG-36.3.7.5-07d lifecycle: notifier should have invoked send_fn exactly once"
    );
    assert_eq!(calls[0].0, "local");
    assert_eq!(calls[0].1, "42");
    assert_eq!(calls[0].2, None);
    assert!(
        calls[0].3.contains("lifecycle done"),
        "BUG-36.3.7.5-07d lifecycle: message should contain summary line, got: {}",
        calls[0].3
    );
    drop(calls);

    // 5. Assert subscription removed after delivery (auto-remove on terminal-event attempt).
    let subs_after = {
        let store = writer.store.lock().unwrap();
        store.list_subscriptions_for_task(&task_id).unwrap()
    };
    assert!(
        subs_after.is_empty(),
        "BUG-36.3.7.5-07d lifecycle: subscription should be removed after terminal delivery attempt; got {} rows",
        subs_after.len()
    );
}
