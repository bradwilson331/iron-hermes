---
phase: 18-context-compression
verified: 2026-04-14T00:00:00Z
updated: 2026-04-14T18:00:00Z
status: human_needed
score: 5/5 roadmap success criteria structurally verified; 7/9 UAT tests pass (1 deferred, 1 skipped)
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 5/5 roadmap SCs structural; 6 UAT tests unblocked pending live re-run
  gaps_closed:
    - "UAT Test 4 (pressure warning CLI path): 18-13 decoupled session_id/tracker from hook registry; live UAT 2026-04-14T16:12 confirmed WARN fires with session_id under hooks=None — Test 4 flipped to pass"
    - "UAT Test 5 (tool-pair atomicity live): 18-10/18-11 shipped tool-pair guard + compute_effective_protect_first_n; live UAT 2026-04-14T01:05 verified zero pair_atomicity_collapsed_range warns across 10/10 compressions — Test 5 pass"
    - "UAT Test 6 (single pinned [CONTEXT HISTORY]): 18-07 sentinel pattern confirmed live 2026-04-13T23:44 — stable at messages=3 across 10 passes — Test 6 pass"
    - "UAT Test 7 (aux-model fallback): live UAT 2026-04-14 with misconfigured compression role — fallback to LocalPruning succeeded — Test 7 pass"
    - "UAT Test 8 (memory flush before prune): live UAT 2026-04-14 with memory-sqlite — sync_turn before destructive prune confirmed — Test 8 pass"
    - "D-24 hysteresis-across-turns gap (18-14): PressureTracker hoisted to CLI REPL session scope; compression_count carryover via Arc<AtomicUsize>; integration test pressure_tracker_hysteresis_survives_across_repl_turns proves single-WARN + transient-to-turn-N+1 contract"
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "Plan 18-14 Task 5 — Live REPL hysteresis re-run (post-merge confirmation)"
    expected: "3 consecutive CLI prompts in the same session, all landing in the pressure band [0.0425, 0.05) after first crossing: exactly one 'WARN context pressure warning' total across all 3 turns; turn 2's outbound message vector contains a system message with body starting '[CONTEXT PRESSURE HIGH'; compression_count in 'summarizing_engine: compressed compression_count=N' increments monotonically (1, 2, 3) instead of resetting to 1 each turn."
    why_human: "Task 5 was intentionally deferred per orchestrator directive at 18-14 ship time. The unit-level REPL harness (pressure_tracker_hysteresis_survives_across_repl_turns) proves the contract end-to-end in memory; this live re-run is post-merge confirmation of the observable CLI behavior, not a blocker."
  - test: "UAT Test 3 — Gateway per-turn compression at 85% threshold (live Telegram session)"
    expected: "With gateway.compression_threshold=0.85, send a turn with a prompt whose token estimate exceeds 85% of context_length. Gateway-side compression runs (per-turn hygiene log), upstream request still succeeds. Below 85%, no compression runs."
    why_human: "Structurally verified via runner_attaches_gateway_engine_from_config test and code review of build_gateway_handler. Requires live gateway + Telegram adapter + provider to confirm the full per-turn path. No live Telegram session was run during this phase."
---

# Phase 18: Context Compression Verification Report

**Phase Goal:** The agent manages context window pressure through dual-mode compression that preserves tool pairs and protects critical message boundaries.

**Verified:** 2026-04-14 (updated after 18-13 + 18-14 landed)
**Status:** human_needed — all structural wiring and automated tests pass; two live re-runs deferred (Plan 18-14 Task 5 post-merge confirmation; UAT Test 3 gateway live run)
**Re-verification:** Yes — sweep 3, after plans 18-10 through 18-14 merged onto develop.

---

## Goal Achievement

