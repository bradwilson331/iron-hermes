# Architecture Research

**Domain:** Rust AI agent — v2.0 Intelligence & Identity milestone
**Researched:** 2026-04-11
**Confidence:** HIGH (based on direct codebase inspection + hermes-agent reference docs)

---

## Current Codebase State (9 Crates)

The workspace has grown from 7 to 9 crates since PROJECT.md was last updated. Current state:

| Crate | Purpose | v2.0 Relevance |
|-------|---------|----------------|
| `ironhermes-core` | Config, types, MemoryStore, SkillRegistry, context scanner | Extend: memory provider trait, progressive discovery, slash commands |
| `ironhermes-state` | SQLite StateStore, FTS5, session/message tables, schema v6 | Wire into AgentLoop + gateway; already complete |
| `ironhermes-tools` | ToolRegistry, all tool impls (memory, file, web, skills, cron, exec, delegate) | Extend: session_search tool, slash command dispatch, toolset check fns |
| `ironhermes-agent` | AgentLoop, LlmClient, PromptBuilder, ContextCompressor, SubagentRunner | Major work: 10-layer prompt, cache_control, ContextEngine trait |
| `ironhermes-gateway` | GatewayRunner, GatewayMessageHandler, in-memory SessionStore, Telegram | Modify: wire StateStore, add 85% gateway compressor |
| `ironhermes-hooks` | HookRegistry, guardrails, hot-reload, webhook, log writer | Extend: CLI parity wiring |
| `ironhermes-exec` | Python sandbox, RPC bridge | Extend: CLI parity wiring |
| `ironhermes-cron` | JobStore, scheduler, delivery, scanner | No v2.0 changes |
| `ironhermes-cli` | Binary entry point, CLI REPL, batch, cron CLI commands | Extend: CLI parity (hooks, guardrails, execute_code), slash commands |

---

## System Overview

```
┌──────────────────────────────────────────────────────────────────────┐
│                         Entry Points                                  │
│  ┌─────────────────┐              ┌──────────────────────────────┐   │
│  │  ironhermes-cli  │              │    ironhermes-gateway         │   │
│  │  CLI REPL        │              │    Telegram adapter           │   │
│  │  Batch/Cron CLI  │              │    GatewayRunner              │   │
│  └────────┬─────────┘              └─────────────┬────────────────┘   │
└───────────┼─────────────────────────────────────┼────────────────────┘
            │                                      │
┌───────────▼──────────────────────────────────────▼────────────────────┐
│                        ironhermes-agent                                │
│                                                                        │
│  ┌─────────────────────────┐   ┌───────────────────────────────────┐  │
│  │      PromptBuilder       │   │           AgentLoop               │  │
│  │  10-layer assembly:      │   │  LlmClient (chat/stream)          │  │
│  │  1. SOUL.md identity     │   │  ContextCompressor (50% agent)    │  │
│  │  2. Platform hint        │   │  Hook integration                 │  │
│  │  3. Tool guidance        │   │  Tool dispatch                    │  │
│  │  4. Project context      │   │  StateStore persistence (NEW)     │  │
│  │  5. AGENTS.md            │   │  Cancellation token               │  │
│  │  6. Skills index         │   └───────────────────────────────────┘  │
│  │  7. Timestamp (NEW)      │                                           │
│  │  8. Memory snapshot      │   ┌───────────────────────────────────┐  │
│  │  9. User profile         │   │     cache_control assembly (NEW)  │  │
│  │  10. Ephemeral hints(NEW)│   │  system_and_3 breakpoints         │  │
│  └─────────────────────────┘   │  cached vs ephemeral separation   │  │
│                                 └───────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────────┘
            │                                      │
            ▼                                      ▼
┌──────────────────────────┐   ┌───────────────────────────────────────┐
│     ironhermes-core       │   │         ironhermes-state               │
│  MemoryStore (file-backed)│   │  StateStore (SQLite WAL + FTS5)        │
│  MemoryProvider trait NEW │   │  sessions / messages tables            │
│  SkillRegistry            │   │  session lineage (parent_session_id)   │
│  ContextScanner           │   │  search_messages() via FTS5            │
│  SlashCommandRouter NEW   │   └───────────────────────────────────────┘
└──────────────────────────┘
            │
            ▼
┌──────────────────────────┐
│     ironhermes-tools      │
│  ToolRegistry + guardrails│
│  MemoryTool (read/write)  │
│  SessionSearchTool NEW    │
│  SkillsTool (existing)    │
└──────────────────────────┘
```

