---
created: 2026-04-16T06:03:59.022Z
title: Memory does not persist across gateway restart and chat mode has no memory wiring
area: memory
files:
  - crates/ironhermes-agent/src/memory/factory.rs:11-72
  - providers/memory-sqlite/src/lib.rs:34-60
  - providers/memory-sqlite/src/lib.rs:125-135
  - providers/memory-duckdb/src/lib.rs:157
  - providers/memory-grafeo/src/lib.rs:184
  - crates/ironhermes-cli/src/main.rs:243-345
  - crates/ironhermes-cli/src/main.rs:348-425
---

## Problem

Reproduction: `cargo run -p ironhermes-cli --features memory-sqlite -- gateway`. Tell the bot a fact (e.g. favorite color). Ask an intervening question. Ask the original fact — it remembers. Stop the process, start it again, ask again — the bot does not remember. Same symptom reported for chat/agent mode.

**Two distinct bugs produce this single-session-only behavior.**

### Bug 1 — Factory never calls `load_from_disk()` on external providers (gateway)

`crates/ironhermes-agent/src/memory/factory.rs:11-72` builds `SqliteMemoryProvider::new(...)` (and the duckdb/grafeo analogues) but never invokes `load_from_disk()`. The module comment at line 10 is wrong:

> External backends (sqlite/grafeo/duckdb) persist natively — no explicit load needed.

SQLite rows do persist. But `SqliteMemoryProvider` uses the frozen-snapshot pattern (D-11, `providers/memory-sqlite/src/lib.rs:34-37`): `format_for_system_prompt` (line 369) and `to_memory_entries` (line 392) read **only** from the in-memory `snapshot: HashMap<MemoryTarget, Vec<String>>`. That snapshot starts empty (line 60) and is populated **only** by `load_from_disk` (line 125). On a fresh process:

1. Factory returns a provider with an empty snapshot.
2. `PromptBuilder::load_memory` (`crates/ironhermes-agent/src/prompt_builder.rs:370-394`) calls `format_for_system_prompt`, which returns `None` because the snapshot is empty.
3. The system prompt contains no Memory/User Profile block.
4. The LLM has no way to see prior facts.

Mid-session "remembering" is an illusion: `memory_add` writes to SQLite, but the turn's own conversation history keeps the fact visible in context. Once the process restarts, the DB is full but the snapshot is empty, so the prompt says nothing about it.

Same bug exists in `providers/memory-duckdb/src/lib.rs:157` and `providers/memory-grafeo/src/lib.rs:184` — both implement the same frozen-snapshot pattern and rely on an explicit `load_from_disk` call that the factory never makes.

### Bug 2 — Chat/single modes have no memory wiring at all

`crates/ironhermes-cli/src/main.rs`:

- `run_single` (line 243) and `run_chat` (line 348) never call `build_memory_provider`.
- They never register the memory tool on the `ToolRegistry` (contrast `run_gateway:617`).
- They never `set_memory_store` on the `PromptBuilder`.
- The calls to `prompt_builder.load_memory()` at lines 281 and 425 are no-ops because `memory_store` on the builder is `None`.

Within a chat-mode session, the model "remembers" only because messages stay in the conversation history. Nothing is ever written to SQLite (no memory tool exposed), so even fixing Bug 1 will not make chat mode persist across restarts.

## Solution

### Fix 1 (gateway — small, isolated)

In `crates/ironhermes-agent/src/memory/factory.rs`:

1. After constructing `SqliteMemoryProvider`, `DuckDbMemoryProvider`, and `GrafeoMemoryProvider`, call `provider.load_from_disk()?` before wrapping in `Arc<Mutex<...>>`.
2. Rewrite the stale module comment (lines 9-10) so it no longer claims external backends skip the load step — the frozen-snapshot contract requires it.
3. Add a regression test per provider: `new()` → `add()` → drop → re-open the same DB path → construct via factory → `format_for_system_prompt` returns the prior entry. This would have caught the bug.

### Fix 2 (chat mode — larger wiring change)

In `run_single` and `run_chat`:

1. Call `ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?`.
2. `registry.register_memory_tool(memory_store.clone())` so the LLM can call `memory_add`/`memory_replace`/`memory_remove`.
3. `prompt_builder.set_memory_store(memory_store.clone())` before `load_memory()` so slot 3 receives the snapshot.
4. Decide whether `delegate_task` subagents in chat mode should share the memory store (currently pass `None` at lines 265 and 410) — gateway passes `memory_store.clone()` at line 669. Consistent answer preferred.
5. Decide feature-flag story: chat mode today builds without `--features memory-sqlite`, so a `provider = "sqlite"` config will bail. Either document the feature requirement or keep a no-memory fallback when the feature is off.

Fix 1 should land first to restore gateway persistence. Fix 2 can follow as its own plan — it changes the chat UX contract (memory tool appears in the tool list) and deserves independent review.
