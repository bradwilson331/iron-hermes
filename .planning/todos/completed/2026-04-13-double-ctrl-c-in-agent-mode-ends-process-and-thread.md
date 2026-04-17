---
created: 2026-04-13T02:58:00.000Z
title: Double ctrl-c in agent mode ends process and thread
area: cli
files:
  - crates/ironhermes-cli/src/main.rs
---

## Problem

When in agent mode, pressing `ctrl+c, ctrl+c` (double interrupt) will end the agent process and then the thread. Needs a graceful interrupt behavior where the first `ctrl+c` cancels the current in-flight turn/stream and returns to the prompt, and only the second `ctrl+c` exits — similar to REPL/shell conventions. Current behavior risks losing conversation state on accidental double-tap.

## Solution

TBD. Likely install a `tokio::signal::ctrl_c` handler in agent-mode run loops (`run_once` / `run_agent_turn`) that:
1. First signal: abort the in-flight provider request / tool call, flush partial response, return to input prompt
2. Second signal within a short window: exit cleanly (persist any thread/session state first)
3. Reset the counter on user input or successful turn completion

---

## Resolution

Completed in Phase 21. See:
- Plan `21-03-run-chat-integration-and-double-ctrl-c-PLAN.md`
- D-10..D-14 in `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/21-CONTEXT.md`
- Integration tests locked via INV-1, INV-2, INV-3 in `crates/ironhermes-cli/tests/run_chat_invariants.rs`
