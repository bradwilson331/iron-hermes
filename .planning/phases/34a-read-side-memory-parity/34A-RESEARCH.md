# Phase 34a: Read-Side Memory Parity - Research

**Researched:** 2026-05-20
**Domain:** Rust memory subsystem extension + streaming output scrubbing
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** `ChatMessage` gains `#[serde(skip)] pub is_recall_context: bool` (default `false`). Wire-transparent.
- **D-02:** Eviction timing is pre-turn, before re-injection: `messages.retain(|m| !m.is_recall_context)` first, then `prefetch_with_query`, then insert if non-empty.
- **D-03:** `ContextCompressor::compress()` adds step 0: `messages.retain(|m| !m.is_recall_context)` before normal tool-result pruning.
- **D-04:** No mid-turn re-injection after compressor fires. Fresh recall at start of next user turn only.
- **D-05:** `StreamingContextScrubber` intercepts at the delta-decode layer — each SSE/WebSocket delta passes through `scrubber.feed(delta)` before writing to output.
- **D-06:** New scrubber per turn — created at agent run start, dropped at stream end. `reset()` kept in API for completeness.
- **D-07:** All 3 surfaces use the same delta-scrub pattern (CLI, gateway, web UI).
- **D-08 (amended — see CONTEXT.md D-08, 2026-05-20):** When `prefetch_with_query` returns empty, skip the new INSERT (build returns `None`) but ALWAYS evict prior recall via the unconditional `retain` at turn start (no-op on a never-injected session). Gating the retain would cause a stale-recall bug. Explicit acceptance criterion.

### Claude's Discretion

- Exact position calculation for `last_user_msg_index()` — scan from end, find last `role: User`.
- `Default::default()` impl for `ChatMessage` — add if not already present.
- Whether `#[serde(default)]` is also needed on `is_recall_context` for forward-compat deserialisation.

### Deferred Ideas (OUT OF SCOPE)

- `@`-reference expansion (`@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, `@url:`) — Phase 34b
- `ContextEngine` lifecycle hooks (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`) — Phase 34b
- `ContextCompressor` counter reset — Phase 34b
- `MemoryProvider` hooks `on_turn_start` / `on_session_switch` / `on_delegation` — future phase
- "Only one external provider" guard — future phase
- `on_pre_compress` returns text — future phase
- LCM-style engine tools (`lcm_grep`, `lcm_describe`, `lcm_expand`) — when an LCM engine lands
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MEM-READ-01 | `prefetch_with_query(query, session_id) -> Result<String>` trait method on `MemoryProvider` (default no-op) + `MemoryManager` proxy joining provider results | Verified: trait is in `memory_provider.rs`, pattern matches existing `queue_prefetch` default. No existing implementor needs to change. |
| MEM-READ-02 | `crates/ironhermes-agent/src/memory_context.rs` — `sanitize_context` and `build_memory_context_block` | Verified: Python source confirmed. No existing file — new module. `lib.rs` must add `pub mod memory_context`. |
| MEM-READ-03 | Pre-turn synthetic-`role: System`-message injection in agent loop; ephemeral marker via `is_recall_context`; evicted before re-injection | Verified: injection point is at lines 919–928 in `agent_loop.rs` (the pre-compression block, top of the main loop body). `ChatMessage::system()` constructor exists. `is_recall_context` field not yet present on `ChatMessage`. |
| MEM-READ-04 | `StreamingContextScrubber` state machine (`feed`/`flush`/`reset`) | Verified: Python state machine fully read. No existing Rust equivalent. `lib.rs` must add `pub mod streaming_scrubber`. |
| MEM-READ-05 | Scrubber wired into all THREE streaming output paths | Verified exact wiring points in all 3 surfaces. |
</phase_requirements>

---

## Summary

Phase 34a ports the read-side of `hermes-agent/agent/memory_manager.py` into Rust. The Python reference is live at `/Users/twilson/code/hermes-agent/agent/memory_manager.py` and was read in full. The implementation is mechanically straightforward: add a defaulted trait method, add two new modules, wire one pre-turn injection into the agent loop, and wrap three streaming callbacks with a scrubber.

The draft plan's line number reference for the `queue_prefetch` call site (`agent_loop.rs:1041–1055`) is **correct and current** — the post-turn background warm-up fires at line 1053–1075 on the natural-end break. The pre-turn injection must be inserted at the block beginning at line 919 (after the compression block, before `turns_used += 1` at line 930), specifically after line 928 (the compression path) and before line 930 (`turns_used += 1`).

The `ChatMessage` struct currently has no `is_recall_context` field and no `Default` impl — both must be added. The `ChatMessage::system()` constructor exists and is the correct builder to use for injected messages; the new field will be set `true` by a struct-update on top of that constructor or by a dedicated helper. The `ContextCompressor::compress()` method currently has no step 0 retain; it must be prepended.

The three streaming surfaces each build `stream_callback: Box<dyn Fn(&str) + Send + Sync>` and pass it into `AgentLoop::with_streaming()`. The scrubber wraps the delta inside that closure. Because the closure is `move`, the scrubber instance must be created before the closure and moved in.

