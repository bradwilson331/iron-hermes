---
phase: "32"
plan: "03"
subsystem: "learning-loop"
tags: [LEARN-01, nudge, web-ui, embedded-agent, memory-curation]
requires:
  - "ironhermes_agent::nudge::spawn_nudge_review (Plan 32-01)"
  - "ironhermes_core::MemoryConfig.nudge_interval / .memory_enabled (Plan 32-01)"
  - "Gateway nudge_turns interior-mutability pattern (Plan 32-02)"
  - "build_main_client(&resolver) (existing helper, Phase 12)"
  - "AnyClient: #[derive(Clone)] (Plan 12)"
provides:
  - "AppState.nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>> per-session counter"
  - "Post-turn nudge fire site in run_web_turn's success arm"
  - "Completes LEARN-01 coverage for the embedded web UI agent path"
affects:
  - "crates/iron_hermes_ui/src/server/state.rs (AppState struct, AppState::init constructor, run_web_turn fire site)"
tech-stack:
  added: []
  patterns:
    - "Interior-mutable Arc<std::sync::Mutex<HashMap<String, u32>>> on AppState — third instance of the per-session-state recipe (after gateway nudge_turns + skill_overlays); session key is the &str session_id used by run_web_turn"
    - "std::sync::MutexGuard scoped inside a block expression that returns `should_fire: bool` — guard dropped at closing brace BEFORE any tokio::spawn / .await (clippy::await_holding_lock clean, T-32-10)"
    - "Explicit drop(store) on the state_store MutexGuard before acquiring nudge_turns Mutex — prevents accidentally holding two locks across the (still-synchronous) counter mutation"
    - "messages.clone() snapshot BEFORE the move into agent.run(messages).await — the snapshot reflects the exact turn the model just consumed"
    - "build_main_client(&self.resolver)? at fire site — fallible client construction matching the existing pattern used by build_agent_loop; failure propagates via ? (same path that would have blocked the main turn)"
key-files:
  created:
    - ".planning/phases/32-periodic-nudge-memory-curation/32-03-SUMMARY.md"
  modified:
    - "crates/iron_hermes_ui/src/server/state.rs"
decisions:
  - "Session key on AppState.nudge_turns is String (not the gateway's SessionKey newtype) — run_web_turn already takes session_id: &str produced by api.rs create_session, so the HashMap key shape matches the existing call surface with zero refactor pressure"
  - "Explicitly drop(store) on the state_store Mutex before the nudge block — the prior shape held the guard implicitly until end-of-scope; making it explicit avoids accidentally holding two std::sync::Mutex guards (state_store + nudge_turns) at once, even though both are scoped synchronously"
  - "build_main_client(&self.resolver)? at the fire site rather than caching a clone earlier — AnyClient does derive Clone, but the embedded path goes through attach_context_engine inside build_agent_loop, which already calls build_main_client internally; constructing a fresh client at fire time keeps the nudge entirely independent of the main turn's wired-fallback/context-engine pipeline"
  - "(*self.config).clone() at fire time — self.config is Arc<Config> and spawn_nudge_review takes &Config; cloning the inner Config gives the spawned task an owned copy without leaking the Arc handle"
  - "Both edits in state.rs land as separate commits (4941df33 field decl + init, 6b4520d1 fire site) — keeps the field-addition commit reviewable independently from the fire-site logic"
metrics:
  duration_minutes: 6
  completed_date: 2026-05-15
  tasks_committed: 2
  files_modified: 1
  tests_added: 0
---

# Phase 32 Plan 03: Periodic Nudge — Web UI Wiring Summary

The embedded web UI agent path now fires the periodic memory-review nudge on the same `memory.nudge_interval` cadence as the CLI REPL (Plan 32-01) and the gateway (Plan 32-02). Wave 3 completes LEARN-01 coverage for every active surface that runs the agent — there is no longer an active path where a user can converse without the periodic memory nudge being eligible to fire.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Add `nudge_turns` field to AppState and initialize in `AppState::init` | `4941df33` | `crates/iron_hermes_ui/src/server/state.rs` |
| 2 | Wire nudge fire site into `run_web_turn` | `6b4520d1` | `crates/iron_hermes_ui/src/server/state.rs` |

## What Shipped

### `AppState.nudge_turns` field (Task 1)

```rust
/// Per-session nudge turn counter. Arc<Mutex<HashMap>> mirrors the gateway
/// nudge_turns pattern from Plan 32-02. Interior mutability required: run_web_turn
/// takes &self, but the counter must mutate across calls for the same session.
/// Session key is the same String used by run_web_turn (e.g.
/// "agent:main:web:dm:{uuid}" produced by api.rs create_session).
/// (Phase 32 LEARN-01 — web UI nudge wiring)
pub nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>,
```

Typed identically in spirit to the gateway's `nudge_turns` from Plan 32-02 — only the key type changes (`String` instead of `SessionKey`) because `run_web_turn` already takes `session_id: &str`. Initialized empty in `AppState::init`; entries are lazily inserted on first turn per session via `map.entry(session_id.to_string()).or_insert(0)`. AppState already derives `Clone`; the `Arc<Mutex<HashMap>>` shape satisfies that derive cleanly (Arc::clone is shallow).

