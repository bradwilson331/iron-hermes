# Phase 20: Memory Provider Plugin Contract - Context

**Gathered:** 2026-04-16
**Status:** Ready for planning

<domain>
## Phase Boundary

Bring the Rust `MemoryProvider` trait to **API parity** with the hermes-agent Python `MemoryProvider` ABC plugin contract — so that memory backends behave like plugins (owning their own tools, config schema, availability check, and rich lifecycle hooks) — **without** introducing runtime plugin discovery. Per `PROJECT.md:52` ("Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity"), providers remain compile-time crates under `providers/memory-*` selected by Cargo features. The factory in `crates/ironhermes-agent/src/memory/factory.rs` stays hard-coded.

**In scope:**
- Extend the `MemoryProvider` trait with the missing hook surface: `name`, `is_available`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`, `on_pre_compress`, `on_memory_write`, alongside existing `shutdown`/`sync_turn`/`on_session_end`/`prefetch`/`initialize`.
- Introduce a new `MemoryManager` layer that wraps the active provider, owns write-path dispatch, and fires `on_memory_write` to a single external mirror when configured.
- Wire each new hook into the correct agent-loop / gateway-runner / context-engine callsite so it actually fires.
- Convert the existing `memory_add` / `memory_replace` / `memory_remove` tool intercept into a thin shim that calls `provider.handle_tool_call(...)` (default impl preserves current behavior; providers can override to surface new tools).
- Config surface: a **minimal** `hermes memory setup` CLI subcommand that reads `get_config_schema()` from the selected provider and writes secrets to `.env` / non-secrets to `$HERMES_HOME/<provider>.json`.
- **Breaking change:** reshape `initialize()` to `(session_id, hermes_home, &Value)` and **delete `MemoryProviderConfig` entirely** — all three provider crates (sqlite, duckdb, grafeo) migrate in this phase, no compat shim.
- Regression: factory calls `load_from_disk()` for every provider (fix for pending todo `memory-does-not-persist-across-gateway-restart-...`, Fix 1).
- Chat-mode / single-mode memory wiring (Fix 2 of same pending todo) lands alongside the setup wizard in the CLI.

**Out of scope:**
- Runtime plugin loading (YAML manifests, `libloading`, WASM/Wasmtime components, dynamic ABI). Explicitly deferred per `PROJECT.md:52`. Captured as a deferred idea — revisit only if that constraint is lifted in a future milestone review.
- Python-style `__init__.py` / `plugin.yaml` discovery. Rust equivalent is compile-time feature flags, already in place.
- New provider backends. Phase 20 refines the contract for the existing three (sqlite, duckdb, grafeo) plus the file-based default.
- Broadcast `on_memory_write` to multiple subscribers — single mirror only in this phase (per `MEM-12`).
- `hermes memory list` / `hermes <provider> test` CLI surface — deferred to a follow-up phase.

</domain>

<decisions>
## Implementation Decisions

### Trait enrichment

- **D-01:** Keep `MemoryProvider` in `ironhermes-core/src/memory_provider.rs` (D-10 from Phase 17 stands). Add new methods with default implementations so providers only edit code for hooks they care about. Exception: `initialize` is a breaking signature change (see D-10 below) — all providers edit.
- **D-02:** Required (no default): `name() -> &'static str`. The factory dispatches on `config.provider`, but after construction the provider's own `name()` is the canonical identifier used in logs, metrics, and `save_config` filenames.
- **D-03:** `is_available(&self) -> bool` — default `true`. Evaluated by the factory after `load_from_disk`; if `false`, factory falls back to the file-based provider and emits a `tracing::warn!` with the reason exposed via `unavailable_reason() -> Option<String>` (default `None`). **No network calls in `is_available`** — mirrors the hermes-agent contract.
- **D-04:** `get_tool_schemas(&self) -> Vec<ToolSchema>` — default returns the current three-tool set (`memory_add`, `memory_replace`, `memory_remove`). Providers that surface extra tools (e.g. `memory_search`) override. Schemas reuse `ironhermes_tools::tool::ToolSchema` — no new schema surface.
- **D-05:** `handle_tool_call(&mut self, name: &str, args: Value) -> MemoryResult` — default routes to the existing `add`/`replace`/`remove` methods by matching on `name`. Providers override to add tools or pre/post-process arguments. The agent-loop intercept in `agent_loop.rs` replaces its hard-coded name match with `provider.get_tool_schemas()` membership (see D-18).

