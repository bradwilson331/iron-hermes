---
phase: 36-gateway-running-agent-guard-wiring-completes-gw-05
plan: "02"
subsystem: gateway
tags:
  - gateway
  - concurrency
  - security
  - guard
  - gw-05

dependency_graph:
  requires:
    - "36-01 (Wave-0 test scaffold with helper infrastructure)"
  provides:
    - "GW-05 behavior: per-session running-agent guard fully wired"
    - "RunningAgentGuard RAII type (pub, D-06)"
    - "is_bypass predicate (D-01 canonical bypass list)"
    - "AGENT_RUNNING_REJECT_MSG const (D-02 locked string)"
    - "SessionStore::get_running_flag accessor (D-03/D-05)"
    - "GatewaySession.running Arc<AtomicBool> field (D-03)"
    - "All 11 GW-05 sub-behavior tests passing"
  affects:
    - "crates/ironhermes-gateway/src/session.rs"
    - "crates/ironhermes-gateway/src/handler.rs"
    - "crates/ironhermes-gateway/src/lib.rs"
    - "crates/ironhermes-gateway/tests/running_agent_guard_tests.rs"

tech_stack:
  added: []
  patterns:
    - "RAII guard (RunningAgentGuard) wrapping Arc<AtomicBool> with SeqCst ordering"
    - "Per-session AtomicBool stored on GatewaySession, retrieved via SessionStore accessor"
    - "Post-alias canonical name check (def.name) for guard bypass — Pitfall 4 mitigation"
    - "Three-site rejection: handle_slash_command, MessageHandler::handle, handle_with_multimodal"

key_files:
  created:
    - "crates/ironhermes-gateway/tests/running_agent_guard_tests.rs"
  modified:
    - "crates/ironhermes-gateway/src/session.rs"
    - "crates/ironhermes-gateway/src/handler.rs"
    - "crates/ironhermes-gateway/src/lib.rs"

decisions:
  - "RunningAgentGuard exposed as pub (not pub(crate)) so integration tests can import it directly from ironhermes_gateway::RunningAgentGuard — preferred over adding a force_set_running test helper"
  - "Test file re-created in worktree (Plan 01 scaffold was in main repo, not in worktree); full implementation written directly rather than un-ignoring the stub file"
  - "tests/running_agent_guard_tests.rs run from worktree root (cargo test -p ...) not main repo — Plan 01 scaffold at main repo path would shadow the worktree file if run from /Users/twilson/code/ironhermes"
  - "Non-slash guard checks use a scoped block to drop the RwLock read guard before calling run_agent (avoids holding async read lock across an await point)"

metrics:
  duration: "18 min"
  completed: "2026-05-24"
  tasks_completed: 3
  files_modified: 4
---

# Phase 36 Plan 02: Running-Agent Guard Wiring (GW-05) — Summary

GW-05 running-agent guard fully implemented: per-session `Arc<AtomicBool>` on `GatewaySession`, `RunningAgentGuard` RAII type, three rejection sites (slash dispatch, non-slash `MessageHandler::handle`, non-slash `handle_with_multimodal`), and all 11 sub-behavior integration tests passing.

## What Was Built

### Task 1: `GatewaySession.running` + `SessionStore::get_running_flag`

`crates/ironhermes-gateway/src/session.rs`:

- Added `use std::sync::atomic::AtomicBool` import
- Added `pub running: Arc<AtomicBool>` as the last field of `GatewaySession` (struct line ~48)
- Initialized `running: Arc::new(AtomicBool::new(false))` in `GatewaySession::new`
- Added `pub fn get_running_flag(&self, key: &SessionKey) -> Arc<AtomicBool>` after `pub fn get` at line ~236: returns `session.running.clone()` if session exists, or `Arc::new(AtomicBool::new(false))` on miss (first-message fallback — no agent turn in flight)

### Task 2: `RunningAgentGuard` + `is_bypass` + three guard sites

`crates/ironhermes-gateway/src/handler.rs` — exact post-edit line numbers:

| Item | Line |
|------|------|
| `AGENT_RUNNING_REJECT_MSG` const | 66-68 |
| `pub struct RunningAgentGuard` | 79 |
| `impl Drop for RunningAgentGuard` | 88 |
| `fn is_bypass` | 101 |
| Shim replacement in `handle_slash_command` | 423 |
| Guard check in `handle_slash_command` (after resolve) | ~488-495 |
| Guard check in `handle_with_multimodal` non-slash | 845-853 |
| `get_running_flag` + `RunningAgentGuard::new` in `run_agent` | 873-875 |
| Guard check in `MessageHandler::handle` non-slash | 1296-1303 |

**`RunningAgentGuard` visibility:** Exposed as `pub` (not `pub(crate)`) and re-exported from `lib.rs` (`pub use handler::RunningAgentGuard`) so integration tests in `tests/` can import it as `ironhermes_gateway::RunningAgentGuard`.

**`run_agent` call sites:** The guard at line 875 (`let _agent_guard = RunningAgentGuard::new(_running_flag)`) is the single guard inside `run_agent`. All 5 original call sites (which shifted during editing) still invoke `run_agent` and reach the guard. Verified: `grep -c "RunningAgentGuard::new"` returns 1 (single production guard site).

### Task 3: 11 tests implemented and passing

`crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` was created fresh in the worktree (the Plan 01 scaffold lived in the main repo, not the worktree). The implementation:

- Preserves all 11 canonical test names from 36-VALIDATION.md verbatim
- 0 `#[ignore]` attributes
- 0 `todo!()` stubs
- All tests use `session.running.store(true, Ordering::SeqCst)` to set in-flight state (directly on the `pub` field — same `Arc<AtomicBool>` as production)
- `test_guard_clears_on_success` and `test_guard_clears_on_error` use `RunningAgentGuard::new` directly (production primitive)
- `RecordingPlatformAdapter`, `build_test_session_store`, `build_test_handler`, `make_event`, `d02_error_message` helpers match Plan 01 infrastructure

## Verification Results

```
running 11 tests
test test_guard_clears_on_success ... ok
test test_guard_clears_on_error ... ok
test test_new_bypasses_guard ... ok
test test_stop_reads_real_flag ... ok
test test_alias_bypasses_guard ... ok
test test_model_rejected_when_running ... ok
test test_freetext_rejected_when_running ... ok
test test_queue_bypasses_guard ... ok
test test_status_bypasses_guard ... ok
test test_stop_bypasses_guard ... ok
test test_session_isolation ... ok

test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

Full gateway suite: all existing tests green (agents_confirm, cron_delivery, gateway_shutdown, invariants_27_1_4_1, invariants_28_1_02, invariants_34, session_workspace_root, skill_registry_wiring).

Workspace build: `cargo build --workspace` exits 0.

Clippy (`cargo clippy -p ironhermes-gateway -- -D warnings`): 0 errors in `ironhermes-gateway` source. Pre-existing clippy errors in `ironhermes-core` are out of scope (not introduced by this plan).

## Deviations from Plan

**1. [Rule 3 - Blocker] Test file re-created in worktree instead of un-ignoring Plan 01 scaffold**
- **Found during:** Task 3
- **Issue:** Plan 01 scaffold was created in the main repo (`/Users/twilson/code/ironhermes/crates/...`). The worktree at `/Users/twilson/code/ironhermes/.claude/worktrees/agent-a49ec23f5ac1b5d86/` did not have the file. Running `cargo test` from the worktree would compile the new test binary but `running_agent_guard_tests.rs` was missing.
- **Fix:** Created the full implementation directly in the worktree's `tests/` directory. Preserved all 11 canonical test names, all helpers, and all doc comments from the Plan 01 scaffold.
- **Files modified:** `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` (created)

**2. [Rule 2 - Missing critical functionality] RunningAgentGuard exposed as pub for test access**
- **Found during:** Task 3
- **Issue:** `RunningAgentGuard` was `struct` (private). Integration tests are separate crates and cannot access `pub(crate)` items. Tests GW-05-7 and GW-05-8 require constructing the guard directly to test RAII semantics.
- **Fix:** Changed to `pub struct RunningAgentGuard` with `pub fn new`, added `pub use handler::RunningAgentGuard` to `lib.rs`.
- **Files modified:** `handler.rs`, `lib.rs`

**3. [Minor] Non-slash guard checks use scoped block**
- Plan text showed inline `let session_key = ...` / `let agent_running = ...` before `self.run_agent(...)`. Implementation wraps in a `{ }` block to drop the `RwLock` read guard before `run_agent` acquires a write lock (via `session_store.write().await` inside `run_agent`). This prevents a deadlock. No behavioral difference.

## Threat Surface Scan

No new network endpoints, auth paths, or schema changes introduced. All types (`AtomicBool`, `Arc`, `Ordering`) are `std`. Threat register mitigations confirmed present:

| Threat ID | Mitigation | Test |
|-----------|------------|------|
| T-21.1-05 (cross-session bleed) | `pub running` on `GatewaySession`, keyed by `SessionKey` | `test_session_isolation` |
| T-21.1-08 (non-slash bypass) | Guard in `MessageHandler::handle` + `handle_with_multimodal` | `test_freetext_rejected_when_running` |
| T-21.1-09 (model-swap TOCTOU) | `/model` not in bypass list | `test_model_rejected_when_running` |
| T-36-01 (alias bypass) | Guard checks `def.name` post-alias | `test_alias_bypasses_guard` |
| T-36-02 (memory ordering) | `Ordering::SeqCst` on all stores and loads | All guard tests |
| T-36-03 (stuck-true flag) | `RunningAgentGuard` Drop fires on `?` | `test_guard_clears_on_error` |

## Self-Check: PASSED

- `crates/ironhermes-gateway/src/session.rs` contains `pub running: Arc<AtomicBool>`: VERIFIED (1 hit)
- `crates/ironhermes-gateway/src/session.rs` contains `pub fn get_running_flag`: VERIFIED (1 hit)
- `crates/ironhermes-gateway/src/handler.rs` contains `struct RunningAgentGuard`: VERIFIED (1 hit)
- `crates/ironhermes-gateway/src/handler.rs` contains `impl Drop for RunningAgentGuard`: VERIFIED (1 hit)
- `crates/ironhermes-gateway/src/handler.rs` contains `fn is_bypass`: VERIFIED (1 hit)
- `crates/ironhermes-gateway/src/handler.rs` shim (`Arc::new(std::sync::atomic::AtomicBool::new(false))`) outside comments: VERIFIED (0 hits)
- `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` contains 0 `#[ignore`: VERIFIED
- `crates/ironhermes-gateway/tests/running_agent_guard_tests.rs` contains 0 `todo!`: VERIFIED
- `cargo test -p ironhermes-gateway --test running_agent_guard_tests`: 11 passed / 0 failed / 0 ignored: VERIFIED
- Commits exist: 4e7e0bd4 (Task 1), 3ea7bed5 (Task 2), 5e4ddc40 (Task 3): VERIFIED
