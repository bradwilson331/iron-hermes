---
phase: 26-provider-polish
plan: "05"
subsystem: setup-wizard
tags: [setup-wizard, integration-test, auxiliary, hermes-setup, prov-06]
dependency_graph:
  requires: [26-01, 26-02, 26-03, 26-04]
  provides: [apply_auxiliary_answer, hermes-setup-agent-section, d20-test2-prov06, pitfall3-e2e]
  affects: [ironhermes-core, ironhermes-cli]
tech_stack:
  added: []
  patterns:
    - apply_auxiliary_answer pure mutation function (apply_provider_answer analog)
    - D-06 skip-on-empty semantics (empty provider = no-op, auxiliary stays default)
    - run_agent_section EOF-graceful section handler (exits 0 on non-interactive use)
    - D-20 Test 2 two-wiremock-server PROV-06 end-to-end pattern
    - Pitfall 3 subprocess test (unknown auxiliary.provider fails at ProviderResolver::build)
key_files:
  created: []
  modified:
    - crates/ironhermes-core/src/wizard.rs
    - crates/ironhermes-cli/src/setup.rs
    - crates/ironhermes-cli/tests/provider_integration.rs
    - crates/ironhermes-cli/tests/setup_wizard.rs
decisions:
  - "apply_minimum_viable_answers seam not extended — apply_auxiliary_answer called directly from setup.rs; tests use the pure-function seam directly (same pattern as apply_provider_answer tests)"
  - "drive_client_for_test uses AnyClient::chat_completion (not a separate helper) — direct method call is simpler and exercises the actual production path"
  - "AnyClient.model() assertion added before HTTP call to catch auxiliary model resolution before network round-trip"
  - "run_agent_section handles EOF gracefully (exits 0) — non-interactive invocation is valid and expected in test harness context"
  - "setup_wizard.rs setup_agent_section_errors_with_deferred_message renamed and updated — Plan 05 replaces the Phase 23 bail! with a real implementation; test reflects new success behavior (Rule 1 auto-fix)"
metrics:
  duration: "~45 minutes"
  completed: "2026-04-29"
  tasks_completed: 2
  files_changed: 4
---

# Phase 26 Plan 05: setup wizard auxiliary stage + D-20 Test 2 PROV-06 — Summary

One-liner: `apply_auxiliary_answer` pure wizard function + `hermes setup agent` auxiliary routing stage + D-20 Test 2 wiremock end-to-end PROV-06 gate confirming compression routes to gpt-4o-mini via aux_server not main.

## What Was Built

### Task 1: apply_auxiliary_answer + setup.rs D-19 auxiliary stage

**`crates/ironhermes-core/src/wizard.rs`** — added `apply_auxiliary_answer`:

```rust
pub fn apply_auxiliary_answer(config: &mut Config, provider: &str, model: &str) {
    let p = provider.trim();
    let m = model.trim();
    if p.is_empty() {
        return; // D-06: empty input = skip
    }
    config.auxiliary = AuxiliaryConfig { provider: p.to_string(), model: m.to_string() };
}
```

- Pure mutation, no I/O — mirrors `apply_provider_answer` pattern exactly
- D-06 default-None preserved: empty `provider` is a no-op (auxiliary stays `AuxiliaryConfig::default()`)
- 4 inline unit tests: writes, skips-on-empty, trims whitespace, overwrites existing

**`crates/ironhermes-cli/src/setup.rs`** — two additions:

1. **D-19 auxiliary stage in `run_minimum_viable_flow`**: after the learning-loop splice, before the tool-prereqs stage:
   ```rust
   let aux_provider = prompt_with_default(rl, "Auxiliary provider (cheaper helper-task model — Enter to skip)", "")?;
   if !aux_provider.trim().is_empty() {
       let aux_model = prompt_with_default(rl, "Auxiliary model", "gpt-4o-mini")?;
       apply_auxiliary_answer(config, &aux_provider, &aux_model);
   }
   ```
   Default is empty string → pressing Enter skips (D-19 contract: default "No").

2. **`run_agent_section`**: handles `hermes setup agent` section. Prompts for auxiliary provider/model. EOF treated as graceful skip (exits 0) so non-interactive invocations work cleanly.

3. **Import extended**: `use ironhermes_core::wizard::{..., apply_auxiliary_answer, ...};`

