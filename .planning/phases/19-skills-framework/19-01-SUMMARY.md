---
phase: 19
plan: 01
subsystem: ironhermes-core/skills
tags: [skills, metadata, rust, serde, tdd]
completed: "2026-04-14"
duration_min: 15

dependency_graph:
  requires: []
  provides:
    - HermesMetadata typed struct with WARN-BUT-LOAD D-18 policy
    - SkillSource enum (Builtin/Official/Community)
    - SkillRecord.hermes_metadata and SkillRecord.source fields
    - extract_hermes_metadata() helper function
    - Wave 0 test scaffolding (4 new tests)
  affects:
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-core/src/lib.rs

tech_stack:
  added:
    - serde::Serialize added to skills.rs imports (HermesMetadata + supporting types now serializable)
    - std::collections::HashMap added to skills.rs imports
  patterns:
    - TDD Red-Green: failing tests committed first (f2fc95b), then implementation (b2306c7)
    - serde(flatten) + serde(default) for WARN-BUT-LOAD unknown field preservation
    - extract_hermes_metadata() free function with match Ok/Err tracing::warn! fallback

key_files:
  modified:
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-core/src/lib.rs

decisions:
  - "Raw metadata: Option<serde_yaml::Value> stays on SkillFrontmatter for backward compat; hermes_metadata is computed on SkillRecord only"
  - "extract_hermes_metadata takes &Option<serde_yaml::Value> (not owned) matching plan spec pattern"
  - "SkillSource defaults to Builtin for all Phase 19 locally-discovered skills per RESEARCH.md A4"

metrics:
  tasks_completed: 2
  files_modified: 2
  test_commits: 1
  impl_commits: 1
---

# Phase 19 Plan 01: Typed HermesMetadata + SkillSource Foundation Summary

**One-liner:** Typed `HermesMetadata` struct with `#[serde(flatten)]` extras bag replaces opaque `serde_yaml::Value` for `metadata.hermes.*`, with `SkillSource` enum added to `SkillRecord` for D-15 scan enforcement provenance.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Wave 0 failing tests (TDD RED) | f2fc95b | crates/ironhermes-core/src/skills.rs |
| 2 | Implement HermesMetadata, SkillSource, typed extraction | b2306c7 | crates/ironhermes-core/src/skills.rs, crates/ironhermes-core/src/lib.rs |

## What Was Built

### New Types (crates/ironhermes-core/src/skills.rs)

- `EnvVarEntry` — required environment variable declaration with `name`, `prompt`, `help`, `required_for`
- `CredentialFileEntry` — untagged enum: `Path(String)` or `Structured { path, description }`
- `SkillConfigField` — config schema entry with `key`, `default`, `description`, `field_type`
- `HermesMetadata` — typed struct for `metadata.hermes.*` with 7 known fields plus `#[serde(flatten)] extras: HashMap<String, serde_yaml::Value>` for unknown fields (D-18)
- `SkillSource` — provenance enum `Builtin | Official | Community`, defaults to `Builtin`

### Modified Types

- `SkillRecord` — added `hermes_metadata: Option<HermesMetadata>` and `source: SkillSource`

### New Functions

- `extract_hermes_metadata(raw: &Option<serde_yaml::Value>) -> Option<HermesMetadata>` — extracts the typed struct from the opaque blob; on serde error, logs WARN and returns `Some(HermesMetadata::default())` (never panics, never rejects)

### Re-exports (crates/ironhermes-core/src/lib.rs)

- `HermesMetadata`, `EnvVarEntry`, `CredentialFileEntry`, `SkillConfigField`, `SkillSource` re-exported from crate root for Plans 02-06 consumption

### Test Scaffolding

Four Wave 0 unit tests added to `#[cfg(test)] mod tests` in skills.rs:

| Test | Validates |
|------|-----------|
| `test_hermes_metadata` | Full typed extraction: env vars, credentials, config, toolset filters |
| `test_warn_but_load_unknown_fields` | D-18: unknown fields land in extras, skill loads cleanly |
| `test_07_2_compat_metadata` | Phase 07.2 `tags`/`related_skills` shape lands in extras without error |
| `test_no_metadata_at_all` | Skills with no metadata block load; `hermes_metadata` is `None` |

## Verification

- `cargo test -p ironhermes-core skills` — 53 passed, 0 failed
- `cargo build -p ironhermes-core` — compiles clean (only pre-existing deprecation warning on `build_memory_provider`)
- All 4 new Wave 0 tests green
- `test_platform_filter` (6 variants) still passes — no regression

## Deviations from Plan

None — plan executed exactly as written.

The plan specified `extract_hermes_metadata` taking `raw: &Option<serde_yaml::Value>` (by reference) which aligns with the usage in `load_with_paths` where `metadata` is moved into `SkillRecord` and the extraction runs before the move.

## Known Stubs

None. All fields are fully typed and populated. `SkillSource::Builtin` is the correct Phase 19 default per RESEARCH.md A4 (not a placeholder — Phase 19.1 flips hub-installed skills to `Community` at install time).

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries introduced. The `extract_hermes_metadata` function only reads from already-parsed YAML (trusted parse context); the WARN-BUT-LOAD policy with `tracing::warn!` on error addresses T-19-01-parse-panic and T-19-01-parse-reject from the plan's threat register.

## Self-Check

- [x] `crates/ironhermes-core/src/skills.rs` — exists and contains `struct HermesMetadata`
- [x] `crates/ironhermes-core/src/lib.rs` — exists and exports `HermesMetadata`
- [x] Commit `f2fc95b` — test(19-01) RED tests
- [x] Commit `b2306c7` — feat(19-01) GREEN implementation
- [x] 53 skills tests passing

## Self-Check: PASSED
