# Phase 12: Provider Resolution - Research

**Researched:** 2026-04-11
**Domain:** Rust provider/credential abstraction, Anthropic API, multi-mode LLM client design
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Resolver API shape**
- D-01: `ProviderResolver` is a struct (not a trait), built once at startup from Config + env vars. `Clone + Send + Sync`, passed like `LlmClient` is today.
- D-02: Resolve once at startup — build a lookup table. Config changes require restart. Matches frozen-snapshot pattern from Phase 11.
- D-03: Resolver produces `LlmClient` directly via `resolver.build_client(provider, model)`. Call sites never touch base_url/api_key directly.
- D-04: Config.yaml gains a `providers:` map with named entries: `base_url`, `api_key`, `default_model`, `api_mode`. Built-in providers (openrouter, anthropic, openai) pre-registered but overridable.
- D-05: Auxiliary model routing ports hermes-agent's pattern. Config has `model.roles:` mapping role names to provider+model pairs. `provider: main` falls back to shared resolver.
- D-06: Four first-class providers: OpenRouter, Anthropic native (Messages API + adapter), OpenAI/custom endpoint, named custom providers.

**API mode routing**
- D-07: `ApiMode` enum: `ChatCompletions`, `CodexResponses`, `AnthropicMessages`. `ResolvedEndpoint` includes `ApiMode`, `build_client()` returns right client type.
- D-08: Anthropic adapter lives in `ironhermes-agent` crate alongside `LlmClient`. `AnthropicClient` as parallel implementation, both behind common trait or enum dispatch.
- D-09: Anthropic native path: credential discovery only — read `~/.claude/credentials.json` → `oauth.accessToken`, fall back to `ANTHROPIC_API_KEY` env var. Credential resolved once at startup. OAuth token refresh deferred to follow-on phase.
- D-10: `CodexResponses` variant defined and config-wired, but returns an error if selected at runtime. Stub-only.

**Fallback & error recovery**
- D-11: Port hermes-agent's one-shot fallback pattern. Three trigger points: (1) max retries on invalid API responses, (2) non-retryable client errors (401/403/404), (3) max retries on transient errors (429/5xx). Swap client in-place, reset retry count, set `_fallback_activated` flag.
- D-12: Fallback scope: main agent only (CLI + gateway). Subagents inherit parent's provider without fallback. Cron runs with fixed provider. Auxiliary tasks use their own independent resolution chain.

**Iteration budget**
- D-13: Budget tracks iteration count (not tokens). Budget = `max_turns`. Counter increments per agent turn.
- D-14: Budget shared between parent and child via `Arc<AtomicUsize>`. Parent creates counter, child receives clone. Each turn increments shared counter. Parent's `max_turns` is global cap.
- D-15: Threshold behavior via system prompt injection: 70% → `[Caution] consolidate your work`; 90% → `[Warning] respond now, summarize progress`; 100% → hard stop.

### Claude's Discretion
- `ResolvedEndpoint` struct design (fields beyond api_mode/base_url/api_key)
- Common client trait vs enum dispatch for LlmClient/AnthropicClient
- Credential file discovery and refresh mechanism details for Anthropic
- Exact config.yaml schema for `providers:`, `custom_providers:`, and `model.roles:`
- How `SubagentConfig.base_url`/`api_key` overrides migrate to the resolver pattern
- Provider-specific API key scoping logic
- Error retry counts and backoff strategy within each trigger category

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope. Setup wizard deferred to Phase 23.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROV-01 | Shared runtime resolver for CLI, gateway, cron, ACP, auxiliary | ProviderResolver struct replacing Config.resolve_base_url/resolve_api_key at 8 call sites |
| PROV-02 | Three API modes: chat_completions, codex_responses, anthropic_messages | ApiMode enum + client dispatch pattern |
| PROV-03 | Resolution precedence: explicit request > config.yaml > env vars > provider defaults | Lookup table built at startup, immutable at runtime |
| PROV-04 | API keys scoped to provider's base URL | Key stored inside ResolvedEndpoint, not exposed separately |
| PROV-05 | Anthropic adapter + credential discovery | Claude Code credentials.json oauth.accessToken; ANTHROPIC_API_KEY env var; credential resolved once at startup (refresh deferred) |
| PROV-06 | Auxiliary model routing with own provider/model chain | model.roles: config section; role-keyed resolution in ProviderResolver |
| PROV-07 | Fallback model switching on 429/5xx/401 | One-shot fallback in AgentLoop; fallback_providers: list in provider config |
| PROV-08 | Named custom providers configurable in config.yaml | custom_providers: list or providers: map in Config |
| PROV-09 | Iteration budget with 70/90/100% thresholds | Arc<AtomicUsize> counter; system prompt injection at threshold crossings |
| PROV-10 | Budget shared across parent and child agents | Arc<AtomicUsize> cloned from parent to AgentSubagentRunner |
</phase_requirements>

