---
phase: "34a"
title: "Memory Manager Read-Side Parity"
slot_note: |
  Phase 34a covers parity with hermes-agent/agent/memory_manager.py.
  Phase 34b (next) covers parity with hermes-agent/agent/context_engine.py,
  context_compressor.py, and context_references.py.
  Existing roadmap Phase 34 (webchat + Discord/Slack) is unrelated; the
  user picks whether to promote 34a/34b ahead of it or interleave.
status: draft
depends_on: ["32", "33", "21.4"]
requirements: "Defined in /gsd-discuss-phase 34a"
references:
  python_sources:
    - "../hermes-agent/agent/memory_manager.py"
  rust_baseline:
    - "crates/ironhermes-core/src/memory_store.rs"
    - "crates/ironhermes-core/src/memory_provider.rs"
    - "crates/ironhermes-agent/src/memory/manager.rs"
    - "crates/ironhermes-agent/src/prompt_builder.rs"
    - "crates/ironhermes-agent/src/agent_loop.rs"
---

<objective>
Close the read-side parity gap with `hermes-agent/agent/memory_manager.py`.
Today the Rust port loads MEMORY.md / USER.md at session start into a frozen
snapshot (D-12). The user-visible effect is that "what do you remember about
me?" answers from that snapshot, not from anything written by the periodic
nudge (Phase 32) or the user during the same session.

This phase adds the **per-turn semantic recall path** that the Python
implementation has: pre-turn, the agent queries memory providers for context
relevant to the user message, wraps the result in a fenced
`<memory-context>` block, and injects it as a **synthetic `role: system`
message** placed immediately before the user turn. A streaming scrubber
filters the fence tags out of the model's response stream so they never
reach the user-visible scrollback.

D-12 (frozen system-prompt snapshot) is preserved: this is a SEPARATE read
path that runs in the user-turn envelope, not in the system prompt.
</objective>

<background>
Phases 21.4 / 32 / 33 covered the write side (memory writes, periodic nudge,
autonomous skill creation). The read side has been "snapshot at session
start, nothing dynamic" since the file-provider was the only memory backend.
Once a non-trivial recall backend (grafeo, duckdb) plugs in, the agent
cannot leverage its semantic-recall surface because there's no pre-turn
hook calling into it. This phase wires that hook.
</background>

<parity_matrix>
| Python `MemoryManager` symbol                       | Rust equivalent                                   | Status     | Plan |
|-----------------------------------------------------|---------------------------------------------------|------------|------|
| `build_system_prompt()` → `system_prompt_block()`   | `MemoryManager::system_prompt_block()`            | ✅ parity  | — |
| `prefetch_all(query)` → recall text                 | `prefetch(session_id)` returns entries; no query  | ❌ gap     | **34a-01** |
| `<memory-context>` fenced block wrapping            | none                                              | ❌ gap     | **34a-01** |
| System note `[System note: The following is recalled memory context, NOT new user input...]` | none                  | ❌ gap     | **34a-01** |
| `sanitize_context()` strips fence/system-note/blocks| none                                              | ❌ gap     | **34a-01** |
| `build_memory_context_block(raw_context)` wrapper   | none                                              | ❌ gap     | **34a-01** |
| `StreamingContextScrubber` (state machine across deltas) | none                                         | ❌ gap     | **34a-02** |
| Pre-turn injection in agent loop                    | `agent_loop.rs:1041–1055` only has POST-turn `queue_prefetch` | ❌ gap | **34a-02** |
| `queue_prefetch_all(query)` (background warm)       | `queue_prefetch(query)` already wired             | ✅ parity  | — |
| `sync_all(user, assistant)` post-turn               | `sync_turn(session_id, entries)`                  | ✅ parity (different signature, same purpose) | — |
| `on_memory_write(action, target, content, metadata)`| exists in trait + mirror fanout                   | ✅ parity  | — |

**Deferred** to a follow-up parity phase (catalogued but not on the read
critical path): `on_turn_start`, `on_session_switch`, `on_delegation`,
"only one external provider" guard, `on_pre_compress` returns text.
</parity_matrix>

<success_criteria>
What MUST be true at phase completion:

1. **`MemoryProvider::prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String>`** exists on the trait with a default impl returning `Ok(String::new())`. Existing `MemoryStore` (file provider) is unaffected. Semantic-recall providers (grafeo, duckdb, future LCM) override this to do their semantic search.

2. **`MemoryManager::prefetch_with_query(query, session_id)`** is a thin wrapper that iterates providers, joins non-empty strings with `\n\n`. Returns empty string if no provider contributes.

