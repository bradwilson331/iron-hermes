---
phase: 23
plan: 02
subsystem: ironhermes-cli
tags: [config, wizard, validation, preflight, learning-loop, redaction, rustyline, clap]
dependency_graph:
  requires:
    - crates/ironhermes-core/src/config_schema.rs::schema
    - crates/ironhermes-core/src/wizard.rs::apply_*_answer
    - crates/ironhermes-core/src/config_validate.rs::Config::validate
    - crates/ironhermes-core/src/config_setter.rs::config_set/config_get/is_cache_breaking
  provides:
    - crates/ironhermes-cli/src/setup.rs::run_setup
    - crates/ironhermes-cli/src/setup.rs::apply_minimum_viable_answers
    - crates/ironhermes-cli/src/config_cli.rs::ConfigSubcommand
    - crates/ironhermes-cli/src/config_cli.rs::handle_config_command
    - crates/ironhermes-cli/src/preflight.rs::run_preflight_check
  affects:
    - crates/ironhermes-cli/src/main.rs (Commands enum, dispatch, preflight call)
    - crates/ironhermes-core/src/commands/handlers.rs (cmd_config stub message)
tech_stack:
  added: []
  patterns:
    - "rustyline 15 readline_with_initial for inline-default prompts (D-04)"
    - "apply_minimum_viable_answers testability seam — bypass rustyline in integration tests"
    - "serde_yaml in-place mutation for secret redaction (mask_secret + redact_at)"
    - "SkillRegistry.load_with_paths for migrate gap discovery (hermes_metadata.config + required_environment_variables)"
    - "preflight skip-list via matches!() on Commands enum variants"
key_files:
  created:
    - crates/ironhermes-cli/src/config_cli.rs
    - crates/ironhermes-cli/src/setup.rs
    - crates/ironhermes-cli/src/preflight.rs
    - crates/ironhermes-cli/tests/setup_wizard.rs
    - crates/ironhermes-cli/tests/config_migrate_discovery.rs
    - crates/ironhermes-cli/tests/config_show_redaction.rs
  modified:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/lib.rs
    - crates/ironhermes-core/src/commands/handlers.rs
decisions:
  - "Binary name is 'ironhermes' (not 'hermes') — assert_cmd tests use cargo_bin('ironhermes') per CARGO_BIN_EXE_ naming"
  - "wizard_does_not_persist_history grep test: comment text in setup.rs must not contain 'load_history' or 'save_history' — removed those words from the guard comment"
  - "config_subcommand_skips_preflight uses 'config path' (always succeeds) instead of 'config get' (Task 4 stub) to avoid false test failure ordering dependency"
  - "cmd_config_migrate uses SkillRegistry.list() (not iter()) — only .list() is pub on SkillRegistry"
  - "env_var_exists_in_dotenv: line-prefix match (VAR=) catches both VAR=value and VAR= forms without importing dotenvy"
metrics:
  duration_minutes: 30
  completed_date: "2026-04-28"
  tasks_completed: 7
  files_changed: 9
---

# Phase 23 Plan 02: CLI Wire-Up — hermes setup + hermes config + preflight Summary

Wired Plan 23-01's pure-function core into three CLI surfaces: rustyline-driven `hermes setup [section]` wizard, six `hermes config` subcommands, and first-run pre-flight middleware — closing CFG-01/02/03 with verbatim D-16 Learning Loop framing, D-09 secret redaction, and D-11 skill-gap discovery.

## What Was Built

### Task 1 — Wave 0 scaffold (7d4a4fe)
- `config_cli.rs`: `ConfigSubcommand` enum (Set/Get/Show/Migrate/Path/EnvPath) + `handle_config_command` dispatcher modeled on `cron.rs`
- `setup.rs`: rustyline wizard skeleton with `apply_minimum_viable_answers` testability seam
- `preflight.rs`: `run_preflight_check` — detects missing/invalid config, dispatches FirstRun or FixMode wizard
- `main.rs`: `mod config_cli/preflight/setup`; `Setup { section }` and `Config { subcommand }` Commands variants; dispatch arms; preflight call with skip-list
- `lib.rs`: `pub mod setup` re-export for integration test access
- Three scaffold test files created; `hermes config path` and `hermes config env-path` emit absolute paths and exit 0

