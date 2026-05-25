---
phase: 34b
slug: context-system-parity
status: ready
nyquist_compliant: true
wave_0_complete: true
created: 2026-05-16
revised: 2026-05-22
---

# Phase 34b — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

**Revision note (2026-05-16):** Wave 0 is now covered by a dedicated plan `34b-00-PLAN.md` (wave: 0). That plan creates the four required test scaffolds (`context_refs.rs` stub, `tests/invariants_34b.rs` stub, `#[ignore]` placeholders for `test_context_compressor_reset_zeroes_counter` and `test_memory_authority_header`) BEFORE plans 34b-01 and 34b-02 execute. With Wave 0 covered, `nyquist_compliant: true` and `wave_0_complete: true`.

**Revision note (2026-05-22) — reconcile with post-28.1 replan:** The plans were regenerated against `AgentRuntime::run_turn` (Phase 28.1). Two verification rows were stale and are corrected here:
- Row **34b-01-03** previously asserted the `preprocess_context_references_async` grep across the 3 surface files "sum must equal 3" — that was the pre-28.1 per-surface assumption. Per D-09 the preprocessing centralizes ONCE in `run_turn`, so the surface grep must sum to **0**. Row description changed from "3-surface wiring" to "centralization guard (preprocessing in run_turn, not surfaces)".
- Row **34b-02-03** previously named `cargo test -p ironhermes-cli --test lifecycle_hooks_wired`, a test no plan creates. Replaced with the `invariants_34b` integration target + workspace build that Plan 02 Task 3 actually verifies.
- The Wave-0 scaffold files are NOT yet on disk (the prior draft marked them "✅ created" but they were never written); the File-Exists column now reflects "⬜ created by 34b-00".

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
| 34b-00-01 | 00 | 0 | context_refs stub | — | N/A | scaffold | `cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast` | ⬜ created by 34b-00 | ⬜ pending |
| 34b-00-02 | 00 | 0 | invariants_34b stub | — | N/A | scaffold | `cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast` | ⬜ created by 34b-00 | ⬜ pending |
| 34b-00-03 | 00 | 0 | reset + header placeholders | — | N/A | scaffold | `cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast` | ⬜ created by 34b-00 | ⬜ pending |
| 34b-01-01 | 01 | 1 | @-ref parser | T-34b-01-PATH, T-34b-01-SC | Sensitive-path rejection | unit | `cargo test -p ironhermes-agent --lib context_refs::tests` | ✅ | ⬜ pending |
| 34b-01-02 | 01 | 1 | @file: expansion + budget | T-34b-01-PATH, T-34b-01-DOS, T-34b-01-SHELL | allowed_root enforced; 50% hard / 25% soft; argv-only subprocesses (no shell), @git:N validated u32 [1,10] | unit | `cargo test -p ironhermes-agent --lib context_refs::tests && grep -nE 'sh -c\|/bin/sh\|Command::new\("sh"\)\|Command::new\("bash"\)' crates/ironhermes-agent/src/context_refs.rs ; test $? -eq 1` | ✅ | ⬜ pending |
| 34b-01-03 | 01 | 1 | centralization guard (preprocessing in run_turn, not surfaces) | T-34b-01-SSRF, T-34b-01-SHELL | No silent drop on LLM failure; centralized @-ref preprocessing | integration | `cargo build --workspace && cargo test -p ironhermes-agent --test invariants_34b && test $(grep -c preprocess_context_references_async crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/state.rs \| paste -sd+ - \| bc) -eq 0` (sum must equal 0 — no per-surface calls; centralized in run_turn per D-09) | ✅ | ⬜ pending |
| 34b-02-01 | 02 | 2 | ContextEngine hooks + counter reset | T-34b-02-COMPAT, T-34b-02-RESET | No counter bleed across /reset (all fields zero) | unit | `cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests::test_context_compressor_reset_zeroes_counter pressure_warning::tests summarizing_engine::tests` | ✅ | ⬜ pending |
| 34b-02-02 | 02 | 2 | Memory-authority reminder | T-34b-02-DRIFT | Header contains MEMORY.md + ALWAYS authoritative | unit | `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header` | ✅ | ⬜ pending |
| 34b-02-03 | 02 | 2 | central per-turn hooks (run_turn) + surface session-reset wiring | T-34b-02-RESET | update_from_response + update_model centralized in run_turn; CLI /new resets compression_count; reset_web_session stub + tracing log | integration | `cargo build --workspace && cargo test -p ironhermes-agent --test invariants_34b && test $(grep -c update_model crates/ironhermes-agent/src/agent_runtime.rs) -ge 1` | ✅ | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements (created by plan 34b-00 — NOT yet on disk)

- [ ] `crates/ironhermes-agent/src/context_refs.rs` — new module stub with `#[cfg(test)] mod tests {}` (Plan 34b-00 Task 1)
- [ ] `crates/ironhermes-agent/tests/invariants_34b.rs` — integration test stub with `#[ignore]` placeholder (Plan 34b-00 Task 2)
- [ ] `crates/ironhermes-agent/src/context_compressor.rs` — `#[ignore]` placeholder `test_context_compressor_reset_zeroes_counter` (Plan 34b-00 Task 3)
- [ ] `crates/ironhermes-agent/src/summarizing_engine.rs` — `#[ignore]` placeholder `test_memory_authority_header` (Plan 34b-00 Task 3)

*Existing `cargo test` infrastructure covers all other phase requirements.*

---

## Wave / Plan Ordering

```
Wave 0: 34b-00-PLAN.md (test scaffolds)
   ↓
Wave 1: 34b-01-PLAN.md (context_refs module + CENTRAL run_turn @-ref preprocessing, D-09/D-11)
   ↓
Wave 2: 34b-02-PLAN.md (ContextEngine hooks + memory-authority reminder + CENTRAL run_turn per-turn hooks + surface session-reset wiring, D-09/D-10)
```

Wave 1 depends on Wave 0 because Plan 34b-01 Task 1 mutates `context_refs.rs` from a stub into the full implementation. Wave 2 depends on Wave 1 (both touch `agent_runtime.rs` — file-overlap forces sequencing). Plan 34b-00 itself has `depends_on: []`.

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
| `on_session_reset` lifecycle in web UI | D-07 | Web UI new-chat reset trigger does not exist yet (RESEARCH Open Q resolved: accepted scope = documented `reset_web_session` stub) | Verify the `reset_web_session` stub exists and logs; full lifecycle verification deferred until a web new-chat trigger lands |

---

## Validation Sign-Off

- [x] All tasks have `<automated>` verify or Wave 0 dependencies
- [x] Sampling continuity: no 3 consecutive tasks without automated verify
- [x] Wave 0 covers all MISSING references (delivered by 34b-00-PLAN.md)
- [x] No watch-mode flags
- [x] Feedback latency < 20s
- [x] `nyquist_compliant: true` set in frontmatter
- [x] Row 34b-01-03 reconciled to centralization guard (sum 0, not 3) per D-09
- [x] Row 34b-02-03 uses invariants_34b (no phantom lifecycle_hooks_wired test)

**Approval:** ready
