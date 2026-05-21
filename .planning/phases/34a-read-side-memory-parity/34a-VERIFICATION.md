---
phase: 34a-read-side-memory-parity
verified: 2026-05-20T00:00:00Z
status: gaps_found
score: 5/8 must-haves verified
overrides_applied: 0
gaps:
  - truth: "Tags never reach user-visible scrollback and never persist to history — scrubber strips fence tags from the model response stream"
    status: failed
    reason: "CR-01: streaming_scrubber.rs computes match/hold byte offsets against buf.to_lowercase() then slices the ORIGINAL buf. to_lowercase() is not byte-length-preserving for non-ASCII Unicode (e.g. U+0130 İ expands from 2 to 3 bytes). When non-ASCII content appears before a tag, the idx returned by buf_lower.find(CLOSE_TAG) / buf_lower.find(OPEN_TAG) is a valid offset in buf_lower but NOT in buf. Slicing buf[idx + CLOSE_TAG.len()..] (line 92), buf[..split] (line 104), buf[split..] (line 105) on misaligned char boundaries panics the streaming task. Also affects max_partial_suffix (lines 147-151) which computes suffix_start from buf_lower.len()."
    artifacts:
      - path: "crates/ironhermes-agent/src/streaming_scrubber.rs"
        issue: "Lines 78-79, 92: buf_lower.find() offset used to slice original buf. Lines 97-98, 104-105, 116: same pattern on open-tag path. Lines 146-151: max_partial_suffix indexes buf_lower byte-length into buf_lower slice — correct within that fn, but callers use returned usize as offset into original buf."
    missing:
      - "Replace to_lowercase() + find() pattern with a case-insensitive search that returns offsets valid in the original buf. Since both OPEN_TAG and CLOSE_TAG are pure ASCII, the correct fix is eq_ignore_ascii_case byte-window search on the original buf bytes — never calling to_lowercase() on buf at all. Apply same fix to max_partial_suffix."

  - truth: "Tags never reach user-visible scrollback and never persist to history — fence tags are stripped from the persisted assistant ChatMessage"
    status: failed
    reason: "CR-02: call_llm_streaming (agent_loop.rs:1197-1246) accumulates raw deltas into `content` via content.push_str(&delta) at line 1210 BEFORE the stream callback cb(&delta) is invoked. The assembled ChatMessage at lines 1236-1247 uses this raw content with no sanitize_context call. The scrubber only runs inside the external stream_callback (CLI/gateway/ws), which is purely display-side. Any <memory-context>...</memory-context> block the model echoes survives in the returned ChatMessage, is pushed into messages (line 1089), and is persisted to SQLite by all three callers. On the next turn it re-enters the model's context verbatim, including any forged [System note:] authority preamble. The non-streaming call_llm path (lines 1165-1183) is worse: it returns choice.message completely unscrubbed with no sanitize_context call at all."
    artifacts:
      - path: "crates/ironhermes-agent/src/agent_loop.rs"
        issue: "call_llm_streaming (line 1210): content.push_str(&delta) accumulates raw delta. ChatMessage assembled at line 1236 uses raw content. No sanitize_context call before building the message."
      - path: "crates/ironhermes-agent/src/agent_loop.rs"
        issue: "call_llm (lines 1165-1183): returns choice.message from the API response with zero scrubbing. Non-streaming path has no sanitize_context at all."
    missing:
      - "In call_llm_streaming: apply crate::memory_context::sanitize_context(&content) before constructing the ChatMessage. Use the cleaned string as the content field."
      - "In call_llm: apply sanitize_context to choice.message's text content before returning. Wrap or post-process the returned ChatMessage's content field."

  - truth: "The context compressor strips recall messages as step 0 before any token estimation — for ALL compression paths"
    status: failed
    reason: "WR-01: agent_loop.rs lines 923-928 show the context engine path (pre_chat_compress / SummarizingEngine) executes BEFORE the unconditional retain at line 933. Only the legacy ContextCompressor::compress() has the step-0 eviction at context_compressor.rs:84. When self.context_engine is Some (the active path), the summarizing engine runs without first evicting recall messages, allowing a prior-turn recall block to be folded into the pinned [CONTEXT HISTORY] summary. This contradicts D-03 and can permanently bake recall text into the compressed history."
    artifacts:
      - path: "crates/ironhermes-agent/src/agent_loop.rs"
        issue: "Lines 923-933: pre_chat_compress() fires at line 924 before messages.retain(|m| !m.is_recall_context) at line 933. The context engine path has no recall eviction of its own."
    missing:
      - "Move messages.retain(|m| !m.is_recall_context) to run BEFORE the compression block (before line 923) so both the legacy compressor and the context engine path never observe a stale recall message."
