---
phase: 18-context-compression
verified: 2026-04-16T00:00:00Z
status: human_needed
score: 9/9 must-haves verified (5 phase-goal SCs + 4 gap-closure truths)
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 5/5 roadmap SCs structural; 7/9 UAT tests pass (1 deferred, 1 skipped)
  gaps_closed:
    - "GAP-18-UAT-02: run_gateway now calls ironhermes_agent::memory::factory::build_memory_provider; deprecated ironhermes_core::build_memory_provider deleted; factory regression tests added"
    - "UAT Test 1 (18-14 Task 5 Live REPL hysteresis): passed 2026-04-16 per 18-HUMAN-UAT.md"
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "UAT Test 3 — Gateway per-turn compression (live Telegram)"
    expected: "With gateway.compression_threshold=0.85, send a turn through the live Telegram gateway whose token estimate exceeds 85% of context_length. Gateway-side compression runs (per-turn hygiene log), upstream request still succeeds. Below 85%, no compression runs."
    why_human: "The gateway engine wiring is structurally verified (runner_attaches_gateway_engine_from_config test; code review confirmed build_gateway_handler attaches the engine at lines 121-131). A live Telegram session was not run during this phase. Requires live gateway + Telegram adapter + provider."
  - test: "UAT Test 2 re-run — Gateway boots with memory.provider=sqlite under --features memory-sqlite (live Telegram)"
    expected: "cargo run -p ironhermes-cli --features memory-sqlite -- gateway --token $TELEGRAM_TOKEN with config.memory.provider=sqlite: gateway boots without error, memory tool uses SqliteMemoryProvider, multi-turn Telegram session works normally. The old 'Memory provider sqlite requires a feature flag that is not enabled. Available providers: file' error must NOT appear."
    why_human: "Plan 18-15 migrated run_gateway to the feature-gated agent factory and the fix is statically verified (factory route confirmed, build passes, regression tests pass at 189/189). However the original GAP-18-UAT-02 was discovered during a live Telegram gateway run. The gap closure must be confirmed by re-running the exact original scenario: live gateway with a real Telegram token and memory.provider=sqlite."
---

# Phase 18: Context Compression Verification Report

**Phase Goal:** The agent manages context window pressure through dual-mode compression that preserves tool pairs and protects critical message boundaries.

**Verified:** 2026-04-16 (sweep 4, after plan 18-15 gap closure)
**Status:** human_needed — all structural wiring, automated tests, and gap-closure truths pass; two live Telegram runs remain for human re-verification (UAT Test 3 original deferred item + UAT Test 2 re-run after GAP-18-UAT-02 fix)
**Re-verification:** Yes — after 18-15 gap closure targeting GAP-18-UAT-02

---

## Goal Achievement

### ROADMAP Success Criteria (Observable Truths)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Pressure warning at 85% of threshold; agent at 50%, gateway at 85% | VERIFIED | `pressure_warning.rs` PressureTracker three-channel; wired via `attach_context_engine` (agent_wiring.rs) and `build_gateway_handler` (runner.rs:121-131); thresholds read from config. 18-13 decoupled session_id/tracker from hook registry. Live UAT 2026-04-14T16:12 confirmed. |
| 2 | Compression never splits tool_call/tool_result pair | VERIFIED | `tool_pair.rs` + `compute_effective_protect_first_n` (18-11) adaptive auto-shrink; atomicity invariant checks in context_compressor; live UAT 2026-04-14T01:05 shows zero pair_atomicity_collapsed_range warns across 10 consecutive compressions. |
| 3 | Protects first N + last N; iterative re-compression updates prior summary | VERIFIED | `context_compressor.rs` protect_first_n/protect_last_tokens; `summarizing_engine.rs` [CONTEXT HISTORY] sentinel pattern with COMPLETED_TOOLS_SENTINEL (18-12); live UAT 2026-04-13T23:44 shows exactly one pinned [CONTEXT HISTORY] stable across 10 passes. |
| 4 | Memory flushed before compression | VERIFIED | `memory_flush_handler.rs` subscribes to `context:pre_compress`; hook fires before destructive prune. Live UAT 2026-04-14 with memory-sqlite confirmed ordering. |
| 5 | ContextEngine trait is pluggable | VERIFIED | `context_engine.rs` trait + `engine_factory::build_context_engine` + two impls (LocalPruning, Summarizing); selected via `agent.context_engine` / `gateway.context_engine` config. Aux-model fallback live-verified 2026-04-14. |

