---
phase: 23-configuration-cli-and-setup-wizard
verified: 2026-04-28T00:00:00Z
status: human_needed
score: 4/4 roadmap success criteria verified
overrides_applied: 0
human_verification:
  - test: "First-run wizard UX (CFG-01 / D-16)"
    expected: "Running `hermes` with no config.yaml launches wizard; D-16 framing paragraph appears before Learning Loop prompt; chat resumes after wizard completes"
    why_human: "Requires a real interactive TTY, rustyline readline, and a valid API key to exercise the full flow end-to-end. Automated tests use the apply_minimum_viable_answers testability seam and assert_cmd process-level checks; they cannot observe the rendered rustyline prompts."
  - test: "hermes config show Learning Loop banner and secret redaction (CFG-02 / D-09 / D-17)"
    expected: "First line is Learning Loop status banner; model.api_key appears as sk-OR-*** (prefix only); full key never appears in stdout"
    why_human: "The unit tests for mask_secret and redact_secrets pass, and the config_show_redaction integration tests pass using a fake config.yaml. A human UAT with a real configured key provides final confirmation that the mask prefix-preservation formula handles the actual key format on disk. (UAT 3 was performed and passed per 23-02-SUMMARY.md §UAT Findings — this item is listed for completeness.)"
---

# Phase 23: Configuration CLI and Setup Wizard — Verification Report

**Phase Goal:** Users can configure IronHermes interactively on first run and manage config values from the command line.
**Verified:** 2026-04-28
**Status:** human_needed
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths (Roadmap Success Criteria)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Running `hermes` for the first time launches an interactive setup wizard that asks for provider selection, API key, model, and writes a valid `config.yaml` | VERIFIED | `preflight::run_preflight_check` (preflight.rs:10) fires on `None` / `Chat` commands when no config.yaml exists, dispatching `run_setup(None, WizardMode::FirstRun)`. `run_minimum_viable_flow` (setup.rs:101) prompts provider, API key, model, Learning Loop; calls `config.save_to` + splices `learning.*` block. Tests: `setup_wizard::test_setup_subcommand_skips_preflight`, `minimum_viable_answers_seed_full_config`. Human TTY confirmation needed — see §Human Verification. |
| 2 | `hermes config set <key> <value>` updates a config.yaml key and `hermes config get <key>` reads it back | VERIFIED | `cmd_config_set` (config_cli.rs:47) calls `config_setter::config_set`; `cmd_config_get` (config_cli.rs:63) calls `config_setter::config_get`. Both wired to `Commands::Config { subcommand }` in main.rs:330. config_setter tests: `config_set_creates_file_and_sets_model_default`, `config_get_returns_some_for_existing_path_and_none_for_missing`, `config_set_preserves_other_keys` — all 8 tests green. |
| 3 | `hermes config show` prints the active config with redacted secrets | VERIFIED | `cmd_config_show` (config_cli.rs:113) reads config.yaml, calls `redact_secrets` which walks all `secret: true` SCHEMA fields via `redact_at`, then prints masked YAML with D-17 Learning Loop banner first. `config_show_redaction` tests: `config_show_masks_api_key`, `config_show_non_secrets_unmasked`, `config_show_learning_loop_enabled_banner`, `config_show_learning_loop_disabled_banner`, `config_show_no_config_friendly_message` — all 5 green. |
| 4 | `hermes config migrate` scans installed skills for unconfigured settings and prompts the user to fill them in | VERIFIED | `cmd_config_migrate` (config_cli.rs:153) calls `SkillRegistry::load_with_paths`, walks `hermes_metadata.config` and `hermes_metadata.required_environment_variables`, diffs against live config.yaml and .env, prints gap table, prompts per-gap with skip/skip-all affordances (D-11). Tests: `migrate_with_no_skills_dir`, `migrate_surfaces_skill_config_gap` — both green. |

**Score:** 4/4 roadmap success criteria verified

---

### Requirements Coverage

