# Phase 34a: Read-Side Memory Parity - Pattern Map

**Mapped:** 2026-05-20
**Files analyzed:** 10 new/modified files
**Analogs found:** 9 / 10 (1 no-analog: streaming_scrubber has no Rust precedent)

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/ironhermes-core/src/memory_provider.rs` | trait | request-response | self (existing `queue_prefetch` default) | exact |
| `crates/ironhermes-core/src/types.rs` | model | transform | self (existing `ChatMessage::system()` block) | exact |
| `crates/ironhermes-agent/src/memory/manager.rs` | service | request-response | self (existing `prefetch()` proxy, line 180) | exact |
| `crates/ironhermes-agent/src/memory_context.rs` (NEW) | utility | transform | `crates/ironhermes-agent/src/nudge.rs` (const + inline `mod tests`) | role-match |
| `crates/ironhermes-agent/src/streaming_scrubber.rs` (NEW) | utility | transform | none — no Rust streaming-scrubber in codebase | no-analog |
| `crates/ironhermes-agent/src/agent_loop.rs` | service | event-driven | self (existing `queue_prefetch` block, lines 1053–1075) | exact |
| `crates/ironhermes-agent/src/context_compressor.rs` | service | transform | self (existing `compress()` method, lines 80–110) | exact |
| `crates/ironhermes-agent/src/lib.rs` | config | — | self (existing `pub mod nudge;` declaration) | exact |
| `crates/ironhermes-cli/src/main.rs` | controller | request-response | self (existing `with_streaming` closure, lines 821–824) | exact |
| `crates/ironhermes-gateway/src/handler.rs` | controller | request-response | self (existing `stream_callback` closure, lines 967–970) | exact |
| `crates/iron_hermes_ui/src/server/ws.rs` | controller | event-driven | self (existing `stream_callback` closure, lines 216–221) | exact |

---

## Pattern Assignments

### `crates/ironhermes-core/src/memory_provider.rs` (trait, request-response)

**Analog:** self — existing defaulted async hook methods on `MemoryProvider`

**Default no-op method pattern** (lines 149–151):
```rust
async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> {
    Ok(())
}
```

**Pattern to copy for `prefetch_with_query`** — add immediately after `queue_prefetch` at line 151, following the exact same shape:
```rust
async fn prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String> {
    Ok(String::new())
}
```

Key observations:
- Takes `&self` (immutable) — matches all other read-path trait methods
- Uses leading `_` on unused params to suppress warnings
- Returns `anyhow::Result<String>`, not `MemoryEntries` — simpler return than `prefetch`
- No `#[async_trait]` attribute needed on the method itself; the trait-level `#[async_trait]` at line 55 covers it
- `MemoryStore`'s `impl MemoryProvider for MemoryStore` block (lines 193–293) does NOT need any change — the default no-op is inherited automatically

**Test pattern to add** — follow `default_hook_methods_return_defaults` test at lines 328–426, which tests all default async hooks. Add `p.prefetch_with_query("q", "sid").await.unwrap()` alongside `p.queue_prefetch("q").await.unwrap()` at line 402.

---

### `crates/ironhermes-core/src/types.rs` (model, transform)

**Analog:** self — existing `ChatMessage` struct and constructor block

**Existing struct** (lines 8–19) — current shape with 5 fields:
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}
```

**Pattern for new field** — add as the LAST field in the struct to minimize diff noise:
```rust
#[serde(skip)]
pub is_recall_context: bool,
```

Note: `#[serde(skip)]` is a new serde attribute pattern in this file (no existing `#[serde(skip)]` instances in `types.rs` — only `#[serde(skip_serializing_if = ...)]` is used). The `skip` attribute omits the field from both serialization and deserialization, so no `#[serde(default)]` is needed (the field is always initialized in code; it never appears on the wire).

**Existing `ChatMessage::system()` constructor** (lines 325–333) — the pattern to mirror for `recall_system()`:
```rust
pub fn system(content: impl Into<String>) -> Self {
    Self {
        role: Role::System,
        content: Some(MessageContent::Text(content.into())),
        tool_calls: None,
        tool_call_id: None,
        name: None,
    }
}
```

**New constructor to add** immediately after `system()` (after line 333):
```rust
pub fn recall_system(content: impl Into<String>) -> Self {
    Self {
        role: Role::System,
        content: Some(MessageContent::Text(content.into())),
        tool_calls: None,
        tool_call_id: None,
        name: None,
        is_recall_context: true,
    }
}
```

