# Phase 20: Memory Provider Plugin Contract - Context

**Gathered:** 2026-04-16
**Status:** Draft — awaiting discussion

<domain>
## Phase Boundary

Bring the Rust `MemoryProvider` trait to **API parity** with the hermes-agent Python `MemoryProvider` ABC plugin contract — so that memory backends behave like plugins (owning their own tools, config schema, availability check, and rich lifecycle hooks) — **without** introducing runtime plugin discovery. Per `PROJECT.md` Out of Scope ("Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity"), providers remain compile-time crates under `providers/memory-*` selected by Cargo features. The factory in `crates/ironhermes-agent/src/memory/factory.rs` stays hard-coded.

**In scope:**
- Extend the `MemoryProvider` trait with missing hook surface: `name`, `is_available`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`, `on_pre_compress`, `on_memory_write`, and the already-present `shutdown`/`sync_turn`/`on_session_end`/`prefetch`/`initialize`.
- Wire each new hook into the correct agent-loop / gateway-runner callsite so it actually fires.
- Convert the existing `memory_add` / `memory_replace` / `memory_remove` tool intercept into a thin shim that calls `provider.handle_tool_call(...)` when the active provider wants to override (default impl preserves current behavior).
- Config surface: a lightweight `hermes memory setup` command that reads `get_config_schema()` from the selected provider and writes secrets to `.env` / non-secrets to `$HERMES_HOME/<provider>.json`.
- Regression: the factory calls `load_from_disk()` for every provider (fix for pending todo `memory-does-not-persist-across-gateway-restart-...`).

**Out of scope:**
- Runtime plugin loading (`plugins/memory/<name>/plugin.yaml` discovery, `register()` entry points, dynamic library ABI). Explicitly deferred per PROJECT.md.
- A Python-style `__init__.py` mechanism. Rust equivalent would be compile-time feature flags, already in place.
- New provider backends. This phase refines the contract for the existing three (sqlite, duckdb, grafeo) plus the file-based default.

</domain>

<decisions>
## Implementation Decisions

### Trait enrichment

- **D-01:** Keep `MemoryProvider` in `ironhermes-core/src/memory_provider.rs` (D-10 from Phase 17 stands). Add new methods with default implementations so existing providers keep compiling without edits — each provider opts in to the hooks it cares about.
- **D-02:** Required (no default): `name() -> &'static str`. Default impl removed from any callsite that previously derived name from the config string. The factory still dispatches on `config.provider`, but after construction the provider's own `name()` is the canonical identifier used in logs, metrics, and `save_config` filenames.
- **D-03:** `is_available(&self) -> bool` — default `true`. Evaluated by the factory after construction but before the provider is installed; if `false`, the factory falls back to the file-based provider and emits a `tracing::warn!` with the reason exposed via `unavailable_reason() -> Option<String>` (default `None`). **No network calls in `is_available`** — mirrors the hermes-agent contract.
- **D-04:** `get_tool_schemas(&self) -> Vec<ToolSchema>` — default returns the current three-tool set (`memory_add`, `memory_replace`, `memory_remove`). Providers like supermemory that surface extra tools (e.g. `memory_search`) override. Schemas use the existing `ironhermes_tools::tool::ToolSchema` type — no new schema surface.
- **D-05:** `handle_tool_call(&mut self, name: &str, args: Value) -> MemoryResult` — default routes to the existing `add`/`replace`/`remove` methods by matching on `name`. Providers can override to add new tools or pre/post-process arguments. The agent-loop intercept in `agent_loop.rs` loses its hard-coded match on tool name and instead checks `provider.get_tool_schemas()` for membership.

### Config schema

- **D-06:** `get_config_schema(&self) -> Vec<ConfigField>` — default returns `vec![]`. New type `ConfigField { key, description, secret, required, default, choices, env_var, url }` in `ironhermes-core/src/config_schema.rs`. Mirrors the hermes-agent shape 1:1.
- **D-07:** `save_config(&self, values: &HashMap<String, Value>, hermes_home: &Path) -> anyhow::Result<()>` — default no-op. Providers write non-secret config to `$HERMES_HOME/<provider_name>.json`. Secret fields are handled by the caller (wizard), not the provider — the provider never sees secrets at save time; it reads them from the env var named by `ConfigField.env_var` at `initialize()` time.
- **D-08:** `hermes memory setup` is a **minimal** CLI subcommand added to `crates/ironhermes-cli/src/main.rs`. It enumerates the compiled-in providers, asks which to activate, calls `get_config_schema()` on that provider, prompts only for fields marked `required=true` + fields without a `default`, writes secrets to `.env` (appending, not replacing), and calls `save_config()` for the rest. Optional fields stay in the JSON for hand-editing. Follows the hermes-agent "Minimal vs Full Schema" guidance.
- **D-09:** The existing `MemoryConfig` in `ironhermes-core/src/config.rs` stays the selector (`provider: "sqlite"`). Provider-specific config lives beside it, loaded lazily by `initialize(session_id, config_json)` — a new argument shape.