---

# Phase 34a: Read-Side Memory Parity Verification Report

**Phase Goal:** Close the read-side parity gap with hermes-agent's Python memory_manager. Add the per-turn semantic recall path: pre-turn, the agent queries memory providers for context relevant to the user message, wraps it in a fenced <memory-context> block, and injects it as a synthetic role:system message immediately before the user turn. A StreamingContextScrubber filters the fence tags out of the model's response stream so they NEVER reach user-visible scrollback. D-12 (frozen snapshot) is preserved.
**Verified:** 2026-05-20
**Status:** GAPS FOUND — 3 blockers
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | prefetch_with_query defaulted trait method exists; file provider unaffected | VERIFIED | memory_provider.rs line 152: `async fn prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String> { Ok(String::new()) }`. MemoryStore impl block does not override it. Test `default_hook_methods_return_defaults` asserts returns "". |
| 2 | MemoryManager proxies prefetch_with_query to primary only | VERIFIED | manager.rs lines 200-204: primary-only proxy following exact pattern of prefetch(). No mirror fan-out. |
| 3 | sanitize_context + build_memory_context_block ported; 8 unit tests pass | VERIFIED | memory_context.rs exists, 203 lines. Three OnceLock<Regex> singletons. Correct order: block -> note -> fence. build_memory_context_block calls sanitize_context before wrapping. 8 tests in inline cfg(test) mod. |
| 4 | Pre-turn recall injection: retain -> fetch -> insert before last user message | VERIFIED | agent_loop.rs lines 930-971: retain at 933, prefetch_with_query at 950, rposition+insert at 957-964. Scoped MutexGuard drop before insert (Pitfall 1). D-08 empty-recall skip present. |
| 5 | recall message carries is_recall_context=true; serde(skip); never serializes to wire | VERIFIED | types.rs line 22-23: `#[serde(skip)] pub is_recall_context: bool`. recall_system() constructor at line 344 sets is_recall_context: true. All 5 existing constructors + 13 direct struct initializations updated with is_recall_context: false. |
| 6 | Tags NEVER reach user-visible scrollback / never persist — scrubber is panic-safe | FAILED (BLOCKER) | CR-01 CONFIRMED: streaming_scrubber.rs lines 78-79 and 97-98 call buf.to_lowercase() then use the returned idx to slice the ORIGINAL buf. to_lowercase() expands certain Unicode code points (e.g. U+0130 İ: 2 bytes -> 3 bytes), making idx from buf_lower invalid as an offset into buf. Slicing buf[idx + CLOSE_TAG.len()..] at line 92 panics on a non-char-boundary when any such character precedes a tag. max_partial_suffix (lines 146-151) has the same flaw. The streaming task panics — tags can leak AND the stream is killed. |
| 7 | Tags NEVER persist to durable history — sanitize_context applied to accumulated content | FAILED (BLOCKER) | CR-02 CONFIRMED: call_llm_streaming (lines 1197-1247) does content.push_str(&delta) at line 1210 accumulating RAW delta. The ChatMessage at line 1236 is built from this raw content with no sanitize_context call. call_llm (lines 1165-1183) returns choice.message completely unscrubbed. Both paths leave <memory-context> blocks in the persisted assistant message, which re-enters the model context verbatim next turn. |
| 8 | Compressor step 0 evicts recall messages on ALL compression paths before token estimation | FAILED (BLOCKER) | WR-01 CONFIRMED: agent_loop.rs lines 923-933 show pre_chat_compress() (SummarizingEngine path) fires at line 924 BEFORE messages.retain at line 933. Only the legacy ContextCompressor::compress() has the step-0 retain. On the active summarizing engine path, prior-turn recall can be folded into the pinned [CONTEXT HISTORY] summary. |