### Config schema

- **D-06:** `get_config_schema(&self) -> Vec<ConfigField>` — default returns `vec![]`. New type `ConfigField { key, description, secret, required, default, choices, env_var, url }` in `ironhermes-core/src/config_schema.rs`. Mirrors the hermes-agent shape 1:1. Additional optional fields (`pattern`, `multiline`, `min_length`) may be added later if a provider needs them — not speculatively.
- **D-07:** `save_config(&self, values: &HashMap<String, Value>, hermes_home: &Path) -> anyhow::Result<()>` — default no-op. Providers write non-secret config to `$HERMES_HOME/<provider_name>.json`. Secret fields are handled by the wizard caller, not the provider — the provider never sees secrets at save time; it reads them from the env var named by `ConfigField.env_var` at `initialize()` time.
- **D-08:** `hermes memory setup` is a **minimal** CLI subcommand in `crates/ironhermes-cli/src/main.rs`. It enumerates the compiled-in providers, asks which to activate, calls `get_config_schema()` on that provider, prompts only for fields marked `required=true` + fields without a `default`, writes secrets to `.env` (appending, not replacing), and calls `save_config()` for the rest. Optional fields stay in the JSON for hand-editing. `hermes memory list` / `hermes <provider> test` are **deferred** — not in this phase.
- **D-09:** `MemoryConfig` in `ironhermes-core/src/config.rs` stays the selector (`provider: "sqlite"`). Provider-specific config lives beside it, loaded lazily by the new `initialize(session_id, hermes_home, provider_config)` argument shape.

### Lifecycle hooks

- **D-10:** **BREAKING.** `initialize` signature becomes `async fn initialize(&mut self, session_id: &str, hermes_home: &Path, provider_config: &Value) -> anyhow::Result<()>`. **`MemoryProviderConfig` is deleted** — no compat shim, no `From` impl. All three provider crates (sqlite, duckdb, grafeo) + the file-based `MemoryStore` migrate inside this phase. Consequence: Plan 20-01 now touches every provider crate; the plans are tightly coupled and land in order.
- **D-11:** `system_prompt_block(&self) -> Option<String>` — default `None`. Additive to the existing `format_for_system_prompt(target)`. Providers that want a single free-form block (e.g. "Recalled: ...") return it here; `PromptBuilder::load_memory` appends it to slot 3 after the target-scoped blocks.
- **D-12:** `queue_prefetch(&self, query: &str) -> anyhow::Result<()>` — default no-op. Called by `AgentLoop` after each completed turn so the provider can pre-warm the next `prefetch` asynchronously. Implementation should `tokio::spawn` and return immediately — agent does not await.
- **D-13:** `on_pre_compress(&self, messages: &[ChatMessage]) -> anyhow::Result<()>` — default no-op. Fires from the context-engine pre-compress hook (already present in Phase 18) before messages are discarded. Providers use it to extract insights into durable memory.
- **D-14:** `on_memory_write(&mut self, action: MemoryAction, target: MemoryTarget, content: &str) -> anyhow::Result<()>` — default no-op. **Fires from the new `MemoryManager` layer (not from any individual provider)** after a successful `add`/`replace`/`remove` on the primary provider. **Single-subscriber only** (per `MEM-12`): the MemoryManager fires to at most one configured external mirror; broadcast is deferred. This survives primary-provider swaps because the emit site is the manager, not the file provider.
- **D-15:** `shutdown` and `on_session_end` stay as-is.

### MemoryManager (new layer)

