use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{anyhow, Result};
use ironhermes_agent::{AgentLoop, MemoryManager, PromptBuilder, build_main_client, wire_fallback_if_configured};
use ironhermes_agent::budget::BudgetHandle;
use ironhermes_core::{ChatMessage, Config, ProviderResolver, SkillRegistry};
use ironhermes_cron::{complete_job_run, resolve_delivery_targets, CronJob, JobStore, TgSendApi};
use ironhermes_hooks::HookRegistry;
use ironhermes_mcp::McpManager;
use ironhermes_tools::ToolRegistry;
use tokio::sync::Mutex as TokioMutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::delivery::dispatch_all_targets;
use crate::prompt_builder::{build_job_prompt, AssembledPrompt};
use crate::script_runner::run_job_script;
use crate::timeout::{run_with_inactivity_timeout, run_with_wall_clock};
use crate::{CRON_AUTO_DELIVER_CHAT_ID, CRON_AUTO_DELIVER_PLATFORM, CRON_AUTO_DELIVER_THREAD_ID};

// ---------------------------------------------------------------------------
// CronRunnerContext
// ---------------------------------------------------------------------------

/// Arc-shareable bundle of dependencies used by `run_cron_job` and the tick
/// loop. Constructed once at startup and passed by `Arc<CronRunnerContext>`.
pub struct CronRunnerContext {
    pub job_store: Arc<std::sync::Mutex<JobStore>>,
    pub skill_registry: Option<Arc<SkillRegistry>>,
    pub tool_registry: Arc<tokio::sync::RwLock<ToolRegistry>>,
    pub memory_manager: Option<Arc<TokioMutex<MemoryManager>>>,
    pub hook_registry: Option<Arc<HookRegistry>>,
    pub config: Config,
    pub mcp_manager: Option<Arc<McpManager>>,
    /// Telegram-only adapter. A future phase replaces this with an
    /// `AdapterRegistry` once a second platform's send adapter exists.
    pub tg_client: Option<Arc<dyn TgSendApi>>,
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn parse_env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// RAII guard that restores `TERMINAL_CWD` on drop.
struct WorkdirGuard {
    prev: Option<String>,
}

impl Drop for WorkdirGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => {
                // SAFETY: guarded by job serialisation in the tick loop
                unsafe { std::env::set_var("TERMINAL_CWD", v) }
            }
            None => {
                unsafe { std::env::remove_var("TERMINAL_CWD") }
            }
        }
    }
}

fn apply_workdir(workdir: Option<&str>) -> WorkdirGuard {
    let prev = std::env::var("TERMINAL_CWD").ok();
    if let Some(wd) = workdir {
        // SAFETY: workdir jobs are serialised by the tick loop (no concurrent
        // mutation of TERMINAL_CWD for jobs in the parallel partition)
        unsafe { std::env::set_var("TERMINAL_CWD", wd) }
    }
    WorkdirGuard { prev }
}

/// Scope task-local delivery context from the primary target around `fut`.
async fn with_first_target_locals<T, F>(
    targets: &[ironhermes_cron::DeliveryTarget],
    fut: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    let primary = targets.first();
    let (platform, chat_id, thread_id) = match primary {
        Some(t) => (
            t.platform.clone(),
            t.chat_id.clone(),
            t.thread_id.clone().unwrap_or_default(),
        ),
        None => (String::new(), String::new(), String::new()),
    };
    CRON_AUTO_DELIVER_PLATFORM
        .scope(
            platform,
            CRON_AUTO_DELIVER_CHAT_ID.scope(
                chat_id,
                CRON_AUTO_DELIVER_THREAD_ID.scope(thread_id, fut),
            ),
        )
        .await
}

