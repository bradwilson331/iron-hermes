//! Phase 36.3.12 Plan 09 (CR-01) — concurrency regression test for session-scoped
//! intercept dispatch.
//!
//! Guards:
//!   - `crates/ironhermes-tools/src/registry.rs`: `ToolRegistry.intercepts` storage,
//!     `register_intercepted_or_replace`, `dispatch_intercepts`.
//!   - `crates/ironhermes-agent/src/agent_runtime.rs:441` / `:482`: the two
//!     `register_intercepted_or_replace("terminal"/"execute_code", ...)` call sites in
//!     `AgentRuntime::run_turn`.
//!   - `crates/ironhermes-agent/src/agent_loop.rs:~2591`: the `dispatch_intercepts`
//!     call site in `AgentLoop::execute_tool_call`.
//!
//! Regression this catches: a return to NAME-ONLY keying of the intercepts map. On the
//! ONE `Arc<AgentRuntime>` the gateway shares across every chat, name-only keying let
//! one chat's turn overwrite another chat's gated closure mid-flight — so a command
//! issued by chat A could dispatch through chat B's closure, misattributing the audit
//! log's `session_id`/`chat_id` and routing the approval prompt to the wrong operator.
//! Derived from `36.3.12-REVIEW.md` CR-01 (Critical).
//!
//! These tests exercise the REAL `ironhermes_tools::ToolRegistry` intercept API — the
//! same `register_intercepted_or_replace` / `dispatch_intercepts` entry points
//! `AgentRuntime::run_turn` and `AgentLoop` call in production — never a local
//! re-implementation of the lookup logic. This repo has a recorded failure mode of
//! tests that re-implement the code's own logic and assert on that (WR-02 in this
//! phase's REVIEW; project memory "tests that verify their own assumptions") — a test
//! that hand-rolled its own HashMap lookup would prove nothing about CR-01.

use std::sync::Arc;

use ironhermes_core::ToolSchema;
use ironhermes_tools::{InterceptHandler, ToolRegistry};
use tokio::sync::{Mutex, RwLock};

/// Build a real `InterceptHandler` closure that closes over its own session id and
/// records it into a shared log before returning it. This mirrors the exact
/// captured-identity-per-closure structure the real gating closures use
/// (`handler.rs:1939-2046`, `approval_gate.rs:184-230`) — that structure IS the
/// mechanism CR-01 exploits, so the test must reproduce it rather than use a
/// session-agnostic stand-in.
fn session_handler(session_id: &str, record: Arc<Mutex<Vec<String>>>) -> InterceptHandler {
    let session_id = session_id.to_string();
    Arc::new(move |_args: serde_json::Value| {
        let session_id = session_id.clone();
        let record = record.clone();
        Box::pin(async move {
            record.lock().await.push(session_id.clone());
            Ok(session_id)
        })
    })
}

fn terminal_schema() -> ToolSchema {
    ToolSchema::new(
        "terminal",
        "test terminal intercept (CR-01 regression harness)",
        serde_json::json!({ "type": "object", "properties": {} }),
    )
}

