---
phase: 12-provider-resolution
plan: 02
subsystem: api
tags: [anthropic, llm-client, provider-dispatch, format-adapter, streaming]

# Dependency graph
requires:
  - phase: 12-provider-resolution (plan 01)
    provides: ProviderResolver, ResolvedEndpoint, ApiMode enum, Config provider fields
provides:
  - AnthropicClient with native Anthropic Messages API support
  - Format adapter translating between OpenAI and Anthropic message formats
  - AnyClient enum dispatch unifying LlmClient and AnthropicClient
  - build_client/build_main_client/build_role_client factory functions
  - discover_anthropic_credential for Anthropic API key resolution
affects: [12-provider-resolution plan 04 (AgentLoop integration), agent_loop.rs]

# Tech tracking
tech-stack:
  added: []
  patterns: [enum-dispatch for multi-provider clients, format adapter pattern for API translation, credential discovery chain]

key-files:
  created:
    - crates/ironhermes-agent/src/anthropic_client.rs
    - crates/ironhermes-agent/src/any_client.rs
  modified:
    - crates/ironhermes-agent/src/lib.rs

key-decisions:
  - "AnthropicClient matches LlmClient public API shape (chat_completion, chat_completion_stream) for uniform dispatch"
  - "Format adapter functions (adapt_messages, adapt_tools, parse_anthropic_response) are public for testing"
  - "Credential discovery is startup-only with no token refresh (deferred per D-09)"
  - "AnyClient uses enum dispatch rather than trait objects for zero-cost abstraction"
  - "CodexResponses variant errors at construction time with clear message"

patterns-established:
  - "Enum dispatch pattern: AnyClient wraps concrete client types, delegates via match"
  - "Format adapter pattern: translate between API formats at the boundary"
  - "Factory function pattern: build_client/build_main_client/build_role_client as single entry points"

requirements-completed: [PROV-02, PROV-05]

# Metrics
duration: 8min
completed: 2026-04-11
---

# Phase 12 Plan 02: Client Adapters Summary

**AnthropicClient with native Messages API adapter and AnyClient enum dispatch unifying LlmClient and AnthropicClient via ApiMode-based construction**

## Performance

- **Duration:** 8 min
- **Started:** 2026-04-11T19:50:00Z
- **Completed:** 2026-04-11T19:59:26Z
- **Tasks:** 2
- **Files modified:** 3

## Accomplishments
- Created AnthropicClient with full Messages API support including streaming SSE parsing
- Built format adapter translating between OpenAI ChatMessage format and Anthropic format (system extraction, tool_use/tool_result blocks, consecutive message merging)
- Created AnyClient enum dispatch with ChatCompletions and AnthropicMessages variants
- Implemented build_client/build_main_client/build_role_client factory functions using ProviderResolver
- Added credential discovery chain: config api_key > ANTHROPIC_API_KEY env > ~/.claude/credentials.json

## Task Commits

Each task was committed atomically:

1. **Task 1: Create AnthropicClient with Messages API adapter** - `f57f88f` (test: add failing tests for AnthropicClient adapter) + implementation in same file
2. **Task 2: Create AnyClient enum dispatch and build_client factory functions** - `1a2f215` (feat: create AnyClient enum dispatch and build_client factory functions)

_Note: TDD tasks had test and implementation committed together_

## Files Created/Modified
- `crates/ironhermes-agent/src/anthropic_client.rs` - AnthropicClient, format adapter (adapt_messages, adapt_tools, parse_anthropic_response), credential discovery, streaming SSE parser
- `crates/ironhermes-agent/src/any_client.rs` - AnyClient enum with ChatCompletions/AnthropicMessages variants, from_endpoint constructors, build_client/build_main_client/build_role_client factories
- `crates/ironhermes-agent/src/lib.rs` - Added module declarations and re-exports for anthropic_client and any_client

## Decisions Made
- AnthropicClient matches LlmClient's exact public API shape (chat_completion returns ChatResponse, chat_completion_stream returns mpsc::Receiver<StreamEvent>) for uniform dispatch through AnyClient
- Format adapter functions are public (not pub(crate)) to enable thorough unit testing
- Credential discovery is startup-only with no token refresh or expiry check, per D-09 scope constraint
- AnyClient uses enum dispatch rather than trait objects — zero overhead, better ergonomics
- CodexResponses errors at construction time rather than at call time

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered
None

## User Setup Required
None - no external service configuration required.

## Next Phase Readiness
- AnyClient is ready for AgentLoop integration in Plan 04
- build_main_client/build_role_client provide the factory interface Plan 04 needs
- Credential discovery ready for Anthropic provider endpoint construction

---
## Self-Check: PASSED

All files verified present, all commits verified in git log.

*Phase: 12-provider-resolution*
*Completed: 2026-04-11*
