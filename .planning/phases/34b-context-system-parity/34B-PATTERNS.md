# Phase 34b: Context-System Parity — Pattern Map

**Mapped:** 2026-05-16
**Revised:** 2026-05-16 (Blocker 1 — corrected `once_cell::sync::Lazy` to `std::sync::LazyLock` to match the actual codebase pattern in `ssrf.rs` and `skills.rs`; `once_cell` is NOT a workspace dep)
**Files analyzed:** 8 (1 new, 7 modified)
**Analogs found:** 8 / 8

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `crates/ironhermes-agent/src/context_refs.rs` | utility/parser | transform + file-I/O + request-response | `crates/ironhermes-agent/src/nudge.rs` + `crates/ironhermes-tools/src/web_extract.rs` | role-match (no regex parser exists; nudge is closest async utility) |
| `crates/ironhermes-agent/src/context_engine.rs` | trait definition | event-driven | `crates/ironhermes-agent/src/context_engine.rs` (self — add hooks to existing trait) | exact (extend existing file) |
| `crates/ironhermes-agent/src/context_compressor.rs` | utility/state | CRUD | `crates/ironhermes-agent/src/context_compressor.rs` (self — add reset) | exact (extend existing file) |
| `crates/ironhermes-agent/src/summarizing_engine.rs` | service | event-driven | `crates/ironhermes-agent/src/summarizing_engine.rs` (self — patch header) | exact (extend existing file) |
| `crates/ironhermes-agent/src/lib.rs` | config | — | `crates/ironhermes-agent/src/lib.rs` (self — add `pub mod`) | exact |
| `crates/ironhermes-cli/src/main.rs` | controller | request-response | `crates/ironhermes-cli/src/main.rs` nudge wiring (Phase 32) | exact (same 3-surface pattern) |
| `crates/ironhermes-gateway/src/handler.rs` | controller | request-response | `crates/ironhermes-gateway/src/handler.rs` nudge wiring (Phase 32) | exact (same 3-surface pattern) |
| `crates/iron_hermes_ui/src/server/state.rs` | service | request-response | `crates/iron_hermes_ui/src/server/state.rs` nudge wiring (Phase 32) | exact (same 3-surface pattern) |

---

## Pattern Assignments

### `crates/ironhermes-agent/src/context_refs.rs` (new utility/parser, transform + file-I/O)

**Primary analog:** `crates/ironhermes-agent/src/nudge.rs` (async utility module structure)
**Secondary analog:** `crates/ironhermes-tools/src/web_extract.rs` (WebExtractTool usage)

No existing regex parser module exists in the ironhermes-agent crate. The `nudge.rs` module is the closest analog for a self-contained async utility that operates on messages and calls into other services fire-and-forget.

**Module header / doc comment pattern** (`nudge.rs` lines 1-30):
```rust
//! Phase 34b Plan 01 (CTX-REF-01 / CTX-REF-02): @-reference parser + expander.
//!
//! Users can write `@file:foo.rs:10-25`, `@folder:src/`, `@diff`, `@staged`,
//! `@git:N`, `@url:https://...` in chat messages. Tokens are parsed pre-turn,
//! expanded into attached-context blocks, and stripped from the inline message.
//! Sensitive-path blocklist + 50%/25% token budget enforced.
//!
//! ## Security
//! - `allowed_root` is fixed to cwd (D-04): @file: and @folder: cannot escape.
//! - SENSITIVE_PATHS blocklist is a second independent defense layer.
//! - @url: expansion uses WebExtractTool with use_llm_processing=true (D-01);
//!   falls back to raw HTTP on LLM failure with a warning (D-02).
```

**Imports pattern** (mirror of `nudge.rs` lines 31-40, adapted; uses `std::sync::LazyLock` — `once_cell` is NOT a workspace dep, RESEARCH §10 outdated assumption):
```rust
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::Result;
use regex::Regex;
use tokio::process::Command;

