//! Phase 41.3 Plan 04 (D-11 / D-12) — the `CommandContext` wiring-parity
//! divergence gate.
//!
//! Two halves, both required:
//! - **Runtime half** (this file, Task 1): `build_core_context` + `CoreContextHandles`
//!   + `missing_core_handles()` — proves the factory itself reports drift correctly.
//! - **Source-grep half** (this file, Task 3 addition): reads the four production
//!   build sites from disk and asserts each calls the factory, mirroring the
//!   existing in-repo self-check convention (`main.rs:5203-5212`, `runner.rs:2871-2930`).
//!
//! Fakes here mirror `tests/cmd_agents_and_stop.rs`'s `SubagentListSnapshot`
//! fixture style rather than inventing new mock idioms.

use std::path::PathBuf;
use std::sync::Arc;

use ironhermes_core::commands::context::{
    CommandContext, CoreContextHandles, McpReloader, McpReloadResult, ProcessRegistrySnapshotHandle,
    StateStoreHandle, SubagentListSnapshot, ToolsetSessionHandle, TrajectoryWriterHandle,
    build_core_context, CORE_CONTEXT_HANDLES,
};
use ironhermes_core::commands::handlers::dispatch;
use ironhermes_core::commands::registry::build_registry;
use ironhermes_core::commands::{CommandResult, CommandRouter};
use ironhermes_core::skills::SkillRegistry;
use ironhermes_core::types::Platform;
use ironhermes_core::workspace::Workspace;

// =============================================================================
// Fakes — one per core-handle trait, minimal, mirroring cmd_agents_and_stop.rs
// =============================================================================

struct FakeSubagents {
    entries: Vec<(String, String, std::time::Duration)>,
}
impl SubagentListSnapshot for FakeSubagents {
    fn active_count(&self) -> usize {
        self.entries.len()
    }
    fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
        self.entries.clone()
    }
    fn kill(&self, _id: &str) -> bool {
        false
    }
    fn transcript_path(&self, _id: &str) -> Option<PathBuf> {
        None
    }
}

struct FakeProc;
impl ProcessRegistrySnapshotHandle for FakeProc {
    fn tracked(&self) -> usize {
        0
    }
    fn snapshot_json(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    fn drain_and_kill<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {})
    }
}

struct FakeStateStore;
impl StateStoreHandle for FakeStateStore {
    fn list_sessions_text(&self, _limit: usize) -> String {
        String::new()
    }
    fn list_sessions_text_filtered(&self, _limit: usize, _workspace_root: Option<&str>) -> String {
        String::new()
    }
    fn history_text(&self, _session_id: &str) -> String {
        String::new()
    }
    fn export_session_text(&self, _session_id: &str) -> String {
        String::new()
    }
    fn update_title(&self, _session_id: &str, _title: &str) -> Result<(), String> {
        Ok(())
    }
    fn get_session_id(&self, _name_or_id: &str) -> Option<String> {
        None
    }
}

