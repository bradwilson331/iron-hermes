---
phase: 19-skills-framework
plan: 04
subsystem: skills
tags: [skills, config, body-injection, serde, yaml]

requires:
  - phase: 19-skills-framework
    provides: "HermesMetadata.config typed Vec<SkillConfigField> (Plan 01); SkillsTool::new 4-arg constructor taking skills_config HashMap (Plan 03)"
provides:
  - "SkillsConfig.config: HashMap<String, HashMap<String, serde_yaml::Value>> with #[serde(default)] — round-trips config.yaml skills.config.<name>.<key> = <value>"
  - "SkillRegistry::declared_config_schema(skill_name) -> Option<&[SkillConfigField]> — exposes metadata.hermes.config for Phase 23 hermes config migrate CLI"
  - "build_skill_config_header helper (skills_tool.rs) — synthesizes deterministic [Skill config: k1 = v1, k2 = v2] header with lex-sorted keys"
  - "handle_activate success-path body-injection — prepends '[Skill config: ...]\\n\\n' when skills_config has entry for skill; returns body unchanged otherwise"
affects: [phase-23-cli, phase-23-hermes-config-migrate, future-skills-using-config]

tech-stack:
  added: []
  patterns:
    - "Per-skill config values stored as HashMap<String, HashMap<String, serde_yaml::Value>> preserve any YAML scalar or nested shape without schema churn"
    - "Deterministic prompt output via lex-sorted keys in HashMap iteration — preserves prompt-cache safety (threat T-19-04-nondeterministic-output)"
    - "Body-injection as skill-content channel piggyback — zero new tool surface, zero extra LLM round-trip (D-08)"

key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/config.rs
    - crates/ironhermes-core/src/skills.rs
    - crates/ironhermes-tools/src/skills_tool.rs

key-decisions:
  - "declared_config_schema returns None (not Some(&[])) when hermes metadata has an empty config slice — 'no schema declared' is the canonical meaning; consistent with not-found and no-hermes-meta cases"
  - "Config header keys sorted lexicographically in build_skill_config_header — deterministic output across HashMap iteration orderings, preserves prompt caching"
  - "format_yaml_value_inline renders scalars unquoted for readability; complex YAML falls back to trimmed serde_yaml::to_string (never panics)"
  - "handle_activate signature extended to accept &HashMap<String, HashMap<String, serde_yaml::Value>> — keeps the header logic pure/testable and avoids threading the whole SkillsTool through the helper"

patterns-established:
  - "Config-driven skill customization without new tools: user edits config.yaml → SkillsConfig deserializes → header synthesized on activate success"
  - "Typed schema extraction via a dedicated registry accessor (declared_config_schema) — decouples Phase 19 schema publishing from Phase 23 CLI consumption"

requirements-completed: [SKILL-05]

duration: ~3 min
completed: 2026-04-15
---

# Phase 19 Plan 04: SkillsConfig.config + [Skill config] Body Injection Summary

**Per-skill YAML config (`skills.config.<name>.<key>`) now round-trips into `SkillsConfig.config` and is injected as a deterministic `[Skill config: k1 = v1, k2 = v2]\n\n` header on activate success; `SkillRegistry::declared_config_schema` exposes the frontmatter-declared schema for Phase 23's `hermes config migrate`.**

## Performance

- **Duration:** ~3 min (191 s)
- **Started:** 2026-04-15T02:01:04Z
- **Completed:** 2026-04-15T02:04:15Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- `SkillsConfig.config: HashMap<String, HashMap<String, serde_yaml::Value>>` with `#[serde(default)]` — backward compatible, full YAML round-trip verified.
- `SkillRegistry::declared_config_schema(skill_name) -> Option<&[SkillConfigField]>` — case-insensitive lookup, returns `None` when skill absent, has no hermes metadata, or declares empty config.
- Free-function `build_skill_config_header` in `skills_tool.rs` — lex-sorts keys, formats YAML scalars inline, returns `None` when entry absent or empty (no empty header emitted).
- `handle_activate` success branch prepends `[Skill config: ...]\n\n` before the body when config exists; body returned unchanged otherwise. Threat `T-19-04-nondeterministic-output` mitigated via explicit lex sort.

## Task Commits

1. **Task 1: SkillsConfig.config round-trip + declared_config_schema helper** — `ec750a6` (feat)
2. **Task 2: Body-injection [Skill config: ...] header on activate (D-08)** — `4b18d48` (feat)

_Both tasks committed atomically with per-task tests green before commit._

## Files Created/Modified
- `crates/ironhermes-core/src/config.rs` — added `SkillsConfig.config` field with serde default, updated `Default` impl, added 2 round-trip tests.
- `crates/ironhermes-core/src/skills.rs` — added `SkillRegistry::declared_config_schema`, added 3 schema tests.
- `crates/ironhermes-tools/src/skills_tool.rs` — added `build_skill_config_header` + `format_yaml_value_inline`, wired into `handle_activate` success branch (now takes `&skills_config`), added 3 injection tests.

## Decisions Made

