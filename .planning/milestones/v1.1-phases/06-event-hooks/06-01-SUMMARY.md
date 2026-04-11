---
phase: 06-event-hooks
plan: 01
subsystem: hooks
tags: [hooks, events, jsonl, observability, agent-loop]
dependency_graph:
  requires: []
  provides: [ironhermes-hooks crate, HookEvent, HookRegistry, JSONL event log, AgentLoop hook wiring]
  affects: [ironhermes-agent, future plans 06-02 guardrails, 06-03 webhooks]
tech_stack:
  added: [ironhermes-hooks crate, toml 0.8]
  patterns: [fire-and-forget tokio::spawn dispatch, floor_char_boundary truncation, append-mode JSONL logging]
key_files:
  created:
    - crates/ironhermes-hooks/Cargo.toml
    - crates/ironhermes-hooks/src/lib.rs
    - crates/ironhermes-hooks/src/event.rs
    - crates/ironhermes-hooks/src/config.rs
    - crates/ironhermes-hooks/src/registry.rs
    - crates/ironhermes-hooks/src/log_writer.rs
  modified:
    - Cargo.toml (added ironhermes-hooks workspace member)
    - crates/ironhermes-agent/Cargo.toml (added ironhermes-hooks dep)
    - crates/ironhermes-agent/src/agent_loop.rs (hook_registry field + 4 fire points)
decisions:
  - "HookEvent uses #[serde(flatten)] on HookEventKind so kind tag appears at top level in JSON"
  - "fire_hook() is a no-op when hook_registry is None — zero cost for callers that don't configure hooks"
  - "platform and chat_id in MessageReceived/ResponseSent default to 'agent'/empty at AgentLoop level; gateway layer sets real values"
metrics:
  duration: "~15 minutes"
  completed: "2026-04-08"
  tasks_completed: 2
  files_created: 6
  files_modified: 3
---

# Phase 6 Plan 1: Event Hooks — Core Crate and AgentLoop Wiring Summary

New ironhermes-hooks crate with HookEvent/HookRegistry/JSONL writer; AgentLoop wired at all four lifecycle points (MessageReceived, ToolCalled, ToolCompleted, ResponseSent) via fire-and-forget tokio::spawn.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Create ironhermes-hooks crate | 39a4667 | 6 new files in crates/ironhermes-hooks/ + Cargo.toml workspace |
| 2 | Wire HookRegistry into AgentLoop | 7664b4c | ironhermes-agent/Cargo.toml + agent_loop.rs |

## What Was Built

### Task 1: ironhermes-hooks crate

**event.rs** — `HookEvent` struct with `id` (UUID v4), `request_id` (per-turn correlation UUID), `timestamp` (UTC), and `HookEventKind` enum with 4 variants:
- `MessageReceived { platform, chat_id, content_preview }`
- `ToolCalled { tool_name, args_preview }`
- `ToolCompleted { tool_name, success, result_preview, duration_ms }`
- `ResponseSent { platform, chat_id, response_preview }`

`preview()` helper uses `floor_char_boundary` for safe UTF-8 truncation (T-06-01 mitigation). `HookEventKind` uses `#[serde(tag = "kind", rename_all = "snake_case")]` with `#[serde(flatten)]` on the struct field so the `kind` tag appears at the top level of serialized JSON.

**config.rs** — `HooksConfig` with `EventLogConfig` (enabled/path), `blocked_tools`, `webhooks: Vec<WebhookEndpointConfig>`, and `ErrorDetailLevel` (Full/Minimal). Loaded from `{hermes_home}/hooks.toml` via `toml::from_str`, falls back to `Default` if file is absent.

**registry.rs** — `HookRegistry` with `fire(&self, event)` that iterates listeners and spawns each in a fresh `tokio::spawn` task (T-06-03 mitigation — never blocks caller). `HookListener = Arc<dyn Fn(HookEvent) + Send + Sync>`.

**log_writer.rs** — `create_jsonl_listener(path)` returns a `HookListener` that appends `serde_json::to_string(event) + "\n"` to `events.jsonl` using `OpenOptions::append(true)`. Errors are `tracing::warn!` only — never panic (T-06-01 mitigation).

**lib.rs** — re-exports all public types.

### Task 2: AgentLoop wiring

- `AgentLoop` struct gains `hook_registry: Option<Arc<HookRegistry>>` and `request_id: String` (initialized to `Uuid::new_v4()` in `new()`).
- `with_hook_registry()` builder method sets the registry.
- `fire_hook()` private helper creates `HookEvent::new(&self.request_id, kind)` and calls `registry.fire()` — no-op when registry is None.
- Four wiring points:
  - **MessageReceived** — top of `run()`, from last user message in history
  - **ToolCalled** — in `execute_tool_call()` before arg parsing and dispatch
  - **ToolCompleted** — after `registry.dispatch()` for both Ok and Err paths, includes `duration_ms`
  - **ResponseSent** — just before `Ok(AgentResult {...})` return

## Verification Results

- `cargo test -p ironhermes-hooks`: 14/14 tests passed
- `cargo test --workspace`: 222/222 tests passed (0 regressions)

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

None. All wiring is functional. `platform` and `chat_id` in `MessageReceived`/`ResponseSent` are set to `"agent"` / empty string at the `AgentLoop` level by design — the gateway layer (Plan 03 or later) will provide real values when calling from Telegram/other platforms.

## Threat Surface Scan

No new network endpoints or auth paths introduced. The JSONL log write is covered by T-06-01 (content previews truncated to 200 chars, file in user home directory). HookRegistry spawning is covered by T-06-03. No new threat surface beyond what is in the plan's threat model.

## Self-Check: PASSED

- crates/ironhermes-hooks/src/event.rs: FOUND
- crates/ironhermes-hooks/src/config.rs: FOUND
- crates/ironhermes-hooks/src/registry.rs: FOUND
- crates/ironhermes-hooks/src/log_writer.rs: FOUND
- crates/ironhermes-hooks/src/lib.rs: FOUND
- commit 39a4667: FOUND
- commit 7664b4c: FOUND
