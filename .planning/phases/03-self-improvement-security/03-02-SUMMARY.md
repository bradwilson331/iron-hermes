---
phase: 03-self-improvement-security
plan: 02
status: complete
started: 2026-04-08
completed: 2026-04-08
---

# Plan 03-02 Summary: Memory Subsystem

## What was built

### Task 1: MemoryStore in ironhermes-core
- `memory_store.rs`: Full MemoryStore with load/add/replace/remove operations
- Two targets: MEMORY.md (2200 char limit) and USER.md (1375 char limit)
- Entry delimiter: `\n§\n` (section sign)
- Advisory file locking via `fs2` crate for concurrent session safety
- Atomic writes: tempfile + fsync + rename for crash-safe persistence
- Injection scanning via `scan_context_content` blocks prompt injection in memory entries
- Duplicate prevention and capacity limit enforcement with JSON error messages
- Frozen snapshot pattern (D-12): `format_for_system_prompt()` returns snapshot captured at `load_from_disk()`, never live state
- Constants added to `constants.rs`: ENTRY_DELIMITER, MEMORY_CHAR_LIMIT, USER_CHAR_LIMIT, filenames

### Task 2: MemoryTool + PromptBuilder + Gateway wiring
- `memory_tool.rs`: MemoryTool implementing Tool trait with add/replace/remove actions
- Uses `ToolSchema::new()` constructor matching existing tool pattern
- No `read` action (D-09) — memory is injected into system prompt
- `registry.rs`: Added `register_memory_tool(Arc<Mutex<MemoryStore>>)` method
- `prompt_builder.rs`: Added `memory_store` field and `set_memory_store()` setter; memory snapshot injected after AGENTS.md content
- `handler.rs`: Added `memory_store` field and `set_memory_store()` setter; injects into PromptBuilder on each message
- `runner.rs`: Added `memory_store` field and `set_memory_store()` setter; passes to handler at startup
- `main.rs`: Creates MemoryStore from `~/.ironhermes/memories/`, loads from disk, registers MemoryTool before Arc wrapping, passes to runner

## Commits
- `c023f55` feat(03-02): add MemoryStore with bounded entries, atomic I/O, and injection scanning
- `1147238` feat(03-02): add MemoryTool, wire into PromptBuilder and gateway startup

## Test results
- 14 tests in `ironhermes-core::memory_store` — all passing
- 6 tests in `ironhermes-tools::memory_tool` — all passing
- 6 tests in `ironhermes-agent::prompt_builder` — all passing
- `cargo check --workspace` — clean

## Files modified
- `Cargo.toml` (workspace: added fs2)
- `crates/ironhermes-core/Cargo.toml` (added fs2)
- `crates/ironhermes-core/src/constants.rs` (memory constants)
- `crates/ironhermes-core/src/lib.rs` (pub mod memory_store, re-exports)
- `crates/ironhermes-core/src/memory_store.rs` (NEW)
- `crates/ironhermes-tools/src/memory_tool.rs` (NEW)
- `crates/ironhermes-tools/src/lib.rs` (pub mod memory_tool)
- `crates/ironhermes-tools/src/registry.rs` (register_memory_tool)
- `crates/ironhermes-agent/src/prompt_builder.rs` (memory injection)
- `crates/ironhermes-gateway/src/handler.rs` (memory store field + injection)
- `crates/ironhermes-gateway/src/runner.rs` (memory store passthrough)
- `crates/ironhermes-cli/src/main.rs` (MemoryStore creation + registration)
