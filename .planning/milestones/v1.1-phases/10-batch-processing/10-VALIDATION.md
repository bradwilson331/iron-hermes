---
phase: 10
slug: batch-processing
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-10
---

# Phase 10 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml workspace — existing test infrastructure |
| **Quick run command** | `cargo test -p ironhermes-cli --lib` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-cli --lib`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 10-01-01 | 01 | 0 | BATCH-01 | — | N/A | unit | `cargo test -p ironhermes-cli batch` | ❌ W0 | ⬜ pending |
| 10-01-02 | 01 | 1 | BATCH-01 | — | N/A | unit | `cargo test -p ironhermes-cli batch` | ❌ W0 | ⬜ pending |
| 10-01-03 | 01 | 1 | BATCH-02 | — | N/A | unit | `cargo test -p ironhermes-cli batch` | ❌ W0 | ⬜ pending |
| 10-01-04 | 01 | 1 | BATCH-03 | — | N/A | unit | `cargo test -p ironhermes-cli batch` | ❌ W0 | ⬜ pending |
| 10-01-05 | 01 | 2 | BATCH-04 | T-10-01 | Secrets not leaked in output | unit | `cargo test -p ironhermes-cli batch` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-cli/src/batch/mod.rs` — batch module skeleton
- [ ] `crates/ironhermes-cli/src/batch/tests.rs` — test stubs for BATCH-01..04
- [ ] Existing test infrastructure covers framework needs — no new deps

*Existing cargo test infrastructure covers all framework requirements.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| HuggingFace dataset viewer loads output | BATCH-02 | External tool validation | Upload sample .jsonl to HF datasets, confirm viewer renders conversations |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