struct FakeToolsetSession;
impl ToolsetSessionHandle for FakeToolsetSession {
    fn enable_toolset(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn disable_toolset(&self, _name: &str) -> Result<(), String> {
        Ok(())
    }
    fn render_list(&self) -> String {
        String::new()
    }
    fn render_show(&self, _name: &str) -> Result<String, String> {
        Ok(String::new())
    }
}

struct FakeTrajectoryWriter;
impl TrajectoryWriterHandle for FakeTrajectoryWriter {
    fn append_json_line(&self, _line: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

struct FakeMcpReloader;
#[async_trait::async_trait]
impl McpReloader for FakeMcpReloader {
    async fn reload(&self) -> McpReloadResult {
        McpReloadResult {
            connected: vec![],
            failed: vec![],
            tool_count: 0,
        }
    }
    fn connected_server_names(&self) -> Vec<String> {
        vec![]
    }
    async fn registered_tool_count(&self) -> usize {
        0
    }
}

fn fake_workspace() -> Workspace {
    Workspace {
        root: PathBuf::from("/tmp/fake-parity-root"),
        soul_path: None,
        agents_chain: vec![],
        memory_dir: PathBuf::from("/tmp/fake-parity-root/.ironhermes/memory"),
        skills_dir: PathBuf::from("/tmp/fake-parity-root/skills"),
        tools_config: None,
    }
}

/// Every field populated — the "fully wired" fixture reused across tests.
fn full_handles() -> CoreContextHandles {
    CoreContextHandles {
        subagent_registry: Some(Arc::new(FakeSubagents {
            entries: vec![(
                "sub_parity01".to_string(),
                "parity fixture".to_string(),
                std::time::Duration::from_secs(1),
            )],
        })),
        process_registry: Some(Arc::new(FakeProc)),
        skill_registry: Some(Arc::new(SkillRegistry::load(&PathBuf::from(
            "/tmp/fake-parity-skills-nonexistent",
        )))),
        state_store: Some(Arc::new(FakeStateStore)),
        toolset_session: Some(Arc::new(FakeToolsetSession)),
        turn_registry: Some(Arc::new(ironhermes_core::concurrency::TurnRegistry::default())),
        workspace: Some(Arc::new(fake_workspace())),
        mcp_reloader: Some(Arc::new(FakeMcpReloader)),
        trajectory_writer: Some(Arc::new(FakeTrajectoryWriter)),
    }
}

fn build(handles: CoreContextHandles) -> CommandContext {
    build_core_context(Platform::Local, "parity-session".to_string(), handles)
}

// =============================================================================
// Task 1 behavior tests
// =============================================================================

#[test]
fn full_handles_leave_nothing_missing() {
    let ctx = build(full_handles());
    assert!(
        ctx.missing_core_handles().is_empty(),
        "fully wired context reported missing handles: {:?}",
        ctx.missing_core_handles()
    );
}

/// A single "omit this one field" case for `each_omitted_handle_is_named`.
type OmitCase = (&'static str, fn(&mut CoreContextHandles));

#[test]
fn each_omitted_handle_is_named() {
    // Nine parameterised cases: build the full set, blank exactly one field,
    // and assert missing_core_handles() names exactly that one handle.
    let cases: Vec<OmitCase> = vec![
        ("subagent_registry", |h| h.subagent_registry = None),
        ("process_registry", |h| h.process_registry = None),
        ("skill_registry", |h| h.skill_registry = None),
        ("state_store", |h| h.state_store = None),
        ("toolset_session", |h| h.toolset_session = None),
        ("turn_registry", |h| h.turn_registry = None),
        ("workspace", |h| h.workspace = None),
        ("mcp_reloader", |h| h.mcp_reloader = None),
        ("trajectory_writer", |h| h.trajectory_writer = None),
    ];
    assert_eq!(cases.len(), 9, "must cover exactly the nine core handles");

    for (name, omit) in cases {
        let mut handles = full_handles();
        omit(&mut handles);
        let ctx = build(handles);
        assert_eq!(
            ctx.missing_core_handles(),
            vec![name],
            "omitting only '{name}' should report exactly that one missing handle"
        );
    }
}

#[test]
fn core_handle_list_is_exactly_the_nine() {
    assert_eq!(
        CORE_CONTEXT_HANDLES,
        [
            "subagent_registry",
            "process_registry",
            "skill_registry",
            "state_store",
            "toolset_session",
            "turn_registry",
            "workspace",
            "mcp_reloader",
            "trajectory_writer",
        ],
        "CORE_CONTEXT_HANDLES must equal the nine D-12 names in their canonical order"
    );
}

#[test]
fn agents_command_reaches_a_wired_registry() {
    let ctx = build(full_handles());
    let cmd = build_registry()
        .into_iter()
        .find(|c| c.name == "agents")
        .expect("agents command must be registered");
    let router = CommandRouter::new(build_registry());
    let res = dispatch(&cmd, &[], &ctx, &router);
    match res {
        CommandResult::Output(s) => {
            assert!(
                !s.contains("Subagent registry not wired"),
                "factory-built context must reach the wired fake, not the fallback; got: {s}"
            );
            assert!(
                s.contains("sub_parity01"),
                "expected the fake's fixture id in the /agents output; got: {s}"
            );
        }
        other => panic!("expected Output, got {other:?}"),
    }
}

#[test]
fn surface_extras_do_not_affect_the_gate() {
    // Chain a surface-specific extra (personality_overlay) onto a fully-wired
    // context — the gate must still report nothing missing.
    struct FakePersonality;
    impl ironhermes_core::commands::context::PersonalityHandle for FakePersonality {
        fn get_preset(&self, _name: &str) -> Option<String> {
            None
        }
        fn list_presets(&self) -> Vec<String> {
            vec![]
        }
    }
    let ctx = build(full_handles()).with_personality_overlay(Arc::new(FakePersonality));
    assert!(
        ctx.missing_core_handles().is_empty(),
        "a surface-specific extra must not affect the core-handle gate; got: {:?}",
        ctx.missing_core_handles()
    );
}

// =============================================================================
// Task 3: source-grep half of the divergence gate
// =============================================================================

/// D-12: reads each of the four production `CommandContext` build sites from
/// disk and asserts each calls `build_core_context(`. Mirrors the in-repo
/// source-grep self-check convention already used at
/// `crates/ironhermes-cli/src/main.rs:5203-5212` and
/// `crates/ironhermes-gateway/src/runner.rs:2871-2930`.
///
/// This is the half of the gate that catches a surface that stops calling the
/// factory altogether; `each_omitted_handle_is_named` above (and
/// `web_core_handles_are_complete` in `iron_hermes_ui`'s own `#[cfg(test)]`
/// module) catch the complementary failure mode — a surface that calls the
/// factory with a half-populated struct.
#[test]
fn every_production_build_site_calls_the_factory() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/ironhermes-core -> crates -> repo root
    let repo_root = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .expect("CARGO_MANIFEST_DIR must be crates/ironhermes-core");

    let sites: &[&str] = &[
        "crates/ironhermes-cli/src/main.rs",
        "crates/ironhermes-cli/src/tui_rata/commands.rs",
        "crates/ironhermes-gateway/src/handler.rs",
        "crates/iron_hermes_ui/src/server/ws.rs",
    ];

    for site in sites {
        let path = repo_root.join(site);
        let source = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        assert!(
            source.contains("build_core_context("),
            "{} must call build_core_context( — D-11/D-12 require every production \
             surface to construct CommandContext through the shared factory",
            path.display()
        );
    }
}
