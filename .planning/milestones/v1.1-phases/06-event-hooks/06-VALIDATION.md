---
phase: 6
slug: event-hooks
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-08
---

# Phase 6 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml |
| **Quick run command** | `cargo test --lib` |
| **Full suite command** | `cargo test` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test --lib`
- **After every plan wave:** Run `cargo test`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 06-01-01 | 01 | 1 | HOOK-01 | — | N/A | unit | `cargo test hook` | ❌ W0 | ⬜ pending |
| 06-01-02 | 01 | 1 | HOOK-02 | T-06-01 | Guardrail blocks unauthorized tool calls | unit | `cargo test guardrail` | ❌ W0 | ⬜ pending |
| 06-01-03 | 01 | 1 | HOOK-03 | T-06-02 | Webhook validates URL before POST | integration | `cargo test webhook` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] Test stubs for HOOK-01 (lifecycle event logging)
- [ ] Test stubs for HOOK-02 (guardrail interception)
- [ ] Test stubs for HOOK-03 (webhook delivery)
- [ ] Shared test fixtures for hook registry setup

*Existing cargo test infrastructure covers framework needs.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Webhook delivery to external endpoint | HOOK-03 | Requires live HTTP server | Start local HTTP server, configure webhook, trigger event, verify POST received |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
