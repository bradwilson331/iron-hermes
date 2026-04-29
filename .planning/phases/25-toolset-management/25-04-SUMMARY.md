---
phase: 25-toolset-management
plan: "04"
subsystem: ironhermes-cli, ironhermes-core
tags: [toolset-cli, slash-command, d-04, d-06, d-26-test1, t-25-01, t-25-03, slug-validation, cache-break-banner, phase-25]
dependency_graph:
  requires: [25-01, 25-02, 25-03]
  provides:
    - ToolsetSubcommand (List/Enable/Disable/Show) on `hermes toolset`
    - /toolset slash command (Universal platform, session-only D-06)
    - ToolsetSessionHandle trait in ironhermes-core
    - render_toolset_list / render_toolset_show shared helpers
    - D-26 Test 1 passing (toolset_enable_disable_persists)
    - T-25-01 mitigation (slug validator reused from Phase 24 D-03)
    - T-25-03 mitigation (cache-break banner on stderr only)
  affects:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/lib.rs
    - crates/ironhermes-cli/src/toolset_cmd.rs
    - crates/ironhermes-cli/tests/toolset_integration.rs
    - crates/ironhermes-core/src/commands/registry.rs
    - crates/ironhermes-core/src/commands/handlers.rs
    - crates/ironhermes-core/src/commands/context.rs
    - crates/ironhermes-core/src/commands/mod.rs
    - crates/ironhermes-core/src/commands/toolset_display.rs
tech_stack:
  added: []
  patterns:
    - Sub-dispatch enum mirroring `cron_cmd` / `config_cli` (Subcommand + handle_* dispatcher)
    - Trait-object handle pattern (ToolsetSessionHandle) to avoid circular dep
    - Pure rendering helpers in a leaf crate (toolset_display.rs in ironhermes-core)
    - Cache-break stderr banner shape (T-25-03)
    - env_lock + CARGO_BIN_EXE_ironhermes subprocess test (carry-forward from Phase 24)
key_files:
  created:
    - crates/ironhermes-cli/src/toolset_cmd.rs
    - crates/ironhermes-cli/tests/toolset_integration.rs
    - crates/ironhermes-core/src/commands/toolset_display.rs
    - .planning/phases/25-toolset-management/deferred-items.md
  modified:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/lib.rs
    - crates/ironhermes-core/src/commands/registry.rs
    - crates/ironhermes-core/src/commands/handlers.rs
    - crates/ironhermes-core/src/commands/context.rs
    - crates/ironhermes-core/src/commands/mod.rs
decisions:
  - "D-04 enacted: hermes toolset list/enable/disable/show subcommands shipped (Setup deferred to Plan 5)"
  - "D-06 contract pinned: slash /toolset enable/disable mutates SESSION ONLY via ToolsetSessionHandle trait; CLI subcommand persists to config.yaml"
  - "T-25-01 mitigation: validate_toolset_name reuses ironhermes_core::profile::validate_profile_name (Phase 24 D-03 slug regex)"
  - "T-25-03 mitigation: cache-break banner emitted on stderr (NOT stdout) for every state-changing command (CLI + slash)"
  - "Architectural fix: shared rendering helpers live in ironhermes-core (toolset_display.rs) — both CLI and slash handler call them; no reverse-dep introduced"
  - "ToolsetSessionHandle trait added to commands/context.rs — same pattern as CronJobReader (avoids circular dep with ironhermes-tools)"
  - "Slash registry: /toolsets stub REPLACED (not aliased) — Phase 22.3 typo system handles operator-typed plural"
metrics:
  duration_minutes: 25
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_modified: 10
---

# Phase 25 Plan 04: Operator Surface — `hermes toolset` + `/toolset` Slash Summary

**One-liner:** Operator-facing `hermes toolset list/enable/disable/show` CLI surface + `/toolset` slash command with session-only mutation contract; D-26 Test 1 passing via subprocess persistence round-trip.

## What Was Built

