# Phase 22: CLI Feature Parity - Research

**Researched:** 2026-04-17
**Domain:** Rust CLI wiring — ToolRegistry, HookRegistry, guardrails, execute_code, skills_tool, cron_tool
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Three-way split. Phase 22 = CLI-01 (tool parity only). Phase 22.1 = CLI-02 (TUI extension hooks). Phase 22.2 = CLI-03..08 (ACP adapter).
- **D-02:** Full parity — wire ALL tools the gateway has into CLI: execute_code, guardrails, hooks, skills_tool, cron_tool.
- **D-03:** Both `run_chat` AND `run_single` get the full tool surface.
- **D-04:** The RPC dispatch registry (sandbox-safe tools for execute_code) is constructed the same way as in `run_gateway`: file tools + web tools + memory tool — no terminal, no execute_code in the RPC registry itself.
- **D-05:** CLI fires the same lifecycle events as gateway: `session:start`, `session:end`, `agent:start`, `agent:step`, `agent:end`, `tool:called`, `tool:completed`, `command:*`. Only `gateway:startup` is skipped.
- **D-06:** JSONL event logging is the default when `hooks_config.event_log.enabled` is true.
- **D-07:** Webhook forwarding is opt-in, not default for CLI. If `hooks_config.webhooks` has entries, they are registered — same as gateway — but the config drives whether they're active.
- **D-08:** Follow the gateway's wiring pattern in `run_gateway` (lines 800-900 of main.rs) as the reference implementation.
- **D-09:** The `attach_context_engine` call in `run_single` currently passes `None` for the hook registry parameter. Phase 22 changes this to pass the actual `HookRegistry`.

### Claude's Discretion

- Exact placement of hook `emit()` calls within run_chat/run_single (before or after state_store writes)
- Whether to extract a shared `wire_tools()` helper that both `run_chat`, `run_single`, and `run_gateway` call, or keep the wiring inline
- Whether the cron_tool in CLI should be limited (e.g., no `tick` in non-gateway mode) or full-featured

### Deferred Ideas (OUT OF SCOPE)

- Phase 22.1: TUI extension hooks (CLI-02)
- Phase 22.2: ACP adapter (CLI-03..08)
- New CLI subcommands (hermes sessions, hermes config, hermes tools, etc.)
- Slash command integration (SKILL-12/13/14)
</user_constraints>

---

## Summary

Phase 22 is a wiring phase — nearly all the required infrastructure already exists and works in gateway mode. The task is to replicate the `run_gateway` tool-registration and hook-wiring sequence inside `run_chat` and `run_single`. No new crates are needed and no new APIs need to be designed.

The gap is precise and well-bounded. Both `run_chat` (line 395) and `run_single` (line 271) call `build_registry()` which only runs `register_defaults()` (terminal, file tools, web tools). They are missing: `register_cronjob_tool`, `register_skills_tool`, `register_execute_code_tool_with_active_skills`, guardrail wiring (`add_guardrail` + `set_error_detail`), and the `HookRegistry` construction + listener registration. Additionally, both `attach_context_engine` calls pass `None` for the hook registry, which means `ContextPreCompress` / `ContextPressure` hook events do not fire in CLI mode.

The gateway wiring sequence (lines 800-900 of main.rs) is the canonical reference. It is the single source of truth for what the CLI paths must replicate. The planner should treat that block as a recipe.

**Primary recommendation:** Replicate the gateway wiring sequence in both `run_chat` and `run_single`, then optionally extract a `wire_full_toolset()` shared helper. Pass the constructed `Arc<HookRegistry>` to `attach_context_engine` in both paths.

---

## Standard Stack

### Core (all already in Cargo.toml — no new dependencies)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `ironhermes-hooks` | workspace | HooksConfig, HookRegistry, BlocklistGuardrail, create_jsonl_listener, create_webhook_listener, RetryQueue, drain_retry_queue | Already wired in gateway; all types exported from lib.rs |
| `ironhermes-exec` | workspace | Sandbox, ExecConfig, ToolDispatch | Already used by execute_code tool |
| `ironhermes-tools` | workspace | register_execute_code_tool_with_active_skills, register_skills_tool, register_cronjob_tool, default_credential_dir | All registration methods already exist on ToolRegistry |
| `ironhermes-cron` | workspace | JobStore | Already used in gateway; already imported in CLI main.rs |

