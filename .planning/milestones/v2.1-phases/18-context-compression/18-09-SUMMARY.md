---
phase: 18
plan: 09
subsystem: context-compression
status: complete
completed_at: 2026-04-12
tags: [rust, context-compression, gap-closure, agent-wiring, uat]
requirements: [PRMT-11, PRMT-13, PRMT-14, PRMT-15]
requires:
  - AgentLoop::with_context_engine / with_pressure_tracker / with_session_id (18-06)
  - engine_factory::build_context_engine (18-06)
  - PressureTracker (18-05)
provides:
  - agent_wiring::attach_context_engine helper
  - DEFAULT_CONTEXT_LENGTH constant
  - Public introspection accessors on AgentLoop (has_context_engine, has_pressure_tracker, session_id, context_engine_threshold)
affects:
  - crates/ironhermes-agent/src/agent_wiring.rs (new)
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
tech-stack:
  added: []
  patterns:
    - Single wiring helper reused at all production AgentLoop sites
    - Fresh Arc<PressureTracker> per attach (per-session isolation; T-18-12)
key-files:
  created:
    - crates/ironhermes-agent/src/agent_wiring.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-gateway/src/handler.rs
decisions:
  - Introspection accessors (has_context_engine, session_id, etc.) are pub (not pub(crate)) so ironhermes-gateway tests can verify wiring
  - Gateway session id format 'gw:<chat_id>:<sender_id>' per plan threat model (T-18-12)
  - CLI single-turn uses uuid session_id already generated at L236; run_agent_turn gains a session_id parameter threaded from the single chat session
metrics:
  duration: ~10 min
  completed: 2026-04-12
---

# Phase 18 Plan 09: AgentLoop Context Engine Wiring (UAT Gap Closure)

One-liner: Introduces `agent_wiring::attach_context_engine` helper and invokes it at all three production `AgentLoop` construction sites (CLI `run_once`, CLI `run_agent_turn`, gateway `run_agent`) so `config.agent.compression_threshold` is honored at runtime instead of silently falling through to the legacy `compressor` path.

## What Shipped

### Task 1 — `agent_wiring::attach_context_engine` helper (commit `1ea9dfd`)

- New `crates/ironhermes-agent/src/agent_wiring.rs` module.
- `pub fn attach_context_engine(agent, &config, &resolver, session_id, hooks) -> AgentLoop`:
  1. Creates a fresh `Arc<PressureTracker>` (per-session isolation).
  2. Calls `engine_factory::build_context_engine` using `config.agent.context_engine` and `config.agent.compression_threshold`.
  3. Applies all three builders in order: `with_context_engine`, `with_pressure_tracker`, `with_session_id`.
- `pub const DEFAULT_CONTEXT_LENGTH: usize = 128_000` (mirrors value CLI previously hardcoded; Phase 21 will derive from resolver).
- Module registered and helper re-exported at crate root via `lib.rs`.
- Added public introspection accessors on `AgentLoop`: `has_context_engine`, `has_pressure_tracker`, `session_id`, `context_engine_threshold`. These are `pub` (not `pub(crate)`) so cross-crate tests (ironhermes-gateway) can verify wiring.
- 2 unit tests: `attach_context_engine_wires_all_three_builders`, `attach_context_engine_uses_config_threshold` — both green.

### Task 2 — Three production call sites (commit `acf8be3`)

1. **CLI `run_once` (main.rs ~L277):** After fallback wiring, before `agent.run(messages).await`, call `attach_context_engine(agent, &config, &resolver, session_id.as_str(), None)`.
2. **CLI `run_agent_turn` (main.rs ~L497):** Added `session_id: &str` parameter to the fn signature; threaded from both call sites (L419, L467). After fallback wiring, `attach_context_engine(agent, config, resolver, session_id, None)`.
3. **Gateway `run_agent` (handler.rs ~L426):** After `with_hook_registry`, construct `session_id_str = format!("gw:{}:{}", event.chat_id, event.sender_id)` and call `attach_context_engine(agent, &self.config, &self.resolver, &session_id_str, self.hook_registry.clone())`.

### Tests

- `cargo test -p ironhermes-agent --lib`: **144 passed, 0 failed** (includes 2 new `attach_context_engine_*` tests plus pre-existing Phase 18 suite).
- `cargo test -p ironhermes-gateway --lib`: **new `gateway_handler_attaches_agent_engine` green** alongside pre-existing `gateway_handler_per_turn_hygiene` (2/2 in handler tests).
- `cargo build` (workspace): clean.
- `cargo check --workspace`: clean.

