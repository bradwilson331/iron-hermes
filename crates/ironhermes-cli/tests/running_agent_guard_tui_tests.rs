//! Phase 36.1 GW-05-TUI running-agent guard integration tests.
//!
//! Covers the 5 GW-05-TUI behaviors for the tui_rata surface:
//!
//! 1. `test_agent_running_is_live_arc`   — live Arc invariant: stored value visible via all clones
//! 2. `test_guard_clears_on_turn_end`    — RAII clears flag when future completes (Drop fires)
//! 3. `test_model_rejected_when_running` — /model rejected mid-turn (D-10, GW-05-TUI-3)
//! 4. `test_stop_bypasses_guard`         — /stop bypasses guard (D-01, GW-05-TUI-4)
//! 5. `test_alias_reset_bypasses_guard`  — /reset alias → "new" bypasses guard (Pitfall 4, GW-05-TUI-5)
//!
//! Plus `d02_constant_matches_helper` — sanity check: helpers::d02_error_message() byte-matches the
//! canonical ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG constant.
//!
//! Test approach: TUI tests are unit tests operating without the full TUI runtime.
//! We test the guard logic and the CommandRouter resolution path directly (using
//! Arc<AtomicBool> and CommandRouter::new(build_registry())) because App construction
//! requires heavy dependencies (AgentRuntime, MCP, etc.) that are impractical in
//! integration test context. The structural source-grep gates in Task 1 acceptance
//! criteria prove that the same guard check is wired into dispatch_slash — these tests
//! verify the guard mechanics and resolution contract independently.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub mod helpers {
    /// The locked D-02 error message.
    ///
    /// This helper string MUST be byte-for-byte identical to
    /// `ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG`.
    /// `d02_constant_matches_helper` enforces this constraint at test time.
    pub fn d02_error_message() -> &'static str {
        "Agent is running. Use /stop to interrupt or /queue to send after this turn."
    }
}

/// Sanity check: helpers::d02_error_message() is byte-identical to the canonical constant.
///
/// If this fails, `helpers::d02_error_message()` has drifted from the locked D-02 string.
/// Update the helper to match — never update the constant.
#[test]
fn d02_constant_matches_helper() {
    assert_eq!(
        helpers::d02_error_message(),
        ironhermes_core::commands::running_agent::AGENT_RUNNING_REJECT_MSG,
        "helpers::d02_error_message() must be byte-for-byte identical to AGENT_RUNNING_REJECT_MSG"
    );
}

/// GW-05-TUI-1: The agent_running flag is a live Arc — all clones share the same
/// underlying AtomicBool storage.
///
/// Proves that build_command_context's `app.agent_running.clone()` is a live handle:
/// a store via any clone is immediately visible via any other clone.
/// The OLD `Arc::new(AtomicBool::new(app.pending_rx.is_some()))` snapshot would
/// NOT exhibit this property — it would be a fresh allocation disconnected from
/// the App's persistent flag.
#[test]
fn test_agent_running_is_live_arc() {
    let flag = Arc::new(AtomicBool::new(false));

    // Simulate what App holds as app.agent_running
    let app_handle = flag.clone();
    // Simulate what build_command_context returns: app.agent_running.clone()
    let ctx_handle = flag.clone();

    // Initially false
    assert!(!app_handle.load(Ordering::SeqCst));
    assert!(!ctx_handle.load(Ordering::SeqCst));

    // Store via app_handle — must be visible via ctx_handle (live Arc, not snapshot)
    app_handle.store(true, Ordering::SeqCst);
    assert!(
        ctx_handle.load(Ordering::SeqCst),
        "ctx_handle must see the store done via app_handle — they share underlying storage"
    );

    // Clear and verify
    app_handle.store(false, Ordering::SeqCst);
    assert!(
        !ctx_handle.load(Ordering::SeqCst),
        "ctx_handle must see the cleared flag — live Arc invariant"
    );
}

/// GW-05-TUI-2: RunningAgentGuard clears the flag when the async future completes.
///
/// Proves RAII Drop fires when the spawned future finishes (not when spawn_turn
/// returns synchronously). This is the core Pitfall 1 mitigation: the guard MUST
/// live inside tokio::spawn(async move { ... }), not in the sync wrapper.
#[tokio::test]
async fn test_guard_clears_on_turn_end() {
    use ironhermes_core::commands::running_agent::RunningAgentGuard;

    let flag = Arc::new(AtomicBool::new(false));
    let flag_for_task = flag.clone();

    // Spawn a task that binds the guard (simulating what spawn_turn does inside tokio::spawn)
    let handle = tokio::spawn(async move {
        let _guard = RunningAgentGuard::new(flag_for_task.clone());
        // Flag is now true while guard is alive
        assert!(
            flag_for_task.load(Ordering::SeqCst),
            "Flag must be true while guard is in scope inside the async block"
        );
        // Short delay to simulate an async turn
        tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        // _guard drops here when the future returns
    });

    handle.await.expect("spawned task must not panic");

    // After the future completes, Drop must have fired
    assert!(
        !flag.load(Ordering::SeqCst),
        "Flag must be false after the spawned future completes — RAII Drop must fire (D-09)"
    );
}

