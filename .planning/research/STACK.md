# Technology Stack — v2.0 Intelligence & Identity

**Project:** IronHermes v2.0
**Researched:** 2026-04-11
**Scope:** NEW additions only — validated stack (tokio, reqwest, serde, rusqlite 0.32, anyhow, tracing, chrono, uuid, cron, clap, rustyline, tempfile, etc.) is NOT re-researched here.
**Confidence:** HIGH (all versions verified against crates.io live data)

---

## Upgrade First: rusqlite 0.32 → 0.39

The workspace currently pins `rusqlite = { version = "0.32", features = ["bundled", "backup"] }`. The FTS5 virtual table support required for session storage belongs to the `vtab` feature, which is bundled in `bundled-full` (= `bundled` + `modern-full`). Version 0.39.0 (released 2026-03-15) bundles SQLite 3.51.3.

**Change to:**
```toml
rusqlite = { version = "0.39", features = ["bundled-full"] }
```

`bundled-full` enables: `bundled` (no system SQLite needed) + `vtab` (FTS5 virtual tables) + `backup` + `hooks` + `functions` + `chrono` + `uuid` + `serialize` + `serde_json` + all other non-bindgen features. This replaces the current `["bundled", "backup"]` and adds FTS5 without any new crate.

**Confidence:** HIGH — verified rusqlite 0.39.0 feature list at docs.rs/crate/rusqlite/latest/features.

---

## Feature-by-Feature Stack Additions

### 1. Session Storage (SQLite + FTS5 + Migrations)

**What it does:** Persists sessions and messages to `~/.ironhermes/state.db` with FTS5 full-text search, schema versioning, write contention handling, and session lineage tracking.

**Approach:** Extend `ironhermes-state` using existing `rusqlite`. FTS5 requires no new crate — it is built into SQLite and exposed via rusqlite's `vtab` feature (included in `bundled-full`). Schema migrations require a lightweight migration runner.

**New crate: `rusqlite_migration = "2.5"`**

rusqlite_migration (latest stable: 2.5.0) is the standard schema migration library for rusqlite. It uses a sequential versioned migration pattern with idempotent `ALTER TABLE ADD COLUMN` — exactly matching the hermes-agent migration pattern (currently at schema version 6). It supports WAL mode and async-compatible usage (synchronous Rust, called from blocking tokio tasks via `spawn_blocking`).

**Schema pattern (from hermes-agent reference implementation):**
```sql
-- Sessions with lineage (parent_session_id for compression ancestry)
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,          -- "telegram" | "cli" | "agent"
    user_id TEXT,
    model TEXT,
    parent_session_id TEXT,        -- lineage: set when session spawned from compression
    started_at REAL NOT NULL,
    ended_at REAL,
    input_tokens INTEGER DEFAULT 0,
    output_tokens INTEGER DEFAULT 0,
    cache_read_tokens INTEGER DEFAULT 0,
    cache_write_tokens INTEGER DEFAULT 0,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
);

-- Messages with FTS5 trigger sync
CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT,
    tool_calls TEXT,               -- JSON string
    timestamp REAL NOT NULL,
    token_count INTEGER
);

-- FTS5 virtual table (content table mode — references messages)
CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    content,
    content=messages,
    content_rowid=id
);
-- Three triggers maintain sync: INSERT, UPDATE, DELETE on messages
```

**Write contention:** Multiple processes share `state.db` (gateway + CLI + worktree agents). Handle via: WAL journal mode (`PRAGMA journal_mode=WAL`), `busy_timeout` of 5000ms, and `BEGIN IMMEDIATE` transactions with application-level retry (20–150ms jitter, up to 15 retries) — no new crate needed, this is rusqlite config.

**New crates required:** `rusqlite_migration = "2.5"`

**What NOT to do:** Do not use `sqlx` — it is async-first with a macro-heavy query system and requires a connection pool abstraction that conflicts with rusqlite's single-connection model. Do not add a connection pool crate; hermes-agent uses a single `Arc<Mutex<Connection>>` per process which is correct for this workload.

