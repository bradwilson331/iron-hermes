---
phase: 25-toolset-management
plan: "02"
subsystem: ironhermes-tools
tags: [tool-registry, intercept-handler, dispatch-intercepts, todo-schemas, d-15-panic-guard, d-26-test3, phase-25]
dependency_graph:
  requires: [25-01]
  provides: [InterceptHandler type alias, register_intercepted, dispatch_intercepts, list_unavailable, list_toolsets, todo_write_schema, todo_read_schema, D-26 Test 3]
  affects: [ironhermes-tools/registry.rs, ironhermes-tools/Cargo.toml]
tech_stack:
  added: [futures = { workspace = true } in ironhermes-tools]
  patterns: [BoxFuture async intercept dispatch, Arc<dyn Fn> handler pattern, HashMap intercepts segregation]
key_files:
  created: []
  modified:
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/Cargo.toml
decisions:
  - "Open Question 2 resolved: todo_write schema requires items array-of-strings; todo_read has no required params"
  - "Open Question 3 resolved: InterceptHandler is async via futures::future::BoxFuture<'static, ...>"
  - "D-15 panic guards applied to register(), register_dynamic(), and register_intercepted()"
  - "get_definitions() extended to union intercepts map with same enabled_tools filter (moved into Task 1 to satisfy Task 1 test)"
  - "Pre-existing workspace build failures in memory-sqlite/duckdb/grafeo are out-of-scope; main crates compile cleanly"
metrics:
  duration_minutes: 25
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_modified: 2
---

# Phase 25 Plan 02: Intercept Infrastructure + Registry Surface Summary

**One-liner:** Intercept registry surface — InterceptHandler BoxFuture type alias, register_intercepted/dispatch_intercepts methods, D-15 structural panic guards, get_definitions() union, list_unavailable/list_toolsets, todo_write/todo_read schemas, D-26 Test 3 locked

## What Was Built

All changes are in `crates/ironhermes-tools/src/registry.rs` and `crates/ironhermes-tools/Cargo.toml`.

### 1. `futures` Dependency Added (Cargo.toml)

`futures = { workspace = true }` added alphabetically between `dirs` and `anyhow` in `[dependencies]`. Workspace root already declares `futures = "0.3"`.

### 2. `InterceptHandler` Type Alias (D-12 / D-14 / Open Question 3)

```rust
pub type InterceptHandler = std::sync::Arc<
    dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, anyhow::Result<String>>
        + Send + Sync,
>;
```

Placed after the `Prerequisite` struct and before `ToolRegistry`. The type alias is `async` via `BoxFuture` per Open Question 3's resolution — required because `execute_tool_call()` is async and `spawn_blocking` for sync `StateStore` stays inside the closure body (Plan 3's session_search migration).

Doc comment explicitly states `library-internal use only` per T-25-04 mitigation.

### 3. `intercepts` Field on `ToolRegistry` (D-14)

```rust
intercepts: HashMap<String, (ToolSchema, InterceptHandler)>,
```

Initialized to `HashMap::new()` in `ToolRegistry::new()`. Intercepted tools are stored separately from the regular `tools` map — this is the structural foundation that makes schema duplication (D-15) impossible.

### 4. D-15 Panic Guards (Bidirectional)

- `register()` — asserts `!self.intercepts.contains_key(&name)` with message containing `"already registered as an intercepted tool"`
- `register_dynamic()` — same guard (MCP-discovered tools also cannot collide with intercept names)
- `register_intercepted()` — asserts `!self.tools.contains_key(name)` with message containing `"already registered as a regular tool"`

Both directions are tested with `#[should_panic(expected = ...)]` tests.

### 5. `register_intercepted()` Method

Stores `(schema, handler)` tuple in `self.intercepts`. Includes D-15 reciprocal guard and T-25-04 library-internal-only doc comment. Handler closures must be constructed by `ironhermes-agent::AgentLoop::with_intercepts()` in Plan 3, not deserialized from config or user input.

### 6. `dispatch_intercepts()` Method (async)

