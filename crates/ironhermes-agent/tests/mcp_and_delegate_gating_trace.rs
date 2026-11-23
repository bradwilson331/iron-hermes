//! Phase 54 Plan 04 (D-17, D-06): regression pins on CURRENT behavior for two
//! open evidence questions this phase resolves with a trace instead of an
//! assumption. These tests are REGRESSION PINS, not fix-verification tests —
//! neither open question is fixed by this plan; each is answered from the
//! code and pinned so a future change in EITHER direction (tightening or
//! silently loosening) is visible instead of discovered live.
//!
//! - D-17: where is an MCP tool call actually gated, and does it reach the
//!   audit log? `mcp_tool_call_reaches_guardrail_and_approval` pins a control
//!   that EXISTS (the generic dispatch path reaches `check_guardrails` and the
//!   same `ApprovalGate` trait `execute_gated_command` uses) alongside a
//!   REFINEMENT of 54-RESEARCH.md's pre-traced answer (see this test's own
//!   doc comment and this plan's SUMMARY for the full nuance).
//! - D-06 (closed by Phase 57 D-15): does a `delegate_task` subagent's own toolset
//!   escape gating? Phase 54 pinned that it did (`build_child_registry` installed zero
//!   guardrails). Phase 57 plan 09 closed it: the child registry carries the parent's
//!   security screen, which applies the full floor to `terminal` when no gated intercept
//!   exists. `delegate_task_subagent_toolset_gating_trace` now pins the gap CLOSED, so a
//!   change that drops the screen from child registries fails here.
//!
//! Because `mcp_tool_call_reaches_guardrail_and_approval` mutates
//! `IRONHERMES_HOME` (a process-global env var), this crate's tests must run
//! with `--test-threads=1` — the same env-var-race property documented
//! project-wide for `ironhermes-cli`/`iron_hermes_ui` and already required by
//! this plan's own `<verification>` block.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_core::{ApprovalGate, ApprovalOutcome, ChatMessage, ToolSchema};
use ironhermes_hooks::{DangerousCommandGuardrail, GuardrailDecision, McpMutationGuardrail};
use ironhermes_tools::delegate_task::{ChildRole, build_child_registry};
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// RAII guard mirroring `ironhermes-cli/tests/execute_code_python_gating.rs`'s
/// `ScopedHomeEnv` — sets `IRONHERMES_HOME` for the duration of a test and
/// restores the previous value on drop.
struct ScopedHomeEnv {
    prev: Option<String>,
}

impl ScopedHomeEnv {
    fn set(dir: &std::path::Path) -> Self {
        let prev = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: single-threaded within this nextest-isolated process
        // (`--test-threads=1` is mandatory for this crate/file, per the
        // module doc above).
        unsafe { std::env::set_var("IRONHERMES_HOME", dir) };
        Self { prev }
    }
}

impl Drop for ScopedHomeEnv {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: single-threaded within this nextest-isolated process.
            Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
            None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
        }
    }
}

// ---------------------------------------------------------------------------
// D-17: MCP tool call gating + audit trace
// ---------------------------------------------------------------------------

/// A hand-rolled dynamic tool simulating an MCP-discovered tool registered via
/// `register_dynamic` — the exact path `crates/ironhermes-mcp/src/server_task.rs`
/// uses for every real MCP tool — with a server-prefixed destructive name.
struct FakeMcpTool {
    tool_name: String,
    invoked: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for FakeMcpTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn toolset(&self) -> &str {
        ironhermes_tools::registry::MCP_TOOLSET
    }
    fn description(&self) -> &str {
        "fake MCP tool for D-17 trace"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            &self.tool_name,
            self.description(),
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Ok("mcp-tool-executed".to_string())
    }
}

/// Recording `ApprovalGate` double — records every `request_approval` call and
/// always resolves `Approved` so the dispatch path runs end to end. This
/// test's job is to observe WHETHER the gate is consulted and whether an
/// audit entry appears — not to exercise approval-outcome branching (that is
/// plan 01's / the gateway's own test surface).
#[derive(Default)]
struct RecordingApprovalGate {
    calls: Mutex<Vec<(String, String, String)>>, // (session_id, tool_name, reason)
}

#[async_trait]
impl ApprovalGate for RecordingApprovalGate {
    async fn request_approval(
        &self,
        session_id: &str,
        tool_name: &str,
        reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        self.calls
            .lock()
            .unwrap()
            .push((session_id.to_string(), tool_name.to_string(), reason.to_string()));
        ApprovalOutcome::Approved
    }

    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }
}

fn tool_call_response(call_id: &str, name: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "resp-1",
        "object": "chat.completion",
        "created": 0,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": { "name": name, "arguments": "{}" }
                }]
            },
            "finish_reason": "tool_calls"
        }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 }
    })
}

fn final_text_response() -> serde_json::Value {
    serde_json::json!({
        "id": "resp-2",
        "object": "chat.completion",
        "created": 1,
        "model": "gpt-4o-mini",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "done" },
            "finish_reason": "stop"
        }],
        "usage": { "prompt_tokens": 20, "completion_tokens": 2, "total_tokens": 22 }
    })
}