---

### 2. Persistent Memory (MEMORY.md / USER.md + MemoryProvider trait)

**What it does:** Bounded fact stores in markdown files, plus a pluggable `MemoryProvider` trait with SQLite, Grafeo, and DuckDB backends. Only one external provider can be active at a time; the built-in file-based memory is always active.

**Approach:** Define `MemoryProvider` as an `async-trait` object (already in workspace) in `ironhermes-core`. The trait mirrors hermes-agent's Python ABC:

```rust
#[async_trait]
pub trait MemoryProvider: Send + Sync {
    fn name(&self) -> &str;
    async fn is_available(&self) -> bool;
    async fn initialize(&mut self, session_id: &str, hermes_home: &Path) -> anyhow::Result<()>;
    fn get_tool_schemas(&self) -> Vec<ToolSchema>;
    async fn handle_tool_call(&self, name: &str, args: &serde_json::Value) -> anyhow::Result<String>;
    // Optional lifecycle hooks (default no-ops):
    async fn sync_turn(&self, _turn: &TurnSummary) -> anyhow::Result<()> { Ok(()) }
    async fn on_session_end(&self) -> anyhow::Result<()> { Ok(()) }
    async fn get_context_prefix(&self) -> Option<String> { None }
}
```

**Built-in (SQLite) provider:** Uses existing rusqlite in `ironhermes-state`. Stores facts as rows with timestamp, category, and content. Bounded capacity managed by the `memory_tool` (add/replace/remove with substring matching). No new crate.

**Grafeo provider:** `grafeo = "0.5"` (latest stable: 0.5.35)

Grafeo is a pure-Rust embeddable graph database (MIT license, actively maintained). For agent memory, it provides: entity-relationship storage for USER.md-style facts, graph traversal for associative recall, HNSW-based vector similarity search for semantic memory retrieval, and GQL/Cypher query support. It is optional — only added to `ironhermes-state` behind a Cargo feature flag `memory-grafeo`.

Key: Grafeo runs embedded (no server process), is a single-binary-compatible library, and was specifically designed for AI agent use cases. The `embedded` profile (default) includes GQL, AI features, vector/text/hybrid search.

**DuckDB provider:** `duckdb = { version = "1.10501", features = ["bundled"] }` (maps to DuckDB v1.5.1)

DuckDB is optimal for analytics queries over conversation history — aggregations, token usage analysis, batch memory queries. For agent memory it excels at: structured fact storage with SQL analytics, Parquet/JSON export of memory snapshots, and columnar aggregation for `session_search` results. It is optional — added behind Cargo feature flag `memory-duckdb`.

Note: `duckdb` with `bundled` feature compiles DuckDB from source during `cargo build`. This increases build time by 2–3 minutes. Use `memory-duckdb` feature flag so it's opt-in, not default.

**What NOT to do:** Do not make DuckDB or Grafeo default dependencies — both increase binary size and build time significantly. Do not implement all three providers in the same crate without feature flags. Do not use `async-std` for Grafeo async calls — convert to blocking calls in `spawn_blocking` to stay on tokio.

---

### 3. Context Compression (Dual System + ContextEngine trait)

**What it does:** Two compression triggers: (1) gateway hygiene at 85% context window — drops oldest messages to keep conversation flowing; (2) agent `ContextEngine` at 50% — calls LLM to produce a structured summary, starts a new session with `parent_session_id` set for lineage.

**Approach:** Define `ContextEngine` as a trait in `ironhermes-agent`. The default implementation calls the LLM API (already in `ironhermes-agent`) with a summarization prompt. No new external crate needed for the compression logic itself.

**Token counting for threshold detection:** `tiktoken-rs = "0.11"` (latest stable: 0.11.0)

tiktoken-rs is the standard Rust crate for OpenAI-compatible token counting. It provides `cl100k_base` (GPT-4/Claude-compatible) and `o200k_base` encoders. Use `tiktoken_rs::get_bpe_from_model` to count tokens in message history before each API call. This is needed to correctly calculate the 85%/50% thresholds without making a speculative API call.

