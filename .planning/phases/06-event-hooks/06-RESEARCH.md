# Phase 6: Event Hooks - Research

**Researched:** 2026-04-08
**Domain:** Rust async event/hook systems, guardrail interceptors, webhook HTTP delivery
**Confidence:** HIGH

## Summary

Phase 6 adds observable, interceptable lifecycle events to the IronHermes agent runtime. The three requirements are: structured logging of all lifecycle events through a hook registry (HOOK-01), guardrail hooks that can intercept and block tool calls before dispatch (HOOK-02), and forwarding of hook events to external HTTP endpoints (HOOK-03).

The project already has a clean, well-understood architecture. The `AgentLoop` in `ironhermes-agent` is the integration point — it currently has `StreamCallback` and `ToolProgressCallback` as simple `Box<dyn Fn>` callbacks. The hook system extends this pattern into a first-class registry of typed hooks, living in a new `ironhermes-hooks` crate (already planned per STATE.md). The `ToolRegistry::dispatch` method in `ironhermes-tools` is where guardrail intercept must occur — before `tool.execute(args)` runs.

The standard Rust pattern for this problem domain is the observer/event-bus pattern using `Arc<dyn Fn(HookEvent) + Send + Sync>` hooks stored in a registry, with async-compatible execution. Tokio's `broadcast` channel or direct async trait calls are both viable; the project already uses `tokio::sync::mpsc` channels in the streaming client, so this idiom is familiar. Webhook delivery maps directly to the existing `reqwest::Client` usage in `ironhermes-agent/src/client.rs`.

**Primary recommendation:** Add `ironhermes-hooks` crate with a `HookRegistry` struct; define a `HookEvent` enum covering the three lifecycle points; wire hooks into `AgentLoop::run` and `ToolRegistry::dispatch`; add a `WebhookDelivery` subscriber in the hooks crate using the existing `reqwest` workspace dependency.

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| HOOK-01 | Agent lifecycle events (message received, tool called, response sent) are logged via a hook registry | HookRegistry + HookEvent enum; fire in AgentLoop::run at three points |
| HOOK-02 | Guardrail hooks can intercept and block tool calls before dispatch | Intercept point is ToolRegistry::dispatch; return Err to block; guardrail hook receives tool name + args, returns allow/deny |
| HOOK-03 | Hook events can be forwarded to external HTTP endpoints via webhook delivery | WebhookDelivery subscriber using reqwest::Client; POST JSON payload; fire async from hook registry |
</phase_requirements>

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| tokio | 1 (workspace) | Async runtime for all async hook delivery | Already in workspace; all async code uses tokio |
| reqwest | 0.12.28 (workspace, locked) | HTTP POST for webhook delivery | Already used in LlmClient, web_read, web_search, telegram |
| serde / serde_json | 1 (workspace) | Serialize HookEvent payloads to JSON | Already used project-wide |
| tracing | 0.1 (workspace) | Structured logging for hook events | Already used project-wide |
| async-trait | 0.1 (workspace) | Async trait for hook subscribers | Already used for Tool trait |
| chrono | 0.4 (workspace) | Timestamps on hook event payloads | Already used project-wide |
| uuid | 1 (workspace) | Event IDs for correlation | Already used for job IDs |
| thiserror | 2 (workspace) | HookError type | Already used for HermesError |

[VERIFIED: Cargo.lock — all libraries confirmed at listed versions, all are existing workspace dependencies]

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| tokio::sync::broadcast | 1 (tokio, workspace) | Fan-out hook events to multiple subscribers | If more than one subscriber is needed simultaneously |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Arc<dyn Fn> hooks | tokio::sync::broadcast channel | broadcast adds queue depth and backpressure but more complexity; direct fn calls are simpler for this use case |
| Direct reqwest in hook | Dedicated WebhookDelivery struct | Struct allows retry logic, timeout config, and testability |

**Installation:**

No new crates to add — all dependencies are already in the workspace. The new `ironhermes-hooks` crate uses only workspace deps.

```bash
# Add crate to workspace Cargo.toml [workspace] members:
"crates/ironhermes-hooks",
```

**Version verification:** All packages verified against `Cargo.lock` — reqwest 0.12.28, tokio 1.x, serde 1.x. [VERIFIED: Cargo.lock]

