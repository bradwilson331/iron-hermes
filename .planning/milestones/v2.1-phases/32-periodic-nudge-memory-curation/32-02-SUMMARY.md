---
phase: "32"
plan: "02"
subsystem: "learning-loop"
tags: [LEARN-01, nudge, gateway, telegram, memory-curation]
requires:
  - "ironhermes_agent::nudge module (Plan 32-01)"
  - "ironhermes_core::MemoryConfig.nudge_interval / .memory_enabled (Plan 32-01)"
  - "ironhermes_gateway::GatewayMessageHandler (skill_overlays interior-mutability pattern)"
  - "AnyClient: #[derive(Clone)] (Plan 12)"
provides:
  - "GatewayMessageHandler.nudge_turns: Arc<Mutex<HashMap<SessionKey, u32>>> per-session counter"
  - "Post-turn nudge fire site in run_agent's Ok(result) arm"
  - "ironhermes_agent::nudge::should_nudge — pub(crate) counter-logic helper"
  - "3 counter-logic unit tests in nudge::tests (fires_at_interval, disabled_when_zero, counter_resets_after_fire)"
affects:
  - "crates/ironhermes-gateway/src/handler.rs (GatewayMessageHandler struct, new(), run_agent fire site)"
  - "crates/ironhermes-agent/src/nudge.rs (new should_nudge helper + 3 tests)"
tech-stack:
  added: []
  patterns:
    - "Interior-mutable Arc<std::sync::Mutex<HashMap<SessionKey, u32>>> on the handler — mirrors skill_overlays / active_personality_overlay because run_agent takes &self"
    - "std::sync::MutexGuard scoped inside a block expression that returns `should_fire: bool` — guard dropped at closing brace BEFORE any tokio::spawn / .await (clippy::await_holding_lock clean, T-32-07)"
    - "AnyClient clone before AgentLoop::new move (AnyClient is #[derive(Clone)]) — the cloned handle feeds the nudge tokio::spawn"
    - "messages.clone() snapshot BEFORE the move into agent.run(messages).await — the snapshot reflects the exact turn the model just consumed"
    - "TDD RED gate: stub returns false → 2/3 new tests fail; GREEN gate: real body → 6/6 pass"
key-files:
  created: []
  modified:
    - "crates/ironhermes-agent/src/nudge.rs"
    - "crates/ironhermes-gateway/src/handler.rs"
decisions:
  - "should_nudge is pub(crate), not pub — gateway and CLI both inline the counter shape inline (the helper is the canonical reference for the contract); pub(crate) keeps the dead_code warning local to the agent crate without spilling the API"
  - "Counter mutation lives in a block expression returning bool so the MutexGuard drop point is structurally enforced — readers do not have to trace the guard lifetime through a multi-statement body"
  - "Snapshot client + messages BEFORE the move into AgentLoop::new and agent.run() so the post-turn nudge sees the exact turn the model just consumed (snapshot, not post-tool-call state)"
  - "Fire site placed AFTER ResponseSent hook block and BEFORE the session-store write of result.appended — matches the plan's 'between info! and session store update' spec while keeping the nudge inside the Ok(result) arm (no fire on agent error)"
  - "interval == 0 with non-zero counter is a true no-op — the counter is NOT touched when disabled, so a later config flip starts from 0 rather than carrying over a stale partial total"
metrics:
  duration_minutes: 11
  completed_date: 2026-05-16
  tasks_committed: 3
  files_modified: 2
  tests_added: 3
---

# Phase 32 Plan 02: Periodic Nudge — Gateway Wiring Summary

Telegram gateway now fires the periodic memory-review nudge on the same `memory.nudge_interval` cadence as the CLI REPL — per-session counter on `GatewayMessageHandler`, fire-and-forget `tokio::spawn` so user replies are never blocked on nudge completion, and a `pub(crate) should_nudge` helper in `ironhermes-agent` makes the counter contract unit-testable independent of any AgentLoop or MemoryManager.

## Tasks Completed

