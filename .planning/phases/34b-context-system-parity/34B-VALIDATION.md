---
phase: 34b
slug: context-system-parity
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-05-16
---

# Phase 34b — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` (Rust / tokio) |
| **Config file** | `Cargo.toml` (workspace) |
| **Quick run command** | `cargo test -p ironhermes-agent --lib 2>&1 \| tail -20` |
| **Full suite command** | `cargo test -p ironhermes-agent && cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` |
| **Estimated runtime** | ~15 seconds |

---

## Sampling Rate

- **After every task commit:** Run `cargo test -p ironhermes-agent --lib 2>&1 | tail -20`
- **After every plan wave:** Run `cargo test -p ironhermes-agent && cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** 20 seconds

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 34b-01-01 | 01 | 1 | @-ref parser | — | Sensitive-path rejection | unit | `cargo test -p ironhermes-agent --lib context_refs::tests` | ❌ W0 | ⬜ pending |
| 34b-01-02 | 01 | 1 | @file: expansion | — | allowed_root enforced | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_file_expansion` | ❌ W0 | ⬜ pending |
| 34b-01-03 | 01 | 1 | @url: LLM + fallback | — | No silent drop on LLM failure | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_url_fallback` | ❌ W0 | ⬜ pending |
| 34b-01-04 | 01 | 1 | Budget enforcement | — | 50% hard reject, 25% warn | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_budget` | ❌ W0 | ⬜ pending |
| 34b-01-05 | 01 | 2 | 3-surface wiring | — | N/A | integration | `cargo test -p ironhermes-agent --lib context_refs::tests::test_preprocess` | ❌ W0 | ⬜ pending |
| 34b-02-01 | 02 | 1 | ContextEngine hooks | — | N/A | unit | `cargo test -p ironhermes-agent --lib context_engine::tests` | ✅ | ⬜ pending |
| 34b-02-02 | 02 | 1 | Counter reset | — | No counter bleed across /reset | unit | `cargo test -p ironhermes-agent --lib context_compressor::tests::test_on_session_reset` | ❌ W0 | ⬜ pending |
| 34b-02-03 | 02 | 1 | SUMMARY_PREFIX | — | Header contains MEMORY.md + ALWAYS authoritative | unit | `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header` | ❌ W0 | ⬜ pending |
| 34b-02-04 | 02 | 2 | 3-surface hook wiring | — | N/A | integration | `cargo test -p ironhermes-agent --test invariants_34b` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-agent/src/context_refs.rs` — new module stub with test module
- [ ] `crates/ironhermes-agent/tests/invariants_34b.rs` — integration test stubs for 3-surface wiring
- [ ] `crates/ironhermes-agent/src/context_compressor.rs` — test stub for `test_on_session_reset`
- [ ] `crates/ironhermes-agent/src/summarizing_engine.rs` — test stub for `test_memory_authority_header`

*Existing `cargo test` infrastructure covers all other phase requirements.*

---

## Regression Gates (must stay green throughout)

- `cargo test -p ironhermes-agent --lib memory_context::tests` — Phase 34a
- `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` — Phase 34a
- `cargo test -p ironhermes-agent --test invariants_33` — Phase 33 (6/6)
- `cargo test -p ironhermes-agent --lib nudge::tests` — Phase 32 (6/6)
- `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` — D-12 gate

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| `@url:` expansion in live CLI session | D-01 | Requires network + live LLM | `cargo run --bin hermes` → type `@url:https://example.com what is this?` → verify attached-context footer appears |
| `@diff` / `@staged` in gateway | D-03 | Requires live git repo + HTTP session | Send message with `@diff` via API; verify diff block appears in context |
| `on_session_start` / `on_session_reset` lifecycle in web UI | D-07 | Web UI reset trigger TBD (research finding #6) | Manually verify WebSocket connect triggers `on_session_start`; planner confirms reset trigger |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 20s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
