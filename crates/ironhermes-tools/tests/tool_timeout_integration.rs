//! Phase 41.3 Plan 01 Task 1 (D-01/D-02/D-03/D-05/D-06): tracer-slice
//! integration tests proving `ToolRegistry::execute_tool` is bounded end-to-end
//! against a **real hanging HTTP endpoint** — not a synthetic
//! `tokio::time::sleep` future (41.3-VALIDATION.md Anti-Self-Verification
//! Guard 1: a test asserting `timeout(d, sleep(d*2))` returns `Err` proves only
//! that `tokio::time::timeout` works, not that the dispatch tail is wrapped).

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ironhermes_core::{Config, ExecConfig, SubagentConfig, ToolSchema};
use ironhermes_tools::delegate_task::{ChildToolProgressCallback, DelegateTaskTool, SubagentRunner};
use ironhermes_tools::execute_code::ExecuteCodeTool;
use ironhermes_tools::fal::FalClient;
use ironhermes_tools::image_gen::ImageGenTool;
use ironhermes_tools::terminal::TerminalTool;
use ironhermes_tools::video_gen::{VideoAnimateTool, VideoGenerateTool};
use ironhermes_tools::video_to_video::VideoToVideoTool;
use ironhermes_tools::{Tool, ToolRegistry};
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A tool whose `execute()` performs a REAL `reqwest` GET against a wiremock
/// endpoint that never responds within the test's timeframe. Declares a 2s
/// budget via `Tool::timeout_secs()` so the test doesn't wait on the trait
/// default (60s) or the wiremock delay (30s).
struct HangingHttpTool {
    url: String,
}

#[async_trait]
impl Tool for HangingHttpTool {
    fn name(&self) -> &str {
        "hanging_http"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "test tool that performs a real HTTP GET against a slow endpoint"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "hanging_http",
            self.description(),
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn timeout_secs(&self) -> Option<u64> {
        Some(2)
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        let body = reqwest::get(&self.url).await?.text().await?;
        Ok(body)
    }
}

/// A tool that returns immediately, to prove fast tools are unaffected by the
/// D-01 wrap.
struct FastTool;

#[async_trait]
impl Tool for FastTool {
    fn name(&self) -> &str {
        "fast_tool"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "test tool that returns immediately"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "fast_tool",
            self.description(),
            json!({ "type": "object", "properties": {} }),
        )
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        Ok("fast payload, byte-for-byte".to_string())
    }
}

/// Test 1 (D-01): a tool whose HTTP call hangs for 30s is abandoned by
/// `execute_tool` at its resolved 2s budget, well under 30s, with the D-01
/// error string.
#[tokio::test(flavor = "multi_thread")]
async fn execute_tool_abandons_a_hanging_http_tool() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    let result = registry.execute_tool("hanging_http", json!({})).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "execute_tool must abandon the hanging call well under the 30s delay; took {elapsed:?}"
    );
    let err = result.expect_err("hanging tool must return Err on expiry");
    assert!(
        err.to_string().contains("timed out after 2s"),
        "error string must contain 'timed out after 2s'; got: {err}"
    );
}

/// Test 2 (D-02): after the hanging call returns `Err`, the mock server
/// recorded exactly one received request, and this test process completes
/// without the 30s delay elapsing — proving the in-flight request future was
/// actually dropped (socket released), not merely reported on.
#[tokio::test(flavor = "multi_thread")]
async fn execute_tool_releases_the_socket_on_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    let result = registry.execute_tool("hanging_http", json!({})).await;
    assert!(result.is_err());

    // wiremock's request-received counter is bumped on receipt, independent
    // of whether the response was ever consumed by the (dropped) client.
    let received = server.received_requests().await.expect("mock request log");
    assert_eq!(
        received.len(),
        1,
        "expected exactly one received request; got: {received:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(30),
        "test must complete without waiting out the 30s delay — proves the \
         in-flight future was dropped, not merely reported on"
    );
}

/// Test 3 (D-01 negative case): a tool that returns immediately still returns
/// `Ok` with its payload, byte-for-byte, unaffected by the timeout wrap.
#[tokio::test(flavor = "multi_thread")]
async fn execute_tool_lets_a_fast_tool_through_untouched() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FastTool));

    let result = registry
        .execute_tool("fast_tool", json!({}))
        .await
        .expect("fast tool must succeed");
    assert_eq!(result, "fast payload, byte-for-byte");
}

