---
phase: 34a-read-side-memory-parity
plan: 02
type: execute
wave: 2
depends_on: ["34a-01"]
files_modified:
  - crates/ironhermes-core/src/types.rs
  - crates/ironhermes-agent/src/agent_loop.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/streaming_scrubber.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/ws.rs
autonomous: true
requirements: [MEM-READ-03, MEM-READ-04, MEM-READ-05]

must_haves:
  truths:
    - "Before each LLM call, the agent loop evicts any prior recall message, fetches query-scoped recall, and (only if non-empty) injects a fresh role:System recall message immediately before the latest user message"
    - "When recall is empty, no new <memory-context> message is inserted (build_memory_context_block returns None). Prior-turn recall is still evicted by the unconditional retain at turn start — a no-op on a never-injected session, so a file-provider-only session's buffer stays byte-identical to pre-34a (D-08, amended 2026-05-20: skip insert, always evict; gating the retain would cause a stale-recall bug)"
    - "The recall message carries is_recall_context=true (a #[serde(skip)] flag), so it never serializes to the wire and is detected purely by flag, never by content parsing (D-01)"
    - "The context compressor strips recall messages as step 0 before any token estimation — recall is lowest-priority and re-derivable next turn (D-03)"
    - "A StreamingContextScrubber removes <memory-context>...</memory-context> spans from streaming deltas across chunk boundaries; flush discards an unterminated open span rather than leaking it"
    - "All three streaming surfaces (CLI run_chat, gateway handle_with_multimodal, web UI ws.rs) scrub every delta and flush the tail at stream end"
    - "The frozen system-prompt snapshot (D-12) is untouched — prompt_builder.rs is not modified"
  artifacts:
    - path: "crates/ironhermes-core/src/types.rs"
      provides: "ChatMessage.is_recall_context field (#[serde(skip)]) + ChatMessage::recall_system constructor"
      contains: "is_recall_context"
    - path: "crates/ironhermes-agent/src/streaming_scrubber.rs"
      provides: "StreamingContextScrubber (feed/flush/reset) + 6 unit tests"
      min_lines: 80
    - path: "crates/ironhermes-agent/src/agent_loop.rs"
      provides: "pre-turn recall injection block (retain -> fetch -> insert)"
      contains: "recall_system"
    - path: "crates/ironhermes-agent/src/context_compressor.rs"
      provides: "step 0 retain stripping recall messages"
      contains: "is_recall_context"
  key_links:
    - from: "crates/ironhermes-agent/src/agent_loop.rs"
      to: "MemoryManager::prefetch_with_query + memory_context::build_memory_context_block"
      via: "pre-turn injection block after compression, before turns_used += 1"
      pattern: "prefetch_with_query"
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "StreamingContextScrubber::feed"
      via: "scrubber wrapped in with_streaming closure"
      pattern: "scrubber.*feed|\\.feed\\("
    - from: "crates/ironhermes-gateway/src/handler.rs"
      to: "StreamingContextScrubber::feed"
      via: "scrubber wrapped in stream_callback closure"
      pattern: "\\.feed\\("
    - from: "crates/iron_hermes_ui/src/server/ws.rs"
      to: "StreamingContextScrubber::feed"
      via: "scrubber wrapped in stream_callback closure"
      pattern: "\\.feed\\("
---

<objective>
Wire the per-turn semantic recall path end to end (MEM-READ-03/04/05). Add the
`is_recall_context` flag + `recall_system` constructor to ChatMessage, the
pre-turn injection block in the agent loop, the compressor step-0 eviction, the
new StreamingContextScrubber module, and the scrubber into all three streaming
surfaces.

Purpose: This is the user-visible payoff — a mid-session memory write surfaces in
a later "what do you remember?" answer, and the model's internal recall fence tags
never leak into the visible scrollback.

Output: ChatMessage schema change + agent_loop injection + compressor step 0 +
streaming_scrubber.rs (6 tests) + 3-surface wiring. D-12 frozen snapshot preserved.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/STATE.md
@.planning/phases/34a-read-side-memory-parity/34A-CONTEXT.md
@.planning/phases/34a-read-side-memory-parity/34A-RESEARCH.md
@.planning/phases/34a-read-side-memory-parity/34A-PATTERNS.md
@.planning/phases/34a-read-side-memory-parity/34a-01-SUMMARY.md

