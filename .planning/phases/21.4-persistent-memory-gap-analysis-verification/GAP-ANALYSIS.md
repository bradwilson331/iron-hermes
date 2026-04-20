# Phase 21.4: Persistent Memory Gap Analysis

**Date:** 2026-04-20
**Auditor:** Phase 21.4 executor (automated code audit)
**Comparison targets:**
- `REFERENCE-hermes-agent-memory.md` (hermes-agent persistent memory reference)
- 6 automatic provider behaviors described by user in CONTEXT.md D-01

---

## Section 1: Feature Comparison Table

| Feature | hermes-agent behavior | IronHermes status | Gap severity | Resolution |
|---------|----------------------|-------------------|--------------|------------|
| MEMORY.md store — add/replace/remove | Bounded curated notes, substring matching for replace/remove | IMPLEMENTED — `MemoryStore.add/replace/remove` with substring matching | None | — |
| MEMORY.md char limit (2,200) | Hard cap; returns error with entries when full | IMPLEMENTED — `MEMORY_CHAR_LIMIT` constant, capacity management in `MemoryStore` | None | — |
| USER.md store — add/replace/remove | Separate user profile store, same operations | IMPLEMENTED — `MemoryTarget::User` path in `MemoryStore` | None | — |
| USER.md char limit (1,375) | Hard cap; same capacity error pattern | IMPLEMENTED — `USER_CHAR_LIMIT` constant | None | — |
| Frozen snapshot pattern | Snapshot captured once at session start; mid-session writes persist to disk but prompt never mutated | IMPLEMENTED — `MemoryStore.snapshot` field, `format_for_system_prompt` reads from `snapshot` not `entries` | None (MEM-06 VERIFIED) | — |
| System prompt format — header with usage % | `══════` header, store name, usage %, char counts | IMPLEMENTED — `format_for_system_prompt` generates capacity header | None | — |
| System prompt format — `§` delimiters | Entries separated by section-sign `§` | IMPLEMENTED — `ENTRY_DELIMITER` constant | None | — |
| Duplicate prevention | Exact duplicate entries rejected | IMPLEMENTED — `MemoryStore.add` checks for existing entries | None | — |
| Security scanning | Injection/exfiltration patterns scanned before acceptance | IMPLEMENTED — `scan_context_content` on all memory writes | None | — |
| Capacity management error | Error with current entries when full | IMPLEMENTED — `MemoryResult::Err` with entries listed | None | — |
| Session search via FTS5 | SQLite state.db with FTS5, `session_search` tool | IMPLEMENTED — `StateStore` with FTS5 in Phase 13 | None (out of phase scope) | — |
| Config toggle — `memory_enabled` | Bool, default `true`; `false` skips entire subsystem | MISSING — `MemoryConfig` has only `provider` + `mirror_provider` | **Major (GAP-4)** | Plan 02 |
| Config toggle — `user_profile_enabled` | Bool, default `true`; `false` skips USER.md only | MISSING — not present in `MemoryConfig` | **Major (GAP-4)** | Plan 02 |
| `hermes memory setup` subcommand | Interactive provider setup wizard | IMPLEMENTED — `MemorySubcommand::Setup` in `main.rs:113` | None | — |
| `hermes memory status` subcommand | Shows provider, availability, store sizes/capacities, mirror | MISSING — `MemorySubcommand` only has `Setup` variant | **Major (GAP-5)** | Plan 03 |
| `hermes memory off` subcommand | Sets provider to built-in file, disables external | MISSING — not in `MemorySubcommand` | **Major (GAP-5)** | Plan 03 |
| External provider support (SQLite) | Run alongside built-in; semantic search, fact extraction | IMPLEMENTED — `memory-sqlite` feature | None | — |
| External provider support (Grafeo) | Graph DB backend | IMPLEMENTED — `memory-grafeo` feature | None | — |
| External provider support (DuckDB) | Analytical DB backend | IMPLEMENTED — `memory-duckdb` feature | None | — |
| Mirror fanout on writes | `on_memory_write` dispatched to mirror after successful primary write | IMPLEMENTED — `MemoryManager.handle_tool_call` with 5s timeout (Phase 20) | None | — |
| Provider tool schemas | Provider-specific tools exposed via `get_tool_schemas` | IMPLEMENTED — `MemoryManager.get_tool_schemas` delegates to primary | None | — |