---

## What Already Exists vs. What Needs Building

### Already Exists — Do Not Rebuild

**`ironhermes-state` (StateStore)** — Complete SQLite implementation with FTS5, WAL mode, schema migrations through v6, session lineage via `parent_session_id`, `search_messages(query, limit)`. Schema is ready. Only integration wiring is missing.

**`ironhermes-core` (MemoryStore)** — Complete file-backed MEMORY.md/USER.md store with bounded capacity, injection scanning, atomic writes, file locking, frozen snapshot pattern (D-12). Already wired into PromptBuilder and ToolRegistry.

**`ironhermes-core` (SkillRegistry)** — SKILL.md parsing, validation, catalog text for prompt injection. Already injected into PromptBuilder.

**`ironhermes-agent` (ContextCompressor)** — Token estimation, threshold-based compression, protect-first-N and protect-last-tokens guards. Agent-level (50%) works. Gateway-level (85%) is not yet invoked.

**`ironhermes-agent` (PromptBuilder)** — 6-layer assembly exists: SOUL.md, platform hint, tool guidance, project context, AGENTS.md, skills catalog, memory snapshot. Needs 4 more layers (timestamps, session hints) and cache_control output format.

**`ironhermes-gateway` (GatewaySession/SessionStore)** — In-memory `HashMap<SessionKey, GatewaySession>`. This is the hot-path conversation buffer. `StateStore` is the durable write-behind layer alongside it — both remain.

### What Needs Building (v2.0)

Described in detail in the Component Map below.

---

## Component Map for v2.0

### 1. Memory Provider Trait (`ironhermes-core`)

**What:** A `MemoryProvider` trait abstracting the current file-backed `MemoryStore`. Single-provider selection at startup via config. Lifecycle hooks: `load()`, `shutdown()`.

**New vs Modify:** New trait in `ironhermes-core/src/memory_provider.rs`. The existing `MemoryStore` becomes `FileMemoryProvider` implementing it. `MemoryTarget` and the frozen snapshot pattern remain.

**Important clarification on Grafeo/DuckDB:** The hermes-agent memory provider docs list 8 providers (Honcho, OpenViking, Mem0, Hindsight, Holographic/SQLite+FTS5+HRR, RetainDB, ByteRover, Supermemory). Neither Grafeo nor DuckDB appear. The PROJECT.md requirement for these is a custom extension. Implement the `MemoryProvider` trait to enable future providers but do not require Grafeo or DuckDB in v2.0 scope — `FileMemoryProvider` and optionally `SqliteMemoryProvider` are sufficient. Grafeo (graph DB) and DuckDB are deferred providers.

```
ironhermes-core/src/
  memory_provider.rs    # NEW: MemoryProvider trait
  memory_store.rs       # MODIFY: FileMemoryProvider impl of trait
  memory_sqlite.rs      # NEW (optional): SqliteMemoryProvider stub
```

**Integration points:** `GatewayRunner` and CLI construct a `Box<dyn MemoryProvider>` from config; pass `Arc<Mutex<dyn MemoryProvider>>` to `PromptBuilder` and `MemoryTool`. This is a backward-compatible refactor since `MemoryStore` currently uses `Arc<Mutex<MemoryStore>>` everywhere.

### 2. StateStore Integration (`ironhermes-agent` + `ironhermes-gateway` + `ironhermes-cli`)

**What:** Wire `StateStore` into `AgentLoop` so every conversation is persisted. The `GatewaySession` in-memory store remains as the hot conversation buffer; `StateStore` is write-behind durable storage.

**New vs Modify:** No new crate needed. `AgentLoop` gains `Option<StateStore>` (owned, not Arc-shared). On conversation start: `create_session()`. Per LLM turn: `add_message()` for each user/assistant/tool message. On end: `end_session()` + `update_session_stats()`.