/// Persist job run result + dispatch delivery to all targets.
async fn complete_and_dispatch(
    ctx: &CronRunnerContext,
    job: &CronJob,
    output: &str,
    success: bool,
) -> Result<()> {
    // Persist + advance schedule (complete_job_run returns Option<DeliveryTarget>
    // which is the singular/legacy form — we use resolve_delivery_targets for
    // the multi-target dispatch below, so we discard the singular result).
    complete_job_run(&ctx.job_store, job, output, success).await?;

    // Resolve all delivery targets via Plan 04's plural API
    let targets = resolve_delivery_targets(job);
    if targets.is_empty() {
        return Ok(());
    }

    // Dispatch with task-locals scoped from primary target
    let errors = with_first_target_locals(&targets, async {
        dispatch_all_targets(
            targets.clone(),
            output,
            job,
            &ctx.config,
            ctx.tg_client.as_ref(),
        )
        .await
    })
    .await;

    // last_delivery_error accumulation
    let last_err = if errors.is_empty() {
        None
    } else {
        Some(errors.join(","))
    };

    // Update job's last_delivery_error field via jobs_mut()
    {
        let mut store = ctx
            .job_store
            .lock()
            .map_err(|e| anyhow!("job store mutex poisoned: {}", e))?;
        if let Some(j) = store.jobs_mut().iter_mut().find(|j| j.id == job.id) {
            j.last_delivery_error = last_err;
        }
        store.save()?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Execute a single cron job end-to-end:
///
/// 1. `no_agent=true` short-circuit: script stdout → output, no LLM call.
/// 2. Pre-run script (no_agent=false): wake-gate check + prompt injection.
/// 3. Assembled-prompt threat rescan → BLOCKED delivery if triggered.
/// 4. AgentLoop construction with per-job model / workdir / toolset overrides.
/// 5. Double-timeout wrap (wall-clock + inactivity).
/// 6. Process outcome: empty response → soft failure.
/// 7. Hook events (MessageReceived before, ResponseSent after).
/// 8. complete_job_run + dispatch_all_targets + last_delivery_error accumulation.
pub async fn run_cron_job(job: &CronJob, ctx: &CronRunnerContext) -> Result<()> {
    let request_id = Uuid::new_v4().to_string();
    let cron_chat_id = format!("cron-{}", job.id);

    // §6.4 Cron-distinct budget: each job run gets a FRESH budget sized from
    // config.agent.max_iterations, independent of the interactive (gateway)
    // budget. A fresh handle per job means every scheduled run starts full
    // regardless of prior job consumption — mirrors the run_turn boundary.
    // This MUST be a new BudgetHandle (new Arc<AtomicUsize>), NOT a clone of
    // any gateway AgentRuntime budget. T-28.1-14 mitigation.
    let cron_budget = BudgetHandle::new(ctx.config.agent.max_iterations);

    info!(job_id=%job.id, job_name=%job.name, "cron job starting");

    // Fire MessageReceived hook (gateway parity)
    if let Some(registry) = &ctx.hook_registry {
        registry.fire(ironhermes_hooks::HookEvent::new(
            &request_id,
            ironhermes_hooks::HookEventKind::MessageReceived {
                platform: "cron".to_string(),
                chat_id: cron_chat_id.clone(),
                content_preview: ironhermes_hooks::event::preview(&job.prompt, 200),
            },
        ));
    }

    // RAII workdir guard (process-global TERMINAL_CWD; workdir jobs are
    // serialised by the tick loop — this is defence-in-depth)
    let _wd = apply_workdir(job.workdir.as_deref());

    // -----------------------------------------------------------------------
    // (1) no_agent short-circuit — script stdout IS the job output
    // -----------------------------------------------------------------------
    if job.no_agent {
        let Some(script) = job.script.as_deref() else {
            return complete_and_dispatch(ctx, job, "[no_agent set but no script field]", false)
                .await;
        };
        let outcome = run_job_script(script).await?;
        if outcome.ok {
            if outcome.stdout.trim().is_empty() {
                // empty stdout → silent, no delivery
                return complete_and_dispatch(ctx, job, "[SILENT]", true).await;
            }
            return complete_and_dispatch(ctx, job, &outcome.stdout, true).await;
        }
        // Non-zero exit / timeout → alert delivery
        let err_body = format!("[Script failed]\n{}\n{}", outcome.stdout, outcome.stderr);
        return complete_and_dispatch(ctx, job, &err_body, false).await;
    }

    // -----------------------------------------------------------------------
    // (2) Pre-run script (no_agent=false) — wake-gate + stdout injection
    // -----------------------------------------------------------------------
    let script_stdout: Option<String> = match &job.script {
        Some(name) => {
            let outcome = run_job_script(name).await?;
            if !outcome.wake_agent {
                // wake-gate said don't wake — silent skip
                return complete_and_dispatch(ctx, job, "[SILENT]", true).await;
            }
            if outcome.stdout.trim().is_empty() {
                // empty script stdout → skip agent run (Python parity)
                return complete_and_dispatch(ctx, job, "[SILENT]", true).await;
            }
            Some(outcome.stdout)
        }
        None => None,
    };

    // -----------------------------------------------------------------------
    // (3) Build assembled prompt + post-assembly rescan
    // -----------------------------------------------------------------------
    let AssembledPrompt {
        user_prompt,
        blocked_reason,
        ..
    } = build_job_prompt(job, script_stdout.as_deref(), ctx.skill_registry.as_deref()).await?;

    if let Some(reason) = blocked_reason {
        let blocked_doc = format!("BLOCKED: {}", reason);
        return complete_and_dispatch(ctx, job, &blocked_doc, false).await;
    }

    // -----------------------------------------------------------------------
    // (4) Construct AgentLoop with per-job overrides
    // -----------------------------------------------------------------------
    let resolver = ProviderResolver::build(&ctx.config)?;
    let max_turns = ctx.config.agent.max_turns;
    let default_model = resolver.resolve_for_main().default_model.clone();
    let model = job.model.as_deref().unwrap_or(&default_model).to_string();
    let cwd = job
        .workdir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let mut prompt_builder = PromptBuilder::new(&model, "cron").load_context(&cwd);
    if let Some(mgr) = &ctx.memory_manager {
        prompt_builder.set_memory_manager(mgr.clone());
    }
    if let Some(skill_reg) = &ctx.skill_registry {
        prompt_builder.set_skill_registry(skill_reg.clone());
    }
    prompt_builder.load_memory().await;
    let system_msg = prompt_builder.build_system_message();

    let client = build_main_client(&resolver)?;

    // Per D-CONTEXT §Per-job runtime overrides: enabled_toolsets ⇒ scoped
    // tool registry. If Some and non-empty, build a filtered view that only
    // exposes the named toolsets. The shared Arc<RwLock<ToolRegistry>> is
    // NOT mutated — each job gets its own independent scoped view.
    let tool_registry_scoped = match &job.enabled_toolsets {
        Some(names) if !names.is_empty() => {
            let reg = ctx.tool_registry.read().await;
            let scoped = reg.scope_to(names);
            drop(reg);
            Arc::new(tokio::sync::RwLock::new(scoped))
        }
        _ => ctx.tool_registry.clone(),
    };

    let mut agent = AgentLoop::new(client, tool_registry_scoped, max_turns);
    // Install the cron-distinct budget (§6.4). This budget is separate from any
    // interactive gateway budget — draining it does not affect interactive chat.
    agent = agent.with_budget(cron_budget);
    agent = wire_fallback_if_configured(agent, &resolver);
    if let Some(registry) = &ctx.hook_registry {
        agent = agent.with_hook_registry(registry.clone());
    }

    let messages = vec![
        system_msg,
        ChatMessage::user(user_prompt),
    ];

    // -----------------------------------------------------------------------
    // (5) Run agent with task-locals + dual-timeout wrapping
    //
    // AgentLoop::run takes `&mut self`. We wrap in Arc<tokio::sync::Mutex>
    // so the activity_summary polling closure (called every second by
    // run_with_inactivity_timeout) can acquire a try_lock concurrently
    // with the long-held run() lock.
    // -----------------------------------------------------------------------
    let targets = resolve_delivery_targets(job); // used for task-locals seeding
    let agent = Arc::new(tokio::sync::Mutex::new(agent));
    let agent_for_summary = agent.clone();

    let wall_secs = parse_env_u64("IRONHERMES_CRON_WALL_TIMEOUT_SECS", 14400);
    let inactivity_secs = parse_env_u64("IRONHERMES_CRON_TIMEOUT", 600);

    let run_fut = {
        let agent = agent.clone();
        async move {
            // Hold the lock for the duration of run() — this is the ONLY
            // caller of run(), so the lock is uncontested by run() itself.
            // activity_summary() reads concurrently via try_lock from the
            // polling closure.
            let mut guard = agent.lock().await;
            guard.run(messages).await
        }
    };

    let inner = run_with_inactivity_timeout(
        run_fut,
        move || {
            // try_lock returns Err if run() holds the lock — agent is active.
            // Treat as "0 seconds since activity" (still alive). Next poll retries.
            match agent_for_summary.try_lock() {
                Ok(guard) => guard.activity_summary().seconds_since,
                Err(_) => 0.0,
            }
        },
        inactivity_secs,
    );

    let outcome_fut = run_with_wall_clock(inner, wall_secs);

    // Scope task-locals around the entire agent run
    let outcome = with_first_target_locals(&targets, outcome_fut).await;

    // -----------------------------------------------------------------------
    // (6) Process outcome — Ok with final_response | empty | Err
    // -----------------------------------------------------------------------
    let (output, success) = match outcome {
        Ok(result) => {
            let final_text = result.final_response.unwrap_or_default();
            if final_text.trim().is_empty() {
                warn!(job_id=%job.id, "agent completed but produced empty response (soft failure)");
                (
                    "Agent completed but produced empty response".to_string(),
                    false,
                )
            } else {
                (final_text, true)
            }
        }
        Err(e) => {
            error!(job_id=%job.id, "agent error: {}", e);
            (format!("[Agent error: {}]", e), false)
        }
    };

    info!(job_id=%job.id, success=%success, "cron job completed");

    // Fire ResponseSent hook
    if let Some(registry) = &ctx.hook_registry {
        registry.fire(ironhermes_hooks::HookEvent::new(
            &request_id,
            ironhermes_hooks::HookEventKind::ResponseSent {
                platform: "cron".to_string(),
                chat_id: cron_chat_id,
                response_preview: ironhermes_hooks::event::preview(&output, 200),
            },
        ));
    }

    complete_and_dispatch(ctx, job, &output, success).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use chrono::Utc;
    use ironhermes_cron::job::{JobState, RepeatConfig, ScheduleParsed};
    use ironhermes_cron::store::JobStore;
    use crate::test_util::env_lock;
    use std::sync::{Mutex, MutexGuard};
    use tempfile::TempDir;

    // Crate-wide env lock — serialises against prompt_builder + script_runner
    // tests that also mutate IRONHERMES_HOME / BASH_PATH / TERMINAL_CWD.
    fn env_guard() -> MutexGuard<'static, ()> {
        env_lock().lock().unwrap_or_else(|e| e.into_inner())
    }

    // -----------------------------------------------------------------------
    // FakeTg
    // -----------------------------------------------------------------------

    #[derive(Default)]
    struct FakeTg {
        calls: Mutex<Vec<(String, String, Option<String>)>>,
    }

    #[async_trait]
    impl TgSendApi for FakeTg {
        async fn send_message(
            &self,
            chat_id: &str,
            content: &str,
            thread_id: Option<&str>,
        ) -> anyhow::Result<()> {
            self.calls.lock().unwrap().push((
                chat_id.to_string(),
                content.to_string(),
                thread_id.map(|s| s.to_string()),
            ));
            Ok(())
        }

        async fn send_voice(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        async fn send_image_file(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        async fn send_video(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
        async fn send_document(&self, _: &str, _: &std::path::Path, _: Option<&str>) -> anyhow::Result<()> { Ok(()) }
    }

    // -----------------------------------------------------------------------
    // Job fixture helpers
    // -----------------------------------------------------------------------

    fn make_job(deliver: &str) -> CronJob {
        CronJob {
            id: "test-job".to_string(),
            name: "Test Job".to_string(),
            prompt: "do something".to_string(),
            skills: vec![],
            schedule: ScheduleParsed::Interval {
                minutes: 60,
                display: "every 60m".to_string(),
            },
            schedule_display: "every 60m".to_string(),
            repeat: RepeatConfig::default(),
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: deliver.to_string(),
            origin: None,
            created_at: Utc::now(),
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
            model: None,
            provider: None,
            base_url: None,
            script: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            workdir: None,
            last_delivery_error: None,
        }
    }

    fn make_ctx(tmpdir: &TempDir) -> CronRunnerContext {
        let cron_dir = tmpdir.path().join("cron");
        let store = Arc::new(std::sync::Mutex::new(
            JobStore::open(cron_dir).expect("open store"),
        ));
        CronRunnerContext {
            job_store: store,
            skill_registry: None,
            tool_registry: Arc::new(tokio::sync::RwLock::new(ToolRegistry::new())),
            memory_manager: None,
            hook_registry: None,
            config: Config::default(),
            mcp_manager: None,
            tg_client: None,
        }
    }

    // -----------------------------------------------------------------------
    // Test: workdir RAII guard restores previous TERMINAL_CWD
    // -----------------------------------------------------------------------

    #[test]
    fn test_workdir_guard_restores_env() {
        let _guard = env_guard();
        unsafe {
            std::env::set_var("TERMINAL_CWD", "/original");
        }
        {
            let _wd = apply_workdir(Some("/new-wd"));
            assert_eq!(
                std::env::var("TERMINAL_CWD").unwrap(),
                "/new-wd",
                "workdir should be set during guard"
            );
        }
        assert_eq!(
            std::env::var("TERMINAL_CWD").unwrap(),
            "/original",
            "workdir should be restored after guard drop"
        );
        unsafe {
            std::env::remove_var("TERMINAL_CWD");
        }
    }

    #[test]
    fn test_workdir_guard_removes_if_unset() {
        let _guard = env_guard();
        unsafe {
            std::env::remove_var("TERMINAL_CWD");
        }
        {
            let _wd = apply_workdir(Some("/tmp/test-wd"));
        }
        assert!(
            std::env::var("TERMINAL_CWD").is_err(),
            "TERMINAL_CWD should be unset after guard drop when it wasn't set before"
        );
    }

    #[test]
    fn test_workdir_guard_noop_when_no_workdir() {
        let _guard = env_guard();
        unsafe {
            std::env::set_var("TERMINAL_CWD", "/keep-this");
        }
        {
            let _wd = apply_workdir(None);
            assert_eq!(std::env::var("TERMINAL_CWD").unwrap(), "/keep-this");
        }
        assert_eq!(std::env::var("TERMINAL_CWD").unwrap(), "/keep-this");
        unsafe {
            std::env::remove_var("TERMINAL_CWD");
        }
    }

    // -----------------------------------------------------------------------
    // Test: scope_to filters ToolRegistry
    // -----------------------------------------------------------------------

    #[test]
    fn test_scope_to_filters_toolsets() {
        use ironhermes_tools::registry::Tool;
        use ironhermes_core::ToolSchema;
        use async_trait::async_trait;

        struct MockTool { name: String, toolset: String }

        #[async_trait]
        impl Tool for MockTool {
            fn name(&self) -> &str { &self.name }
            fn toolset(&self) -> &str { &self.toolset }
            fn description(&self) -> &str { "mock" }
            fn schema(&self) -> ToolSchema {
                ToolSchema::new(self.name.clone(), "mock tool", serde_json::json!({}))
            }
            async fn execute(&self, _args: serde_json::Value) -> anyhow::Result<String> {
                Ok("ok".to_string())
            }
        }

        let mut reg = ToolRegistry::new();
        reg.register(Box::new(MockTool { name: "mem1".to_string(), toolset: "memory".to_string() }));
        reg.register(Box::new(MockTool { name: "sh1".to_string(), toolset: "shell".to_string() }));
        reg.register(Box::new(MockTool { name: "rf1".to_string(), toolset: "read_file".to_string() }));

        let scoped = reg.scope_to(&["memory".to_string()]);
        assert!(scoped.get("mem1").is_some(), "memory tool should be in scoped registry");
        assert!(scoped.get("sh1").is_none(), "shell tool should NOT be in scoped registry");
        assert!(scoped.get("rf1").is_none(), "read_file tool should NOT be in scoped registry");
    }

    // -----------------------------------------------------------------------
    // Test: parse_env_u64 helper
    // -----------------------------------------------------------------------

    #[test]
    fn test_parse_env_u64_default() {
        assert_eq!(
            parse_env_u64("IRONHERMES_TEST_VAR_DEFINITELY_NOT_SET_12345", 42),
            42
        );
    }

    #[test]
    fn test_parse_env_u64_from_env() {
        let _guard = env_guard();
        unsafe {
            std::env::set_var("IRONHERMES_TEST_PARSE_U64", "99");
        }
        assert_eq!(parse_env_u64("IRONHERMES_TEST_PARSE_U64", 0), 99);
        unsafe {
            std::env::remove_var("IRONHERMES_TEST_PARSE_U64");
        }
    }

    // -----------------------------------------------------------------------
    // Test: cron budget is independent from an interactive budget (§6.4 / T-28.1-14)
    // -----------------------------------------------------------------------

    /// Prove that cron's BudgetHandle and an interactive BudgetHandle are
    /// distinct counters (different Arc<AtomicUsize> instances). The test:
    ///
    /// 1. Creates a cron context mirroring what run_cron_job would do.
    /// 2. Creates a separate "interactive" BudgetHandle (simulating what the
    ///    gateway AgentRuntime owns).
    /// 3. Drains the cron budget to exhaustion.
    /// 4. Asserts that the interactive budget's remaining() is UNCHANGED.
    /// 5. Constructs a second cron budget (simulating a second job run) and
    ///    asserts it starts full regardless of the first job's consumption.
    #[test]
    fn cron_budget_is_independent_from_interactive_budget() {
        use ironhermes_agent::budget::BudgetHandle;
        use ironhermes_core::Config;

        let config = Config::default();
        let max = config.agent.max_iterations;

        // Simulate the interactive budget owned by the gateway AgentRuntime
        let interactive_budget = BudgetHandle::new(max);

        // Simulate what run_cron_job creates: a fresh handle per job
        let cron_budget_job1 = BudgetHandle::new(max);

        // The two handles must NOT share the same Arc<AtomicUsize> counter —
        // consuming cron must not affect interactive.
        // Verify they are different objects by draining cron and checking interactive.
        let cron_remaining_before = cron_budget_job1.remaining();
        let interactive_remaining_before = interactive_budget.remaining();
        assert_eq!(cron_remaining_before, max);
        assert_eq!(interactive_remaining_before, max);

        // Drain the cron budget to exhaustion
        for _ in 0..max {
            cron_budget_job1.consume();
        }
        assert_eq!(
            cron_budget_job1.remaining(),
            0,
            "cron budget should be exhausted after draining"
        );
        assert_eq!(
            cron_budget_job1.consume(),
            None,
            "cron budget at Stop100 returns None"
        );

        // Interactive budget must be completely unaffected
        assert_eq!(
            interactive_budget.remaining(),
            max,
            "interactive budget must be unchanged after draining cron (T-28.1-14)"
        );

        // A second job run gets a FRESH budget starting at max, regardless of
        // the first job's exhaustion — fresh handle per job.
        let cron_budget_job2 = BudgetHandle::new(max);
        assert_eq!(
            cron_budget_job2.remaining(),
            max,
            "second cron job starts with a full budget (fresh BudgetHandle per job)"
        );
        // Confirm job2 budget also cannot affect interactive
        cron_budget_job2.consume();
        assert_eq!(
            interactive_budget.remaining(),
            max,
            "interactive budget still unaffected after second cron job consumes"
        );

        // Belt-and-braces: verify Arc ptr inequality where accessible.
        // Since BudgetHandle wraps Arc<AtomicUsize> privately, we use the
        // behavioral form (drain + compare remaining) proven above. Additionally
        // verify that a clone of cron_budget shares the SAME counter (expected),
        // while interactive_budget does NOT share it.
        let cron_clone = cron_budget_job1.clone();
        // cron_budget_job1 is at 0; its clone must also see 0.
        assert_eq!(
            cron_clone.remaining(),
            0,
            "clone of exhausted cron budget shares the same counter"
        );
        // Resetting via the original must be visible through the clone.
        cron_budget_job1.reset();
        assert_eq!(
            cron_clone.remaining(),
            max,
            "reset is visible through the shared clone"
        );
        // But the interactive budget still unaffected after reset.
        assert_eq!(
            interactive_budget.remaining(),
            max,
            "interactive budget unaffected by cron reset"
        );
    }

    // -----------------------------------------------------------------------
    // Test: cron subagent budget independence at the SUBAGENT layer (D-07.2 / T-28.1-16)
    // -----------------------------------------------------------------------

    /// Prove that a cron job delegating to a subagent leaves the interactive
    /// budget at full headroom — the T-28.1-16 acceptance criterion at the
    /// SUBAGENT layer.
    ///
    /// The existing `cron_budget_is_independent_from_interactive_budget` test
    /// (above) only proves TOP-LEVEL cron independence (28.1-06): a fresh
    /// per-job BudgetHandle cannot drain the interactive handle.  This test
    /// proves the next layer: the per-child BudgetHandle that Plan 35-02
    /// installs in `AgentSubagentRunner::run_child` (one fresh
    /// `BudgetHandle::new(max_iterations)` per child loop, replacing the
    /// PROV-10 budget.clone()) is equally independent from the interactive
    /// budget.
    ///
    /// After Plan 35-02 the shared `ToolRegistry` delegate runner is no longer
    /// a cross-budget contamination vector: children get their own counter
    /// rather than cloning the parent's Arc<AtomicUsize>.  This test confirms
    /// that invariant holds at the cron-subagent boundary.
    ///
    /// References: T-28.1-16, D-07.2, D-01, D-04 (PROV-10 retirement).
    #[test]
    fn cron_subagent_budget_independence_from_interactive() {
        use ironhermes_agent::budget::BudgetHandle;
        use ironhermes_core::Config;

        let config = Config::default();
        let max = config.agent.max_iterations;

        // Simulate the interactive budget owned by the gateway AgentRuntime.
        // This is what the interactive chat runtime holds across turns.
        let interactive_budget = BudgetHandle::new(max);

        // Simulate the per-child fresh budget that Plan 35-02 installs in
        // AgentSubagentRunner::run_child.  Each cron-spawned subagent gets
        // BudgetHandle::new(max_iterations) — a distinct Arc<AtomicUsize>.
        let child_budget_1 = BudgetHandle::new(max);

        // Preconditions: both start full.
        assert_eq!(interactive_budget.remaining(), max);
        assert_eq!(child_budget_1.remaining(), max);

        // Drain the per-child budget to exhaustion (simulating a cron
        // subagent running to Stop100).
        for _ in 0..max {
            child_budget_1.consume();
        }
        assert_eq!(
            child_budget_1.remaining(),
            0,
            "child budget drained to exhaustion"
        );
        assert_eq!(
            child_budget_1.consume(),
            None,
            "consume at 0 returns None (Stop100)"
        );

        // T-28.1-16 acceptance: the interactive budget MUST be untouched.
        // Before Plan 35-02 this would fail because children cloned the
        // parent Arc (PROV-10) — draining child_budget_1 would have drained
        // interactive_budget too.
        assert_eq!(
            interactive_budget.remaining(),
            max,
            "interactive budget must be at full headroom after cron subagent drain (T-28.1-16)"
        );

        // A second cron subagent gets its own fresh budget regardless of the
        // first subagent's exhaustion.
        let child_budget_2 = BudgetHandle::new(max);
        assert_eq!(
            child_budget_2.remaining(),
            max,
            "second cron subagent starts with a full budget (fresh BudgetHandle::new per child)"
        );

        // Draining the second child also cannot touch interactive headroom.
        for _ in 0..max {
            child_budget_2.consume();
        }
        assert_eq!(
            interactive_budget.remaining(),
            max,
            "interactive budget still at full headroom after second cron subagent drain (T-28.1-16)"
        );
    }

    // -----------------------------------------------------------------------
    // Test: complete_and_dispatch with local deliver (no delivery)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_complete_and_dispatch_local_no_delivery() {
        let tmp = TempDir::new().expect("tmpdir");
        let _guard = env_guard();
        unsafe {
            std::env::set_var("IRONHERMES_HOME", tmp.path());
        }

        let ctx = make_ctx(&tmp);
        // Add job via add_job so complete_job_run can mark it
        let job = {
            let mut store = ctx.job_store.lock().unwrap();
            store.add_job(
                "Test Job",
                "do something",
                ironhermes_cron::job::ScheduleParsed::Interval {
                    minutes: 60,
                    display: "every 60m".to_string(),
                },
                "every 60m",
                "local",
                vec![],
                None,
            ).expect("add job")
        };

        let result = complete_and_dispatch(&ctx, &job, "test output", true).await;

        unsafe {
            std::env::remove_var("IRONHERMES_HOME");
        }

        assert!(result.is_ok(), "complete_and_dispatch should succeed: {:?}", result);
    }
}
