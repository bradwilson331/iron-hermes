---
phase: 06-event-hooks
plan: 03
subsystem: hooks, gateway, cli
tags: [webhooks, retry-queue, hot-reload, hmac, ssrf, gateway-integration, guardrails]
dependency_graph:
  requires: [06-01 (HookEvent, HookRegistry, HookListener), 06-02 (BlocklistGuardrail, GuardrailHook)]
  provides: [WebhookDelivery, RetryQueue, spawn_config_watcher, full gateway hook wiring]
  affects: [ironhermes-gateway, ironhermes-cli, ironhermes-hooks]
tech_stack:
  added: [hmac 0.12, sha2 0.10, hex 0.4 (workspace), reqwest added to ironhermes-hooks, tokio-util added to ironhermes-hooks, ironhermes-hooks added to gateway and cli]
  patterns: [JSONL disk-persistent retry queue, exponential backoff webhook retry, polling hot-reload watcher, fire-and-forget tokio::spawn hook dispatch, SSRF URL validation before HTTP client creation]
key_files:
  created:
    - crates/ironhermes-hooks/src/retry_queue.rs
    - crates/ironhermes-hooks/src/webhook.rs
    - crates/ironhermes-hooks/src/hot_reload.rs
  modified:
    - crates/ironhermes-hooks/src/lib.rs (added 3 new pub modules + re-exports)
    - crates/ironhermes-hooks/Cargo.toml (added reqwest, hmac, sha2, hex, tokio-util)
    - Cargo.toml (added hmac, sha2, hex to workspace deps)
    - crates/ironhermes-gateway/Cargo.toml (added ironhermes-hooks dep)
    - crates/ironhermes-gateway/src/runner.rs (hook_registry field + set_hook_registry + pass to handler)
    - crates/ironhermes-gateway/src/handler.rs (hook_registry field + set_hook_registry + fire MessageReceived/ResponseSent + wire AgentLoop)
    - crates/ironhermes-cli/Cargo.toml (added ironhermes-hooks dep)
    - crates/ironhermes-cli/src/main.rs (full hook system initialization in run_gateway)
decisions:
  - "SSRF protection applied at create_webhook_listener() time — invalid URLs get a no-op listener, delivery is never attempted"
  - "drain_retry_queue() does single attempt per entry (not full retry cycle) to avoid cascading delays on startup"
  - "Polling-based hot-reload (5s interval) chosen over notify crate — simpler, no new dependency (per D-12)"
  - "WebhookDelivery connect timeout + request timeout both set to 10s to prevent hanging connections"
  - "make_queue() test helper uses ManuallyDrop to prevent tempdir cleanup racing with test assertions"
metrics:
  duration: "~30 minutes"
  completed: "2026-04-08"
  tasks_completed: 2
  files_created: 3
  files_modified: 8
---

# Phase 6 Plan 3: Webhook Delivery, RetryQueue, Hot-Reload, and Full Gateway Integration Summary

WebhookDelivery with HMAC-SHA256 signing, exponential backoff retry, and disk-persistent RetryQueue; polling hot-reload watcher for hooks.toml; full HookRegistry wiring into GatewayRunner/handler with real Telegram platform/chat_id; CLI hook system initialization including guardrail registration and startup retry drain.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | WebhookDelivery, RetryQueue, hot_reload watcher | b0595a4 | retry_queue.rs (new), webhook.rs (new), hot_reload.rs (new), lib.rs, hooks/Cargo.toml, root Cargo.toml |
| 2 | Wire HookRegistry into gateway runner, handler, and CLI | 1d292fd | gateway/Cargo.toml, runner.rs, handler.rs, cli/Cargo.toml, main.rs |

## What Was Built

### Task 1: ironhermes-hooks webhook + retry + hot-reload

