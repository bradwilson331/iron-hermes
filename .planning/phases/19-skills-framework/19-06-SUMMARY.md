---
phase: 19-skills-framework
plan: 06
subsystem: exec
tags: [skills, sandbox, env-passthrough, security, tdd, rust]

requires:
  - phase: 19-skills-framework/01
    provides: "HermesMetadata.required_environment_variables (Vec<EnvVarEntry>) typed on SkillRecord"
  - phase: 19-skills-framework/03
    provides: "SkillsTool active_skills (Arc<Mutex<Vec<SkillRecord>>>) shared across tools"
provides:
  - "Sandbox::build_env with skill_env_whitelist: &[String] bypass for SECRET_PATTERNS strip (D-05)"
  - "Sandbox::run signature now threads skill_env_whitelist to build_env"
  - "active_skill_env_names(&[SkillRecord]) -> Vec<String> helper in ironhermes-tools::skills_tool"
  - "ExecuteCodeTool::with_active_skills ctor; ToolRegistry::register_execute_code_tool_with_active_skills"
  - "CLI wiring: skill-declared env vars reach Python sandbox child process (SKILL-11 end-to-end)"
affects: [phase-19.1 community-skills-trust-ux, future-sandbox-consumers]

tech-stack:
  added: []
  patterns:
    - "Declared-and-present whitelist: skill frontmatter supplies NAMES, build_env still reads std::env::vars() — absent vars are naturally never injected (D-05)"
    - "Case-insensitive whitelist comparison via single .to_uppercase() normalization pass"
    - "Ctor + alt-ctor pattern (ExecuteCodeTool::new / ::with_active_skills) preserves test ergonomics while adding optional wiring"

key-files:
  created: []
  modified:
    - "crates/ironhermes-exec/src/sandbox.rs"
    - "crates/ironhermes-exec/src/rpc_server.rs"
    - "crates/ironhermes-tools/src/skills_tool.rs"
    - "crates/ironhermes-tools/src/execute_code.rs"
    - "crates/ironhermes-tools/src/registry.rs"
    - "crates/ironhermes-cli/src/main.rs"
    - "crates/ironhermes-gateway/src/handler.rs"

key-decisions:
  - "Used secondary ctor ExecuteCodeTool::with_active_skills rather than breaking existing ExecuteCodeTool::new to avoid churn in Phase 8 tests"
  - "active_skill_env_names does NOT check parent env presence — delegated to build_env's std::env::vars() filter (preserves single source of truth for 'is it set?')"
  - "Whitelist comparison normalizes via .to_uppercase() once (not per-iter) for small efficiency"
  - "Pre-existing delegate_task schema test failure logged to deferred-items.md rather than fixed (out of scope per Plan 06 file list)"

patterns-established:
  - "D-05 whitelist idiom: declared NAMES via skill frontmatter + build_env presence filter = zero empty-env injection risk"
  - "Optional-shared-state ctor pattern: new() for tests, with_active_skills() for production wiring"

requirements-completed: [SKILL-11]

# Metrics
duration: 7min
completed: 2026-04-14
---

# Phase 19 Plan 06: Skill Env Whitelist Summary

**Sandbox::build_env gains a skill-declared env whitelist so skill-activated API keys (e.g. TENOR_API_KEY) reach the Python child process while all other secret-pattern vars remain stripped.**

## Performance

- **Duration:** ~7 min
- **Started:** 2026-04-15T02:10:54Z
- **Completed:** 2026-04-15T02:17:22Z
- **Tasks:** 2 (TDD RED + GREEN)
- **Files modified:** 7 source files + 1 deferred-items doc

## Accomplishments
- Closed SKILL-11 end-to-end: user exports a skill-declared env var, activates the skill, and `os.environ[VAR]` resolves inside Python sandbox child.
- Preserved Phase 8 D-35 secret-strip defense for undeclared vars — zero regression.
- Case-insensitive whitelist comparison eliminates `tenor_api_key` vs `TENOR_API_KEY` mismatch bypass (T-19-env-leak-case-mismatch mitigation).
- Workspace test count increased by 11 (8 plan tests + 3 recovered by fixing gateway test helper).

## Task Commits

Each task was committed atomically following TDD:

1. **Task 1: Wave 0 RED tests** — `f3d03ba` (test)
   - 8 failing tests proving target API (Sandbox::run 4-arg, build_env 3-arg, active_skill_env_names)
2. **Task 2: GREEN implementation + call-site wiring** — `cab58df` (feat)
   - build_env + run signature extensions, helper, ExecuteCodeTool ctor, CLI wiring, gateway test fix

**Plan metadata commit:** to follow (docs: complete plan)