## Architecture Patterns

### Recommended Project Structure
```
crates/ironhermes-hooks/
├── Cargo.toml
└── src/
    ├── lib.rs          # pub use re-exports
    ├── event.rs        # HookEvent enum + HookContext struct
    ├── registry.rs     # HookRegistry — registers listeners, fires events
    ├── guardrail.rs    # GuardrailHook trait + deny logic
    └── webhook.rs      # WebhookDelivery subscriber (reqwest POST)
```

### Pattern 1: HookEvent Enum — Typed Lifecycle Events
**What:** A single enum covering all lifecycle points. Each variant carries structured context needed by subscribers.
**When to use:** Always — all hook firing points use this type.
**Example:**
```rust
// Source: [ASSUMED] — idiomatic Rust event modeling for this domain
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEvent {
    pub id: String,           // uuid v4 for correlation
    pub timestamp: DateTime<Utc>,
    pub kind: HookEventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookEventKind {
    MessageReceived {
        platform: String,
        chat_id: String,
        content_preview: String, // first 200 chars
    },
    ToolCalled {
        tool_name: String,
        args_preview: String,    // first 200 chars of JSON args
    },
    ToolBlocked {
        tool_name: String,
        reason: String,
    },
    ResponseSent {
        platform: String,
        chat_id: String,
        response_preview: String,
    },
}
```
Note: `content_preview` / `args_preview` avoid logging full message content into hook payloads (privacy/size). Full content is available in the normal tracing spans.

### Pattern 2: HookRegistry — Observer Pattern with Arc Sharing
**What:** A registry holding a list of async-compatible hook listener functions. Arc-wrapped for sharing across `AgentLoop` and `ToolRegistry`.
**When to use:** Pass `Arc<HookRegistry>` to `AgentLoop::new()` and `ToolRegistry::dispatch()`.
**Example:**
```rust
// Source: [ASSUMED] — follows existing StreamCallback/ToolProgressCallback patterns in agent_loop.rs
use std::sync::Arc;
use tokio::sync::Mutex;

pub type HookListener = Arc<dyn Fn(HookEvent) + Send + Sync>;

pub struct HookRegistry {
    listeners: Vec<HookListener>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self { listeners: vec![] }
    }

    pub fn add_listener(&mut self, listener: HookListener) {
        self.listeners.push(listener);
    }

    /// Fire an event to all listeners. Non-blocking (spawn tasks for each).
    pub fn fire(&self, event: HookEvent) {
        for listener in &self.listeners {
            let l = listener.clone();
            let e = event.clone();
            tokio::spawn(async move { l(e); });
        }
    }
}
```
Note: `fire` is intentionally non-blocking — hooks must not slow down the agent loop. Webhook delivery failures are logged but do not propagate as errors to the caller.

### Pattern 3: GuardrailHook — Intercept at ToolRegistry::dispatch
**What:** A synchronous decision at the point where `ToolRegistry::dispatch` is called. Guardrails check tool name + args and return `GuardrailDecision::Allow` or `GuardrailDecision::Deny { reason }`.
**When to use:** This is the HOOK-02 requirement. The intercept point is `ToolRegistry::dispatch`, not `AgentLoop`.
**Example:**
```rust
// Source: [ASSUMED] — standard interceptor pattern
pub enum GuardrailDecision {
    Allow,
    Deny { reason: String },
}

pub trait GuardrailHook: Send + Sync {
    fn check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision;
}

// In ToolRegistry::dispatch — add before tool.execute():
for guardrail in &self.guardrails {
    match guardrail.check(name, &args) {
        GuardrailDecision::Allow => {},
        GuardrailDecision::Deny { reason } => {
            return Err(anyhow::anyhow!("Tool '{}' blocked by guardrail: {}", name, reason));
        }
    }
}
```
The agent loop already handles `Err` from `dispatch` by converting it to an error string returned to the LLM — so blocking behavior is automatic without changes to `AgentLoop`.

