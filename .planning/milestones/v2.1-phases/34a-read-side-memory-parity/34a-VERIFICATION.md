---
phase: 34a-read-side-memory-parity
verified: 2026-05-20T00:00:00Z
status: passed
score: 8/8 must-haves verified
overrides_applied: 0
re_verification:
  previous_status: gaps_found
  previous_score: 5/8
  gaps_closed:
    - "Tags never reach user-visible scrollback and never persist to history — scrubber is panic-safe (CR-01: find_ascii_ci + eq_ignore_ascii_case replaces to_lowercase offset pattern)"
    - "Tags never persist to durable history — sanitize_context applied to accumulated content in both call_llm and call_llm_streaming (CR-02)"
    - "Compressor step 0 evicts recall messages on ALL compression paths before token estimation — retain now runs before compression block (WR-01)"
  gaps_remaining: []
  regressions: []
---

# Phase 34a: Read-Side Memory Parity Verification Report

**Phase Goal:** Close the read-side parity gap with hermes-agent's Python memory_manager. Add the per-turn semantic recall path: pre-turn, the agent queries memory providers for context relevant to the user message, wraps it in a fenced <memory-context> block, and injects it as a synthetic role:system message immediately before the user turn. A StreamingContextScrubber filters the fence tags out of the model's response stream so they NEVER reach user-visible scrollback. D-12 (frozen snapshot) is preserved.
**Verified:** 2026-05-20
**Status:** PASSED
**Re-verification:** Yes — after gap closure (commit b798bbc7); previous score 5/8, current score 8/8

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | prefetch_with_query defaulted trait method exists; file provider unaffected | VERIFIED | memory_provider.rs line 152: `async fn prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String> { Ok(String::new()) }`. MemoryStore impl does not override it. |
| 2 | MemoryManager proxies prefetch_with_query to primary only | VERIFIED | manager.rs lines 200-204: primary-only proxy following exact pattern of prefetch(). No mirror fan-out. |
| 3 | sanitize_context + build_memory_context_block ported; 8 unit tests pass | VERIFIED | memory_context.rs exists, 203 lines. Three OnceLock<Regex> singletons. Correct order: block -> note -> fence. 8 tests in inline cfg(test) mod. |
| 4 | Pre-turn recall injection: retain -> fetch -> insert before last user message | VERIFIED | agent_loop.rs: retain at line 928, pre_chat_compress at 934, prefetch_with_query at 956, rposition+insert at 963-969. Scoped MutexGuard drop before insert (Pitfall 1). D-08 empty-recall skip present. |
| 5 | recall message carries is_recall_context=true; serde(skip); never serializes to wire | VERIFIED | types.rs: `#[serde(skip)] pub is_recall_context: bool`. recall_system() constructor sets is_recall_context: true. All existing constructors default is_recall_context: false. |
| 6 | Tags NEVER reach user-visible scrollback / never persist — scrubber is panic-safe | VERIFIED | CR-01 RESOLVED: streaming_scrubber.rs uses find_ascii_ci (line 148) and max_partial_suffix (line 162) with eq_ignore_ascii_case byte-window search on the ORIGINAL buffer. to_lowercase() is entirely absent from the implementation (only appears in comments). Two new regression tests confirm panic-safety: non_ascii_before_tag_does_not_panic (U+0130 + Cyrillic), non_ascii_inside_split_close_tag (CJK span + split close tag). |
| 7 | Tags NEVER persist to durable history — sanitize_context applied to accumulated content | VERIFIED | CR-02 RESOLVED: call_llm (agent_loop.rs line 1192) applies sanitize_context before returning the ChatMessage. call_llm_streaming (line 1260) applies sanitize_context(&content) before assembling the ChatMessage — after all deltas are accumulated, before the struct is built. Both paths confirmed. |
| 8 | Compressor step 0 evicts recall messages on ALL compression paths before token estimation | VERIFIED | WR-01 RESOLVED: agent_loop.rs line 928 — messages.retain(|m| !m.is_recall_context) — runs BEFORE the compression block at line 933 (context_engine path) and line 935 (legacy compressor path). Comment at line 920-927 documents the D-03 rationale. |

