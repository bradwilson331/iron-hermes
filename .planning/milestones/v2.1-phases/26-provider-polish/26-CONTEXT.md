# Phase 26: Provider Polish - Context

**Gathered:** 2026-04-29
**Status:** Ready for planning
**Source:** Discuss-phase with user picking all 6 gray areas + "all-Recommended" defaults; user steer: "this should cover fallback APIs and providers"

<domain>
## Phase Boundary

Provider polish — three closely-related correctness/UX fixes on top of the existing Phase 12 + Phase 21 + Phase 21.3 ProviderResolver infrastructure:

- **PROV-04: Per-provider key scoping.** Eliminate the generic `OPENAI_API_KEY` fallback for unknown providers (the leak path at `crates/ironhermes-core/src/provider.rs:212`). Each provider — built-in or custom — gets its own explicit `api_key_env: VAR_NAME` reference; no implicit cross-provider fallback. Legacy `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `OPENROUTER_API_KEY` env vars stay as a deprecated-but-still-accepted fallback for the matching built-in provider only, with a one-line stderr deprecation banner on load.

- **PROV-06: Auxiliary model routing.** Add an `auxiliary: { provider, model }` config block that routes the seven helper task categories (vision, compression, session_search, skills_hub, mcp_helper, summarization, curator) to a separate cheaper model. Per-task overrides supported. Wire the existing `ProviderResolver::resolve_role(role)` through `crates/ironhermes-agent/src/engine_factory.rs` and `crates/ironhermes-agent/src/summarizing_engine.rs` so the auxiliary endpoint is actually used (today the field is referenced but the route doesn't complete). (Phase 26 originally shipped with 5 roles; Phase 25.2 D-13 added `summarization`, Phase 25.3 D-P0-1 added `curator`.)

- **PROV-08: Named custom providers in config.yaml.** Already partially implemented via `config.custom_providers: Vec<CustomProvider>`. Phase 26 unifies the surface: one `providers:` HashMap covers built-ins and custom alike, deprecates the parallel `custom_providers:` Vec (redirected with stderr migration warning), and `--provider <name>` selects any name from this single block.

Phase 26 covers **PROV-04, PROV-06, PROV-08** only.

**Out of scope:**
- Provider auto-discovery (e.g., scanning Ollama/LocalAI on localhost) — separate concern.
- Per-request key rotation / streaming key refresh — not in REQUIREMENTS.md.
- Multi-key-per-provider (org-keyed routing) — punted.
- Provider rate limiting / quota tracking — separate phase.
- Live failover (auto-switch on 5xx) — `fallback_providers` chain stays as-is from Phase 21; no new failover policy in Phase 26.

</domain>

<decisions>
## Implementation Decisions

### Provider Config Block Shape (PROV-04 + PROV-08)

- **D-01: One unified `providers:` HashMap covers built-ins + custom.** Schema:
  ```yaml
  providers:
    openai:
      base_url: https://api.openai.com/v1   # optional for built-ins (defaults baked in)
      api_key_env: OPENAI_API_KEY
      default_model: gpt-4o                  # optional override
      api_mode: chat_completions             # optional (chat_completions | anthropic_messages)
      fallback_providers: []                 # optional, references other provider names
    my-local-llm:
      base_url: http://localhost:8080/v1
      api_key_env: MY_LLM_KEY
      default_model: llama3.1
      api_mode: chat_completions
      fallback_providers: [openai]
  ```
  No `api_key:` literal field — env-var-name reference only. Each provider has exactly one URL and one key resolution path.

- **D-02: `config.custom_providers: Vec<CustomProvider>` is DEPRECATED.** On config load, if `custom_providers:` is present and `providers:` has no entry for a given custom-provider name, copy the entry over and emit one stderr warning per migrated entry: `[provider:NAME] migrated from deprecated custom_providers list — move to providers.NAME in config.yaml`. After two minor releases, drop `custom_providers:` parsing entirely.

- **D-03: Built-in providers (anthropic, openai, openrouter) are pre-populated** with canonical defaults (URL + ApiMode + default model) at resolver build, then config.providers entries OVERLAY those defaults. Same overlay rule that exists today (provider.rs:149) — Phase 26 keeps it. A user setting `providers.openai.base_url:` to a custom URL is allowed (Azure proxy, openai-compatible gateway, etc.).

- **D-04: api_key_env values are validated as identifier-like at config load.** Regex: `[A-Z][A-Z0-9_]*` (uppercase, digit, underscore; must start with letter). Rejects empty / lowercase / shell-injection-shaped values. Mirrors the slug-validation pattern from Phase 24 / Phase 25.

### Auxiliary Routing Granularity (PROV-06)

- **D-05: Single `auxiliary` block + optional per-task overrides.** Schema:
  ```yaml
  auxiliary:
    provider: openai
    model: gpt-4o-mini
  vision:           # optional override; falls through to auxiliary, then main
    provider: anthropic
    model: claude-haiku-4-5
  compression: { provider: openai, model: gpt-4o-mini }
  session_search:   # absent = use auxiliary
  skills_hub:       # absent = use auxiliary
  mcp_helper:       # absent = use auxiliary
  ```
  Resolution cascade for `resolve_role("vision")`:
  1. If `vision: { provider, model }` is set → use it
  2. Else if `auxiliary: { provider, model }` is set → use it
  3. Else → use main (`config.model.provider` / `config.model.default`)

  All seven role names are reserved keys: `vision`, `compression`, `session_search`, `skills_hub`, `mcp_helper`, `summarization`, `curator` (D-05, Phase 25.2 D-13, Phase 25.3 D-P0-1). Unknown role names rejected at config load.

- **D-06: `auxiliary` is OPTIONAL.** Default config has no `auxiliary:` block — all helper tasks use main. Adding `auxiliary:` is opt-in. Per-task overrides require the role-specific block.

- **D-07: `resolve_role(role)` returns an `Option<ResolvedEndpoint>` (clone or by-value, not `&` borrow).** Current signature returns `Option<ResolvedEndpoint>` (provider.rs:275) — Phase 26 keeps that shape but populates the cascade per D-05. Callers in `engine_factory.rs` and `summarizing_engine.rs` must call `.unwrap_or_else(|| resolver.resolve_for_main().clone())` to handle the role-not-set case.

### Resolver Semantics + Fallback Chain (PROV-04)

- **D-08: Name-keyed lookup stays the resolver's primary contract.** Each entry in `providers.NAME` has its own `api_key_env` — that's how PROV-04 "API key per base URL" is enforced (each named provider has exactly one URL and exactly one resolved key). No (name, base_url) tuple keying. No URL-only mapping. If a user wants two providers at the same base_url with different keys (e.g., two OpenAI orgs), they define two named entries: `openai-personal` and `openai-work`.

- **D-09: `fallback_providers: [other_name, ...]` from Phase 21 stays unchanged.** Phase 26 does NOT add new failover policy. `provider.rs:225-235` already validates the fallback chain references known names (T-12-03) — Phase 26 preserves this validation.

- **D-10: Auxiliary fallback layers ON TOP of provider fallback chains.** When `resolve_role("vision")` selects a per-task or auxiliary endpoint, that endpoint's `fallback_providers` chain is honored. If the chosen aux/role provider is unreachable at runtime (network error / 5xx), the AnyClient fallback path takes over (existing behavior). Phase 26 does NOT add a "fall back from aux to main" policy at the role-resolution layer — that would mask config errors. If `auxiliary.provider` is misconfigured (unknown name), config load fails fast.

- **D-11: PROV-04 leaky fallback REMOVED.** Delete `crates/ironhermes-core/src/provider.rs:212` — the `_ => std::env::var("OPENAI_API_KEY").ok()` arm. Replace with: for built-ins, use the canonical env var matching the provider name (`OPENAI_API_KEY` → `openai`, `ANTHROPIC_API_KEY` → `anthropic`, `OPENROUTER_API_KEY` → `openrouter`); for non-built-ins, ONLY use `providers.NAME.api_key_env` (no fallback). Custom providers without an api_key_env declared get `api_key: None` → calls fail with a clear "no key configured for provider X" error.

### Legacy Env-Var Compatibility (PROV-04)

- **D-12: Legacy env vars `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `OPENROUTER_API_KEY` remain accepted** as a fallback when `providers.<name>.api_key_env` is unset for the matching built-in. Behavior:
  1. If `providers.openai.api_key_env: FOO` is set → use `$FOO`
  2. Else if `OPENAI_API_KEY` is set in env → use it AND emit one-line stderr banner: `[provider:openai] using deprecated env var OPENAI_API_KEY — set providers.openai.api_key_env in config.yaml to silence this warning`
  3. Else → key is None
  Emitted exactly once per resolver build (not once per LLM call). The banner is a Phase 23 D-13 / Phase 25 D-04 cache-break-style stderr warning.