| Requirement | Description | Status | Evidence |
|-------------|-------------|--------|----------|
| CFG-01 | Interactive setup wizard for first-run configuration | VERIFIED | `hermes setup [section]` in setup.rs; preflight dispatch in main.rs:219-223; 4-question minimum-viable flow; Learning Loop opt-in with D-16 framing |
| CFG-02 | `hermes config set/get/show` for managing config.yaml values | VERIFIED | All six `ConfigSubcommand` variants wired in config_cli.rs; secret redaction in `cmd_config_show`; D-10 cache-break warnings in `cmd_config_set` |
| CFG-03 | `hermes config migrate` scans skills for unconfigured settings | VERIFIED | `cmd_config_migrate` in config_cli.rs:153 with skill frontmatter gap discovery |

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-core/src/config_schema.rs` | ConfigField + cache_breaking + pub fn schema() | VERIFIED | `cache_breaking: bool` at line 21; `pub fn schema()` at line 46; 18-entry registry covering all D-09/D-13/D-18 fields |
| `crates/ironhermes-core/src/wizard.rs` | Pure-function apply_*_answer helpers + LEARNING_LOOP_FRAMING | VERIFIED | All 8 `apply_*_answer` functions present; `LEARNING_LOOP_FRAMING` const at line 24; `WizardMode` enum at line 12 |
| `crates/ironhermes-core/src/config_validate.rs` | ConfigValidationError + Config::validate() | VERIFIED | `ConfigValidationError` struct at line 11; `Config::validate()` at line 20; checks api_key, model.default, model.provider, memory.provider |
| `crates/ironhermes-core/src/config_setter.rs` | config_set/config_get/is_cache_breaking; unknown-key survival | VERIFIED | All three pub functions present; `serde_yaml::Value`-based (zero `Config` import — D-15 Anti-Pattern #1 guard confirmed); D-15 anchor test green |
| `crates/ironhermes-cli/src/setup.rs` | run_setup + rustyline wizard + apply_minimum_viable_answers seam | VERIFIED | `run_setup` at line 71; 4-section dispatch; `apply_minimum_viable_answers` testability seam at line 250; history anti-pattern guard comment present |
| `crates/ironhermes-cli/src/config_cli.rs` | ConfigSubcommand enum + handle_config_command | VERIFIED | 6-variant `ConfigSubcommand` at line 13; `handle_config_command` dispatcher at line 29; mask_secret + redact_secrets + redact_at present |
| `crates/ironhermes-cli/src/preflight.rs` | run_preflight_check — missing/invalid config detection | VERIFIED | `run_preflight_check` at line 10; handles missing config (FirstRun), load error (FixMode), validate() non-empty (FixMode) |
| `crates/ironhermes-cli/src/main.rs` | Commands::Setup + Commands::Config variants; preflight call | VERIFIED | `Commands::Setup { section }` at line 151; `Commands::Config { subcommand }` at line 156; preflight gate at line 219; dispatch arms at lines 327 and 330 |
| `crates/ironhermes-core/src/lib.rs` | pub mod wizard; pub mod config_validate; pub mod config_setter | VERIFIED | Lines 4-7: all three modules declared `pub mod` |
| `crates/ironhermes-core/tests/wizard_flow.rs` | Pure-function wizard tests | VERIFIED | 16 tests — all pass |
| `crates/ironhermes-core/tests/config_validate.rs` | Config::validate() property tests | VERIFIED | 9 tests — all pass |
| `crates/ironhermes-core/tests/config_setter.rs` | Round-trip + D-15 anchor test | VERIFIED | 8 tests — all pass; `unknown_keys_survive_roundtrip_d15` green |
| `crates/ironhermes-cli/tests/setup_wizard.rs` | CLI integration tests | VERIFIED | 16 tests — all pass |
| `crates/ironhermes-cli/tests/config_show_redaction.rs` | Redaction integration tests | VERIFIED | 5 tests — all pass |
| `crates/ironhermes-cli/tests/config_migrate_discovery.rs` | Migrate discovery tests | VERIFIED | 2 tests — all pass |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `main.rs::preflight gate` | `preflight::run_preflight_check` | `matches!(cli.command, Some(Commands::Chat {..}) | None) && cli.execute.is_none()` | WIRED | Lines 219-223; narrowed to interactive-only entry points per post-merge fix commit 4e2dbc2 |
| `main.rs::Commands::Setup` | `setup::run_setup` | dispatch arm at line 327 | WIRED | `Some(Commands::Setup { ref section }) => { setup::run_setup(section.as_deref(), WizardMode::Explicit).await? }` |
| `main.rs::Commands::Config` | `config_cli::handle_config_command` | dispatch arm at line 330 | WIRED | `Some(Commands::Config { subcommand }) => { config_cli::handle_config_command(subcommand).await? }` |
| `setup.rs::run_minimum_viable_flow` | `wizard::apply_learning_loop_answer` | direct call; returns `serde_yaml::Mapping` | WIRED | setup.rs:127; D-15 splice loop at setup.rs:134-146 |
| `setup.rs::learning block splice` | `config_setter::config_set` | per-key iteration over `serde_yaml::Mapping` | WIRED | setup.rs:145; preserves unknown keys per D-15 |
| `config_cli.rs::cmd_config_set` | `config_schema::schema` + `config_setter::is_cache_breaking` | D-10 warning path | WIRED | config_cli.rs:48-56; emits warning to stderr before persisting |
| `config_cli.rs::cmd_config_show` | `config_schema::schema` + `redact_secrets` | D-09 secret masking | WIRED | config_cli.rs:145-146 |
| `preflight.rs` | `config_validate::Config::validate` | `config.validate().is_empty()` | WIRED | preflight.rs:20 |
| `wizard::apply_learning_loop_answer` | `config.memory.memory_enabled` | direct field mutation | WIRED | wizard.rs:75-76 |

---

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `config_cli.rs::cmd_config_show` | `doc` (serde_yaml::Value) | `std::fs::read_to_string(cfg_path)` → `serde_yaml::from_str` | Yes — reads actual config.yaml | FLOWING |
| `config_cli.rs::cmd_config_show` (LL banner) | `memory_enabled`, `skill_gen` | `config_setter::config_get` reading live config.yaml | Yes | FLOWING |
| `config_cli.rs::cmd_config_migrate` | `config_gaps`, `env_gaps` | `SkillRegistry::load_with_paths` + `config_setter::config_get` per field | Yes | FLOWING |
| `config_setter::config_set` | `doc` | `load_doc` reads config.yaml; `save_doc` writes back | Yes — file I/O round-trip | FLOWING |
| `config_validate::Config::validate` | `errors` | `self.model.api_key`, `self.model.default`, `self.model.provider`, `self.memory.*` | Yes — reads typed Config fields | FLOWING |

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| config_schema lib tests (4+5 = 9 tests) | `cargo test -p ironhermes-core --lib config_schema` | 9 passed, 0 failed | PASS |
| wizard_flow integration tests (16 tests) | `cargo test -p ironhermes-core --test wizard_flow` | 16 passed, 0 failed | PASS |
| config_validate integration tests (9 tests) | `cargo test -p ironhermes-core --test config_validate` | 9 passed, 0 failed | PASS |
| config_setter integration tests (8 tests) | `cargo test -p ironhermes-core --test config_setter` | 8 passed, 0 failed | PASS |
| setup_wizard CLI integration (16 tests) | `cargo test -p ironhermes-cli --test setup_wizard` | 16 passed, 0 failed | PASS |
| config_show_redaction CLI integration (5 tests) | `cargo test -p ironhermes-cli --test config_show_redaction` | 5 passed, 0 failed | PASS |
| config_migrate_discovery CLI integration (2 tests) | `cargo test -p ironhermes-cli --test config_migrate_discovery` | 2 passed, 0 failed | PASS |
| Pre-existing handlers failure (out of scope) | `dispatch_all_todo_stubs_return_not_yet_available` | 1 failure — cron returns real response not stub message | OUT-OF-SCOPE — predates Phase 23 (introduced Phase 22.4.2.1 commit 96b19c1; confirmed failing on base commit fb3b03e before any Phase 23 changes) |

**Total Phase 23 tests: 65 passing, 0 failing**

---

### Anti-Patterns Scan

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `setup.rs::run_gateway_section` | Returns Ok(()) immediately after printing "Gateway setup will gain... Phase 25" | INFO — intentional stub | Documented in CONTEXT.md D-02/D-03 deferral; Phase 25 owns implementation. Not a blocker. |
| `setup.rs::run_tools_section` | Returns Ok(()) immediately after printing "Tools setup... Phase 25" | INFO — intentional stub | Same deferral rationale. |
| `wizard.rs::apply_gateway_section_answer` | `Ok(())` no-op | INFO — intentional stub | Phase 25/26 hook point per plan must_haves. |
| `wizard.rs::apply_tools_section_answer` | `Ok(())` no-op | INFO — intentional stub | Same. |
| `learning.*` config keys | Written to config.yaml but not consumed by any runtime logic | INFO — intentional reservation | D-15 explicitly documents these as Phase 32/33 reservations. `unknown_keys_survive_roundtrip_d15` test locks the contract. |

No blocker anti-patterns found. All stub/placeholder items are explicitly deferred per CONTEXT.md decisions with downstream phase assignments.

---

### Plan 23-01 must_haves.truths Verification

| Truth | Status | Evidence |
|-------|--------|----------|
| ConfigField gains `cache_breaking: bool` with `#[serde(default)]` | VERIFIED | config_schema.rs:21 |
| All D-13+D-18 cache-breaking fields enumerated in SCHEMA registry | VERIFIED | schema() returns 8 cache-breaking entries; `schema_contains_all_cache_breaking_fields` test green |
| All D-09 secret fields enumerated in SCHEMA tagged `secret: true` | VERIFIED | 5 secret entries in schema(); `schema_contains_all_secret_fields` test green |
| Config::validate() returns Vec<ConfigValidationError { path, reason, suggested_fix }> | VERIFIED | config_validate.rs:20; 9 integration tests green |
| wizard.rs exposes pure-function helpers with no I/O | VERIFIED | wizard.rs imports only `crate::config::Config` and serde — no rustyline, no std::io |
| apply_learning_loop_answer with empty/"y" writes memory_enabled=true + full learning.* block | VERIFIED | wizard.rs:70-100; tests `learning_loop_yes_writes_full_block` + `learning_loop_empty_input_defaults_to_yes` green |
| apply_learning_loop_answer with "n" writes explicit false sentinels (never absent) | VERIFIED | wizard.rs:70-100; test `learning_loop_no_writes_explicit_false_never_absent` green |
| config_setter::config_set loads as serde_yaml::Value; unknown keys survive round-trip | VERIFIED | config_setter.rs operates entirely on `serde_yaml::Value`; `unknown_keys_survive_roundtrip_d15` green |
| config_setter::config_get returns Option<String> scalar | VERIFIED | config_setter.rs:92 |
| is_cache_breaking(dotted_path, schema) returns true iff key in SCHEMA with cache_breaking=true | VERIFIED | config_setter.rs:101; `is_cache_breaking_uses_schema_correctly` test green |
| Plan 01 does NOT modify main.rs, setup.rs, config_cli.rs | VERIFIED | SUMMARY confirms untouched; both files created in Plan 02 commits |
| Plan 01 does NOT spawn rustyline editors | VERIFIED | wizard.rs has zero rustyline imports |
| LEARNING_LOOP_FRAMING const locked by regression test with 3 D-16 phrases | VERIFIED | wizard.rs:24; test `learning_loop_framing_locked_phrases_present` checks "Learning Loop", "grow with you", "hermes config set memory.enabled false" |

