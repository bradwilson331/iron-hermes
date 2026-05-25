---
phase: 22-cli-feature-parity
verified: 2026-04-17T21:15:00Z
status: passed
score: 14/14
overrides_applied: 0
---

# Phase 22: CLI Tool Parity Verification Report

**Phase Goal:** Wire execute_code, skills_tool, cron_tool, BlocklistGuardrail, and HookRegistry (JSONL event logging + webhook listeners) into both `run_chat` and `run_single` CLI paths, achieving full tool-level parity with `run_gateway`. Pass the HookRegistry to AgentLoop and attach_context_engine so all lifecycle events fire in CLI mode. Per D-01: this phase covers CLI-01 only (tool parity). TUI extension hooks split to Phase 22.1; ACP adapter split to Phase 22.2.
**Verified:** 2026-04-17T21:15:00Z
**Status:** PASSED
**Re-verification:** No -- initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | run_chat registers cron_tool, skills_tool, and execute_code_tool before Arc wrapping the registry | VERIFIED | Lines 560, 568, 587 all before `Arc::new(registry)` at line 602 |
| 2 | run_single registers cron_tool, skills_tool, and execute_code_tool before Arc wrapping the registry | VERIFIED | Lines 300, 308, 327 all before `Arc::new(registry)` at line 342 |
| 3 | Both paths construct an active_skills Arc shared between skills_tool and execute_code | VERIFIED | `active_skills.clone()` appears twice in each path: run_single (310, 330), run_chat (570, 590) |
| 4 | Both paths construct a BlocklistGuardrail from HooksConfig and add it to the registry | VERIFIED | `add_guardrail(Box::new(BlocklistGuardrail::from_config(...)))` at lines 336 (run_single) and 596 (run_chat) |
| 5 | Both paths set error_detail on the registry from HooksConfig | VERIFIED | `set_error_detail(hooks_config.error_detail.clone())` at lines 340 (run_single) and 600 (run_chat) |
| 6 | The RPC dispatch registry for execute_code contains only file tools, web tools, and memory tool (no terminal, no execute_code) | VERIFIED | Lines 316-324 (run_single), 576-584 (run_chat): ReadFileTool, WriteFileTool, PatchFileTool, SearchFilesTool, WebSearchTool, WebReadTool, memory_tool. No terminal or execute_code registered. Matches run_gateway (lines 977-985). |
| 7 | run_chat constructs a HookRegistry with JSONL listener (when event_log.enabled) and webhook listeners (when webhooks configured) | VERIFIED | Lines 604-624: `HookRegistry::new`, `create_jsonl_listener`, `create_webhook_listener` all present |
| 8 | run_single constructs a HookRegistry with JSONL listener and webhook listeners | VERIFIED | Lines 344-364: identical construction pattern to run_chat |
| 9 | run_agent_turn receives and wires hook_registry into AgentLoop via .with_hook_registry() | VERIFIED | Line 866: `hook_registry: Arc<ironhermes_hooks::HookRegistry>` parameter; line 879: `.with_hook_registry(hook_registry.clone())` |
| 10 | run_single wires hook_registry into AgentLoop via .with_hook_registry() | VERIFIED | Line 398: `.with_hook_registry(hook_registry.clone())` |
| 11 | attach_context_engine in run_agent_turn passes Some(hook_registry.clone()) instead of None | VERIFIED | Line 913: `Some(hook_registry.clone())` -- no `None` for hooks parameter |
| 12 | attach_context_engine in run_single passes Some(hook_registry.clone()) instead of None | VERIFIED | Line 424: `Some(hook_registry.clone())` -- no `None` for hooks parameter |
| 13 | Both CLI paths drain the retry queue on startup | VERIFIED | `drain_retry_queue` at lines 370-374 (run_single), 630-634 (run_chat) |
| 14 | Static-grep regression tests confirm all wiring calls are present in both run_chat and run_single | VERIFIED | `cli_tool_parity.rs` exists with 4 tests, all passing (161 lines, brace-balanced extraction) |

