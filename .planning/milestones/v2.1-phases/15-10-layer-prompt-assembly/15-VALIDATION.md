---
phase: 15
slug: 10-layer-prompt-assembly
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-12
---

# Phase 15 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Rust built-in `#[test]` + `tempfile` crate |
| **Config file** | none (inline `#[cfg(test)]` modules) |
| **Quick run command** | `cargo test -p ironhermes-agent -- --test-threads=1 2>&1` |
| **Full suite command** | `cargo test --workspace -- --test-threads=1 2>&1` |
| **Estimated runtime** | ~15 seconds |

Note: `--test-threads=1` is required because prompt_builder tests manipulate environment variables (`IRONHERMES_HOME`) with a static `ENV_MUTEX` lock.

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-agent -- --test-threads=1 2>&1`
- **After every plan wave:** Run `cargo test --workspace -- --test-threads=1 2>&1`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** ~15 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 15-01-01 | 01 | 1 | PRMT-01 | — | N/A | unit | `cargo test -p ironhermes-agent test_slot_ordering -- --test-threads=1` | ❌ W0 | ⬜ pending |
| 15-01-02 | 01 | 1 | PRMT-02 | — | N/A | unit | `cargo test -p ironhermes-agent test_build_split -- --test-threads=1` | ❌ W0 | ⬜ pending |
| 15-01-03 | 01 | 1 | PRMT-03 | — | N/A | unit | `cargo test -p ironhermes-agent test_soul_replaces_default -- --test-threads=1` | ✅ (update) | ⬜ pending |
| 15-01-04 | 01 | 1 | PRMT-04 | T-15-01 | Blocked SOUL.md falls back to default identity | unit | `cargo test -p ironhermes-agent test_soul_security_scan -- --test-threads=1` | ❌ W0 | ⬜ pending |
| 15-01-05 | 01 | 1 | PRMT-05 | — | N/A | unit | `cargo test -p ironhermes-agent test_skip_context_files_default_identity -- --test-threads=1` | ✅ (update) | ⬜ pending |
| 15-02-01 | 02 | 2 | PRMT-06 | — | N/A | unit | `cargo test -p ironhermes-agent test_personality_overlay -- --test-threads=1` | ❌ W0 | ⬜ pending |
| 15-02-02 | 02 | 2 | PRMT-07 | T-15-02 | Custom personality files security scanned | unit | `cargo test -p ironhermes-agent test_personality_registry -- --test-threads=1` | ❌ W0 | ⬜ pending |
| 15-02-03 | 02 | 2 | MEM-06 | — | N/A | unit | `cargo test -p ironhermes-core test_snapshot_frozen_after_load -- --test-threads=1` | ✅ exists | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `test_slot_ordering` — verify slots 1-9 appear in correct order in `build()` output — covers PRMT-01
- [ ] `test_build_split_durable_ephemeral` — verify `build_split()` partitions correctly — covers PRMT-02
- [ ] `test_soul_security_scan` — verify blocked SOUL.md falls back to default identity — covers PRMT-04
- [ ] `test_personality_overlay` — verify `set_overlay()` / `clear_overlay()` affect slot 8 — covers PRMT-06
- [ ] `test_personality_registry_builtins` — verify all 14 presets present — covers PRMT-07
- [ ] `test_personality_registry_custom_config` — verify config.yaml presets loaded + override HERMES_HOME — covers PRMT-07
- [ ] `test_hermes_md_in_candidates` — verify HERMES.md is in CONTEXT_CANDIDATES (D-18) — covers D-18
- [ ] `test_skip_context_files_skips_slots_3_to_8` — verify subagent gets only slots 1-2 — covers PRMT-05

Existing tests that need **updating** (not replacing):
- `test_assembly_order` — assert new slot ordering (SOUL < MEMORY < SKILLS < PROJECT CONTEXT)
- `test_context_candidates_case_sensitive` — update len() assertion from 4 to 5, add HERMES.md

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| /personality command end-to-end | PRMT-06 | Requires interactive CLI session | Run CLI, issue `/personality pirate`, verify response tone changes |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
