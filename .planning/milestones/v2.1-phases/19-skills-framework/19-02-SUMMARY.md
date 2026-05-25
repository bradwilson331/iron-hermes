---
phase: 19
plan: 02
subsystem: ironhermes-core/skills + ironhermes-agent/prompt_builder
tags: [skills, prompt-builder, filtering, catalog, d-01, d-03, tdd]
completed: "2026-04-14"
duration_min: 20

dependency_graph:
  requires:
    - "19-01 (typed HermesMetadata + SkillSource on SkillRecord)"
  provides:
    - SkillRegistry::filtered_catalog_text(&active_toolsets, &active_tools)
    - skill_passes_filter free helper (module-private, pure)
    - PromptBuilder::set_active_toolsets / set_active_tools setters
    - PromptBuilder active_toolsets / active_tools snapshot fields (empty by default)
    - 6 Wave 0 filter tests + 1 prompt_builder integration test
  affects:
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-agent/src/agent_loop.rs (test helper fix — Rule 3 auto-fix)

tech_stack:
  added: []
  patterns:
    - TDD Red-Green: failing tests added first, then implementation in one commit due to bash commit permissions
    - Pure in-memory filter (no env/fs access) honoring D-06
    - AND semantics on requires_* (all must match), OR semantics on fallback_for_* (any match hides)
    - Phase 19 stub: empty HashSet snapshots per RESEARCH.md Open Question #1 (Phase 20 wires real state)

key_files:
  modified:
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-agent/src/prompt_builder.rs
    - crates/ironhermes-agent/src/agent_loop.rs

decisions:
  - "filtered_catalog_text() returns empty string when all skills filtered out; both call sites guard with trim().is_empty() to avoid rendering an empty Skills header"
  - "active_toolsets / active_tools default to empty HashSet — per RESEARCH.md Open Question #1, real values arrive in Phase 20"
  - "skill_passes_filter is module-private (not pub) — only the registry method consumes it"
  - "Rule 3 auto-fix: agent_loop.rs test helper make_skill_record was still using the pre-Plan-01 SkillRecord struct literal (missing hermes_metadata + source fields); fixed inline to unblock ironhermes-agent test compilation"

metrics:
  tasks_completed: 2
  files_modified: 3
  test_commits: 0
  impl_commits: 1
---

# Phase 19 Plan 02: Catalog-Render Filter Summary

**One-liner:** D-01/D-03 catalog-render filter on `SkillRegistry` that hides skills whose `requires_toolsets`/`requires_tools` are unmet or whose `fallback_for_*` is shadowed by an active toolset/tool, wired into both prompt_builder slot-4 call sites with empty-HashSet stubs until Phase 20.

## Tasks Completed

| Task | Name                                                                 | Commit  | Files                                                                                                                         |
| ---- | -------------------------------------------------------------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------- |
| 1    | Filter logic + 6 Wave 0 tests on SkillRegistry                       | pending | crates/ironhermes-core/src/skills.rs                                                                                          |
| 2    | Wire filter into prompt_builder (both call sites) + integration test | pending | crates/ironhermes-agent/src/prompt_builder.rs, crates/ironhermes-agent/src/agent_loop.rs (Rule 3 auto-fix test helper update) |

**Commit status note:** During execution the environment denied `git commit` permissions for the executor bash tool. Source/test changes are complete and verified via `cargo test`; commits are pending operator approval and will be performed either by the orchestrator or on next executor run with commit permission restored.

## What Was Built

### `crates/ironhermes-core/src/skills.rs`

- **`SkillRegistry::filtered_catalog_text(&active_toolsets, &active_tools) -> String`**
  Renders the same `- name: description` lines as `catalog_text()` but only for skills passing `skill_passes_filter`.
- **`fn skill_passes_filter(record, active_toolsets, active_tools) -> bool`** (module-private)
  - Skill with `hermes_metadata: None` → always pass.
  - `requires_toolsets` non-empty → ALL listed toolsets must be in `active_toolsets`, else hide.
  - `requires_tools` non-empty → ALL listed tools must be in `active_tools`, else hide.
  - Any element of `fallback_for_toolsets` in `active_toolsets` → hide.
  - Any element of `fallback_for_tools` in `active_tools` → hide.
  - Pure: accepts only `&SkillRecord` + `&HashSet<String>`; no env/fs access (D-06).