---

## Summary

Phase 12 replaces a scattered pattern of `config.resolve_base_url()` / `config.resolve_api_key()` calls — identified at **8 concrete call sites** across CLI, gateway, cron, batch runner, and subagent runner — with a single `ProviderResolver` struct that is built once at startup and passed through the system like `LlmClient` is today.

The phase has three distinct work streams. First, the resolver itself: a startup-time lookup table built from `config.yaml` + environment variables that maps (provider, model) to a `ResolvedEndpoint` (base_url, api_key, api_mode, model). Second, the client abstraction: `LlmClient` becomes the `ChatCompletions` implementation; `AnthropicClient` is added alongside it; both are accessible through an `AnyClient` enum or a `LlmClientTrait` trait. Third, the behavior extensions: fallback chain logic in `AgentLoop`, iteration budget with `Arc<AtomicUsize>` shared between parent and child agents, and system prompt injection at budget thresholds.

The Anthropic path requires special handling: Claude Code stores OAuth tokens in `~/.claude/credentials.json` under an `oauth` key. This credential must be refreshed before use and retried on 401. The Anthropic Messages API format differs from OpenAI chat completions, requiring an adapter that translates `Vec<ChatMessage>` into Anthropic's `messages` + `system` parameter format.

**Primary recommendation:** Implement ProviderResolver in `ironhermes-core`, AnthropicClient + adapter in `ironhermes-agent`, and wire the budget counter as an `Arc<AtomicUsize>` field on `AgentLoop` with injection into subagent runners via a new `with_budget` builder method.

---

## Standard Stack

### Core (already in workspace — no new dependencies needed for most features)

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `reqwest` | 0.12 [VERIFIED: Cargo.toml] | HTTP client for Anthropic Messages API calls | Already in workspace with json+stream features |
| `serde` / `serde_json` / `serde_yaml` | 1.x / 1.x / 0.9 [VERIFIED: Cargo.toml] | Config serialization, JSON request/response | Already workspace deps |
| `tokio` | 1.x [VERIFIED: Cargo.toml] | Async runtime, AtomicUsize via std::sync::atomic | Already workspace dep |
| `anyhow` / `thiserror` | 1.x / 2.x [VERIFIED: Cargo.toml] | Error handling | Already workspace deps |
| `async-trait` | 0.1 [VERIFIED: Cargo.toml] | If using trait dispatch for client abstraction | Already workspace dep |

### Supporting

| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `std::sync::atomic::AtomicUsize` | stdlib [ASSUMED] | Lock-free iteration budget counter | PROV-09/PROV-10: shared between parent and child via Arc<AtomicUsize> |

No new Cargo dependencies are required for this phase. All necessary libraries are already workspace dependencies.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `Arc<AtomicUsize>` for budget | `Arc<Mutex<usize>>` | AtomicUsize is lock-free and sufficient for simple increment+read; Mutex adds overhead for no benefit |
| Enum dispatch (`AnyClient`) | Trait object (`Box<dyn LlmClientTrait>`) | Enum dispatch avoids dynamic dispatch overhead and is exhaustive; trait object is more extensible but requires vtable; enum is preferred per project patterns |
| Startup-time lookup table | Runtime config re-reads | Frozen-snapshot matches Phase 11 pattern (locked decision D-02) |

