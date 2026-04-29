---
phase: 25-toolset-management
plan: "03"
subsystem: ironhermes-core, ironhermes-tools, ironhermes-agent, ironhermes-cli
tags: [toolset-config, toolsconfig, registry-filter, with-intercepts, dispatch-intercepts, d-15-collision, d-20-defaults, d-23-filter-order, d-26-test2, d-26-test3, phase-25]
dependency_graph:
  requires: [25-01, 25-02]
  provides: [ToolsConfig, ToolsetEntry, DEFAULT_TOOLSETS, set_toolset_config, get_definitions filter chain, with_intercepts builder, D-26 Test 2, D-26 Test 3 live]
  affects:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/constants.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/tests/toolset_prereq.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-cli/src/main.rs
tech_stack:
  added: []
  patterns:
    - D-23 4-layer filter chain (toolset → is_available → enabled_tools → disabled)
    - try_write() instead of blocking_write() for async-safe builder pattern
    - Source-text invariant tests using runtime-constructed forbidden strings
key_files:
  created:
    - crates/ironhermes-tools/tests/toolset_prereq.rs
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/constants.rs
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-cli/src/main.rs
decisions:
  - "D-20 defaults locked in ToolsConfig::default(): memory/session/agent/skills enabled; web/code disabled"
  - "D-22/D-23 filter order: toolset → is_available → enabled_tools → disabled (4-layer chain)"
  - "D-A2 mitigation: registry without toolset_config preserves all-tools-on behavior"
  - "D-12/D-14 migration: session_search has exactly one schema source (registry intercepts)"
  - "D-16 with_intercepts builder uses try_write() (works in both sync and async context)"
  - "delegate_task registered as intercept stub — full DelegateTaskTool wiring deferred to Plan 4"
  - "blocking_write() in with_intercepts replaced by try_write() — avoids panic inside tokio runtime"
metrics:
  duration_minutes: 13
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_modified: 8
---

# Phase 25 Plan 03: Config + Registry Filter + Agent Loop Migration Summary

**One-liner:** ToolsConfig + DEFAULT_TOOLSETS in ironhermes-core; toolset_config filter chain in ToolRegistry; with_intercepts builder + session_search/todo migration in AgentLoop; D-26 Tests 2+3 passing

## What Was Built

### Task 1: ToolsConfig + DEFAULT_TOOLSETS (ironhermes-core)

**`crates/ironhermes-core/src/config.rs`:**
- Added `ToolsetEntry { enabled: bool }` struct with `#[serde(default)]`
- Added `ToolsConfig` with `toolsets: HashMap<String, ToolsetEntry>`, `skip_prompts: Vec<String>`, `disabled: Vec<String>`
- Implemented `Default for ToolsConfig` per D-20: memory/session/agent/skills enabled; web/code disabled
- Added `is_toolset_enabled(&self, name: &str) -> bool` — unknown names default to false (D-23 opt-in)
- Added `pub tools: ToolsConfig` field to `Config` struct with `#[serde(default)]` (D-24 silent migration)

**`crates/ironhermes-core/src/constants.rs`:**
- Added `DEFAULT_TOOLSETS: &[&str] = &["memory", "session", "agent", "skills"]` (D-20)

**`crates/ironhermes-core/src/lib.rs`:**
- Exported `ToolsetEntry` and `ToolsConfig` from crate root

**5 tests added:**
- `tools_config_default_has_correct_enabled_set` — D-20 defaults
- `tools_config_unknown_toolset_defaults_to_disabled` — D-23 opt-in semantics
- `tools_config_serde_roundtrip_preserves_enabled_state` — YAML roundtrip
- `config_with_default_tools_field_loads_with_no_tools_block` — D-24 silent migration
- `default_toolsets_constant_matches_d20` — DEFAULT_TOOLSETS content

### Task 2: Registry Toolset Filter + D-26 Test 2 (ironhermes-tools)

**`crates/ironhermes-tools/src/registry.rs`:**
- Added `toolset_config: Option<ironhermes_core::config::ToolsConfig>` field (initialized to None)
- Added `pub fn set_toolset_config()` setter
- Added `intercepted_owner_toolset(name: &str) -> &'static str` helper (D-13 mapping)
- Updated `get_definitions()` with D-23 rustdoc and 4-layer filter chain:
  1. toolset_config.is_toolset_enabled(t.toolset()) (when Some; None = no filter per A2/Pitfall 8)
  2. t.is_available()
  3. enabled_tools list filter
  4. toolset_config.disabled list override
- Same 4 filters applied to intercepts via intercepted_owner_toolset()

**`crates/ironhermes-tools/tests/toolset_prereq.rs`:** (new file)
- D-26 Test 2: `tool_excluded_when_prereq_missing` — env_lock + FIRECRAWL_API_KEY toggle