### Plan 23-02 must_haves.truths Verification

| Truth | Status | Evidence |
|-------|--------|----------|
| ConfigSubcommand enum: Set/Get/Show/Migrate/Path/EnvPath | VERIFIED | config_cli.rs:13-27 |
| handle_config_command dispatcher wired to all 6 variants | VERIFIED | config_cli.rs:29-45 |
| run_setup dispatches section variants; agent→"Phase 26"; skills→"Phase 28"; unknown→error with valid list | VERIFIED | setup.rs:78-90 |
| apply_minimum_viable_answers testability seam bypasses rustyline | VERIFIED | setup.rs:250-261; used by 16 setup_wizard tests |
| cmd_config_set emits D-10 cache-break warning to stderr | VERIFIED | config_cli.rs:49-56; `config_set_cache_breaking_warns_then_persists` test green |
| cmd_config_show renders D-17 Learning Loop banner first | VERIFIED | config_cli.rs:127-141; `config_show_learning_loop_enabled_banner` + `config_show_learning_loop_disabled_banner` tests green |
| mask_secret: prefix-preserved redaction (4-6 chars + ***) | VERIFIED | config_cli.rs:73-79; `config_show_masks_api_key` test green |
| run_preflight_check: missing config → FirstRun; load error → FixMode; validate() non-empty → FixMode | VERIFIED | preflight.rs:10-26; behavior confirmed by setup_wizard tests |
| Preflight skip-list: Setup, Config, Version, Doctor, Status, Cron, etc. do NOT trigger wizard | VERIFIED | main.rs:219 — `matches!(cli.command, Some(Commands::Chat {..}) | None)`; tests `setup_subcommand_skips_preflight`, `config_subcommand_skips_preflight`, `version_skips_preflight` green |
| Wizard history anti-pattern guard: no load_history/save_history in setup.rs | VERIFIED | setup.rs:26-27 guard comment; `wizard_does_not_persist_history` source-grep test green |
| LEARNING_LOOP_FRAMING sourced from wizard.rs const (not duplicated in setup.rs) | VERIFIED | setup.rs:14 imports `LEARNING_LOOP_FRAMING`; `setup_source_uses_learning_loop_framing_const` source-grep test green |
| cmd_config_migrate uses SkillRegistry.list() for skill walk | VERIFIED | config_cli.rs:172; gap discovery loops over `registry.list()` |