use ironhermes_core::ChatMessage;
use ironhermes_tools::web_extract::WebExtractTool;
use crate::context_compressor::estimate_tokens;
```

**Static regex pattern** — use the `std::sync::LazyLock` + `regex` pattern. `LazyLock` is stable since Rust 1.80 and is the established codebase idiom (see `crates/ironhermes-core/src/ssrf.rs:19` and `crates/ironhermes-core/src/skills.rs:23`). **Do NOT use `once_cell::sync::Lazy` — `once_cell` is not in the workspace deps; adding it would be a new dependency for no benefit.**
```rust
static REFERENCE_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"@(?:(?P<simple>diff|staged)\b|(?P<kind>file|folder|git|url):(?P<value>(?:`[^`\n]+`|"[^"\n]+"|\x27[^\x27\n]+\x27)(?::\d+(?:-\d+)?)?|\S+))"#
    ).unwrap()
});
```
Note: Rust `regex` crate has no lookbehind — implement post-match position check (RESEARCH.md §8.1):
```rust
for m in REFERENCE_PATTERN.find_iter(message) {
    let start = m.start();
    if start > 0 {
        let prev_char = message[..start].chars().last().unwrap();
        if prev_char.is_alphanumeric() || prev_char == '_' || prev_char == '/' {
            continue; // lookbehind rejection
        }
    }
    // process match
}
```

**Struct/enum definitions** (from RESEARCH.md §2.2 — no existing analog, use Python parity):
```rust
#[derive(Debug, Clone, PartialEq)]
pub enum RefKind { Diff, Staged, File, Folder, Git, Url }

