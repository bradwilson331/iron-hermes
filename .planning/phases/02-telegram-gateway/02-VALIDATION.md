---
phase: 02
slug: telegram-gateway
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-01
---

# Phase 02 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | cargo test (Rust built-in) |
| **Config file** | `Cargo.toml` workspace config |
| **Quick run command** | `cargo test -p ironhermes-gateway` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~30 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-gateway`
- **After every plan wave:** Run `cargo test --workspace`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 30 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|-----------|-------------------|-------------|--------|
| 02-01-01 | 01 | 1 | ASYNC-01 | unit | `cargo test -p ironhermes-gateway cancellation` | ❌ W0 | ⬜ pending |
| 02-01-02 | 01 | 1 | ASYNC-02 | unit | `cargo test -p ironhermes-gateway backoff` | ❌ W0 | ⬜ pending |
| 02-01-03 | 01 | 1 | ASYNC-03 | unit | `cargo test -p ironhermes-gateway concurrency` | ❌ W0 | ⬜ pending |
| 02-02-01 | 02 | 1 | TG-01 | integration | `cargo test -p ironhermes-gateway polling` | ❌ W0 | ⬜ pending |
| 02-02-02 | 02 | 1 | TG-02 | unit | `cargo test -p ironhermes-gateway stream_consumer` | ❌ W0 | ⬜ pending |
| 02-03-01 | 03 | 2 | TG-03 | unit | `cargo test -p ironhermes-gateway session` | ❌ W0 | ⬜ pending |
| 02-03-02 | 03 | 2 | TG-04 | unit | `cargo test -p ironhermes-gateway whitelist` | ❌ W0 | ⬜ pending |
| 02-04-01 | 04 | 2 | TG-05 | unit | `cargo test -p ironhermes-gateway slash_commands` | ❌ W0 | ⬜ pending |
| 02-04-02 | 04 | 2 | TG-06 | unit | `cargo test -p ironhermes-gateway multimodal` | ❌ W0 | ⬜ pending |
| 02-05-01 | 05 | 3 | TG-07, TG-08 | integration | `cargo test -p ironhermes-gateway error_recovery` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-gateway/tests/` — test module structure
- [ ] Mock Telegram API server or trait-based mock for Bot API calls
- [ ] Test fixtures for TgUpdate, TgMessage payloads
- [ ] AgentLoop mock/stub for testing gateway without live LLM

*Existing infrastructure covers workspace-level cargo test. Gateway-specific test stubs needed.*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Progressive streaming edits visible in Telegram | TG-02 | Requires live Telegram client to observe edit cadence | Send message to bot, observe message edits arriving ~300ms apart with cursor |
| Graceful shutdown on ctrl+c | ASYNC-01 | Signal handling requires interactive terminal | Run bot, send message, press ctrl+c during response, verify clean exit |
| Bot reconnects after network drop | TG-07 | Requires network interruption simulation | Start bot, disable network briefly, re-enable, verify polling resumes |
| 409 conflict detection and fatal exit | TG-08 | Requires second bot instance on same token | Start two bot instances, verify first detects 409 and exits after retries |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 30s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