### Pattern 4: WebhookDelivery — reqwest POST with Fire-and-Forget
**What:** A `HookListener` implementation that serializes `HookEvent` to JSON and POSTs to a configured URL. Uses existing `reqwest::Client`.
**When to use:** Configured via `IRONHERMES_WEBHOOK_URL` env var or config file. If not configured, no-op.
**Example:**
```rust
// Source: [VERIFIED: existing reqwest usage in crates/ironhermes-agent/src/client.rs]
use reqwest::Client;

pub struct WebhookDelivery {
    client: Client,
    url: String,
    timeout_secs: u64,
}

impl WebhookDelivery {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| Client::new()),
            url: url.into(),
            timeout_secs: 10,
        }
    }

    pub async fn deliver(&self, event: &HookEvent) -> anyhow::Result<()> {
        self.client
            .post(&self.url)
            .header("Content-Type", "application/json")
            .json(event)
            .timeout(std::time::Duration::from_secs(self.timeout_secs))
            .send()
            .await
            .map(|_| ())
            .map_err(|e| anyhow::anyhow!("Webhook delivery failed: {}", e))
    }
}
```
Failures are logged via `tracing::warn!` but do not propagate — webhook delivery is best-effort.

### Integration Points

**Where to wire hook firing (HOOK-01):**

1. `AgentLoop::run` — add `Arc<HookRegistry>` field, fire `HookEventKind::MessageReceived` at loop entry (from the last user message), fire `HookEventKind::ResponseSent` before returning `AgentResult`.
2. `AgentLoop::execute_tool_call` — fire `HookEventKind::ToolCalled` after args parse succeeds, before dispatch.

**Where to wire guardrail intercept (HOOK-02):**

`ToolRegistry::dispatch` — add `guardrails: Vec<Box<dyn GuardrailHook>>` field, check all guardrails before `tool.execute(args)`. Return `Err` to block.

**Constructor pattern (follows existing crate patterns):**
```rust
// In ToolRegistry (matching register_memory_tool / register_cronjob_tool pattern)
pub fn add_guardrail(&mut self, hook: Box<dyn GuardrailHook>) { ... }

// In AgentLoop (matching with_streaming / with_tool_progress pattern)
pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self { ... }
```

### Anti-Patterns to Avoid
- **Blocking the agent loop on hook delivery:** Hook listeners must be fire-and-forget (spawn or clone+send). Never `.await` a webhook call in the critical path.
- **Putting guardrail logic in AgentLoop:** Guardrails belong in `ToolRegistry::dispatch` — this is the single intercept point regardless of which caller triggers a tool.
- **Logging full message content in hook payloads:** Use previews (first N chars). Full content is in tracing spans.
- **Creating a new reqwest::Client per webhook event:** Instantiate once, reuse. reqwest::Client is designed to be cloned (connection pool is shared).
- **Making guardrail checks async:** The guardrail decision must be synchronous — the `check` method takes `&str` and `&Value`, no I/O needed. Async guardrails add complexity with no benefit for this use case.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTTP POST to webhook | Custom TCP/HTTP implementation | reqwest 0.12 (workspace) | reqwest handles TLS, connection pooling, timeouts, JSON serialization — already in workspace |
| JSON serialization of events | Manual format strings | serde_json (workspace) | Type-safe, handles escaping, already used everywhere |
| Event timestamps | Manual chrono calls in each fire site | Timestamp in HookEvent constructor | Single source of truth; consistent format |
| Unique event IDs | Counter-based or timestamp-based IDs | uuid v4 (workspace) | Already used for job IDs in ironhermes-cron |
| Async fire-and-forget | Manual thread::spawn | tokio::spawn | Already used project-wide; proper async task lifecycle |

**Key insight:** All infrastructure for this phase already exists in the workspace. No new external crates are needed — the entire implementation uses existing workspace dependencies.

## Common Pitfalls

### Pitfall 1: Deadlock from Mutex in Hook Fire
**What goes wrong:** If `HookRegistry` or `ToolRegistry` is protected by `std::sync::Mutex` and a hook listener tries to re-acquire the same lock, deadlock occurs.
**Why it happens:** `fire()` is called while holding the registry lock; listener spawned task tries to re-enter.
**How to avoid:** Use `Arc<HookRegistry>` with interior mutability only during setup (registration). At runtime, `fire()` only reads the listener list — a `RwLock` held only during registration, or freeze the listener list after setup and use `Arc<Vec<HookListener>>` with no lock at all.
**Warning signs:** Hang during tool execution after adding hook listeners.

