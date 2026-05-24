# Phase 36 — Deferred Work Backlog

The following items surfaced during Phase 36 research/planning but were
intentionally scoped OUT of this phase. Each is tracked here for the next
roadmap planning pass so they are not silently forgotten.

## 1. Web UI never intercepts slash commands

Source: 36-RESEARCH.md "Cross-Interface Wiring Survey"
Evidence: crates/iron_hermes_ui/src/server/state.rs:208 — run_web_turn() goes
directly to runtime.run_turn() without slash-command interception. The
AppState.command_router is held but only used by the list_slash_commands()
REST endpoint, never as part of message routing.
Impact: Slash commands typed in the web UI fall through to the agent as
plain text instead of being dispatched to handlers. /stop, /model, /new,
/memory, etc. silently no-op or get echoed.
Effort estimate: medium — needs interception logic in run_web_turn() +
CommandContext construction symmetric to handle_slash_command.
Status: candidate for a future Phase 36.x or a dedicated web-UI parity
phase. NOT in current ROADMAP.

## 2. Per-turn LLM cancellation still missing on gateway

Source: 36-RESEARCH.md, locked at handler.rs:1032 ("gateway has no per-turn
cancel today")
Impact: /stop on gateway today only kills subagent processes via
process_registry; it does not interrupt an in-flight LLM call on the
parent turn. The user can wait, but Ctrl-C-equivalent semantics are not
available.
Effort estimate: high — requires runtime support for cancelable LLM
requests across all providers + per-session cancellation token plumbing.
Status: separate phase. Blocks any future move from D-03's two-state
AtomicBool to the richer enum AgentState { Idle, Running, Cancelling,
Queued } that 36-CONTEXT.md Deferred Ideas references.

## 3. CLI / TUI / gateway running-agent state model not unified

Source: 36-RESEARCH.md "Cross-Interface Wiring Survey"
Evidence: CLI uses a process-local agent_running flag set at
main.rs:1707/2024; TUI derives from app.pending_rx.is_some() at
tui_rata/commands.rs:537; gateway uses per-session
GatewaySession.running (this phase). Three different mechanisms.
Impact: Future maintenance hazard. A behavior change to the guard
semantics has to land in three places.
Effort estimate: medium — extract a shared RunningAgentRegistry trait
in ironhermes-core that all three interfaces implement. Mirrors the
MemoryManagerHandle / McpReloader trait patterns from Phase 20 / 21.2.
Status: cleanup phase candidate.

## 4. /approve and /deny bypass list addition (deferred per D-01)

Source: 36-CONTEXT.md D-01
Trigger: when the real approval queue is implemented (separate phase),
add "approve" and "deny" to the is_bypass() match arm in
crates/ironhermes-gateway/src/handler.rs and update test coverage to
include test_approve_bypasses_guard / test_deny_bypasses_guard.
Tracker: TODO comment is already in the is_bypass() body per Plan 02
Task 2 spec.

---

Phase 36 closed: 2026-05-24 (planning) — GW-05 status flipped in
REQUIREMENTS.md upon Plan 03 merge.