### Post-turn fire site in `run_web_turn` (Task 2)

Placement: inside the `Ok(result)` flow of `run_web_turn`, **after** the for-loop that writes `result.appended` to the state_store and **after** an explicit `drop(store)` on the state_store guard, but **before** the final `Ok(result)`. The state-store write completes first so the nudge snapshot reflects exactly what the user sees.

Shape (anchored to T-32-10 / clippy `await_holding_lock`):

```rust
// Phase 32 LEARN-01: periodic memory nudge (turn-based, post-response).
let nudge_interval = self.config.memory.nudge_interval;
if nudge_interval > 0 && self.config.memory.memory_enabled {
    let should_fire = {
        let mut map = self.nudge_turns.lock().unwrap_or_else(|e| e.into_inner());
        let count = map.entry(session_id.to_string()).or_insert(0);
        *count += 1;
        if *count >= nudge_interval { *count = 0; true } else { false }
    }; // std::sync::Mutex guard dropped here — BEFORE any await/spawn
    if should_fire {
        if let Some(ref mgr) = self.memory_manager {
            let mgr_clone = Arc::clone(mgr);
            let client_clone = build_main_client(&self.resolver)?;
            let config_clone = (*self.config).clone();
            tokio::spawn(async move {
                ironhermes_agent::nudge::spawn_nudge_review(
                    messages_snapshot, mgr_clone, client_clone, &config_clone,
                ).await;
            });
        }
    }
}
```

Supporting snapshot earlier in `run_web_turn`:

- `let messages_snapshot = messages.clone();` — taken immediately after `build_messages_for_turn(...)` and BEFORE `agent.run(messages).await` consumes the original vec. Matches the gateway pattern from Plan 32-02; the snapshot reflects the exact turn the model just consumed (no post-tool-call mutation).

Disable sentinel preserved: `nudge_interval == 0` OR `memory_enabled == false` OR `memory_manager is None` → fire is skipped silently. Same gate shape as both prior waves.

The `nudge_turns` HashMap uses interior mutability rather than `tokio::sync::Mutex` because the entire counter mutation is synchronous (increment + compare + reset) — there is no `.await` inside the lock scope, so std::sync::Mutex is correct and clippy-clean.

## Verification

| Gate | Command | Result |
|------|---------|--------|
| Web UI build | `cargo build -p iron_hermes_ui` | exit 0 (15 pre-existing warnings, none new) |
| Web UI clippy | `cargo clippy -p iron_hermes_ui --no-deps` | exit 0; no `await_holding_lock` warning at the fire site |
| Web UI tests | `cargo test -p iron_hermes_ui` | All pre-existing-passing tests still pass; 4 pre-existing-failing tests still fail identically (documented below — same baseline as 32-02-SUMMARY.md) |
| Workspace build | `cargo build --workspace` | exit 0 |
| Nudge regression | `cargo test -p ironhermes-agent nudge::tests --lib` | 6 passed, 0 failed (3 prompt + 3 counter) |
| Field/init/lock/entry count | `grep -c "nudge_turns" state.rs` | 5 (>= 4 required) |
| Module call site | `grep -c "ironhermes_agent::nudge::spawn_nudge_review" state.rs` | 1 (== 1 required) |
| Fire site lock | `grep -c "self.nudge_turns" state.rs` | 1 (== 1 required, only inside should_fire block) |
| Snapshot | `grep -c "messages_snapshot" state.rs` | 2 (>= 1 required: 1 binding + 1 move) |
| Threshold pair | `grep -c "nudge_interval" state.rs` | 3 (>= 2 required: read + threshold compare + matched if-let) |
| Guard scope close | `grep "}; //" state.rs` | match — `}; // std::sync::Mutex guard dropped here — BEFORE any await/spawn` |

## Deviations from Plan

None — plan executed as written. Wave 3 mirrors Wave 2 structurally; the only differences are the session-key type (`String` not `SessionKey`) and the client construction (`build_main_client(&self.resolver)?` at fire time, not a pre-cloned handle), both of which the plan explicitly specified.

## Known Issues (pre-existing, out of scope)

The same 4 `iron_hermes_ui` test failures documented in `32-02-SUMMARY.md` Known Issues are still present on develop — they pre-date this plan and were verified pre-existing in Wave 2's baseline check. Per executor scope rules, these are NOT introduced by this plan:

| Test target | Crate | Pre-existing baseline |
|-------------|-------|------------------------|
| `server_runtime_parity::api_sessions_and_tools_are_backed_by_real_state` | `iron_hermes_ui` | "list_sessions must query StateStore for Platform::Web sessions" — same as Plan 32-01/32-02 already documented |
| `websocket_lifecycle_parity::client_ws_disconnect_notices_are_generic_and_deduplicated_per_disconnect_window` | `iron_hermes_ui` | Pre-existing websocket UAT regression |
| `websocket_lifecycle_parity::tab_click_clears_blocks_and_switches_session_id` | `iron_hermes_ui` | Pre-existing websocket UAT regression |
| `websocket_lifecycle_parity::tab_close_uses_stop_propagation` | `iron_hermes_ui` | Pre-existing websocket UAT regression |