### Task 1: `toolset_cmd.rs` + `Commands::Toolset` Variant + Dispatch (ironhermes-cli)

**`crates/ironhermes-cli/src/toolset_cmd.rs`** (new file):
- `pub enum ToolsetSubcommand { List, Enable, Disable, Show }` — clap-derived subcommand surface
- `pub async fn handle_toolset_command` — dispatcher mirroring `config_cli::handle_config_command`
- `pub fn validate_toolset_name(name: &str) -> Result<String>` (T-25-01 gate) — reuses `profile::validate_profile_name`
- `pub async fn cmd_toolset_enable / cmd_toolset_disable` — slug-validate → check known set → `config_setter::config_set` → emit cache-break banner on stderr
- `cmd_toolset_list` — loads ToolsConfig via Config::load, builds default registry, builds rows via `build_toolset_rows`, calls `render_toolset_list`
- `cmd_toolset_show` — slug-validate → enumerate members → render via `render_toolset_show`
- `KNOWN_TOOLSETS` const lists the six D-01 toolsets (web, code, memory, agent, skills, session)
- `toolset_members_map()` static membership map per D-01

**`crates/ironhermes-cli/src/main.rs`:**
- `mod toolset_cmd;` declared near `mod config_cli;`
- `Commands::Toolset { subcommand: toolset_cmd::ToolsetSubcommand }` variant added after `Commands::Config`
- `Some(Commands::Toolset { subcommand })` dispatch arm added after `Some(Commands::Config { ... })`

**`crates/ironhermes-cli/src/lib.rs`:**
- `pub mod toolset_cmd;` re-export so integration tests can call `validate_toolset_name` etc.

**5 unit tests (all passing):**
- `validate_toolset_name_rejects_path_traversal` — T-25-01 gate
- `validate_toolset_name_rejects_empty`
- `validate_toolset_name_accepts_known_d01_names` — all 6 D-01 names
- `validate_toolset_name_rejects_uppercase` — slug regex is lowercase-only
- `cmd_toolset_enable_rejects_unknown_name` — unknown toolset rejected before any write

### Task 2: `/toolset` Slash Command + Session-Only Mutation Handler (ironhermes-core)

**`crates/ironhermes-core/src/commands/registry.rs`:**
- REPLACED the existing `/toolsets` stub line (single-line edit) with `/toolset` (singular per D-06):
  ```rust
  CommandDef::new("toolset", "Manage toolsets (list/enable/disable/show)", ToolsAndSkills)
      .args_hint("[list|enable|disable|show] [name]")
      .platform(Universal),
  ```
- Added `#[cfg(test)]` test module with 3 tests for slash registration

**`crates/ironhermes-core/src/commands/context.rs`:**
- Added `ToolsetSessionHandle` trait (parallel to `CronJobReader`/`McpReloader` — avoids circular dep with ironhermes-tools):
  - `enable_toolset(&self, name: &str) -> Result<(), String>`
  - `disable_toolset(&self, name: &str) -> Result<(), String>`
  - `render_list(&self) -> String`
  - `render_show(&self, name: &str) -> Result<String, String>`
- Added `pub toolset_session: Option<Arc<dyn ToolsetSessionHandle>>` field to `CommandContext`
- Added `with_toolset_session(...)` builder method

**`crates/ironhermes-core/src/commands/handlers.rs`:**
- Added `"toolset" => cmd_toolset(args, ctx)` arm to dispatch
- Added `cmd_toolset` sub-dispatcher mirroring `cmd_cron`:
  - `list` / no-args → `handle.render_list()`
  - `show <name>` → `handle.render_show(name)`
  - `enable <name>` → `handle.enable_toolset(name)` + emit cache-break banner on stderr (T-25-03)
  - `disable <name>` → `handle.disable_toolset(name)` + emit cache-break banner on stderr
  - typo suggestion via existing `suggest_typo` helper
