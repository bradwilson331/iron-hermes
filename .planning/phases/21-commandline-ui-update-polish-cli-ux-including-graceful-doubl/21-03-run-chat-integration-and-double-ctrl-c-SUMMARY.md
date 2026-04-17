---
phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl
plan: "03"
subsystem: ironhermes-cli/main
tags: [tui, ctrl-c, tokio-select, cancellation-token, integration, static-grep]
dependency_graph:
  requires:
    - phase: 21-01
      provides: DoubleCtrlCState, ActivityState, StatusLineState pure cores
    - phase: 21-02
      provides: TuiHandle with watch channels and render loop
  provides:
    - run_chat TUI integration (spawn, activity publishing, shutdown)
    - Double ctrl-c state machine wired into tokio::select! + rustyline
    - Per-turn child CancellationToken pattern preventing token poisoning
    - 3rd-press emergency exit(130) within 3s window
    - DECSTBM scroll region reserving bottom 3 rows
    - Static-grep invariant tests INV-1 through INV-6
  affects:
    - crates/ironhermes-cli/src/main.rs (run_chat fully wired)
tech_stack:
  added: []
  patterns:
    - tokio::select! biased with ctrl_c arm racing agent future
    - Parent/child CancellationToken — parent survives session, children per-turn
    - DECSTBM scroll region for fixed bottom bar
    - prepare_prompt/finish_prompt cursor positioning around readline
    - Static-grep integration tests locking structural invariants
key_files:
  created:
    - crates/ironhermes-cli/tests/run_chat_invariants.rs
    - .planning/todos/completed/2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread.md
  modified:
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/tui/mod.rs
    - crates/ironhermes-cli/src/tui/render.rs
    - crates/ironhermes-cli/src/tui/double_ctrl_c.rs
key_decisions:
  - "CancellationToken must be passed to AgentLoop via with_cancellation_token() — without it, ctrl-c cancels the select but streaming continues"
  - "DoubleCtrlCState checks window even when not in-flight — second ctrl-c at prompt after cancelled turn must trigger ExitCleanly"
  - "Don't reset double_ctrl_c window after cancelled turn — only reset on clean completion"
  - "DECSTBM scroll region reserves bottom 3 rows for prompt + scanner + status bar"
patterns_established:
  - "Cross-boundary ctrl-c: in-flight cancel → prompt exit must share state machine window"
  - "cleanup_on_exit() for hard exit paths that cannot consume Arc<TuiHandle>"
requirements_completed: []
duration: 45min
completed: 2026-04-17
---

# Plan 21-03: Run-chat Integration and Double Ctrl-C Summary

**Integrated TuiHandle + double-ctrl-c state machine into run_chat with tokio::select!, DECSTBM scroll region, and 6 static-grep invariant tests**

## Performance

- **Duration:** ~45 min (across 2 sessions, including 3 UAT-driven bug fixes)
- **Started:** 2026-04-16T18:00:00Z
- **Completed:** 2026-04-17T02:30:00Z
- **Tasks:** 4 (3 auto + 1 human-verify checkpoint)
- **Files modified:** 5

## Accomplishments
- run_chat spawns TuiHandle, publishes ActivityState from streaming/tool callbacks, shuts down on exit
- Double ctrl-c via tokio::select! + DoubleCtrlCState: first press cancels turn, second within 1.5s exits cleanly
- 3rd-press emergency exit(130) prevents hung-shutdown footgun
- DECSTBM scroll region keeps status bar fixed while content scrolls
- 6 static-grep invariant tests lock structural guarantees
- All 9 manual QA scenarios passed

## Task Commits

1. **Task 1: Wire TuiHandle into run_chat** — `1e85ce7` (feat)
2. **Task 2: tokio::select! + DoubleCtrlCState + emergency ctrl-c** — `d438a18` (feat)
3. **Task 3: Static-grep invariant tests + move rolled-in todo** — `bbca896` (feat)
4. **Task 4: Manual QA** — all 9 VALIDATION.md scenarios PASS