<interfaces>
<!-- Contracts the executor needs. Extracted from codebase + RESEARCH/PATTERNS. -->

From 34a-01 (must exist before this plan runs):
  - MemoryManager::prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String>
  - memory_context::build_memory_context_block(raw: &str) -> Option<String>  (None if empty/whitespace)
  - memory_context::sanitize_context(text: &str) -> String

ChatMessage (crates/ironhermes-core/src/types.rs lines 8-19) — current 5 fields:
  role: Role, content: Option<MessageContent>, tool_calls, tool_call_id, name
  - all serde(skip_serializing_if = Option::is_none); NO Default impl
  - constructors system/user/assistant/assistant_tool_calls/tool_result each list all 5 fields explicitly
  - ChatMessage::system (lines 325-333) is the template for recall_system
  - content_text() helper exists for extracting text from a message

agent_loop.rs injection point: AFTER line 928 (end of compression block), BEFORE
line 930 (turns_used += 1). Existing post-turn queue_prefetch block (lines 1053-1075)
is the lock-acquire/rev-find analog. Existing pressure-advisory push (lines 941-952)
is the message-insert analog (uses push; recall uses insert at index).

context_compressor.rs compress() opens at line 80 with `if !self.should_compress(...)`.
Step 0 retain prepends before that check.

Streaming surfaces (the closures to wrap):
  - CLI: crates/ironhermes-cli/src/main.rs ~line 821 — `.with_streaming(Box::new(|delta| { print!; flush }))`; agent.run(...).await at ~line 876
  - Gateway: crates/ironhermes-gateway/src/handler.rs ~lines 967-970 — `stream_callback: StreamCallback = Box::new(move |delta| stream_tx_clone.try_send(delta.to_string()))`; try_send takes String
  - Web UI: crates/iron_hermes_ui/src/server/ws.rs lines 216-221 — `stream_callback = Box::new(move |delta| tx_stream.send(ChatStreamEvent::Delta{text: delta.to_string()}))`; run_web_turn invoked at ~line 256 in the same async block; outer sender is `tx` (line ~209), closure clone is `tx_stream` (line ~215)

Scrubber flush pattern (RESEARCH Pitfall 2 — REQUIRED, the closure is Fn not FnOnce):
  Arc<std::sync::Mutex<StreamingContextScrubber>> (std, not tokio — no await in callback).
  One Arc::clone moves into the closure (feed); the outer Arc calls flush() after the
  agent.run / run_web_turn await returns. Emit the flush tail if non-empty.