### Lifecycle hooks

- **D-10:** `initialize` signature becomes `async fn initialize(&mut self, session_id: &str, hermes_home: &Path, provider_config: &Value) -> anyhow::Result<()>`. The existing `MemoryProviderConfig` struct is deprecated; the `Value` slot lets each provider deserialize its own shape. Session id and `hermes_home` match the hermes-agent `kwargs` contract.
- **D-11:** `system_prompt_block(&self) -> Option<String>` — default `None`. Additive to the existing `format_for_system_prompt(target)`. Providers that want a single free-form block (e.g. "Recalled: ...") return it here; `PromptBuilder` appends it to slot 3 after the target-scoped blocks.
- **D-12:** `queue_prefetch(&self, query: &str) -> anyhow::Result<()>` — default no-op. Called by `AgentLoop` after each completed turn so the provider can pre-warm the next `prefetch` asynchronously. Implementation should `tokio::spawn` and return immediately — agent does not await.
- **D-13:** `on_pre_compress(&self, messages: &[ChatMessage]) -> anyhow::Result<()>` — default no-op. Fires from the context-engine pre-compress hook (already present in Phase 18) before messages are discarded. Providers use it to extract insights into durable memory.
- **D-14:** `on_memory_write(&mut self, action: MemoryAction, target: MemoryTarget, content: &str) -> anyhow::Result<()>` — default no-op. Fires from the built-in `MemoryStore` (file provider) after a successful add/replace/remove, so an *additional* external provider can mirror writes. Enables the hermes-agent "mirror to supermemory" pattern without replacing the file provider.
- **D-15:** `shutdown` stays as it is. `on_session_end` stays as it is.

### Factory + wiring

- **D-16:** The factory keeps the `cfg!(feature = "memory-<name>")` match but after `Provider::new(...)` it unconditionally calls `provider.load_from_disk()?` — fixes the pending todo. Stale comment at factory.rs:9-10 rewritten.
- **D-17:** `is_available` is evaluated *after* `load_from_disk` so providers can check both env and on-disk state. If `false`, factory returns the file provider instead and logs the reason — the agent still boots.
- **D-18:** Agent-loop change: `agent_loop.rs` currently hard-codes `memory_add`/`memory_replace`/`memory_remove` as intercepted tools. Change: build the intercept set from `provider.get_tool_schemas().iter().map(|s| s.name)` at agent startup and match against that. Default impl preserves today's behavior.
- **D-19:** `session_search` stays its own intercept. Not a memory-provider tool — owned by `StateStore`.

### Migration

- **D-20:** The default-impl strategy means none of the existing `providers/memory-*` crates need edits to keep compiling. They opt in to new hooks as separate PRs under the same phase. Track per-provider adoption in `20-VERIFICATION.md` — file, sqlite, duckdb, grafeo each decide which hooks to override.
- **D-21:** The deprecated `MemoryProviderConfig` stays for one more release as a compatibility shim — `From<&MemoryProviderConfig> for Value` — so downstream code can migrate incrementally. Removed in a follow-up phase.

### Testing strategy

- **D-22:** Trait-level tests use a `MockMemoryProvider` in `ironhermes-core/tests/` that records every hook invocation. Agent-loop tests assert the right hooks fire in the right order: `initialize` → (per turn) `prefetch` → (agent runs) → `sync_turn` → `queue_prefetch` → (on compress) `on_pre_compress` → (on end) `on_session_end` → `shutdown`.
- **D-23:** `hermes memory setup` gets a scripted-input integration test that runs the wizard against a temp `HERMES_HOME` with a fake provider that has a representative `ConfigField` list (one secret, one required-with-default, one optional).
- **D-24:** Regression test for the `load_from_disk` factory bug: spawn a SQLite provider via the factory, add an entry, drop, re-open via the factory at the same `HERMES_HOME`, assert `format_for_system_prompt` returns the entry. This is the same test requested in the pending todo; it lands here rather than as a one-off fix.

### Claude's Discretion

- Exact shape of `ConfigField` (optional fields list may grow — `pattern`, `multiline`, `min_length` — decide based on real provider needs)
- Whether `get_tool_schemas` returns owned `Vec<ToolSchema>` or borrows from a `&'static` slice (performance vs ergonomics)
- Whether `on_memory_write` is a broadcast (multiple mirror providers) or single-target in this phase
- File layout for the setup wizard (single `setup_memory_wizard.rs` module vs subfolder)
- Exact message for fallback-to-file log when `is_available` returns false

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### External spec (provided by user)
- hermes-agent `agent/memory_provider.py` -- MemoryProvider ABC: `name`, `is_available`, `initialize(session_id, **kwargs)`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `prefetch`, `queue_prefetch`, `sync_turn`, `on_session_end`, `on_pre_compress`, `on_memory_write`, `shutdown`. Canonical method list + docstrings.
- hermes-agent `plugins/memory/` -- Directory-per-provider layout. Use as **API** reference, NOT structural reference (we stay compile-time, per PROJECT.md Out of Scope).
- hermes-agent supermemory provider -- Example of "minimal schema" (only API key prompted; rest in JSON file).