### Tasks 2+3 — Wizard flows (a5a44bb)
- `run_minimum_viable_flow`: 4-question flow — provider, API key, model, Learning Loop opt-in with verbatim `LEARNING_LOOP_FRAMING` (D-16) printed before opt-in prompt
- Learning Loop block spliced via `config_setter::config_set` per key after `Config::save_to` (D-15 unknown-key survival)
- `run_model_section`, `run_memory_section` (with Learning Loop re-framing), `run_gateway_section`, `run_tools_section`
- `apply_memory_section_answers` testability seam parallel to `apply_minimum_viable_answers`
- Section error messages: `agent` → "Phase 26", `skills` → "Phase 28", unknown → "unknown setup section: X (valid: model, memory, gateway, tools)"
- 12 tests covering: minimum-viable flow, LL Y/N, LEARNING_LOOP_FRAMING source lock, no history persistence, all section dispatches, preflight skip-list

### Task 4 — config set/get (5da580a)
- `cmd_config_set`: calls `is_cache_breaking`, emits D-10 warning to stderr (`⚠ Changing <key> invalidates the prompt cache...`) before `config_setter::config_set`; prints `Persisted: key = value` to stdout
- `cmd_config_get`: prints raw scalar to stdout; missing key exits 0 silently
- 4 tests: cache-breaking warns, non-cache-breaking silent, get roundtrip, missing key silent

### Task 5 — config show (50403f1)
- `cmd_config_show`: D-17 Learning Loop banner as first line (enabled/disabled based on memory + learning.skill_generation_enabled)
- `mask_secret(value)`: first 4–6 chars + `***` per D-09
- `redact_secrets`/`redact_at`: in-place YAML mutation walks all secret SCHEMA fields
- Missing config.yaml → friendly message + exit 0
- 5 tests: API key masked, telegram token masked, LL enabled banner, LL disabled banner, no-config message, non-secrets unmasked

### Task 6 — config migrate (06dc15f)
- `cmd_config_migrate`: loads `SkillRegistry.load_with_paths`, walks `hermes_metadata.config` (SkillConfigField.key) + `hermes_metadata.required_environment_variables` (EnvVarEntry.name)
- Gap table printed; per-gap prompts with skip/skip-all/y via rustyline; .env gaps printed with path hint
- `env_var_exists_in_dotenv`: line-prefix match for `VAR=` in .env file
- 2 tests: no-skills-dir friendly message, skill with config gap surfaces the key

### Task 7 — Preflight + handlers.rs (d5ba4fe)
- `run_preflight_check` verified: missing config → FirstRun, load error → FixMode, validate() non-empty → FixMode, valid → Ok(())
- `cmd_config` slash-command stub updated to: "Use `hermes config show` to inspect, `hermes config set <key> <value>` to change, or `hermes config migrate` to discover skill gaps."

## Test Counts

| Suite | Tests | Status |
|-------|-------|--------|
| setup_wizard (integration) | 16 | all pass |
| config_show_redaction (integration) | 5 | all pass |
| config_migrate_discovery (integration) | 2 | all pass |
| **Total new** | **23** | **all pass** |

## D-XX Decisions Covered

| Decision | Coverage |
|----------|----------|
| D-01 | `hermes setup` (no args) → minimum-viable 4-question flow |
| D-02 | Section dispatch: model/memory/gateway/tools; agent/skills deferred |
| D-03 | Unknown section errors cleanly with valid-section list |
| D-04 | rustyline 15 readline_with_initial for inline defaults |
| D-05 | Preflight auto-launches wizard on missing config or validate() errors |
| D-07 | Transparent resume — preflight runs wizard then command continues |
| D-08 | Dotted-path syntax in config set/get |
| D-09 | Secret field prefix-preserved redaction in config show |
| D-10 | Cache-breaking warning to stderr before config set persistence |
| D-11 | config migrate skill frontmatter gap discovery with skip/skip-all affordances |
| D-14 | Learning Loop opt-in: empty/Y → true; n → explicit false (never absent) |
| D-15 | learning.* block spliced via config_setter after Config::save_to |
| D-16 | Verbatim LEARNING_LOOP_FRAMING rendered before opt-in; source-grep test locks |
| D-17 | Learning Loop banner as first config show line |

## Checkpoint Gate: Task 8 Manual UAT

Task 8 is `type="checkpoint:human-verify"`. The following automated tasks are complete and committed. Manual UAT remains:

**UAT 1: First-run wizard UX (CFG-01 / D-16)**
```bash
export IRONHERMES_HOME=/tmp/uat-23-fresh-$(date +%s)
cargo run -p ironhermes-cli -- chat
# Wizard should launch; verify D-16 framing paragraph before opt-in prompt
# Accept defaults; provide a real API key; verify chat resumes after wizard
```

