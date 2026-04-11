---
phase: 06-event-hooks
verified: 2026-04-08T00:00:00Z
status: passed
score: 3/3 must-haves verified
overrides_applied: 0
gaps: []
deferred: []
human_verification: []
---

# Phase 6: Event Hooks Verification Report

**Phase Goal:** Add an event hook system so agent lifecycle events are observable, interceptable, and forwardable. New ironhermes-hooks crate with HookRegistry, guardrail interception, structured event logging, and webhook delivery with persistent retry.
**Verified:** 2026-04-08
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Every message received, tool called, and response sent produces a structured log entry via the hook registry | VERIFIED | All four event kinds exist in `event.rs` with correct fields; all four fire points wired in `agent_loop.rs` (lines 131, 317, 337, 348, 200); JSONL listener registered in CLI `run_gateway()` |
| 2 | A configured guardrail hook can intercept a tool call before dispatch and block it, returning a clear error to the agent | VERIFIED | `guardrail.rs` has `GuardrailHook` trait + `BlocklistGuardrail`; `registry.rs` dispatches through guardrail chain before `tool.execute()`; `BlocklistGuardrail::from_config()` wired in CLI |
| 3 | A configured webhook endpoint receives hook events as HTTP POST requests when events fire | VERIFIED | `webhook.rs` has `WebhookDelivery::deliver()` with POST + auth + HMAC; `create_webhook_listener()` registers per-endpoint; wired into CLI `run_gateway()` with retry drain |