Lock discipline (RESEARCH Pitfall 1): in agent_loop, the tokio MutexGuard from
mgr.lock().await must be dropped before messages.insert(). Use a scoped block:
  let raw = { let g = mgr.lock().await; g.prefetch_with_query(..).await };
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: ChatMessage.is_recall_context + recall_system constructor; compressor step 0 (MEM-READ-03 schema + D-01/D-03)</name>
  <files>crates/ironhermes-core/src/types.rs, crates/ironhermes-agent/src/context_compressor.rs</files>
  <read_first>
    - crates/ironhermes-core/src/types.rs — the ChatMessage struct (lines 8-19), ALL constructors (system 325-333, user, assistant, assistant_tool_calls, tool_result), and content_text(); confirm there is no Default impl on ChatMessage and Role has no obvious default
    - crates/ironhermes-agent/src/context_compressor.rs — compress() opening (lines 80-89), should_compress, prune_tool_results; confirm there is no existing step-0 retain
    - 34A-CONTEXT.md decisions D-01, D-03; 34A-PATTERNS.md types.rs and context_compressor.rs sections; 34A-RESEARCH.md Pitfall 4 (serde skip), Pitfall 5 (no Default)
  </read_first>
  <action>
    In types.rs, add `#[serde(skip)] pub is_recall_context: bool` as the LAST field of ChatMessage (per D-01: wire-transparent, flag-based detection, no sentinel string). `#[serde(skip)]` omits the field from both serialize and deserialize so no `#[serde(default)]` is needed (Pitfall 4). Every existing constructor (system/user/assistant/assistant_tool_calls/tool_result) must add `is_recall_context: false` as the final field — they list fields explicitly so the compiler will catch any omission; do NOT add a Default impl (Role has no default — Pitfall 5). Add a new `pub fn recall_system(content: impl Into<String>) -> Self` immediately after `system()`, identical to `system()` but with `is_recall_context: true`. Do NOT touch prompt_builder.rs or memory_store.rs (D-12 preservation).
    In context_compressor.rs, prepend step 0 to `compress()` BEFORE the `should_compress` check: `messages.retain(|m| !m.is_recall_context);` with a comment tagging Phase 34a D-03 (recall is lowest-priority, re-derivable next turn). Add a unit test in the existing context_compressor test module: build a message vec containing one recall message (via recall_system) plus normal messages, call compress(), assert no message with is_recall_context==true remains.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-core 2>&1 | tail -5</automated>
    <automated>cargo test -p ironhermes-agent --lib context_compressor 2>&1 | tail -6</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` is clean (all constructors across all crates compile — proves every ChatMessage construction site got the new field or used a constructor)
    - `grep -c "is_recall_context" crates/ironhermes-core/src/types.rs` >= 6 (1 field decl + recall_system + each existing constructor)
    - `grep -c "pub fn recall_system" crates/ironhermes-core/src/types.rs` == 1
    - `grep -c "#\[serde(skip)\]" crates/ironhermes-core/src/types.rs` >= 1
    - `git diff --stat crates/ironhermes-agent/src/prompt_builder.rs crates/ironhermes-core/src/memory_store.rs` shows NO changes (D-12 preservation — explicit acceptance criterion)
    - `cargo test -p ironhermes-agent --lib context_compressor` passes including the new recall-eviction test
    - `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` is GREEN (D-12 gate)
  </acceptance_criteria>
  <done>ChatMessage carries a wire-transparent is_recall_context flag with a recall_system constructor; the compressor evicts recall messages as step 0; the frozen snapshot (D-12) is provably untouched; workspace compiles.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Create streaming_scrubber.rs (StreamingContextScrubber feed/flush/reset) with 6 tests (MEM-READ-04)</name>
  <files>crates/ironhermes-agent/src/streaming_scrubber.rs, crates/ironhermes-agent/src/lib.rs</files>
  <read_first>
    - /Users/twilson/code/hermes-agent/agent/memory_manager.py — the canonical StreamingContextScrubber state machine: in_span/buf fields, feed() logic, flush() semantics, and the _max_partial_suffix helper (case-insensitive suffix-could-be-tag-prefix check)
    - crates/ironhermes-agent/src/nudge.rs — structural template (file doc comment + struct + impl + inline sync `#[cfg(test)] mod tests`)
    - crates/ironhermes-agent/src/lib.rs — existing `pub mod` declarations; mirror placement
    - 34A-RESEARCH.md Pattern 4 (struct/impl skeleton), Pitfall 7 (case-insensitivity); 34A-PATTERNS.md streaming_scrubber.rs section (tag consts, flush semantics, Arc<std::sync::Mutex> flush pattern)
  </read_first>
  <behavior>
    - Test 1 (full_block_in_one_delta): feed("hello <memory-context>secret</memory-context> world") returns visible text containing "hello" and "world" but NOT "secret" and NOT "<memory-context>"
    - Test 2 (split_open_tag_across_two_deltas): feed("hi <memory-con") then feed("text>secret</memory-context> bye") — concatenated visible output contains "hi " and " bye" but not "secret" nor any fence tag (partial open tag held back, not leaked)
    - Test 3 (split_close_tag_across_two_deltas): feed("a<memory-context>secret</memory-con") then feed("text>b") — visible contains "a" and "b", not "secret", not the tag
    - Test 4 (partial_tail_held_then_completes): feed("ok <memory-cont") returns "ok " (the partial-tag tail is held back, not emitted); a subsequent feed("inues normally") that disproves the tag emits the held buffer + new text as ordinary output (no fence detected)
    - Test 5 (span_never_closes_flush_returns_empty): feed("x<memory-context>open forever") emits "x"; flush() returns "" (unterminated open span discarded, not leaked)
    - Test 6 (two_complete_blocks_back_to_back): feed("<memory-context>a</memory-context><memory-context>b</memory-context>tail") returns only "tail"; flush() returns ""
  </behavior>
  <action>
    Create crates/ironhermes-agent/src/streaming_scrubber.rs. File-level doc comment tagging Phase 34a Plan 02 / MEM-READ-04 and citing the Python source. Define `const OPEN_TAG: &str = "<memory-context>";` and `const CLOSE_TAG: &str = "</memory-context>";` (lowercase literals). Define `pub struct StreamingContextScrubber { in_span: bool, buf: String }` with `pub fn new()`, `pub fn feed(&mut self, text: &str) -> String`, `pub fn flush(&mut self) -> String`, `pub fn reset(&mut self)`, and a private `fn max_partial_suffix(buf: &str, tag: &str) -> usize`. Port the Python state machine directly. Case-insensitivity (Pitfall 7): compare via lowercased copies / eq_ignore_ascii_case so split mixed-case tags are caught; tag constants stay lowercase. flush() semantics: if in_span return "" and clear buf; else return buf and clear (held non-tag tail emitted). The buf holds back the tail of a partial open/close tag until the next delta confirms whether it is a real tag or ordinary text. Add a `#[derive(Default)]` or `impl Default` if convenient for `new()`. Write the 6 sync `#[test]` fns in an inline `#[cfg(test)] mod tests` per the behavior block. In lib.rs add `pub mod streaming_scrubber;` alongside the other declarations.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib streaming_scrubber::tests 2>&1 | tail -8</automated>
  </verify>
  <acceptance_criteria>
    - `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` reports 6 passed; 0 failed
    - `cargo build -p ironhermes-agent` is clean
    - `grep -c "pub mod streaming_scrubber" crates/ironhermes-agent/src/lib.rs` == 1
    - `grep -c "pub fn feed\|pub fn flush\|pub fn reset" crates/ironhermes-agent/src/streaming_scrubber.rs` == 3
    - `grep -c "eq_ignore_ascii_case\|to_lowercase" crates/ironhermes-agent/src/streaming_scrubber.rs` >= 1 (case-insensitive comparison present)
    - Test 5 specifically asserts flush() == "" for an unterminated span (no leak)
  </acceptance_criteria>
  <done>streaming_scrubber.rs exists with a working StreamingContextScrubber (feed/flush/reset); 6 tests pass including split-tag, never-closes, and back-to-back cases; module declared in lib.rs.</done>
