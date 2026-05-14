<!-- generated-by: gsd-doc-writer -->
# Crate Reference

IronHermes is a Cargo workspace composed of 13 library/binary crates plus 3 pluggable memory-provider crates under `providers/`. This document describes each crate's purpose, public API surface, sibling dependencies, and notable implementation details, followed by the full dependency order.

---

## Workspace layout

```
ironhermes/
├── crates/
│   ├── ironhermes-core        # leaf: types, config, provider abstraction
│   ├── ironhermes-state       # SQLite session persistence
│   ├── ironhermes-trajectory  # append-only JSONL tool-call ledger
│   ├── ironhermes-exec        # Python sandbox + process registry
│   ├── ironhermes-cron        # cron scheduler
│   ├── ironhermes-hooks       # event hook system
│   ├── ironhermes-hub         # skills hub (install / update / trust)
│   ├── ironhermes-tools       # tool registry and all tool implementations
│   ├── ironhermes-mcp         # MCP client (stdio + HTTP transports)
│   ├── ironhermes-agent       # agent loop and LLM client
│   ├── ironhermes-gateway     # multi-platform messaging gateway
│   ├── ironhermes-cli         # interactive CLI binary + ratatui REPL
│   └── iron_hermes_ui         # Dioxus fullstack web/desktop UI
└── providers/
    ├── memory-sqlite          # SQLite FTS5 memory provider
    ├── memory-grafeo          # Grafeo graph memory provider
    └── memory-duckdb          # DuckDB columnar memory provider
```

> **Note:** `crates/ironagent-tools-api` is excluded from the workspace. It is a stale byte-for-byte duplicate of `ironhermes-tools` whose test files import a sibling crate it does not declare as a dependency; `cargo test --workspace` cannot compile it. It is pending removal.

---

## Crate descriptions

### `ironhermes-core`

**Purpose:** Foundational leaf crate. Owns all shared types, the global `Config` struct, the provider abstraction, token estimation, skill registry, SSRF safety, and the slash-command routing framework. Every other crate depends on this one; it has no sibling dependencies.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `ChatMessage` | struct | OpenAI-compatible chat message with `Role`, `MessageContent`, and optional `tool_calls` |
| `Role` | enum | `System` / `User` / `Assistant` / `Tool` |
| `MessageContent` | enum | `Text(String)` or `Parts(Vec<ContentPart>)` with `.as_text()` helper |
| `ToolCall` / `FunctionCall` | structs | Wire-format tool call and function invocation |
| `ToolSchema` / `FunctionSchema` | structs | OpenAI-compatible tool definition |
| `Config` | struct | Top-level config with ~20 sub-structs covering models, memory, cron, exec, browser, security, hub, subagents, and batch |
| `ProviderConfig` / `CustomProviderConfig` | structs | Per-provider API endpoint and key configuration |
| `ProviderResolver` / `ResolvedEndpoint` | structs | Runtime provider selection and endpoint resolution |
| `ModelRegistry` / `ModelMetadata` / `ModelCapabilities` | structs | Cached model capability metadata |
| `ModelsCache` / `fetch_all` | struct + fn | Fetches and normalizes model lists from OpenRouter and models.dev |
| `SkillRegistry` / `SkillRecord` / `SkillSource` | structs | Skill discovery, metadata, and source tracking |
| `MemoryProvider` / `MemoryStore` / `MemoryTarget` | trait + structs | Memory backend abstraction and store wrapper |
| `CommandContext` | struct | Per-command execution context threaded through slash commands |
| `CommandRouter` / `CommandDef` / `CommandCategory` | structs | Slash-command registration and routing |
| `TokenEstimator` / `init_global_estimator` | struct + fn | tiktoken-rs token counting with singleton warm-up |
| `HermesError` / `Result` | type aliases | Unified error type |
| `Workspace` / `resolve_workspace_from_cwd` | struct + fn | Walk-up cwd resolver for workspace root detection |
| `is_safe_url` | fn | SSRF guard for outbound HTTP calls |
| `scan_context_content` / `truncate_content` | fns | Context file scanning and length limiting |

**Sibling dependencies:** None — this is the dependency root.

**Notable details:**
- `Config` is loaded from `~/.ironhermes/config.yaml` and merged with `.env` via `dotenvy`. The `config_validate` and `config_setter` modules enforce field constraints and provide programmatic setters.
- `ProviderResolver` selects the active provider at runtime from the config, resolving the correct base URL and API key for each role (main, summarization, subagent, etc.).
- `TokenEstimator` uses tiktoken-rs with a globally initialized singleton; callers invoke `init_global_estimator` at startup and `global_estimate_tokens` thereafter.
- `TrajectoryWriterHandle` (the cycle-breaking trait used by `AgentLoop`) lives here to avoid a circular dependency between `ironhermes-agent` and `ironhermes-trajectory`.

