//! Phase 36 Wave-0 scaffold — GW-05 running-agent guard integration tests.
//!
//! This file contains all 11 sub-behavior stubs for GW-05 as defined in
//! 36-VALIDATION.md. All tests are marked `#[ignore]` so the harness compiles
//! and reports 11 ignored / 0 failed / 0 passed. Plan 36-02 implements the
//! production behavior; Plan 36-03 removes the `#[ignore]` attributes once
//! the guard is wired.
//!
//! ## GW-05 sub-behaviors covered
//!
//! 1. `test_session_isolation` — per-session state: session A Running does not block session B (Idle)
//! 2. `test_model_rejected_when_running` — `/model` rejected during active turn (D-04, D-02)
//! 3. `test_stop_bypasses_guard` — `/stop` dispatches even when flag is true (D-01)
//! 4. `test_new_bypasses_guard` — `/new` dispatches even when flag is true (D-01)
//! 5. `test_status_bypasses_guard` — `/status` dispatches even when flag is true (D-01)
//! 6. `test_queue_bypasses_guard` — `/queue` dispatches even when flag is true (D-01)
//! 7. `test_guard_clears_on_success` — flag clears on `run_agent` returning `Ok(...)` (D-06)
//! 8. `test_guard_clears_on_error` — flag clears on `run_agent` returning `Err(...)` (D-06)
//! 9. `test_alias_bypasses_guard` — `/reset` (alias for `new`) bypasses guard (D-01)
//! 10. `test_freetext_rejected_when_running` — non-slash free-text rejected during active turn (Pitfall 1)
//! 11. `test_stop_reads_real_flag` — `cmd_stop` reads a non-false `agent_running` on gateway (integration)

pub mod helpers {
    use std::sync::Arc;
    use tokio::sync::{Mutex as TokioMutex, RwLock};

    use anyhow::Result;
    use async_trait::async_trait;
    use ironhermes_core::{Config, MessageResponse, Platform, ProviderResolver};
    use ironhermes_gateway::{
        handler::GatewayMessageHandler,
        session::{SessionKey, SessionStore},
    };
    use ironhermes_tools::ToolRegistry;

    /// Build an in-memory `SessionStore` wrapped in `Arc<RwLock<...>>`, mirroring
    /// the pattern at `handler.rs:1261`. Callers may inject sessions and flip flags
    /// directly through the returned handle.
    pub fn build_test_session_store() -> Arc<RwLock<SessionStore>> {
        let state_store = Arc::new(std::sync::Mutex::new(
            ironhermes_state::StateStore::new(":memory:").expect("in-memory StateStore"),
        ));
        Arc::new(RwLock::new(SessionStore::new(state_store)))
    }

    /// A `PlatformAdapter` that records every `send_message` call as a
    /// `(chat_id, text)` tuple so tests can assert on sent messages without
    /// a live Telegram connection.
    ///
    /// All other trait methods are no-ops that return `Ok(Default::default())`.
    pub struct RecordingPlatformAdapter {
        log: TokioMutex<Vec<(String, String)>>,
    }

    impl RecordingPlatformAdapter {
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                log: TokioMutex::new(Vec::new()),
            })
        }

        /// Return a snapshot of all recorded `(chat_id, text)` pairs.
        pub async fn messages(&self) -> Vec<(String, String)> {
            self.log.lock().await.clone()
        }
    }

    #[async_trait]
    impl ironhermes_gateway::adapter::PlatformAdapter for RecordingPlatformAdapter {
        fn platform(&self) -> Platform {
            Platform::Telegram
        }

        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            _thread_id: Option<&str>,
        ) -> Result<MessageResponse> {
            self.log
                .lock()
                .await
                .push((chat_id.to_string(), content.to_string()));
            Ok(MessageResponse {
                message_id: "stub-msg-id".to_string(),
                chat_id: chat_id.to_string(),
                platform: Platform::Telegram,
            })
        }

        async fn edit_message(
            &self,
            _chat_id: &str,
            _message_id: &str,
            _content: &str,
        ) -> Result<()> {
            Ok(())
        }

        async fn edit_message_markdown(
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

    /// Build a `GatewayMessageHandler` for testing, replicating the construction
    /// logic from `handler.rs:1226` (`make_handler()` is `pub(crate)` and not
    /// reachable from integration tests — so we replicate it here).
    ///
    /// Source pattern: `crates/ironhermes-gateway/src/handler.rs:1226`
    pub fn build_test_handler(store: Arc<RwLock<SessionStore>>) -> GatewayMessageHandler {
        let config = Config::default();
        let resolver = ProviderResolver::build(&config).unwrap();
        let tool_registry = Arc::new(RwLock::new(ToolRegistry::new()));
        GatewayMessageHandler::new(config, resolver, store, tool_registry)
    }

    /// Construct a canonical test `SessionKey` for `chat_id` with a fixed user `u1`.
    pub fn test_session_key(chat_id: &str) -> SessionKey {
        SessionKey::new(Platform::Telegram, chat_id).with_user("u1")
    }

    /// The locked D-02 error message. Plan 02's emitted string MUST equal this
    /// byte-for-byte; Plan 03's assertions reference this helper.
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }
}