---

## Section 2: Provider Lifecycle Hook Audit

hermes-agent defines 6 automatic provider behaviors. Each maps to a `MemoryProvider` trait hook. This section assesses end-to-end wiring for each.

### Behavior 1: Inject provider context into system prompt

**Hook:** `system_prompt_block()`
**Manager method:** `MemoryManager.system_prompt_block()` → delegates to primary
**Fire site:** `prompt_builder.rs` `load_memory()` (lines 404-411)
**CLI wired?** YES — `run_single` (main.rs:415) and `run_chat` (main.rs:691) both call `prompt_builder.load_memory().await`
**Gateway wired?** YES — `run_gateway` (runner.rs:737) calls `load_memory` once per session
**Status:** WIRED

For built-in file provider, `system_prompt_block()` returns `None` (no-op). External providers that want to inject a custom context block can override the default. Infrastructure is correct.

---

### Behavior 2: Prefetch relevant memories before each turn

**Hook:** `queue_prefetch(query: &str)`
**Manager method:** `MemoryManager.queue_prefetch()`
**Fire site:** `agent_loop.rs:542-564` (natural-end break, fires as `tokio::spawn`)
**CLI wired?** NO — GAP-1 (Critical)

`run_agent_turn` (main.rs:974-993) builds `AgentLoop` without `.with_memory_manager(...)`. The `AgentLoop.run()` natural-end block at line 545 guards with `if let Some(ref mgr) = self.memory_manager` — which is always `None` in CLI mode because `memory_manager` is never passed to `run_agent_turn`.

**Gateway wired?** NO — GAP-3 (Critical)

`handler.rs:469-472` builds `AgentLoop` without `.with_memory_manager(...)` despite `self.memory_manager` being set on the handler struct (field at line 56, setter at line 149).

**Status:** GAP (critical in both CLI and gateway paths)

---

### Behavior 3: Sync conversation turns after each response

**Hook:** `sync_turn(session_id, entries)`
**Fire site:** `memory_flush_handler.rs` via `ContextPreCompress` hook
**CLI wired?** YES — Phase 18 wired `sync_turn` through the compression hook pipeline
**Gateway wired?** YES — same pipeline applies
**Status:** WIRED

---

### Behavior 4: Extract memories on session end

**Hook:** `on_session_end(session_id, entries)`
**Manager method:** `MemoryManager.on_session_end()`
**Fire site:** None in any production code path
**CLI wired?** NO (Partial) — GAP-6 (Minor)

`run_chat` main.rs:873 has comment: "on_session_end requires MemoryEntries; skip with debug log" in the `ExitCleanly` path. No call to `on_session_end` in clean-exit paths of `run_chat`, `run_single`, or gateway session end.