---

### `ironhermes-state`

**Purpose:** SQLite-backed session persistence. Stores sessions and messages with FTS5 full-text search, schema-versioned migrations (current: v8), and WAL mode.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `StateStore` | struct | Main store; wraps a `rusqlite::Connection` |
| `StateStore::new(path)` | fn | Open or create a database at a given path |
| `StateStore::open_default()` | fn | Open the default `$IRONHERMES_HOME/state.db` |
| `StateStore::create_session(...)` | fn | Insert a new session row with optional workspace root |
| `StateStore::end_session(id, reason)` | fn | Mark session as ended with a reason |
| `StateStore::add_message(session_id, msg)` | fn | Append a `ChatMessage`; returns the row id |
| `StateStore::get_session(id)` | fn | Look up a single session |
| `StateStore::get_messages(session_id)` | fn | Ordered messages for a session (by `id ASC`) |
| `StateStore::list_sessions(source, limit)` | fn | List sessions, most recent first |
| `StateStore::list_sessions_filtered(source, limit, workspace_root)` | fn | Filter by source and/or workspace root |
| `StateStore::search_messages(filter)` | fn | FTS5 full-text or metadata-only search |
| `StateStore::update_session_stats(...)` | fn | Accumulate token and tool-call counts |
| `StateStore::update_session_title(id, title)` | fn | Set a human-readable session title |
| `StateStore::export_session(id)` / `export_sessions(source)` | fns | JSON export of session + messages |
| `StateStore::prune_sessions(days, source)` | fn | Delete ended sessions older than N days |
| `StateStore::wal_checkpoint()` | fn | Passive WAL checkpoint |
| `Session` / `StoredMessage` / `SearchResult` | structs | Data transfer objects |
| `SearchFilter` | struct | Composable filter for `search_messages` |
| `SessionDirectoryExport` | struct | Phase 25.3 four-file directory export |
| `StateError` | enum | `Sqlite` / `Json` / `SessionNotFound` / `Other` |
| `sanitize_fts_query` | fn | Strip FTS5 special characters from user input |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- Messages are ordered by `id ASC` (not `timestamp ASC`) to preserve insertion order when multiple messages share the same millisecond timestamp — prevents OpenAI 400 errors from assistant/tool pairing violations.
- Schema v8 adds `workspace_root TEXT` to sessions; migrations run forward-only on first open.
- The busy-retry wrapper retries up to three times on `SQLITE_BUSY` with 50 ms / 125 ms deterministic jitter, without a `rand` dependency.

---

### `ironhermes-trajectory`

**Purpose:** Append-only JSONL per-tool-call trajectory ledger. Records tool name, (redacted) args, result or error, duration, impact level, and turn index after every tool execution for downstream training data and audit.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `TrajectoryWriter` | struct | Opens/appends to `trajectories.jsonl`; one per session |
| `TrajectoryWriter::open(path)` | fn | Create with `O_APPEND | O_CREAT`; creates parent dirs |
| `TrajectoryWriter::append(entry)` | fn | Serialize entry as JSONL + `sync_data()` per line |
| `TrajectoryReader` | struct | Reads back entries from a trajectory file |
| `TrajectoryEntry` | struct | Wire record: `name`, `args`, `result`, `error`, `duration_ms`, `impact_level`, `turn_index`, `tool_call_id`, `ts` |
| `ImpactLevel` | enum | `Read=0` / `Write=5` / `SystemChange=10` (wire-stable discriminants) |
| `TrajectoryWriterHandleImpl` | struct | Concrete impl of `TrajectoryWriterHandle` (the core trait) |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- `sync_data()` is called after every write for crash safety. `Drop` also calls `sync_data().ok()` so the final entry survives panics and Ctrl+C.
- The JSONL format is IronHermes-original, not a port of the Python `agent/trajectory.py` which uses session-level ShareGPT format.
- `ImpactLevel` discriminant values are wire-stable: downstream consumers (Phase 25.4 Curator, RL pipelines) rely on the numeric weights.
- `TrajectoryWriterHandle` (the trait) lives in `ironhermes-core` to break a potential cycle between this crate and `ironhermes-agent`.

---

### `ironhermes-exec`