**Write contention solution:** Each `AgentLoop` instance owns its own `StateStore` (connection-per-task pattern). Do not share a single `Arc<Mutex<StateStore>>` across concurrent gateway tasks — this creates a bottleneck. SQLite WAL mode handles concurrent readers and serializes writers transparently.

```
ironhermes-agent/src/agent_loop.rs      # MODIFY: accept Option<StateStore>, persist per turn
ironhermes-gateway/src/handler.rs       # MODIFY: create StateStore per handler task
ironhermes-gateway/src/runner.rs        # MODIFY: add StateStore dependency note in new()
ironhermes-cli/src/main.rs              # MODIFY: create StateStore for CLI sessions
```

### 3. Context Compression — Dual System (`ironhermes-agent` + `ironhermes-gateway`)

**What:** Two distinct compression thresholds:
- **Agent ContextEngine at 50%**: Applied inside `AgentLoop` mid-conversation. Already wired via `ContextCompressor`.
- **Gateway hygiene at 85%**: Applied in `GatewayMessageHandler` before handing conversation history to a new `AgentLoop` invocation. Prunes old messages when the in-memory `GatewaySession` grows too large.

**New vs Modify:** The 85% gateway compressor is the gap. A `ContextEngine` trait enables pluggable compression strategies (local prune vs LLM-based summarization). Add it to `ironhermes-agent`. The existing `ContextCompressor` becomes the default implementation.

```
ironhermes-agent/src/context_engine.rs      # NEW: ContextEngine trait
ironhermes-agent/src/context_compressor.rs  # MODIFY: impl ContextEngine trait
ironhermes-gateway/src/handler.rs           # MODIFY: add 85% hygiene check before AgentLoop
```

### 4. Prompt Caching — Anthropic cache_control (`ironhermes-agent`)

**What:** Anthropic's `cache_control` breakpoints injected into the system prompt array. Strategy `system_and_3`: mark the system prompt as an array of content blocks, with stable blocks tagged as cached and dynamic blocks as ephemeral.

**New vs Modify:** `LlmClient` in `client.rs` currently sends a flat string system prompt. When `provider=anthropic`, send `system` as an array of content blocks. Add `build_system_blocks()` to `PromptBuilder` returning `Vec<SystemBlock>`. Other providers continue to receive the flat string from `build()`.

**Key constraint:** cache_control is Anthropic-API-specific. The client must branch on provider. This is not a breaking change — OpenRouter and OpenAI paths are unchanged.

```
ironhermes-core/src/types.rs                # MODIFY: SystemBlock type, CacheControl enum
ironhermes-agent/src/prompt_builder.rs      # MODIFY: add build_system_blocks()
ironhermes-agent/src/client.rs              # MODIFY: branch on provider for block format
```

### 5. 10-Layer Prompt Assembly (`ironhermes-agent`)

**What:** Current `PromptBuilder::build()` handles layers 1-6 and 8-9. Missing layers:

| Layer | Content | Cache Status | Status |
|-------|---------|-------------|--------|
| 1 | SOUL.md identity | Cached | Exists |
| 2 | Platform hint | Cached | Exists |
| 3 | Tool-aware guidance | Cached | Exists |
| 4 | Project context (.hermes.md priority chain) | Cached | Exists |
| 5 | Home AGENTS.md | Cached | Exists |
| 6 | Skills index | Cached | Exists |
| 7 | Current timestamp + date context | Ephemeral | **Missing** |
| 8 | Memory snapshot (MEMORY.md) | Ephemeral | Exists |
| 9 | User profile snapshot (USER.md) | Ephemeral | Exists |
| 10 | Session/platform ephemeral hints | Ephemeral | **Missing** |

**New vs Modify:** Additive additions to `prompt_builder.rs`. Layer 7 is `chrono::Utc::now()` formatted as a string injected at `build()` time. Layer 10 is session-specific context (session ID, active skills list, current turn count).

```
ironhermes-agent/src/prompt_builder.rs  # MODIFY: add layers 7 + 10, overlay_soul field
```

### 6. SOUL.md Personality System with /personality Overlays (`ironhermes-agent`)

