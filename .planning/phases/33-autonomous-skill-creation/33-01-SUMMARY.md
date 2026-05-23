---
phase: 33
plan: 01
subsystem: skills
tags: [learning-loop, skill-source, prompt-builder, tdd, learn-03, learn-04]
requires:
  - SkillSource enum already exists in ironhermes-core::skills (Phase 19.1)
  - PromptBuilder already exposes set_active_tools(HashSet) (Phase 27.1.1)
  - MemoryConfig already typed in ironhermes-core::config (Phase 21.4)
provides:
  - SkillSource::SelfCreated variant (serde rename = "Self-created")
  - pub fn validate_skill_name — cross-crate slug validator for Plan 02
  - MemoryConfig.skill_creation_guidance: bool (default true)
  - PromptBuilder::set_skill_creation_guidance(bool) setter
  - SKILL_CREATION_GUIDANCE block injected into ToolGuidance slot when
    skill_manage is in active_tools AND the flag is true
affects:
  - ironhermes-core skill enforcement match (WARN-BUT-LOAD arm)
  - trust_level_str in three sites (ironhermes-tools, ironagent-tools-api, ironhermes-cli)
  - system prompt content visible to every CLI/gateway/TUI session that registers skill_manage
tech-stack:
  added: []
  patterns:
    - "Variant-level #[serde(rename)] for hyphenated YAML form"
    - "Durable-slot prompt injection guarded by active_tools.contains + bool flag"
    - "Plan 33-03 follow-up: wire config.memory.skill_creation_guidance through at session freeze"
key-files:
  created: []
  modified:
    - crates/ironhermes-core/src/skills.rs (+ SelfCreated, + pub fn, + WARN-BUT-LOAD arm, + 3 tests, + variants-exhaustive update)
    - crates/ironhermes-core/src/config.rs (+ default_skill_creation_guidance, + MemoryConfig.skill_creation_guidance, + Default update)
    - crates/ironhermes-agent/src/prompt_builder.rs (+ SKILL_CREATION_GUIDANCE const, + field, + setter, + build_tool_guidance branch, + 3 tests)
    - crates/ironhermes-tools/src/skills_tool.rs (+ SelfCreated arm in trust_level_str)
    - crates/ironagent-tools-api/src/skills_tool.rs (+ SelfCreated arm in trust_level_str — byte-identical twin)
    - crates/ironhermes-cli/src/skills_cmd.rs (+ SelfCreated arm in trust_level_str)
decisions:
  - "Place skill_creation_guidance on MemoryConfig (not a new LearningConfig)
    — the wizard-managed `learning:` raw-YAML block has no typed Config analog
    today, and adjacency to nudge_interval matches the plan's intent (both are
    Learning Loop flags read at session freeze). Plan 33-03 will wire it
    through to PromptBuilder."
  - "Inject the guidance into the ToolGuidance slot (slot 3, durable) rather
    than a new slot. The block is behavioral guidance about a tool — semantic
    fit. Durable placement keeps Anthropic prompt-cache hit rate intact
    (D-04 cache-stability)."
  - "active_tools.contains(\"skill_manage\") is the registration check — same
    HashSet PromptBuilder already uses for the catalog-render filter. No new
    tool-registry handle plumbing needed."
metrics:
  duration: "9 minutes"
  completed: "2026-05-16T03:43:13Z"
  tasks_completed: 2
  files_changed: 6
  commits: 4
---

# Phase 33 Plan 01: SkillSource::SelfCreated + Trigger Guidance — Summary

Add the type-level foundation Plans 02 and 03 build on: the `SkillSource::SelfCreated` enum variant, `pub` exposure of `validate_skill_name`, and the system-prompt "Skill Creation (Learning Loop)" trigger guidance block — gated by `skill_manage` registration and a default-on `MemoryConfig.skill_creation_guidance` flag.

## What Shipped

| Capability | Site | Behavior |
|------------|------|----------|
| `SkillSource::SelfCreated` | `ironhermes-core::skills` | Serializes as the hyphenated string `"Self-created"` (variant-level `#[serde(rename)]`); placed in the WARN-BUT-LOAD scan-enforcement arm alongside `Builtin | Official | Trusted` — agent-authored, not untrusted external |
| `pub fn validate_skill_name` | `ironhermes-core::skills` | Promoted from private to public so `SkillManageTool` (Plan 02) can validate slugs cross-crate |
| `MemoryConfig.skill_creation_guidance` | `ironhermes-core::config` | Typed `bool` field (default `true`) with `#[serde(default = "...")]` for backward-compat YAML deserialization |
| `PromptBuilder::set_skill_creation_guidance` | `ironhermes-agent::prompt_builder` | Setter that Plan 33-03 will wire from `config.memory.skill_creation_guidance` at session freeze |
| `SKILL_CREATION_GUIDANCE` block | `ironhermes-agent::prompt_builder` | Appended to ToolGuidance slot (slot 3, durable) when `active_tools.contains("skill_manage")` AND the flag is true — full RESEARCH.md Pattern 6 text verbatim |

