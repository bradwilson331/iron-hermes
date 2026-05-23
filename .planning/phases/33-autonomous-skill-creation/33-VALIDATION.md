---
phase: 33
phase_slug: autonomous-skill-creation
date: 2026-05-15
---

# Phase 33: Validation Strategy

## Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Quick run | `cargo test -p ironhermes-tools -- skill_manage` |
| Full suite | `cargo test --workspace` |

## Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File |
|--------|----------|-----------|-------------------|------|
| LEARN-03 | Trigger guidance text contains all 4 conditions in assembled prompt | unit | `cargo test -p ironhermes-agent -- prompt skill_creation` | `prompt_builder.rs` tests |
| LEARN-04 | Created SKILL.md has platforms field + Self-created trust_tier in frontmatter | unit | `cargo test -p ironhermes-tools -- skill_manage::tests::test_skill_manage_create_frontmatter` | `skill_manage.rs` |
| LEARN-04 | Created skill discovered by SkillRegistry on next load with SelfCreated source | unit | `cargo test -p ironhermes-core -- test_skill_registry_discovers_self_created` | `skills.rs` tests |
| LEARN-05 | patch action replaces old_string without full rewrite | unit | `cargo test -p ironhermes-tools -- skill_manage::tests::test_skill_manage_patch` | `skill_manage.rs` |
| LEARN-05 | All 6 actions present in JSON schema enum | unit | `cargo test -p ironhermes-tools -- skill_manage::tests::test_skill_manage_schema_actions` | `skill_manage.rs` |
| LEARN-05 | write_file rejects path traversal | unit | `cargo test -p ironhermes-tools -- skill_manage::tests::test_skill_manage_path_traversal_rejected` | `skill_manage.rs` |
| LEARN-05 | Security scan blocks injected content on create | unit | `cargo test -p ironhermes-tools -- skill_manage::tests::test_skill_manage_create_blocked_content` | `skill_manage.rs` |
| LEARN-03,04,05 | All 6 INV-33-* static-grep regression gates pass | unit | `cargo test -p ironhermes-agent -- invariants_33` | `invariants_33.rs` |

## Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-tools -- skill_manage`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate (before /gsd:verify-work):** `cargo test --workspace` exits 0

## Wave 0 Gaps (tests written as part of plan execution)

- [ ] `crates/ironhermes-tools/src/skill_manage.rs` — 7 unit tests (Plan 33-02 Task 1)
- [ ] `crates/ironhermes-core/src/skills.rs` — `test_skill_registry_discovers_self_created` (Plan 33-03 Task 1)
- [ ] `crates/ironhermes-agent/tests/invariants_33.rs` — 6 INV-33-* tests (Plan 33-03 Task 2)
