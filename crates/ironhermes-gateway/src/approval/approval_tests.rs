//! Unit tests for the ApprovalCoordinator (EXEC-05-e..m).
//!
//! Declared as `#[cfg(test)] mod approval_tests;` in `approval.rs` so this
//! module is a child of `crate::approval` and can access `pub(crate)` items
//! (e.g. `simulate_restart`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use ironhermes_core::{
    ApprovalOutcome, ApprovalsStore, AuditConfig, AuditLog, KeyKind, MessageResponse, Platform,
};
use tokio::sync::Mutex as TokioMutex;

use super::ApprovalCoordinator;
use crate::adapter::PlatformAdapter;

// ─────────────────────────────────────────────────────────────────────────────
// MockAdapter — captures every send_message call for assertion
// ─────────────────────────────────────────────────────────────────────────────

struct MockAdapter {
    messages: Arc<TokioMutex<Vec<String>>>,
}

impl MockAdapter {
    fn new() -> (Self, Arc<TokioMutex<Vec<String>>>) {
        let messages = Arc::new(TokioMutex::new(Vec::new()));
        (
            Self {
                messages: Arc::clone(&messages),
            },
            messages,
        )
    }
}

#[async_trait]
impl PlatformAdapter for MockAdapter {
    fn platform(&self) -> Platform {
        Platform::Telegram
    }

    async fn send_message(
        &self,
        _chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.messages.lock().await.push(content.to_owned());
        Ok(MessageResponse {
            message_id: "0".to_string(),
            chat_id: "0".to_string(),
            platform: Platform::Telegram,
        })
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }

    async fn edit_message(&self, _chat_id: &str, _message_id: &str, _content: &str) -> Result<()> {
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _content: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Test helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a working `AuditLog` backed by a fresh temp directory. The directory is
/// leaked (`TempDir::keep`) so it survives for the coordinator's lifetime in-test —
/// acceptable in a short-lived test process.
fn make_working_audit_log() -> Arc<AuditLog> {
    let dir = tempfile::tempdir().expect("tempdir for audit log").keep();
    Arc::new(AuditLog::with_path(
        dir.join("audit.jsonl"),
        AuditConfig::default(),
    ))
}

/// Build an `AuditLog` pointed at a path whose parent can never be created —
/// `blocker` is a regular file, so `create_dir_all(blocker/subdir)` fails with
/// `NotADirectory`. Used to exercise the D-02 fail-closed downgrade (EXEC-05 Task 3).
fn make_broken_audit_log() -> Arc<AuditLog> {
    let dir = tempfile::tempdir()
        .expect("tempdir for broken audit log")
        .keep();
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"not a directory").expect("create blocker file");
    let unwritable_path = blocker.join("subdir").join("audit.jsonl");
    Arc::new(AuditLog::with_path(unwritable_path, AuditConfig::default()))
}

/// Build a coordinator backed by an empty in-memory ApprovalsStore and a working
/// audit log (writes succeed, so tests exercise pre-46 approval-flow behavior
/// unaffected by the audit hook).
fn make_coordinator(timeout_secs: u64) -> (ApprovalCoordinator, Arc<TokioMutex<Vec<String>>>) {
    let (adapter, messages) = MockAdapter::new();
    // Use a non-existent path — we never call save_to_disk (D-07 negative).
    let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
        "/tmp/test-approval-placeholder-not-used.json",
    )));
    let coord = ApprovalCoordinator::new(
        timeout_secs,
        Arc::new(adapter) as Arc<dyn PlatformAdapter>,
        store,
        make_working_audit_log(),
    );
    (coord, messages)
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-e: resolve(true) unblocks a parked request() with Approved
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_resolve() {
    let (coord, _messages) = make_coordinator(120);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });

    // Yield to let the spawned task insert into pending and park at rx.await.
    tokio::task::yield_now().await;

    let found = coord.resolve("sess1", true).await;
    assert!(found, "resolve() must find and unblock the pending entry");

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "resolve(true) must yield Approved"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-f: timeout elapses → TimedOut
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_timeout() {
    tokio::time::pause();

    let (coord, _messages) = make_coordinator(5);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });

    // Let the spawned task park at rx.await.
    tokio::task::yield_now().await;

    // Advance time past the 5-second deadline.
    tokio::time::advance(Duration::from_secs(6)).await;

    // Let the timeout Elapsed branch execute.
    tokio::task::yield_now().await;

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::TimedOut,
        "elapsed timeout must yield TimedOut"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-g: two concurrent requests for same session — second is AlreadyPending
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_d02_already_pending() {
    let (coord, _messages) = make_coordinator(120);
    let coord = Arc::new(coord);

    // First request — will park at rx.await.
    let coord_first = Arc::clone(&coord);
    let first = tokio::spawn(async move {
        coord_first
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "first",
                &serde_json::json!({}),
            )
            .await
    });

    // Yield to let the first request insert into pending.
    tokio::task::yield_now().await;

    // Second request for the same session — must return AlreadyPending immediately.
    let second = coord
        .request(
            "sess1",
            "chat1",
            "cloudflare__kv_delete",
            "second",
            &serde_json::json!({}),
        )
        .await;
    assert_eq!(
        second,
        ApprovalOutcome::AlreadyPending,
        "D-02: second concurrent request for same session must be AlreadyPending"
    );

    // Resolve the first request to clean up.
    coord.resolve("sess1", false).await;
    first.await.expect("first task must not panic");
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-h: on timeout the adapter captures a D-05 expiry notice
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_d05_expiry_notify() {
    tokio::time::pause();

    let (coord, messages) = make_coordinator(5);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });

    // Let the spawned task park at rx.await.
    tokio::task::yield_now().await;

    // Advance past the 5-second timeout.
    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::task::yield_now().await;

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(outcome, ApprovalOutcome::TimedOut);

    // D-05: verify the expiry notice was sent.
    // We expect at minimum: [approval prompt, expiry notice] → ≥ 2 messages.
    let msgs = messages.lock().await;
    assert!(
        msgs.len() >= 2,
        "D-05: expected approval prompt + expiry notice (got {} messages: {msgs:?})",
        msgs.len()
    );
    let has_expiry = msgs
        .iter()
        .any(|m| m.contains("expired") || m.contains("Approval request expired"));
    assert!(
        has_expiry,
        "D-05: expiry notice must be sent on timeout; messages: {msgs:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-i: dropped oneshot sender (simulated restart) → Denied (D-07 fail-closed)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_d07_fail_closed() {
    let (coord, _messages) = make_coordinator(3600); // long timeout — won't fire
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });

    // Yield to let the spawned task park at rx.await.
    tokio::task::yield_now().await;

    // Simulate a coordinator restart: clear all pending entries, which drops
    // every oneshot::Sender.  The parked rx resolves to Err(RecvError) → Denied.
    coord.simulate_restart().await;

    // Allow the spawned task to process the dropped rx.
    tokio::task::yield_now().await;

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Denied,
        "D-07: dropped sender must resolve to Denied (fail-closed), not a hang"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-j: pre-seeded ApprovalsStore `always` → Approved with NO prompt (D-03)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_d03_always_bypass() {
    let (adapter, messages) = MockAdapter::new();
    // Build a store with a pre-seeded `always` approval (Command namespace, D-02).
    let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
        "/tmp/test-approval-d03-placeholder.json",
    )));
    let tool_name = "cloudflare__kv_delete";
    let cmd_key = ApprovalsStore::normalize_command(tool_name);
    store.approve_always(&cmd_key, KeyKind::Command).await;

    let coord = ApprovalCoordinator::new(
        120,
        Arc::new(adapter) as Arc<dyn PlatformAdapter>,
        store,
        make_working_audit_log(),
    );

    let outcome = coord
        .request("sess1", "chat1", tool_name, "test", &serde_json::json!({}))
        .await;
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "D-03: CLI always-approval must bypass chat prompt and return Approved immediately"
    );

    // D-03 negative: NO prompt should have been sent to chat.
    let msgs = messages.lock().await;
    assert!(
        msgs.is_empty(),
        "D-03: always-bypass must NOT send a chat prompt; captured messages: {msgs:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// EXEC-05-m: tool_name with glob → prompt contains TOCTOU warning (D-06)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn toctou_warning_glob() {
    let (coord, messages) = make_coordinator(120);
    let coord = Arc::new(coord);

    // A tool name with a glob metacharacter must trigger the TOCTOU warning.
    let tool_name = "cloudflare__delete_*";

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                tool_name,
                "bulk deletion",
                &serde_json::json!({}),
            )
            .await
    });

    // Allow the spawned task to send the approval prompt.
    tokio::task::yield_now().await;

    // Inspect the captured prompt before resolving.
    let prompt = {
        let msgs = messages.lock().await;
        assert!(
            !msgs.is_empty(),
            "expected an approval prompt message to be sent"
        );
        msgs[0].clone()
    };

    assert!(
        prompt.contains("TOCTOU") || prompt.contains("shell expansion"),
        "D-06: approval prompt must contain TOCTOU warning when tool_name has globs; \
         got: {prompt}"
    );

    // Resolve to unblock the spawned task.
    coord.resolve("sess1", false).await;
    tokio::task::yield_now().await;
    handle.await.expect("task must not panic");
}