**Score:** 5/5 roadmap success criteria verified.

---

### Gap-Closure Truths (Plan 18-15 — GAP-18-UAT-02)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Gateway with --features memory-sqlite and memory.provider=sqlite constructs SqliteMemoryProvider without bailing | VERIFIED | `crates/ironhermes-cli/src/main.rs:612-613` — `run_gateway` calls `ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?`. Factory `factory.rs:23-28` routes `#[cfg(feature = "memory-sqlite")] "sqlite"` to `SqliteMemoryProvider::new`. `cargo build --workspace --features ironhermes-cli/memory-sqlite` passes (orchestrator ground truth). |
| 2 | Without --features memory-sqlite, sqlite fails naming the flag (not "Available providers: file") | VERIFIED | `factory.rs:29-35` — `#[cfg(not(feature = "memory-sqlite"))] "sqlite"` branch bails with "Memory provider 'sqlite' requires the 'memory-sqlite' feature. Rebuild with: cargo build --features memory-sqlite". Test `sqlite_provider_without_feature_returns_err_naming_feature` asserts both `"memory-sqlite"` and `"cargo build --features memory-sqlite"` present. 189 tests pass without feature flag. |
| 3 | ironhermes-core no longer exports build_memory_provider — symbol compiler-removed | VERIFIED | `grep -rn "build_memory_provider" crates/ironhermes-core/` returns zero matches. `crates/ironhermes-core/src/lib.rs:5` imports `{ChatMessage, Config, MemoryProvider, ProviderResolver, SkillRegistry}` with no `build_memory_provider`. `memory_provider.rs` has zero occurrences. |
| 4 | ironhermes_agent::memory::factory has regression tests covering sqlite+feature (Ok) and sqlite-no-feature (Err naming flag) | VERIFIED | `factory.rs:74-136` — 4 tests: `file_provider_returns_ok`, `unknown_provider_returns_err_with_message`, `sqlite_provider_with_feature_returns_ok` (cfg(feature = "memory-sqlite")), `sqlite_provider_without_feature_returns_err_naming_feature` (cfg(not(feature = "memory-sqlite"))). Both `cargo test -p ironhermes-agent --lib` and `cargo test -p ironhermes-agent --lib --features memory-sqlite` report 189 passed (orchestrator ground truth). |

