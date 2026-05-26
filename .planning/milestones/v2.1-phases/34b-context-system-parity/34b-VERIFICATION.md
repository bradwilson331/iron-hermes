---
phase: 34b-context-system-parity
verified: 2026-05-22T16:00:00Z
status: human_needed
score: 16/16 must-haves verified
overrides_applied: 0
re_verification:
  previous_status: human_needed
  previous_score: 15/16
  gaps_closed:
    - "WR-01: context_warnings surface rendering — all three surfaces (CLI run_single + run_chat_turn, gateway run_agent, web run_web_turn) now read result.context_warnings and render the --- Context Warnings --- block out-of-band; in-message embedding removed from preprocess_context_references_async"
  gaps_remaining: []
  regressions: []
human_verification:
  - test: "Trigger @-reference context warnings and confirm out-of-band rendering in the CLI REPL"
    expected: "Send a message containing @file:~/.ssh/id_rsa (blocked sensitive path). The --- Context Warnings --- block should appear visibly as a distinct output after the agent response in the CLI REPL scroll region, NOT embedded inside the model's response text."
    why_human: "The surface wiring (run_single + run_chat_turn) is code-verified, but the actual visual separation from model response text can only be confirmed with a live CLI session. write_into_scroll_region placement is the right call, but TUI scroll-region behavior needs a human eye."
  - test: "Confirm gateway context_warnings rendered as a distinct message (not appended to streamed response)"
    expected: "When @file:~/.ssh/id_rsa is sent via the gateway (Telegram or other adapter), the warnings block arrives as a separate adapter.send_message call — a visually distinct message from the agent's response."
    why_human: "The adapter.send_message call for warnings is code-verified in handler.rs, but confirmation requires a live gateway session."
  - test: "Confirm web run_web_turn streams context_warnings via stream_callback after response"
    expected: "In the web UI, after a message with a blocked @-reference, the --- Context Warnings --- block appears via the stream_callback after the main response ends, not embedded in the response."
    why_human: "Arc<StreamCallback> wrapping and post-turn callback invocation are code-verified in state.rs, but the web client rendering of that streamed annotation needs human confirmation. The web client has no dedicated annotation channel; the warnings arrive as a streamed text block."
---

# Phase 34b: Context System Parity — Verification Report

**Phase Goal:** Close the parity gap with three hermes-agent context-system modules, wired into the post-28.1 `AgentRuntime::run_turn` chokepoint. (1) `@`-reference expansion: tokens parsed, expanded into a bounded `--- Attached Context ---` footer, stripped from inline message, preprocessed ONCE centrally in run_turn with sensitive-path blocklist and 50% hard / 25% soft token budget; expansion warnings ride back on `AgentResult.context_warnings` so all three surfaces (CLI, gateway, web) render the `--- Context Warnings ---` block out-of-band. (2) ContextEngine 5 additive default-no-op lifecycle hooks fired centrally per-turn in run_turn; per-session reset at surfaces. (3) ContextCompressor counter reset on /new + memory-authority reminder in compaction header.

