//! Plan 21.7 Plan 08 (D-03 / D-26) — smoke tests for the filled-in
//! `/agents` and `/stop` handlers using minimal fake trait-object impls
//! of `SubagentListSnapshot` + `ProcessRegistrySnapshotHandle`.
//!
//! Driven through `dispatch(...)` so the slash-command routing, the
//! registry match arm, and the handler bodies are all exercised in one
//! shot. The existing `plan_21_7_07_tests` fakes in `context.rs` are
//! in-crate and cannot be reused from an integration test — so we
//! define tiny purpose-built fakes here.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use ironhermes_core::commands::context::{
    CommandContext, ProcessRegistrySnapshotHandle, SubagentListSnapshot,
};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;

// =============================================================================
// Fakes
// =============================================================================

/// SubagentListSnapshot fake parameterized by a static summary list.
struct FakeSubagents {
    entries: Vec<(String, String, std::time::Duration)>,
    killed: Mutex<Vec<String>>,
    kill_result: bool,
}

impl SubagentListSnapshot for FakeSubagents {
    fn active_count(&self) -> usize {
        self.entries.len()
    }
    fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
        self.entries.clone()
    }
    fn kill(&self, id: &str) -> bool {
        self.killed.lock().unwrap().push(id.to_string());
        self.kill_result && self.entries.iter().any(|(i, _, _)| i == id)
    }
    fn transcript_path(&self, _id: &str) -> Option<std::path::PathBuf> {
        None
    }
}

/// ProcessRegistrySnapshotHandle fake with a settable tracked count +
/// a "drained" flag that flips when `drain_and_kill` completes.
struct FakeProc {
    tracked: usize,
    drained: Arc<std::sync::atomic::AtomicBool>,
}

impl ProcessRegistrySnapshotHandle for FakeProc {
    fn tracked(&self) -> usize {
        self.tracked
    }
    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::json!({"tracked": self.tracked})
    }
    fn drain_and_kill<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        let flag = self.drained.clone();
        Box::pin(async move {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn base_ctx() -> CommandContext {
    CommandContext::new(
        Platform::Local,
        "test-session".to_string(),
        Arc::new(AtomicBool::new(false)),
    )
}

fn find_cmd(name: &str) -> CommandDef {
    build_registry()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("Command '{}' not found in registry", name))
}

fn router() -> CommandRouter {
    CommandRouter::new(build_registry())
}

// =============================================================================
// /agents
// =============================================================================

#[test]
fn agents_list_empty_registry_says_no_active_subagents() {
    let fake = FakeSubagents {
        entries: vec![],
        killed: Mutex::new(vec![]),
        kill_result: true,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("No active subagents"),
            "empty registry should emit 'No active subagents'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn agents_list_two_entries_contains_both_ids() {
    let fake = FakeSubagents {
        entries: vec![
            (
                "sub_aaaaaaaa".into(),
                "research thing".into(),
                std::time::Duration::from_secs(1),
            ),
            (
                "sub_bbbbbbbb".into(),
                "other thing".into(),
                std::time::Duration::from_secs(2),
            ),
        ],
        killed: Mutex::new(vec![]),
        kill_result: true,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => {
            assert!(s.contains("sub_aaaaaaaa"), "missing first id: {}", s);
            assert!(s.contains("sub_bbbbbbbb"), "missing second id: {}", s);
            assert!(s.contains("research thing"), "missing first summary: {}", s);
            assert!(s.contains("other thing"), "missing second summary: {}", s);
        }
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn agents_kill_present_id_emits_cancelled() {
    let fake = FakeSubagents {
        entries: vec![(
            "sub_cccccccc".into(),
            "doomed".into(),
            std::time::Duration::from_secs(0),
        )],
        killed: Mutex::new(vec![]),
        kill_result: true,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &["kill", "sub_cccccccc"], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Cancelled subagent sub_cccccccc"),
            "expected 'Cancelled subagent sub_cccccccc'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn agents_kill_absent_id_emits_no_active() {
    let fake = FakeSubagents {
        entries: vec![],
        killed: Mutex::new(vec![]),
        kill_result: false,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &["kill", "sub_nope"], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("No active subagent with id sub_nope"),
            "expected absent-id message; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn agents_kill_missing_id_returns_error() {
    let fake = FakeSubagents {
        entries: vec![],
        killed: Mutex::new(vec![]),
        kill_result: true,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &["kill"], &ctx, &r);
    assert!(
        matches!(res, CommandResult::Error(_)),
        "missing id on /agents kill should return Error, got {:?}",
        res
    );
}

#[test]
fn agents_logs_without_registry_says_not_wired() {
    let ctx = base_ctx();
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &["logs", "sub_xx"], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Subagent registry not wired"),
            "expected 'Subagent registry not wired'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn agents_unknown_subcommand_returns_error() {
    let fake = FakeSubagents {
        entries: vec![],
        killed: Mutex::new(vec![]),
        kill_result: true,
    };
    let ctx = base_ctx().with_subagent_registry(Arc::new(fake));
    let cmd = find_cmd("agents");
    let r = router();
    let res = dispatch(&cmd, &["explode"], &ctx, &r);
    assert!(
        matches!(res, CommandResult::Error(_)),
        "unknown /agents subcommand should return Error, got {:?}",
        res
    );
}

// =============================================================================
// /stop
// =============================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_with_zero_tracked_emits_count_zero() {
    let drained = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fake = FakeProc {
        tracked: 0,
        drained: drained.clone(),
    };
    let ctx = base_ctx().with_process_registry(Arc::new(fake));
    let cmd = find_cmd("stop");
    let r = router();
    let res = tokio::task::spawn_blocking(move || dispatch(&cmd, &[], &ctx, &r))
        .await
        .unwrap();
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Stopped 0 background process(es)"),
            "expected 'Stopped 0 background process(es)'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
    assert!(
        drained.load(std::sync::atomic::Ordering::SeqCst),
        "drain_and_kill must have run"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_with_three_tracked_emits_count_three() {
    let drained = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fake = FakeProc {
        tracked: 3,
        drained: drained.clone(),
    };
    let ctx = base_ctx().with_process_registry(Arc::new(fake));
    let cmd = find_cmd("stop");
    let r = router();
    let res = tokio::task::spawn_blocking(move || dispatch(&cmd, &[], &ctx, &r))
        .await
        .unwrap();
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Stopped 3 background process(es)"),
            "expected 'Stopped 3 background process(es)'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

#[test]
fn stop_with_no_registry_falls_back_to_idle_advisory() {
    let ctx = base_ctx();
    let cmd = find_cmd("stop");
    let r = router();
    let res = dispatch(&cmd, &[], &ctx, &r);
    match res {
        CommandResult::Output(s) => assert!(
            s.contains("No agent is currently running"),
            "expected idle fallback; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}