**Purpose:** Python sandbox runtime. Spawns Python scripts in a child process and exposes the agent's tool registry via JSON-RPC over Unix domain sockets. Also provides the session-scoped `ProcessRegistry` for tracking background terminal processes.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `Sandbox` | struct | Manages a single Python execution (child process + RPC server + timeout) |
| `SandboxResult` | struct | Captured stdout, stderr, exit code, and RPC call count |
| `SandboxConfig` | struct | Tunable parameters: `python_path`, `timeout_secs` (300), `max_rpc_calls` (50), `max_output_bytes` (50 KB), `max_stderr_bytes` (10 KB) |
| `RpcServer` | struct | Unix socket JSON-RPC server that dispatches tool calls from the Python script |
| `ToolDispatch` | trait | Decouples the RPC server from `ToolRegistry`; implemented by `ExecuteCodeTool` |
| `ProcessRegistry` | struct | In-memory registry of background processes (`MAX_PROCESSES=64`, `FINISHED_TTL_SECONDS=30 min`) |
| `ProcessSession` | struct | A tracked child process with output buffer, watch patterns, and cancellation token |
| `WatchState` | struct | Rate-limiter for watch-pattern hits (`WATCH_MAX_PER_WINDOW=8` per 10 s) |
| `CancellationToken` | re-export | `tokio_util::sync::CancellationToken` for `Sandbox::run` callers |
| `HERMES_TOOLS_PY` | const | Embedded Python helper module (`include_str!`) scripts import for tool access |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- `ProcessRegistry` is session-scoped and RAM-only (no persistence). It is drained via `drain_and_kill` on session end — `Drop`-based cleanup is intentionally absent to avoid async-in-drop hazards.
- Watch-pattern rate limiting uses `tokio::time::Instant` (not `std::time::Instant`) so tests can drive time with `tokio::time::pause()` + `advance()`.
- `ProcessSession.id` format: `"proc_"` + 12 hex characters.

---

### `ironhermes-cron`

**Purpose:** Cron scheduler. Parses cron expressions, maintains a job store, drives tick delivery, and serializes jobs to/from disk. Uses a file-based tick lock to prevent concurrent tick processing across multiple processes.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `JobStore` | struct | Persistent list of scheduled cron jobs |
| `CronJob` | struct | Job definition |
| `parse_duration` | fn | Parse duration strings (e.g. `"5m"`, `"1h"`) into seconds |
| `parse_schedule` | fn | Parse standard 5-field cron expressions into a `ScheduleParsed` |
| `scan_cron_prompt` | fn | Extract cron expressions from natural language |
| `TickResult` / `run_tick_check` | struct + fn | Evaluate which jobs are due; return due-job list |
| `DeliveryTarget` | struct | Deliver a due job to the agent loop |
| `acquire_tick_lock` / `acquire_tick_lock_at` | fns | Atomic file-based tick lock (O_CREAT | O_EXCL) |
| `LockGuard` | struct | RAII guard that removes the lock file on drop |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- The tick lock writes the owner PID to the lock file. On `AlreadyExists`, a stale-lock recovery path checks whether the recorded PID is still alive (via `kill(pid, 0)` on Unix) and removes the file if dead, then retries acquisition once.
- On non-Unix platforms the liveness check conservatively returns `true` (assume alive).

---

### `ironhermes-hooks`

**Purpose:** Event hook system. Dispatches lifecycle events (message received, tool called, tool completed, response sent, context pre-compress, etc.) to registered listeners including file-based JSONL loggers, HTTP webhooks with HMAC signing, and inline guardrail hooks.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `HookRegistry` | struct | Central dispatcher; holds sync and async listeners |
| `HookListener` / `AsyncHookListener` | type aliases | Sync and async hook subscriber interfaces |
| `HookEvent` / `HookEventKind` | structs | Event envelope and tagged event variant |
| `HookEventKind` variants | enum | `MessageReceived`, `ToolCalled`, `ToolCompleted`, `ResponseSent`, `SkillActivated`, `ContextPreCompress`, and others |
| `HooksConfig` / `WebhookEndpointConfig` | structs | Config types loaded from `config.yaml` |
| `ErrorDetailLevel` | enum | Controls how much error detail is included in hook payloads |
| `GuardrailHook` / `BlocklistGuardrail` / `GuardrailDecision` | trait + structs | Pre-execution tool-call guardrails |
| `format_guardrail_error` | fn | Human-readable guardrail block message |
| `RetryQueue` | struct | In-memory retry queue for failed webhook deliveries |
| `WebhookDelivery` / `create_webhook_listener` / `drain_retry_queue` | struct + fns | HTTP webhook dispatch with HMAC-SHA256 signing |
| `create_jsonl_listener` | fn | File-based JSONL event log listener |
| `spawn_config_watcher` | fn | Hot-reload watcher for `hooks.toml` config changes |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- `HookRegistry::fire_awaitable` is used for `ContextPreCompress` events so memory flush handlers can complete async work before the compressor runs.
- Webhook payloads are HMAC-SHA256 signed using the configured secret; the `Retry-After` header is respected on 429 responses.
- Config is loaded from `hooks.toml` (TOML format) alongside the main YAML config.

