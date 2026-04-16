---
phase: 20-memory-provider-plugin-contract
plan: 04
subsystem: memory
tags: [memory, provider, plugin-contract, config-schema, hooks, sqlite, duckdb, grafeo, tdd]

# Dependency graph
requires:
  - phase: 20-01
    provides: "MemoryProvider trait with name()/get_config_schema() defaults, ConfigField shape, factory that names each provider"
  - phase: 20-02
    provides: "MemoryManager with SharedProvider (Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>), handle_tool_call write-then-mirror path, timeout-bounded mirror fanout, read-path primary-only isolation"
provides:
  - "File, SQLite, DuckDB, Grafeo providers all override name() with stable filename-safe literals"
  - "File, SQLite, DuckDB, Grafeo providers all override get_config_schema() with provider-appropriate fields and defaults"
  - "12 automated assertions pinning name literal + schema shape + secret-implies-env_var invariant across 4 providers"
  - "End-to-end sqlite_mirror_fixture.rs with 4 tokio tests proving on_memory_write composition through MemoryManager"
affects: [20-03-setup-wizard, 22-memory-ui, future memory provider onboarding]

# Tech tracking
tech-stack:
  added: []  # No new production or dev dependencies
  patterns:
    - "Plugin-contract schema pinning via integration tests at provider crate boundary (tests/config_schema.rs pattern)"
    - "Secret-implies-env_var invariant helper pattern — vacuously true for all 4 current providers; codified as reusable assertion"
    - "Mirror fixture MockMirrorProvider — Arc<StdMutex<MirrorInner>> shared-state recorder mirroring 20-02 MockRecorderProvider pattern; adds fail_on_write flag for failure-swallow coverage"

key-files:
  created:
    - "providers/memory-sqlite/tests/config_schema.rs"
    - "providers/memory-duckdb/tests/config_schema.rs"
    - "providers/memory-grafeo/tests/config_schema.rs"
    - "crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs"
  modified:
    - "crates/ironhermes-core/src/memory_provider.rs"  # Added get_config_schema to impl MemoryProvider for MemoryStore (3 fields)
    - "crates/ironhermes-core/src/memory_store.rs"     # Added 3 file-provider unit tests in tests mod
    - "providers/memory-sqlite/src/lib.rs"             # Added get_config_schema to impl (1 field: db_path)
    - "providers/memory-duckdb/src/lib.rs"             # Added get_config_schema to impl (2 fields: db_path, threads)
    - "providers/memory-grafeo/src/lib.rs"             # Added get_config_schema to impl (1 field: graph_dir)

key-decisions:
  - "Plan hinted file-provider impl lives in memory_store.rs; actual impl block lives in memory_provider.rs (MemoryProvider trait impl for MemoryStore, already established in 20-01). Added get_config_schema there; unit tests in memory_store.rs tests mod as planned (Rule 3 scope correction)."
  - "ConfigField.description is Option<String> in actual 20-01 code (plan interface sample shows bare String). Used Some(\"...\".to_string()) for all 4 providers."
  - "Mirror fixture uses Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>> per actual 20-02 SharedProvider type alias (plan samples showed Box<dyn MemoryProvider> + parking_lot::Mutex). No new dependency needed."
  - "DuckDB threads field is declarative only — actual PRAGMA threads=N wiring deferred to follow-on; wizard can still prompt+persist the value via provider_config."
  - "Mirror fixture MockMirrorProvider increments read_calls counter in BOTH prefetch() AND format_for_system_prompt() to catch any future accidental read-fanout. Test 4 asserts count stays 0 after calling prefetch + format_for_system_prompt + system_prompt_block on the manager."

patterns-established:
  - "Schema pinning tests: each provider crate owns tests/config_schema.rs with name-literal, schema-shape, and secret-implies-env_var invariant assertions. Any new provider onboarding must add this test file."
  - "File provider tests co-located in crate tests mod with qualified trait call syntax `<MemoryStore as MemoryProvider>::name(&store)` to disambiguate from inherent methods."
  - "Mirror fixture fail-on-write flag pattern: MockMirrorProvider::new() + ::failing() constructors return (provider, Arc<StdMutex<inner>>) so tests can hold the state handle while the manager owns the provider."

requirements-completed: [MEM-08, MEM-09, MEM-10, MEM-11]

# Metrics
duration: 8min
completed: 2026-04-16
---

# Phase 20 Plan 04: Provider Hook Adoption Summary

