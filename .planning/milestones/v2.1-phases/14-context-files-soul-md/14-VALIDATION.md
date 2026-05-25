---
phase: 14
slug: context-files-soul-md
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-12
---

# Phase 14 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | Cargo.toml (workspace) |
| **Quick run command** | `cargo test -p ironhermes-agent --lib` |
| **Full suite command** | `cargo test -p ironhermes-agent -p ironhermes-core` |
| **Estimated runtime** | ~15 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-agent --lib`
- **After every plan wave:** Run `cargo test -p ironhermes-agent -p ironhermes-core`
- **Before `/gsd-verify-work`:** Full suite must be green
- **Max feedback latency:** 15 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| TBD | TBD | TBD | CTX-01 | — | Priority chain enforced | unit | `cargo test -p ironhermes-agent context` | ⬜ | ⬜ pending |
| TBD | TBD | TBD | CTX-02 | — | Git-root walk for .hermes.md | unit | `cargo test -p ironhermes-agent context` | ⬜ | ⬜ pending |
| TBD | TBD | TBD | CTX-03 | — | Subdirectory discovery | unit | `cargo test -p ironhermes-agent context` | ⬜ | ⬜ pending |
| TBD | TBD | TBD | CTX-04 | — | Visited-dir dedup | unit | `cargo test -p ironhermes-agent context` | ⬜ | ⬜ pending |
| TBD | TBD | TBD | CTX-05 | T-14-01 | Injection patterns blocked | unit | `cargo test -p ironhermes-core security` | ✅ | ⬜ pending |
| TBD | TBD | TBD | CTX-06 | — | Truncation at 20K chars | unit | `cargo test -p ironhermes-core truncat` | ✅ | ⬜ pending |
| TBD | TBD | TBD | CTX-07 | — | YAML frontmatter stripped | unit | `cargo test -p ironhermes-agent frontmatter` | ⬜ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] Context loading test stubs for CTX-01 through CTX-04, CTX-07
- [ ] Test fixtures: sample .hermes.md, AGENTS.md, CLAUDE.md, .cursorrules files

*Existing security scanning and truncation tests cover CTX-05, CTX-06.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Subdirectory discovery during live agent session | CTX-03 | Requires real tool calls in agent loop | Run agent, navigate to subdir, verify context injected in tool result |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 15s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
