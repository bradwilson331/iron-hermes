---
phase: 20
slug: memory-provider-plugin-contract
status: draft
nyquist_compliant: false
wave_0_complete: false
created: 2026-04-16
---

# Phase 20 — Validation Strategy

> Per-phase validation contract for feedback sampling during execution.

---

## Test Infrastructure

| Property | Value |
|----------|-------|
| **Framework** | Cargo test runner (Rust built-in `#[test]` / `#[tokio::test]`) |
| **Config file** | `Cargo.toml` workspace — no per-crate test config |
| **Quick run command** | `cargo test -p ironhermes-core memory_provider --lib` |
| **Full suite command** | `cargo test --workspace --all-features` |
| **Estimated runtime** | ~120 seconds (workspace full suite); ~8 seconds (per-crate quick) |

---

## Sampling Rate

- **After every task commit:** Run `cargo check --workspace --all-features` + `cargo test -p <edited crate>` for touched crate
- **After every plan wave:** Run `cargo test --workspace --all-features` + `cargo clippy --workspace --all-features -- -D warnings`
- **Before `/gsd-verify-work`:** Full suite must be green + manual UAT (gateway + chat-mode both persist memory across restart)
- **Max feedback latency:** 15 seconds (quick), 120 seconds (full)

---

## Per-Task Verification Map

| Task ID | Plan | Wave | Requirement | Threat Ref | Secure Behavior | Test Type | Automated Command | File Exists | Status |
|---------|------|------|-------------|------------|-----------------|-----------|-------------------|-------------|--------|
| 20-01-01 | 01 | 1 | MEM-07 | — | Trait methods compile; defaults behave correctly | unit | `cargo test -p ironhermes-core memory_provider::tests` | ❌ W0 | ⬜ pending |
| 20-01-02 | 01 | 1 | MEM-07 | T-20-01 | `initialize(session_id, hermes_home, &Value)` rejects path traversal in `hermes_home`; factory invokes once | unit | `cargo test -p ironhermes-agent memory::factory::tests::factory_calls_initialize` | ❌ W0 | ⬜ pending |
| 20-01-03 | 01 | 1 | MEM-08 | — | File `MemoryStore` implements all new trait methods | unit | `cargo test -p ironhermes-core memory_store::tests` | ✅ (extend) | ⬜ pending |
| 20-01-04 | 01 | 1 | MEM-10 | — | Grafeo provider passes migration + existing tests | unit | `cargo test -p memory-grafeo` | ✅ | ⬜ pending |
| 20-01-05 | 01 | 1 | MEM-11 | — | DuckDB provider passes migration + existing tests | unit | `cargo test -p memory-duckdb` | ✅ | ⬜ pending |
| 20-01-06 | 01 | 2 | MEM-07 / D-24 | — | Factory persistence round-trip for sqlite, duckdb, grafeo (the Fix 1 regression test) | integration | `cargo test -p ironhermes-agent --features memory-sqlite memory::factory::tests::sqlite_round_trip_via_factory` (and duckdb/grafeo variants) | ❌ W0 | ⬜ pending |
| 20-02-01 | 02 | 1 | MEM-12 | — | `MemoryManager::new(primary, None)` + `new(primary, Some(mirror))` compile; single-primary invariant enforced | unit | `cargo test -p ironhermes-agent memory::manager::tests::construction` | ❌ W0 | ⬜ pending |
| 20-02-02 | 02 | 1 | MEM-12 / D-29 | T-20-02 | Mirror observes each write (add/replace/remove) with correct action/target/content | unit | `cargo test -p ironhermes-agent memory::manager::tests::mirror_observes_writes` | ❌ W0 | ⬜ pending |
| 20-02-03 | 02 | 1 | MEM-12 / D-29 | T-20-02 | Failing mirror does not propagate error to primary write path (logged, swallowed) | unit | `cargo test -p ironhermes-agent memory::manager::tests::mirror_failure_does_not_block_primary` | ❌ W0 | ⬜ pending |
| 20-02-04 | 02 | 2 | MEM-07 / D-22 | — | Agent loop fires hooks in correct order: initialize → prefetch → sync_turn → queue_prefetch → on_pre_compress → on_memory_write → on_session_end → shutdown | integration | `cargo test -p ironhermes-agent agent_loop::tests::hook_ordering` | ❌ W0 | ⬜ pending |
| 20-02-05 | 02 | 2 | MEM-07 | — | `prompt_builder::load_memory` appends `system_prompt_block` after target-scoped blocks | unit | `cargo test -p ironhermes-agent prompt_builder::tests::system_prompt_block_appended` | ❌ W0 | ⬜ pending |
| 20-03-01 | 03 | 1 | MEM-07 / D-23 | T-20-03 | `hermes memory setup` scripted-stdin round-trip; `.env` append-only; no overwriting existing keys; shell-unsafe characters quoted | integration | `cargo test -p ironhermes-cli memory_setup::tests::scripted_wizard_round_trip` | ❌ W0 | ⬜ pending |
| 20-03-02 | 03 | 1 | — | T-20-03 | Wizard does not prompt for optional fields with defaults (D-08) | integration | `cargo test -p ironhermes-cli memory_setup::tests::optional_defaults_skipped` | ❌ W0 | ⬜ pending |
| 20-03-03 | 03 | 2 | — | — | `run_chat` + `run_single` wire `MemoryManager`; `prompt_builder.set_memory_store` is called; memory persists across invocations (Fix 2) | integration | `cargo test -p ironhermes-cli run_chat::tests::memory_persists_across_invocations` | ❌ W0 | ⬜ pending |
| 20-04-01 | 04 | 1 | MEM-08 | — | File provider exposes `name = "file"`, `get_config_schema` returns memory-dir + char limits | unit | `cargo test -p ironhermes-core memory_store::tests::config_schema` | ❌ W0 | ⬜ pending |
| 20-04-02 | 04 | 1 | MEM-09 | — | SQLite provider exposes `name = "sqlite"`, `get_config_schema` returns DB path | unit | `cargo test -p memory-sqlite tests::config_schema` | ❌ W0 | ⬜ pending |
| 20-04-03 | 04 | 1 | MEM-11 | — | DuckDB provider exposes `name = "duckdb"`, `get_config_schema` returns DB path + thread count | unit | `cargo test -p memory-duckdb tests::config_schema` | ❌ W0 | ⬜ pending |
| 20-04-04 | 04 | 1 | MEM-10 | — | Grafeo provider exposes `name = "grafeo"`, `get_config_schema` returns graph dir | unit | `cargo test -p memory-grafeo tests::config_schema` | ❌ W0 | ⬜ pending |
| 20-04-05 | 04 | 2 | MEM-12 / D-29 | — | One provider (sqlite fixture) demonstrates `on_memory_write` mirror behavior end-to-end | integration | `cargo test -p ironhermes-agent memory::manager::tests::sqlite_mirror_fixture` | ❌ W0 | ⬜ pending |