---

### Human Verification Required

#### 1. First-Run Wizard TTY Experience

**Test:** In a shell with a valid OpenRouter or Anthropic API key available, run:
```bash
export IRONHERMES_HOME=/tmp/uat-23-$(date +%s)
cargo run -p ironhermes-cli -- chat
```
**Expected:**
- Wizard launches immediately (no existing config.yaml)
- Prompts appear in order: Provider, API key, Default model, Learning Loop opt-in
- The full D-16 framing paragraph appears verbatim before the opt-in prompt:
  > "IronHermes can curate its own memory and write its own skills as you use it — this is the "Learning Loop" that makes the agent grow with you instead of starting fresh every session. We strongly recommend enabling it now..."
- Pressing Enter at the Learning Loop prompt (default Y) writes `memory.memory_enabled: true` and the full `learning.*` block
- After wizard completes, chat session starts normally (not stuck in wizard loop)

**Why human:** Real rustyline TTY interaction and LLM round-trip required; automated tests use the `apply_minimum_viable_answers` seam which bypasses readline.

**Note:** UAT was performed per 23-02-SUMMARY.md §UAT Findings (2026-04-28) and all four cases (1a, 1b, 2, 3) reported PASS. This item is retained for completeness of the verification record; a fresh independent run would provide the highest confidence.