**Score:** 5/8 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-core/src/memory_provider.rs` | prefetch_with_query defaulted trait method | VERIFIED | Line 152: async fn prefetch_with_query, default Ok(String::new()) |
| `crates/ironhermes-agent/src/memory/manager.rs` | MemoryManager::prefetch_with_query primary-only proxy | VERIFIED | Lines 200-204: primary-only, no mirror fan-out |
| `crates/ironhermes-agent/src/memory_context.rs` | sanitize_context + build_memory_context_block + 8 unit tests | VERIFIED | 203 lines, 8 tests, OnceLock<Regex>, correct regex order |
| `crates/ironhermes-agent/src/lib.rs` | pub mod memory_context + pub mod streaming_scrubber declarations | VERIFIED | Line 13: pub mod memory_context; Line 21: pub mod streaming_scrubber; |
| `crates/ironhermes-core/src/types.rs` | ChatMessage.is_recall_context #[serde(skip)] + recall_system constructor | VERIFIED | Line 22-23: #[serde(skip)] bool. Line 344: recall_system() sets is_recall_context: true |
| `crates/ironhermes-agent/src/streaming_scrubber.rs` | StreamingContextScrubber (feed/flush/reset) + 6 unit tests | STUB/DEFECTIVE | File exists, 244 lines, 6 tests present, but feed() uses to_lowercase() offsets against original buf — CR-01 panic-on-Unicode. Not safe for production. |
| `crates/ironhermes-agent/src/agent_loop.rs` | Pre-turn recall injection block (retain -> fetch -> insert) | VERIFIED | Lines 930-971: correct injection pattern present |
| `crates/ironhermes-agent/src/context_compressor.rs` | Step 0 retain stripping recall messages | PARTIAL | Line 84: retain present in legacy compress(). Missing from pre_chat_compress/SummarizingEngine path — WR-01. |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| agent_loop.rs | MemoryManager::prefetch_with_query + build_memory_context_block | pre-turn injection block | WIRED | Lines 950, 954: both calls present in scoped guard pattern |
| ironhermes-cli/src/main.rs | StreamingContextScrubber::feed | scrubber wrapped in with_streaming closure | WIRED | Lines 818-831: Arc<Mutex<>>, feed() at 828, flush() at 887 |
| ironhermes-gateway/src/handler.rs | StreamingContextScrubber::feed | scrubber wrapped in stream_callback closure | WIRED | Lines 968-976: Arc<Mutex<>>, feed() at 974, flush() at 1076 |
| iron_hermes_ui/src/server/ws.rs | StreamingContextScrubber::feed | scrubber wrapped in stream_callback closure | WIRED | Lines 216-223: Arc<Mutex<>>, feed() at 223, flush() at 274 |
| call_llm_streaming | sanitize_context (accumulated content) | sanitize before ChatMessage construction | NOT WIRED | content.push_str(&delta) at line 1210; no sanitize_context call before ChatMessage assembly at line 1236. CR-02. |
| call_llm | sanitize_context (choice.message content) | sanitize on non-streaming path | NOT WIRED | Returns choice.message directly at line 1182 with no scrubbing. CR-02. |

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| streaming_scrubber.rs | 78-79, 92 | `buf.to_lowercase()` offset used to slice original `buf` | BLOCKER | Non-ASCII input before a tag produces non-char-boundary slice, panicking the stream task |
| streaming_scrubber.rs | 97-98, 104-105, 116 | Same to_lowercase offset pattern on open-tag path | BLOCKER | Same panic vector on the out-of-span branch |
| streaming_scrubber.rs | 146-151 | `max_partial_suffix` computes `suffix_start` from `buf_lower.len()` then slices `buf_lower` — this is self-consistent BUT the caller uses the returned `usize` (measured in buf_lower bytes) as an offset into the original `buf` at lines 84-85, 103-105 | BLOCKER | When to_lowercase expands a char, buf_lower.len() > buf.len(), making split = buf.len() - held potentially underflow or misalign |
| agent_loop.rs | 1210, 1236-1246 | Raw delta accumulated into content without sanitize_context | BLOCKER | Model-echoed <memory-context> blocks survive in persisted assistant ChatMessage; replay next turn as trusted context |
| agent_loop.rs | 1165-1183 | call_llm returns choice.message with no scrubbing | BLOCKER | Non-streaming path provides zero tag-leak protection |

---

### Requirements Coverage

The MEM-READ-01 through MEM-READ-05 requirement IDs are phase-local identifiers defined in the PLAN frontmatter. They do not appear in .planning/REQUIREMENTS.md — this file does not track Phase 34a scope (which is a sub-phase under the v2.1 roadmap). This is consistent with the existing REQUIREMENTS.md structure where sub-phase IDs are plan-internal only.

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| MEM-READ-01 | 34a-01-PLAN.md | prefetch_with_query trait method + MemoryManager proxy | SATISFIED | memory_provider.rs + manager.rs confirmed |
| MEM-READ-02 | 34a-01-PLAN.md | memory_context.rs: sanitize_context + build_memory_context_block | SATISFIED | memory_context.rs confirmed, 8 tests |
| MEM-READ-03 | 34a-02-PLAN.md | Pre-turn synthetic-system-message injection in agent_loop | SATISFIED | agent_loop.rs lines 930-971 confirmed |
| MEM-READ-04 | 34a-02-PLAN.md | StreamingContextScrubber state machine | BLOCKED | CR-01: to_lowercase offset desync causes panic on Unicode |
| MEM-READ-05 | 34a-02-PLAN.md | 3-surface scrubber wiring: CLI / gateway / web UI | PARTIAL | Wiring exists on all 3 surfaces, but CR-02 means the persisted record is unsanitized — the "tags never persist" guarantee MEM-READ-05 is meant to enforce is not met |

---

### Gaps Summary

Three blockers prevent the phase goal from being achieved.

**Root cause of CR-01:** The `StreamingContextScrubber` uses `str::to_lowercase()` to produce a case-folded copy of the working buffer, then uses byte offsets from that copy to slice the original buffer. For pure-ASCII content this works coincidentally. For any model output containing Unicode characters whose lowercase representation occupies more bytes than the original (U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE and similar special-casing code points), the offset from `buf_lower.find()` is larger than the corresponding offset in `buf`. Slicing `buf[idx..]` at that position hits a non-char-boundary and panics the async streaming task. Since the tags `<memory-context>` and `</memory-context>` are pure ASCII, the correct fix is to avoid `to_lowercase()` entirely on the haystack and use `eq_ignore_ascii_case` on byte windows of the original buffer.

**Root cause of CR-02:** The scrubber is wired only to the *display* callback (`stream_callback`), not to the content accumulator. The display-only scrubber serves visual correctness but the `AgentResult.messages`/`appended` payload (and by extension SQLite persistence) receives the raw model output. A model that echoes its injected `<memory-context>` block (which real LLMs do when asked "what context do you have?") permanently bakes the recall markup into durable history. The fix is a single `sanitize_context` call on the accumulated content string before assembling the returned `ChatMessage`, applied in both `call_llm_streaming` and `call_llm`.

**Root cause of WR-01:** The `messages.retain(|m| !m.is_recall_context)` line was placed after the compression block, not before it. This makes the step-0 eviction in `ContextCompressor::compress()` redundant (it runs inside the legacy compressor which already ran), and leaves the summarizing engine path without any recall eviction before it fires. Moving the retain to before line 923 fixes both paths with one change.

**Relationship between CR-01 and the phase goal:** The phase goal explicitly states the scrubber "filters the fence tags out of the model's response stream so they NEVER reach user-visible scrollback." A scrubber that panics on multilingual model output does not satisfy this guarantee.

**Relationship between CR-02 and the phase goal:** The phase goal's threat model (T-34a-05) states recall messages are "evicted pre-turn and never enter durable history." An unsanitized assistant ChatMessage containing `<memory-context>` blocks that is persisted to SQLite and replayed next turn directly violates this contract.

---

### Human Verification Required

None. All identified failures are mechanically verifiable from the source code.

---

_Verified: 2026-05-20_
_Verifier: Claude (gsd-verifier)_
