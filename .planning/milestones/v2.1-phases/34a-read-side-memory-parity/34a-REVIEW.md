---
phase: 34a-read-side-memory-parity
reviewed: 2026-05-20T00:00:00Z
depth: standard
files_reviewed: 16
files_reviewed_list:
  - crates/iron_hermes_ui/src/server/ws.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/anthropic_client.rs
  - crates/ironhermes-agent/src/any_client.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/memory_context.rs
  - crates/ironhermes-agent/src/memory/manager.rs
  - crates/ironhermes-agent/src/streaming_scrubber.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-cli/tests/provider_integration.rs
  - crates/ironhermes-core/src/memory_provider.rs
  - crates/ironhermes-core/src/types.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/ironhermes-state/tests/tool_pair_round_trip.rs
findings:
  critical: 2
  warning: 5
  info: 3
  total: 10
status: issues_found
---

# Phase 34a: Code Review Report

**Reviewed:** 2026-05-20
**Depth:** standard
**Files Reviewed:** 16
**Status:** issues_found

## Summary

Phase 34a wires a per-turn semantic-recall path (synthetic `<memory-context>` system message injected before each LLM turn) plus a `StreamingContextScrubber` state machine that strips the recall fence tags from streamed output across CLI, gateway, and web-UI surfaces. The design is sound and the happy-path tests are thorough, but the scrubber's chunk-boundary slicing makes an unsafe assumption that `str::to_lowercase()` preserves byte offsets. It does not for several Unicode code points, which can produce a non-char-boundary slice and **panic the streaming task** (BLOCKER) and, less severely, leak unscrubbed tag fragments. Separately, the scrubber only cleans the *displayed* deltas — the assistant message that is accumulated, returned, and **persisted to SQLite is never sanitized**, so a model-echoed `<memory-context>` block survives in conversation history and re-enters the next turn's context (BLOCKER). There is also an ordering defect in `agent_loop::run()` where the active summarizing-engine compression runs *before* recall eviction, allowing recall content to be folded into the persisted `[CONTEXT HISTORY]` summary (contradicting D-03).

## Critical Issues

### CR-01: Scrubber slices `buf` with byte offsets computed from `buf.to_lowercase()` — non-char-boundary panic on Unicode

**File:** `crates/ironhermes-agent/src/streaming_scrubber.rs:78-92, 97-116, 145-156`
**Issue:** `feed()` computes match/hold offsets against the *lowercased* copy of the buffer, then applies those byte offsets to the *original* buffer:

```rust
let buf_lower = buf.to_lowercase();
match buf_lower.find(CLOSE_TAG) {
    ...
    Some(idx) => {
        buf = buf[idx + CLOSE_TAG.len()..].to_owned(); // idx is a buf_lower offset
    }
}
```
and in `max_partial_suffix`:
```rust
let held = Self::max_partial_suffix(&buf, OPEN_TAG); // measured in buf_lower bytes
let split = buf.len() - held;
out.push_str(&buf[..split]);   // split applied to original buf
self.buf = buf[split..].to_owned();
```

`str::to_lowercase()` is **not** byte-length-preserving for all input. Examples that change byte length when lowercased: U+0130 `İ` (2 bytes) → `i̇` (3 bytes), and other Turkic/Greek/special-casing code points. When streamed LLM output contains such a character before a tag (or before a held partial-tag tail), `idx`/`split` no longer point at a char boundary in `buf`, so `buf[idx + CLOSE_TAG.len()..]` / `buf[..split]` / `buf[split..]` panics with "byte index is not a char boundary". Because the scrubber runs inside the streaming callback (`call_llm_streaming` → `cb(&delta)`), the panic kills the streaming task on CLI/gateway/web. This is reachable from ordinary multilingual model output — no adversary required.

**Fix:** Operate on a single consistent representation. Either (a) do the search/hold against the original `buf` using a case-insensitive matcher that returns *original* byte offsets, or (b) match on `buf_lower` but translate offsets back via a parallel mapping. Simplest correct approach — lowercase only for comparison while iterating original byte indices:
```rust
// find tag case-insensitively, returning an offset valid in `buf`
fn find_ci(haystack: &str, needle_lower: &str) -> Option<usize> {
    let hay_lower = haystack.to_lowercase();
    // Only safe when to_lowercase preserves boundaries; it does NOT in general.
    // Instead, scan original char_indices and compare ASCII-folded windows:
    let nb = needle_lower.as_bytes();
    let hb = haystack.as_bytes();
    haystack.char_indices().find_map(|(i, _)| {
        hb[i..].len() >= nb.len()
            && hb[i..i + nb.len()].eq_ignore_ascii_case(nb)
            && haystack.is_char_boundary(i + nb.len())
    }).map(|_| /* return i */ unimplemented!())
}
```
Since both tags are pure ASCII, the robust fix is to fold only ASCII case and search byte-wise on the original buffer (`eq_ignore_ascii_case` on byte windows), never calling `to_lowercase()` on the buffer at all. That keeps every index valid in `buf`. Apply the same to `max_partial_suffix` (compare ASCII-folded suffix bytes of the original `buf`).