---

### `ironhermes-hub`

**Purpose:** Skills Hub client. Installs, updates, uninstalls, and trust-manages skills from GitHub repos, the skills.sh blob service, well-known HTTPS origins, and local directories. Includes content-hash locking, tarball extraction, path sanitization, and security scanning.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `install` / `uninstall` / `update` | fns | Primary lifecycle operations |
| `InstallOutcome` / `UninstallOutcome` / `UpdateOutcome` | structs | Operation results |
| `HubSource` / `SkillBundle` / `SkillMeta` / `BundleFile` | trait + structs | Source abstraction and bundle representation |
| `GitHubSource` / `GitHubTap` | structs | GitHub repo source with optional tap configuration |
| `SkillsShBlobSource` / `BlobSkill` | structs | skills.sh blob backend |
| `LocalDirSource` | struct | Install from a local filesystem directory |
| `WellKnownSkillSource` | struct | Curated well-known skill registry |
| `SkillLock` / `SkillLockEntry` / `compute_folder_hash` | structs + fn | Lock file management and content-hash integrity |
| `HubManifest` / `ManifestEntry` | structs | Hub manifest format |
| `SkillScanner` / `CoreSkillScanner` / `ScanVerdict` | trait + structs | Pluggable security scanning |
| `AlwaysBlockedScanner` / `AlwaysCleanScanner` | structs | Test-only scanner stubs |
| `enforce_trust_gate` | fn | Block installation of untrusted skills |
| `sanitize_name` / `sanitize_subpath` / `is_path_safe` / `sanitize_metadata` | fns | Path and metadata sanitization |
| `GitHubAuth` | struct | GitHub OAuth token management |
| `fetch_audit` / `AuditData` / `PartnerAudit` | fn + structs | Partner audit data retrieval |
| `HubError` / `HubErrorKind` | enums | Typed hub errors |

**Sibling dependencies:** `ironhermes-core`

**Notable details:**
- `compute_folder_hash` produces a SHA-256 content hash over all files in a skill bundle; stored in the lock file and verified on update.
- `migrate_from_hub_manifest` converts old-style hub manifests to the lock file format.
- Path sanitization (`assert_temp_contained`, `is_contained_in`) prevents directory-traversal attacks during tarball extraction.

---

### `ironhermes-tools`

**Purpose:** Tool registry and all tool implementations. Defines the `Tool` trait, registers default tools, and implements the full set of agent capabilities: file I/O, terminal execution, web fetch/extract/search, browser automation via Chromium (chromiumoxide), PDF extraction, memory operations, skill management, cron job management, background process watching, sub-agent delegation, and hardware (Hexapod TCP/video).

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `Tool` | trait | `name()`, `toolset()`, `description()`, `schema()`, `is_available()`, `prerequisites()`, `redact_args()`, `on_session_end()`, `execute()` |
| `ToolRegistry` | struct | Holds all registered `Box<dyn Tool>` instances; dispatches by name |
| `Prerequisite` | struct | Per-tool env-var or config-field requirement for the setup wizard |
| `InterceptHandler` | type alias | Pre-execution intercept point (used by guardrails) |
| `MemoryManagerHandle` | trait | Shared `Arc<Mutex<MemoryManager>>` wrapper passed to memory tools |
| `RegistryToolsetSession` | struct | Production `ToolsetSessionHandle` implementation for toolset switching |
| `WebExtractTool` | struct | Firecrawl / local fallback web content extraction |
| `todo_read_schema` / `todo_write_schema` | fns | Schema helpers for todo list tools |

Tool modules (each exports one or more `Tool` implementations):

