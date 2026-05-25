---
phase: 26
plan: 02
subsystem: ironhermes-core/provider
tags: [provider-polish, prov-04, prov-06, prov-08, d-11, d-12, d-13, d-14, d-05, d-07, d-10, d-21]
requires:
  - .planning/phases/26-provider-polish/26-01-SUMMARY.md (ProviderConfig.api_key_env, AuxiliaryConfig, validate_api_key_env, validate_role_name)
  - .planning/phases/26-provider-polish/26-CONTEXT.md (locked decisions D-01..D-21)
  - .planning/phases/26-provider-polish/26-RESEARCH.md
provides:
  - PROV-04 leak removed: custom providers with no api_key_env get api_key=None (D-11)
  - D-12: legacy OPENAI/ANTHROPIC/OPENROUTER_API_KEY env vars accepted for built-ins with once-only OnceLock deprecation banner
  - D-13: config.model.api_key deprecated with once-only stderr banner
  - D-14: providers.NAME.disabled=true excludes provider from resolver; disabled main provider errors at build
  - D-05/D-07: resolve_role() 3-level cascade (per-task override -> auxiliary -> None)
  - D-10: auxiliary.provider validated against known names at build time
  - env_lock applied to pre-existing parallelism-flaky disk-cache tests (Plan 01 SUMMARY deferred item)
affects:
  - crates/ironhermes-core/src/provider.rs
tech-stack:
  added: []
  patterns:
    - OnceLock<Mutex<HashSet<String>>> once-only banner emission (D-12/D-13)
    - env_lock() process-wide Mutex for env-var-sensitive tests (RESEARCH.md §Sampling Strategy)
    - Phase 26 D-11 _ => None replacement for wildcard OPENAI_API_KEY fallback
key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/provider.rs
decisions:
  - 26-02: OnceLock<Mutex<HashSet<String>>> keyed by string banner_key (provider name for D-12,
    "__model_api_key__" for D-13, "__config_api_key__{name}" for deprecated api_key literal) — allows
    fine-grained once-only tracking per warning type while sharing one static
  - 26-02: auxiliary_endpoint stored as Option<ResolvedEndpoint> on ProviderResolver struct —
    computed once at build() time and cloned on each resolve_role() call (D-07 by-value return)
  - 26-02: D-14 disabled gate removes pre-populated built-in entry before overlay loop so that
    disabling "openai" removes the built-in AND skips the user overlay (single code path)
  - 26-02: D-13 api_key_env priority is FIRST (before deprecated api_key literal and legacy env vars)
    so new configs with api_key_env work correctly even if old api_key field still present
  - 26-02: memory-sqlite pre-existing compile error (missing cache_breaking field) is out-of-scope —
    does not affect ironhermes-core, ironhermes-agent, or ironhermes-cli crates
metrics:
  duration_minutes: 7
  completed_date: 2026-04-30
  tasks: 1
  files_modified: 1
  files_created: 0
  tests_added: 11
---

# Phase 26 Plan 02: ProviderResolver Resolver Changes Summary

Rewrote the API key resolution loop in `ProviderResolver::build()` to implement
PROV-04 key scoping (D-11), legacy deprecation banners (D-12/D-13), disabled-
provider gate (D-14), and auxiliary endpoint validation (D-10). Extended
`resolve_role()` to the full D-05 three-level cascade. Fixed the pre-existing
`IRONHERMES_HOME` env-var parallelism flake from Plan 01's deferred-items list.

## Plan Objective

Land the `provider.rs` resolver behavioral changes that depend on the Plan 01
config schema additions:

- **D-11 / PROV-04:** Delete `_ => std::env::var("OPENAI_API_KEY").ok()` wildcard
  arm. Custom providers with no `api_key_env` now get `api_key: None`.
- **D-12:** Legacy `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `OPENROUTER_API_KEY`
  env vars retained as deprecated fallback for matching built-ins with a once-only
  `OnceLock<Mutex<HashSet<String>>>` deprecation banner emitted from `build()` only.
- **D-13:** `config.model.api_key` literal deprecated with once-only stderr banner.
- **D-14:** `providers.NAME.disabled: true` skips endpoint entry creation;
  disabling the main provider fails at build with a clear actionable error.
- **D-10:** `auxiliary.provider` validated against known endpoint names at build;
  unknown names fail with a clear error message.
- **D-05/D-07:** `resolve_role()` extended to 3-level cascade: per-task override
  → auxiliary block → `None` (caller falls through to main).
- **Flake fix:** Added `env_lock()` guard to `provider_resolver_loads_disk_cache_at_build`
  and `provider_resolver_cache_overrides_static_for_same_model` (deferred from Plan 01).

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | PROV-04 leak fix + D-12/D-13 banners + D-14 gate + D-07 cascade + D-10 auxiliary validation + env_lock fix + all unit tests | 04a2b68 | crates/ironhermes-core/src/provider.rs |

## Files Modified

- `crates/ironhermes-core/src/provider.rs` — added `OnceLock`/`Mutex`/`HashSet`
  imports; added `emit_deprecation_once()` helper; added `auxiliary_endpoint:
  Option<ResolvedEndpoint>` field to `ProviderResolver`; rewrote Step 4 (API key
  resolution) with 4-priority chain; added Steps 2b (D-14 main disabled check) and
  8 (auxiliary endpoint build); rewrote `resolve_role()` for 3-level cascade; added
  `env_lock()` to tests module; added 11 new tests.

## Tests Added (11)

### D-21 / PROV-04 key leak prevention

- `legacy_openai_key_does_not_leak_to_unknown_provider` — D-21 required test:
  sets `OPENAI_API_KEY=sk-leaked`, builds resolver with `my-local-llm` having
  `api_key_env: None`, asserts `api_key == None`. Holds `env_lock`.
- `custom_provider_api_key_env_resolves_own_var` — api_key_env on custom provider
  resolves `MY_LLM_KEY`, not `OPENAI_API_KEY`. Holds `env_lock`.

### D-14 disabled gate

- `disabled_provider_excluded_from_resolver` — built-in `openai` disabled; asserts
  `resolver.resolve("openai") == None`; other providers still present.
- `disabled_main_provider_errors_at_build` — disabling the main provider returns
  `Err` with "disabled" in message.

### D-05/D-07 resolve_role cascade

- `resolve_role_per_task_override_wins` — per-task `vision→openai` wins over
  `auxiliary→openrouter`. (D-05 cascade level 1)
- `resolve_role_falls_through_to_auxiliary` — no per-task `compression`; falls
  through to `auxiliary→openai/gpt-4o-mini`. (D-05 cascade level 2)
- `resolve_role_returns_none_when_no_role_set` — no per-task, no auxiliary;
  returns `None`. (D-05 cascade level 3)

### D-10 auxiliary.provider validation

- `auxiliary_provider_unknown_name_fails_build` — `auxiliary.provider: nonexistent`
  returns `Err` identifying the unknown name.
- `auxiliary_provider_known_name_builds_successfully` — known auxiliary provider
  builds and is returned for unconfigured roles.

### Pre-existing flake fix (Plan 01 deferred)

- `provider_resolver_loads_disk_cache_at_build` — added `env_lock()` guard.
- `provider_resolver_cache_overrides_static_for_same_model` — added `env_lock()` guard.

All 42 `provider::tests::*` pass under `--test-threads=1`.
All 62 `provider::*` + `config::*` tests pass (104 total with skills::).

## Verification

```
$ cargo build -p ironhermes-core
    Finished `dev` profile in 0.65s