For the built-in file provider, `on_session_end` is a no-op (data persisted immediately on write). External providers that implement fact extraction depend on this call for end-of-session processing (behavior #4 of 6).

**Status:** PARTIAL GAP — GAP-6 (Minor)

---

### Behavior 5: Mirror built-in memory writes to external provider

**Hook:** `on_memory_write(action, target, content)`
**Manager method:** `MemoryManager.handle_tool_call()` → on success → `mirror.on_memory_write(...)` with 5s timeout
**Fire site:** `MemoryManager.handle_tool_call` in `manager.rs`
**CLI wired?** YES — `run_chat` and `run_single` both call `registry.register_memory_tool(memory_manager.clone())`
**Gateway wired?** YES — `runner.rs` wires `memory_manager` to registry
**Status:** WIRED (Phase 20, Plan 20-02)

---

### Behavior 6: Add provider-specific tools to registry

**Hook:** `get_tool_schemas()`
**Manager method:** `MemoryManager.get_tool_schemas()` → delegates to primary
**Fire site:** Tool registration at startup in each entry point
**CLI wired?** YES — `registry.register_memory_tool(memory_manager.clone())` in all CLI entry points
**Gateway wired?** YES — same registration in gateway runner
**Status:** WIRED

---

### Additional: on_pre_compress (PRMT-16 safety requirement)

Not one of the 6 automatic behaviors, but required by PRMT-16 ("Memory flushed to disk before compression to prevent data loss").

**Hook:** `on_pre_compress(messages: &[ChatMessage])`
**Manager method:** `MemoryManager.on_pre_compress()`
**Fire site:** Context engines have `with_memory_manager()` (on concrete types only — `LocalPruningEngine` and `SummarizingEngine`), but this method is NOT on the `ContextEngine` trait
**CLI wired?** NO — GAP-2 (Critical)

`attach_context_engine` (agent_wiring.rs:43) and `build_context_engine` (engine_factory.rs:33) do not accept a `memory_manager` parameter. Both engines are Arc-wrapped via `Arc::new(e)` before return. Since `with_memory_manager()` is on the concrete type only (not the `ContextEngine` trait), it must be called inside `build_context_engine` before `Arc::new()`.

**Gateway wired?** NO — same root cause (same factory function)
**Status:** GAP (Critical in all paths)

---

## Section 3: Hook Wiring Matrix

| Hook | Trait Method | Manager Method | Fire Site | CLI Wired? | Gateway Wired? | Status |
|------|-------------|----------------|-----------|-----------|----------------|--------|
| `initialize` | `initialize(session_id, hermes_home, config)` | N/A (factory-level) | `build_memory_provider` / `build_tokio_provider` in `factory.rs` | YES | YES | WIRED |
| `load_from_disk` | `load_from_disk()` | N/A (factory-level) | `build_memory_provider` / `build_tokio_provider` in `factory.rs` | YES | YES | WIRED |
| `prefetch` | `prefetch(session_id)` | Called via `format_for_system_prompt` at session start | `prompt_builder.rs:load_memory()` | YES (once at session start) | YES (once per session) | WIRED (session-start only) |
| `sync_turn` | `sync_turn(session_id, entries)` | N/A (fires on provider directly) | `memory_flush_handler.rs` via ContextPreCompress hook | YES | YES | WIRED |
| `on_session_end` | `on_session_end(session_id, entries)` | `MemoryManager.on_session_end()` | Nowhere (skipped) | NO | NO | GAP-6 (Minor) |
| `queue_prefetch` | `queue_prefetch(query)` | `MemoryManager.queue_prefetch()` | `agent_loop.rs:542-564` (natural-end break, `tokio::spawn`) | NO — AgentLoop built without `memory_manager` in `run_agent_turn` | NO — AgentLoop built without `memory_manager` in `handler.rs:469` | GAP-1 (CLI), GAP-3 (Gateway), Critical |
| `on_pre_compress` | `on_pre_compress(messages)` | `MemoryManager.on_pre_compress()` | Concrete engines have fire site, but never invoked | NO — `build_context_engine` / `attach_context_engine` have no `memory_manager` param | NO — same root cause | GAP-2 (Critical) |
| `on_memory_write` | `on_memory_write(action, target, content)` | `MemoryManager.handle_tool_call()` → mirror fanout | Tool intercept in `MemoryManager.handle_tool_call()` | YES | YES | WIRED (Phase 20) |
| `system_prompt_block` | `system_prompt_block()` | N/A (fires on primary directly) | `prompt_builder.rs:load_memory()` lines 404-411 | YES | YES | WIRED |
| `get_tool_schemas` | `get_tool_schemas()` | `MemoryManager.get_tool_schemas()` | Tool registry registration at startup | YES | YES | WIRED |
| `shutdown` | `shutdown()` | `MemoryManager.shutdown()` | Gateway session cleanup | N/A (process exit) | YES | WIRED (gateway) |

**Summary:** 7 hooks wired, 4 hooks have gaps (queue_prefetch × 2 paths = GAP-1+3, on_pre_compress = GAP-2, on_session_end = GAP-6)

---

## Section 4: MEM-06 Verification

**Requirement:** Memory snapshots are frozen at session start. Mid-session writes persist to disk immediately but the active system prompt is not mutated. The frozen snapshot preserves the LLM prefix cache.

### Evidence

**MemoryStore snapshot field** (`crates/ironhermes-core/src/memory_store.rs:59-62`)

```rust
pub struct MemoryStore {
    /// Live entries keyed by target -- disk-authoritative for mutations.
    entries: HashMap<MemoryTarget, Vec<String>>,
    /// Frozen snapshot captured at load_from_disk(), never mutated after (D-12).
    snapshot: HashMap<MemoryTarget, Vec<String>>,
    memory_dir: PathBuf,
}
```

**load_from_disk captures snapshot** (`memory_store.rs:80-99`)

```rust
pub fn load_from_disk(&mut self) -> anyhow::Result<()> {
    // ... reads file, splits by ENTRY_DELIMITER ...
    // Capture frozen snapshot for prompt injection (store raw entries, D-12)
    if !entries.is_empty() {
        self.snapshot.insert(*target, entries.clone());
    }
    self.entries.insert(*target, entries);
}
```

**format_for_system_prompt reads from snapshot, not entries** (`memory_store.rs:329-348`)

```rust
pub fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
    let entries = self.snapshot.get(&target)?;  // <-- snapshot, never self.entries
    // ...
}
```

**load_memory called exactly once per entry point:**

| Entry point | File | Line | Timing |
|-------------|------|------|--------|
| `run_single` | `crates/ironhermes-cli/src/main.rs` | 416 | Before `build_system_message` |
| `run_chat` | `crates/ironhermes-cli/src/main.rs` | 692 | Before REPL loop starts |
| `run_gateway` | `crates/ironhermes-gateway/src/runner.rs` | ~737 | Once per session-start |

**Test coverage:** `test_snapshot_frozen_after_load` in `crates/ironhermes-core/src/memory_store.rs` (line 737-755) pins the frozen-snapshot contract as an automated regression test.

### Conclusion

**MEM-06 is VERIFIED. No gap.** The frozen-snapshot pattern is correctly implemented with appropriate test coverage. No fix needed.

---

## Section 5: Concrete Gaps Catalogue

### GAP-1 — Critical: `queue_prefetch` never fires in CLI chat mode

**ID:** GAP-1
**Severity:** Critical
**Description:** `queue_prefetch` is the mechanism for pre-warming the external provider's cache with the most recent user query before the next turn. This is hermes-agent automatic behavior #2 (prefetch before each turn). In CLI chat mode, `AgentLoop` is built in `run_agent_turn` without a `memory_manager`, so the natural-end-break code that fires `queue_prefetch` (agent_loop.rs:545 guard `if let Some(ref mgr) = self.memory_manager`) never executes.

**Evidence:**
- `crates/ironhermes-cli/src/main.rs:974-993` — `AgentLoop::new(...)` builder chain in `run_agent_turn` has no `.with_memory_manager(...)` call
- `memory_manager` is constructed at `main.rs:536` in `run_chat` and passed to `prompt_builder` but not into `run_agent_turn`'s function signature
- `crates/ironhermes-agent/src/agent_loop.rs:545` — guard `if let Some(ref mgr) = self.memory_manager` is always `None` in CLI

**hermes-agent equivalent:** Behavior #2 — Prefetch relevant memories before each turn

**Fix plan:** Plan 02 — Add `memory_manager: Arc<tokio::sync::Mutex<MemoryManager>>` parameter to `run_agent_turn`, call `.with_memory_manager(memory_manager.clone())` on the `AgentLoop` builder chain, wire at all three `run_agent_turn` call sites inside `run_chat`.

**Status:** OPEN

---

### GAP-2 — Critical: `on_pre_compress` never fires in any code path

**ID:** GAP-2
**Severity:** Critical
**Description:** `on_pre_compress` is the PRMT-16 safety hook that flushes memory to disk before destructive context compression, preventing data loss. Both `LocalPruningEngine` and `SummarizingEngine` have `with_memory_manager()` methods and fire `on_pre_compress` before compression. However, `build_context_engine` (engine_factory.rs:33) does not accept a `memory_manager` parameter and never calls `.with_memory_manager(...)` on the engines it builds. The `attach_context_engine` wrapper (agent_wiring.rs:43) also has no `memory_manager` parameter.

Crucially, `with_memory_manager()` is a method on the **concrete engine types only** (not on the `ContextEngine` trait). The factory boxes the engine into `Arc<dyn ContextEngine>` at the end, so the fix must apply `with_memory_manager()` inside `build_context_engine` **before** `Arc::new()` wrapping.

**Evidence:**
- `crates/ironhermes-agent/src/engine_factory.rs:33` — `build_context_engine` signature: no `memory_manager` parameter; `Arc::new(e)` at end of each match arm
- `crates/ironhermes-agent/src/agent_wiring.rs:43` — `attach_context_engine` signature: no `memory_manager` parameter
- `crates/ironhermes-agent/src/context_engine.rs` — `with_memory_manager()` on `LocalPruningEngine` concrete struct, not on `ContextEngine` trait
- `crates/ironhermes-agent/src/summarizing_engine.rs` — `with_memory_manager()` on `SummarizingEngine` concrete struct

**hermes-agent equivalent:** PRMT-16 memory flush before compression (safety requirement)

**Fix plan:** Plan 02 — Add `memory_manager: Option<Arc<tokio::sync::Mutex<MemoryManager>>>` to `build_context_engine` and `attach_context_engine`. Call `.with_memory_manager(mgr)` on each concrete engine type inside `build_context_engine` before `Arc::new()`. Update all callers to pass `Some(memory_manager.clone())` where available.

**Status:** OPEN

---

### GAP-3 — Critical: `queue_prefetch` never fires in gateway mode

**ID:** GAP-3
**Severity:** Critical
**Description:** Same root cause as GAP-1 but in the gateway path. The gateway's `AgentLoop` is built in `handler.rs:469-472` without `.with_memory_manager(...)` despite `self.memory_manager` being set on the handler struct (field at line 56). The `memory_manager` is already passed to the `prompt_builder` (line 382-388) but not to the `AgentLoop`.

**Evidence:**
- `crates/ironhermes-gateway/src/handler.rs:469-472`:
  ```rust
  let mut agent = AgentLoop::new(client, self.tool_registry.clone(), max_turns)
      .with_streaming(stream_callback)
      .with_tool_progress(tool_callback)
      .with_active_skills(self.active_skills.clone());
  // NO .with_memory_manager(...) call
  ```
- `handler.rs:56` — `memory_manager: Option<Arc<TokioMutex<MemoryManager>>>` field exists
- `handler.rs:382-388` — `memory_manager` IS passed to `prompt_builder` (prompt injection works)
- `agent_loop.rs:545` — guard `if let Some(ref mgr) = self.memory_manager` always `None`

**hermes-agent equivalent:** Behavior #2 — Prefetch relevant memories before each turn

**Fix plan:** Plan 02 — In `handle_agent_request` (handler.rs), add conditional wiring after the initial `AgentLoop` builder: `if let Some(ref mgr) = self.memory_manager { agent = agent.with_memory_manager(mgr.clone()); }`

**Status:** OPEN

---

### GAP-4 — Major: `memory_enabled` and `user_profile_enabled` config toggles missing

**ID:** GAP-4
**Severity:** Major
**Description:** The hermes-agent reference config schema specifies `memory_enabled` (bool) and `user_profile_enabled` (bool) toggles. IronHermes `MemoryConfig` has only `provider` and `mirror_provider`. Without `memory_enabled`, there is no way to disable the entire memory subsystem via config. Without `user_profile_enabled`, there is no way to disable USER.md while keeping MEMORY.md active.

**Evidence:**
- `crates/ironhermes-core/src/config.rs:136-153`:
  ```rust
  pub struct MemoryConfig {
      pub provider: String,
      pub mirror_provider: Option<String>,
      // memory_enabled: MISSING
      // user_profile_enabled: MISSING
  }
  ```
- `REFERENCE-hermes-agent-memory.md` shows:
  ```yaml
  memory:
    memory_enabled: true
    user_profile_enabled: true
    memory_char_limit: 2200
    user_char_limit: 1375
  ```

**hermes-agent equivalent:** Config schema parity (D-07/D-08)

**Fix plan:** Plan 02 —
1. Add `memory_enabled: bool` (default `true`) and `user_profile_enabled: bool` (default `true`) to `MemoryConfig` with `#[serde(default = "...")]`
2. Change `build_memory_manager` return type to `Option<Arc<...>>` — returns `None` when `memory_enabled=false`
3. Update all 3 callers in `main.rs` to handle `Option<Arc<...>>`
4. In `prompt_builder.rs` `load_memory()`: skip `MemoryTarget::User` when `user_profile_enabled=false`
5. In `MemoryTool`: reject writes to `MemoryTarget::User` with clear error when `user_profile_enabled=false`

**Status:** OPEN

---

### GAP-5 — Major: `hermes memory status` and `hermes memory off` subcommands missing

**ID:** GAP-5
**Severity:** Major
**Description:** `MemorySubcommand` (main.rs:113-116) has only `Setup`. hermes-agent exposes `hermes memory status` and `hermes memory off`. These are important operational tools: `status` lets the user inspect the memory subsystem state without entering the agent, and `off` provides a safe way to revert to the built-in file provider.

**Evidence:**
- `crates/ironhermes-cli/src/main.rs:113-116`:
  ```rust
  enum MemorySubcommand {
      /// Interactive setup for the currently-selected memory provider.
      Setup,
  }
  ```
- `REFERENCE-hermes-agent-memory.md:98-99`:
  ```
  hermes memory setup      # pick a provider and configure it
  hermes memory status     # check what's active
  ```
  (D-10 also specifies `hermes memory off`)

**hermes-agent equivalent:** CLI surface parity (D-09/D-10)

**Fix plan:** Plan 03 —
- `Status` variant: reads config, locks memory_manager, displays active provider name + `is_available()` status, MEMORY.md size/capacity/entries, USER.md size/capacity/entries, mirror status. Uses `colored` crate output pattern from `memory_setup.rs` and `cron.rs`.
- `Off` variant: loads config, checks if provider already is `"file"`, otherwise sets `config.memory.provider = "file"` and `config.memory.mirror_provider = None`, saves via `Config::save_to()`. Does NOT set `memory_enabled=false`.

**Status:** OPEN

---

### GAP-6 — Minor: `on_session_end` not called in clean exit paths

**ID:** GAP-6
**Severity:** Minor
**Description:** `MemoryManager::on_session_end` exists and is correctly implemented, but is never called in any production code path. The `run_chat` exit comment at main.rs:873 acknowledges: "on_session_end requires MemoryEntries; skip with debug log." `run_single` (natural end) and gateway session expiry also skip it.

For the built-in file provider this is a no-op (data is persisted immediately on each `add/replace/remove` write). However, external providers implementing hermes-agent behavior #4 (extract memories on session end — e.g., fact extraction, knowledge graph update) rely on this call.

**Evidence:**
- `crates/ironhermes-cli/src/main.rs:873` — comment confirms intentional skip in ctrl-c path
- No `on_session_end` call anywhere in `run_chat`, `run_single`, or gateway runner clean-exit paths
- `crates/ironhermes-agent/src/memory/manager.rs` — `on_session_end` method exists and is implemented

**hermes-agent equivalent:** Behavior #4 — Extract memories on session end

**Fix plan:** Plan 03 — Call `on_session_end` in the natural-end path of `run_single` (async context trivially available) and in the `run_chat` clean-exit path. Use `MemoryEntries::default()` as a best-effort argument for the ctrl-c case (providers use their own internal state). The true ctrl-c path remains intentionally skipped (async context unsuitable).

**Status:** OPEN

---

## Section 6: Summary Statistics

### Overview

| Metric | Count |
|--------|-------|
| Features assessed | 21 |
| Features fully implemented (no gap) | 15 |
| MEM-06 verified (no fix needed) | 1 |
| Gaps found | 6 |
| Critical gaps | 3 (GAP-1, GAP-2, GAP-3) |
| Major gaps | 2 (GAP-4, GAP-5) |
| Minor gaps | 1 (GAP-6) |
| Hooks assessed (MemoryProvider trait) | 11 |
| Hooks fully wired | 7 |
| Hooks with gaps | 4 (queue_prefetch in 2 paths, on_pre_compress, on_session_end) |

### Already Correct

The following features are implemented correctly and require no changes:

- MEM-06 frozen snapshot pattern (MemoryStore.snapshot, load_from_disk, format_for_system_prompt)
- `system_prompt_block` wiring (prompt_builder.rs load_memory, all 3 entry points)
- `on_memory_write` mirror fanout (MemoryManager.handle_tool_call, Phase 20)
- `sync_turn` via memory_flush_handler.rs / ContextPreCompress hook (Phase 18)
- `get_tool_schemas` / tool registration (Phase 20)
- `initialize` + `load_from_disk` in factory
- MEMORY.md / USER.md store operations (add/replace/remove, substring matching)
- Duplicate prevention + security scanning
- Capacity management with error responses
- File provider + SQLite + Grafeo + DuckDB provider implementations
- `hermes memory setup` wizard

### Gaps by Fix Plan

| Fix Plan | Gaps | Description |
|----------|------|-------------|
| Plan 02 | GAP-1, GAP-2, GAP-3, GAP-4 | Lifecycle hook wiring + config toggles (3 critical + 1 major) |
| Plan 03 | GAP-5, GAP-6 | CLI subcommands + on_session_end (1 major + 1 minor) |

### Gap Status Tracking

After Plans 02 and 03 execute, this table will be updated to reflect CLOSED status:

| Gap ID | Severity | Description | Fix Plan | Status |
|--------|----------|-------------|----------|--------|
| GAP-1 | Critical | `queue_prefetch` not fired in CLI chat mode | Plan 02 | OPEN |
| GAP-2 | Critical | `on_pre_compress` not fired in any path | Plan 02 | OPEN |
| GAP-3 | Critical | `queue_prefetch` not fired in gateway mode | Plan 02 | OPEN |
| GAP-4 | Major | `memory_enabled` + `user_profile_enabled` toggles missing | Plan 02 | OPEN |
| GAP-5 | Major | `hermes memory status` + `hermes memory off` missing | Plan 03 | OPEN |
| GAP-6 | Minor | `on_session_end` not called on clean exit | Plan 03 | OPEN |

---

*Report produced: 2026-04-20*
*Baseline for: Phase 21.4 Plans 02 and 03*
*Update policy: After each plan executes, set relevant gap STATUS to CLOSED*