---

## Architecture Patterns

### Recommended Project Structure Changes

```
crates/ironhermes-core/src/
├── config.rs           # Add providers: map, custom_providers:, model.roles: sections
├── provider.rs         # NEW: ProviderResolver, ResolvedEndpoint, ApiMode, ProviderConfig
└── constants.rs        # Already has OPENROUTER_BASE_URL, ANTHROPIC_BASE_URL

crates/ironhermes-agent/src/
├── client.rs           # LlmClient stays as ChatCompletions client
├── anthropic_client.rs # NEW: AnthropicClient (Messages API) + format adapter
├── any_client.rs       # NEW: AnyClient enum dispatch (or trait in client.rs)
├── agent_loop.rs       # Add budget counter field, threshold injection, fallback logic
└── subagent_runner.rs  # Migrate to ProviderResolver; receive Arc<AtomicUsize> from parent
```

### Pattern 1: ProviderResolver Lookup Table

**What:** Build a `HashMap<String, ResolvedEndpoint>` at startup from config + env. Keys are provider names. `build_client(provider, model)` looks up endpoint, constructs the right client variant.

**When to use:** Every LLM call site (replaces all 8 instances of `config.resolve_base_url()` / `config.resolve_api_key()`).

**Identified call sites (all must migrate):**
- `crates/ironhermes-cli/src/main.rs` lines ~237, ~302, ~509-517, ~607 [VERIFIED: grep]
- `crates/ironhermes-gateway/src/handler.rs` line ~352 [VERIFIED: grep]
- `crates/ironhermes-gateway/src/runner.rs` line ~646 [VERIFIED: grep]
- `crates/ironhermes-cli/src/batch/runner.rs` line ~67 [VERIFIED: grep]
- `crates/ironhermes-agent/src/subagent_runner.rs` line ~71 [VERIFIED: grep]

```rust
// Source: Design based on locked decisions D-01, D-02, D-03 [ASSUMED pattern]
#[derive(Debug, Clone)]
pub struct ResolvedEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,        // None for credential-file auth paths
    pub api_mode: ApiMode,
    pub default_model: String,
    pub fallback_providers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ApiMode {
    ChatCompletions,
    AnthropicMessages,
    CodexResponses,  // stub — errors at runtime if selected
}

#[derive(Clone)]
pub struct ProviderResolver {
    endpoints: HashMap<String, ResolvedEndpoint>,
    roles: HashMap<String, String>,  // role name -> provider name
}

impl ProviderResolver {
    pub fn build(config: &Config) -> Self { ... }

    pub fn resolve(&self, provider: &str) -> Option<&ResolvedEndpoint> { ... }

    pub fn build_client(&self, provider: &str, model: &str) -> Result<AnyClient> { ... }

    pub fn resolve_role(&self, role: &str) -> Option<&ResolvedEndpoint> { ... }
}
```

### Pattern 2: AnyClient Enum Dispatch

**What:** Enum wrapping `LlmClient` (ChatCompletions) and `AnthropicClient` (Messages API). All `AgentLoop` call sites use `AnyClient` instead of raw `LlmClient`.

**When to use:** Anywhere a client needs to be selected based on `ApiMode`. Avoids Box<dyn Trait> overhead.

```rust
// Source: [ASSUMED — idiomatic Rust for closed enum dispatch]
pub enum AnyClient {
    ChatCompletions(LlmClient),
    AnthropicMessages(AnthropicClient),
}

impl AnyClient {
    pub async fn chat_completion(&self, ...) -> Result<ChatResponse> {
        match self {
            Self::ChatCompletions(c) => c.chat_completion(...).await,
            Self::AnthropicMessages(c) => c.chat_completion_adapted(...).await,
        }
    }
}
```

### Pattern 3: Anthropic Message Format Adapter

**What:** Translate `Vec<ChatMessage>` (OpenAI format) to Anthropic Messages API format. System messages extracted into `system` parameter. Tool calls/results mapped to Anthropic's `tool_use` / `tool_result` content block format.