### New tests (`skills.rs` test module)

| Test                                 | Validates                                                                |
| ------------------------------------ | ------------------------------------------------------------------------ |
| `test_filter_requires_toolsets`      | alpha (requires web) hidden without web, shown with web; beta always in  |
| `test_filter_requires_tools`         | gamma (requires fetch_url AND parse_html) hidden on 1/2, shown on 2/2    |
| `test_filter_fallback_for_toolsets`  | fallback-web hidden when playwright active, shown otherwise              |
| `test_filter_fallback_for_tools`     | fallback-tool hidden when playwright_nav active, shown otherwise         |
| `test_filter_no_metadata_always_shown` | bare skill (no hermes metadata) passes for any active set              |
| `test_filter_pure_no_io`             | sentinel env var absent before and after — filter does not touch env    |

All tests construct `SkillRegistry` via `load_with_paths` against tempdir fixtures, so hermes_metadata flows through the real parser path (matching Plan 01 pattern).

### `crates/ironhermes-agent/src/prompt_builder.rs`

- **Imports:** added `HashSet` to the existing `std::collections::BTreeMap` use.
- **`PromptBuilder` struct fields (new):**
  - `active_toolsets: HashSet<String>` — snapshot captured at session-freeze.
  - `active_tools: HashSet<String>` — snapshot captured at session-freeze.
- **`PromptBuilder::new` initialization:** both default to `HashSet::new()`.
- **Setters (public):** `set_active_toolsets(HashSet<String>)`, `set_active_tools(HashSet<String>)`.
- **`load_skills()` (around line 380):** replaced `registry.catalog_text()` with `registry.filtered_catalog_text(&self.active_toolsets, &self.active_tools)`. When the filter produces an empty string (all skills hidden) the slot is omitted — no empty `## Available Skills` block.
- **`build_split()` Skills fallback (around line 440):** same replacement with the same empty-guard.

### New prompt_builder test

- `test_prompt_builder_skills_slot_filter_applies`: registers two skills — one with `requires_toolsets: [nonexistent]`, one with no hermes metadata — calls `set_active_toolsets(HashSet::new())`, builds the prompt, and asserts the filtered skill name is absent while the always-shown skill is present, plus the `## Available Skills` header exists.

### `crates/ironhermes-agent/src/agent_loop.rs` (Rule 3 auto-fix)

`make_skill_record` test helper was constructing `SkillRecord { ... }` directly without the `hermes_metadata` / `source` fields added by Plan 01. This was pre-existing breakage blocking `cargo test -p ironhermes-agent` compilation. Added:

```rust
hermes_metadata: None,
source: ironhermes_core::SkillSource::Builtin,
```

No behavior change for the skill enforcement tests — they pass `None` tool filters and rely only on `allowed_tools`.

## Verification

- `cargo test -p ironhermes-core test_filter_` — 6 passed, 0 failed.
- `cargo test -p ironhermes-core skills` — 59 passed, 0 failed (53 from Plan 01 + 6 new filter tests).
- `cargo test -p ironhermes-agent prompt_builder` — 27 passed, 0 failed (26 previous + new `test_prompt_builder_skills_slot_filter_applies`).
- `cargo test -p ironhermes-agent --lib` — 186 passed, 0 failed.
- `cargo build --workspace` — compiles clean (only pre-existing warnings: deprecated `build_memory_provider`, `estimate_message_tokens` unused import, `reject_file_path` unused function).
- `grep -n 'registry.catalog_text()'` in prompt_builder.rs returns zero matches.
- `grep -n 'filtered_catalog_text'` in prompt_builder.rs returns two matches (load_skills + build_split).
- D-06 purity enforced: `skill_passes_filter` body contains no `std::env::`, no `fs::`, no `Path::`; the `test_filter_pure_no_io` test asserts env is not mutated.