| Module | Tools |
|--------|-------|
| `file_tools` | `read_file`, `write_file`, `list_directory`, `search_files`, and related |
| `terminal` | `terminal` (foreground shell execution) |
| `execute_code` | `execute_code` (Python sandbox via `ironhermes-exec`) |
| `web_read` | `web_read` (fetch + markdown conversion) |
| `web_search` | `web_search` (Brave / Firecrawl search) |
| `web_extract` | `web_extract` (Firecrawl or local HTML→Markdown) |
| `web_local` | Shared HTML→Markdown helpers |
| `browser_*` (12 modules) | `browser_navigate`, `browser_click`, `browser_type`, `browser_scroll`, `browser_press`, `browser_back`, `browser_close`, `browser_snapshot`, `browser_get_images`, `browser_console`, `browser_vision`, `browser_session` |
| `memory_tool` | `memory_read`, `memory_write` |
| `skills_tool` | `skills_install`, `skills_list`, `skills_search` |
| `cronjob_tool` | `cronjob_create`, `cronjob_list`, `cronjob_delete` |
| `delegate_task` | `delegate_task` (sub-agent spawning) |
| `hexapod_tcp` | Hexapod robot TCP control |
| `hexapod_video` | Stateless single-frame JPEG capture |

**Sibling dependencies:** `ironhermes-core`, `ironhermes-hub`, `ironhermes-cron`, `ironhermes-hooks`, `ironhermes-exec`

**Notable details:**
- `Tool::is_available()` walks `prerequisites()` and checks env vars; tools that require an API key return `false` when the key is absent, allowing the setup wizard to guide the user.
- `Tool::redact_args()` strips secrets from args before they are written to the trajectory ledger. The default implementation returns args unchanged; `WebExtractTool` overrides it to redact URL-embedded credentials.
- `Tool::on_session_end()` is a synchronous hook (kept non-async for object safety) that uses `tokio::spawn` internally to run cleanup fire-and-forget.
- Browser tools use `chromiumoxide` to drive a headless Chromium instance; the `browser_session` module manages the singleton browser lifecycle.

---

### `ironhermes-mcp`

**Purpose:** MCP (Model Context Protocol) client infrastructure. Connects to external MCP servers over stdio child-process or HTTP/SSE transports, exposes their tools through the agent's `ToolRegistry` under `server__tool` naming, and handles server-initiated sampling requests.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `McpManager` | struct | Orchestrates all MCP server tasks; `start_all()` / `shutdown_all()` |
| `StartResult` | struct | Per-server start outcome |
| `McpServerConfig` / `SamplingConfig` | structs | Config parsed from `mcp.yaml` / `config.yaml` |
| `interpolate_config` / `interpolate_env` | fns | `${ENV_VAR}` substitution in config values |
| `McpTool` | struct | `Tool` impl wrapping an MCP tool call; names are `server__tool` |
| `McpCallRequest` | struct | Wire format for MCP tool invocations |
| `make_prefixed_name` / `sanitize_server_name` | fns | Tool name construction and sanitization |
| `SamplingHandler` | struct | Handles server-initiated LLM requests back to the agent |
| `build_safe_env` | fn | Filters environment variables before passing to stdio server child process |
| `sanitize_error` | fn | Strips credentials from error messages |
| `CREDENTIAL_PATTERN` | const | Regex pattern for credential detection |

**Sibling dependencies:** `ironhermes-core`, `ironhermes-tools`

**Notable details:**
- Uses the `rmcp` crate (official Rust MCP SDK) with `transport-child-process` (stdio) and `transport-streamable-http-client-reqwest` (HTTP/SSE) features.
- Each MCP server runs in its own tokio task (`server_task` module) with automatic reconnection on failure.
- `build_safe_env` prevents credential leakage to untrusted stdio child processes by filtering the inherited environment.
- Tool names follow the `server__tool` convention (double underscore) to namespace MCP tools within the shared `ToolRegistry`.

---

### `ironhermes-agent`