#[derive(Debug, Clone)]
pub struct ContextReference {
    pub raw: String,
    pub kind: RefKind,
    pub target: String,
    pub start: usize,
    pub end: usize,
    pub line_start: Option<u32>,
    pub line_end: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct ContextReferenceResult {
    pub message: String,
    pub original_message: String,
    pub references: Vec<ContextReference>,
    pub warnings: Vec<String>,
    pub injected_tokens: usize,
    pub expanded: bool,
    pub blocked: bool,
}
```

**Error handling pattern** — `anyhow::Result` throughout (matches `agent_loop.rs` line 1 import, `nudge.rs` pattern):
```rust
// Per-reference expansion errors are captured as warning strings, not hard errors.
// Only the async fn signature returns anyhow::Result.
pub async fn preprocess_context_references_async(
    message: &str,
    context_length: usize,
    allowed_root: &Path,
    web_extract_tool: Option<Arc<WebExtractTool>>,
) -> Result<ContextReferenceResult> {
    // ...
    // Per-ref errors:
    Err(e) => {
        result.warnings.push(format!("{}: {}", ref_.raw, e));
    }
}
```

**WebExtractTool calling pattern** (`web_extract.rs` lines 174-195, confirmed):
```rust
// D-01: call with use_llm_processing: true
let args = serde_json::json!({
    "urls": [url],
    "use_llm_processing": true,
    "format": "markdown"
});
match tool.execute(args).await {
    Ok(json_str) => { /* parse ExtractionResult */ }
    Err(e) => {
        // D-02: fall back to raw HTTP
        let fallback_args = serde_json::json!({
            "urls": [url],
            "use_llm_processing": false,
            "format": "markdown"
        });
        result.warnings.push(format!("@url:{url}: LLM processing failed ({e}), using raw content"));
        tool.execute(fallback_args).await?
    }
}
```

**Async subprocess pattern** — use `tokio::process::Command` (not `std::process::Command`) for `@diff`, `@staged`, `@git:N`, `@folder:` rg:
```rust
// From RESEARCH.md §8.2 — avoids blocking tokio thread
let output = tokio::process::Command::new("git")
    .args(["diff"])
    .output()
    .await
    .map_err(|e| anyhow::anyhow!("git diff failed: {e}"))?;
let content = String::from_utf8_lossy(&output.stdout).to_string();
```

**Token budget enforcement pattern** (from RESEARCH.md §1.3):
```rust
let hard_limit = (context_length / 2).max(1);
let soft_limit = (context_length / 4).max(1);

if injected_tokens > hard_limit {
    return Ok(ContextReferenceResult {
        message: original_message.to_string(),
        blocked: true,
        warnings: vec![format!(
            "@ context injection refused: {} tokens exceeds the 50% hard limit ({}).",
            injected_tokens, hard_limit
        )],
        ..Default::default()
    });
}
if injected_tokens > soft_limit {
    warnings.push(format!(
        "@ context injection warning: {} tokens exceeds the 25% soft limit ({}).",
        injected_tokens, soft_limit
    ));
}
```

**Output assembly pattern** (from RESEARCH.md §1.4):
```rust
// 1. Start with stripped message
let mut result_msg = stripped;
// 2. Append warnings block
if !warnings.is_empty() {
    result_msg.push_str("\n\n--- Context Warnings ---\n");
    for w in &warnings {
        result_msg.push_str(&format!("- {w}\n"));
    }
}
// 3. Append context blocks
if !blocks.is_empty() {
    result_msg.push_str("\n\n--- Attached Context ---\n\n");
    result_msg.push_str(&blocks.join("\n\n"));
}
// 4. Strip trailing whitespace
let result_msg = result_msg.trim().to_string();
```

**Test module pattern** (from `context_engine.rs` lines 321-348 and `context_compressor.rs` lines 241-276):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_diff() { /* sync — no async needed */ }

    #[test]
    fn test_parse_quoted_path() { /* ... */ }

    #[tokio::test]
    async fn test_expand_file() {
        // use tempfile for isolation
    }

    #[tokio::test]
    async fn test_expand_url_stub() {
        // pass a mock url_fetcher
    }
}
```

---

### `crates/ironhermes-agent/src/context_engine.rs` (trait extension, event-driven)

**Analog:** Self — extend existing file. The `check_pressure` default no-op (lines 63-65) is the exact template for all 5 new hooks.

**Existing default no-op pattern to copy** (`context_engine.rs` lines 57-66):
```rust
/// Phase 18 Plan 06: Run only the pressure-warning channel without
/// performing any destructive compression. Agent loop calls this when
/// the token ratio is below the compression threshold so the 85% warning
/// can still fire on the pre-compression slope.
///
/// Default implementation is a no-op; both shipped engines override it.
async fn check_pressure(&self, _stats: &ContextStats) -> bool {
    false
}
```

**Five new hooks to add** (RESEARCH.md §3.2 — follow exact same doc + default pattern):
```rust
/// Called when a new conversation session begins.
/// Default: no-op.
fn on_session_start(&self, _session_id: &str) {}

/// Called at real session end (CLI exit, /reset, gateway expiry).
/// Default: no-op.
fn on_session_end(&self, _session_id: &str, _messages: &[ChatMessage]) {}

/// Called on /new or /reset. Override to clear per-session counters.
/// Default: no-op.
fn on_session_reset(&self) {}

/// Called after every LLM turn with aggregated token usage.
/// Default: no-op.
fn update_from_response(&self, _usage: &AggregatedUsage) {}

/// Called when the user switches model or on fallback activation.
/// Default: no-op.
fn update_model(&self, _model: &str, _context_length: usize, _base_url: Option<&str>) {}

/// Quick check: is there content worth compacting?
/// Default: true (conservative — always attempt compression).
fn has_content_to_compress(&self, _messages: &[ChatMessage]) -> bool {
    true
}
```

**Key constraints from RESEARCH.md §3.2:**
- New hooks are synchronous `fn` (not `async fn`) — no `#[async_trait]` needed for them
- `&self` not `&mut self` — any counter state uses `AtomicUsize` or `Mutex<T>` (interior mutability)
- `AggregatedUsage` type is already re-exported from `lib.rs` line 26: `pub use agent_loop::{AgentLoop, AgentResult, AggregatedUsage}`
- `ChatMessage` is already imported at line 2 of `context_engine.rs`

**Trait signature context** (existing `context_engine.rs` lines 47-66):
```rust
#[async_trait]
pub trait ContextEngine: Send + Sync + 'static {
    async fn compress(
        &self,
        messages: &mut Vec<ChatMessage>,
        stats: ContextStats,
    ) -> Result<CompressionOutcome, ContextError>;
    fn threshold(&self) -> f32;
    fn mode(&self) -> CompressionMode;

    async fn check_pressure(&self, _stats: &ContextStats) -> bool {
        false
    }
    // ← NEW HOOKS GO HERE (after check_pressure)
}
```

---

### `crates/ironhermes-agent/src/context_compressor.rs` (utility state, CRUD)

**Analog:** Self — add `on_session_reset` override. The `compression_count` field (line 44) is a bare `usize`. **Python-parity token-counter fields (`last_prompt_tokens`, `last_completion_tokens`, `last_total_tokens`) do NOT currently exist on the Rust struct — Plan 34b-02 Task 1 adds them as `AtomicUsize` fields and zeroes them in `reset()` to satisfy CONTEXT.md `<specifics>`.**

**Existing `ContextCompressor` struct** (lines 39-45):
```rust
pub struct ContextCompressor {
    context_length: usize,
    threshold_percent: f64,
    protect_first_n: usize,
    protect_last_tokens: usize,
    compression_count: usize,   // ← must be cleared by reset()
    // Plan 34b-02 Task 1 adds these three fields (AtomicUsize for interior mutability under &self):
    // last_prompt_tokens: AtomicUsize,
    // last_completion_tokens: AtomicUsize,
    // last_total_tokens: AtomicUsize,
}
```

**Key insight from RESEARCH.md §8.3:** `LocalPruningEngine::compress` creates a **new** `ContextCompressor::new(...)` every call (confirmed at `context_engine.rs` lines 252-256). Therefore `LocalPruningEngine.on_session_reset` is a genuine no-op — no persistent compressor accumulates state across turns.

**`reset` method pattern** — place as inherent method on `ContextCompressor` (since it's not a trait implementor); the `LocalPruningEngine` trait impl is a no-op:
```rust
impl ContextCompressor {
    // ... existing methods ...

    /// Reset per-session counters. Called by ContextEngine::on_session_reset overrides.
    /// Per CONTEXT.md <specifics>, zeroes compression_count AND the three Python-parity
    /// token counters (added in Plan 34b-02 Task 1 if not present).
    pub fn reset(&mut self) {
        self.compression_count = 0;
        // Plan 34b-02 Task 1 adds these three (using Atomic store with Ordering::Relaxed):
        // self.last_prompt_tokens.store(0, Ordering::Relaxed);
        // self.last_completion_tokens.store(0, Ordering::Relaxed);
        // self.last_total_tokens.store(0, Ordering::Relaxed);
    }
}
```

**SUMMARY_PREFIX / compaction message** to patch (existing at `drop_middle_messages` lines 180-183):
```rust
// EXISTING (lines 180-183) — missing memory-authority reminder:
let summary = format!(
    "[CONTEXT COMPACTED] {} earlier messages were removed to save context space. \
     The conversation continues from the most recent messages below.",
    dropped_count
);

// PATCHED — add memory-authority reminder per CONTEXT.md <specifics>:
let summary = format!(
    "[CONTEXT COMPACTED] {} earlier messages were removed to save context space. \
     The conversation continues from the most recent messages below. {}",
    dropped_count, MEMORY_AUTHORITY_REMINDER
);
```

**Test pattern** (copy from `context_compressor.rs` lines 241-276):
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compaction_header_contains_memory_authority() {
        // Trigger drop_middle_messages by building a large message vec
        // and asserting the compaction message contains the reminder.
        let header = /* extract from compacted messages[protect_first_n] */;
        assert!(header.contains("MEMORY.md"), "header must mention MEMORY.md");
        assert!(header.contains("ALWAYS authoritative"), "header must include authority reminder");
    }

    #[test]
    fn test_context_compressor_reset_zeroes_counter() {
        // Per CONTEXT.md <specifics>: all four counters MUST be zero after reset().
        let mut compressor = ContextCompressor::new(/* ... */);
        compressor.compression_count = 5;
        // (Plan 34b-02 Task 1 also seeds the three token-counter AtomicUsize fields.)
        compressor.reset();
        assert_eq!(compressor.compression_count, 0);
        // assert_eq!(compressor.last_prompt_tokens.load(Ordering::Relaxed), 0);
        // assert_eq!(compressor.last_completion_tokens.load(Ordering::Relaxed), 0);
        // assert_eq!(compressor.last_total_tokens.load(Ordering::Relaxed), 0);
    }
}
```

---

### `crates/ironhermes-agent/src/summarizing_engine.rs` (service, event-driven)

**Analog:** Self — add `on_session_reset` override + patch `make_history_message`.

**Existing constants to reference** (lines 28-45):
```rust
pub const HISTORY_SENTINEL: &str = "[CONTEXT HISTORY]";
pub const HISTORY_NAME: &str = "context_history";
pub const COMPLETED_TOOLS_SENTINEL: &str =
    "Tool executions already completed; do NOT re-call unless the user explicitly asks again.";
```

**`make_history_message` function** (lines 54-70) — SUMMARY_PREFIX patch target:
```rust
fn make_history_message(summary_body: &str) -> ChatMessage {
    let truncated = if summary_body.len() > HISTORY_SUMMARY_MAX_CHARS {
        &summary_body[..HISTORY_SUMMARY_MAX_CHARS]
    } else {
        summary_body
    };
    ChatMessage {
        role: Role::System,
        content: Some(MessageContent::Text(format!(
            "{}\n{}",    // ← HISTORY_SENTINEL + body
            HISTORY_SENTINEL, truncated
        ))),
        tool_calls: None,
        tool_call_id: None,
        name: Some(HISTORY_NAME.into()),
    }
}
```

**Add `MEMORY_AUTHORITY_REMINDER` constant and patch `make_history_message`:**
```rust
/// Memory-authority reminder injected into every compaction header (Phase 34b).
/// Prevents the model from deprioritizing memory content due to compaction notes.
pub const MEMORY_AUTHORITY_REMINDER: &str =
    "Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS \
     authoritative — never ignore or deprioritize memory content due to this compaction note.";

fn make_history_message(summary_body: &str) -> ChatMessage {
    // ...
    content: Some(MessageContent::Text(format!(
        "{}\n{}\n{}",
        HISTORY_SENTINEL, MEMORY_AUTHORITY_REMINDER, truncated
    ))),
    // ...
}
```

**`on_session_reset` for `SummarizingEngine`** — from RESEARCH.md §5.3, the engine stores its running summary IN the message list (via `locate_history_segment`). When `messages.truncate(1)` fires on CLI `/new`, the `[CONTEXT HISTORY]` segment is wiped automatically. Therefore `on_session_reset` is a genuine no-op for `SummarizingEngine`:
```rust
#[async_trait]
impl ContextEngine for SummarizingEngine {
    // ... existing impls ...