- **D-13: `config.model.api_key` legacy field DEPRECATED.** Today `provider.rs:216` accepts a top-level `model.api_key:` as a fallback for the main provider. Phase 26 keeps this as a deprecated path with the same one-shot stderr banner: `[config:model.api_key] deprecated — set providers.<main-provider>.api_key_env instead`. Drop in next major.

### Operator Surface — `hermes provider` CLI (PROV-04 + PROV-08 visibility)

- **D-14: `hermes provider` subcommand mirrors `hermes toolset` from Phase 25.** Five subcommands:
  - `hermes provider list` — aligned-columns: NAME / BASE_URL / API_KEY / MODEL / ROLE / FALLBACKS, with API_KEY column showing `✓ via $VAR` or `✗ missing $VAR` (without ever printing the key value)
  - `hermes provider show <name>` — single-provider detailed view (URL, mode, default_model, fallback chain, role assignments referencing this provider)
  - `hermes provider test <name>` — live ping: GET `${base_url}/models` (or POST a tiny no-op completion if /models 404s) with the resolved key; report 2xx/4xx/5xx status, latency, and which key var was used
  - `hermes provider enable <name>` / `hermes provider disable <name>` — toggle a `providers.NAME.disabled: true` flag (NEW, additive). Disabled providers skip resolver entry creation. Mirrors Phase 25 D-05 per-profile persistence pattern.
  Slash commands MIRROR the CLI per Phase 25 D-06: `/provider list/show/test/enable/disable` registered through Phase 21.1 CommandRouter, session-only mutation for enable/disable.

