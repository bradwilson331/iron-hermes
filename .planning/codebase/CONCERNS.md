# Codebase Concerns

**Analysis Date:** 2026-04-01

## Feature Gaps (vs Python hermes-agent)

### Gateway Platform Adapters

- Issue: Only Telegram adapter is implemented. The Python gateway has 15 platform adapters: Discord, Slack, WhatsApp, Signal, Matrix, Mattermost, Email, SMS, DingTalk, Feishu, WeCom, HomeAssistant, Webhook, and API Server.
- Files: `crates/ironhermes-gateway/src/telegram.rs` (only adapter), `crates/ironhermes-gateway/src/lib.rs`
- Impact: IronHermes cannot replace the Python agent for any non-Telegram gateway deployment. Discord is likely the highest-priority gap given community usage.
- Fix approach: Implement adapters one at a time following the `PlatformAdapter` trait in `crates/ironhermes-gateway/src/adapter.rs`. Start with Discord (most requested), then Slack, then Webhook/API Server for programmatic access.

### Gateway Runner Not Wired to Agent

- Issue: `GatewayRunner::start()` in `crates/ironhermes-gateway/src/runner.rs` accepts a `Box<dyn MessageHandler>` but never actually calls `adapter.start(handler)` on created adapters. The handler is accepted but unused -- adapters are pushed to the vec without being started. There is a comment: "In a real implementation, we'd use Arc<dyn MessageHandler>".
- Files: `crates/ironhermes-gateway/src/runner.rs` (lines 26-56)
- Impact: The gateway binary path does not function at all. Even the Telegram adapter, which is fully implemented, cannot receive or respond to messages through the gateway runner.
- Fix approach: Change the handler parameter to `Arc<dyn MessageHandler>`, clone it per adapter, and call `adapter.start()` for each configured adapter before entering the ctrl-c wait loop.

### Missing Tools (40+ Python tools vs 6 Rust tools)

- Issue: The Rust crate implements only 6 tools: `terminal`, `read_file`, `write_file`, `patch`, `search_files`, `web_search`. The Python agent has 40+ tools including critical ones.
- Files: `crates/ironhermes-tools/src/registry.rs` (lines 75-86, `register_defaults`)
- Impact: Major capability gap. The agent cannot perform many tasks the Python agent handles.
- Missing critical tools (by priority):
  - **code_execution** (sandboxed execution) -- Python: `tools/code_execution_tool.py`
  - **delegate_task** (sub-agent spawning) -- Python: `tools/delegate_tool.py`
  - **memory** (persistent memory/recall) -- Python: `tools/memory_tool.py`
  - **vision** (image analysis) -- Python: `tools/vision_tools.py`
  - **image_generation** -- Python: `tools/image_generation_tool.py`
  - **send_message** (cross-platform messaging) -- Python: `tools/send_message_tool.py`
  - **clarify** (ask user for clarification) -- Python: `tools/clarify_tool.py`
  - **mcp** (Model Context Protocol) -- Python: `tools/mcp_tool.py`
  - **skills** (skill management) -- Python: `tools/skills_tool.py`, `tools/skill_manager_tool.py`
  - **honcho** (memory integration) -- Python: `tools/honcho_tools.py`
  - **todo** (task management) -- Python: `tools/todo_tool.py`
  - **browser** (web browsing) -- Python: `tools/browser_tool.py`
  - **cronjob_tools** (cron management from agent) -- Python: `tools/cronjob_tools.py`
  - **checkpoint_manager** -- Python: `tools/checkpoint_manager.py`
  - **transcription/TTS** -- Python: `tools/transcription_tools.py`, `tools/tts_tool.py`
- Fix approach: Implement tools incrementally. Priority order: clarify, delegate_task, memory, send_message, code_execution, vision, mcp.

### Execution Environments

- Issue: The Python agent supports 7 execution environments (Local, Docker, SSH, Modal, Daytona, Singularity, PersistentShell). IronHermes terminal tool only runs local `sh -c` commands with no environment abstraction.
- Files: `crates/ironhermes-tools/src/terminal.rs` (entire file, single `sh -c` approach)
- Impact: Cannot run commands in sandboxed or remote environments. No Docker, SSH, or cloud execution support.
- Fix approach: Create an `Environment` trait in `ironhermes-tools` similar to Python's `BaseEnvironment` in `tools/environments/base.py`, then implement backends.