## TDD Cycle

| Phase | Commit | Files | Notes |
|-------|--------|-------|-------|
| RED #1 | `c9cc164c` | `skills.rs` | 3 failing tests: SelfCreated serializes as "Self-created", SelfCreated WARN-BUT-LOAD, validate_skill_name pub-callable |
| GREEN #1 | `8fe5134c` | `skills.rs`, `skills_tool.rs` ×2, `skills_cmd.rs` | Variant + pub + 4 match-arm updates across the workspace |
| RED #2 | `1b4adac0` | `prompt_builder.rs` | 3 failing tests: present-when-active, absent-when-flag-false, absent-when-tool-missing |
| GREEN #2 | `90a5e371` | `config.rs`, `prompt_builder.rs` | Field + setter + constant + `build_tool_guidance` branch |

## Acceptance Checks (from PLAN.md)

| Check | Expected | Actual | Pass |
|-------|----------|--------|------|
| `grep -c "SelfCreated" crates/ironhermes-core/src/skills.rs` | ≥3 | 13 | ✓ |
| `grep "pub fn validate_skill_name" crates/ironhermes-core/src/skills.rs` | exactly 1 | 1 | ✓ |
| `grep "Self-created" crates/ironhermes-core/src/skills.rs` (serde rename) | ≥1 | 4 (rename + 3 docs/tests) | ✓ |
| `grep "skill_manage" crates/ironhermes-agent/src/prompt_builder.rs` | ≥1 | 19 | ✓ |
| `grep "Skill Creation" crates/ironhermes-agent/src/prompt_builder.rs` | section header | line 46 (const) + docs | ✓ |
| `grep "5 or more tool calls" crates/ironhermes-agent/src/prompt_builder.rs` | trigger condition | line 49 + test | ✓ |
| `grep "skill_creation_guidance" crates/ironhermes-core/src/config.rs` | ≥1 | 6 | ✓ |
| `cargo build -p ironhermes-core -p ironhermes-agent` | exit 0 | exit 0 (11.78s) | ✓ |
| `cargo test -p ironhermes-core` (skill enum tests) | pass | 466/466 lib tests pass (0 failed) | ✓ |
| Phase 33 prompt_builder tests | pass | 3/3 pass | ✓ |

## SkillSource Match-Arm Audit

Every exhaustive match on `SkillSource` was located via `grep -rn "SkillSource::" crates/` and updated:

| Site | File | Treatment |
|------|------|-----------|
| Scan enforcement match | `crates/ironhermes-core/src/skills.rs:594-610` | SelfCreated joined Builtin/Official/Trusted in the WARN-BUT-LOAD arm |
| Test `test_skill_source_variants_exhaustive` | `crates/ironhermes-core/src/skills.rs:2596-2615` | SelfCreated added to iteration array + match arm |
| `trust_level_str` (ironhermes-tools) | `crates/ironhermes-tools/src/skills_tool.rs:326-339` | SelfCreated → `"self-created"` |
| `trust_level_str` (ironagent-tools-api twin) | `crates/ironagent-tools-api/src/skills_tool.rs:326-339` | SelfCreated → `"self-created"` (byte-identical maintenance) |
| `trust_level_str` (ironhermes-cli) | `crates/ironhermes-cli/src/skills_cmd.rs:201-210` | SelfCreated → `"self-created"` |
| `blob.rs` test match | `crates/ironhermes-hub/src/blob.rs:797-800` | No change — non-exhaustive `match` with `other =>` fallback already handles the new variant correctly (test panics on anything not Community, which is the intended assertion) |

`resolve_source` is a series of early returns (no exhaustive match), so it required no update — it cannot return `SelfCreated` because that variant is assigned by Plan 02's `skill_manage` tool at write time, not by registry source-resolution.

## Deviations from Plan

### Auto-fixed (Rule 3 — blocking issue)

**1. [Rule 3 - Missing referenced struct] `LearningConfig` does not exist**

- **Found during:** Task 2 read_first phase
- **Issue:** Plan 33-01 Task 2 Step 1 reads: "If LearningConfig does not have a skill_creation_guidance: bool field, add it ... Place it adjacent to periodic_nudge_interval_seconds in the struct." Neither `LearningConfig` nor `periodic_nudge_interval_seconds` exists as a typed Rust struct/field — the `learning:` YAML block is wizard-managed raw YAML spliced by `wizard.rs` (intentionally not in the typed `Config` graph), and the closest typed analog is `MemoryConfig.nudge_interval` (Phase 32 LEARN-01).
- **Fix:** Added `skill_creation_guidance: bool` to `MemoryConfig` adjacent to `nudge_interval`. Both fields are Learning-Loop session-freeze knobs read by the agent at startup; co-locating them keeps related state together and lets `PromptBuilder` read a single typed handle. Plan 33-03 will wire `config.memory.skill_creation_guidance → prompt_builder.set_skill_creation_guidance(...)`.
- **Files modified:** `crates/ironhermes-core/src/config.rs`
- **Commit:** `90a5e371`
- **Acceptance still met:** Plan's grep gate (`grep skill_creation_guidance crates/ironhermes-core/src/config.rs` ≥1) returns 6 hits.