- **D-06 contract: handler does NOT call `config_setter::config_set` anywhere** — verified by grep (acceptance criteria)
- Removed `"toolsets"` from `todo_stub` and from the test list

**`crates/ironhermes-core/src/commands/toolset_display.rs`** (new file):
- `pub struct ToolsetRow { name, enabled, member_count, available_count, member_summary }` — display data shape
- `pub fn render_toolset_list(rows: Vec<ToolsetRow>) -> String` — aligned-column table per CONTEXT.md §Specifics
- `pub fn render_toolset_show(row: &ToolsetRow, members: &[(String, bool, String)]) -> String` — detail view
- Pure functions — no I/O. Lives in `ironhermes-core` because `ironhermes-cli` depends on `ironhermes-core` (NEVER the reverse). Both CLI and slash surfaces call these.
- 4 unit tests covering header, enabled row, disabled row, show format

**`crates/ironhermes-core/src/commands/mod.rs`:**
- `pub mod toolset_display;` added

**6 new ironhermes-core tests (all passing):**
- `slash_toolset_registered_in_build_registry`
- `slash_toolsets_plural_not_registered` (verifies plural was REPLACED, not aliased)
- `slash_toolset_platform_is_universal`
- `slash_toolset_handler_session_only_no_config_write` (D-06 contract — fake handle records calls; tempdir config.yaml MUST stay absent)
- `slash_toolset_no_handle_returns_informational`
- `slash_toolset_list_renders_via_handle`

### Task 3: D-26 Test 1 + T-25-01 + T-25-03 Integration Tests (ironhermes-cli)

**`crates/ironhermes-cli/tests/toolset_integration.rs`** (new file):

Three subprocess + tempdir tests using the `CARGO_BIN_EXE_ironhermes` pattern (carry-forward from Phase 24's `profile_isolation.rs`):

| Test | Mitigation | Asserts |
|------|-----------|---------|
| `toolset_enable_disable_persists` | D-26 Test 1 | enable web → stderr banner → config.yaml has `enabled: true` → list shows "web enabled" → disable web → stderr disable banner → list shows "web disabled" (across 4 separate binary invocations) |
| `toolset_enable_rejects_path_traversal_name` | T-25-01 | `toolset enable ../etc/passwd` exits non-zero, stderr contains "invalid", config.yaml does NOT contain path traversal |
| `toolset_enable_emits_cache_break_banner_on_stderr` | T-25-03 | Banner present on stderr ("schema cache will rebuild on next LLM call"), NOT on stdout (pipes-clean) |

All 3 integration tests pass under `cargo test -p ironhermes-cli --test toolset_integration -- --test-threads=1`.

## D-26 Test Status

| Test | Location | Status |
|------|----------|--------|
| Test 1: `toolset_enable_disable_persists` | `crates/ironhermes-cli/tests/toolset_integration.rs` | PASSING (Plan 04) |
| Test 2: `tool_excluded_when_prereq_missing` | `crates/ironhermes-tools/tests/toolset_prereq.rs` | PASSING (Plan 03) |
| Test 3 (stub): `intercepted_tool_no_schema_duplicate` | `crates/ironhermes-tools/src/registry.rs` | PASSING (Plan 03) |
| Test 3 (live): `intercepted_no_duplicate_with_real_handlers` | `crates/ironhermes-agent/src/agent_loop.rs` | PASSING (Plan 03) |

## Critical Constraint Verification

| Constraint | Status | Evidence |
|-----------|--------|----------|
| **C1**: `toolset_display.rs` in ironhermes-core (no reverse-dep) | PASS | `[[ -f crates/ironhermes-core/src/commands/toolset_display.rs ]] && grep -q "render_toolset_list" $_` returns 0 |
| **C2**: Slash D-06 — session-only, no `config_setter::config_set` in slash handler | PASS | `grep -A100 '"toolset" => cmd_toolset' crates/ironhermes-core/src/commands/handlers.rs \| grep -c "config_setter::config_set"` returns 0 |
| **C3**: T-25-01 — `validate_toolset_name("../etc/passwd")` returns Err | PASS | `validate_toolset_name_rejects_path_traversal` unit test + `toolset_enable_rejects_path_traversal_name` integration test |
| **C4**: T-25-03 — cache-break banner on stderr | PASS | `toolset_enable_emits_cache_break_banner_on_stderr` integration test (asserts present on stderr, absent on stdout) |
| Wave 3 invariant: no direct `register(Box::new((MemoryTool\|DelegateTaskTool\|CronjobTool)::new` in main.rs | PASS | `grep -E ... main.rs` returns no matches |
| Workspace builds | PASS | `cargo build -p ironhermes-cli -p ironhermes-core` exits 0 |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 — Bug] Pre-existing test failure in `dispatch_all_todo_stubs_return_not_yet_available`**
- **Found during:** Task 2 verification (running broader ironhermes-core tests)
- **Issue:** The test expects "cron" to return "is not yet available" stub, but `cmd_cron` has a real handler since Phase 22.4.2.1-01 that returns "/cron: cron store not configured."
- **Disposition:** OUT OF SCOPE for Plan 04 — verified pre-existing on base commit `62be4f0` via `git stash` round-trip. Logged to `.planning/phases/25-toolset-management/deferred-items.md` for a future maintenance pass.
- **Plan-04-introduced changes:** None. The `"toolsets"` line was correctly removed from the same test list as part of D-06 (because `/toolset` now has a real handler) — that change is correct and necessary. The `cron` failure pre-dates this plan.