// ─────────────────────────────────────────────────────────────────────────────
// HI-01: a dropped request() future must NOT wedge the session forever.
//
// Before the fix, only resolve() purged expired entries; the insert path checked
// contains_key and returned AlreadyPending without purging. A dropped request()
// future (turn cancelled at rx.await) left the PendingApproval in the map, so
// every subsequent request() returned AlreadyPending even after expiry, wedging
// the session until some unrelated resolve() happened to run retain().
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_hi01_dropped_future_not_wedged() {
    // NOTE: this test uses REAL elapsed time (no tokio::time::pause). The
    // coordinator's `expires_at` uses `std::time::Instant`, which
    // `tokio::time::advance()` does NOT move — so we must let real wall-clock time
    // pass for the zombie entry to actually expire and exercise the insert-path
    // purge. A 1-second timeout keeps the real wait short (~1.1 s).
    let (coord, _messages) = make_coordinator(1);
    let coord = Arc::new(coord);

    // First request parks at rx.await, inserting a PendingApproval (expires in 1s).
    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "srv__delete",
                "first",
                &serde_json::json!({}),
            )
            .await
    });
    tokio::task::yield_now().await;

    // Drop the request() future by aborting the task BEFORE its 1s timeout — the
    // PendingApproval stays in the map (its oneshot sender lives on) but the
    // receiver is gone. This is the exact "dropped future" leak HI-01 describes:
    // the timeout-cleanup block never runs, so only the insert-path purge can
    // evict it.
    handle.abort();
    let _ = handle.await; // join the cancelled task

    // Let real time pass the 1-second deadline so the zombie entry is expired.
    tokio::time::sleep(Duration::from_millis(1_100)).await;

    // A fresh request() for the SAME session must NOT be wedged: the insert-path
    // purge evicts the expired zombie and re-parks. We then resolve it → Approved.
    let coord_req2 = Arc::clone(&coord);
    let handle2 = tokio::spawn(async move {
        coord_req2
            .request(
                "sess1",
                "chat1",
                "srv__delete",
                "second",
                &serde_json::json!({}),
            )
            .await
    });
    tokio::task::yield_now().await;

    let found = coord.resolve("sess1", true).await;
    assert!(
        found,
        "HI-01: session must not be wedged by a dropped request() future"
    );
    let outcome = handle2.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "HI-01: the fresh request must resolve to Approved, not stay AlreadyPending"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// MED-02: resolve() must NOT report a false "Approved" for a dead-receiver entry.