**Installation:** No new packages. All dependencies are already workspace members.

[VERIFIED: crates/ironhermes-cli/src/main.rs imports — ironhermes_hooks, ironhermes_cron, ironhermes_tools all present]

---

## Architecture Patterns

### Gateway Wiring Sequence (the canonical recipe)

The gateway wiring in `run_gateway` (main.rs lines 800-903) follows this exact sequence:

```rust
// [VERIFIED: crates/ironhermes-cli/src/main.rs lines 800-903]

// 1. Memory (already done in run_chat/run_single)
let memory_manager = build_memory_manager(&config.memory).await?;
registry.register_memory_tool(memory_manager.clone());

// 2. Cron — open JobStore and register
let cron_dir = ironhermes_core::get_hermes_home().join("cron");
let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
registry.register_cronjob_tool(job_store.clone());

// 3. Skills — build active_skills Arc, register skills_tool
let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
    Arc::new(std::sync::Mutex::new(Vec::new()));
let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);
registry.register_skills_tool(
    skill_registry.clone(),
    active_skills.clone(),
    credential_dir,
    std::collections::HashMap::new(),
);

// 4. RPC dispatch registry (D-04: safe subset only)
let mut rpc_registry = ToolRegistry::new();
rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
rpc_registry.register_memory_tool(memory_manager.clone());
let rpc_registry = Arc::new(rpc_registry);

// 5. execute_code (pass active_skills for env var pass-through)
registry.register_execute_code_tool_with_active_skills(
    rpc_registry,
    config.exec.clone(),
    active_skills.clone(),
);

// 6. delegate_task (already done in run_chat/run_single)

// 7. Guardrails — BEFORE Arc wrapping
let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
if !hooks_config.blocked_tools.is_empty() {
    registry.add_guardrail(Box::new(
        ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
    ));
}
registry.set_error_detail(hooks_config.error_detail.clone());

// 8. Arc wrap
let registry = Arc::new(registry);

// 9. Build HookRegistry
let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());

// 10. JSONL listener (D-06: default when enabled)
if hooks_config.event_log.enabled {
    let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
    hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
}

// 11. Retry queue and webhook listeners (D-07: opt-in — only fires if config has entries)
let retry_queue = std::sync::Arc::new(
    ironhermes_hooks::RetryQueue::new(
        ironhermes_hooks::RetryQueue::default_path()
    ).expect("Failed to initialize webhook retry queue")
);
for endpoint in &hooks_config.webhooks {
    hook_registry.add_listener(
        ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone())
    );
}
let hook_registry = std::sync::Arc::new(hook_registry);

// 12. Drain retry queue from previous runs
let default_ttl = hooks_config.webhooks.first()
    .and_then(|e| e.queue_ttl_hours)
    .unwrap_or(24);
ironhermes_hooks::drain_retry_queue(
    retry_queue.clone(),
    &hooks_config.webhooks,
    default_ttl,
).await;
```

### Hook Registry in AgentLoop

The `AgentLoop` accepts a hook registry via builder pattern:

```rust
// [VERIFIED: crates/ironhermes-agent/src/agent_loop.rs line 246]
pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self {
    self.hook_registry = Some(registry);
    self
}
```