**UAT 2: `hermes config show` banner glanceability (CFG-02 / D-17)**
```bash
cargo run -p ironhermes-cli -- config show
# First non-blank line: 🧠 Learning Loop: enabled (memory + skill generation)
cargo run -p ironhermes-cli -- config set memory.memory_enabled false
# Verify ⚠ cache-break warning on stderr (D-10)
cargo run -p ironhermes-cli -- config show
# First non-blank line: ⚠ Learning Loop: disabled — IronHermes is operating as a single-session agent...
```

**UAT 3: Secret redaction visual (CFG-02 / D-09)**
```bash
cargo run -p ironhermes-cli -- config show
# model.api_key MUST appear masked (first 4–6 chars + ***); full key must NOT appear in stdout
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Binary name is 'ironhermes' not 'hermes'**
- **Found during:** Task 2 test run
- **Issue:** `assert_cmd::Command::cargo_bin("hermes")` panics — `CARGO_BIN_EXE_hermes` is unset; binary is named `ironhermes` per `[[bin]] name`
- **Fix:** Changed all `cargo_bin("hermes")` to `cargo_bin("ironhermes")` in test files
- **Files modified:** `tests/setup_wizard.rs`, `tests/config_show_redaction.rs`, `tests/config_migrate_discovery.rs`
- **Commit:** a5a44bb

**2. [Rule 1 - Bug] wizard_does_not_persist_history test failed due to comment text**
- **Found during:** Task 2 test run
- **Issue:** Guard comment `// Do NOT call set_max_history_size, load_history, or save_history` contained the substring `load_history` which the source-grep test flagged
- **Fix:** Replaced comment with `// Anti-Pattern #3: no history file persistence — only set_history_ignore_dups is allowed.`
- **Files modified:** `crates/ironhermes-cli/src/setup.rs`
- **Commit:** a5a44bb

**3. [Rule 1 - Bug] config_subcommand_skips_preflight test used stub command**
- **Found during:** Task 2 test run
- **Issue:** `config get model.default` returns exit code 1 (stub "not implemented") — test `.assert().success()` failed even though preflight was correctly skipped
- **Fix:** Changed test to use `config path` (always succeeds in Task 1) — tests the skip-list contract without depending on Task 4 stub state
- **Files modified:** `tests/setup_wizard.rs`
- **Commit:** a5a44bb

**4. [Rule 1 - Bug] env_var_exists_in_dotenv missing closing brace**
- **Found during:** Task 6 build
- **Issue:** Edit that replaced the `cmd_config_migrate` stub omitted the closing `}` of `env_var_exists_in_dotenv` function
- **Fix:** Added closing brace
- **Files modified:** `crates/ironhermes-cli/src/config_cli.rs`
- **Commit:** 06dc15f

### Deferred Issues (pre-existing, out of scope)

- `dispatch_all_todo_stubs_return_not_yet_available` — pre-existing failure (cron returns "cron store not configured"); confirmed per Plan 01 Summary
- `provider_resolver_loads_disk_cache_at_build` — pre-existing test failure unrelated to Phase 23 changes
- 25 pre-existing clippy warnings in `ironhermes-cli` (tui/render.rs, tui/mod.rs) — pre-date this plan

## Known Stubs

- `run_gateway_section` — prints "Gateway setup will gain Telegram/Discord prompts in Phase 25 (TOOL-05)." — no rustyline prompts; Phase 25 implementation
- `run_tools_section` — prints "Tools setup will gain prerequisite-check prompts in Phase 25 (TOOL-05)." — Phase 25 implementation
- `preflight::run_preflight_check` — dispatches wizard but wizard's `run_minimum_viable_flow` requires a real TTY for rustyline; in non-interactive (CI/test) contexts it will error on EOF — expected behavior per Anti-Pattern #3

## Threat Flags

None — no new network endpoints, auth paths, or trust boundary crossings introduced. `config show` redaction (T-23-09) implemented as designed: `mask_secret()` + `redact_at()` with test locking that `12345-secret` does not appear in stdout.

## Self-Check: PASSED

Files created exist:
- crates/ironhermes-cli/src/config_cli.rs ✓
- crates/ironhermes-cli/src/setup.rs ✓
- crates/ironhermes-cli/src/preflight.rs ✓
- crates/ironhermes-cli/tests/setup_wizard.rs ✓
- crates/ironhermes-cli/tests/config_migrate_discovery.rs ✓
- crates/ironhermes-cli/tests/config_show_redaction.rs ✓

Commits exist: 7d4a4fe, a5a44bb, 5da580a, 50403f1, 06dc15f, d5ba4fe ✓

All 23 integration tests pass ✓