//
// Before the fix, resolve() unconditionally returned `true` whenever it removed an
// entry, even if the oneshot receiver was already gone (dropped request() future
// or a request() that already committed to TimedOut). That produced a misleading
// "Approved — running command" reply while nothing ran. The fix returns
// send().is_ok(), so a dead-receiver zombie reports false (fail-closed feedback).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_med02_resolve_dead_receiver_reports_false() {
    // Long timeout so the entry does NOT expire during the test — this isolates the
    // dead-receiver case from the expiry-purge case.
    let (coord, _messages) = make_coordinator(3600);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request("sess1", "chat1", "srv__delete", "x", &serde_json::json!({}))
            .await
    });
    tokio::task::yield_now().await;

    // Drop the request() future — the receiver dies, the entry (non-expired) stays.
    handle.abort();
    let _ = handle.await;

    // resolve(true) must report FALSE: the receiver is dead, so no command can run.
    let found = coord.resolve("sess1", true).await;
    assert!(
        !found,
        "MED-02: resolving a dead-receiver entry must return false (no misleading 'Approved')"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// MED-02 (part 2): after a request() times out, a racing /approve must return
// false (the entry is evicted BEFORE the expiry send await), so the operator sees
// a consistent "no pending approval" rather than a contradictory "Approved".
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_coordinator_med02_resolve_after_timeout_is_consistent() {
    tokio::time::pause();

    let (coord, _messages) = make_coordinator(5);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request("sess1", "chat1", "srv__delete", "x", &serde_json::json!({}))
            .await
    });
    tokio::task::yield_now().await;

    tokio::time::advance(Duration::from_secs(6)).await;
    tokio::task::yield_now().await;

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(outcome, ApprovalOutcome::TimedOut);

    // The timed-out request evicted its own entry; a late /approve must agree.
    let found = coord.resolve("sess1", true).await;
    assert!(
        !found,
        "MED-02: /approve after timeout must return false (consistent with TimedOut)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// BL-01 / BL-02: the per-turn GatewayApprovalGate parks under the session id
// passed to request_approval, and the /approve path (coordinator.resolve) must
// resolve under the SAME id. This encodes the unified-session-key invariant — the
// BL-02 bug had the terminal intercept keying on chat_id while /approve used the
// canonical session UUID, so shell approvals never resolved.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn gateway_gate_and_resolve_share_the_canonical_session_key() {
    use super::GatewayApprovalGate;
    use ironhermes_core::ApprovalGate;

    let (coord, _messages) = make_coordinator(120);
    let coord = Arc::new(coord);

    // The gate binds a chat_id (for prompt delivery) but keys the pending entry on
    // the session_id argument — the canonical session UUID, NOT the chat_id.
    let gate = GatewayApprovalGate::new(Arc::clone(&coord), "chat-999".to_string());
    let canonical = "canonical-session-uuid".to_string();

    let canonical_req = canonical.clone();
    let handle = tokio::spawn(async move {
        gate.request_approval(
            &canonical_req,
            "srv__delete",
            "reason",
            &serde_json::json!({}),
        )
        .await
    });
    tokio::task::yield_now().await;

    // Resolve on the SAME canonical id the /approve path uses.
    let found = coord.resolve(&canonical, true).await;
    assert!(
        found,
        "BL-02: resolve on the canonical session id must find the gate's pending entry"
    );
    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "BL-01/BL-02: a NeedsApproval parked via the gate must resolve to Approved via /approve"
    );

    // Cross-key negative: resolving under the chat_id (the OLD buggy key) must NOT
    // find anything — proving the entry is keyed on the canonical id, not chat_id.
    let coord2 = Arc::clone(&coord);
    let gate2 = GatewayApprovalGate::new(Arc::clone(&coord2), "chat-999".to_string());
    let canonical2 = "another-canonical-uuid".to_string();
    let canonical2_req = canonical2.clone();
    let handle2 = tokio::spawn(async move {
        gate2
            .request_approval(
                &canonical2_req,
                "srv__delete",
                "reason",
                &serde_json::json!({}),
            )
            .await
    });
    tokio::task::yield_now().await;
    let wrong_key = coord2.resolve("chat-999", true).await;
    assert!(
        !wrong_key,
        "BL-02: resolving under chat_id must NOT match a pending keyed on the canonical id"
    );
    // Clean up the second parked request.
    let right_key = coord2.resolve(&canonical2, false).await;
    assert!(right_key, "sanity: canonical key resolves the second entry");
    handle2.await.expect("task must not panic");
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 46 D-02: a broken audit sink fail-closes an operator-approved resolution
// to Denied — no destructive op ever runs unrecorded.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn audit_append_failure_downgrades_operator_approved_to_denied() {
    let (adapter, _messages) = MockAdapter::new();
    let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
        "/tmp/test-approval-d02-audit-placeholder.json",
    )));
    let coord = ApprovalCoordinator::new(
        120,
        Arc::new(adapter) as Arc<dyn PlatformAdapter>,
        store,
        make_broken_audit_log(),
    );
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });

    // Yield to let the spawned task insert into pending and park at rx.await.
    tokio::task::yield_now().await;

    let found = coord.resolve("sess1", true).await;
    assert!(found, "resolve() must find and unblock the pending entry");

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Denied,
        "D-02: a broken audit sink must downgrade an operator-approved resolution to \
         Denied — no destructive op may run unrecorded"
    );
}