**Purpose:** Agent loop and LLM client. Drives the turn-by-turn conversation with tool execution, context compression, memory management, sub-agent orchestration, budget enforcement, and personality selection. The most central integration crate in the workspace.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `AgentLoop` | struct | Core turn loop: calls LLM, parses tool calls, executes tools, repeats |
| `AgentResult` | struct | Loop output: `messages`, `appended`, `turns_used`, `finished_naturally`, `final_response`, `total_usage`, `compression_count_after`, `stop_reason` |
| `AggregatedUsage` | struct | Accumulated input/output token counts |
| `StopReason` | enum | `Natural` / `MaxIterations` / `BudgetExhausted` / `Cancelled` |
| `LlmClient` | struct | Async streaming LLM interface |
| `AnthropicClient` | struct | Anthropic API client (Claude models) |
| `AnyClient` | enum | Dispatching wrapper: selects Anthropic, OpenAI-compatible, or custom provider |
| `build_client` / `build_main_client` / `build_role_client` | fns | Client factory functions |
| `wire_fallback_if_configured` | fn | Wire a fallback provider to the main client |
| `ContextEngine` / `attach_context_engine` | trait + fn | Sliding-window context management |
| `ContextCompressor` | struct | Summarization-based context compression |
| `PromptBuilder` / `PromptSlot` | struct + enum | Composable system prompt assembly |
| `PersonalityRegistry` | struct | Named personality presets (concise, technical, noir, hype, catgirl, default) |
| `MemoryManager` / `SharedProvider` | struct + type | Multi-backend memory abstraction |
| `PressureTracker` | struct | Context-window pressure monitoring with tiered warnings |
| `BudgetHandle` | struct | Shared turn budget with hard-stop enforcement |
| `AgentSubagentRunner` | struct | Spawns and supervises sub-agent loops |
| `AppRuntimeBundle` / `build_app_runtime_bundle` | struct + fn | Assembled runtime dependencies for a full agent session |

**Cargo features:**
- `memory-sqlite` — enables the SQLite memory provider
- `memory-duckdb` — enables the DuckDB memory provider
- `memory-grafeo` — enables the Grafeo graph memory provider
- `test-support` — enables test-only helpers

**Sibling dependencies:** `ironhermes-core`, `ironhermes-tools`, `ironhermes-state`, `ironhermes-hooks`, `ironhermes-cron`, `ironhermes-exec`, `ironhermes-mcp`, `ironhermes-trajectory`

**Notable details:**
- `AgentResult.appended` contains only the messages produced by the current run (not the full history), making it safe to persist without re-filtering for role pairing — critical for correct OpenAI assistant↔tool ordering across turns.
- `BudgetHandle::consume()` returning `None` triggers a clean `BudgetExhausted` result rather than a panic or `process::exit`. This path cannot be bypassed by yolo mode.
- Context compression fires `ContextPreCompress` hook events and awaits async listeners (e.g., memory flush) before pruning.
- `build_app_runtime_bundle` is the single assembly point for all session dependencies; CLI, gateway, and UI all call it (or its equivalent) at session start.

---

### `ironhermes-gateway`

**Purpose:** Multi-platform messaging gateway. Currently implements a Telegram long-polling adapter that receives messages, queues them per-user, dispatches agent sessions, and delivers streaming responses back. Manages PID locking for single-instance enforcement and graceful shutdown.

**Key public items:**

| Item | Kind | Description |
|------|------|-------------|
| `GatewayRunner` | struct | Top-level orchestrator: long-polling loop, JoinSet supervision, Semaphore concurrency, CancellationToken shutdown |
| `dispatch_delivery` | fn | Route a completed agent response to the platform |
| `GatewayMessageHandler` | struct | Per-message handler: assembles `AgentLoop`, executes turn, persists messages |
| `PlatformAdapter` / `MessageHandler` | traits | Adapter interface for adding new messaging platforms |
| `TelegramAdapter` | struct | Telegram Bot API long-polling implementation |
| `TgMessage` / `TgUpdate` / `TgUser` / `TgChat` / `TgDocument` / `TgPhotoSize` / `TgFile` / `TgSendApi` / `TgBotCommand` | structs | Telegram API type wrappers |
| `GatewaySession` | struct | Per-user session state and message history |
| `StreamConsumer` | struct | Consumes streaming LLM responses and accumulates text |
| `UserQueueManager` | struct | Per-user message queues with backpressure |
| `BackoffState` | struct | Exponential backoff for long-polling errors |
| `GatewayPidRecord` / `PidLockGuard` / `acquire_pid_lock` / `read_gateway_pid` / `write_gateway_pid` / `is_pid_alive` / `PidLiveness` | struct + fns | PID file management for single-instance enforcement |

**Sibling dependencies:** `ironhermes-core`, `ironhermes-agent`, `ironhermes-tools`, `ironhermes-exec`, `ironhermes-state`, `ironhermes-cron`, `ironhermes-hooks`, `ironhermes-mcp`, `ironhermes-trajectory`