**Score:** 4/4 gap-closure truths verified. **Total: 9/9.**

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/context_engine.rs` | ContextEngine trait; LocalPruningEngine with independent with_session_id / with_hooks | VERIFIED | Present; 18-13 split confirmed |
| `crates/ironhermes-agent/src/engine_factory.rs` | build_context_engine; three independent builder branches | VERIFIED | Combined `if let (Some(h), Some(t))` guard removed by 18-13 |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | SummarizingEngine; with_session_id builder; COMPLETED_TOOLS_SENTINEL | VERIFIED | Present; 18-12 sentinel + 18-13 builder split confirmed |
| `crates/ironhermes-agent/src/context_compressor.rs` | LocalPruningEngine body | VERIFIED | Present |
| `crates/ironhermes-agent/src/pressure_warning.rs` | PressureTracker three-channel; warn_count field + accessor | VERIFIED | warn_count, was_warned, warn_count() accessors confirmed |
| `crates/ironhermes-agent/src/tool_pair.rs` | Tool-pair atomicity; compute_effective_protect_first_n | VERIFIED | Present; adaptive auto-shrink logic (18-11) live-verified |
| `crates/ironhermes-agent/src/memory_flush_handler.rs` | context:pre_compress -> sync_turn | VERIFIED | Present |
| `crates/ironhermes-agent/src/memory/factory.rs` | build_memory_provider with feature-gated sqlite/duckdb/grafeo branches; regression tests | VERIFIED | factory.rs:1-136; 4 tests confirmed; `#[cfg(feature = "memory-sqlite")]` and `#[cfg(not(feature = "memory-sqlite"))]` branches both present |
| `crates/ironhermes-core/src/memory_provider.rs` | MemoryProvider trait only — no build_memory_provider factory | VERIFIED | Zero occurrences of `build_memory_provider` in file |
| `crates/ironhermes-core/src/lib.rs` | Re-exports without build_memory_provider | VERIFIED | Zero occurrences; exports `{MemoryEntries, MemoryProvider, MemoryProviderConfig}` only |
| `crates/ironhermes-cli/src/main.rs` | run_gateway calls ironhermes_agent::memory::factory::build_memory_provider | VERIFIED | Line 613: single call site confirmed; no separate hardcoded MemoryStore::new path |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| `crates/ironhermes-cli/src/main.rs (run_gateway)` | `crates/ironhermes-agent/src/memory/factory.rs (build_memory_provider)` | `ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?` at line 613 | WIRED | Return value bound to `memory_store: Arc<Mutex<dyn MemoryProvider + Send>>` and used for `registry.register_memory_tool(memory_store.clone())` downstream |
| CLI `run_once` (one-shot) | AgentLoop engine | `attach_context_engine(..., None, None)` | WIRED | main.rs one-shot path |
| CLI REPL `run_agent_turn` | AgentLoop engine | `attach_context_engine(..., None, Some(pressure_tracker.clone()))` | WIRED | Session-scoped tracker threaded in |
| CLI REPL session scope | `Arc<PressureTracker>` | Constructed once at session start, reused each turn | WIRED | main.rs:347-352 + clones at 436-437 and 495-496 |
| Gateway `run_agent` | AgentLoop engine | `attach_context_engine(..., hooks, None)` | WIRED | handler.rs:452 |
| GatewayRunner::start | Handler gateway engine | `build_gateway_handler` -> `set_gateway_engine` | WIRED | runner.rs:199 calls builder; lines 121-131 |
| ContextEngine | MemoryProvider | `context:pre_compress` hook -> `sync_turn` | WIRED | memory_flush_handler.rs; live-verified 2026-04-14 |
| PressureTracker | tracing/hook/system-msg | Three-channel emission; session_id set unconditionally in engine_factory | WIRED | 18-13 factory fix; live UAT Test 4 pass |

---

### Behavioral Spot-Checks

| Behavior | Command/Test | Result | Status |
|----------|-------------|--------|--------|
| Workspace builds clean | `cargo build --workspace` | 0 errors; 2 pre-existing dead_code warnings in cli/batch (unrelated to phase 18) | PASS |
| Workspace builds with memory-sqlite | `cargo build --workspace --features ironhermes-cli/memory-sqlite` | 0 errors | PASS |
| Agent lib tests (no feature) | `cargo test -p ironhermes-agent --lib` | 189 passed; 0 failed | PASS |
| Agent lib tests with memory-sqlite | `cargo test -p ironhermes-agent --lib --features memory-sqlite` | 189 passed; 0 failed | PASS |
| Core lib tests | `cargo test -p ironhermes-core --lib` | 159 passed; 0 failed | PASS |
| sqlite regression test (no feature) | `sqlite_provider_without_feature_returns_err_naming_feature` | error message contains "memory-sqlite" and "cargo build --features memory-sqlite" | PASS |
| sqlite regression test (with feature) | `sqlite_provider_with_feature_returns_ok` | result.is_ok() | PASS |
| Deprecated symbol removed | `grep -rn "build_memory_provider" crates/ironhermes-core/` | zero matches | PASS |
| Single CLI call site | `grep -rn "build_memory_provider" crates/ironhermes-cli/` | exactly one match at main.rs:613 referencing agent factory | PASS |
| Live UAT Test 1 (18-14 REPL hysteresis) | CLI session 2026-04-16 (18-HUMAN-UAT.md) | pass | PASS |
| Live UAT Test 2 (gateway memory-sqlite) | Live Telegram gateway run | Blocked — GAP-18-UAT-02 fix applied statically; live re-run required | HUMAN NEEDED |
| Live UAT Test 3 (gateway per-turn compression) | Live Telegram gateway run | Structurally verified only | HUMAN NEEDED |