Alternative: use a character-count heuristic (~4 chars/token). This is LOW confidence and will misfire on code blocks, JSON tool results, and non-Latin text. Use tiktoken-rs instead.

**Compression trait:**
```rust
#[async_trait]
pub trait ContextEngine: Send + Sync {
    /// Returns a structured summary of the messages provided.
    async fn compress(&self, messages: &[ChatMessage], model: &str) -> anyhow::Result<String>;
    /// Token threshold (0.0–1.0) at which compression triggers.
    fn threshold(&self) -> f32 { 0.50 }
}
```

**Structured summary format:** The default engine instructs the LLM to produce a markdown summary with sections: `## Summary`, `## Key Decisions`, `## Pending Tasks`, `## Context` — matching hermes-agent's structured summary format. Iterative re-compression (compressing a summary that's still too long) uses the same engine with a `[SUMMARY OF SUMMARY]` prefix.

**New crates required:** `tiktoken-rs = "0.11"`

**What NOT to do:** Do not use `compression-prompt` or similar external compression crates — they are statistical filters that reduce token count without semantic preservation, which breaks agent continuity. The correct approach is LLM-based summarization, which requires no new compression library. Do not use `llm-token-saver-rs` — same problem.

---

### 4. Prompt Caching (Anthropic `cache_control` Breakpoints)

**What it does:** Marks stable system prompt sections with `cache_control: {type: "ephemeral"}` to enable Anthropic's prompt caching (10% cost for cache reads vs full input token price). Up to 4 breakpoints per request.

**Approach:** No new crate needed. This is a JSON structure change to the existing `reqwest`-based API client in `ironhermes-agent`. The existing `serde_json::Value` API request builder needs to emit `cache_control` fields on specific content blocks.

**Cache placement strategy (from Anthropic docs):**

The Anthropic API orders content as: `tools → system → messages`. Cache breakpoints are placed on the last stable element in each stable section:

1. **Breakpoint 1:** Last tool definition (tools array is stable across turns)
2. **Breakpoint 2:** System prompt text block (SOUL.md + AGENTS.md content — stable per session)
3. **Breakpoints 3–4:** Reserved for large memory snapshots or skills index injections

**Wire format:**
```json
{
  "system": [
    {
      "type": "text",
      "text": "<SOUL.md content + memory snapshot>",
      "cache_control": { "type": "ephemeral" }
    }
  ],
  "tools": [
    { "name": "last_tool", "description": "...", "input_schema": {...},
      "cache_control": { "type": "ephemeral" } }
  ]
}
```

**Minimum cacheable length:** 1024 tokens (Claude Sonnet/Opus), 2048 tokens (Claude Haiku). The system prompt must exceed this threshold for caching to activate — the 10-layer prompt assembly ensures this for any non-trivial agent configuration.

**Important:** The `cache_control` field is Anthropic-specific. The existing `OpenAICompatibleClient` needs a `supports_cache_control()` method that returns `true` only when the configured `base_url` points to `api.anthropic.com`. Cache breakpoints must be stripped for OpenRouter/Nous endpoints that don't support them.

**New crates required:** None.

---

### 5. Context File Loading (Progressive Discovery + Security Scanning)

**What it does:** Discovers `.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules` in the working directory and parent directories (up to project root), loads with priority chain assembly, applies security scanning before injection, truncates at token budget.

**Approach:** Extends existing context file loading in `ironhermes-core`. Already uses `glob` (workspace) for file discovery. Priority chain assembly is already implemented for v1.0 (SOUL.md, AGENTS.md, project context).

**Progressive subdirectory discovery:** Walk from `$PWD` upward to `$HOME` or git root, collecting context files. Stop at git root (use `git rev-parse --show-toplevel` via `std::process::Command` — no new crate needed).

**Security scanning:** Reuse existing injection scanning infrastructure from v1.0 self-improvement feature. Apply the same `InjectionScanner` to loaded context files before injection into the system prompt.