### ROADMAP Success Criteria (Observable Truths)

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Pressure warning at 85% of threshold; agent at 50%, gateway at 85% | VERIFIED | `pressure_warning.rs` PressureTracker three-channel; wired via `attach_context_engine` (agent_wiring.rs) and `build_gateway_handler` (runner.rs:121-131); thresholds read from config. 18-13 decoupled session_id/tracker from hook registry so CLI path fires without hooks=None regression. Live UAT 2026-04-14T16:12 confirmed. |
| 2 | Compression never splits tool_call/tool_result pair | VERIFIED | `tool_pair.rs` + `compute_effective_protect_first_n` (18-11) adaptive auto-shrink; atomicity invariant checks in context_compressor; live UAT 2026-04-14T01:05 shows zero pair_atomicity_collapsed_range warns across 10 consecutive compressions. |
| 3 | Protects first N + last N; iterative re-compression updates prior summary | VERIFIED | `context_compressor.rs` protect_first_n/protect_last_tokens; `summarizing_engine.rs` [CONTEXT HISTORY] sentinel pattern with COMPLETED_TOOLS_SENTINEL (18-12); live UAT 2026-04-13T23:44 shows exactly one pinned [CONTEXT HISTORY] stable across 10 passes. |
| 4 | Memory flushed before compression | VERIFIED | `memory_flush_handler.rs` subscribes to `context:pre_compress`; hook fires before destructive prune. Live UAT 2026-04-14 with memory-sqlite confirmed ordering. |
| 5 | ContextEngine trait is pluggable | VERIFIED | `context_engine.rs` trait + `engine_factory::build_context_engine` + two impls (LocalPruning, Summarizing); selected via `agent.context_engine` / `gateway.context_engine` config. Aux-model fallback live-verified 2026-04-14. |