Returns `Option<anyhow::Result<String>>`:
- `Some(result)` when name is in the intercepts map
- `None` to fall through to the normal `dispatch()` path

Plan 3's `agent_loop::execute_tool_call()` will call this first and only fall through to `dispatch()` on `None`.

### 7. `get_definitions()` Extended to Union Both Maps (D-14)

Existing signature `pub fn get_definitions(&self, enabled_tools: Option<&[String]>) -> Vec<ToolSchema>` is unchanged (Pitfall 5 — 3+ call sites pass `None`). Body now collects from `self.tools` (filtered by `is_available()` + enabled_tools) then extends with `self.intercepts.iter()` applying the same `enabled_tools` filter.

Note: this was moved into the Task 1 commit (rather than Task 2) to satisfy Task 1's `register_intercepted_inserts_schema_and_handler` test which validates via `get_definitions(None)`.

### 8. `list_unavailable()` Method

Returns `Vec<(String, Vec<Prerequisite>)>` — tool name paired with the list of required-but-missing prerequisites. Only checks `kind == "env_var"` (D-08 / D-09: `config_field` checked at config load). Used by Plan 5's preflight banner (D-17).

### 9. `list_toolsets()` Method

Returns sorted, deduplicated `Vec<String>` of `toolset()` values from all regular tools. Per D-03: source of truth is `Tool::toolset()` — no separate registry table. Only includes `self.tools` entries; intercepted tools have no `toolset()` because they are not `Tool` impls.

### 10. `todo_write_schema()` and `todo_read_schema()` Free Functions (D-13 / Open Question 2)

Public free functions placed before the `#[cfg(test)]` block:

- `todo_write_schema()`: name = `"todo_write"`, required field `items` (array of strings) — replaces the session todo list
- `todo_read_schema()`: name = `"todo_read"`, no required parameters — returns current list

Not registered in `ToolRegistry::new()` — Plan 3 wires real handlers via `AgentLoop::with_intercepts()` (D-16).

## Open Questions Resolved

| Question | Resolution |
|----------|-----------|
| Open Question 2 | `todo_write` requires `items: [string]`; `todo_read` takes no args |
| Open Question 3 | `InterceptHandler` is async via `futures::future::BoxFuture<'static, ...>` |

## Unit Tests Added (13 total across 3 tasks)

| Test Name | Task | What It Covers |
|-----------|------|----------------|
| `register_intercepted_inserts_schema_and_handler` | 1 | Intercept appears in get_definitions(None) |
| `register_intercepted_panics_on_duplicate_with_tools` | 1 | D-15 guard — regular→intercept collision panics |
| `register_tools_panics_on_duplicate_with_intercepts` | 1 | D-15 guard — intercept→regular collision panics (reciprocal) |
| `dispatch_intercepts_returns_some_for_known` | 1 | Returns Some(Ok("hello")) for known name |
| `dispatch_intercepts_returns_none_for_unknown` | 1 | Returns None for unregistered name |
| `get_definitions_includes_intercept_schemas` | 2 | Union: regular + 2 intercepts = 3 schemas |
| `get_definitions_with_enabled_tools_filter_includes_intercepts` | 2 | enabled_tools filter applies to both maps |
| `get_definitions_filters_unavailable_regular_tools_only` | 2 | Unavailable regular tools filtered; intercepts always shown |
| `list_unavailable_returns_missing_required_prereqs` | 2 | Returns 1 entry when MISSING_KEY_25_02 absent; empty when set |
| `list_toolsets_returns_unique_set` | 2 | ["code", "web"] from 3 tools with duplicated "web" |
| `todo_write_schema_minimal_shape` | 3 | items field is required array-of-strings |
| `todo_read_schema_minimal_shape` | 3 | Empty properties and no required fields |
| `intercepted_tool_no_schema_duplicate` | 3 | D-26 Test 3: all 6 intercepted names appear exactly once |

All 174 `ironhermes-tools` lib tests pass (161 from Plan 1 + 13 new from Plan 2).

