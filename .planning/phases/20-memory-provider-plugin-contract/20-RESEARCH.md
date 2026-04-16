# Phase 20: Memory Provider Plugin Contract — Research

**Researched:** 2026-04-16
**Domain:** Rust trait evolution, async lifecycle hooks, `Arc<Mutex<dyn …>>` sharing, Tokio fire-and-forget, CLI wizard authoring
**Confidence:** HIGH (codebase grounded) — VERIFIED via direct inspection of every file cited in CONTEXT.md canonical refs + every provider crate + CLI + gateway.

## Summary

Phase 20 is a Rust trait-shape change with an unusually low blast radius: `MemoryProvider::initialize` has **no live call sites in the workspace** (factory constructs providers and hands them off as `Arc<Mutex<dyn …>>` without ever calling `initialize`). So D-10 "breaking change" is strictly a trait-definition and four-impl edit — not a gateway/agent/CLI wiring change. `MemoryProviderConfig` deletion is likewise safe: its only consumers are the four `initialize` stubs themselves.

The real integration work is narrower and concentrated in `crates/ironhermes-agent/`: a new `memory/manager.rs` module (D-25..D-28), a prompt-builder append for `system_prompt_block` (D-11), a post-turn `queue_prefetch` fire-and-forget in `agent_loop.rs` (D-12), an additional async listener on `context:pre_compress` for `on_pre_compress` (D-13 — the `memory_flush_handler.rs` file already pioneers the exact pattern), a factory `load_from_disk` fix (D-16 — absorbs pending todo Fix 1), and chat-mode memory wiring (Fix 2, absorbed into Plan 20-03).

**One surprise worth calling out:** the research focus said the agent loop hard-codes `memory_add`/`memory_replace`/`memory_remove` name-match. That is **incorrect**. The memory tool registered with the registry is a single tool named `"memory"` (with `action: "add"|"replace"|"remove"` as an argument) — see `memory_tool.rs:113`. The agent loop's only hard-coded intercept is `session_search` (`agent_loop.rs:769`). So D-18 ("switch intercept from name-list to `get_tool_schemas()` membership") is simpler than CONTEXT.md implied — the dispatch already flows through the registry by virtue of the memory tool being in it. The new shape is: `MemoryTool::execute` delegates to `MemoryManager::handle_tool_call` → `primary.handle_tool_call` → (on success) `mirror.on_memory_write`. No agent-loop intercept set is required.

**Primary recommendation:** Land Plan 20-01 as a single atomic commit (trait + `MemoryProviderConfig` deletion + all four provider impls + factory fix). The tight coupling makes partial landings impossible. Plans 20-02..20-04 are additive and can be incremental.

## User Constraints (from CONTEXT.md)

### Locked Decisions (D-01 … D-29, verbatim)

**Trait enrichment**
- **D-01:** Keep `MemoryProvider` in `ironhermes-core/src/memory_provider.rs`. Add new methods with default implementations (exception: `initialize` is a breaking signature change).
- **D-02:** Required (no default): `name() -> &'static str`.
- **D-03:** `is_available(&self) -> bool` — default `true`. Evaluated by factory after `load_from_disk`; if false, factory falls back to file-based provider + `tracing::warn!` with reason via `unavailable_reason() -> Option<String>` (default `None`). **No network calls in `is_available`.**
- **D-04:** `get_tool_schemas(&self) -> Vec<ToolSchema>` — default returns the current three-action tool set. Providers surface extras by override. Reuses `ironhermes_core::ToolSchema`.
- **D-05:** `handle_tool_call(&mut self, name: &str, args: Value) -> MemoryResult` — default routes to `add`/`replace`/`remove` by matching on name.

**Config schema**
- **D-06:** `get_config_schema(&self) -> Vec<ConfigField>` — default `vec![]`. New type in `ironhermes-core/src/config_schema.rs` with fields: `key, description, secret, required, default, choices, env_var, url`.
- **D-07:** `save_config(&self, values: &HashMap<String, Value>, hermes_home: &Path) -> anyhow::Result<()>` — default no-op. Non-secret → `$HERMES_HOME/<provider_name>.json`. Secrets → `.env` (handled by wizard caller, not provider).
- **D-08:** `hermes memory setup` is a **minimal** CLI subcommand. Enumerates compiled-in providers, asks which to activate, reads `get_config_schema()`, prompts for `required=true` + no-default fields, appends secrets to `.env`, calls `save_config()` for the rest. `hermes memory list` / `hermes <provider> test` deferred.
- **D-09:** `MemoryConfig` stays the selector (`provider: "sqlite"`). Provider-specific config lives beside it, loaded lazily via `initialize(session_id, hermes_home, provider_config)`.

**Lifecycle hooks**
- **D-10:** **BREAKING.** `async fn initialize(&mut self, session_id: &str, hermes_home: &Path, provider_config: &Value) -> anyhow::Result<()>`. **`MemoryProviderConfig` deleted.** No compat shim. All three provider crates + file `MemoryStore` migrate in Plan 20-01.
- **D-11:** `system_prompt_block(&self) -> Option<String>` — default `None`. Additive. `PromptBuilder::load_memory` appends it to slot 3 after target-scoped blocks.
- **D-12:** `queue_prefetch(&self, query: &str) -> anyhow::Result<()>` — default no-op. Called by `AgentLoop` after each completed turn. Fire-and-forget `tokio::spawn`.
- **D-13:** `on_pre_compress(&self, messages: &[ChatMessage]) -> anyhow::Result<()>` — default no-op. Fires from context-engine pre-compress hook.
- **D-14:** `on_memory_write(&mut self, action: MemoryAction, target: MemoryTarget, content: &str) -> anyhow::Result<()>` — default no-op. **Fires from `MemoryManager`, not any provider.** Single-subscriber only per MEM-12.
- **D-15:** `shutdown` and `on_session_end` stay as-is.

**MemoryManager**
- **D-25:** New `crates/ironhermes-agent/src/memory/manager.rs`. Wraps primary + optional mirror. Writes flow `primary.handle_tool_call(...)` → on success → `mirror.on_memory_write(...)` (error-logged, NOT propagated).
- **D-26:** Manager owns `queue_prefetch` (post-turn) and `on_pre_compress` (context-engine) fire sites. Reads (`prefetch`, `format_for_system_prompt`, `system_prompt_block`) → primary only.
- **D-27:** Mirror selection via optional `memory.mirror_provider` config field. Factory constructs both and passes to `MemoryManager::new(primary, mirror_opt)`.
- **D-28:** MEM-12 preserved — mirror is write-only observational.

