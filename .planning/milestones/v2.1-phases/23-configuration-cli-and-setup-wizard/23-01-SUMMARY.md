---
phase: 23
plan: 01
subsystem: ironhermes-core
tags: [config, wizard, validation, schema, learning-loop]
dependency_graph:
  requires: []
  provides:
    - crates/ironhermes-core/src/config_schema.rs::schema
    - crates/ironhermes-core/src/wizard.rs::apply_*_answer
    - crates/ironhermes-core/src/config_validate.rs::Config::validate
    - crates/ironhermes-core/src/config_setter.rs::config_set/config_get/is_cache_breaking
  affects:
    - crates/ironhermes-core/src/memory_provider.rs (ConfigField struct literal fix)
tech_stack:
  added: []
  patterns:
    - "serde_yaml::Value load-mutate-save for unknown-key survival (D-15)"
    - "Pure-function wizard helpers with no I/O dependency"
    - "Infallible Vec<ConfigValidationError> returns from Config::validate()"
key_files:
  created:
    - crates/ironhermes-core/src/config_schema.rs (extended)
    - crates/ironhermes-core/src/wizard.rs
    - crates/ironhermes-core/src/config_validate.rs
    - crates/ironhermes-core/src/config_setter.rs
    - crates/ironhermes-core/tests/wizard_flow.rs
    - crates/ironhermes-core/tests/config_validate.rs
    - crates/ironhermes-core/tests/config_setter.rs
  modified:
    - crates/ironhermes-core/src/lib.rs
    - crates/ironhermes-core/src/memory_provider.rs
decisions:
  - "pub fn schema() -> Vec<ConfigField> chosen over static Lazy<> to avoid const heap-allocation headaches with String keys"
  - "apply_learning_loop_answer returns serde_yaml::Mapping (not a side-effect on config) so Plan 23-02's setup.rs can splice it via Value load-mutate-save"
  - "config_setter.rs has zero imports of Config — operates entirely on serde_yaml::Value (Anti-Pattern #1 avoidance)"
  - "is_cache_breaking takes &[ConfigField] slice so callers can pass schema() or a custom registry in tests"
metrics:
  duration_minutes: 7
  completed_date: "2026-04-28"
  tasks_completed: 6
  files_changed: 9
---

# Phase 23 Plan 01: Config Schema + Pure-Function Core Summary

Pure-function core for the configuration/setup-wizard subsystem: schema extension, wizard helpers, validator, and dotted-path setter — all in `ironhermes-core`, zero I/O, zero CLI changes. Plan 23-02 wires this surface into rustyline + clap subcommands.

## What Was Built

### Task 1 — ConfigField extension + SCHEMA registry (73e119e)
- Added `cache_breaking: bool` with `#[serde(default)]` to `ConfigField` (back-compat preserved)
- Added `pub fn schema() -> Vec<ConfigField>` with 18 entries covering all D-09 secret fields and D-13/D-18 cache-breaking fields
- 4 new tests: round-trip (updated), schema_contains_all_cache_breaking_fields, schema_contains_all_secret_fields, model_api_key_is_both_secret_and_cache_breaking

### Tasks 2-5 — New modules + Wave 0 tests + full implementations (be721e5)
All three modules created with full implementations (not just stubs) plus complete test suites:

**wizard.rs:**
- `WizardMode` enum (Explicit/FirstRun/FixMode) — cross-crate plain-data type per D-12
- `LEARNING_LOOP_FRAMING` const — verbatim D-16 framing, locked by regression test
- `apply_model_answer`, `apply_provider_answer`, `apply_api_key_answer` — minimum-viable helpers
- `apply_learning_loop_answer` — returns `serde_yaml::Mapping` with full 5-key learning.* block; YES/NO/empty-as-YES per D-14
- `apply_memory_provider_answer` — validates against known backends, returns `anyhow::Result<()>`
- `apply_hermes_home_answer` — returns trimmed string, leaves path normalization to caller
- `apply_gateway_section_answer`, `apply_tools_section_answer` — no-op stubs for Phase 25/26

**config_validate.rs:**
- `ConfigValidationError { path, reason, suggested_fix }` struct
- `Config::validate()` inherent method — checks model.api_key, model.default, model.provider, memory.provider-when-enabled
- Errors carry `suggested_fix: Some("hermes setup <section>")` for wizard-repairable fields

