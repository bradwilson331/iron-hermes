//! Phase 57 Plan 23 (G-1, D-25): a web fetch (`web_read`, `web_extract`) is judged exactly as the
//! operator decided.
//!
//! D-25 = `b-strict-levels`: at DEFCON 1-2 a clean web fetch needs the operator's approval; at
//! DEFCON 3-5 it runs freely. A Critical URL is still a Block at every level (D-07), pinned by
//! `web_read_url_screened_as_exec` in `registry_screen_e2e.rs`.
//!
//! Every test drives a real `AgentLoop` against a mocked LLM, with the screen installed on the
//! registry over an in-process `FakeBackend` (no sidecar binary). The tests mutate
//! `IRONHERMES_HOME` and the process-wide DEFCON registry, so this file must run with
//! `--test-threads=1`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_core::{
    ApprovalGate, ApprovalOutcome, AuditConfig, AuditLog, ChatMessage, SecurityEvent, ToolSchema,
};
use ironhermes_hooks::{ScreenDecision, SecurityToolScreen};
use ironhermes_security::service::SecurityService;
use ironhermes_security::testing::FakeBackend;
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The highest DEFCON level at which a clean web fetch needs approval (D-25, option B).
const THRESHOLD: u8 = 2;

/// The rule id the screen names for a web-fetch Warn.
const RULE: &str = "web_fetch_needs_approval";

/// Sets `IRONHERMES_HOME` for a test and restores the previous value on drop.
struct ScopedHomeEnv {
    prev: Option<String>,
}

impl ScopedHomeEnv {
    fn set(dir: &std::path::Path) -> Self {
        let prev = std::env::var("IRONHERMES_HOME").ok();
        // SAFETY: this file requires `--test-threads=1`, so no other thread reads the env.
        unsafe { std::env::set_var("IRONHERMES_HOME", dir) };
        Self { prev }
    }
}

impl Drop for ScopedHomeEnv {
    fn drop(&mut self) {
        match &self.prev {
            // SAFETY: single-threaded, see `set`.
            Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
            None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
        }
    }
}

/// A tool that counts how many times it really ran.
struct StubTool {
    tool_name: String,
    invoked: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn toolset(&self) -> &str {
        ironhermes_tools::registry::MCP_TOOLSET
    }
    fn description(&self) -> &str {
        "stub web-fetch tool for the Phase 57 D-25 tests"
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
        Ok("stub-executed".to_string())
    }
}

fn tool_call_response(name: &str, args: &serde_json::Value) -> serde_json::Value {
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
                    "id": "call-1",
                    "type": "function",
                    "function": { "name": name, "arguments": args.to_string() }
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

/// One tool result seen by the loop: `(tool, success, text)`.
type SeenResult = (String, bool, String);

/// A temp `IRONHERMES_HOME` plus the audit log inside it.
struct Harness {
    _home: tempfile::TempDir,
    _env: ScopedHomeEnv,
    audit_path: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let env = ScopedHomeEnv::set(home.path());
        let audit_path = home.path().join("logs").join("audit.jsonl");
        Self {
            _home: home,
            _env: env,
            audit_path,
        }
    }

    fn screen_over(&self, fake: &Arc<FakeBackend>) -> Arc<SecurityToolScreen> {
        let service = Arc::new(SecurityService::new(fake.clone()));
        Arc::new(
            SecurityToolScreen::with_service(service, AuditConfig::default()).with_audit_log(
                AuditLog::with_path(self.audit_path.clone(), AuditConfig::default()),
            ),
        )
    }
}

/// An approval gate that records what it was asked and answers with a fixed outcome.
struct FixedGate {
    outcome: ApprovalOutcome,
    asked: Mutex<Vec<(String, String)>>,
}

impl FixedGate {
    fn new(outcome: ApprovalOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            asked: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl ApprovalGate for FixedGate {
    async fn request_approval(
        &self,
        _session_id: &str,
        tool_name: &str,
        reason: &str,
        _args: &serde_json::Value,
    ) -> ApprovalOutcome {
        self.asked
            .lock()
            .unwrap()
            .push((tool_name.to_string(), reason.to_string()));
        self.outcome
    }

    async fn resolve(&self, _session_id: &str, _approved: bool) -> bool {
        false
    }

    async fn notify_security_event(&self, _session_id: &str, _event: &SecurityEvent) {}
}

/// Drive one LLM tool call through a real `AgentLoop` over `registry`.
async fn run_one_call_with_gate(
    registry: ToolRegistry,
    session_id: &str,
    tool: &str,
    args: serde_json::Value,
    gate: Option<Arc<dyn ApprovalGate>>,
) -> Vec<SeenResult> {
    let server = MockServer::start().await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(tool_call_response(tool, &args)))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(final_text_response()))
        .mount(&server)
        .await;