### Skills System

- Issue: The Python agent has a full skills system (loading, syncing, executing markdown-defined skills from a hub). IronHermes has no skills support whatsoever.
- Files: Not applicable -- no Rust code exists for this.
- Impact: Cannot use community skills or custom skill definitions.
- Fix approach: Port the skills loader and executor. The Python implementation spans `tools/skills_tool.py`, `tools/skills_hub.py`, `tools/skills_sync.py`, `tools/skill_manager_tool.py`.

### Smart Model Routing and Prompt Caching

- Issue: The Python agent has `agent/smart_model_routing.py` for dynamic model selection and `agent/prompt_caching.py` for Anthropic-style prompt caching. IronHermes has neither.
- Files: `crates/ironhermes-agent/src/client.rs` (single model, no routing)
- Impact: Higher API costs (no caching), no ability to route simple vs complex queries to different models.
- Fix approach: Add model routing logic to `LlmClient` and implement cache_control content blocks for Anthropic API.

### Credential Pool and Usage Pricing

- Issue: The Python agent has `agent/credential_pool.py` for managing multiple API keys with rotation and `agent/usage_pricing.py` for cost tracking. IronHermes tracks token counts but has no cost calculation or key rotation.
- Files: `crates/ironhermes-agent/src/agent_loop.rs` (`AggregatedUsage` struct, lines 27-38)
- Impact: Cannot manage multiple API keys for high-volume deployments. No cost visibility.
- Fix approach: Add pricing data to `ironhermes-core/src/constants.rs` and implement key rotation in `LlmClient`.

### Title Generation and Session Insights

- Issue: The Python agent auto-generates session titles via `agent/title_generator.py` and provides session insights via `agent/insights.py`. IronHermes `StateStore` has a `title` column but no auto-generation.
- Files: `crates/ironhermes-state/src/lib.rs` (line 132, `title` field exists but unused by agent)
- Impact: Sessions are unnamed, making history browsing difficult.
- Fix approach: After first user message, send a title-generation prompt to the LLM and call `update_session_title`.

### Anthropic Native API Support

- Issue: The Python agent has `agent/anthropic_adapter.py` for native Anthropic API format (non-OpenAI compatible). IronHermes only speaks OpenAI-compatible chat completions format.
- Files: `crates/ironhermes-agent/src/client.rs` (entire file, OpenAI format only)
- Impact: Cannot use Anthropic's native features (extended thinking, prompt caching with cache_control blocks, etc.) when connecting directly to Anthropic API.
- Fix approach: Add an Anthropic-native client alongside the OpenAI-compatible one, selected by provider config.

## Security Concerns

### No Command Approval System

- Risk: The terminal tool executes arbitrary shell commands with zero approval or safety checks. The Python agent has a comprehensive dangerous command detection system (`tools/approval.py`) with pattern matching for destructive operations (rm -rf, chmod 777, mkfs, dd, etc.).
- Files: `crates/ironhermes-tools/src/terminal.rs` (lines 59-64, direct `sh -c` execution)
- Current mitigation: None. Any command the LLM generates is executed immediately.
- Recommendations: Port the dangerous command pattern matching from `hermes-agent/tools/approval.py` (DANGEROUS_PATTERNS list). Implement an approval callback in the `Tool` trait. At minimum, block commands matching known destructive patterns without explicit user confirmation.

### No Secret Redaction Implementation

- Risk: The config has `redact_secrets: bool` but it is never read or used anywhere in the codebase. The Python agent has thorough regex-based secret redaction (`agent/redact.py`) that masks API keys, tokens, and credentials in logs and tool output.
- Files: `crates/ironhermes-core/src/config.rs` (lines 158-168, config exists), no implementation anywhere
- Current mitigation: None. API keys and tokens in command output are passed through unredacted.
- Recommendations: Implement a `redact()` function in `ironhermes-core` matching the patterns from `hermes-agent/agent/redact.py`. Apply it to all tool output before adding to conversation history. Apply it to all log output.

