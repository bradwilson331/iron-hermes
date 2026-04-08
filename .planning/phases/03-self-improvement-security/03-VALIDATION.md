---
phase: 3
slug: self-improvement-security
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-07
---

# Phase 3 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace) |
| **Quick run command** | `cargo test -p ironhermes-core` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~15 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-core`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 15 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 03-01-01 | 01 | 1 | SELF-03/SEC-02 | T-03-01 | Threat patterns block context file writes | unit | `cargo test -p ironhermes-core -- context_scanner` | ❌ W0 | ⬜ pending |
| 03-01-02 | 01 | 1 | SELF-06 | — | Atomic writes produce complete files | unit | `cargo test -p ironhermes-core -- atomic_write` | ❌ W0 | ⬜ pending |
| 03-02-01 | 02 | 1 | SELF-04 | — | MemoryStore respects char limits | unit | `cargo test -p ironhermes-core -- memory_store` | ❌ W0 | ⬜ pending |
| 03-02-02 | 02 | 1 | SELF-05 | T-03-02 | Memory tool add/replace/remove actions | unit | `cargo test -p ironhermes-core -- memory` | ❌ W0 | ⬜ pending |
| 03-03-01 | 03 | 2 | SEC-01 | T-03-03 | SSRF blocks private IPs, localhost, CGNAT | unit | `cargo test -p ironhermes-core -- ssrf` | ❌ W0 | ⬜ pending |
| 03-04-01 | 04 | 2 | SEC-03 | T-03-04 | Rate limiter drops excess messages | unit | `cargo test -p ironhermes-gateway -- rate_limit` | ❌ W0 | ⬜ pending |
| 03-05-01 | 05 | 3 | SELF-01/SELF-02 | T-03-05 | File tools scan context files before write | integration | `cargo test -p ironhermes-tools -- file_tools` | ❌ W0 | ⬜ pending |
| 03-05-02 | 05 | 3 | SELF-04 | — | Memory injected into system prompt | integration | `cargo test -p ironhermes-agent -- prompt_builder` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/context_scanner.rs` — moved from agent, tests for all threat patterns
- [ ] `crates/ironhermes-core/tests/memory_store_tests.rs` — unit tests for MemoryStore
- [ ] `crates/ironhermes-core/tests/ssrf_tests.rs` — unit tests for SSRF validation
- [ ] `crates/ironhermes-gateway/tests/rate_limit_tests.rs` — unit tests for rate limiter

*Existing test infrastructure (cargo test) covers framework requirements.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Memory persists across sessions | SELF-04 | Requires agent restart to verify frozen-snapshot reload | 1. Start agent, add memory entry 2. Stop agent 3. Start agent, verify memory in prompt |
| Context edit reflected next session | SELF-02 | Requires full agent lifecycle | 1. Start agent, edit SOUL.md via tool 2. Stop agent 3. Start agent, verify personality change |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
