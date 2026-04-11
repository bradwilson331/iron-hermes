---
phase: 08
slug: code-execution
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-10
---

# Phase 08 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust) + pytest (Python helper) |
| **Config file** | Cargo.toml workspace |
| **Quick run command** | `cargo test -p ironhermes-exec` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-exec`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| TBD | TBD | TBD | EXEC-01 | — | N/A | integration | `cargo test -p ironhermes-exec` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | EXEC-02 | — | N/A | integration | `cargo test -p ironhermes-exec` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | EXEC-03 | T-08-01 | env stripping verified | unit | `cargo test -p ironhermes-exec` | ❌ W0 | ⬜ pending |
| TBD | TBD | TBD | EXEC-04 | — | N/A | unit | `cargo test -p ironhermes-exec` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-exec/tests/` — test module stubs for EXEC-01..04
- [ ] Test fixtures for Python script execution (sample .py scripts)
- [ ] Mock tool dispatch for RPC integration tests

*If none: "Existing infrastructure covers all phase requirements."*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Env var stripping inspection | EXEC-03 | Needs runtime env inspection | Run execute_code, inspect child process env via Python `os.environ` dump |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
