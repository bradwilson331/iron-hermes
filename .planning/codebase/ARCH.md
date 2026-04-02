# Architecture

**Analysis Date:** 2026-04-01

## Crate Dependency Graph

```
ironhermes-core          (leaf — no internal deps)
    ^
    |
    +--- ironhermes-state    (core)
    |
    +--- ironhermes-tools    (core)
    |
    +--- ironhermes-cron     (core)
    |
    +--- ironhermes-agent    (core, tools, state)
    |
    +--- ironhermes-gateway  (core, agent, tools, state)
    |
    +--- ironhermes-cli      (core, agent, tools, state)   [binary crate]
```

**Dependency matrix (rows depend on columns):**

| Crate | core | state | tools | agent | cron | gateway |
|-------|------|-------|-------|-------|------|---------|
| **core** | - | | | | | |
| **state** | Y | - | | | | |
| **tools** | Y | | - | | | |
| **cron** | Y | | | | - | |
| **agent** | Y | Y | Y | - | | |
| **gateway** | Y | Y | Y | Y | | - |
| **cli** | Y | Y | Y | Y | | |

Key observations:
- `ironhermes-core` is the foundation: every other crate depends on it.
- `ironhermes-agent` is the integration hub: it combines core types, tools, and state.
- `ironhermes-cli` and `ironhermes-gateway` are the two top-level consumers (entry points).
- `ironhermes-cron` is isolated: only depends on core, not wired into agent or CLI yet.
- There is **no circular dependency**; the graph is a clean DAG.

## Key Abstractions

### Tool trait (`crates/ironhermes-tools/src/registry.rs`)

The central extensibility point for agent capabilities.

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;           // grouping: "file", "system", "web"
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;       // OpenAI function-calling JSON schema
    fn is_available(&self) -> bool { true }
    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```

All tools return `String` results. The `ToolRegistry` stores `Box<dyn Tool>` in a `HashMap<String, Box<dyn Tool>>` keyed by name.

**Current tool implementations:**
- `ReadFileTool` — `crates/ironhermes-tools/src/file_tools.rs`
- `WriteFileTool` — `crates/ironhermes-tools/src/file_tools.rs`
- `PatchFileTool` — `crates/ironhermes-tools/src/file_tools.rs`
- `SearchFilesTool` — `crates/ironhermes-tools/src/file_tools.rs`
- `TerminalTool` — `crates/ironhermes-tools/src/terminal.rs`
- `WebSearchTool` — `crates/ironhermes-tools/src/web_search.rs`

Register new tools by implementing `Tool` and calling `registry.register(Box::new(MyTool))` or adding to `register_defaults()` in `crates/ironhermes-tools/src/registry.rs`.

### PlatformAdapter trait (`crates/ironhermes-gateway/src/adapter.rs`)

Abstracts messaging platform integrations (Telegram, Discord, Slack, etc.).

```rust
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    fn platform(&self) -> Platform;
    async fn start(&mut self, handler: Box<dyn MessageHandler>) -> Result<()>;
    async fn stop(&mut self) -> Result<()>;
    async fn send_message(&self, chat_id: &str, content: &str, thread_id: Option<&str>) -> Result<MessageResponse>;
    async fn edit_message(&self, chat_id: &str, message_id: &str, content: &str) -> Result<()>;
    async fn delete_message(&self, chat_id: &str, message_id: &str) -> Result<()>;
    async fn add_reaction(&self, _chat_id: &str, _message_id: &str, _emoji: &str) -> Result<()> { Ok(()) }
    fn is_running(&self) -> bool;
}
```

### MessageHandler trait (`crates/ironhermes-gateway/src/adapter.rs`)

Connects gateway adapters to the agent. Adapters call `handler.handle(&event)` for each incoming message.

```rust
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle(&self, event: &MessageEvent) -> Result<String>;
}
```

### Core types (`crates/ironhermes-core/src/types.rs`)

All LLM communication uses OpenAI-compatible types:
- `ChatMessage` — role + content + optional tool_calls/tool_call_id
- `ChatRequest` / `ChatResponse` — full request/response envelopes
- `ChatStreamChunk` / `StreamDelta` — SSE streaming types
- `ToolSchema` / `FunctionSchema` — tool definitions sent to the LLM
- `MessageEvent` / `MessageResponse` — platform-agnostic gateway message types
- `Platform` enum — 16 platform variants (Local, Telegram, Discord, etc.)

### Config (`crates/ironhermes-core/src/config.rs`)

Hierarchical YAML config loaded from `~/.ironhermes/config.yaml`:
- `ModelConfig` — model name, base_url, provider, api_key
- `AgentConfig` — max_turns, context_compression threshold, tool_delay
- `TerminalConfig` — backend, cwd, timeout
- `WebConfig` — backend (firecrawl)
- `GatewayConfig` — per-platform enable/token/api_key
- `SecurityConfig` — redact_secrets flag
- `CronConfig` — wrap_response flag

API key resolution order: config file > provider-specific env var > `OPENROUTER_API_KEY` > `OPENAI_API_KEY`.

### Error types

- `HermesError` (`crates/ironhermes-core/src/error.rs`) — unified error enum with `thiserror`, variants: Config, Api, Tool, State, Provider, ContextOverflow, MaxIterations, Io, Json, Http, NotFound, Unauthorized, Other.
- `StateError` (`crates/ironhermes-state/src/lib.rs`) — Sqlite, Json, SessionNotFound, Other.
- Tool errors use `anyhow::Result<String>` (errors are stringified and returned to the LLM as tool results).

## Data Flow

### CLI Interactive Chat Flow

```
User input (stdin via rustyline)
    |
    v