**New crates required:** None — `glob`, `regex`, `fs2`, and `std::fs` cover all needs.

---

### 6. SOUL.md Personality System

**What it does:** Loads durable agent identity from `$HERMES_HOME/SOUL.md`, with a compiled-in default fallback, and supports `/personality` session overlays that temporarily override personality for the duration of a session.

**Approach:** `ironhermes-core` gains a `PersonalityLoader` struct. Session overlays are stored in `SessionState` (in-memory, not persisted). The `/personality` slash command sets an overlay that prepends to or replaces the SOUL.md block in the system prompt for that session.

**New crates required:** None.

---

### 7. Skill Framework (SKILL.md Format, Discovery, Conditional Activation)

**What it does:** Discovers skills from `~/.ironhermes/skills/`, `.hermes/skills/`, and a Skills Hub URL. Each skill is a directory with `SKILL.md` (YAML frontmatter + markdown instructions). Progressive disclosure: name+description loaded at startup (~100 tokens/skill), full body loaded on activation (~500–5000 tokens).

**SKILL.md frontmatter spec** (agentskills.io open standard, published December 2025):

| Field | Required | Notes |
|-------|----------|-------|
| `name` | Yes | 1–64 chars, lowercase+hyphens, matches directory name |
| `description` | Yes | 1–1024 chars, what + when to use |
| `license` | No | License name or bundled file reference |
| `compatibility` | No | 1–500 chars, environment requirements |
| `metadata` | No | Arbitrary key-value map |
| `allowed-tools` | No | Space-separated pre-approved tools (experimental) |

**Hermes-agent extensions** (not in open standard — iron-hermes-specific additions in frontmatter `metadata`):
- `metadata.category`: skill category for grouping (tools/research/coding/etc.)
- `metadata.env_vars`: required env var names for conditional activation check
- `metadata.required_tools`: tool names that must be registered for activation

**Conditional activation:** A skill is `available` if all `env_vars` are set and all `required_tools` exist in the registry. Unavailable skills are shown in `list_skills` with an indication of what's missing, but not injected into the system prompt.

**Approach:** Extends the existing skills system in `ironhermes-core` (already has `SkillsManager` and progressive disclosure from v1.1). The SKILL.md parser needs `serde_yaml` (already in workspace) for frontmatter parsing.

**New crates required:** None — `serde_yaml`, `glob`, and `std::fs` cover all needs.

---

### 8. Slash Commands

**What it does:** Parses `/command [args]` from user input, dispatches to a `SlashCommand` trait implementation. Commands include `/personality`, `/model`, `/skills`, `/memory`, `/session`, `/help`.

**Approach:** A `SlashCommandRegistry` (mirroring `ToolRegistry` pattern) in `ironhermes-core` or `ironhermes-agent`. Parsing is a simple prefix check on input before the agent loop processes it — no parser combinator library needed.

**New crates required:** None.

---

### 9. Tool Registry Improvements

**What it does:** Toolset management (named subsets of the full registry), `check` functions on tools (return `ToolAvailability::Available/Unavailable(reason)` for setup wizard), and setup wizard integration.

**Approach:** Extends `ToolRegistry` in `ironhermes-tools`. Check functions are sync closures stored alongside tool definitions. The setup wizard queries all check functions and reports missing dependencies.

**New crates required:** None.

---

### 10. CLI Feature Parity (execute_code, hooks, guardrails in CLI mode)

**What it does:** Makes `execute_code`, `hooks`, and `guardrails` available when running in CLI mode (currently gateway-only per PROJECT.md key decision ⚠️).

**Approach:** The feature isolation was a deliberate v1.1 decision to ship faster. For v2.0, the CLI `AgentLoop` construction needs to pass the same `HookChain` and `ToolRegistry` as the gateway. No architectural change — just wire up existing components in `ironhermes-cli`.

**New crates required:** None.

---

## Summary Table: New Dependencies

