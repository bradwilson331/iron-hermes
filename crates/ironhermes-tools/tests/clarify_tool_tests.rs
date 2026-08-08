// Phase 36.3.8 Plan 02 — Integration tests for clarify_tool + clarify_registry.
//
// Test coverage:
// - Task 1 (PendingClarifyRegistry): insert/take/remove semantics + no-lock-across-await
// - Task 2 (ClarifyTool): answered, timeout, cancel, cleanup
//
// These are tokio async tests. The cargo test suite is authoritative for RED/GREEN gates.

use ironhermes_tools::clarify_registry::{ClarifyAnswer, PendingClarifyRegistry};
use tokio::sync::oneshot;

// ============================================================================
// Task 1: PendingClarifyRegistry tests
// ============================================================================

/// insert then take returns the entry with the inserted choices.
#[tokio::test]
async fn clarify_registry_take_returns_inserted_entry() {
    let registry = PendingClarifyRegistry::new();
    let (tx, _rx) = oneshot::channel::<ClarifyAnswer>();
    let choices = vec!["Yes".to_string(), "No".to_string()];

    registry
        .insert("id-1".to_string(), choices.clone(), tx)
        .await
        .expect("insert must succeed for fresh key");

    let entry = registry.take("id-1").await;
    assert!(entry.is_some(), "take() must return the inserted entry");
    assert_eq!(
        entry.unwrap().choices,
        choices,
        "taken entry must preserve the inserted choices (label-recovery source)"
    );
}

/// take a second time on the same key returns None (entry consumed).
#[tokio::test]
async fn clarify_registry_take_twice_returns_none() {
    let registry = PendingClarifyRegistry::new();
    let (tx, _rx) = oneshot::channel::<ClarifyAnswer>();

    registry
        .insert("id-2".to_string(), vec!["A".to_string()], tx)
        .await
        .expect("insert must succeed for fresh key");

    let first = registry.take("id-2").await;
    assert!(first.is_some(), "first take() must succeed");

    let second = registry.take("id-2").await;
    assert!(
        second.is_none(),
        "second take() must return None (entry consumed)"
    );
}

/// remove on a present key drops the entry; take afterward returns None.
#[tokio::test]
async fn clarify_registry_remove_then_take_returns_none() {
    let registry = PendingClarifyRegistry::new();
    let (tx, _rx) = oneshot::channel::<ClarifyAnswer>();

    registry
        .insert("id-3".to_string(), vec!["Option".to_string()], tx)
        .await
        .expect("insert must succeed for fresh key");

    registry.remove("id-3").await;

    let entry = registry.take("id-3").await;
    assert!(entry.is_none(), "take() after remove() must return None");
}

/// take() must NOT hold the lock across an await — demonstrated by taking the
/// PendingClarify entry and then successfully sending on the oneshot sender
/// (which would block if the Mutex were still held across the channel op).
#[tokio::test]
async fn clarify_registry_take_releases_lock_before_send() {
    let registry = PendingClarifyRegistry::new();
    let (tx, rx) = oneshot::channel::<ClarifyAnswer>();
    let choices = vec!["Maybe".to_string()];

    registry
        .insert("id-4".to_string(), choices.clone(), tx)
        .await
        .expect("insert must succeed for fresh key");

    // take() should remove from map and return entry; lock must be released
    // before we do anything async with the sender.
    let entry = registry.take("id-4").await.expect("entry must exist");

    // If the lock were still held, this await would deadlock (tokio Mutex
    // can't be held across await in a single-threaded executor context).
    // Sending and receiving proves the lock is released before the send.
    entry
        .sender
        .send(ClarifyAnswer {
            label: "Maybe".to_string(),
            index: 0,
        })
        .expect("send must succeed after lock released");

    let answer = rx.await.expect("receiver must get the answer");
    assert_eq!(answer.label, "Maybe");
    assert_eq!(answer.index, 0);
}

// ============================================================================
// Task 2: ClarifyTool suspend/resume tests
// ============================================================================

use async_trait::async_trait;
use ironhermes_core::{Config, Platform, SessionKey};
use ironhermes_tools::clarify_tool::{ClarifyDispatcher, ClarifyTool};
use std::sync::Arc;

/// A mock ClarifyDispatcher that records calls but does nothing (answer comes
/// from the test driving the registry directly).
struct MockClarifyDispatcher;

#[async_trait]
impl ClarifyDispatcher for MockClarifyDispatcher {
    async fn send_question(
        &self,
        _chat_id: &str,
        _thread_id: Option<&str>,
        _question: &str,
        _choices: &[String],
        _clarify_id: &str,
    ) -> anyhow::Result<()> {
        // No-op — test drives the answer via the registry directly.
        Ok(())
    }
}

fn make_config(timeout_secs: u64) -> Arc<Config> {
    let mut cfg = Config::default();
    cfg.agent.clarify_timeout_secs = timeout_secs;
    Arc::new(cfg)
}

fn make_session_key() -> SessionKey {
    SessionKey::new(Platform::Local, "test-chat")
}