/// D-17 REGRESSION PIN — a control that EXISTS, plus an independently
/// re-verified (not inherited) refinement of 54-RESEARCH.md Summary point 2.
///
/// **Confirmed by this test, exactly as the research states:** the generic
/// agent-loop dispatch path (`AgentLoop::execute_tool_call`, private, driven
/// here via the public `run()` entry point against a mocked LLM) calls
/// `ToolRegistry::check_guardrails` — which runs `McpMutationGuardrail` — and
/// on `NeedsApproval` calls the SAME `ironhermes_core::ApprovalGate` trait
/// `execute_gated_command` (`crates/ironhermes-hooks/src/gated_exec.rs`) uses.
/// `ironhermes-agent`'s own code (`agent_loop.rs`, read in full for this task)
/// contains ZERO references to `AuditLog`/`audit.jsonl` anywhere in this path.
///
/// **Refinement beyond the research's literal claim (independently
/// re-verified by reading `crates/ironhermes-gateway/src/approval.rs` in
/// full, which the research's own read_first list did not cover):** the
/// blanket statement "MCP tool calls are gated and approved but never
/// audited" is true of `ironhermes-agent`'s OWN code, but is NOT true of every
/// surface. The gateway's real `ApprovalGate` implementation
/// (`GatewayApprovalGate` / `ApprovalCoordinator::request`) DOES write exactly
/// one `AuditEntry` (`kind = "mcp_mutation"`) for EVERY resolution — bypass,
/// operator-approved, operator-denied, dropped-sender, and timed-out — when
/// wired for a platform (Telegram/Discord/Buzz via `runner.rs`). So whether an
/// MCP approval resolution is durably audited is NOT a guarantee
/// `ironhermes-agent` provides structurally — it is entirely a property of
/// which CONCRETE `ApprovalGate` a surface wires in. A surface with no gate
/// wired (`GateUnavailable`) or, hypothetically, a future non-auditing gate,
/// leaves the resolution unaudited; the gateway platforms do not. This
/// nuance — not the flatter claim — is what `docs/SECURITY.md` must carry
/// (see this plan's SUMMARY for the exact prose plan 07 lifts).
///
/// Because this test's own recording double does NOT write to `AuditLog`
/// (mirroring the `ironhermes-agent`-only slice this test can exercise —
/// `ironhermes-gateway` is not and must not become a dependency of
/// `ironhermes-agent`), the audit-file assertion below observes the "file
/// absent" case — the true, expected outcome for `ironhermes-agent`'s own
/// dispatch path in isolation from any specific gate implementation.
#[tokio::test]
async fn mcp_tool_call_reaches_guardrail_and_approval() {
    let home = tempfile::tempdir().expect("tempdir");
    let _env = ScopedHomeEnv::set(home.path());

    let server = MockServer::start().await;
    let destructive_name = "hostile_server__delete_resource";
    Mock::given(any())
        .respond_with(
            ResponseTemplate::new(200).set_body_json(tool_call_response("call-1", destructive_name)),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(final_text_response()))
        .mount(&server)
        .await;

    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    // The real control this test pins: McpMutationGuardrail, added the same
    // way production wires it (`ironhermes-agent/src/app_runtime_factory.rs`).
    registry.add_guardrail(Box::new(McpMutationGuardrail::new()));
    registry.register_dynamic(Box::new(FakeMcpTool {
        tool_name: destructive_name.to_string(),
        invoked: invoked.clone(),
    }));
    let registry = Arc::new(RwLock::new(registry));

    let gate = Arc::new(RecordingApprovalGate::default());
    let client = AnyClient::ChatCompletions(LlmClient::new(
        server.uri(),
        "test-key".to_string(),
        "gpt-4o-mini",
    ));
    let mut agent = AgentLoop::new(client, registry, 4)
        .with_session_id("sess-d17")
        .with_approval_gate(gate.clone());

    let result = agent
        .run(vec![ChatMessage::user("delete the resource")])
        .await;
    assert!(result.is_ok(), "run must complete: {result:?}");

    // (a) the approval gate WAS consulted for the destructive MCP call —
    // proving the dispatch path reaches the same ApprovalGate seam
    // execute_gated_command uses.
    let calls = gate.calls.lock().unwrap().clone();
    assert_eq!(
        calls.len(),
        1,
        "approval gate must be consulted exactly once for a destructive-verb \
         MCP tool call, got: {calls:?}"
    );
    assert_eq!(calls[0].1, destructive_name);
    assert!(
        calls[0].2.contains("destructive verb"),
        "the approval reason must name McpMutationGuardrail's own decision \
         text, got: {}",
        calls[0].2
    );

    // The approved tool actually executed (Approved -> execute_allowed_tool
    // -> registry.execute_tool, which skips the guardrail chain a second
    // time by design).
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        1,
        "an Approved MCP tool call must execute exactly once"
    );

    // (b) the audit-log outcome — explicit, not a bare grep: distinguish
    // "file absent" from "file present and empty" and report which held,
    // rather than accepting either silently.
    let audit_path = home.path().join("logs").join("audit.jsonl");
    if audit_path.exists() {
        let contents = std::fs::read_to_string(&audit_path).unwrap_or_default();
        assert!(
            contents.trim().is_empty(),
            "D-17: logs/audit.jsonl EXISTS after this call, but ironhermes-agent's \
             own dispatch path (this test's slice) contains no AuditLog call — \
             an unexpected non-empty file would mean something ELSE in this test \
             wrote to it, which would invalidate this trace. Found: {contents}"
        );
    }
    // else: file absent, which is the TRUE, expected outcome for
    // ironhermes-agent's own dispatch path in isolation — see this test's
    // doc comment for the surface-dependent nuance that follows from this.
}

