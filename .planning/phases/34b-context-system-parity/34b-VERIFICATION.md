---
phase: 34b-context-system-parity
verified: 2026-05-22T14:30:00Z
status: human_needed
score: 15/16 must-haves verified
overrides_applied: 0
human_verification:
  - test: "Trigger @ context warnings render in all 3 surfaces (CLI, gateway, web)"
    expected: "When a user sends a message with @file: referencing a blocked/oversized path, the '--- Context Warnings ---' block is rendered visibly to the user in the CLI REPL, gateway response, and (if accessible) the web UI"
    why_human: "context_warnings is populated and wired onto AgentResult but no production surface reads result.context_warnings to render it separately. Warnings currently reach users only because preprocess_context_references_async embeds a '--- Context Warnings ---' block directly into the message text. The doc comment on context_warnings (agent_loop.rs:71-76) and run_turn claims surfaces render this field — but no surface reads it (grep across cli/main.rs, gateway/handler.rs, iron_hermes_ui/state.rs returns 0). This is WR-01 from the code review, and it needs human eyes to confirm whether in-message delivery is sufficient or whether the field needs out-of-band wiring."
---

# Phase 34b: Context System Parity — Verification Report

**Phase Goal:** Close the parity gap with three hermes-agent context-system modules, wired into the post-28.1 `AgentRuntime::run_turn` chokepoint: (1) `@`-reference expansion (context_refs.rs), (2) ContextEngine lifecycle hook parity (context_engine.rs), (3) ContextCompressor counter reset on /new + memory-authority reminder in compaction header.
**Verified:** 2026-05-22T14:30:00Z
**Status:** human_needed
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `@file:/@folder:/@diff/@staged/@git:N/@url:` parsed and expanded into `--- Attached Context ---` footer; refs stripped from inline message; runs once in `run_turn` | VERIFIED | `parse_context_references` + `preprocess_context_references_async` present and tested (17 tests green); `run_turn` calls it at line 270 before `attach_context_engine` at line 340; invariants_34b `preprocess_before_attach_context_engine_in_run_turn` passes |
| 2 | Expansion runs BEFORE `attach_context_engine`/`agent.run` in `run_turn` — preprocessed once centrally (D-09/D-11) | VERIFIED | Byte offset of `preprocess_context_references_async` (line 52 import + line 270 call site) is before `attach_context_engine(` (line 340) in agent_runtime.rs; invariants_34b assertion passes |
| 3 | Sensitive-path blocklist (.ssh/.aws/.env/etc.) rejects sensitive files with warning; original message preserved when ALL refs blocked | VERIFIED | `is_sensitive_path` covers all 3 const lists (SENSITIVE_HOME_DIRS, SENSITIVE_HOME_FILES, SENSITIVE_HERMES_DIRS); `test_sensitive_path_blocklist_all_entries` covers every entry and passes; WR-02 canonicalization fix present (lines 821-824) |
| 4 | 50% hard limit blocks all expansion; 25% soft limit warns but expands | VERIFIED | `hard_limit = context_length/2`, `soft_limit = context_length/4` at lines 850-851; `test_hard_limit_blocked` (blocked==true, message==original_message) and `test_soft_limit_warning` both pass |
| 5 | Expansion warnings reach all 3 surfaces via `AgentResult.context_warnings`; `--- Context Warnings ---` block renders | UNCERTAIN | `context_warnings: Vec<String>` field is correctly populated in `run_turn` (line 379: `out.context_warnings = context_warnings`) and initialized to `Vec::new()` in all 5 `AgentResult` construction sites. However, no production surface (CLI main.rs, gateway handler.rs, web state.rs) reads `result.context_warnings` — warnings reach users only because `preprocess_context_references_async` embeds the warning block directly into the message text. The doc comments promise out-of-band surface rendering that does not exist. See human verification item. |
| 6 | `@git:N` validated as u32 in [1,10]; git/rg subprocesses argv-only, no shell (CWE-78) | VERIFIED | Range validation at context_refs.rs:752-772; `test_git_n_validation` passes (@git:0 warns, @git:11 warns, @git:3 no range warning); CWE-78 grep returns 0 matches; `.arg(` count = 3 |
| 7 | D-01/D-02: `@url:` uses `WebExtractTool` with `use_llm_processing:true`; falls back with warning on failure; never silently drops | VERIFIED | Production UrlFetcher closure in agent_runtime.rs:235-268 wraps web_extract tool with `use_llm_processing: true`; D-02 fallback path at line 257; `test_expand_url_fetcher_error_surfaces_warning` passes |
| 8 | D-03/D-04: `@file:/@folder:` cannot escape workspace root; `allowed_root` fixed to cwd, no escape hatch | VERIFIED | `resolve_within_root` enforces `allowed_root`; D-04 comment at line 244; `allowed_root: None` in run_turn call (line 275) defaults to cwd (line 815) |
| 9 | 5 lifecycle hooks (on_session_start, on_session_reset, update_from_response, update_model, has_content_to_compress) are additive default-no-ops on ContextEngine trait; existing implementors compile unchanged | VERIFIED | All 5 hooks present in context_engine.rs:80-97; `SummarizingEngine` and `LocalPruningEngine` compile unchanged (cargo tests pass); `test_has_content_to_compress_default_true` passes |
| 10 | `ContextCompressor::on_session_reset` zeroes compression_count and all token counters | VERIFIED | `on_session_reset` override at context_compressor.rs:342-349 zeroes all 5 AtomicUsize fields (compression_count, ineffective_compression_count, last_prompt/completion/total_tokens); `test_context_compressor_reset_zeroes_counter` passes |
| 11 | Compaction history-segment header contains "MEMORY.md" + "ALWAYS authoritative" | VERIFIED | `MEMORY_AUTHORITY_REMINDER` const at summarizing_engine.rs:45; `make_history_message` embeds it at line 71-73; `test_memory_authority_header` passes; `prior_summary_text` strips it on re-compression (lines 396-400) to prevent accretion |
| 12 | `update_from_response` + `update_model` invoked once centrally in `run_turn` (D-07/D-09, no hedge) | VERIFIED | `engine.update_model(...)` at agent_runtime.rs:362-366 (before `agent.run`); `engine.update_from_response(&out.total_usage)` at line 376 (after `agent.run`); invariants_34b tests `update_model_present_in_run_turn` and `update_from_response_after_agent_run_in_run_turn` both pass |
| 13 | Per-session reset wired at surfaces: CLI /new resets compression_count; gateway /new note; web `reset_web_session` stub | VERIFIED | CLI: `compression_count.store(0, Ordering::SeqCst)` at main.rs:1581 in ClearSession arm; invariants_34b `cli_clear_session_resets_compression_count` passes; Gateway: NewSession arm at handler.rs:506 discards session store; Web: `reset_web_session` stub present at state.rs:201-204 (tracing::debug only — accepted Phase-34b scope) |
| 14 | CR-01 fix: reversed `@file:` line range does not panic (slice bounds guard) | VERIFIED | `end_idx.max(start_idx)` at context_refs.rs:531; `test_expand_file_reversed_range_does_not_panic` passes |
| 15 | CR-02 fix: overflow `@file:` line number does not panic (`.ok()` not `.unwrap()`) | VERIFIED | All `parse::<usize>()` calls in unquoted range branch use `.ok()` (lines 395, 398); `test_expand_file_overflow_line_does_not_panic` passes |
| 16 | WR-02 fix: sensitive-path blocklist canonicalizes home/hermes_home before comparison | VERIFIED | `home.canonicalize()` and `hermes_home.canonicalize()` at context_refs.rs:822-824; comparison now like-for-like |