    let seen: Arc<Mutex<Vec<SeenResult>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_cb = seen.clone();
    let client = AnyClient::ChatCompletions(LlmClient::new(
        server.uri(),
        "test-key".to_string(),
        "gpt-4o-mini",
    ));
    let mut agent = AgentLoop::new(client, Arc::new(RwLock::new(registry)), 4)
        .with_session_id(session_id)
        .with_tool_result(Box::new(move |name, ok, result| {
            seen_cb
                .lock()
                .unwrap()
                .push((name.to_string(), ok, result.to_string()));
        }));
    if let Some(gate) = gate {
        agent = agent.with_approval_gate(gate);
    }
    let result = agent.run(vec![ChatMessage::user("go")]).await;
    assert!(result.is_ok(), "run must complete: {result:?}");
    let seen = seen.lock().unwrap().clone();
    if !seen.is_empty() {
        return seen;
    }
    // A call refused before it ran never reaches the result callback; the model still sees the
    // refusal as the tool message, so read it from the transcript.
    result
        .expect("checked above")
        .appended
        .iter()
        .filter(|m| m.role == ironhermes_core::Role::Tool)
        .filter_map(|m| m.content_text().map(str::to_string))
        .map(|text| (tool.to_string(), false, text))
        .collect()
}

fn registry_with_stub(name: &str, invoked: &Arc<AtomicUsize>) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register_dynamic(Box::new(StubTool {
        tool_name: name.to_string(),
        invoked: invoked.clone(),
    }));
    registry
}