**What:** SOUL.md from `HERMES_HOME` is already loaded. v2.0 adds:
- `/personality` slash command creates a session-scoped overlay — a temporary identity string for the current session, without writing to disk
- Default fallback content already exists as `DEFAULT_AGENT_IDENTITY` const

**New vs Modify:** `PromptBuilder` gains an `overlay_soul: Option<String>` field. When set, it takes priority over SOUL.md in layer 1. Set via `with_soul_overlay(text)` builder method. The slash command router calls this on session construction.

```
ironhermes-agent/src/prompt_builder.rs  # MODIFY: overlay_soul field, with_soul_overlay()
```

### 7. Context Files with Progressive Discovery (`ironhermes-core`)

**What:** Currently `load_project_context()` only scans `cwd`. Progressive discovery walks parent directories up to a depth limit, collecting context files at each level with inner-directory files taking priority.

**New vs Modify:** Add `discover_context_files(cwd: &Path, depth: usize) -> Vec<(PathBuf, String)>` to `context_scanner.rs`. `PromptBuilder::load_project_context()` calls it instead of the single-directory scan.

Priority chain remains: `.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules`. Progressive discovery adds depth: repeat at each parent up to configured limit (default: 3).

```
ironhermes-core/src/context_scanner.rs  # MODIFY: add discover_context_files()
ironhermes-agent/src/prompt_builder.rs  # MODIFY: call discover_context_files()
```

### 8. Session Search Tool (`ironhermes-tools`)

**What:** A `session_search` tool wrapping `StateStore::search_messages()` for FTS5 search. Exposes to the agent for searching past conversations.

**New vs Modify:** New tool file. `StateStore` already has `search_messages(query, limit)`. Tool takes `query: String` and optional `limit: usize`.

```
ironhermes-tools/src/session_search_tool.rs  # NEW
ironhermes-tools/src/lib.rs                   # MODIFY: mod + pub use
```

**Integration:** Add to `ToolRegistry` in both CLI and gateway setup. `StateStore` passed as `Arc<Mutex<StateStore>>` (read-only queries, separate from the per-task write connection).

**Cargo.toml change:** `ironhermes-tools/Cargo.toml` gains `ironhermes-state` dependency.

### 9. Slash Commands (`ironhermes-core` + `ironhermes-gateway` + `ironhermes-cli`)

**What:** Slash commands (`/help`, `/personality`, `/reset`, `/skills`, `/memory`, `/sessions`, `/search`) are intercepted before text reaches `AgentLoop`. They are not LLM tool calls — they are UI-layer commands with deterministic behavior.

**New vs Modify:** A `SlashCommandRouter` in `ironhermes-core` parses `/command [args]` and returns either a direct response string or a session state mutation. Placed in core so both CLI and gateway import it without circular dependencies.

```
ironhermes-core/src/slash_commands.rs   # NEW: SlashCommandRouter, built-in commands
ironhermes-gateway/src/handler.rs       # MODIFY: check slash commands before AgentLoop
ironhermes-cli/src/main.rs              # MODIFY: check slash commands in REPL loop
```

Built-in commands:
- `/help` — list available commands
- `/reset` — clear session history
- `/personality <text>` — set soul overlay for session
- `/skills` — list active skills
- `/memory` — show current memory snapshot
- `/sessions` — list recent sessions from StateStore
- `/search <query>` — FTS5 search via StateStore

### 10. Tool Registry Improvements (`ironhermes-tools`)

**What:** Tool `is_available()` currently always returns `true`. v2.0 adds real checks: env var present, binary installed, credential file exists. Toolset-level filtering enables enabling/disabling groups of tools.

**New vs Modify:** Modify `ToolRegistry` and individual tool `is_available()` implementations. `ToolRegistry::get_definitions()` already accepts `enabled_tools: Option<&[String]>` — extend with toolset-level map.

```
ironhermes-tools/src/registry.rs        # MODIFY: toolset registry, check fn wiring
ironhermes-tools/src/execute_code.rs    # MODIFY: is_available() checks Python env
ironhermes-tools/src/web_search.rs      # MODIFY: is_available() checks API key
ironhermes-tools/src/web_read.rs        # MODIFY: is_available() checks Firecrawl key
```

### 11. CLI Feature Parity (`ironhermes-cli`)

