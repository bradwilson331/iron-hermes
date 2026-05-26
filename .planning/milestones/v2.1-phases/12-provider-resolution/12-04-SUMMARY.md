---
phase: 12-provider-resolution
plan: "04"
subsystem: integration
tags: [provider-resolver, any-client, migration, budget, fallback, cli, gateway, cron, batch]

dependency_graph:
  requires:
    - phase: 12-01
      provides: ProviderResolver, ResolvedEndpoint, ApiMode
    - phase: 12-02
      provides: AnyClient, build_main_client, build_client factory functions
  provides:
    - Single resolution path via ProviderResolver for all client construction
    - Budget counter wired at CLI and gateway entry points
    - Fallback client wired for main agent (CLI + gateway), not cron
    - Old resolve_base_url/resolve_api_key deleted from Config
  affects: [ironhermes-cli, ironhermes-agent, ironhermes-gateway, ironhermes-core]

tech_stack:
  added: []
  patterns:
    - "ProviderResolver::build(&config) at startup, then build_main_client(&resolver) at call sites"
    - "AnyClient as the universal client type for AgentLoop (replaces LlmClient)"
    - "Arc<AtomicUsize> shared budget passed through AgentLoop -> SubagentRunner -> child agents"
    - "One-shot fallback via take() with retry loop (MAX_RETRIES=3, exponential backoff)"

key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/src/any_client.rs
    - crates/ironhermes-agent/src/subagent_runner.rs
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/src/batch/runner.rs
    - crates/ironhermes-gateway/src/handler.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-core/src/config.rs

key-decisions:
  - "AgentLoop.client changed from LlmClient to AnyClient -- the key integration change"
  - "with_fallback accepts AnyClient instead of LlmClient (supports cross-API-mode fallback)"
  - "SubagentRunner rewritten to accept ProviderResolver instead of raw base_url/api_key strings"
  - "run() changed to &mut self to allow in-place client swap on fallback activation"
  - "Plan 03 budget/fallback code integrated directly (cherry-pick from divergent branch)"
  - "Cron runs use fixed provider with NO fallback per D-12"

patterns-established:
  - "ProviderResolver::build at startup: all entry points (CLI, gateway, batch, cron) construct resolver once"
  - "build_main_client(&resolver) for primary client, build_client(&resolver, provider, model) for overrides"
  - "Budget counter: Arc::new(AtomicUsize::new(0)) at entry, passed via with_budget() and SubagentRunner"
  - "Fallback wiring: resolver.resolve_for_main().fallback_providers.first() -> build_client -> with_fallback"

requirements-completed: [PROV-01, PROV-03, PROV-07]

metrics:
  duration_minutes: 35
  completed: 2026-04-11
---

# Phase 12 Plan 04: Call-Site Migration Summary

**Migrated all 8+ call sites from config.resolve_base_url/api_key + LlmClient::new to ProviderResolver + build_main_client, with budget and fallback wired at entry points**

## Performance

- **Duration:** 35 min
- **Started:** 2026-04-11T20:02:23Z
- **Completed:** 2026-04-11T20:37:00Z
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments
- Single resolution path via ProviderResolver for all client construction across CLI, gateway, cron, and batch
- AgentLoop.client changed from LlmClient to AnyClient (the key integration change connecting Plans 01-03)
- Budget counter (Arc<AtomicUsize>) wired at CLI and gateway entry points, shared with child agents
- One-shot fallback with retry loop (MAX_RETRIES=3) for 429/5xx/401 errors
- Old Config::resolve_base_url() and Config::resolve_api_key() deleted -- zero remaining callers
- SubagentRunner simplified: accepts ProviderResolver instead of 5 raw string parameters

## Task Commits

Each task was committed atomically:

1. **Task 1: Migrate CLI and batch call sites to ProviderResolver** - `0eb9e19` (feat)
2. **Task 2: Migrate gateway call sites and remove old resolution methods** - `74d0557` (feat)

## Files Created/Modified
- `crates/ironhermes-agent/src/agent_loop.rs` - Changed client type to AnyClient, added budget/fallback/retry logic, &mut self on run()
- `crates/ironhermes-agent/src/any_client.rs` - Added model() method for PromptBuilder compatibility
- `crates/ironhermes-agent/src/subagent_runner.rs` - Rewritten to accept AnyClient + ProviderResolver (replaces raw URLs)
- `crates/ironhermes-cli/src/main.rs` - All call sites migrated to ProviderResolver, budget wired, fallback wired
- `crates/ironhermes-cli/src/batch/runner.rs` - Worker clients constructed via build_main_client(&resolver)
- `crates/ironhermes-gateway/src/handler.rs` - Added resolver field, migrated run_agent(), wired fallback
- `crates/ironhermes-gateway/src/runner.rs` - Added resolver field to GatewayRunner, migrated execute_cron_job()
- `crates/ironhermes-core/src/config.rs` - Deleted resolve_base_url() and resolve_api_key()

## Decisions Made
- AgentLoop.client changed from LlmClient to AnyClient (not a trait object -- zero-cost enum dispatch)
- with_fallback() accepts AnyClient instead of LlmClient to support cross-API-mode fallback
- SubagentRunner completely rewritten to accept ProviderResolver instead of raw URL/key strings
- Plan 03's budget/fallback code was integrated directly since cherry-pick from divergent branch had planning file conflicts
- Cron runs intentionally have NO fallback per D-12 design decision

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Plan 03 changes missing from branch**
- **Found during:** Task 1 setup
- **Issue:** Plan 03 (budget + fallback) was completed on a different branch. Commit e9a3d9f existed but cherry-pick failed due to divergent planning files.
- **Fix:** Manually integrated Plan 03's budget/fallback/retry changes into agent_loop.rs alongside the AnyClient migration, producing a combined result.
- **Files modified:** agent_loop.rs, subagent_runner.rs
- **Verification:** All 11 budget/fallback tests pass, cargo build --workspace exits 0

---

**Total deviations:** 1 auto-fixed (1 blocking)
**Impact on plan:** Integration of Plan 03 changes was necessary for Plan 04 to function. The combined result is cleaner than separate commits would have been.

## Issues Encountered
- Pre-existing test failure in `ironhermes-tools::delegate_task::tests::test_delegate_task_schema_has_required_task` (not related to our changes, no files modified in ironhermes-tools)

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- Provider resolution is now fully integrated across the entire codebase
- All client construction goes through ProviderResolver -- single source of truth
- Budget and fallback are wired and ready for production use
- Phase 12 is complete -- all 4 plans executed

---
*Phase: 12-provider-resolution*
*Completed: 2026-04-11*