**All four MemoryProvider implementations (file, sqlite, duckdb, grafeo) expose stable name() + get_config_schema() overrides with 16 pinning test assertions, plus an end-to-end sqlite_mirror_fixture proving on_memory_write composition through MemoryManager.**

## Performance

- **Duration:** ~8 min (5 commits across 3 TDD task cycles)
- **Started:** 2026-04-16T14:17:27Z
- **Completed:** 2026-04-16T14:21:17Z
- **Tasks:** 3
- **Files modified:** 9 (4 created, 5 modified)

## Accomplishments

- **Plugin-contract closed:** All four compiled-in providers (file, sqlite, duckdb, grafeo) now satisfy the name() + get_config_schema() surface required by the Phase 20-03 setup wizard. Wizard can iterate providers and enumerate config fields without provider-specific code.
- **Schema pinning:** 12 integration-test assertions across 4 providers lock down the exact shape the wizard consumes — any drift becomes a compile-or-test-time regression.
- **Mirror composition proven:** 4 tokio tests in sqlite_mirror_fixture.rs cover (a) single-op propagation, (b) multi-op ordering (Add→Replace→Remove), (c) failure-swallow (D-14: mirror Err is logged, not propagated; outer Ok; primary persisted), (d) read-isolation (D-26/D-28: prefetch + format_for_system_prompt + system_prompt_block never fan out).
- **MEM-12 single-primary invariant** additionally verified at the fixture level: `MemoryManager::new(primary=sqlite, mirror=Some(mock))` composes correctly.

## Task Commits

Each task was committed atomically via TDD (test → feat):

1. **Task 20-04-01 (RED):** test(20-04) for file + sqlite get_config_schema — `509a22c`
2. **Task 20-04-01 (GREEN):** feat(20-04) file + sqlite get_config_schema overrides — `360c4e9`
3. **Task 20-04-02 (RED):** test(20-04) for duckdb + grafeo get_config_schema — `14be442`
4. **Task 20-04-02 (GREEN):** feat(20-04) duckdb + grafeo get_config_schema overrides — `50f89fd`
5. **Task 20-04-03:** test(20-04) sqlite mirror fixture end-to-end — `cfa1bd9`

## Schemas Delivered

### File provider (`MemoryStore`, in `memory_provider.rs`)
```
[
  ConfigField { key: "memory_dir",        secret: false, required: false, default: "$HERMES_HOME/memory",   env_var: None },
  ConfigField { key: "memory_char_limit", secret: false, required: false, default: 2200,                    env_var: None },  // from constants::MEMORY_CHAR_LIMIT
  ConfigField { key: "user_char_limit",   secret: false, required: false, default: 1375,                    env_var: None },  // from constants::USER_CHAR_LIMIT
]
```

### SqliteMemoryProvider
```
[
  ConfigField { key: "db_path", secret: false, required: false, default: "$HERMES_HOME/memory.db", env_var: None }
]
```

### DuckDbMemoryProvider
```
[
  ConfigField { key: "db_path", secret: false, required: false, default: "$HERMES_HOME/memory.duckdb", env_var: None },
  ConfigField { key: "threads", secret: false, required: false, default: 1,                            env_var: None }  // declarative; PRAGMA wiring deferred
]
```

### GrafeoMemoryProvider
```
[
  ConfigField { key: "graph_dir", secret: false, required: false, default: "$HERMES_HOME/grafeo", env_var: None }
]
```

All four providers declare `secret: false` on every field. Secret-implies-env_var invariant holds vacuously for the current set; the assertion helper is in place for any future secret-bearing provider.

## Mirror Fixture Test Results

`cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture` → 4/4 green:

| Test | Covers | Result |
|------|--------|--------|
| sqlite_primary_fires_on_memory_write_to_mirror | D-14, T-20-07 — single Add propagates action/target/content | PASS |
| mirror_observes_replace_and_remove | D-25..D-29 — 3-op sequence Add→Replace→Remove, ordered log | PASS |
| failing_mirror_does_not_block_sqlite_writes | D-14 — mirror Err swallowed, outer Ok, primary persisted, mirror counter=1 | PASS |
| mirror_never_receives_reads | D-26, D-28 — prefetch + format + system_prompt_block keep mirror.read_calls=0 | PASS |

## Files Created/Modified