---

### Requirements Coverage

| Requirement | Plans | Description | Status | Evidence |
|-------------|-------|-------------|--------|----------|
| PRMT-10 | 18-05, 18-06 | Pressure warnings at 85% of threshold | SATISFIED | `pressure_warning.rs` three-channel; wired via attach_context_engine; 18-13 fixed CLI path; live UAT Test 4 pass |
| PRMT-11 | 18-06, 18-09 | Dual-mode (agent 50% / gateway 85%) | SATISFIED | Config keys honored; both call sites attach engines; live UAT Test 2 pass (agent side); gateway structurally verified |
| PRMT-12 | 18-01, 18-08 | ContextEngine trait pluggability | SATISFIED | Trait + factory + two impls (LocalPruning, Summarizing) |
| PRMT-13 | 18-02, 18-08, 18-09, 18-10, 18-11, 18-13, 18-14 | Tool-pair atomicity | SATISFIED | tool_pair.rs + compute_effective_protect_first_n; live UAT Test 5 pass (10/10, zero violations) |
| PRMT-14 | 18-01, 18-06 through 18-14 | Protect first N + last N | SATISFIED | context_compressor.rs; auto-shrink via 18-11; live-verified |
| PRMT-15 | 18-03, 18-09, 18-10, 18-12 | Iterative re-compression updates prior summary | SATISFIED | [CONTEXT HISTORY] sentinel + COMPLETED_TOOLS_SENTINEL; live UAT Test 6 pass |
| PRMT-16 | 18-04, 18-06 | Memory flush before compression | SATISFIED | memory_flush_handler.rs; live UAT Test 8 pass |
| GAP-18-UAT-02 | 18-15 | Gateway boots with memory.provider=sqlite under --features memory-sqlite | SATISFIED (static) — live re-run pending | run_gateway now calls agent factory; deprecated core symbol deleted; 189/189 regression tests pass. Live Telegram confirmation still required. |

All 7 phase 18 requirements (PRMT-10 through PRMT-16) satisfied. No orphaned requirement IDs.

---

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `crates/ironhermes-agent/src/memory/factory.rs:115` | sqlite test sets `HERMES_HOME` instead of `IRONHERMES_HOME` — env var does not redirect `get_hermes_home()` | Warning | Test passes but isolation promise is not delivered; touches real `~/.ironhermes/memory.db` each `cargo test --features memory-sqlite` run. Identified by 18.15-REVIEW.md WR-01. Does NOT invalidate Truth 4 — factory routing is correct and test proves no bail; isolation is degraded. |
| `crates/ironhermes-agent/src/memory/factory.rs:115` | sqlite test sets env var without restore — leaks process state | Warning | Future tests reading `HERMES_HOME` observe deleted tempdir path. Identified by 18.15-REVIEW.md WR-02. Low risk today. |
| `crates/ironhermes-core/src/memory_store.rs:429` | Pre-existing clippy: manual is_multiple_of usage | Info | Pre-dates phase 18; unrelated to gap closure |
| `crates/ironhermes-core/src/config.rs` | Pre-existing clippy: derivable Default impl | Info | Pre-dates phase 18; unrelated to gap closure |