- **D-15: `hermes provider test <name>` NEVER prints the API key value to stdout/stderr.** Output format: `[provider:NAME] HTTP 200 (latency 142ms) — key from $OPENAI_API_KEY` — the env var name is shown, the value is not. T-26-01 mitigation.

### Cache-Break Banner Semantics

- **D-16: Cache-break stderr banner emitted ONLY on persistent `hermes config set` / `hermes provider enable|disable` writes that change provider/aux config.** Format: `[provider: NAME] config changed — schema cache will rebuild on next LLM call` (mirrors Phase 23 D-13 / Phase 25 D-04 banner). Session-only `--provider <name>` flag changes do NOT emit this banner — they're per-invocation overrides, not config mutations.

- **D-17: `auxiliary` and per-task overrides are also cache-breaking.** Same banner on `hermes config set auxiliary.provider openai` etc. — the schema sent to the LLM doesn't change, but the model identity does, which invalidates cache.

### Config Schema (carry-forward)

- **D-18: New types in `ironhermes-core` use plain Strings per Phase 22.4.2.2 / Phase 23 D-12 / Phase 24 D-17 / Phase 25 D-25.** `ProviderConfig`, `AuxiliaryConfig`, `RoleOverride` — all field types are String / Option<String> / Vec<String>. No enum cross-crate boundaries.

- **D-19: `apply_minimum_viable_answers` (Phase 23 testability seam) is REUSED for the `hermes setup` provider/aux defaults.** When the wizard captures a default provider, write `providers.<chosen>.api_key_env` and (if user opted in) `auxiliary.provider` / `auxiliary.model`. No new testability seam.

### Test Strategy

- **D-20: Three integration tests are mandatory.** Plan must lock all three:
  1. `key_does_not_leak_to_wrong_provider` — Spawn binary with `OPENAI_API_KEY=sk-real` set, define a custom provider `my-local-llm` at `http://localhost:8080/v1` with NO api_key_env. Capture the outbound HTTP request to `my-local-llm` (mock server) and assert the Authorization header does NOT contain `sk-real`. PROV-04 verbatim.
  2. `auxiliary_routes_to_separate_model` — Set `auxiliary: { provider: openai, model: gpt-4o-mini }` with main `provider: anthropic, model: claude-sonnet-4`. Trigger a compression task; assert the outbound request goes to `api.openai.com` with model `gpt-4o-mini`, NOT to `api.anthropic.com`. PROV-06 verbatim.
  3. `custom_provider_selectable_by_name` — Define `providers.my-local-llm` with custom base_url + api_key_env. Run `hermes --provider my-local-llm chat "ping"`. Assert resolver returns the custom endpoint AND the request hits the configured base_url. PROV-08 verbatim.

- **D-21: One unit test must cover the PROV-04 leak fix.** `legacy_openai_key_does_not_leak_to_unknown_provider` — set `OPENAI_API_KEY` env, build resolver with a custom provider that has no api_key_env declared, assert that custom provider's `api_key` is None.