/// Test 4 (D-03): `timeout_count()` is 0 before, 1 after the hanging call,
/// and still 1 after a subsequent fast call — the counter increments only on
/// expiry.
#[tokio::test(flavor = "multi_thread")]
async fn timeout_counter_increments_only_on_expiry() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));
    registry.register(Box::new(FastTool));

    assert_eq!(registry.timeout_count(), 0, "counter must start at 0");

    let _ = registry.execute_tool("hanging_http", json!({})).await;
    assert_eq!(
        registry.timeout_count(),
        1,
        "counter must be 1 after the hanging call expires"
    );

    let _ = registry.execute_tool("fast_tool", json!({})).await;
    assert_eq!(
        registry.timeout_count(),
        1,
        "counter must stay at 1 after a subsequent fast (non-expiring) call"
    );
}

// ---------------------------------------------------------------------------
// Phase 41.3 Plan 02 Task 1 (D-01): the three remaining dispatch tails —
// `handle_tool_call`, `dispatch_with_hook`, and `dispatch` (which delegates to
// `dispatch_with_hook` with no tail of its own) — plus the `execute_tool`
// skills-rewrite fallback. All four now route through the single shared
// `run_bounded` helper extracted from Plan 01's `execute_tool` wrap.
// ---------------------------------------------------------------------------

/// A tool named "skills" whose `execute()` performs a REAL hanging HTTP call,
/// used to stand in for the real `SkillsTool` so the skills-rewrite tail's
/// wrap can be proven against a genuine hang rather than the instant-return
/// real skill activation.
struct HangingSkillsTool {
    url: String,
}

#[async_trait]
impl Tool for HangingSkillsTool {
    fn name(&self) -> &str {
        "skills"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "test stand-in for the skills tool that hangs on a real HTTP GET"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "skills",
            self.description(),
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn timeout_secs(&self) -> Option<u64> {
        Some(2)
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        let body = reqwest::get(&self.url).await?.text().await?;
        Ok(body)
    }
}

fn skill_md(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n{}",
        name, description, body
    )
}

/// Test 5 (D-01, Plan 02 Task 1): `handle_tool_call`'s tail is bounded by the
/// shared `run_bounded` wrap, same as `execute_tool`'s.
#[tokio::test(flavor = "multi_thread")]
async fn handle_tool_call_abandons_a_hanging_http_tool() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    let result = registry.handle_tool_call("hanging_http", json!({})).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "handle_tool_call must abandon the hanging call well under the 30s delay; took {elapsed:?}"
    );
    let err = result.expect_err("hanging tool must return Err on expiry");
    assert!(
        err.to_string().contains("timed out after 2s"),
        "error string must contain 'timed out after 2s'; got: {err}"
    );
}

/// Test 6 (D-01, Plan 02 Task 1): `dispatch_with_hook`'s tail is bounded.
#[tokio::test(flavor = "multi_thread")]
async fn dispatch_with_hook_abandons_a_hanging_http_tool() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    let result = registry
        .dispatch_with_hook("hanging_http", json!({}), None::<fn(&str, &str)>)
        .await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "dispatch_with_hook must abandon the hanging call well under the 30s delay; took {elapsed:?}"
    );
    let err = result.expect_err("hanging tool must return Err on expiry");
    assert!(
        err.to_string().contains("timed out after 2s"),
        "error string must contain 'timed out after 2s'; got: {err}"
    );
}

/// Test 7 (D-01, Plan 02 Task 1): `dispatch` — the entry point the realtime
/// tool-exec bridge (`iron_hermes_ui/src/server/api.rs:3513`/`:3767`) calls
/// directly — is bounded by inheritance through `dispatch_with_hook`'s
/// wrapped tail, proven directly rather than assumed.
#[tokio::test(flavor = "multi_thread")]
async fn dispatch_abandons_a_hanging_http_tool() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(HangingHttpTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    let result = registry.dispatch("hanging_http", json!({})).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "dispatch must abandon the hanging call well under the 30s delay; took {elapsed:?}"
    );
    let err = result.expect_err("hanging tool must return Err on expiry");
    assert!(
        err.to_string().contains("timed out after 2s"),
        "error string must contain 'timed out after 2s'; got: {err}"
    );
}