### CR-02: Persisted/returned assistant message is never sanitized — model-echoed `<memory-context>` leaks into history and next-turn context

**File:** `crates/ironhermes-agent/src/agent_loop.rs:1210, 1236-1247` (accumulation); persisted at `crates/ironhermes-cli/src/main.rs:943-946`, gateway/web equivalents
**Issue:** `call_llm_streaming` forwards each delta to the scrubber *only for display* (`cb(&delta)`), but accumulates the **raw** delta into `content` (`content.push_str(&delta)`), then builds the assistant `ChatMessage` from that raw content. The returned `AgentResult.messages`/`appended` therefore still contain any `<memory-context>...</memory-context>` (or `[System note: ...]`) text the model emitted. That message is pushed back into `messages` (line 1088) and persisted to SQLite (cli main.rs:943-946; same on gateway/web). On the next turn it is replayed verbatim to the model. The non-streaming path (`call_llm`, line 1165-1183) is worse: it returns `choice.message` with no scrubbing or `sanitize_context` at all, so even the displayed output leaks when `self.streaming == false`. The phase doc states the fence tags are "internal" and must be stripped "across CLI, gateway, and web UI surfaces" — display-only scrubbing does not meet that contract; the authoritative conversation record still carries the internal markup, defeating the recall-boundary threat model (a persisted forged `[System note: ...]` re-enters as trusted context next turn).

**Fix:** Sanitize the *accumulated* assistant content (not just the displayed stream) before constructing the message in both `call_llm_streaming` and `call_llm`. Reuse `crate::memory_context::sanitize_context` on the final text:
```rust
let cleaned = crate::memory_context::sanitize_context(&content);
let message = ChatMessage {
    role: Role::Assistant,
    content: if cleaned.is_empty() { None } else { Some(MessageContent::Text(cleaned)) },
    ..
};
```
and likewise wrap `choice.message`'s text in `call_llm`. This makes the persisted record and the next-turn replay tag-free regardless of which surface or whether streaming is enabled, and keeps the streaming scrubber purely as a display optimization.

## Warnings

### WR-01: Recall eviction runs AFTER compression — recall content can be summarized into the persisted `[CONTEXT HISTORY]`

**File:** `crates/ironhermes-agent/src/agent_loop.rs:923-933`
**Issue:** In `run()` the order is (1) compress — `pre_chat_compress()` for the active context-engine path, or legacy `compressor.compress()` — then (2) `messages.retain(|m| !m.is_recall_context)`. Only the **legacy** `ContextCompressor::compress` performs the D-03 step-0 recall eviction (`context_compressor.rs:84`). The active `SummarizingEngine`/`pre_chat_compress` path (`agent_loop.rs:924` → `summarizing_engine.rs:241`) has **no** recall eviction. Because the unconditional retain at line 933 fires *after* compression, on turn N the prior turn's recall message is still present when the summarizer runs and can be folded into the pinned `[CONTEXT HISTORY]` summary, which persists. This both contradicts D-03 ("re-derivable next turn, must be freed first") and risks baking recall text permanently into the summary.
**Fix:** Move the `messages.retain(|m| !m.is_recall_context)` eviction to run BEFORE the compression block (before line 923), so neither compression path ever observes a stale recall message. The legacy step-0 eviction in `context_compressor.rs` can stay as defense-in-depth.

### WR-02: `flush()` of an unterminated span permanently wedges the scrubber if reused across turns

**File:** `crates/ironhermes-agent/src/streaming_scrubber.rs:132-140`; reuse risk at `crates/ironhermes-cli/src/main.rs:818, 2312`
**Issue:** `flush()` resets `in_span`/`buf` correctly, so a scrubber that is `flush()`ed between turns is fine. But the documented `reset()` exists precisely so a scrubber can be reused without reallocation, and the CLI/gateway/web sites instead construct a fresh scrubber per turn (good) — except they never call `reset()` and rely on `flush()` having been called. If any future caller reuses a scrubber across turns *without* calling `flush()` (e.g., an error path that returns before the `flush()` at main.rs:887/2416, gateway handler.rs:1076, ws.rs:274), a leftover `in_span = true` silently swallows the entire next turn's output. The `flush()` calls are not in a `Drop`/`finally` position, so an early `?` return between `run()` and the flush skips them.
**Fix:** Either call `scrubber.reset()` at the top of each turn's stream setup, or guarantee `flush()` runs on all exit paths (e.g., a guard struct that flushes on drop). At minimum document that callers MUST `flush()` or `reset()` before reuse and add a debug assertion.