### No Tirith Security Scanning

- Risk: The Python agent integrates Tirith (`tools/tirith_security.py`) for pre-execution security scanning of commands (homograph URLs, pipe-to-interpreter, terminal injection, etc.). IronHermes has no equivalent.
- Files: Not applicable -- no Rust code exists for this.
- Current mitigation: None.
- Recommendations: Either port the Tirith integration (calling the binary as a subprocess) or implement equivalent pattern-based scanning natively in Rust.

### File Operations Have No Path Restrictions

- Risk: `ReadFileTool`, `WriteFileTool`, and `PatchFileTool` operate on any path with no sandboxing, allowlisting, or path traversal protection. The agent can read/write `/etc/passwd`, `~/.ssh/`, or any system file.
- Files: `crates/ironhermes-tools/src/file_tools.rs` (lines 51-73 read, lines 117-141 write)
- Current mitigation: None. No path canonicalization, no symlink resolution, no restricted paths.
- Recommendations: Add configurable path restrictions (allowlist of directories). Canonicalize paths before operations. Block writes to sensitive locations (`~/.ssh/`, `/etc/`, `.env` files). The Python agent's `tools/credential_files.py` and `tools/approval.py` have path-based restrictions to reference.

### API Key Stored in Memory as Plain String

- Risk: `LlmClient` stores the API key as a plain `String` field. This means the key persists in heap memory and could appear in core dumps or memory inspection.
- Files: `crates/ironhermes-agent/src/client.rs` (line 36, `api_key: String`)
- Current mitigation: None.
- Recommendations: Use a `secrecy::Secret<String>` wrapper (from the `secrecy` crate) to zeroize on drop and prevent accidental logging via `Debug`/`Display`.

### Telegram Bot Token in Memory

- Risk: Same plain-string storage issue for the Telegram bot token.
- Files: `crates/ironhermes-gateway/src/telegram.rs` (line 17, `token: String`)
- Current mitigation: None.
- Recommendations: Same as above -- use `secrecy::Secret<String>`.

## Incomplete Implementations

### Gateway Runner is a Stub

- Issue: `GatewayRunner::start()` creates adapter instances but never starts them. The handler parameter is unused. The method signature accepts `Box<dyn MessageHandler>` but needs `Arc<dyn MessageHandler>` to clone across adapters.
- Files: `crates/ironhermes-gateway/src/runner.rs` (lines 26-56)
- Impact: Gateway mode is completely non-functional.
- Fix approach: Wire up adapter.start() calls, change handler to Arc, implement actual message routing.

### Cron Scheduler Has No Runner Loop

- Issue: `ironhermes-cron` has a complete `JobStore` with persistence, due-job detection, and tick locking, but there is no scheduler loop that periodically checks for due jobs and dispatches them to the agent. The CLI has no `cron` subcommand.
- Files: `crates/ironhermes-cron/src/lib.rs` (complete store, no runner), `crates/ironhermes-cli/src/main.rs` (no cron subcommand)
- Impact: Cron jobs can be defined but never execute.
- Fix approach: Add a `CronRunner` that loops on a timer, calls `get_due_jobs()`, dispatches each to an `AgentLoop`, and calls `mark_job_run()`. Add CLI subcommands for `cron add`, `cron list`, `cron remove`, `cron run` (daemon mode).

### CLI Missing Subcommands

- Issue: The CLI only has `chat`, `status`, `doctor`, and `version`. The Python CLI has many more: `gateway`, `cron`, `session` (history/search/resume), `config`, `setup`.
- Files: `crates/ironhermes-cli/src/main.rs` (lines 46-59, `Commands` enum)
- Impact: Cannot manage gateway, cron jobs, sessions, or configuration from the CLI.
- Fix approach: Add subcommands incrementally. Priority: `gateway` (start gateway mode), `session list/search/show` (history), `cron` (job management).

### Session State Not Used by CLI