- **D-25:** New module `crates/ironhermes-agent/src/memory/manager.rs`. `MemoryManager` wraps the active primary provider plus an optional mirror subscriber. All write-path calls from `MemoryTool` and from agent-intercepted actions flow through `MemoryManager::handle_tool_call` / `MemoryManager::add/replace/remove`, which:
  1. Delegates to `primary.handle_tool_call(...)`.
  2. On success, calls `mirror.on_memory_write(action, target, content)` if a mirror is configured — error-logged but **not propagated** (mirror failures must not break primary writes).
- **D-26:** `MemoryManager` also owns the call sites for `queue_prefetch` (post-turn) and `on_pre_compress` (from context engine), routing to both primary and mirror. Read paths (`prefetch`, `format_for_system_prompt`, `system_prompt_block`) go to the primary only.
- **D-27:** Mirror selection is config-driven: one new optional field `memory.mirror_provider` (string, e.g. `"supermemory"`). When set, the factory constructs both primary and mirror and passes them to `MemoryManager::new(primary, Some(mirror))`. When absent, `MemoryManager::new(primary, None)`.
- **D-28:** `MEM-12` (single active memory provider) still holds for **primary**; the mirror is observational (write-only shadow) and does not serve reads. This preserves the requirement's intent (no conflicting facts on read) while enabling the supermemory mirror pattern the user wants.

### Factory + wiring

- **D-16:** The factory keeps the `cfg!(feature = "memory-<name>")` match but after `Provider::new(...)` it unconditionally calls `provider.load_from_disk()?` — fixes the pending todo (Fix 1). Stale comment at `factory.rs:9-10` rewritten.
- **D-17:** `is_available` is evaluated *after* `load_from_disk` so providers can check both env and on-disk state. If `false`, factory returns the file provider instead and logs the reason via `unavailable_reason()`. Agent still boots.
- **D-18:** Agent-loop change: `agent_loop.rs` currently hard-codes `memory_add`/`memory_replace`/`memory_remove` as intercepted tool names. Change: build the intercept set from `memory_manager.get_tool_schemas().iter().map(|s| s.name)` at agent startup and match against that. Dispatch through `memory_manager.handle_tool_call`. Default trait impl preserves today's behavior for unchanged providers.
- **D-19:** `session_search` stays its own intercept. Not a memory-provider tool — owned by `StateStore`.

### Migration

- **D-20:** Because `initialize` is a breaking change (D-10), every provider crate **must** migrate in this phase. The remaining new methods are defaulted — providers opt in to hooks they care about in Plan 20-04. Track per-provider adoption in `20-VERIFICATION.md`.
- **D-21:** *(Removed — no compat shim. See D-10.)*

### Testing strategy

- **D-22:** Trait-level tests use a `MockMemoryProvider` in `ironhermes-core/tests/` that records every hook invocation. Agent-loop tests assert the right hooks fire in the right order: `initialize` → (per turn) `prefetch` → (agent runs) → `sync_turn` → `queue_prefetch` → (on compress) `on_pre_compress` → (on write) `on_memory_write` → (on end) `on_session_end` → `shutdown`.
- **D-23:** `hermes memory setup` gets a scripted-input integration test that runs the wizard against a temp `HERMES_HOME` with a fake provider that has a representative `ConfigField` list (one secret, one required-with-default, one optional).
- **D-24:** Regression test for the factory `load_from_disk` bug: spawn a SQLite provider via the factory, add an entry, drop, re-open via the factory at the same `HERMES_HOME`, assert `format_for_system_prompt` returns the entry. Lands here rather than as a one-off fix.
- **D-29:** New test for `MemoryManager` mirror behavior: configure primary=file + mirror=MockMirrorProvider. Perform add/replace/remove on the manager. Assert the mirror saw each write with the correct action/target/content. Assert a failing mirror does not block primary writes (logged, not returned).

### Claude's Discretion