**What:** Hooks, guardrails, and execute_code are gateway-only. v2.0 brings them to CLI interactive mode. No new abstractions needed — only construction/registration wiring.

```
ironhermes-cli/src/main.rs  # MODIFY: add HookRegistry, GuardrailHook, ExecuteCodeTool setup
```

---

## Data Flow

### Incoming Message (Gateway)

```
Telegram update
    → TelegramAdapter → tg_message_to_event()
    → UserQueueManager (per-user serialization)
    → GatewayMessageHandler::handle()
        → rate limiter check
        → SlashCommandRouter check (NEW)
            → if slash: return direct response, skip AgentLoop
        → SessionStore::get_or_create() [in-memory GatewaySession]
        → StateStore::create_session() if new session (WIRE)
        → GatewayCompressor at 85% threshold (NEW: check session.messages)
        → PromptBuilder::build_system_blocks() [10-layer, cache_control] (EXTEND)
        → AgentLoop::run()
            → ContextCompressor at 50% mid-conversation (existing)
            → LlmClient::chat() with cache_control block format (MODIFY)
            → ToolRegistry::dispatch() with guardrail checks
            → StateStore::add_message() per turn (WIRE)
        → StateStore::end_session() + update_session_stats() (WIRE)
    → stream response to Telegram
```

### Incoming Message (CLI)

```
User input (rustyline REPL)
    → SlashCommandRouter check (NEW)
        → if slash: execute + print + loop continue
    → messages Vec<ChatMessage> (in-memory for session lifetime)
    → PromptBuilder::build() [same 10-layer] (EXTEND)
    → AgentLoop::run()
        → HookRegistry event emission (NEW in CLI)
        → GuardrailHook checks (NEW in CLI)
        → ExecuteCodeTool available (NEW in CLI)
    → print response
```

### Memory Data Flow

```
Startup:
    Config → MemoryProvider selection → FileMemoryProvider (default)
    → load_from_disk() → frozen snapshot captured (D-12)
    → Arc<Mutex<dyn MemoryProvider>> →
        PromptBuilder (snapshot injection, layers 8+9)
        MemoryTool (live read/write, tool calls)

During conversation:
    MemoryTool::execute(add/replace/remove)
        → file lock → reload → mutate → atomic write
        → snapshot NOT updated (frozen per D-12)
        → next session load sees new content

System prompt assembly:
    Layers 8+9 = EPHEMERAL blocks in Anthropic cache_control
    (memory content changes each session — never put in cached block)
```

### Session Persistence Data Flow

```
GatewaySession (ironhermes-gateway)  = hot in-memory conversation buffer
StateStore (ironhermes-state)        = durable SQLite write-behind

On new conversation:
    GatewaySession::new()                     [in-memory hot path]
    StateStore::create_session(id, src, model, system_prompt)  [durable]

Each LLM turn:
    AgentLoop processes user → assistant → tool → assistant messages
    StateStore::add_message(session_id, msg)  [each message, per turn]

On session end (timeout, /reset, shutdown):
    StateStore::end_session(session_id, reason)
    StateStore::update_session_stats(input_tokens, output_tokens, tool_calls)
    GatewaySession cleared from in-memory store
```

---

## Crate Dependency Graph (v2.0 Changes)

```
ironhermes-cli
    depends on: core, state(existing), tools, agent, gateway, cron, hooks, exec

ironhermes-gateway
    depends on: core, state(existing), tools, agent, cron, hooks
    NEW dep: state already present in workspace, ensure in Cargo.toml

ironhermes-agent
    depends on: core, state(existing), tools, hooks
    NEW: uses StateStore for session persistence (already in Cargo.toml)

ironhermes-tools
    depends on: core, cron, hooks, exec
    NEW dep: ironhermes-state (for SessionSearchTool)

ironhermes-state
    depends on: core (no change)

ironhermes-core
    depends on: nothing internal (leaf crate, no change)
```

New dependency edges in v2.0:
- `ironhermes-tools` → `ironhermes-state` (for `SessionSearchTool`)
- `ironhermes-gateway/Cargo.toml` — verify `ironhermes-state` is listed (may already be via transitive)

---

## Suggested Build Order