- Issue: The `StateStore` in `ironhermes-state` is fully implemented with session creation, message storage, and FTS search. However, the CLI chat mode (`run_chat`, `run_single`) never creates sessions or stores messages.
- Files: `crates/ironhermes-cli/src/main.rs` (lines 204-241 `run_single`, lines 244-328 `run_chat` -- no StateStore usage)
- Impact: No conversation history persistence. Sessions are lost when the CLI exits.
- Fix approach: Open `StateStore::open_default()` at CLI startup, call `create_session()`, and `add_message()` for each turn. Wire up `update_session_stats()` with usage data from `AgentResult`.

### Context Compression Lacks LLM-Based Summarization

- Issue: The `ContextCompressor` only does local truncation (pruning tool results, dropping middle messages). The Python agent's `trajectory_compressor.py` and `agent/context_compressor.py` use LLM calls to generate intelligent summaries of dropped context.
- Files: `crates/ironhermes-agent/src/context_compressor.rs` (lines 79-109, local-only compression)
- Impact: When context is compressed, the agent loses important information without intelligent summarization. The placeholder comment "For LLM-based summarization, use `compress_with_summary`" indicates intent but no implementation.
- Fix approach: Add a `compress_with_summary` method that calls the LLM to summarize the dropped middle section before replacing it.

### No Streaming Response Splitting for Gateway

- Issue: The Python gateway has `stream_consumer.py` for chunking streaming LLM output into platform-appropriate message sizes. IronHermes Telegram adapter sends the entire response as one message after the handler completes.
- Files: `crates/ironhermes-gateway/src/telegram.rs` (lines 126-133, single sendMessage call)
- Impact: Long responses may exceed Telegram's 4096-char message limit. No progressive "typing" indicator.
- Fix approach: Implement message chunking in the adapter, split on paragraph boundaries, and use editMessage for progressive updates.

## Performance Considerations

### Synchronous File I/O in Async Context

- Problem: `ReadFileTool`, `WriteFileTool`, `PatchFileTool`, and `SearchFilesTool` all use blocking `std::fs` operations inside async tool execution (the `Tool::execute` is async but the implementations block).
- Files: `crates/ironhermes-tools/src/file_tools.rs` (entire file uses `std::fs`)
- Cause: The `async_trait` execute method gives the appearance of async, but the actual I/O is synchronous, blocking the tokio runtime thread.
- Improvement path: Use `tokio::fs` for file operations, or wrap blocking operations in `tokio::task::spawn_blocking()`. For `SearchFilesTool`, which can scan many files, this is especially important.

### SearchFilesTool Scans Files Sequentially

- Problem: The glob-based file search reads every matching file sequentially, one at a time. For large directory trees this is very slow.
- Files: `crates/ironhermes-tools/src/file_tools.rs` (lines 286-323)
- Cause: Single-threaded sequential glob iteration and file reading.
- Improvement path: Use `tokio::task::spawn_blocking` with rayon or similar for parallel file scanning. Consider using the `ignore` crate (used by ripgrep) for gitignore-aware, parallel directory traversal.

### New reqwest::Client Per WebSearch Call

- Problem: `WebSearchTool::execute` creates a new `reqwest::Client` on every invocation instead of reusing a shared client with connection pooling.
- Files: `crates/ironhermes-tools/src/web_search.rs` (line 82, `let client = reqwest::Client::new()`)
- Cause: Tool trait has no mechanism for shared state or dependency injection.
- Improvement path: Add a way to inject shared state (e.g., a shared HTTP client) into tools, or make `WebSearchTool` hold a `Client` instance.

### In-Memory Gateway Sessions Not Bounded

- Problem: `SessionStore` in the gateway is a `HashMap` that grows without limit. Long-running gateway instances accumulate sessions forever.
- Files: `crates/ironhermes-gateway/src/session.rs` (lines 76-111, no eviction)
- Cause: No TTL, no LRU eviction, no max size.
- Improvement path: Add a max session count with LRU eviction, or a TTL-based cleanup that removes sessions idle for more than N hours.

### Agent Loop Clones Full Message History Each Turn (CLI)

- Problem: In `run_agent_turn`, the entire message vec is cloned before passing to `agent.run()`, then the result replaces the original. This means the full conversation is copied every turn.
- Files: `crates/ironhermes-cli/src/main.rs` (line 351, `agent.run(messages.clone())`)
- Cause: Ownership model -- `AgentLoop::run` takes `Vec<ChatMessage>` by value.
- Improvement path: Change `AgentLoop::run` to take `&mut Vec<ChatMessage>` or use `Arc<Mutex<Vec<ChatMessage>>>` to avoid the clone.