No blocker anti-patterns. No TODO/FIXME/placeholder markers in any 18-15 source files. No stub returns in factory call paths.

The two warnings (WR-01, WR-02) from the code review are hygiene issues — they do not affect correctness of the factory routing fix or the build/test results. A follow-up plan may address them.

---

### Human Verification Required

#### 1. UAT Test 2 Re-run — Gateway boots with memory.provider=sqlite (live Telegram, post-GAP-18-UAT-02 fix)

**Test:** Build with `--features memory-sqlite` and run the live gateway:
```bash
cargo run -p ironhermes-cli --features memory-sqlite -- gateway --token $TELEGRAM_TOKEN
```
With `config.memory.provider = "sqlite"` in `config.yaml`.

**Expected:**
- Gateway boots cleanly without the error "Memory provider 'sqlite' requires a feature flag that is not enabled. Available providers: file"
- Memory tool uses `SqliteMemoryProvider` (log: SqliteMemoryProvider or memory.db path visible at startup)
- Multi-turn Telegram conversation works; memory tool operations (add, replace, remove) persist to `memory.db`

**Why human:** GAP-18-UAT-02 was originally discovered during a live Telegram gateway run. The fix (migrating `run_gateway` to the agent factory) is fully verified statically — `run_gateway` at `main.rs:613` calls the feature-gated factory, the factory routes correctly, deprecated core symbol is deleted, and regression tests pass at 189/189. However the original failure was a live runtime error that blocked UAT Test 2. The closure must be confirmed by re-running the exact scenario with a real Telegram token.

#### 2. UAT Test 3 — Gateway Per-Turn Compression (Live Telegram)

**Test:** With `gateway.compression_threshold=0.85`, send a turn through the live Telegram gateway whose token estimate exceeds 85% of context_length.

**Expected:** Gateway-side compression fires (per-turn hygiene log visible), upstream request still succeeds and returns a response. Below 85%, no compression runs.

**Why human:** The gateway engine wiring is structurally verified (`runner_attaches_gateway_engine_from_config` test and code review of `build_gateway_handler` at runner.rs:121-131). A live Telegram session was not run during this phase. Requires live gateway + Telegram adapter + LLM provider.

---

### Gap Closure History

#### Sweep 1 (pre-18-08/18-09)
- Gap A: AgentLoop::with_context_engine never called in production — closed by 18-09.
- Gap B: Handler::set_gateway_engine never called from GatewayRunner::start — closed by 18-08.

#### Sweep 2 (after 18-08/18-09, before 18-10 through 18-14)
- Status: `human_needed` (structural wiring complete; live UAT required).

#### Sweep 3 (after 18-10 through 18-14)
- UAT Tests 2, 4, 5, 6, 7, 8: live-verified pass.
- D-24 hysteresis-across-turns: closed by 18-13 + 18-14. Integration test `pressure_tracker_hysteresis_survives_across_repl_turns` proves contract.
- Remaining human items: Plan 18-14 Task 5 post-merge live CLI confirmation (Test 1 in 18-HUMAN-UAT.md) + UAT Test 3.

#### Sweep 4 — this verification (after 18-15 gap closure)
- GAP-18-UAT-02 closed statically: `run_gateway` migrated to agent factory, deprecated core symbol deleted, regression tests added and passing (189/189 in both feature configurations).
- UAT Test 1 (18-14 REPL hysteresis): confirmed pass per 18-HUMAN-UAT.md 2026-04-16.
- Remaining human items: UAT Test 2 re-run (live Telegram with memory-sqlite, post-fix confirmation) + UAT Test 3 (live gateway compression). Both require a live Telegram session with a real bot token.

---

_Verified: 2026-04-16 (sweep 4, after 18-15 gap closure)_
_Verifier: Claude (gsd-verifier)_