- `crates/ironhermes-core/src/memory_provider.rs` — Added `get_config_schema()` override to `impl MemoryProvider for MemoryStore` (3 fields).
- `crates/ironhermes-core/src/memory_store.rs` — Added 3 file-provider unit tests in `#[cfg(test)] mod tests` (name literal, schema shape, secret-implies-env_var).
- `providers/memory-sqlite/src/lib.rs` — Added `get_config_schema()` (1 field: db_path).
- `providers/memory-sqlite/tests/config_schema.rs` — New integration test (3 tests).
- `providers/memory-duckdb/src/lib.rs` — Added `get_config_schema()` (2 fields: db_path, threads).
- `providers/memory-duckdb/tests/config_schema.rs` — New integration test (3 tests).
- `providers/memory-grafeo/src/lib.rs` — Added `get_config_schema()` (1 field: graph_dir).
- `providers/memory-grafeo/tests/config_schema.rs` — New integration test (3 tests).
- `crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs` — New end-to-end fixture (4 tokio tests).

## Decisions Made

- **File-provider impl location:** The 20-01 migration placed `impl MemoryProvider for MemoryStore` in `memory_provider.rs` (not `memory_store.rs` as the plan's action steps hinted). Added `get_config_schema()` override at the actual impl site. Tests remain in `memory_store.rs` tests mod as specified — they reference the impl via fully-qualified trait syntax.
- **ConfigField.description is Option<String>:** The 20-01 type has `#[serde(default)] pub description: Option<String>` (per `config_schema.rs`), not bare `String` as the plan's interface sample showed. All four providers use `Some("...".to_string())`. Integration tests assert `description.as_ref().is_some_and(|d| !d.is_empty())`.
- **Mirror fixture signature match:** The 20-02 `MemoryManager::new` takes `SharedProvider = Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`, not `Box<dyn MemoryProvider>` as the plan samples indicated. The fixture uses `Arc::new(Mutex::new(...))` throughout and `SharedProvider` as the public type name — no need for the plan's proposed `ObservableMirror` adapter or `parking_lot` dependency.
- **DuckDB threads scope:** The `threads` field is declarative only (wizard prompts, config persists). Actual `PRAGMA threads=N` wiring in the bridge is out of scope for 20-04; deferred to a follow-on. Test asserts default=1 but does not exercise runtime effect.
- **Char-limit constants sourced from `crate::constants` module:** Plan suggested promoting local module-level constants; they were already `pub const MEMORY_CHAR_LIMIT/USER_CHAR_LIMIT` in `constants.rs` from pre-20-01 code. No refactor needed.

## Deviations from Plan

### Scope Corrections (Rule 3 — Blocking)

**1. [Rule 3] File provider `get_config_schema` written to `memory_provider.rs`, not `memory_store.rs`**
- **Found during:** Task 20-04-01 action step 1
- **Issue:** Plan specified adding `fn name()` + `fn get_config_schema()` "inside the existing impl MemoryProvider for MemoryStore block" and mentioned `memory_store.rs` as the target file. But 20-01 placed that impl block in `memory_provider.rs` alongside the trait definition. `memory_store.rs` contains the `MemoryStore` struct + inherent methods only.
- **Fix:** Added `get_config_schema()` override in `memory_provider.rs` at the real impl site. Tests still live in `memory_store.rs` tests mod as planned (they compile-in the trait via `use super::*;` + qualified syntax). `name()` was already added in 20-01 so only the new method needed placement.
- **Files modified:** crates/ironhermes-core/src/memory_provider.rs (added method), crates/ironhermes-core/src/memory_store.rs (added tests only)
- **Verification:** `cargo test -p ironhermes-core memory_store::tests::file_provider_ --lib` — 3/3 green.
- **Committed in:** 360c4e9

**2. [Rule 3] ConfigField.description type adjustment**
- **Found during:** Task 20-04-01 action step 1
- **Issue:** Plan's interface sample (and action-step code snippets) showed `description: String` / `description: "text".to_string()`. Actual `ConfigField` struct in `config_schema.rs` declares `#[serde(default)] pub description: Option<String>`.
- **Fix:** Wrapped every description literal in `Some(...)` across all 4 provider impls. Integration tests use `description.as_ref().is_some_and(|d| !d.is_empty())` instead of `!description.is_empty()`. File-provider schema-shape test in `memory_store.rs` doesn't explicitly assert description content (spec didn't require it for the file provider — just the keys + defaults).
- **Files modified:** all 4 provider impls + 3 test files.
- **Verification:** All 12 provider schema tests green.
- **Committed in:** 360c4e9, 50f89fd, plus test commits 509a22c, 14be442.