**Score:** 3/3 truths verified

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-hooks/src/lib.rs` | All modules exported | VERIFIED | 8 modules declared: config, event, guardrail, hot_reload, log_writer, registry, retry_queue, webhook |
| `crates/ironhermes-hooks/src/event.rs` | 4 event kinds with JSONL | VERIFIED | `HookEventKind` enum has all 4 variants with correct fields; serde flatten places `kind` tag at top level |
| `crates/ironhermes-hooks/src/guardrail.rs` | GuardrailHook trait + BlocklistGuardrail | VERIFIED | Trait with `check()` and `name()`; `GuardrailDecision` (Allow/Warn/Block); `BlocklistGuardrail::from_config()` |
| `crates/ironhermes-hooks/src/webhook.rs` | WebhookDelivery with auth/HMAC/retry | VERIFIED | `deliver()` sets `Authorization` + `X-Signature: sha256=...` headers; `deliver_with_retry()` with 5-attempt exponential backoff (1s/5s/25s/2m/10m); SSRF URL validation |
| `crates/ironhermes-hooks/src/retry_queue.rs` | JSONL-backed persistent queue | VERIFIED | `RetryQueue` with `enqueue()` / `drain(ttl_hours)` backed by JSONL file; TTL expiry; truncates file on drain |
| `crates/ironhermes-hooks/src/hot_reload.rs` | Polling config watcher | VERIFIED | `spawn_config_watcher()` polls mtime every 5s; respects `CancellationToken`; returns `Arc<RwLock<HooksConfig>>` |
| `crates/ironhermes-agent/src/agent_loop.rs` | 4 hook fire points | VERIFIED | `MessageReceived` at top of `run()` (line 131); `ToolCalled` before dispatch in `execute_tool_call()` (line 317); `ToolCompleted` in both Ok+Err branches (lines 337, 348); `ResponseSent` before return (line 200) |
| `crates/ironhermes-tools/src/registry.rs` | Guardrail intercept in dispatch() | VERIFIED | `guardrails: Vec<Box<dyn GuardrailHook>>` field; dispatch() loops over guardrails before `tool.execute()`; Allow/Warn/Block handled correctly (lines 87-109) |
| `crates/ironhermes-gateway/src/handler.rs` | Gateway fires MessageReceived/ResponseSent with real platform/chat_id | VERIFIED | `run_agent()` fires `MessageReceived { platform: "telegram", chat_id: event.chat_id }` at top; `ResponseSent` after agent succeeds; `with_hook_registry()` wires registry into AgentLoop |
| `crates/ironhermes-cli/src/main.rs` | Full hook system initialization | VERIFIED | `HooksConfig::load()`, `BlocklistGuardrail::from_config()`, `HookRegistry::new()`, `create_jsonl_listener()`, `RetryQueue::new(default_path())`, `create_webhook_listener()` per endpoint, `drain_retry_queue()` on startup, `runner.set_hook_registry()` |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `agent_loop.rs` | `HookRegistry::fire()` | `fire_hook()` helper | WIRED | `fire_hook()` creates `HookEvent::new(&self.request_id, kind)` and calls `registry.fire(event)` |
| `registry.rs (tools)` | `GuardrailHook::check()` | dispatch() guardrail loop | WIRED | Same `&args` reference checked by guardrails and then passed to `tool.execute()` (T-06-04 satisfied) |
| `cli/main.rs` | `HookRegistry` | `runner.set_hook_registry()` | WIRED | `runner.set_hook_registry(hook_registry)` called before `runner.start()` |
| `gateway/runner.rs` | `GatewayMessageHandler` | `handler.set_hook_registry()` | WIRED | Confirmed via grep: runner passes registry to handler in `start()` |
| `webhook.rs` | `RetryQueue` | `deliver_with_retry()` → `enqueue()` | WIRED | After all retries exhausted, `RetryEntry` serialized and appended to JSONL queue file |
| `hot_reload.rs` | `HooksConfig` | mtime polling, `Arc<RwLock<HooksConfig>>` | WIRED | Config handle returned; watcher updates shared state on file change |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|--------------|--------|--------------------|--------|
| `agent_loop.rs` MessageReceived | `content_preview` | Last user message in `messages` Vec | Yes — real message text | FLOWING |
| `agent_loop.rs` ToolCalled | `args_preview` | `tool_call.function.arguments` from LLM | Yes — real args string | FLOWING |
| `agent_loop.rs` ToolCompleted | `result_preview` + `duration_ms` | `dispatch_result` + `Instant::elapsed()` | Yes — actual tool output | FLOWING |
| `handler.rs` MessageReceived | `chat_id`, `content_preview` | `event.chat_id`, `event.content` from Telegram | Yes — real Telegram values | FLOWING |
| `handler.rs` ResponseSent | `response` | `result.final_response` from AgentLoop | Yes — actual agent response | FLOWING |
| `webhook.rs` deliver | POST body | `serde_json::to_string(event)` | Yes — full structured event | FLOWING |
| `retry_queue.rs` | JSONL entries | `enqueue()` on delivery failure | Yes — real event + metadata | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| All workspace tests pass | `cargo test --workspace` | 8+45+71+31+31+59 = 245 tests, 0 failures, 3 ignored | PASS |
| ironhermes-hooks crate exports all types | Module check in lib.rs | 8 pub mod + 17 pub use re-exports | PASS |
| HMAC produces 64-char hex (256-bit) | Test `test_hmac_signature_computation` | Verified in test: sig.len()==64, all hex digits, deterministic, key-dependent | PASS |

### Requirements Coverage

| Requirement | Plans | Description | Status | Evidence |
|-------------|-------|-------------|--------|----------|
| HOOK-01 | 06-01 | Four lifecycle events with JSONL logging | SATISFIED | All 4 `HookEventKind` variants exist with correct field sets; `create_jsonl_listener()` appends JSON lines; AgentLoop fires all 4 |
| HOOK-02 | 06-02 | Guardrail interception (GuardrailHook, BlocklistGuardrail, ToolRegistry wiring) | SATISFIED | `GuardrailHook` trait with `check()`; `BlocklistGuardrail` with `from_config()`; `dispatch()` checks all guardrails before tool execution |
| HOOK-03 | 06-03 | Webhook delivery with auth, HMAC, retry queue, hot-reload, gateway integration | SATISFIED | `WebhookDelivery` with `Authorization` + `X-Signature` headers; 5-attempt exponential retry; JSONL `RetryQueue`; `spawn_config_watcher()` with 5s polling; full gateway + CLI wiring |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| None found | — | — | — | — |

Scanned all phase-created and modified files. No TODO/FIXME/placeholder comments, no stub return values, no empty handlers, no hardcoded empty data flowing to renderable output. The hot_reload watcher returns an `Arc<RwLock<HooksConfig>>` handle but does not rebuild live listeners on config change — this is documented in the SUMMARY as an intentional future enhancement, not a stub (the watcher itself is fully functional).

### Human Verification Required

None. All requirements are verifiable programmatically and through code inspection. The webhook delivery test (`test_deliver_failure_does_not_panic`) uses TEST-NET addresses to verify error handling without requiring a live endpoint.

### Gaps Summary

No gaps. All three success criteria from the ROADMAP are met:

1. **HOOK-01** — Four lifecycle events fire at the correct agent loop points and write structured JSONL. The event model is correct (`kind` tag at top level via `#[serde(flatten)]`), all four `HookEventKind` variants exist with the required fields, and all four fire points are wired in `agent_loop.rs`.

2. **HOOK-02** — The guardrail interception system is complete. `GuardrailHook` trait, `BlocklistGuardrail`, and `ToolRegistry::dispatch()` intercept are all substantive and wired. The same `&args` reference is used for guardrail checking and tool execution (T-06-04). Error detail level controls whether tool names appear in block messages (T-06-05).

3. **HOOK-03** — Webhook delivery with `Authorization` header auth, HMAC-SHA256 signing, exponential backoff retry (up to 5 attempts), disk-persistent `RetryQueue` (JSONL at `{hermes_home}/hooks/retry-queue.jsonl`), SSRF validation at listener creation, hot-reload watcher (5s mtime polling), and full gateway + CLI wiring are all implemented and tested.

The `cargo test --workspace` run confirms 245 tests passing with 0 failures.

---

_Verified: 2026-04-08_
_Verifier: Claude (gsd-verifier)_