## Files Created/Modified
- `crates/ironhermes-exec/src/sandbox.rs` — `build_env(temp_dir, socket_path, skill_env_whitelist)` with case-insensitive whitelist bypass; `run(..., &[String])` threads whitelist; 5 new tests (4 build_env unit + 1 Python-child passthrough integration)
- `crates/ironhermes-exec/src/rpc_server.rs` — existing `sandbox.run(...)` test call sites updated to pass `&[]`
- `crates/ironhermes-tools/src/skills_tool.rs` — `pub fn active_skill_env_names(&[SkillRecord]) -> Vec<String>` helper; 3 new tests (happy path, empty, skips-hermes-meta-None)
- `crates/ironhermes-tools/src/execute_code.rs` — ExecuteCodeTool gains optional `active_skills: Option<Arc<Mutex<Vec<SkillRecord>>>>`; `with_active_skills` ctor; `execute()` computes whitelist per-invocation
- `crates/ironhermes-tools/src/registry.rs` — `register_execute_code_tool_with_active_skills` helper for production wiring
- `crates/ironhermes-cli/src/main.rs` — wires `active_skills.clone()` into execute_code registration
- `crates/ironhermes-gateway/src/handler.rs` — test helper updated to include Plan 01's new `hermes_metadata` + `source` SkillRecord fields (Rule 3 blocker — gateway test crate wouldn't compile otherwise)

## Decisions Made
- **Decision: Declared-and-present only, never declared-but-absent** — `active_skill_env_names` returns names regardless of env presence; `build_env` filters on `std::env::vars()` so absent declared vars naturally never enter child env. Rationale: single source of truth for "is it set?" + avoids empty-var leakage.
- **Decision: Secondary ctor instead of breaking ExecuteCodeTool::new** — keeps Phase 8 test ergonomics; production CLI wiring uses `with_active_skills` + registry helper.
- **Decision: Case-insensitive whitelist** — whitelist normalized to uppercase once; matches `upper == *w` against uppercase env name. Eliminates a full class of mismatch bypasses.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Fixed gateway handler test helper missing new SkillRecord fields**
- **Found during:** Task 2 (cargo test --workspace verification gate)
- **Issue:** `crates/ironhermes-gateway/src/handler.rs:560` `make_skill_record` helper built a `SkillRecord` literal missing `hermes_metadata` and `source` fields added by Plan 01. Gateway test crate wouldn't compile, blocking the plan's `cargo test --workspace` verification gate.
- **Fix:** Added `hermes_metadata: None` and `source: ironhermes_core::SkillSource::Builtin` to the struct literal.
- **Files modified:** `crates/ironhermes-gateway/src/handler.rs`
- **Verification:** `cargo test -p ironhermes-gateway` compiles; its tests pass (34 tests).
- **Committed in:** `cab58df` (Task 2 commit)

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Necessary for workspace test gate. Trivial field addition, zero behavior change.

## Issues Encountered

- **Pre-existing failing test (out of scope):** `ironhermes-tools::delegate_task::tests::test_delegate_task_schema_has_required_task` at `crates/ironhermes-tools/src/delegate_task.rs:757` fails with `schema must have 'task' as required`. Last modified in Phase 09/11 (commit `6d63a69`). Plan 06 does not touch `delegate_task.rs` and the failure mode (missing required "task" field in schema) is unrelated to env-var whitelisting. Logged in `.planning/phases/19-skills-framework/deferred-items.md` for Phase 20 backlog.

## Self-Check: PASSED

- `crates/ironhermes-exec/src/sandbox.rs` — FOUND (modified)
- `crates/ironhermes-tools/src/skills_tool.rs` — FOUND (modified)
- `crates/ironhermes-tools/src/execute_code.rs` — FOUND (modified)
- `crates/ironhermes-tools/src/registry.rs` — FOUND (modified)
- `crates/ironhermes-cli/src/main.rs` — FOUND (modified)
- `crates/ironhermes-gateway/src/handler.rs` — FOUND (modified)
- Commit `f3d03ba` (test RED) — FOUND
- Commit `cab58df` (feat GREEN) — FOUND
- All 8 new tests execute green; workspace tests: 690+ passed, 1 pre-existing failure (delegate_task schema, out of scope).

## User Setup Required

None — no external service configuration required. The end-user benefit is now exposed: export a skill-declared API key (e.g. `export TENOR_API_KEY=...`), activate the corresponding skill, and sandbox Python can read `os.environ['TENOR_API_KEY']`.

## Next Phase Readiness

- **SKILL-11 closed.** Phase 19 skills framework end-to-end: parse → activate → config inject → registry scan → env whitelist.
- **Phase 19 is now complete** (all 6 plans landed: 01 typed metadata, 02 catalog filter, 03 setup-error envelope, 04 config body injection, 05 registry-load security scan, 06 env whitelist).
- **Phase 19.1 (community trust UX) readiness:** the D-05 pattern (whitelist via active skill list) composes with future trust-level gating — community-origin skills can be filtered out of the whitelist source list without changing build_env.

---
*Phase: 19-skills-framework*
*Completed: 2026-04-14*