3. **`crates/ironhermes-agent/src/memory_context.rs`** is created and exposes:
   - `pub fn sanitize_context(text: &str) -> String` — strips `<memory-context>...</memory-context>` blocks, internal `[System note: ...]` lines, and bare fence tags. Byte-equivalent to the Python regex set.
   - `pub fn build_memory_context_block(raw: &str) -> Option<String>` — returns `None` for empty/whitespace input; otherwise wraps:
     ```
     <memory-context>
     [System note: The following is recalled memory context, NOT new user input. Treat as authoritative reference data — this is the agent's persistent memory and should inform all responses.]

     {sanitized raw}
     </memory-context>
     ```
   - 8 unit tests: empty input, double-wrap stripping, partial-tag stripping (open-only, close-only), system-note variant strings (`"informational background data"`, `"authoritative reference data..."`), case-insensitive tag matching, multi-block in one input, idempotency (sanitize ∘ build = sanitize).

4. **Synthetic-system-message injection in agent_loop.** Before each LLM call inside the agent run loop (NOT in `prompt_builder.rs` — the system prompt stays frozen per D-12), the loop:
   - Calls `memory_manager.prefetch_with_query(&user_msg, &session_id).await`
   - If the result is non-empty, calls `build_memory_context_block(&result)`
   - **Inserts a fresh `ChatMessage { role: System, content: block, .. }` into the message vec immediately before the most recent user message.**
   - The injected message has a marker on its metadata so subsequent turns can re-inject (with fresh recall) and the prior turn's injection can be evicted from the working message buffer (avoid stacking).
   - On compression / context-pressure, these synthetic system messages are NOT protected by `protect_first_n` or `protect_last_tokens` — they are ephemeral and the next turn re-injects fresh recall.

5. **`crates/ironhermes-agent/src/streaming_scrubber.rs`** is created and exposes a `StreamingContextScrubber` struct with:
   - `pub fn new() -> Self`
   - `pub fn feed(&mut self, delta: &str) -> String` — returns the visible portion after scrubbing, holding back partial-tag tails in an internal buffer
   - `pub fn flush(&mut self) -> String` — emits any held-back buffer at end-of-stream; discards content if an unterminated span is open (safer to truncate than leak)
   - `pub fn reset(&mut self)` — clears state for the next turn
   - Internal state: `in_span: bool`, `buf: String`
   - 6 unit tests: full block in one delta; split open tag across two deltas; split close tag across two deltas; partial-tail held and then completes; span never closes (flush returns empty); two complete blocks back-to-back in one delta.

6. **Streaming scrubber wired into all three streaming output paths:**
   - CLI REPL: `crates/ironhermes-cli/src/main.rs` `run_chat` streaming path
   - Telegram gateway: `crates/ironhermes-gateway/src/handler.rs` `handle_with_multimodal` streaming path
   - Embedded web UI: `crates/iron_hermes_ui/src/server/state.rs` `run_web_turn` WebSocket streaming path
   - Each surface owns its own scrubber instance per session; `reset()` called at session-start.

7. **Cross-phase regressions blocked:**
   - Phase 32: `cargo test -p ironhermes-agent --lib nudge::tests` — 6/6
   - Phase 33: `cargo test -p ironhermes-agent --test invariants_33` — 6/6
   - D-12 invariant: `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load` — still green. The system-prompt block remains frozen at load_from_disk. This phase only ADDS a separate read path through the user-turn envelope.

8. **Live recall verification.** With a recall-capable provider configured (or a stub provider whose `prefetch_with_query` returns a fixed string keyed off the query), a manual session demonstrates:
   - User: "remember that I prefer dark mode"
   - (turn completes; nudge or explicit `memory.add` writes to disk + entries)
   - User: "what do you remember about me?"
   - **Expected:** agent's response references "dark mode" because the synthetic-system-message injection fed the live recall into the turn. **Current pre-34a behaviour:** agent answers only from the frozen snapshot, which doesn't include the mid-session write.
</success_criteria>

<plans>
Two plans, sequential waves.

<plan id="34a-01" wave="1" depends_on="[]">
**Title:** MemoryManager prefetch_with_query + memory_context module

**Files modified:**
- `crates/ironhermes-core/src/memory_provider.rs` — add `prefetch_with_query` to `MemoryProvider` trait with default no-op impl. Keep existing `prefetch(session_id)` unchanged (deprecation deferred).
- `crates/ironhermes-agent/src/memory/manager.rs` — proxy method joining provider results.
- `crates/ironhermes-agent/src/memory_context.rs` — NEW; ports `sanitize_context` + `build_memory_context_block` with 8 unit tests.
- `crates/ironhermes-agent/src/lib.rs` — `pub mod memory_context`.