| Task | Name | Commits | Files |
|------|------|---------|-------|
| 1a (RED) | Add failing tests for should_nudge | `d1299f23` | `crates/ironhermes-agent/src/nudge.rs` |
| 1b (GREEN) | Implement should_nudge counter helper | `a0db624d` | `crates/ironhermes-agent/src/nudge.rs` |
| 2 | Wire per-session nudge counter into GatewayHandler | `f24dc950` | `crates/ironhermes-gateway/src/handler.rs` |

## What Shipped

### `pub(crate) fn should_nudge(interval, counter)` in `ironhermes-agent::nudge` (Task 1)

Pure-function predicate that captures the LEARN-01 counter contract:

- `interval == 0` → returns `false`, leaves `*counter` untouched (documented disable sentinel; a later config flip starts from a clean 0 rather than carrying over a stale partial total).
- Otherwise: increments `*counter`; when `*counter >= interval`, resets `*counter` to 0 and returns `true`; else returns `false`.

`pub(crate)` keeps it visible inside the agent crate only — CLI `run_chat` and gateway `run_agent` callers live in other crates and inline the same shape (the helper is the canonical reference, not the only implementation site). This deliberately produces a single `dead_code` warning local to `ironhermes-agent` rather than spilling the helper into the crate's public API.

### 3 counter-logic unit tests in `nudge::tests` (Task 1)

Live alongside the 3 prompt-content tests from Plan 32-01:

| Test | Interval | What it asserts |
|------|----------|-----------------|
| `fires_at_interval` | 3 | Turns 1, 2 return false; turn 3 fires; counter resets to 0; turn 1 of next cycle is back to false. |
| `disabled_when_zero` | 0 | 20 consecutive calls all return false; counter stays at 0 (no side effect). |
| `counter_resets_after_fire` | 2 | Fires at turn 2, resets to 0, fires again at turn 2 of the next cycle (proves the reset is reusable, not a one-shot). |

`cargo test -p ironhermes-agent nudge::tests` now reports 6/6 pass.

### `GatewayMessageHandler.nudge_turns` field (Task 2)

```rust
nudge_turns: Arc<std::sync::Mutex<std::collections::HashMap<SessionKey, u32>>>,
```

Typed identically to `skill_overlays` and `active_personality_overlay` — same interior-mutability story, same `unwrap_or_else(|e| e.into_inner())` poison-resilience pattern. Initialized empty in `new()`; entries lazily inserted on first turn per session via `map.entry(key.clone()).or_insert(0)`.

### Post-turn fire site in `run_agent` (Task 2)