**5 unit tests added + 1 integration test:**
- `set_toolset_config_then_get_definitions_filters_by_toolset`
- `get_definitions_no_config_preserves_existing_behavior` (D-A2/Pitfall 8)
- `get_definitions_per_tool_disabled_filter` (D-23 layer 4)
- `get_definitions_intercepted_owner_toolset_mapping`
- `with_intercepts_does_not_collide_with_regular_registration` (D-15 guard)
- D-26 Test 2: `tool_excluded_when_prereq_missing` (integration)

### Task 3: Agent Loop Migration + with_intercepts Builder (ironhermes-agent + ironhermes-cli)

**`crates/ironhermes-agent/src/agent_loop.rs`:**
- Removed `tool_schemas.push(session_search_schema())` injection from `run()` (D-14)
- Replaced hardcoded `if name == "session_search"` block with `dispatch_intercepts()` call (D-12)
- Added `with_intercepts()` builder registering: session_search, memory (when handle provided), delegate_task stub, todo_read, todo_write
- Uses `try_write()` instead of `blocking_write()` (async-safe for test contexts)

**`crates/ironhermes-tools/src/lib.rs`:**
- Exported `InterceptHandler`, `Prerequisite`, `todo_read_schema`, `todo_write_schema`

**`crates/ironhermes-cli/src/main.rs`:**
- Wired `with_intercepts(None, None, None, Some(todo_state), None)` in `run_single`
- Wired `with_intercepts(None, None, None, Some(todo_state), None)` in `run_agent_turn`

**5 tests added:**
- `agent_loop_with_intercepts_registers_session_search`
- `agent_loop_with_intercepts_registers_todo_pair`
- `agent_loop_session_search_schema_injection_removed` (source-text invariant)
- `agent_loop_session_search_match_block_removed` (source-text invariant)
- `intercepted_no_duplicate_with_real_handlers` (D-26 Test 3 live-handler version)

## D-26 Test Status

| Test | Location | Status |
|------|----------|--------|
| Test 1: `toolset_enable_disable_persists` | Plan 4 (hermes toolset CLI) | Deferred |
| Test 2: `tool_excluded_when_prereq_missing` | `ironhermes-tools/tests/toolset_prereq.rs` | PASSING |
| Test 3 (stub): `intercepted_tool_no_schema_duplicate` | `ironhermes-tools/src/registry.rs` | PASSING |
| Test 3 (live): `intercepted_no_duplicate_with_real_handlers` | `ironhermes-agent/src/agent_loop.rs` | PASSING |

## Critical Constraint Verification

| Constraint | Status |
|-----------|--------|
| D-15 collision: no `registry.register(Box::new((MemoryTool\|DelegateTaskTool\|CronjobTool)::new` | PASS |
| Rustdoc: `get_definitions` contains "no toolset filter is applied" | PASS |
| main.rs disclosure: `with_intercepts` present in main.rs | PASS |
| Pitfall 8/A2: `get_definitions(None)` with no toolset_config shows all tools | PASS |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] blocking_write() panics inside tokio runtime**
- **Found during:** Task 3 GREEN phase — agent tests failed with "Cannot block the current thread from within a runtime"
- **Issue:** `self.registry.blocking_write()` in `with_intercepts()` panics when called from `#[tokio::test]` async tests
- **Fix:** Changed to `try_write().expect(...)` — non-blocking, always succeeds during construction phase (no contention), works in both sync and async contexts
- **Files modified:** `crates/ironhermes-agent/src/agent_loop.rs`

**2. [Rule 1 - Bug] Source-text invariant tests had false positives**
- **Found during:** Task 3 GREEN phase — tests `agent_loop_session_search_schema_injection_removed` and `agent_loop_session_search_match_block_removed` failed because the forbidden strings appeared in the test assertion messages themselves
- **Fix:** Construct forbidden strings at runtime via array concat so they don't appear as string literals in source
- **Files modified:** `crates/ironhermes-agent/src/agent_loop.rs`

**3. [Rule 3 - Blocking] todo_read_schema/todo_write_schema not exported from ironhermes-tools**
- **Found during:** Task 3 build phase — `error[E0425]: cannot find function todo_read_schema in crate ironhermes_tools`
- **Fix:** Added `todo_read_schema`, `todo_write_schema`, `InterceptHandler`, `Prerequisite` to `pub use registry::...` in `lib.rs`
- **Files modified:** `crates/ironhermes-tools/src/lib.rs`

**4. [Rule 3 - Blocking] MemoryTool::schema() required Tool trait in scope**
- **Found during:** Task 3 build — `no method named schema found for struct MemoryTool in the current scope`
- **Fix:** Added `use ironhermes_tools::Tool as _;` in scope before the schema() call
- **Files modified:** `crates/ironhermes-agent/src/agent_loop.rs`

