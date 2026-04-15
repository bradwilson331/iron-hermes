---
phase: 19-skills-framework
plan: 03
subsystem: skills
tags: [skills, tool-surface, error-envelope, credentials, setup-needed, rust, serde, tdd]

# Dependency graph
requires:
  - phase: 19-01
    provides: HermesMetadata, EnvVarEntry, CredentialFileEntry, SkillRecord.hermes_metadata
provides:
  - Three-branch SkillsTool::handle_activate (not_found | setup_needed | ok)
  - setup_needed envelope shape aligned with Phase 17 D-15 structured errors
  - SkillsConfig.credential_dir optional path (backward-compatible via serde(default))
  - default_credential_dir(config) helper with 3-level precedence
  - 4-arg SkillsTool::new(registry, active_skills, credential_dir, skills_config) signature
affects: [19-04, 19-05, 19-06]

# Tech tracking
tech-stack:
  added: [serde_yaml workspace dep in ironhermes-tools, dirs workspace dep in ironhermes-tools]
  patterns:
    - "Readiness envelope relay: setup_note constructed as human-quotable string (\"I need $VAR to {required_for}.\") for verbatim LLM relay"
    - "Credential dir precedence: explicit config → HERMES_HOME/credentials → ~/.ironhermes/credentials → ./.ironhermes/credentials"
    - "Per-skill credential isolation via credential_dir.join(skill.name).join(rel) (D-10 path-traversal containment)"
    - "Activation-time requirement checks (catalog filter unchanged; skills stay discoverable even when unready)"

key-files:
  created: []
  modified:
    - crates/ironhermes-tools/src/skills_tool.rs
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-tools/Cargo.toml

key-decisions:
  - "Env-var presence check rejects empty-string values (std::env::var returns Ok(\"\") for unset-but-empty edge cases — treat as missing)"
  - "setup_help populated from the FIRST missing env var with a declared help link (deterministic ordering)"
  - "required_for suffix in setup_note uses only the FIRST hint to keep relay concise (\"I need $X, $Y to do Z.\")"
  - "Env-mutating tests serialized via module-level std::sync::Mutex ENV_LOCK (no external serial_test dep)"
  - "skills_config field marked #[allow(dead_code)] — reserved for Plan 04 per-skill config injection"

patterns-established:
  - "setup_needed envelope: {status, name, readiness_status, missing_required_environment_variables, missing_credential_files, setup_note, setup_help}"
  - "SkillsConfig extension pattern: all new fields use #[serde(default)] + update existing struct-literal tests with ..SkillsConfig::default()"
  - "Credential resolution helper lives in the tool crate (ironhermes-tools), not core — keeps env/dirs dependencies contained"

requirements-completed: [SKILL-04, SKILL-06]

# Metrics
duration: 6min
completed: 2026-04-15
---

# Phase 19 Plan 03: Setup-Error Envelope Summary

**Three-branch SkillsTool::handle_activate flow — not-found / setup-needed / success — with Phase-17-D-15-shaped envelope carrying missing env vars, missing credential files, and a human-quotable setup_note relay.**

## Performance

- **Duration:** ~6 min
- **Started:** 2026-04-15T01:49:57Z
- **Completed:** 2026-04-15T01:55:21Z
- **Tasks:** 2 (1 RED, 1 GREEN)
- **Files modified:** 6

## Accomplishments
- Implemented setup-error envelope that lets the agent verbatim-relay "I need $TENOR_API_KEY to GIF search." to the user when requirements aren't met
- Added `SkillsConfig.credential_dir: Option<PathBuf>` with full serde backward-compat; all 25 config tests still pass
- Migrated `SkillsTool::new` from 2-arg to 4-arg signature workspace-wide (single production call site in `ironhermes-cli/src/main.rs`; registry helper `register_skills_tool` updated)
- Added `default_credential_dir()` helper encoding the 3-level precedence (config → HERMES_HOME → home) so all callers (including future Plan 04/05) share one resolution path
- 21/21 skills_tool tests green (6 new + 15 existing), all 25 ironhermes-core config tests green, workspace build clean

## Task Commits

1. **Task 1: Wave 0 tests for setup-error envelope (RED)** — `2c24eb0` (test) — 6 failing tests referencing target 4-arg signature
2. **Task 2: Implement three-branch handle_activate (GREEN)** — `6455aaa` (feat) — struct change, three-branch logic, credential_dir helper, call-site migration, skills.rs test-literal fixes

## Files Created/Modified
- `crates/ironhermes-tools/src/skills_tool.rs` — new 4-arg `SkillsTool::new`, `default_credential_dir()`, three-branch `handle_activate`, 6 new tests with ENV_LOCK Mutex for serialization
- `crates/ironhermes-core/src/config.rs` — added `SkillsConfig.credential_dir: Option<PathBuf>` with `#[serde(default)]`
- `crates/ironhermes-tools/src/registry.rs` — `register_skills_tool` now takes `credential_dir` and `skills_config` args
- `crates/ironhermes-cli/src/main.rs` — CLI call site passes `default_credential_dir(&config.skills)` and empty `HashMap`
- `crates/ironhermes-core/src/skills.rs` — 3 existing test literals updated with `..SkillsConfig::default()` to stay robust to future field additions
- `crates/ironhermes-tools/Cargo.toml` — added `serde_yaml` and `dirs` workspace deps (needed by new 4-arg signature + helper)