**Score:** 14/14 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-cli/src/main.rs` | Full tool registration in run_chat and run_single matching run_gateway parity | VERIFIED | 1132 lines; contains all tool registrations, HookRegistry wiring, guardrails in both CLI paths |
| `crates/ironhermes-cli/tests/cli_tool_parity.rs` | Static-grep regression tests for tool and hook wiring | VERIFIED | 161 lines; 4 test functions covering tool parity, hook registry, attach_context_engine, active_skills sharing |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| run_chat | register_cronjob_tool, register_skills_tool, register_execute_code_tool_with_active_skills | tool registration calls before Arc::new(registry) | WIRED | Lines 560, 568, 587 all before line 602 |
| run_single | register_cronjob_tool, register_skills_tool, register_execute_code_tool_with_active_skills | tool registration calls before Arc::new(registry) | WIRED | Lines 300, 308, 327 all before line 342 |
| active_skills Arc | register_skills_tool AND register_execute_code_tool_with_active_skills | shared Arc cloned to both | WIRED | 2 clones per path: run_single (310, 330), run_chat (570, 590) |
| run_chat | HookRegistry | HookRegistry::new(hooks_config.clone()) + add_listener calls | WIRED | Line 605: `HookRegistry::new`, lines 610, 620-622: listeners |
| run_agent_turn | AgentLoop.with_hook_registry | hook_registry parameter passed to .with_hook_registry(hook_registry.clone()) | WIRED | Line 879: `.with_hook_registry(hook_registry.clone())` |
| attach_context_engine | hook_registry | Some(hook_registry.clone()) replacing None | WIRED | Lines 424 (run_single), 913 (run_agent_turn) |

### Data-Flow Trace (Level 4)

Not applicable -- this phase wires infrastructure (tool registration, hooks, guardrails) rather than rendering dynamic data. No components render data from these registrations.

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| Static-grep regression tests pass | `cargo test -p ironhermes-cli --test cli_tool_parity` | 4 passed; 0 failed | PASS |
| All commit hashes exist | `git log --oneline -1 {hash}` for 230fdfe, 75f631b, 9eb6c37, b312022 | All 4 found | PASS |
| CLI path tool surface matches gateway | Manual comparison of run_single/run_chat vs run_gateway registration order | Identical: memory, delegate_task, cron, skills, rpc_registry, execute_code, guardrails, Arc wrap | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| CLI-01 | 22-01, 22-02 | CLI registers execute_code, hooks, and guardrails (feature parity with gateway) | SATISFIED | execute_code (lines 327, 587), HookRegistry (lines 344-364, 604-634), BlocklistGuardrail (lines 336, 596), error_detail (lines 340, 600) all present in both CLI paths |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| (none) | - | - | - | No TODO, FIXME, placeholder, or stub patterns found in modified files |

### Human Verification Required

No human verification items identified. All wiring is structurally verifiable through code inspection and static-grep tests.

### Gaps Summary

No gaps found. All 14 must-haves verified. Both CLI paths (run_chat and run_single) achieve full tool-level and event-hook parity with run_gateway:

1. **Tool registration parity:** cron_tool, skills_tool, execute_code_tool (with shared active_skills Arc and safe-subset RPC registry), BlocklistGuardrail, and error_detail all registered before Arc wrap in both paths.
2. **HookRegistry parity:** HookRegistry constructed with JSONL listener and webhook listeners, wired into AgentLoop via `.with_hook_registry()` and into attach_context_engine via `Some(hook_registry.clone())` in both paths.
3. **Retry queue drain:** Both paths drain the persistent retry queue on startup.
4. **Regression protection:** 4 static-grep tests lock all wiring calls against accidental removal.

---

_Verified: 2026-04-17T21:15:00Z_
_Verifier: Claude (gsd-verifier)_