**3. [Rule 3] Mirror fixture uses `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>` (SharedProvider), not `Box<dyn MemoryProvider>`**
- **Found during:** Task 20-04-03 read-first
- **Issue:** Plan Task 3's `<interfaces>` showed `MemoryManager::new(Box<dyn MemoryProvider>, Option<Box<dyn MemoryProvider>>)` and the sample test body used `parking_lot::Mutex` + `ObservableMirror` adapter to bridge observability. The 20-02 delivered signature is `MemoryManager::new(primary: SharedProvider, mirror: Option<SharedProvider>)` where `SharedProvider = Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`.
- **Fix:** Fixture uses `SharedProvider` directly. No adapter wrapper required — `Arc::new(Mutex::new(MockMirrorProvider))` lets the manager own the provider while the test holds an `Arc<StdMutex<MirrorInner>>` for state introspection (same pattern as existing `MockRecorderProvider` in `manager.rs` tests). No new dependency.
- **Files modified:** crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs
- **Verification:** `cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture` — 4/4 green.
- **Committed in:** cfa1bd9

---

**Total deviations:** 3 scope corrections (all Rule 3 — blocking inconsistencies between plan interface samples and actual 20-01/20-02 artefacts). No auto-fixes to production bugs. No new dependencies introduced.

**Impact on plan:** All deviations were realigning plan text with delivered 20-01/20-02 code. Intent of the plan (name literal + schema overrides, mirror composition fixture) executed exactly. No scope creep.

## Issues Encountered

- None. TDD RED phase produced the expected failures (schema-shape tests panic on empty Vec), GREEN phase made them pass on first compile.
- Pre-existing clippy warnings in `skills.rs`, `context_engine.rs`, `anthropic_client.rs`, `tool_pair.rs`, `summarizing_engine.rs` surfaced via `cargo clippy --tests -- -D warnings`. None in files modified by this plan. Already tracked in `.planning/phases/20-memory-provider-plugin-contract/deferred-items.md` from 20-01.

## Known Stubs

- **DuckDB `threads` field is declarative only.** The schema declares the field with default=1 but the DuckDB provider's `initialize` and construction paths do not currently apply `PRAGMA threads=N` from `provider_config`. The wizard (20-03) can prompt for and persist the value; actual runtime effect is a follow-on (e.g., when the DuckDB bridge gains `initialize`-path provider_config consumption). This is intentional and disclosed in the plan's Task 20-04-02 action step 1. Does NOT block the plan's acceptance: the schema surface is correct and enumeration works.

## Threat Flags

No new surfaces introduced — this plan only adds declarative hook overrides and tests. No new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries.

## User Setup Required

None — no external service configuration required.

## Next Phase Readiness

- **Plan 20-03 (setup wizard)** unblocked: can iterate `[file, sqlite, duckdb, grafeo]` providers, call `get_config_schema()`, and branch on returned fields without provider-specific knowledge. Plan 20-03 was effectively completed in the prior wave; these overrides populate the schemas it enumerates.
- **Plan 20-05+ (any future provider crates)** have a clear onboarding template: implement `MemoryProvider`, override `name()`, override `get_config_schema()`, add `tests/config_schema.rs` mirroring this plan's 3-test pattern.
- **No blockers** for Phase 21+ memory-related work. MEM-08/09/10/11 complete.

---
*Phase: 20-memory-provider-plugin-contract*
*Plan: 04*
*Completed: 2026-04-16*

## Self-Check: PASSED

All 9 claimed files exist on disk:
- crates/ironhermes-core/src/memory_provider.rs — FOUND
- providers/memory-sqlite/src/lib.rs — FOUND
- providers/memory-sqlite/tests/config_schema.rs — FOUND
- providers/memory-duckdb/src/lib.rs — FOUND
- providers/memory-duckdb/tests/config_schema.rs — FOUND
- providers/memory-grafeo/src/lib.rs — FOUND
- providers/memory-grafeo/tests/config_schema.rs — FOUND
- crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs — FOUND
- .planning/phases/20-memory-provider-plugin-contract/20-04-SUMMARY.md — FOUND

All 5 claimed task commits exist in git history:
- 509a22c (test: file+sqlite RED) — FOUND
- 360c4e9 (feat: file+sqlite GREEN) — FOUND
- 14be442 (test: duckdb+grafeo RED) — FOUND
- 50f89fd (feat: duckdb+grafeo GREEN) — FOUND
- cfa1bd9 (test: sqlite mirror fixture) — FOUND

All 16 test assertions green across 5 test surfaces (3 file + 3 sqlite + 3 duckdb + 3 grafeo + 4 mirror fixture).