**Key translation rules** [ASSUMED based on Anthropic API knowledge, verify against docs]:
- OpenAI `role: system` → Anthropic `system` parameter (string or array of content blocks)
- OpenAI `role: user/assistant` → Anthropic `messages[].role`
- OpenAI `tool_calls` → Anthropic `content: [{type: "tool_use", id, name, input}]`
- OpenAI `role: tool` with `tool_call_id` → Anthropic `role: user, content: [{type: "tool_result", tool_use_id, content}]`
- Anthropic requires alternating user/assistant turns — consecutive same-role messages must be merged

### Pattern 4: One-Shot Fallback in AgentLoop

**What:** On qualifying error, swap the active client to the fallback provider's client. Reset retry count. Set `fallback_activated` flag to prevent re-firing. Three trigger conditions (from D-11).

**When to use:** Main agent loop only (CLI interactive + gateway message handling). Not in subagents, cron, or auxiliary tasks.

```rust
// Source: [ASSUMED — port of hermes-agent pattern per D-11]
struct FallbackState {
    activated: bool,
    fallback_client: Option<AnyClient>,
}

// In agent_loop.run():
// On 429/5xx/401 + not already activated:
//   swap self.client → fallback_client
//   self.fallback_state.activated = true
//   reset retry counter
//   continue loop
```

### Pattern 5: Shared Iteration Budget

**What:** `Arc<AtomicUsize>` created by parent, cloned into child agents. Each turn in any agent (parent or child) calls `budget.fetch_add(1, Ordering::SeqCst)`. Before each turn, check against `max_turns`.

**Threshold injection (D-15):**
```rust
// Source: [ASSUMED — per D-13, D-14, D-15]
let used = self.budget.load(Ordering::SeqCst);
let pct = used * 100 / self.max_turns;
if pct >= 100 { return hard_stop(); }
let injection = match pct {
    90..=99 => Some("[Warning] respond now, summarize progress"),
    70..=89 => Some("[Caution] consolidate your work"),
    _ => None,
};
// Prepend injection to next system message if Some
```

### Pattern 6: Config Schema Extension

**What:** Add `providers:` map, `custom_providers:` list, and `model.roles:` section to `Config` in `ironhermes-core/src/config.rs`. All new fields use `#[serde(default)]` for backward compatibility.

```yaml
# Source: [ASSUMED — per D-04, D-05, D-06]
providers:
  openrouter:
    base_url: "https://openrouter.ai/api/v1"
    api_key: null          # falls back to OPENROUTER_API_KEY env var
    api_mode: chat_completions
    default_model: "anthropic/claude-sonnet-4"
    fallback_providers: []
  anthropic:
    base_url: "https://api.anthropic.com"
    api_key: null          # falls back to ANTHROPIC_API_KEY or credential file
    api_mode: anthropic_messages
    default_model: "claude-sonnet-4-20250514"
    fallback_providers: ["openrouter"]
  openai:
    base_url: "https://api.openai.com/v1"
    api_key: null          # falls back to OPENAI_API_KEY
    api_mode: chat_completions
    default_model: "gpt-4o"
    fallback_providers: []

custom_providers:
  - name: "local-llama"
    base_url: "http://localhost:11434/v1"
    api_key: "ollama"
    api_mode: chat_completions
    default_model: "llama3"

model:
  default: "anthropic/claude-sonnet-4"
  provider: "openrouter"
  roles:
    vision: { provider: openrouter, model: "openai/gpt-4o" }
    compression: { provider: openrouter, model: "anthropic/claude-haiku-4" }
    session_search: { provider: main }   # falls through to main resolver
```

### Pattern 7: Anthropic Credential File Resolution

**What:** Claude Code stores OAuth tokens in `~/.claude/credentials.json` under key `oauth`. This is a JSON object with `accessToken`, `refreshToken`, `expiresAt` (timestamp). For this phase, only `accessToken` is read at startup (D-09 narrowed scope).