**5. [Scoped deviation] delegate_task intercept uses stub schema + handler**
- **Reason:** `DelegateTaskTool` requires a `SubagentRunner` + `Semaphore` + config in its constructor; these aren't available in the `with_intercepts` builder (which receives only a `Arc<dyn SubagentRunner>` option). Full DelegateTaskTool registration with all context is a Plan 4 task.
- **Impact:** `delegate_task` is registered as an intercept with a minimal schema and a stub handler. The D-15 panic guard is satisfied (no name in both maps). Plan 4 will replace the stub with the live DelegateTaskTool wiring.

**6. [Scoped deviation] memory intercept passes through MemoryManagerHandle::handle_tool_call**
- **Reason:** The `with_intercepts` builder receives `Option<Arc<Mutex<MemoryManager>>>` but main.rs currently registers memory via `register_memory_tool()` (which puts it in the regular tools map). To avoid D-15 collision, memory is passed as `None` at all current call sites in main.rs. The memory intercept registration code exists in `with_intercepts` but is only activated when `Some(memory_manager)` is passed.
- **Impact:** Existing behavior preserved — memory tool continues to work via regular registry. Plan 4 will migrate to intercept-only path.

### Pre-existing Out-of-Scope Issues

- `memory-sqlite`, `memory-duckdb`, `memory-grafeo` provider crates fail with `missing field 'cache_breaking'` — confirmed pre-existing on base commit (documented in 25-02 SUMMARY). Not introduced by this plan.

## Known Stubs

- `delegate_task` intercept handler in `with_intercepts()` returns `{"error":"not_wired","reason":"delegate_task intercept stub — full wiring in Plan 4"}`. This is intentional — Plan 4 (operator surface) will replace with live DelegateTaskTool dispatch. The schema stub in place allows the D-15 guard to function and the D-26 Test 3 shape to be validated.
- `with_intercepts(None, None, None, Some(todo_state), None)` in main.rs passes `None` for memory_manager, state_store, subagent_runner, cron_router — these are wired in future plans. Currently todo_write/todo_read are the only new intercepts active in the binary.

## Threat Flags

None. Plan 3 adds no new operator-facing surface, no I/O, no config writes, no untrusted-input parsing. T-25-04 mitigated via `with_intercepts()` signature type-restricted to workspace-internal handles. Operator-facing `hermes toolset` CLI is still pending (Plans 4 + 5).

## Commits

| Hash | Description |
|------|-------------|
| `b41e764` | feat(25-03): add ToolsConfig + ToolsetEntry + DEFAULT_TOOLSETS to ironhermes-core |
| `7fd6da2` | feat(25-03): wire toolset_config filter into ToolRegistry + D-26 Test 2 |
| `049005b` | feat(25-03): migrate agent_loop to dispatch_intercepts + add with_intercepts builder |

## Self-Check: PASSED

- `crates/ironhermes-core/src/config.rs` — contains `pub struct ToolsetEntry` ✓
- `crates/ironhermes-core/src/config.rs` — contains `pub struct ToolsConfig` ✓
- `crates/ironhermes-core/src/config.rs` — contains `pub fn is_toolset_enabled` ✓
- `crates/ironhermes-core/src/config.rs` — contains `tools: ToolsConfig` ✓
- `crates/ironhermes-core/src/constants.rs` — contains `DEFAULT_TOOLSETS` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `toolset_config: Option<ironhermes_core::config::ToolsConfig>` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub fn set_toolset_config` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `fn intercepted_owner_toolset` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `is_toolset_enabled` call site ✓
- `crates/ironhermes-tools/src/registry.rs` — contains "no toolset filter is applied" ✓
- `crates/ironhermes-tools/tests/toolset_prereq.rs` — contains `tool_excluded_when_prereq_missing` ✓
- `crates/ironhermes-tools/tests/toolset_prereq.rs` — contains `env_lock` ✓
- `crates/ironhermes-agent/src/agent_loop.rs` — contains `pub fn with_intercepts` ✓
- `crates/ironhermes-agent/src/agent_loop.rs` — session_search injection count = 0 ✓
- `crates/ironhermes-agent/src/agent_loop.rs` — hardcoded match count = 0 ✓
- `crates/ironhermes-agent/src/agent_loop.rs` — contains `dispatch_intercepts` ✓
- `crates/ironhermes-agent/src/agent_loop.rs` — contains `register_intercepted` (8 occurrences) ✓
- `crates/ironhermes-cli/src/main.rs` — no direct register for MemoryTool/DelegateTaskTool/CronjobTool ✓
- `crates/ironhermes-cli/src/main.rs` — contains `with_intercepts` ✓
- Commit `b41e764` exists ✓
- Commit `7fd6da2` exists ✓
- Commit `049005b` exists ✓