### Folded Todos

- *(none — no pending todos in `.planning/todos/` match Phase 26 scope; will be re-checked at plan-phase)*

> **Phase 25.3 update (2026-05-03):** RESERVED_ROLE_NAMES was extended from 6 -> 7 with `"curator"` (Phase 25.3 Plan 0, D-P0-1) so Phase 25.4 Curator can `resolve_role("curator")` without forward references. The cascade contract documented above is unchanged — `curator` is just another auxiliary role following the same precedence rules.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### v2.1 Milestone Architectural Principles
- `.planning/PROJECT.md` §"Architectural Principles (carried through every v2.1 phase)" — Principle #2 (cache-awareness): provider config changes are cache-breaking on the next LLM call. Phase 26 D-16 / D-17 surface this with stderr banners.
- `.planning/REQUIREMENTS.md` §"Provider polish" lines 132/134/136 — PROV-04, PROV-06, PROV-08 verbatim text.
- `.planning/ROADMAP.md` §"Phase 26: Provider Polish" — full success criteria + dependencies on Phase 21 (ProviderResolver) and Phase 23 (config CLI).

### Phase 21 Carry-Forward (REQUIRED reading)
- `.planning/phases/21-commandline-ui-update-polish-cli-ux-including-graceful-doubl/` — original ProviderResolver + ResolvedEndpoint design. Phase 26 D-08/D-09 preserve the name-keyed contract.
- `.planning/phases/21.3-model-metadata-models-dev-context-lengths-token-estimation/` — `ResolvedEndpoint.model_metadata` + `config_context_length` fields. Phase 26 keeps these untouched on per-role endpoints.