/// Test 8 (D-01, Plan 02 Task 1): an unknown tool name that resolves through
/// `resolve_skill_fallback` into a hanging `skills` tool still returns `Err`
/// at the budget — the skills-rewrite tail inside `execute_tool` is bounded,
/// with the budget resolved against `skills` (the tool actually executing).
#[tokio::test(flavor = "multi_thread")]
async fn skills_rewrite_tail_is_bounded() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    let skill_dir = skills_dir.join("arxiv");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        skill_md("arxiv", "search arxiv papers", "ARXIV_SKILL_BODY"),
    )
    .unwrap();

    let skill_registry = std::sync::Arc::new(ironhermes_core::SkillRegistry::load_with_paths(&[
        skills_dir,
    ]));
    let active_skills = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cred_dir = tempfile::tempdir().unwrap().keep();

    let mut registry = ToolRegistry::new();
    registry.register_skills_tool(
        skill_registry,
        active_skills,
        cred_dir,
        std::collections::HashMap::new(),
    );
    // Overwrite the real (instant-return) `SkillsTool` with a stand-in that
    // performs a genuine hanging HTTP call, so the wrap is proven against a
    // real hang rather than a synthetic sleep.
    registry.register(Box::new(HangingSkillsTool {
        url: server.uri(),
    }));

    let start = Instant::now();
    // "arxiv" is not a registered tool, but IS a known skill — resolves
    // through resolve_skill_fallback into the (now-hanging) "skills" tool.
    let result = registry.execute_tool("arxiv", json!({})).await;
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(30),
        "skills-rewrite tail must abandon the hanging call well under the 30s delay; took {elapsed:?}"
    );
    let err = result.expect_err("hanging skills stand-in must return Err on expiry");
    assert!(
        err.to_string().contains("timed out after 2s"),
        "error string must contain 'timed out after 2s'; got: {err}"
    );
}

// ---------------------------------------------------------------------------
// Phase 41.3 Plan 02 Task 2 (D-02/D-04): per-tool budget declarations and the
// real-subprocess hard-cancel proof. D-02 was revised mid-discussion from
// "report early, detach" to "kill the process" — the entire safety argument
// rests on every `ironhermes-exec` backend already setting
// `kill_on_drop(true)`. These tests prove the CONSEQUENCE (the child process
// never reaches its post-sleep side effect), not the mechanism itself.
// ---------------------------------------------------------------------------

/// A test-local tool whose `execute()` spawns a REAL OS subprocess via
/// `tokio::process::Command` with `kill_on_drop(true)` — the exact machinery
/// every `ironhermes-exec` backend already uses (D-02). No cooperative
/// interrupt flag, no `Drop` impl, no new trait hook: dropping the future
/// (via `tokio::time::timeout`'s `match` returning) is the whole mechanism.
struct SubprocessTool {
    budget_secs: u64,
    sleep_secs: u64,
    marker: std::path::PathBuf,
}

#[async_trait]
impl Tool for SubprocessTool {
    fn name(&self) -> &str {
        "subprocess_tool"
    }
    fn toolset(&self) -> &str {
        "test"
    }
    fn description(&self) -> &str {
        "test tool that spawns a real OS subprocess"
    }
    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "subprocess_tool",
            self.description(),
            json!({ "type": "object", "properties": {} }),
        )
    }
    fn timeout_secs(&self) -> Option<u64> {
        Some(self.budget_secs)
    }
    async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
        let marker = self.marker.display().to_string();
        let cmd = format!("sleep {}; touch {}", self.sleep_secs, marker);
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .kill_on_drop(true)
            .spawn()?;
        child.wait().await?;
        Ok("subprocess completed".to_string())
    }
}

