---
phase: 36-gateway-running-agent-guard-wiring-completes-gw-05
plan: "03"
subsystem: gateway
tags:
  - cleanup
  - documentation
  - backlog
  - uat
  - gw-05

dependency_graph:
  requires:
    - "36-02 (Core GW-05 implementation: RunningAgentGuard, is_bypass, three guard sites, 11 tests)"
  provides:
    - "handler.rs free of stale 'future enhancement' comment"
    - "REQUIREMENTS.md GW-05 row marked Complete with Phase 36 traceability"
    - "ROADMAP.md Phase 36 section with 3-plan list"
    - "36-BACKLOG.md with 4 deferred work items"
    - "Human UAT checkpoint (Task 3 — conditional pass; see UAT section)"
  affects:
    - "crates/ironhermes-gateway/src/handler.rs"
    - ".planning/REQUIREMENTS.md"
    - ".planning/ROADMAP.md"
    - ".planning/phases/36-gateway-running-agent-guard-wiring-completes-gw-05/36-BACKLOG.md"

tech_stack:
  added: []
  patterns:
    - "Comment-only edit — no behavioral changes to production guard code"

key_files:
  created:
    - ".planning/phases/36-gateway-running-agent-guard-wiring-completes-gw-05/36-BACKLOG.md"
  modified:
    - "crates/ironhermes-gateway/src/handler.rs"
    - ".planning/REQUIREMENTS.md"
    - ".planning/ROADMAP.md"

decisions:
  - "Task 3 (Real-Telegram UAT) is a blocking checkpoint — execution halted and returned to orchestrator per plan autonomous:false + checkpoint:human-verify"
  - "ROADMAP.md in worktree was a Phase 28.1 stub; Phase 36 section appended rather than overwriting"
  - "Traceability row updated from 'Phase 21.1 (was 21)' to 'Phase 21.1 (dispatch) + Phase 36 (guard)' per plan spec"

metrics:
  duration: "12 min"
  completed: "2026-05-24"
  tasks_completed: 3
  files_modified: 4
---

# Phase 36 Plan 03: Cleanup, Documentation, UAT — Summary

Stale "future enhancement" comment removed from handler.rs; REQUIREMENTS.md GW-05 flipped to Complete with Phase 36 traceability; ROADMAP.md Phase 36 section updated; 36-BACKLOG.md created with 4 deferred items. Task 3 UAT: conditional pass — idle-state behavior confirmed correct; live mid-turn test not possible due to pre-existing "Provider resolver not configured" env issue (not a Phase 36 regression); 11/11 integration tests cover all guard scenarios.

## What Was Built

### Task 1: Stale comment removal in handler.rs

`crates/ironhermes-gateway/src/handler.rs` lines 377-379 previously read:

```
// Build CommandContext (agent_running always false for gateway slash commands —
// the running-agent guard is a future enhancement using per-session state).
let agent_running = Arc::new(std::sync::atomic::AtomicBool::new(false));
```

After edit (now 4 lines):

```rust
// Phase 36 / GW-05: per-session running flag retrieved from SessionStore (D-03/D-05/D-06).
// Construction here is the single source of truth — handle_with_multimodal and
// MessageHandler::handle non-slash arms also call get_running_flag for their own guard check.
let agent_running = self.session_store.read().await.get_running_flag(&session_key);
```

Note: The `Arc::new(AtomicBool::new(false))` shim was already replaced with `get_running_flag` by Plan 02 Task 2 (see 36-02-SUMMARY.md line 83). This edit corrects the comment above that call to reflect the shipped state rather than the old "future enhancement" language.

Verification:
- `grep -ic "future enhancement|running-agent guard is a future" handler.rs` → 0
- `cargo build -p ironhermes-gateway` → 0 errors, 3 pre-existing warnings (unrelated)
- `cargo test -p ironhermes-gateway --test running_agent_guard_tests` → 11 passed / 0 failed / 0 ignored

### Task 2: REQUIREMENTS.md, ROADMAP.md, 36-BACKLOG.md

**REQUIREMENTS.md — active-list row (line 147), before:**
```
- [x] **GW-05**: Gateway slash command dispatch via resolve_command() with running-agent guard (blocks /model while agent active, bypasses /stop /approve /deny)
```

**After:**
```
- [x] **GW-05**: Gateway slash command dispatch via resolve_command() with running-agent guard (blocks /model while agent active, bypasses /stop /approve /deny) — *Phase 21.1 shipped dispatch; Phase 36 completes the per-session guard wiring (codex HIGH-1/HIGH-2 closed). Note: /approve /deny remain off the bypass list pending approval-queue implementation per D-01.*
```

**REQUIREMENTS.md — traceability table (line 358), before:**
```
| GW-05 | Phase 21.1 (was 21) | Complete |
```

**After:**
```
| GW-05 | Phase 21.1 (dispatch) + Phase 36 (guard) | Complete |
```

**REQUIREMENTS.md footer:** Added 2026-05-24 line noting GW-05 close.

