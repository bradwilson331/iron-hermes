---
created: 2026-04-16T06:30:00.000Z
title: Chat and single CLI modes have no memory wiring
area: cli
phase: 20-memory-provider-plugin-contract
plan: 20-03
files:
  - crates/ironhermes-cli/src/main.rs:243-345
  - crates/ironhermes-cli/src/main.rs:348-541
  - crates/ironhermes-agent/src/prompt_builder.rs:370-394
  - crates/ironhermes-agent/src/memory/factory.rs
  - crates/ironhermes-tools/src/delegate_task.rs
---

## Problem

Reproduction: `cargo run -p ironhermes-cli --features memory-sqlite -- chat`. Tell the bot a fact (e.g. favorite color). Ask an intervening question. Ask for the fact back — it "remembers". Stop and restart. Ask again — the bot does not remember.

Within a chat-mode session, the model "remembers" only because messages stay in the conversation history. Nothing is ever written to SQLite.

`crates/ironhermes-cli/src/main.rs`:

- `run_single` (line 243) and `run_chat` (line 348) never call `build_memory_provider`.
- They never register the memory tool on the `ToolRegistry` (contrast `run_gateway:617`).
- They never `set_memory_store` on the `PromptBuilder`.
- The calls to `prompt_builder.load_memory()` at lines 281 and 425 are no-ops because `memory_store` on the builder is `None`.
- `delegate_task` registration passes `None` for the memory store at lines 265 and 410 (gateway passes `memory_store.clone()` at line 669).

Even after the separate gateway factory fix lands (see sibling todo `gateway-memory-does-not-persist-across-restart-factory-never`), chat mode will still not persist across restarts because there is nothing wired to write to SQLite in the first place.

## Solution

Lands as part of **Plan 20-03** (Phase 20: Memory Provider Plugin Contract), alongside the `hermes memory setup` wizard — memory gaining a UI in chat mode pairs naturally with the configuration surface that manages it.

In `run_single` and `run_chat`:

1. Call `ironhermes_agent::memory::factory::build_memory_provider(&config.memory)?`.
2. `registry.register_memory_tool(memory_store.clone())` so the LLM can call `memory_add` / `memory_replace` / `memory_remove`.
3. `prompt_builder.set_memory_store(memory_store.clone())` before `load_memory()` so slot 3 receives the snapshot.
4. Decide whether `delegate_task` subagents in chat mode should share the memory store (currently pass `None` at lines 265 and 410) — gateway passes `memory_store.clone()` at line 669. Pick the consistent answer.
5. Feature-flag story: chat mode today builds without `--features memory-sqlite`, so a `provider = "sqlite"` config will bail. Either document the feature requirement or keep a no-memory fallback when the feature is off. Likely gated on `provider.is_available()` once Plan 20-01 adds that hook.

## Verification

Chat mode: tell bot favorite color → stop → start → `chat` again → ask → remembers. Same for single-prompt mode: `ironhermes <prompt>` that stores a fact, then a follow-up invocation that recalls it.

## Dependencies

- Blocks on Plan 20-01 (factory `load_from_disk` fix + `is_available` hook).