/// Test (D-02): a tool whose declared 1s budget expires while its real child
/// process is still 7s away from `touch`ing the marker file. The child's
/// future is dropped at expiry; `kill_on_drop(true)` must reap the OS
/// process. Proof is an OBSERVABLE SIDE EFFECT (the marker file), not PID or
/// zombie-state inspection: a surviving child would have created it.
#[tokio::test(flavor = "multi_thread")]
async fn expired_tool_future_stops_its_os_child() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SubprocessTool {
        budget_secs: 1,
        sleep_secs: 8,
        marker: marker.clone(),
    }));

    let start = Instant::now();
    let result = registry.execute_tool("subprocess_tool", json!({})).await;
    let elapsed = start.elapsed();

    let err = result.expect_err("expired subprocess tool must return Err");
    assert!(
        err.to_string().contains("timed out after 1s"),
        "error string must contain 'timed out after 1s'; got: {err}"
    );
    assert!(
        elapsed < Duration::from_secs(8),
        "must abandon at the 1s budget, not the 8s sleep; took {elapsed:?}"
    );

    // D-02 proof: wait 10 real wall-clock seconds — well past the 8s sleep
    // the child would need to finish if it were still running — then assert
    // the marker was never created. This is NOT a synthetic sleep standing
    // in for the timeout mechanism; it is the test giving a REAL child every
    // chance to have survived, so absence of the marker proves the OS
    // process was actually killed, not merely abandoned by the Rust task.
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert!(
        !marker.exists(),
        "OS child must have been killed on future drop (kill_on_drop(true)) — \
         marker file must not exist"
    );
}

/// Control test (D-02): the same tool with a budget that comfortably covers
/// the child's 1s sleep returns `Ok`, and the child runs to completion and
/// creates the marker — proving the previous test's absence assertion is
/// about cancellation, not about the child failing to start or run at all.
#[tokio::test(flavor = "multi_thread")]
async fn unexpired_tool_future_lets_its_os_child_finish() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SubprocessTool {
        budget_secs: 20,
        sleep_secs: 1,
        marker: marker.clone(),
    }));

    let result = registry.execute_tool("subprocess_tool", json!({})).await;
    assert!(
        result.is_ok(),
        "unexpired subprocess tool must return Ok; got {result:?}"
    );
    assert!(
        marker.exists(),
        "child must have run to completion and created the marker — proves the \
         previous test's absence assertion is about cancellation, not about the \
         child never running"
    );
}

/// A `SubagentRunner` stand-in for `declared_budgets_snapshot` only —
/// `DelegateTaskTool::timeout_secs()` is a static override that never touches
/// the runner, so `run_child` is never invoked.
struct UnusedRunner;

#[async_trait]
impl SubagentRunner for UnusedRunner {
    async fn run_child(
        &self,
        _registry: Arc<tokio::sync::RwLock<ToolRegistry>>,
        _system_prompt: String,
        _max_iterations: usize,
        _model_override: Option<&str>,
        _cancel_token: Option<CancellationToken>,
        _tool_progress: Option<ChildToolProgressCallback>,
        _stale_warn_seconds: u64,
    ) -> anyhow::Result<Option<String>> {
        unreachable!("declared_budgets_snapshot never calls execute()")
    }
}

/// Test (D-04): constructs each of the seven tools this plan declares a
/// budget for (`WebExtractTool` is Plan 03's, declared alongside its D-16
/// per-URL deadline) and asserts its exact `timeout_secs()` value, so a
/// future edit that silently changes a budget fails this test.
#[test]
fn declared_budgets_snapshot() {
    assert_eq!(TerminalTool::new().timeout_secs(), None, "terminal");

    let delegate_task = DelegateTaskTool::new(
        Arc::new(UnusedRunner) as Arc<dyn SubagentRunner>,
        Arc::new(tokio::sync::Semaphore::new(1)),
        None,
        SubagentConfig::default(),
        None,
    );
    assert_eq!(delegate_task.timeout_secs(), None, "delegate_task");

    let video_generate = VideoGenerateTool::new(Arc::new(Config::default()), FalClient::new());
    assert_eq!(video_generate.timeout_secs(), Some(900), "video_generate");

    let video_animate = VideoAnimateTool::new(Arc::new(Config::default()), FalClient::new());
    assert_eq!(video_animate.timeout_secs(), Some(900), "video_animate");

    let video_to_video = VideoToVideoTool::new(Arc::new(Config::default()), FalClient::new());
    assert_eq!(video_to_video.timeout_secs(), Some(900), "video_to_video");

    let image_gen = ImageGenTool::new(Arc::new(Config::default()), FalClient::new());
    assert_eq!(image_gen.timeout_secs(), Some(300), "image_gen");

    let execute_code = ExecuteCodeTool::new(Arc::new(ToolRegistry::new()), ExecConfig::default(), None);
    assert_eq!(execute_code.timeout_secs(), Some(360), "execute_code");
}
