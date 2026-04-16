---
created: 2026-04-16T06:30:00.000Z
title: Gateway memory does not persist across restart — factory never calls load_from_disk
area: memory
phase: 20-memory-provider-plugin-contract
plan: 20-01
files:
  - crates/ironhermes-agent/src/memory/factory.rs:11-72
  - providers/memory-sqlite/src/lib.rs:34-60
  - providers/memory-sqlite/src/lib.rs:125-135
  - providers/memory-duckdb/src/lib.rs:157
  - providers/memory-grafeo/src/lib.rs:184
---

## Problem

Reproduction: `cargo run -p ironhermes-cli --features memory-sqlite -- gateway`. Tell the bot a fact. Stop and restart. Ask again — the bot does not remember.

`crates/ironhermes-agent/src/memory/factory.rs:11-72` builds `SqliteMemoryProvider::new(...)` (and the duckdb/grafeo analogues) but never invokes `load_from_disk()`. The module comment at line 10 is wrong:

> External backends (sqlite/grafeo/duckdb) persist natively — no explicit load needed.

SQLite rows do persist. But `SqliteMemoryProvider` uses the frozen-snapshot pattern (D-11, `providers/memory-sqlite/src/lib.rs:34-37`): `format_for_system_prompt` (line 369) and `to_memory_entries` (line 392) read **only** from the in-memory `snapshot: HashMap<MemoryTarget, Vec<String>>`. That snapshot starts empty (line 60) and is populated **only** by `load_from_disk` (line 125). On a fresh process:

1. Factory returns a provider with an empty snapshot.
2. `PromptBuilder::load_memory` (`crates/ironhermes-agent/src/prompt_builder.rs:370-394`) calls `format_for_system_prompt`, which returns `None` because the snapshot is empty.
3. The system prompt contains no Memory/User Profile block.
4. The LLM has no way to see prior facts.

Mid-session "remembering" is an illusion: `memory_add` writes to SQLite, but the turn's own conversation history keeps the fact visible in context. Once the process restarts, the DB is full but the snapshot is empty, so the prompt says nothing about it.

Same bug exists in `providers/memory-duckdb/src/lib.rs:157` and `providers/memory-grafeo/src/lib.rs:184` — both implement the same frozen-snapshot pattern and rely on an explicit `load_from_disk` call that the factory never makes.

## Solution

Lands as part of **Plan 20-01** (Phase 20: Memory Provider Plugin Contract).

In `crates/ironhermes-agent/src/memory/factory.rs`:

1. After constructing `SqliteMemoryProvider`, `DuckDbMemoryProvider`, and `GrafeoMemoryProvider`, call `provider.load_from_disk()?` before wrapping in `Arc<Mutex<...>>`.
2. Rewrite the stale module comment (lines 9-10) so it no longer claims external backends skip the load step — the frozen-snapshot contract requires it.
3. Regression test per provider: `new()` → `add()` → drop → re-open the same DB path → construct via factory → `format_for_system_prompt` returns the prior entry. This would have caught the bug.

## Verification

Gateway mode: tell bot favorite color → stop → start → ask → remembers. Same for any other memory fact.