**config_setter.rs:**
- `config_set(hermes_home, dotted_path, value) -> Result<Option<String>>` — Value-based load-mutate-save
- `config_get(hermes_home, dotted_path) -> Result<Option<String>>` — walks nested mappings
- `is_cache_breaking(dotted_path, schema) -> bool` — looks up SCHEMA registry
- Smart scalar coercion: bool → i64 → string
- Does NOT import `Config` — pure `serde_yaml::Value` operations (Anti-Pattern #1 guard)

### Task 6 — Clippy fix (34b35de)
- Fixed `match-with-early-return` → `?` operator in `get_at` function per clippy suggestion

## Test Counts

| Suite | Tests | Status |
|-------|-------|--------|
| config_schema (lib) | 4 | all pass |
| wizard_flow (integration) | 16 | all pass |
| config_validate (integration) | 9 | all pass |
| config_setter (integration) | 8 | all pass |
| **Total new** | **37** | **all pass** |

Full `cargo test -p ironhermes-core`: 309 passed, 1 pre-existing failure (see Deferred Issues).

## D-XX Decisions Covered

| Decision | Coverage |
|----------|----------|
| D-06 | `Config::validate()` returns `Vec<ConfigValidationError>` per spec |
| D-08 | Dotted-path syntax in `config_setter::config_set/config_get` |
| D-09 | All secret fields (model.api_key, telegram.token/api_key, subagent.api_key, batch.api_key) in SCHEMA registry |
| D-12 | `WizardMode` as cross-crate plain-data type in `ironhermes-core` |
| D-13 | `cache_breaking: bool` field on ConfigField; all D-13 fields tagged in schema() |
| D-14 | Learning Loop opt-out via apply_learning_loop_answer; empty = YES; "n" writes explicit false |
| D-15 | config_setter uses serde_yaml::Value (not Config::save) so unknown keys survive; D-15 anchor test passes |
| D-16 | LEARNING_LOOP_FRAMING const present; regression test locks 3 verbatim phrases |
| D-18 | memory.memory_enabled + learning.skill_generation_enabled tagged cache_breaking: true in schema |

## Open Hooks for Plan 23-02

1. **Rustyline I/O** — `setup.rs` in ironhermes-cli calls all `apply_*_answer` functions after reading lines from rustyline editor
2. **Learning block splice** — `apply_learning_loop_answer` returns `serde_yaml::Mapping`; setup.rs must splice it into config.yaml via `config_setter::config_set` per key
3. **Clap subcommands** — `Commands::Setup { section }` and `Commands::Config { subcommand }` in main.rs (plan 23-02 Task 1)
4. **Preflight middleware** — `Config::validate()` called in bare `hermes` / `hermes chat` arms to trigger fix-mode wizard (plan 23-02 Task 4)
5. **`hermes config show`** — `config_schema::schema()` provides the secret field list for masking; `is_cache_breaking` for warn-on-set (plan 23-02 Task 3)
6. **apply_gateway_section_answer / apply_tools_section_answer** — stubs awaiting Phase 25/26 implementation

## Plan 02 Territory Untouched Confirmation

- `crates/ironhermes-cli/src/main.rs` — NOT modified
- `crates/ironhermes-cli/src/setup.rs` — does not exist (Plan 23-02 creates it)
- `crates/ironhermes-cli/src/config_cli.rs` — does not exist (Plan 23-02 creates it)
- No rustyline editors spawned
- No clap subcommand changes

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] memory_provider.rs ConfigField literals missing cache_breaking field**
- **Found during:** Task 1
- **Issue:** Adding `cache_breaking: bool` to `ConfigField` broke 3 struct literal initializations in `memory_provider.rs` (compiler error E0063)
- **Fix:** Added `cache_breaking: false` to all 3 `ConfigField` struct literals in `get_config_schema()`
- **Files modified:** `crates/ironhermes-core/src/memory_provider.rs`
- **Commit:** 73e119e

**2. [Rule 1 - Bug] Clippy match-to-? in config_setter.rs**
- **Found during:** End-of-plan clippy check
- **Issue:** `get_at` function used explicit `match`/`return None` pattern where `?` operator suffices
- **Fix:** Replaced 4-line match block with single `?` expression
- **Files modified:** `crates/ironhermes-core/src/config_setter.rs`
- **Commit:** 34b35de

### Deferred Issues (pre-existing, out of scope)

- `commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available` — pre-existing test failure (cron command returns "cron store not configured" instead of stub message); confirmed failing on base commit `fb3b03e` before any plan changes
- 8 clippy warnings in pre-existing files: `commands/handlers.rs` (4), `commands/typo.rs` (2), `config.rs` (1), `memory_store.rs` (1), `skills.rs` (4) — all pre-date this plan

## Known Stubs

- `apply_gateway_section_answer` — no-op, Phase 25 implementation
- `apply_tools_section_answer` — no-op, Phase 26 implementation
- `learning.*` config keys (learning.periodic_nudge_interval_seconds, learning.reflection_depth, learning.skill_eval, learning.max_skills, learning.skill_generation_enabled) — written to config.yaml by wizard but inert until Phase 32/33 wire them per D-15

These stubs are intentional per D-02/D-03 section deferral decisions and D-15 Phase 32/33 reservation. Plan 23-02 does not need these stubs resolved.

## Self-Check: PASSED

Files created exist:
- crates/ironhermes-core/src/wizard.rs ✓
- crates/ironhermes-core/src/config_validate.rs ✓
- crates/ironhermes-core/src/config_setter.rs ✓
- crates/ironhermes-core/tests/wizard_flow.rs ✓
- crates/ironhermes-core/tests/config_validate.rs ✓
- crates/ironhermes-core/tests/config_setter.rs ✓

Commits exist: 73e119e, be721e5, 34b35de ✓
