---
phase: 34b-context-system-parity
plan: 02
type: execute
wave: 2
depends_on:
  - 34b-01
files_modified:
  - crates/ironhermes-agent/src/context_engine.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/src/pressure_warning.rs
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-gateway/src/handler.rs
  - crates/iron_hermes_ui/src/server/state.rs
autonomous: true
requirements:
  - CTX-ENG-01
  - CTX-ENG-02
  - CTX-ENG-03
  - CTX-ENG-04
tags:
  - context-engine
  - lifecycle-hooks
  - compaction
  - memory-authority
  - 3-surface-wiring

must_haves:
  truths:
    - "The ContextEngine trait exposes 5 new lifecycle hooks (on_session_start, on_session_end, on_session_reset, update_from_response, update_model, has_content_to_compress) — existing implementors continue to work via default no-ops."
    - "Calling `engine.on_session_reset()` on a SummarizingEngine clears its embedded ContextCompressor's compression_count and any per-session PressureTracker entry."
    - "The compaction header injected by ContextCompressor.drop_middle_messages and the [CONTEXT HISTORY] segment built by SummarizingEngine.make_history_message both contain the exact strings `MEMORY.md` and `ALWAYS authoritative`."
    - "All three surfaces fire `on_session_start` when a new session is created and `on_session_reset` when /new or /reset clears messages (Web UI: `on_session_reset` is a documented stub — no new-chat trigger exists today)."
    - "After every successful AgentLoop::run, the engine receives `update_from_response(&result.total_usage)` so token counters stay live for compression decisions."
    - "At REPL start, CLI calls `engine.update_model(&model, context_length, Some(base_url))` once so the engine knows the configured endpoint."
  artifacts:
    - path: "crates/ironhermes-agent/src/context_engine.rs"
      provides: "ContextEngine trait extended with 6 default-no-op lifecycle hooks"
      contains: "fn on_session_start"
    - path: "crates/ironhermes-agent/src/context_compressor.rs"
      provides: "ContextCompressor::reset() inherent method; patched compaction header"
      contains: "MEMORY.md"
    - path: "crates/ironhermes-agent/src/summarizing_engine.rs"
      provides: "MEMORY_AUTHORITY_REMINDER constant; patched make_history_message; on_session_reset override on SummarizingEngine"
      contains: "MEMORY_AUTHORITY_REMINDER"
    - path: "crates/ironhermes-agent/src/pressure_warning.rs"
      provides: "PressureTracker::reset_session(&self, session_id: &str)"
      contains: "pub fn reset_session"
  key_links:
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "ContextEngine::on_session_start / on_session_reset / update_from_response / update_model"
      via: "engine.method() calls at REPL start, /new, /reset, after run_agent_turn"
      pattern: "on_session_start\\|on_session_reset\\|update_from_response\\|update_model"
    - from: "crates/ironhermes-gateway/src/handler.rs"
      to: "ContextEngine::on_session_start / on_session_reset / update_from_response"
      via: "engine.method() calls in NewSession arm + after agent.run"
      pattern: "on_session_start\\|on_session_reset\\|update_from_response"
    - from: "crates/iron_hermes_ui/src/server/state.rs"
      to: "ContextEngine::on_session_start / update_from_response"
      via: "engine.method() calls in ensure_web_session + after agent.run"
      pattern: "on_session_start\\|update_from_response"
    - from: "crates/ironhermes-agent/src/context_compressor.rs"
      to: "summary message body"
      via: "compaction header format! string"
      pattern: "MEMORY.md.*ALWAYS authoritative\\|ALWAYS authoritative.*MEMORY.md"
---

<objective>
Close the parity gap with Python's `context_engine.py` and `context_compressor.py`:
1. Add 5 lifecycle hooks (`on_session_start`, `on_session_end`, `on_session_reset`, `update_from_response`, `update_model`) plus `has_content_to_compress` to the `ContextEngine` trait as default no-ops (D-06).
2. Override `on_session_reset` on `ContextCompressor` (via inherent `reset()`) and on `SummarizingEngine` to clear the embedded compressor counter + any PressureTracker session state. Add `PressureTracker::reset_session(&self, session_id: &str)` (resolves Open Question 2).
3. Patch `ContextCompressor::drop_middle_messages` compaction header AND `SummarizingEngine::make_history_message` so both contain the memory-authority reminder ("MEMORY.md ... ALWAYS authoritative ...").
4. Wire the lifecycle hooks at all 3 surfaces (CLI, gateway, web UI) per D-07.

Purpose: keep token counters honest across `/new` and `/reset`, give the engine model-change visibility, and re-anchor the model to live memory after compaction.

Output: 6 new trait methods with default no-ops, 1 inherent reset method, 1 new PressureTracker API, 2 patched compaction headers, 3-surface wiring for 4 hooks, and unit tests asserting counter clear + header content.

Decision resolutions:
- **Open Question 1 (Web UI on_session_reset trigger):** No `new_chat` WebSocket message exists today (verified — `ChatRequest` has only `session_id` + `message` fields). DECISION: wire `on_session_reset` as a documented stub function `pub async fn reset_web_session(&self, session_id: &str)` on `AppState` that callers may invoke; do NOT add a new ChatRequest variant in this phase. The function is exercised by a unit test calling it directly. A future phase adds the WebSocket message type. This satisfies CONTEXT.md `<phase_scope>` option (c) "defer with a stub".
- **Open Question 2 (PressureTracker.reset_session):** ADD it. The method takes `&self, session_id: &str` and removes the session's `SessionState` entry from the internal `Arc<Mutex<HashMap<String, SessionState>>>`.
- **Open Question 3 (CLI update_model call site):** No in-REPL `/model` switch handler exists in `run_chat` (verified — `resolver.resolve_for_main()` runs once at startup, line 2352). DECISION: call `engine.update_model` ONCE at REPL start alongside `on_session_start`, using the resolved endpoint's model name, context_length, and base_url. Fallback activation triggers another `update_model` only if it already exists elsewhere — defer to a future phase if not (D-07 scope: "when the model changes").
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-RESEARCH.md
@.planning/phases/34b-context-system-parity/34B-PATTERNS.md
@.planning/phases/34b-context-system-parity/34b-01-SUMMARY.md