**Primary recommendation:** Follow the two-wave plan exactly as drafted. All claimed APIs and file paths are confirmed against current code. The only stale claim in the draft is the suggestion that `nudge::tests` lives in a separate test file — it lives as `mod tests` inside `nudge.rs` itself (lines 154–183), and the cargo test filter `--lib nudge::tests` is correct.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| `prefetch_with_query` trait method | `ironhermes-core` (trait def) | `ironhermes-agent` (proxy) | Trait ownership lives in core; proxy in agent mirrors the existing `queue_prefetch` pattern |
| `memory_context.rs` (sanitize + build) | `ironhermes-agent` | — | Agent-layer transform; not needed in core or surfaces |
| Pre-turn injection | `ironhermes-agent` (agent_loop) | — | Loop owns message assembly; system prompt assembly (`prompt_builder.rs`) must NOT be touched (D-12) |
| `streaming_scrubber.rs` | `ironhermes-agent` | — | Shared by all three surfaces via the crate; surfaces depend on agent crate |
| Scrubber wiring | CLI crate / gateway crate / UI crate | — | Each surface owns its `stream_callback` closure; scrubber instance moves into it |
| `is_recall_context` field | `ironhermes-core` (`types.rs`) | — | `ChatMessage` is in core; `#[serde(skip)]` makes it wire-transparent |
| Compressor step 0 | `ironhermes-agent` (`context_compressor.rs`) | — | Compressor is agent-local; step 0 prepended before existing prune logic |

---

## Standard Stack

### Core

No new external dependencies. This phase uses only existing crates already in the workspace.

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `async-trait` | already in workspace | `prefetch_with_query` default async impl on trait | Pattern already used for all other `MemoryProvider` async methods |
| `regex` | already in workspace (if present) / pure Rust | `sanitize_context` tag stripping | Can also use `str::find` for the scrubber — see pitfall note |
| `anyhow` | already in workspace | `prefetch_with_query` return type `anyhow::Result<String>` | Existing error type throughout |

**No new crate dependencies required.** `sanitize_context` can be implemented with `regex` if it is already a transitive dep, or with manual string operations for the tag-stripping (which are simple enough). The `StreamingContextScrubber` uses only `String` operations — no regex.

### Supporting

None.

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| Manual string `find` in scrubber | `regex` crate | `regex` is heavier; the scrubber state machine with `_max_partial_suffix` is simpler and matches the Python byte-for-byte |
| `#[serde(skip)]` on `is_recall_context` | Sentinel string prefix in content | Sentinel risks leaking on bugs; schema field is cleaner and wire-transparent |
| Scan-from-end for `last_user_msg_index` | `position()` from front | Scan from end is O(1) in the common case (user message is last or second-to-last) |

---

## Package Legitimacy Audit

No new external packages are introduced by this phase. This section is not applicable.

---

## Architecture Patterns

### System Architecture Diagram

```
User message arrives
        │
        ▼
[top of agent loop turn]
        │
        ├─► retain(|m| !m.is_recall_context)   ← evict prior recall injection (D-02)
        │
        ├─► memory_manager.prefetch_with_query(user_msg, session_id).await
        │          │
        │          ├── provider.prefetch_with_query() → "" (file provider no-op)
        │          └── provider.prefetch_with_query() → "fact A\nfact B" (semantic provider)
        │
        ├─► if non-empty: build_memory_context_block(raw) → Option<String>
        │          │
        │          └── inject ChatMessage { role: System, is_recall_context: true, content: block }
        │              immediately BEFORE last user message
        │
        ▼
[LLM call — messages include recall block]
        │
        ▼
StreamEvent::ContentDelta(delta)
        │
        ▼
scrubber.feed(delta)  ← strips <memory-context>...</memory-context> from output stream
        │
        ▼
[visible delta → CLI print / gateway send / ws tx.send]
        │
        ▼
[stream end] → scrubber.flush() → emit any held tail
```

### Recommended Project Structure

```
crates/ironhermes-core/src/
└── types.rs                   # add is_recall_context: bool field + serde(skip)

crates/ironhermes-agent/src/
├── lib.rs                     # add: pub mod memory_context; pub mod streaming_scrubber;
├── memory_context.rs          # NEW: sanitize_context + build_memory_context_block + 8 tests
├── streaming_scrubber.rs      # NEW: StreamingContextScrubber + 6 tests
├── memory/
│   └── manager.rs             # add prefetch_with_query proxy method
├── agent_loop.rs              # pre-turn injection (D-02/D-03/D-04/D-08)
└── context_compressor.rs      # add step 0 retain before prune_tool_results

crates/ironhermes-core/src/
└── memory_provider.rs         # add prefetch_with_query defaulted trait method
```

### Pattern 1: Default No-Op Trait Method (MEM-READ-01)

**What:** Add `prefetch_with_query` to the `MemoryProvider` trait with a default no-op impl.
**When to use:** Whenever extending the trait without breaking existing implementors.

```rust
// Source: existing queue_prefetch pattern in memory_provider.rs (line 149)
async fn prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String> {
    Ok(String::new())
}
```

The file provider (`MemoryStore`) gets the no-op automatically. Semantic providers (grafeo, duckdb) override this. This follows the established pattern: `queue_prefetch`, `on_pre_compress`, and `on_memory_write` all use identical default-no-op idiom.

**Important:** `prefetch_with_query` takes `&self` (immutable) matching the existing `prefetch(&self, session_id)` signature. It does NOT take `&mut self`. [VERIFIED: codebase grep]

### Pattern 2: MemoryManager Proxy (MEM-READ-01)

**What:** Proxy method on `MemoryManager` that iterates providers and joins results.
**When to use:** For all read paths per D-26 (primary only).

