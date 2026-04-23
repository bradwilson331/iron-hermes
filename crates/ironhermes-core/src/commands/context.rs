use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::skills::SkillRegistry;
use crate::types::Platform;

// =============================================================================
// McpReloader trait — D-15 (trait-object form, avoids circular dep)
// =============================================================================

/// Result of an MCP reload operation (D-12).
///
/// The `failed` field carries `(server_name, error_message)` tuples sourced from
/// `ServerTaskResult.failure_reason` (Plan 03 data contract). Used by the REPL
/// loop to format partial failure messages per UI-SPEC.
pub struct McpReloadResult {
    pub connected: Vec<String>,
    /// `(server_name, sanitized_error)` — populated from `ServerTaskResult.failure_reason`.
    pub failed: Vec<(String, String)>,
    pub tool_count: usize,
}

/// Trait for reloading MCP server connections (D-15, trait-object form).
///
/// Defined in `ironhermes-core` to avoid a circular dependency with
/// `ironhermes-mcp`. `McpManager` implements this in `ironhermes-mcp`.
/// Follows the `MemoryManagerHandle` pattern from Phase 20.
#[async_trait::async_trait]
pub trait McpReloader: Send + Sync {
    /// Reload all MCP connections: disconnect, re-read config, reconnect, re-discover.
    /// Returns `McpReloadResult` with connected/failed server lists (D-12).
    async fn reload(&self) -> McpReloadResult;
    /// Return names of currently connected servers.
    fn connected_server_names(&self) -> Vec<String>;
    /// Count of registered MCP tools.
    async fn registered_tool_count(&self) -> usize;
}

// =============================================================================
// Snapshot traits — Plan 21.7-07 (D-03 / D-09 / D-17 / D-26)
// =============================================================================
//
// These trait objects keep `ironhermes-core` as a leaf crate (no dep on
// `ironhermes-agent` or `ironhermes-exec`). The concrete types live in their
// home crates and impl the relevant trait. Consumers (Plan 08 `/agents`,
// Plan 09 `hermes status`) read through these handles.

/// D-17: Budget snapshot readable from `hermes status` + advisory consumers.
/// Implemented in `ironhermes-agent` for `BudgetHandle`.
pub trait BudgetSnapshot: Send + Sync {
    /// Iterations consumed so far.
    fn iterations_used(&self) -> usize;
    /// Maximum iterations this budget was seeded with.
    fn iterations_max(&self) -> usize;
    /// Current pressure label: `"none" | "caution70" | "warning90" | "stop100"`.
    fn pressure_label(&self) -> &'static str;
}

/// D-03 / D-09: Subagent registry snapshot for `/agents list|kill|logs`.
/// Implemented in `ironhermes-agent` for `Arc<RwLock<SubagentRegistry>>`.
pub trait SubagentListSnapshot: Send + Sync {
    /// Count of active subagents (used for the `agents: N/M` pill).
    fn active_count(&self) -> usize;
    /// List of `(id, task_summary, uptime)` triples — read-consistent snapshot.
    fn list_summary(&self) -> Vec<(String, String, std::time::Duration)>;
    /// `/agents kill <id>` — returns true when id was present.
    fn kill(&self, id: &str) -> bool;
    /// `/agents logs <id>` — transcript file path for a registered subagent.
    fn transcript_path(&self, id: &str) -> Option<std::path::PathBuf>;
}

/// D-26: Process registry snapshot for `hermes status` sandbox view.
/// Implemented in `ironhermes-exec` for `Arc<RwLock<ProcessRegistry>>`.
pub trait ProcessRegistrySnapshotHandle: Send + Sync {
    /// Total tracked (running + finished within TTL) process count.
    fn tracked(&self) -> usize;
    /// JSON snapshot for structured `hermes status --json` output.
    fn snapshot_json(&self) -> serde_json::Value;
    /// Best-effort drain & kill of all tracked children for this session.
    fn drain_and_kill<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>;
}