**Discovery order for Anthropic auth** (per D-09) [VERIFIED: credentials.json structure confirmed via direct file check]:
1. `config.yaml` explicit `api_key` for anthropic provider
2. `ANTHROPIC_API_KEY` environment variable
3. `~/.claude/credentials.json` → `oauth.accessToken` (read once at startup, no refresh)

**Refresh mechanism:** DEFERRED to follow-on phase. The refresh endpoint URL/body format requires additional research. For this phase, credential is resolved once at startup — if the token is expired, the API call will fail and the user must re-authenticate via Claude Code.

### Anti-Patterns to Avoid

- **Calling `config.resolve_base_url()` in any new code:** After this phase, all client construction goes through `resolver.build_client()`. The old methods should be removed or deprecated.
- **Passing base_url/api_key as raw strings between modules:** Leaks provider-specific secrets to wrong endpoints. All key access is through `ResolvedEndpoint` which is scoped to its provider.
- **Sharing fallback state across subagents:** Fallback is main-agent-only per D-12. Subagents receive a client directly, not a resolver with fallback.
- **Mutable budget counters without Arc:** Budget must be `Arc<AtomicUsize>` so parent and child share the same atomic counter, not independent copies.
- **Storing AtomicUsize behind Mutex:** Use `AtomicUsize` directly with `fetch_add` / `load` — no mutex needed for this use case.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Anthropic Messages API request serialization | Custom serializer | Derive `Serialize` structs matching Anthropic schema | Anthropic's schema has edge cases (content blocks array vs string, tool_use block format) |
| OAuth token refresh HTTP call | Custom retry loop | Standard `reqwest` POST with `serde_json` — already in workspace | Existing HTTP client handles the TLS/timeout concerns |
| Thread-safe counter | Custom counter with Mutex | `std::sync::atomic::AtomicUsize` | Lock-free, available in std, sufficient for increment-and-read pattern |
| Provider name → endpoint lookup | Linear scan | `HashMap<String, ResolvedEndpoint>` | O(1) lookup, startup-time construction |

**Key insight:** The Anthropic Messages API format adapter is the one genuinely novel piece. Everything else (HTTP, JSON, atomic counters, config parsing) is already solved by existing workspace dependencies.

---

## Common Pitfalls

### Pitfall 1: Alternating Turn Requirement (Anthropic)
**What goes wrong:** Anthropic Messages API rejects requests where consecutive messages have the same `role`. The OpenAI format allows back-to-back `assistant` messages (e.g., when context compression emits multiple tool results).
**Why it happens:** OpenAI and Anthropic have different message sequence requirements.
**How to avoid:** In the Anthropic adapter, merge consecutive same-role messages before sending. Specifically, merge adjacent `user` messages and adjacent `assistant` messages.
**Warning signs:** HTTP 400 from Anthropic with "messages must alternate between user and assistant roles".

### Pitfall 2: System Message Handling Difference
**What goes wrong:** OpenAI accepts `role: system` inside the messages array. Anthropic requires system content in a separate top-level `system` parameter, not in `messages`.
**Why it happens:** Different API schemas.
**How to avoid:** Strip all system messages from the messages array in the adapter; concatenate them into the `system` parameter string.
**Warning signs:** HTTP 400 "system role not supported in messages".

### Pitfall 3: Tool Result Format Mismatch
**What goes wrong:** OpenAI uses `role: tool` with `tool_call_id`. Anthropic uses `role: user` with `content: [{type: "tool_result", tool_use_id: ..., content: ...}]`.
**Why it happens:** Anthropic wraps tool results inside a user message as a content block.
**How to avoid:** In the adapter, detect `role: tool` messages and transform them into Anthropic tool_result content blocks, grouped under a single `role: user` message if multiple tool results follow each other.
**Warning signs:** Adapter emitting role: tool to Anthropic → 400 error.

### Pitfall 4: Budget Counter Double-Count
**What goes wrong:** If `Arc<AtomicUsize>` is cloned into AgentLoop AND a copy is retained in AgentSubagentRunner, turns could be counted twice per actual turn.
**Why it happens:** Arc<AtomicUsize> is a shared pointer — `fetch_add` on any clone increments the same underlying counter.
**How to avoid:** Increment the counter exactly once per logical agent turn, in the loop body where `turns_used += 1` currently lives. The parent and child both share the same Arc; each increments once per their own turns.
**Warning signs:** Budget hitting 100% before expected turn count.