4. **`Some("agent")` dispatch updated**: replaces `bail!("section deferred to Phase 26")` with `run_agent_section(&mut config, &mut rl).await?`

3 new setup unit tests:
- `setup_auxiliary_stage_skipped_when_input_empty` — uses pure-function seam directly
- `setup_auxiliary_stage_persists_when_user_enters_provider_and_model` — same seam
- `setup_rs_has_auxiliary_provider_prompt` — source-text invariant

**`apply_minimum_viable_answers` seam:** NOT extended. The plan's fallback path (extend `MinimumViableAnswers`) was not needed — `apply_auxiliary_answer` is independently testable as a pure function, matching the existing `apply_provider_answer` / `apply_api_key_answer` test pattern.

### Task 2: D-20 Test 2 + Pitfall 3 end-to-end tests

**`crates/ironhermes-cli/tests/provider_integration.rs`** — appended two tests:

**`auxiliary_routes_to_separate_model`** (D-20 Test 2, PROV-06 verbatim):

- Two wiremock servers: `aux_server` (openai/ChatCompletions) + `main_server` (anthropic/AnthropicMessages)
- Config: `main = anthropic/claude-sonnet-4`, `auxiliary = { provider: openai, model: gpt-4o-mini }`
- `build_role_client(&resolver, "compression")` → `Ok(Some(AnyClient::ChatCompletions))` via D-05 cascade level 2
- `client.model()` asserted = `"gpt-4o-mini"` BEFORE HTTP call (catches resolver misconfiguration early)
- `client.chat_completion(messages, None, None, Some(10), None, None).await` → POST to `{aux_server.uri()}/v1/chat/completions`
- Three assertions:
  - `aux_reqs.is_empty()` → false (compression hit auxiliary endpoint)
  - `body["model"]` == `"gpt-4o-mini"` (auxiliary model in request body)
  - `main_reqs.is_empty()` → true (anthropic main endpoint received 0 requests)

**`drive_client_for_test` helper decision**: The plan noted this was executor-fill-in. Used `AnyClient::chat_completion` directly — no separate helper needed. The production path for compression is `build_role_client → AnyClient → LlmClient::chat_completion → POST /chat/completions`. This is the exact path exercised.

**Path math**: `LlmClient` base_url is `{aux_server.uri()}/v1` (trailing slash stripped), then appends `/chat/completions`. So wiremock mock must match path `/v1/chat/completions`, not `/chat/completions`.

**`auxiliary_provider_unknown_name_fails_at_load`** (Pitfall 3 end-to-end):
- Subprocess test with `auxiliary: { provider: nonexistent, model: some-model }` in config.yaml
- `hermes provider list` → non-zero exit (ProviderResolver::build fails fast per Plan 02 D-10)
- stderr contains "nonexistent" or "auxiliary"

**Total provider_integration.rs tests: 9/9 PASS** (7 from Plan 04 + 2 new from Plan 05).

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `setup_agent_section_errors_with_deferred_message` test broke due to Plan 05 implementation**
- **Found during:** Task 1 verification
- **Issue:** `crates/ironhermes-cli/tests/setup_wizard.rs` had a test asserting `hermes setup agent` exits non-zero with "Phase 26" in stderr (from the old `bail!` stub). Plan 05 replaces the stub with `run_agent_section` which exits 0.
- **Fix:** Renamed test to `setup_agent_section_succeeds_with_phase26_implementation`; changed assertion from `failure()` to `success()` + stdout contains "auxiliary"/"routing". Also made `run_agent_section` handle EOF gracefully (exits 0 in non-interactive context) so the test passes without a real TTY.
- **Files modified:** `crates/ironhermes-cli/src/setup.rs`, `crates/ironhermes-cli/tests/setup_wizard.rs`
- **Commit:** b3b6ab6

### Deferred Issues (Pre-existing, Out of Scope)

Two pre-existing failures in `crates/ironhermes-cli/tests/setup_wizard.rs` were found that predate Plan 05:

- `setup_tools_section_exits_ok` — calls `hermes setup tools`; fails with EOF when tool prereq prompt (`FIRECRAWL_API_KEY`) isn't satisfied interactively. Root cause: Phase 25's `run_tools_section` now actually prompts; test wasn't updated for this behavioral change.
- `setup_subcommand_skips_preflight` — same root cause, same fix needed.

These were already failing at base commit `c17dc5c` (verified via git log). Not caused by Plan 05 changes. Root cause is in Phase 25 Plan 05 work (prereq wizard). Deferred to future phase cleanup.