// ---------------------------------------------------------------------------
// D-06: delegate_task subagent toolset gating trace
// ---------------------------------------------------------------------------

/// D-06 REGRESSION PIN, FLIPPED by Phase 57 plan 09 (D-15): the gap Phase 54 named is
/// CLOSED, and this test now guards against re-opening it.
///
/// Phase 54 found that a `delegate_task` child's own toolset never re-entered the
/// parent's gating: `build_child_registry`
/// (`crates/ironhermes-tools/src/delegate_task.rs`) built every child registry from a bare
/// `ToolRegistry::new()` with zero guardrails, so the same `rm -rf /` the parent Blocks was
/// `Allow` in the child. That function is still driven here directly, not reimplemented.
/// It now takes the parent's security screen and installs it on the child registry. A child
/// has no gated `terminal` intercept, so the screen itself applies the full floor to the
/// call, and a Tier-2 command is a Block at every DEFCON level. If `build_child_registry`
/// ever stops carrying the screen (or the screen stops covering `terminal` without an
/// intercept), this test fails instead of the gap silently coming back.
#[tokio::test]
async fn delegate_task_subagent_toolset_gating_trace() {
    let destructive_command = serde_json::json!({ "command": "rm -rf /" });

    // Ground truth: the PARENT registry, wired the same way production does
    // (`ironhermes-agent/src/app_runtime_factory.rs`), DOES Block a Tier-2
    // destructive shell command on "terminal" via DangerousCommandGuardrail.
    // Without this ground-truth assertion, an Allow on the child registry
    // below would prove nothing — it could mean DangerousCommandGuardrail
    // itself is broken, not that the child lacks it.
    let mut parent_registry = ToolRegistry::new();
    parent_registry.add_guardrail(Box::new(DangerousCommandGuardrail::new()));
    let parent_decision = parent_registry.check_guardrails("terminal", &destructive_command);
    assert!(
        matches!(parent_decision, GuardrailDecision::Block { .. }),
        "ground truth: the parent registry (wired like production) must Block \
         'rm -rf /' on terminal, or the child-vs-parent comparison below is \
         moot; got: {parent_decision:?}"
    );

    // The screen a production child registry carries: the same `SecurityToolScreen` the
    // factory installs on the parent, here over an EMPTY fake sidecar so only the
    // ironhermes-side floor can be the thing that blocks (Pitfall 3). Audit rows go to a
    // private file, never the operator's real home.
    let audit_dir = tempfile::tempdir().expect("tempdir");
    let screen: Arc<dyn ironhermes_hooks::ToolScreen> = Arc::new(
        ironhermes_hooks::SecurityToolScreen::with_service(
            Arc::new(ironhermes_security::service::SecurityService::new(
                Arc::new(ironhermes_security::testing::FakeBackend::new()),
            )),
            ironhermes_core::AuditConfig::default(),
        )
        .with_audit_log(ironhermes_core::AuditLog::with_path(
            audit_dir.path().join("audit.jsonl"),
            ironhermes_core::AuditConfig::default(),
        )),
    );

    // The REAL child-registry construction function every production
    // delegate_task dispatch calls.
    let child_registry = build_child_registry(
        &["terminal".to_string()],
        None,
        std::path::Path::new("/tmp"),
        ChildRole::Leaf,
        0,
        &ironhermes_core::SubagentConfig::default(),
        None,
        None,
        Arc::new(ironhermes_tools::credentials::ToolCredentials::env_only()),
        Some(screen),
    )
    .expect("build_child_registry must succeed for a plain 'terminal' allowlist");

    let child_decision = child_registry
        .screen_call("terminal", &destructive_command, "sess-d06-child", None)
        .await;
    assert!(
        matches!(
            child_decision,
            ironhermes_hooks::ScreenDecision::Block { .. }
        ),
        "the Phase 54 D-06 gap is closed by Phase 57 D-15: the SAME 'rm -rf /' \
         command the parent Blocks must be a Block in the child registry too, \
         because build_child_registry now installs the security screen and a \
         child has no gated terminal intercept (the screen applies the full \
         floor itself). If this fails the child registry has lost its screen, \
         re-opening the delegate_task bypass; got: {child_decision:?}"
    );
}