### Pitfall 5: Fallback Re-activation
**What goes wrong:** Without a `fallback_activated` flag, a fallback provider that also hits 429 could trigger another fallback cycle indefinitely.
**Why it happens:** Error trigger conditions fire without checking if fallback is already active.
**How to avoid:** Set `fallback_activated = true` after first fallback swap. Subsequent errors after fallback is active go through normal retry exhaustion, not another provider swap.
**Warning signs:** More than one provider swap per agent run.

### Pitfall 6: API Key Leakage to Wrong Endpoint
**What goes wrong:** `OPENROUTER_API_KEY` sent to `api.anthropic.com`, or vice versa. This is the current risk with the scattered `resolve_api_key()` pattern.
**Why it happens:** Single resolution path with no provider context.
**How to avoid:** `ResolvedEndpoint` pairs key with URL at resolution time. `build_client()` constructs the HTTP client with the key already embedded in the endpoint struct — call sites cannot mix them.
**Warning signs:** 401 errors from provider that has a valid key in a different env var.

### Pitfall 7: Backward-Compat Config Parsing
**What goes wrong:** Existing config.yaml files without `providers:` section fail to parse after the schema extension.
**Why it happens:** serde_yaml fails on missing required fields.
**How to avoid:** All new Config fields use `#[serde(default)]`. The built-in providers (openrouter, anthropic, openai) are pre-populated in `ProviderResolver::build()` from defaults even when absent from config — user config overlays, not replaces.
**Warning signs:** Deserialization errors on startup after adding new config fields.

---

## Code Examples

### Existing resolve_base_url / resolve_api_key (to be replaced)
```rust
// Source: crates/ironhermes-core/src/config.rs lines 375-398 [VERIFIED]
pub fn resolve_base_url(&self) -> String {
    if let Some(ref url) = self.model.base_url { return url.clone(); }
    if let Ok(url) = std::env::var("OPENAI_BASE_URL") { return url; }
    crate::constants::OPENROUTER_BASE_URL.to_string()
}

pub fn resolve_api_key(&self) -> Option<String> {
    if let Some(ref key) = self.model.api_key { return Some(key.clone()); }
    match self.model.provider.as_str() {
        "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
        "openai" => std::env::var("OPENAI_API_KEY").ok(),
        _ => std::env::var("OPENROUTER_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY")).ok(),
    }
}
```

### Current SubagentConfig fields that migrate to resolver (to be removed)
```rust
// Source: crates/ironhermes-core/src/config.rs lines 290-297 [VERIFIED]
pub base_url: Option<String>,   // currently SubagentConfig.base_url
pub api_key: Option<String>,    // currently SubagentConfig.api_key
// These raw fields migrate to a `provider: Option<String>` that resolves via ProviderResolver
```

### AgentLoop current max_iterations guard (budget counter hooks here)
```rust
// Source: crates/ironhermes-agent/src/agent_loop.rs lines 170-173 [VERIFIED]
if turns_used >= self.max_iterations {
    warn!(turns = turns_used, "Max iterations reached");
    break;
}
// Budget counter (Arc<AtomicUsize>) will be incremented at line 181 (turns_used += 1)
// Threshold checks happen before the LLM call, after increment
```

### Anthropic credentials.json structure (confirmed)
```json
// Source: ~/.claude/credentials.json [VERIFIED: keys confirmed via file check]
{
  "oauth": {
    "accessToken": "...",
    "refreshToken": "...",
    "expiresAt": 1234567890
  }
}
```