## Commits

| Hash | Description |
|------|-------------|
| `0a496f6` | feat(25-02): add InterceptHandler type alias, intercepts field, register_intercepted/dispatch_intercepts, D-15 panic guards |
| `d5b5c2b` | feat(25-02): extend get_definitions() intercept union + add list_unavailable + list_toolsets |
| `c0df1e4` | feat(25-02): add todo_write_schema/todo_read_schema constructors + D-26 Test 3 intercepted_tool_no_schema_duplicate |

## Plan 3 Deferred Items

The following were explicitly deferred to Plan 3 per the plan's action items:

- `toolset_config: Option<ToolsConfig>` field on `ToolRegistry`
- `toolset_enabled()` helper
- `AgentLoop::with_intercepts()` builder method
- Real handlers for memory, session_search, delegate_task, todo_write, todo_read, cronjob
- `agent_loop.rs` migration: collapse hardcoded session_search block into `dispatch_intercepts()` call site
- D-26 Test 3 re-verification with live handlers (Plan 3 runs the same assertion shape against real wiring)

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] get_definitions() intercept union moved from Task 2 into Task 1 commit**
- **Found during:** Task 1 GREEN phase — `register_intercepted_inserts_schema_and_handler` test validates via `get_definitions(None)`
- **Issue:** Task 1's test requires `get_definitions()` to return intercept schemas, but the plan placed that code change in Task 2. The test would have failed at GREEN phase without the union step.
- **Fix:** Extended `get_definitions()` in the Task 1 commit (`0a496f6`) instead of Task 2. Task 2's commit still contains `list_unavailable()`, `list_toolsets()`, and the Task 2-specific tests as planned.
- **Impact:** None — the behavioral outcome is identical; only the commit grouping differs.
- **Commit:** `0a496f6`

**2. [Out of scope] Pre-existing workspace build failures in provider crates**
- **Found during:** Final `cargo build --workspace` verification
- **Issue:** `memory-sqlite`, `memory-duckdb`, and `memory-grafeo` providers fail with `missing field 'cache_breaking' in initializer of ConfigField` — confirmed pre-existing on base commit `e6fb8da` before any Plan 02 changes
- **Action:** Logged to deferred-items. `cargo build -p ironhermes-tools -p ironhermes-agent -p ironhermes-cli` exits 0. Zero new errors introduced.
- **Files modified:** None (out of scope per deviation rule scope boundary)

## Known Stubs

- `todo_write_schema()` and `todo_read_schema()` are schema-only constructors with no handler logic. By design — Plan 3 wires real handlers. The schemas are complete and non-stub; only the dispatch handlers are deferred.

## Threat Flags

None. Plan 2 adds no operator-facing surface, no I/O, no config writes, no new untrusted-input parsing. T-25-04 mitigated via `register_intercepted()` doc comment explicitly stating library-internal-only constraint.

## Self-Check: PASSED

- `crates/ironhermes-tools/Cargo.toml` — contains `futures = { workspace = true }` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub type InterceptHandler` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `futures::future::BoxFuture` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `intercepts: HashMap<String, (ToolSchema, InterceptHandler)>` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `fn register_intercepted` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `async fn dispatch_intercepts` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub fn list_unavailable` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub fn list_toolsets` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub fn todo_write_schema` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `pub fn todo_read_schema` ✓
- `crates/ironhermes-tools/src/registry.rs` — contains `fn intercepted_tool_no_schema_duplicate` ✓
- `crates/ironhermes-tools/src/registry.rs` — D-15 messages: "already registered as a regular tool" (4 occurrences) ✓
- `crates/ironhermes-tools/src/registry.rs` — D-15 messages: "already registered as an intercepted tool" (4 occurrences) ✓
- Commit `0a496f6` exists ✓
- Commit `d5b5c2b` exists ✓
- Commit `c0df1e4` exists ✓
- `cargo build -p ironhermes-tools` exits 0 ✓
- `cargo test -p ironhermes-tools --lib` exits 0 (174 tests) ✓