**Notable details:**
- `GatewayRunner` holds an optional `Arc<McpManager>` and calls `mgr.shutdown_all().await` on graceful shutdown so stdio MCP children are reaped and the process exits in bounded time.
- Multimodal attachments (images, documents) are handled by the `multimodal` module before passing to the agent.
- The `rate_limiter` module caps per-user request rates to prevent abuse.
- `TrajectoryWriter` is opened at gateway start and threaded through `GatewayRunner` → `GatewayMessageHandler` → per-message `CommandContext`.

---

### `ironhermes-cli`

**Purpose:** Interactive CLI binary (`ironhermes`) and supporting library. Provides the primary user-facing entry point with subcommands for chat (classic REPL and ratatui TUI), single-shot execution, gateway daemon control, configuration, sessions, memory, models, providers, skills, toolsets, cron, and status/health checks.

**Binary:** `ironhermes` (defined in `src/main.rs`)

**Key public items (library):**

| Item | Kind | Description |
|------|------|-------------|
| `tui_rata` module | mod | Ratatui-backed REPL (`App`, `ui`, `StreamEvent`) |
| `tui` module | mod | Classic terminal rendering helpers |
| `cli_args` module | mod | `Cli` / `Commands` clap argument definitions |
| `io_gate` module | mod | `can_prompt()` / `is_terminal_stdin()` TTY detection |
| `yolo` module | mod | Yolo-mode resolution and banner printing |
| `repl_input` module | mod | Non-blocking rustyline input on dedicated OS thread |
| `ReplInputChannel` / `ReplLine` / `PromptRequest` / `ExternalPrinterHandle` | structs | REPL input channel types |
| `can_prompt` / `is_terminal_stdin` | fns | Re-exported TTY helpers |
| `maybe_print_yolo_banner` / `print_yolo_banner_to_stderr` / `resolve_yolo` | fns | Yolo mode helpers |
| `status_cmd` module | mod | `StatusReport` and deep-probe health check |
| `session_cmd` module | mod | `hermes session export` / `export-all` subcommands |
| `memory_cmd` / `skills_cmd` / `toolset_cmd` | mods | Memory, skills, and toolset subcommands |
| `setup` module | mod | Interactive setup wizard |

**Cargo features:** `memory-sqlite`, `memory-duckdb`, `memory-grafeo`, `test-support` — all forwarded to `ironhermes-agent`.

**Sibling dependencies:** `ironhermes-core`, `ironhermes-agent`, `ironhermes-tools`, `ironhermes-exec`, `ironhermes-state`, `ironhermes-gateway`, `ironhermes-cron`, `ironhermes-hooks`, `ironhermes-hub`, `ironhermes-mcp`, `ironhermes-trajectory`

**Notable details:**
- The ratatui REPL (`tui_rata`) runs the agent loop and renders streaming output side-by-side with an input textarea, log panel, and status bar.
- `ReplInputChannel` hosts a blocking `rustyline::DefaultEditor` on a dedicated OS thread so `run_chat` can poll for user input from a `tokio::select!` arm alongside an in-flight agent turn — enabling mid-turn commands like `/agents list|kill|logs`.
- `TrajectoryWriter` is opened at session start in `run_chat`, `run_single`, `run_gateway`, and `tui_rata::build_app_deps` and wrapped as `TrajectoryWriterHandleImpl` before being threaded into `CommandContext`.
- `LiveDeepProbe` (in `status_cmd`) uses `reqwest` to HEAD-probe configured providers and `rusqlite` to run `PRAGMA integrity_check` on the state database.

---

### `iron_hermes_ui`

**Purpose:** Dioxus 0.7 fullstack web/desktop/mobile UI. Renders a terminal-style application shell (command stream, agent side panel, command palette, theme system) compiled to WASM for the web target and to native binaries for desktop and mobile. In `server` feature mode it embeds the agent directly via Axum and the IronHermes sibling crates.

**Key modules:**

| Module | Description |
|--------|-------------|
| `app` | Root `App` component; Dioxus router entry point |
| `components/` | UI components: HermesApp, shell variants, input, blocks, panels |
| `server/` | Server-side code: `AppState` initialization, Axum route handlers, WebSocket (`ws.rs`), agent API (`api.rs`) |
| `state` | Client-side reactive state (signals) |
| `protocol` | Shared message types between client and server |
| `platform/` | Platform-specific helpers (native sleep, etc.) |
| `mocks/` | Mock agent responses for development/demo |
| `fonts` | Font asset registration |
| `ui_prefs` | User preference persistence (localStorage / server-side) |

**Cargo features:**