- Exact additional fields on `ConfigField` beyond the core set — add only when a real provider needs them.
- Whether `get_tool_schemas` returns owned `Vec<ToolSchema>` or borrows from `&'static` — pick based on ergonomics during implementation.
- File layout for the setup wizard (single `setup_memory_wizard.rs` vs submodule).
- Exact log message wording for `is_available = false` fallback.
- Whether `MemoryManager` is shared as `Arc<Mutex<MemoryManager>>` or held directly in AgentLoop — decide based on gateway-sharing needs.

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### External spec (user-referenced)
- hermes-agent `agent/memory_provider.py` — MemoryProvider ABC: `name`, `is_available`, `initialize(session_id, **kwargs)`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `prefetch`, `queue_prefetch`, `sync_turn`, `on_session_end`, `on_pre_compress`, `on_memory_write`, `shutdown`. Canonical method list + docstrings.
- hermes-agent `plugins/memory/` — Directory-per-provider layout. Use as **API** reference, NOT structural reference (we stay compile-time, per `PROJECT.md:52`).
- hermes-agent supermemory provider — Example of "minimal schema" (only API key prompted; rest in JSON file) and the mirror-via-`on_memory_write` pattern.

### Existing implementation
- `crates/ironhermes-core/src/memory_provider.rs:40-127` — Current trait + `MemoryStore` impl. Target of trait enrichment.
- `crates/ironhermes-agent/src/memory/factory.rs:11-72` — Hard-coded provider factory. Contains the `load_from_disk` bug (Fix 1).
- `crates/ironhermes-agent/src/prompt_builder.rs:370-394` — `load_memory` calls `format_for_system_prompt`; extend to also call `system_prompt_block`.
- `crates/ironhermes-tools/src/memory_tool.rs` — Current hard-coded memory tool; becomes a `MemoryManager`-delegating shim.
- `crates/ironhermes-agent/src/agent_loop.rs` — Memory-tool intercept site; switches from name-list to `memory_manager.get_tool_schemas()`.
- `crates/ironhermes-agent/src/memory_flush_handler.rs` — Existing pre-compress hook; becomes the caller of `MemoryManager::on_pre_compress`.
- `providers/memory-sqlite/src/lib.rs`, `providers/memory-duckdb/src/lib.rs`, `providers/memory-grafeo/src/lib.rs` — Three existing providers; all migrate to new `initialize` signature in this phase.

### Prior phase context
- `.planning/phases/11-memory-provider-trait/` — Original trait design (D-09..D-12 for provider crate layout).
- `.planning/phases/17-memory-tools-external-providers/17-CONTEXT.md` — Feature-gated crate layout, provider selection, migration prompt; `MEM-12` single-provider constraint.
- `.planning/phases/18-context-compression/` — Pre-compress hook wiring; reused for `on_pre_compress` fire site.

### Constraints
- `.planning/PROJECT.md:52` — "Plugin/extension system — tools are compiled-in; dynamic loading is premature complexity." Governs the "API parity only, no runtime loader" stance.
- `.planning/PROJECT.md:65` — "Rust 2024 edition — committed, no mixed Python/Rust."
- `.planning/PROJECT.md:103` — "Port faithfully, deviate only with documented rationale." Documenting the dynamic-loader deviation is part of this phase's deliverable.
- `.planning/REQUIREMENTS.md` **MEM-12** — "Only one external memory provider can be active at a time — single-provider selection via config." Mirror-as-observational preserves this.

### Pending todo folded in
- `.planning/todos/pending/2026-04-16-memory-does-not-persist-across-gateway-restart-and-chat-mode.md` — Fix 1 (factory `load_from_disk`) absorbed into Plan 20-01. Fix 2 (chat-mode wiring) lands in Plan 20-03 alongside the setup wizard.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `MemoryProvider` trait already has `initialize`, `prefetch`, `sync_turn`, `on_session_end`, `shutdown`, `load_from_disk`, `add`, `replace`, `remove`, `format_for_system_prompt`, `to_memory_entries`. Extension is additive except for `initialize` (breaking).
- `ToolSchema` type in `ironhermes-tools/src/tool.rs` — reused as-is for `get_tool_schemas`.
- `scan_context_content` already runs in every write path — `on_memory_write` observers get scanned content for free.
- `ContextEngine` pre-compress hook (Phase 18) already fires; just needs to call `MemoryManager::on_pre_compress`.
- `MemoryStore` (file) stays the default primary; the new `MemoryManager` is the layer where mirroring happens.

