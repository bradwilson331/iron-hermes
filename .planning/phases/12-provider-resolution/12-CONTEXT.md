# Phase 12: Provider Resolution - Context

**Gathered:** 2026-04-11
**Status:** Ready for planning

<domain>
## Phase Boundary

A single shared runtime resolver maps (provider, model) to (api_mode, api_key, base_url) for every call site in the system — CLI, gateway, cron, ACP, and auxiliary calls. Eliminates the duplicated `config.resolve_base_url()` / `config.resolve_api_key()` pattern currently scattered across 5+ call sites. Adds multi-API-mode support (chat_completions, anthropic_messages, codex_responses), fallback provider chain, iteration budget enforcement, and named custom providers.

</domain>

<decisions>
## Implementation Decisions

### Resolver API shape
- **D-01:** `ProviderResolver` is a struct (not a trait), built once at startup from Config + env vars. It is `Clone + Send + Sync` and passed to call sites like `LlmClient` is today.
- **D-02:** Resolve once at startup — build a lookup table from config.yaml + environment variables. If config changes, restart required. Matches the frozen-snapshot pattern from Phase 11.
- **D-03:** Resolver produces `LlmClient` directly via `resolver.build_client(provider, model)`. Call sites never touch base_url/api_key directly. Single point of change for all client construction.
- **D-04:** Config.yaml gains a `providers:` map with named entries, each specifying `base_url`, `api_key`, `default_model`, and `api_mode`. Built-in providers (openrouter, anthropic, openai) are pre-registered but overridable by user config.
- **D-05:** Auxiliary model routing ports hermes-agent's pattern: auxiliary tasks (vision, compression, session search, skills hub, MCP helper, memory flushes) use their own provider/model routing with a `provider: main` option that falls back to the shared resolver path. Config has a `model.roles:` section mapping role names to provider+model pairs.
- **D-06:** Four first-class providers in this phase: OpenRouter (current default), Anthropic native (Messages API + adapter), OpenAI/custom endpoint (generic OpenAI-compatible), and named custom providers (user-defined in `custom_providers` list).

### API mode routing
- **D-07:** `ApiMode` enum with three variants: `ChatCompletions`, `CodexResponses`, `AnthropicMessages`. `ResolvedEndpoint` includes the `ApiMode` and `build_client()` returns the right client type for the mode.
- **D-08:** Anthropic adapter lives in `ironhermes-agent` crate alongside `LlmClient`. `AnthropicClient` as a parallel implementation to `LlmClient`, both behind a common trait or enum dispatch. Keeps format conversion close to where requests are made.
- **D-09:** Anthropic native path supports full refreshable credentials — prefer Claude Code credential files with refreshable auth, preflight refresh before API calls, retry once on 401 after rebuilding client. Port hermes-agent's credential resolution pattern per PROV-05.
- **D-10:** Codex Responses API mode: define `CodexResponses` variant in `ApiMode` enum, wire up resolution so config can select it, but actual Responses API client implementation is deferred — returns an error if selected at runtime. Keeps the abstraction complete without blocking on a rarely-used path.

### Fallback & error recovery
- **D-11:** Port hermes-agent's one-shot fallback pattern in AgentLoop's retry logic. Three trigger points: (1) max retries on invalid API responses (None choices, missing content), (2) non-retryable client errors (HTTP 401, 403, 404), (3) max retries on transient errors (HTTP 429, 5xx). Swap client in-place, reset retry count, set `_fallback_activated` flag to prevent re-firing.
- **D-12:** Fallback scope matches hermes-agent: main agent only (CLI interactive + gateway message handling). Subagents inherit parent's provider without fallback config. Cron runs with fixed provider, no fallback mechanism. Auxiliary tasks use their own independent resolution chain.

### Iteration budget
- **D-13:** Budget tracks iteration count (not tokens). Budget = `max_turns` from config. Counter increments per agent turn (tool call + response cycle). 70/90/100% thresholds applied to that count.
- **D-14:** Budget shared between parent and child agents via `Arc<AtomicUsize>`. Parent creates the shared counter, child agents receive a clone. Each turn increments the shared counter. Parent's `max_turns` is the global cap.
- **D-15:** Threshold behavior via system prompt injection: at 70% inject `[Caution] consolidate your work` into next system message; at 90% inject `[Warning] respond now, summarize progress`; at 100% hard stop the agent loop and return last response. Guides model behavior through prompting.