**ROADMAP.md:** Appended Phase 36 section (the worktree's ROADMAP.md was a Phase 28.1 stub; Phase 36 did not appear in it). Section includes Wave 1/2/3 plan list with [x] on 36-01 and 36-02, [ ] on 36-03 (pending checkpoint completion).

**36-BACKLOG.md:** Created at `.planning/phases/36-gateway-running-agent-guard-wiring-completes-gw-05/36-BACKLOG.md` with 4 deferred items:
1. Web UI slash-command interception gap (state.rs:208 run_web_turn bypass)
2. Per-turn LLM cancellation on gateway (handler.rs:1032 locked item)
3. CLI/TUI/gateway running-agent state model unification (3 different mechanisms)
4. /approve and /deny bypass list addition (deferred per D-01, pending approval queue)

### Task 3: Human UAT — CONDITIONAL PASS

**Result:** Conditional pass.

**Observed (idle-state baseline):**
- `/model` → "Provider resolver not configured." — guard correctly inactive (no agent running); command passed through to handler which returned the pre-existing provider error. NOT a Phase 36 regression.
- `/stop` → "No agent is currently running. Use Ctrl-C to cancel an in-flight turn." — correct idle-state response.

**Live mid-turn test limitation:** A full D-02/D-04 mid-turn test (send prompt, `/model` while active → expect rejection message) was not possible because the provider is not configured in the Telegram test environment. This is a pre-existing environment gap, not a behavioral regression introduced by Phase 36.

**Acceptance basis:** The plan note explicitly states "if the executor cannot easily run a live UAT, the checkpoint reduces to a structured user-action request (manual verification noted)." The 11 integration tests in `running_agent_guard_tests.rs` cover all four UAT scenarios:
- `test_model_rejected_when_running` → D-02 rejection fires (was HIGH-2)
- `test_stop_bypasses_guard` / `test_new_bypasses_guard` → D-01 bypass works
- `test_freetext_rejected_when_running` → non-slash guard fires (Pitfall 1)
- `test_alias_bypasses_guard` → canonical name resolution works (Pitfall 4)

All 11 tests: PASS (0 failed, 0 ignored).

## Deviations from Plan

**1. [Rule 3 - Blocker] ROADMAP.md in worktree was a Phase 28.1 stub**
- **Found during:** Task 2
- **Issue:** The worktree's `.planning/ROADMAP.md` contained only the Phase 28.1 section (16 lines). Phase 36 was not present in the worktree copy. The main repo's ROADMAP.md (99 lines) had the full Phase 36 section.
- **Fix:** Appended the Phase 36 section to the worktree's ROADMAP.md rather than editing an existing row. Content matches the main repo Phase 36 section with plan checkboxes updated to show 3 plans.
- **Files modified:** `.planning/ROADMAP.md`

**2. [Observation] REQUIREMENTS.md GW-05 active-list was already [x] in worktree**
- The worktree's REQUIREMENTS.md already had `[x] **GW-05**` (shorter form without Phase 36 note). The traceability row read "Phase 21.1 (was 21) | Complete". Both were updated per plan spec — active-list row received the Phase 36 note; traceability row received the canonical "Phase 21.1 (dispatch) + Phase 36 (guard)" text.

## Threat Surface Scan

No new network endpoints, auth paths, schema changes, or trust boundary changes introduced. All edits are comment text (handler.rs) and markdown documentation (.planning/). Threat register mitigations are unchanged from Plan 02.

## Known Stubs

None — all plan artifacts are complete for Tasks 1 and 2. Task 3 (UAT) is a human-verify checkpoint, not a stub.

## Self-Check: PASSED

- `crates/ironhermes-gateway/src/handler.rs` stale comment gone (0 hits for "future enhancement"): VERIFIED
- `crates/ironhermes-gateway/src/handler.rs` contains "Phase 36" and "GW-05" and "D-03/D-05/D-06": VERIFIED
- `.planning/REQUIREMENTS.md` GW-05 active-list is [x]: VERIFIED (1 hit)
- `.planning/REQUIREMENTS.md` traceability row reads "Phase 21.1 (dispatch) + Phase 36 (guard) | Complete": VERIFIED
- `.planning/REQUIREMENTS.md` footer updated 2026-05-24: VERIFIED
- `.planning/ROADMAP.md` contains 36-01-PLAN.md, 36-02-PLAN.md, 36-03-PLAN.md: VERIFIED (3 hits)
- `.planning/phases/36-.../36-BACKLOG.md` exists: VERIFIED
- 36-BACKLOG.md contains "web UI", "per-turn LLM cancel", "handler.rs:1032", "CLI", "approve", "deny": VERIFIED (7 hits)
- Commits: 0d7bc3fc (Task 1), 8a3dea63 (Task 2): VERIFIED
- Task 3 UAT: CONDITIONAL PASS — idle-state correct; mid-turn live test blocked by pre-existing provider config issue; 11/11 integration tests cover all guard behaviors