$ cargo test -p ironhermes-core --lib -- provider config --test-threads=1
test result: ok. 104 passed; 0 failed; 0 ignored

$ ! grep -E '_ => std::env::var\("OPENAI_API_KEY"\)' \
    crates/ironhermes-core/src/provider.rs
(no output — leak removed)

$ cargo build -p ironhermes-core -p ironhermes-agent -p ironhermes-cli
    Finished `dev` profile in 11.65s
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Applied env_lock to pre-existing disk-cache parallelism flake**
- **Found during:** Post-Task-1 regression check (Plan 01 SUMMARY deferred item)
- **Issue:** `provider_resolver_loads_disk_cache_at_build` and
  `provider_resolver_cache_overrides_static_for_same_model` mutated `IRONHERMES_HOME`
  without holding the process-wide `env_lock()`, causing race conditions in parallel
  test runs. Documented as deferred in Plan 01 SUMMARY.
- **Fix:** Added `let _g = env_lock().lock()...` at the top of both tests.
- **Files modified:** `crates/ironhermes-core/src/provider.rs`
- **Commit:** 04a2b68

### Out-of-Scope Pre-Existing Issues (deferred)

**1. [Out-of-scope] `memory-sqlite` crate compile error**
- **Issue:** `providers/memory-sqlite/src/lib.rs:191` — missing `cache_breaking`
  field in `ConfigField` struct initializer. Fails `cargo build --workspace`.
- **Verified pre-existing:** Failure reproduces on base commit `235d069` before
  any Plan 02 changes.
- **Impact:** Does not affect `ironhermes-core`, `ironhermes-agent`, or
  `ironhermes-cli`. Plan 02 crate scope builds clean.
- **Action:** Logged to deferred-items. Out of Plan 02 scope.

**2. [Out-of-scope] `commands::handlers::tests::dispatch_all_todo_stubs_return_not_yet_available` failure**
- **Issue:** Pre-existing test failure in `ironhermes-core` commands handlers (cron
  store not configured returns different message than stub expects).
- **Verified pre-existing:** Fails on base commit `235d069`.
- **Action:** Logged to deferred-items. Out of Plan 02 scope.

### D-12 Banner Once-Only Test: subprocess approach deferred

Per RESEARCH.md §"Open Questions", testing the once-only property of the
`OnceLock<Mutex<HashSet<String>>>` banner guard requires process isolation because
`OnceLock` cannot be reset between unit tests in the same binary. The subprocess-
based test (`D-12 legacy_env_banner_once_only`) is deferred to Plan 05
(integration tests) which already uses subprocess invocations via
`CARGO_BIN_EXE_ironhermes`.

## Known Stubs

None — all resolver logic is fully wired. The auxiliary cascade is live and tested.
The deprecation banners emit to stderr and are observable via subprocess tests
(deferred to Plan 05).

## Threat Flags

| Flag | File | Description |
|------|------|-------------|
| threat_flag: information_disclosure | crates/ironhermes-core/src/provider.rs | New `emit_deprecation_once()` prints provider names and env var names to stderr — no key values printed (D-15 compliant); provider names are operator-supplied config, not secrets |

## Self-Check: PASSED

- `crates/ironhermes-core/src/provider.rs` — FOUND (modified).
- Commit `04a2b68` — FOUND (`git log --oneline -1` confirms).
- `cargo build -p ironhermes-core` — exits 0.
- `legacy_openai_key_does_not_leak_to_unknown_provider` — PASSES.
- `resolve_role_per_task_override_wins`, `resolve_role_falls_through_to_auxiliary`,
  `resolve_role_returns_none_when_no_role_set` — all PASS.
- PROV-04 leak grep: `! grep -E '_ => std::env::var\("OPENAI_API_KEY"\)'` — no matches.
- `disabled_provider_excluded_from_resolver`, `disabled_main_provider_errors_at_build` — PASS.
- `auxiliary_provider_unknown_name_fails_build` — PASS.
- No modifications to STATE.md or ROADMAP.md.