### Claude's Discretion
- `ResolvedEndpoint` struct design (fields beyond api_mode/base_url/api_key)
- Common client trait vs enum dispatch for LlmClient/AnthropicClient
- Credential file discovery and refresh mechanism details for Anthropic
- Exact config.yaml schema for `providers:`, `custom_providers:`, and `model.roles:`
- How `SubagentConfig.base_url`/`api_key` overrides migrate to the resolver pattern
- Provider-specific API key scoping logic (OPENROUTER_API_KEY only to openrouter.ai, etc.)
- Error retry counts and backoff strategy within each trigger category

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Provider resolution requirements
- `.planning/REQUIREMENTS.md` — PROV-01 (shared resolver), PROV-02 (three API modes), PROV-03 (resolution precedence), PROV-04 (key scoping), PROV-05 (Anthropic adapter + refreshable creds), PROV-06 (auxiliary routing), PROV-07 (fallback chain), PROV-08 (named custom providers), PROV-09 (iteration budget), PROV-10 (budget parent/child sharing)

### hermes-agent architecture (Python reference)
- User-provided hermes-agent Provider Runtime Resolution docs — covers: resolution precedence, provider families, API key scoping, native Anthropic path, Codex Responses path, auxiliary model routing, fallback model behavior (one-shot pattern, trigger points, activation flow, config flow, scope limitations). **This is the primary architecture reference for porting.**

### Existing IronHermes code
- `crates/ironhermes-core/src/config.rs` — Current `Config` struct with `ModelConfig`, `resolve_base_url()`, `resolve_api_key()`. These methods are the duplication target being replaced.
- `crates/ironhermes-agent/src/client.rs` — Current `LlmClient` (OpenAI chat completions only). Will gain sibling `AnthropicClient` and common abstraction.
- `crates/ironhermes-agent/src/subagent_runner.rs` — `AgentSubagentRunner` with `parent_base_url`/`parent_api_key`/`override_base_url`/`override_api_key` — migrates to resolver pattern.
- `crates/ironhermes-cli/src/main.rs` — CLI client construction (lines ~237, ~302, ~510, ~607) — all become `resolver.build_client()`.
- `crates/ironhermes-gateway/src/handler.rs` — Gateway client construction (line ~352) — becomes `resolver.build_client()`.
- `crates/ironhermes-gateway/src/runner.rs` — Cron job client construction (line ~646) — becomes `resolver.build_client()`.

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph, key abstractions, concurrency model
- `.planning/ROADMAP.md` — Phase 12 success criteria, downstream dependencies (Phase 15 depends on 12)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `Config.resolve_base_url()` / `Config.resolve_api_key()` — logic to port into `ProviderResolver`, then remove these methods
- `LlmClient` — becomes the `ChatCompletions` mode client, gains a sibling `AnthropicClient`
- `ModelConfig` — already has `provider`, `base_url`, `api_key`, `vision_model` fields as starting point for richer provider config
- `SubagentConfig` — has `base_url`/`api_key` override pattern that migrates to resolver

### Established Patterns
- `async_trait + Send + Sync` for shared abstractions (Tool trait, MemoryProvider trait)
- `Arc<Mutex<>>` sharing pattern for stateful resources
- `Arc<AtomicUsize>` available for lock-free counters (iteration budget)
- Config-driven selection with hard error on misconfiguration (from Phase 11)

### Integration Points
- Every `LlmClient::new()` call site → replaced by `resolver.build_client()`
- `AgentLoop` → gains budget counter check per turn + system prompt injection at thresholds
- `AgentSubagentRunner` → receives shared budget counter from parent
- `Config` → gains `providers:`, `custom_providers:`, `model.roles:` sections

</code_context>

<specifics>
## Specific Ideas

- Port hermes-agent's provider resolution faithfully — the Python architecture docs provided during discussion are the authoritative reference for how resolution precedence, key scoping, fallback, and auxiliary routing should work
- API key scoping is critical: OPENROUTER_API_KEY only sent to openrouter.ai endpoints, ANTHROPIC_API_KEY only to Anthropic endpoints, OPENAI_API_KEY used for custom endpoints and as fallback
- The distinction between "real custom endpoint" vs "OpenRouter fallback" matters for local model servers and config-saved endpoints
- Codex Responses API is stub-only for now — define the variant, wire the config, error at runtime if actually selected

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" — belongs in Phase 23 (Configuration & Setup Wizard), not provider resolution scope.

</deferred>

---

*Phase: 12-provider-resolution*
*Context gathered: 2026-04-11*