## Deployment Gaps

### No Dockerfile

- Issue: No Dockerfile, docker-compose, or container configuration exists. The Python agent has `Dockerfile`, `docker/` directory with multi-stage builds, and compose configurations.
- Files: Not applicable -- no deployment files exist.
- Impact: Cannot containerize or deploy IronHermes in any standard container orchestration system.
- Fix approach: Create a multi-stage Dockerfile (builder with Rust toolchain, runtime with minimal base). Reference `hermes-agent/Dockerfile` and `hermes-agent/docker/` for patterns.

### No CI/CD Pipeline

- Issue: No `.github/workflows/` directory, no CI configuration of any kind.
- Files: Not applicable.
- Impact: No automated testing, building, or release process. No protection against regressions.
- Fix approach: Create GitHub Actions workflows for: `cargo test`, `cargo clippy`, `cargo fmt --check`, and release binary builds.

### No Configuration Setup Wizard

- Issue: The Python agent has `setup-hermes.sh` for guided first-run configuration. IronHermes has `doctor` and `status` but no interactive setup.
- Files: `crates/ironhermes-cli/src/main.rs` (lines 155-192, `cmd_doctor` only checks, does not create)
- Impact: New users must manually create `~/.ironhermes/config.yaml` and `.env` files.
- Fix approach: Add a `setup` subcommand that prompts for API keys, creates the home directory, writes default config, and validates connectivity.

### No systemd/launchd Service Files

- Issue: No service definition files for running the gateway or cron daemon as a system service.
- Files: Not applicable.
- Impact: Cannot easily run IronHermes as a persistent background service.
- Fix approach: Add service file templates for systemd (Linux) and launchd (macOS).

### No Release/Distribution Mechanism

- Issue: No release configuration, no cross-compilation setup, no binary distribution plan.
- Files: Not applicable.
- Impact: Users must build from source with a Rust toolchain.
- Fix approach: Set up `cargo-dist` or GitHub Actions matrix builds for Linux (x86_64, aarch64), macOS (x86_64, aarch64), and Windows targets.

## Test Coverage Gaps

### Minimal Test Coverage

- What's not tested: The entire agent loop, LLM client, all tools, gateway adapters, CLI commands, config loading, and state store operations.
- Files: Only `crates/ironhermes-cron/src/lib.rs` (9 tests) and `crates/ironhermes-agent/src/context_compressor.rs` (2 tests) have any tests.
- Risk: Any refactoring or feature addition could silently break existing functionality. The core agent loop (`agent_loop.rs`), LLM client (`client.rs`), SSE stream parsing, tool execution, and state persistence are all untested.
- Priority: High
- Fix approach: Add tests in this order:
  1. `ironhermes-core`: Config loading, type serialization/deserialization, message constructors
  2. `ironhermes-state`: Session CRUD, message storage, FTS search (use temp SQLite databases)
  3. `ironhermes-tools`: Tool execution for file_tools (use temp directories), terminal tool (safe commands)
  4. `ironhermes-agent`: LLM client request building (mock HTTP), stream parsing, tool call assembly
  5. `ironhermes-gateway`: Telegram message event conversion, session store operations

## Dependencies at Risk

### rusqlite Bundled SQLite

- Risk: Using `rusqlite` with `bundled` feature compiles SQLite from C source. This increases build time and binary size, and the bundled version may lag behind security patches.
- Impact: Slower CI builds, potential security lag.
- Migration plan: Consider using system SQLite on platforms where it's available (feature-flag `bundled` for cross-compilation only).

### edition = "2024"

- Risk: Rust edition 2024 requires a very recent Rust toolchain (1.85+). This limits contributor accessibility and may cause issues with older CI runner images.
- Impact: Contributors with older Rust versions cannot build.
- Migration plan: Document minimum Rust version in README. Consider whether edition 2021 would suffice.

---

*Concerns audit: 2026-04-01*