/// Same invariant for the D-03 bypass path: a pre-seeded `always` approval must
/// still downgrade to `Denied` when the audit sink cannot be written.
#[tokio::test]
async fn audit_append_failure_downgrades_bypass_approved_to_denied() {
    let (adapter, _messages) = MockAdapter::new();
    let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
        "/tmp/test-approval-d02-bypass-audit-placeholder.json",
    )));
    let tool_name = "cloudflare__kv_delete";
    let cmd_key = ApprovalsStore::normalize_command(tool_name);
    store.approve_always(&cmd_key, KeyKind::Command).await;

    let coord = ApprovalCoordinator::new(
        120,
        Arc::new(adapter) as Arc<dyn PlatformAdapter>,
        store,
        make_broken_audit_log(),
    );

    let outcome = coord
        .request("sess1", "chat1", tool_name, "test", &serde_json::json!({}))
        .await;
    assert_eq!(
        outcome,
        ApprovalOutcome::Denied,
        "D-02: a broken audit sink must downgrade a D-03 bypass-approved resolution \
         to Denied — no destructive op may run unrecorded"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Phase 47.6 Plan 06 (P1-2): one ApprovalCoordinator per platform.
//
// A separate mock adapter type (rather than adding a field to `MockAdapter`
// above) so the existing Phase 45 test suite in this file stays byte-for-byte
// untouched — this task's own acceptance criterion requires zero edits to any
// existing test.
// ─────────────────────────────────────────────────────────────────────────────

/// A `MockAdapter` variant that reports a caller-chosen [`Platform`] from
/// `platform()`, instead of `MockAdapter`'s hardcoded `Platform::Telegram`.
struct PlatformMockAdapter {
    platform: Platform,
    messages: Arc<TokioMutex<Vec<String>>>,
}

impl PlatformMockAdapter {
    fn new(platform: Platform) -> (Self, Arc<TokioMutex<Vec<String>>>) {
        let messages = Arc::new(TokioMutex::new(Vec::new()));
        (
            Self {
                platform,
                messages: Arc::clone(&messages),
            },
            messages,
        )
    }
}

#[async_trait]
impl PlatformAdapter for PlatformMockAdapter {
    fn platform(&self) -> Platform {
        self.platform.clone()
    }

    async fn send_message(
        &self,
        _chat_id: &str,
        content: &str,
        _thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.messages.lock().await.push(content.to_owned());
        Ok(MessageResponse {
            message_id: "0".to_string(),
            chat_id: "0".to_string(),
            platform: self.platform.clone(),
        })
    }

    async fn send_message_markdown_v2(
        &self,
        chat_id: &str,
        content: &str,
        thread_id: Option<&str>,
    ) -> Result<MessageResponse> {
        self.send_message(chat_id, content, thread_id).await
    }

    async fn edit_message(&self, _chat_id: &str, _message_id: &str, _content: &str) -> Result<()> {
        Ok(())
    }

    async fn edit_message_markdown_v2(
        &self,
        _chat_id: &str,
        _message_id: &str,
        _content: &str,
    ) -> Result<()> {
        Ok(())
    }

    async fn delete_message(&self, _chat_id: &str, _message_id: &str) -> Result<()> {
        Ok(())
    }

    fn is_running(&self) -> bool {
        true
    }
}

/// Build a coordinator bound to a [`PlatformMockAdapter`] reporting `platform`,
/// backed by an empty in-memory `ApprovalsStore` and a working audit log at a
/// fresh temp path.
fn make_coordinator_with_platform(
    timeout_secs: u64,
    platform: Platform,
) -> (ApprovalCoordinator, Arc<TokioMutex<Vec<String>>>) {
    let (adapter, messages) = PlatformMockAdapter::new(platform);
    let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
        "/tmp/test-approval-platform-placeholder-not-used.json",
    )));
    let coord = ApprovalCoordinator::new(
        timeout_secs,
        Arc::new(adapter) as Arc<dyn PlatformAdapter>,
        store,
        make_working_audit_log(),
    );
    (coord, messages)
}

