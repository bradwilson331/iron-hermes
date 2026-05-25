---
phase: 25-toolset-management
plan: "01"
subsystem: ironhermes-tools
tags: [tool-registry, prerequisites, toolset-names, trait-surface, phase-25]
dependency_graph:
  requires: []
  provides: [Prerequisite struct, prerequisites() trait method, walking default is_available(), D-01 toolset names]
  affects: [ironhermes-tools/registry.rs, ironhermes-tools/web_search.rs, ironhermes-tools/web_read.rs, ironhermes-tools/terminal.rs, ironhermes-tools/file_tools.rs, ironhermes-tools/cronjob_tool.rs]
tech_stack:
  added: []
  patterns: [Plain-String type D-25, OnceLock env_lock test isolation, include_str! source-text invariant]
key_files:
  created: []
  modified:
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/src/web_search.rs
    - crates/ironhermes-tools/src/web_read.rs
    - crates/ironhermes-tools/src/terminal.rs
    - crates/ironhermes-tools/src/file_tools.rs
    - crates/ironhermes-tools/src/cronjob_tool.rs
decisions:
  - "Open Question 1 resolved: cronjob -> agent toolset per D-01"
  - "Open Question 4 resolved: default is_available() walks prerequisites() filtering required env_var prereqs"
  - "Pitfall 1 closed: terminal/file_tools/cronjob toolset name mismatches fixed to D-01 names"
  - "D-09 / D-25: Prerequisite plain-String struct added to registry.rs (no Serialize/Deserialize)"
  - "web_search retains manual is_available() override per D-09; web_read drops hard-coded true in favor of default walk"
metrics:
  duration_minutes: 6
  completed_date: "2026-04-29"
  tasks_completed: 3
  files_modified: 6
---

# Phase 25 Plan 01: Toolset Trait Surface + Name Fixes Summary

**One-liner:** JWT-style additive trait surface: Prerequisite struct + prerequisites() method + walking default is_available() + D-01 toolset name corrections for terminal/file_tools/cronjob

## What Was Built

Pure additive trait expansion on `Tool` in `crates/ironhermes-tools/src/registry.rs`:

1. **`Prerequisite` struct** (plain-String per D-25): `{ kind: String, name: String, description: String, required: bool }`. Derives `Debug, Clone` only — no Serialize/Deserialize needed (never stored in YAML).

2. **`prerequisites()` default method** on `Tool` trait: returns empty `Vec<Prerequisite>`. Zero breaking changes — all existing impls continue to compile.

3. **Walking default `is_available()`**: replaces the old `{ true }` stub with a filter over `prerequisites()` that returns `false` when any `required: true` env_var prereq is absent. `config_field` and unknown kinds return `true` (checked at config load, not trait level per D-09).

4. **Toolset name fixes** (Pitfall 1 closed):
   - `terminal.rs`: `"system"` → `"code"`
   - `file_tools.rs` (4 impls: ReadFileTool, WriteFileTool, PatchFileTool, SearchFilesTool): `"file"` → `"code"`
   - `cronjob_tool.rs`: `"cronjob"` → `"agent"` (Open Question 1 resolved)

5. **`prerequisites()` impls** on web tools:
   - `web_search.rs`: adds `prerequisites()` with `FIRECRAWL_API_KEY` required:true; keeps existing `is_available()` override per D-09
   - `web_read.rs`: removes hard-coded `is_available() { true }`; adds `prerequisites()` with `FIRECRAWL_API_KEY` required:false (plain-text fallback path preserved)

## D-01 Toolset Name Distribution (Post-Plan-1)

| Toolset | Tools |
|---------|-------|
| `web` | web_search, web_read |
| `code` | execute_code, terminal, read_file, write_file, patch, search_files |
| `memory` | memory |
| `agent` | delegate_task, cronjob |
| `skills` | skills |
| `session` | session_search (intercepted-only, no toolset() impl needed) |

No `"system"`, `"file"`, or `"cronjob"` appear in any toolset() return value.

## Unit Tests Added

All tests colocated in `crates/ironhermes-tools/src/registry.rs` `#[cfg(test)]` block:

| Test Name | What It Covers |
|-----------|---------------|
| `prerequisite_default_impl_returns_empty` | Default prerequisites() returns empty Vec |
| `is_available_default_walks_prerequisites_required_env_var_present` | Required env_var present → is_available() == true |
| `is_available_default_walks_prerequisites_required_env_var_absent` | Required env_var absent → is_available() == false |
| `is_available_default_walks_prerequisites_optional_env_var_absent` | Optional (required:false) env_var absent → still true |
| `is_available_default_treats_unknown_kind_as_satisfied` | config_field required:true → is_available() == true (deferred to config load) |
| `toolset_names_match_d01_enumeration` | Direct instantiation of 5 tools + include_str! invariant for CronjobTool |
| `web_search_prerequisites_lists_firecrawl_required_true` | WebSearchTool prereq shape |
| `web_read_prerequisites_lists_firecrawl_required_false` | WebReadTool prereq shape |
| `web_search_is_available_remains_blocked_without_firecrawl` | WebSearchTool keeps manual override |
| `web_read_is_available_stays_true_without_firecrawl` | WebReadTool uses default walk (required:false = no block) |

All tests use `OnceLock<Mutex<()>>` env_lock pattern for env-var-mutating tests (Rust 2024 edition `unsafe` requirement, Phase 21.6 D). Unique env var name `TEST_PREREQ_25_01_PRESENT` avoids collision with `FIRECRAWL_API_KEY` tests.

## Operator-Facing Surface

None added in Plan 1. This plan ships only internal trait surface + literal-string fixes. Operator-facing surfaces (`hermes toolset` CLI subcommand, slash commands, setup wizard, preflight banner) are deferred to Plans 2-5.

## Commits

| Hash | Description |
|------|-------------|
| `7e95e46` | feat(25-01): add Prerequisite struct + prerequisites() trait method + walking default is_available() |
| `3cdd6e7` | feat(25-01): fix six toolset() mismatches to match D-01 enumeration |
| `d76c83c` | feat(25-01): add prerequisites() to web_search and web_read; 4 colocated tests |

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Fixed stale toolset() assertion in cronjob_tool.rs test**
- **Found during:** Task 2 — running `cargo test -p ironhermes-tools --lib` after toolset name fixes
- **Issue:** `cronjob_tool::tests::test_name` asserted `tool.toolset() == "cronjob"` — the old value that Plan 1 was changing to `"agent"`
- **Fix:** Updated assertion to `"agent"` with comment linking to D-01 / Phase 25 Plan 1
- **Files modified:** `crates/ironhermes-tools/src/cronjob_tool.rs:751`
- **Commit:** `d76c83c`

**2. [Rule 1 - Bug] Pre-existing clippy errors in ironhermes-core (out of scope)**
- **Found during:** Final verification
- **Issue:** `cargo clippy -p ironhermes-tools -- -D warnings` fails because `ironhermes-core` (a dependency) has 8 pre-existing clippy errors in unrelated code
- **Action:** Logged to deferred-items. Zero new clippy warnings introduced in `ironhermes-tools` itself — the crate targeted by this plan passes with no warnings.
- **Files modified:** None (out of scope per deviation rule scope boundary)

## Known Stubs

None. Plan 1 is pure additive trait surface + name corrections. All implementation is complete.

## Threat Flags

None. Plan 1 adds no operator-facing surface, no I/O, no config writes, no new untrusted-input parsing. The trait changes only read env vars (which the process already reads).

## Self-Check: PASSED

- `crates/ironhermes-tools/src/registry.rs` — modified, contains `pub struct Prerequisite` ✓
- `crates/ironhermes-tools/src/web_search.rs` — modified, contains `fn prerequisites` ✓
- `crates/ironhermes-tools/src/web_read.rs` — modified, contains `fn prerequisites`, no `fn is_available` ✓
- `crates/ironhermes-tools/src/terminal.rs` — modified, toolset() returns `"code"` ✓
- `crates/ironhermes-tools/src/file_tools.rs` — modified, all 4 toolset() return `"code"` ✓
- `crates/ironhermes-tools/src/cronjob_tool.rs` — modified, toolset() returns `"agent"` ✓
- Commit `7e95e46` exists ✓
- Commit `3cdd6e7` exists ✓
- Commit `d76c83c` exists ✓
- `cargo build --workspace` exits 0 ✓
- All 161 unit tests in `ironhermes-tools` pass ✓