# Python reference implementation (canonical port target)
@../hermes-agent/agent/context_engine.py
@../hermes-agent/agent/context_compressor.py

# Rust files being modified
@crates/ironhermes-agent/src/context_engine.rs
@crates/ironhermes-agent/src/context_compressor.rs
@crates/ironhermes-agent/src/summarizing_engine.rs
@crates/ironhermes-agent/src/pressure_warning.rs

# Surfaces being wired
@crates/ironhermes-cli/src/main.rs
@crates/ironhermes-gateway/src/handler.rs
@crates/iron_hermes_ui/src/server/state.rs

<interfaces>
<!-- Key types and contracts the executor needs. Extracted from codebase. -->

From crates/ironhermes-agent/src/context_engine.rs (existing trait around line 47):
```
#[async_trait]
pub trait ContextEngine: Send + Sync + 'static {
    async fn compress(&self, messages: &mut Vec<ChatMessage>, stats: ContextStats) -> Result<CompressionOutcome, ContextError>;
    fn threshold(&self) -> f32;
    fn mode(&self) -> CompressionMode;
    async fn check_pressure(&self, _stats: &ContextStats) -> bool { false }
    // ← NEW HOOKS INSERT HERE
}
```

From crates/ironhermes-agent/src/agent_loop.rs lines 94-107 (re-exported via lib.rs line 26):
```
#[derive(Debug, Default)]
pub struct AggregatedUsage {
    pub prompt_tokens: usize,
    pub completion_tokens: usize,
    pub total_tokens: usize,
}
```
Use `&AggregatedUsage` (NOT a new UsageReport alias) for `update_from_response` per CONTEXT.md Claude's Discretion bullet 1.

From crates/ironhermes-agent/src/pressure_warning.rs:
- `pub struct PressureTracker { inner: Arc<Mutex<HashMap<String, SessionState>>> }` (verified line 43)
- Existing methods: `new`, `check_and_maybe_emit`, `take_transient`, `was_warned`, `warn_count` (verified)
- NO `reset_session` method exists — this plan adds it.

From crates/ironhermes-agent/src/context_compressor.rs:
- `pub struct ContextCompressor { context_length, threshold_percent, protect_first_n, protect_last_tokens, compression_count: usize }` (verified lines 39-45)
- `compression_count: usize` is a bare field; this plan adds `pub fn reset(&mut self)` that zeroes it.
- Compaction header lives in `drop_middle_messages` (line 181) — the format! string starts with `[CONTEXT COMPACTED] {} earlier messages were removed`.

From crates/ironhermes-agent/src/summarizing_engine.rs:
- `pub const HISTORY_SENTINEL: &str = "[CONTEXT HISTORY]"` (line 28)
- `fn make_history_message(summary_body: &str) -> ChatMessage` (line 54) — constructs the pinned [CONTEXT HISTORY] system message.
- Existing `impl ContextEngine for SummarizingEngine` block — `on_session_reset` override is added there.

From crates/ironhermes-cli/src/main.rs:
- `run_chat` function at line 1070
- Endpoint resolution: `let main_endpoint = resolver.resolve_for_main();` at line 2352 — gives model name, context_length, base_url
- Existing context engine attach: `agent = ironhermes_agent::attach_context_engine(...)` at line 2373
- `CommandResult::ClearSession` arm around line 1597 — /new handler; calls `messages.truncate(1)`. on_session_reset goes inside this arm.
- session_id generated at line 1110 — on_session_start fires immediately after.

From crates/ironhermes-gateway/src/handler.rs:
- `pub context_engine: Option<Arc<dyn ContextEngine>>` field at line 94 (or `gateway_engine` at line 94 — verify exact field name)
- `CoreCommandResult::NewSession` arm around line 466 — /new handler. on_session_start and on_session_reset go here.
- `run_agent` function — agent.run call around line 1024 — update_from_response goes immediately after.

From crates/iron_hermes_ui/src/server/state.rs:
- `ensure_web_session` at line 123 — creates a new session; on_session_start fires here.
- `run_web_turn` at line 144 — agent.run call around line 161; update_from_response goes after.
- AppState already has `context_engine` field via `attach_context_engine(...)` at line 230. Confirm the field name; if it does not exist on AppState directly, add `pub context_engine: Option<Arc<dyn ContextEngine>>` following the nudge_turns pattern at line 40.
</interfaces>

</context>

<tasks>