// ---------------------------------------------------------------------------
// GW-05 sub-behavior stubs (all #[ignore]'d — Wave 0 scaffold)
// ---------------------------------------------------------------------------

/// GW-05-1: Per-session isolation.
///
/// Session A is set to Running (`running` flag = true). Session B remains Idle.
/// Dispatching `/model` to session B must succeed (not be rejected with D-02).
///
/// Verifies that the guard uses per-session state, not a global flag
/// (codex HIGH-2 TOCTOU concern). Two `SessionKey`s, two distinct `Arc<AtomicBool>`s.
///
/// Plan 36-02 must:
/// - Add `running: Arc<AtomicBool>` to `GatewaySession` (Option B).
/// - Make `handle_slash_command` read the flag from the session, not create a new `false` one.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_session_isolation() {
    // TODO(Plan 36-02): GW-05-1
    //   let store = helpers::build_test_session_store();
    //   let adapter = helpers::RecordingPlatformAdapter::new();
    //   let key_a = helpers::test_session_key("chat-A");
    //   let key_b = helpers::test_session_key("chat-B");
    //   // Get-or-create both sessions so the store has entries
    //   { let mut s = store.write().await; s.get_or_create(key_a.clone(), "model", "test"); s.get_or_create(key_b.clone(), "model", "test"); }
    //   // Set session A running
    //   { let s = store.read().await; s.get(&key_a).unwrap().running.store(true, Ordering::SeqCst); }
    //   // Dispatch /model to session B — must succeed (no D-02 rejection)
    //   // ... assert adapter.messages() is empty (no rejection sent) ...
    todo!("Plan 36-02: GW-05-1 — session A Running must not block session B Idle; per-session Arc<AtomicBool> required")
}

/// GW-05-2: `/model` rejected when agent is running.
///
/// Session flag is set to `true` (running). Sending `/model gpt-4` must trigger
/// the D-02 rejection path in `handle_slash_command`.
///
/// D-04: `/model` during active turn is rejected. Closes codex HIGH-2 TOCTOU.
/// D-02: rejection message is `helpers::d02_error_message()`.
///
/// Plan 36-02 must:
/// - After `router.resolve()` returns `Exact(def)` for `model`, check the session flag.
/// - If running and def.name is not in the bypass list, send D-02 message and return Ok(()).
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_model_rejected_when_running() {
    // TODO(Plan 36-02): GW-05-2
    //   let store = helpers::build_test_session_store();
    //   let adapter = helpers::RecordingPlatformAdapter::new();
    //   let key = helpers::test_session_key("chat-1");
    //   { let mut s = store.write().await; s.get_or_create(key.clone(), "model", "test"); }
    //   // Set running = true
    //   { let s = store.read().await; s.get(&key).unwrap().running.store(true, Ordering::SeqCst); }
    //   // Send /model event → handle_slash_command
    //   // ... assert adapter.messages()[0].1 == helpers::d02_error_message() ...
    todo!("Plan 36-02: GW-05-2 — /model must be rejected (D-04/D-02) when session flag is true")
}