### WR-03: Scrubbed flush tail / deltas dropped silently when the bounded channel is full (gateway)

**File:** `crates/ironhermes-gateway/src/handler.rs:976, 1078`
**Issue:** Both the per-delta visible text (`stream_tx_clone.try_send(visible)`) and the end-of-stream flush tail (`stream_tx.try_send(tail)`) use `try_send` and discard the result. If the bounded `stream_tx` channel is full, the held-back partial-tag tail that turned out to be benign text is silently dropped, truncating the user-visible answer. The web path uses an unbounded channel (ws.rs:209 `mpsc::unbounded_channel`) so it is unaffected; the inconsistency is gateway-specific.
**Fix:** For the flush tail specifically (a single small message at end-of-stream), prefer a blocking/awaited `send` (the stream is already done, so backpressure is acceptable) or log on drop so truncation is observable rather than silent.

### WR-04: `internal_note_re` regex is brittle and order-coupled — a reworded note prefix leaks

**File:** `crates/ironhermes-agent/src/memory_context.rs:29-35, 56-60`
**Issue:** `internal_note_re` matches the system-note line by its *exact* English phrasing with two hard-coded variant tails ("informational background data" | "authoritative reference data..."). If the wrapper text in `build_memory_context_block` (lines 76-83) is ever edited (e.g., wording tweak, localization, added sentence) without updating this regex, an orphaned `[System note: ...]` line — exactly the forged-authority-preamble vector the threat model T-34a-02 calls out — will pass through `sanitize_context` unstripped. The coupling is implicit and undocumented at the regex site.
**Fix:** Derive the note prefix from a single shared constant used by both `build_memory_context_block` and the regex (or match a stable structural marker like the literal leading `[System note: The following is recalled memory context`), and add a test that asserts `sanitize_context(build_memory_context_block(x))` strips the note for every wrapper variant. The existing `double_wrap_idempotency` test only covers the current exact wording.

### WR-05: `insert_idx` fallback to `messages.len()` places recall AFTER the user message when no user message exists

**File:** `crates/ironhermes-agent/src/agent_loop.rs:957-964`
**Issue:** The insert index is `rposition(role == User).unwrap_or(messages.len())`. The guard at line 946 (`!user_msg_text.is_empty()`) is derived from `rev().find(role == User)`, so normally a User message exists and `rposition` succeeds. But the two scans are independent: a User message whose content is non-text (e.g., `MessageContent::Parts` only / image-only) yields a non-empty... actually empty `user_msg_text` (so injection is skipped) — but a User message with empty-string text plus a later non-User message could make `user_msg_text` empty and skip, while a User message present-but-found-only-by-one-scan is possible if roles are filtered differently. The `unwrap_or(messages.len())` path inserts the recall block at the very end (after the user turn) rather than before it, which inverts the intended "inject BEFORE the last user message" semantics and weakens the recall-as-background framing.
**Fix:** Compute the user-message position once and reuse it for both the emptiness check and the insert index, so the "found a usable user message" decision and the insert location can never diverge. If no user message is found, skip injection entirely rather than appending at the tail.

## Info

### IN-01: `flush()` returns the buffer via a redundant temporary

**File:** `crates/ironhermes-agent/src/streaming_scrubber.rs:138-139`
**Issue:** `let tail = std::mem::take(&mut self.buf); tail` introduces a named temporary with no purpose.
**Fix:** `std::mem::take(&mut self.buf)` directly as the tail return expression.

### IN-02: `default_memory_tool_schemas` returns `vec![]` behind a long TODO — dead default

**File:** `crates/ironhermes-core/src/memory_provider.rs:40-49`
**Issue:** The function always returns an empty vec and exists only to host a Plan-20-04 TODO. As written it is effectively dead behavior dressed as a default.
**Fix:** Either inline `vec![]` at the single call site (line 71) or keep but track the TODO in an issue; not blocking.

### IN-03: Repeated `to_lowercase()` allocations on the hot streaming path

**File:** `crates/ironhermes-agent/src/streaming_scrubber.rs:78, 97, 146-147`
**Issue:** `feed()` allocates a fresh lowercased copy of the (possibly large) buffer on every loop iteration, and `max_partial_suffix` allocates two more. Beyond CR-01's correctness problem, switching to ASCII-only byte folding (the CR-01 fix) also removes these allocations. Noted as a maintainability/cleanliness follow-on, not a v1 performance gate.
**Fix:** Folded into the CR-01 ASCII-fold rewrite.

---

_Reviewed: 2026-05-20_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