</task>

<task type="auto">
  <name>Task 3: Pre-turn recall injection in agent_loop + scrubber wiring into all 3 streaming surfaces (MEM-READ-03 injection + MEM-READ-05)</name>
  <files>crates/ironhermes-agent/src/agent_loop.rs, crates/ironhermes-cli/src/main.rs, crates/ironhermes-gateway/src/handler.rs, crates/iron_hermes_ui/src/server/ws.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/agent_loop.rs — the compression block (lines 920-928), the line after it (turns_used += 1 at 930), the post-turn queue_prefetch block (1053-1075) for the lock/rev-find analog, the pressure-advisory push (941-952), and how self.memory_manager / self.session_id are typed
    - crates/ironhermes-cli/src/main.rs — the with_streaming closure (~821) and the agent.run(...).await call (~876)
    - crates/ironhermes-gateway/src/handler.rs — the stream_callback closure (~967-970) and the agent.run await in handle_with_multimodal
    - crates/iron_hermes_ui/src/server/ws.rs — the stream_callback closure (216-221), the run_web_turn invocation (~256), the outer tx (~209) and the tx_stream clone (~215)
    - 34A-CONTEXT.md D-02/D-04/D-05/D-06/D-07/D-08; 34A-RESEARCH.md Pattern 3 (injection), Pattern 5 (wiring), Pitfalls 1/2/3; 34A-PATTERNS.md agent_loop.rs section + all 3 surface sections
  </read_first>
  <action>
    AGENT LOOP (agent_loop.rs): Insert the pre-turn recall block AFTER line 928 (end of compression) and BEFORE line 930 (turns_used += 1), in this exact order (D-02, Pitfall 3):
    (1) `messages.retain(|m| !m.is_recall_context);` — evict prior recall FIRST so insert-index scans are correct.
    (2) `if let Some(ref mgr) = self.memory_manager {` — only when wired.
    (3) extract last user message text via rev-find on Role::User + content_text (the queue_prefetch analog); if empty, skip.
    (4) fetch in a scoped block so the tokio MutexGuard drops before mutation (Pitfall 1): `let raw = { let g = mgr.lock().await; g.prefetch_with_query(&user_msg_text, session_id).await };`
    (5) `if let Ok(raw) = raw { if let Some(block) = crate::memory_context::build_memory_context_block(&raw) { ... } }` — D-08: when build returns None (empty recall), do NOTHING further (no insert). Because step (1) already ran unconditionally, ALSO ensure the empty-recall path leaves the buffer unchanged from a no-recall session: the retain only removes the prior recall message, which is correct (D-08 is satisfied — no NEW insert on empty).
    (6) compute insert index via `messages.iter().rposition(|m| m.role == Role::User).unwrap_or(messages.len())` and `messages.insert(insert_idx, ChatMessage::recall_system(block));` — recall sits immediately BEFORE the last user message (Pattern 3 / Pitfall, NOT after). session_id comes from `self.session_id.as_deref().unwrap_or("")`. Do NOT touch prompt_builder.rs.
    THREE SURFACES (CLI/gateway/ws): For each, create `let scrubber = Arc::new(std::sync::Mutex::new(StreamingContextScrubber::new()));` before the closure, move an `Arc::clone` into the closure, replace the raw delta write with `let visible = scrubber_cb.lock().unwrap().feed(delta); if !visible.is_empty() { <emit visible> }`, and after the agent.run / run_web_turn await returns, call `let tail = scrubber.lock().unwrap().flush(); if !tail.is_empty() { <emit tail> }` (Pitfall 2, D-05/D-06/D-07). CLI emits via print!+flush; gateway via stream_tx.try_send(visible) (try_send takes String — visible is already owned, no .to_string()); ws via tx_stream_cb.send(ChatStreamEvent::Delta{text: visible}) and flush via the outer tx. Use the full path `ironhermes_agent::streaming_scrubber::StreamingContextScrubber` in cli/gateway/ws since those crates import from the agent crate.
  </action>
  <verify>
    <automated>cargo build --workspace 2>&1 | tail -8</automated>
    <automated>cargo test -p ironhermes-agent --lib agent_loop 2>&1 | tail -6</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` is clean (no errors, no new warnings)
    - `grep -c "prefetch_with_query" crates/ironhermes-agent/src/agent_loop.rs` >= 1
    - `grep -c "recall_system\|build_memory_context_block" crates/ironhermes-agent/src/agent_loop.rs` >= 2
    - `grep -c "retain(|m| !m.is_recall_context)\|retain(| *m *| *!m.is_recall_context)" crates/ironhermes-agent/src/agent_loop.rs` >= 1 (eviction present)
    - Static-grep MEM-READ-05 wiring (each surface scrubs deltas): `grep -c "\.feed(" crates/ironhermes-cli/src/main.rs` >= 1 AND `grep -c "\.feed(" crates/ironhermes-gateway/src/handler.rs` >= 1 AND `grep -c "\.feed(" crates/iron_hermes_ui/src/server/ws.rs` >= 1
    - Each surface flushes: `grep -c "\.flush()" crates/ironhermes-cli/src/main.rs crates/ironhermes-gateway/src/handler.rs crates/iron_hermes_ui/src/server/ws.rs` total >= 3 (CLI's io flush is separate — confirm a scrubber.lock().unwrap().flush() per surface by reading)
    - `git diff --stat crates/ironhermes-agent/src/prompt_builder.rs` shows NO changes (D-12 preservation)
    - injection ordering verified by reading agent_loop body: retain BEFORE rposition BEFORE insert (Pitfall 3)
  </acceptance_criteria>
  <done>The agent loop injects fresh query-scoped recall before the latest user message each turn (evicting prior recall first, skipping insert on empty per D-08); all three streaming surfaces scrub deltas and flush their tail; the workspace builds; prompt_builder.rs is untouched.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| memory provider -> model context | The recall block injected into `messages` as a synthetic system message originates from a provider; it is sanitized (34a-01) before wrapping, but the wrapped block still crosses into the model's authoritative context. |
| model output -> user-visible stream | The model may echo `<memory-context>` fence tags it saw; these must never reach the user-visible scrollback (CLI/gateway/web). |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34a-04 | Information Disclosure | streaming delta path on all 3 surfaces | mitigate | `StreamingContextScrubber.feed` strips `<memory-context>...</memory-context>` spans from every delta, holding partial tags across chunk boundaries; `flush()` discards an unterminated open span rather than leaking it (Test 5). Wired identically on CLI, gateway, and web UI (D-07). |
| T-34a-05 | Tampering | injected recall message stacking / leaking into durable history | mitigate | `is_recall_context` recall messages are evicted pre-turn (D-02 retain) and as compressor step 0 (D-03); they are `#[serde(skip)]` so they never serialize to wire/disk, and being evicted before the LLM call that produces `result.appended` they do not enter durable history. |
| T-34a-06 | Tampering | D-12 frozen-snapshot integrity | mitigate | Injection is a SEPARATE path into the `messages` vec inside the agent loop; `prompt_builder.rs` and `memory_store.rs` are not modified. `test_snapshot_frozen_after_load` is an explicit acceptance gate in Task 1. |
| T-34a-07 | Denial of Service | empty-recall cache invalidation | accept | D-08 skips injection entirely when recall is empty, so file-provider-only sessions are byte-identical to pre-34a and keep prompt-prefix cache hits. The common case has zero added overhead beyond a no-op `prefetch_with_query`. |
</threat_model>

