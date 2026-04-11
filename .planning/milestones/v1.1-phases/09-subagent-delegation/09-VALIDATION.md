---
phase: 9
slug: subagent-delegation
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-10
---

# Phase 9 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml workspace |
| **Quick run command** | `cargo test -p ironhermes-tools --lib` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-tools --lib`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 09-01-01 | 01 | 1 | AGENT-01 | — | N/A | unit | `cargo test -p ironhermes-tools delegate` | ❌ W0 | ⬜ pending |
| 09-01-02 | 01 | 1 | AGENT-02 | T-09-01 | Child cannot call tools outside allowlist | unit | `cargo test -p ironhermes-tools delegate` | ❌ W0 | ⬜ pending |
| 09-01-03 | 01 | 1 | AGENT-05 | T-09-02 | delegate_task never in child toolset | unit | `cargo test -p ironhermes-tools delegate` | ❌ W0 | ⬜ pending |
| 09-02-01 | 02 | 1 | AGENT-04 | — | Subagent CWD isolated from parent | unit | `cargo test -p ironhermes-tools terminal` | ❌ W0 | ⬜ pending |
| 09-02-02 | 02 | 1 | AGENT-03 | — | Semaphore blocks at limit | unit | `cargo test -p ironhermes-tools delegate` | ❌ W0 | ⬜ pending |
| 09-03-01 | 03 | 2 | AGENT-01 | — | N/A | integration | `cargo test -p ironhermes-cli delegate` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-tools/src/delegate_task.rs` — test module with stubs for AGENT-01..05
- [ ] Test helpers for building filtered ToolRegistry instances

*Existing cargo test infrastructure covers framework needs.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| "Waiting for slot" message visible to user | AGENT-03 | Requires observing LLM tool result or log output in real session | Run delegate_task 4 times concurrently, verify 4th shows waiting message |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
