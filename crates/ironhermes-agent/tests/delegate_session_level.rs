//! Phase 57 Plan 21 (CR-02 / D-09 / D-15 / D-21): a `delegate_task` child is judged at the
//! DELEGATING chat's live DEFCON level, not at the shared runtime's transcript label.
//!
//! On the gateway the runner is labelled `gateway`, on the web server `web-ui`: process-level
//! literals no `/yolo` ever targets. Before this plan a child ran `with_session_id(<label>)`,
//! so the screen read the label's level and a chat tightened to `/yolo 1` still got a bypass for
//! its children. These tests drive the whole path (parent loop -> delegate_task -> runner ->
//! child loop -> screen) over a wiremock LLM, with the runner built the way the gateway builds
//! it: `.with_transcript_scope(<home>, "gateway")`.
//!
//! The tests mutate `IRONHERMES_HOME` and the process-wide DEFCON registry (levels and the
//! process default), so this file must run with `--test-threads=1`, and every test restores
//! both before it returns.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use ironhermes_agent::AgentLoop;
use ironhermes_agent::any_client::AnyClient;
use ironhermes_agent::client::LlmClient;
use ironhermes_agent::subagent_runner::AgentSubagentRunner;
use ironhermes_core::config::DefconLevel;
use ironhermes_core::defcon::session_defcon;
use ironhermes_core::{AuditConfig, AuditLog, ChatMessage, ToolSchema};
use ironhermes_hooks::SecurityToolScreen;
use ironhermes_hooks::screen::UNSCOPED_DELEGATE_SESSION_PREFIX;
use ironhermes_security::service::SecurityService;
use ironhermes_security::testing::FakeBackend;
use ironhermes_tools::delegate_task::SubagentRunner;
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The shared-runtime transcript label the gateway gives its runner.
const GATEWAY_LABEL: &str = "gateway";
/// The delegating chat's real session id.
const CHAT: &str = "chat-cr02";
/// A Warn-class command at DEFCON 4-5 and a Block at DEFCON 1: a pipe trips the regex floor.
const WARN_CLASS_COMMAND: &str = "echo cr02 | cat";

/// Sets `IRONHERMES_HOME` for a test and restores the previous value on drop.
struct ScopedHomeEnv {
    prev: Option<String>,
}

