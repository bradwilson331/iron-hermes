---
phase: 26
plan: 01
subsystem: ironhermes-core/config
tags: [provider-polish, config-schema, prov-04, prov-06, prov-08, d-01, d-04, d-05, d-06, d-14, d-18]
requires:
  - .planning/phases/26-provider-polish/26-CONTEXT.md (locked decisions D-01..D-21)
  - .planning/phases/26-provider-polish/26-RESEARCH.md
  - .planning/phases/26-provider-polish/26-VALIDATION.md
provides:
  - ProviderConfig.api_key_env: Option<String> (D-01)
  - ProviderConfig.disabled: Option<bool> (D-14)
  - AuxiliaryConfig { provider: String, model: String } (D-05/D-18)
  - Config.auxiliary: AuxiliaryConfig (D-05/D-06)
  - validate_api_key_env(value) (D-04)
  - validate_role_name(name) + RESERVED_ROLE_NAMES (D-05)
  - Config::load_from() custom_providers → providers migration (D-02)
affects:
  - crates/ironhermes-core/src/config.rs
tech-stack:
  added: []
  patterns:
    - Phase 22.4.2.2 plain-String cross-crate convention (D-18)
    - Phase 23/25 #[serde(default)] backward-compat pattern
    - Phase 26 D-02 stderr migration banner at config-parse time
key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/config.rs
decisions:
  - 26-01: api_key field retained as deprecated fallback for one release cycle
    (resolver in Plan 02 will emit one-shot stderr banner per Pitfall 5)
  - 26-01: AuxiliaryConfig uses plain Strings + is_set() helper that returns
    false on empty `provider` — matches D-18 cross-crate convention
  - 26-01: validate_api_key_env hand-rolled (no regex instantiation) to
    keep cold-path fast and consistent with project's slug-validator style
  - 26-01: D-02 migration runs in Config::load_from(), NOT in
    ProviderResolver::build() — keeps resolver focused on resolution and
    avoids re-emitting the warning when a resolver is rebuilt mid-process
metrics:
  duration_minutes: 4
  completed_date: 2026-04-29
  tasks: 2
  files_modified: 1
  files_created: 0
  tests_added: 16
---

# Phase 26 Plan 01: Config Schema Summary

Added the Phase 26 config-schema scaffolding to `ironhermes-core/config.rs`:
new `api_key_env` / `disabled` fields on `ProviderConfig`, a new
`AuxiliaryConfig` block on `Config`, two validators (`validate_api_key_env`,
`validate_role_name`) plus a `RESERVED_ROLE_NAMES` constant, and the D-02
`custom_providers:` → `providers:` migration with stderr banner. All
existing configs parse cleanly via `#[serde(default)]`. Plan 02
(provider.rs resolver changes — D-11 leak fix, D-12 legacy banner, D-07
auxiliary cascade) consumes this surface.

## Plan Objective

Land the schema additions that PROV-04 / PROV-06 / PROV-08 all depend on,
without any behavior change in the resolver yet:

- D-01 / D-14: `ProviderConfig` gains `api_key_env: Option<String>` and
  `disabled: Option<bool>` so future `hermes provider enable|disable`
  writes have a destination.
- D-05 / D-06 / D-18: top-level `auxiliary: AuxiliaryConfig` block with
  plain-String fields per the Phase 22.4.2.2 cross-crate convention. Empty
  `provider` means "unset" so pre-Phase-26 configs default cleanly.
- D-04: `validate_api_key_env([A-Z][A-Z0-9_]*)` — to be called from
  `ProviderResolver::build()` in Plan 02.
- D-05: `validate_role_name` + `RESERVED_ROLE_NAMES` constant covering the
  five Phase 26 roles (`vision`, `compression`, `session_search`,
  `skills_hub`, `mcp_helper`).
- D-02: `Config::load_from()` migrates `custom_providers:` entries that
  are missing from the `providers:` HashMap, with one stderr warning per
  migrated entry. `providers:` takes precedence on collision (silently
  drops the dup `custom_providers:` entry per Pitfall 2).

## Tasks Completed

| # | Task                                                            | Commit  |
|---|-----------------------------------------------------------------|---------|
| 1 | ProviderConfig + AuxiliaryConfig + auxiliary field + D-02 migration + validate_api_key_env (D-01/D-04/D-05/D-06/D-14/D-18/D-02) | 827ad60 |
| 2 | RESERVED_ROLE_NAMES + validate_role_name (D-05)                 | 0947f5f |

## Files Modified

- `crates/ironhermes-core/src/config.rs` — schema additions, validators,
  migration logic, 16 new unit tests.

## Tests Added (16)

### Task 1 — schema, migration, api_key_env validator (12)

- `api_key_env_validation_rejects_invalid` — D-04 (empty, lowercase,
  mixed-case, space, leading-digit, leading-underscore, shell-injection).