<verification>
Full phase verification recipe:
```bash
cargo build --workspace
cargo test -p ironhermes-agent --lib memory_context::tests        # 8/8 (from 34a-01)
cargo test -p ironhermes-agent --lib streaming_scrubber::tests     # 6/6
cargo test -p ironhermes-agent --lib context_compressor            # includes recall-eviction test
cargo test -p ironhermes-agent --lib agent_loop
# Cross-phase regression gates (MUST stay green):
cargo test -p ironhermes-agent --lib nudge::tests                  # Phase 32 — 6/6
cargo test -p ironhermes-agent --test invariants_33                # Phase 33 — 6/6
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load # D-12 gate

# Manual live recall (stub provider returning fixed "Recall: user prefers dark mode."):
#   hermes chat -> "what do you remember about me?" -> response references dark mode;
#   no <memory-context> tags visible in scrollback.
```
</verification>

<success_criteria>
- ChatMessage has a #[serde(skip)] is_recall_context flag + recall_system constructor; all constructors updated; workspace compiles
- Agent loop injects fresh recall before the latest user message each turn (retain -> fetch -> insert), skipping insert when recall is empty (D-08)
- Compressor step 0 evicts recall messages (D-03)
- streaming_scrubber.rs: StreamingContextScrubber with feed/flush/reset; 6 tests pass
- Scrubber wired + flushed on CLI, gateway, and web UI (MEM-READ-05; static-grep .feed in all 3)
- D-12 preserved: prompt_builder.rs and memory_store.rs unchanged; test_snapshot_frozen_after_load green
- Phase 32 nudge::tests (6/6) and Phase 33 invariants_33 (6/6) stay green
</success_criteria>

<output>
Create `.planning/phases/34a-read-side-memory-parity/34a-02-SUMMARY.md` when done.
</output>
