//! Phase 57 Plan 09 (SC-1 / D-15): the async `ToolScreen` on `ToolRegistry`, driven through a
//! real `AgentLoop` against a mocked LLM.
//!
//! Every test installs the screen on the registry directly, over an in-process
//! `FakeBackend`, so the sidecar binary is never needed. The tests mutate
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
    ApprovalGate, ApprovalOutcome, AuditConfig, AuditLog, ChatMessage, SecurityEvent,
    SecurityEventKind, ToolSchema,
};
use ironhermes_hooks::SecurityToolScreen;
use ironhermes_security::policy::Severity;
use ironhermes_security::service::SecurityService;
use ironhermes_security::testing::FakeBackend;
use ironhermes_tools::{Tool, ToolRegistry};
use tokio::sync::RwLock;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

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
        "stub tool for the Phase 57 registry screen"
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

/// The pieces a test needs after driving one call through a turn.
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

    fn home_path(&self) -> PathBuf {
        self._home.path().to_path_buf()
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

/// Drive one LLM tool call through a real `AgentLoop` over `registry`.
async fn run_one_call(
    registry: ToolRegistry,
    session_id: &str,
    tool: &str,
    args: serde_json::Value,
) -> Vec<SeenResult> {
    run_one_call_with_gate(registry, session_id, tool, args, None).await
}

/// As [`run_one_call`], with the turn's approval gate.
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
    // A call refused before it ran (an unapproved Warn) never reaches the result callback;
    // the model still sees the refusal as the tool message, so read it from the transcript.
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

// ---------------------------------------------------------------------------
// Task 1 (tracer): an MCP call whose ARGUMENTS carry a blocked canary never runs
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_args_canary_blocked_end_to_end() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub("demo__write_note", &invoked);
    registry.set_tool_screen(h.screen_over(&fake));

    let results = run_one_call(
        registry,
        "sess-t1-block",
        "demo__write_note",
        serde_json::json!({ "text": "CANARY-BLOCK" }),
    )
    .await;

    assert_eq!(
        invoked.load(Ordering::SeqCst),
        0,
        "a call whose argument carries a Critical canary must never execute"
    );
    assert_eq!(results.len(), 1, "exactly one tool result: {results:?}");
    assert!(!results[0].1, "the tool result must be a failure");
    assert!(
        results[0]
            .2
            .contains("blocked by guardrail 'security-screen'"),
        "the tool_result must be the formatted guardrail error, got: {}",
        results[0].2
    );
    let seen = fake.seen();
    assert!(
        seen.iter().any(|s| s.contains("CANARY-BLOCK")),
        "the sidecar must have been shown the rendered ARGUMENTS, saw: {seen:?}"
    );
    assert!(
        seen.iter().any(|s| s.contains("demo__write_note")),
        "the rendered subject names the tool, saw: {seen:?}"
    );
}

#[tokio::test]
async fn benign_mcp_call_runs() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub("demo__write_note", &invoked);
    registry.set_tool_screen(h.screen_over(&fake));

    let results = run_one_call(
        registry,
        "sess-t1-benign",
        "demo__write_note",
        serde_json::json!({ "text": "hello" }),
    )
    .await;

    assert_eq!(invoked.load(Ordering::SeqCst), 1, "a benign call must run");
    assert_eq!(results.len(), 1);
    assert!(results[0].1, "benign call succeeds: {results:?}");
    assert!(
        !fake.seen().is_empty(),
        "the benign call must still have been screened"
    );
}

#[tokio::test]
async fn screen_absent_is_unchanged() {
    let _h = Harness::new();
    let invoked = Arc::new(AtomicUsize::new(0));
    let registry = registry_with_stub("demo__write_note", &invoked);

    let results = run_one_call(
        registry,
        "sess-t1-absent",
        "demo__write_note",
        serde_json::json!({ "text": "CANARY-BLOCK" }),
    )
    .await;

    assert_eq!(
        invoked.load(Ordering::SeqCst),
        1,
        "with no screen installed the registry behaves exactly as before"
    );
    assert!(results[0].1);
}

// ---------------------------------------------------------------------------
// Task 2: the screen goes everywhere the registry goes
// ---------------------------------------------------------------------------

/// The data address of a screen, for "is this the same screen" checks that never compare
/// trait-object vtables.
fn screen_addr(screen: &Arc<dyn ironhermes_hooks::ToolScreen>) -> *const () {
    Arc::as_ptr(screen) as *const ()
}