### Atomic budget counter pattern
```rust
// Source: [ASSUMED — std::sync::atomic, idiomatic Rust]
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// Parent creates:
let budget = Arc::new(AtomicUsize::new(0));

// Per turn (replaces turns_used += 1):
let used = budget.fetch_add(1, Ordering::SeqCst) + 1;
let pct = used * 100 / max_turns;

// Child receives clone of same Arc:
let child_budget = budget.clone();  // Same underlying counter
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Scattered `resolve_base_url()` / `resolve_api_key()` at 8 call sites | `ProviderResolver.build_client()` as single choke point | Phase 12 | Single point of change for credential/endpoint logic |
| Only OpenAI-compatible (chat_completions) mode | Three API modes with enum dispatch | Phase 12 | Native Anthropic path enabled |
| No fallback on provider failure | One-shot fallback chain | Phase 12 | Resilience to 429/5xx |
| Per-agent iteration cap (local counter) | Shared Arc<AtomicUsize> budget | Phase 12 | Parent budget caps total turns across all subagents |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Anthropic Messages API requires alternating user/assistant turns | Common Pitfalls #1 | Adapter may not need merge logic; extra code is harmless but unnecessary |
| A2 | Anthropic credential refresh endpoint accepts refreshToken via POST | Pattern 7 | DEFERRED — refresh not implemented this phase; discovery-only per D-09 |
| A3 | `AnyClient` enum dispatch is preferred over Box<dyn trait> | Pattern 2 | Trait object approach would require different signature changes; both work |
| A4 | Budget injection is prepended to system message content (string concat) | Pattern 5 | May need to modify PromptBuilder instead if system message is structured |
| A5 | Fallback trigger on "None choices" / "missing content" from LLM is via pattern match on `AgentResult` fields | Common Pitfalls #5 | Trigger detection logic differs; does not affect fallback mechanism correctness |

---

## Open Questions (RESOLVED)

1. **Anthropic OAuth refresh endpoint** [RESOLVED — DEFERRED]
   - D-09 narrowed to credential discovery only. OAuth token refresh (preflight expiry check, refresh POST, retry-on-401) deferred to a follow-on phase. The refresh endpoint URL/body format will be researched when that phase is planned.
   - For this phase: read `oauth.accessToken` from `~/.claude/credentials.json` at startup; fall back to `ANTHROPIC_API_KEY` env var.

2. **SubagentConfig migration scope** [RESOLVED]
   - Decision: Keep `SubagentConfig.provider: Option<String>` as override mechanism. Remove raw `base_url`/`api_key` fields. Named provider resolved via `ProviderResolver`. Implemented in Plan 04.

3. **Cron ACP call sites** [RESOLVED]
   - ACP is Phase 22 scope. PROV-01 satisfied by wiring CLI/gateway/cron/batch in this phase. ACP will use the same `ProviderResolver` when implemented.

---

## Environment Availability

Step 2.6: SKIPPED (no external tool dependencies — all changes are code/config within the Rust workspace)

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness (`cargo test`) [VERIFIED: Cargo.toml has no separate test framework] |
| Config file | `Cargo.toml` workspace members |
| Quick run command | `cargo test -p ironhermes-agent -p ironhermes-core 2>&1` |
| Full suite command | `cargo test --workspace 2>&1` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| PROV-01 | ProviderResolver replaces all call sites | unit | `cargo test -p ironhermes-core provider 2>&1` | ❌ Wave 0 |
| PROV-02 | ApiMode variants + client dispatch | unit | `cargo test -p ironhermes-agent api_mode 2>&1` | ❌ Wave 0 |
| PROV-03 | Resolution precedence order | unit | `cargo test -p ironhermes-core resolution_precedence 2>&1` | ❌ Wave 0 |
| PROV-04 | Key scoped to provider URL | unit | `cargo test -p ironhermes-core key_scoping 2>&1` | ❌ Wave 0 |
| PROV-05 | Anthropic credential discovery order | unit | `cargo test -p ironhermes-agent anthropic_creds 2>&1` | ❌ Wave 0 |
| PROV-06 | Auxiliary role routing | unit | `cargo test -p ironhermes-core role_routing 2>&1` | ❌ Wave 0 |
| PROV-07 | Fallback triggers on 429/5xx/401 | unit | `cargo test -p ironhermes-agent fallback 2>&1` | ❌ Wave 0 |
| PROV-08 | Named custom providers load from config | unit | `cargo test -p ironhermes-core custom_providers 2>&1` | ❌ Wave 0 |
| PROV-09 | Budget thresholds inject correct messages | unit | `cargo test -p ironhermes-agent budget 2>&1` | ❌ Wave 0 |
| PROV-10 | Shared budget counter increments across parent+child | unit | `cargo test -p ironhermes-agent shared_budget 2>&1` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-agent -p ironhermes-core 2>&1`
- **Per wave merge:** `cargo test --workspace 2>&1`
- **Phase gate:** Full workspace test suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-core/src/provider.rs` — new file for ProviderResolver, ApiMode, ResolvedEndpoint with unit tests
- [ ] `crates/ironhermes-agent/src/anthropic_client.rs` — new file for AnthropicClient + adapter with unit tests
- [ ] `crates/ironhermes-agent/src/any_client.rs` — new file or extension of client.rs for AnyClient enum
- [ ] Budget counter tests in `crates/ironhermes-agent/src/agent_loop.rs` `#[cfg(test)]` block (already has test block at line ~625 [VERIFIED])

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | API key scoping in ResolvedEndpoint; no key leakage across providers |
| V3 Session Management | no | Not applicable to provider resolution |
| V4 Access Control | no | Not applicable |
| V5 Input Validation | yes | Provider name validation — reject unknown provider names with hard error |
| V6 Cryptography | no | TLS via reqwest/rustls — already in workspace |