Build order respects the dependency graph and groups by logical feature cohesion. Each phase is independently testable.

### Phase 1 — Memory Provider Trait (ironhermes-core)
**Rationale:** Foundation for all memory wiring. Establishing the trait before modifying PromptBuilder and tools prevents rework. Non-breaking — `FileMemoryProvider` is a drop-in rename.
- Define `MemoryProvider` trait with `load()`, `add()`, `replace()`, `remove()`, `format_for_prompt()`, `shutdown()`
- Refactor `MemoryStore` into `FileMemoryProvider` implementing the trait
- Update `MemoryTool`, `PromptBuilder`, `GatewayMessageHandler`, CLI to use `Arc<Mutex<dyn MemoryProvider>>`
- `SqliteMemoryProvider` stub if desired, behind a cargo feature flag

### Phase 2 — StateStore Integration + Session Persistence (ironhermes-agent + gateaway + CLI)
**Rationale:** StateStore is complete — only wiring is missing. Highest-value safety net: every conversation persisted before building features on top.
- `AgentLoop` gains `Option<StateStore>` (owned), persists each turn
- Gateway handler creates `StateStore` per task (connection-per-task)
- CLI creates `StateStore` for interactive sessions
- Integration tests: session round-trip through AgentLoop with real SQLite

### Phase 3 — 10-Layer Prompt Assembly + SOUL.md Overlays (ironhermes-agent)
**Rationale:** Prompt assembly is the core identity feature. Layers 7 and 10 are additive and low-risk. Sets stable foundation for cache_control in Phase 4.
- Add timestamp injection (layer 7) in `build()`
- Add session ephemeral hints (layer 10): session ID, active skills count, turn number
- Add `overlay_soul` field and `with_soul_overlay()` method
- Update layer ordering tests

### Phase 4 — Prompt Caching with cache_control (ironhermes-agent)
**Rationale:** Depends on 10-layer assembly being stable. Cache partitioning (which layers are stable vs dynamic) must be decided after the full layer set is known.
- Add `SystemBlock` + `CacheControl` types to `ironhermes-core::types`
- `PromptBuilder::build_system_blocks()` returns `Vec<SystemBlock>` with cache markers
  - Layers 1-6: `cache_control: {"type": "ephemeral"}` breakpoint after layer 6
  - Layers 7-10: no cache marker (ephemeral by default)
- `LlmClient` detects `provider=anthropic`, sends block format; other providers unchanged

### Phase 5 — Context Compression Dual System (ironhermes-agent + ironhermes-gateway)
**Rationale:** Gateway hygiene depends on stable prompt to know what to protect. Phase 3+4 must be stable first.
- Add `ContextEngine` trait to `ironhermes-agent`
- `ContextCompressor` implements `ContextEngine`
- `GatewayMessageHandler` applies 85% hygiene compressor on `GatewaySession.messages` before each `AgentLoop` invocation

### Phase 6 — Context Files Progressive Discovery (ironhermes-core)
**Rationale:** Isolated change. Low risk, additive, no dependencies on other v2.0 phases.
- `discover_context_files(cwd, depth_limit)` in `context_scanner.rs`
- `PromptBuilder::load_project_context()` calls it
- Security scan at each discovered file
- Tests: nested directory structure with competing context files

### Phase 7 — Session Search Tool + Tool Registry Improvements (ironhermes-tools)
**Rationale:** Depends on StateStore wiring (Phase 2). Tool registry improvements are isolated and can be done in parallel with Phase 6.
- Add `ironhermes-state` dep to `ironhermes-tools/Cargo.toml`
- `SessionSearchTool` wrapping `StateStore::search_messages()`
- Tool `is_available()` real implementations for execute_code, web_search, web_read
- Toolset-level filtering map in `ToolRegistry`

### Phase 8 — Slash Commands (ironhermes-core + ironhermes-gateway + ironhermes-cli)
**Rationale:** Depends on SOUL.md overlay (Phase 3) for `/personality`, StateStore (Phase 2) for `/sessions` and `/search`.
- `SlashCommandRouter` in `ironhermes-core/src/slash_commands.rs`
- Wire in `GatewayMessageHandler` before AgentLoop dispatch
- Wire in CLI REPL loop before AgentLoop call
- Built-in commands: `/help`, `/reset`, `/personality`, `/skills`, `/memory`, `/sessions`, `/search`