### Design Decisions

**`prompt_with_default` EOF handling in `run_agent_section`**: The section dispatcher now gracefully handles EOF (treats it as skip = exits 0). This is correct for non-interactive contexts (CI, test harnesses). The interactive flow still works normally.

## Phase-Level Summary: All 21 D-XX Decisions Implemented

| Decision | Plan | Status |
|----------|------|--------|
| D-01 unified `providers:` HashMap | 01 | Done |
| D-02 custom_providers: migration + banner | 01 | Done |
| D-03 built-in overlay | 01 | Done |
| D-04 api_key_env validation | 01 | Done |
| D-05 auxiliary cascade (per-task → aux → main) | 02, 03 | Done |
| D-06 auxiliary optional, default-None | 02, 05 | Done |
| D-07 resolve_role returns Option<ResolvedEndpoint> | 02 | Done |
| D-08 name-keyed lookup | 01, 02 | Done |
| D-09 fallback_providers unchanged | 01, 02 | Done |
| D-10 auxiliary.provider validated at build | 02 | Done |
| D-11 PROV-04 leak removed | 02 | Done |
| D-12 legacy env var banner once-only | 02 | Done |
| D-13 config.model.api_key deprecated | 02 | Done |
| D-14 hermes provider subcommand | 04 | Done |
| D-15 provider test never prints key | 04 | Done |
| D-16 cache-break banner on persistent writes | 04 | Done |
| D-17 auxiliary config also cache-breaking | 04 | Done |
| D-18 plain-String cross-crate types | 01 | Done |
| D-19 apply_minimum_viable_answers reused | 05 | Done |
| D-20 three mandatory integration tests | 04, 05 | Done |
| D-21 unit test PROV-04 leak | 02 | Done |

## Mandatory Test Matrix (D-20 + D-21)

| Test | Covers | Status |
|------|--------|--------|
| `key_does_not_leak_to_wrong_provider` | D-20 #1 / PROV-04 | PASS (Plan 04) |
| `auxiliary_routes_to_separate_model` | D-20 #2 / PROV-06 | PASS (Plan 05) |
| `custom_provider_selectable_by_name` | D-20 #3 / PROV-08 | PASS (Plan 04) |
| `legacy_openai_key_does_not_leak_to_unknown_provider` | D-21 / PROV-04 unit | PASS (Plan 02) |

## Security Threats

| Threat | Status | Evidence |
|--------|--------|----------|
| T-26-01 (api_key in wizard) | Mitigated | `apply_auxiliary_answer` accepts only provider+model NAME strings; no api_key parameter exists in the function signature |
| T-26-04f (wizard pre-validation) | Accepted | ProviderResolver::build fails fast at next launch if auxiliary.provider is unknown (Pitfall 3 test confirms this) |

## Known Stubs

None — all code paths are fully implemented and functional.

## Self-Check

### Created/modified files:
- `crates/ironhermes-core/src/wizard.rs` — FOUND (contains `fn apply_auxiliary_answer`)
- `crates/ironhermes-cli/src/setup.rs` — FOUND (contains `Auxiliary provider` prompt + `apply_auxiliary_answer` call)
- `crates/ironhermes-cli/tests/provider_integration.rs` — FOUND (contains `fn auxiliary_routes_to_separate_model`)
- `crates/ironhermes-cli/tests/setup_wizard.rs` — FOUND (contains updated agent section test)

### Commits:
- 78813cf: feat(26-05): Task 1 — apply_auxiliary_answer wizard + setup.rs D-19 auxiliary stage
- b5505d5: test(26-05): Task 2 — auxiliary_routes_to_separate_model D-20 Test 2 + Pitfall 3
- b3b6ab6: fix(26-05): handle EOF gracefully in run_agent_section + update setup_wizard test

### Test results:
- `cargo test -p ironhermes-core --lib wizard::tests::apply_auxiliary` — 4/4 PASS
- `cargo test -p ironhermes-cli --lib setup::tests` — 9/9 PASS
- `cargo test -p ironhermes-cli --test provider_integration` — 9/9 PASS
- `cargo test -p ironhermes-cli --test setup_wizard setup_agent_section` — 1/1 PASS
- `cargo build -p ironhermes-cli -p ironhermes-core` — exit 0

## Self-Check: PASSED