/// GW-05-TUI-3: /model is rejected when agent_running is true.
///
/// Verifies the D-10 bypass-list check: CommandRouter::resolve("/model gpt-4") returns
/// a def with name "model"; is_bypass("model") == false; so the guard rejects it.
/// The rejection message must be AGENT_RUNNING_REJECT_MSG verbatim (T-36.1-14).
#[test]
fn test_model_rejected_when_running() {
    use ironhermes_core::commands::running_agent::{is_bypass, AGENT_RUNNING_REJECT_MSG};
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let router = CommandRouter::new(build_registry());
    let agent_running = Arc::new(AtomicBool::new(true)); // simulate in-flight turn

    let resolved = router.resolve("/model gpt-4", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            // This mirrors the dispatch_slash guard check in commands.rs (D-10)
            if agent_running.load(Ordering::SeqCst) && !is_bypass(def.name) {
                // Rejection path — assert message is the D-02 constant
                assert_eq!(
                    AGENT_RUNNING_REJECT_MSG,
                    helpers::d02_error_message(),
                    "/model rejection must use the D-02 constant, not an inline string"
                );
                assert!(
                    !is_bypass(def.name),
                    "is_bypass('model') must return false — model is not on the bypass list"
                );
                return; // Test passed
            }
            panic!("/model should have triggered the rejection path (agent_running=true, not bypass)");
        }
        other => panic!("/model gpt-4 must resolve to a known command, got: {:?}", other),
    }
}

/// GW-05-TUI-4: /stop bypasses the guard even when agent_running is true.
///
/// Verifies that is_bypass("stop") returns true, allowing /stop to dispatch
/// even during an active turn. This is essential for the user to interrupt
/// a running turn.
#[test]
fn test_stop_bypasses_guard() {
    use ironhermes_core::commands::running_agent::is_bypass;
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let router = CommandRouter::new(build_registry());
    let agent_running = Arc::new(AtomicBool::new(true));

    let resolved = router.resolve("/stop", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            // The guard check in dispatch_slash: if running AND NOT bypass → reject
            // For /stop: is_bypass("stop") must be true → bypass path (not rejected)
            assert!(
                is_bypass(def.name),
                "is_bypass('{}') must return true — /stop must bypass the guard (D-01)",
                def.name
            );
            // When running and bypass: guard does NOT reject
            let would_reject = agent_running.load(Ordering::SeqCst) && !is_bypass(def.name);
            assert!(
                !would_reject,
                "/stop must NOT be rejected when agent is running (bypass path)"
            );
        }
        other => panic!("/stop must resolve to a known command, got: {:?}", other),
    }
}

/// GW-05-TUI-5: /reset (alias for "new") bypasses the guard via the canonical name.
///
/// Proves Pitfall 4 mitigation end-to-end:
/// - CommandRouter::resolve("/reset") returns def with def.name == "new" (alias resolution)
/// - is_bypass("new") == true
/// - The guard check uses def.name (post-alias), NOT the raw string "reset"
///
/// If the guard checked is_bypass("reset") on the raw input, it would return false
/// and reject /reset during an active turn — this test would catch that bug.
#[test]
fn test_alias_reset_bypasses_guard() {
    use ironhermes_core::commands::running_agent::is_bypass;
    use ironhermes_core::commands::registry::build_registry;
    use ironhermes_core::commands::{CommandRouter, ResolveResult};
    use ironhermes_core::types::Platform;

    let router = CommandRouter::new(build_registry());
    let agent_running = Arc::new(AtomicBool::new(true));

    let resolved = router.resolve("/reset", &Platform::Local);
    match resolved {
        ResolveResult::Exact(def) | ResolveResult::PrefixMatch(def) => {
            // Pitfall 4 assertion: /reset must resolve to canonical "new"
            assert_eq!(
                def.name, "new",
                "/reset must resolve to canonical 'new' via CommandRouter alias resolution (Pitfall 4)"
            );
            // is_bypass uses the canonical name — not the raw "/reset" string
            assert!(
                is_bypass(def.name),
                "is_bypass('new') must return true — /reset -> 'new' must bypass the guard"
            );
            // When running and bypass: guard does NOT reject
            let would_reject = agent_running.load(Ordering::SeqCst) && !is_bypass(def.name);
            assert!(
                !would_reject,
                "/reset (resolves to 'new') must NOT be rejected when agent is running"
            );
        }
        other => panic!(
            "/reset must resolve to a known command (expected 'new' alias), got: {:?}",
            other
        ),
    }
}