#[tokio::test]
async fn dispatch_with_hook_screens() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub("demo__x", &invoked);
    registry.set_tool_screen(h.screen_over(&fake));

    let result = registry
        .dispatch("demo__x", serde_json::json!({ "text": "CANARY-BLOCK" }))
        .await;

    let err = result.expect_err("a screened Block must come back as an error");
    assert!(
        err.to_string().contains("security-screen"),
        "the error must name the security screen, got: {err}"
    );
    assert_eq!(invoked.load(Ordering::SeqCst), 0, "the tool must never run");
}

#[tokio::test]
async fn rpc_registry_screened() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let screen: Arc<dyn ironhermes_hooks::ToolScreen> = h.screen_over(&fake);
    let rpc = ironhermes_agent::app_runtime_factory::build_rpc_registry(
        None,
        Arc::new(ironhermes_tools::credentials::ToolCredentials::env_only()),
        Some(screen),
    );
    let out_dir = tempfile::tempdir().expect("tempdir");
    let target = out_dir.path().join("rpc-out.txt");

    // The path execute_code's RPC bridge takes: dispatch_with_hook on the sandbox registry,
    // which never passes through agent_loop.
    let result = rpc
        .dispatch_with_hook(
            "write_file",
            serde_json::json!({ "path": target, "content": "CANARY-BLOCK" }),
            None::<fn(&str, &str)>,
        )
        .await;

    let err = result.expect_err("write_file with a canary must be refused");
    assert!(
        err.to_string().contains("security-screen"),
        "the refusal must come from the security screen, got: {err}"
    );
    assert!(
        !target.exists(),
        "the file must not have been created by the refused call"
    );
}

#[test]
fn scope_to_keeps_screen() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new());
    let mut registry = ToolRegistry::new();
    let screen: Arc<dyn ironhermes_hooks::ToolScreen> = h.screen_over(&fake);
    registry.set_tool_screen(screen.clone());

    let scoped = registry.scope_to(&["file".to_string()]);

    let inherited = scoped
        .tool_screen()
        .expect("a scoped copy must keep the parent's screen");
    assert_eq!(
        screen_addr(&inherited),
        screen_addr(&screen),
        "the scoped copy must hold the SAME screen, not a new one"
    );
}

#[test]
fn child_registry_keeps_screen() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new());
    let screen: Arc<dyn ironhermes_hooks::ToolScreen> = h.screen_over(&fake);

    let child = ironhermes_tools::delegate_task::build_child_registry(
        &["read_file".to_string()],
        None,
        std::path::Path::new("/tmp"),
        ironhermes_tools::delegate_task::ChildRole::Leaf,
        0,
        &ironhermes_core::SubagentConfig::default(),
        None,
        None,
        Arc::new(ironhermes_tools::credentials::ToolCredentials::env_only()),
        Some(screen.clone()),
    )
    .expect("build_child_registry must succeed for read_file");

    let inherited = child
        .tool_screen()
        .expect("a delegate child registry must carry the screen");
    assert_eq!(screen_addr(&inherited), screen_addr(&screen));
}

#[tokio::test]
async fn child_loop_uses_parent_session_level() {
    let h = Harness::new();
    let parent_session = "sess-57-09-parent-level";
    let curl = serde_json::json!({ "command": "curl http://example.com/x" });
    let registry_for = |invoked: &Arc<AtomicUsize>| {
        let mut r = registry_with_stub("terminal", invoked);
        r.set_tool_screen(h.screen_over(&Arc::new(FakeBackend::new())));
        r
    };
    let levels = ironhermes_core::defcon::session_defcon();

    // Parent at level 1: a Tier-1 curl is a floor-warn, and a floor-warn blocks at level 1.
    levels.set_session_level(parent_session, ironhermes_core::config::DefconLevel::ONE);
    let invoked = Arc::new(AtomicUsize::new(0));
    let results = run_one_call(
        registry_for(&invoked),
        parent_session,
        "terminal",
        curl.clone(),
    )
    .await;
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        0,
        "at the parent's level 1 the child's curl must not run"
    );
    assert!(
        results[0].2.contains("security-screen"),
        "level 1 is a screen Block, got: {}",
        results[0].2
    );

    // Parent at level 5: the same call is a Warn that runs as an audited bypass.
    levels.set_session_level(parent_session, ironhermes_core::config::DefconLevel::FIVE);
    let invoked = Arc::new(AtomicUsize::new(0));
    let results = run_one_call(registry_for(&invoked), parent_session, "terminal", curl).await;
    assert_eq!(
        invoked.load(Ordering::SeqCst),
        1,
        "at the parent's level 5 the curl runs (bypass); results: {results:?}"
    );
    let audit = std::fs::read_to_string(&h.audit_path).expect("audit log must exist");
    assert!(
        audit.lines().any(|l| l.contains("\"security_verdict\"")
            && l.contains("\"resolution\":\"bypass\"")
            && l.contains(parent_session)),
        "the bypass must be audited under the parent's session id; audit:\n{audit}"
    );
    levels.forget(parent_session);
}