// =============================================================================
// CommandContext
// =============================================================================

/// Context passed to every command handler.
///
/// Keeps ironhermes-core as a leaf crate by only including deps
/// that live in core itself. CLI and gateway extend context at
/// their integration layer before calling dispatch().
pub struct CommandContext {
    // Required — always available
    pub platform: Platform,
    pub session_id: String,
    pub agent_running: Arc<AtomicBool>,

    // Optional — platform-dependent or not always wired
    pub skill_registry: Option<Arc<SkillRegistry>>,
    /// MCP reload capability (D-15, trait object to avoid circular dep with ironhermes-mcp).
    pub mcp_reloader: Option<Arc<dyn McpReloader>>,

    // ---- Plan 21.7-07: Wave-2 handles consumed by Plan 08 (/agents) +
    // Plan 09 (hermes status). All trait-object form to keep ironhermes-core
    // a leaf crate (no back-dep on ironhermes-agent / ironhermes-exec).
    /// D-26: ProcessRegistry snapshot for `hermes status` sandbox view.
    pub process_registry: Option<Arc<dyn ProcessRegistrySnapshotHandle>>,
    /// D-03 / D-09: SubagentRegistry snapshot for `/agents list|kill|logs`
    /// and the `agents: N/M` status-line pill.
    pub subagent_registry: Option<Arc<dyn SubagentListSnapshot>>,
    /// D-17: BudgetHandle snapshot for `hermes status`.
    pub budget: Option<Arc<dyn BudgetSnapshot>>,
    /// D-09: Concurrency semaphore surfaced to handlers that read
    /// `.available_permits()` for capacity reporting.
    pub subagent_semaphore: Option<Arc<tokio::sync::Semaphore>>,
    /// D-04: Max concurrent subagents — denominator of the `agents: N/M` pill.
    pub max_subagents: Option<usize>,
}

impl CommandContext {
    /// Create a minimal context with all optional fields set to None.
    pub fn new(
        platform: Platform,
        session_id: String,
        agent_running: Arc<AtomicBool>,
    ) -> Self {
        Self {
            platform,
            session_id,
            agent_running,
            skill_registry: None,
            mcp_reloader: None,
            process_registry: None,
            subagent_registry: None,
            budget: None,
            subagent_semaphore: None,
            max_subagents: None,
        }
    }

    /// Builder: attach a skill registry.
    pub fn with_skill_registry(mut self, registry: Arc<SkillRegistry>) -> Self {
        self.skill_registry = Some(registry);
        self
    }

    /// Builder: attach an MCP reloader (D-15).
    pub fn with_mcp_reloader(mut self, reloader: Arc<dyn McpReloader>) -> Self {
        self.mcp_reloader = Some(reloader);
        self
    }

    /// Builder: attach a ProcessRegistry snapshot handle (D-26 / Plan 21.7-07).
    pub fn with_process_registry(
        mut self,
        pr: Arc<dyn ProcessRegistrySnapshotHandle>,
    ) -> Self {
        self.process_registry = Some(pr);
        self
    }

    /// Builder: attach a SubagentRegistry snapshot handle (D-03 / D-09 / Plan 21.7-07).
    pub fn with_subagent_registry(
        mut self,
        reg: Arc<dyn SubagentListSnapshot>,
    ) -> Self {
        self.subagent_registry = Some(reg);
        self
    }

    /// Builder: attach a BudgetHandle snapshot (D-17 / Plan 21.7-07).
    pub fn with_budget(mut self, budget: Arc<dyn BudgetSnapshot>) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Builder: attach the shared subagent concurrency semaphore
    /// (D-09 / Plan 21.7-07). Handlers read `.available_permits()`.
    pub fn with_subagent_semaphore(mut self, sem: Arc<tokio::sync::Semaphore>) -> Self {
        self.subagent_semaphore = Some(sem);
        self
    }

