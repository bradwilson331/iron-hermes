# Phase 32.3.1 Handoff — Fix delegate_task Kill Abort Wiring

> Drop this in via `@.planning/phases/32.3.1-.../HANDOFF.md` when /gsd-plan-phase 32.3.1 starts.
> Delete after PLAN is written.

## Discovery context

Surfaced during Phase 26.7.1 Wave 2 UAT (2026-05-19). User clicked `/api/agents/kill` 5 separate times across 53 seconds against 3 live subagents. All requests returned **200 OK** but **none of the agents terminated** — every subsequent `/api/agents/list` continued to return all 3 as live.

The 26.7.1 frontend (HOLD-N=5s fade machinery, `diff_terminations` pure fn, push-driven `use_effect` on `subagent_events`) had nothing to render because the upstream registry never reflected a termination.

## Root cause

**`DelegateTaskTool::with_shrike_handle_map(...)` has zero callers anywhere in the workspace.** The wiring is defined but never invoked.

Chain that's supposed to work:
1. `DelegateTaskTool::execute_batch` spawns each subagent via `tokio::spawn` (`crates/ironhermes-tools/src/delegate_task.rs:356`)
2. The spawned JoinHandle is supposed to be registered into `ShrikeService::active_handles` via `self.shrike_handles` (line 455)
3. `ShrikeService::kill(id)` looks up the handle and calls `JoinHandle::abort()` (`crates/ironhermes-agent/src/shrike.rs:122-167`)
4. Aborted task drops its `RegistrationGuard`, which unregisters the agent from `SubagentRegistry` via an OS-thread `block_on(unregister_internal)` bridge (`crates/ironhermes-agent/src/subagent_registry.rs:85-121`)
5. `GET /api/agents/list` no longer returns the agent → frontend `diff_terminations` adds the missing id to the HOLD set → 5s fade

What actually happens:
- `shrike_handles` is always `None` because `with_shrike_handle_map` is never called at construction
- `shrike.active_handles` map is always empty
- `shrike.kill(id)` only cancels the `CancellationToken`, then logs an info line and returns `Some(KillResult)` (returns 200 OK)
- The agent_loop is mid-LLM-stream and never polls the cancel token until the stream completes
- Frontend shows no termination

## What the existing code itself documents

`crates/ironhermes-tools/src/delegate_task.rs:469-480` openly states this is incomplete:

```
// NOTE: handles.push(handle) below consumes the JoinHandle
// (JoinHandle is not Clone). To both `.push` and register, we
// need an Arc-shared handle — but that's incompatible with
// `handle.await` in the result-collection loop. For Plan 03
// we keep the result-collection path canonical (push handle
// to handles Vec) and instead expose the abort path via a
// direct ShrikeService::kill against the registry id. The
// handle map is populated in tests via direct insert; runtime
// batch correlation is Plan 04 scope.
let _ = batch_task_key; // reserved for Plan 04 wiring
handles.push(handle);
```

Plan 32.3-04 only built the REST endpoints and never closed this loop.

There is also a key-correlation gap: the spawn site uses `batch_task_<index>` keys but `ShrikeService::kill(id)` is called with the real `SubagentId` (e.g. `sub_xxxx`). Even with wiring, lookups would miss.

## Recommended fix (sketch — verify during /gsd-discuss-phase)

1. **Switch from `JoinHandle` to `AbortHandle`** in `shrike.active_handles`. `AbortHandle` is Clone-ish (you derive it once via `handle.abort_handle()` before pushing the JoinHandle). The original JoinHandle stays in `handles: Vec<JoinHandle<...>>` for the `.await` collection loop. Registry holds AbortHandle which can `.abort()` without consuming.
2. **Wire `with_shrike_handle_map`** at the `DelegateTaskTool` construction site. Search for `DelegateTaskTool::new(` in `app_runtime_factory.rs` and add the chained `.with_shrike_handle_map(shrike.handle_map())` builder call. Three call sites likely: `run_chat`, `run_single`, `run_gateway` (and possibly `iron_hermes_ui::server::state`).
3. **Key by real `SubagentId`, not `batch_task_<index>`.** Move the handle registration from outside the spawn (where only the synthetic batch key is available) to inside the spawn, after `runner.run_child` mints the real id — or thread the real id back out via a `oneshot` channel before the spawn returns. Alternatively, do a post-registration step inside the spawn (after the SubagentRegistry::register_guarded call inside `subagent_runner.rs`) that inserts into `shrike.handle_map()` keyed by the canonical id.
4. **Cooperative-cancel responsiveness in the LLM stream loop.** Even with abort, a wedged HTTP stream is killed by `tokio::task::abort` only at the next yield point. Ensure `select!` over `cancel_token.cancelled()` is woven around `client.chat_stream()` in the agent_loop so cancel responds within tens of ms, not seconds.

## Test coverage needed

- Integration test: register N agents, call `shrike.kill(real_id)`, assert `api_agents_list` returns N-1 within reasonable bound (the OS-thread block_on bridge has a tiny latency).
- Regression test: kill during in-flight LLM stream — agent must exit within ~500ms.
- Static-grep invariant: assert `.with_shrike_handle_map(` appears at every `DelegateTaskTool::new(` construction site.

## References

- `crates/ironhermes-agent/src/shrike.rs:90-167` (ShrikeService::kill)
- `crates/ironhermes-agent/src/subagent_registry.rs:85-155` (RegistrationGuard::drop + unregister_internal)
- `crates/ironhermes-tools/src/delegate_task.rs:154-480` (DelegateTaskTool + execute_batch)
- `crates/ironhermes-tools/src/delegate_task.rs:469-480` (deferred-to-Plan-04 inline comment)
- `crates/iron_hermes_ui/src/server/api.rs:287-302` (REST handler — returns 200 unconditionally)
- `crates/iron_hermes_ui/src/server/state.rs:439-468` (api_agents_kill body)
- UAT log evidence: 2026-05-19 05:03:24/28/32/38, 05:04:17 — 5 kill clicks, 0 terminations

## Out of scope (defer to future phase)

- Multi-host / distributed kill (gateway-vs-CLI agent affinity)
- "Interrupt and resume" semantics — interrupt is already the soft path; kill is the hard path
- `/agents prune` test coverage (separate concern, Plan 32.3-04 already shipped)