### Auto-fixed (Rule 2 — missing critical functionality)

**2. [Rule 2 - Missing match arms in cross-crate consumers] `SkillSource` is matched in three sites outside ironhermes-core**

- **Found during:** Task 1 read_first scan via `grep -rn "SkillSource::" crates/`
- **Issue:** Plan only enumerated the in-crate match sites (skills.rs scan enforcement + the test exhaustive). Two `trust_level_str` copies (ironhermes-tools, ironagent-tools-api) and one in ironhermes-cli's `skills_cmd.rs` would have failed compilation as non-exhaustive matches.
- **Fix:** Added `SelfCreated => "self-created"` arm to all three sites (kebab-case to match the existing lowercase trust_level convention; the YAML frontmatter form is `"Self-created"` enforced by the serde rename). The `ironagent-tools-api` copy is the byte-identical twin maintained alongside `ironhermes-tools` per the PROJECT.md Phase 26.3.2 convention.
- **Files modified:** `crates/ironhermes-tools/src/skills_tool.rs`, `crates/ironagent-tools-api/src/skills_tool.rs`, `crates/ironhermes-cli/src/skills_cmd.rs`
- **Commit:** `8fe5134c`

## Threat Model Verification

| Threat ID | Disposition | Verification |
|-----------|-------------|--------------|
| T-33-01-A — Elevation via SelfCreated scan placement | mitigate | SelfCreated is in the WARN-BUT-LOAD arm of `try_register_skill_from_dir` (skills.rs:602-610), NOT the hard-reject Community arm. `scan_skill_content` still runs at registry load before the source check (skills.rs:584-591), enforcing the same SKILL_THREAT_PATTERNS that protect Builtin/Official/Trusted skills. Verified by `test_self_created_skill_scan_warn_load` — a SelfCreated skill with a known scan-hit phrase loads successfully but emits the WARN log. |
| T-33-01-B — Tampering of trigger guidance text | accept | `SKILL_CREATION_GUIDANCE` is a `const &str` literal; no user-controlled content enters the injection site. The `active_tools.contains("skill_manage")` and `self.skill_creation_guidance` guards are both derived from server-side state (tool registry + typed config), not user input. |
| T-33-01-SC — Supply-chain | accept | Zero new packages installed. No legitimacy gate needed. |

## Test Suite Status

| Crate | Total | Pass | Fail | Notes |
|-------|-------|------|------|-------|
| `ironhermes-core` (lib) | 466 | 466 | 0 | 3 ignored (env-mutex tests); includes new SelfCreated tests |
| `ironhermes-agent` (lib) | 294 | 294 | 0 | Includes 3 new prompt_builder tests |

Pre-existing test failures called out in the executor brief (`chat_memory_persistence` in ironhermes-cli; `server_runtime_parity` and `websocket_lifecycle_parity` in iron_hermes_ui) were not exercised — they are out of scope for plan 33-01 and reproducible on `develop` tip. No new test failures introduced.

## Deferred Issues

None — every Task 1 / Task 2 acceptance criterion passes on the first verification run.

## Self-Check: PASSED

Files exist:

- ✓ `crates/ironhermes-core/src/skills.rs` (modified — SelfCreated variant present)
- ✓ `crates/ironhermes-core/src/config.rs` (modified — skill_creation_guidance field present)
- ✓ `crates/ironhermes-agent/src/prompt_builder.rs` (modified — guidance block injected)
- ✓ `crates/ironhermes-tools/src/skills_tool.rs` (modified — SelfCreated arm)
- ✓ `crates/ironagent-tools-api/src/skills_tool.rs` (modified — SelfCreated arm, twin)
- ✓ `crates/ironhermes-cli/src/skills_cmd.rs` (modified — SelfCreated arm)

Commits exist in `git log`:

- ✓ `c9cc164c` — `test(33-01): add failing tests for SkillSource::SelfCreated variant`
- ✓ `8fe5134c` — `feat(33-01): add SkillSource::SelfCreated variant + expose validate_skill_name as pub`
- ✓ `1b4adac0` — `test(33-01): add failing tests for skill-creation trigger guidance block`
- ✓ `90a5e371` — `feat(33-01): inject skill-creation trigger guidance into ToolGuidance slot`