    /// Builder: attach the configured max subagent count (D-04 / Plan 21.7-07).
    /// Denominator of the `agents: N/M` status-line pill.
    pub fn with_max_subagents(mut self, max: usize) -> Self {
        self.max_subagents = Some(max);
        self
    }
}

#[cfg(test)]
mod plan_21_7_07_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn ctx() -> CommandContext {
        CommandContext::new(
            Platform::Local,
            "sess-1".to_string(),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[test]
    fn builders_default_to_none_for_plan_21_7_07_fields() {
        let c = ctx();
        assert!(c.process_registry.is_none());
        assert!(c.subagent_registry.is_none());
        assert!(c.budget.is_none());
        assert!(c.subagent_semaphore.is_none());
        assert!(c.max_subagents.is_none());
    }

    #[test]
    fn builders_set_four_new_fields() {
        let sem = Arc::new(tokio::sync::Semaphore::new(4));
        let c = ctx()
            .with_subagent_semaphore(sem.clone())
            .with_max_subagents(4);
        assert_eq!(
            c.subagent_semaphore.as_ref().map(|s| s.available_permits()),
            Some(4)
        );
        assert_eq!(c.max_subagents, Some(4));
    }

    // BudgetSnapshot impl for testing; real impl lives in ironhermes-agent.
    struct FakeBudget;
    impl BudgetSnapshot for FakeBudget {
        fn iterations_used(&self) -> usize {
            7
        }
        fn iterations_max(&self) -> usize {
            10
        }
        fn pressure_label(&self) -> &'static str {
            "caution70"
        }
    }

    #[test]
    fn with_budget_installs_trait_object() {
        let b: Arc<dyn BudgetSnapshot> = Arc::new(FakeBudget);
        let c = ctx().with_budget(b);
        let b = c.budget.as_ref().expect("budget should be Some");
        assert_eq!(b.iterations_used(), 7);
        assert_eq!(b.iterations_max(), 10);
        assert_eq!(b.pressure_label(), "caution70");
    }

    // SubagentListSnapshot fake for testing.
    struct FakeSubagentList;
    impl SubagentListSnapshot for FakeSubagentList {
        fn active_count(&self) -> usize {
            2
        }
        fn list_summary(&self) -> Vec<(String, String, std::time::Duration)> {
            vec![(
                "sub_deadbeef".into(),
                "do thing".into(),
                std::time::Duration::from_secs(3),
            )]
        }
        fn kill(&self, _id: &str) -> bool {
            true
        }
        fn transcript_path(&self, _id: &str) -> Option<std::path::PathBuf> {
            Some(std::path::PathBuf::from("/tmp/x.jsonl"))
        }
    }

    #[test]
    fn with_subagent_registry_installs_trait_object() {
        let r: Arc<dyn SubagentListSnapshot> = Arc::new(FakeSubagentList);
        let c = ctx().with_subagent_registry(r);
        let r = c.subagent_registry.as_ref().unwrap();
        assert_eq!(r.active_count(), 2);
        assert_eq!(r.list_summary().len(), 1);
        assert!(r.kill("sub_deadbeef"));
        assert!(r.transcript_path("sub_x").is_some());
    }

    // ProcessRegistrySnapshotHandle fake for testing.
    struct FakeProc;
    impl ProcessRegistrySnapshotHandle for FakeProc {
        fn tracked(&self) -> usize {
            1
        }
        fn snapshot_json(&self) -> serde_json::Value {
            serde_json::json!({"running": 1})
        }
        fn drain_and_kill<'a>(
            &'a self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>
        {
            Box::pin(async move { /* no-op */ })
        }
    }

    #[test]
    fn with_process_registry_installs_trait_object() {
        let p: Arc<dyn ProcessRegistrySnapshotHandle> = Arc::new(FakeProc);
        let c = ctx().with_process_registry(p);
        let p = c.process_registry.as_ref().unwrap();
        assert_eq!(p.tracked(), 1);
        assert_eq!(p.snapshot_json()["running"], 1);
    }
}