/// Reproduce CR-01's exact four-step interleaving:
///   1. Register a `"terminal"` intercept for `"sess-a"` whose handler records
///      `"sess-a"` into a shared `Arc<Mutex<Vec<String>>>` and returns it.
///   2. BEFORE dispatching for sess-a, register a `"terminal"` intercept for
///      `"sess-b"` whose handler records `"sess-b"`.
///   3. Dispatch `"terminal"` for `"sess-a"`.
///   4. Assert the recorded value AND the returned value are `"sess-a"`.
///
/// Against pre-fix (name-only-keyed) code, step 2's registration overwrites the
/// single name-keyed slot, so step 4 would observe `"sess-b"` instead — this is the
/// assertion that fails today (falsification evidence recorded in
/// `36.3.12-09-SUMMARY.md`).
#[tokio::test]
async fn interleaved_session_registration_does_not_cross_dispatch() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let record: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    // Step 1: sess-a registers first.
    {
        let mut reg = registry.write().await;
        reg.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            terminal_schema(),
            session_handler("sess-a", record.clone()),
        );
    }

    // Step 2: sess-b registers BEFORE sess-a ever dispatches — the exact interleaving
    // CR-01 describes (turn B installs its gated closure on the shared registry while
    // turn A's earlier registration is still "current").
    {
        let mut reg = registry.write().await;
        reg.register_intercepted_or_replace(
            "terminal",
            "sess-b",
            terminal_schema(),
            session_handler("sess-b", record.clone()),
        );
    }

    // Step 3: dispatch "terminal" for sess-a.
    let result = {
        let reg = registry.read().await;
        reg.dispatch_intercepts("terminal", "sess-a", serde_json::json!({}))
            .await
    };

    // Step 4: both the returned value and the recorded side effect must be "sess-a".
    let returned = result
        .expect("terminal is intercepted; sess-a must have a registered handler")
        .expect("sess-a's handler must succeed");
    assert_eq!(
        returned, "sess-a",
        "CR-01 regression: sess-a's dispatch returned a different session's output — \
         one chat's command dispatched through another chat's closure"
    );
    let recorded = record.lock().await.clone();
    assert_eq!(
        recorded,
        vec!["sess-a".to_string()],
        "CR-01 regression: the recorded side effect shows a different session's closure \
         ran for sess-a's dispatch, not sess-a's own closure — got {recorded:?}"
    );
}

/// Register handlers for eight distinct session ids (matching
/// `TelegramConfig::max_concurrent_runs`'s default of 8), each returning its own
/// session id. Drive all eight dispatches concurrently via `JoinSet`-style spawns;
/// every returned value must equal the session id it was dispatched for.
#[tokio::test]
async fn concurrent_dispatches_each_hit_their_own_session_handler() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let record: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let session_ids: Vec<String> = (0..8).map(|i| format!("sess-{i}")).collect();

    {
        let mut reg = registry.write().await;
        for sid in &session_ids {
            reg.register_intercepted_or_replace(
                "terminal",
                sid,
                terminal_schema(),
                session_handler(sid, record.clone()),
            );
        }
    }

    let mut handles = Vec::new();
    for sid in session_ids.clone() {
        let registry = registry.clone();
        handles.push(tokio::spawn(async move {
            // Take the lock per-operation (not held across the interleaving) so this
            // reflects real contention on the shared registry, not serialized access.
            let reg = registry.read().await;
            let result = reg
                .dispatch_intercepts("terminal", &sid, serde_json::json!({}))
                .await;
            (sid, result)
        }));
    }

    for handle in handles {
        let (sid, result) = handle.await.expect("dispatch task must not panic");
        let returned = result
            .unwrap_or_else(|| panic!("session {sid} must have a registered handler"))
            .unwrap_or_else(|e| panic!("session {sid}'s handler must succeed: {e}"));
        assert_eq!(
            returned, sid,
            "session {sid}'s concurrent dispatch must resolve to its OWN handler, \
             never another session's (CR-01)"
        );
    }
}

/// D-10 fail-closed lock: register `"terminal"` for `"sess-a"` ONLY. Dispatching
/// `"terminal"` for an unknown session (`"sess-ghost"`) must return `Some(Err(..))` —
/// never `None`, which would let the caller fall through to the raw ungated tool.
#[tokio::test]
async fn unknown_session_on_intercepted_tool_is_an_error_not_a_fallthrough() {
    let registry = Arc::new(RwLock::new(ToolRegistry::new()));
    let record: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    {
        let mut reg = registry.write().await;
        reg.register_intercepted_or_replace(
            "terminal",
            "sess-a",
            terminal_schema(),
            session_handler("sess-a", record),
        );
    }

    let result = {
        let reg = registry.read().await;
        reg.dispatch_intercepts("terminal", "sess-ghost", serde_json::json!({}))
            .await
    };

    assert!(
        result.is_some(),
        "an unknown session on an intercepted tool name must be Some(..), never None — \
         None would fall through to dispatch() and run the raw ungated tool (D-10)"
    );
    assert!(
        result.unwrap().is_err(),
        "an unknown session on an intercepted tool name must be Some(Err(..)) — a silent \
         Ok would be just as dangerous as a fall-through"
    );
}