```rust
// Source: existing prefetch() proxy at manager.rs:180
pub async fn prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String> {
    let p = self.primary.lock().await;
    p.prefetch_with_query(query, session_id).await
}
```

Note: The Python `prefetch_all` iterates ALL providers and joins with `\n\n`. The Rust `MemoryManager` architecture has a single primary + optional write-only mirror. Mirror is write-only (D-26, D-28) — `prefetch_with_query` goes to primary only. This is correct and matches the existing `prefetch()` proxy.

### Pattern 3: Pre-Turn Injection (MEM-READ-03)

**What:** Insert recall block before the last user message at the top of each loop turn.
**When to use:** At the injection point inside `run()`.

The exact insertion point in `agent_loop.rs` is **after line 928** (end of compression block) and **before line 930** (`turns_used += 1`). The current sequence at that point is:

```
Line 920-928: pre-chat compression (context_engine or compressor)
// ← INSERT RECALL INJECTION HERE
Line 930:     turns_used += 1
```

The injection code pattern:

```rust
// Source: D-02 decision + existing queue_prefetch pattern (agent_loop.rs:1053)
// Step 1: evict prior recall injection
messages.retain(|m| !m.is_recall_context);

// Step 2: fetch (only if memory_manager is wired)
if let Some(ref mgr) = self.memory_manager {
    let session_id = self.session_id.as_deref().unwrap_or("");
    let user_msg_text = messages.iter().rev()
        .find(|m| m.role == ironhermes_core::Role::User)
        .and_then(|m| m.content_text().map(|s| s.to_string()))
        .unwrap_or_default();

    if !user_msg_text.is_empty() {
        let guard = mgr.lock().await;
        if let Ok(raw) = guard.prefetch_with_query(&user_msg_text, session_id).await {
            drop(guard); // release lock before message vec mutation
            if let Some(block) = crate::memory_context::build_memory_context_block(&raw) {
                // Find insertion point: immediately before last user message
                let insert_idx = messages.iter().rposition(|m| m.role == ironhermes_core::Role::User)
                    .unwrap_or(messages.len());
                let recall_msg = ChatMessage {
                    role: ironhermes_core::Role::System,
                    content: Some(ironhermes_core::MessageContent::Text(block)),
                    tool_calls: None,
                    tool_call_id: None,
                    name: None,
                    is_recall_context: true,
                };
                messages.insert(insert_idx, recall_msg);
            }
        }
    }
}
```

**Critical lock discipline:** The `mgr.lock().await` guard must be **dropped before** `messages.insert()`. The guard is `await`ed (async), so the borrow checker will not allow holding it while mutating `messages` anyway — but calling `drop(guard)` explicitly before the insert is the cleanest pattern. Do NOT hold the lock across the `messages.insert` call.

### Pattern 4: StreamingContextScrubber (MEM-READ-04)

**What:** State machine that processes streaming deltas, suppressing `<memory-context>` spans.
**When to use:** Wrap every streaming delta before it reaches the output channel.

The Python implementation is the canonical reference (read in full). The Rust port is a direct translation:

```rust
// Source: /Users/twilson/code/hermes-agent/agent/memory_manager.py (StreamingContextScrubber)
pub struct StreamingContextScrubber {
    in_span: bool,
    buf: String,
}

impl StreamingContextScrubber {
    pub fn new() -> Self {
        Self { in_span: false, buf: String::new() }
    }

    pub fn feed(&mut self, text: &str) -> String { /* ... state machine ... */ }
    pub fn flush(&mut self) -> String { /* ... emit or discard tail ... */ }
    pub fn reset(&mut self) { self.in_span = false; self.buf.clear(); }

    fn max_partial_suffix(buf: &str, tag: &str) -> usize { /* ... */ }
}
```

The key insight: the `_max_partial_suffix` helper checks case-insensitively whether the tail of `buf` could be the start of `tag`. This is what allows correct handling of `<MeMoRy-CoNtExT>` split across chunks. The Python uses `.lower()` on both sides; Rust must use `.to_lowercase()` or compare byte-by-byte with `.eq_ignore_ascii_case()` on the suffix.

The tag constants are lowercase literals:
- Open tag: `"<memory-context>"` (16 chars)
- Close tag: `"</memory-context>"` (17 chars)

**flush() semantics:** If `in_span == true` at flush, discard `buf` and return `""`. If `in_span == false`, return `buf` and clear it. This matches Python exactly.

### Pattern 5: Scrubber Wiring in Streaming Closures

**What:** Each surface creates a scrubber and moves it into the stream_callback closure.
**When to use:** When wiring any of the three surfaces.

**CLI surface** (`crates/ironhermes-cli/src/main.rs`, line 821):

Current code:
```rust
.with_streaming(Box::new(|delta| {
    print!("{}", delta);
    io::stdout().flush().ok();
}))
```

After wiring (scrubber must be created before and moved in):
```rust
let mut scrubber = StreamingContextScrubber::new();
.with_streaming(Box::new(move |delta| {
    let visible = scrubber.feed(delta);
    if !visible.is_empty() {
        print!("{}", visible);
        io::stdout().flush().ok();
    }
}))
// Note: flush() is called at turn end — see pitfall on flush placement
```

**Gateway surface** (`crates/ironhermes-gateway/src/handler.rs`, line 968):

Current code:
```rust
let stream_tx_clone = stream_tx.clone();
let stream_callback: StreamCallback = Box::new(move |delta: &str| {
    let _ = stream_tx_clone.try_send(delta.to_string());
});
```