- **`declared_config_schema` returns `None` for empty config slice** (not `Some(&[])`). The three "no schema" cases — unknown skill, no hermes metadata, empty config — collapse into a single `None` sentinel; Phase 23 consumers can match once.
- **Lex-sort keys in the config header.** HashMap iteration order is non-deterministic; a stable sort makes two back-to-back activations produce byte-identical output, which is critical for downstream prompt-cache key stability (threat T-19-04-nondeterministic-output).
- **`format_yaml_value_inline` renders strings unquoted.** The header is human-readable narrative ("path = ~/research"), not a round-trippable YAML fragment. Complex/nested values fall back to `serde_yaml::to_string` trimmed so the function is total.
- **Extend `handle_activate` signature rather than promote it to a method.** Keeps the activate handlers as free functions (matching the existing pattern for list/view/deactivate) and avoids widening the blast radius to test scaffolding.

## Deviations from Plan

None - plan executed exactly as written.

## Deferred Issues

- `delegate_task::tests::test_delegate_task_schema_has_required_task` fails on pre-existing code in `crates/ironhermes-tools/src/delegate_task.rs:757` — unrelated to Plan 04 changes. Verified by stashing Plan 04 modifications and reproducing the failure against the prior commit (`ec750a6`). Out of scope per execute-plan.md scope-boundary rule. Logged here for phase-level tracking; should be filed against its originating phase.

## Issues Encountered

None.

## Verification Evidence

```
$ cargo test -p ironhermes-core test_skills_config_
test result: ok. 3 passed; 0 failed

$ cargo test -p ironhermes-core test_declared_config_schema_
test result: ok. 3 passed; 0 failed

$ cargo test -p ironhermes-core config
test result: ok. 30 passed; 0 failed

$ cargo test -p ironhermes-core skills
test result: ok. 64 passed; 0 failed

$ cargo test -p ironhermes-tools test_activate_config_injection
test result: ok. 1 passed; 0 failed

$ cargo test -p ironhermes-tools test_activate_no_config_no_header
test result: ok. 1 passed; 0 failed

$ cargo test -p ironhermes-tools test_activate_config_key_ordering_stable
test result: ok. 1 passed; 0 failed

$ cargo test -p ironhermes-tools test_activate_missing_env_var
test result: ok. 1 passed; 0 failed (Plan 03 regression clean)

$ cargo test -p ironhermes-tools test_activate_all_requirements_met
test result: ok. 1 passed; 0 failed (Plan 03 regression clean)

$ cargo build --workspace
Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.57s
```

## Acceptance Criteria Check

- [x] `pub config: HashMap<String, HashMap<String, serde_yaml::Value>>` in config.rs (line 380)
- [x] `#[serde(default)]` within 3 lines above `pub config:` field
- [x] `pub fn declared_config_schema(` in skills.rs (line 529)
- [x] All 5 Task 1 test fn names present in their respective files
- [x] All 5 Task 1 tests pass
- [x] Pre-existing SkillsConfig round-trip tests still pass (test_config_skills_round_trip, test_skills_config_default, test_config_parses_without_skills_section, test_config_parses_with_skills_section)
- [x] `fn build_skill_config_header(` + `fn format_yaml_value_inline(` in skills_tool.rs
- [x] Literal `"[Skill config: "` prefix in skills_tool.rs
- [x] `pairs.sort_by` in skills_tool.rs for deterministic ordering
- [x] `build_skill_config_header(skills_config, &canonical_name)` call inside `handle_activate`
- [x] All 3 Task 2 tests pass
- [x] Plan 03 regression tests (`test_activate_missing_env_var`, `test_activate_all_requirements_met`) still pass
- [x] `cargo build --workspace` succeeds

## Threat Flags

None — threat register (T-19-04-config-inject-instructions, T-19-04-config-bleed, T-19-04-nondeterministic-output) mitigations all in place:
- **config-inject-instructions:** config values are user-owned (user edits their own config.yaml) — Plan 05 scan will cover schema strings from skill frontmatter.
- **config-bleed:** `skills_config.get(skill_name)` scopes the lookup to the activated skill's map only — verified by Task 2 implementation.
- **nondeterministic-output:** `pairs.sort_by(|a, b| a.0.cmp(&b.0))` present and covered by `test_activate_config_key_ordering_stable`.

## Self-Check: PASSED

- [x] `crates/ironhermes-core/src/config.rs` FOUND (modified, field at line 380)
- [x] `crates/ironhermes-core/src/skills.rs` FOUND (modified, method at line 529)
- [x] `crates/ironhermes-tools/src/skills_tool.rs` FOUND (modified)
- [x] Commit `ec750a6` FOUND in git log (Task 1)
- [x] Commit `4b18d48` FOUND in git log (Task 2)

## Next Phase Readiness

- Wave 2 Plan 04 complete; Plan 05 (skill-content scan with provenance-based enforcement, D-14/D-15) and Plan 06 can now proceed independently.
- Phase 23 `hermes config migrate` CLI has a stable consumption point: call `SkillRegistry::declared_config_schema(name)` per skill and write results under `config.yaml skills.config.<name>`.
- No blockers for downstream waves.

---
*Phase: 19-skills-framework*
*Plan: 04*
*Completed: 2026-04-15*
