---
phase: 34a
slug: read-side-memory-parity
status: planned
nyquist_compliant: true
wave_0_complete: true
created: 2026-05-20
---

# Phase 34a — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | `cargo test` (Rust 2024) |
| **Config file** | none — workspace Cargo.toml |
| **Quick run command** | `cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests` |
| **Full suite command** | `cargo test --workspace` |
| **Estimated runtime** | ~quick <10s / full several minutes |

---

## Sampling Rate

- **After every task commit:** Run the quick command for the modules touched
- **After every plan wave:** Run the full suite
- **Before `/gsd:verify-work`:** Full suite must be green
- **Max feedback latency:** ~10 seconds (quick)

---

## Per-Task Verification Map

> Filled by the planner from RESEARCH.md "## Validation Architecture". Each MEM-READ requirement maps to unit tests + cross-phase regression gates.

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 34a-01-T1 | 01 | 1 | MEM-READ-01 | T-34a-01/02 | primary-only recall proxy; file provider inherits no-op | unit | `cargo test -p ironhermes-core --lib memory_provider && cargo test -p ironhermes-agent --lib memory::manager` | ✅ (extend existing) | ⬜ pending |
| 34a-01-T2 | 01 | 1 | MEM-READ-02 | T-34a-01/02 | sanitize_context strips fences/notes before wrapping; build idempotent | unit (8) | `cargo test -p ironhermes-agent --lib memory_context::tests` | ❌ created in T2 | ⬜ pending |
| 34a-02-T1 | 02 | 2 | MEM-READ-03 | T-34a-05/06 | recall flag wire-transparent; compressor step-0 evicts recall; D-12 snapshot untouched | unit | `cargo test -p ironhermes-agent --lib context_compressor && cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` | ✅ (extend existing) | ⬜ pending |
| 34a-02-T2 | 02 | 2 | MEM-READ-04 | T-34a-04 | fence tags never reach user-visible stream; unterminated span discarded on flush | unit (6) | `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` | ❌ created in T2 | ⬜ pending |
| 34a-02-T3 | 02 | 2 | MEM-READ-03 (inject) + MEM-READ-05 | T-34a-04/05/06/07 | recall injected before last user msg, evicted pre-turn, skipped on empty (D-08); scrubber wired+flushed on all 3 surfaces | unit + static-grep | `cargo build --workspace && cargo test -p ironhermes-agent --lib agent_loop && grep -c "\.feed(" crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/ws.rs` | ✅ (modify existing) | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Cross-Phase Regression Gates (run at every wave merge + phase gate)

| Gate | Command | Expected |
|------|---------|----------|
| Phase 32 nudge | `cargo test -p ironhermes-agent --lib nudge::tests` | 6/6 green |
| Phase 33 invariants | `cargo test -p ironhermes-agent --test invariants_33` | 6/6 green |
| D-12 frozen snapshot | `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` | green |

---

## Wave 0 Requirements

*Existing `cargo test` infrastructure covers all phase requirements — no Wave 0 scaffold install needed. New unit test modules (`memory_context::tests`, `streaming_scrubber::tests`) are created inline with their production code in Plan 01 Task 2 and Plan 02 Task 2 respectively (test-first via `tdd="true"` + `<behavior>` blocks).*

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Live recall round-trip | MEM-READ-03 | Needs a running agent + recall-capable (or stub) provider | Configure stub provider returning fixed recall; "remember I prefer dark mode" → later "what do you remember?" references it; no `<memory-context>` tags in scrollback |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references (test modules created inline, test-first)
- [x] No watch-mode flags
- [x] Feedback latency < 10s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** planned — 2026-05-20