| Feature | Effect |
|---------|--------|
| `web` (default) | Compile to WASM via Dioxus web renderer |
| `desktop` | Native window via Dioxus desktop (webview) |
| `mobile` | iOS/Android via Dioxus mobile |
| `server` | Add Axum + fullstack Dioxus; embed agent crates (non-WASM only) |
| `demo` | Use mock responses instead of live agent |
| `legacy-shell` | Mount `WarpHermes` shell instead of the default `HermesApp` |

**Sibling dependencies (server/non-WASM target only):** `ironhermes-core`, `ironhermes-agent`, `ironhermes-exec`, `ironhermes-hooks`, `ironhermes-state`, `ironhermes-tools`

**Notable details:**
- WASM and native targets share one codebase. IronHermes sibling crates are gated behind `#[cfg(not(target_arch = "wasm32"))]` so the WASM client never compiles server-only code.
- The `server` feature activates Axum and switches to a fullstack Dioxus server that serves both the WASM bundle and the agent API from a single binary.
- Uses Dioxus 0.7 signal-based state (`use_signal`, `use_memo`, `use_resource`, `use_context_provider`). The Dioxus 0.6 APIs (`cx`, `Scope`, `use_state`) are forbidden.
- `clippy.toml` enforces that `GenerationalRef` / `GenerationalRefMut` / `WriteLock` are never held across `.await` points.
- The design system is an ANSI-derived 16-color palette with four theme variants (`cyan`, `magenta`, `green`, `amber`) and three density modes (`comfy`, `compact`).

---

## Memory provider crates (`providers/`)

All three providers implement the `MemoryProvider` trait from `ironhermes-core` and are loaded into `ironhermes-agent` via optional Cargo features.

### `memory-sqlite`

**Purpose:** SQLite-backed memory store with FTS5 full-text search. Default memory backend.

**Sibling dependencies:** `ironhermes-core`

**External dependency:** `rusqlite` (bundled)

---

### `memory-grafeo`

**Purpose:** Grafeo graph database memory provider. Stores memories as graph nodes and edges, enabling relationship-aware retrieval.

**Sibling dependencies:** `ironhermes-core`

**External dependency:** `grafeo` 0.5, `grafeo-common` 0.5

---

### `memory-duckdb`

**Purpose:** DuckDB columnar memory provider. Enables analytical queries over memory entries using DuckDB's in-process SQL engine.

**Sibling dependencies:** `ironhermes-core`

**External dependency:** `duckdb` 1.x (bundled)

---

## Dependency graph

The table below shows each crate's direct sibling-crate dependencies in topological order (leaves first, most-dependent last). External crates are omitted.

```
Level 0 — no sibling deps (leaves):
  ironhermes-core

Level 1 — depend only on ironhermes-core:
  ironhermes-state
  ironhermes-trajectory
  ironhermes-exec
  ironhermes-cron
  ironhermes-hooks
  ironhermes-hub
  memory-sqlite
  memory-grafeo
  memory-duckdb

Level 2 — depend on level-0/1 crates:
  ironhermes-tools        → core, hub, cron, hooks, exec
  ironhermes-mcp          → core, tools

Level 3 — depend on level-0/1/2 crates:
  ironhermes-agent        → core, tools, state, hooks, cron, exec, mcp, trajectory
                             (+ optional: memory-sqlite, memory-grafeo, memory-duckdb)

Level 4 — depend on level-0..3 crates:
  ironhermes-gateway      → core, agent, tools, exec, state, cron, hooks, mcp, trajectory
  iron_hermes_ui          → core, agent, exec, hooks, state, tools  (server feature only)

Level 5 — depends on all other crates:
  ironhermes-cli          → core, agent, tools, exec, state, gateway, cron, hooks,
                             hub, mcp, trajectory
```

### Simplified directed graph

```
ironhermes-core
    ├── ironhermes-state
    ├── ironhermes-trajectory
    ├── ironhermes-exec
    ├── ironhermes-cron
    ├── ironhermes-hooks
    ├── ironhermes-hub
    ├── memory-sqlite
    ├── memory-grafeo
    ├── memory-duckdb
    └── ironhermes-tools
            ├── (hub, cron, hooks, exec)
            └── ironhermes-mcp
                    └── ironhermes-agent
                            ├── (state, hooks, cron, exec, trajectory)
                            ├── (memory-sqlite / memory-grafeo / memory-duckdb)
                            ├── ironhermes-gateway
                            │       └── ironhermes-cli  ← binary entry point
                            └── iron_hermes_ui          ← UI entry point
```

> **Excluded from workspace:** `crates/ironagent-tools-api` — stale duplicate of `ironhermes-tools`, pending removal.