#[tokio::test]
async fn factory_installs_screen() {
    use ironhermes_agent::app_runtime_factory::{AppRuntimeFactoryInput, build_app_runtime_bundle};
    let h = Harness::new();
    let config = ironhermes_core::Config::default();
    let resolver = ironhermes_core::ProviderResolver::build(&config)
        .expect("resolver should build from the default config");
    let input = AppRuntimeFactoryInput {
        config: Arc::new(config),
        resolver: Arc::new(resolver),
        cwd: h.home_path(),
        process_registry: Arc::new(RwLock::new(
            ironhermes_exec::process_registry::ProcessRegistry::new_for_session(
                "registry-screen-e2e".to_string(),
            ),
        )),
        memory_manager: None,
        delegate_task: None,
        hooks_config: ironhermes_hooks::HooksConfig::default(),
        emit_mcp_startup_logs: false,
        session_key: None,
        telegram_adapter: None,
        resolver_handle: None,
    };

    let bundle = build_app_runtime_bundle(input)
        .await
        .expect("factory bundle should build");

    assert!(
        bundle.registry.read().await.tool_screen().is_some(),
        "the factory-built main registry must carry a tool screen"
    );
}

// ---------------------------------------------------------------------------
// Task 3: subjects per tool, the Warn path, and "screened exactly once"
// ---------------------------------------------------------------------------

/// An approval gate that records what it was asked and told, and answers with a fixed
/// outcome.
struct RecordingGate {
    outcome: ApprovalOutcome,
    asked: Mutex<Vec<(String, String)>>,
    events: Mutex<Vec<SecurityEvent>>,
}

impl RecordingGate {
    fn new(outcome: ApprovalOutcome) -> Arc<Self> {
        Arc::new(Self {
            outcome,
            asked: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
        })
    }

    fn asked(&self) -> Vec<(String, String)> {
        self.asked.lock().unwrap().clone()
    }