### Pre-existing Out-of-Scope Issues

- `memory-sqlite`, `memory-duckdb`, `memory-grafeo` provider crates fail with `missing field 'cache_breaking'` — confirmed pre-existing on base commit (documented in 25-02 and 25-03 SUMMARY files). Not introduced by this plan.

## Auth Gates

None — plan was fully autonomous (no human-action checkpoints).

## Plan 5 Deferrals

Per the plan's `<objective>`, the following are explicitly deferred to Plan 5:
- `Setup` variant on `ToolsetSubcommand` (the Plan 5 setup-wizard hook)
- `hermes toolset setup` per-prerequisite walkthrough
- Preflight prereq probe (`run_preflight_check` extension per D-17)
- Optional-tools final stage in `hermes setup` (D-19)
- `delegate_task` intercept stub → live wiring (Plan 3 deferral)

## Known Stubs

- `cmd_toolset_list` and `cmd_toolset_show` in `toolset_cmd.rs` use the static `toolset_members_map()` rather than reading `Tool::toolset()` from a fully-populated registry. This is intentional for v2.1 because D-01 fixes the six toolset names in the binary; MCP-driven dynamic toolsets are deferred (per CONTEXT.md `<deferred>`). The display still calls `registry.list_unavailable()` for per-tool prerequisite info, so availability marks are accurate.
- `ToolsetSessionHandle` trait is defined in ironhermes-core but no concrete impl is wired into the live REPL yet — this is intentional. Plan 4's contract is the trait + slash dispatch shape; the actual wire-up of a `ToolRegistry`-backed implementation onto `CommandContext` for the REPL's live session belongs to a future plan that needs the slash UI active in the binary REPL. Until then, `/toolset list` returns the informational fallback ("/toolset: toolset session handle not configured.") in the live binary.

## Threat Flags

None. Plan 04's surface is bounded by the four mitigations in the plan's `<threat_model>`:
- T-25-01 closed (slug validator gate)
- T-25-03 closed (cache-break banner)
- T-25-02 (secret leakage) and T-25-04 (intercept arbitrary code) are explicitly out of scope per the plan's threat model — Plan 5 owns T-25-02 (setup wizard prompts), Plan 3 owned T-25-04 (with_intercepts type-restricted signature).

## Commits

