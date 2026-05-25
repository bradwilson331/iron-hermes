---
phase: 11
slug: memory-provider-trait
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-11
---

# Phase 11 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace) |
| **Quick run command** | `cargo test --package ironhermes-core` |
| **Full suite command** | `cargo test` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --package ironhermes-core`
- **After every plan wave:** Run `cargo test`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 11-01-01 | 01 | 1 | MEM-07 | — | Trait compiles with Send + Sync + 'static bounds | unit | `cargo test --package ironhermes-core` | ✅ | ⬜ pending |
| 11-01-02 | 01 | 1 | MEM-08 | — | MemoryStore implements MemoryProvider | unit | `cargo test --package ironhermes-core` | ✅ | ⬜ pending |
| 11-02-01 | 02 | 1 | MEM-08 | — | All call sites use dyn MemoryProvider | integration | `cargo test` | ✅ | ⬜ pending |
| 11-03-01 | 03 | 2 | MEM-12 | — | Config accepts single provider selection | unit | `cargo test --package ironhermes-core` | ✅ | ⬜ pending |
| 11-03-02 | 03 | 2 | MEM-12 | — | Rejects multiple provider activation | unit | `cargo test --package ironhermes-core` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] Test for provider config validation (single-provider rejection) — may need new test
- Existing test infrastructure covers trait compilation and MemoryStore behavior

*Existing infrastructure covers most phase requirements. One new test needed for MEM-12 rejection behavior.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Hard error on missing feature-gated provider | MEM-12 (D-09) | Requires building without feature flag | Build with `--no-default-features`, set config to `sqlite`, verify startup error message |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