### Pitfall 2: WebhookDelivery Slowing Agent Loop
**What goes wrong:** A slow or unresponsive webhook endpoint causes the agent to hang mid-conversation.
**Why it happens:** If `deliver().await` is called in the hook firing path without spawning.
**How to avoid:** Always `tokio::spawn` webhook delivery. The `fire()` method in `HookRegistry` must never `.await` inside the calling task. [VERIFIED: existing codebase pattern — `tool_progress_callback` in agent_loop.rs is also fire-and-forget]
**Warning signs:** Agent responses slow down after HOOK-03 is wired up; timeouts in agent tests.

### Pitfall 3: GuardrailHook Breaks Existing Tool Tests
**What goes wrong:** Tests for `ToolRegistry::dispatch` start failing after guardrail intercept is added because test fixtures don't configure any guardrails.
**Why it happens:** If guardrails are checked with an empty `Vec`, all calls pass through — this is correct. Problem only occurs if a default restrictive guardrail is registered globally.
**How to avoid:** Default guardrail list must be empty. Guardrails are opt-in. Tests that want to test blocking use explicit guardrail registration.
**Warning signs:** Pre-existing tool tests fail after ToolRegistry changes.

### Pitfall 4: ironhermes-hooks Circular Dependency
**What goes wrong:** `ironhermes-hooks` imports `ironhermes-tools` (for `GuardrailHook` integration), and `ironhermes-tools` imports `ironhermes-hooks` for `HookRegistry` — circular dependency.
**Why it happens:** Trying to put all hooks integration into one crate.
**How to avoid:** Define the core types (`HookEvent`, `HookRegistry`, `GuardrailHook` trait) in `ironhermes-hooks`. The `ToolRegistry` in `ironhermes-tools` adds an optional `guardrails: Vec<Box<dyn GuardrailHook>>` field by importing only `ironhermes-hooks`. The `AgentLoop` in `ironhermes-agent` adds `Arc<HookRegistry>` by importing `ironhermes-hooks`. No circular deps.
**Warning signs:** `cargo check` reports "cyclic dependency" error.

### Pitfall 5: Content Preview Truncation Panics
**What goes wrong:** `&args_preview[..200]` panics on non-ASCII UTF-8 strings if byte 200 falls in the middle of a multi-byte character.
**Why it happens:** String slicing in Rust is byte-indexed; multi-byte chars straddle boundaries.
**How to avoid:** Use `str::floor_char_boundary(200)` (stable since Rust 1.65) — the same pattern already used in `crates/ironhermes-cron/src/delivery.rs:format_delivery_message`. [VERIFIED: codebase — delivery.rs line 128 uses `floor_char_boundary`]
**Warning signs:** Panic with "byte index N is not a char boundary" during hook event construction.

## Code Examples

Verified patterns from official sources and existing codebase:

### reqwest POST JSON (matches existing LlmClient pattern)
```rust
// Source: [VERIFIED: crates/ironhermes-agent/src/client.rs — existing reqwest::Client usage]
let response = self.client
    .post(&self.url)
    .header("Content-Type", "application/json")
    .json(&event)               // serde_json serialization
    .timeout(Duration::from_secs(10))
    .send()
    .await?;
```

### floor_char_boundary for safe string preview
```rust
// Source: [VERIFIED: crates/ironhermes-cron/src/delivery.rs line 128]
let safe_end = content.floor_char_boundary(200);
let preview = &content[..safe_end];
```

### tokio::spawn fire-and-forget hook delivery
```rust
// Source: [ASSUMED] — follows existing pattern in agent_loop.rs StreamCallback
for listener in &self.listeners {
    let l = listener.clone();
    let e = event.clone();
    tokio::spawn(async move { l(e); });
}
```

### GuardrailHook integration in ToolRegistry::dispatch
```rust
// Source: [ASSUMED] — extends existing dispatch method in ironhermes-tools/src/registry.rs
pub async fn dispatch(&self, name: &str, args: serde_json::Value) -> anyhow::Result<String> {
    let tool = self.tools.get(name)
        .ok_or_else(|| anyhow::anyhow!("Tool not found: {}", name))?;

    if !tool.is_available() {
        return Err(anyhow::anyhow!("Tool '{}' is not available", name));
    }

    // NEW: guardrail intercept
    for guardrail in &self.guardrails {
        match guardrail.check(name, &args) {
            GuardrailDecision::Allow => {}
            GuardrailDecision::Deny { reason } => {
                return Err(anyhow::anyhow!("Tool '{}' blocked: {}", name, reason));
            }
        }
    }

    tool.execute(args).await
}
```