### Existing implementation
- `crates/ironhermes-core/src/memory_provider.rs:40-127` -- Current trait + MemoryStore impl.
- `crates/ironhermes-agent/src/memory/factory.rs:11-72` -- Hard-coded provider factory (also has the load_from_disk bug captured in the pending todo).
- `crates/ironhermes-agent/src/prompt_builder.rs:370-394` -- `load_memory` calls `format_for_system_prompt`; needs to also call `system_prompt_block`.
- `crates/ironhermes-tools/src/memory_tool.rs` -- Current hard-coded memory tool; becomes a provider-delegating shim.
- `crates/ironhermes-agent/src/agent_loop.rs` -- Memory-tool intercept site; switches from name-list to `provider.get_tool_schemas()`.
- `crates/ironhermes-agent/src/memory_flush_handler.rs` -- Existing pre-compress hook; becomes the caller of `on_pre_compress`.
- `providers/memory-sqlite/src/lib.rs`, `providers/memory-duckdb/src/lib.rs`, `providers/memory-grafeo/src/lib.rs` -- Three existing providers; must stay compiling via default trait impls.

### Prior phase context
- `.planning/phases/11-memory-provider-trait/` -- Original trait design.
- `.planning/phases/17-memory-tools-external-providers/17-CONTEXT.md` -- Feature-gated crate layout (D-09..D-12), provider selection, migration prompt.
- `.planning/phases/18-context-compression/` -- Pre-compress hook wiring; reused for `on_pre_compress`.

### Constraints
- `.planning/PROJECT.md:52` -- "Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity." Governs the "API parity only, no runtime loader" stance.
- `.planning/PROJECT.md:65` -- "Rust 2024 edition — committed, no mixed Python/Rust."
- `.planning/PROJECT.md:103` -- "Port faithfully, deviate only with documented rationale." Documenting the dynamic-loader deviation is part of this phase's deliverable.