| Crate | Version | Feature Flags | Purpose | Adds To |
|-------|---------|---------------|---------|---------|
| `rusqlite` | `"0.39"` | `["bundled-full"]` | **UPGRADE** — adds FTS5 (vtab) + all modern features | `[workspace.dependencies]` |
| `rusqlite_migration` | `"2.5"` | (none) | Schema migrations with versioning | `[workspace.dependencies]`, `ironhermes-state` |
| `tiktoken-rs` | `"0.11"` | (none) | Token counting for compression thresholds | `[workspace.dependencies]`, `ironhermes-agent` |
| `grafeo` | `"0.5"` | `["embedded"]` | Graph DB memory provider (optional) | `ironhermes-state` behind `memory-grafeo` feature |
| `duckdb` | `"1.10501"` | `["bundled"]` | Columnar analytics memory provider (optional) | `ironhermes-state` behind `memory-duckdb` feature |

**Everything else uses crates already in the workspace.**

---

## Crates Explicitly Ruled Out

| Crate | Reason | Use Instead |
|-------|--------|-------------|
| `sqlx` | Async-first, macro-heavy, connection pool model conflicts with rusqlite single-connection | rusqlite 0.39 + rusqlite_migration |
| `compression-prompt` / `llm-token-saver-rs` | Statistical token reduction — destroys semantic continuity for agent memory | LLM-based summarization via existing API client |
| `tiktoken` (anysphere fork) | Pure-Rust port, less maintained than zurawiki/tiktoken-rs | `tiktoken-rs = "0.11"` |
| `sea-orm` / `diesel` | Full ORM — massively over-engineered for this schema | rusqlite direct + rusqlite_migration |
| `surrealdb` | Multi-model DB — unnecessary complexity, not embeddable as single-file DB | grafeo (if graph needed) |
| `pyo3` | Python embedding — breaks single-binary constraint (already ruled out in v1.1) | — |
| `rayon` | CPU parallelism — wrong for async I/O-bound LLM work | tokio JoinSet (already used) |

---

## Feature Flag Architecture for Optional Providers

```toml
# In crates/ironhermes-state/Cargo.toml
[features]
default = []
memory-grafeo = ["dep:grafeo"]
memory-duckdb = ["dep:duckdb"]

[dependencies]
grafeo = { version = "0.5", features = ["embedded"], optional = true }
duckdb = { version = "1.10501", features = ["bundled"], optional = true }
```

This keeps the default build (single binary, fast compile) free of the heavy optional backends. Users opt in by building with `--features memory-grafeo` or `--features memory-duckdb`.

---

## Integration Map

```
ironhermes-core/
  memory_provider.rs     — MemoryProvider trait (async-trait, already in workspace)
  personality.rs         — PersonalityLoader, SOUL.md loading
  slash_commands.rs      — SlashCommandRegistry + SlashCommand trait
  context_files.rs       — Progressive discovery, security scanning (extends v1.0)
  skills/                — SKILL.md parser extensions (conditional activation, env_var check)

ironhermes-state/
  session_db.rs          — Sessions + messages tables + FTS5 + WAL + write contention
  migrations.rs          — rusqlite_migration sequential versioning (schema v7+)
  providers/
    sqlite_provider.rs   — Built-in MemoryProvider (always active)
    grafeo_provider.rs   — (feature: memory-grafeo)
    duckdb_provider.rs   — (feature: memory-duckdb)

ironhermes-agent/
  context_engine.rs      — ContextEngine trait + default LLM-based compression
  prompt_builder.rs      — 10-layer system prompt assembly with cache_control injection
  token_counter.rs       — tiktoken-rs wrapper for threshold detection

ironhermes-tools/
  session_search.rs      — session_search tool (queries messages_fts)
  memory_tool.rs         — memory add/replace/remove (extends existing)
  tool_registry.rs       — check() functions, ToolAvailability enum

ironhermes-cli/
  (wire HookChain + full ToolRegistry — same as gateway, for CLI parity)
```

---

## Version Compatibility

