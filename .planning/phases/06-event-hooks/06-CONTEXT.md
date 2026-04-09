# Phase 6: Event Hooks - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Add an event hook system to IronHermes so that agent lifecycle events (message received, tool called, tool completed, response sent) are observable, interceptable, and forwardable. Delivers a new `ironhermes-hooks` crate with a HookRegistry, guardrail interception, structured event logging, and webhook delivery with persistent retry.

</domain>

<decisions>
## Implementation Decisions

### Hook Event Model
- **D-01:** Four lifecycle events: `message_received`, `tool_called`, `tool_completed`, `response_sent`. The first three satisfy HOOK-01; `tool_completed` adds observability for tool outcomes (success/error).
- **D-02:** Configurable verbosity per hook registration — always include metadata (event type, timestamp, session/chat ID, type-specific fields like tool name). Full content (complete message text, full tool args, full tool result) is opt-in per subscriber.
- **D-03:** Per-turn `request_id` (UUID) threaded through all events in a single message-to-response cycle. Enables log correlation across the full request lifecycle.
- **D-04:** Structured event log written to a dedicated JSONL file (`~/.ironhermes/hooks/events.jsonl`), not to the tracing subsystem. Isolated from application logs, easy to tail/parse.

### Guardrail Design
- **D-05:** Two-layer guardrail system — config-driven blocklist for simple rules (tool name matching) + `GuardrailHook` trait for complex logic (arg inspection, rate limiting). Config blocklist checked first, trait hooks second.
- **D-06:** Configurable error detail level — default is clear error with reason (e.g., "Tool 'terminal' blocked by guardrail: untrusted context"). Config option to reduce to generic "Tool call blocked by security policy" for high-security deployments.
- **D-07:** Three guardrail outcomes: `allow`, `warn` (log + emit event but proceed), `block`. Warn mode enables monitoring before enforcing.

### Webhook Delivery
- **D-08:** Both authentication methods supported per endpoint — static `Authorization` header (Bearer token, API key) for simple setups, and HMAC-SHA256 payload signing (`X-Signature` header) for security-conscious deployments. Configured per webhook endpoint.
- **D-09:** Persistent retry queue — failed deliveries queued to disk, retried with exponential backoff. Survives restarts. Sensible defaults (max 5 retries, exponential backoff 1s/5s/25s/2m/10m, discard after 24h) with configurable `max_retries` and `queue_ttl` overrides.
- **D-10:** Per-endpoint event filter — each webhook config specifies which event types it subscribes to. Reduces noise and bandwidth for consumers that only care about specific events.

### Hook Configuration
- **D-11:** Dedicated config file at `~/.ironhermes/hooks.toml` — separate from main config. Contains webhook endpoints, blocklist rules, event filters, verbosity settings.
- **D-12:** Hot-reload on file change — watch `hooks.toml` for modifications and reload hook configuration without restart. Better DX for iterating on hook rules.

### Claude's Discretion
- Internal HookEvent enum design and serialization format
- JSONL rotation/size-limiting strategy for the event log file
- File watcher implementation (notify crate vs polling)
- Retry queue file format and location
- GuardrailHook trait exact method signatures
- How trait-based guardrail hooks are registered in code (likely in `register_defaults()` pattern)

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Existing Rust Codebase
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop::run(), StreamCallback pattern, execute_tool_call — primary integration points for hook event emission
- `crates/ironhermes-tools/src/registry.rs` — ToolRegistry::dispatch() — interception point for guardrail hooks before tool.execute()
- `crates/ironhermes-core/src/config.rs` — Config structure for adding HooksConfig
- `crates/ironhermes-gateway/src/handler.rs` — StreamCallback usage pattern, shows how fire-and-forget callbacks are wired
- `crates/ironhermes-gateway/src/runner.rs` — GatewayRunner where message_received events originate
- `crates/ironhermes-cron/src/delivery.rs` — Existing webhook delivery pattern (URL resolution, reqwest usage, floor_char_boundary for truncation)

### Python Reference (loosely inspired)
- `~/code/hermes-agent/` — Check for any hook/event/middleware patterns worth borrowing. Not a strict port.

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph. `ironhermes-hooks` must be a leaf or near-leaf crate that `ironhermes-tools` and `ironhermes-agent` can depend on without circular deps.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `StreamCallback` type in `agent_loop.rs` — fire-and-forget callback pattern, directly mirrors hook event emission
- `reqwest` 0.12.28 already in workspace — use for webhook HTTP delivery
- `floor_char_boundary()` in `ironhermes-cron/src/delivery.rs` — use for safe content truncation in event previews
- `MessageEvent` in `ironhermes-core` — existing event struct for gateway messages
- Webhook URL resolution in `ironhermes-cron/src/delivery.rs` — reusable pattern for webhook target parsing

### Established Patterns
- `Tool` trait + `ToolRegistry` in `ironhermes-tools` — extensibility via trait objects, same pattern for `GuardrailHook` trait
- Atomic file I/O via temp-file-and-rename (used in JobStore, memory subsystem) — use for JSONL event log writes
- `async-trait` for async trait methods — already in workspace
- `tokio::spawn` for fire-and-forget async work — matches hook delivery pattern

### Integration Points
- `ToolRegistry::dispatch()` — insert guardrail check before `tool.execute()` call
- `AgentLoop::run()` — emit `message_received` at entry, `response_sent` at exit
- `AgentLoop` tool execution path — emit `tool_called` before and `tool_completed` after dispatch
- `GatewayRunner` — pass HookRegistry to handler for event emission
- New `ironhermes-hooks` crate — depends on `ironhermes-core` only, consumed by `ironhermes-tools` and `ironhermes-agent`

</code_context>

<specifics>
## Specific Ideas

- Match the fire-and-forget pattern from `StreamCallback` — hook emission must never block the agent loop
- Reuse the webhook delivery infrastructure from `ironhermes-cron` where possible (reqwest, URL parsing, truncation)
- The `ironhermes-hooks` crate should be a clean leaf dependency like `ironhermes-cron` — minimal deps, imported by tools and agent crates

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 06-event-hooks*
*Context gathered: 2026-04-08*