The existing constructors (`system`, `user`, `assistant`, `assistant_tool_calls`, `tool_result`) all list all 5 fields explicitly. After adding `is_recall_context`, every existing constructor must add `is_recall_context: false` as the final field (they don't use struct-update syntax, so the compiler will catch any missing field).

---

### `crates/ironhermes-agent/src/memory/manager.rs` (service, request-response)

**Analog:** self — existing `prefetch()` proxy at lines 180–183 and `queue_prefetch()` at lines 201–204

**Existing `prefetch()` read-path proxy** (lines 180–183):
```rust
pub async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries> {
    let p = self.primary.lock().await;
    p.prefetch(session_id).await
}
```

**Existing `queue_prefetch()` hook proxy** (lines 201–204):
```rust
pub async fn queue_prefetch(&self, query: &str) -> anyhow::Result<()> {
    let p = self.primary.lock().await;
    p.queue_prefetch(query).await
}
```

**New `prefetch_with_query()` method** — add in the "Read paths" section (after line 198, before `queue_prefetch`), copying the `prefetch()` proxy shape exactly:
```rust
pub async fn prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String> {
    let p = self.primary.lock().await;
    p.prefetch_with_query(query, session_id).await
}
```

Key: primary-only (D-26/D-28). The mirror is write-only; no fan-out loop needed. This matches every other read proxy in the file.

**Test to add** — follow `read_paths_hit_primary_only` test at lines 601–618. Add a parallel assertion that `prefetch_with_query` returns `Ok("")` on the file provider and that the mock recorder's read_calls does NOT include a new entry.

---

### `crates/ironhermes-agent/src/memory_context.rs` (NEW utility, transform)

**Analog:** `crates/ironhermes-agent/src/nudge.rs` — pure-logic module with a `pub const`, top-level functions, and an inline `#[cfg(test)] mod tests` block

**Module structure to copy from `nudge.rs`** (lines 1–46 and 154–238):

File header comment pattern (lines 1–30):
```rust
//! Phase 34a Plan 01 (MEM-READ-02): memory context sanitization and block building.
//!
//! Ports `sanitize_context` and `build_memory_context_block` from
//! `hermes-agent/agent/memory_manager.py`.
```

Import block pattern (lines 31–35 of nudge.rs):
```rust
use std::sync::Arc;

use ironhermes_core::{ChatMessage, Config};
use ironhermes_tools::ToolRegistry;
use tokio::sync::{Mutex, RwLock};
```

For `memory_context.rs`, the import block will be simpler:
```rust
use regex::Regex;
use std::sync::OnceLock;
```

**Static regex pattern** — use `OnceLock<Regex>` (standard library, no lazy_static needed in Rust 2021+):
```rust
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
```

**Public functions** (ported from Python — RESEARCH.md lines 482–537):
```rust
pub fn sanitize_context(text: &str) -> String {
    let text = internal_context_re().replace_all(text, "");
    let text = internal_note_re().replace_all(&text, "");
    fence_tag_re().replace_all(&text, "").into_owned()
}

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

Note: `\u{2014}` is the em dash (U+2014). The system note must byte-match the Python output or `internal_note_re()` won't strip it on re-wrap (idempotency breaks).

**Test block shape** — copy from `nudge.rs` lines 154–238:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_none() { ... }

    #[test]
    fn double_wrap_stripping_idempotency() { ... }

    // ... 6 more tests
}
```

All 8 tests are `#[test]` (sync) — no `#[tokio::test]` needed since these are pure string transforms.

**regex crate dependency** — before implementing, verify with:
```bash
cargo tree -p ironhermes-agent | grep "^regex"
```
If absent, add to `crates/ironhermes-agent/Cargo.toml`:
```toml
regex = "1"
```

---

### `crates/ironhermes-agent/src/streaming_scrubber.rs` (NEW utility, transform)

**Analog:** None in the Rust codebase. Port directly from Python reference at `/Users/twilson/code/hermes-agent/agent/memory_manager.py`.

**Module structure** — follows same file shape as `nudge.rs` and `memory_context.rs`: file-level doc comment, structs, impl block, `#[cfg(test)] mod tests`.

**Full struct and impl skeleton** (from RESEARCH.md pattern 4, lines 264–283):
```rust
//! Phase 34a Plan 02 (MEM-READ-04): streaming context scrubber.
//!
//! State machine that strips <memory-context>...</memory-context> spans
//! from streaming LLM output deltas. Handles tags split across chunk boundaries.
//! Ported from hermes-agent/agent/memory_manager.py StreamingContextScrubber.

const OPEN_TAG: &str = "<memory-context>";   // 16 chars
const CLOSE_TAG: &str = "</memory-context>"; // 17 chars

pub struct StreamingContextScrubber {
    in_span: bool,
    buf: String,
}

impl StreamingContextScrubber {
    pub fn new() -> Self {
        Self { in_span: false, buf: String::new() }
    }

    pub fn feed(&mut self, text: &str) -> String { /* state machine */ }
    pub fn flush(&mut self) -> String { /* emit or discard tail */ }
    pub fn reset(&mut self) { self.in_span = false; self.buf.clear(); }

    fn max_partial_suffix(buf: &str, tag: &str) -> usize { /* ... */ }
}
```

**flush() semantics** (from RESEARCH.md pitfall 2):
- If `in_span == true`: discard `buf`, return `""`
- If `in_span == false`: return `buf`, clear it

**Case-insensitivity** (pitfall 7): use `.to_lowercase()` for tag comparisons inside `feed()` and `max_partial_suffix()`. The tag constants themselves are lowercase literals.

**Arc<std::sync::Mutex<StreamingContextScrubber>> flush pattern** (RESEARCH.md pitfall 2, recommended approach): The scrubber is shared between the `stream_callback` closure (which calls `feed`) and the call site after the stream completes (which calls `flush`). Use `std::sync::Mutex` (not `tokio::sync::Mutex`) since the callback is a sync `Fn`:
```rust
// At stream setup:
let scrubber = Arc::new(std::sync::Mutex::new(StreamingContextScrubber::new()));
let scrubber_for_flush = Arc::clone(&scrubber);

// In closure:
let scrubber_cb = Arc::clone(&scrubber);
let stream_callback = Box::new(move |delta: &str| {
    let visible = scrubber_cb.lock().unwrap().feed(delta);
    if !visible.is_empty() { /* emit */ }
});

// After stream completes:
let tail = scrubber_for_flush.lock().unwrap().flush();
if !tail.is_empty() { /* emit tail */ }
```

**Test block** — 6 `#[test]` (sync) tests:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]  fn full_block_in_one_delta() { ... }
    #[test]  fn split_open_tag_across_two_deltas() { ... }
    #[test]  fn split_close_tag_across_two_deltas() { ... }
    #[test]  fn partial_tail_held_then_completes() { ... }
    #[test]  fn span_never_closes_flush_returns_empty() { ... }
    #[test]  fn two_complete_blocks_back_to_back() { ... }
}
```

---

### `crates/ironhermes-agent/src/agent_loop.rs` (service, event-driven) — pre-turn injection

**Analog:** self — existing `queue_prefetch` block at lines 1053–1075 (post-turn) and the pressure advisory inject at lines 941–952 (pre-LLM-call)

**Insertion point:** After line 928 (end of compression block), before line 930 (`turns_used += 1`). The current sequence:

```rust
// lines 920–928: compression path
if self.context_engine.is_some() {
    self.pre_chat_compress(&mut messages).await;
} else if let Some(ref compressor) = self.compressor {
    let mut comp = compressor.lock().await;
    comp.compress(&mut messages);
}
// ← INSERT RECALL INJECTION HERE (between line 928 and 930)
turns_used += 1;
```

**Existing pressure-advisory inject pattern** (lines 941–952) — closest structural analog for a pre-LLM message insert:
```rust
if let Some(advisory) = self.check_budget_threshold() {
    // ...
    messages.push(ChatMessage::system(advisory));
}
```

The recall injection is similar but uses `messages.insert(idx, ...)` instead of `push` and uses `ChatMessage::recall_system(block)` instead of `ChatMessage::system(advisory)`.

**Existing `queue_prefetch` block** (lines 1053–1075) — canonical pattern for lock-acquire → call → drop on the `memory_manager`:
```rust
if let Some(ref mgr) = self.memory_manager {
    let query = messages
        .iter()
        .rev()
        .find(|m| m.role == ironhermes_core::Role::User)
        .and_then(|m| m.content_text().map(|s| s.to_string()))
        .unwrap_or_default();
    if !query.is_empty() {
        let mgr = Arc::clone(mgr);
        tokio::spawn(async move {
            let guard = mgr.lock().await;
            if let Err(e) = guard.queue_prefetch(&query).await {
                warn!(error = ?e, "queue_prefetch failed after natural-end break");
            }
        });
    }
}
```

The recall injection uses the same `if let Some(ref mgr) = self.memory_manager` guard and the same rev-find pattern for the last user message. Key difference: the lock must be DROPPED before `messages.insert()` (pitfall 1). Use a scoped block:
```rust
let raw = {
    let guard = mgr.lock().await;
    guard.prefetch_with_query(&user_msg_text, session_id).await
};
// guard dropped here — safe to mutate messages
```

**Full injection block to insert at line 929** (D-02 / D-08 order: retain → fetch → insert):
```rust
// Phase 34a D-02/D-08: pre-turn recall injection.
// Step 1: evict prior recall injection (must come before insert-index scan).
messages.retain(|m| !m.is_recall_context);

