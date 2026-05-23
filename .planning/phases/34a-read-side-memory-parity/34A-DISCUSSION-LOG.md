# Phase 34a: Memory Manager Read-Side Parity - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-05-16
**Phase:** 34a-read-side-memory-parity
**Areas discussed:** Recall message marker, Compression treatment, Scrubber pipeline stage, Empty-recall cache guard

---

## Recall message marker

**Question 1: How to tag injected synthetic system messages as ephemeral?**

| Option | Description | Selected |
|--------|-------------|----------|
| Typed field on ChatMessage | `#[serde(skip)] pub is_recall_context: bool` — clean flag, wire-transparent, no content parsing | ✓ |
| Sentinel prefix in content | `[RECALL]\n` prefix detected/stripped by string operations — avoids schema change, risk of sentinel leaking on bug | |

**User's choice:** Typed field on ChatMessage
**Notes:** `#[serde(skip)]` keeps it wire-transparent; no migration needed.

---

**Question 2: When should prior-turn recall messages be evicted?**

| Option | Description | Selected |
|--------|-------------|----------|
| Evict at pre-turn injection time | `retain(|m| !m.is_recall_context)` at top of each turn before re-injecting — never more than one injection in buffer | ✓ |
| Evict lazily during context compression | Let compressor strip them when it runs — simpler eviction, but stale recall accumulates until compression fires | |

**User's choice:** Evict at pre-turn injection time
**Notes:** Retain + re-inject pattern keeps the buffer clean and predictable.

---

## Compression treatment

**Question 1: How should ContextCompressor handle live recall messages mid-turn?**

| Option | Description | Selected |
|--------|-------------|----------|
| Compressor evicts recall messages freely | Step 0 in `compress()`: `retain(|m| !m.is_recall_context)` before normal pruning — lowest priority, first to go | ✓ |
| Compressor ignores recall messages | Exclude from compression input entirely — risk: can't reclaim those tokens if context is critically full | |
| You decide | Leave to implementer | |

**User's choice:** Compressor evicts recall messages freely
**Notes:** Recall is re-derivable next turn; no reason to protect it from compression.

---

**Question 2: After compressor drops recall mid-turn, re-inject within the same turn?**

| Option | Description | Selected |
|--------|-------------|----------|
| No re-injection mid-turn | Loop continues without recall; fresh recall at start of next user turn — preserves the compression win | ✓ |
| Re-inject after compression | Inject fresh recall immediately after compressor runs — adds complexity, may re-trigger compression | |

**User's choice:** No re-injection mid-turn
**Notes:** Mid-turn recall loss is acceptable; the next user turn will re-inject.

---

## Scrubber pipeline stage

**Question 1: Where should StreamingContextScrubber intercept output?**

| Option | Description | Selected |
|--------|-------------|----------|
| Delta-decode layer — mirrors Python | Each SSE/WebSocket delta → `scrubber.feed(delta)` before output buffer. Handles tags split across chunk boundaries. | ✓ |
| Assembled-message layer | Scrub fully-assembled response after streaming — simpler but partial tags flicker in output during streaming | |

**User's choice:** Delta-decode layer
**Notes:** Mirrors Python placement exactly; the only approach that survives chunk boundaries safely.

---

**Question 2: Scrubber lifecycle — when is reset() called?**

| Option | Description | Selected |
|--------|-------------|----------|
| New scrubber per turn | Fresh instance at agent run start, dropped at stream end — no reset() needed, no state bleed | ✓ |
| Long-lived scrubber with reset() per turn | One instance per session; reset() before each turn — saves tiny allocation, adds statefulness risk | |
| You decide | Leave to implementer | |

**User's choice:** New scrubber per turn
**Notes:** `reset()` kept in API for completeness but not required by this lifecycle choice.

---

**Question 3: Web UI WebSocket scrubbing approach?**

| Option | Description | Selected |
|--------|-------------|----------|
| Same delta pattern | Each WebSocket delta → `scrubber.feed(delta)` → `ws_tx.send` — symmetric with CLI and gateway | ✓ |
| Scrub assembled response | Collect full response, strip tags, send — simpler wiring but tags appear in intermediate WS frames | |

**User's choice:** Same delta pattern
**Notes:** Symmetry across all 3 surfaces makes the implementation and testing straightforward.

---

## Empty-recall cache guard

**Question 1: Skip injection when recall is empty?**

| Option | Description | Selected |
|--------|-------------|----------|
| Yes — explicit acceptance criterion | When all providers return `""`, no `retain()` and no insert — buffer identical to pre-34a session. Make this testable. | ✓ |
| Leave implicit | Already covered by `build_memory_context_block` returning `None` — no need for another criterion | |

**User's choice:** Make it an explicit acceptance criterion
**Notes:** Important for cache-friendliness with the common case (file provider only). User confirmed this should be stated explicitly in Plan 34a-02.

---

## Claude's Discretion

- Exact `last_user_msg_index()` implementation — scan from end of messages vec to find last `role: User`
- Whether `#[serde(default)]` is needed on `is_recall_context` for forward-compat deserialisation
- `Default::default()` impl for `ChatMessage` (struct-update syntax) — add if not already present

## Deferred Ideas

- `@`-reference expansion (`@file:`, `@folder:`, `@diff`, `@staged`, `@git:N`, `@url:`) → Phase 34b
- `ContextEngine` lifecycle hooks → Phase 34b
- `MemoryProvider` hooks `on_turn_start` / `on_session_switch` / `on_delegation` → future phase
- "Only one external provider" guard → future phase