/// GW-05-3: `/stop` bypasses the guard when agent is running.
///
/// Session flag is `true`. Sending `/stop` must dispatch (not be rejected with D-02).
/// The bypass list (D-01) includes `"stop"`.
///
/// Plan 36-02 must:
/// - Implement `is_bypass(name: &str) -> bool` that returns true for `"stop"`.
/// - Guard check: if running AND is_bypass(def.name) → dispatch normally.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_stop_bypasses_guard() {
    // TODO(Plan 36-02): GW-05-3
    //   // Set running = true, dispatch /stop
    //   // Assert: NO D-02 message sent; /stop handler output (if any) in messages instead
    todo!("Plan 36-02: GW-05-3 — /stop must bypass guard (D-01); is_bypass(\"stop\") == true")
}

/// GW-05-4: `/new` bypasses the guard when agent is running.
///
/// Session flag is `true`. Sending `/new` must dispatch (not be rejected with D-02).
/// The bypass list (D-01) includes `"new"`.
///
/// Plan 36-02 must:
/// - `is_bypass("new")` returns true.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_new_bypasses_guard() {
    // TODO(Plan 36-02): GW-05-4
    //   // Set running = true, dispatch /new
    //   // Assert: guard does not reject; /new handler runs
    todo!("Plan 36-02: GW-05-4 — /new must bypass guard (D-01); is_bypass(\"new\") == true")
}

/// GW-05-5: `/status` bypasses the guard when agent is running.
///
/// Session flag is `true`. Sending `/status` must dispatch (not be rejected with D-02).
/// The bypass list (D-01) includes `"status"`.
///
/// Plan 36-02 must:
/// - `is_bypass("status")` returns true.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_status_bypasses_guard() {
    // TODO(Plan 36-02): GW-05-5
    //   // Set running = true, dispatch /status
    //   // Assert: guard does not reject; /status handler runs
    todo!("Plan 36-02: GW-05-5 — /status must bypass guard (D-01); is_bypass(\"status\") == true")
}

/// GW-05-6: `/queue` bypasses the guard when agent is running.
///
/// Session flag is `true`. Sending `/queue` must dispatch (not be rejected with D-02).
/// The bypass list (D-01) includes `"queue"` for hermes-agent parity (inert stub today,
/// but bypassed so the user can run it during a turn).
///
/// Plan 36-02 must:
/// - `is_bypass("queue")` returns true.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_queue_bypasses_guard() {
    // TODO(Plan 36-02): GW-05-6
    //   // Set running = true, dispatch /queue
    //   // Assert: guard does not reject; /queue handler runs (TODO stub response ok)
    todo!("Plan 36-02: GW-05-6 — /queue must bypass guard (D-01); is_bypass(\"queue\") == true")
}

/// GW-05-7: Running flag clears on agent success.
///
/// `RunningAgentGuard` (D-06) is constructed at the top of `run_agent`.
/// After `run_agent` returns `Ok(...)`, the flag must be `false`.
///
/// D-06: RAII guard — `Drop` clears the flag on every exit path.
///
/// Plan 36-02 must:
/// - Define `RunningAgentGuard(Arc<AtomicBool>)` with `Drop` impl that stores false.
/// - Construct it at the top of `run_agent`; verify flag is false after `run_agent` returns.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_guard_clears_on_success() {
    // TODO(Plan 36-02): GW-05-7
    //   let flag = Arc::new(AtomicBool::new(false));
    //   { let _guard = RunningAgentGuard::new(flag.clone()); assert!(flag.load(SeqCst)); }
    //   assert!(!flag.load(SeqCst), "Drop must clear to false on success exit");
    todo!("Plan 36-02: GW-05-7 — RunningAgentGuard Drop must clear flag on Ok exit (D-06)")
}