**Score:** 5/5 roadmap success criteria verified.

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/context_engine.rs` | ContextEngine trait; LocalPruningEngine with independent with_session_id / with_hooks | VERIFIED | Present; 18-13 split confirmed by `with_session_id` at 15 grep hits |
| `crates/ironhermes-agent/src/engine_factory.rs` | build_context_engine; three independent builder branches | VERIFIED | Combined `if let (Some(h), Some(t))` guard removed by 18-13; zero hits in grep |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | SummarizingEngine; with_session_id builder; COMPLETED_TOOLS_SENTINEL | VERIFIED | Present; 18-12 sentinel + 18-13 builder split confirmed |
| `crates/ironhermes-agent/src/context_compressor.rs` | LocalPruningEngine body | VERIFIED | Present |
| `crates/ironhermes-agent/src/pressure_warning.rs` | PressureTracker three-channel; warn_count field + accessor | VERIFIED | warn_count field + was_warned + warn_count() accessors added by 18-13/18-14; confirmed by grep |
| `crates/ironhermes-agent/src/tool_pair.rs` | Tool-pair atomicity; compute_effective_protect_first_n | VERIFIED | Present; adaptive auto-shrink logic (18-11) live-verified |
| `crates/ironhermes-agent/src/memory_flush_handler.rs` | context:pre_compress → sync_turn | VERIFIED | Present |
| `crates/ironhermes-agent/src/agent_wiring.rs` | attach_context_engine with optional Arc<PressureTracker>; pressure_tracker_hysteresis_survives_across_repl_turns test | VERIFIED | Sixth param `tracker: Option<Arc<PressureTracker>>` confirmed; integration test confirmed; `Arc::new(PressureTracker::new())` only inside unwrap_or_else fallback |
| `crates/ironhermes-agent/src/agent_loop.rs` | with_compression_count builder; compression_count_after on AgentResult | VERIFIED | with_compression_count at line 156; compression_count_after at lines 35, 399, 462, 549 — all 3 construction sites populated |
| `crates/ironhermes-agent/src/lib.rs` | pub use PressureTracker re-export | VERIFIED | CLI imports `ironhermes_agent::PressureTracker` — confirmed by main.rs:4 |
| `crates/ironhermes-cli/src/main.rs` | REPL session-scoped pressure_tracker + compression_count; passed into run_agent_turn | VERIFIED | Lines 347-352: `let pressure_tracker = Arc::new(PressureTracker::new())` + `Arc<AtomicUsize>`; cloned into run_agent_turn at both call sites (lines 436-437, 495-496) |
| `crates/ironhermes-gateway/src/runner.rs` | GatewayRunner wires gateway engine | VERIFIED | `build_gateway_handler` calls `build_context_engine` + `set_gateway_engine` (lines 89, 121, 131); 49 gateway tests pass |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|----|--------|---------|
| CLI `run_once` (one-shot) | AgentLoop engine | `attach_context_engine(..., None, None)` | WIRED | main.rs one-shot path; None/None preserves fresh-tracker behavior unchanged |
| CLI REPL `run_agent_turn` | AgentLoop engine | `attach_context_engine(..., None, Some(pressure_tracker.clone()))` | WIRED | main.rs:576; session-scoped tracker threaded in |
| CLI REPL session scope | `Arc<PressureTracker>` | constructed once at session start; reused by every run_agent_turn call | WIRED | main.rs:347-352 + clones at 436-437 and 495-496 |
| CLI REPL session scope | `compression_count` carryover | `Arc<AtomicUsize>`; seed via `with_compression_count(starting_count)`; persist via `compression_count.store(result.compression_count_after, ...)` | WIRED | main.rs:545, 549, 584 confirmed |
| Gateway `run_agent` | AgentLoop engine | `attach_context_engine(..., hooks, None)` | WIRED | handler.rs:452; None for tracker (gateway session-scope hoist out of 18-14 scope) |
| GatewayRunner::start | Handler gateway engine | `build_gateway_handler` → `set_gateway_engine` | WIRED | runner.rs:199 calls builder; lines 121-131 |
| ContextEngine | MemoryProvider | `context:pre_compress` hook → `sync_turn` | WIRED | memory_flush_handler.rs; live-verified 2026-04-14 |
| PressureTracker | tracing/hook/system-msg | Three-channel emission; session_id now set unconditionally in engine_factory | WIRED | 18-13 factory fix; 18-14 session-scope survival; live UAT Test 4 pass |

---

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|--------------|--------|--------------------|--------|
| `pressure_warning.rs` PressureTracker | `above_threshold`, `pending_transient`, `warn_count` per session_id | CLI REPL session scope `Arc<PressureTracker>` constructed once; shared across turns | Yes — hysteresis state survives across REPL turns; integration test asserts warn_count==1 and transient in turn-2 messages | FLOWING |
| `agent_loop.rs` `compression_count` | `AgentLoop::compression_count` | Seeded from `Arc<AtomicUsize>` via `with_compression_count(starting_count)` each turn; persisted back via `compression_count.store(result.compression_count_after, ...)` | Yes — monotonically increments across REPL turns rather than resetting | FLOWING |

---

### Behavioral Spot-Checks

| Behavior | Command/Test | Result | Status |
|----------|-------------|--------|--------|
| Workspace builds clean | `cargo build --workspace` | 0 errors; pre-existing ironhermes-core dead_code warnings (unrelated to phase 18) | PASS |
| Agent lib tests | `cargo test -p ironhermes-agent --lib` | 185 passed; 0 failed; 0 ignored | PASS |
| Gateway lib tests | `cargo test -p ironhermes-gateway --lib` | 49 passed; 0 failed; 1 ignored | PASS |
| REPL hysteresis integration test | `pressure_tracker_hysteresis_survives_across_repl_turns` | Turn 1: warn_count==1, was_warned==true. Turn 2: transient "[CONTEXT PRESSURE HIGH" in message vector, warn_count==1. Turn 3: warn_count==1, no second transient. | PASS |
| Caller tracker reuse | `attach_context_engine_reuses_caller_tracker` | Arc::strong_count >= 3 when Some(t) passed | PASS |
| 18-13 regression | `pressure_check_fires_when_session_id_attached_without_hooks` + summarizing mirror | Both pass; tracker fires with session_id independent of hooks | PASS |
| Existing 18-10/18-11/18-12 tests | tool-pair atomicity, summary sentinel, auto-shrink tests | All pass unchanged | PASS |
| Live UAT Test 2 — agent compression at 50% | CLI session 2026-04-13 | compression fires every turn at configured threshold; logs show context:pre_compress | PASS |
| Live UAT Test 4 — pressure warning CLI path | CLI session 4c3bda53-... 2026-04-14T16:12 | WARN context pressure warning fired with session_id populated under hooks=None | PASS |
| Live UAT Test 5 — tool-pair atomicity | CLI session 2026-04-14T01:05 | Zero pair_atomicity_collapsed_range warns; 10/10 compressions succeeded; auto-shrink visible in log | PASS |
| Live UAT Test 6 — single pinned [CONTEXT HISTORY] | CLI session 2026-04-13T23:44 | messages=3 stable across 10 passes; no second pin | PASS |
| Live UAT Test 7 — aux-model fallback | CLI session 2026-04-14 (misconfigured role) | SummarizingEngine fell back to LocalPruning; no user-visible error | PASS |
| Live UAT Test 8 — memory flush ordering | CLI session 2026-04-14 (memory-sqlite) | sync_turn before destructive prune confirmed | PASS |
| Live UAT Test 3 — gateway per-turn compression | Not run (live Telegram) | Structurally verified only | HUMAN NEEDED |
| Live UAT Task 5 (18-14) — 3-turn REPL band straddling | Not run (deferred per orchestrator) | Unit test proves contract; live observation deferred | HUMAN NEEDED |

---

### Requirements Coverage

| Requirement | Plans | Description | Status | Evidence |
|-------------|-------|-------------|--------|----------|
| PRMT-10 | 18-05, 18-06 | Pressure warnings at 85% of threshold | SATISFIED | `pressure_warning.rs` three-channel; wired via attach_context_engine; 18-13 fixed CLI path; live UAT Test 4 pass |
| PRMT-11 | 18-06, 18-09 | Dual-mode (agent 50% / gateway 85%) | SATISFIED | Config keys honored; both call sites attach engines; live UAT Test 2 pass (agent side); gateway structurally verified |
| PRMT-12 | 18-01, 18-08 | ContextEngine trait pluggability | SATISFIED | Trait + factory + two impls (LocalPruning, Summarizing); factory_unknown_engine_falls_back test |
| PRMT-13 | 18-02, 18-08, 18-09, 18-10, 18-11, 18-13, 18-14 | Tool-pair atomicity | SATISFIED | tool_pair.rs + compute_effective_protect_first_n; live UAT Test 5 pass (10/10 compressions, zero atomicity violations) |
| PRMT-14 | 18-01, 18-06, 18-07, 18-08, 18-09, 18-10, 18-11, 18-12, 18-13, 18-14 | Protect first N + last N | SATISFIED | context_compressor.rs protect_first_n/protect_last_tokens; auto-shrink via 18-11; live-verified |
| PRMT-15 | 18-03, 18-09, 18-10, 18-12 | Iterative re-compression updates prior summary | SATISFIED | [CONTEXT HISTORY] sentinel + COMPLETED_TOOLS_SENTINEL in summarizing_engine; live UAT Test 6 pass |
| PRMT-16 | 18-04, 18-06 | Memory flush before compression | SATISFIED | memory_flush_handler.rs context:pre_compress hook; live UAT Test 8 pass |

All 7 phase 18 requirements (PRMT-10 through PRMT-16) satisfied. No orphaned requirement IDs found across all 14 plans.

---

### Anti-Patterns Found

| File | Pattern | Severity | Impact |
|------|---------|----------|--------|
| `crates/ironhermes-core/src/memory_store.rs:429` | Pre-existing clippy: manual is_multiple_of usage | Info | Pre-dates 18-14; in ironhermes-core (not phase 18 scope); deferred per 18-14 SUMMARY scope boundary rule |
| `crates/ironhermes-core/src/config.rs` | Pre-existing clippy: derivable Default impl | Info | Same as above |
| `crates/ironhermes-core/src/memory_provider.rs` | Pre-existing clippy: deprecated build_memory_provider fn | Info | Introduced by commit c3b6cd7 in 18-11; outside touched crates |

No blocker anti-patterns in phase 18 artifacts. No TODO/FIXME/placeholder markers in any phase 18 source files. No stub returns in engine call paths. Touched crates (ironhermes-agent, ironhermes-cli, ironhermes-gateway) are clippy-clean for 18-14 changes.

One **warning-grade** latent gap identified by the 18-14 code review (WR-01): `pre_chat_compress` passes `prior_summary: None` in `ContextStats`. Both shipped engines discover prior_summary via `locate_history_segment` from the message vector directly, so this is not an active regression — it is a future-engine documentation gap. Not a blocker for phase 18.

One **info-grade** item (IN-01 from review): gateway `attach_context_engine` still passes `None` for tracker, meaning D-24 hysteresis-across-turns will also affect Telegram multi-turn sessions. This is explicitly out of scope for 18-14 and documented as a future gap-closure plan.

---

### Gap Closure History

#### Sweep 1 (pre-18-08/18-09)
- Gap A: AgentLoop::with_context_engine never called in production → closed by 18-09 (`attach_context_engine` helper at 3 production sites).
- Gap B: Handler::set_gateway_engine never called from GatewayRunner::start → closed by 18-08 (`build_gateway_handler` calls `build_context_engine` + `set_gateway_engine`).

#### Sweep 2 (after 18-08/18-09, before 18-10 through 18-14)
- Status: `human_needed` (structural wiring complete; live UAT required).
- Human verification block listed Tests 2–9.

#### Sweep 3 (this verification, after 18-10 through 18-14)
- UAT Tests 2, 4, 5, 6, 7, 8: live-verified pass.
- UAT Test 4 edge case (D-24 hysteresis-across-turns): closed by 18-13 (session_id decoupled from hooks) + 18-14 (tracker hoisted to REPL session scope, compression_count carryover). Integration test `pressure_tracker_hysteresis_survives_across_repl_turns` proves the full contract end-to-end.
- Remaining human items: Plan 18-14 Task 5 post-merge live CLI confirmation + UAT Test 3 live gateway run (both post-merge, neither blocking phase closure).

---

### Human Verification Required

#### 1. Plan 18-14 Task 5 — Live REPL Hysteresis Confirmation (Post-Merge)

**Test:** After merging develop, run `cargo run -p ironhermes-cli` with `agent.compression_threshold=0.05`. Send 3 consecutive prompts in the same session where the token ratio stays in the band `[0.0425, 0.05)` after the first crossing without descending below `0.0425`.

**Expected:**
- Exactly one `WARN context pressure warning` log line total across all 3 turns (not one per turn).
- Turn 2's outbound message vector contains a system message whose body starts with `[CONTEXT PRESSURE HIGH — earlier history may soon be summarized]`.
- `summarizing_engine: compressed compression_count=N` increments monotonically (1, 2, 3...) instead of resetting to 1 every turn.

**Why human:** Task 5 was deferred per orchestrator directive at 18-14 ship time. The unit-level REPL harness (`pressure_tracker_hysteresis_survives_across_repl_turns`) proves the contract end-to-end in memory and is the authoritative coverage for phase closure. This live run is a post-merge observability confirmation, not a blocker.

#### 2. UAT Test 3 — Gateway Per-Turn Compression (Live Telegram)

**Test:** With `gateway.compression_threshold=0.85`, send a turn through the live Telegram gateway whose token estimate exceeds 85% of context_length. Observe gateway-side compression fires (per-turn hygiene log); confirm upstream request still succeeds.

**Expected:** Gateway per-turn hygiene log visible; main LLM response returns; no 400/5xx errors from provider.

**Why human:** The gateway engine wiring is structurally verified (`runner_attaches_gateway_engine_from_config` test; code review confirmed `build_gateway_handler` attaches the engine at lines 121-131). A live Telegram session was not run during this phase. Requires live gateway + Telegram adapter + provider.

---

### Deferred Items

The gateway session-scope hoist (D-24 for Telegram multi-turn) is explicitly out of scope for 18-14 and documented in the 18-14 review as IN-01. This is not a phase 18 gap — the phase goal is about CLI + structural gateway wiring, both of which are complete.

---

_Verified: 2026-04-14 (sweep 3, after 18-10..18-14)_
_Verifier: Claude (gsd-verifier)_