### Pending todo folded in
- `.planning/todos/pending/2026-04-16-memory-does-not-persist-across-gateway-restart-and-chat-mode.md` -- Fix 1 (factory `load_from_disk`) is absorbed into Plan 20-01. Fix 2 (chat-mode wiring) is a separate plan within this phase.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MemoryProvider` trait already has `initialize`, `prefetch`, `sync_turn`, `on_session_end`, `shutdown`, `load_from_disk`, `add`, `replace`, `remove`, `format_for_system_prompt`, `to_memory_entries`. Extension is additive.
- `ToolSchema` type in `ironhermes-tools/src/tool.rs` — reused as-is for `get_tool_schemas`.
- `scan_context_content` already runs in every write path — `on_memory_write` observers get scanned content for free.
- `ContextEngine` pre-compress hook (Phase 18) already fires; just needs to call `provider.on_pre_compress` when a provider is registered.
- `MemoryStore` (file) is still the default; stays as the anchor provider that other providers can mirror via `on_memory_write`.

### Established Patterns
- `Arc<Mutex<dyn MemoryProvider + Send>>` sharing across agent + gateway + tools — unchanged.
- Default trait method impls are the safe migration lever — no breakage across provider crates.
- Feature-gated optional deps in entry-point `Cargo.toml` — unchanged.
- Hook-from-agent-loop pattern (same as memory-tool intercept) for `queue_prefetch`, `on_pre_compress`.

### Integration Points
- `agent_loop.rs`: switch memory-tool intercept from hard-coded name set to `provider.get_tool_schemas()` membership; call `queue_prefetch` after each turn.
- `context_engine`: call `provider.on_pre_compress` from existing pre-compress hook.
- `prompt_builder.rs`: extend `load_memory` to also pull `system_prompt_block`.
- `memory_tool.rs`: delegate actions to `provider.handle_tool_call`.
- `cli/main.rs`: new `memory setup` subcommand; chat/single-mode memory wiring (covered by Fix 2 in the pending todo).
- `ironhermes-core/src/config_schema.rs`: new module.
- `ironhermes-core/src/memory_provider.rs`: trait edits.

### Known Bug Folded In
- Factory does not call `load_from_disk` on external providers (pending todo 2026-04-16). Plan 20-01 fixes it plus adds the regression test.

</code_context>

<specifics>
## Specific Ideas

- The hermes-agent plugin **shape** is what the user is asking for; the *mechanism* (Python dynamic loading) is explicitly off the table. API parity is achievable by enriching the trait surface and wiring the hooks.
- Keeping every new trait method defaulted lets the phase ship incrementally: Plan 20-01 extends the trait + fixes the factory bug; Plan 20-02 wires the agent loop + prompt builder; Plan 20-03 implements the setup wizard; Plan 20-04 migrates each provider onto the new hooks.
- Chat-mode memory wiring (Fix 2 in the pending todo) fits naturally in Plan 20-03 alongside the setup wizard — both live in the CLI.
- `on_memory_write` is the most powerful hook: it lets an external memory service (supermemory, mem0, LangGraph store) mirror the file provider's writes without replacing it. This was the key hermes-agent differentiator the user is after.
- If the user later revisits PROJECT.md Out of Scope and wants *true* runtime plugin loading, this phase becomes the foundation — the trait already matches the dynamic contract; only `libloading`/`abi_stable`/`wasm-component` plumbing would be new.

</specifics>

<deferred>
## Deferred Ideas

- Runtime plugin discovery (`plugins/memory/<name>/` directory, `plugin.yaml` manifest, `register()` entry point, dynamic library loading). Deferred per PROJECT.md Out of Scope. Revisit if/when the compile-in constraint is lifted.
- A `hermes memory list` / `hermes memory test <name>` CLI surface. Useful but not in the core contract — follow-up phase.
- Multi-provider concurrent operation (e.g. file + supermemory both active as peers, not mirror). `on_memory_write` enables a single mirror; true multi-provider needs a router. Out of scope here.
- Async variants of `add`/`replace`/`remove`. Current trait is sync; providers use internal mutexes. External HTTP providers will want async — revisit when the first one lands.
- Web UI for setup. CLI wizard only.

### Reviewed Todos (not folded)
- `2026-04-02-add-setup-wizard-and-config-scaffolding-for-gateway-testing` — that todo is broader than memory (it covers gateway credentials, Telegram token, etc.). The memory portion is folded here; the rest stays for Phase 23 per existing deferral.
- `2026-04-13-live-uat-re-pass-for-phase-18-behavioral-tests-2-8` — unrelated; Phase 18 UAT.
- `2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread` — unrelated; CLI signal handling.

</deferred>

<proposed_plans>
## Proposed Plan Breakdown

Four plans, ordered for safe incremental landing:

### 20-01: Trait enrichment + factory load regression
- Add defaulted trait methods: `name`, `is_available`, `unavailable_reason`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`, `on_pre_compress`, `on_memory_write`.
- Introduce `ConfigField` + `MemoryAction` types.
- Reshape `initialize` signature (deprecate old `MemoryProviderConfig`, keep as `From` shim).
- Fix factory: call `load_from_disk()` for every provider; handle `is_available = false` by falling back to file provider.
- Regression test: factory persistence round-trip for sqlite, duckdb, grafeo.
- Absorbs Fix 1 of the pending todo.

### 20-02: Agent-loop and prompt-builder wiring
- `agent_loop.rs` intercept uses `provider.get_tool_schemas()` membership instead of hard-coded names; dispatches via `provider.handle_tool_call`.
- `prompt_builder.rs` calls `provider.system_prompt_block` and appends after target-scoped blocks.
- `agent_loop.rs` calls `provider.queue_prefetch(query)` after each turn (fire-and-forget).
- `context_engine` pre-compress hook calls `provider.on_pre_compress(messages)`.
- `MemoryStore::add/replace/remove` emit `on_memory_write` on the configured external provider if any.

### 20-03: Setup wizard + chat-mode memory wiring
- `hermes memory setup` subcommand: enumerate providers, read `get_config_schema`, prompt minimally, write secrets to `.env`, call `save_config`.
- Chat-mode wiring (Fix 2 of pending todo): `run_chat` / `run_single` call `build_memory_provider`, register memory tool, `set_memory_store` on prompt builder.
- Integration test with scripted stdin.

### 20-04: Provider adoption
- Each of file, sqlite, duckdb, grafeo picks up the hooks it cares about:
  - file: `name = "file"`, `get_config_schema` returns memory-dir + char limits.
  - sqlite: `name = "sqlite"`, `get_config_schema` returns DB path; later `queue_prefetch` for FTS5 warmup.
  - duckdb: `name = "duckdb"`, `get_config_schema` returns DB path + thread count.
  - grafeo: `name = "grafeo"`, `get_config_schema` returns graph dir.
- One or two providers demonstrate `on_memory_write` mirror behavior as a test fixture.

</proposed_plans>

---

*Phase: 20-memory-provider-plugin-contract*
*Context drafted: 2026-04-16 — awaiting `/gsd-discuss-phase` before planning*