| Hash | Description |
|------|-------------|
| `7c2011a` | feat(25-04): add ToolsetSubcommand + handler + slug validation + cache-break banner |
| `c394c44` | feat(25-04): add /toolset slash command + ToolsetSessionHandle (session-only D-06) |
| `c7d20ee` | test(25-04): add D-26 Test 1 + T-25-01 + T-25-03 integration tests |

## Self-Check: PASSED

- `crates/ironhermes-cli/src/toolset_cmd.rs` — exists, contains `pub enum ToolsetSubcommand` ✓
- `crates/ironhermes-cli/src/toolset_cmd.rs` — contains `validate_toolset_name` ✓
- `crates/ironhermes-cli/src/toolset_cmd.rs` — contains `validate_profile_name` reuse (T-25-01) ✓
- `crates/ironhermes-cli/src/toolset_cmd.rs` — contains `config_setter::config_set` ✓
- `crates/ironhermes-cli/src/toolset_cmd.rs` — contains `schema cache will rebuild` (T-25-03 banner) ✓
- `crates/ironhermes-cli/src/main.rs` — contains `Commands::Toolset` ✓
- `crates/ironhermes-cli/src/main.rs` — contains `mod toolset_cmd` ✓
- `crates/ironhermes-cli/src/main.rs` — does NOT directly register MemoryTool/DelegateTaskTool/CronjobTool (Wave 3 invariant) ✓
- `crates/ironhermes-cli/src/lib.rs` — contains `pub mod toolset_cmd` ✓
- `crates/ironhermes-cli/tests/toolset_integration.rs` — contains `fn toolset_enable_disable_persists` ✓
- `crates/ironhermes-cli/tests/toolset_integration.rs` — contains `fn toolset_enable_rejects_path_traversal_name` ✓
- `crates/ironhermes-cli/tests/toolset_integration.rs` — contains `fn toolset_enable_emits_cache_break_banner_on_stderr` ✓
- `crates/ironhermes-cli/tests/toolset_integration.rs` — contains `env_lock` ✓
- `crates/ironhermes-cli/tests/toolset_integration.rs` — contains `CARGO_BIN_EXE_ironhermes` ✓
- `crates/ironhermes-core/src/commands/registry.rs` — contains `CommandDef::new("toolset"` (1 occurrence) ✓
- `crates/ironhermes-core/src/commands/registry.rs` — does NOT contain `CommandDef::new("toolsets"` (0 occurrences) ✓
- `crates/ironhermes-core/src/commands/handlers.rs` — contains `"toolset" => cmd_toolset` ✓
- `crates/ironhermes-core/src/commands/handlers.rs` — slash handler does NOT call `config_setter::config_set` (D-06) ✓
- `crates/ironhermes-core/src/commands/context.rs` — contains `pub trait ToolsetSessionHandle` ✓
- `crates/ironhermes-core/src/commands/context.rs` — contains `pub toolset_session:` ✓
- `crates/ironhermes-core/src/commands/toolset_display.rs` — exists, contains `render_toolset_list` ✓
- `crates/ironhermes-core/src/commands/toolset_display.rs` — contains `render_toolset_show` ✓
- `crates/ironhermes-core/src/commands/mod.rs` — contains `pub mod toolset_display` ✓
- `cargo build -p ironhermes-cli -p ironhermes-core` exits 0 ✓
- `cargo test -p ironhermes-cli --test toolset_integration -- --test-threads=1` exits 0 (3 tests pass) ✓
- `cargo test -p ironhermes-core --lib slash_toolset` exits 0 (6 tests pass) ✓
- `cargo test -p ironhermes-core --lib render_toolset` exits 0 (4 tests pass) ✓
- `cargo test -p ironhermes-cli --lib validate_toolset_name` exits 0 (4 tests pass) ✓
- `cargo test -p ironhermes-cli --lib cmd_toolset_enable_rejects_unknown` exits 0 (1 test passes) ✓
- Commit `7c2011a` exists ✓
- Commit `c394c44` exists ✓
- Commit `c7d20ee` exists ✓