    fn kinds(&self) -> Vec<SecurityEventKind> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.kind.clone())
            .collect()
    }

    fn events_of(&self, kind: SecurityEventKind) -> Vec<SecurityEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.kind == kind)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl ApprovalGate for RecordingGate {
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

    async fn notify_security_event(&self, _session_id: &str, event: &SecurityEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
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

/// Run one call with a fresh stub registry carrying `fake`'s screen. Returns whether the stub
/// ran, plus the tool results.
async fn run_screened(
    h: &Harness,
    fake: &Arc<FakeBackend>,
    session: &str,
    tool: &str,
    args: serde_json::Value,
    gate: Option<Arc<dyn ApprovalGate>>,
) -> (usize, Vec<SeenResult>) {
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub(tool, &invoked);
    registry.set_tool_screen(h.screen_over(fake));
    let results = run_one_call_with_gate(registry, session, tool, args, gate).await;
    (invoked.load(Ordering::SeqCst), results)
}

#[tokio::test]
async fn web_read_url_screened_as_exec() {
    let url = "http://169.254.169.254/latest/meta-data/";
    let subjects =
        ironhermes_hooks::screen::extract_subjects("web_read", &serde_json::json!({ "url": url }));
    assert_eq!(
        subjects.len(),
        1,
        "web_read yields one subject: {subjects:?}"
    );
    assert_eq!(subjects[0].kind, ironhermes_security::SubjectKind::Exec);
    assert_eq!(subjects[0].text, format!("curl '{url}'"));

    let h = Harness::new();
    let session = "sess-57-09-web-read";
    for n in 1..=5u8 {
        let fake =
            Arc::new(FakeBackend::new().block_if_contains("169.254.169.254", Severity::Critical));
        set_level(session, defcon(n));
        let (ran, results) = run_screened(
            &h,
            &fake,
            session,
            "web_read",
            serde_json::json!({ "url": url }),
            None,
        )
        .await;
        assert_eq!(ran, 0, "a Critical URL must be Blocked at level {n}");
        assert!(
            results[0].2.contains("security-screen"),
            "level {n}: {}",
            results[0].2
        );
        assert!(
            fake.seen().contains(&format!("curl '{url}'")),
            "level {n}: the sidecar must be shown `curl '<url>'`, saw {:?}",
            fake.seen()
        );
    }
    ironhermes_core::defcon::session_defcon().forget(session);

    let many = ironhermes_hooks::screen::extract_subjects(
        "web_extract",
        &serde_json::json!({ "urls": ["https://a.example/x", "https://b.example/y"] }),
    );
    assert_eq!(
        many.iter().map(|s| s.text.clone()).collect::<Vec<_>>(),
        vec![
            "curl 'https://a.example/x'".to_string(),
            "curl 'https://b.example/y'".to_string()
        ],
        "web_extract screens every URL as an exec subject"
    );
    let quoted = ironhermes_hooks::screen::extract_subjects(
        "web_read",
        &serde_json::json!({ "url": "http://h/it's" }),
    );
    assert_eq!(
        quoted[0].text, "curl 'http://h/it'\\''s'",
        "quotes are escaped"
    );
}

#[test]
fn write_file_content_screened_as_file() {
    let subjects = ironhermes_hooks::screen::extract_subjects(
        "write_file",
        &serde_json::json!({ "path": "/tmp/x/SOUL.md", "content": "be kind" }),
    );
    assert_eq!(subjects.len(), 1, "{subjects:?}");
    assert_eq!(subjects[0].kind, ironhermes_security::SubjectKind::File);
    assert_eq!(subjects[0].name.as_deref(), Some("SOUL.md"));
    assert_eq!(subjects[0].text, "be kind");

    let patch = ironhermes_hooks::screen::extract_subjects(
        "patch",
        &serde_json::json!({ "path": "/a/b/AGENTS.md", "before": "old", "after": "new text" }),
    );
    assert_eq!(patch.len(), 1, "{patch:?}");
    assert_eq!(patch[0].kind, ironhermes_security::SubjectKind::File);
    assert_eq!(patch[0].name.as_deref(), Some("AGENTS.md"));
    assert_eq!(
        patch[0].text, "new text",
        "the NEW text is what gets written"
    );
}

#[test]
fn delegate_and_cronjob_text_screened_as_paste() {
    use ironhermes_security::SubjectKind::Paste;
    let one = ironhermes_hooks::screen::extract_subjects(
        "delegate_task",
        &serde_json::json!({ "task": "do the thing", "context": "with this", "role": "leaf" }),
    );
    assert_eq!(one.len(), 1, "{one:?}");
    assert_eq!(one[0].kind, Paste);
    assert_eq!(one[0].text, "do the thing\n\nwith this");

    let batch = ironhermes_hooks::screen::extract_subjects(
        "delegate_task",
        &serde_json::json!({ "tasks": [
            { "goal": "first goal", "context": "c1" },
            { "goal": "second goal" }
        ] }),
    );
    assert_eq!(
        batch.iter().map(|s| s.text.clone()).collect::<Vec<_>>(),
        vec!["first goal\n\nc1".to_string(), "second goal".to_string()]
    );
    assert!(batch.iter().all(|s| s.kind == Paste));

    let cron = ironhermes_hooks::screen::extract_subjects(
        "cronjob",
        &serde_json::json!({ "action": "create", "name": "nightly", "schedule": "0 3 * * *", "prompt": "summarise the day" }),
    );
    assert_eq!(cron.len(), 1, "{cron:?}");
    assert_eq!(cron[0].kind, Paste);
    assert_eq!(
        cron[0].text, "summarise the day",
        "only the prompt is free text"
    );
}

#[tokio::test]
async fn mcp_arg_mutation_command_is_critical() {
    let h = Harness::new();
    let session = "sess-57-09-mcp-mutation";
    // The most permissive level, and an EMPTY sidecar: only the ironhermes-side rule can
    // be the thing that blocks (Pitfall 3).
    set_level(session, defcon(5));
    let fake = Arc::new(FakeBackend::new());
    let (ran, results) = run_screened(
        &h,
        &fake,
        session,
        "cloudflare_bindings__run",
        serde_json::json!({ "command": "wrangler delete prod" }),
        None,
    )
    .await;
    ironhermes_core::defcon::session_defcon().forget(session);

    assert_eq!(
        ran, 0,
        "an MCP argument carrying a mutation command never runs"
    );
    assert!(
        results[0].2.contains("security-screen"),
        "got: {}",
        results[0].2
    );
    let rows = rows_for(
        &h.audit_path,
        "security_verdict",
        "cloudflare_bindings__run",
    );
    assert_eq!(rows.len(), 1, "one blocked verdict row: {rows:?}");
    assert_eq!(rows[0]["decision"], "blocked");
    assert!(
        rows[0]["args"]
            .as_str()
            .unwrap()
            .contains("mutation_command"),
        "the row names the rule: {}",
        rows[0]["args"]
    );
    assert!(
        !rows[0]["args"].as_str().unwrap().contains("wrangler"),
        "the row never carries the argument text"
    );
}

#[tokio::test]
async fn screen_unavailable_warn_class_by_level() {
    use ironhermes_security::UnavailableReason;
    let cases: [(&str, serde_json::Value); 2] = [
        (
            "write_file",
            serde_json::json!({ "path": "/tmp/p.txt", "content": "hello" }),
        ),
        (
            "cloudflare_bindings__r2_bucket_delete",
            serde_json::json!({ "bucket": "b" }),
        ),
    ];
    for (tool, args) in cases {
        // Level 3 first: the strict half is the one an "ignore the outage" regression breaks.
        for (level, should_run, verdict, outcome) in [
            (3u8, false, "block", "blocked"),
            (5u8, true, "warn", "bypass"),
        ] {
            let h = Harness::new();
            let session = format!("sess-57-09-unavail-{tool}-{level}");
            set_level(&session, defcon(level));
            let fake = Arc::new(FakeBackend::unavailable(UnavailableReason::Crash));
            let gate = RecordingGate::new(ApprovalOutcome::Denied);
            let (ran, results) =
                run_screened(&h, &fake, &session, tool, args.clone(), Some(gate.clone())).await;
            ironhermes_core::defcon::session_defcon().forget(&session);

            assert_eq!(
                ran,
                usize::from(should_run),
                "{tool} at level {level}: ran={ran}, results={results:?}"
            );
            let rows = rows_for(&h.audit_path, "sidecar_unavailable", tool);
            assert_eq!(
                rows.len(),
                1,
                "{tool}@{level}: exactly one outage row: {rows:?}"
            );
            let body: serde_json::Value =
                serde_json::from_str(rows[0]["args"].as_str().unwrap()).unwrap();
            assert_eq!(body["reason"], "crash", "{tool}@{level}");
            assert_eq!(body["tool"], tool);
            assert_eq!(body["level"], level);
            assert_eq!(body["verdict"], verdict, "{tool}@{level}");
            let events = gate.events_of(SecurityEventKind::SidecarUnavailable);
            assert_eq!(events.len(), 1, "{tool}@{level}: one outage event");
            assert_eq!(events[0].summary, format!("crash; outcome {outcome}"));
            if level == 3 {
                // G-2: the Blocked event names the cause no rule id could.
                let blocked = gate.events_of(SecurityEventKind::Blocked);
                assert_eq!(blocked.len(), 1, "{tool}@{level}: one Blocked event");
                assert_eq!(
                    blocked[0].rule_ids,
                    vec!["sidecar_outage:crash".to_string()],
                    "{tool}@{level}"
                );
            }
            if should_run {
                assert_eq!(
                    rows_for(&h.audit_path, "security_verdict", tool)
                        .iter()
                        .filter(|r| r["resolution"] == "bypass")
                        .count(),
                    1,
                    "{tool}@{level}: the audited bypass"
                );
                assert!(gate.asked().is_empty(), "a bypass asks nobody");
            }
        }
    }
}

/// GC-WR-03 / G-2: a subject the sidecar refused is reported as "could not be screened",
/// never as a scanner outage, on the registry path too.
#[tokio::test]
async fn screen_unscreened_block_names_the_reason() {
    use ironhermes_security::UnavailableReason;
    let h = Harness::new();
    let session = "sess-57-25-unscreened";
    set_level(session, defcon(3));
    let fake = Arc::new(FakeBackend::unavailable(UnavailableReason::TooLarge));
    let gate = RecordingGate::new(ApprovalOutcome::Denied);
    let (ran, results) = run_screened(
        &h,
        &fake,
        session,
        "write_file",
        serde_json::json!({ "path": "/tmp/p.txt", "content": "hello" }),
        Some(gate.clone()),
    )
    .await;
    ironhermes_core::defcon::session_defcon().forget(session);

    assert_eq!(ran, 0, "the tool must not run: {results:?}");
    assert_eq!(results.len(), 1, "{results:?}");
    let text = &results[0].2;
    assert!(text.contains("could not be screened (too_large)"), "{text}");
    assert!(!text.contains("sidecar unavailable"), "{text}");

    let rows = rows_for(&h.audit_path, "sidecar_unavailable", "write_file");
    assert_eq!(rows.len(), 1, "{rows:?}");
    let body: serde_json::Value = serde_json::from_str(rows[0]["args"].as_str().unwrap()).unwrap();
    assert_eq!(body["reason"], "too_large");
    assert_eq!(body["verdict"], "block");
    let blocked = gate.events_of(SecurityEventKind::Blocked);
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert_eq!(
        blocked[0].rule_ids,
        vec!["unscreened:too_large".to_string()]
    );
    let unavailable = gate.events_of(SecurityEventKind::SidecarUnavailable);
    assert_eq!(unavailable.len(), 1, "{unavailable:?}");
    assert_eq!(unavailable[0].summary, "too_large; outcome blocked");
}

#[tokio::test]
async fn screen_block_beats_bypass_at_level_five() {
    let h = Harness::new();
    let session = "sess-57-09-block-beats-bypass";
    set_level(session, defcon(5));
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let gate = RecordingGate::new(ApprovalOutcome::Approved);
    let ledger = Arc::new(ironhermes_hooks::YoloLedger::default());
    let (ran, results) = ironhermes_hooks::YOLO_LEDGER
        .scope(
            ledger.clone(),
            run_screened(
                &h,
                &fake,
                session,
                "write_file",
                serde_json::json!({ "path": "/tmp/x.txt", "content": "CANARY-BLOCK" }),
                Some(gate.clone()),
            ),
        )
        .await;
    ironhermes_core::defcon::session_defcon().forget(session);

    assert_eq!(
        ran, 0,
        "a Block is never bypassed, even at level 5: {results:?}"
    );
    let rows = rows_for(&h.audit_path, "security_verdict", "write_file");
    assert!(
        rows.iter().any(|r| r["decision"] == "blocked"),
        "a blocked verdict row: {rows:?}"
    );
    assert!(
        rows.iter().all(|r| r["resolution"] != "bypass"),
        "no bypass row: {rows:?}"
    );
    assert_eq!(ledger.recorded_count(), 0, "no bypass ledger entry");
    let kinds = gate.kinds();
    assert!(kinds.contains(&SecurityEventKind::Blocked), "{kinds:?}");
    assert!(!kinds.contains(&SecurityEventKind::BypassRan), "{kinds:?}");
    assert!(
        gate.asked().is_empty(),
        "a Block is not put to the operator"
    );
}

#[tokio::test]
async fn screen_warn_bypassed_only_at_four_five() {
    let h = Harness::new();
    // High is a Warn at level 5, a Block at 3 (policy table); Medium is a Warn at 3 and 4.
    let make_fake = || {
        Arc::new(
            FakeBackend::new()
                .warn_if_contains("CANARY-WARN", Severity::High)
                .warn_if_contains("CANARY-MED", Severity::Medium),
        )
    };
    let write = |text: &str| serde_json::json!({ "path": "/tmp/w.txt", "content": text });

    // Level 5: bypass, audited and recorded in the turn ledger, nobody asked.
    let session = "sess-57-09-warn-5";
    set_level(session, defcon(5));
    let gate = RecordingGate::new(ApprovalOutcome::Denied);
    let ledger = Arc::new(ironhermes_hooks::YoloLedger::default());
    let (ran, _) = ironhermes_hooks::YOLO_LEDGER
        .scope(
            ledger.clone(),
            run_screened(
                &h,
                &make_fake(),
                session,
                "write_file",
                write("CANARY-WARN"),
                Some(gate.clone()),
            ),
        )
        .await;
    assert_eq!(ran, 1, "a Warn runs as a bypass at level 5");
    assert!(gate.asked().is_empty());
    assert_eq!(
        ledger.recorded_count(),
        1,
        "the bypass is in the turn ledger"
    );
    assert!(gate.kinds().contains(&SecurityEventKind::BypassRan));
    let bypass_rows: Vec<_> = rows_for(&h.audit_path, "security_verdict", "write_file")
        .into_iter()
        .filter(|r| r["resolution"] == "bypass")
        .collect();
    assert_eq!(bypass_rows.len(), 1, "one audited bypass row");
    ironhermes_core::defcon::session_defcon().forget(session);

    // Level 4: the Medium finding is a Warn and bypasses too.
    let session = "sess-57-09-warn-4";
    set_level(session, defcon(4));
    let (ran, _) = run_screened(
        &h,
        &make_fake(),
        session,
        "write_file",
        write("CANARY-MED"),
        None,
    )
    .await;
    assert_eq!(ran, 1, "a Warn runs as a bypass at level 4");
    ironhermes_core::defcon::session_defcon().forget(session);

    // Level 3: the gate is asked. Denied -> not run; Approved -> run; no gate -> not run.
    let session = "sess-57-09-warn-3";
    set_level(session, defcon(3));
    let denied = RecordingGate::new(ApprovalOutcome::Denied);
    let (ran, results) = run_screened(
        &h,
        &make_fake(),
        session,
        "write_file",
        write("CANARY-MED"),
        Some(denied.clone()),
    )
    .await;
    assert_eq!(ran, 0, "Denied: the Warn does not run");
    assert_eq!(denied.asked().len(), 1, "the operator was asked once");
    assert!(
        results[0]
            .2
            .contains("The operator denied this write_file call"),
        "{}",
        results[0].2
    );

    let approved = RecordingGate::new(ApprovalOutcome::Approved);
    let (ran, _) = run_screened(
        &h,
        &make_fake(),
        session,
        "write_file",
        write("CANARY-MED"),
        Some(approved.clone()),
    )
    .await;
    assert_eq!(ran, 1, "Approved: the Warn runs");
    assert_eq!(approved.asked().len(), 1);
    assert!(
        rows_for(&h.audit_path, "security_verdict", "write_file")
            .iter()
            .any(|r| r["decision"] == "approved" && r["resolution"] == "operator"),
        "the operator's approval is recorded"
    );
    assert!(approved.kinds().contains(&SecurityEventKind::WarnRan));

    let (ran, results) = run_screened(
        &h,
        &make_fake(),
        session,
        "write_file",
        write("CANARY-MED"),
        None,
    )
    .await;
    assert_eq!(ran, 0, "no gate wired: a Warn fails closed");
    assert!(
        results[0].2.contains("no approval channel is available"),
        "{}",
        results[0].2
    );
    ironhermes_core::defcon::session_defcon().forget(session);
}

#[tokio::test]
async fn screened_once_per_call() {
    let h = Harness::new();
    let session = "sess-57-09-once";
    let terminal_args = serde_json::json!({ "command": "echo hello" });

    // A session-scoped gated intercept owns `terminal`: the registry screen must stay out.
    let fake = Arc::new(FakeBackend::new());
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_in_handler = ran.clone();
    let mut registry = ToolRegistry::new();
    registry.register_intercepted_or_replace(
        "terminal",
        session,
        ToolSchema::new(
            "terminal",
            "gated terminal",
            serde_json::json!({ "type": "object", "properties": {} }),
        ),
        Arc::new(move |_args: serde_json::Value| {
            let ran = ran_in_handler.clone();
            Box::pin(async move {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok::<String, anyhow::Error>("intercept-ran".to_string())
            })
        }),
    );
    registry.set_tool_screen(h.screen_over(&fake));
    let results = run_one_call(registry, session, "terminal", terminal_args.clone()).await;
    assert_eq!(ran.load(Ordering::SeqCst), 1, "{results:?}");
    assert!(
        fake.seen().is_empty(),
        "with a gated intercept the registry screen produces ZERO subjects, saw {:?}",
        fake.seen()
    );

    // No intercept (a cron or child registry): the screen judges it, exactly once.
    let fake = Arc::new(FakeBackend::new());
    let (ran, _) = run_screened(&h, &fake, session, "terminal", terminal_args, None).await;
    assert_eq!(ran, 1, "a benign command runs");
    assert_eq!(
        fake.seen().len(),
        1,
        "without an intercept the call is screened exactly once, saw {:?}",
        fake.seen()
    );
}

#[tokio::test]
async fn nudge_registry_is_screened() {
    let _h = Harness::new();
    let manager = ironhermes_agent::memory::factory::build_memory_manager(
        &ironhermes_core::config::MemoryConfig::default(),
    )
    .await
    .expect("memory manager builds")
    .expect("memory is enabled by default");

    let registry = ironhermes_agent::nudge::build_nudge_registry(manager);

    assert!(
        registry.tool_screen().is_some(),
        "the background memory-review agent's private registry must carry a screen"
    );
    assert_eq!(
        registry.list_tools(),
        vec!["memory"],
        "and it must still be the narrow memory-only registry"
    );
}

#[tokio::test]
async fn every_default_tool_has_a_screen_path() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new());
    let mut registry = ToolRegistry::new();
    registry.register_defaults();
    registry.register_skill_manage_tool();
    registry.set_tool_screen(h.screen_over(&fake));

    // Representative arguments that carry text under every key a default tool reads.
    let args = serde_json::json!({
        "q": "x", "query": "x", "path": "/tmp/p", "content": "x", "before": "a", "after": "b",
        "url": "http://example.com", "command": "echo hi", "code": "print(1)",
    });
    let names: Vec<String> = registry
        .list_tools()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        names.len() >= 8,
        "the default registry is populated: {names:?}"
    );
    for name in &names {
        let decision = registry
            .screen_call(name, &args, "sess-57-09-sweep", None)
            .await;
        let exempt = ironhermes_hooks::screen::SCREEN_EXEMPT_TOOLS.contains(&name.as_str());
        if exempt {
            assert_eq!(
                decision,
                ironhermes_hooks::ScreenDecision::Skip,
                "{name} is listed as exempt, so it must be Skip"
            );
        } else {
            assert_ne!(
                decision,
                ironhermes_hooks::ScreenDecision::Skip,
                "{name} has no screen path and is not in SCREEN_EXEMPT_TOOLS: a new tool must \
                 either be screened or be added to that list with a reason"
            );
        }
    }
    for must_be_exempt in ["todo_read", "read_file"] {
        assert!(
            ironhermes_hooks::screen::SCREEN_EXEMPT_TOOLS.contains(&must_be_exempt),
            "{must_be_exempt} carries no free text and is read-only: it belongs on the exempt list"
        );
    }
    ironhermes_core::defcon::session_defcon().forget("sess-57-09-sweep");
}