### Known Threat Patterns for Provider Resolution

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| API key sent to wrong endpoint | Information Disclosure | ResolvedEndpoint pairs key+URL — never expose raw key separately |
| Config injection via custom provider base_url | Tampering | Validate URL scheme (https only, or explicit localhost exception) before storing in resolver |
| Credential file path traversal | Elevation of Privilege | Use `dirs::home_dir()` to compute credential path — never accept user-supplied paths for credential files |
| Fallback to unintended provider | Tampering | Fallback providers must be in the named provider map — unknown names rejected at startup |

---

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-core/src/config.rs` [VERIFIED] — Current Config struct, resolve_base_url(), resolve_api_key() implementation
- `crates/ironhermes-agent/src/client.rs` [VERIFIED] — LlmClient struct, streaming architecture
- `crates/ironhermes-agent/src/agent_loop.rs` [VERIFIED] — Turn counter location (line 181), max_iterations guard (line 170), AgentLoop struct
- `crates/ironhermes-agent/src/subagent_runner.rs` [VERIFIED] — parent_base_url/parent_api_key/override pattern
- `crates/ironhermes-cli/src/main.rs` [VERIFIED] — All CLI call sites for resolve_base_url/resolve_api_key
- `crates/ironhermes-gateway/src/handler.rs` [VERIFIED] — Gateway handler call site (line 352)
- `crates/ironhermes-gateway/src/runner.rs` [VERIFIED] — Cron call site (line 646)
- `~/.claude/credentials.json` [VERIFIED] — Top-level key is `oauth`
- `Cargo.toml` workspace [VERIFIED] — All dependencies confirmed present; no new deps needed

### Secondary (MEDIUM confidence)
- `.planning/phases/12-provider-resolution/12-CONTEXT.md` [VERIFIED] — Locked decisions D-01 through D-15
- `.planning/codebase/ARCH.md` [VERIFIED] — Crate dependency graph, concurrency patterns

### Tertiary (LOW confidence)
- Anthropic Messages API format translation rules [ASSUMED] — alternating turns, system parameter extraction, tool_use block format — needs verification against current Anthropic API docs before implementation

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all dependencies verified in Cargo.toml; no new deps needed
- Architecture patterns: HIGH for resolver/budget (directly from locked decisions); MEDIUM for Anthropic adapter details (format assumptions need verification)
- Pitfalls: HIGH for provider-scoping and config backward-compat (pattern established in codebase); MEDIUM for Anthropic-specific format errors (assumed from API knowledge)

**Research date:** 2026-04-11
**Valid until:** 2026-05-11 (stable Rust ecosystem; Anthropic API format could change)