### Phase 9 — CLI Feature Parity (ironhermes-cli)
**Rationale:** Last because it's wiring existing functionality into a new context. No new abstractions needed.
- Add `HookRegistry` construction to CLI setup (same as gateway)
- Add `GuardrailHook` registration on CLI `ToolRegistry`
- Add `ExecuteCodeTool` to CLI registration with `is_available()` guard

---

## Architectural Patterns to Follow

### Pattern 1: Connection-Per-Task for StateStore

**What:** Each concurrent gateway handler task opens its own `StateStore::open_default()` connection rather than sharing a single `Arc<Mutex<StateStore>>`.

**When to use:** Always for write-heavy paths (message persistence per turn). A shared read-only Arc is acceptable for `SessionSearchTool` since FTS5 queries are reads.

**Trade-off:** Slightly more file handles. Acceptable for single-operator deployment. SQLite WAL handles concurrent readers + one writer per connection correctly.

### Pattern 2: Frozen Snapshot for Prompt Stability

**What:** `MemoryProvider::load()` captures a frozen snapshot at session start. Mutations during the session write to disk but do NOT update the snapshot. The next session load picks up new content.

**When to use:** Always for system prompt content. Extend to cache_control: blocks marked as cached must be stable across turns. Never include dynamic content (timestamps, memory snapshots) in a cached block.

### Pattern 3: Platform-Agnostic Core, Platform-Specific Entry Points

**What:** `PromptBuilder`, `AgentLoop`, `ContextCompressor`, `MemoryProvider`, `StateStore`, `SlashCommandRouter` have no platform imports. Gateway and CLI are the only files with platform-specific code.

**When to use:** Any new component that must work in both CLI and gateway. If you find yourself importing `ironhermes-gateway` types in `ironhermes-agent`, stop and find a trait boundary instead.

### Pattern 4: Option<Arc<T>> for Optional Subsystems

**What:** `AgentLoop`, `GatewayRunner`, `GatewayMessageHandler` use `Option<Arc<T>>` for optional subsystems (hook registry, memory store, skill registry). Components degrade gracefully when subsystems are absent.

**For v2.0:** Extend this pattern to `StateStore` in `AgentLoop` — use `Option<StateStore>` (owned). When `None`, sessions are not persisted. Useful for batch mode and unit tests.

---

## Anti-Patterns to Avoid

### Anti-Pattern 1: Shared Single StateStore Across Gateway Tasks

**What people do:** Create one `Arc<Mutex<StateStore>>` in `GatewayRunner` and clone it to every `GatewayMessageHandler` task.

**Why it's wrong:** Every concurrent user blocks on the same mutex to write messages. Under load (10+ concurrent users), this serializes all database writes. rusqlite's sync API inside async also requires `spawn_blocking` boilerplate everywhere.

**Do this instead:** Connection-per-task. Each handler opens `StateStore::open_default()` at task start. SQLite WAL handles the concurrency at the storage layer.

### Anti-Pattern 2: Dynamic Content in Cached Blocks

**What people do:** Put timestamps, memory snapshots, session IDs, or active skill lists in the Anthropic `cache_control` cached portion of the system prompt.

**Why it's wrong:** Cache is invalidated on every request because the content changes. You pay cache write tokens on every call but never benefit from cache read savings.

**Do this instead:** Layers 1-6 (SOUL, platform hint, tool guidance, project context, AGENTS.md, skills) → mark as cached. Layers 7-10 (timestamp, memory snapshot, user profile, session hints) → leave ephemeral (no cache_control marker).

### Anti-Pattern 3: Slash Commands as LLM Tool Calls

**What people do:** Implement `/reset`, `/help`, `/personality` as entries in `ToolRegistry` that the LLM might choose to call.

**Why it's wrong:** The LLM may not recognize the command, may execute it in the wrong context, may refuse, or may misinterpret arguments. User expects immediate deterministic behavior from `/` prefix.

**Do this instead:** Intercept `/` prefix text in `GatewayMessageHandler` and CLI REPL before passing to `AgentLoop`. `SlashCommandRouter` dispatches directly. No LLM involvement.