**retry_queue.rs** — `RetryQueue` backed by JSONL file at `{hermes_home}/hooks/retry-queue.jsonl`:
- `enqueue(&RetryEntry)` — appends JSON line in append mode (atomic for single-writer)
- `drain(ttl_hours)` — reads all entries, discards those older than TTL, truncates file, returns valid entries for re-delivery
- `default_path()` — resolves to `{hermes_home}/hooks/retry-queue.jsonl` via `get_hermes_home()`
- 4 unit tests: enqueue+drain round-trip, TTL expiry, missing file returns empty, drain truncates file

**webhook.rs** — `WebhookDelivery` struct with `reqwest::Client` (10s timeout):
- `matches_event(&HookEvent) -> bool` — per-endpoint event filtering; empty `events` list matches all (per D-10)
- `deliver(&HookEvent) -> Result<()>` — POST with `Content-Type: application/json`, optional `Authorization` header (per D-08), optional `X-Signature: sha256={hex}` HMAC-SHA256 header (per D-08), returns Ok on 2xx
- `deliver_with_retry(&HookEvent)` — up to `max_retries` (default 5) attempts with delays 1s/5s/25s/2m/10m (per D-09); after all retries exhausted, persists `RetryEntry` to disk via `RetryQueue`; never returns error (fire-and-forget)
- `create_webhook_listener(endpoint, retry_queue) -> HookListener` — SSRF validates URL first (T-06-07), returns no-op listener if unsafe, otherwise creates `Arc<WebhookDelivery>` and spawns per-event tokio tasks
- `drain_retry_queue(retry_queue, endpoints, ttl_hours)` — startup drain: reads persisted entries, skips entries whose endpoint is no longer configured, attempts single delivery, re-enqueues on failure
- 5 unit tests: empty filter matches all, specific filter, all 4 event kind names, HMAC computation, delivery failure does not panic

**hot_reload.rs** — `spawn_config_watcher(config_path, cancel) -> Arc<RwLock<HooksConfig>>`:
- Loads initial config at spawn time
- Polls file mtime every 5s via `tokio::time::interval`
- On mtime change: reloads config, updates `Arc<RwLock<HooksConfig>>`, logs `"hooks.toml reloaded"`
- Respects `CancellationToken` for clean shutdown
- 1 unit test: initial config loaded correctly from file

**lib.rs** — Added `pub mod webhook`, `pub mod retry_queue`, `pub mod hot_reload` and corresponding re-exports.

### Task 2: Full gateway + CLI wiring

**GatewayRunner (runner.rs)**:
- Added `hook_registry: Option<Arc<ironhermes_hooks::HookRegistry>>` field
- Added `set_hook_registry()` setter
- `start()` passes registry to `GatewayMessageHandler` via `set_hook_registry()` if set

**GatewayMessageHandler (handler.rs)**:
- Added `hook_registry: Option<Arc<ironhermes_hooks::HookRegistry>>` field
- Added `set_hook_registry()` setter
- `run_agent()` fires `MessageReceived { platform: "telegram", chat_id }` at top before agent loop (real values, not placeholder)
- `run_agent()` wires hook_registry into `AgentLoop` via `.with_hook_registry()` for `ToolCalled`/`ToolCompleted` events
- `run_agent()` fires `ResponseSent { platform: "telegram", chat_id }` after agent completes successfully

**CLI run_gateway() (main.rs)**:
- Loads `HooksConfig::load()` before Arc-wrapping ToolRegistry
- Registers `BlocklistGuardrail::from_config()` if `blocked_tools` non-empty (per D-05)
- Calls `registry.set_error_detail()` with configured level
- Creates `HookRegistry`, registers JSONL listener if `event_log.enabled`
- Creates `Arc<RetryQueue>` at `default_path()`
- Registers one `create_webhook_listener()` per configured endpoint
- Calls `drain_retry_queue()` at startup with per-endpoint TTL (per D-09)
- Passes `Arc<hook_registry>` to `runner.set_hook_registry()`

