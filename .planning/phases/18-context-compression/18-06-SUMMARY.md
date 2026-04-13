---
phase: 18
plan: 06
status: complete
completed_at: 2026-04-12
requirements: [PRMT-10, PRMT-11, PRMT-13, PRMT-14, PRMT-15, PRMT-16]
files_modified:
  - crates/ironhermes-agent/src/engine_factory.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-gateway/src/handler.rs
tests_added: 8
---

# 18-06 — engine_factory + dual-mode compression wiring

## Goal
End-to-end dual-mode context compression: agent loop triggers at 50% (configurable
via `agent.compression_threshold`), gateway handler triggers at 85% (configurable
via `gateway.compression_threshold`). `engine_factory` routes config strings to
engine impls with aux-model fallback.

## Outcome
- Both call sites operational; pressure warnings fire on the pre-compression
  slope; transient warning reaches the model exactly once per crossing.
- `cargo check --workspace`, `cargo test -p ironhermes-agent`, and
  `cargo test -p ironhermes-gateway` all pass.

## Key changes

### Task 1 — engine_factory + aux-model fallback (commit `b2bf431`)
- `crates/ironhermes-agent/src/engine_factory.rs`: `build_context_engine(config,
  engine_kind, resolver, context_length, threshold, session_id, hooks, tracker)`
  returns `Arc<dyn ContextEngine>`.
  - `"local_prune"` → `LocalPruningEngine` (Hard)
  - `"summarizing"` → `SummarizingEngine` (Soft); resolves `build_role_client(&resolver, "compression")`
    with warn-log fallback to `build_main_client` (T-18-10).
  - unknown → warn-log fallback to `local_prune`.
- Registered `pub mod engine_factory;` in lib.rs.
- 4 tests: `factory_returns_local_prune_for_local_prune_string`,
  `factory_returns_summarizing_for_summarizing_string`,
  `factory_aux_model_fallback`, `factory_unknown_engine_falls_back`.

### Task 2 — agent_loop + gateway wiring
**Trait:** added `ContextEngine::check_pressure(&self, &ContextStats) -> bool`
with a no-op default. Both shipped engines override it to run
`PressureTracker::check_and_maybe_emit` without any destructive work.

**Agent loop** (`agent_loop.rs`):
- New fields: `context_engine: Option<Arc<dyn ContextEngine>>`,
  `pressure_tracker: Option<Arc<PressureTracker>>`, `session_id: Option<String>`,
  `context_length: usize`, `compression_count: usize`.
- Builders: `with_context_engine(engine, ctx_len)`, `with_pressure_tracker`,
  `with_session_id`.
- Extracted `pre_chat_compress(&mut self, &mut Vec<ChatMessage>)`:
  1. drains `PressureTracker::take_transient(session_id)` as a system message;
  2. builds `ContextStats` from current estimate;
  3. `ratio ≥ engine.threshold()` → `engine.compress`; else → `engine.check_pressure`.
- Main loop calls `pre_chat_compress` immediately before the LLM request; when
  no `context_engine` is set, legacy `compressor` path remains for back-compat.
- 3 tests in `plan_18_06_tests`: `dual_mode_thresholds`,
  `agent_loop_injects_transient_pressure_message`,
  `agent_loop_compression_before_chat`.

**Gateway handler** (`handler.rs`):
- New fields: `gateway_engine: Option<Arc<dyn ContextEngine>>`, `context_length: usize`.
- Setter: `set_gateway_engine(engine, ctx_len)`.
- `maybe_compress_gateway(&mut Vec<ChatMessage>) -> bool` runs per-turn hygiene
  when `ratio ≥ config.gateway.compression_threshold`; errors are logged and
  swallowed so a bad compression never drops a user turn.
- Called in `run_agent` after the system message is prepended and before
  `agent.run(messages)`.
- 1 test: `gateway_handler_per_turn_hygiene` (fires above 0.85, skips below).

### Deferred
- D-13 `parent_session_id` lineage on gateway message drop → Phase 21 (full
  gateway lifecycle). Marked in code comments.

## Verification
- `cargo test -p ironhermes-agent --lib engine_factory` — 4/4
- `cargo test -p ironhermes-agent --lib plan_18_06_tests` — 3/3
- `cargo test -p ironhermes-gateway --lib gateway_handler_per_turn_hygiene` — 1/1
- `cargo check --workspace` — clean
- Pre-existing Phase 11–17 suite — unaffected