### Arc<HookRegistry> wiring into AgentLoop (builder pattern)
```rust
// Source: [ASSUMED] — follows with_streaming / with_tool_progress pattern in agent_loop.rs
pub fn with_hook_registry(mut self, registry: Arc<HookRegistry>) -> Self {
    self.hook_registry = Some(registry);
    self
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `Box<dyn Fn>` callbacks wired at construction | Arc-wrapped registry of listeners | N/A — this phase introduces hooks | Registry allows multiple subscribers; callbacks only support one |
| No tool intercept | GuardrailHook at dispatch boundary | N/A — this phase introduces guardrails | Enables policy enforcement without changing tool code |

**Deprecated/outdated:**
- Manual tracing spans only: adequate for debugging, but does not provide structured queryable events. This phase adds structured events that survive outside the process (webhook delivery).

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `HookRegistry.fire()` uses `tokio::spawn` for fire-and-forget delivery | Architecture Patterns (Pattern 2) | If wrong approach chosen, webhook delivery could block agent loop — switch to spawn |
| A2 | GuardrailHook check method is synchronous (not async) | Architecture Patterns (Pattern 3) | If async guardrails are needed in future, trait would need `async fn check()` via async-trait; low risk for this phase |
| A3 | `ironhermes-hooks` crate holds core types; `ToolRegistry` imports it (not reverse) | Architecture Patterns, Pitfall 4 | Circular dependency if dependency direction is reversed; cargo check catches this immediately |
| A4 | Webhook delivery is best-effort (failures logged, not propagated) | Pattern 4, Pitfall 2 | If strict delivery guarantees are needed, a retry queue would be required — out of scope for this phase |

## Open Questions

1. **Webhook authentication**
   - What we know: reqwest supports `Authorization` header
   - What's unclear: Is a shared secret / HMAC signature required on outbound webhook calls, or is plain POST to a URL sufficient for v1.1?
   - Recommendation: Start with plain POST + configurable `Authorization` header via env var (`IRONHERMES_WEBHOOK_SECRET`). HMAC signing is a v2 concern.

2. **Hook event persistence / replay**
   - What we know: ironhermes-cron saves job output to files; similar pattern possible for events
   - What's unclear: Should hook events be written to disk for replay/audit, or is logging-only sufficient?
   - Recommendation: Logging-only for this phase (HOOK-01 says "logged via a hook registry"). File persistence is a v2/OBS concern.

3. **Configuration for guardrail rules**
   - What we know: The requirement says "e.g., block terminal in untrusted contexts"
   - What's unclear: Are guardrail rules defined in code (compiled-in) or in a config file (runtime)?
   - Recommendation: Compiled-in for this phase with a `BlocklistGuardrail` that takes a list of tool names to block. Config-file-driven rules are a v2 extension.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| cargo | Build | Yes | 1.94.0 | — |
| reqwest | HOOK-03 webhook delivery | Yes (workspace) | 0.12.28 | — |
| tokio | Async runtime | Yes (workspace) | 1.x | — |

[VERIFIED: `cargo --version` on target machine, Cargo.lock for library versions]

No missing dependencies — all required libraries are already in the workspace.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in (`#[test]`, `#[tokio::test]`) |
| Config file | none — Cargo handles test discovery |
| Quick run command | `cargo test -p ironhermes-hooks 2>&1` |
| Full suite command | `cargo test --workspace 2>&1` |

