# Phase 34a: Memory Manager Read-Side Parity - Context

**Gathered:** 2026-05-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Phase 34a closes the **read-side parity gap** with `hermes-agent/agent/memory_manager.py`.

What this phase delivers:

1. **`MemoryProvider::prefetch_with_query` trait method** — `prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String>` with a default no-op impl. Existing `MemoryStore` (file provider) is unaffected; semantic-recall backends (grafeo, duckdb, future) override this.

2. **`crates/ironhermes-agent/src/memory_context.rs`** — new module porting `sanitize_context` + `build_memory_context_block` from Python, with 8 unit tests.

3. **Pre-turn synthetic-system-message injection in `agent_loop.rs`** — before each LLM call, the loop calls `prefetch_with_query`, wraps the result in a `<memory-context>` block, and inserts a `ChatMessage { role: System, is_recall_context: true, ... }` immediately before the most recent user message.

4. **`crates/ironhermes-agent/src/streaming_scrubber.rs`** — new `StreamingContextScrubber` struct with `feed`/`flush`/`reset` and 6 unit tests.

5. **Scrubber wired into all 3 streaming surfaces** — CLI `run_chat`, gateway `handle_with_multimodal`, web UI `run_web_turn` WebSocket path.

**D-12 (Phase 21.4) is preserved**: the frozen system-prompt snapshot is untouched. This phase adds a SEPARATE per-turn read path through the user-turn envelope, not system prompt assembly.

**Does not deliver:** `@`-reference expansion, `ContextEngine` lifecycle hooks, `on_turn_start` / `on_session_switch` / `on_delegation` hooks, "only one external provider" guard — all deferred to Phase 34b or later.

</domain>

<decisions>
## Implementation Decisions

### Recall message marker

- **D-01:** `ChatMessage` in `crates/ironhermes-core/src/types.rs` gains `#[serde(skip)] pub is_recall_context: bool` (default `false`). Wire-transparent — skipped by serde on serialise/deserialise. No sentinel string in content; detection is purely flag-based, no content parsing anywhere.

- **D-02:** Eviction timing is **pre-turn, before re-injection**. At the top of each user turn: `messages.retain(|m| !m.is_recall_context)` runs first, then `prefetch_with_query` is called, then (if non-empty) the fresh recall is inserted. The buffer never holds more than one recall injection at a time.

### Compression treatment

- **D-03:** `ContextCompressor::compress()` adds **step 0**: `messages.retain(|m| !m.is_recall_context)` before its normal tool-result pruning and middle-drop logic. Recall messages are the lowest-priority content — first to go when context is tight. They are re-derivable next turn.

- **D-04:** **No mid-turn re-injection** after the compressor fires. If the compressor drops the recall message during a multi-tool-call sequence, the loop continues without recall for the remainder of that turn. Fresh recall is injected at the start of the next user turn.

### Scrubber pipeline stage

- **D-05:** `StreamingContextScrubber` intercepts at the **delta-decode layer** — each SSE/WebSocket delta chunk passes through `scrubber.feed(delta)` before being written to the output buffer. Mirrors Python's placement; handles `<memory-context>` tags split across chunk boundaries correctly.

- **D-06:** **New scrubber per turn** — created at agent run start, dropped at stream end. `reset()` method kept in the API for completeness but not required by this lifecycle. No risk of state bleeding between turns.

- **D-07:** All 3 surfaces use the same delta-scrub pattern:
  - CLI `run_chat`: each streaming callback delta → `scrubber.feed(delta)` → print
  - Gateway `handle_with_multimodal`: each streaming event → `scrubber.feed(delta)` → send
  - Web UI `run_web_turn`: each WebSocket delta → `scrubber.feed(delta)` → `ws_tx.send`

### Empty-recall cache guard

- **D-08 (amended 2026-05-20):** When `prefetch_with_query` returns empty (all providers return `""`), **skip the new INSERT** — `build_memory_context_block` returns `None`, so no `<memory-context>` system message is added that turn. The prior-turn recall message is **still evicted** via the unconditional `messages.retain(|m| !m.is_recall_context)` at turn start. That retain is a no-op on a session that has never had recall injected (e.g. the file-provider-only common case), so the buffer remains byte-identical to a pre-34a session there. Unconditional eviction is required for correctness: gating it behind non-empty recall would let a turn-1 recall message persist into a later empty-recall turn (stale-recall bug). This is an **explicit acceptance criterion** in Plan 34a-02. For the common case (no semantic provider configured), `prefetch_with_query` returns `""` immediately and the retain is a no-op, preserving prompt-prefix cache hits.
  - *Original wording said "no `retain()` call" on empty; that conflated "skip injection" (correct) with "skip eviction" (a bug — prior recall must always be evicted). Reconciled per plan-checker finding + user decision on 2026-05-20.*

### Claude's Discretion

- Exact position calculation for `last_user_msg_index()` — implementer picks the cleanest approach (scan from end, find last `role: User` message)
- `Default::default()` impl for `ChatMessage` (needed for struct-update syntax when inserting recall messages) — add if not already present
- Whether `#[serde(default)]` is also needed on `is_recall_context` for forward-compat deserialisation of old message payloads — implementer decides based on usage

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Python reference implementation
- `../hermes-agent/agent/memory_manager.py` — canonical port target; `prefetch_all`, `build_memory_context_block`, `sanitize_context`, `StreamingContextScrubber` are the key symbols