After wiring:
```rust
let stream_tx_clone = stream_tx.clone();
let mut scrubber = StreamingContextScrubber::new();
let stream_callback: StreamCallback = Box::new(move |delta: &str| {
    let visible = scrubber.feed(delta);
    if !visible.is_empty() {
        let _ = stream_tx_clone.try_send(visible);
    }
});
```

**Web UI surface** (`crates/iron_hermes_ui/src/server/ws.rs`, line 216):

Current code:
```rust
let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
    Box::new(move |delta: &str| {
        let _ = tx_stream.send(ChatStreamEvent::Delta {
            text: delta.to_string(),
        });
    });
```

After wiring:
```rust
let mut scrubber = ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new();
let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
    Box::new(move |delta: &str| {
        let visible = scrubber.feed(delta);
        if !visible.is_empty() {
            let _ = tx_stream.send(ChatStreamEvent::Delta {
                text: visible,
            });
        }
    });
```

**Note on flush():** The `stream_callback` closure is a `Fn` (not `FnOnce`), so the scrubber is moved in and the closure is called once per delta. The `flush()` must be called at stream-end. In `call_llm_streaming()` (agent_loop.rs:1158–1205), after the `while let Some(event) = rx.recv().await` loop breaks, the assembled `content` is returned. The flush call must happen at the end of that loop, inside `call_llm_streaming`, OR the stream_callback owns the scrubber and the scrubber's flush is handled by detecting `StreamEvent::Done`. See pitfall section for the borrow-checker constraint.

### Anti-Patterns to Avoid