**Score:** 15/16 truths verified (1 UNCERTAIN pending human check)

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/context_refs.rs` | Parser + expander + blocklist + budget + 14+ unit tests | VERIFIED | 1272 lines; 17 tests (6 parser, 1 blocklist, 5 expander, hard/soft limit, git:N validation, 2 CR regression tests); exports `parse_context_references`, `preprocess_context_references_async`, `ContextReference`, `ContextReferenceResult` |
| `crates/ironhermes-agent/src/agent_runtime.rs` | Central @-ref preprocessing in run_turn before attach_context_engine; context_warnings on AgentResult | VERIFIED | `preprocess_context_references_async` called at line 270; `context_warnings` assigned at line 379; both before/after ordering enforced by invariants |
| `crates/ironhermes-agent/src/agent_loop.rs` | `AgentResult.context_warnings: Vec<String>` carrier field | VERIFIED | Field at line 76; initialized in `budget_exhausted` (line 95) and all 4 `Ok(AgentResult{..})` sites (lines 910, 940, 1059, 1193) |
| `crates/ironhermes-agent/src/context_engine.rs` | 5 default-no-op lifecycle hooks on ContextEngine trait | VERIFIED | All 5 hooks at lines 80-97 with default no-op bodies following `check_pressure` idiom |
| `crates/ironhermes-agent/src/context_compressor.rs` | `on_session_reset` override + AtomicUsize counters | VERIFIED | Override at lines 342-349; all counters converted to AtomicUsize; `record_usage` + accessor methods present |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | `MEMORY_AUTHORITY_REMINDER` in compaction header; strip-on-recompression | VERIFIED | Const at line 45; embedded in `make_history_message` at line 71-73; stripped in `prior_summary_text` at lines 396-400 |
| `crates/ironhermes-agent/tests/invariants_34b.rs` | 5 source-guard tests proving centralization + loci + CLI reset | VERIFIED | 5 tests, all pass: `preprocess_before_attach_context_engine_in_run_turn`, `preprocess_not_called_in_surfaces`, `update_model_present_in_run_turn`, `update_from_response_after_agent_run_in_run_turn`, `cli_clear_session_resets_compression_count` |
| `crates/ironhermes-agent/src/lib.rs` | `pub mod context_refs` declaration | VERIFIED | Line 12: `pub mod context_refs;` in the context_* cluster |
| `crates/iron_hermes_ui/src/server/state.rs` | `reset_web_session` stub | VERIFIED | Present at lines 199-204; tracing::debug stub (accepted Phase-34b scope per PLAN 02 interfaces) |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `agent_runtime.rs` | `context_refs.rs` | `preprocess_context_references_async` call in run_turn before `attach_context_engine` | VERIFIED | Byte offset of call site (line ~270) < attach_context_engine (line ~340); invariants_34b test passes |
| `agent_runtime.rs` | `agent_loop.rs` | `context_warnings` populated from `ContextReferenceResult.warnings` | VERIFIED | `out.context_warnings = context_warnings` at line 379; field defined in AgentResult at line 76 |
| `agent_runtime.rs` | `context_engine.rs` | `engine.update_from_response(&usage)` + `engine.update_model(...)` called once in run_turn | VERIFIED | Both calls present; invariants_34b source-guard tests verify position |
| `crates/ironhermes-cli/src/main.rs` | `context_engine.rs` | `compression_count.store(0, ...)` at ClearSession arm | VERIFIED | line 1581; invariants_34b guard passes |
| `crates/ironhermes-gateway/src/handler.rs` | `context_engine.rs` | NewSession arm discards session state; no separate on_session_reset call reachable | VERIFIED | handler.rs:506-531 NewSession arm; per-session state discarded via session_store removal; compression_count reset to 0 at line 309/1039 for new session context |

---

### Data-Flow Trace (Level 4)

Not applicable — this phase produces agent infrastructure (preprocessing, hooks, counters), not UI components rendering dynamic data.

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| context_refs::tests (17 tests) | `cargo test -p ironhermes-agent --lib context_refs::tests` | 17 passed, 0 failed | PASS |
| context_compressor::tests (6 tests, including reset) | `cargo test -p ironhermes-agent --lib context_compressor::tests` | 6 passed, 0 failed | PASS |
| summarizing_engine::test_memory_authority_header | `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header` | 1 passed, 0 failed | PASS |
| invariants_34b (5 source-guard tests) | `cargo test -p ironhermes-agent --test invariants_34b` | 5 passed, 0 failed, 0 ignored | PASS |
| Regression: nudge::tests | `cargo test -p ironhermes-agent --lib nudge::tests` | passed | PASS |
| Regression: memory_context::tests | `cargo test -p ironhermes-agent --lib memory_context::tests` | passed | PASS |
| Regression: streaming_scrubber::tests | `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` | passed | PASS |
| Regression: invariants_33 | `cargo test -p ironhermes-agent --test invariants_33` | 8 passed, 0 failed | PASS |
| CWE-78 no-shell gate | `grep -nE 'sh -c|/bin/sh|Command::new("sh")|Command::new("bash")' context_refs.rs` | 0 matches | PASS |

---

### Probe Execution

No `probe-*.sh` scripts declared for this phase. Step 7c: SKIPPED (no probe scripts).

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| CTX-REF-W0 | Plan 00 | context_refs.rs empty module + lib wiring scaffold | SATISFIED | Module compiles, `pub mod context_refs` in lib.rs |
| CTX-ENG-W0 | Plan 00 | invariants_34b.rs integration-test scaffold + 2 placeholder tests | SATISFIED | invariants_34b.rs exists with real tests (placeholder replaced); compressor/summarizing placeholders un-ignored and passing |
| CTX-REF-01 | Plan 01 | @-ref parser + types + sensitive-path blocklist | SATISFIED | `parse_context_references`, structs, blocklist all present and tested |
| CTX-REF-02 | Plan 01 | Expander + budget + preprocess_async + run_turn wiring + AgentResult.context_warnings | SATISFIED | All present; 17 tests green; invariants_34b passes |
| CTX-ENG-01 | Plan 02 | 5 lifecycle hooks on ContextEngine trait (additive no-ops) | SATISFIED | All 5 hooks on trait; existing engines compile unchanged |
| CTX-ENG-02 | Plan 02 | ContextCompressor::on_session_reset zeroes counters | SATISFIED | Override present; test_context_compressor_reset_zeroes_counter passes |
| CTX-ENG-03 | Plan 02 | Memory-authority reminder in compaction header | SATISFIED | MEMORY_AUTHORITY_REMINDER const + make_history_message embed + test passes |
| CTX-ENG-04 | Plan 02 | Central per-turn hooks (update_from_response + update_model) in run_turn; surface session-reset wiring | SATISFIED | Both hooks in run_turn; CLI/gateway/web reset wiring present; invariants_34b proves loci |

All 8 phase-local requirement IDs are accounted for.

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `context_refs.rs` | 471 | `"toml"` duplicated in `text_exts` array | Info (WR-04) | Dead array entry; no correctness impact; null-byte scan still runs correctly for files not in list |

No TBD/FIXME/XXX markers found in any phase-modified file. No stubs blocking behavior (web `reset_web_session` stub is accepted Phase-34b scope per PLAN-02 interfaces section).

---

### Human Verification Required

### 1. context_warnings Surface Rendering

**Test:** Send a message with `@file:~/.ssh/id_rsa` (blocked path) or `@file:/some/large/file` (over budget) and observe the response in the CLI REPL, a gateway response, and the web UI.

**Expected:** The `--- Context Warnings ---` block appears visibly in the agent's response text, informing the user that expansion was blocked or warned.

**Why human:** `context_warnings` is correctly populated on `AgentResult` and warnings are embedded in the message text by `preprocess_context_references_async` (so users DO see them). However, the `context_warnings` field itself is never read by any production surface — CLI `main.rs`, gateway `handler.rs`, and `iron_hermes_ui/state.rs` all return 0 matches for `context_warnings`. The doc comment at `agent_loop.rs:71-76` and `agent_runtime.rs:369-370` both claim "Surfaces to all three channels (CLI, gateway, web) without per-surface preprocessing" — but there is no code doing that. The actual delivery path is in-message (the preprocessor embeds the warning block in the message text itself). A human needs to confirm either:
(a) In-message delivery is sufficient and the doc comments should be corrected to reflect this, OR
(b) The field needs to be read at the surfaces for out-of-band rendering (e.g. as a header/annotation separate from the agent response text)

This is WR-01 from the code review. It does not block the D-11 contract functionally (warnings reach the user), but the gap between documentation and implementation is a maintenance trap.

---

### Gaps Summary

No blocking gaps found. The only open item is the WR-01 doc/behavior mismatch on `context_warnings` surface rendering, which requires a human decision: the warnings DO reach the user via in-message embedding, but the field that was architecturally designed for out-of-band surface rendering is currently unused at all surfaces.

All security fixes from the code review are confirmed present in the codebase:
- CR-01 (reversed range panic): fixed at line 531 with `.max(start_idx)` guard; regression test passes
- CR-02 (overflow parse panic): fixed at lines 395-398 using `.ok()` throughout; regression test passes
- WR-02 (blocklist canonicalization bypass): fixed at lines 821-824 with `home.canonicalize()` and `hermes_home.canonicalize()`; called before any blocklist comparison

---

_Verified: 2026-05-22T14:30:00Z_
_Verifier: Claude (gsd-verifier)_