### Draft plan (parity matrix + success criteria)
- `.planning/phases/34a-read-side-memory-parity/34a-PLAN-DRAFT.md` — complete parity matrix, 8 success criteria, plan breakdown, open questions (now resolved), verification recipe

### Memory subsystem (ironhermes-core)
- `crates/ironhermes-core/src/memory_provider.rs` — `MemoryProvider` trait; add `prefetch_with_query` here with default no-op
- `crates/ironhermes-core/src/memory_store.rs` — file-backed `MemoryStore`; must NOT need changes (default no-op covers it)
- `crates/ironhermes-core/src/types.rs` — `ChatMessage` struct; add `#[serde(skip)] pub is_recall_context: bool`

### Memory manager + agent loop (ironhermes-agent)
- `crates/ironhermes-agent/src/memory/manager.rs` — `MemoryManager`; add `prefetch_with_query` proxy method joining provider results
- `crates/ironhermes-agent/src/agent_loop.rs` — pre-turn injection site (lines ~1041–1055 reference the existing `queue_prefetch` call site); D-01/D-02/D-04/D-08 all land here
- `crates/ironhermes-agent/src/context_compressor.rs` — add step 0 `retain` for D-03/D-04
- `crates/ironhermes-agent/src/prompt_builder.rs` — read to confirm D-12: system prompt assembly must not be modified

### Streaming surfaces (3 wiring points)
- `crates/ironhermes-cli/src/main.rs` — `run_chat` streaming path (D-07 CLI wiring)
- `crates/ironhermes-gateway/src/handler.rs` — `handle_with_multimodal` streaming path (D-07 gateway wiring)
- `crates/iron_hermes_ui/src/server/state.rs` — `run_web_turn` WebSocket streaming path (D-07 web wiring)

### Regression gates (must stay green)
- Phase 32: `cargo test -p ironhermes-agent --lib nudge::tests` — 6/6
- Phase 33: `cargo test -p ironhermes-agent --test invariants_33` — 6/6
- D-12 gate: `cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load`

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MemoryManager::queue_prefetch(query)` already exists in `manager.rs` — the new `prefetch_with_query` is an awaited (synchronous from the caller's perspective) variant; the post-turn background warm-up path is unchanged
- `messages.retain(...)` pattern already used in agent_loop.rs for other eviction purposes — step 0 in compressor follows the same idiom
- `PromptBuilder::build_system_prompt_block()` and `MemoryManager::system_prompt_block()` already handle the frozen snapshot (D-12) — do not touch these

### Established Patterns
- All `MemoryProvider` methods have default no-op impls for optional capabilities — `prefetch_with_query` follows the same pattern; file provider gets a no-op, semantic providers override
- `#[serde(skip)]` is used elsewhere in the codebase for runtime-only fields; `is_recall_context` follows the same convention
- Agent loop streaming callbacks are closures passed into `AgentLoop::run` — scrubber instance moves into the closure naturally

### Integration Points
- `ChatMessage` schema change touches `ironhermes-core` — all downstream crates (agent, gateway, cli, ui) pick up the new field automatically via `#[serde(skip)]` (no wire-format change, no migration needed)
- `ContextCompressor` is behind `tokio::sync::Mutex<ContextCompressor>` in agent_loop — step 0 `retain` runs inside the existing lock scope
- Web UI `run_web_turn` WebSocket sender is `ws_tx: mpsc::Sender<WsMessage>` — scrubber wraps the delta before the send call

</code_context>

<specifics>
## Specific Ideas

- The injection point is "immediately before the most recent user message" — scan from the end of `messages` to find the last `role: User` entry, insert at that index. This ensures the recall context is maximally local to the user query.
- The `build_memory_context_block` wrapper text must match Python byte-for-byte:
  ```
  <memory-context>
  [System note: The following is recalled memory context, NOT new user input. Treat as authoritative reference data — this is the agent's persistent memory and should inform all responses.]

  {sanitized raw}
  </memory-context>
  ```
- Scrubber internal state: `in_span: bool` + `buf: String`. The `buf` holds back the tail of a partial open or close tag until the next delta confirms whether it's a real tag or ordinary text.
- The empty-recall skip (D-08) means that for users with only the file provider (the common case today), Phase 34a has zero runtime overhead beyond the `prefetch_with_query` call returning `""` immediately.

</specifics>

<deferred>
## Deferred Ideas

- **`@`-reference expansion** (`@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, `@url:`) — Phase 34b
- **`ContextEngine` lifecycle hooks** (`on_session_start`, `on_session_reset`, `update_from_response`, `update_model`, `has_content_to_compress`) — Phase 34b
- **`ContextCompressor` counter reset** — Phase 34b
- **`MemoryProvider` hooks** `on_turn_start` / `on_session_switch` / `on_delegation` — future phase
- **"Only one external provider" guard** — future phase
- **`on_pre_compress` returns text** — future phase
- **LCM-style engine tools** (`lcm_grep`, `lcm_describe`, `lcm_expand`) — when an LCM engine lands

</deferred>

---

*Phase: 34a-read-side-memory-parity*
*Context gathered: 2026-05-16*