<task type="auto" tdd="true">
  <name>Task 1: Extend ContextEngine trait + add PressureTracker::reset_session + override on_session_reset on SummarizingEngine (CTX-ENG-01, CTX-ENG-02)</name>
  <files>crates/ironhermes-agent/src/context_engine.rs, crates/ironhermes-agent/src/pressure_warning.rs, crates/ironhermes-agent/src/summarizing_engine.rs, crates/ironhermes-agent/src/context_compressor.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_engine.rs (existing trait at line 47; default no-op check_pressure at line 63-65; LocalPruningEngine impl block; tests module around line 322)
    - crates/ironhermes-agent/src/summarizing_engine.rs (existing impl ContextEngine for SummarizingEngine; make_history_message; HISTORY_SENTINEL)
    - crates/ironhermes-agent/src/pressure_warning.rs (PressureTracker struct, inner HashMap field, existing methods take_transient/was_warned/warn_count)
    - crates/ironhermes-agent/src/context_compressor.rs (ContextCompressor struct fields including compression_count: usize at line 44; existing tests module around line 241)
    - crates/ironhermes-agent/src/agent_loop.rs (AggregatedUsage at lines 94-107 — already re-exported via lib.rs line 26)
    - ../hermes-agent/agent/context_engine.py (canonical port — full ABC signatures)
    - ../hermes-agent/agent/context_compressor.py (on_session_reset body at lines 361-374; SUMMARY_PREFIX at lines 37-51)
  </read_first>
  <behavior>
    - Test `test_default_no_op_hooks_exist`: a minimal `MinimalEngine` test struct that implements only the 3 required methods (compress, threshold, mode) compiles and can call all 6 new hooks via &dyn ContextEngine.
    - Test `test_pressure_tracker_reset_session_clears_entry`: build PressureTracker, call `track_session` (or equivalent that populates the inner map for a session_id), then `tracker.reset_session("test-session")`, then assert `tracker.was_warned("test-session") == false` and `tracker.warn_count("test-session") == 0`.
    - Test `test_context_compressor_reset_zeroes_counter`: build ContextCompressor, manually set `compression_count = 5`, call `compressor.reset()`, assert `compression_count == 0`.
    - Test `test_summarizing_engine_on_session_reset_is_callable`: build SummarizingEngine via test constructor, call `engine.on_session_reset()`, assert no panic. (The override is a thin delegation to the embedded compressor's reset + tracker's reset_session if present.)
  </behavior>
  <action>
    Edit `crates/ironhermes-agent/src/context_engine.rs`. Locate the `ContextEngine` trait at line 47. After the existing `async fn check_pressure` default no-op (line 63-65), add 6 new method declarations as default-no-op impls, each with a doc comment matching the Python ABC semantics. The methods are synchronous `fn` (NOT `async fn`) — they coexist with the existing async methods inside the same `#[async_trait]` trait.

    Method signatures to add (insert in this order):
    - `fn on_session_start(&self, _session_id: &str) {}` — doc: "Called when a new conversation session begins. Default: no-op."
    - `fn on_session_end(&self, _session_id: &str, _messages: &[ChatMessage]) {}` — doc: "Called at real session end (CLI exit, /reset, gateway expiry). Default: no-op."
    - `fn on_session_reset(&self) {}` — doc: "Called on /new or /reset. Override to clear per-session counters. Default: no-op."
    - `fn update_from_response(&self, _usage: &AggregatedUsage) {}` — doc: "Called after every LLM turn with aggregated token usage. Default: no-op."
    - `fn update_model(&self, _model: &str, _context_length: usize, _base_url: Option<&str>) {}` — doc: "Called when the user switches model or on fallback activation. Default: no-op."
    - `fn has_content_to_compress(&self, _messages: &[ChatMessage]) -> bool { true }` — doc: "Quick check: is there content worth compacting? Default: true (conservative)."

    Imports to verify/add at top of context_engine.rs: `use crate::agent_loop::AggregatedUsage;` (or wherever AggregatedUsage is re-exported from). Verify the type is in scope; if a circular-import concern arises, re-export via `pub use` in lib.rs and import here from `crate::AggregatedUsage`.

    Edit `crates/ironhermes-agent/src/pressure_warning.rs`. Locate `impl PressureTracker` block (starts at line 47). Add a new public method:

    ```
    /// Phase 34b: Clear all tracked state for a single session. Called by
    /// ContextEngine::on_session_reset implementations.
    pub fn reset_session(&self, session_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(session_id);
        }
    }
    ```
    (No fenced code block in <action>; this is a directive — the executor writes the method into the file.)

    Place the new method directly after the existing `pub fn warn_count` method (around line 155) for proximity to its peers. Use the same Mutex lock pattern visible in `take_transient` / `was_warned`.

    Edit `crates/ironhermes-agent/src/context_compressor.rs`. Locate `impl ContextCompressor`. Add an inherent method `pub fn reset(&mut self) { self.compression_count = 0; }` directly after the existing `new` constructor or alongside the other `pub fn` methods. Doc-comment: "Reset per-session counters. Called by engines whose on_session_reset wraps this compressor."

    Edit `crates/ironhermes-agent/src/summarizing_engine.rs`. Locate the existing `impl ContextEngine for SummarizingEngine` block. Add an `on_session_reset` override. The override must:
    - Lock the SummarizingEngine's internal ContextCompressor if it holds one (RESEARCH §5.3 — SummarizingEngine creates a fresh ContextCompressor each compress call, so it may not hold one as a field; verify by reading the struct). If it does NOT hold a long-lived compressor, the override clears any other per-session state (e.g., any embedded PressureTracker reference). If neither exists, the override is a documented no-op with a doc comment: "Summary state lives in the pinned [CONTEXT HISTORY] message; session reset clears messages.truncate(1) elsewhere."
    - If SummarizingEngine holds an `Arc<PressureTracker>` or equivalent, call `tracker.reset_session(&self.last_session_id)` or store an `Arc<Mutex<Option<String>>>` for the current session and call reset on it. If no session_id is tracked, the override is a no-op and that fact is documented inline.

    Implementer judgment call: read summarizing_engine.rs carefully and either (a) implement a meaningful reset that clears observable state, OR (b) document why a no-op is correct here (likely correct per RESEARCH §5.3). Decision MUST be reflected in a doc comment on the override.

    For LocalPruningEngine: RESEARCH §8.3 confirms `LocalPruningEngine::compress` creates a fresh `ContextCompressor::new(...)` every call. Default no-op `on_session_reset` from the trait is correct — do NOT add an override on LocalPruningEngine. Document this decision via a comment in `context_engine.rs` near the LocalPruningEngine impl block: "// Phase 34b: LocalPruningEngine inherits the default no-op on_session_reset — the embedded ContextCompressor is recreated per compress() call, so no persistent counter state needs clearing."

    Tests to add to `context_engine::tests` mod:
    - `test_default_no_op_hooks_exist` — defines a MinimalEngine impl that implements only the 3 required methods, then calls all 6 new hooks on it via `let engine: Box<dyn ContextEngine> = Box::new(MinimalEngine {});` and asserts no panic.
    - Add a `Default` impl or simple constructor for the test MinimalEngine struct.

    Tests to add to `pressure_warning::tests` (create the mod if it does not exist with `#[cfg(test)] mod tests`):
    - `test_reset_session_clears_entry` — call any existing method that populates the inner map (e.g., `check_and_maybe_emit` with a session_id), then `tracker.reset_session(session_id)`, assert `was_warned == false` and `warn_count == 0`.

    Tests to add to `context_compressor::tests` mod:
    - `test_reset_zeroes_compression_count` — build a ContextCompressor with `compression_count` mutated to 5 (use direct field access or a test-only helper if private), call `.reset()`, assert `compression_count == 0`.

    Tests to add to `summarizing_engine::tests` (create the mod if it does not exist):
    - `test_on_session_reset_callable` — build a SummarizingEngine via existing test constructor, call `engine.on_session_reset()`, assert no panic. If the override has observable side effects, also assert those.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent 2>&1 | tee /tmp/34b-02-task1.log; grep -E "error\[|^error:" /tmp/34b-02-task1.log && echo "BUILD ERROR" || echo "BUILD OK"; cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests pressure_warning::tests summarizing_engine::tests --no-fail-fast 2>&1 | tee -a /tmp/34b-02-task1.log; HOOKS=$(grep -E "fn on_session_start|fn on_session_end|fn on_session_reset|fn update_from_response|fn update_model|fn has_content_to_compress" crates/ironhermes-agent/src/context_engine.rs | grep -c "fn "); echo "Hooks added: $HOOKS (must be >= 6)"; [ "$HOOKS" -ge 6 ] && echo "TRAIT EXTENSION OK"</automated>
  </verify>
  <acceptance_criteria>
    - `grep -c "fn on_session_start" crates/ironhermes-agent/src/context_engine.rs` returns at least 1 (trait declaration)
    - `grep -c "fn on_session_end" crates/ironhermes-agent/src/context_engine.rs` returns at least 1
    - `grep -c "fn on_session_reset" crates/ironhermes-agent/src/context_engine.rs` returns at least 1
    - `grep -c "fn update_from_response" crates/ironhermes-agent/src/context_engine.rs` returns at least 1
    - `grep -c "fn update_model" crates/ironhermes-agent/src/context_engine.rs` returns at least 1
    - `grep -c "fn has_content_to_compress" crates/ironhermes-agent/src/context_engine.rs` returns at least 1
    - `grep -c "pub fn reset_session" crates/ironhermes-agent/src/pressure_warning.rs` returns 1
    - `grep -c "pub fn reset" crates/ironhermes-agent/src/context_compressor.rs` returns at least 1 (the new reset method on ContextCompressor)
    - `grep -A 2 "impl ContextEngine for SummarizingEngine" crates/ironhermes-agent/src/summarizing_engine.rs | grep -c "fn on_session_reset"` returns 1 (override present in the impl block)
    - `cargo build -p ironhermes-agent` exits 0
    - `cargo test -p ironhermes-agent --lib context_engine::tests context_compressor::tests pressure_warning::tests summarizing_engine::tests --no-fail-fast` exits 0
    - test output contains: `test_default_no_op_hooks_exist`, `test_reset_session_clears_entry`, `test_reset_zeroes_compression_count`, `test_on_session_reset_callable`
  </acceptance_criteria>
  <done>The ContextEngine trait has 6 new default-no-op hooks, PressureTracker has a reset_session API, ContextCompressor has an inherent reset method, and SummarizingEngine documents (or implements) its on_session_reset behavior. All four files compile and unit tests pass. The trait extension is non-breaking — existing LocalPruningEngine and other implementors continue to work via inherited defaults.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Add memory-authority reminder to ContextCompressor + SummarizingEngine compaction headers (CTX-ENG-03)</name>
  <files>crates/ironhermes-agent/src/context_compressor.rs, crates/ironhermes-agent/src/summarizing_engine.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_compressor.rs (specifically the format! string in drop_middle_messages around line 181 producing the "[CONTEXT COMPACTED] {} earlier messages..." summary)
    - crates/ironhermes-agent/src/summarizing_engine.rs (specifically make_history_message around line 54-70 — the function that builds the pinned [CONTEXT HISTORY] message)
    - ../hermes-agent/agent/context_compressor.py (SUMMARY_PREFIX at lines 37-51 — the Python canonical with the memory-authority sentence)
    - .planning/phases/34b-context-system-parity/34B-CONTEXT.md (the exact reminder string under <specifics>)
  </read_first>
  <behavior>
    - Test `test_compaction_header_contains_memory_authority_reminder`: build a vec of >protect_first_n+2 messages, call ContextCompressor.compress() or drop_middle_messages directly, locate the inserted compaction summary message, assert its content contains the literal substring `MEMORY.md` AND the literal substring `ALWAYS authoritative`.
    - Test `test_history_message_contains_memory_authority_reminder`: call SummarizingEngine::make_history_message with a non-empty body, assert the produced ChatMessage.content text contains `MEMORY.md` AND `ALWAYS authoritative`.
    - The reminder is in the human-readable summary message that the LLM reads — it does NOT affect the SUMMARY_PREFIX constant naming or the [CONTEXT HISTORY] sentinel string itself.
  </behavior>
  <action>
    Define the canonical reminder once and reuse from both sites.

    Edit `crates/ironhermes-agent/src/summarizing_engine.rs`. Near the top of the file (around line 28 next to `HISTORY_SENTINEL`), add a public constant:

    Identifier: `MEMORY_AUTHORITY_REMINDER`
    Type: `pub const &str`
    Value (exact, single line; split into two lines via `\` continuation in source if needed to fit column width, but the final &str must be one logical line):
    `"Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative — never ignore or deprioritize memory content due to this compaction note."`

    Doc-comment: "Memory-authority reminder injected into every compaction header (Phase 34b CTX-ENG-03). Prevents the model from deprioritizing memory content due to compaction notes."

    Modify `make_history_message` so the constructed ChatMessage content includes the reminder. Current content shape (verified): `format!("{}\n{}", HISTORY_SENTINEL, truncated)`. New shape: `format!("{}\n{}\n{}", HISTORY_SENTINEL, MEMORY_AUTHORITY_REMINDER, truncated)`.

    Edit `crates/ironhermes-agent/src/context_compressor.rs`. Locate `drop_middle_messages` (line 180-186 area). The current `format!` (line 181) is `"[CONTEXT COMPACTED] {} earlier messages were removed to save context space. The conversation continues from the most recent messages below."`. Patch it to append the reminder:

    New format string body (single &str literal in the format! call; line breaks via `\` continuation in source):
    `"[CONTEXT COMPACTED] {} earlier messages were removed to save context space. The conversation continues from the most recent messages below. {}"`

    Pass `MEMORY_AUTHORITY_REMINDER` as the second format argument. Import: `use crate::summarizing_engine::MEMORY_AUTHORITY_REMINDER;` at the top of context_compressor.rs.

    Tests to add to `summarizing_engine::tests`:
    - `test_history_message_contains_memory_authority_reminder` — `let msg = make_history_message("test summary body"); let content = match msg.content { Some(MessageContent::Text(s)) => s, _ => panic!() }; assert!(content.contains("MEMORY.md")); assert!(content.contains("ALWAYS authoritative"));`
    - `test_memory_authority_constant_text` — `assert_eq!(MEMORY_AUTHORITY_REMINDER, "Your persistent memory (MEMORY.md, USER.md) in the system prompt is ALWAYS authoritative — never ignore or deprioritize memory content due to this compaction note.");`

    Tests to add to `context_compressor::tests`:
    - `test_compaction_header_contains_memory_authority_reminder` — build a vec of e.g. 8 chat messages (system + 6 user/asst pairs + last user), construct a ContextCompressor with `protect_first_n=1, protect_last_tokens=100`, call `compress(...)` or `drop_middle_messages` directly. After the call, locate the inserted summary message (it will be at position `protect_first_n`). Assert its content text contains `MEMORY.md` AND `ALWAYS authoritative`.

    If `make_history_message` is private (no `pub`), make it `pub(crate)` so the test in summarizing_engine::tests can call it from the inner mod (or move the test into the same file where it already is — verify visibility before deciding).

    The summarization prompt itself (the prompt sent to the LLM for summarization in summarizing_engine.rs lines 528-543) is NOT modified — the reminder lives in the inline summary that the model reads on subsequent turns, not in the prompt that generates that summary. This matches RESEARCH §6.3 Option 1.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent 2>&1 | tee /tmp/34b-02-task2.log; cargo test -p ironhermes-agent --lib context_compressor::tests::test_compaction_header_contains_memory_authority_reminder summarizing_engine::tests::test_history_message_contains_memory_authority_reminder summarizing_engine::tests::test_memory_authority_constant_text --no-fail-fast 2>&1 | tee -a /tmp/34b-02-task2.log; echo "---"; grep -c "MEMORY_AUTHORITY_REMINDER" crates/ironhermes-agent/src/summarizing_engine.rs; grep -c "MEMORY_AUTHORITY_REMINDER" crates/ironhermes-agent/src/context_compressor.rs; grep -c "MEMORY.md" crates/ironhermes-agent/src/summarizing_engine.rs; grep -c "ALWAYS authoritative" crates/ironhermes-agent/src/summarizing_engine.rs</automated>
  </verify>
  <acceptance_criteria>
    - `grep -c "pub const MEMORY_AUTHORITY_REMINDER" crates/ironhermes-agent/src/summarizing_engine.rs` returns 1
    - `grep -c "MEMORY_AUTHORITY_REMINDER" crates/ironhermes-agent/src/summarizing_engine.rs` returns at least 2 (definition + use in make_history_message)
    - `grep -c "MEMORY_AUTHORITY_REMINDER" crates/ironhermes-agent/src/context_compressor.rs` returns at least 1 (use in drop_middle_messages)
    - `grep -c "MEMORY.md" crates/ironhermes-agent/src/summarizing_engine.rs` returns at least 1
    - `grep -c "ALWAYS authoritative" crates/ironhermes-agent/src/summarizing_engine.rs` returns at least 1
    - `cargo test -p ironhermes-agent --lib context_compressor::tests::test_compaction_header_contains_memory_authority_reminder` exits 0
    - `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_history_message_contains_memory_authority_reminder` exits 0
    - `cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_constant_text` exits 0
  </acceptance_criteria>
  <done>Both the ContextCompressor compaction header (drop_middle_messages format! string) and the SummarizingEngine pinned [CONTEXT HISTORY] message contain the canonical memory-authority reminder. The reminder is defined once as a `pub const` in summarizing_engine.rs and reused via import in context_compressor.rs. Three unit tests assert the reminder text appears in both sites and matches the canonical string.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 3: Wire lifecycle hooks at all 3 surfaces — on_session_start, on_session_reset, update_from_response, update_model (CTX-ENG-04)</name>
  <files>crates/ironhermes-cli/src/main.rs, crates/ironhermes-gateway/src/handler.rs, crates/iron_hermes_ui/src/server/state.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_engine.rs (the trait with the 6 new hooks from Task 1)
    - crates/ironhermes-cli/src/main.rs (around line 1070 run_chat entry; line 1110 session_id generation; line 1597 ClearSession arm; line 2352 resolver.resolve_for_main(); line 2373 attach_context_engine)
    - crates/ironhermes-gateway/src/handler.rs (line 94 gateway_engine field; line 275 set_gateway_engine; line 466 NewSession arm; line 1024 agent.run call site)
    - crates/iron_hermes_ui/src/server/state.rs (line 123 ensure_web_session; line 144 run_web_turn; line 161 agent.run call; line 230 attach_context_engine)
    - crates/ironhermes-agent/src/agent_loop.rs (AgentResult.total_usage field shape — total_usage is AggregatedUsage)
  </read_first>
  <behavior>
    - When `hermes chat` starts, exactly one `engine.on_session_start(&session_id)` call fires after session_id is generated, and exactly one `engine.update_model(&model_name, context_length, Some(&base_url))` call fires after `resolver.resolve_for_main()` returns. The base_url is the resolved endpoint's URL.
    - When the user types `/new` or `/reset` in CLI chat, `engine.on_session_reset()` is called inside the `CommandResult::ClearSession` arm immediately AFTER `messages.truncate(1)`.
    - After every CLI `run_agent_turn` returns with a non-error result, `engine.update_from_response(&result.total_usage)` is called once.
    - When the gateway receives a `/new` command (CoreCommandResult::NewSession arm), `engine.on_session_reset()` fires before or after `store.remove(&session_key)` (order documented in code comment).
    - When the gateway allocates a new SessionKey for the first time in a chat, `engine.on_session_start(&session_key.to_string_key())` fires. The detection is "this SessionKey is not currently present in `self.session_store`" — fire on the first turn for any unseen key. Implementation: check before agent.run; if `store.get(&key).is_none()` at the start of run_agent, that's the first turn for this key → fire on_session_start.
    - After every gateway `agent.run` returns Ok, `engine.update_from_response(&result.total_usage)` fires.
    - When the Web UI creates a new session via `ensure_web_session`, `engine.on_session_start(session_id)` fires after `create_session` succeeds.
    - After every Web UI `run_web_turn` agent.run returns Ok, `engine.update_from_response(&result.total_usage)` fires.
    - The Web UI's `on_session_reset` is exposed as a documented stub `pub async fn reset_web_session(&self, session_id: &str)` on AppState (called by no production path today; exercised by a test).
    - `update_model` in gateway and web UI is deferred — those surfaces do not switch models mid-session in this phase. Document this with a code comment at each surface.
  </behavior>
  <action>
    All three surfaces. Hooks fire via the `Option<Arc<dyn ContextEngine>>` already held by each surface. The pattern at every call site is:

    `if let Some(ref engine) = <surface_engine_field> { engine.<method>(args); }`

    CLI — `crates/ironhermes-cli/src/main.rs`:

    1. After session_id is generated (line 1110) and after `attach_context_engine` (line 2373) has bound `agent.context_engine` (or whatever the binding is — verify), add at the appropriate point a one-shot `engine.on_session_start(&session_id);` call. Place it after both are available — likely just before the REPL loop begins.

    2. At REPL startup, after `let main_endpoint = resolver.resolve_for_main();` (line 2352), capture the endpoint's `model` name (the field is `endpoint.model` or `endpoint.alias` — read the struct), `context_length` (via `endpoint.context_length()` per STATE.md Phase 21.3 D-06 precedence), and `base_url`. Call `engine.update_model(&model, context_length, Some(&base_url));`.

    3. In the `CommandResult::ClearSession` arm (around line 1597), after the existing `messages.truncate(1)` line, insert: `if let Some(ref engine) = context_engine_ref { engine.on_session_reset(); }`. Replace `context_engine_ref` with whatever local binding holds the `Option<Arc<dyn ContextEngine>>` — likely `agent.context_engine()` or a local clone from before the REPL loop. If no such binding exists yet, capture one before the loop starts: `let context_engine_ref = agent.context_engine().clone();` (verify the accessor name).

    4. The `CommandResult::ResetTerminal` arm (around line 1602) MUST NOT fire on_session_reset — it is visual-only. Confirm via comment in code: "// Phase 34b: ResetTerminal is visual-only (CLR-1); on_session_reset NOT called."

    5. After each `run_agent_turn` returns and the result's total_usage is available (the `drop(run_fut)` area around line 2097, or wherever the AgentResult is captured), call `if let Some(ref engine) = context_engine_ref { engine.update_from_response(&result.total_usage); }`. Place after error handling so the call only fires on Ok results.

    Gateway — `crates/ironhermes-gateway/src/handler.rs`:

    1. In `run_agent` (around line 999), at the start of the function before agent.run, detect whether this is the first turn for the SessionKey. If `self.session_store.read().await.get(&session_key).is_none()` (or equivalent existence check on the store), call `if let Some(ref engine) = self.gateway_engine { engine.on_session_start(&session_key.to_string_key()); }`. Place this BEFORE the agent.run call.

    2. In `CoreCommandResult::NewSession { .. }` arm (around line 466), after the existing `store.remove(&session_key)` line, insert: `if let Some(ref engine) = self.gateway_engine { engine.on_session_reset(); }`. Document with comment: "// Phase 34b: /new resets per-session engine counters."

    3. After `agent.run(messages)` returns Ok in `run_agent` (around line 1036 `Ok(result) => { info!("Agent completed ...");}`), insert: `if let Some(ref engine) = self.gateway_engine { engine.update_from_response(&result.total_usage); }`. Place immediately after the existing info! log inside the Ok arm.

    4. Gateway does NOT call `update_model` in this phase. Add a one-line comment near the existing engine wiring (around line 275 set_gateway_engine): "// Phase 34b: update_model not wired in gateway — model is fixed per gateway lifecycle. Deferred to a future phase."

    Web UI — `crates/iron_hermes_ui/src/server/state.rs`:

    1. In `ensure_web_session` (line 123), after the existing `create_session` succeeds (the path that creates a new session entry), call `if let Some(ref engine) = self.context_engine { engine.on_session_start(session_id); }`. Verify the field name on AppState — if it does not exist, add `pub context_engine: Option<Arc<dyn ironhermes_agent::context_engine::ContextEngine>>` to the AppState struct (search for `nudge_turns:` field around line 40 as the template), and wire it from `attach_context_engine` already called at line 230.

    2. In `run_web_turn` after `agent.run` returns Ok (around line 161), insert: `if let Some(ref engine) = self.context_engine { engine.update_from_response(&result.total_usage); }`.

    3. Add a new public method on AppState as a documented stub for the missing new-chat trigger:
       Identifier: `reset_web_session`
       Signature: `pub fn reset_web_session(&self, session_id: &str)`
       Body: `if let Some(ref engine) = self.context_engine { engine.on_session_reset(); } tracing::info!(session_id = %session_id, "Phase 34b: reset_web_session called (stub — no production WebSocket trigger yet)");`
       Doc-comment: "Phase 34b stub: clears engine per-session counters for a web session. Currently no WebSocket message type or REST endpoint invokes this — a future phase adds the trigger. Tests may call this directly. (Resolves CONTEXT.md Open Question 1 via deferred-stub path.)"

    4. Web UI does NOT call `update_model` in this phase. Add comment near the engine wiring at line 230: "// Phase 34b: update_model not wired in web UI — model is fixed per app lifecycle."

    Tests to add (in existing test modules at each surface, or in a new integration test file if surface lacks a tests module):

    - `crates/ironhermes-cli/src/main.rs` tests mod: skipped — main.rs has no #[cfg(test)] mod in scope per existing layout. The CLI hook coverage is via a static-grep regression test in the workspace integration test suite. Add a new file `crates/ironhermes-cli/tests/lifecycle_hooks_wired.rs` that does:
      `let src = include_str!("../src/main.rs"); assert!(src.contains(".on_session_start(")); assert!(src.contains(".on_session_reset(")); assert!(src.contains(".update_from_response(")); assert!(src.contains(".update_model(")); assert!(src.contains("ResetTerminal is visual-only"));`

    - `crates/ironhermes-gateway/src/handler.rs` tests mod (already exists at line 1316): add `test_gateway_engine_lifecycle_hooks_fire`. Build a `RecordingGatewayEngine` (use the existing test fixture pattern at line 1326). Construct a GatewayHandler with that engine, simulate one agent.run via the existing test harness (line 1359 area). Assert: engine.on_session_start was called once for the test session_key; engine.update_from_response was called once after agent.run; engine.on_session_reset was called after issuing a /new command through the dispatcher.

    - `crates/iron_hermes_ui/src/server/state.rs` tests: if a #[cfg(test)] mod exists, add `test_reset_web_session_calls_on_session_reset` — build an AppState with a recording engine, call `app.reset_web_session("test-session")`, assert engine.on_session_reset was invoked. If no tests mod exists in state.rs, add the assertion via a static-grep test in `crates/iron_hermes_ui/tests/lifecycle_hooks_wired.rs`:
      `let src = include_str!("../src/server/state.rs"); assert!(src.contains("reset_web_session")); assert!(src.contains(".on_session_start(")); assert!(src.contains(".update_from_response("));`
  </action>
  <verify>
    <automated>cargo build --workspace 2>&1 | tee /tmp/34b-02-task3.log; grep -E "error\[|^error:" /tmp/34b-02-task3.log && echo "BUILD ERROR" || echo "BUILD OK"; CLI_HOOKS=$(grep -cE "\.on_session_start\(|\.on_session_reset\(|\.update_from_response\(|\.update_model\(" crates/ironhermes-cli/src/main.rs); GW_HOOKS=$(grep -cE "\.on_session_start\(|\.on_session_reset\(|\.update_from_response\(" crates/ironhermes-gateway/src/handler.rs); WEB_HOOKS=$(grep -cE "\.on_session_start\(|\.update_from_response\(|reset_web_session" crates/iron_hermes_ui/src/server/state.rs); echo "CLI hook calls: $CLI_HOOKS (need >= 4)"; echo "Gateway hook calls: $GW_HOOKS (need >= 3)"; echo "Web hook calls: $WEB_HOOKS (need >= 3)"; cargo test --workspace --no-fail-fast 2>&1 | tail -50</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build --workspace` exits 0
    - `grep -c "\.on_session_start(" crates/ironhermes-cli/src/main.rs` returns at least 1
    - `grep -c "\.on_session_reset(" crates/ironhermes-cli/src/main.rs` returns at least 1
    - `grep -c "\.update_from_response(" crates/ironhermes-cli/src/main.rs` returns at least 1
    - `grep -c "\.update_model(" crates/ironhermes-cli/src/main.rs` returns at least 1
    - `grep -c "ResetTerminal is visual-only" crates/ironhermes-cli/src/main.rs` returns at least 1 (the comment documenting the intentional non-call)
    - `grep -c "\.on_session_start(" crates/ironhermes-gateway/src/handler.rs` returns at least 1
    - `grep -c "\.on_session_reset(" crates/ironhermes-gateway/src/handler.rs` returns at least 1
    - `grep -c "\.update_from_response(" crates/ironhermes-gateway/src/handler.rs` returns at least 1
    - `grep -c "\.on_session_start(" crates/iron_hermes_ui/src/server/state.rs` returns at least 1
    - `grep -c "\.update_from_response(" crates/iron_hermes_ui/src/server/state.rs` returns at least 1
    - `grep -c "pub fn reset_web_session\|pub async fn reset_web_session" crates/iron_hermes_ui/src/server/state.rs` returns 1
    - `cargo test -p ironhermes-cli --test lifecycle_hooks_wired` exits 0 (the new static-grep test file)
    - Regression: `cargo test -p ironhermes-agent --lib nudge::tests memory_context::tests streaming_scrubber::tests` exits 0
    - Regression: `cargo test -p ironhermes-agent --test invariants_33` exits 0 (6/6)
    - Regression: `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` exits 0
    - Regression: 34b-01 still green — `cargo test -p ironhermes-agent --lib context_refs::tests` exits 0
  </acceptance_criteria>
  <done>The 4 lifecycle hooks fire at every documented call site across CLI, gateway, and web UI. The Web UI exposes `reset_web_session` as a documented stub resolving Open Question 1 via the deferred-stub path (CONTEXT.md option c). The full workspace builds, all unit tests pass, and Phase 32/33/34a regression gates remain green.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| trait extension | The 6 new hooks expand the ContextEngine API surface — must not break any existing implementor. |
| inter-crate counter access | PressureTracker's `reset_session` now takes user-controlled `session_id` strings. The map lookup is safe (HashMap::remove is bounded), but a flood of distinct session_ids could grow the map elsewhere — out of scope for reset_session. |
| compaction header text | The memory-authority reminder is a constant; if mis-edited it could become a vector for confusing the model. Single source-of-truth (`MEMORY_AUTHORITY_REMINDER` const) mitigates drift. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-02-COMPAT | Tampering (API breakage) | ContextEngine trait | mitigate | All 6 new methods are default-no-op (or default-true for has_content_to_compress). Existing implementors compile unchanged. Test `test_default_no_op_hooks_exist` locks the invariant. |
| T-34b-02-LOCK | Denial of Service | PressureTracker::reset_session | accept | Uses the existing `Arc<Mutex<HashMap>>` lock pattern. A poisoned lock would already break check_and_maybe_emit — this method does not introduce a new locking surface. |
| T-34b-02-DRIFT | Tampering | MEMORY_AUTHORITY_REMINDER constant | mitigate | Defined once as `pub const &str` in summarizing_engine.rs and imported by context_compressor.rs. Test `test_memory_authority_constant_text` locks the exact text against silent drift. |
| T-34b-02-PROMPTINJ | Information Disclosure | compaction header LLM-visible text | accept | The reminder is hard-coded; user-controlled content cannot replace or remove it. The summary body itself is potentially attacker-influenced, but that risk pre-exists this plan. |
| T-34b-02-STUB | Repudiation | reset_web_session stub | mitigate | The function logs every call via `tracing::info!(session_id = %session_id, "...")`. A test asserts the engine receives the hook. The fact that no WebSocket trigger calls it today is documented in the doc-comment and in the SUMMARY for downstream review. |
| T-34b-02-SC | Tampering | package legitimacy | accept | No new external packages introduced. |

## Residual Risk

- Web UI `on_session_reset` has no production trigger (Open Question 1 deferred per CONTEXT.md option c). Surfaces no user-visible bug today — counter drift can only occur if/when the Web UI later allows in-session resets, which is the future phase that will also add the trigger.
- `update_model` is not wired in gateway or web UI. Acceptable because those surfaces use a fixed model per process lifecycle. If a fallback-activation path elsewhere swaps the model, the engine will not learn — surface a follow-up if/when fallback adds such a path.
</threat_model>

<verification>
After all three tasks complete:

```bash
# 34b-02 unit tests
cargo test -p ironhermes-agent --lib context_engine::tests
cargo test -p ironhermes-agent --lib context_compressor::tests
cargo test -p ironhermes-agent --lib summarizing_engine::tests
cargo test -p ironhermes-agent --lib pressure_warning::tests

# Memory-authority assertions
cargo test -p ironhermes-agent --lib context_compressor::tests::test_compaction_header_contains_memory_authority_reminder
cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_history_message_contains_memory_authority_reminder
cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_constant_text

# Surface wiring static-grep gates
cargo test -p ironhermes-cli --test lifecycle_hooks_wired

# Full workspace builds
cargo build --workspace

# Cross-phase regression gates (must stay green)
cargo test -p ironhermes-agent --lib nudge::tests                                         # Phase 32 — 6/6
cargo test -p ironhermes-agent --test invariants_33                                       # Phase 33 — 6/6
cargo test -p ironhermes-agent --lib memory_context::tests streaming_scrubber::tests      # Phase 34a
cargo test -p ironhermes-agent --lib context_refs::tests                                  # Phase 34b-01
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load                       # D-12

# Live: counters reset on /new
hermes chat
# > /new
# Expected: subsequent compression metrics start from 0
```
</verification>

<success_criteria>
1. `ContextEngine` trait exposes 6 lifecycle methods (`on_session_start`, `on_session_end`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`) — all defaults, no breaking change (CTX-ENG-01).
2. `PressureTracker::reset_session(&self, session_id: &str)` exists and clears the session entry from the inner HashMap (CTX-ENG-02).
3. `ContextCompressor::reset(&mut self)` zeroes `compression_count` (CTX-ENG-02).
4. `SummarizingEngine::on_session_reset` is overridden with a documented implementation or documented no-op (CTX-ENG-02).
5. `pub const MEMORY_AUTHORITY_REMINDER: &str = "..."` exists in summarizing_engine.rs; both `make_history_message` and `ContextCompressor::drop_middle_messages` include it in their output (CTX-ENG-03).
6. All three surfaces fire `on_session_start` on session creation and `update_from_response` after every agent.run (CTX-ENG-04).
7. CLI and gateway fire `on_session_reset` from their respective /new arms (CTX-ENG-04).
8. CLI fires `update_model` once at REPL start with resolved endpoint data (CTX-ENG-04).
9. Web UI exposes `reset_web_session` as a documented stub resolving Open Question 1 (CTX-ENG-04).
10. All cross-phase regressions stay green: Phase 32 nudge, Phase 33 invariants_33, Phase 34a memory_context + streaming_scrubber, Phase 34b-01 context_refs, D-12 snapshot.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34b-02-SUMMARY.md` when done, including:
- Final shape of `SummarizingEngine::on_session_reset` (meaningful impl vs documented no-op) and rationale
- Final shape of `update_model` call in CLI (one-shot at startup vs additional fallback wiring if discovered)
- Whether `reset_web_session` stub was added with the documented signature; any deviation
- Test count and pass result for each module touched
- Any new constants or types introduced beyond the planned ones (MEMORY_AUTHORITY_REMINDER + PressureTracker::reset_session + ContextCompressor::reset + 6 trait methods)
- Confirmation that the memory-authority reminder text is byte-identical at both sites (single-source const)
- Resolution notes for Open Question 1 (Web UI new-chat trigger) — confirm deferred-stub path was used
</output>