/// Every audit row in `path`, parsed. Empty when the file does not exist.
fn audit_rows(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Rows of one `kind` written for `tool`.
fn rows_for(path: &std::path::Path, kind: &str, tool: &str) -> Vec<serde_json::Value> {
    audit_rows(path)
        .into_iter()
        .filter(|r| r["kind"] == kind && r["tool"] == tool)
        .collect()
}

fn set_level(session: &str, level: ironhermes_core::config::DefconLevel) {
    ironhermes_core::defcon::session_defcon().set_session_level(session, level);
}

fn defcon(n: u8) -> ironhermes_core::config::DefconLevel {
    ironhermes_core::config::DefconLevel::try_from(n).expect("level 1..=5")
}

/// Run one clean web-fetch call at `level`. Returns whether the stub ran, the tool results and
/// the number of times the audit log recorded a `security_verdict` row for the tool.
async fn run_fetch(
    h: &Harness,
    tool: &str,
    args: serde_json::Value,
    level: u8,
    gate: Option<Arc<dyn ApprovalGate>>,
) -> (usize, Vec<SeenResult>) {
    let session = format!("sess-57-23-{tool}-l{level}");
    set_level(&session, defcon(level));
    let fake = Arc::new(FakeBackend::new());
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub(tool, &invoked);
    registry.set_tool_screen(h.screen_over(&fake));
    let results = run_one_call_with_gate(registry, &session, tool, args, gate).await;
    ironhermes_core::defcon::session_defcon().forget(&session);
    (invoked.load(Ordering::SeqCst), results)
}

fn read_args() -> serde_json::Value {
    serde_json::json!({ "url": "https://example.com/a" })
}

fn extract_args() -> serde_json::Value {
    serde_json::json!({ "urls": ["https://example.com/a"] })
}

/// The screen's constants are the operator's D-25 choice.
#[test]
fn web_fetch_threshold_is_the_d25_choice() {
    assert_eq!(
        ironhermes_hooks::screen::WEB_FETCH_APPROVAL_MAX_LEVEL,
        THRESHOLD,
        "the screen's threshold is the operator's D-25 choice"
    );
    assert_eq!(ironhermes_hooks::screen::RULE_WEB_FETCH_APPROVAL, RULE);
    assert_eq!(
        ironhermes_hooks::screen::WEB_FETCH_TOOLS,
        &["web_read", "web_extract"]
    );
}

/// At DEFCON 1-2 with no approval channel, a clean `web_read` never runs, the model is told
/// who refused (the same text any unavailable gate gives), and the audit row records the
/// refusal as a web-fetch approval.
#[tokio::test]
async fn web_fetch_needs_approval_at_or_below_threshold() {
    let h = Harness::new();
    for level in 1..=THRESHOLD {
        let (ran, results) = run_fetch(&h, "web_read", read_args(), level, None).await;
        assert_eq!(
            ran, 0,
            "level {level}: a web fetch needing approval never runs"
        );
        assert_eq!(results.len(), 1, "level {level}: {results:?}");
        assert_eq!(
            results[0].2,
            ironhermes_hooks::approval_refusal_text("web_read", &ApprovalOutcome::GateUnavailable)
                .unwrap(),
            "level {level}"
        );
    }
    let rows = rows_for(&h.audit_path, "security_verdict", "web_read");
    assert_eq!(
        rows.len(),
        usize::from(THRESHOLD),
        "one security_verdict row per refused call: {rows:?}"
    );
    for row in &rows {
        assert_eq!(row["decision"], "denied", "{row}");
        assert_eq!(row["resolution"], "gate_unavailable", "{row}");
        assert!(
            row["reason"]
                .as_str()
                .unwrap_or_default()
                .contains("a web fetch needs the operator's approval"),
            "the row must say why: {row}"
        );
    }

    // The screen names the rule on the Warn it returns.
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub("web_read", &invoked);
    registry.set_tool_screen(h.screen_over(&Arc::new(FakeBackend::new())));
    let session = "sess-57-23-rule-id";
    set_level(session, defcon(THRESHOLD));
    let decision = registry
        .screen_call("web_read", &read_args(), session, None)
        .await;
    ironhermes_core::defcon::session_defcon().forget(session);
    match decision {
        ScreenDecision::Warn { rules, .. } => {
            assert!(rules.iter().any(|r| r == RULE), "rules: {rules:?}");
        }
        other => panic!("expected a Warn naming {RULE}, got {other:?}"),
    }
}

/// An approved web fetch at the threshold level runs exactly once, and the audit records an
/// operator approval.
#[tokio::test]
async fn approved_web_fetch_runs_once() {
    let h = Harness::new();
    let gate = FixedGate::new(ApprovalOutcome::Approved);
    let (ran, results) = run_fetch(
        &h,
        "web_read",
        read_args(),
        THRESHOLD,
        Some(gate.clone() as Arc<dyn ApprovalGate>),
    )
    .await;
    assert_eq!(
        ran, 1,
        "an approved web fetch runs exactly once: {results:?}"
    );
    assert_eq!(gate.asked().len(), 1, "the operator was asked once");
    assert_eq!(gate.asked()[0].0, "web_read");
    let rows = rows_for(&h.audit_path, "security_verdict", "web_read");
    assert!(
        rows.iter()
            .any(|r| r["decision"] == "approved" && r["resolution"] == "operator"),
        "an approved row with resolution operator: {rows:?}"
    );
}

/// Above the threshold a clean web fetch runs with no approval asked and no refusal row.
#[tokio::test]
async fn web_fetch_runs_freely_above_threshold() {
    let h = Harness::new();
    for level in (THRESHOLD + 1)..=5 {
        let gate = FixedGate::new(ApprovalOutcome::Denied);
        let (ran, results) = run_fetch(
            &h,
            "web_read",
            read_args(),
            level,
            Some(gate.clone() as Arc<dyn ApprovalGate>),
        )
        .await;
        assert_eq!(
            ran, 1,
            "level {level}: a clean fetch runs once: {results:?}"
        );
        assert!(
            gate.asked().is_empty(),
            "level {level}: no approval may be asked"
        );
        let (ran_no_gate, _) = run_fetch(&h, "web_read", read_args(), level, None).await;
        assert_eq!(ran_no_gate, 1, "level {level}: runs with no gate too");
    }
    let rows = rows_for(&h.audit_path, "security_verdict", "web_read");
    assert!(
        rows.iter()
            .all(|r| r["decision"] != "denied" && r["resolution"] != "gate_unavailable"),
        "no refusal row above the threshold: {rows:?}"
    );
}

/// `web_extract` follows `web_read`: one clean URL at DEFCON 1 with no gate does not run, and
/// above the threshold it runs.
#[tokio::test]
async fn web_extract_follows_web_read() {
    let h = Harness::new();
    let (ran, results) = run_fetch(&h, "web_extract", extract_args(), 1, None).await;
    assert_eq!(ran, 0, "web_extract needs approval at level 1: {results:?}");
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(
        results[0].2,
        ironhermes_hooks::approval_refusal_text("web_extract", &ApprovalOutcome::GateUnavailable)
            .unwrap()
    );
    let (ran, results) = run_fetch(&h, "web_extract", extract_args(), THRESHOLD + 1, None).await;
    assert_eq!(ran, 1, "web_extract runs above the threshold: {results:?}");
}
