---
phase: 25-toolset-management
plan: "05"
subsystem: toolset-setup-wizard
tags: [toolset, setup-wizard, preflight, env-var, secret-masking, d17, d18, d19, t25-02, tool-05]
dependency_graph:
  requires: [25-03, 25-04]
  provides: [TOOL-05]
  affects: [preflight, setup-wizard, toolset-cmd, hermes-setup]
tech_stack:
  added: []
  patterns:
    - atomic-tempfile-rename-chmod-0600
    - rustyline-masked-input
    - writer-injection-seam
    - testability-seam-parallel-to-apply-minimum-viable-answers
key_files:
  created: []
  modified:
    - crates/ironhermes-cli/src/toolset_cmd.rs
    - crates/ironhermes-cli/src/setup.rs
    - crates/ironhermes-cli/src/preflight.rs
    - crates/ironhermes-cli/tests/toolset_integration.rs
    - crates/ironhermes-cli/Cargo.toml
decisions:
  - D-17 preflight: stderr banner only; NO auto-wizard launch; Phase 23 gate location byte-identical
  - D-18 toolset setup: rustyline prereq-walk with masked input for secrets; skip=never-reprompt; defer=move-on
  - D-19 hermes setup: opt-in after model+key wizard; default=No; Phase 23 floor unchanged
  - T-25-02: is_secret_prereq_name + write_env_var_to_dotenv 0600 + no echo after acceptance
metrics:
  duration: "~30 minutes"
  completed: "2026-04-30T00:06:12Z"
  tasks_completed: 3
  files_changed: 5
---

# Phase 25 Plan 05: Prerequisite Discovery + Setup Wizard Surface Summary

Prereq-discovery wizard closes TOOL-05: `hermes toolset setup` walks missing required prereqs, preflight emits a stderr banner (no blocking), and `hermes setup` gains an opt-in tool-prereq stage after the model/key wizard. T-25-02 secret-masking locked at both unit and integration layers.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Setup variant + run_tools_section real impl + T-25-02 + testability seam | b647d16 | toolset_cmd.rs, setup.rs, Cargo.toml |
| 2 | Preflight D-17 probe — stderr banner, no auto-wizard | a424c29 | preflight.rs |
| 3 | D-19 opt-in stage + subprocess integration tests | 8b092b8 | setup.rs, toolset_integration.rs |

## Implementation Details

### D-17: Preflight Tool-Prereq Probe (preflight.rs)

`run_preflight_check` now probes `build_full_registry().list_unavailable()` AFTER `config.validate()` passes, BEFORE returning `Ok(())`. A `tools.skip_prompts` filter is applied before emitting. Banner emitted via writer-injection seam `emit_prereq_banner(active, &mut dyn Write)` so unit tests capture output without subprocess.

Phase 23 gate at `main.rs:273-276` is byte-identical — the `run_preflight = matches!(...)` condition and `preflight::run_preflight_check(&cli)` call site are UNCHANGED.

No auto-wizard launch near `list_unavailable` (D-17 contract verified via grep).

### D-18: `hermes toolset setup` (toolset_cmd.rs + setup.rs)

`ToolsetSubcommand::Setup` variant added with dispatch arm calling `cmd_toolset_setup()`. The function constructs a rustyline editor via `make_wizard_editor()` (now pub) and delegates to `run_tools_section(rl, hermes_home)`.

`run_tools_section` replaces the Phase 23 stub with real prereq-walking logic:
- `build_full_registry()` — shared with preflight; calls `registry.register_defaults()`
- `list_unavailable()` — returns tools with missing required prereqs
- Per prereq: prompt with masking for secret-pattern names; "skip" writes to `tools.skip_prompts`; empty Enter defers
- `apply_prereq_value` dispatches by `prereq.kind`: `"env_var"` → `write_env_var_to_dotenv`; `"config_field"` → `config_setter::config_set`
- `write_env_var_to_dotenv` — atomic upsert: read+parse existing `.env`, upsert line, write via `tempfile::NamedTempFile`, chmod 0600 before rename (T-25-02)
- `apply_skip_prompts` — idempotent append to `config.tools.skip_prompts`

### D-19: `hermes setup` Opt-In Stage (setup.rs)

`run_minimum_viable_flow` gains a `prompt_yes_no` gate AFTER the "Setup complete" message (after `apply_minimum_viable_answers` and all Phase 23 wizard steps). Default is "No" — operator who declines gets the Phase 23 minimum-viable config unchanged. Operator who accepts walks through `run_tools_section`.

