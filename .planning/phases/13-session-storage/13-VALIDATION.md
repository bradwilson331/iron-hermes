---
phase: 13
slug: session-storage
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-11
---

# Phase 13 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | `Cargo.toml` workspace config |
| **Quick run command** | `cargo test -p ironhermes-state` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-state`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 13-01-01 | 01 | 1 | SESS-01 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-02 | 01 | 1 | SESS-02 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-03 | 01 | 1 | SESS-03 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-04 | 01 | 1 | SESS-04 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-05 | 01 | 1 | SESS-05 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-06 | 01 | 1 | SESS-06 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-07 | 01 | 1 | SESS-07 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-08 | 01 | 1 | SESS-08 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-09 | 01 | 1 | SESS-09 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-10 | 01 | 1 | SESS-10 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |
| 13-01-11 | 01 | 1 | SESS-11 | — | N/A | integration | `cargo test -p ironhermes-state` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-state/tests/` — test module stubs for all SESS requirements
- [ ] `tempfile` dev-dependency — for isolated per-test databases

*Existing infrastructure covers framework needs (cargo test built-in).*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| WAL checkpoint timer fires | SESS-10 | Requires 5-minute wait for timer | Start process, wait 5min, check WAL file size decreases |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