**Verified:** 2026-05-22T16:00:00Z
**Status:** human_needed
**Re-verification:** Yes — after WR-01 gap closure (Plan 34b-03 complete; all 4 plans now executed)

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | `@file:/@folder:/@diff/@staged/@git:N/@url:` parsed and expanded into `--- Attached Context ---` footer; refs stripped from inline message; runs once in `run_turn` | VERIFIED | `parse_context_references` + `preprocess_context_references_async` present in context_refs.rs (1328 lines); 18 tests green; `run_turn` calls preprocess before `attach_context_engine`; invariants_34b `preprocess_before_attach_context_engine_in_run_turn` passes |
| 2 | Expansion runs BEFORE `attach_context_engine`/`agent.run` in `run_turn` — preprocessed once centrally (D-09/D-11) | VERIFIED | Byte offset of `preprocess_context_references_async` call site is before `attach_context_engine(` in agent_runtime.rs; invariants_34b assertion passes; centralization grep: `main.rs handler.rs state.rs` all return 0 for `preprocess_context_references_async` |
| 3 | Sensitive-path blocklist (.ssh/.aws/.env/etc.) rejects sensitive files with warning; original message preserved when ALL refs blocked | VERIFIED | `is_sensitive_path` covers all 3 const lists; `test_sensitive_path_blocklist_all_entries` covers every entry; blocklist canonicalization fix (`home.canonicalize()` + `hermes_home.canonicalize()`) present |
| 4 | 50% hard limit blocks all expansion; 25% soft limit warns but expands | VERIFIED | `hard_limit = context_length/2`, `soft_limit = context_length/4`; `test_hard_limit_blocked` (`blocked==true`, `message==original_message`) and `test_soft_limit_warning` both pass; 18 tests total |
| 5 | Expansion warnings reach all 3 surfaces via `AgentResult.context_warnings`; `--- Context Warnings ---` block renders out-of-band at each surface | VERIFIED | `context_warnings` field populated in run_turn; in-message embedding REMOVED from `preprocess_context_references_async` (grep for `final_msg.push_str("\n\n--- Context Warnings ---"` returns 0); CLI run_single (line 863), CLI run_chat_turn (line 2345), gateway run_agent (line 1133), web run_web_turn (line 264) each guard `context_warnings.is_empty()` and render the block out-of-band; `surfaces_consume_context_warnings` + `warnings_not_embedded_in_message_text` invariant tests pass |
| 6 | `@git:N` validated as u32 in [1,10]; git/rg subprocesses argv-only, no shell (CWE-78) | VERIFIED | Range validation present; `test_git_n_validation` passes; CWE-78 no-shell grep returns 0 matches; `.arg(` count = 3 (argv-style confirmed) |
| 7 | D-01/D-02: `@url:` uses `WebExtractTool` with `use_llm_processing:true`; falls back with warning on failure; never silently drops | VERIFIED | Production UrlFetcher closure wraps web_extract tool with `use_llm_processing: true`; D-02 fallback path present; `test_expand_url_fetcher_error_surfaces_warning` passes |
| 8 | D-03/D-04: `@file:/@folder:` cannot escape workspace root; `allowed_root` fixed to cwd, no escape hatch | VERIFIED | `resolve_within_root` enforces `allowed_root`; D-04 comment present; `allowed_root: None` in run_turn defaults to cwd |
| 9 | 5 lifecycle hooks (on_session_start, on_session_reset, update_from_response, update_model, has_content_to_compress) are additive default-no-ops on ContextEngine trait; existing implementors compile unchanged | VERIFIED | All 5 hooks present in context_engine.rs with default no-op bodies following `check_pressure` idiom; `SummarizingEngine` and `LocalPruningEngine` compile unchanged; `test_has_content_to_compress_default_true` passes; 35 summarizing_engine tests green |
| 10 | `ContextCompressor::on_session_reset` zeroes compression_count and all token counters | VERIFIED | `on_session_reset` override in context_compressor.rs zeroes all 5 AtomicUsize fields; `test_context_compressor_reset_zeroes_counter` passes (was un-ignored from Wave-0 placeholder); 6 compressor tests green |
| 11 | Compaction history-segment header contains "MEMORY.md" + "ALWAYS authoritative" | VERIFIED | `MEMORY_AUTHORITY_REMINDER` const present; `make_history_message` embeds it; `grep -c 'ALWAYS authoritative' summarizing_engine.rs` = 4; `test_memory_authority_header` passes (was un-ignored); strip-on-recompression confirmed by `iterative_summary` test |
| 12 | `update_from_response` + `update_model` invoked once centrally in `run_turn` (D-07/D-09, no hedge) | VERIFIED | `engine.update_model(...)` in run_turn before `agent.run`; `engine.update_from_response(&out.total_usage)` after `agent.run`; `grep -c update_from_response agent_runtime.rs` = 1; `grep -c update_model agent_runtime.rs` = 2; invariants_34b `update_model_present_in_run_turn` + `update_from_response_after_agent_run_in_run_turn` both pass |
| 13 | Per-session reset wired at surfaces: CLI /new resets compression_count; gateway /new note; web `reset_web_session` stub | VERIFIED | CLI: `compression_count.store(0, Ordering::SeqCst)` at ClearSession arm; invariants_34b `cli_clear_session_resets_compression_count` passes; Gateway: NewSession arm discards session store; Web: `reset_web_session` stub present with tracing::debug (accepted Phase-34b scope per PLAN-02 interfaces) |
| 14 | WR-01 closed: in-message warnings embedding removed; `--- Attached Context ---` still embedded; warnings exclusively on `AgentResult.context_warnings` | VERIFIED | `grep` for `final_msg.push_str("\n\n--- Context Warnings ---"` in context_refs.rs returns 0; `grep -c 'Attached Context' context_refs.rs` = 7 (production push_str retained); `warnings_not_embedded_in_message_text` invariant test passes |
| 15 | Doc comments corrected: agent_loop.rs context_warnings field + agent_runtime.rs run_turn no longer promise behavior that did not exist | VERIFIED | `grep -c 'without per-surface preprocessing' agent_loop.rs` = 0; doc comments updated to describe actual out-of-band surface-rendering path |
| 16 | Source-guard test in invariants_34b proves each surface references `context_warnings`; context_refs unit test proves warnings not in `result.message` while on `result.warnings` | VERIFIED | `surfaces_consume_context_warnings` test passes (MAIN_SOURCE + HANDLER_SOURCE + STATE_SOURCE each contain "context_warnings"); `warnings_not_embedded_in_message_text` test passes; `test_warnings_not_in_message_text_but_on_warnings_vec` unit test passes; invariants_34b: 7 tests, all green |

