# Phase 26 Plan 03 — Task 1 Audit: Agent Crate Role Call Site Discovery

**Audited:** 2026-04-30
**Scope:** `crates/ironhermes-agent/src/`
**Method:** `rg -n` grep on production code (test files excluded from classification)

---

## Audit Greps Run

```bash
# Per-role consumer site discovery
rg -n 'vision_complete|VisionTool|vision_model|complete_with_vision' crates/ironhermes-agent/src/
rg -n 'session_search_handle|SessionSearchTool|fn session_search|complete_session_search' crates/ironhermes-agent/src/
rg -n 'skills_hub_query|SkillsHubTool|fn skills_hub|skills_hub_complete' crates/ironhermes-agent/src/
rg -n 'mcp_helper|McpHelper|mcp_helper_query|mcp_helper_complete' crates/ironhermes-agent/src/

# General build_role_client / resolve_role consumers
rg -n 'build_role_client|resolve_role' crates/ironhermes-agent/src/

# Negative confirmation: summarizing_engine has no auxiliary_model
rg -c 'auxiliary_model' crates/ironhermes-agent/src/summarizing_engine.rs
```

---

## Wireup Map

| Role | Status | Call sites | Wireup action |
|------|--------|-----------|---------------|
| compression | wired (Phase 12) | engine_factory.rs:84, engine_factory.rs:112 | regression test only |
| vision | greenfield | zero production hits | TODO comment + regression test |
| session_search | greenfield (no LLM client) | session_search.rs has schema+handler but uses StateStore (DB search, no LLM) — no resolve_role needed | TODO comment |
| skills_hub | greenfield | zero production hits | TODO comment |
| mcp_helper | greenfield | zero production hits | TODO comment |

---

## Detailed Findings

### compression — PRE-WIRED (Phase 12)

- `engine_factory.rs:84`: `let client = match build_role_client(resolver, "compression") {`
- `engine_factory.rs:112`: `.resolve_role("compression")`
- Status: **wired-needed** already satisfied by Phase 12. Plan 03 adds regression test only.

### vision — GREENFIELD

- `rg -n 'vision_complete|VisionTool|vision_model|complete_with_vision'` → **zero hits**
- No consumer call site exists in the agent crate.
- Action: TODO comment at logical injection point in engine_factory.rs

### session_search — GREENFIELD (no LLM routing needed)

- `rg -n 'session_search_handle|SessionSearchTool|fn session_search|complete_session_search'` → **zero hits** on these specific patterns
- `session_search.rs` exists with `session_search_schema()` and `handle_session_search()` — but these are **pure text-search handlers** using `StateStore` (FTS5 SQLite, no LLM invocation).
- The session_search tool intercept in `agent_loop.rs` dispatches to `handle_session_search(&args, &store)` — no LLM client involved.
- The D-05 reserved role `session_search` is for a hypothetical **LLM-assisted** session search path. Today's impl is a simple DB query with no model routing.
- Action: TODO comment noting that if/when an LLM-assisted session search is added, `resolve_role("session_search")` should be wired at that point.

### skills_hub — GREENFIELD

- `rg -n 'skills_hub_query|SkillsHubTool|fn skills_hub|skills_hub_complete'` → **zero hits**
- No consumer call site exists in the agent crate.
- Action: TODO comment at logical injection point in engine_factory.rs

### mcp_helper — GREENFIELD

- `rg -n 'mcp_helper|McpHelper|mcp_helper_query|mcp_helper_complete'` → **zero hits**
- No consumer call site exists in the agent crate.
- Action: TODO comment at logical injection point in engine_factory.rs

---

## Negative Confirmation: summarizing_engine.rs

```
rg -c 'auxiliary_model' crates/ironhermes-agent/src/summarizing_engine.rs
→ 0 matches
```

RESEARCH.md finding confirmed: `summarizing_engine.rs` receives `Arc<dyn SummarizationClient>` from the factory and does **not** read `auxiliary_model` directly. No changes needed to `summarizing_engine.rs`.

---

## Task 2 Action Plan (driven by this audit)

1. Add greenfield TODO comment block to `engine_factory.rs` (after the compression branch) covering vision, session_search, skills_hub, mcp_helper.
2. Add Test 1: `compression_cascade_uses_auxiliary_when_no_per_role_set` — regression for Plan 02 D-05 cascade level 2.
3. Add Test 2: `compression_falls_back_to_main_when_no_aux_no_role` — regression for D-07 caller pattern.
4. Add Test 3: `summarizing_engine_does_not_read_auxiliary_model_directly` — static grep gate.
5. No `summarizing_engine.rs` modifications (audit confirms no call sites there).
6. No new wired-needed role wireups (all four non-compression roles are greenfield).
