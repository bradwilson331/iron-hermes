---
phase: 26
plan: 03
subsystem: ironhermes-agent/engine_factory
tags: [agent-wireup, role-routing, auxiliary, fallback, prov-06, d-05, d-07]
requires:
  - .planning/phases/26-provider-polish/26-02-SUMMARY.md (resolve_role D-05 cascade implemented)
  - .planning/phases/26-provider-polish/26-CONTEXT.md (locked decisions D-01..D-21)
provides:
  - Audit findings: all four non-compression roles (vision, session_search, skills_hub, mcp_helper) are greenfield in the agent crate
  - Greenfield TODO comment block in engine_factory.rs documenting the 4 pending roles with future-phase wiring instructions
  - Regression test: compression_cascade_uses_auxiliary_when_no_per_role_set — Plan 02 D-05 cascade level 2 reaches agent layer intact
  - Regression test: compression_falls_back_to_main_when_no_aux_no_role — D-07 Ok(None) arm behavior locked
  - Regression test: summarizing_engine_does_not_read_auxiliary_model_directly — static grep gate (comment-stripped)
  - Compression cascade end-to-end verified; Plan 05 integration test 2 has the wireup it needs
affects:
  - crates/ironhermes-agent/src/engine_factory.rs
tech-stack:
  added: []
  patterns:
    - include_str! static grep gate (comment-stripped) for regression defense (Test 3)
    - Phase 26 D-05/D-07 greenfield TODO block pattern for roles without current call sites
key-files:
  created:
    - .planning/phases/26-provider-polish/26-03-audit.md (Task 1 audit artifact)
  modified:
    - crates/ironhermes-agent/src/engine_factory.rs (greenfield TODO block + 3 regression tests)
decisions:
  - 26-03: All four non-compression roles are greenfield — no production call sites found in crates/ironhermes-agent/src/; session_search.rs uses StateStore (FTS5, no LLM) not a model client; greenfield TODO comment documents future wiring point
  - 26-03: summarizing_engine.rs left untouched — audit confirmed zero auxiliary_model references; receives Arc<dyn SummarizationClient> from factory only
  - 26-03: Test 3 uses include_str! + comment-strip filter per Grep Gate Hygiene — avoids false positives from historical comment text
  - 26-03: Plan 04 (hermes provider CLI) does not depend on Plan 03 agent-side API — resolver introspection from Plan 02 is sufficient
metrics:
  duration_minutes: 12
  completed_date: 2026-04-30
  tasks: 2
  files_modified: 1
  files_created: 1
  tests_added: 3
---

# Phase 26 Plan 03: Agent Crate Wireup Summary

Audited `crates/ironhermes-agent/src/` for all five D-05 reserved role call
sites; confirmed four roles are greenfield (no LLM consumer today); added
greenfield TODO comment block at the logical injection point in
`engine_factory.rs`; added three regression tests locking Plan 02's D-05
cascade end-to-end through the agent layer.

## Plan Objective

Wire the agent crate to consume Plan 02's D-05 resolve_role cascade. Per
researcher Open Question 1 + Resolution #1, non-compression roles may be
greenfield. Task 1 was a discovery audit; Task 2 acted on the findings.

## Task 1 Audit Table

| Role | Status | Call sites | Wireup action |
|------|--------|-----------|---------------|
| compression | wired (Phase 12) | engine_factory.rs:84, engine_factory.rs:112 | regression test only |
| vision | greenfield | zero production hits | TODO comment block |
| session_search | greenfield (no LLM client) | session_search.rs uses StateStore FTS5 — no model routing | TODO comment block |
| skills_hub | greenfield | zero production hits | TODO comment block |
| mcp_helper | greenfield | zero production hits | TODO comment block |

### Audit Grep Results

```
rg -n 'vision_complete|VisionTool|vision_model|complete_with_vision' crates/ironhermes-agent/src/
→ zero hits

rg -n 'session_search_handle|SessionSearchTool|fn session_search|complete_session_search' crates/ironhermes-agent/src/
→ zero hits on these patterns
  (session_search.rs has session_search_schema() + handle_session_search() but uses
   StateStore for FTS5 DB search — no LLM client, no resolve_role needed)

rg -n 'skills_hub_query|SkillsHubTool|fn skills_hub|skills_hub_complete' crates/ironhermes-agent/src/
→ zero hits

rg -n 'mcp_helper|McpHelper|mcp_helper_query|mcp_helper_complete' crates/ironhermes-agent/src/
→ zero hits

rg -c 'auxiliary_model' crates/ironhermes-agent/src/summarizing_engine.rs
→ 0 matches (RESEARCH.md negative confirmation holds)
```

### session_search classification note

`crates/ironhermes-agent/src/session_search.rs` exists and exports
`session_search_schema()` + `handle_session_search()`. However, the handler
is a **pure text-search function** using `StateStore` (FTS5 SQLite queries) —
it takes no LLM client parameter and makes no LLM call. The D-05 reserved
role `session_search` is for a hypothetical future LLM-assisted session search
path. Today's implementation needs no `resolve_role("session_search")` wiring.

## Tasks Completed

| # | Task | Commit | Files |
|---|------|--------|-------|
| 1 | Audit agent crate for role call sites — produce wireup map | 0c3b5c8 | .planning/phases/26-provider-polish/26-03-audit.md |
| 2 | Apply audit findings — greenfield TODO block + 3 regression tests | 6639ca4 | crates/ironhermes-agent/src/engine_factory.rs |