## Deviations from Plan

### Rule 3 auto-fix: agent_loop.rs test helper

- **Found during:** Task 2 (`cargo test -p ironhermes-agent prompt_builder`).
- **Issue:** Compile error `missing fields hermes_metadata and source in initializer of SkillRecord` at `crates/ironhermes-agent/src/agent_loop.rs:1121`.
- **Root cause:** Plan 01 added `hermes_metadata: Option<HermesMetadata>` and `source: SkillSource` to `SkillRecord` but did not update this test helper (it was out of the Plan 01 scope because Plan 01 touched only `skills.rs` + `lib.rs`).
- **Fix:** Added `hermes_metadata: None, source: ironhermes_core::SkillSource::Builtin` to the struct literal. No behavior change to skill enforcement tests.
- **Files modified:** crates/ironhermes-agent/src/agent_loop.rs (1 helper function).

### TDD RED/GREEN merged into one commit (tooling constraint)

- **Found during:** Task 1 commit attempt.
- **Issue:** The executor bash shell denied `git commit` invocations in this session.
- **Impact:** The RED state was locally verified (`cargo test -p ironhermes-core test_filter_` produced `no method named 'filtered_catalog_text'` compile errors) but no separate RED commit was landed. GREEN implementation was verified with `cargo test` showing all 6 filter tests pass.
- **Mitigation:** The RED verification output is documented in this SUMMARY (see Verification §). Operator is asked to create the commits below from the staged working tree.

## Pending Commits (to be applied by operator)

```
feat(19-02): implement filtered_catalog_text + skill_passes_filter
  - crates/ironhermes-core/src/skills.rs
    (both the implementation and the 6 new tests)

feat(19-02): wire filtered_catalog_text into prompt_builder (slot 4)
  - crates/ironhermes-agent/src/prompt_builder.rs
  - crates/ironhermes-agent/src/agent_loop.rs  (Rule 3 auto-fix: SkillRecord struct literal)
```

## Threat Surface Scan

No new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries. The filter is a pure in-memory function over `SkillRecord` fields already validated by Plan 01's parsing pipeline. Threat `T-19-02-filter-bypass` is mitigated by the AND semantics in `iter().all(...)` (test_filter_requires_tools covers the 1-of-2 case). Threat `T-19-02-filter-side-effect` is mitigated by the immutable-only signature and the `test_filter_pure_no_io` env assertion.

## Known Stubs

- `active_toolsets` / `active_tools` on PromptBuilder default to `HashSet::new()` and are populated only via explicit setter calls. Phase 19 callers do not invoke the setters; Phase 20 will wire real toolset/tool state at session-freeze. This is intentional (RESEARCH.md Open Question #1) — with empty snapshots, skills with `requires_*` non-empty will not render; skills with no hermes metadata or only `fallback_for_*` still render. The filter infrastructure is complete and Phase 20 only needs to call the setters.

## Self-Check

- [x] `crates/ironhermes-core/src/skills.rs` — contains `pub fn filtered_catalog_text(` and `fn skill_passes_filter(`
- [x] 6 `fn test_filter_*` functions present in skills.rs test module
- [x] `crates/ironhermes-agent/src/prompt_builder.rs` — no `registry.catalog_text()` calls remain
- [x] `crates/ironhermes-agent/src/prompt_builder.rs` — 2 `filtered_catalog_text` call sites
- [x] PromptBuilder struct has `active_toolsets:` and `active_tools:` fields
- [x] `fn set_active_toolsets` and `fn set_active_tools` setters defined
- [x] All 6 filter unit tests pass (`cargo test -p ironhermes-core test_filter_`)
- [x] `test_prompt_builder_skills_slot_filter_applies` passes
- [x] 186 ironhermes-agent lib tests pass
- [x] 59 ironhermes-core skills tests pass (Plan 01 regression clear)
- [x] `cargo build --workspace` succeeds
- [ ] Atomic commits landed — deferred to operator per "Pending Commits" section above

## Self-Check: PASSED (code+tests); COMMITS PENDING