/// Build a working `AuditLog` backed by a fresh temp directory, ALSO returning
/// the exact path so a test can read the JSONL back and assert on its fields
/// (`make_working_audit_log` above intentionally hides the path — callers that
/// only need the coordinator to accept writes don't need it).
fn make_working_audit_log_with_path() -> (Arc<AuditLog>, PathBuf) {
    let dir = tempfile::tempdir()
        .expect("tempdir for audit log")
        .keep();
    let path = dir.join("audit.jsonl");
    (
        Arc::new(AuditLog::with_path(path.clone(), AuditConfig::default())),
        path,
    )
}

/// Pins the core claim of task 1: two coordinator instances have entirely
/// independent `pending` maps. A pending entry parked in coordinator A is not
/// resolvable through coordinator B, and resolving it in A leaves B untouched.
#[tokio::test]
async fn two_coordinators_have_independent_pending_maps() {
    let (coord_a, _messages_a) = make_coordinator_with_platform(120, Platform::Telegram);
    let (coord_b, _messages_b) = make_coordinator_with_platform(120, Platform::Buzz);
    let coord_a = Arc::new(coord_a);
    let coord_b = Arc::new(coord_b);

    let coord_a_req = Arc::clone(&coord_a);
    let handle_a = tokio::spawn(async move {
        coord_a_req
            .request("sess1", "chat1", "srv__delete", "test", &serde_json::json!({}))
            .await
    });
    tokio::task::yield_now().await;

    // Session "sess1" parked in A must not be resolvable via B — proving the
    // two maps are entirely independent, not a shared/keyed-by-platform map.
    let found_via_b = coord_b.resolve("sess1", true).await;
    assert!(
        !found_via_b,
        "coordinator B must not resolve an entry parked in coordinator A"
    );

    // Resolving via A finds its own entry.
    let found_via_a = coord_a.resolve("sess1", true).await;
    assert!(found_via_a, "coordinator A must resolve its own pending entry");

    let outcome = handle_a.await.expect("task must not panic");
    assert_eq!(outcome, ApprovalOutcome::Approved);

    // B still reports nothing pending for the same session id — proving its
    // map was never mutated by A's insert or A's resolve.
    let still_nothing_in_b = coord_b.resolve("sess1", true).await;
    assert!(
        !still_nothing_in_b,
        "coordinator B's map must remain independent of coordinator A"
    );
}