Placement: inside the `Ok(result)` arm of the `match agent_result` block, **after** the `ResponseSent` hook fire and **before** the session-store write of `result.appended` (matches the plan's "between info! and session store update" spec while keeping the nudge in the success arm — no fire on agent error).

Shape (anchored to T-32-07 / clippy `await_holding_lock`):

```rust
let nudge_interval = self.config.memory.nudge_interval;
if nudge_interval > 0 && self.config.memory.memory_enabled {
    let should_fire = {
        let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(key.clone()).or_insert(0);
        *count += 1;
        if *count >= nudge_interval { *count = 0; true } else { false }
    }; // std::sync::MutexGuard dropped here — BEFORE any tokio::spawn / .await
    if should_fire {
        if let Some(ref mgr) = self.memory_manager {
            let mgr_clone = Arc::clone(mgr);
            let client_clone = nudge_client.clone();
            let messages_snapshot = messages_for_nudge.clone();
            let config_clone = self.config.clone();
            tokio::spawn(async move {
                ironhermes_agent::nudge::spawn_nudge_review(
                    messages_snapshot, mgr_clone, client_clone, &config_clone,
                ).await;
            });
        }
    }
}
```

Two supporting snapshots earlier in `run_agent`:

- `let nudge_client = client.clone();` — taken immediately after `build_main_client(...)` and BEFORE `AgentLoop::new(client, ...)` consumes the original. `AnyClient` derives `Clone`, so this is cheap.
- `let messages_for_nudge = messages.clone();` — taken immediately before `agent.run(messages).await` consumes the vec. The snapshot reflects the exact turn the model just consumed (no post-tool-call mutation).

Disable sentinel preserved: `nudge_interval == 0` OR `memory_enabled == false` OR `memory_manager is None` → fire is skipped silently. Same gate shape as the CLI `run_chat` path landed in Plan 32-01.

## Verification

| Gate | Command | Result |
|------|---------|--------|
| Counter-logic tests | `cargo test -p ironhermes-agent nudge::tests --lib` | 6 passed, 0 failed (3 prompt + 3 counter) |
| Gateway build | `cargo build -p ironhermes-gateway -p ironhermes-agent` | exit 0 (4 pre-existing warnings in `runner.rs`, none new) |
| Gateway tests | `cargo test -p ironhermes-gateway` | 80 passed, 0 failed across 7 binaries |
| Field/init/lock/entry count | `grep -c "nudge_turns" handler.rs` | 4 (≥4 required) |
| Module call site | `grep -c "ironhermes_agent::nudge::spawn_nudge_review" handler.rs` | 1 (=1 required) |
| Helper visibility | `grep -c "pub(crate) fn should_nudge" nudge.rs` | 1 |
| Helper signature | `grep "fn should_nudge(interval: u32, counter: &mut u32) -> bool" nudge.rs` | match |
| Tests landed | `grep -c "fires_at_interval\|disabled_when_zero\|counter_resets_after_fire" nudge.rs` | 3 |
| Clippy await_holding_lock | `cargo clippy --lib -p ironhermes-gateway -- -W clippy::await_holding_lock` | no `await_holding_lock` warning in nudge fire site |

## Deviations from Plan

None — plan executed as written.

### Plan-spec note (not a deviation)

- **Plan referenced `handle_with_multimodal` line ~999-1045 as the fire site**, but the actual `agent.run()` call site is in the `run_agent` helper (lines 999-1045 of the develop tip), which `handle_with_multimodal` forwards to via `self.run_agent(...).await`. Same function, just a level deeper than the plan's quick reference suggested — the fire site went into `run_agent` as the only location where `agent_result`, `key`, `client`, and `messages` are all in scope. Acceptance grep gates pass against `handler.rs` as a whole, so this is not a divergence from spec.

## Known Issues (pre-existing, out of scope)

Per the executor scope rule, the following test failures were observed but **NOT** introduced by this plan. Verified by `git stash`-ing Plan 32-02 changes and re-running each failing target against the plain develop tip — they fail identically. Logged here for visibility only; no change made.

| Test target | Crate | Pre-existing baseline |
|------|-------|------------------|
| `chat_memory_persistence::run_chat_and_run_single_both_wire_memory_manager` | `ironhermes-cli` | "expected ≥3 register_memory_tool calls; got 2" — same as Plan 32-01 already documented (the static-grep predicate is orthogonal to nudge wiring). |
| `invariants_21_7::invariant_21_7_02_three_register_delegate_task_sites` | `ironhermes-cli` | Pre-existing static-grep count regression unrelated to Plan 32-02. |
| `invariants_21_7::invariant_21_7_03_three_register_execute_code_sites` | `ironhermes-cli` | Same — pre-existing static-grep count regression. |
| `setup_wizard::setup_subcommand_skips_preflight` | `ironhermes-cli` | Pre-existing wizard test failure on develop. |
| `setup_wizard::setup_tools_section_exits_ok` | `ironhermes-cli` | Pre-existing wizard test failure on develop. |
| `installer::tests::update_post_rename_compare_is_advisory_when_hashes_diverge` | `ironhermes-hub` | Pre-existing hub regression. |
| `lock::migration_tests::migrates_entries_and_deletes_old_manifest` | `ironhermes-hub` | Pre-existing hub regression. |
| `lock::tests::save_sorts_alphabetically` | `ironhermes-hub` | Pre-existing hub regression. |
| `server_runtime_parity::api_sessions_and_tools_are_backed_by_real_state` | `iron_hermes_ui` | Same as Plan 32-01 already documented (StateStore-backed list_sessions for Platform::Web). |
| `websocket_lifecycle_parity::client_ws_disconnect_notices_are_generic_and_deduplicated_per_disconnect_window` | `iron_hermes_ui` | Pre-existing websocket UAT regression. |
| `websocket_lifecycle_parity::tab_click_clears_blocks_and_switches_session_id` | `iron_hermes_ui` | Pre-existing websocket UAT regression. |
| `websocket_lifecycle_parity::tab_close_uses_stop_propagation` | `iron_hermes_ui` | Pre-existing websocket UAT regression. |

These belong to phases unrelated to Plan 32-02 (CLI memory wiring, wizard, hub installer, web UI) and the test files were not touched by this plan.

## Threat Coverage

| Threat ID | Disposition | Where mitigated |
|-----------|-------------|-----------------|
| T-32-06 (DoS: per-session HashMap growth) | accepted | One u32 per active session; bounded by Telegram user count; eviction is best-effort via session_store lifecycle |
| T-32-07 (Availability: Mutex held across await) | mitigated | std::sync::MutexGuard scoped inside a block expression returning `bool`; guard drops at closing brace BEFORE any tokio::spawn / .await; clippy `await_holding_lock` clean |
| T-32-08 (DoS: gateway nudge recursion) | accepted | `nudge_turns` lives on `GatewayMessageHandler`; the nudge's inner `AgentLoop` cannot reference it — structurally impossible to recurse |
| T-32-SC (Tampering: cargo installs) | accepted | Zero new external packages |

## Threat Flags

(none — no new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries)

## Known Stubs

(none — no hardcoded empty values or placeholder text)

## TDD Gate Compliance

Plan-level TDD gate sequence verified in `git log --oneline`:

1. RED gate: `test(32-02): add failing tests for should_nudge counter helper` (`d1299f23`)
2. GREEN gate: `feat(32-02): implement should_nudge counter helper` (`a0db624d`)
3. (REFACTOR gate not needed — implementation was minimal and clear; no cleanup commit)

The RED commit's stub returned `false` for all inputs. `cargo test -p ironhermes-agent nudge::tests` reported `fires_at_interval` and `counter_resets_after_fire` failing at runtime; `disabled_when_zero` was coincidentally satisfied by the constant-false stub but the other two prove the predicate is observably wrong. The GREEN commit fills in the real body and brings the suite to 6/6 pass.

## Self-Check: PASSED

- `crates/ironhermes-agent/src/nudge.rs` exists: FOUND
- `pub(crate) fn should_nudge(interval: u32, counter: &mut u32) -> bool` in nudge.rs: FOUND (1 match)
- `fires_at_interval` test in nudge.rs: FOUND
- `disabled_when_zero` test in nudge.rs: FOUND
- `counter_resets_after_fire` test in nudge.rs: FOUND
- `nudge_turns: Arc<std::sync::Mutex<std::collections::HashMap<SessionKey, u32>>>` in handler.rs: FOUND (field decl)
- `nudge_turns: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()))` in handler.rs: FOUND (new() init)
- `self.nudge_turns.lock()` in handler.rs: FOUND (fire site lock call)
- `map.entry(key.clone()).or_insert(0)` in handler.rs: FOUND (fire site entry call)
- `ironhermes_agent::nudge::spawn_nudge_review` call site in handler.rs: FOUND (1 match)
- `let nudge_client = client.clone();` in handler.rs: FOUND
- `let messages_for_nudge = messages.clone();` in handler.rs: FOUND
- Commit `d1299f23` (Task 1 RED): FOUND
- Commit `a0db624d` (Task 1 GREEN): FOUND
- Commit `f24dc950` (Task 2 gateway wiring): FOUND
- `cargo test -p ironhermes-agent nudge::tests --lib`: 6 passed, 0 failed
- `cargo build -p ironhermes-gateway -p ironhermes-agent`: exit 0
- `cargo test -p ironhermes-gateway`: 80 passed, 0 failed

## Next-Plan Handoff

- **Plan 32-03** (web UI nudge wiring) — the iron_hermes_ui server path needs the same per-session counter shape on its handler equivalent. `should_nudge` is reusable as-is; the only new code is the field + init + fire site, mirroring the gateway change made here.
- The `Arc<std::sync::Mutex<HashMap<SessionKey, u32>>>` pattern is now the third instance of the "handler-with-interior-mutable-per-session-state" recipe (skill_overlays, active_personality_overlay, nudge_turns). A future refactor could extract a `PerSessionState<T>` newtype, but it's not load-bearing for Phase 32.