### Phase 23 Carry-Forward (REQUIRED reading)
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-CONTEXT.md` — preflight middleware, dotted-path config setter (D-22 reused), `apply_minimum_viable_answers` testability seam (D-19 reused), Learning Loop banner stack ordering, cache-break warning style (D-13 mirrored in D-16).

### Phase 24 Carry-Forward (REQUIRED reading)
- `.planning/phases/24-profile-isolation/24-CONTEXT.md` — `IRONHERMES_HOME` pivot makes per-profile provider config automatic (D-14 enable/disable writes to active profile). Slug validator pattern reused in D-04 for `api_key_env` validation.

### Phase 25 Carry-Forward (REQUIRED reading)
- `.planning/phases/25-toolset-management/25-CONTEXT.md` — `hermes toolset` subcommand pattern (D-04). Phase 26 D-14 mirrors this for `hermes provider`. Cross-crate plain-String types (D-25 → D-18). Stderr banner pattern (D-04 → D-16).

### Phase 22.4.2.2 Carry-Forward
- `.planning/PROJECT.md` Key Decisions row "Cross-crate transport types use plain Strings" — Phase 26 D-18 follows verbatim.

### Codebase Code Sites (verified via grep)
- `crates/ironhermes-core/src/provider.rs:18-30` — `ResolvedEndpoint` struct. Phase 26 keeps shape; the per-resolution cascade lives elsewhere (resolve_role implementation).
- `crates/ironhermes-core/src/provider.rs:91-105` — `ProviderResolver::build(config)` entry point. Phase 26 D-11 modifies the API key resolution loop (lines 200-222).
- `crates/ironhermes-core/src/provider.rs:212` — **THE PROV-04 LEAK** (`_ => std::env::var("OPENAI_API_KEY").ok()`). Phase 26 D-11 deletes this arm.
- `crates/ironhermes-core/src/provider.rs:215-216` — `config.model.api_key` fallback for main provider. Phase 26 D-13 retains with deprecation banner.
- `crates/ironhermes-core/src/provider.rs:259-269` — `resolve()` and `resolve_for_main()`. Phase 26 D-07 modifies `resolve_role()` (line 275) to implement the auxiliary cascade.
- `crates/ironhermes-core/src/provider.rs:175-198` — `config.custom_providers` overlay. Phase 26 D-02 deprecates this loop, redirects entries to the unified `providers:` HashMap.
- `crates/ironhermes-agent/src/engine_factory.rs` and `crates/ironhermes-agent/src/summarizing_engine.rs` — current `auxiliary_model` references. Phase 26 wires `resolver.resolve_role("compression")` etc. through these factories.
- `crates/ironhermes-cli/src/main.rs` Cli struct — Phase 26 D-14 adds `Provider(ProviderCommand)` to the `Commands` enum (mirrors Phase 23 `Config`, Phase 25 `Toolset`).
- `crates/ironhermes-tools/src/web_search.rs:67` and `crates/ironhermes-tools/src/web_read.rs:172` — env-var prereq pattern from Phase 25. Phase 26 reuses this for `hermes provider test` (which can call the same `prerequisites()` method internally to surface missing keys before live-ping).

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`ProviderResolver::build(&Config)` (`provider.rs:104`)** — already pre-populates 3 built-ins, overlays config.providers, adds custom_providers, validates fallback chains. Phase 26 modifies the API-key resolution loop (D-11) and the role resolution (D-07); the overall pipeline stays.
- **`ResolvedEndpoint::context_length()` (`provider.rs:33`)** — Phase 21.3 D-06 precedence (config > metadata > default). Phase 26 keeps as-is on per-role endpoints.
- **`is_provider_url_safe(url)` (`provider.rs:179`)** — T-12-02 https-or-localhost validator. Phase 26 keeps as-is for new `providers.*.base_url` writes.
- **`ModelRegistry` (`provider.rs:106-108`)** — Phase 21.3 model metadata cache. Phase 26 doesn't touch it.
- **Phase 23's `apply_minimum_viable_answers` testability seam (`setup_wizard.rs:250`)** — Phase 26 D-19 reuses for provider/aux defaults capture during `hermes setup`.
- **Phase 25's `validate_toolset_name` slug validator** — Phase 26 D-04 adapts the pattern (uppercase regex variant) for `api_key_env` validation.
- **Phase 21.1 `CommandRouter`** — Phase 26 D-14 registers `/provider list/show/test/enable/disable` here.
- **Phase 24's profile pivot** — Phase 26 D-14 inherits per-profile provider config without any new code.

### Established Patterns

- **Cross-crate plain-String pattern (Phase 22.4.2.2 → 23 D-12 → 24 D-17 → 25 D-25)** — Phase 26 D-18 follows verbatim for new `ProviderConfig`, `AuxiliaryConfig`, `RoleOverride` structs.
- **Stderr-banner UX convention (Phase 21.7 D-11/D-12, Phase 23 D-13, Phase 24 D-08, Phase 25 D-04)** — Phase 26 D-16 mirrors for cache-break warnings; D-12 for legacy env-var deprecation.
- **Subcommand-namespace minimum surface (Phase 23 `hermes config`, Phase 24 `--profile`, Phase 25 `hermes toolset`)** — Phase 26 D-14 stays minimum: list/show/test/enable/disable. NO create/delete/rename/clone/import/export.
- **Atomic file writes via tempfile + rename (Phase 21.5/21.8/24 D-10/25 D-22)** — Phase 26 reuses for any provider config writes (the existing config setter already does this).
- **Per-tool prerequisite pattern (Phase 25 D-09)** — `hermes provider test` may internally use the Tool::prerequisites() pattern to enumerate missing keys before live-ping.

### Integration Points

- **`crates/ironhermes-core/src/provider.rs`** — Phase 26 modifies the API key resolution loop (D-11), the `resolve_role` cascade (D-07), and adds `custom_providers:` migration (D-02). Existing API surface (resolve / resolve_for_main / resolve_role / main_provider / model_registry) is preserved.
- **`crates/ironhermes-core/src/config.rs`** — Phase 26 adds `AuxiliaryConfig`, `RoleOverride` types; deprecates `custom_providers:` field with serde rename + warning at parse time; ensures `providers:` HashMap entries have `api_key_env: Option<String>` field.
- **`crates/ironhermes-agent/src/engine_factory.rs`** + **`summarizing_engine.rs`** — Phase 26 swaps direct `config.auxiliary_model` reads for `resolver.resolve_role(role)` calls. The factories receive an `Arc<ProviderResolver>` (or already have one — verify during research).
- **`crates/ironhermes-cli/src/main.rs` Cli struct** — Phase 26 D-14 adds `Provider(ProviderCommand)` enum variant.
- **`crates/ironhermes-cli/src/setup.rs`** — Phase 26 D-19 adds optional auxiliary-provider stage to `hermes setup`.

</code_context>

<specifics>
## Specific Ideas

- **`hermes provider list` output format** (aligned columns, ANSI-color-aware):
  ```
  NAME              BASE_URL                          API_KEY              MODEL              ROLE      FALLBACKS
  openai            https://api.openai.com/v1         ✓ $OPENAI_API_KEY    gpt-4o            main      —
  anthropic         https://api.anthropic.com         ✓ $ANTHROPIC_API_KEY claude-sonnet-4   —         openai
  openrouter        https://openrouter.ai/api/v1      ✗ missing $OPENROUTER_API_KEY  —       —         —
  my-local-llm      http://localhost:8080/v1          ✓ $MY_LLM_KEY        llama3.1          aux       openai
  ```
  Width-aware. `--json` flag for machine-readable output mirrors Phase 25 list pattern.

- **`hermes provider test <name>` HTTP probe**: `GET ${base_url}/models` first; if 404, try `POST ${base_url}/chat/completions` with `{"model": "<default_model>", "messages": [{"role":"user","content":"ping"}], "max_tokens":1}`. Time the request. Report HTTP status + latency. Never print the key value (D-15).

- **Banner format on `hermes config set providers.openai.api_key_env OPENAI_KEY`**:
  `[provider: openai] config changed — schema cache will rebuild on next LLM call`
  Mirrors Phase 25 D-04 banner style verbatim.

- **`auxiliary` config block as YAML**:
  ```yaml
  auxiliary:
    provider: openai           # any name from `providers:`
    model: gpt-4o-mini         # any model that provider serves
  ```
  Validation at config load: `auxiliary.provider` must be a key in `providers:`; otherwise fail with "auxiliary.provider 'xyz' is not a known provider — define it in providers: first".

- **PROV-04 unit test pseudocode** (D-21):
  ```rust
  #[test]
  #[serial]   // env_lock
  fn legacy_openai_key_does_not_leak_to_unknown_provider() {
      std::env::set_var("OPENAI_API_KEY", "sk-leaked");
      let mut config = Config::default();
      config.providers.insert("my-local-llm".to_string(), ProviderConfig {
          base_url: Some("http://localhost:8080/v1".to_string()),
          api_key_env: None,        // explicitly unset
          ..Default::default()
      });
      let resolver = ProviderResolver::build(&config).unwrap();
      let endpoint = resolver.resolve("my-local-llm").unwrap();
      assert_eq!(endpoint.api_key, None, "OPENAI_API_KEY MUST NOT leak to my-local-llm");
      std::env::remove_var("OPENAI_API_KEY");
  }
  ```

- **Documentation note**: `hermes provider test <name>` is the operator-facing diagnostic; `hermes setup` is the user-facing first-run flow. Avoids the same confusion that motivated the Phase 25 D-18 wizard documentation note.

</specifics>

<deferred>
## Deferred Ideas

- **Provider auto-discovery (Ollama / LocalAI / LM Studio scanners)** — interesting but out of REQUIREMENTS.md for v2.1. Re-open when local-LLM users ask for it.
- **Per-request key rotation** — not in REQUIREMENTS.md. Org/team API key juggling lives in deployment infra, not in the agent.
- **Multi-key-per-provider (e.g., two OpenAI orgs sharing one base_url)** — D-08 explicitly rejects (define two named entries instead).
- **Live failover policy (auto-switch on 5xx)** — `fallback_providers` chain stays as-is from Phase 21. New auto-failover logic deferred to a future "Provider Resilience" phase.
- **Provider rate limiting / quota tracking** — separate concern, separate phase.
- **`hermes provider create / delete / rename / clone / import / export`** — full lifecycle. Skipped per the "active scope only" pattern from Phase 24 D-16 / Phase 25 D-04. Operators edit config.yaml directly for create/delete; rename is just edit-the-key.
- **Encrypted at-rest storage of api_key values** — D-01 explicitly forbids inline `api_key:` literal; values live in env vars (process memory) only. OS-keyring integration is a future phase.
- **`hermes doctor --providers`** — cross-provider availability check (would walk all providers and run `provider test` on each). Skipped per Phase 24 D-16. Operator uses `hermes provider list` (which shows ✓/✗ for keys) + `hermes provider test <name>` for individual live checks.
- **Per-toolset model override (e.g., `tools.web.toolset_model: gpt-4o-mini`)** — interesting (cheap searches with main-model orchestration), but auxiliary routing already covers the helper-task case. Re-open if PROV-06's seven role categories prove insufficient (D-05 + Phase 25.2 D-13 + Phase 25.3 D-P0-1).

### Reviewed Todos (not folded)

None.

</deferred>

---

*Phase: 26-provider-polish*
*Context gathered: 2026-04-29 via /gsd-discuss-phase with all-Recommended defaults*
*All gray areas decided; user reserves the right to object before plan-phase or execute-phase.*