The failing test names mention neither `nudge` nor `nudge_turns`; the panic messages relate to `Platform::Web` session listing and websocket disconnect handling — completely orthogonal subsystems. Wave 2's summary file (`32-02-SUMMARY.md` Known Issues) verified the identical failures against the plain develop tip via `git stash`-and-re-run.

## Threat Coverage

| Threat ID | Disposition | Where mitigated |
|-----------|-------------|-----------------|
| T-32-09 (DoS: per-session HashMap unbounded growth) | accepted | One u32 per active web session; bounded by uuid-keyed entries; small per-entry size (4 bytes + key string). No active eviction needed for MVP. |
| T-32-10 (Availability: Mutex held across await) | mitigated | std::sync::MutexGuard scoped inside a block expression returning `bool`; guard drops at closing brace BEFORE any tokio::spawn / .await; clippy `await_holding_lock` clean |
| T-32-11 (DoS: web UI nudge recursion) | accepted | `nudge_turns` lives on `AppState`; spawn_nudge_review constructs an independent AgentLoop with a narrow ToolRegistry that does not reference AppState — structurally impossible to recurse |
| T-32-12 (Information Disclosure: nudge snapshot leaks across sessions) | mitigated | messages_snapshot built from per-session messages vec returned by build_messages_for_turn(session_id, ...); HashMap entry keyed by session_id (String). Cross-session leakage requires uuid collision (~10^-37). |
| T-32-13 (Tampering: prompt injection via memory review) | mitigated | Inherited from Plan 32-01: nudge writes flow through MemoryManager::handle_tool_call -> MemoryStore::scan_content (Phase 17 security scanner). No web-UI-specific bypass added. |
| T-32-14 (Availability: provider resolver failure on nudge client build) | accepted | `build_main_client(&self.resolver)?` propagates via `?` — same failure that would block the main turn |
| T-32-SC (Tampering: cargo installs) | accepted | Zero new external packages; pure intra-workspace changes to `crates/iron_hermes_ui` |

## Threat Flags

(none — no new network endpoints, auth paths, file access patterns, or schema changes at trust boundaries)

## Known Stubs

(none — no hardcoded empty values or placeholder text)

## Self-Check: PASSED

- `crates/iron_hermes_ui/src/server/state.rs` exists: FOUND
- `pub nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>` in state.rs: FOUND (field decl)
- `nudge_turns: Arc::new(std::sync::Mutex::new(HashMap::new()))` in state.rs: FOUND (init)
- `self.nudge_turns.lock()` in state.rs: FOUND (fire site lock call)
- `map.entry(session_id.to_string()).or_insert(0)` in state.rs: FOUND (fire site entry call)
- `ironhermes_agent::nudge::spawn_nudge_review` call site in state.rs: FOUND (1 match)
- `let messages_snapshot = messages.clone();` in state.rs: FOUND
- `drop(store);` after state_store write in state.rs: FOUND
- `}; // std::sync::Mutex guard dropped here — BEFORE any await/spawn` in state.rs: FOUND
- Commit `4941df33` (Task 1 — AppState field): FOUND in git log
- Commit `6b4520d1` (Task 2 — run_web_turn fire site): FOUND in git log
- `cargo build -p iron_hermes_ui`: exit 0
- `cargo clippy -p iron_hermes_ui --no-deps`: exit 0; no `await_holding_lock` warning at the fire site
- `cargo build --workspace`: exit 0
- `cargo test -p ironhermes-agent nudge::tests --lib`: 6 passed, 0 failed
- `cargo test -p iron_hermes_ui`: only pre-existing failures from 32-02 baseline; no new failures introduced

## Next-Plan Handoff

- **Wave 3 closes LEARN-01.** All three active agent surfaces (CLI REPL via Plan 32-01, Telegram gateway via Plan 32-02, embedded web UI via this plan) now fire the periodic memory-review nudge on the same `memory.nudge_interval` cadence with the same disable sentinel (`interval == 0` OR `memory_enabled == false`). The phase's LEARN-01 requirement is fully covered across surfaces.
- **`Arc<std::sync::Mutex<HashMap<K, u32>>>` is now the canonical per-session-state recipe in this workspace** — third concrete instance (skill_overlays, gateway nudge_turns, web UI nudge_turns). A future refactor could extract a `PerSessionState<K, T>` newtype, but the duplication is acceptable for now since each handler has a slightly different key type (`SessionKey` vs `String`).
- The pre-existing `iron_hermes_ui` test failures (server_runtime_parity + websocket_lifecycle_parity) remain on develop and are owned by Phases unrelated to nudge wiring. Fixing them is out of scope for Phase 32.