**UAT-driven fixes:**
- `df39ebb` — DECSTBM scroll region + prompt positioning + cleanup_on_exit
- `7ac8b73` — Wire CancellationToken into AgentLoop (fix: streaming continued after ctrl-c)
- `9d8e4ee` — Double ctrl-c exits when second press hits rustyline at prompt
- `0f7bc48` — Preserve double-ctrl-c window after cancelled turn

## Files Created/Modified
- `crates/ironhermes-cli/src/main.rs` — TuiHandle wiring, tokio::select!, double ctrl-c, DECSTBM prompt positioning
- `crates/ironhermes-cli/src/tui/render.rs` — DECSTBM scroll region, cleanup_on_exit, resize tracking, prepare/finish_prompt
- `crates/ironhermes-cli/src/tui/mod.rs` — Re-export prepare_prompt, finish_prompt
- `crates/ironhermes-cli/src/tui/double_ctrl_c.rs` — Cross-boundary window check (not-in-flight within window → ExitCleanly)
- `crates/ironhermes-cli/tests/run_chat_invariants.rs` — 6 invariant tests (INV-1 through INV-6)

## Decisions Made
- CancellationToken must be forwarded to AgentLoop — discovered during UAT when ctrl-c didn't stop streaming
- DoubleCtrlCState window must persist across in-flight/prompt boundary — discovered during UAT when second ctrl-c at prompt showed hint instead of exiting
- Don't reset window after cancelled turn — only on clean completion
- DECSTBM reserves bottom 3 rows instead of overwriting — cleaner than cursor juggling

## Deviations from Plan

### Auto-fixed Issues

**1. Missing CancellationToken wiring to AgentLoop**
- **Found during:** Task 4 (UAT scenario 4)
- **Issue:** chat_cancel_token was cancelled but never passed to AgentLoop.with_cancellation_token()
- **Fix:** Added cancel_token parameter to run_agent_turn, wired via .with_cancellation_token()
- **Committed in:** `7ac8b73`

**2. Cross-boundary double ctrl-c not handled**
- **Found during:** Task 4 (UAT scenario 5)
- **Issue:** Second ctrl-c at prompt after cancelled turn hit rustyline which didn't consult DoubleCtrlCState; state machine returned ShowPromptHint when not in-flight
- **Fix:** State machine checks window even when not in-flight; rustyline Interrupted branch now consults double_ctrl_c
- **Committed in:** `9d8e4ee`

**3. Window reset after cancelled turn**
- **Found during:** Task 4 (UAT scenario 5, second attempt)
- **Issue:** double_ctrl_c.reset() ran after the cancelled turn resolved, clearing the window before the second press
- **Fix:** Only reset on clean completion (check chat_cancel_token.is_cancelled())
- **Committed in:** `0f7bc48`

---

**Total deviations:** 3 auto-fixed (all discovered via manual QA)
**Impact on plan:** All fixes required for correct ctrl-c behavior. No scope creep.

## UAT Results

| # | Scenario | Result |
|---|----------|--------|
| 1 | Status line present and colored | PASS |
| 2 | Knight rider animates during turn | PASS |
| 3 | Scanner hides when idle | PASS |
| 4 | First ctrl-c cancels mid-stream | PASS |
| 5 | Second ctrl-c within 1.5s exits | PASS |
| 6 | Ctrl-c at prompt doesn't exit | PASS |
| 7 | 3rd ctrl-c emergency escape | PASS |
| 8 | Terminal resize | PASS |
| 9 | Non-tty pipe | PASS |

## Issues Encountered
- `cargo run -- chat --execute` flag is top-level (`-e`), not on the `chat` subcommand — UAT scenario 9 needed adjusted invocation

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Phase 21 fully integrated and UAT-verified
- All TUI features operational: status bar, knight-rider scanner, double ctrl-c
- 77 bin tests + 6 invariant tests + 10 integration tests all green
- Zero new dependencies added (INV-6 enforced)

---
*Phase: 21-commandline-ui-update-polish-cli-ux-including-graceful-doubl*
*Completed: 2026-04-17*
