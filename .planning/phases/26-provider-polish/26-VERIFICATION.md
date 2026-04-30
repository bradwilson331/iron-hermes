---
phase: 26-provider-polish
verified: 2026-04-30T10:00:00Z
status: human_needed
score: 2/3 must-haves verified
overrides_applied: 0
gaps: []
human_verification:
  - test: "Confirm SC-2 scope acceptance: vision and session-search are NOT routed to auxiliary model in production code (only compression is). Plan 03 documents this as intentional greenfield. Accept or reopen."
    expected: "Either accept that Phase 26 only wires compression (not vision/session-search) — or treat this as a gap requiring additional wiring."
    why_human: "The ROADMAP SC-2 says 'compression, vision, and session-search tasks' but Plan 03 audit found vision and session_search have zero LLM consumer call sites in the agent crate today. The team must decide if this deviation is intentional and acceptable."
  - test: "Verify --provider flag behavior without --model: run `hermes --provider my-local-llm chat 'ping'` with my-local-llm in providers: config. Confirm whether it routes to my-local-llm or falls back to main provider."
    expected: "If SC-3 requires --provider alone (without --model) to work, this is a gap. build_client() only uses cli.provider when cli.model is also Some; otherwise build_main_client() is called."
    why_human: "The code path cli.provider is only activated when cli.model is set. SC-3 says '--provider my-local-llm' must work. However, setting model.provider in config.yaml does work via build_main_client. Team must decide if this CLI limitation satisfies SC-3."
---

# Phase 26: Provider Polish — Verification Report

**Phase Goal:** API keys are scoped to their provider's base URL, auxiliary tasks can route to a separate cheaper model, and operators can define named custom providers in config.yaml for any OpenAI-compatible endpoint.
**Verified:** 2026-04-30T10:00:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | Configuring two providers with different base URLs and different API keys sends the correct key to each endpoint — no key leaks to the wrong URL | VERIFIED | D-11 leak arm removed from provider.rs (grep confirms `_ => std::env::var("OPENAI_API_KEY")` arm is absent). `legacy_openai_key_does_not_leak_to_unknown_provider` unit test passes. `key_does_not_leak_to_wrong_provider` integration test passes. |
| 2 | Setting `auxiliary_model` in config.yaml routes compression, vision, and session-search tasks to that model instead of the main conversational model | PARTIAL | Compression routes correctly via `build_role_client(resolver, "compression")` in engine_factory.rs. `auxiliary_routes_to_separate_model` integration test passes. BUT: vision has zero production consumer call sites (greenfield, documented in Plan 03 audit). session_search uses StateStore FTS5, no LLM routing needed. ROADMAP wording includes "vision" which is not wired. |
| 3 | A named custom provider (e.g., `my-local-llm`) defined in config.yaml is selectable as `--provider my-local-llm` and resolves its base URL, API key, and model correctly | PARTIAL | Resolver correctly resolves named providers: `custom_provider_selectable_by_name` integration test passes at resolver level. `--provider` flag exists in CLI. BUT: `build_client()` only uses `cli.provider` when `cli.model` is also set — `hermes --provider my-local-llm chat "ping"` without `--model` falls through to `build_main_client()`. config.yaml-based `model.provider` routing works unconditionally. |

**Score:** 2/3 truths fully verified (Truth 1 VERIFIED, Truths 2 and 3 PARTIAL — require human decision)

### Deferred Items