    /// on_session_reset is a no-op for SummarizingEngine: the running summary
    /// is stored IN the message list as a pinned [CONTEXT HISTORY] system message.
    /// Session reset clears the message list (messages.truncate(1) in CLI /new),
    /// which automatically removes the history segment.
    fn on_session_reset(&self) {}
}
```

**`SummarizingEngine` impl block location** (lines 11-25 imports, then struct + impl follow):
```rust
use async_trait::async_trait;
use ironhermes_core::{ChatMessage, MessageContent, Role};
use ironhermes_hooks::{HookEvent, HookEventKind, HookRegistry};
use std::sync::Arc;
use tokio::sync::Mutex as TokioMutex;

use crate::context_engine::{
    CompressionMode, CompressionOutcome, ContextEngine, ContextError, ContextStats,
    LocalPruningEngine,
};
```

---

### `crates/ironhermes-agent/src/lib.rs` (config, module registration)

**Analog:** Self — add `pub mod context_refs` following existing module list pattern.

**Existing module list pattern** (lines 1-23):
```rust
pub mod agent_loop;
pub mod agent_wiring;
// ... (alphabetical order) ...
pub mod context_compressor;
pub mod context_engine;
pub mod context_loader;
// ← INSERT: pub mod context_refs;   (alphabetical: after context_loader, before engine_factory)
pub mod engine_factory;
```

**Re-export pattern** (lines 26-43) — add public exports if needed:
```rust
pub use agent_loop::{AgentLoop, AgentResult, AggregatedUsage};
// Add if the preprocessor function needs to be callable from surfaces:
// pub use context_refs::{ContextReferenceResult, preprocess_context_references_async};
```

---

### `crates/ironhermes-cli/src/main.rs` (controller, request-response — 3-surface wiring)

**Analog:** Phase 32 nudge wiring in the same file. The pattern for adding a new pre-turn hook and post-turn call is established.

**Pre-turn preprocessing insertion point** — user message is pushed to `messages` at line 1764 (`messages.push(user_msg)`), then `run_agent_turn` is called at line 1774. Insert `preprocess_context_references_async` BETWEEN those two points:
```rust
// AFTER: messages.push(user_msg);  (line 1764)
// BEFORE: Box::pin(run_agent_turn(...))  (line 1774)

// Phase 34b Plan 01: expand @-references in the user message pre-turn.
let allowed_root = config.agent.terminal.cwd.clone()
    .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
let ref_result = ironhermes_agent::context_refs::preprocess_context_references_async(
    &input,
    context_length,
    &allowed_root,
    web_extract_tool.clone(),  // Option<Arc<WebExtractTool>> threaded from build
).await.unwrap_or_else(|e| {
    tracing::warn!(error = %e, "context_refs preprocessing failed");
    ironhermes_agent::context_refs::ContextReferenceResult::passthrough(input.clone())
});
if ref_result.expanded {
    // Replace the last user message with the expanded version
    if let Some(last) = messages.last_mut() {
        *last = ChatMessage::user(&ref_result.message);
    }
}
// Surface warnings (inline, before the turn):
for warning in &ref_result.warnings {
    println!("{}", format!("Warning: {warning}").yellow());
}
```

**`on_session_start` wiring** — fires once at REPL start, after `session_id` is generated (line 1110):
```rust
// After: let session_id = uuid::Uuid::new_v4().to_string();  (line 1110)
// Phase 34b Plan 02: lifecycle hook — session start.
if let Some(ref engine) = context_engine_ref {
    engine.on_session_start(&session_id);
}
```

**`on_session_reset` wiring** — in `CommandResult::ClearSession` arm (lines 1597-1601):
```rust
CommandResult::ClearSession(output) => {
    messages.truncate(1); // Keep system message
    // Phase 34b Plan 02: clear per-session counters on /new.
    if let Some(ref engine) = context_engine_ref {
        engine.on_session_reset();
    }
    println!("{}", output.dimmed());
    continue;
}
// CommandResult::ResetTerminal does NOT call on_session_reset (visual TTY only — CLR-1)
```

**`update_from_response` wiring** — the post-turn nudge block (lines 2129-2161) is the exact insertion point. Add immediately after `drop(run_fut)` (line 2097) when `response.is_some()`:
```rust
// After: drop(run_fut);  (line 2097)
// Phase 34b Plan 02: update context engine with token usage from this turn.
if let (Some(ref engine), Some(ref usage)) = (&context_engine_ref, &last_turn_usage) {
    engine.update_from_response(usage);
}
```
`last_turn_usage: Option<AggregatedUsage>` comes from `run_agent_turn` return value (which wraps `AgentResult.total_usage`).

**Existing nudge fire-and-forget pattern** (lines 2137-2161) — copy this exact shape for `update_from_response`:
```rust
// Phase 32 wiring — established pattern for post-turn actions:
if response.is_some() && nudge_interval > 0 && config.memory.memory_enabled {
    turns_since_nudge += 1;
    if turns_since_nudge >= nudge_interval {
        turns_since_nudge = 0;
        if let Some(ref mgr) = memory_manager {
            let mgr_clone = Arc::clone(mgr);
            let client_clone = client.clone();
            let messages_snapshot = messages.clone();
            let config_clone = config.clone();
            tokio::spawn(async move {
                ironhermes_agent::nudge::spawn_nudge_review(
                    messages_snapshot, mgr_clone, client_clone, &config_clone,
                ).await;
            });
        }
    }
}
```

---

### `crates/ironhermes-gateway/src/handler.rs` (controller, request-response — 3-surface wiring)

**Analog:** Phase 32 nudge wiring in the same file (lines 1055-1100+).

**Existing `context_engine` import** (line 11 — already imported):
```rust
use ironhermes_agent::context_engine::{ContextEngine, ContextStats};
```

**`on_session_start` wiring** — in `CoreCommandResult::NewSession` arm (line 466) after the intro agent run:
```rust
CoreCommandResult::NewSession { .. } => {
    // ... existing store.remove(&session_key) ...
    // Phase 34b Plan 02: fire on_session_start when a new session key is first allocated.
    if let Some(ref engine) = self.context_engine {
        engine.on_session_start(&session_key.to_string_key());
    }
}
```

**`on_session_reset` wiring** — same `NewSession` arm before/after `store.remove`:
```rust
// /new: clear entire session history
let had_session = {
    let mut store = self.session_store.write().await;
    store.remove(&session_key).is_some()
};
// Phase 34b Plan 02: reset per-session counters.
if let Some(ref engine) = self.context_engine {
    engine.on_session_reset();
}
```

**`preprocess_context_references_async` wiring** — in `run_agent` after `messages` is built, before `agent.run(messages)` (line 1024). Exact insertion follows the `messages_for_nudge = messages.clone()` pattern (line 1023):
```rust
// EXISTING (line 1023):
let messages_for_nudge = messages.clone();
// INSERT BEFORE line 1024 (agent.run):
// Phase 34b Plan 01: expand @-references pre-turn.
let messages = preprocess_and_replace(messages, &self.context_refs_config).await;
let agent_result = agent.run(messages).await;
```

**`update_from_response` wiring** — in the `Ok(result)` arm of `match agent_result` (line 1036), immediately after `info!("Agent completed")`:
```rust
Ok(result) => {
    info!("Agent completed, turns_used={}", result.turns_used);
    // Phase 34b Plan 02: update context engine with token usage.
    if let Some(ref engine) = self.context_engine {
        engine.update_from_response(&result.total_usage);
    }
    // ... existing hook firing (line 1041+) ...
}
```

---

### `crates/iron_hermes_ui/src/server/state.rs` (service, request-response — 3-surface wiring)

**Analog:** Phase 32 nudge wiring in the same file (lines 171-202). The `nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>` field (line 40) is the exact template for any new per-session state.

**`on_session_start` wiring** — in `ensure_web_session` after `create_session` succeeds (lines 131-139):
```rust
pub fn ensure_web_session(&self, session_id: &str) -> Result<()> {
    let mut store = self.state_store.lock().unwrap();
    if store.get_session(session_id)?.is_none() {
        store.create_session(session_id, ...)?;
        // Phase 34b Plan 02: fire on_session_start for new web sessions.
        drop(store); // release lock before calling engine
        if let Some(ref engine) = self.context_engine {
            engine.on_session_start(session_id);
        }
    }
    Ok(())
}
```

**`preprocess_context_references_async` wiring** — in `run_web_turn` (line 144) before `agent.run(messages)` (line 161). Insert between `build_messages_for_turn` (line 152) and `agent.run` (line 161):
```rust
pub async fn run_web_turn(&self, session_id: &str, user_input: &str, ...) -> Result<AgentResult> {
    let messages = self.build_messages_for_turn(session_id, user_input).await?;
    let messages_snapshot = messages.clone();  // existing line 156

    // Phase 34b Plan 01: expand @-references pre-turn.
    let messages = self.preprocess_refs(messages, user_input).await;

    let mut agent = self.build_agent_loop(stream_callback, tool_progress_callback)?;
    // ... existing code ...
    let result = agent.run(messages).await?;
    // ...
}
```

**`update_from_response` wiring** — after `agent.run` returns (line 161), before nudge block:
```rust
let result = agent.run(messages).await?;
// Phase 34b Plan 02: update context engine with token usage.
if let Some(ref engine) = self.context_engine {
    engine.update_from_response(&result.total_usage);
}
```

**`on_session_reset` wiring** — web UI does not currently expose a new-chat trigger. Per RESEARCH.md §8.7, this requires either a new `POST /api/sessions/{id}/reset` endpoint or a WebSocket message type. The planner must check `ws.rs` for a `new_chat`/`reset` message type and scope accordingly. The hook call when triggered follows the same pattern:
```rust
// When new-chat fires:
if let Some(ref engine) = self.context_engine {
    engine.on_session_reset();
}
```

**`Arc<dyn ContextEngine>` field pattern** — add to `AppState` struct following `nudge_turns` field pattern (line 40):
```rust
pub struct AppState {
    // ... existing fields ...
    pub nudge_turns: Arc<std::sync::Mutex<HashMap<String, u32>>>,
    // Phase 34b Plan 02: context engine for lifecycle hooks.
    pub context_engine: Option<Arc<dyn ironhermes_agent::context_engine::ContextEngine>>,
}
```

---

### `crates/ironhermes-agent/src/agent_loop.rs` (service — add `update_from_response` call site)

**Pattern:** After `AgentLoop::run` returns in each surface, call `engine.update_from_response(&result.total_usage)`. The `AggregatedUsage` type is already in scope (lines 94-107):
```rust
#[derive(Debug, Default)]
pub struct AggregatedUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}
```

The `total_usage` field on `AgentResult` (line 63) is the source for every `update_from_response` call. No changes to `agent_loop.rs` itself are needed — the hook call sites are in the three surfaces.

---

## Shared Patterns

### Default No-Op Trait Hooks
**Source:** `crates/ironhermes-agent/src/context_engine.rs` lines 57-66
**Apply to:** All 5 new lifecycle hooks on `ContextEngine` trait
```rust
async fn check_pressure(&self, _stats: &ContextStats) -> bool {
    false
}
// ← Template: synchronous hooks follow same shape with `fn` instead of `async fn`:
fn on_session_reset(&self) {}
```

### 3-Surface Wiring (Pre-Turn Hook)
**Source:** `crates/ironhermes-cli/src/main.rs` lines 2129-2161 (nudge fire pattern)
**Apply to:** `preprocess_context_references_async` insertion at all 3 surfaces
- CLI: between `messages.push(user_msg)` and `Box::pin(run_agent_turn(...))`
- Gateway: between `messages_for_nudge = messages.clone()` and `agent.run(messages)`
- Web UI: between `build_messages_for_turn` and `agent.run(messages)`

### Post-Turn `update_from_response` Call
**Source:** `crates/ironhermes-gateway/src/handler.rs` line 1024 (`agent_result = agent.run(messages).await`)
**Apply to:** All 3 surfaces, in the `Ok(result)` arm
```rust
Ok(result) => {
    // call engine.update_from_response(&result.total_usage) here
}
```

### Interior Mutability for Hook State
**Source:** `crates/ironhermes-agent/src/pressure_warning.rs` (PressureTracker uses `Arc<Mutex<HashMap>>`)
**Apply to:** Any counter fields that `on_session_reset` needs to clear
- Hooks are `&self` → counters must be `AtomicUsize` or `Mutex<T>`
- Pattern: `Arc<std::sync::Mutex<...>>` for std-context fields, `Arc<TokioMutex<...>>` for async fields
- The three new `last_*_tokens` fields on `ContextCompressor` (Plan 34b-02 Task 1) use `AtomicUsize` — consistent with this pattern and avoids changing `&self` callers.

### Error Handling
**Source:** `crates/ironhermes-agent/src/nudge.rs` lines 117-122
```rust
match agent.run(augmented).await {
    Ok(_result) => { tracing::info!("..."); }
    Err(e) => { tracing::warn!(error = %e, "... failed"); }
}
```
**Apply to:** All fallible operations inside `preprocess_context_references_async` — per-reference errors become warning strings, not hard errors. Only catastrophic failures bubble up as `anyhow::Result::Err`.

### `tokio::spawn` Fire-and-Forget
**Source:** `crates/iron_hermes_ui/src/server/state.rs` lines 191-199
```rust
tokio::spawn(async move {
    ironhermes_agent::nudge::spawn_nudge_review(
        messages_snapshot, mgr_clone, client_clone, &config_clone,
    ).await;
});
```
**Apply to:** `preprocess_context_references_async` is NOT fire-and-forget — it runs inline (awaited) before the agent turn. This pattern does NOT apply to @-ref preprocessing.

### Tracing
**Source:** `crates/ironhermes-agent/src/context_compressor.rs` lines 98-106
```rust
info!(
    compression = self.compression_count,
    messages_before = original_count,
    messages_after = messages.len(),
    "Context compressed"
);
```
**Apply to:** `context_refs.rs` — log reference count, token count, blocked/warned status at `tracing::info!` level.

---

## No Analog Found

| File | Role | Data Flow | Reason |
|------|------|-----------|--------|
| (none) | — | — | All files have analogs. `context_refs.rs` has a partial match via `nudge.rs` module structure + `web_extract.rs` tool calling convention. |

---

## Metadata

**Analog search scope:** `crates/ironhermes-agent/src/`, `crates/ironhermes-cli/src/`, `crates/ironhermes-gateway/src/`, `crates/iron_hermes_ui/src/server/`, `crates/ironhermes-tools/src/`
**Files scanned:** ~12 source files read in full or targeted sections
**Pattern extraction date:** 2026-05-16
**Pattern correction date:** 2026-05-16 (Blocker 1 — `LazyLock` confirmed, `once_cell` removed)

**Key confirmed facts:**
- `AggregatedUsage` is defined at `agent_loop.rs` lines 94-99 and re-exported in `lib.rs` line 26
- **Static-init pattern is `std::sync::LazyLock` (Rust 1.80+) — confirmed in `crates/ironhermes-core/src/ssrf.rs:14,19` and `crates/ironhermes-core/src/skills.rs:3,23`. `once_cell` is NOT a workspace dep (verified via grep on root Cargo.toml + crate Cargo.toml — zero matches). The earlier RESEARCH.md §10 note about `once_cell` was an outdated assumption.**
- `check_pressure` default no-op (context_engine.rs line 63) is the exact template for all 5 new hooks
- `LocalPruningEngine` creates a fresh `ContextCompressor::new(...)` every compress call (context_engine.rs lines 252-256) → `on_session_reset` is a genuine no-op for `LocalPruningEngine`
- `SummarizingEngine` stores running summary IN the message list → `on_session_reset` is also a genuine no-op (messages.truncate(1) handles it)
- Web UI `on_session_reset` trigger does not currently exist — planner must define scope
- `WebExtractTool::execute` takes `serde_json::Value` args with `urls: Vec<String>`, `use_llm_processing: bool`, `format: &str`
- **`ContextCompressor` struct (line 39-45) holds only `compression_count: usize` today; Python-parity fields `last_prompt_tokens`, `last_completion_tokens`, `last_total_tokens` do NOT exist on the Rust struct and are added by Plan 34b-02 Task 1 as `AtomicUsize` fields to satisfy CONTEXT.md `<specifics>` without requiring executor judgment.**