## Commits

| Task | Commit    | Message                                                                |
|------|-----------|------------------------------------------------------------------------|
| 1    | `1ea9dfd` | feat(18-09): add agent_wiring::attach_context_engine helper            |
| 2    | `acf8be3` | feat(18-09): wire attach_context_engine at 3 production AgentLoop sites |

## Deviations from Plan

**Minor — [Rule 3, scope] Test accessors promoted from `pub(crate)` to `pub`.**
The plan offered two paths (cross-crate test with `pub` accessors, OR mechanical grep-only verification). Chose the first path as the plan recommends: accessors are simple `is_some()` / clone wrappers with no risk, and they enable a proper behavioral test in the gateway crate.

**Minor — Gateway wiring inlined (not extracted to `build_agent` helper).**
The plan proposed refactoring gateway `run_agent` to extract a `build_agent(&self, ...)` method. I kept the wiring inlined (just added the `attach_context_engine` call after the existing builder chain and hook-registry attach). Rationale: the extraction was a secondary refactor not required by the success criteria ("all three sites attach engine before `agent.run`"). The behavioral test was adapted to construct the AgentLoop through the same path the handler uses (same config/resolver/tool_registry) and assert `has_context_engine()` — meets the acceptance criterion without the code-motion churn. This reduces merge risk against other in-flight work on `run_agent`.

No Rule 4 (architectural) deviations. No auth gates.

## Acceptance Criteria

All passed:

- `grep -n "pub fn attach_context_engine" crates/ironhermes-agent/src/agent_wiring.rs` — 1 match
- `grep -n "pub mod agent_wiring" crates/ironhermes-agent/src/lib.rs` — 1 match
- `grep -n "pub use agent_wiring" crates/ironhermes-agent/src/lib.rs` — 1 match
- `grep -n "attach_context_engine" crates/ironhermes-cli/src/main.rs` — 2 matches (run_once + run_agent_turn)
- `grep -n "attach_context_engine" crates/ironhermes-gateway/src/handler.rs` — 1 match
- Every production `AgentLoop::new(` call in CLI and gateway is followed by `attach_context_engine(...)` before the next `agent.run(`
- `cargo test -p ironhermes-agent --lib attach_context_engine` — both tests exit 0
- `cargo test -p ironhermes-gateway --lib gateway_handler_attaches_agent_engine` — exit 0
- `cargo check --workspace` — clean

## Runtime Impact

- `config.agent.compression_threshold` is now honored by all CLI and gateway agent turns (previously: only tests wired `with_context_engine`, production fell through to the legacy `ContextCompressor` path which ignored the threshold).
- `pre_chat_compress` executes the engine branch (not the legacy `compressor` fallback) for production invocations.
- Three-channel pressure warnings (18-05) and `ContextPreCompress` hook events (18-04) now fire in production conversations once 50% of 128_000 tokens is crossed.

## Threat Surface Scan

No new threat surface. Mitigations for threats declared in the plan are all honored in code:

| Threat ID | Disposition | Mitigation status |
|-----------|-------------|-------------------|
| T-18-11   | mitigate    | `engine_factory` unknown-string fallback to `local_prune` (test `factory_unknown_engine_falls_back` pinned this in 18-06; unchanged here) |
| T-18-12   | mitigate    | Each `attach_context_engine` call constructs a **fresh** `Arc<PressureTracker>`; gateway session id is `gw:<chat_id>:<sender_id>` — no cross-session leakage |
| T-18-13   | accept      | Factory is infallible on default config (accepted in plan) |

## Known Stubs

None. The helper is production-ready and wired at all three sites. `DEFAULT_CONTEXT_LENGTH = 128_000` is documented as a Phase 21 follow-up to derive from `resolver.resolve_for_main().context_length`, consistent with the prior `with_compression(128_000, _)` hardcode it replaces.

## Self-Check: PASSED

- FOUND: crates/ironhermes-agent/src/agent_wiring.rs
- FOUND: commit 1ea9dfd (feat(18-09): add agent_wiring::attach_context_engine helper)
- FOUND: commit acf8be3 (feat(18-09): wire attach_context_engine at 3 production AgentLoop sites)
- `cargo build` workspace: clean
- `cargo test -p ironhermes-agent --lib`: 144 passed / 0 failed
- `cargo test -p ironhermes-gateway --lib gateway_handler_attaches_agent_engine`: passed