### Established Patterns
- `Arc<Mutex<dyn MemoryProvider + Send>>` sharing across agent + gateway + tools — pattern reused by `MemoryManager`.
- Default trait method impls are the safe migration lever — no breakage across provider crates for defaulted methods.
- Feature-gated optional deps in entry-point `Cargo.toml` — unchanged.
- Hook-from-agent-loop pattern (same as memory-tool intercept) for `queue_prefetch` and `on_pre_compress`.

### Integration Points
- `agent_loop.rs`: switch memory-tool intercept from hard-coded name set to `memory_manager.get_tool_schemas()` membership; call `queue_prefetch` after each turn.
- `context_engine` / `memory_flush_handler.rs`: call `memory_manager.on_pre_compress` from the existing pre-compress hook.
- `prompt_builder.rs`: extend `load_memory` to also pull `system_prompt_block` from the primary provider.
- `memory_tool.rs`: delegate actions to `memory_manager.handle_tool_call`.
- `cli/main.rs`: new `memory setup` subcommand; chat/single-mode memory wiring (Fix 2 of the pending todo).
- `ironhermes-core/src/config_schema.rs`: new module (`ConfigField`, `MemoryAction`).
- `ironhermes-core/src/memory_provider.rs`: trait edits (breaking `initialize`, new defaulted hooks).
- `crates/ironhermes-agent/src/memory/manager.rs`: new module.

### Known Bug Folded In
- Factory does not call `load_from_disk` on external providers (pending todo 2026-04-16, Fix 1). Plan 20-01 fixes it plus adds the regression test.

### Chat-mode Memory Gap Folded In
- Fix 2 of the same pending todo: `run_chat` / `run_single` in `crates/ironhermes-cli/src/main.rs` do not construct a memory manager or register the memory tool. Plan 20-03 wires this alongside the setup wizard.

</code_context>

<specifics>
## Specific Ideas

- The hermes-agent plugin **shape** is what the user is asking for; the *mechanism* (Python dynamic loading) is explicitly off the table per `PROJECT.md:52`. API parity is achievable by enriching the trait surface and wiring the hooks.
- Clean-break migration on `initialize` (no compat shim) is the right call for a Rust port — no parallel config paths to maintain.
- `on_memory_write` firing from a new `MemoryManager` layer (not from the file provider) is what makes the supermemory mirror pattern compose cleanly: the mirror is triggered by the manager regardless of which provider is primary. This is the key hermes-agent differentiator the user is after.
- Single-mirror (not broadcast) preserves `MEM-12` while still delivering the pattern. Broadcast is a future-phase config flip.
- If PROJECT.md Out of Scope is later revised and *true* runtime plugin loading is wanted, this phase becomes the foundation — the trait already matches the dynamic contract; only `libloading`/`abi_stable`/`wasm-component` plumbing would be new.

</specifics>

<deferred>
## Deferred Ideas

- **Runtime plugin discovery** (`plugins/memory/<name>/` directory, `plugin.yaml` manifest, `register()` entry point, dynamic library loading via `libloading` or WASM/Wasmtime). Blocked by `PROJECT.md:52`. Revisit if/when the compile-in constraint is lifted in a future milestone review.
- **`hermes memory list` / `hermes <provider> test` CLI subcommands.** Useful for troubleshooting but not in the core contract. Separate follow-up phase once at least two non-file providers are in real use.
- **Broadcast `on_memory_write`** to multiple subscribers. Requires revisiting `MEM-12` and adding loop-prevention (de-dup keys). Phase 20 ships single-mirror only.
- **Multi-provider peer operation** (file + supermemory both active as peers with read/write on both). The mirror is write-only today. True peer mode needs a router and conflict-resolution rules.
- **Async variants of `add`/`replace`/`remove`.** Current trait is sync; providers use internal mutexes. External HTTP providers will want async — revisit when the first one lands.
- **Web UI for setup.** CLI wizard only.