**Score:** 8/8 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-core/src/memory_provider.rs` | prefetch_with_query defaulted trait method | VERIFIED | Line 152: async fn prefetch_with_query, default Ok(String::new()) |
| `crates/ironhermes-agent/src/memory/manager.rs` | MemoryManager::prefetch_with_query primary-only proxy | VERIFIED | Lines 200-204: primary-only, no mirror fan-out |
| `crates/ironhermes-agent/src/memory_context.rs` | sanitize_context + build_memory_context_block + 8 unit tests | VERIFIED | 203 lines, 8 tests, OnceLock<Regex>, correct regex order |
| `crates/ironhermes-agent/src/lib.rs` | pub mod memory_context + pub mod streaming_scrubber declarations | VERIFIED | pub mod memory_context; pub mod streaming_scrubber; |
| `crates/ironhermes-core/src/types.rs` | ChatMessage.is_recall_context #[serde(skip)] + recall_system constructor | VERIFIED | #[serde(skip)] bool. recall_system() sets is_recall_context: true |
| `crates/ironhermes-agent/src/streaming_scrubber.rs` | StreamingContextScrubber (feed/flush/reset) + 8 unit tests (6 original + 2 regression) | VERIFIED | 288 lines. find_ascii_ci + max_partial_suffix use eq_ignore_ascii_case on original bytes. to_lowercase() absent from implementation. 8 tests including non_ascii_before_tag_does_not_panic and non_ascii_inside_split_close_tag. |
| `crates/ironhermes-agent/src/agent_loop.rs` | Pre-turn recall injection block (retain -> fetch -> insert) | VERIFIED | Lines 928-969: retain at 928 (before compress at 933), prefetch at 956, insert at 967-969 |
| `crates/ironhermes-agent/src/context_compressor.rs` | Step 0 retain stripping recall messages | VERIFIED | Legacy compress() retains at line 84. Active path guarded by retain at agent_loop.rs line 928 which now runs BEFORE pre_chat_compress (line 934). Both paths covered. |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| agent_loop.rs | MemoryManager::prefetch_with_query + build_memory_context_block | pre-turn injection block | WIRED | Lines 956, 960: both calls present in scoped guard pattern |
| ironhermes-cli/src/main.rs | StreamingContextScrubber::feed | scrubber wrapped in with_streaming closure | WIRED | Arc<Mutex<>>, feed() and flush() wired |
| ironhermes-gateway/src/handler.rs | StreamingContextScrubber::feed | scrubber wrapped in stream_callback closure | WIRED | Arc<Mutex<>>, feed() and flush() wired |
| iron_hermes_ui/src/server/ws.rs | StreamingContextScrubber::feed | scrubber wrapped in stream_callback closure | WIRED | Arc<Mutex<>>, feed() and flush() wired |
| call_llm_streaming | sanitize_context (accumulated content) | sanitize before ChatMessage construction | WIRED | Line 1260: `let content = crate::memory_context::sanitize_context(&content)` before ChatMessage struct at line 1262 |
| call_llm | sanitize_context (choice.message content) | sanitize on non-streaming path | WIRED | Lines 1191-1198: match on MessageContent::Text, sanitize_context applied, message.content updated |

---

### Anti-Patterns Found

No blockers. All previously flagged blocker patterns have been resolved:

| File | Line | Pattern | Severity | Resolution |
|------|------|---------|----------|------------|
| streaming_scrubber.rs | — | `to_lowercase()` offset desync | RESOLVED | find_ascii_ci + eq_ignore_ascii_case on original bytes; to_lowercase absent |
| agent_loop.rs | — | Raw delta accumulated without sanitize_context | RESOLVED | sanitize_context applied at line 1260 (streaming) and 1192 (non-streaming) |
| agent_loop.rs | — | retain after compression block | RESOLVED | retain at line 928, compression block at line 933 |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| MEM-READ-01 | 34a-01-PLAN.md | prefetch_with_query trait method + MemoryManager proxy | SATISFIED | memory_provider.rs + manager.rs confirmed |
| MEM-READ-02 | 34a-01-PLAN.md | memory_context.rs: sanitize_context + build_memory_context_block | SATISFIED | memory_context.rs confirmed, 8 tests |
| MEM-READ-03 | 34a-02-PLAN.md | Pre-turn synthetic-system-message injection in agent_loop | SATISFIED | agent_loop.rs lines 928-969 confirmed |
| MEM-READ-04 | 34a-02-PLAN.md | StreamingContextScrubber state machine — panic-safe | SATISFIED | CR-01 resolved: find_ascii_ci + eq_ignore_ascii_case; 8 tests including 2 Unicode regression tests |
| MEM-READ-05 | 34a-02-PLAN.md | 3-surface scrubber wiring + tags never persist | SATISFIED | Wiring confirmed on all 3 surfaces; CR-02 resolved: sanitize_context on both call_llm paths |

---

### Behavioral Spot-Checks

Step 7b: SKIPPED — agent library requires embedded runtime context; 319 unit tests (0 failed) per build green confirmation covers the key behaviors.

---

### Human Verification Required

None. All identified failures were mechanically verifiable from source code and have been resolved.

---

### Gaps Summary

No gaps remaining. All three blockers from the initial verification have been resolved:

**CR-01 resolved:** streaming_scrubber.rs now uses `find_ascii_ci` (a pure-ASCII byte-window search with `eq_ignore_ascii_case`) and `max_partial_suffix` (same technique). `to_lowercase()` does not appear anywhere in the implementation — only in doc comments explaining why it was removed. Both helpers return offsets valid in the original buffer. Two new regression tests (`non_ascii_before_tag_does_not_panic`, `non_ascii_inside_split_close_tag`) explicitly exercise the U+0130 and CJK code points that previously triggered the panic.

**CR-02 resolved:** Both LLM call paths now strip fence tags before persisting. `call_llm_streaming` applies `sanitize_context(&content)` at line 1260 on the fully assembled content string before building the `ChatMessage` struct. `call_llm` applies `sanitize_context` at line 1192 by matching on `MessageContent::Text` and replacing `message.content` with the cleaned value. A model-echoed `<memory-context>` block can no longer survive in durable SQLite history.

**WR-01 resolved:** `messages.retain(|m| !m.is_recall_context)` is at line 928. The compression block (both `pre_chat_compress` for the context-engine path and the legacy `ContextCompressor::compress()` path) starts at line 933. The retain unconditionally fires first on every turn, regardless of which compression path is active.

---

_Verified: 2026-05-20_
_Verifier: Claude (gsd-verifier) — re-verification after blocker fixes (commit b798bbc7)_