- `api_key_env_validation_accepts_valid` — D-04 (`OPENAI_API_KEY`,
  `MY_KEY_123`, single-letter, `ANTHROPIC_API_KEY`, `MY_LLM_KEY`).
- `provider_config_parses_api_key_env` — D-01 round-trip via YAML.
- `provider_config_parses_disabled_field` — D-14 `true|false|absent`.
- `provider_config_backward_compat_without_new_fields` — D-18 default
  parsing for old configs without `api_key_env` / `disabled`.
- `auxiliary_config_default_is_unset` — D-06.
- `auxiliary_config_parses_from_yaml` — D-05.
- `config_without_auxiliary_block_parses_cleanly` — D-06 backward compat.
- `auxiliary_config_serde_roundtrip` — D-05 round-trip.
- `custom_providers_migration_copies_missing_entries_to_providers` — D-02
  (writes config to tempdir, asserts post-load `providers` HashMap entry).
- `custom_providers_migration_does_not_overwrite_existing_providers_entry`
  — D-02 collision rule (Pitfall 2 — `providers:` wins).

### Task 2 — role-name validator (4)

- `reserved_role_names_contains_all_five_d05_roles` — exactly 5 entries.
- `validate_role_name_accepts_all_reserved_roles`.
- `validate_role_name_rejects_unknown_names` — anti-pattern guard.
- `validate_role_name_error_message_lists_allowed_roles` — operator UX.

All 53 `config::` tests pass under `cargo test --test-threads=1`.

## Verification

```bash
$ cargo build -p ironhermes-core
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.07s

$ cargo test -p ironhermes-core --lib -- --test-threads=1 config::tests::
test result: ok. 53 passed; 0 failed; 0 ignored; 0 measured;
            304 filtered out; finished in 0.01s
```

Validator behavior smoke-tested per the plan's success criteria:

- `validate_api_key_env("")` → Err
- `validate_api_key_env("lower_case")` → Err
- `validate_api_key_env("HAS SPACE")` → Err
- `validate_api_key_env("OPENAI_API_KEY")` → Ok
- `validate_api_key_env("MY_KEY_123")` → Ok

All new types use plain `String` / `Option<String>` / `Option<bool>` per
D-18. All schema additions are `#[serde(default)]` so existing configs
parse without changes.

## Deviations from Plan

### Auto-fixed Issues

None — schema-only plan executed exactly as written.

### Out-of-Scope Pre-Existing Issues (deferred)

**1. [Out-of-scope] `provider_resolver_cache_overrides_static_for_same_model`
flakes under parallel test runs.**

- **Found during:** post-Task-1 verification (`cargo test -p ironhermes-core
  --lib provider::`).
- **Issue:** `provider::tests::provider_resolver_cache_overrides_static_for_same_model`
  fails when run in parallel with other tests because it mutates
  `IRONHERMES_HOME` without holding the process-wide ENV_LOCK. New Plan
  26-01 tests in `config::tests::custom_providers_migration_*` use
  tempfile (no env mutation) and don't introduce the race; they merely
  surface a pre-existing flake by running in the same binary.
- **Reason out-of-scope:** The flake lives in `provider.rs` (not modified
  by Plan 01) and predates Phase 26. Plan 02 will revisit `provider.rs`
  test infrastructure (the env_lock pattern from `toolset_integration.rs`
  is the documented fix per RESEARCH.md §"Sampling Strategy for env-var
  Sensitive Tests").
- **Verification:** Test passes in isolation (`--test-threads=1`).
- **Action:** Logged here. Plan 02 will need to add ENV_LOCK to the two
  affected `provider.rs` tests.

## Self-Check: PASSED

- File created/modified: `crates/ironhermes-core/src/config.rs` — FOUND.
- Commit `827ad60`: feat(26-01): add provider/auxiliary config schema for
  Phase 26 — FOUND.
- Commit `0947f5f`: feat(26-01): add validate_role_name +
  RESERVED_ROLE_NAMES (D-05) — FOUND.
- `cargo build -p ironhermes-core` — exits 0.
- 53 config::tests::* all green under `--test-threads=1`.
- All success criteria from the executor prompt met:
  - `validate_api_key_env` rejects "" / "lower_case" / "HAS SPACE" — verified.
  - `validate_api_key_env` accepts "OPENAI_API_KEY" / "MY_KEY_123" — verified.
  - All `#[serde(default)]` so existing configs still parse — verified by
    `provider_config_backward_compat_without_new_fields` and
    `config_without_auxiliary_block_parses_cleanly`.
  - All new types use plain Strings per D-18 — `AuxiliaryConfig`,
    `ProviderConfig.api_key_env: Option<String>` confirmed.
- No modifications to STATE.md or ROADMAP.md.