/// GW-05-8: Running flag clears on agent error.
///
/// `RunningAgentGuard` must fire `Drop` even when `run_agent` propagates an error
/// via `?`. After `run_agent` returns `Err(...)`, the flag must be `false`.
///
/// D-06: RAII covers success, error, and `?` early-return paths. Rust has no `finally`.
///
/// Plan 36-02 must:
/// - Prove Drop fires on `?` / `Err` path (same `RunningAgentGuard` struct as GW-05-7).
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_guard_clears_on_error() {
    // TODO(Plan 36-02): GW-05-8
    //   let flag = Arc::new(AtomicBool::new(false));
    //   let result: Result<()> = {
    //       let _guard = RunningAgentGuard::new(flag.clone());
    //       Err(anyhow::anyhow!("simulated error"))
    //   };
    //   assert!(result.is_err());
    //   assert!(!flag.load(SeqCst), "Drop must clear to false on Err exit");
    todo!("Plan 36-02: GW-05-8 — RunningAgentGuard Drop must clear flag on Err/? exit (D-06)")
}

/// GW-05-9: Alias `/reset` (resolves to canonical `"new"`) bypasses the guard.
///
/// `CommandRouter::resolve("/reset")` returns `Exact(def)` where `def.name == "new"`.
/// The guard checks `resolved_def.name`, not the raw input string, so `/reset` must
/// correctly bypass because `is_bypass("new") == true`.
///
/// Anti-pattern (Pitfall 4): checking raw `"reset"` against the bypass list would
/// fail to bypass because `"reset"` is not in the list.
///
/// Plan 36-02 must:
/// - Confirm guard check uses `resolved_def.name` after `router.resolve()`.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_alias_bypasses_guard() {
    // TODO(Plan 36-02): GW-05-9
    //   // Set running = true, dispatch /reset (alias for new)
    //   // Assert: guard does not reject; handler for "new" runs
    todo!("Plan 36-02: GW-05-9 — /reset alias must bypass guard (checks resolved def.name == \"new\", not raw input)")
}

/// GW-05-10: Non-slash free-text message rejected when agent is running.
///
/// A plain-text (non-`/`) message dispatches through `MessageHandler::handle`
/// to `run_agent` directly. This path must ALSO check the session flag and reject
/// with the D-02 message when running.
///
/// Pitfall 1: guard only in `handle_slash_command` leaves the non-slash path
/// unguarded — user can bypass by omitting `/`.
///
/// Plan 36-02 must:
/// - Add the flag check in `MessageHandler::handle` before calling `run_agent`
///   for non-slash messages.
/// - Reject with the same `helpers::d02_error_message()` string.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_freetext_rejected_when_running() {
    // TODO(Plan 36-02): GW-05-10
    //   // Set running = true
    //   // Dispatch a plain-text MessageEvent (no leading /)
    //   // Assert: adapter.messages()[0].1 == helpers::d02_error_message()
    //   // Assert: run_agent was NOT called (flag still true — guard short-circuited)
    todo!("Plan 36-02: GW-05-10 — free-text during active turn must be rejected with D-02 (Pitfall 1 guard)")
}

/// GW-05-11: `cmd_stop` reads a non-false `agent_running` on gateway.
///
/// After Phase 36 implementation, `cmd_stop` in `ironhermes-core/src/commands/handlers.rs:127`
/// reads `ctx.agent_running.load(Ordering::SeqCst)`. This integration test confirms
/// that when the gateway wires the REAL per-session `Arc<AtomicBool>` into
/// `CommandContext.agent_running`, the `/stop` handler observes `true` when the
/// session is running (instead of always seeing the hardcoded-false shim).
///
/// Pre-Phase-36 behavior (the bug): handler.rs:377-380 creates a NEW `AtomicBool`
/// set to `false` each time, so `cmd_stop`'s read always returns false.
///
/// Plan 36-02 must:
/// - Populate `CommandContext.agent_running` with the per-session flag from `GatewaySession.running`.
/// - Assert that `/stop` handler receives `true` when the session flag is true.
#[tokio::test]
#[ignore = "Wave 0 scaffold — implementation lands in Plan 36-02"]
async fn test_stop_reads_real_flag() {
    // TODO(Plan 36-02): GW-05-11
    //   // Set running = true on session
    //   // Build CommandContext for that session (via handle_slash_command path)
    //   // Assert ctx.agent_running.load(SeqCst) == true
    //   // (i.e. the plan's CommandContext received the real per-session Arc, not a new false one)
    todo!("Plan 36-02: GW-05-11 — CommandContext.agent_running must be the real per-session handle, not a fresh false AtomicBool")
}