**Factory + wiring**
- **D-16:** Factory keeps `cfg!(feature = …)` match; after `Provider::new(...)` unconditionally calls `provider.load_from_disk()?`. Stale comment at `factory.rs:9-10` rewritten. (Absorbs pending todo Fix 1.)
- **D-17:** `is_available` evaluated after `load_from_disk`. If false → fall back to file provider + `tracing::warn!(unavailable_reason())`.
- **D-18:** Agent-loop change — intercept set built from `memory_manager.get_tool_schemas()` (see Pitfall 1 below — actual code shape differs from CONTEXT.md description).
- **D-19:** `session_search` stays its own intercept — owned by StateStore.

**Migration**
- **D-20:** Every provider crate migrates in this phase (due to D-10). Other hooks are defaulted → providers opt in as needed in Plan 20-04.
- **D-21:** (Removed — no compat shim.)

**Testing**
- **D-22:** Trait-level tests via `MockMemoryProvider` in `ironhermes-core/tests/`. Agent-loop tests assert hook-ordering: `initialize → prefetch → sync_turn → queue_prefetch → on_pre_compress → on_memory_write → on_session_end → shutdown`.
- **D-23:** `hermes memory setup` scripted-stdin integration test with fake provider (one secret, one required-with-default, one optional).
- **D-24:** Factory `load_from_disk` regression test for sqlite (and duckdb/grafeo).
- **D-29:** `MemoryManager` mirror test — primary=file + mirror=MockMirror; add/replace/remove; assert mirror saw each write; assert failing mirror does not block primary.

### Claude's Discretion (verbatim)

- Exact additional fields on `ConfigField` beyond the core set — add only when a real provider needs them.
- Whether `get_tool_schemas` returns owned `Vec<ToolSchema>` or borrows from `&'static` — pick based on ergonomics during implementation.
- File layout for the setup wizard (single `setup_memory_wizard.rs` vs submodule).
- Exact log message wording for `is_available = false` fallback.
- Whether `MemoryManager` is shared as `Arc<Mutex<MemoryManager>>` or held directly in AgentLoop — decide based on gateway-sharing needs.

### Deferred Ideas (OUT OF SCOPE)

- **Runtime plugin discovery** (`plugin.yaml`, `libloading`, WASM/Wasmtime). Blocked by `PROJECT.md:52`.
- **`hermes memory list` / `hermes <provider> test` CLI subcommands.**
- **Broadcast `on_memory_write`** to multiple subscribers.
- **Multi-provider peer operation** (file + supermemory both read/write).
- **Async variants of `add`/`replace`/`remove`.**
- **Web UI for setup.**

## Phase Requirements

| ID | Description (REQUIREMENTS.md) | Research Support |
|----|-------------------------------|------------------|
| MEM-07 | MemoryProvider trait defines lifecycle hooks (initialize, prefetch, sync_turn, on_session_end, shutdown) with Send + Sync + 'static bounds | Trait already exists at `crates/ironhermes-core/src/memory_provider.rs:40-61`; Phase 20 extends it with 11 new methods (10 defaulted, 1 required: `name()`) and reshapes `initialize` signature. |
| MEM-08 | Built-in file-based MemoryStore implements MemoryProvider as the default backend | `impl MemoryProvider for MemoryStore` at `memory_provider.rs:67-127` stays; receives the new defaulted methods + new `initialize` signature migration in Plan 20-01. |
| MEM-10 | Grafeo graph database memory provider | `providers/memory-grafeo/src/lib.rs:140` impl migrates in Plan 20-01 + adopts schema hooks in Plan 20-04. |
| MEM-11 | DuckDB memory provider | `providers/memory-duckdb/src/lib.rs:114` impl migrates in Plan 20-01 + adopts schema hooks in Plan 20-04. |
| MEM-12 | Only one external memory provider can be active at a time — single-provider selection via config | Preserved. D-14/D-28 add a write-only mirror (observational) but the primary read path is still single. Factory selection remains 1-to-1. |

## Project Constraints (from CLAUDE.md)

**No `./CLAUDE.md` file exists at the project root.** No project-wide AGENTS.md either. The canonical constraints are therefore:

- `.planning/PROJECT.md:52` — "Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity." **Governs the entire shape of this phase.**
- `.planning/PROJECT.md:65` — "Rust 2024 edition — committed, no mixed Python/Rust." (Workspace confirms: `edition = "2024"` in `Cargo.toml:21`.)
- `.planning/PROJECT.md:70` — "Config: YAML + .env at `~/.ironhermes/` — established pattern, don't change." **The setup wizard in Plan 20-03 must follow this: secrets to `.env`, non-secrets to `$HERMES_HOME/<provider>.json`.**
- `.planning/PROJECT.md:103` — "Port faithfully, deviate only with documented rationale." (Applied: `PROJECT.md:52` is the documented deviation from Python's plugin loading.)

## Standard Stack (workspace-verified)

All crates already in the workspace — Phase 20 adds **no new dependencies**.

| Library | Version | Already In Workspace | Phase 20 Use |
|---------|---------|----------------------|--------------|
| `tokio` | 1 (full) | ✓ `Cargo.toml:28` | `tokio::spawn` for `queue_prefetch`; `tokio::sync::Mutex` for MemoryManager |
| `async-trait` | 0.1 | ✓ `Cargo.toml:51` | Already used by `MemoryProvider` trait — keeps working |
| `serde` / `serde_json` | 1 | ✓ `Cargo.toml:34-35` | `ConfigField` serde derives; `&Value` for `initialize` provider_config |
| `clap` | 4 (derive) | ✓ `Cargo.toml:40` | `hermes memory setup` subcommand — extend existing `Commands` enum in `crates/ironhermes-cli/src/main.rs:54` |
| `rustyline` | 15 | ✓ `Cargo.toml:67` | Already used for chat-mode readline — **reuse for setup wizard interactive prompts** (no new `dialoguer`/`inquire` dep needed) |
| `dotenvy` | 0.15 | ✓ `Cargo.toml:48` | Existing env loader — setup wizard appends to `.env` (file-level write, not via dotenvy API) |
| `dirs` | 6 | ✓ `Cargo.toml:49` | Already used by `get_hermes_home()` at `constants.rs:30` |
| `tracing` | 0.1 | ✓ `Cargo.toml:42` | `warn!` for mirror failures, fallback messages |
| `tempfile` | 3 | ✓ `Cargo.toml:85` | Setup wizard integration test + factory regression test |
| `anyhow` | 1 | ✓ `Cargo.toml:45` | All new method signatures return `anyhow::Result<()>` |

**Installation:** none — no `cargo add` required for Phase 20.

**Version verification:** all versions read directly from `Cargo.toml`; workspace is pinned and all dependencies resolved. [VERIFIED: workspace Cargo.toml]

### Alternatives Considered

| Instead of | Could Use | Tradeoff / Why Rejected |
|------------|-----------|-------------------------|
| `rustyline` for wizard prompts | `dialoguer` 0.11 | New dep, larger surface; `rustyline` is already used and `std::io::stdin().read_line()` works for simple secret prompts. Reject — no new dep. |
| `rustyline` for wizard prompts | `inquire` 0.7 | Richer UX (select lists, validation) but new dep + bigger blast radius. Reject for "minimal wizard" (D-08). |
| `serde_json::Value` for `initialize(provider_config: &Value)` | Typed generic `P: DeserializeOwned` | Complicates trait object usage — `dyn MemoryProvider` requires concrete types. `&Value` matches hermes-agent's `**kwargs` shape 1:1. [CITED: CONTEXT.md D-10] |
| New `ConfigField` type | Reuse `SkillConfigField` from `crates/ironhermes-core/src/skills.rs:85` | Different field set (skills don't have `secret`, `env_var`, `url`). Defining a separate type aligned to hermes-agent plugin contract is cleaner; copy the serde pattern though. |

## Architecture Patterns

### Recommended Module Structure (brownfield — minimal new surface)

```
crates/ironhermes-core/src/
├── memory_provider.rs      (edit: new trait methods, new initialize sig, delete MemoryProviderConfig)
├── config_schema.rs        (NEW: ConfigField, MemoryAction)
├── lib.rs                  (edit: pub use config_schema::*)
└── config.rs               (edit: add optional mirror_provider to MemoryConfig)

crates/ironhermes-agent/src/
├── memory/
│   ├── mod.rs              (edit: pub mod manager;)
│   ├── factory.rs          (edit: load_from_disk + is_available fallback)
│   └── manager.rs          (NEW: MemoryManager, primary + optional mirror)
├── agent_loop.rs           (edit: queue_prefetch fire-and-forget after each turn)
├── prompt_builder.rs       (edit: load_memory appends system_prompt_block)
├── memory_flush_handler.rs (edit: also call on_pre_compress on the manager)
└── lib.rs                  (edit: pub use memory::manager::MemoryManager;)

providers/memory-sqlite/src/lib.rs     (edit: initialize sig; opt-in to hooks in 20-04)
providers/memory-duckdb/src/lib.rs     (edit: initialize sig; opt-in to hooks in 20-04)
providers/memory-grafeo/src/lib.rs     (edit: initialize sig; opt-in to hooks in 20-04)

crates/ironhermes-cli/src/
├── main.rs                 (edit: Commands enum, run_chat/run_single wiring — Fix 2)
└── memory_setup.rs         (NEW: setup wizard — or inline module in main.rs, discretion D-08)
```

### Pattern 1: Fire-and-forget post-turn hook (D-12 `queue_prefetch`)

**Established in codebase:** `crates/ironhermes-agent/src/client.rs:150` and `crates/ironhermes-agent/src/anthropic_client.rs:616` both use `tokio::spawn(async move { … })` for fire-and-forget streaming work.

**Shape for `queue_prefetch`:**
```rust
// Source: pattern extracted from client.rs:150 + D-12 semantics
// In agent_loop.rs after a completed turn:
if let Some(ref mgr) = self.memory_manager {
    let mgr = mgr.clone();                // Arc<Mutex<MemoryManager>>
    let query = user_message_text.clone();
    tokio::spawn(async move {
        let guard = mgr.lock().await;
        if let Err(e) = guard.queue_prefetch(&query).await {
            tracing::warn!(error = ?e, "queue_prefetch failed");
        }
    });
}
// agent does not await
```

**Critical constraint:** provider must live inside `Arc<…>` to survive the move into the task. `Arc<Mutex<dyn MemoryProvider + Send>>` (the existing sharing pattern, see `memory_tool.rs:10`) satisfies this.

### Pattern 2: Async hook listener fired by context engine (D-13 `on_pre_compress`)

**Prior art:** `crates/ironhermes-agent/src/memory_flush_handler.rs:16-37` — `build_memory_flush_listener` already listens for `HookEventKind::ContextPreCompress` and invokes `provider.sync_turn(...)`. Phase 20 adds a second listener that invokes `manager.on_pre_compress(messages)`.

**Limitation:** `HookEventKind::ContextPreCompress { session_id, estimated_tokens, threshold, mode, pruned_range }` does NOT carry the `messages: &[ChatMessage]` slice. To fire `on_pre_compress(messages)` the engine must either (a) extend the event kind to carry the slice, or (b) call the manager directly from `LocalPruningEngine::compress` / `SummarizingEngine::compress` rather than via the hook. **Option (b) is cleaner** — the engines own the `&mut messages` at the emit site (`context_engine.rs:137-150`). Wire it as: `engine.set_memory_manager(Arc<Mutex<MemoryManager>>)`; `compress` calls `mgr.on_pre_compress(messages)` synchronously inside the async fn before the destructive loop starts.

### Pattern 3: `Arc<Mutex<dyn MemoryProvider + Send>>` sharing (unchanged)

**Established:** `memory_tool.rs:10`, `factory.rs:13`, `registry.rs:225`. `MemoryManager` adopts the same shape so gateway/CLI can clone-and-share exactly as they do today.

```rust
// Proposed type aliases for manager.rs
pub type SharedProvider = Arc<Mutex<dyn MemoryProvider + Send>>;

pub struct MemoryManager {
    primary: SharedProvider,
    mirror: Option<SharedProvider>,
}
```

**Mutex choice:** `std::sync::Mutex` for parity with existing code (`memory_tool.rs:212 store.lock().unwrap()`). Not `tokio::sync::Mutex` — memory operations are sync (`add`/`replace`/`remove` are `fn`, not `async fn`), so a sync Mutex is correct. The only async hook (`queue_prefetch`, `on_pre_compress`, etc.) can still take a sync Mutex guard briefly.

**Caveat:** `memory_flush_handler.rs:17` uses `tokio::sync::Mutex<dyn MemoryProvider + Send>` for the listener path (because the listener is async and holds the guard across await points via `to_memory_entries` + `sync_turn`). There are now **two Mutex flavors in circulation** for the same trait object — the planner must decide whether MemoryManager takes a `std::sync::Mutex` (matching the tool path) or `tokio::sync::Mutex` (matching the listener path). **Recommendation:** MemoryManager uses `std::sync::Mutex` (hot path is tool dispatch, which is sync); the on-compress listener adapts by wrapping the sync lock in `tokio::task::block_in_place` or restructures to avoid holding across await. This is a real design decision the planner must surface.

### Pattern 4: Feature-gated crate selection (unchanged)

`factory.rs:22-61` — `#[cfg(feature = "memory-sqlite")]` / `#[cfg(not(feature = …))]` pairs stay exactly as is. Factory dispatch on `config.provider` string stays. Only the body of each arm changes (add `load_from_disk` + `is_available` check).

### Anti-Patterns to Avoid

- **Threading `&Value` config through many call sites.** `initialize` takes `&Value` once; provider stores what it needs as internal struct fields. Do NOT propagate `&Value` past `initialize`.
- **Holding the primary Mutex across the mirror call.** `MemoryManager::handle_tool_call` must drop the primary guard before calling `mirror.on_memory_write` — otherwise a slow mirror blocks primary reads.
- **Propagating mirror errors.** D-14 / D-25 / D-29 all say: log via `tracing::warn!`, return `Ok(())` to the caller. A broken mirror must never corrupt the primary write path.
- **Calling `initialize` from the factory without `.await`.** `initialize` is `async`; the factory signature would need to become async to call it. Current factory is sync; since `initialize` has zero live callers, there's no migration blocker — BUT if the planner chooses to wire `initialize` from the factory (matching the semantics hermes-agent has), the factory signature must flip to `async fn`. Plan 20-01 should make this explicit choice.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Interactive CLI prompts | Custom stdin loop with echo-off | `rustyline::DefaultEditor` (already in workspace) + `rpassword` 7 (NEW dep) for secrets | `rustyline` is already used by chat-mode (`main.rs:432`). Secrets need echo-off; if `rpassword` feels heavy, `termios` mode toggling is acceptable. **Reject if possible:** the minimal wizard can require user to paste in plaintext and immediately clear terminal. Planner decides. |
| `.env` append/parse | String splicing | `dotenvy` read path + manual `OpenOptions::append().open()` for writes; never parse .env line-by-line for updates — treat as append-only | `.env` is append-only in the wizard; updates to existing keys are out of scope (user edits by hand). |
| Config JSON schema validation | Hand-written JSON walker | `serde_json::Value::get` + explicit `.as_str()` / `.as_bool()` guards inside `initialize` | Providers know their own shape; schema-driven validation is over-engineered for the `ConfigField` set. |
| `is_available` probe for sqlite/duckdb/grafeo | Ping network | **File-path existence + feature flag** only | D-03: "No network calls in `is_available`." Cheap local check only. |
| Async mutex for memory | `parking_lot::Mutex` | `std::sync::Mutex` | Matches `memory_tool.rs:212` pattern. No reason to split. |
| Mirror broadcast | `tokio::sync::broadcast` | Single `Option<SharedProvider>` field | D-14/D-28 explicit: single subscriber only. Broadcast deferred. |

**Key insight:** Phase 20 is almost entirely trait plumbing + one new 100-line module (`manager.rs`) + one CLI subcommand. No library additions, no architectural inventions.

## Runtime State Inventory

Phase 20 is trait refactor + CLI wizard + manager layer. Not a rename or a data migration. No stored state gets renamed. Still, the inventory is required because a provider-config shape is being deleted.

| Category | Items Found | Action Required |
|----------|-------------|-----------------|
| Stored data | **None.** `MemoryProviderConfig` is constructed in-memory by callers today; it has never been serialized to disk. All provider data (SQLite rows, Grafeo graph files, DuckDB tables, MEMORY.md/USER.md) keeps its on-disk schema **unchanged** — Phase 20 changes how providers are *initialized*, not what they *store*. | None. |
| Live service config | **None.** No running services key off `MemoryProviderConfig`. Gateway reads `config.memory.provider` (a string) — same field stays. | None. |
| OS-registered state | **None.** No launchd/systemd/Task Scheduler units reference the memory layer. | None. |
| Secrets / env vars | **None renamed.** Setup wizard *writes new* env vars to `.env` based on `ConfigField.env_var` values that each provider declares (e.g. `SUPERMEMORY_API_KEY`). No existing env vars get renamed. | None for this phase. Future providers decide their own env var names. |
| Build artifacts | **None.** `cargo clean` is not required; `cargo build` recompiles normally after trait changes. Workspace members are listed at `Cargo.toml:2-16` — no new crate added. | None. |

**All five categories verified. No runtime state migration required.**

## Common Pitfalls

### Pitfall 1: CONTEXT.md description of the memory tool intercept is inaccurate
**What goes wrong:** CONTEXT.md says "agent_loop.rs hard-codes `memory_add`/`memory_replace`/`memory_remove` as intercepted tool names" (D-18 narrative). The codebase does NOT match. The memory tool is a single tool named `"memory"` registered in the registry; it dispatches by an `action` argument internally. The only name-matched intercept in `agent_loop.rs:769` is `session_search`.
**Why it happens:** The hermes-agent Python plugin contract exposes three distinct tool names; the Rust port collapsed them into one `memory` tool with an `action` enum. The intercept-by-name pattern described in D-18 applies to the hermes-agent shape, not the current Rust shape.
**How to avoid:** Plan 20-02 should NOT try to "replace a hard-coded name list." Instead: have `MemoryTool::execute` (currently at `memory_tool.rs:184-259`) delegate to `MemoryManager::handle_tool_call(action, args)` instead of directly calling `store.add/replace/remove`. The `handle_tool_call` default impl still dispatches by action. If a provider overrides `get_tool_schemas()` to expose additional tools (e.g. `memory_search`), those schemas get registered separately via a future registry extension — NOT in this phase.
**Warning signs:** Planner writes a task saying "enumerate `memory_manager.get_tool_schemas()` into an `agent_loop.rs` intercept HashSet" — that task has no code target. Reject and restructure around `MemoryTool::execute` delegation.

### Pitfall 2: `provider.initialize(...)` has zero live callers today
**What goes wrong:** The factory constructs each provider with `Provider::new(...)` (which does all setup synchronously) and never calls the async `initialize`. Assumption: D-10 "breaking signature change" ripples out to many call sites — **it doesn't.**
**Why it happens:** Phase 11 defined the trait anticipating async `initialize`, but the factory shortcut (doing work in `new`) made `initialize` vestigial. D-10 reshapes the signature AND gives it a purpose (reading `provider_config: &Value` from `$HERMES_HOME/<provider>.json`).
**How to avoid:** Plan 20-01 must decide: (a) call `initialize` from the factory (factory becomes `async fn`, all four call sites in `cli/main.rs:613`, `cli/main.rs:645`, and tests flip to `.await`), or (b) keep `initialize` vestigial and move its config-loading purpose into a separate factory step. **Recommendation: (a)** — matches hermes-agent semantics and gives providers a predictable async boundary. Document the choice explicitly in Plan 20-01.
**Warning signs:** Plan 20-01 edits only the trait + impls without updating the factory signature or call sites → `initialize` never fires → `provider_config` never reaches the provider → setup wizard has nothing to hand off.

### Pitfall 3: `MemoryProviderConfig` deletion affects test-only Mocks and Phase 18 code
**What goes wrong:** Grep for `MemoryProviderConfig` finds usages in `memory_flush_handler.rs:44,68` (test Mock) and every provider test file (mock initialize). Plan 20-01 must update these mocks too — missing them = test crate compile error.
**How to avoid:** Plan 20-01 task list must include an audit step: `rg 'MemoryProviderConfig' crates/ providers/` — every match is either a definition (delete), an import (delete), or a mock impl (edit to new signature).
**Warning signs:** `cargo test --all-features` fails with "cannot find type `MemoryProviderConfig`" — indicates an orphan reference.

### Pitfall 4: Two Mutex flavors (`std::sync::Mutex` vs `tokio::sync::Mutex`) on the same trait object
**What goes wrong:** `memory_tool.rs:10` uses `Arc<std::sync::Mutex<dyn MemoryProvider + Send>>`. `memory_flush_handler.rs:17` uses `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>`. MemoryManager must pick one. If it picks `std::sync`, the listener path needs refactor. If it picks `tokio::sync`, the hot tool path pays async cost.
**Why it happens:** The flush handler holds the guard across `.await` (`guard.sync_turn(...).await`), which `std::sync::Mutex` cannot safely do.
**How to avoid:** Plan 20-02 task: MemoryManager uses `std::sync::Mutex` (hot path priority). The async listener is rewritten to acquire, clone the entries, release, then await — exactly as `memory_flush_handler.rs:25-27` should be restructured. Document this in the PLAN's design notes.
**Warning signs:** Compile error "cannot hold `std::sync::MutexGuard` across an `await` point" during Plan 20-02 build — signals a missed drop.

### Pitfall 5: File-based provider is stateful on its memory_dir — `initialize` needs to respect existing shape
**What goes wrong:** `MemoryStore::new(memory_dir: PathBuf)` (`memory_store.rs:67`) constructs with a directory path. New signature `initialize(session_id, hermes_home, &Value)` must not double-construct. Options: (a) `MemoryStore` moves its state setup into `initialize` (removing `new(memory_dir)`), or (b) `initialize` is a no-op for the file provider.
**How to avoid:** Pick (b) — keep `MemoryStore::new(...)` as construction; `initialize` remains no-op for file provider (as it is today at `memory_provider.rs:69-72`). The factory continues to call `new(hermes_home.join("memories"))` for the file path and passes `hermes_home` to `initialize` for provider metadata. This matches the existing pattern and minimizes churn.
**Warning signs:** Plan 20-01 writes "file MemoryStore initialize reads the memory_dir from provider_config" — that's scope creep; don't do it.

### Pitfall 6: `queue_prefetch` spawn lifetimes — the `Arc<Mutex<…>>` must outlive the spawned task
**What goes wrong:** `tokio::spawn` requires `'static` — a borrow of `&self` cannot move in. Must clone the `Arc` before spawning.
**How to avoid:** Pattern from `client.rs:150` — clone before `tokio::spawn`:
```rust
let mgr = Arc::clone(&self.memory_manager);
let query = query.to_string();
tokio::spawn(async move { /* use mgr, query */ });
```
**Warning signs:** Compile error "borrowed value does not live long enough" — missing clone before spawn.

### Pitfall 7: `is_available = false` → file-provider fallback can lose user data
**What goes wrong:** Config says `provider = "sqlite"`; sqlite init fails (`is_available` returns false); factory falls back to the file provider. User's memories live in the SQLite DB but the app now reads/writes the file provider — silent data divergence.
**How to avoid:** D-17 says `tracing::warn!` with `unavailable_reason()`. That's fine for visibility, but also: the fallback should be **refused** when a provider was explicitly configured — unless the user passed a flag. Clarify in Plan 20-01: `is_available` false → fall back to **empty file provider in a separate temp dir** (so data doesn't commingle) + return a clear error message suggesting `hermes memory setup` to reconfigure. **This is a design choice the planner must make explicit.**
**Warning signs:** Test case: configure sqlite without the `memory-sqlite` feature flag → `cargo run` succeeds silently → user stores memory → no-one notices it went to a file.

### Pitfall 8: `on_pre_compress` doesn't fit in `HookEventKind::ContextPreCompress` — the event doesn't carry messages
**What goes wrong:** `hooks::event.rs` defines `ContextPreCompress { session_id, estimated_tokens, threshold, mode, pruned_range }` — no `messages`. Listeners can't get the slice they need.
**How to avoid:** Call `manager.on_pre_compress(messages)` directly from inside `LocalPruningEngine::compress` / `SummarizingEngine::compress` at `context_engine.rs:136-150` — which owns `&mut messages`. The listener-via-hook pattern is unsuitable for this hook. Plan 20-02 adds `fn set_memory_manager(&mut self, mgr: Arc<Mutex<MemoryManager>>)` to both engines.
**Warning signs:** Plan 20-02 says "register a second async listener on ContextPreCompress that reads messages" — that path doesn't work; the event doesn't carry messages.

## Code Examples

### Existing: tokio fire-and-forget (for `queue_prefetch`)
```rust
// Source: crates/ironhermes-agent/src/client.rs:148-163
let (tx, rx) = mpsc::channel(256);

tokio::spawn(async move {
    let mut byte_stream = response.bytes_stream();
    let mut buffer = String::new();
    let chunk_timeout = Duration::from_secs(60);
    // ... work ...
});
```

### Existing: async hook listener (prior art for `on_pre_compress` if routed through hooks)
```rust
// Source: crates/ironhermes-agent/src/memory_flush_handler.rs:16-37
pub fn build_memory_flush_listener(
    provider: Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>,
) -> AsyncHookListener {
    Arc::new(move |event: HookEvent| {
        let provider = Arc::clone(&provider);
        Box::pin(async move {
            if let HookEventKind::ContextPreCompress { session_id, .. } = &event.kind {
                let sid = session_id.clone();
                let guard = provider.lock().await;
                let entries = guard.to_memory_entries();
                if let Err(e) = guard.sync_turn(&sid, &entries).await {
                    tracing::warn!(error = ?e, "memory flush failed");
                }
            }
        })
    })
}
```

### Existing: `ConfigField`-shaped type (prior art in skills.rs)
```rust
// Source: crates/ironhermes-core/src/skills.rs:85-95
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillConfigField {
    pub key: String,
    #[serde(default)]
    pub default: Option<serde_yaml::Value>,
    #[serde(default)]
    pub description: Option<String>,
    // ...
}
```
**New shape** (for `ironhermes-core/src/config_schema.rs`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub key: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub secret: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryAction { Add, Replace, Remove }
```

### Existing: sync memory tool shape (to become MemoryManager delegate)
```rust
// Source: crates/ironhermes-tools/src/memory_tool.rs:210-217 (today)
let result = {
    let mut store = self.store.lock().unwrap();
    store.add(target, content)
};
```
**New shape** (Plan 20-02):
```rust
// self.manager: Arc<std::sync::Mutex<MemoryManager>>
let result = {
    let mut mgr = self.manager.lock().unwrap();
    mgr.handle_tool_call(action, args)   // delegates to primary + fires mirror
};
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `MemoryProviderConfig` struct with `provider`, `memory_dir`, char limits, `extra` | `initialize(session_id, hermes_home, &Value)` — provider decides what to extract | This phase (D-10) | Removes dead code (no live callers); aligns with hermes-agent plugin contract |
| Single hard-coded `memory` tool in registry | Same shape, but **delegates** through `MemoryManager::handle_tool_call` | This phase (D-25, Plan 20-02) | Enables mirror write-observer without touching tool registry |
| `memory_flush_handler.rs` calls `provider.sync_turn` directly | Calls `MemoryManager::on_pre_compress(messages)` from context-engine (new) + `sync_turn` stays for the flush-to-disk purpose | This phase (D-13, Plan 20-02) | Two separate concerns: (1) flush before discard, (2) provider extracts insights before discard |
| Factory skips `load_from_disk` for sqlite/duckdb/grafeo | Factory calls `load_from_disk` for every provider | This phase (D-16, Plan 20-01) | Fixes pending todo: gateway/chat memory now persists across restarts |
| Chat/single mode has no memory wiring | Chat/single mode wire MemoryManager, register memory tool, set_memory_store on prompt builder | This phase (D-08 + Fix 2, Plan 20-03) | Feature parity between CLI modes and gateway |

**Deprecated/outdated:**
- `MemoryProviderConfig` (all of it). Four current impls pass `_config: &MemoryProviderConfig` as an unused argument — goes away.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Picking `std::sync::Mutex` for `MemoryManager` (over `tokio::sync::Mutex`) is correct because the tool path is sync and is the hot path. | Architecture Pattern 3 / Pitfall 4 | If async paths (`queue_prefetch`, `on_pre_compress`) become hot, contention may show. Planner should flag this as a reviewable choice in Plan 20-02's design notes. |
| A2 | The file-based `MemoryStore::new(memory_dir)` should stay; `initialize` is a no-op for file provider. | Pitfall 5 | If the planner decides otherwise, Plan 20-01 scope grows by one file. Low risk. |
| A3 | `on_pre_compress` should be called directly from `ContextEngine::compress`, NOT via a second hook listener. | Pattern 2 / Pitfall 8 | If planner picks the hook route, they need to extend `ContextPreCompress` to carry `Vec<ChatMessage>` — larger and wider-reaching change. |
| A4 | `rustyline` is sufficient for the setup wizard; no new dep needed for secret echo-off. | Don't Hand-Roll | If the user wants proper password UX, `rpassword` is a future add. For "minimal wizard" per D-08 this is fine. |
| A5 | `is_available = false` fallback behavior: log warn + fall back to file provider is acceptable (matches D-03/D-17). I noted a sharper alternative in Pitfall 7 — plan may want to refuse fallback instead. | Pitfall 7 | If planner picks the "refuse fallback" route, error message wording and user remediation flow are additional task work. |
| A6 | Factory becomes `async fn build_memory_provider(...)` to allow calling `provider.initialize(...).await`. | Pitfall 2 | If planner picks the sync-factory path, `initialize` stays vestigial and config-from-JSON loading needs another route. |

## Open Questions

1. **Should `hermes memory setup` also update `config.yaml`'s `memory.provider` field?**
   - What we know: D-08 says the wizard writes `.env` and provider JSON files. Silent about whether it sets `memory.provider`.
   - What's unclear: If the wizard runs but `config.yaml` still says `provider = "file"`, the user's choice has no effect next launch.
   - Recommendation: Plan 20-03 should include writing the selected provider back to `config.yaml` (or a provider-override file). Needs user decision in discuss-phase if ambiguous.

2. **Should `MemoryManager::on_pre_compress` call both primary AND mirror, per D-26 ("routing to both primary and mirror")?**
   - What we know: D-26 says yes; but `on_pre_compress` is an *observer* hook, and a mirror is configured as "observational write-only" (D-28). Calling `on_pre_compress` on the mirror means the mirror can also extract insights — does it then *write* them? To where?
   - What's unclear: Mirror's write destination on `on_pre_compress`. If the mirror writes back into primary via `on_memory_write`, we have a cycle risk.
   - Recommendation: Plan 20-02 clarifies — `on_pre_compress` fires on primary only; mirror only receives `on_memory_write` events. This is stricter than D-26's literal text but matches the "observational write-only shadow" semantics. Surface for review.

3. **Factory signature: sync or async?** (See Pitfall 2 / Assumption A6)
   - What we know: Today sync. `initialize` is async but unused.
   - What's unclear: Whether to keep sync (workaround: don't wire `initialize`) or go async (call sites in `cli/main.rs:613,645` flip to `.await` + `run_gateway` already async).
   - Recommendation: Go async — call `initialize` from factory immediately after `new(...)`, before `load_from_disk`. Covered in Plan 20-01.

4. **Does the file-based `MemoryStore` expose `get_config_schema`?**
   - What we know: D-06 defaults to `vec![]`; D-08 wizard skips providers with empty schemas for required-fields prompting.
   - What's unclear: Whether file provider has any configurable knob (memory char limits? dir?) that should surface.
   - Recommendation: Phase 20 ships file's `get_config_schema` returning a couple of optional fields (memory_dir, char limits) — but all optional-with-default, so wizard never prompts. Plan 20-04 task.

## Environment Availability

Phase 20 is Rust code/config only — runs entirely inside the existing Cargo workspace.

| Dependency | Required By | Available | Version | Fallback |
|------------|-------------|-----------|---------|----------|
| `cargo` (Rust 2024) | Everything | ✓ (workspace already building) | Toolchain pinned by `rust-toolchain.toml` (not checked — verify before implementation) | — |
| `tokio`, `clap`, `rustyline`, `serde`, `serde_json`, `anyhow`, `async-trait`, `tracing`, `tempfile`, `dirs`, `dotenvy` | Phase 20 deliverables | ✓ all in `Cargo.toml:26-89` | see Standard Stack table | — |
| `rusqlite` (for memory-sqlite tests) | D-24 regression test | ✓ `Cargo.toml:38` (bundled) | 0.32 | — |
| `grafeo`, `grafeo_common` | memory-grafeo | ✓ `providers/memory-grafeo/Cargo.toml` (not re-read; confirmed via `providers/memory-grafeo/src/lib.rs:15-16`) | — | — |
| `duckdb` | memory-duckdb | ✓ (crate already building, per Phase 17 completion) | — | — |
| `rpassword` (optional, for wizard secret echo-off) | D-08 setup wizard | ✗ not in workspace | — | Use plaintext prompt with clear-screen warning, or `read_line` into a secret buffer + `zeroize` manually. Recommended fallback: plaintext prompt — D-08 says "minimal". |

**Missing dependencies with no fallback:** none.

**Missing dependencies with fallback:**
- `rpassword` — not required; D-08 permits plaintext secret prompting. Planner may add in a follow-up.

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Cargo test runner (Rust built-in `#[test]` / `#[tokio::test]`) |
| Config file | `Cargo.toml` workspace — no per-crate test config |
| Quick run command | `cargo test -p ironhermes-core memory_provider --lib` |
| Full suite command | `cargo test --workspace --all-features` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MEM-07 | New trait methods (`name`, `is_available`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`, `on_pre_compress`, `on_memory_write`) compile + defaults behave correctly | unit | `cargo test -p ironhermes-core memory_provider::tests` | ❌ Wave 0 (new test module) |
| MEM-07 | `initialize(session_id, hermes_home, &Value)` new signature compiles and is invoked once by the factory | unit | `cargo test -p ironhermes-agent memory::factory::tests::factory_calls_initialize` | ❌ Wave 0 |
| MEM-08 | File `MemoryStore` implements all new trait methods (defaulted ones + name + is_available) | unit | `cargo test -p ironhermes-core memory_store::tests` | ✅ (extend existing) |
| MEM-10 | Grafeo provider passes migration (new `initialize` sig) + existing tests | unit | `cargo test -p memory-grafeo` | ✅ |
| MEM-11 | DuckDB provider passes migration + existing tests | unit | `cargo test -p memory-duckdb` | ✅ |
| MEM-12 | Single primary; mirror observational; mirror failure does not break primary (D-29) | unit | `cargo test -p ironhermes-agent memory::manager::tests::mirror_failure_does_not_block_primary` | ❌ Wave 0 |
| D-24 regression | Factory persistence round-trip for sqlite, duckdb, grafeo | integration | `cargo test -p ironhermes-agent --features memory-sqlite memory::factory::tests::sqlite_round_trip_via_factory`; `--features memory-duckdb ...`; `--features memory-grafeo ...` | ❌ Wave 0 |
| D-22 hook ordering | Agent loop fires hooks in order: initialize → prefetch → sync_turn → queue_prefetch → on_pre_compress → on_memory_write → on_session_end → shutdown | integration | `cargo test -p ironhermes-agent agent_loop::tests::hook_ordering` | ❌ Wave 0 (needs MockProvider recorder) |
| D-23 wizard | `hermes memory setup` scripted-stdin integration test with fake provider (one secret + one required-with-default + one optional) | integration | `cargo test -p ironhermes-cli memory_setup::tests::scripted_wizard_round_trip` | ❌ Wave 0 |
| D-29 mirror | Primary=file + mirror=MockMirror; add/replace/remove; mirror sees each write | unit | `cargo test -p ironhermes-agent memory::manager::tests::mirror_observes_writes` | ❌ Wave 0 |
| Plan 20-03 chat wiring | `run_chat` + `run_single` wire MemoryManager; `prompt_builder.set_memory_store` is called; `register_memory_tool` is called; persistence survives restart | integration | `cargo test -p ironhermes-cli run_chat::tests::memory_persists_across_invocations` | ❌ Wave 0 |

### Sampling Rate

- **Per task commit:** `cargo check --workspace --all-features` (fast, type checking); `cargo test -p <edited crate>` for touched crate
- **Per wave merge:** `cargo test --workspace --all-features` + `cargo clippy --workspace --all-features -- -D warnings`
- **Phase gate:** Full suite green + manual UAT: gateway + chat-mode both persist memory across restart.

### Wave 0 Gaps

- [ ] `crates/ironhermes-core/src/config_schema.rs` — covers ConfigField serialization, MemoryAction variants
- [ ] `crates/ironhermes-core/tests/memory_provider_contract.rs` — trait-level mock tests with invocation recorder (D-22)
- [ ] `crates/ironhermes-agent/src/memory/manager.rs` — MemoryManager + tests (D-25, D-29)
- [ ] `crates/ironhermes-agent/src/memory/factory.rs` — add load_from_disk + is_available tests (D-16, D-24) — **extend** existing test module, not new file
- [ ] `crates/ironhermes-agent/src/agent_loop.rs` — hook ordering test with MockProvider recorder (D-22) — **extend** existing tests
- [ ] `crates/ironhermes-cli/src/memory_setup.rs` — new module with wizard + scripted-stdin integration test (D-23)
- [ ] `crates/ironhermes-cli/src/main.rs` — extend `run_chat`/`run_single` + regression test for Fix 2

**Framework install:** none — Cargo + tokio-test are already available.

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|------------------|
| V2 Authentication | no | N/A — local CLI, no auth boundary in this phase |
| V3 Session Management | no | N/A |
| V4 Access Control | partial | File permissions on `$HERMES_HOME/<provider>.json` (0600) and `.env` (0600) — verify with `std::fs::set_permissions` on Unix in the wizard. `set_permissions` with `PermissionsExt::from_mode(0o600)` after write. |
| V5 Input Validation | yes | `ConfigField.key` — validate against an allow-set of identifier chars (`[a-zA-Z0-9_]+`) before it becomes a filename component or env-var key. Prevents traversal via provider-declared malicious schema. |
| V6 Cryptography | yes | Never hand-roll crypto. Secrets stay plaintext in `.env` (same risk envelope as today). For the future mirror provider HTTP case, TLS via `reqwest` + `rustls-tls` (already in workspace at `Cargo.toml:30`) — not in this phase. |

### Known Threat Patterns for Rust trait + CLI wizard + disk I/O

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Path traversal on `hermes_home.join(provider.name() + ".json")` if `provider.name()` contains `"../"` | Tampering | `name()` returns `&'static str` (D-02) — literal strings only, no user input. Still, lint check in code review: `name()` must never be `format!()`-generated. Add a `debug_assert!(!name.contains(['/', '\\', '.']))` in `save_config` default impl. |
| `.env` append with untrusted value containing `\n` or shell metacharacters | Tampering | Wizard serializes `<KEY>=<VALUE>` with value quoted (`"..."`) and embedded quotes escaped. Reject value containing raw `\n`. |
| `ConfigField.env_var` collision with existing important env var (e.g., `PATH`, `HOME`) | Tampering | Wizard check before append: refuse if `env_var` matches a deny-list (`PATH`, `HOME`, `USER`, `SHELL`, `IRONHERMES_HOME`). Provider-declared — reject with error. |
| Config JSON path traversal via malicious `provider.name()` (same as above) | Tampering | Same mitigation. |
| Secret leak via log: `tracing::warn!("save_config got {}", values)` printing full secret | Information Disclosure | `ConfigField.secret = true` fields redacted in `Debug` impl of the wizard's value map. Implement a `RedactedValue` wrapper with `Debug` that prints `***`. Never `format!("{:?}", values)` the raw map. |
| Mirror provider receiving secrets via `on_memory_write(content)` | Information Disclosure | Content scanning (`scan_context_content`) already runs in every primary write path (existing, Phase 17). Mirror receives already-scanned content. If mirror is a remote service (supermemory), the provider-side HTTPS + API key constraint is the user's concern, outside this phase. |
| Mirror provider DoS via slow `on_memory_write` blocking primary | Denial of Service | `MemoryManager::handle_tool_call` drops primary guard before calling mirror (see Pattern 3 caveat). Additionally, mirror call wrapped in `tokio::time::timeout(Duration::from_secs(5), ...)` — failure logged, primary returns Ok. |

## Sources

### Primary (HIGH confidence — direct codebase read)
- `crates/ironhermes-core/src/memory_provider.rs` (127 lines, full) — trait definition + file-provider impl
- `crates/ironhermes-agent/src/memory/factory.rs` (137 lines, full) — factory + existing tests
- `crates/ironhermes-agent/src/memory_flush_handler.rs` (164 lines, full) — existing async listener prior art
- `crates/ironhermes-agent/src/agent_loop.rs:1-230, 700-830` — session_search intercept pattern + tool dispatch flow
- `crates/ironhermes-agent/src/prompt_builder.rs:350-394` — load_memory target-scoped emit site (slot 3)
- `crates/ironhermes-agent/src/context_engine.rs:1-160` — ContextEngine trait + LocalPruningEngine.compress shape
- `crates/ironhermes-tools/src/memory_tool.rs` (461 lines, full) — memory tool is a single `"memory"` tool with action arg (not three tools)
- `crates/ironhermes-tools/src/registry.rs:209-228` — register_memory_tool signature
- `providers/memory-sqlite/src/lib.rs` (667 lines, full) — SQLite impl, snapshot pattern
- `providers/memory-duckdb/src/lib.rs` (484 lines, full) — DuckDB impl via bridge
- `providers/memory-grafeo/src/lib.rs` (692 lines, full) — Grafeo impl
- `crates/ironhermes-cli/src/main.rs:1-100, 240-345, 605-760` — clap shape, run_single, run_chat, run_gateway wiring
- `crates/ironhermes-core/src/skills.rs:85-107, 750-770` — SkillConfigField prior art for ConfigField shape
- `crates/ironhermes-core/src/types.rs:94-118` — ToolSchema shape (owned Vec<ToolSchema> is the standard pattern)
- `Cargo.toml` (89 lines, full) — workspace dependencies
- `.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md` — locked decisions
- `.planning/REQUIREMENTS.md` — MEM-07, MEM-08, MEM-10, MEM-11, MEM-12
- `.planning/PROJECT.md` — line 52 constraint
- `.planning/todos/pending/2026-04-16-gateway-memory-does-not-persist-across-restart-factory-never.md`
- `.planning/todos/pending/2026-04-16-chat-and-single-cli-modes-have-no-memory-wiring.md`

### Secondary (MEDIUM confidence)
- `.planning/phases/18-context-compression/18-04-SUMMARY.md` — context:pre_compress hook event shape (grepped, not fully read)

### Tertiary (LOW confidence)
- None — this research did not require external web lookups. Every claim is verified against a workspace file.

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — every crate already in workspace Cargo.toml
- Architecture: HIGH — all four patterns have prior art in workspace
- Pitfalls: HIGH — all eight pitfalls identified by direct code inspection, not speculation
- Validation: HIGH — test layout matches established crate conventions
- Security: MEDIUM — threat model grounded in concrete file write paths, but no formal STRIDE pass done

**Research date:** 2026-04-16
**Valid until:** 2026-05-16 (30 days — stable Rust workspace, not fast-moving)

---

## RESEARCH COMPLETE

Phase 20 research grounds every claim in direct workspace code. Key surprises: (1) `MemoryProviderConfig` has zero live callers so D-10 is lower risk than CONTEXT.md suggested; (2) the "hard-coded memory_add/replace/remove name-match" described in D-18 does NOT exist — the memory tool is one tool named `"memory"` with an action arg, so Plan 20-02 reshapes `MemoryTool::execute` to delegate through `MemoryManager` rather than replacing a name-list intercept; (3) `on_pre_compress` cannot ride on the existing `ContextPreCompress` hook event (no `messages` in the payload) — call `MemoryManager::on_pre_compress` directly from inside `ContextEngine::compress`. Eight pitfalls documented; six assumptions logged; four open questions flagged for the planner.