| Package | Compatible With | Notes |
|---------|-----------------|-------|
| `rusqlite 0.39` | `rusqlite_migration 2.5` | rusqlite_migration 2.x targets rusqlite 0.3x series |
| `duckdb 1.10501` | DuckDB v1.5.1 | Version encoding: 1.MAJOR_MINOR_PATCH.x |
| `tiktoken-rs 0.11` | `cl100k_base`, `o200k_base` encoders | Compatible with Claude/GPT-4 token counts |
| `grafeo 0.5` | tokio 1.x | Grafeo is async-compatible; use `spawn_blocking` for sync operations |

---

## Build Time Impact

| Addition | Build Time Delta | Notes |
|----------|-----------------|-------|
| `rusqlite 0.39` (upgrade) | ~0 | Same bundled compile, just newer SQLite |
| `rusqlite_migration 2.5` | ~2s | Small pure-Rust crate |
| `tiktoken-rs 0.11` | ~10s | Pulls in tokenizer data files |
| `grafeo 0.5` (opt-in) | ~45s | Rust graph DB — significant but opt-in |
| `duckdb 1.10501` (opt-in) | ~3min | Compiles DuckDB C++ source — always use feature flag |

Default build (no optional providers): adds ~12s to compile time.

---

## Installation

```toml
# [workspace.dependencies] in root Cargo.toml — changes and additions
rusqlite = { version = "0.39", features = ["bundled-full"] }   # upgrade from 0.32
rusqlite_migration = "2.5"                                      # new
tiktoken-rs = "0.11"                                            # new

# crates/ironhermes-state/Cargo.toml
[dependencies]
rusqlite_migration = { workspace = true }
grafeo = { version = "0.5", features = ["embedded"], optional = true }
duckdb = { version = "1.10501", features = ["bundled"], optional = true }

[features]
default = []
memory-grafeo = ["dep:grafeo"]
memory-duckdb = ["dep:duckdb"]

# crates/ironhermes-agent/Cargo.toml
[dependencies]
tiktoken-rs = { workspace = true }
```

---

## Confidence Assessment

| Area | Confidence | Notes |
|------|------------|-------|
| rusqlite 0.39 upgrade | HIGH | Version verified live at docs.rs; bundled-full feature list confirmed |
| rusqlite_migration 2.5 | HIGH | Version verified live at crates.io; standard rusqlite migration pattern |
| tiktoken-rs 0.11 | HIGH | Version verified live at crates.io |
| grafeo 0.5.35 | MEDIUM | Version verified live; Grafeo is newer (HN post 2025), production maturity less established |
| duckdb 1.10501 | HIGH | Official duckdb-rs maintained by DuckDB team; bundled feature confirmed |
| Anthropic cache_control | HIGH | Verified against official Anthropic docs; wire format confirmed |
| SKILL.md spec | HIGH | Verified against agentskills.io/specification.md directly |
| FTS5 schema | HIGH | Verified against hermes-agent session storage docs directly |
| No new crates for compression/slash/context | HIGH | Logic is pure Rust using existing workspace crates |

---

## Sources

- crates.io live API: rusqlite 0.39.0, rusqlite_migration 2.5.0, tiktoken-rs 0.11.0, grafeo 0.5.35, duckdb 1.10501.0 — versions verified 2026-04-11
- docs.rs rusqlite features: https://docs.rs/crate/rusqlite/latest/features — bundled-full feature list
- hermes-agent session storage docs: https://hermes-agent.nousresearch.com/docs/developer-guide/session-storage — exact SQLite schema
- hermes-agent memory provider plugin docs: https://hermes-agent.nousresearch.com/docs/developer-guide/memory-provider-plugin/ — MemoryProvider interface
- Anthropic prompt caching docs: https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching — cache_control wire format
- agentskills.io specification: https://agentskills.io/specification.md — SKILL.md frontmatter fields
- duckdb-rs GitHub: https://github.com/duckdb/duckdb-rs — version encoding scheme, bundled feature
- grafeo.dev: https://grafeo.dev/ — embedded profile features

---

*Stack research for: IronHermes v2.0 Intelligence & Identity*
*Researched: 2026-04-11*