main.rs::run_chat()                         [crates/ironhermes-cli/src/main.rs]
    |
    +-- build_client() -> LlmClient         [reads Config, resolves API key/URL]
    +-- build_registry() -> ToolRegistry    [registers 6 default tools]
    +-- PromptBuilder::build_system_message()
    |
    v
run_agent_turn()
    |
    v
AgentLoop::run(messages)                    [crates/ironhermes-agent/src/agent_loop.rs]
    |
    +-- registry.get_definitions() -> Vec<ToolSchema>
    |
    +-- LOOP:
    |     |
    |     +-- ContextCompressor::compress()  [prune old tool results, drop middle msgs]
    |     |
    |     +-- LlmClient::chat_completion() OR chat_completion_stream()
    |     |       |
    |     |       +-- POST {base_url}/chat/completions
    |     |       +-- Parse ChatResponse or stream SSE chunks via tokio::spawn
    |     |
    |     +-- If response has tool_calls:
    |     |       |
    |     |       +-- For each ToolCall:
    |     |       |     registry.dispatch(name, args) -> Tool::execute()
    |     |       |     Push ChatMessage::tool_result() to messages
    |     |       |
    |     |       +-- Continue loop
    |     |
    |     +-- If no tool_calls: break (natural completion)
    |     +-- If turns >= max_iterations: break
    |
    v
AgentResult { messages, turns_used, final_response, total_usage }
    |
    v
Print response, update conversation history
```

### Gateway Message Flow

```
Platform (e.g. Telegram long-poll)
    |
    v
TelegramAdapter::start()                   [crates/ironhermes-gateway/src/telegram.rs]
    |
    +-- tokio::spawn(polling loop)
    |     |
    |     +-- GET /getUpdates with 30s timeout
    |     +-- For each TgUpdate:
    |           |
    |           +-- tg_message_to_event() -> MessageEvent
    |           +-- tokio::spawn:
    |                 handler.handle(&event) -> response_text
    |                 POST /sendMessage with response
    |
    v
GatewayRunner::start()                     [crates/ironhermes-gateway/src/runner.rs]
    |
    +-- Iterates config.gateway.platforms
    +-- Creates and starts each enabled PlatformAdapter
    +-- Waits on ctrl_c signal, then stops all adapters
```

### Session Persistence Flow

```
StateStore::create_session()                [crates/ironhermes-state/src/lib.rs]
    |
    +-- INSERT INTO sessions
    |
StateStore::add_message()
    |
    +-- INSERT INTO messages
    +-- UPDATE sessions SET message_count++
    +-- FTS5 trigger auto-indexes content
    |
StateStore::search_messages()
    |
    +-- FTS5 MATCH query via messages_fts
```

## Module Structure

### ironhermes-core (`crates/ironhermes-core/src/`)
```
lib.rs          — re-exports: Config, constants::*, error::*, types::*
config.rs       — Config struct with YAML load/save, env var resolution
constants.rs    — VERSION, API URLs, defaults, get_hermes_home()
error.rs        — HermesError enum (thiserror), Result type alias
types.rs        — ChatMessage, ChatRequest/Response, streaming types,
                  ToolSchema, Platform enum, MessageEvent/Response
```

### ironhermes-state (`crates/ironhermes-state/src/`)
```
lib.rs          — StateStore (rusqlite Connection wrapper)
                  Schema DDL with migrations (v1-v6)
                  Session/StoredMessage/SearchResult structs
                  CRUD: create_session, add_message, get_messages,
                        list_sessions, search_messages, update_session_stats
                  FTS5 full-text search on message content
```

### ironhermes-tools (`crates/ironhermes-tools/src/`)
```
lib.rs          — re-exports: Tool, ToolRegistry
registry.rs     — Tool trait definition, ToolRegistry (HashMap-based),
                  register_defaults() registers all 6 built-in tools
file_tools.rs   — ReadFileTool, WriteFileTool, PatchFileTool, SearchFilesTool
terminal.rs     — TerminalTool (shell exec via tokio::process::Command)
web_search.rs   — WebSearchTool (Firecrawl API integration)
```

### ironhermes-agent (`crates/ironhermes-agent/src/`)
```
lib.rs              — re-exports: AgentLoop, AgentResult, LlmClient,
                      PromptBuilder, ContextCompressor