### Anti-Pattern 4: Re-loading SOUL.md Per Turn

**What people do:** Call `PromptBuilder::load_context()` inside the `AgentLoop` on every LLM turn to "keep context fresh."

**Why it's wrong:** File I/O on every turn, and the system prompt changes mid-conversation. This breaks Anthropic cache effectiveness (cache invalidated every turn) and violates prompt stability (model context becomes inconsistent with its initial system prompt).

**Do this instead:** `load_context()` once at session start. The `AgentLoop` reuses the frozen system message for the entire session. A new session gets a freshly-loaded prompt.

---

## Integration Points Summary

| New Component | Crate | Integrates With | Communication Pattern |
|---------------|-------|-----------------|----------------------|
| `MemoryProvider` trait | `ironhermes-core` | `MemoryTool`, `PromptBuilder`, CLI, Gateway | `Arc<Mutex<dyn MemoryProvider>>` |
| `FileMemoryProvider` | `ironhermes-core` | replaces current `MemoryStore` | backward-compatible rename |
| `StateStore` wiring | `ironhermes-agent`, `ironhermes-gateway`, CLI | `AgentLoop`, handler, REPL | Connection-per-task (owned) |
| `ContextEngine` trait | `ironhermes-agent` | `AgentLoop`, `GatewayMessageHandler` | `Box<dyn ContextEngine>` |
| `SystemBlock` + `CacheControl` | `ironhermes-core` types | `PromptBuilder`, `LlmClient` | Anthropic request block format |
| 10-layer prompt layers 7+10 | `ironhermes-agent` `PromptBuilder` | all `PromptBuilder` callers | additive, backward compatible |
| `overlay_soul` field | `ironhermes-agent` `PromptBuilder` | slash commands, CLI flags | builder setter method |
| `discover_context_files()` | `ironhermes-core` `context_scanner` | `PromptBuilder` | direct function call |
| `SessionSearchTool` | `ironhermes-tools` | `StateStore`, `ToolRegistry` | `Arc<Mutex<StateStore>>` (read) |
| `SlashCommandRouter` | `ironhermes-core` | `GatewayMessageHandler`, CLI REPL | returns `SlashResult` enum |
| Tool `is_available()` checks | `ironhermes-tools` | `ToolRegistry::get_definitions()` | existing trait method, now non-trivial |
| Toolset-level filtering | `ironhermes-tools` `ToolRegistry` | `AgentLoop`, handler | `HashMap<String, Vec<String>>` |
| CLI hooks/guardrails/exec | `ironhermes-cli` | `HookRegistry`, `ToolRegistry` | same construction path as gateway |

---

## No New Crates Required

All v2.0 features fit within the existing 9-crate workspace. The workspace has already expanded from the original 7 crates to 9 (adding `ironhermes-state` and `ironhermes-cli` as separate crates).

New source files within existing crates:
- `ironhermes-core/src/memory_provider.rs` — `MemoryProvider` trait
- `ironhermes-core/src/slash_commands.rs` — `SlashCommandRouter`
- `ironhermes-agent/src/context_engine.rs` — pluggable compression trait
- `ironhermes-tools/src/session_search_tool.rs` — FTS5 session search tool

One new Cargo.toml dependency:
- `ironhermes-tools/Cargo.toml` gains `ironhermes-state = { path = "../ironhermes-state" }`

---

## Sources

- Direct codebase inspection: all 9 crates, key files (agent_loop.rs, prompt_builder.rs, context_compressor.rs, memory_store.rs, ironhermes-state/src/lib.rs, gateway/session.rs, gateway/handler.rs, gateway/runner.rs, tools/registry.rs, core/skills.rs, cli/main.rs, Cargo.toml files)
- hermes-agent architecture reference: https://hermes-agent.nousresearch.com/docs/developer-guide/architecture
- hermes-agent memory providers reference: https://hermes-agent.nousresearch.com/docs/user-guide/features/memory-providers (Grafeo and DuckDB are NOT in hermes-agent — custom additions per PROJECT.md)

---
*Architecture research for: IronHermes v2.0 Intelligence & Identity*
*Researched: 2026-04-11*
