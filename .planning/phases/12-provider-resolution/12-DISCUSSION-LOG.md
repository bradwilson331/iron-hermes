# Phase 12: Provider Resolution - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-11
**Phase:** 12-provider-resolution
**Areas discussed:** Resolver API shape, API mode routing, Fallback & error recovery, Iteration budget

---

## Resolver API shape

| Option | Description | Selected |
|--------|-------------|----------|
| Resolver struct | ProviderResolver struct built once at startup, Clone + Send + Sync | ✓ |
| Resolver trait | ProviderResolver trait with resolve() method + ConfigResolver impl | |
| You decide | Claude picks | |

**User's choice:** Resolver struct
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Resolve once at startup | Build lookup table at startup. Restart for changes. Matches frozen-snapshot pattern. | ✓ |
| Re-resolve per call | Check config + env on every call | |
| Hybrid — cached with refresh | Resolve at startup, support explicit refresh() | |

**User's choice:** Resolve once at startup
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Resolver produces LlmClient | resolver.build_client(provider, model) → LlmClient. Single point of change. | ✓ |
| Resolver returns ResolvedEndpoint | resolver.resolve() → { base_url, api_key, api_mode }. Call sites construct client. | |
| You decide | Claude picks | |

**User's choice:** Resolver produces LlmClient
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| providers map in config | config.yaml `providers:` section with named entries (base_url, api_key, model, api_mode) | ✓ |
| Flat model.* overrides | Multiple profiles under model.providers.{name} | |
| You decide | Claude picks | |

**User's choice:** providers map in config
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Named roles in config | `model.roles:` mapping role names to provider+model pair with fallback | |
| Per-feature model override | Each feature's config section specifies own override | |
| You decide | Claude picks | |

**User's choice:** Other — User provided full hermes-agent Provider Runtime Resolution architecture documentation
**Notes:** User shared comprehensive hermes-agent docs covering: resolution precedence, 17+ provider families, API key scoping, native Anthropic path with refreshable credentials, Codex Responses path, auxiliary model routing with `provider: main` fallback, fallback model one-shot pattern with trigger points and activation flow. This became the authoritative reference for the entire phase.

---

| Option | Description | Selected |
|--------|-------------|----------|
| OpenRouter | Current default, OPENROUTER_API_KEY scoping | ✓ |
| Anthropic native | Native Messages API + adapter, refreshable creds | ✓ |
| OpenAI / custom endpoint | Generic OpenAI-compatible including local servers | ✓ |
| Named custom providers | User-defined in config.yaml custom_providers list | ✓ |

**User's choice:** All four selected
**Notes:** None

---

## API mode routing

| Option | Description | Selected |
|--------|-------------|----------|
| ApiMode enum on resolver | ResolvedEndpoint includes ApiMode enum, build_client() returns right type | ✓ |
| Single client with mode flag | LlmClient gains api_mode field, branches internally | |
| You decide | Claude picks | |

**User's choice:** ApiMode enum on resolver
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| ironhermes-agent crate | Alongside LlmClient, AnthropicClient as parallel impl | ✓ |
| ironhermes-core crate | In core alongside message types | |
| You decide | Claude picks | |

**User's choice:** ironhermes-agent crate
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Full refreshable support | Port hermes-agent pattern: prefer Claude Code cred files, preflight refresh, retry on 401 | ✓ |
| Static API key only | ANTHROPIC_API_KEY env var only, no refresh | |
| You decide | Claude picks | |

**User's choice:** Full refreshable support
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Enum variant + stub | Define CodexResponses in ApiMode, error at runtime if selected | ✓ |
| Full implementation | Build complete Codex Responses API client | |
| You decide | Claude picks | |

**User's choice:** Enum variant + stub
**Notes:** None

---

## Fallback & error recovery

| Option | Description | Selected |
|--------|-------------|----------|
| Port hermes-agent pattern | One-shot fallback in AgentLoop retry logic, 3 trigger points, swap in-place | ✓ |
| Resolver-level fallback chain | Resolver holds ordered list, resolver.next_fallback() returns new client | |
| You decide | Claude picks | |

**User's choice:** Port hermes-agent pattern
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Match hermes-agent | Main agent only (CLI + gateway). No subagent/cron fallback. | ✓ |
| Universal fallback | Extend to all call sites including subagents and cron | |
| You decide | Claude picks | |

**User's choice:** Match hermes-agent
**Notes:** None

---

## Iteration budget

| Option | Description | Selected |
|--------|-------------|----------|
| Iteration count | Budget = max_turns, increment per turn, thresholds on count | ✓ |
| Token usage | Cumulative token usage against configurable cap | |
| You decide | Claude picks | |

**User's choice:** Iteration count
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| Shared counter via Arc | Arc<AtomicUsize>, child gets clone, parent's max_turns is global cap | ✓ |
| Budget allocation | Parent allocates portion to each child, local caps | |
| You decide | Claude picks | |

**User's choice:** Shared counter via Arc
**Notes:** None

---

| Option | Description | Selected |
|--------|-------------|----------|
| System prompt injection | 70%: caution message, 90%: warning message, 100%: hard stop | ✓ |
| Callback-based signals | AgentLoop emits budget events, call sites handle | |
| You decide | Claude picks | |

**User's choice:** System prompt injection
**Notes:** None
