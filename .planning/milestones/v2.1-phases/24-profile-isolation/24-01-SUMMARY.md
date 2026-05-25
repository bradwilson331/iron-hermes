---
phase: 24
plan: 01
subsystem: ironhermes-core
tags: [phase-24, profile-isolation, validation, ironhermes-core, security]
dependency_graph:
  requires: []
  provides:
    - ironhermes_core::profile::validate_profile_name
    - ironhermes_core::profile::ProfileNameError
    - ironhermes_core::constants::PROFILES_SUBDIR
  affects:
    - Plans 02-07 (all downstream profile consumers)
tech_stack:
  added: []
  patterns:
    - hand-rolled slug validator (no regex dep, pure char iteration)
    - plain-String cross-crate return type (D-17)
    - in-module #[cfg(test)] unit tests
key_files:
  created:
    - crates/ironhermes-core/src/profile.rs
  modified:
    - crates/ironhermes-core/src/constants.rs
    - crates/ironhermes-core/src/lib.rs
decisions:
  - "Used pure char iteration (not regex) for slug validation — no new deps, follows Phase 21.8 sanitize.rs precedent"
  - "Reserved names list is exactly [default, current, none] per D-03"
  - "validate_profile_name returns Result<String, ProfileNameError> (D-17 plain-String convention)"
  - "pub mod profile; inserted alphabetically between models_cache and provider in lib.rs"
  - "PROFILES_SUBDIR inserted after MEMORIES_DIR block, before get_hermes_home() — D-01 constraint honored"
metrics:
  duration_seconds: 247
  completed_date: "2026-04-29"
  tasks_completed: 2
  files_created: 1
  files_modified: 2
---

# Phase 24 Plan 01: Profile Validator + PROFILES_SUBDIR Constant Summary

**One-liner:** `validate_profile_name()` slug validator with `ProfileNameError` enum plus `PROFILES_SUBDIR = "profiles"` constant — foundational gate defending all downstream path construction against T-24-01 path traversal.

## What Was Built

### `ironhermes_core::profile::validate_profile_name`

Callable as `ironhermes_core::profile::validate_profile_name(&str) -> Result<String, ProfileNameError>`.

No `pub use` re-export was added to `lib.rs` for `validate_profile_name` — downstream consumers (Plan 03's CLI pivot) call it via the full path `ironhermes_core::profile::validate_profile_name`. The `pub mod profile;` declaration in `lib.rs` makes the module publicly accessible. If a `pub use` shorthand is needed by Plan 03, it can be added to `lib.rs`'s existing `pub use` block at that time.

### `ironhermes_core::constants::PROFILES_SUBDIR`

Re-exported via `pub use constants::*` (already in `lib.rs` line 25), so callers can use `ironhermes_core::PROFILES_SUBDIR` directly without qualification.

### `ProfileNameError` variants

| Variant | Trigger |
|---------|---------|
| `Empty` | Empty string input |
| `LeadingUnderscore` | Name begins with `_` |
| `Reserved(String)` | Name is `default`, `current`, or `none` |
| `InvalidChars` | Char outside `[a-z0-9-]` or starts with `-` |
| `TooLong` | Length > 64 |

Both `fmt::Display` and `std::error::Error` are implemented.

## Test Results

`cargo test -p ironhermes-core -- profile::tests`: **14 passed, 0 failed**

| Test | Behavior | Result |
|------|----------|--------|
| `accepts_simple_slug` | "work", "client-acme", "a1b2" → Ok | PASS |
| `rejects_default_token` | "default" → Reserved("default") | PASS |
| `rejects_current_token` | "current" → Reserved | PASS |
| `rejects_none_token` | "none" → Reserved | PASS |
| `rejects_leading_underscore` | "_priv" → LeadingUnderscore | PASS |
| `rejects_empty` | "" → Empty | PASS |
| `rejects_path_traversal_slash` | "foo/bar" → InvalidChars (T-24-01) | PASS |
| `rejects_path_traversal_dotdot` | "../etc" → InvalidChars (T-24-01) | PASS |
| `rejects_uppercase` | "Work" → InvalidChars | PASS |
| `rejects_space` | "foo bar" → InvalidChars | PASS |
| `rejects_leading_dash` | "-leading" → InvalidChars | PASS |
| `rejects_too_long` | 65-char name → TooLong | PASS |
| `accepts_64_char_boundary` | 64-char name → Ok, len=64 | PASS |
| `returns_owned_string_for_d17` | Return type is `String` not `&str` | PASS |

## Verification of D-01 Constraint

`get_hermes_home()` and `display_hermes_home()` are byte-for-byte unchanged. Only the `PROFILES_SUBDIR` constant was added between the memory constants block and the function declaration:

```
git diff crates/ironhermes-core/src/constants.rs shows:
+/// Profile isolation constants (D-04, Phase 24)
+pub const PROFILES_SUBDIR: &str = "profiles";
```

No edits to lines containing `get_hermes_home` or `display_hermes_home`.

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None — profile.rs stub from Task 1 was fully replaced by Task 2 implementation. All exports are functional.

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes introduced. The `validate_profile_name` function is a pure in-memory validator — no filesystem access. T-24-01 and T-24-RES mitigations are implemented and locked by tests.

## Commits

| Task | Commit | Description |
|------|--------|-------------|
| Task 1 | `d0c0924` | feat(24-01): add PROFILES_SUBDIR constant + profile module scaffold |
| Task 2 | `af6051a` | feat(24-01): implement validate_profile_name + ProfileNameError + unit tests |

## Self-Check

See below.