### Reviewed Todos (not folded)
- `2026-04-02-add-setup-wizard-and-config-scaffolding-for-gateway-testing` — broader than memory (gateway credentials, Telegram token). The memory portion is folded here; the rest stays for Phase 23.
- `2026-04-13-live-uat-re-pass-for-phase-18-behavioral-tests-2-8` — unrelated (Phase 18 UAT).
- `2026-04-13-double-ctrl-c-in-agent-mode-ends-process-and-thread` — unrelated (CLI signal handling).

</deferred>

<proposed_plans>
## Proposed Plan Breakdown

Four plans, ordered for safe incremental landing. Plans 20-01 and 20-04 are now tightly coupled due to the `initialize` breaking change (D-10) — 20-01 must land all three providers at once.

### 20-01: Trait enrichment + factory load regression + provider `initialize` migration
- Add defaulted trait methods: `name`, `is_available`, `unavailable_reason`, `get_tool_schemas`, `handle_tool_call`, `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`, `on_pre_compress`, `on_memory_write`.
- Introduce `ConfigField` + `MemoryAction` types in `ironhermes-core/src/config_schema.rs`.
- **Breaking:** reshape `initialize` → `(session_id, hermes_home, &Value)`. Delete `MemoryProviderConfig`. Migrate all three provider crates + file `MemoryStore` in this plan.
- Fix factory: call `load_from_disk()` for every provider; handle `is_available = false` by falling back to file provider (D-17).
- Regression test: factory persistence round-trip for sqlite, duckdb, grafeo (D-24).
- Absorbs Fix 1 of the pending todo.

### 20-02: MemoryManager layer + agent-loop / prompt-builder / context-engine wiring
- New module `crates/ironhermes-agent/src/memory/manager.rs` implementing `MemoryManager` with primary + optional single mirror (D-25..D-28).
- `agent_loop.rs` intercept uses `memory_manager.get_tool_schemas()` membership instead of hard-coded names; dispatches via `memory_manager.handle_tool_call`.
- `prompt_builder.rs` calls `primary.system_prompt_block` and appends after target-scoped blocks.
- `agent_loop.rs` calls `memory_manager.queue_prefetch(query)` after each turn (fire-and-forget).
- `memory_flush_handler.rs` pre-compress hook calls `memory_manager.on_pre_compress(messages)`.
- MemoryManager fires `on_memory_write` to the configured mirror after successful writes; mirror failures are logged, not propagated (D-29).

### 20-03: Setup wizard + chat-mode memory wiring
- `hermes memory setup` subcommand: enumerate providers, read `get_config_schema`, prompt only required + no-default fields, write secrets to `.env`, call `save_config` (D-08).
- Chat-mode wiring (Fix 2 of pending todo): `run_chat` / `run_single` construct `MemoryManager`, register memory tool, `set_memory_store` on prompt builder.
- Integration test with scripted stdin.

### 20-04: Provider hook adoption
- Each of file, sqlite, duckdb, grafeo picks up the hooks it cares about:
  - **file:** `name = "file"`, `get_config_schema` returns memory-dir + char limits.
  - **sqlite:** `name = "sqlite"`, `get_config_schema` returns DB path; later `queue_prefetch` for FTS5 warmup.
  - **duckdb:** `name = "duckdb"`, `get_config_schema` returns DB path + thread count.
  - **grafeo:** `name = "grafeo"`, `get_config_schema` returns graph dir.
- One provider (sqlite likely) demonstrates `on_memory_write` mirror behavior as a test fixture.

</proposed_plans>

---

*Phase: 20-memory-provider-plugin-contract*
*Context refined via /gsd-discuss-phase on 2026-04-16*