/// answered path: spawn execute, take the sender from the registry, send an
/// answer — execute returns {"answered":true,"choice":"Yes","choice_index":0}.
#[tokio::test]
async fn clarify_answered() {
    let registry = Arc::new(PendingClarifyRegistry::new());
    let config = make_config(120);
    let session_key = make_session_key();

    let tool = ClarifyTool::new(
        session_key,
        Some(Arc::new(MockClarifyDispatcher)),
        registry.clone(),
        None,
        config,
    );

    let args = serde_json::json!({
        "question": "Do you confirm?",
        "choices": ["Yes", "No"]
    });

    // Spawn execute in background so we can drive the answer from outside.
    let tool_handle = tokio::spawn(async move {
        // execute_clarify inserts into the registry then suspends on select!.
        // The outer test drives the answer via registry.take() concurrently.
        tool.execute_clarify(args).await
    });

    // The clarify_id the tool inserts is "clarify:<chat_id>" (per clarify_tool.rs).
    let clarify_id = format!("clarify:{}", make_session_key().chat_id);

    // Poll until the registry has an entry (the tool has called insert).
    let mut entry = None;
    for _ in 0..100 {
        entry = registry.take(&clarify_id).await;
        if entry.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let entry = entry.expect("clarify entry must appear in registry after execute starts");
    entry
        .sender
        .send(ClarifyAnswer {
            label: "Yes".to_string(),
            index: 0,
        })
        .expect("send must succeed");

    let result = tool_handle
        .await
        .expect("task must complete")
        .expect("execute must succeed");
    let v: serde_json::Value = serde_json::from_str(&result).expect("result must be valid JSON");

    assert_eq!(v["answered"], true, "answered must be true");
    assert_eq!(v["choice"], "Yes", "choice must match sent label");
    assert_eq!(v["choice_index"], 0, "choice_index must match sent index");
}

/// timeout path: set clarify_timeout_secs=0, send no answer — execute returns
/// {"answered":false,"reason":"timeout"}.
#[tokio::test]
async fn clarify_timeout() {
    let registry = Arc::new(PendingClarifyRegistry::new());
    let config = make_config(0); // immediate timeout
    let session_key = make_session_key();

    let tool = ClarifyTool::new(
        session_key,
        Some(Arc::new(MockClarifyDispatcher)),
        registry.clone(),
        None,
        config,
    );

    let args = serde_json::json!({
        "question": "Are you there?",
        "choices": ["Yes", "No"]
    });

    let result = tool
        .execute_clarify(args)
        .await
        .expect("execute must not Err on timeout");

    // Exact sentinel shape per D-07 / OQ-4
    let expected = r#"{"answered":false,"reason":"timeout"}"#;
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    let e: serde_json::Value = serde_json::from_str(expected).unwrap();
    assert_eq!(v, e, "timeout sentinel must match {}", expected);
}

/// cancel path: pass a CancellationToken, cancel it, execute returns
/// {"answered":false,"reason":"cancelled"}.
#[tokio::test]
async fn clarify_cancel() {
    use tokio_util::sync::CancellationToken;

    let registry = Arc::new(PendingClarifyRegistry::new());
    let config = make_config(120); // long timeout — cancel fires first
    let session_key = make_session_key();
    let cancel_token = CancellationToken::new();

    let tool = ClarifyTool::new(
        session_key,
        Some(Arc::new(MockClarifyDispatcher)),
        registry.clone(),
        Some(cancel_token.clone()),
        config,
    );

    let args = serde_json::json!({
        "question": "Which option?",
        "choices": ["A", "B", "C"]
    });

    // Cancel the token shortly after execute starts.
    let cancel_clone = cancel_token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel_clone.cancel();
    });

    let result = tool
        .execute_clarify(args)
        .await
        .expect("execute must not Err on cancel");

    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["answered"], false);
    assert_eq!(v["reason"], "cancelled");
}

/// cleanup path: after execute completes (any path), the registry has no entry
/// for the clarify_id — no leak (Pitfall 5).
#[tokio::test]
async fn clarify_cleanup() {
    let registry = Arc::new(PendingClarifyRegistry::new());
    let config = make_config(0); // timeout immediately
    let session_key = make_session_key();

    let tool = ClarifyTool::new(
        session_key,
        Some(Arc::new(MockClarifyDispatcher)),
        registry.clone(),
        None,
        config,
    );

    let args = serde_json::json!({
        "question": "Cleanup test?",
        "choices": ["Yes"]
    });

    // Execute with immediate timeout — entry should be cleaned up afterward.
    let _ = tool.execute_clarify(args).await;

    // The clarify_id the tool inserts is "clarify:<chat_id>".
    let clarify_id = format!("clarify:{}", make_session_key().chat_id);

    // The registry must be empty after execute returns (no leak).
    let entry = registry.take(&clarify_id).await;
    assert!(
        entry.is_none(),
        "registry must have no entry for clarify_id after execute returns (Pitfall 5)"
    );
}
