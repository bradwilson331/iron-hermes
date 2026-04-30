---
phase: 25-toolset-management
verified: 2026-04-30T01:15:00Z
status: gaps_found
score: 4/5 must-haves verified
overrides_applied: 0
gaps:
  - truth: "Agent-intercepted tools (memory, session_search, delegate_task) are handled before registry dispatch without being visible to the LLM as duplicates"
    status: partial
    reason: >
      The registry infrastructure is fully implemented (D-15 panic guards, register_intercepted,
      dispatch_intercepts, D-26 Test 3 passes). However, session_search is broken in the live
      binary: Phase 25 Plan 03 removed the old hardcoded schema injection (agent_loop.rs line ~479)
      and the hardcoded dispatch match block (agent_loop.rs line ~951), but the new intercept path
      requires state_store=Some(...) passed to with_intercepts() — which is never done in main.rs.
      Both run_single (line 780) and run_agent_turn (line 1964) pass None for state_store.
      The comment at line 1964 reads "state_store: session_search wiring in Plan 4" but neither
      Plan 4 nor Plan 5 completed this wiring. session_search was working before Phase 25 and is
      now a regression: the LLM cannot see the session_search tool schema and any call to it will
      fail at dispatch.
    artifacts:
      - path: "crates/ironhermes-cli/src/main.rs"
        issue: "with_intercepts called with None for state_store at lines 780 and 1964; session_search never registered as intercept in live binary"
      - path: "crates/ironhermes-agent/src/agent_loop.rs"
        issue: "self.state_store field set by with_state_store() but never read (only 1 reference: the setter at line 287); old hardcoded session_search paths removed; new intercept path requires state_store=Some(...)"
    missing:
      - "Pass Arc<Mutex<StateStore>> to with_intercepts() in run_single and run_agent_turn in main.rs"
      - "Wire the state_store variable (already created at lines 594 and 950) into the with_intercepts() call as the second argument"
---

# Phase 25: Toolset Management Verification Report

**Phase Goal:** Tools are organized into named toolsets with runtime enable/disable, prerequisite check functions that silently exclude unavailable tools from the LLM schema, and a setup wizard hook that guides users through missing tool prerequisites.
**Verified:** 2026-04-30T01:15:00Z
**Status:** GAPS FOUND
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Each tool has an `is_available()` check; tools whose prerequisites (env vars, API keys) are absent are silently excluded from the schema sent to the LLM | VERIFIED | `Prerequisite` struct + `prerequisites()` method on `Tool` trait in `registry.rs:13,47`. Default `is_available()` walks prerequisites at `registry.rs:34`. `get_definitions()` filters `is_available()` at line 249. D-26 Test 2 (`tool_excluded_when_prereq_missing`) passes: 1/1 |
| 2 | Tools are grouped into named toolsets and operator can list/enable/disable a toolset at runtime | VERIFIED | `hermes toolset list/enable/disable/show` implemented in `toolset_cmd.rs`. `Commands::Toolset` dispatch in `main.rs:391`. Persistent config writes via `config_setter::config_set`. D-26 Test 1 (`toolset_enable_disable_persists`) passes: 5/5 integration tests green |
| 3 | Adding a new tool requires only a registration call — no changes to dispatch logic | VERIFIED | `ToolRegistry::register()` and `register_intercepted()` in `registry.rs`. Dispatch is fully registry-driven via `dispatch_intercepts()` then `dispatch()`. D-15 panic guards structurally enforce single registration |
| 4 | Agent-intercepted tools (memory, session_search, delegate_task) are handled before registry dispatch without being visible to the LLM as duplicates | PARTIAL — FAILED | Infrastructure verified: `intercepts` HashMap, `register_intercepted`, `dispatch_intercepts`, D-15 guards, D-26 Test 3 passes. REGRESSION: `session_search` old hardcoded paths removed by Plan 03 but new intercept path not wired in `main.rs`. `state_store` passed as `None` at both call sites. session_search is invisible to the LLM and will fail at dispatch. `delegate_task` and `memory` remain in regular tool registry (no intercept in live binary) — not a regression, they function correctly via `register_delegate_task_tool` / `register_memory_tool` |
| 5 | `hermes setup` (or first-run wizard) detects tools with missing prerequisites and guides the user through configuring them | VERIFIED | `run_tools_section` real impl in `setup.rs:464`. `hermes toolset setup` via `cmd_toolset_setup` in `toolset_cmd.rs:48`. Preflight `emit_prereq_banner` in `preflight.rs:44,52`. D-19 opt-in stage in `setup.rs:153-163`. T-25-02 secret masking via `is_secret_prereq_name` + `write_env_var_to_dotenv` 0600 chmod. All 3 preflight tests pass; 5/5 integration tests pass |