// Step 2: fetch and inject (only when memory_manager is wired).
if let Some(ref mgr) = self.memory_manager {
    let session_id = self.session_id.as_deref().unwrap_or("");
    let user_msg_text = messages
        .iter()
        .rev()
        .find(|m| m.role == ironhermes_core::Role::User)
        .and_then(|m| m.content_text().map(|s| s.to_string()))
        .unwrap_or_default();

    if !user_msg_text.is_empty() {
        let raw = {
            let guard = mgr.lock().await;
            guard.prefetch_with_query(&user_msg_text, session_id).await
        };
        if let Ok(raw) = raw {
            if let Some(block) = crate::memory_context::build_memory_context_block(&raw) {
                let insert_idx = messages
                    .iter()
                    .rposition(|m| m.role == ironhermes_core::Role::User)
                    .unwrap_or(messages.len());
                messages.insert(insert_idx, ChatMessage::recall_system(block));
            }
        }
    }
}
```

Note: `messages.rposition()` does not exist on `Vec` — use `.iter().rposition(...)` which is a slice method. Confirmed correct in RESEARCH.md pattern 3 line 238.

---

### `crates/ironhermes-agent/src/context_compressor.rs` (service, transform) — step 0

**Analog:** self — existing `compress()` method starting at line 80

**Current `compress()` opening** (lines 80–89):
```rust
pub fn compress(&mut self, messages: &mut Vec<ChatMessage>) -> bool {
    if !self.should_compress(messages) {
        return false;
    }

    let original_count = messages.len();
    let original_tokens = estimate_messages_tokens(messages);

    // Step 1: Prune old tool results (replace long results with truncated versions)
    self.prune_tool_results(messages);
```

**Step 0 to prepend** (D-03) — insert at line 81, before the `should_compress` check:
```rust
pub fn compress(&mut self, messages: &mut Vec<ChatMessage>) -> bool {
    // Step 0 (Phase 34a D-03): strip ephemeral recall messages before any
    // token estimation — they are re-derivable next turn and must be freed
    // first when context is tight.
    messages.retain(|m| !m.is_recall_context);

    if !self.should_compress(messages) {
        return false;
    }
    // ... rest unchanged
```

The `retain` pattern is already used in agent_loop.rs (the injection block above). No new API — `Vec::retain` with a field predicate.

---

### `crates/ironhermes-agent/src/lib.rs` (config)

**Analog:** self — existing `pub mod nudge;` declaration at line 14

**Pattern to copy:**
```rust
pub mod nudge;
```

**Lines to add** — insert alongside other `pub mod` declarations (alphabetically or after `nudge`):
```rust
pub mod memory_context;
pub mod streaming_scrubber;
```

No re-exports needed in `pub use` block unless the planner decides to surface the types. The RESEARCH.md plan specifies only `pub mod` declarations.

---

### CLI streaming surface: `crates/ironhermes-cli/src/main.rs` (controller, request-response)

**Analog:** self — existing `with_streaming` closure at lines 821–824

**Current closure** (lines 821–824):
```rust
.with_streaming(Box::new(|delta| {
    print!("{}", delta);
    io::stdout().flush().ok();
}))
```

**Pattern after wiring** — scrubber created before the `AgentLoop::new` call (because `with_streaming` is a builder method that moves the closure in):
```rust
let scrubber = Arc::new(std::sync::Mutex::new(
    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new()
));
let scrubber_cb = Arc::clone(&scrubber);
// ... (AgentLoop builder chain)
.with_streaming(Box::new(move |delta| {
    let visible = scrubber_cb.lock().unwrap().feed(delta);
    if !visible.is_empty() {
        print!("{}", visible);
        io::stdout().flush().ok();
    }
}))
```

**flush() call site** — after `agent.run(messages).await` returns (the `run` call is at approximately line 876 in context). The scrubber `Arc` is held by the outer scope:
```rust
let result = agent.run(messages).await?;
// Emit any held tail (e.g. partial tag at end of stream)
let tail = scrubber.lock().unwrap().flush();
if !tail.is_empty() {
    print!("{}", tail);
    io::stdout().flush().ok();
}
```

---

### Gateway streaming surface: `crates/ironhermes-gateway/src/handler.rs` (controller, request-response)

**Analog:** self — existing `stream_callback` closure at lines 967–970

**Current closure** (lines 967–970):
```rust
let stream_tx_clone = stream_tx.clone();
let stream_callback: StreamCallback = Box::new(move |delta: &str| {
    let _ = stream_tx_clone.try_send(delta.to_string());
});
```

**Pattern after wiring** — scrubber created before the closure, `Arc` held by outer scope for flush:
```rust
let stream_tx_clone = stream_tx.clone();
let scrubber = Arc::new(std::sync::Mutex::new(
    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new()
));
let scrubber_cb = Arc::clone(&scrubber);
let stream_callback: StreamCallback = Box::new(move |delta: &str| {
    let visible = scrubber_cb.lock().unwrap().feed(delta);
    if !visible.is_empty() {
        let _ = stream_tx_clone.try_send(visible);
    }
});
```

Note: `try_send` takes `String` not `&str` — `scrubber.feed(delta)` returns `String`, so `visible` is already owned. No `.to_string()` needed.

**flush() call site** — after the `AgentLoop::run` await in `handle_with_multimodal`. The `Arc` is held in the outer function scope. The planner should locate the exact `agent.run(...).await` line and add the flush immediately after:
```rust
let tail = scrubber.lock().unwrap().flush();
if !tail.is_empty() {
    let _ = stream_tx.try_send(tail);
}
```

---

### Web UI streaming surface: `crates/iron_hermes_ui/src/server/ws.rs` (controller, event-driven)

**Analog:** self — existing `stream_callback` closure at lines 216–221

**Current closure** (lines 216–221):
```rust
let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
    Box::new(move |delta: &str| {
        let _ = tx_stream.send(ChatStreamEvent::Delta {
            text: delta.to_string(),
        });
    });
```

**Pattern after wiring** — `tx_stream` is an `mpsc::UnboundedSender<ChatStreamEvent>` (line 209 shows `let (tx, rx) = mpsc::unbounded_channel::<ChatStreamEvent>()`). The scrubber wraps before the send:
```rust
let scrubber = Arc::new(std::sync::Mutex::new(
    ironhermes_agent::streaming_scrubber::StreamingContextScrubber::new()
));
let scrubber_cb = Arc::clone(&scrubber);
let tx_stream_cb = tx_stream.clone();  // tx_stream is already a clone from line 215
let stream_callback: ironhermes_agent::agent_loop::StreamCallback =
    Box::new(move |delta: &str| {
        let visible = scrubber_cb.lock().unwrap().feed(delta);
        if !visible.is_empty() {
            let _ = tx_stream_cb.send(ChatStreamEvent::Delta {
                text: visible,
            });
        }
    });
```

**flush() call site** — after `app_state.run_web_turn(...).await` at line 255 (inside the `tokio::spawn` block). The `scrubber` Arc and `tx` clone are held in the same `async move` block:
```rust
let result = app_state.run_web_turn(...).await;
// Flush scrubber tail
let tail = scrubber.lock().unwrap().flush();
if !tail.is_empty() {
    let _ = tx.send(ChatStreamEvent::Delta { text: tail });
}
```

Note: `tx_stream` in this file is named `tx` at the outer scope (line 209) and cloned as `tx_stream` at line 215 for use in the closure. The flush uses the original `tx` since the closure has consumed `tx_stream`.

---

## Shared Patterns

### `#[serde(skip)]` field annotation
**Source:** Decision D-01 (new pattern in `types.rs` — no existing instance)
**Apply to:** `ChatMessage.is_recall_context` field only
**Pattern:**
```rust
#[serde(skip)]
pub is_recall_context: bool,
```
Omits the field from both serialize and deserialize. The field is always `false` on deserialization of existing payloads. No `#[serde(default)]` needed because the field is never present in wire data.

### Defaulted async trait method (no-op)
**Source:** `crates/ironhermes-core/src/memory_provider.rs`, lines 149–151 (`queue_prefetch`)
**Apply to:** New `prefetch_with_query` trait method
**Pattern:**
```rust
async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> {
    Ok(())
}
```

### Primary-only read proxy
**Source:** `crates/ironhermes-agent/src/memory/manager.rs`, lines 180–183 (`prefetch`)
**Apply to:** New `MemoryManager::prefetch_with_query` proxy
**Pattern:**
```rust
pub async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries> {
    let p = self.primary.lock().await;
    p.prefetch(session_id).await
}
```

### Scoped lock drop before mutation
**Source:** `crates/ironhermes-agent/src/memory/manager.rs`, lines 108–114 (primary write path)
**Apply to:** Agent loop recall injection (pitfall 1 mitigation)
**Pattern:**
```rust
let (outcome, action_target_content) = {
    let mut p = self.primary.lock().await;
    let outcome = p.handle_tool_call(name, args.clone())?;
    let inferred = infer_action_target_content(name, &args);
    (outcome, inferred)
};
// guard dropped here — safe to fire mirror
```

### Rev-scan for last user message
**Source:** `crates/ironhermes-agent/src/agent_loop.rs`, lines 1057–1062 (queue_prefetch block)
**Apply to:** Both the recall injection (finding the insert index) and the pre-turn user-message text extraction
**Pattern:**
```rust
let query = messages
    .iter()
    .rev()
    .find(|m| m.role == ironhermes_core::Role::User)
    .and_then(|m| m.content_text().map(|s| s.to_string()))
    .unwrap_or_default();
```

### Inline `#[cfg(test)] mod tests` block
**Source:** `crates/ironhermes-agent/src/nudge.rs`, lines 154–238
**Apply to:** Both `memory_context.rs` (8 tests) and `streaming_scrubber.rs` (6 tests)
**Pattern:**
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_name() {
        assert!(...);
    }
}
```
All tests in these modules are sync `#[test]` — no `#[tokio::test]` since the functions under test are pure string transforms.

---

## No Analog Found

| File | Role | Data Flow | Reason |
|---|---|---|---|
| `crates/ironhermes-agent/src/streaming_scrubber.rs` | utility | transform | No streaming output scrubber exists in the Rust codebase. Port directly from Python `StreamingContextScrubber` in `/Users/twilson/code/hermes-agent/agent/memory_manager.py`. The module structure (file header + struct + impl + inline tests) follows `nudge.rs` as a structural template, but the logic is a greenfield port. |

---

## Open Pitfalls for Planner

1. **Pitfall 1 (borrow checker) — lock across await:** In the agent_loop injection block, use a scoped block `let raw = { let guard = mgr.lock().await; guard.prefetch_with_query(...).await };` so the `tokio::sync::MutexGuard` is dropped before `messages.insert()`. See RESEARCH.md pitfall 1.

2. **Pitfall 2 (flush placement) — Arc<std::sync::Mutex> pattern:** Each surface must hold an `Arc<std::sync::Mutex<StreamingContextScrubber>>` — one `Arc::clone` moves into the closure for `feed()`, the outer `Arc` is used for `flush()` after the stream completes. The closure is `Fn` (not `FnOnce`), so the scrubber cannot be moved in without the Arc wrapper.

3. **Pitfall 3 (retain before insert-index scan):** `messages.retain(|m| !m.is_recall_context)` MUST fire before `rposition` to find the correct last-user-message index. If a prior recall message sat before the user message, `retain` shifts indices.

4. **Pitfall 5 (ChatMessage no Default impl):** Use the dedicated `ChatMessage::recall_system(block)` constructor rather than struct-update syntax. Every existing constructor must also add `is_recall_context: false` after the new field is added.

5. **Pitfall 6 (sanitize_context regex order):** Run in this order: `internal_context_re` → `internal_note_re` → `fence_tag_re`. Reversing the order leaves system-note content after the tags are stripped.

6. **regex crate dependency:** Run `cargo tree -p ironhermes-agent | grep "^regex"` before implementing `memory_context.rs`. If absent, add `regex = "1"` to `crates/ironhermes-agent/Cargo.toml`.

---

## Metadata

**Analog search scope:** `crates/ironhermes-core/src/`, `crates/ironhermes-agent/src/`, `crates/ironhermes-cli/src/`, `crates/ironhermes-gateway/src/`, `crates/iron_hermes_ui/src/server/`
**Files read:** 11 source files
**Pattern extraction date:** 2026-05-20