### Phase Requirements to Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| HOOK-01 | HookRegistry fires events to all listeners | unit | `cargo test -p ironhermes-hooks hook_registry 2>&1` | No — Wave 0 |
| HOOK-01 | HookEvent serializes to valid JSON | unit | `cargo test -p ironhermes-hooks hook_event_serialization 2>&1` | No — Wave 0 |
| HOOK-02 | GuardrailHook blocks named tool | unit | `cargo test -p ironhermes-tools guardrail_blocks_tool 2>&1` | No — Wave 0 |
| HOOK-02 | GuardrailHook allows non-blocked tool | unit | `cargo test -p ironhermes-tools guardrail_allows_tool 2>&1` | No — Wave 0 |
| HOOK-03 | WebhookDelivery POSTs correct JSON payload | unit (mock server) | `cargo test -p ironhermes-hooks webhook_delivery 2>&1` | No — Wave 0 |
| HOOK-03 | WebhookDelivery failure does not panic | unit | `cargo test -p ironhermes-hooks webhook_delivery_failure 2>&1` | No — Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-hooks 2>&1`
- **Per wave merge:** `cargo test --workspace 2>&1`
- **Phase gate:** Full workspace test suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-hooks/` — new crate, needs `Cargo.toml` and `src/lib.rs` scaffolded
- [ ] `crates/ironhermes-hooks/src/event.rs` — HookEvent unit tests
- [ ] `crates/ironhermes-hooks/src/registry.rs` — HookRegistry unit tests
- [ ] `crates/ironhermes-hooks/src/webhook.rs` — WebhookDelivery unit tests (use `wiremock` or `mockito` for mock HTTP server, or test with a direct `reqwest` POST to a local test server)
- [ ] `crates/ironhermes-tools/src/registry.rs` — extend existing tests with guardrail cases

**Note on mock HTTP:** For webhook delivery tests, `wiremock` crate is the standard choice in the Rust ecosystem. It is not currently in the workspace — the Wave 0 task should add `wiremock = "0.6"` as a dev-dependency in `ironhermes-hooks/Cargo.toml`. [ASSUMED — wiremock is standard but not verified against registry in this session]

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | Webhook is outbound only |
| V3 Session Management | no | Hooks are stateless events |
| V4 Access Control | yes | Guardrail hooks must be checked server-side; no client bypass possible |
| V5 Input Validation | yes | Hook event content_preview uses floor_char_boundary, not raw slicing |
| V6 Cryptography | partial | If webhook secret is added: use HMAC-SHA256 (std library / sha2 crate), never plain text |

### Known Threat Patterns for this stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Guardrail bypass via arg mutation | Tampering | Guardrail check receives the same `args` value that `tool.execute()` receives — no copy-after-check gap |
| SSRF via webhook URL | Elevation of Privilege | Reuse existing `is_safe_url` from `ironhermes-core::ssrf` when webhook URL is configured [VERIFIED: ssrf.rs exists in ironhermes-core] |
| Log injection via hook event content | Tampering | Use structured tracing fields, not string interpolation; preview content is not executed |
| Denial of service via hook flood | DoS | `tokio::spawn` per event creates bounded task load; no unbounded queue growth |

## Sources

### Primary (HIGH confidence)
- [VERIFIED: Cargo.lock] — reqwest 0.12.28, all workspace dependencies confirmed
- [VERIFIED: crates/ironhermes-agent/src/agent_loop.rs] — existing callback patterns (StreamCallback, ToolProgressCallback), integration points
- [VERIFIED: crates/ironhermes-tools/src/registry.rs] — ToolRegistry::dispatch, existing register_* pattern
- [VERIFIED: crates/ironhermes-cron/src/delivery.rs] — floor_char_boundary usage, fire-and-forget delivery pattern, atomic write pattern
- [VERIFIED: crates/ironhermes-agent/src/client.rs] — reqwest::Client construction and usage pattern
- [VERIFIED: crates/ironhermes-core/src/ssrf.rs exists] — SSRF protection available for webhook URL validation

### Secondary (MEDIUM confidence)
- [ASSUMED] — observer/event-registry pattern using `Arc<dyn Fn(Event) + Send + Sync>` is standard Rust idiom for this use case; consistent with existing callback style in the codebase

### Tertiary (LOW confidence)
- [ASSUMED] — `wiremock` crate for mock HTTP in tests; should be verified against crates.io before Wave 0

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all deps verified in Cargo.lock, no new crates needed
- Architecture: HIGH — integration points verified in source; patterns follow existing codebase conventions
- Pitfalls: HIGH — floor_char_boundary pattern verified from existing codebase; others derived from architecture analysis
- Security: MEDIUM — SSRF protection confirmed available; HMAC recommendation is ASSUMED standard practice

**Research date:** 2026-04-08
**Valid until:** 2026-06-08 (stable domain, no fast-moving dependencies)