- **Touching `prompt_builder.rs`:** The system prompt assembly (`build_system_prompt_block`, `system_prompt_block`) must NOT be modified. D-12 is the invariant — the frozen snapshot is read-only. The recall injection is a separate path in the agent loop, not in the system prompt.
- **Holding the MemoryManager lock across `.await`:** The `mgr.lock().await` in the injection block acquires a `tokio::sync::Mutex` guard. This guard cannot be held across a `.await` boundary under some Rust lifetime situations. Pattern: acquire, call, drop explicitly, then mutate messages.
- **Treating `appended` as durable history for recall messages:** The `is_recall_context: true` messages should NOT appear in `result.appended`. They are transient. The `appended` vec is built by the loop; the planner should verify that the retain before re-injection also removes them from any persistent-history path (though since they're evicted before the LLM call that produces `appended`, they should not end up there naturally).
- **Injecting recall after the user message:** The injection point is **before** the last user message (insert at `last_user_idx`), not after it. The model sees: `[...history...] [recall:System] [user:User]`. This is the Python contract.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Partial-tag boundary detection across chunks | Custom buffering from scratch | Port Python `_max_partial_suffix` exactly | The Python implementation is already battle-tested for this edge case; deviating risks leaking fence tags |
| Regex-based sanitize_context | Multi-pass string replace | Port the 3-regex sequence from Python exactly | The regex set handles case-insensitivity and whitespace variants in the system note |
| Lock management across providers | Custom fan-out | Proxy through `self.primary` only | Mirror is write-only per D-26/D-28; primary-only matches all existing read proxies |

---

## Runtime State Inventory

Not applicable. This is a greenfield extension (new modules + additive code changes). No rename/refactor/migration.

---

## Common Pitfalls

### Pitfall 1: Borrow Checker — Mutex Guard Held Across `.await`

**What goes wrong:** Code like `let guard = mgr.lock().await; guard.prefetch_with_query(...).await; messages.insert(...)` can cause a borrow-checker error if `guard` is still live during `messages.insert`. With `tokio::sync::Mutex`, the guard holds a reference into the mutex's interior; if the compiler determines it could still be in scope at the `.await`, it rejects the code.

**Why it happens:** Rust's async borrow checker tracks `Send` bounds across `.await` points. A `tokio::sync::MutexGuard` is not `Send`, so if it's in scope at an `.await`, the future becomes `!Send` and fails to compile when the future must be `Send`.

**How to avoid:** Explicitly `drop(guard)` before any subsequent `.await` or mutation. Or scope the lock acquisition inside a block that ends before the `messages.insert`:
```rust
let raw = {
    let guard = mgr.lock().await;
    guard.prefetch_with_query(query, session_id).await
};
// guard dropped here — safe to mutate messages
if let Ok(r) = raw { ... messages.insert(...) }
```

**Warning signs:** Compiler error "future is not `Send`" or "`MutexGuard` is held across an await point".

### Pitfall 2: `flush()` Placement — Where Does the Scrubber Emit Its Tail?

**What goes wrong:** The scrubber holds back a partial tag tail in `self.buf`. If `flush()` is never called, trailing content that was NOT a tag gets dropped.

**Why it happens:** The scrubber cannot distinguish `<memory-cont` (start of a tag, should be withheld) from `<memory-cont` (ordinary text containing that substring, should be emitted) until more bytes arrive. At end-of-stream, `flush()` resolves the ambiguity.

**How to avoid:** `flush()` must be called after the streaming loop ends, before returning from the turn. In `call_llm_streaming` (agent_loop.rs:1158), this means: after the `while let` loop, invoke the callback one more time with `scrubber.flush()`, or — better — expose a `flush` on the scrubber that the `AgentLoop` invokes after the stream completes.

**The tension:** The scrubber is owned by the `stream_callback` closure (moved in). The `AgentLoop` cannot call `flush()` on it because it doesn't have access after the move. Two solutions:
1. Use an `Arc<Mutex<StreamingContextScrubber>>` shared between the closure and the call site.
2. Detect `StreamEvent::Done` inside `call_llm_streaming` and call `cb(scrubber.flush())` — but the callback owns the scrubber.
3. Handle flush inside the closure by checking a signal.

**Recommended approach:** The simplest: in `call_llm_streaming`, after the loop, call `if let Some(ref cb) = self.stream_callback { cb("") }` — the scrubber's `feed("")` returns `""` (no-op), which doesn't help. Instead, expose an `on_stream_end()` hook on `AgentLoop`, or (simplest of all) make the scrubber stateless at end by having the closure detect the empty-string sentinel that `call_llm_streaming` could send as a flush signal. **Cleanest production approach:** `Arc<std::sync::Mutex<StreamingContextScrubber>>` shared between closure and a post-loop `scrubber.lock().unwrap().flush()` call in `call_llm_streaming`. This is what the planner should specify.

**Warning signs:** Missing trailing text in responses; scrubber buf not empty after turn.

### Pitfall 3: `messages.insert()` Index Invalidated by Prior `retain()`

**What goes wrong:** Code that computes `last_user_idx` before `retain()` will get the wrong index after `retain()` removes recall messages.

**Why it happens:** `retain()` shifts all subsequent elements left. If a recall message appeared before the last user message, the user message's index decreases.

**How to avoid:** Always call `retain()` FIRST (step 1), THEN scan for `last_user_idx` (step 2), THEN call `prefetch_with_query` (step 3), THEN insert (step 4). The draft plan D-02 describes this order correctly.

### Pitfall 4: `is_recall_context` Deserialisation of Old Payloads

**What goes wrong:** Existing `ChatMessage` payloads serialized without `is_recall_context` will fail to deserialise if the field is required.

**Why it doesn't happen here:** `#[serde(skip)]` means the field is NEVER serialized or deserialised — it is always the `Default` value (`false`) after deserialisation. No `#[serde(default)]` needed because the field is never present on the wire.

**However:** If there is any code path that serialises `ChatMessage` without `#[serde(skip)]` (e.g. to disk or across a process boundary), the field would appear in the JSON. Verify no such path exists. The context notes confirm `#[serde(skip)]` is the chosen approach.

### Pitfall 5: `ChatMessage` Has No `Default` Impl

**What goes wrong:** Struct-update syntax `ChatMessage { is_recall_context: true, ..ChatMessage::system(block) }` requires `ChatMessage` to implement `Default` OR all other fields to be explicitly listed.

**Current state:** `ChatMessage` has no `#[derive(Default)]` and no explicit `Default` impl (confirmed by reading `types.rs`). The constructor pattern `ChatMessage::system(content)` sets all fields. To add `is_recall_context: true` cleanly, either:
1. Add `#[derive(Default)]` to `ChatMessage` — but `Role` would also need `Default`. `Role` is an enum with no obvious default.
2. Use a dedicated constructor: `ChatMessage::recall_system(content: String) -> Self` that sets `role: System, content: Some(...), is_recall_context: true, rest: None`.
3. Construct explicitly: list all 6 fields.

**Recommended:** Option 2 (dedicated constructor `ChatMessage::recall_system`) is cleanest and avoids the `Default` problem entirely. The planner should specify this.

### Pitfall 6: `sanitize_context` Regex Order Matters

**What goes wrong:** Running `_FENCE_TAG_RE` (bare `<memory-context>` tags) before `_INTERNAL_CONTEXT_RE` (full blocks) could strip the open/close tags leaving the inner content un-stripped.

**Why it happens:** The Python code runs the regexes in order: block regex first, then system-note regex, then bare-tag regex. If the order is reversed, a full `<memory-context>...</memory-context>` block would have its tags stripped but its content (including the system note) left in place.

**How to avoid:** In `sanitize_context`, run the three passes in EXACTLY this order:
1. `_INTERNAL_CONTEXT_RE` — strip complete blocks
2. `_INTERNAL_NOTE_RE` — strip orphaned system notes
3. `_FENCE_TAG_RE` — strip bare tags

**Warning signs:** Unit test for "double-wrap stripping" fails (idempotency test).

### Pitfall 7: Scrubber Case-Insensitivity

**What goes wrong:** The Python scrubber uses `.lower()` for tag comparison. If the Rust port uses byte-equality (`==`) on ASCII strings, it would miss `<Memory-Context>` or `<MEMORY-CONTEXT>`.

**How to avoid:** Use `buf.to_lowercase().find(tag)` or `.eq_ignore_ascii_case()` for the suffix check. The scrubber internal state machine does lowercase comparison on every `find` call, matching Python exactly. The tag constants themselves should be lowercase.

---

## Code Examples

### sanitize_context Regex Set (Ported from Python)

```rust
// Source: /Users/twilson/code/hermes-agent/agent/memory_manager.py lines 43-51
use regex::Regex;
use std::sync::OnceLock;

static FENCE_TAG_RE: OnceLock<Regex> = OnceLock::new();
static INTERNAL_CONTEXT_RE: OnceLock<Regex> = OnceLock::new();
static INTERNAL_NOTE_RE: OnceLock<Regex> = OnceLock::new();

fn fence_tag_re() -> &'static Regex {
    FENCE_TAG_RE.get_or_init(|| {
        Regex::new(r"(?i)</?\s*memory-context\s*>").unwrap()
    })
}
fn internal_context_re() -> &'static Regex {
    INTERNAL_CONTEXT_RE.get_or_init(|| {
        Regex::new(r"(?is)<\s*memory-context\s*>[\s\S]*?</\s*memory-context\s*>").unwrap()
    })
}
fn internal_note_re() -> &'static Regex {
    INTERNAL_NOTE_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\[System note:\s*The following is recalled memory context,\s*NOT new user input\.\s*Treat as (?:informational background data|authoritative reference data[^\]]*)\.\]\s*"
        ).unwrap()
    })
}

pub fn sanitize_context(text: &str) -> String {
    let text = internal_context_re().replace_all(text, "");
    let text = internal_note_re().replace_all(&text, "");
    fence_tag_re().replace_all(&text, "").into_owned()
}
```

**Note on regex dependency:** `regex` may already be a transitive dep. Check `cargo tree -p ironhermes-agent | grep regex`. If not present, add to `[dependencies]` in `ironhermes-agent/Cargo.toml`. Alternatively, `sanitize_context` can be implemented without `regex` using `str::find` loops — but the regex approach is more maintainable and matches the Python structure exactly.

### build_memory_context_block (Ported from Python)

```rust
// Source: /Users/twilson/code/hermes-agent/agent/memory_manager.py lines 173-187
pub fn build_memory_context_block(raw: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    let clean = sanitize_context(raw);
    Some(format!(
        "<memory-context>\n\
         [System note: The following is recalled memory context, \
         NOT new user input. Treat as authoritative reference data \u{2014} \
         this is the agent's persistent memory and should inform all responses.]\n\n\
         {clean}\n\
         </memory-context>"
    ))
}
```

**Important:** The em dash `—` in the system note is U+2014 (`\u{2014}`). The Python source uses a literal `—`. Both produce the same UTF-8 bytes. The Rust version must produce an identical string or the system-note regex in `sanitize_context` will fail to strip it (idempotency requirement).

### ContextCompressor Step 0 (D-03)

```rust
// Source: context_compressor.rs compress() method, line 80
pub fn compress(&mut self, messages: &mut Vec<ChatMessage>) -> bool {
    // Step 0 (Phase 34a D-03): strip ephemeral recall messages before
    // any token estimation — they're re-derivable next turn and must
    // be freed first when context is tight.
    messages.retain(|m| !m.is_recall_context);

    if !self.should_compress(messages) {
        return false;
    }
    // ... existing steps 1-2 unchanged ...
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Frozen snapshot only (load_from_disk at session start) | Frozen snapshot + per-turn semantic recall injection | Phase 34a | Agent can now answer from mid-session memory writes without waiting for next session |
| No streaming fence-tag filtering | StreamingContextScrubber on every delta | Phase 34a | Model's internal recall context never leaks to visible scrollback |
| `queue_prefetch` only (background warm, no pre-turn inject) | `prefetch_with_query` (awaited, pre-turn, query-scoped) | Phase 34a | Recall is semantically relevant to the current user message, not just session-level warm |

**Deprecated/outdated:**
- The pattern of "memory is only in system prompt snapshot" is superseded by the hybrid approach: frozen snapshot (D-12) + per-turn recall injection (this phase). The D-12 invariant is preserved — they coexist.

---

## D-12 Invariant Analysis

`test_snapshot_frozen_after_load` is confirmed at `crates/ironhermes-core/src/memory_store.rs:782`. It tests that `format_for_system_prompt()` returns the same value before and after `add()` — the in-memory snapshot loaded by `load_from_disk()` is frozen and mid-session writes do not mutate it.

Phase 34a **does not violate D-12** because:
1. `prompt_builder.rs` is NOT modified.
2. `MemoryManager::system_prompt_block()` is NOT modified.
3. The recall injection goes into the `messages` vec inside `agent_loop.run()`, NOT into the system prompt assembled by `PromptBuilder`.
4. The injected message has `role: System` but is a separate message appended to the conversation vec — this is distinct from the system prompt (which is the first `role: System` message, frozen at session start).

The OpenAI/Anthropic APIs accept multiple `role: system` messages. The frozen system prompt is `messages[0]`; the recall injection is inserted at `last_user_idx`, typically near the end of the vec. They are structurally distinct.

---

## Parity Matrix (Verified Against Current Code)

| Python symbol | Rust target | Draft status | Verified |
|--------------|-------------|-------------|---------|
| `build_system_prompt()` | `MemoryManager::system_prompt_block()` | ✅ parity | Confirmed at manager.rs:190 |
| `prefetch_all(query)` | `prefetch_with_query(query, session_id)` | ❌ gap → **34a-01** | Trait has `prefetch(session_id)` only; no query arg; confirmed |
| `<memory-context>` block wrapping | `build_memory_context_block` | ❌ gap → **34a-01** | No file exists; confirmed |
| `sanitize_context()` | `sanitize_context` | ❌ gap → **34a-01** | No Rust equivalent; confirmed |
| `StreamingContextScrubber` | `StreamingContextScrubber` | ❌ gap → **34a-02** | No Rust equivalent; confirmed |
| Pre-turn injection in agent loop | agent_loop.rs injection block | ❌ gap → **34a-02** | Only POST-turn `queue_prefetch` exists at line 1053; confirmed |
| `queue_prefetch_all(query)` | `queue_prefetch(query)` | ✅ parity | At manager.rs:201; confirmed |
| `sync_all(user, assistant)` | `sync_turn(session_id, entries)` | ✅ parity | At manager.rs:220; confirmed |

---

## Regression Gates

### Phase 32: nudge::tests

**Location:** `crates/ironhermes-agent/src/nudge.rs:154` (inline `mod tests`)
**Run:** `cargo test -p ironhermes-agent --lib nudge::tests`
**Count:** 2 tests confirmed in file (lines 159 `prompt_contains_tier_guidance`, 176 `prompt_contains_cap_info`). The draft says "6/6" — this may be the full count including tests added later in nudge.rs not visible in the 30-line window read. Run the command to confirm current count.
**Risk to Phase 34a:** Low — these tests check `MEMORY_REVIEW_PROMPT` string content only; no dependency on `ChatMessage` fields or memory provider methods.

### Phase 33: invariants_33

**Location:** `crates/ironhermes-agent/tests/invariants_33.rs`
**Run:** `cargo test -p ironhermes-agent --test invariants_33`
**Count:** 6 invariants (INV-33-01 through INV-33-06) per file header.
**Risk to Phase 34a:** Low — these are static-grep tests checking symbol presence (e.g. `register_skill_manage_tool`, `SelfCreated`). Adding `memory_context` and `streaming_scrubber` modules does not remove any of the grepped symbols.

### D-12 Gate: test_snapshot_frozen_after_load

**Location:** `crates/ironhermes-core/src/memory_store.rs:782`
**Run:** `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`
**Risk to Phase 34a:** None — this phase does not touch `memory_store.rs` or `prompt_builder.rs`. The test will stay green because the snapshot mechanism is untouched.

---

## Open Questions (RESOLVED)

1. **regex crate availability in ironhermes-agent**
   - What we know: `regex` is not explicitly imported in agent source files examined.
   - What's unclear: Whether it's already a transitive dep (e.g. via `ironhermes-core` or another crate).
   - Recommendation: Run `cargo tree -p ironhermes-agent | grep "^regex"` before writing the memory_context module. If present: use it. If absent: either add it to `ironhermes-agent/Cargo.toml` or implement `sanitize_context` with `str::find` loops (feasible for 3 simple patterns).
   - **RESOLVED (2026-05-20):** `regex` IS a direct workspace dep of `ironhermes-agent` (Cargo.toml:38, `regex = { workspace = true }`). Plan 34a-01 `<interfaces>` instructs OnceLock + `regex::Regex`; no new dependency added.

2. **flush() call site for StreamingContextScrubber**
   - What we know: The scrubber is moved into the `stream_callback` closure; `AgentLoop` has no post-stream hook.
   - What's unclear: Whether the planner should specify an `Arc<Mutex<Scrubber>>` shared pattern, or add a `with_stream_end_callback` hook to `AgentLoop`.
   - Recommendation: Use `Arc<std::sync::Mutex<StreamingContextScrubber>>` (std, not tokio — no await in the callback). The closure holds an `Arc::clone`; the call site (after `call_llm_streaming` returns) calls `Arc::lock().unwrap().flush()` and emits if non-empty. This is the cleanest pattern that doesn't require modifying `AgentLoop`'s streaming contract.
   - **RESOLVED (2026-05-20):** Plan 34a-02 Task 3 adopts the `Arc<std::sync::Mutex<StreamingContextScrubber>>` shared pattern verbatim across all 3 surfaces; `flush()` fires at each surface's post-stream call site. Documented in 34A-PATTERNS.md.

3. **nudge::tests count discrepancy**
   - What we know: The draft says "6/6" but only 2 tests were visible in the 30-line read of the nudge.rs test module.
   - What's unclear: Whether there are more tests after line 183.
   - Recommendation: Run `cargo test -p ironhermes-agent --lib nudge::tests -- --list` to confirm actual count before documenting the acceptance criterion.
   - **RESOLVED (2026-05-20):** Confirmed via `cargo test -p ironhermes-agent --lib nudge::tests -- --list` = **6 tests**. The "6/6" assertion in 34A-VALIDATION.md and the plan success_criteria is correct.

---

## Environment Availability

Step 2.6: SKIPPED — this phase is purely code/config changes within the existing workspace. No external tools, services, or CLIs are introduced.

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | none (standard cargo test) |
| Quick run command | `cargo test -p ironhermes-agent --lib` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MEM-READ-01 | `prefetch_with_query` default no-op on `MemoryProvider` | unit | `cargo test -p ironhermes-core --lib` | ✅ (add to existing `memory_provider.rs` tests) |
| MEM-READ-01 | `MemoryManager::prefetch_with_query` proxy joins results | unit | `cargo test -p ironhermes-agent --lib memory::manager` | ✅ (add to `manager.rs` tests) |
| MEM-READ-02 | `sanitize_context` strips blocks, notes, bare tags | unit (8 tests) | `cargo test -p ironhermes-agent --lib memory_context::tests` | ❌ Wave 0 — new file |
| MEM-READ-02 | `build_memory_context_block` wraps and idempotency | unit (included in 8) | `cargo test -p ironhermes-agent --lib memory_context::tests` | ❌ Wave 0 |
| MEM-READ-03 | Injection ordering: recall message appears before last user message | unit (inline in agent_loop tests) | `cargo test -p ironhermes-agent --lib agent_loop` | ✅ (add to existing agent_loop tests) |
| MEM-READ-03 | Empty recall skips inject (D-08) | unit | `cargo test -p ironhermes-agent --lib agent_loop` | ✅ (add) |
| MEM-READ-03 | D-03: compressor step 0 retain strips recall messages | unit | `cargo test -p ironhermes-agent --lib context_compressor` | ✅ (add to existing) |
| MEM-READ-04 | `StreamingContextScrubber` — 6 cases (full block, split open, split close, partial tail, no-close flush, two blocks) | unit (6 tests) | `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` | ❌ Wave 0 — new file |
| MEM-READ-05 | CLI/gateway/web UI wiring — static grep for `scrubber.feed` in each surface | static-grep invariant | `cargo test -p ironhermes-agent --test <invariants_34a if created>` | ❌ Wave 0 — optional |
| Cross-phase | Phase 32 regression gate | regression | `cargo test -p ironhermes-agent --lib nudge::tests` | ✅ |
| Cross-phase | Phase 33 regression gate | regression | `cargo test -p ironhermes-agent --test invariants_33` | ✅ |
| Cross-phase | D-12 gate | regression | `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` | ✅ |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-agent --lib && cargo test -p ironhermes-core --lib`
- **Per wave merge:** `cargo build --workspace && cargo test -p ironhermes-agent --lib nudge::tests && cargo test -p ironhermes-agent --test invariants_33 && cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`
- **Phase gate:** Full `cargo test --workspace` green before `/gsd:verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-agent/src/memory_context.rs` — 8 unit tests for REQ MEM-READ-02
- [ ] `crates/ironhermes-agent/src/streaming_scrubber.rs` — 6 unit tests for REQ MEM-READ-04
- [ ] `crates/ironhermes-agent/src/lib.rs` — `pub mod memory_context; pub mod streaming_scrubber;` declarations

---

## Security Domain

This phase does not introduce authentication, sessions, cryptography, or user-controlled input into new code paths. The `sanitize_context` function strips content from memory provider output — it reduces attack surface (removes injected fence tags). No new ASVS categories are introduced.

The `build_memory_context_block` output is injected as a synthetic system message. It comes from the memory provider, which is already security-scanned at write time (MEM-05). No additional security controls are needed.

`security_enforcement` is not explicitly disabled in `.planning/config.json`, so this section is included.

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | partial | `sanitize_context` strips injected fence tags from provider output; existing MEM-05 security scan covers write path |
| V6 Cryptography | no | — |

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Memory provider returning pre-wrapped `<memory-context>` content | Tampering | `sanitize_context` strips it before re-wrapping; Python does same (logs warning) |
| Fence tags in model response leaking to user | Information Disclosure | `StreamingContextScrubber` strips them on every delta |

---

## Sources

### Primary (HIGH confidence)

- `/Users/twilson/code/ironhermes/crates/ironhermes-core/src/memory_provider.rs` — `MemoryProvider` trait, full method set, default impl patterns [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-core/src/types.rs` — `ChatMessage` struct, `Role` enum, `ChatMessage::system()` constructor [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/memory/manager.rs` — `MemoryManager` full source, `queue_prefetch` proxy at line 201, mirror architecture [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/agent_loop.rs` — `run()` main loop lines 834–1119, streaming loop lines 1143–1205, `queue_prefetch` at lines 1053–1075 [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/context_compressor.rs` — `compress()` method lines 80–110, step 0 absence confirmed [VERIFIED: codebase read]
- `/Users/twilson/code/hermes-agent/agent/memory_manager.py` — Python canonical reference: `sanitize_context`, `build_memory_context_block`, `StreamingContextScrubber`, regex set [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-cli/src/main.rs` — `run_chat` streaming callback at line 821 [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-gateway/src/handler.rs` — `handle_with_multimodal` streaming callback at lines 967–970 [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/iron_hermes_ui/src/server/ws.rs` — web UI `stream_callback` at lines 216–221, `run_web_turn` call at line 255 [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-core/src/memory_store.rs:782` — `test_snapshot_frozen_after_load` test confirmed [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/tests/invariants_33.rs` — 6 invariants confirmed [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/nudge.rs:154` — nudge test module confirmed [VERIFIED: codebase read]
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/lib.rs` — current module list confirmed; `memory_context` and `streaming_scrubber` absent [VERIFIED: codebase read]

### Secondary (MEDIUM confidence)

- None required — all claims verified from primary sources.

### Tertiary (LOW confidence)

- None. All claims are either VERIFIED against current source or CITED from the Python reference.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `regex` crate may not be a direct dependency of `ironhermes-agent` | Standard Stack | If absent, `Cargo.toml` needs a new dep entry; implementation of `sanitize_context` may use manual string ops instead |
| A2 | `nudge::tests` has more tests beyond the 2 visible in the 30-line window (draft says "6/6") | Regression Gates | Acceptance criterion count may be wrong; run `-- --list` to confirm |

**All other claims were verified directly from source code in this session.**

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new external deps; all patterns verified in current code
- Architecture: HIGH — all file paths, line numbers, and API shapes verified
- Pitfalls: HIGH — borrow-checker patterns verified against actual code structure; scrubber analysis is from direct Python source read
- Python parity: HIGH — Python source read in full; all 3 regex patterns and scrubber state machine confirmed

**Research date:** 2026-05-20
**Valid until:** 2026-06-20 (stable Rust codebase; no fast-moving deps)