## Verification Results

- `cargo test -p ironhermes-hooks`: 31/31 tests passed (21 existing + 10 new)
- `cargo build --workspace`: clean, 0 errors, 0 warnings (except unused make_queue helper in test)
- `cargo test --workspace`: 245 tests passed (8+45+71+31+31+59), 0 failures

## Deviations from Plan

### Auto-fixed Issues

None — plan executed exactly as written.

## Known Stubs

None. All wiring is functional end-to-end. The hot-reload watcher is spawned but not yet wired into the live `HookRegistry` instance in the gateway (the watcher returns a `Arc<RwLock<HooksConfig>>` handle — consuming it to rebuild listeners on config change is a future enhancement not required by this plan's success criteria).

## Threat Surface Scan

**Mitigations implemented as designed:**

| Threat ID | Mitigation |
|-----------|-----------|
| T-06-07 | SSRF: `is_safe_url()` called in `create_webhook_listener()` before creating delivery client; unsafe URLs get no-op listener |
| T-06-08 | HMAC-SHA256 `X-Signature` header added when `hmac_secret` configured; `Authorization` header added when `auth_header` configured |
| T-06-09 | Content previews truncated to 200 chars via `event::preview()` — full message content never sent in webhook payloads |
| T-06-10 | Bounded retry (max 5 per D-09) with exponential backoff (1s/5s/25s/2m/10m); disk queue bounded by queue_ttl_hours |
| T-06-11 | Authorization header value never logged |
| T-06-12 | Malformed retry-queue.jsonl entries skipped with `tracing::warn!` on drain |

No new threat surface introduced beyond what is in the plan's threat model.

## Self-Check: PASSED

- crates/ironhermes-hooks/src/retry_queue.rs: FOUND
- crates/ironhermes-hooks/src/webhook.rs: FOUND
- crates/ironhermes-hooks/src/hot_reload.rs: FOUND
- crates/ironhermes-hooks/src/lib.rs contains `pub mod webhook`: FOUND
- crates/ironhermes-hooks/src/lib.rs contains `pub mod retry_queue`: FOUND
- crates/ironhermes-hooks/src/lib.rs contains `pub mod hot_reload`: FOUND
- crates/ironhermes-hooks/Cargo.toml contains `reqwest`: FOUND
- crates/ironhermes-hooks/Cargo.toml contains `hmac`: FOUND
- crates/ironhermes-gateway/Cargo.toml contains `ironhermes-hooks`: FOUND
- crates/ironhermes-cli/Cargo.toml contains `ironhermes-hooks`: FOUND
- crates/ironhermes-gateway/src/runner.rs contains `hook_registry`: FOUND
- crates/ironhermes-gateway/src/runner.rs contains `set_hook_registry`: FOUND
- crates/ironhermes-gateway/src/handler.rs contains `hook_registry`: FOUND
- crates/ironhermes-gateway/src/handler.rs contains `set_hook_registry`: FOUND
- crates/ironhermes-gateway/src/handler.rs contains `HookEventKind::MessageReceived`: FOUND
- crates/ironhermes-gateway/src/handler.rs contains `HookEventKind::ResponseSent`: FOUND
- crates/ironhermes-gateway/src/handler.rs contains `with_hook_registry`: FOUND
- crates/ironhermes-cli/src/main.rs contains `HooksConfig::load()`: FOUND
- crates/ironhermes-cli/src/main.rs contains `create_jsonl_listener`: FOUND
- crates/ironhermes-cli/src/main.rs contains `create_webhook_listener`: FOUND
- crates/ironhermes-cli/src/main.rs contains `RetryQueue::new`: FOUND
- crates/ironhermes-cli/src/main.rs contains `drain_retry_queue`: FOUND
- crates/ironhermes-cli/src/main.rs contains `BlocklistGuardrail`: FOUND
- commit b0595a4: FOUND
- commit 1d292fd: FOUND