// ---------------------------------------------------------------------------
// Phase 57 Plan 22 (G-1): the agent loop's own refusals carry the no-circumvention text
// ---------------------------------------------------------------------------

/// A `terminal` stub behind a screen at level 3: the floor makes `curl` a Warn, so the call
/// needs the operator's approval. Returns whether the stub ran and the tool message.
async fn run_terminal_warn(
    h: &Harness,
    session: &str,
    gate: Option<Arc<dyn ApprovalGate>>,
) -> (usize, String) {
    set_level(session, defcon(3));
    let fake = Arc::new(FakeBackend::new());
    let (ran, results) = run_screened(
        h,
        &fake,
        session,
        "terminal",
        serde_json::json!({ "command": "curl http://example.com/x" }),
        gate,
    )
    .await;
    ironhermes_core::defcon::session_defcon().forget(session);
    assert_eq!(results.len(), 1, "exactly one tool message: {results:?}");
    (ran, results[0].2.clone())
}

#[tokio::test]
async fn screen_warn_refusal_forbids_circumvention() {
    let h = Harness::new();
    let (ran, text) = run_terminal_warn(&h, "sess-57-22-warn-nogate", None).await;
    assert_eq!(ran, 0, "an unapproved Warn never runs");
    assert_eq!(
        text,
        ironhermes_hooks::approval_refusal_text("terminal", &ApprovalOutcome::GateUnavailable)
            .unwrap()
    );
    assert!(
        text.ends_with(ironhermes_hooks::NO_CIRCUMVENTION_NOTICE),
        "{text}"
    );
}