In `run_agent_turn` (the CLI's per-turn agent construction), this call must be added after `.with_cancellation_token(cancel_token)` and before `.with_streaming(...)`. Pattern from gateway handler:

```rust
// [VERIFIED: crates/ironhermes-gateway/src/handler.rs line 447-448]
if let Some(ref registry) = self.hook_registry {
    agent = agent.with_hook_registry(registry.clone());
}
```

The CLI equivalent: `agent = agent.with_hook_registry(hook_registry.clone());`

### Context Engine Hook Wiring (D-09)

`attach_context_engine` already accepts an `Option<Arc<HookRegistry>>` parameter:

```rust
// [VERIFIED: crates/ironhermes-agent/src/agent_wiring.rs lines 41-48]
pub fn attach_context_engine(
    agent: AgentLoop,
    config: &Config,
    resolver: &ProviderResolver,
    session_id: impl Into<String>,
    hooks: Option<Arc<HookRegistry>>,      // <-- currently None in both CLI paths
    tracker: Option<Arc<PressureTracker>>,
) -> AgentLoop
```

Both `run_agent_turn` (line 750) and `run_single` (line 341) currently pass `None`. Phase 22 changes both to pass `Some(hook_registry.clone())`.

### What CLI Currently Has vs What's Missing

| Tool/Feature | run_gateway | run_chat | run_single | Phase 22 Action |
|---|---|---|---|---|
| register_defaults (file, web, terminal) | YES | YES | YES | No change |
| register_memory_tool | YES | YES | YES | No change |
| register_delegate_task_tool | YES | YES | YES | No change |
| register_cronjob_tool | YES | NO | NO | ADD to both |
| register_skills_tool | YES | NO | NO | ADD to both |
| register_execute_code_tool_with_active_skills | YES | NO | NO | ADD to both |
| active_skills Arc | YES | skill_registry only | skill_registry only | CREATE Arc, pass to skills + exec tools |
| BlocklistGuardrail | YES | NO | NO | ADD before Arc wrap |
| set_error_detail | YES | NO | NO | ADD before Arc wrap |
| HookRegistry construction | YES | NO | NO | ADD after Arc wrap |
| JSONL listener | YES | NO | NO | ADD if event_log.enabled |
| Webhook listeners | YES | NO | NO | ADD if hooks_config.webhooks non-empty |
| RetryQueue + drain | YES | NO | NO | ADD |
| hook_registry → AgentLoop | YES | NO | NO | ADD in run_agent_turn |
| hook_registry → attach_context_engine | YES | NO (None) | NO (None) | CHANGE None → Some |

### Optional: wire_full_toolset() Helper (Claude's Discretion)

Both `run_chat` and `run_single` would duplicate ~60 lines of wiring. A shared function would be:

```rust
struct WiredTools {
    registry: Arc<ToolRegistry>,
    hook_registry: Arc<ironhermes_hooks::HookRegistry>,
    job_store: Arc<Mutex<JobStore>>,
    skill_registry: Arc<SkillRegistry>,
    active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>>,
}

async fn wire_full_toolset(
    config: &Config,
    client: &AnyClient,
    resolver: &ProviderResolver,
    budget: &Arc<AtomicUsize>,
    cancel_token: Option<CancellationToken>,
    progress_callback: Option<SubagentProgressCallback>,
) -> Result<WiredTools>
```

**Recommendation:** Extract the helper. The gateway already set this precedent (its wiring is already centralized in `run_gateway`). Three copies of the same 60-line block creates maintenance risk — a future tool addition would require three edits instead of one.

### Anti-Patterns to Avoid

- **Registering tools AFTER `Arc::new(registry)`:** `add_guardrail` and tool registration require `&mut self`. The registry must be fully configured before wrapping in `Arc`. [VERIFIED: registry.rs line 46 — `add_guardrail` takes `&mut self`]
- **Passing `active_skills` to `execute_code` without first populating it via `register_skills_tool`:** Both share the same `Arc<Mutex<Vec<SkillRecord>>>`. The skills_tool writes to it when the agent activates a skill; execute_code reads it. They must share the same Arc instance.
- **Creating a fresh `HooksConfig` in each path:** Load once, use for both guardrail registration and HookRegistry construction. The gateway loads it once and passes it to both.
- **Forgetting `drain_retry_queue` in CLI:** The gateway drains the retry queue on startup to flush events from previous runs. CLI must do the same, otherwise webhook retries accumulate indefinitely.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Tool blocklist enforcement | Custom pre-dispatch check | `BlocklistGuardrail::from_config(&hooks_config)` | Already implemented, tested, and integrated in agent_loop guardrail chain |
| JSONL event serialization | Custom file writer | `create_jsonl_listener(log_path)` | Already handles file rotation, UTF-8 boundaries, append-mode |
| Webhook delivery with retry | Custom HTTP client loop | `create_webhook_listener` + `RetryQueue` | HMAC signing, template mapping, TTL expiry already implemented |
| Sandbox RPC dispatch | Custom bridge | `RegistryDispatch` (internal to execute_code.rs) | Circular dep handled — RegistryDispatch lives in ironhermes-tools, not ironhermes-exec |
| execute_code env var whitelist | Manual env filtering | `active_skills` Arc pass-through | `register_execute_code_tool_with_active_skills` already handles skill env var bypass of secret-strip |
| Hook event ordering | Custom pre/post hooks | `check_guardrails` + `fire_hook` in agent_loop | Ordering already correct: guardrail → ToolCalled → execute → ToolCompleted |

---

## Common Pitfalls

### Pitfall 1: Arc wrapping registry before guardrail registration

**What goes wrong:** `add_guardrail` requires `&mut ToolRegistry`. If called after `Arc::new(registry)`, the compiler rejects it.

**Why it happens:** The gateway code has a clear comment "before Arc wrapping" but the CLI might inline these calls in a different order.

**How to avoid:** Follow the gateway sequence exactly — all `add_guardrail`, `set_error_detail`, and `register_*` calls must precede `let registry = Arc::new(registry)`.

**Warning signs:** Compiler error "cannot borrow as mutable through `Arc`".

### Pitfall 2: active_skills Arc not shared between skills_tool and execute_code

**What goes wrong:** Skills activated via `skills_tool` (which writes to the `active_skills` Vec) won't propagate their env vars into the execute_code sandbox.

**Why it happens:** If `active_skills` is created independently in two places, they don't share state.

**How to avoid:** Create one `Arc<std::sync::Mutex<Vec<SkillRecord>>>` before calling `register_skills_tool`, pass `.clone()` to both `register_skills_tool` and `register_execute_code_tool_with_active_skills`.

**Warning signs:** `execute_code` sandbox fails to access env vars declared by an active skill.

### Pitfall 3: hook_registry not wired into run_agent_turn

**What goes wrong:** `ToolCalled` and `ToolCompleted` events are never fired in CLI mode even though HookRegistry is constructed. The agent_loop only fires events when `self.hook_registry` is `Some`.

**Why it happens:** `run_agent_turn` constructs a fresh `AgentLoop` every turn. The hook registry must be passed via `.with_hook_registry(hook_registry.clone())` in every `AgentLoop::new(...)` call.

**How to avoid:** Add the `.with_hook_registry(...)` builder call in `run_agent_turn` immediately after `.with_cancellation_token(cancel_token)`. Also add it in `run_single` where `AgentLoop::new` is called.

**Warning signs:** JSONL event log exists but only contains context compression events, never ToolCalled/ToolCompleted.

### Pitfall 4: attach_context_engine still passes None for hook registry

**What goes wrong:** `ContextPreCompress` and `ContextPressure` events never fire in CLI mode, so memory flush hooks don't trigger before compression.

**Why it happens:** D-09 is easy to overlook — it's not in `run_agent_turn` itself but in the `attach_context_engine` call at line 750.

**How to avoid:** Change both `attach_context_engine(agent, config, resolver, session_id, None, ...)` calls to `attach_context_engine(agent, config, resolver, session_id, Some(hook_registry.clone()), ...)`.

**Warning signs:** Memory is not flushed before context compression in CLI mode.

### Pitfall 5: Missing active_skills in run_single

**What goes wrong:** `run_single` currently has `skill_registry` (for PromptBuilder) but no `active_skills` Arc. The `register_skills_tool` and `register_execute_code_tool_with_active_skills` calls both need it.

**Why it happens:** The gateway created `active_skills` as a separate Arc alongside `skill_registry`. The CLI only tracks `skill_registry` because it never needed `active_skills` before.

**How to avoid:** Create `active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> = Arc::new(std::sync::Mutex::new(Vec::new()))` in both `run_chat` and `run_single` before the tool registration block.

### Pitfall 6: Lifetime of job_store in CLI

**What goes wrong:** `run_chat` and `run_single` don't hold a `job_store` reference after wiring, so if the tool tries to use it after the local `Arc` is dropped, it panics.

**Why it happens:** In gateway mode, `job_store` is stored in `GatewayRunner` (field) for the process lifetime. In CLI, it must stay alive for the duration of `run_chat` / `run_single`.

**How to avoid:** Keep `job_store` as a local `let` binding in the same scope as the agent loop execution. Since `register_cronjob_tool` passes an `Arc::clone`, the `CronjobTool` keeps a strong reference — the local binding can be dropped after tool registration. But if cron ticking is needed (unlikely for CLI), the runner needs to hold it.

---

## Code Examples

### Complete wiring block to add in run_chat / run_single

```rust
// [Pattern: VERIFIED from run_gateway lines 800-922, adapted for CLI]

// --- Step A: Cron ---
let cron_dir = ironhermes_core::get_hermes_home().join("cron");
let job_store = Arc::new(Mutex::new(JobStore::open(cron_dir)?));
registry.register_cronjob_tool(job_store.clone());

// --- Step B: Skills ---
let cwd = std::env::current_dir().unwrap_or_default();
let skill_registry = Arc::new(SkillRegistry::load_with_config(&cwd, &config.skills));
let active_skills: Arc<std::sync::Mutex<Vec<ironhermes_core::SkillRecord>>> =
    Arc::new(std::sync::Mutex::new(Vec::new()));
let credential_dir = ironhermes_tools::skills_tool::default_credential_dir(&config.skills);
registry.register_skills_tool(
    skill_registry.clone(),
    active_skills.clone(),
    credential_dir,
    std::collections::HashMap::new(),
);

// --- Step C: RPC registry for execute_code (D-04 safe subset) ---
let mut rpc_registry = ToolRegistry::new();
rpc_registry.register(Box::new(ironhermes_tools::file_tools::ReadFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::WriteFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::PatchFileTool));
rpc_registry.register(Box::new(ironhermes_tools::file_tools::SearchFilesTool));
rpc_registry.register(Box::new(ironhermes_tools::web_search::WebSearchTool));
rpc_registry.register(Box::new(ironhermes_tools::web_read::WebReadTool));
rpc_registry.register_memory_tool(memory_manager.clone());
let rpc_registry = Arc::new(rpc_registry);

// --- Step D: execute_code ---
registry.register_execute_code_tool_with_active_skills(
    rpc_registry,
    config.exec.clone(),
    active_skills.clone(),
);

// --- Step E: Guardrails (before Arc wrap) ---
let hooks_config = ironhermes_hooks::HooksConfig::load().unwrap_or_default();
if !hooks_config.blocked_tools.is_empty() {
    registry.add_guardrail(Box::new(
        ironhermes_hooks::BlocklistGuardrail::from_config(&hooks_config),
    ));
}
registry.set_error_detail(hooks_config.error_detail.clone());

// --- Step F: Arc wrap ---
let registry = Arc::new(registry);

// --- Step G: HookRegistry ---
let mut hook_registry = ironhermes_hooks::HookRegistry::new(hooks_config.clone());

// JSONL listener (D-06)
if hooks_config.event_log.enabled {
    let log_path = hooks_config.event_log.path.as_ref().map(std::path::PathBuf::from);
    hook_registry.add_listener(ironhermes_hooks::create_jsonl_listener(log_path));
}

// Webhook listeners (D-07: opt-in; RetryQueue always created for drain)
let retry_queue = Arc::new(
    ironhermes_hooks::RetryQueue::new(
        ironhermes_hooks::RetryQueue::default_path()
    ).context("Failed to initialize webhook retry queue")?
);
for endpoint in &hooks_config.webhooks {
    hook_registry.add_listener(
        ironhermes_hooks::create_webhook_listener(endpoint.clone(), retry_queue.clone())
    );
}
let hook_registry = Arc::new(hook_registry);

// Drain persistent retry queue (D-09)
let default_ttl = hooks_config.webhooks.first()
    .and_then(|e| e.queue_ttl_hours)
    .unwrap_or(24);
ironhermes_hooks::drain_retry_queue(
    retry_queue.clone(),
    &hooks_config.webhooks,
    default_ttl,
).await;
```

### Change to run_agent_turn — wire hook_registry into AgentLoop

```rust
// [VERIFIED pattern: crates/ironhermes-gateway/src/handler.rs line 447]
// Add after .with_cancellation_token(cancel_token):
let mut agent = AgentLoop::new(client.clone(), registry, max_turns)
    .with_budget(budget.clone())
    .with_cancellation_token(cancel_token)
    .with_hook_registry(hook_registry.clone())   // ADD THIS
    .with_compression(...)
    // ...rest unchanged
```

### Change to attach_context_engine calls (D-09)

```rust
// run_agent_turn line 750 — BEFORE:
agent = ironhermes_agent::attach_context_engine(
    agent, config, resolver, session_id, None, Some(pressure_tracker.clone()),
);
// AFTER:
agent = ironhermes_agent::attach_context_engine(
    agent, config, resolver, session_id, Some(hook_registry.clone()), Some(pressure_tracker.clone()),
);

// run_single line 341 — BEFORE:
agent = ironhermes_agent::attach_context_engine(
    agent, &config, &resolver, session_id.as_str(), None, None,
);
// AFTER:
agent = ironhermes_agent::attach_context_engine(
    agent, &config, &resolver, session_id.as_str(), Some(hook_registry.clone()), None,
);
```

### Signature changes to run_agent_turn (if not extracting shared helper)

`run_agent_turn` currently doesn't receive a `hook_registry` parameter. If NOT extracting a shared helper, the signature must gain one:

```rust
// Add to run_agent_turn signature:
hook_registry: Arc<ironhermes_hooks::HookRegistry>,
```

This requires the call sites in `run_chat` (the initial_message path and the REPL loop) to pass `hook_registry.clone()`.

---

## State of the Art

| Old Approach | Current Approach | Impact for Phase 22 |
|---|---|---|
| CLI had no HookRegistry | Gateway built full hook system in Phase prior to 20 | CLI now needs to replicate — all types available |
| execute_code had no active_skills | Phase 19 Plan 06 added active_skills Arc | CLI must create and share the Arc |
| attach_context_engine took no hooks | Phase 18 added hooks parameter | CLI must pass hook_registry instead of None |
| MemoryManager was gateway-only | Phase 20 Fix 2 wired it in CLI | Already done — CLI is ahead here |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | cron_tool in CLI should be full-featured (no tick suppression) | Architecture Patterns / Pitfall 6 | If `tick` is called unexpectedly in non-gateway mode, it may open network connections or fire webhooks without a running gateway. Low risk — cron_tool tick is gateway-initiated, not agent-initiated. | 
| A2 | PromptBuilder.set_skill_registry() in run_chat already uses the skill_registry that will be shared with register_skills_tool | Code Examples | If they were constructed separately before, they'd diverge. Inspection shows run_chat constructs `skill_registry` then immediately passes it to prompt_builder — the Phase 22 work replaces this with the shared Arc. | [VERIFIED: main.rs lines 483, 487, 807] |

**If this table is empty:** All claims in this research were verified or cited — no user confirmation needed.

---

## Open Questions

1. **Should `run_chat` hold `job_store` for the session lifetime or drop after wiring?**
   - What we know: `register_cronjob_tool` takes `Arc<Mutex<JobStore>>` — the tool holds a clone. The local binding can be dropped safely.
   - What's unclear: Whether the planner wants to expose `job_store` to any session-end cleanup logic (e.g., flushing pending cron jobs on `/quit`).
   - Recommendation: Drop after wiring for now. The Arc inside `CronjobTool` keeps the store alive for the session. Add session-end cleanup to Phase 22.1 if needed.

2. **Should `drain_retry_queue` be skipped in CLI if no webhooks are configured?**
   - What we know: The gateway always drains regardless. The drain no-ops gracefully if the queue file is empty.
   - What's unclear: Whether CLI users running without webhook config want the queue file created at `RetryQueue::default_path()`.
   - Recommendation: Mirror gateway behavior exactly — always drain. A no-op drain is zero cost; omitting it would diverge CLI from gateway behavior and could leave stale events if the user later enables webhooks.

3. **`run_agent_turn` signature change vs. closure capture**
   - What we know: `hook_registry` is constructed in `run_chat` and must reach `run_agent_turn`. The function currently takes 12 parameters (`#[allow(clippy::too_many_arguments)]`).
   - What's unclear: Whether the planner prefers a 13th parameter or extracting a struct.
   - Recommendation: If `wire_full_toolset()` helper is extracted (Claude's discretion), `hook_registry` becomes part of its return struct and can be passed cleanly. If not, a 13th parameter is the mechanical correct answer.

---

## Environment Availability

Step 2.6: SKIPPED — Phase 22 is purely Rust code wiring changes within the existing workspace. No external tools, services, or CLIs beyond the project's own build system are needed.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` / `#[tokio::test]` |
| Config file | Workspace Cargo.toml (no separate test runner config) |
| Quick run command | `cargo test -p ironhermes-cli 2>&1 \| tail -20` |
| Full suite command | `cargo test --workspace 2>&1 \| tail -40` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| CLI-01 (D-02) | All tools registered in run_chat + run_single | static-grep regression | grep-in-test asserting `register_cronjob_tool`, `register_skills_tool`, `register_execute_code_tool_with_active_skills` appear in `run_chat` and `run_single` | No — Wave 0 |
| CLI-01 (D-03) | active_skills Arc shared between skills_tool and execute_code | unit test | `cargo test -p ironhermes-cli` | No — Wave 0 |
| CLI-01 (D-06) | JSONL listener registered when event_log.enabled | unit/integration | `cargo test -p ironhermes-hooks` | Partial (hooks tests exist) |
| CLI-01 (D-07) | Webhook listener registered only when webhooks configured | unit | `cargo test -p ironhermes-hooks` | Partial |
| CLI-01 (D-09) | hook_registry passed to attach_context_engine | static-grep test | grep asserting None not passed | No — Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-cli`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full workspace suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-cli/src/main.rs` static-grep regression test — asserts all 5 wiring calls present in both `run_chat` and `run_single` (mirrors Phase 20-03's `run_chat_and_run_single_both_wire_memory_manager` pattern)
- [ ] Integration smoke test for CLI with hooks: construct wired registry + HookRegistry, fire a mock tool call, assert ToolCalled + ToolCompleted appear in JSONL

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | N/A — CLI is single-operator |
| V3 Session Management | no | Already handled by StateStore |
| V4 Access Control | yes | BlocklistGuardrail — blocks named tools before dispatch |
| V5 Input Validation | yes | `BlocklistGuardrail::from_config` validates tool name against blocklist; skill env var whitelist via `active_skill_env_names` |
| V6 Cryptography | yes | HMAC-SHA256 webhook signing via `create_webhook_listener` — do not hand-roll |

### Known Threat Patterns for CLI + execute_code

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| execute_code calling execute_code recursively | Elevation of Privilege | RPC registry excludes execute_code and terminal (D-04) — already enforced |
| Skill-injected env vars leaking secrets into sandbox | Information Disclosure | `active_skill_env_names` whitelist in `Sandbox::build_env` — share same `active_skills` Arc |
| Blocked tool bypass via delegate_task | Elevation of Privilege | Subagent receives same registry with same guardrails |
| Stale webhook events from previous sessions | Tampering | `drain_retry_queue` on startup — same as gateway |

---

## Sources

### Primary (HIGH confidence)

- `crates/ironhermes-cli/src/main.rs` — run_gateway (lines 786-923), run_chat (lines 377-693), run_single (lines 261-374), run_agent_turn (lines 695-783), build_registry (line 942). All gap analysis is VERIFIED by direct code reading.
- `crates/ironhermes-hooks/src/lib.rs` — public API surface verified
- `crates/ironhermes-hooks/src/registry.rs` — HookRegistry API: fire, fire_awaitable, add_listener, add_async_listener, with_hook_registry
- `crates/ironhermes-hooks/src/config.rs` — HooksConfig: event_log, blocked_tools, webhooks, error_detail
- `crates/ironhermes-hooks/src/event.rs` — HookEventKind variants: ToolCalled, ToolCompleted, MessageReceived, ResponseSent, SkillActivated, ContextPreCompress, ContextPressure
- `crates/ironhermes-agent/src/agent_loop.rs` — with_hook_registry builder, fire_hook internal method
- `crates/ironhermes-agent/src/agent_wiring.rs` — attach_context_engine signature (hooks parameter position 5)
- `crates/ironhermes-tools/src/registry.rs` — add_guardrail, set_error_detail, register_execute_code_tool_with_active_skills, register_skills_tool, register_cronjob_tool signatures
- `crates/ironhermes-gateway/src/handler.rs` — with_hook_registry wiring pattern (line 447)
- `crates/ironhermes-gateway/src/runner.rs` — hook event fire calls in gateway context

### Secondary (MEDIUM confidence)

- `22-CONTEXT.md` — user decisions D-01..D-09 consulted for scope constraints

### Tertiary (LOW confidence)

- None — all claims are VERIFIED from direct codebase inspection.

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all APIs verified by reading source
- Architecture: HIGH — gateway wiring sequence read line-by-line and mapped to CLI gaps
- Pitfalls: HIGH — derived from direct structural analysis of what CLI currently does vs gateway
- Security: HIGH — controls already implemented, research confirmed they apply to CLI too

**Research date:** 2026-04-17
**Valid until:** 2026-05-17 (stable Rust workspace; no external deps)