impl ScopedHomeEnv {
    fn set(dir: &Path) -> Self {
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

/// Restores the process-global DEFCON state a test touched, on every exit path (a failed
/// assertion included), so one red test cannot poison the next.
struct DefconGuard {
    sessions: Vec<String>,
}

impl DefconGuard {
    fn new(sessions: &[&str]) -> Self {
        Self {
            sessions: sessions.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl Drop for DefconGuard {
    fn drop(&mut self) {
        let levels = session_defcon();
        for s in &self.sessions {
            levels.forget(s);
        }
        levels.set_process_default(DefconLevel::THREE);
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

/// Mount `responses` in order (each answers exactly one request), then a trailing unbounded
/// final-text response so a surplus request never hangs the run.
async fn mount_in_order(server: &MockServer, responses: Vec<serde_json::Value>) {
    for body in responses {
        Mock::given(any())
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .up_to_n_times(1)
            .mount(server)
            .await;
    }
    Mock::given(any())
        .respond_with(ResponseTemplate::new(200).set_body_json(final_text_response()))
        .mount(server)
        .await;
}

fn client_for(server: &MockServer) -> AnyClient {
    AnyClient::ChatCompletions(LlmClient::new(
        server.uri(),
        "test-key".to_string(),
        "gpt-4o-mini",
    ))
}

fn runner_labelled(server: &MockServer, home: &Path, label: &str) -> AgentSubagentRunner {
    let resolver = ironhermes_core::ProviderResolver::build(&ironhermes_core::Config::default())
        .expect("the default config builds a resolver");
    AgentSubagentRunner::new(client_for(server), resolver, None)
        .with_transcript_scope(home.to_path_buf(), label.to_string())
}

/// A screen over an in-process clean sidecar, writing audit rows to `audit_path`.
fn screen_to(audit_path: &Path) -> Arc<SecurityToolScreen> {
    let service = Arc::new(SecurityService::new(Arc::new(FakeBackend::new())));
    Arc::new(
        SecurityToolScreen::with_service(service, AuditConfig::default()).with_audit_log(
            AuditLog::with_path(audit_path.to_path_buf(), AuditConfig::default()),
        ),
    )
}

/// Every audit row in `path`, parsed. Empty when the file does not exist.
fn audit_rows(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

fn terminal_verdict_rows(path: &Path) -> Vec<serde_json::Value> {
    audit_rows(path)
        .into_iter()
        .filter(|r| r["kind"] == "security_verdict" && r["tool"] == "terminal")
        .collect()
}

/// A tempdir holding the scoped `IRONHERMES_HOME`, the audit file, and the runner's transcripts.
struct Harness {
    home: tempfile::TempDir,
    _env: ScopedHomeEnv,
    audit_path: PathBuf,
}

impl Harness {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("tempdir");
        let env = ScopedHomeEnv::set(home.path());
        let audit_path = home.path().join("logs").join("audit.jsonl");
        Self {
            home,
            _env: env,
            audit_path,
        }
    }
}

/// Drive a parent loop running as `CHAT` through `delegate_task` into a child on a runner
/// labelled `GATEWAY_LABEL`. The child calls `terminal` with `WARN_CLASS_COMMAND`.
async fn delegate_through_gateway_runner(h: &Harness) {
    let server = MockServer::start().await;
    mount_in_order(
        &server,
        vec![
            // (1) the parent delegates
            tool_call_response(
                "delegate_task",
                &serde_json::json!({
                    "task": "run the check command",
                    "toolsets": ["terminal"]
                }),
            ),
            // (2) the child calls terminal
            tool_call_response(
                "terminal",
                &serde_json::json!({ "command": WARN_CLASS_COMMAND }),
            ),
            // (3) the child finishes
            final_text_response(),
            // (4) the parent finishes
            final_text_response(),
        ],
    )
    .await;

    let runner: Arc<dyn SubagentRunner> =
        Arc::new(runner_labelled(&server, h.home.path(), GATEWAY_LABEL));
    // The screen goes on BEFORE the delegate tool is registered: the tool captures the
    // registry's screen at registration and hands it to every child registry.
    let mut registry = ToolRegistry::new();
    registry.set_tool_screen(screen_to(&h.audit_path));
    registry.register_delegate_task_tool(
        runner,
        Arc::new(tokio::sync::Semaphore::new(3)),
        None,
        ironhermes_core::SubagentConfig::default(),
        None,
        None,
        None,
    );

    let mut parent = AgentLoop::new(client_for(&server), Arc::new(RwLock::new(registry)), 4)
        .with_session_id(CHAT);
    let result = parent.run(vec![ChatMessage::user("go")]).await;
    assert!(result.is_ok(), "the parent run must complete: {result:?}");
}

#[tokio::test]
async fn gateway_child_judged_at_chat_level_not_label() {
    let h = Harness::new();
    let _restore = DefconGuard::new(&[CHAT, GATEWAY_LABEL]);
    let levels = session_defcon();
    levels.set_process_default(DefconLevel::FIVE);
    // The label at its bad value: permissive. The chat tightened itself with `/yolo 1`.
    levels.set_session_level(GATEWAY_LABEL, DefconLevel::FIVE);
    levels.set_session_level(CHAT, DefconLevel::ONE);

    delegate_through_gateway_runner(&h).await;

    let rows = terminal_verdict_rows(&h.audit_path);
    assert!(
        rows.iter().all(|r| r["resolution"] != "bypass"),
        "a chat at /yolo 1 must not get a bypass for its child's Warn-class call: {rows:?}"
    );
    let blocked: Vec<_> = rows.iter().filter(|r| r["decision"] == "blocked").collect();
    assert_eq!(
        blocked.len(),
        1,
        "exactly one blocked terminal row expected: {rows:?}"
    );
    assert_eq!(
        blocked[0]["session"], CHAT,
        "the row is attributed to the delegating chat, not the label: {rows:?}"
    );
}

#[tokio::test]
async fn gateway_child_bypasses_when_chat_is_five() {
    let h = Harness::new();
    let _restore = DefconGuard::new(&[CHAT, GATEWAY_LABEL]);
    let levels = session_defcon();
    levels.set_process_default(DefconLevel::ONE);
    // D-21 both directions: the label is at 1 but the chat is at 5, so the child follows the chat.
    levels.set_session_level(GATEWAY_LABEL, DefconLevel::ONE);
    levels.set_session_level(CHAT, DefconLevel::FIVE);

    delegate_through_gateway_runner(&h).await;

    let rows = terminal_verdict_rows(&h.audit_path);
    let bypass: Vec<_> = rows
        .iter()
        .filter(|r| r["resolution"] == "bypass")
        .collect();
    assert_eq!(
        bypass.len(),
        1,
        "exactly one audited bypass expected: {rows:?}"
    );
    assert_eq!(
        bypass[0]["session"], CHAT,
        "the bypass is audited under the delegating chat: {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r["decision"] != "blocked"),
        "a chat at 5 gets no blocked terminal row: {rows:?}"
    );
}

/// A stand-in `terminal` that counts how many times it really ran.
struct CountingTerminal {
    invoked: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for CountingTerminal {
    fn name(&self) -> &str {
        "terminal"
    }
    fn toolset(&self) -> &str {
        ironhermes_tools::registry::MCP_TOOLSET
    }
    fn description(&self) -> &str {
        "counting stand-in for terminal (CR-02 unscoped-child test)"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "terminal",
            self.description(),
            serde_json::json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        self.invoked.fetch_add(1, Ordering::SeqCst);
        Ok("counted".to_string())
    }
}

#[tokio::test]
async fn unscoped_child_judged_at_level_one() {
    let h = Harness::new();
    let _restore = DefconGuard::new(&[GATEWAY_LABEL]);
    let levels = session_defcon();
    // The process default is a bypass level: exactly the failure CR-02 describes if an
    // unscoped child were judged at it.
    levels.set_process_default(DefconLevel::FIVE);
    levels.set_session_level(GATEWAY_LABEL, DefconLevel::FIVE);

    let server = MockServer::start().await;
    mount_in_order(
        &server,
        vec![
            tool_call_response(
                "terminal",
                &serde_json::json!({ "command": WARN_CLASS_COMMAND }),
            ),
            final_text_response(),
        ],
    )
    .await;
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = ToolRegistry::new();
    registry.register_dynamic(Box::new(CountingTerminal {
        invoked: invoked.clone(),
    }));
    registry.set_tool_screen(screen_to(&h.audit_path));

    // No `AgentLoop::run` above this call, so no delegating session is in scope.
    assert_eq!(ironhermes_hooks::screen::delegating_session(), None);
    let runner = runner_labelled(&server, h.home.path(), GATEWAY_LABEL);
    let out = runner
        .run_child(
            Arc::new(RwLock::new(registry)),
            "Complete the task: run the check command".to_string(),
            4,
            None,
            None,
            None,
            120,
        )
        .await;
    assert!(out.is_ok(), "the child run must complete: {out:?}");

    assert_eq!(
        invoked.load(Ordering::SeqCst),
        0,
        "an unscoped child is judged at DEFCON 1: the Warn-class call must not run"
    );
    let rows = terminal_verdict_rows(&h.audit_path);
    let blocked: Vec<_> = rows.iter().filter(|r| r["decision"] == "blocked").collect();
    assert_eq!(
        blocked.len(),
        1,
        "exactly one blocked terminal row expected: {rows:?}"
    );
    assert!(
        blocked[0]["session"]
            .as_str()
            .is_some_and(|s| s.starts_with(UNSCOPED_DELEGATE_SESSION_PREFIX)),
        "the row carries the reserved unscoped id: {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r["resolution"] != "bypass"),
        "no bypass at the process default of 5: {rows:?}"
    );
}
