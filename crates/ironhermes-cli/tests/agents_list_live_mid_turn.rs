//! Phase 21.7 Plan 11 — deterministic regression test for GAP-21.7-01.
//!
//! Builds a REAL SubagentRegistry, registers a fake SubagentInfo directly,
//! wraps it in the production SubagentRegistryHandle trait-object, and
//! exercises the LIVE cmd_agents dispatch path. Asserts that with a
//! populated registry the handler returns "Active subagents:\n- <id> ..."
//! (the UAT-1 expected surface) — not the "No active subagents" leaf.
//!
//! This is the handler-level Nyquist gate for GAP-21.7-01: the pre-plan-11
//! defect was NOT in the handler (handler.rs:144-213 was already correct);
//! it was in run_chat's sequential rustyline readline blocking the handler
//! from ever being invoked while the registry was populated. This test
//! locks the handler contract so any future refactor that re-breaks
//! cmd_agents's ability to read a populated registry will fire here.
//!
//! The live end-to-end mid-turn keystroke path is tested by the HUMAN-UAT
//! re-run (requires a real TTY); the static-grep invariants INV-21.7-12
//! and INV-21.7-13 in tests/invariants_21_7.rs lock the REPL wiring so
//! the main.rs select arm cannot silently disappear.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ironhermes_agent::subagent_registry::{
    SubagentInfo, SubagentRegistry, SubagentRegistryHandle,
};
use ironhermes_core::commands::context::{CommandContext, SubagentListSnapshot};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandDef, CommandResult, CommandRouter};
use ironhermes_core::types::Platform;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

// =========================================================================
// Helpers
// =========================================================================

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

fn fake_info(id: &str, summary: &str) -> SubagentInfo {
    SubagentInfo {
        id: id.to_string(),
        task_summary: summary.to_string(),
        parent_id: None,
        started_at: Instant::now(),
        cancel: CancellationToken::new(),
        transcript_path: PathBuf::from(format!("/tmp/transcript-{}.jsonl", id)),
    }
}

// =========================================================================
// GAP-21.7-01 Regression Gate
// =========================================================================

/// P11-T01: With a populated SubagentRegistry (the exact shape that exists
/// mid-turn when delegate_task has spawned subagents), `/agents list`
/// must return a non-empty "Active subagents:" string containing the
/// registered id and task summary.
///
/// Before plan-11 the user could never reach this code path because
/// run_chat's sequential readline blocked on rustyline until every
/// subagent had already unregistered. This test proves the handler
/// contract holds when the registry IS populated — the piece plan-11
/// closes is REPL concurrency, not handler correctness.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_agents_list_returns_active_subagents_when_registry_populated() {
    // Build a REAL SubagentRegistry — not a fake — wrap it in the
    // production SubagentRegistryHandle trait-object, and install on
    // CommandContext via the real with_subagent_registry builder.
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    reg.write().await.register(fake_info(
        "sub_deadbeefcafe",
        "research LoRA training corpora",
    ));

    let handle: Arc<dyn SubagentListSnapshot> =
        Arc::new(SubagentRegistryHandle::new(reg.clone()));
    let ctx = base_ctx().with_subagent_registry(handle);

    // Dispatch via the SAME dispatch function the REPL calls (plan 08
    // precedent). /agents default subcommand is "list".
    let cmd = find_cmd("agents");
    let r = router();
    let res = tokio::task::spawn_blocking(move || dispatch(&cmd, &["list"], &ctx, &r))
        .await
        .unwrap();

    match res {
        CommandResult::Output(s) => {
            assert!(
                s.contains("Active subagents"),
                "expected 'Active subagents' header; got: {}",
                s
            );
            assert!(
                s.contains("sub_deadbeefcafe"),
                "expected registered id in output; got: {}",
                s
            );
            assert!(
                s.contains("research LoRA training corpora"),
                "expected task summary in output; got: {}",
                s
            );
        }
        other => panic!("expected Output, got {:?}", other),
    }
}

/// P11-T02: Baseline — empty registry must still return
/// "No active subagents." so the handler's two-branch contract
/// (empty vs populated) is fully covered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_agents_list_returns_empty_string_when_registry_empty() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    let handle: Arc<dyn SubagentListSnapshot> =
        Arc::new(SubagentRegistryHandle::new(reg.clone()));
    let ctx = base_ctx().with_subagent_registry(handle);

    let cmd = find_cmd("agents");
    let r = router();
    let res = tokio::task::spawn_blocking(move || dispatch(&cmd, &["list"], &ctx, &r))
        .await
        .unwrap();

    match res {
        CommandResult::Output(s) => assert!(
            s.contains("No active subagents"),
            "expected 'No active subagents'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }
}

/// P11-T03: `/agents kill <id>` against a populated registry cancels the
/// stored CancellationToken and removes the entry. This confirms the
/// full D-03 contract (kill semantics) works through the trait-object
/// bridge with a real tokio::sync::RwLock — identical to the live
/// production wiring in main.rs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_agents_kill_cancels_registered_subagent() {
    let reg = Arc::new(RwLock::new(SubagentRegistry::new()));
    // Construct SubagentInfo with a KNOWN CancellationToken so we can
    // assert is_cancelled() after the dispatch.
    let token = CancellationToken::new();
    let info = SubagentInfo {
        id: "sub_killme_1234".to_string(),
        task_summary: "doomed task".to_string(),
        parent_id: None,
        started_at: Instant::now(),
        cancel: token.clone(),
        transcript_path: PathBuf::from("/tmp/transcript-killme.jsonl"),
    };
    reg.write().await.register(info);

    let handle: Arc<dyn SubagentListSnapshot> =
        Arc::new(SubagentRegistryHandle::new(reg.clone()));
    let ctx = base_ctx().with_subagent_registry(handle);

    let cmd = find_cmd("agents");
    let r = router();
    let res = tokio::task::spawn_blocking(move || {
        dispatch(&cmd, &["kill", "sub_killme_1234"], &ctx, &r)
    })
    .await
    .unwrap();

    match res {
        CommandResult::Output(s) => assert!(
            s.contains("Cancelled subagent sub_killme_1234"),
            "expected 'Cancelled subagent sub_killme_1234'; got: {}",
            s
        ),
        other => panic!("expected Output, got {:?}", other),
    }

    // Give the kill token a tick to propagate (same-task synchronous
    // propagation is guaranteed by SubagentRegistry::kill's .cancel()
    // call, but be defensive).
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        token.is_cancelled(),
        "D-03: /agents kill must cancel the stored CancellationToken"
    );

    // And the entry must be gone from the registry.
    let remaining = reg.read().await.active_count();
    assert_eq!(
        remaining, 0,
        "D-03: /agents kill must remove the entry from the registry"
    );
}