### T-25-02 Mitigation

- `is_secret_prereq_name(name)` — detects `*_KEY`, `*_TOKEN`, `*_SECRET`, `*_PASSWORD` via `.ends_with()` on uppercased name
- `write_env_var_to_dotenv` — `tempfile::NamedTempFile::new_in`, `set_permissions(mode 0o600)`, write content, `persist()` (atomic rename). Value never echoed; prints "Saved." only.
- `apply_tool_prereq_answers` — testability seam; integration test captures stdout+stderr and asserts sentinel value `test_secret_value` is NOT in output (T-25-02 subprocess assertion)
- Mode 0600 verified by `toolset_setup_writes_dotenv_with_0600_mode` integration test (`std::os::unix::fs::PermissionsExt`)

## Test Results

### Unit Tests (lib)

| Test | Result |
|------|--------|
| `is_secret_prereq_name_matches_key_token_secret_password` | PASS |
| `apply_tool_prereq_answers_writes_to_dotenv_with_0600_mode` | PASS (#[cfg(unix)]) |
| `apply_tool_prereq_answers_upserts_existing_env` | PASS |
| `apply_skip_prompts_appends_to_list` | PASS |
| `run_tools_section_returns_ok_when_no_unavailable` | PASS |
| `run_setup_appends_optional_tool_prereq_stage_d19` | PASS |
| `preflight_emits_banner_when_required_prereq_missing` | PASS (bin test) |
| `preflight_suppresses_banner_for_skip_prompts_tools` | PASS (bin test) |
| `preflight_no_banner_when_active_is_empty` | PASS (bin test) |

### Integration Tests (toolset_integration.rs)

| Test | Source | Result |
|------|--------|--------|
| `toolset_enable_disable_persists` | Plan 4 (D-26 Test 1) | PASS |
| `toolset_enable_rejects_path_traversal_name` | Plan 4 (T-25-01) | PASS |
| `toolset_enable_emits_cache_break_banner_on_stderr` | Plan 4 (T-25-03) | PASS |
| `toolset_setup_writes_dotenv_with_0600_mode` | Plan 5 (T-25-02) | PASS |
| `preflight_banner_appears_for_required_missing_prereq` | Plan 5 (D-17) | PASS |

Total: 5/5 integration tests pass.

## D-26 Mandatory Tests — Final Status

- D-26 Test 1 (`toolset_enable_disable_persists`): GREEN from Plan 4, verified passing Plan 5
- D-26 Test 2 (`tool_excluded_when_prereq_missing`): GREEN from Plan 3
- D-26 Test 3 (`intercepted_tool_no_schema_duplicate`): GREEN from Plan 2/3

All three D-26 mandatory tests green workspace-wide.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] `tempfile` was dev-dependency only**
- **Found during:** Task 1 build
- **Issue:** `write_env_var_to_dotenv()` in library code used `tempfile::NamedTempFile` but `tempfile` was only in `[dev-dependencies]`, causing compile error in lib target.
- **Fix:** Added `tempfile = "3"` to `[dependencies]` in `crates/ironhermes-cli/Cargo.toml`. The dev-dependency entry was retained for test-only consumers.
- **Files modified:** `crates/ironhermes-cli/Cargo.toml`

**2. [Rule 1 - Pattern] Preflight tests run via `--bin ironhermes`, not `--lib`**
- **Found during:** Task 2 test run
- **Issue:** `preflight.rs` references `crate::Cli` which lives in `main.rs`, so it cannot be added to `lib.rs`. The `--lib` test target filtered out all preflight tests (0 tests run).
- **Fix:** Tests confirmed passing via `cargo test -p ironhermes-cli --bin ironhermes -- preflight`. Plan acceptance criteria verified via this path instead of `--lib`.

## Known Stubs

None — all stubs from Phase 23 (`run_tools_section`) replaced with real implementations.

## Threat Flags

None. No new network endpoints, auth paths, file access patterns, or schema changes beyond what the plan's threat model covers (T-25-02 mitigated at unit + integration layers).

## Self-Check: PASSED

| Check | Result |
|-------|--------|
| `25-05-SUMMARY.md` exists | FOUND |
| Commit b647d16 (Task 1) | FOUND |
| Commit a424c29 (Task 2) | FOUND |
| Commit 8b092b8 (Task 3) | FOUND |
| `cargo build -p ironhermes-cli` exits 0 | PASS |
| All 5 integration tests pass | PASS (5/5) |
| All 172 lib unit tests pass | PASS |