**Score:** 16/16 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/context_refs.rs` | Parser + expander + blocklist + budget + 18 unit tests; in-message warnings embedding removed | VERIFIED | 1328 lines; 18 tests green (18 passed, 0 failed, 0 ignored); `parse_context_references`, `preprocess_context_references_async`, `ContextReference`, `ContextReferenceResult` all exported; `final_msg.push_str("\n\n--- Context Warnings ---"` absent from production code |
| `crates/ironhermes-agent/src/agent_runtime.rs` | Central @-ref preprocessing in run_turn before attach_context_engine; context_warnings populated on AgentResult | VERIFIED | `preprocess_context_references_async` count = 3; `context_warnings` count = 3; both before/after ordering enforced by invariants |
| `crates/ironhermes-agent/src/agent_loop.rs` | `AgentResult.context_warnings: Vec<String>` carrier field; doc comment corrected | VERIFIED | Field present; `context_warnings` count = 7; stale "without per-surface preprocessing" phrase absent |
| `crates/ironhermes-agent/src/context_engine.rs` | 5 default-no-op lifecycle hooks on ContextEngine trait | VERIFIED | `grep -c 'fn on_session_reset' context_engine.rs` = 1; all 5 hooks present |
| `crates/ironhermes-agent/src/context_compressor.rs` | `on_session_reset` override + AtomicUsize counters | VERIFIED | `grep -c 'fn on_session_reset' context_compressor.rs` = 1; counters converted to AtomicUsize; reset test passes |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | `MEMORY_AUTHORITY_REMINDER` in compaction header; strip-on-recompression | VERIFIED | "ALWAYS authoritative" count = 4; `test_memory_authority_header` passes; 35 summarizing_engine tests green |
| `crates/ironhermes-cli/src/main.rs` | Out-of-band context_warnings rendering at run_single + run_chat_turn | VERIFIED | `context_warnings` count = 6 (>= 2 required); `context_warnings.is_empty()` guards at both sites; "Context Warnings" header present at lines 869 and 2351 |
| `crates/ironhermes-gateway/src/handler.rs` | Out-of-band context_warnings rendering in run_agent | VERIFIED | `context_warnings` count = 3 (>= 1 required); `context_warnings.is_empty()` guard present; "Context Warnings" header at line 1140 |
| `crates/iron_hermes_ui/src/server/state.rs` | run_web_turn surfaces context_warnings out-of-band via stream_callback; reset_web_session stub | VERIFIED | `context_warnings` count = 6 (>= 1 required); `context_warnings.is_empty()` guard present; "Context Warnings" header at line 271; `reset_web_session` stub present |
| `crates/ironhermes-agent/tests/invariants_34b.rs` | 7 source-guard tests proving centralization + loci + CLI reset + WR-01 closure | VERIFIED | 177 lines; 7 tests, all pass: `preprocess_before_attach_context_engine_in_run_turn`, `preprocess_not_called_in_surfaces`, `update_model_present_in_run_turn`, `update_from_response_after_agent_run_in_run_turn`, `cli_clear_session_resets_compression_count`, `surfaces_consume_context_warnings`, `warnings_not_embedded_in_message_text` |
| `crates/ironhermes-agent/src/lib.rs` | `pub mod context_refs` declaration | VERIFIED | `grep -c 'pub mod context_refs' lib.rs` = 1 |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `agent_runtime.rs` | `context_refs.rs` | `preprocess_context_references_async` call in run_turn before `attach_context_engine` | VERIFIED | Byte offset ordering confirmed; invariants_34b `preprocess_before_attach_context_engine_in_run_turn` passes |
| `agent_runtime.rs` | `agent_loop.rs` | `context_warnings` populated from `ContextReferenceResult.warnings` | VERIFIED | `out.context_warnings = context_warnings` in run_turn; field defined in AgentResult |
| `agent_runtime.rs` | `context_engine.rs` | `engine.update_from_response(&usage)` + `engine.update_model(...)` called once in run_turn | VERIFIED | Both calls present; invariants_34b source-guard tests verify position (before/after `agent.run`) |
| `crates/ironhermes-cli/src/main.rs` | `agent_loop.rs` | `result.context_warnings` read after `runtime.run_turn` returns; rendered out-of-band at run_single + run_chat_turn | VERIFIED | Lines 863 + 2345; `context_warnings.is_empty()` guards present; `surfaces_consume_context_warnings` passes |
| `crates/ironhermes-gateway/src/handler.rs` | `agent_loop.rs` | `result.context_warnings` read in run_agent Ok arm; sent via `adapter.send_message` out-of-band | VERIFIED | Line 1133; `context_warnings.is_empty()` guard present; `surfaces_consume_context_warnings` passes |
| `crates/iron_hermes_ui/src/server/state.rs` | `agent_loop.rs` | `result.context_warnings` read in run_web_turn before returning; emitted via `stream_callback` | VERIFIED | Line 264; `context_warnings.is_empty()` guard present; `surfaces_consume_context_warnings` passes |
| `crates/ironhermes-cli/src/main.rs` | `context_engine.rs` | `compression_count.store(0, Ordering::SeqCst)` at ClearSession (/new) arm | VERIFIED | invariants_34b `cli_clear_session_resets_compression_count` passes |

---

### Data-Flow Trace (Level 4)

Not applicable — this phase produces agent infrastructure (preprocessing, hooks, counters), not UI components rendering dynamic data from a database.

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| context_refs::tests (18 tests including WR-01 unit test) | `cargo test -p ironhermes-agent --lib context_refs::tests` | 18 passed, 0 failed, 0 ignored | PASS |
| context_compressor::tests (6 tests, including reset) | `cargo test -p ironhermes-agent --lib context_compressor` | 6 passed, 0 failed, 0 ignored | PASS |
| summarizing_engine::tests (35 tests including memory-authority) | `cargo test -p ironhermes-agent --lib summarizing_engine` | 35 passed, 0 failed, 0 ignored | PASS |
| invariants_34b (7 source-guard tests) | `cargo test -p ironhermes-agent --test invariants_34b` | 7 passed, 0 failed, 0 ignored | PASS |
| CWE-78 no-shell gate | `grep -nE 'sh -c\|/bin/sh' context_refs.rs \| wc -l` | 0 | PASS |
| Centralization gate | `grep -c preprocess_context_references_async main.rs handler.rs state.rs` | 0 0 0 | PASS |
| WR-01 in-message embedding removed | `grep -c 'final_msg.push_str.*Context Warnings' context_refs.rs` | 0 | PASS |
| Attached-context still embedded | `grep -c 'Attached Context' context_refs.rs` | 7 | PASS |
| surface context_warnings counts | cli=6, gateway=3, web=6 | all >= required minimums | PASS |

---

### Probe Execution

No `probe-*.sh` scripts declared for this phase. Step 7c: SKIPPED (no probe scripts).

---

### Requirements Coverage

Phase-local requirement IDs (defined during /gsd:discuss-phase 34b; not in global REQUIREMENTS.md — this is expected and documented in the phase prompt):

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|-------------|-------------|--------|----------|
| CTX-REF-W0 | Plan 00 | context_refs.rs empty module + lib wiring scaffold; invariants_34b integration-test scaffold | SATISFIED | Module compiles; `pub mod context_refs` in lib.rs; invariants_34b.rs exists with 7 real tests (all placeholders replaced) |
| CTX-ENG-W0 | Plan 00 | Wave-0 `#[ignore]` placeholder tests for reset + memory-authority header | SATISFIED | Both placeholders un-ignored and asserting real behavior (`test_context_compressor_reset_zeroes_counter` + `test_memory_authority_header`) |
| CTX-REF-01 | Plan 01 | @-ref parser + types + sensitive-path blocklist | SATISFIED | `parse_context_references`, structs, blocklist all present and tested (18 tests green) |
| CTX-REF-02 | Plans 01 + 03 | Expander + budget + preprocess_async + run_turn wiring + AgentResult.context_warnings carrier; out-of-band warnings rendering at all 3 surfaces | SATISFIED | All present; 18 tests green; invariants_34b passes; all 3 surfaces wired; WR-01 closed |
| CTX-REF-03 | Plan 03 | WR-01 closure: in-message warnings embedding removed; out-of-band rendering at CLI/gateway/web; source-guard tests | SATISFIED | In-message embed removed (grep=0); 3 surfaces render out-of-band (grep counts confirmed); `surfaces_consume_context_warnings` + `warnings_not_embedded_in_message_text` tests pass |
| CTX-ENG-01 | Plan 02 | 5 lifecycle hooks on ContextEngine trait (additive no-ops) | SATISFIED | All 5 hooks on trait; existing engines compile unchanged; workspace build green |
| CTX-ENG-02 | Plan 02 | ContextCompressor::on_session_reset zeroes counters | SATISFIED | Override present; `test_context_compressor_reset_zeroes_counter` passes |
| CTX-ENG-03 | Plan 02 | Memory-authority reminder in compaction header | SATISFIED | `MEMORY_AUTHORITY_REMINDER` const + `make_history_message` embed; "ALWAYS authoritative" count = 4; `test_memory_authority_header` passes |
| CTX-ENG-04 | Plan 02 | Central per-turn hooks (update_from_response + update_model) in run_turn; surface session-reset wiring | SATISFIED | Both hooks in run_turn; CLI/gateway/web reset wiring present; invariants_34b proves loci |

**All 9 phase-local requirement IDs accounted for.** (CTX-REF-03 was declared in Plan 03 frontmatter but not listed in the original phase prompt — included here because the plan explicitly claims it.)

Note: These requirement IDs (CTX-REF-W0, CTX-ENG-W0, CTX-REF-01, CTX-REF-02, CTX-REF-03, CTX-ENG-01..04) are phase-local, defined during /gsd:discuss-phase 34b. They do not appear in the global REQUIREMENTS.md, which is expected — this phase closed parity gaps identified in the discussion phase rather than satisfying v2.0/v2.1 roadmap requirements by ID.

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `context_refs.rs` | ~471 | `"toml"` duplicated in `text_exts` array | Info | Dead array entry; no correctness impact; null-byte scan still runs correctly for files not in list |

No TBD/FIXME/XXX markers found in any phase-modified file (grep across all 10 modified files returned 0). No functional stubs — web `reset_web_session` stub is an accepted Phase-34b scope boundary per PLAN-02 interfaces section (no web new-chat trigger exists yet; stub documents the future wiring locus).

---

### Human Verification Required

### 1. CLI context_warnings Out-of-Band Visual Separation

**Test:** Open a CLI REPL session (`ironhermes` or equivalent). Send a message containing `@file:~/.ssh/id_rsa` (sensitive path, will be blocked). Observe whether the `--- Context Warnings ---` block appears:
- As a distinct, visually separate output from the model's response text, OR
- Embedded inside the model's response text

**Expected:** The warnings block appears AFTER the scrubber tail flush, written to the scroll region via `write_into_scroll_region`, visually separate from the agent's response. The model response itself should NOT contain "--- Context Warnings ---".

**Why human:** The `write_into_scroll_region` call at run_chat_turn:2351 is the correct out-of-band channel, but TUI scroll-region rendering behavior under real terminal conditions (colors, line breaks, position relative to model output) requires a live session to confirm visual separation.

---

### 2. Gateway context_warnings as a Distinct Message

**Test:** Send a message with `@file:~/.ssh/id_rsa` through the gateway (Telegram or other adapter). Observe whether the warnings appear as:
- A separate message from the agent's response, OR
- Appended to / part of the agent's response stream

**Expected:** A DISTINCT follow-up message containing "--- Context Warnings ---" arrives after (or alongside) the main response. The main response should not contain the warnings block.

**Why human:** The `adapter.send_message` call for warnings at handler.rs:1133 is code-verified to be separate from the streamed response, but the gateway platform adapter's actual message delivery (timing, ordering relative to streamed chunks) requires a live session.

---

### 3. Web UI context_warnings via stream_callback

**Test:** Using the web UI, send a message with a blocked `@file:~/.ssh/id_rsa` reference. Observe whether the `--- Context Warnings ---` block:
- Appears after the model response as a distinct streamed annotation
- Is absent from the model response body

**Expected:** After the main response completes, a streamed `--- Context Warnings ---` block appears (delivered via the `Arc<StreamCallback>` clone invoked at state.rs:264). The model response body itself should not contain the warnings.

**Why human:** The `Arc<StreamCallback>` wrapping and post-turn invocation are code-verified, but the web client's rendering of streamed post-response content (whether it appears inline, in a sidebar, or is otherwise distinguished) requires a live web UI session to confirm.

---

### Gaps Summary

No blocking gaps. All 16 must-have truths are VERIFIED against the codebase.

The three human verification items above are surface-rendering confirmations (visual separation, message ordering) that cannot be verified by grep/static analysis. They do not block the phase goal's technical achievement — the wiring is present and proven by invariants. They are end-user experience confirmations of the out-of-band rendering contract.

**All code review security fixes confirmed present:**
- CR-01 (reversed `@file:` line range panic): `end_idx.max(start_idx)` guard; regression test passes
- CR-02 (overflow `@file:` line number panic): `.ok()` used throughout; regression test passes
- WR-02 (blocklist canonicalization bypass): `home.canonicalize()` + `hermes_home.canonicalize()` before comparison

**WR-01 (context_warnings out-of-band rendering) — CLOSED by Plan 34b-03:**
- In-message embedding removed from `preprocess_context_references_async`
- CLI run_single + run_chat_turn, gateway run_agent, web run_web_turn each read and render `result.context_warnings` out-of-band
- Doc comments corrected in agent_loop.rs + agent_runtime.rs
- `surfaces_consume_context_warnings` + `warnings_not_embedded_in_message_text` source-guard tests prove the contract

---

_Verified: 2026-05-22T16:00:00Z_
_Verifier: Claude (gsd-verifier)_
_Re-verification: Yes — Plan 34b-03 (WR-01 gap closure) complete; all 4 plans (00, 01, 02, 03) now verified_