## Task 2 Findings

### Greenfield TODO Comment Block (engine_factory.rs)

Added immediately above the `"summarizing"` match arm's compression block:

```rust
// === Phase 26 D-05/D-07: auxiliary roles greenfield in this phase ===
// The following roles are RESERVED in config but have no consumer call sites
// in the agent crate as of Phase 26. When the downstream phase that ships
// each tool wires its consumer, drop in the same three-branch
// build_role_client(resolver, "<role>") pattern used for "compression" below.
//
// - "vision":         awaiting vision-complete tool (TBD phase)
// - "session_search": no LLM routing needed today (pure StateStore FTS5 query
//                     in session_search.rs); wire here if/when an LLM-assisted
//                     session rewrite adds a model call (Phase 17 / 21.5 follow-up)
// - "skills_hub":     awaiting skills hub auxiliary query path (TBD phase)
// - "mcp_helper":     awaiting MCP helper auto-routing (TBD phase)
//
// Resolver-side cascade (Plan 02 D-05) is already in place — only the
// consumer call sites are pending. resolve_role() will return Some(endpoint)
// for any role with auxiliary or per-role config; None falls through to main.
// === end Phase 26 ===
```

### Regression Tests Added (3 new)

**Test 1 — `compression_cascade_uses_auxiliary_when_no_per_role_set`**
- Sets `auxiliary.provider = "openai"`, no per-role compression in `model.roles`
- Calls `build_role_client(&resolver, "compression")` — asserts `Ok(Some(client))`
- Calls `resolver.resolve_role("compression")` — asserts `ep.base_url` contains `api.openai.com`
- Proves Plan 02's D-05 cascade level 2 propagates through the agent layer
- Result: PASS

**Test 2 — `compression_falls_back_to_main_when_no_aux_no_role`**
- No auxiliary, no per-role compression
- Asserts `build_role_client("compression")` returns `Ok(None)` (cascade level 3)
- Builds a `"summarizing"` engine — asserts `CompressionMode::Soft` (factory's `Ok(None)` arm uses main client, engine is still SummarizingEngine)
- Locks the D-07 `Ok(None)` branch behavior end-to-end
- Result: PASS

**Test 3 — `summarizing_engine_does_not_read_auxiliary_model_directly`**
- `include_str!("summarizing_engine.rs")` + comment-strip filter
- Asserts no non-comment occurrence of `auxiliary_model` in the file
- Permanent regression gate for RESEARCH.md finding
- Result: PASS

### All Tests Pass

```
running 7 tests
test engine_factory::tests::summarizing_engine_does_not_read_auxiliary_model_directly ... ok
test engine_factory::tests::factory_unknown_engine_falls_back ... ok
test engine_factory::tests::compression_cascade_uses_auxiliary_when_no_per_role_set ... ok
test engine_factory::tests::factory_returns_summarizing_for_summarizing_string ... ok
test engine_factory::tests::compression_falls_back_to_main_when_no_aux_no_role ... ok
test engine_factory::tests::factory_aux_model_fallback ... ok
test engine_factory::tests::factory_returns_local_prune_for_local_prune_string ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 224 filtered out
```

## Plan-Level Confirmations

- **Plan 04 independence confirmed**: `hermes provider list/show/test/enable/disable` only needs
  resolver introspection from Plan 02's `ProviderResolver`. No agent-side API changes required.
  Plan 04 can proceed without waiting for Plan 03 agent-side output.

- **Plan 05 integration test 2 has the wireup it needs**: The compression cascade end-to-end is
  verified — `build_role_client(resolver, "compression")` reaches Plan 02's D-05 cascade and
  returns the auxiliary endpoint when `auxiliary.provider` is set. Plan 05's
  `auxiliary_routes_to_separate_model` test can trigger a compression task and assert it hits the
  auxiliary provider endpoint.

## Verification

```
$ cargo test -p ironhermes-agent --lib engine_factory::tests::
test result: ok. 7 passed; 0 failed; 0 ignored

$ cargo build -p ironhermes-agent -p ironhermes-core -p ironhermes-cli
    Finished `dev` profile in 12.07s

$ git diff a8c9a4f..HEAD -- crates/ironhermes-core/
(no output — critical constraint met)

$ grep -q "Phase 26 D-05/D-07: auxiliary roles greenfield in this phase" \
    crates/ironhermes-agent/src/engine_factory.rs
(exits 0 — TODO marker present)

$ git diff crates/ironhermes-agent/src/summarizing_engine.rs
(no output — summarizing_engine.rs untouched)
```

## Deviations from Plan

### Auto-fixed Issues

None — plan executed exactly as written.

### Notes on TDD Phase

Task 2 is marked `tdd="true"`. The three regression tests protect pre-existing
correct behavior (compression already wired, summarizing_engine already clean).
As regression gates, they pass immediately after being written — this is expected:
the RED phase "failure" is conceptual (the absence of these tests meant the
property was unguarded). The tests now permanently lock the behavior.

## Known Stubs

None — all changes are either documentation (TODO block) or tests. No data
flows through stub paths. The greenfield TODO comments are explicit markers, not
stub call sites that would execute.

## Threat Flags

None — Plan 03 introduces no new trust boundaries. The regression tests consume
Plan 02's already-mitigated resolver. Tracing messages in the existing compression
fallback path reference role names only (no api_key values).

## Self-Check: PASSED