/// A coordinator bound to an adapter reporting the buzz platform produces
/// audit entries whose `surface` field names buzz; one bound to a
/// telegram-reporting adapter names telegram. Proves the audit surface truly
/// follows the BOUND adapter, not a hardcoded string.
#[tokio::test]
async fn coordinator_audit_surface_follows_its_bound_adapter() {
    for (platform, expected_surface) in [(Platform::Buzz, "buzz"), (Platform::Telegram, "telegram")]
    {
        let (adapter, _messages) = PlatformMockAdapter::new(platform);
        let (audit_log, audit_path) = make_working_audit_log_with_path();
        let store = Arc::new(ApprovalsStore::with_path(PathBuf::from(
            "/tmp/test-approval-surface-placeholder.json",
        )));
        let coord = ApprovalCoordinator::new(
            120,
            Arc::new(adapter) as Arc<dyn PlatformAdapter>,
            store,
            audit_log,
        );
        let coord = Arc::new(coord);

        let coord_req = Arc::clone(&coord);
        let handle = tokio::spawn(async move {
            coord_req
                .request("sess1", "chat1", "srv__delete", "test", &serde_json::json!({}))
                .await
        });
        tokio::task::yield_now().await;
        coord.resolve("sess1", true).await;
        handle.await.expect("task must not panic");

        let contents = std::fs::read_to_string(&audit_path).expect("read audit log");
        let last_line = contents.lines().last().expect("at least one audit line");
        let entry: serde_json::Value =
            serde_json::from_str(last_line).expect("audit line must be valid JSON");
        assert_eq!(
            entry["surface"], expected_surface,
            "audit surface must follow the coordinator's bound adapter's platform()"
        );
    }
}

/// The prompt is delivered through the coordinator's OWN bound adapter and no
/// other — a second coordinator's adapter never sees a prompt it did not
/// generate.
#[tokio::test]
async fn coordinator_prompt_goes_to_its_own_adapter() {
    let (coord_a, messages_a) = make_coordinator_with_platform(120, Platform::Telegram);
    let (coord_b, messages_b) = make_coordinator_with_platform(120, Platform::Buzz);
    let coord_a = Arc::new(coord_a);

    let coord_a_req = Arc::clone(&coord_a);
    let handle = tokio::spawn(async move {
        coord_a_req
            .request("sess1", "chat1", "srv__delete", "test", &serde_json::json!({}))
            .await
    });
    tokio::task::yield_now().await;

    {
        let msgs_a = messages_a.lock().await;
        assert!(
            !msgs_a.is_empty(),
            "coordinator A's own adapter must receive its prompt"
        );
    }
    {
        let msgs_b = messages_b.lock().await;
        assert!(
            msgs_b.is_empty(),
            "coordinator B's adapter must never receive A's prompt"
        );
    }

    coord_a.resolve("sess1", true).await;
    handle.await.expect("task must not panic");
    drop(coord_b);
}

/// Sanity check that the pre-Plan-06 single-coordinator flow (the whole
/// Phase 45 suite above, exercised via the SAME `make_coordinator` helper)
/// still passes alongside the new multi-coordinator test code added by this
/// task — nothing about `ApprovalCoordinator`'s shape changed, only how many
/// instances `runner.rs` constructs.
#[tokio::test]
async fn existing_phase_45_invariants_still_hold() {
    let (coord, _messages) = make_coordinator(120);
    let coord = Arc::new(coord);

    let coord_req = Arc::clone(&coord);
    let handle = tokio::spawn(async move {
        coord_req
            .request(
                "sess1",
                "chat1",
                "cloudflare__kv_delete",
                "test",
                &serde_json::json!({}),
            )
            .await
    });
    tokio::task::yield_now().await;

    let found = coord.resolve("sess1", true).await;
    assert!(found, "resolve() must find and unblock the pending entry");

    let outcome = handle.await.expect("task must not panic");
    assert_eq!(
        outcome,
        ApprovalOutcome::Approved,
        "single-coordinator flow must be unaffected by Plan 06's per-platform additions"
    );
}
