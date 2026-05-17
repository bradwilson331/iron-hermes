---
phase: 34b
slug: context-system-parity
status: ready
nyquist_compliant: true
wave_0_complete: true
created: 2026-05-16
revised: 2026-05-16
---

# Phase 34b — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

**Revision note (2026-05-16):** Wave 0 is now covered by a dedicated plan `34b-00-PLAN.md` (wave: 0). That plan creates the four required test scaffolds (`context_refs.rs` stub, `tests/invariants_34b.rs` stub, `#[ignore]` placeholders for `test_context_compressor_reset_zeroes_counter` and `test_memory_authority_header`) BEFORE plans 34b-01 and 34b-02 execute. With Wave 0 covered, `nyquist_compliant: true` and `wave_0_complete: true`.

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
| 34b-00-01 | 00 | 0 | context_refs stub | — | N/A | scaffold | `cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast` | ✅ (created by this plan) | ⬜ pending |
| 34b-00-02 | 00 | 0 | invariants_34b stub | — | N/A | scaffold | `cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast` | ✅ (created by this plan) | ⬜ pending |
| 34b-00-03 | 00 | 0 | reset + header placeholders | — | N/A | scaffold | `cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast` | ✅ (created by this plan) | ⬜ pending |
| 34b-01-01 | 01 | 1 | @-ref parser | T-34b-01-PATH, T-34b-01-SC | Sensitive-path rejection | unit | `cargo test -p ironhermes-agent --lib context_refs::tests` | ✅ | ⬜ pending |
| 34b-01-02 | 01 | 1 | @file: expansion + budget | T-34b-01-PATH, T-34b-01-DOS | allowed_root enforced; 50% hard / 25% soft | unit | `cargo test -p ironhermes-agent --lib context_refs::tests::test_expand_file_full context_refs::tests::test_expand_file_with_range context_refs::tests::test_hard_limit_blocks_all context_refs::tests::test_soft_limit_warns` | ✅ | ⬜ pending |
| 34b-01-03 | 01 | 1 | 3-surface wiring | T-34b-01-SSRF, T-34b-01-SHELL | No silent drop on LLM failure; no shell injection | integration | `cargo build --workspace && grep -c preprocess_context_references_async crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs` (sum must equal 3) | ✅ | ⬜ pending |
| 34b-02-01 | 02 | 2 | ContextEngine hooks + counter reset | T-34b-02-COMPAT | No counter bleed across /reset (all 4 fields zero) | unit | `cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests::test_context_compressor_reset_zeroes_counter pressure_warning::tests summarizing_engine::tests` | ✅ | ⬜ pending |
| 34b-02-02 | 02 | 2 | Memory-authority reminder | T-34b-02-DRIFT | Header contains MEMORY.md + ALWAYS authoritative | unit | `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header summarizing_engine::tests::test_memory_authority_constant_text context_compressor::tests::test_compaction_header_contains_memory_authority_reminder` | ✅ | ⬜ pending |
| 34b-02-03 | 02 | 2 | 3-surface hook wiring | T-34b-02-STUB | reset_web_session stub + tracing log | integration | `cargo test -p ironhermes-cli --test lifecycle_hooks_wired && cargo build --workspace` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements (all SATISFIED by plan 34b-00)

- [x] `crates/ironhermes-agent/src/context_refs.rs` — new module stub with `#[cfg(test)] mod tests {}` (Plan 34b-00 Task 1)
- [x] `crates/ironhermes-agent/tests/invariants_34b.rs` — integration test stub with `#[ignore]` placeholder (Plan 34b-00 Task 2)
- [x] `crates/ironhermes-agent/src/context_compressor.rs` — `#[ignore]` placeholder `test_context_compressor_reset_zeroes_counter` (Plan 34b-00 Task 3)
- [x] `crates/ironhermes-agent/src/summarizing_engine.rs` — `#[ignore]` placeholder `test_memory_authority_header` (Plan 34b-00 Task 3)

*Existing `cargo test` infrastructure covers all other phase requirements.*

---

## Wave / Plan Ordering

```
Wave 0: 34b-00-PLAN.md (test scaffolds)
   ↓
Wave 1: 34b-01-PLAN.md (context_refs module + 3-surface @-ref wiring)
   ↓
Wave 2: 34b-02-PLAN.md (ContextEngine hooks + memory-authority reminder + 3-surface lifecycle wiring)
```

Wave 1 depends on Wave 0 because Plan 34b-01 Task 1 mutates `context_refs.rs` from a stub into the full implementation. Wave 2 depends on Wave 1 only for sequencing — there are no file-overlap conflicts. Plan 34b-00 itself has `depends_on: []`.

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
| `on_session_start` / `on_session_reset` lifecycle in web UI | D-07 | Web UI reset trigger TBD (research finding #6) | Manually verify WebSocket connect triggers `on_session_start`; planner confirms reset trigger via `reset_web_session` stub call |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references (delivered by 34b-00-PLAN.md)
- [x] No watch-mode flags
- [x] Feedback latency < 20s
- [x] `nyquist_compliant: true` set in frontmatter

**Approval:** ready