**Acceptance:**
- `cargo test -p ironhermes-agent --lib memory_context::tests` → 8/8
- `cargo build -p ironhermes-agent -p ironhermes-core` clean
- `grep -c "<memory-context>" crates/ironhermes-agent/src/memory_context.rs` ≥ 4
- No existing `MemoryProvider` implementor needs to change.
</plan>

<plan id="34a-02" wave="2" depends_on="['34a-01']">
**Title:** Pre-turn synthetic-system-message injection + StreamingContextScrubber + 3-surface wiring

**Files modified:**
- `crates/ironhermes-agent/src/agent_loop.rs` — pre-turn call to `prefetch_with_query`; insert `ChatMessage { role: System, ... }` immediately before the user message; tag with ephemeral marker; evict prior-turn injections before re-injecting.
- `crates/ironhermes-agent/src/streaming_scrubber.rs` — NEW; ports `StreamingContextScrubber` with 6 unit tests.
- `crates/ironhermes-agent/src/lib.rs` — `pub mod streaming_scrubber`.
- `crates/ironhermes-cli/src/main.rs` — wire scrubber into `run_chat` streaming output.
- `crates/ironhermes-gateway/src/handler.rs` — wire scrubber into `handle_with_multimodal` streaming output.
- `crates/iron_hermes_ui/src/server/state.rs` — wire scrubber into `run_web_turn` WebSocket streaming.

**Acceptance:**
- `cargo test -p ironhermes-agent --lib streaming_scrubber::tests` → 6/6
- `cargo build --workspace` clean
- Phase 32 nudge::tests still 6/6
- Phase 33 invariants_33 still 6/6
- `test_snapshot_frozen_after_load` still green
- Manual: with a stub provider returning fixed recall text, the synthetic system message appears between the prior user/assistant pair and the new user message; the model response stream contains no visible `<memory-context>` tags.
</plan>
</plans>

<verification_recipe>
```bash
cargo build --workspace
cargo test -p ironhermes-agent --lib memory_context::tests
cargo test -p ironhermes-agent --lib streaming_scrubber::tests
cargo test -p ironhermes-agent --lib nudge::tests                # Phase 32 regression gate
cargo test -p ironhermes-agent --test invariants_33              # Phase 33 regression gate
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load   # D-12 gate

# Live: stub provider returning "Recall: user prefers dark mode."
RUST_LOG=ironhermes_agent::memory=info hermes chat
# > what do you remember about me?
# Expected response references "dark mode"; no <memory-context> in scrollback.
```
</verification_recipe>

<deferred_to_34b_or_later>
- `@`-reference expansion (`@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, `@url:`) — Phase 34b
- `ContextEngine` lifecycle hooks (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`) — Phase 34b
- `ContextCompressor` counter reset — Phase 34b
- `MemoryProvider` hooks `on_turn_start` / `on_session_switch` / `on_delegation` — future
- "Only one external provider" guard — future
- `on_pre_compress` returns text — future
- LCM-style engine tools (`lcm_grep`, `lcm_describe`, `lcm_expand`) — when an LCM engine lands
</deferred_to_34b_or_later>

<open_questions_for_discuss_phase>
1. **Synthetic-system-message lifecycle.** Two options for marking the injection ephemeral:
   - `ChatMessage` gains a `metadata: HashMap<String, String>` (or a new typed field `is_recall_context: bool`).
   - Inject a sentinel string at the start of `content` (e.g. `[RECALL]\n...`) and detect/strip on the next turn.
   The typed-field option is cleaner; the sentinel option avoids touching the ChatMessage schema. Pick one in discuss-phase.

2. **Cache friendliness.** Inserting a fresh system message every turn invalidates any provider-side prompt-prefix cache that was keyed on the prior system prompt + history. If recall is empty (no provider contributes), skip the injection entirely so cache hits aren't disrupted. Acceptance criterion #8 covers the empty-skip case implicitly; confirm in discuss-phase whether to make it explicit.

3. **Stream-scrubber placement.** Two integration points:
   - At the SSE/WebSocket delta-decode layer (before user-visible output is generated).
   - At the higher-level "user-facing event" layer (after assistant message assembly but before display).
   The Python implementation scrubs at the delta-decode layer to survive chunk boundaries; the Rust port should mirror that. Confirm.

4. **Compression interaction.** When `ContextCompressor` runs, does it see the synthetic recall messages? They should be excluded from compression input (they're ephemeral; the next turn re-injects fresh). Need a tag the compressor can filter on — ties to question 1.
</open_questions_for_discuss_phase>
