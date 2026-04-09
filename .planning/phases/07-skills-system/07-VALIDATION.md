---
phase: 7
slug: skills-system
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-09
---

# Phase 7 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in test + `#[tokio::test]` for async |
| **Config file** | `Cargo.toml` per-crate (no separate config file) |
| **Quick run command** | `cargo test -p ironhermes-core skills 2>&1 \| tail -20` |
| **Full suite command** | `cargo test --workspace 2>&1 \| tail -40` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core skills` and `cargo test -p ironhermes-tools skills`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 07-01-01 | 01 | 1 | SKILL-01 | T-07-01 | Paths from trusted registry only | unit | `cargo test -p ironhermes-core skill_registry` | ❌ W0 | ⬜ pending |
| 07-01-02 | 01 | 1 | SKILL-01 | — | N/A | unit | `cargo test -p ironhermes-core skill_registry_missing_paths` | ❌ W0 | ⬜ pending |
| 07-01-03 | 01 | 1 | SKILL-03 | T-07-02 | No path traversal from name | unit | `cargo test -p ironhermes-core parse_skill_md` | ❌ W0 | ⬜ pending |
| 07-01-04 | 01 | 1 | SKILL-03 | — | N/A | unit | `cargo test -p ironhermes-core parse_skill_md_no_frontmatter` | ❌ W0 | ⬜ pending |
| 07-01-05 | 01 | 1 | SKILL-02 | — | N/A | unit | `cargo test -p ironhermes-core catalog_text` | ❌ W0 | ⬜ pending |
| 07-02-01 | 02 | 1 | SKILL-04 | T-07-03 | Name lookup only, no fs from args | unit | `cargo test -p ironhermes-tools skills_tool_list` | ❌ W0 | ⬜ pending |
| 07-02-02 | 02 | 1 | SKILL-04 | — | N/A | unit | `cargo test -p ironhermes-tools skills_tool_view` | ❌ W0 | ⬜ pending |
| 07-02-03 | 02 | 1 | SKILL-04 | — | N/A | unit | `cargo test -p ironhermes-tools skills_tool_activate` | ❌ W0 | ⬜ pending |
| 07-02-04 | 02 | 1 | SKILL-04 | — | N/A | unit | `cargo test -p ironhermes-tools skills_tool_unknown_action` | ❌ W0 | ⬜ pending |
| 07-03-01 | 03 | 2 | SKILL-02 | — | N/A | unit | `cargo test -p ironhermes-agent prompt_builder_skills` | ❌ W0 | ⬜ pending |
| 07-03-02 | 03 | 2 | D-08/D-09 | — | N/A | unit | `cargo test -p ironhermes-gateway cron_skill_resolution` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/skills.rs` — SkillRegistry, SkillRecord, parse_skill_md tests
- [ ] `crates/ironhermes-tools/src/skills_tool.rs` — SkillsTool with list/view/activate tests
- [ ] `crates/ironhermes-agent/src/prompt_builder.rs` — extend with skill_registry field + tests
- [ ] `crates/ironhermes-gateway/src/runner.rs` — skill resolution at tick time + tests

*Existing infrastructure covers test framework — only test files need creation.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Skill catalog appears in system prompt during live session | SKILL-02 | Requires live agent session with LLM | Start agent with skills dir containing 2+ skills, verify catalog in system prompt output |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
