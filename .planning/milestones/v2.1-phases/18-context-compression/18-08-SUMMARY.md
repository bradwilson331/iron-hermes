---
phase: 18
plan: 08
status: complete
completed_at: 2026-04-12
requirements: [PRMT-12, PRMT-13, PRMT-14]
gap_closure: true
closes_uat_tests: [3, 4, 5, 6, 7, 8]
files_modified:
  - crates/ironhermes-gateway/src/runner.rs
  - crates/ironhermes-gateway/src/handler.rs
tests_added: 2
commits:
  - c66cda6
---

# 18-08 — Gateway hygiene engine wired in GatewayRunner::start

## Goal
Close UAT test 3 blocker: `Handler::set_gateway_engine` was only called from
tests. `GatewayRunner::start` constructed the handler but never built a gateway
engine, so `maybe_compress_gateway` always short-circuited with `gateway_engine
= None` in production.

## Outcome
- `GatewayRunner::start` now builds the per-turn gateway hygiene engine via
  `engine_factory::build_context_engine` and attaches it through
  `handler.set_gateway_engine(engine, 128_000)` before the handler is wrapped
  in `Arc` and dispatched.
- Unknown `gateway.context_engine` values log a warn and fall back to
  `local_prune` (T-18-08 mitigation pinned by `runner_gateway_engine_respects_unknown_kind_fallback`).
- `cargo build -p ironhermes-gateway`, `cargo test -p ironhermes-gateway --lib`
  (48 pass), and `cargo check --workspace` are clean.
- UAT tests 4–8 (pressure warning, tool-pair atomicity, summarizing history,
  aux-model fallback, memory flush) are unblocked since the gateway engine now
  runs in production.

## Key changes

### Task 1 — Wire gateway hygiene engine in GatewayRunner::start (commit `c66cda6`)

**`crates/ironhermes-gateway/src/runner.rs`**:
- Added imports: `engine_factory::build_context_engine`,
  `pressure_warning::PressureTracker`, `context_engine::ContextEngine`.
- Factored handler construction out of `start()` into
  `fn build_gateway_handler(&self) -> GatewayMessageHandler` so it is
  unit-testable without a live Telegram adapter.
- `build_gateway_handler`:
  1. Constructs `GatewayMessageHandler::new(...)` and applies existing
     memory/hook/skill/active-skills setters.
  2. Builds the per-turn hygiene engine:
     ```rust
     let ctx_len: usize = 128_000;
     let tracker = Some(Arc::new(PressureTracker::new()));
     let engine: Arc<dyn ContextEngine> = build_context_engine(
         &self.config,
         &self.config.gateway.context_engine,
         &self.resolver,
         ctx_len,
         self.config.gateway.compression_threshold,
         "gateway", // D-13: per-session lineage deferred to Phase 21
         self.hook_registry.clone(),
         tracker,
     );
     handler.set_gateway_engine(engine, ctx_len);
     ```
- `start()` now reads:
  ```rust
  // --- 6. Create handler (with gateway hygiene engine wired) and queue manager ---
  let handler = self.build_gateway_handler();
  let handler = Arc::new(handler);
  ```
  Everything downstream (UserQueueManager, poll loop, cron tick) is unchanged.

**`crates/ironhermes-gateway/src/handler.rs`**:
- Added test-only accessor `#[cfg(test)] pub(crate) fn gateway_engine_is_some(&self) -> bool`
  on `GatewayMessageHandler` so runner tests can assert the engine was attached
  without exposing the private field or building oversized message vectors.

**Tests (`crates/ironhermes-gateway/src/runner.rs::tests`)**:
- `runner_attaches_gateway_engine_from_config` — config with
  `gateway.context_engine = "local_prune"` + `compression_threshold = 0.85` →
  `build_gateway_handler()` returns a handler whose `gateway_engine_is_some()`
  is true.
- `runner_gateway_engine_respects_unknown_kind_fallback` — config with
  `gateway.context_engine = "bogus_engine_kind"` → factory warns and falls
  back; handler still has an engine attached. No panic. (Pins T-18-08.)

## Decisions / Deviations
- **Context length hardcoded to 128_000** matching the CLI's existing
  `with_compression(128_000, _)` pattern. Phase 21 (Gateway Architecture
  Alignment) plumbs a resolver-derived context length end-to-end.
- **Session ID = `"gateway"`** (per-session lineage deferred to Phase 21 per D-13).
- **PressureTracker constructed fresh** per handler (one per gateway instance);
  cooldown is per-session inside the tracker.
- No deviations from Rules 1–4 — plan executed as written.

## Verification
- `cargo test -p ironhermes-gateway --lib runner_` — 3/3 (2 new + 1 pre-existing)
- `cargo test -p ironhermes-gateway --lib` — 48 pass, 1 ignored, 0 failed
- `cargo build -p ironhermes-gateway` — clean
- `cargo check --workspace` — clean
- Grep acceptance criteria:
  - `build_context_engine` — present in runner.rs
  - `set_gateway_engine` — called in `build_gateway_handler`
  - `build_gateway_handler` — defined + called once from `start()`

## Threat Model Pins
- **T-18-08 Tampering (unknown context_engine string):** mitigated by factory
  fallback (already warn-logs) and pinned by
  `runner_gateway_engine_respects_unknown_kind_fallback`.
- **T-18-09 DoS (engine attached but compression fails on live turn):**
  mitigated by `maybe_compress_gateway` logging and swallowing errors
  (handler.rs:123–127 — inherited from 18-06).

## Self-Check: PASSED
- FOUND: crates/ironhermes-gateway/src/runner.rs (build_gateway_handler, build_context_engine, set_gateway_engine)
- FOUND: crates/ironhermes-gateway/src/handler.rs (gateway_engine_is_some accessor)
- FOUND: commit c66cda6
- FOUND: runner_attaches_gateway_engine_from_config (passing)
- FOUND: runner_gateway_engine_respects_unknown_kind_fallback (passing)