#[tokio::test]
async fn screen_warn_operator_denial_forbids_circumvention() {
    let h = Harness::new();
    let gate = RecordingGate::new(ApprovalOutcome::Denied);
    let (ran, text) = run_terminal_warn(&h, "sess-57-22-warn-denied", Some(gate.clone())).await;
    assert_eq!(ran, 0, "an operator denial never runs the call");
    assert_eq!(gate.asked().len(), 1, "the operator was asked once");
    assert_eq!(
        text,
        ironhermes_hooks::approval_refusal_text("terminal", &ApprovalOutcome::Denied).unwrap()
    );
    assert!(text.starts_with("The operator denied this terminal call"));
    assert!(
        text.ends_with(ironhermes_hooks::NO_CIRCUMVENTION_NOTICE),
        "{text}"
    );
}

#[tokio::test]
async fn screen_block_text_forbids_circumvention() {
    let h = Harness::new();
    let fake = Arc::new(FakeBackend::new().block_if_contains("CANARY-BLOCK", Severity::Critical));
    let invoked = Arc::new(AtomicUsize::new(0));
    let mut registry = registry_with_stub("demo__write_note", &invoked);
    registry.set_tool_screen(h.screen_over(&fake));

    let results = run_one_call(
        registry,
        "sess-57-22-block",
        "demo__write_note",
        serde_json::json!({ "text": "CANARY-BLOCK" }),
    )
    .await;

    assert_eq!(invoked.load(Ordering::SeqCst), 0);
    assert_eq!(results.len(), 1, "{results:?}");
    let text = &results[0].2;
    assert!(
        text.contains("blocked by guardrail 'security-screen'"),
        "{text}"
    );
    assert!(text.contains("It did not run."), "{text}");
    assert!(
        text.ends_with(ironhermes_hooks::NO_CIRCUMVENTION_NOTICE),
        "{text}"
    );
}
