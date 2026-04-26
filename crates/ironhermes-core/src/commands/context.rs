use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::skills::SkillRegistry;
use crate::types::{ChatMessage, Platform};

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
// CronJobReader trait — Phase 22.4.2.1 Plan 01 (circular-dep-safe trait)
// =============================================================================

/// Trait for reading cron job state from the slash command handler.
/// Defined in ironhermes-core to avoid circular dep with ironhermes-cron
/// (ironhermes-cron depends on ironhermes-core; adding JobStore here would
/// be circular). Same topology as McpReloader (core/defined, gateway/impl).
pub trait CronJobReader: Send + Sync {
    fn list_jobs_text(&self) -> String;
    fn get_job_text(&self, id_or_name: &str) -> Option<String>;
    fn status_text(&self) -> String;
    fn pause_job(&self, id_or_name: &str) -> Result<String, String>;
    fn resume_job(&self, id_or_name: &str) -> Result<String, String>;
    fn remove_job(&self, id_or_name: &str) -> Result<String, String>;
    fn queue_run(&self, id_or_name: &str) -> Result<String, String>;
}

// =============================================================================
// Phase 22.4.2 Plan 00 — D-04 handle traits
// =============================================================================
//
// These trait objects extend `CommandContext` with handles for the eight new
// subsystems (D-04). All traits are `Send + Sync` and defined here in core
// to keep `ironhermes-core` a leaf crate. Concrete impls live in their home
// crates and implement the relevant trait. Pattern mirrors `McpReloader`,
// `SubagentListSnapshot`, `ProcessRegistrySnapshotHandle`, `BudgetSnapshot`.

/// Handle for McpManager full server enumeration (D-04 — separate from
/// `McpReloader` which only handles reload operations).
pub trait McpManagerHandle: Send + Sync {
    /// Names of currently connected MCP servers.
    fn connected_server_names(&self) -> Vec<String>;
}

/// Handle for MemoryManager status inspection (D-04).
pub trait MemoryManagerHandle: Send + Sync {
    /// Returns a formatted status block showing active memory context.
    /// Async work is bridged via `block_in_place + block_on` at the call site.
    fn status_text(&self) -> String;
}

/// Handle for StateStore session operations (D-04).
/// All methods are synchronous (rusqlite) — callers run inside `block_in_place`.
pub trait StateStoreHandle: Send + Sync {
    /// List recent sessions (up to `limit`). Returns formatted text.
    fn list_sessions_text(&self, limit: usize) -> String;
    /// Get history for `session_id` as formatted text.
    fn history_text(&self, session_id: &str) -> String;
    /// Export session as formatted text.
    fn export_session_text(&self, session_id: &str) -> String;
    /// Update session title. Returns Ok(()) or an error message.
    fn update_title(&self, session_id: &str, title: &str) -> Result<(), String>;
    /// Get a session by name or id. Returns `Some(session_id)` when found.
    fn get_session_id(&self, name_or_id: &str) -> Option<String>;
}

/// Handle for ProviderResolver status and model information (D-04).
pub trait ProviderResolverHandle: Send + Sync {
    /// Current provider name.
    fn main_provider(&self) -> String;
    /// Current model name.
    fn main_model(&self) -> String;
    /// Formatted status text for `/provider`.
    /// V8.1: MUST NOT include api_key in the returned string.
    fn status_text(&self) -> String;
    /// Validate a model string. Returns Ok(model_name) or Err(message).
    fn validate_model(&self, model: &str) -> Result<String, String>;
    /// Model listing text for `/model` with no args.
    fn model_list_text(&self) -> String;
    /// Resolve the "fast" role. Returns Some(model_name) if a fast preset is configured,
    /// None if no fast role exists in the config.
    fn fast_role_model(&self) -> Option<String>;
}

/// Handle for ContextCompressor operations (D-04).
pub trait ContextCompressorHandle: Send + Sync {
    /// Trigger compression. Returns a status message.
    /// Async work is bridged via `block_in_place + block_on` at the call site.
    fn compress_text(&self) -> String;
    /// Status info for `/compress` with no args.
    fn status_text(&self) -> String;
}

/// Handle for PersonalityRegistry (D-04).
pub trait PersonalityHandle: Send + Sync {
    /// Apply a named preset. Returns `Some(overlay_text)` if found.
    fn get_preset(&self, name: &str) -> Option<String>;
    /// List all available preset names.
    fn list_presets(&self) -> Vec<String>;
}

/// Handle for AgentLoop spawn paths (D-04 — Tier D session control).
pub trait AgentLoopHandle: Send + Sync {
    /// Returns true if an agent turn is currently running.
    fn is_running(&self) -> bool;
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