## Decisions Made
- Treat `Ok("")` from `std::env::var` as missing (rare but possible when a user literally exports `FOO=`). Avoids the user thinking they set it and then being confused by a runtime failure later.
- `setup_help` pulls from the FIRST missing env var's `help` field (deterministic, avoids picking ambiguously between competing help URLs).
- `required_for` suffix in `setup_note` uses only the first hint so the relay stays short; this matches the plan's exact format spec `"I need {joined}{ to required_for}."`.
- No external test-isolation crate (serial_test, temp_env): used a module-local `static ENV_LOCK: StdMutex<()>` to serialize env-mutating tests. Keeps the dep tree unchanged.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added missing workspace deps `serde_yaml` and `dirs` to `ironhermes-tools/Cargo.toml`**
- **Found during:** Task 1 RED (compile time)
- **Issue:** The target 4-arg signature `SkillsTool::new(..., HashMap<String, HashMap<String, serde_yaml::Value>>)` needs `serde_yaml` in ironhermes-tools, and `default_credential_dir` uses `dirs::home_dir()`. Neither dep was declared in the crate's Cargo.toml (both are available as workspace deps).
- **Fix:** Added `serde_yaml = { workspace = true }` and `dirs = { workspace = true }` to `[dependencies]`.
- **Verification:** `cargo build -p ironhermes-tools --tests` passes.
- **Committed in:** `6455aaa` (Task 2 commit — bundled with feature since the deps are only exercised by the new code)

**2. [Rule 3 - Blocking] Updated 3 pre-existing `SkillsConfig { ... }` struct literals in `ironhermes-core/src/skills.rs` tests**
- **Found during:** Task 2 GREEN (compile time)
- **Issue:** Adding `credential_dir: Option<PathBuf>` to `SkillsConfig` broke 3 existing tests that constructed `SkillsConfig { enabled, extra_paths }` without field rest. Pure E0063 blocker.
- **Fix:** Appended `..SkillsConfig::default()` to each of the 3 literals (keeps the tests robust to future additive field changes too).
- **Verification:** `cargo test -p ironhermes-core config` passes (25/25); `cargo test -p ironhermes-core skills::tests::test_load_with_config_*` passes.
- **Committed in:** `6455aaa`

---

**Total deviations:** 2 auto-fixed (2 Rule 3 blocking)
**Impact on plan:** Both auto-fixes were strictly required to make the planned signature change compile — no scope creep, no extra features, just the mechanical keep-it-green work.

## Issues Encountered
- Build warning: `ironhermes-cli/src/batch/runner.rs::reject_file_path` dead-code warning (pre-existing, unrelated).
- `cargo test <NAME1> <NAME2>` is not valid CLI syntax — cargo test accepts a single filter string. Worked around by running the whole `skills_tool` module via `cargo test -p ironhermes-tools skills_tool`.

## Deferred Issues
- `delegate_task::tests::test_delegate_task_schema_has_required_task` fails (pre-existing; last touched in Phase 09/11). Out of scope per SCOPE BOUNDARY. Logged to `.planning/phases/19-skills-framework/deferred-items.md`.

## Known Stubs
- `SkillsTool.skills_config` field is stored but not yet read in this plan. It's declared `#[allow(dead_code)]` with a comment pointing to Plan 04, which will wire it into the body-injection flow. This is an intentional stub documented in the PLAN.md Task 2 action step 2.

## User Setup Required
None — no external service configuration required.

## Next Phase Readiness
- **Plan 19-04** (config injection) can now consume `skills_config` field on `SkillsTool` as planned.
- **Plan 19-05** (registry-load scan) mitigates T-19-03-setup-note-injection by scanning `prompt`/`help`/`required_for` string fields before they reach the envelope.
- All public surface stable: `default_credential_dir`, `SkillsTool::new` 4-arg, `SkillsConfig.credential_dir`, setup_needed envelope shape.

## Self-Check: PASSED

Verification run:
- `cargo test -p ironhermes-tools skills_tool` → 21/21 ok
- `cargo test -p ironhermes-core config` → 25/25 ok
- `cargo build --workspace` → Finished (warnings unrelated)
- `git log --oneline | grep -E "(2c24eb0|6455aaa)"` → both present

Acceptance-criteria greps all satisfied:
- `setup_needed`, `missing_required_environment_variables`, `missing_credential_files`, `setup_note` in skills_tool.rs ✓
- `credential_dir: Option<PathBuf>` in config.rs:371 ✓
- `fn default_credential_dir(` in skills_tool.rs:30 ✓
- `credential_dir: PathBuf,` in SkillsTool struct at skills_tool.rs:52 ✓
- `std::env::var(&entry.name)` in skills_tool.rs:141 ✓
- `credential_dir.join(&record.name)` in skills_tool.rs:161 ✓
- All six test functions present: `test_activate_missing_env_var`, `test_activate_missing_credential`, `test_activate_all_requirements_met`, `test_activate_mixed_missing`, `test_activate_not_found`, `test_setup_note_format` ✓
- No 2-arg `SkillsTool::new(` calls remain (grep shows 4 call sites, all 4-arg) ✓

---
*Phase: 19-skills-framework*
*Completed: 2026-04-15*