agent_loop.rs       — AgentLoop struct: orchestrates LLM calls + tool dispatch
                      AgentResult, AggregatedUsage, StreamCallback, ToolProgressCallback
client.rs           — LlmClient: HTTP client for OpenAI-compatible APIs
                      StreamEvent enum, SSE parser, assemble_tool_calls_from_stream()
prompt_builder.rs   — PromptBuilder: system prompt assembly with platform hints,
                      context file loading (SOUL.md, AGENTS.md)
context_compressor.rs — ContextCompressor: token estimation (~4 chars/token),
                        tool result pruning, middle message dropping
```

### ironhermes-cli (`crates/ironhermes-cli/src/`)
```
main.rs         — Binary entry point, clap CLI with subcommands:
                  Chat (default), Status, Doctor, Version
                  run_chat() — interactive REPL via rustyline
                  run_single() — one-shot execution (-e flag)
                  build_client(), build_registry() setup helpers
                  Slash commands: /quit, /clear, /status, /help
```

### ironhermes-gateway (`crates/ironhermes-gateway/src/`)
```
lib.rs          — re-exports: PlatformAdapter, MessageHandler,
                  GatewaySession, GatewayRunner
adapter.rs      — PlatformAdapter trait, MessageHandler trait
runner.rs       — GatewayRunner: multi-platform orchestrator,
                  resolve_env_var() for config token references
session.rs      — SessionKey, GatewaySession, SessionStore (in-memory HashMap)
telegram.rs     — TelegramAdapter: long-polling impl of PlatformAdapter,
                  Telegram Bot API types, tg_message_to_event() converter
```

### ironhermes-cron (`crates/ironhermes-cron/src/`)
```
lib.rs          — CronJob struct, JobStore (JSON file persistence at
                  ~/.ironhermes/cron/jobs.json), atomic save via rename,
                  compute_next_run() with 5-field/6-field cron normalization,
                  LockGuard/acquire_tick_lock() for exclusive tick execution,
                  Comprehensive test suite (7 tests)
```

## Concurrency Model

### Runtime

- **Tokio** multi-threaded runtime (`#[tokio::main]` with `features = ["full"]`).
- The CLI binary is the only entry point; it initializes Tokio in `main.rs`.

### Async patterns

**Agent loop (`crates/ironhermes-agent/src/agent_loop.rs`):**
- The `AgentLoop::run()` method is `async` and drives the LLM-tool loop sequentially.
- Tool calls within a single turn execute **sequentially** (for-loop over `tool_calls`).
- No parallel tool execution currently.
- `ContextCompressor` is behind `tokio::sync::Mutex` to allow interior mutability in the async context.

**LLM streaming (`crates/ironhermes-agent/src/client.rs`):**
- `chat_completion_stream()` spawns a background `tokio::spawn` task that reads the HTTP byte stream.
- Stream events are forwarded via `tokio::sync::mpsc::channel(256)` to the caller.
- The caller (`call_llm_streaming`) reads from the receiver with `rx.recv().await`.
- Tool call deltas are accumulated and assembled after the stream completes.

**Gateway polling (`crates/ironhermes-gateway/src/telegram.rs`):**
- Each platform adapter runs its polling loop in a dedicated `tokio::spawn` task.
- The `TelegramAdapter` uses long polling (30s timeout) via `reqwest`.
- Each incoming message is handled in a separate `tokio::spawn` task (fire-and-forget).
- Graceful shutdown via `Arc<AtomicBool>` — `stop()` sets the flag and aborts the poll handle.

**Cron tick lock (`crates/ironhermes-cron/src/lib.rs`):**
- File-based exclusive lock (`O_CREAT | O_EXCL`) prevents concurrent tick execution.
- RAII `LockGuard` removes the lock file on drop.

### Shared state patterns

| Pattern | Usage | Location |
|---------|-------|----------|
| `Arc<ToolRegistry>` | Shared tool registry across agent turns | `crates/ironhermes-cli/src/main.rs` |
| `Arc<AtomicBool>` | Adapter running flag | `crates/ironhermes-gateway/src/telegram.rs` |
| `Arc<Box<dyn MessageHandler>>` | Shared handler in spawned tasks | `crates/ironhermes-gateway/src/telegram.rs` |
| `Mutex<ContextCompressor>` | Compressor state in async context | `crates/ironhermes-agent/src/agent_loop.rs` |
| `LlmClient: Clone` | Cloneable HTTP client (reqwest::Client is Arc internally) | `crates/ironhermes-agent/src/client.rs` |

### Sync vs Async boundary

- `rusqlite` is **synchronous**. `StateStore` methods are sync and should be called from blocking contexts or wrapped with `tokio::task::spawn_blocking()` if called from async code. Currently the state crate has no async wrapper — this is a potential concern under high gateway load.
- `ironhermes-cron` is also fully synchronous (file I/O only). No async runtime dependency despite `tokio` being listed as a dependency.

---

*Architecture analysis: 2026-04-01*