    // ---- Phase 22.4.2 Plan 00: D-04 eight new optional handles.
    // All Option<Arc<...>> for backwards-compat — None when not wired.
    // Guard pattern (D-05): every handler checks .is_some() before use.

    /// D-04: McpManager handle for `/mcp` full server enumeration.
    /// Separate from `mcp_reloader` (which only handles reload).
    pub mcp_manager: Option<Arc<dyn McpManagerHandle>>,
    /// D-04: MemoryManager handle for `/memory` status inspection.
    pub memory_manager: Option<Arc<dyn MemoryManagerHandle>>,
    /// D-04: StateStore handle for `/sessions` `/resume` `/save` `/history` `/title`.
    pub state_store: Option<Arc<dyn StateStoreHandle>>,
    /// D-04: ProviderResolver handle for `/model` `/provider` `/fast`.
    pub provider_resolver: Option<Arc<dyn ProviderResolverHandle>>,
    /// D-04: ContextCompressor handle for `/compress` `/rollback`.
    pub context_compressor: Option<Arc<dyn ContextCompressorHandle>>,
    /// D-04: PersonalityRegistry handle for `/personality`.
    pub personality_overlay: Option<Arc<dyn PersonalityHandle>>,
    /// D-04: History snapshot for `/history` `/retry` `/undo` `/rollback`.
    /// Populated with a clone snapshot at `build_command_context` time.
    /// Mutations (for `/retry`, `/undo`, `/rollback`) apply in the post-router hook.
    pub history: Option<Arc<std::sync::RwLock<Vec<ChatMessage>>>>,
    /// D-04: AgentLoop handle for Tier D session control spawn paths.
    pub agent_loop: Option<Arc<dyn AgentLoopHandle>>,

    /// Phase 22.4.2.1 Plan 01: CronJobReader handle for `/cron` slash UI.
    /// Option<Arc<dyn>> to avoid circular dep with ironhermes-cron.
    pub cron_store: Option<Arc<dyn CronJobReader>>,
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
            // Phase 22.4.2 Plan 00: D-04 eight new optional handles.
            mcp_manager: None,
            memory_manager: None,
            state_store: None,
            provider_resolver: None,
            context_compressor: None,
            personality_overlay: None,
            history: None,
            agent_loop: None,
            // Phase 22.4.2.1 Plan 01: CronJobReader for /cron slash UI.
            cron_store: None,
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

    // ---- Phase 22.4.2 Plan 00: D-04 eight new builder methods.

    /// Builder: attach McpManager handle for `/mcp` server enumeration (D-04).
    pub fn with_mcp_manager(mut self, mgr: Arc<dyn McpManagerHandle>) -> Self {
        self.mcp_manager = Some(mgr);
        self
    }

    /// Builder: attach MemoryManager handle for `/memory` status (D-04).
    pub fn with_memory_manager(mut self, mgr: Arc<dyn MemoryManagerHandle>) -> Self {
        self.memory_manager = Some(mgr);
        self
    }

    /// Builder: attach StateStore handle for session operations (D-04).
    pub fn with_state_store(mut self, store: Arc<dyn StateStoreHandle>) -> Self {
        self.state_store = Some(store);
        self
    }

    /// Builder: attach ProviderResolver handle for `/model` `/provider` `/fast` (D-04).
    pub fn with_provider_resolver(mut self, resolver: Arc<dyn ProviderResolverHandle>) -> Self {
        self.provider_resolver = Some(resolver);
        self
    }

    /// Builder: attach ContextCompressor handle for `/compress` `/rollback` (D-04).
    pub fn with_context_compressor(mut self, engine: Arc<dyn ContextCompressorHandle>) -> Self {
        self.context_compressor = Some(engine);
        self
    }

    /// Builder: attach PersonalityRegistry handle for `/personality` (D-04).
    pub fn with_personality_overlay(mut self, registry: Arc<dyn PersonalityHandle>) -> Self {
        self.personality_overlay = Some(registry);
        self
    }

    /// Builder: attach a history snapshot for `/history` `/retry` `/undo` (D-04).
    /// Pass a clone snapshot; mutations apply in the tui_rata post-router hook.
    pub fn with_history(
        mut self,
        history: Arc<std::sync::RwLock<Vec<ChatMessage>>>,
    ) -> Self {
        self.history = Some(history);
        self
    }

    /// Builder: attach AgentLoop handle for Tier D session control (D-04).
    pub fn with_agent_loop(mut self, agent_loop: Arc<dyn AgentLoopHandle>) -> Self {
        self.agent_loop = Some(agent_loop);
        self
    }

    /// Builder: attach a CronJobReader handle for `/cron` slash UI (Phase 22.4.2.1 Plan 01).
    pub fn with_cron_store(mut self, store: Arc<dyn CronJobReader>) -> Self {
        self.cron_store = Some(store);
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