#### 2. Secret Redaction with Real Key

**Test:**
```bash
cargo run -p ironhermes-cli -- config show
```
**Expected:**
- `model.api_key` line shows masked value (e.g., `sk-or-***` — first 4-6 chars only)
- Full API key text does NOT appear anywhere in stdout
- First output line is the Learning Loop status banner

**Why human:** Integration tests use a synthetic fixture key. Verification with the actual key format on disk (real prefix length, actual key structure) provides stronger confidence in the masking formula.

---

### Pre-Existing Test Failure (Out of Scope)

`commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available` fails because the `/cron` command now returns "cron store not configured" instead of the original stub message "not yet available". This was introduced by Phase 22.4.2.1 commit 96b19c1 which replaced the cron stub with real dispatch logic but did not update the test assertion. Confirmed failing on base commit `fb3b03e` before any Phase 23 changes were applied. This failure is pre-existing and unrelated to Phase 23.

---

## Gaps Summary

No gaps found. All four roadmap success criteria are verified by code inspection, wiring trace, data-flow trace, and automated test results (65 tests passing). The two human-verification items above are standard UAT for interactive TTY behavior that cannot be automated — they do not represent code gaps.

Phase 23 goal is achieved. The human_needed status reflects that the rustyline first-run wizard experience and live secret redaction require human eyes on a real terminal to confirm end-to-end UX correctness.

---

_Verified: 2026-04-28_
_Verifier: Claude (gsd-verifier)_