*Status: ⬜ pending · ✅ green · ❌ red · ⚠️ flaky*

---

## Wave 0 Requirements

- [ ] `crates/ironhermes-core/src/config_schema.rs` — new module (`ConfigField`, `MemoryAction`) + inline unit tests for serialization
- [ ] `crates/ironhermes-core/tests/memory_provider_contract.rs` — MockMemoryProvider with invocation recorder (D-22 hook ordering)
- [ ] `crates/ironhermes-agent/src/memory/manager.rs` — MemoryManager module + tests (D-25, D-29)
- [ ] `crates/ironhermes-agent/src/memory/factory.rs` — extend existing test module with `load_from_disk` round-trip + `is_available` fallback tests (D-16, D-17, D-24)
- [ ] `crates/ironhermes-agent/src/agent_loop.rs` — extend tests with hook-ordering recorder test (D-22)
- [ ] `crates/ironhermes-cli/src/memory_setup.rs` — new module with wizard + scripted-stdin integration test (D-23)
- [ ] `crates/ironhermes-cli/src/main.rs` — extend `run_chat` / `run_single` + regression test for Fix 2 (chat-mode memory wiring)

**Framework install:** none — Cargo + tokio-test are already available.

---

## Manual-Only Verifications

| Behavior | Requirement | Why Manual | Test Instructions |
|----------|-------------|------------|-------------------|
| Gateway memory persists across restart | MEM-07 (Fix 1) | Requires running gateway process, sending real messages, restarting, observing memory retained | 1) Start gateway with sqlite provider. 2) Add memory via `memory_add` tool from a Telegram turn. 3) SIGTERM gateway. 4) Restart. 5) Verify `format_for_system_prompt` includes the entry. |
| Chat-mode memory persists across invocations | MEM-07 (Fix 2) | Requires running `hermes chat` CLI, exiting, re-running, observing memory retained | 1) `hermes chat --memory sqlite`. 2) Ask agent to add a memory. 3) `/exit`. 4) `hermes chat` again. 5) Verify recalled in first-turn system prompt. |
| Setup wizard produces working config | D-23 | Interactive; scripted test covers logic but final UX requires a real terminal | 1) `hermes memory setup`. 2) Select sqlite. 3) Complete prompts. 4) Start agent and verify memory works. |

---

## Validation Sign-Off

- [ ] All tasks have `<automated>` verify or Wave 0 dependencies
- [ ] Sampling continuity: no 3 consecutive tasks without automated verify
- [ ] Wave 0 covers all MISSING references
- [ ] No watch-mode flags
- [ ] Feedback latency < 120s
- [ ] `nyquist_compliant: true` set in frontmatter

**Approval:** pending
