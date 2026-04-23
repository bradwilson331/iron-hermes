//! D-03 / D-04 / D-09 SubagentRegistry contract tests.

use ironhermes_agent::subagent_registry::{SubagentInfo, SubagentRegistry};
use std::path::PathBuf;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

fn mk(id: &str, cancel: CancellationToken) -> SubagentInfo {
    SubagentInfo {
        id: id.to_string(),
        task_summary: format!("task {}", id),
        parent_id: None,
        started_at: Instant::now(),
        cancel,
        transcript_path: PathBuf::from(format!("/tmp/{}.jsonl", id)),
    }
}

#[test]
fn register_then_unregister_leaves_count_zero() {
    let mut r = SubagentRegistry::new();
    let tok = CancellationToken::new();
    r.register(mk("sub1", tok));
    assert_eq!(r.active_count(), 1);
    assert_eq!(r.list().len(), 1);
    assert!(r.unregister("sub1").is_some());
    assert_eq!(r.active_count(), 0);
}

#[test]
fn kill_cancels_token_and_removes_entry() {
    let mut r = SubagentRegistry::new();
    let tok = CancellationToken::new();
    r.register(mk("sub1", tok.clone()));
    assert!(r.kill("sub1"), "kill must return true for present id");
    assert!(tok.is_cancelled(), "D-03 kill must cancel the token");
    assert_eq!(r.active_count(), 0);
}

#[test]
fn kill_missing_returns_false() {
    let mut r = SubagentRegistry::new();
    assert!(!r.kill("nope"));
}

#[test]
fn transcript_path_returns_stored_value() {
    let mut r = SubagentRegistry::new();
    let tok = CancellationToken::new();
    r.register(mk("sub1", tok));
    assert_eq!(
        r.transcript_path("sub1"),
        Some(PathBuf::from("/tmp/sub1.jsonl"))
    );
    assert_eq!(r.transcript_path("missing"), None);
}

#[test]
fn list_preserves_all_entries() {
    let mut r = SubagentRegistry::new();
    for id in ["a", "b", "c"] {
        r.register(mk(id, CancellationToken::new()));
    }
    assert_eq!(r.active_count(), 3);
    let list = r.list();
    assert_eq!(list.len(), 3);
    let ids: std::collections::HashSet<String> = list.into_iter().map(|i| i.id).collect();
    assert!(ids.contains("a") && ids.contains("b") && ids.contains("c"));
}