**Score:** 4/5 truths verified (1 partial — BLOCKER)

---

## Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-tools/src/registry.rs` | `Prerequisite` struct, `prerequisites()`, `is_available()` default, `register_intercepted`, `dispatch_intercepts`, `list_unavailable`, `list_toolsets`, `set_toolset_config` | VERIFIED | All 8 items confirmed present with correct signatures |
| `crates/ironhermes-core/src/config.rs` | `ToolsConfig`, `ToolsetEntry`, `is_toolset_enabled` | VERIFIED | Present at lines 7-50; `pub tools: ToolsConfig` on `Config` struct at line 150 |
| `crates/ironhermes-core/src/constants.rs` | `DEFAULT_TOOLSETS` | VERIFIED | `&["memory", "session", "agent", "skills"]` |
| `crates/ironhermes-cli/src/toolset_cmd.rs` | `ToolsetSubcommand` enum, CLI handlers, `validate_toolset_name` | VERIFIED | All subcommands including `Setup` variant present; cache-break banner on stderr |
| `crates/ironhermes-core/src/commands/toolset_display.rs` | `render_toolset_list`, `render_toolset_show` | VERIFIED | Pure render helpers in leaf crate (no reverse dep) |
| `crates/ironhermes-core/src/commands/context.rs` | `ToolsetSessionHandle` trait | VERIFIED | Trait defined at line 116; `toolset_session: Option<Arc<dyn ToolsetSessionHandle>>` at line 282. NOTE: no concrete impl wired into live REPL — `/toolset` slash returns informational fallback |
| `crates/ironhermes-cli/src/setup.rs` | `run_tools_section` real impl, `is_secret_prereq_name`, `write_env_var_to_dotenv` | VERIFIED | All present; stub replaced with real prereq-walking logic |
| `crates/ironhermes-cli/src/preflight.rs` | `emit_prereq_banner`, `list_unavailable` call, no auto-wizard launch | VERIFIED | Banner emitted via writer-injection seam; no `run_tools_section` call in preflight |
| `crates/ironhermes-tools/tests/toolset_prereq.rs` | D-26 Test 2 | VERIFIED | `tool_excluded_when_prereq_missing`: 1/1 PASS |
| `crates/ironhermes-cli/tests/toolset_integration.rs` | D-26 Test 1 + T-25-01 + T-25-03 + T-25-02 + D-17 integration | VERIFIED | 5/5 PASS |
| `crates/ironhermes-agent/src/agent_loop.rs` | `with_intercepts` builder, `dispatch_intercepts` call, session_search injection removed | PARTIAL | `with_intercepts` and `dispatch_intercepts` call present. Old injection removed. BUT `with_intercepts(None, None, ...)` in main.rs means session_search is never registered as intercept |

---

## Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `get_definitions()` | `is_available()` | filter in `registry.rs:249` | WIRED | Confirmed in source |
| `get_definitions()` | `toolset_config.is_toolset_enabled()` | D-23 4-layer chain | WIRED | Layers 1-4 verified in source |
| `hermes toolset enable` | `config.yaml` | `config_setter::config_set` | WIRED | D-26 Test 1 subprocess round-trip confirms |
| `with_intercepts(state_store=Some)` | `register_intercepted("session_search")` | guarded `if let Some(state)` | ORPHANED | Only fires when state_store=Some; main.rs passes None |
| `run_agent_turn` | `session_search` intercept | via `with_intercepts` | NOT_WIRED | Comment at main.rs:1964 "session_search wiring in Plan 4" — never completed |
| `cmd_toolset_setup` | `run_tools_section` | `setup.rs:83` | WIRED | Prereq-walking loop confirmed |
| `preflight::run_preflight_check` | `emit_prereq_banner` | `list_unavailable()` | WIRED | D-17 probe confirmed in preflight.rs |
| D-15 `register()` | panic on intercept collision | `registry.rs:102,116,142` | WIRED | Both directions panic-guarded with tests |

---

## Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `get_definitions()` | `schemas` from `tools` + `intercepts` | `is_available()` → toolset filter → enabled_tools | Yes — real env var checks | FLOWING |
| `cmd_toolset_list` | `rows` via `build_toolset_rows` | `Config::load` + `registry.list_unavailable()` | Yes — reads config.yaml + checks env vars | FLOWING |
| `run_tools_section` | `unavailable` list | `registry.list_unavailable()` | Yes — real env var presence checks | FLOWING |
| session_search schema | LLM tool_schemas | `registry.get_definitions()` via intercept | No — intercept never registered | DISCONNECTED |

---

## Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| D-26 Test 1: toolset_enable_disable_persists | `cargo test -p ironhermes-cli --test toolset_integration` | 5/5 pass in 5.23s | PASS |
| D-26 Test 2: tool_excluded_when_prereq_missing | `cargo test -p ironhermes-tools --test toolset_prereq` | 1/1 pass | PASS |
| D-26 Test 3: intercepted_tool_no_schema_duplicate | ironhermes-tools lib tests (179 pass) | 179/179 pass | PASS |
| D-26 Test 3 live: intercepted_no_duplicate_with_real_handlers | ironhermes-agent lib tests (228 pass) | 228/228 pass | PASS |
| Preflight banner tests | `cargo test -p ironhermes-cli --bin ironhermes -- preflight` | 3/3 pass | PASS |
| Build: target crates compile | `cargo build -p ironhermes-tools -p ironhermes-core -p ironhermes-agent -p ironhermes-cli` | exit 0 in 4.37s | PASS |

---

## Requirements Coverage

| Requirement | Description | Status | Evidence |
|------------|-------------|--------|----------|
| TOOL-01 | Tool trait includes `is_available()` check; tools silently excluded from schema when prerequisites absent | SATISFIED | `prerequisites()` + `is_available()` default walk + `get_definitions()` filter chain |
| TOOL-02 | Tools organized into named toolsets with platform-specific presets | SATISFIED | 6 D-01 toolsets, `hermes toolset list/enable/disable/show`, D-20 defaults |
| TOOL-03 | Tool registration happens at import/init time via registry — adding a tool requires registration call only | SATISFIED | `register()` / `register_intercepted()` API; dispatch fully registry-driven |
| TOOL-04 | Agent-intercepted tools (memory, session_search, delegate_task, todo) handled before registry dispatch | BLOCKED | Infrastructure exists; session_search regression: old paths removed, new intercept not wired in main.rs |
| TOOL-05 | Setup wizard checks tool availability and guides users through missing prerequisites | SATISFIED | `hermes toolset setup` + `hermes setup` D-19 stage + preflight D-17 banner |

---

## Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `crates/ironhermes-agent/src/agent_loop.rs:382` | `{"error":"not_wired","reason":"delegate_task intercept stub — full wiring in Plan 4"}` | Warning | delegate_task intercept handler returns error stub — but the regular tool path handles delegate_task correctly, so no regression |
| `crates/ironhermes-cli/src/main.rs:1964` | `None, // state_store: session_search wiring in Plan 4` — never completed | Blocker | session_search completely unwired in live binary; LLM cannot use session search in chat mode |
| `crates/ironhermes-cli/src/main.rs:779` | `None, None, None, Some(todo_state_single), None` — state_store=None in run_single | Blocker | session_search also unwired in batch mode |

---

## Human Verification Required

None — all gaps are programmatically verifiable and confirmed.

---

## Gaps Summary

### BLOCKER: session_search regression in live binary

**Root cause:** Phase 25 Plan 03 correctly removed the old hardcoded session_search schema injection and dispatch block from `agent_loop.rs`. The replacement is the intercept registry pattern (`with_intercepts(state_store=Some(...))`). However, Plans 4 and 5 were completed without ever passing the `state_store` to `with_intercepts()` in `main.rs`.

**Evidence:**
- `main.rs:779`: `.with_intercepts(None, None, None, Some(todo_state_single), None)` — state_store=None
- `main.rs:1962-1965`: `.with_intercepts(None, None, None, Some(todo_state), None)` — state_store=None
- `agent_loop.rs:287`: `self.state_store = Some(store)` — the only reference to `state_store` field; the field is set but never read
- `agent_loop.rs:318-330`: `if let Some(state) = state_store { reg.register_intercepted("session_search", ...) }` — guarded, never fires

**Fix:** In both `run_single` and `run_agent_turn` in `main.rs`, wrap the existing `state_store` variable in `Arc<std::sync::Mutex<>>` and pass it as the second argument to `with_intercepts()`.

**Other known stubs (not blockers):**
- `ToolsetSessionHandle` not wired into live REPL `CommandContext` → `/toolset` slash command returns informational fallback. The CLI subcommand path works correctly. This is an intentional deferral per Plan 04.
- `delegate_task` uses regular tool registry (not intercepted) — this is not a regression and the tool functions correctly.
- 3 pre-existing `ironhermes-core` lib test failures: `dispatch_all_todo_stubs_return_not_yet_available` (pre-Phase-25, documented in deferred-items.md), `provider_resolver_loads_disk_cache_at_build` and `provider_resolver_populates_model_metadata` (test-ordering artifacts — both pass in isolation).

---

_Verified: 2026-04-30T01:15:00Z_
_Verifier: Claude (gsd-verifier)_
