---
phase: 5
slug: scheduled-tasks
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-08
---

# Phase 5 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace) |
| **Quick run command** | `cargo test -p ironhermes-cron` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-cron`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 05-01-01 | 01 | 1 | SCHED-01 | — | N/A | unit | `cargo test -p ironhermes-cron parse_schedule` | ❌ W0 | ⬜ pending |
| 05-01-02 | 01 | 1 | SCHED-02 | — | N/A | unit | `cargo test -p ironhermes-cron schedule_kind` | ❌ W0 | ⬜ pending |
| 05-02-01 | 02 | 1 | SCHED-02 | — | N/A | unit | `cargo test -p ironhermes-cron job_update` | ❌ W0 | ⬜ pending |
| 05-02-02 | 02 | 1 | SCHED-03 | — | N/A | unit | `cargo test -p ironhermes-cron skill_attach` | ❌ W0 | ⬜ pending |
| 05-03-01 | 03 | 2 | SCHED-01 | T-05-01 | Cron prompt security scan blocks injection | unit | `cargo test -p ironhermes-cron cron_security` | ❌ W0 | ⬜ pending |
| 05-03-02 | 03 | 2 | SCHED-04 | — | N/A | unit | `cargo test -p ironhermes-cron delivery` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-cron/src/tests/` — test module stubs for parse_schedule, schedule_kind, job_update, skill_attach, cron_security, delivery
- [ ] Test fixtures for sample cron jobs with various schedule kinds

*If none: "Existing infrastructure covers all phase requirements."*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Telegram delivery of cron output | SCHED-04 | Requires live Telegram bot connection | Create a test job with `deliver: "origin"`, trigger tick, verify message appears in Telegram chat |
| Gateway tick spawns tokio task | SCHED-04 | Requires running gateway process | Start gateway, create a job due in 1 min, confirm it executes and delivers |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