None — no items identified as covered by later phases.

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-core/src/config.rs` | ProviderConfig.api_key_env, AuxiliaryConfig, validate_api_key_env, validate_role_name | VERIFIED | All fields present and substantive. 16 unit tests in config::tests pass. |
| `crates/ironhermes-core/src/provider.rs` | D-11 leak removed, D-07 resolve_role cascade, D-12 legacy banners | VERIFIED | Leak arm removed. resolve_role 3-level cascade implemented. Once-only banner via OnceLock. 6 cascade tests pass. |
| `crates/ironhermes-agent/src/engine_factory.rs` | build_role_client("compression") wiring, greenfield TODOs | VERIFIED | Compression wired. 7 engine_factory tests pass including cascade regression. Vision/session_search/skills_hub/mcp_helper documented as greenfield with TODO comments. |
| `crates/ironhermes-cli/src/provider_cmd.rs` | hermes provider list/show/test/enable/disable | VERIFIED | All 5 subcommands implemented. D-15 (no key value in output). D-16 cache-break banner. T-26-03 slug validation. |
| `crates/ironhermes-core/src/commands/provider_display.rs` | ProviderRow, render_provider_list, render_provider_show | VERIFIED | File exists with substantive implementation. T-26-01 by construction (no api_key value field). |
| `crates/ironhermes-core/src/wizard.rs` | apply_auxiliary_answer | VERIFIED | Function present and tested (4 unit tests). D-06 empty-skip semantics correct. |
| `crates/ironhermes-cli/src/setup.rs` | D-19 auxiliary stage in run_minimum_viable_flow | VERIFIED | Auxiliary provider prompt added. run_agent_section implemented (replaces bail! stub). |
| `crates/ironhermes-cli/tests/provider_integration.rs` | D-20 Tests 1+2+3, D-21, T-26-01, D-12, D-14, D-16, T-26-03 | VERIFIED | 9/9 tests pass (confirmed by cargo test run). |

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `build_client()` in main.rs | `ProviderResolver::build()` | Config::load() → resolver | WIRED | resolver built from config in build_client() |
| `engine_factory.rs` "summarizing" arm | `ProviderResolver::resolve_role("compression")` | `build_role_client(resolver, "compression")` | WIRED | Line 102 of engine_factory.rs |
| `cmd_provider_enable/disable` | `config_setter::config_set` | dotted-path write "providers.NAME.disabled" | WIRED | Confirmed in provider_cmd.rs lines 252-276 |
| `cli.provider` flag | `build_provider_client(resolver, provider, model)` | Only when `cli.model` is Some | PARTIAL | Without --model, cli.provider is ignored; build_main_client() used instead |
| `AuxiliaryConfig` resolver build | `ProviderResolver::auxiliary_endpoint` | `if config.auxiliary.is_set()` | WIRED | provider.rs lines 373-388 |
| `resolve_role("vision")` | consumer call site | n/a | NOT_WIRED | No production consumer for vision role in agent crate |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
|----------|---------------|--------|--------------------|--------|
| `engine_factory.rs` SummarizingEngine | compression client | `build_role_client(resolver, "compression")` → `resolve_role` → `auxiliary_endpoint` | Yes — flows through cascade to resolver endpoint | FLOWING |
| `provider_cmd.rs` cmd_provider_list | ProviderRow.api_key_status | `endpoint.api_key.is_some()` + `config.providers[name].api_key_env` | Yes — reads real config and resolver state | FLOWING |
| `setup.rs` auxiliary stage | `config.auxiliary` | `apply_auxiliary_answer(config, provider, model)` from user input | Yes — pure mutation, no stubs | FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| D-21 PROV-04 unit test | `cargo test -p ironhermes-core --lib legacy_openai_key_does_not_leak` | 1 passed | PASS |
| resolve_role cascade | `cargo test -p ironhermes-core --lib resolve_role` | 6 passed | PASS |
| All 9 provider integration tests | `cargo test -p ironhermes-cli --test provider_integration` | 9/9 passed | PASS |
| Build (4 target crates) | `cargo build -p ironhermes-core -p ironhermes-tools -p ironhermes-agent -p ironhermes-cli` | exit 0 | PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| PROV-04 | 26-01, 26-02 | API keys scoped to provider's base URL — no cross-provider leak | SATISFIED | D-11 leak arm removed; D-21 + D-20 Test 1 pass |
| PROV-06 | 26-02, 26-03, 26-05 | Auxiliary model routing for helper tasks | PARTIAL | Compression route wired end-to-end. Vision/session_search have no LLM consumer call sites (greenfield per Plan 03 audit decision). ROADMAP SC-2 names vision explicitly. |
| PROV-08 | 26-01, 26-02, 26-04 | Named custom providers in config.yaml | PARTIAL | Resolver resolves named providers correctly. --provider flag only activates alongside --model. config.yaml model.provider routing always works. |

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `engine_factory.rs` | 86-98 | Greenfield TODO block for vision/session_search/skills_hub/mcp_helper | Info | Intentional — Plan 03 explicitly documented these as deferred. Not a stub: no code executes through these paths; resolver cascade is in place for when consumers are added. |
| `main.rs` build_client() | 2279-2283 | cli.provider only used when cli.model is Some | Warning | SC-3 says "--provider my-local-llm" should work; without --model the flag is silently ignored. config.yaml model.provider works unconditionally. |

### Human Verification Required

#### 1. SC-2: Vision and Session-Search Auxiliary Routing Scope

**Test:** Review Plan 03 audit decision. Confirm whether Phase 26's PROV-06 deliverable is considered complete with only compression wired to auxiliary, or whether vision tool routing was also required.

**Expected:** Either: (a) Team accepts that vision/session_search auxiliary routing is deferred to the phase that implements the vision tool (the resolver cascade is already in place), OR (b) treat this as a gap and add a plan to wire `build_role_client(resolver, "vision")` when the vision tool exists.

**Why human:** The ROADMAP SC-2 explicitly names "compression, vision, and session-search tasks" but Plan 03 documents that vision has zero consumer call sites in the agent crate today. The resolver infrastructure is fully in place. The team must decide if this satisfies the spirit of PROV-06 for Phase 26.

#### 2. SC-3: `--provider` Flag Without `--model`

**Test:** Run `hermes --provider my-local-llm chat "ping"` (without `--model`) with `my-local-llm` defined in `providers:` in config.yaml. Verify whether the request routes to my-local-llm's base_url or to the default main provider.

**Expected:** The request will go to the main provider's endpoint (not my-local-llm) because `build_client()` only uses `cli.provider` when `cli.model` is also specified. Setting `model.provider: my-local-llm` in config.yaml DOES work unconditionally.

**Why human:** This is a behavioral limitation in the `--provider` flag implementation. Whether this satisfies SC-3 depends on whether the team considers config.yaml-based provider selection sufficient, or whether the `--provider` CLI flag must work standalone.

---

## Evidence Summary

### What Works Correctly (PROV-04 — SC-1)

The PROV-04 key scoping fix is complete and verified at three levels:

1. **Code level:** The `_ => std::env::var("OPENAI_API_KEY").ok()` arm is removed from provider.rs. The new resolution logic uses explicit match arms for `"openai"`, `"anthropic"`, `"openrouter"` and `_ => None` for all other providers.

2. **Unit level:** `legacy_openai_key_does_not_leak_to_unknown_provider` (D-21) passes: with `OPENAI_API_KEY=sk-leaked` set and a custom provider `my-local-llm` with no `api_key_env`, the resolved endpoint has `api_key = None`.

3. **Integration level:** `key_does_not_leak_to_wrong_provider` (D-20 Test 1) passes: wiremock server captures requests; Authorization header does not contain the canary key value.

### What Is Partially Implemented (PROV-06 — SC-2, PROV-08 — SC-3)

**PROV-06:** The resolver cascade (`resolve_role` 3-level: per-task → auxiliary → None) is fully implemented and tested. Compression routing through `build_role_client` is wired in `engine_factory.rs`. The D-20 Test 2 wiremock test proves compression hits the auxiliary endpoint. However, vision and session_search have no LLM consumer call sites — they cannot route to the auxiliary model because there is nothing to route. The resolver WOULD route them if consumers existed.

**PROV-08:** The named custom provider resolver works correctly: any name in `providers:` HashMap resolves its base_url, api_key, and model. The D-20 Test 3 confirms this at the resolver level. The `--provider` CLI flag limitation (requires `--model`) is a UX gap relative to SC-3's literal description.

---

_Verified: 2026-04-30T10:00:00Z_
_Verifier: Claude (gsd-verifier)_
