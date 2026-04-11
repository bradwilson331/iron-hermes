# Phase 11: Memory Provider Trait - Research

**Researched:** 2026-04-11
**Domain:** Rust async trait abstraction, pluggable provider pattern, config extension
**Confidence:** HIGH

## Summary

Phase 11 introduces a `MemoryProvider` trait that makes the memory backend swappable
without changing agent code. The existing `MemoryStore` (file-based, ~400 LOC,
`ironhermes-core`) becomes the default implementation. Every call site today passes
`Arc<Mutex<MemoryStore>>` — after this phase those sites will pass
`Arc<Mutex<dyn MemoryProvider + Send>>` (or equivalent). No external providers are
introduced in this phase; they are Phase 17 scope.

The design is fully constrained by the CONTEXT.md decisions. The only discretion
areas are: crate placement (clearly `ironhermes-core` given the dependency graph),
`MemoryProviderConfig` struct design, `MemoryEntries` wrapper type design, refactor
strategy for `MemoryStore`, and the factory/registry pattern.

The Rust toolchain is 1.94 (stable, edition 2024). Async fn in traits (RPITIT) is
stable since Rust 1.75, so the project has two valid options. However, `async_trait`
0.1.89 is already a workspace dependency used by `Tool` and `PlatformAdapter` — using
it for `MemoryProvider` is consistent with existing patterns and eliminates any
dyn-compatibility complications. Decision D-01 locks this: use `#[async_trait]`.

**Primary recommendation:** Define `MemoryProvider` trait in `ironhermes-core`, wrap
`MemoryStore` to implement it, update all five call sites that hold
`Arc<Mutex<MemoryStore>>` to hold `Arc<Mutex<dyn MemoryProvider + Send>>`, and add a
`MemoryConfig` section to `Config` with `provider: String` defaulting to `"file"`.

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Use `#[async_trait]` for all 5 lifecycle hooks (not native RPITIT). File-based
  MemoryStore returns trivially from async hooks.
- **D-02:** Trait bounds: `Send + Sync + 'static`.
- **D-03:** `initialize(&mut self, config: &MemoryProviderConfig)` — typed config struct
  with provider name/type, provider-specific settings, memory dir path, char limits.
- **D-04:** `prefetch(&self, session_id: &str) -> Result<MemoryEntries>` — returns
  `MemoryEntries` wrapper (HashMap<MemoryTarget, Vec<String>>).
- **D-05:** `sync_turn(&self, session_id: &str, entries: &MemoryEntries)`.
- **D-06:** `on_session_end(&self, session_id: &str, entries: &MemoryEntries)`.
- **D-07:** `shutdown(&mut self)` — clean teardown, no session context.
- **D-08:** Config key `memory.provider` in config.yaml. Values: `"file"` (default),
  `"sqlite"`, `"grafeo"`, `"duckdb"`. Provider-specific settings under
  `memory.<provider_name>:` namespace.
- **D-09:** If config specifies a provider not compiled in, hard error at startup with
  clear message listing available providers and required feature flag.
- **D-10:** Default provider is `"file"` when `memory.provider` absent. File provider
  needs no additional config beyond default memory directory.
- **D-11:** `initialize()` and `shutdown()` errors are fatal — propagate up.
- **D-12:** `prefetch()`, `sync_turn()`, `on_session_end()` errors are logged as
  warnings; on prefetch failure return empty entries; on sync_turn/on_session_end
  failure log and skip.

### Claude's Discretion

- Crate placement for the trait (likely `ironhermes-core`)
- `MemoryProviderConfig` struct design (fields, serde derive, validation)
- `MemoryEntries` wrapper type design
- Refactor strategy for `MemoryStore` to implement the trait while preserving all tests
- Whether to use native RPITIT or `async_trait` (D-01 settles this: use `async_trait`)
- Provider factory/registry pattern for instantiating the configured provider

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope.
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| MEM-07 | MemoryProvider trait with 5 lifecycle hooks and Send+Sync+'static bounds | Trait definition in `ironhermes-core`, `#[async_trait]` already workspace dep |
| MEM-08 | Built-in file-based MemoryStore implements MemoryProvider as default backend | MemoryStore is 100% synchronous internally; all 5 hooks can wrap existing methods trivially |
| MEM-12 | Single-provider selection via config; one external provider at a time | Add `MemoryConfig` to `Config`; provider factory selects at startup; hard error for unknown/uncompiled providers |
</phase_requirements>

---

## Standard Stack

### Core

| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `async-trait` | 0.1.89 | async fn in trait objects | Already workspace dep; used by `Tool` and `PlatformAdapter`; consistent with project |
| `serde` + `serde_yaml` | 1.x / 0.9 | Config struct serialization | Already used throughout `Config` hierarchy |
| `anyhow` | 1.x | Error type for hook return values | All tool/state errors use anyhow |
| `tracing` | 0.1 | Warning logs on non-fatal hook failures | Already used in `MemoryStore` |

[VERIFIED: Cargo.lock — async-trait 0.1.89, all others at workspace versions]

### No New Dependencies Needed

All required crates are already in `[workspace.dependencies]`. This phase adds zero new
Cargo dependencies.

## Architecture Patterns

### Recommended Project Structure

New files added to `ironhermes-core`:

```
crates/ironhermes-core/src/
├── memory_store.rs       (existing — add MemoryProvider impl for MemoryStore)
├── memory_provider.rs    (NEW — MemoryProvider trait, MemoryEntries, MemoryProviderConfig)
└── lib.rs                (re-export new types)
```

Config extension in `ironhermes-core/src/config.rs`:
```
Config {
    ...existing fields...
    pub memory: MemoryConfig,   // NEW
}
```

Provider factory in `ironhermes-core/src/memory_provider.rs`:
```rust
pub fn build_memory_provider(config: &MemoryConfig) -> anyhow::Result<Box<dyn MemoryProvider + Send>>
```

### Pattern 1: MemoryProvider Trait Definition

**What:** Async trait with 5 lifecycle hooks, following the existing `Tool` trait pattern.
**When to use:** All memory backend access — the single interface the rest of the system uses.

```rust
// Source: [VERIFIED: crates/ironhermes-tools/src/registry.rs — Tool trait pattern]
use async_trait::async_trait;
use crate::memory_store::MemoryTarget;
use std::collections::HashMap;
use anyhow::Result;

/// Wrapper for memory entries keyed by target.
#[derive(Debug, Clone, Default)]
pub struct MemoryEntries {
    pub entries: HashMap<MemoryTarget, Vec<String>>,
}

/// Typed config passed to initialize().
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryProviderConfig {
    pub provider: String,
    pub memory_dir: std::path::PathBuf,
    pub memory_char_limit: usize,
    pub user_char_limit: usize,
    /// Provider-specific extra settings (e.g., sqlite.path, grafeo.url).
    #[serde(default)]
    pub extra: std::collections::HashMap<String, serde_json::Value>,
}

#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    async fn initialize(&mut self, config: &MemoryProviderConfig) -> Result<()>;
    async fn prefetch(&self, session_id: &str) -> Result<MemoryEntries>;
    async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> Result<()>;
    async fn on_session_end(&self, session_id: &str, entries: &MemoryEntries) -> Result<()>;
    async fn shutdown(&mut self) -> Result<()>;

    /// Convenience: format entries for system prompt injection.
    /// Default impl mirrors existing MemoryStore::format_for_system_prompt().
    fn format_for_prompt(&self, entries: &MemoryEntries, target: MemoryTarget) -> Option<String>;
}
```

[ASSUMED] — `format_for_prompt` as a trait method is one option; alternatively it can
remain a standalone function. Either approach is valid; the planner should pick one.

### Pattern 2: MemoryStore implements MemoryProvider

**What:** Wrap existing synchronous `MemoryStore` methods in trivial async hooks.
**When to use:** Default file-based backend.

```rust
// Source: [VERIFIED: crates/ironhermes-core/src/memory_store.rs]
#[async_trait]
impl MemoryProvider for MemoryStore {
    async fn initialize(&mut self, config: &MemoryProviderConfig) -> Result<()> {
        // memory_dir is already set in MemoryStore::new(); reload from disk
        self.load_from_disk().map_err(Into::into)
    }

    async fn prefetch(&self, _session_id: &str) -> Result<MemoryEntries> {
        // MemoryStore is already loaded; return current entries as MemoryEntries
        let mut map = HashMap::new();
        for target in &[MemoryTarget::Memory, MemoryTarget::User] {
            if let Some(entries) = self.entries.get(target) {
                map.insert(*target, entries.clone());
            }
        }
        Ok(MemoryEntries { entries: map })
    }

    async fn sync_turn(&self, _session_id: &str, _entries: &MemoryEntries) -> Result<()> {
        Ok(())  // File provider: disk is authoritative, no-op
    }

    async fn on_session_end(&self, _session_id: &str, _entries: &MemoryEntries) -> Result<()> {
        Ok(())  // File provider: writes happen in-place, no flush needed
    }

    async fn shutdown(&mut self) -> Result<()> {
        Ok(())  // File provider: no resources to release
    }
}
```

[ASSUMED] — The exact data exposure needed (making `entries` field accessible) requires
either making the field `pub(crate)`, adding a getter, or restructuring. The planner must
decide the refactor approach that preserves the existing `load_from_disk`/`add`/`replace`/
`remove` methods and their test coverage.

### Pattern 3: Provider Factory

**What:** Single function that reads `MemoryConfig` and returns a boxed trait object.
**When to use:** At startup (CLI `main.rs`, gateway `runner.rs`) — replaces direct
`MemoryStore::new()` calls.

```rust
// Source: [ASSUMED — follows pattern of SkillRegistry::load() factory]
pub fn build_memory_provider(config: &MemoryConfig) -> anyhow::Result<Box<dyn MemoryProvider + Send>> {
    match config.provider.as_str() {
        "file" => {
            let memory_dir = get_hermes_home().join(MEMORIES_DIR);
            Ok(Box::new(MemoryStore::new(memory_dir)))
        }
        other => anyhow::bail!(
            "Unknown memory provider '{}'. Available providers: file. \
             Future providers (sqlite, grafeo, duckdb) require feature flags.",
            other
        ),
    }
}
```

### Pattern 4: Config Extension

**What:** Add `MemoryConfig` to the top-level `Config` struct.
**When to use:** Follows the established pattern (each subsystem has its own `XxxConfig`).

```rust
// Source: [VERIFIED: crates/ironhermes-core/src/config.rs — SkillsConfig, ExecConfig patterns]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Provider name. Default: "file". Future: "sqlite", "grafeo", "duckdb".
    pub provider: String,
    /// Provider-specific settings keyed by provider name.
    #[serde(default)]
    pub sqlite: SqliteMemoryConfig,
    pub grafeo: GrafeoMemoryConfig,
    pub duckdb: DuckdbMemoryConfig,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self { provider: "file".to_string(), ... }
    }
}
```

All sub-structs default to empty/None so existing `config.yaml` files without a
`memory:` section parse correctly via `#[serde(default)]`.

### Pattern 5: Call Site Migration

**What:** Five locations pass `Arc<Mutex<MemoryStore>>` today — change to
`Arc<Mutex<dyn MemoryProvider + Send>>`.

**Call sites (VERIFIED from grep):**

| File | Current | After |
|------|---------|-------|
| `ironhermes-tools/src/registry.rs:225` | `register_memory_tool(store: Arc<Mutex<MemoryStore>>)` | `register_memory_tool(store: Arc<Mutex<dyn MemoryProvider + Send>>)` |
| `ironhermes-agent/src/prompt_builder.rs:52` | `set_memory_store(store: Arc<Mutex<MemoryStore>>)` | `set_memory_store(store: Arc<Mutex<dyn MemoryProvider + Send>>)` |
| `ironhermes-gateway/src/handler.rs:78` | `set_memory_store(store: Arc<Mutex<MemoryStore>>)` | same → trait object |
| `ironhermes-gateway/src/runner.rs:50` | `set_memory_store(store: Arc<Mutex<MemoryStore>>)` | same → trait object |
| `ironhermes-cli/src/main.rs:472` | `Arc::new(Mutex::new(store))` | `Arc::new(Mutex::new(build_memory_provider(config)?))` |

[VERIFIED: grep of crates/ — 5 sites confirmed]

### Anti-Patterns to Avoid

- **Cloning `MemoryStore` into the trait object:** MemoryStore is not Clone and should
  not become Clone. The `Arc<Mutex<dyn MemoryProvider + Send>>` sharing pattern is
  identical to what exists today.
- **Making the trait `dyn`-safe with native async:** Native `async fn in trait` (RPITIT)
  is not dyn-compatible without `impl Trait` return types; `#[async_trait]` erases this
  via boxing and IS dyn-compatible. D-01 is already correct.
- **Changing MemoryStore's sync public API:** `add()`, `replace()`, `remove()` must stay
  synchronous and unchanged. Only the new `MemoryProvider` impl wraps them asynchronously.
  All existing tests exercise the sync API and must continue to pass.
- **Putting the trait in `ironhermes-tools`:** MemoryProvider belongs in `ironhermes-core`
  (the leaf crate). Tools and agent both depend on core; placing the trait in tools would
  create a circular dependency for agent.

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| async fn in trait objects | native RPITIT + Box<dyn ...> workaround | `#[async_trait]` 0.1.89 | Already in workspace; proven dyn-compatible; project-wide convention |
| Concurrent file writes | custom locking | `fs2::FileExt` (existing `with_file_lock`) | Already handles `flock` sidecar pattern; all MemoryStore mutations go through it |
| Config struct parsing | manual YAML parsing | `serde` + `serde_yaml` + `#[serde(default)]` | Backward compat: existing config.yaml files without `memory:` key must parse cleanly |
| Provider validation at startup | runtime string matching | Compile-time exhaustive match in factory | Hard error on unknown provider string; clear message; no silent fallback |

**Key insight:** The entire complexity of this phase is a refactoring, not a feature build.
No new algorithms needed. The risk is accidentally breaking existing MemoryStore behavior
or test isolation. The plan must verify that `cargo test --package ironhermes-core` and
`cargo test --package ironhermes-tools` remain green after every sub-task.

## Common Pitfalls

### Pitfall 1: `dyn MemoryProvider` requires `Send` in the bound

**What goes wrong:** `Arc<Mutex<dyn MemoryProvider>>` fails to compile across `tokio::spawn`
boundaries because `dyn Trait` is not `Send` unless `Send` is in the bound.
**Why it happens:** `Arc<Mutex<dyn Trait + Send>>` vs `Arc<Mutex<dyn Trait>>` — the former
satisfies `Send`, the latter does not.
**How to avoid:** Declare as `Arc<Mutex<dyn MemoryProvider + Send>>` at every call site.
The trait definition includes `Send + Sync + 'static` (D-02), so all implementations
automatically satisfy this.
**Warning signs:** Compiler error "dyn MemoryProvider cannot be sent between threads safely".

### Pitfall 2: `#[async_trait]` and `&mut self` lifecycle hooks

**What goes wrong:** `initialize(&mut self, ...)` and `shutdown(&mut self)` require
exclusive access; if the provider is stored behind `Arc<Mutex<...>>`, callers must hold
the lock for the duration of the async call.
**Why it happens:** `async_trait` boxes the future, which must satisfy `Send`. A `MutexGuard`
across an await point is not `Send`.
**How to avoid:** Call `initialize` and `shutdown` before placing the provider behind
`Arc<Mutex<>>`. Initialize the boxed provider, call `initialize()`, then wrap in
`Arc::new(Mutex::new(...))`. Use `tokio::sync::Mutex` if lock must span an await; use
`std::sync::Mutex` (existing pattern) and call `initialize` synchronously before wrapping.
**Warning signs:** Compiler error "future cannot be sent between threads safely, `MutexGuard` held across await".

### Pitfall 3: MemoryStore private `entries` field

**What goes wrong:** `MemoryProvider::prefetch()` needs to return current in-memory
entries from `MemoryStore`, but `entries: HashMap<MemoryTarget, Vec<String>>` is private.
**Why it happens:** `MemoryStore` was designed with encapsulation; `prefetch` is a new
access pattern.
**How to avoid:** Add a `pub(crate)` or `pub` getter `fn entries_for(&self, target: MemoryTarget) -> &[String]`
in `memory_store.rs`, or make `MemoryProvider::prefetch` for the file backend simply call
`load_from_disk` equivalent. The simplest approach: `prefetch` for file provider calls
`reload_target` for both targets and returns the result — since the file provider's
lifecycle means `initialize` already calls `load_from_disk`, `prefetch` can re-use the
loaded state. Either way, no test breakage since tests use the public `add/replace/remove`
API.
**Warning signs:** Compile error "field `entries` of struct `MemoryStore` is private".

### Pitfall 4: Snapshot field not exposed to MemoryEntries

**What goes wrong:** `PromptBuilder::set_memory_store` reads `format_for_system_prompt`
(which accesses `self.snapshot`). After migration to `dyn MemoryProvider`, prompt builder
needs a way to get formatted prompt snippets.
**Why it happens:** `format_for_system_prompt` is a concrete method on `MemoryStore`, not
on the new trait.
**How to avoid:** Either add `format_for_prompt(&self, entries: &MemoryEntries, target: MemoryTarget) -> Option<String>`
to the `MemoryProvider` trait, OR keep `format_for_system_prompt` as a standalone free
function that operates on `&MemoryEntries`. The latter avoids coupling the trait to
formatting concerns and is the cleaner design.
**Warning signs:** Compile error in `prompt_builder.rs` after migration — "method not found in `dyn MemoryProvider`".

### Pitfall 5: serde backward compatibility for MemoryConfig

**What goes wrong:** Existing `config.yaml` files in production have no `memory:` key.
Adding a required field causes `serde_yaml::from_str` to fail.
**Why it happens:** serde default must be explicitly declared.
**How to avoid:** `#[serde(default)]` on `MemoryConfig` and on `Config::memory`. This is
the established pattern — `SkillsConfig`, `ExecConfig`, `SubagentConfig` all use it.
**Warning signs:** Test `test_config_parses_without_memory_section` fails.

### Pitfall 6: Provider-specific sub-config structs bloat compile time

**What goes wrong:** Defining `SqliteMemoryConfig`, `GrafeoMemoryConfig`, `DuckdbMemoryConfig`
in core unconditionally adds serde fields for providers that won't exist until Phase 17.
**Why it happens:** Premature struct definitions for future providers.
**How to avoid:** Use `HashMap<String, serde_json::Value>` as the provider-specific extra
settings field in `MemoryProviderConfig` (the config passed to `initialize()`). Config
parsing stays flat; provider-specific parsing happens inside each future provider's
`initialize()` method. No per-provider structs in core until Phase 17.
**Warning signs:** Phase 17 needing to break the `MemoryConfig` struct shape.

## Code Examples

### Full trait object usage pattern

```rust
// Source: [VERIFIED: registry.rs pattern + ASSUMED composition]
// In main.rs / runner.rs — AFTER this phase:
let provider_config = MemoryProviderConfig {
    provider: config.memory.provider.clone(),
    memory_dir: get_hermes_home().join(MEMORIES_DIR),
    memory_char_limit: MEMORY_CHAR_LIMIT,
    user_char_limit: USER_CHAR_LIMIT,
    extra: HashMap::new(),
};

let mut provider = build_memory_provider(&config.memory)?;
provider.initialize(&provider_config).await?;  // fatal if fails (D-11)

let memory_store: Arc<Mutex<dyn MemoryProvider + Send>> =
    Arc::new(Mutex::new(provider));

registry.register_memory_tool(memory_store.clone());
```

### Graceful error handling for non-fatal hooks

```rust
// Source: [ASSUMED — follows D-12 error semantics]
// In agent loop or gateway session end handler:
{
    let store = memory_store.lock().unwrap();
    if let Err(e) = store.on_session_end(session_id, &entries).await {
        tracing::warn!("Memory provider on_session_end failed (ignored): {e}");
    }
}
```

### Config YAML shape (after phase)

```yaml
# ~/.ironhermes/config.yaml
memory:
  provider: file
  # No additional config needed for file provider.
  # Future providers:
  # sqlite:
  #   path: ~/.ironhermes/memory.db
  # grafeo:
  #   url: http://localhost:7474
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Concrete `Arc<Mutex<MemoryStore>>` everywhere | `Arc<Mutex<dyn MemoryProvider + Send>>` | This phase | Enables Phase 17 external providers |
| Direct `MemoryStore::new()` at startup | `build_memory_provider(&config.memory)?` factory | This phase | Config-driven backend selection |
| No memory config section | `memory.provider` key in config.yaml | This phase | Backward compat via `#[serde(default)]` |

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `format_for_prompt` as a standalone free function (not trait method) is cleaner | Architecture Patterns / Pattern 1 | Low — either approach works; planner should decide |
| A2 | Provider-specific sub-configs use `HashMap<String, Value>` extra field rather than named structs | Common Pitfalls #6 | Low — named structs are an alternative; Phase 17 will expand regardless |
| A3 | `initialize` called before wrapping in `Arc<Mutex<>>` to avoid `MutexGuard` across await | Common Pitfalls #2 | Medium — if provider must be wrapped first, use `tokio::sync::Mutex` instead |
| A4 | `MemoryStore::entries` access via a new getter method is the right refactor path | Common Pitfalls #3 | Low — alternative is to restructure `prefetch` to re-read from disk |

## Open Questions

1. **Should `format_for_system_prompt` move to the trait or become a free function?**
   - What we know: `PromptBuilder` currently calls it on `&MemoryStore`; after migration it
     needs to work on `dyn MemoryProvider`.
   - What's unclear: Whether future network-backed providers need custom prompt formatting
     (they might surface different metadata).
   - Recommendation: Make it a standalone function `format_entries_for_prompt(entries: &MemoryEntries, target: MemoryTarget) -> Option<String>` operating on `MemoryEntries`. Keeps the trait minimal.

2. **`std::sync::Mutex` or `tokio::sync::Mutex` for the shared provider?**
   - What we know: Current codebase uses `std::sync::Mutex` for `MemoryStore`. The async
     hooks should not be held under a lock across an await point.
   - What's unclear: If `sync_turn` / `on_session_end` need to be called while the lock
     is still held (to pass entries), this becomes `MutexGuard + await = compile error`.
   - Recommendation: Take a snapshot of entries outside the lock, drop the lock, then
     call the async hooks. This is the same frozen-snapshot pattern already in use.

## Environment Availability

Step 2.6: No external dependencies. This phase is purely Rust code changes — no new
tools, services, CLIs, runtimes, or databases required. All compilation happens via
`cargo build` which is confirmed working (Rust 1.94 stable, edition 2024).

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | none (Cargo.toml `[dev-dependencies]`) |
| Quick run command | `cargo test --package ironhermes-core` |
| Full suite command | `cargo test --package ironhermes-core --package ironhermes-tools --package ironhermes-agent` |

**Baseline (VERIFIED):** `cargo test --package ironhermes-core` — 99 passed, 0 failed.

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| MEM-07 | MemoryProvider trait compiles with Send+Sync+'static | unit | `cargo test --package ironhermes-core` | Wave 0 |
| MEM-07 | All 5 lifecycle hooks are async fn | unit | `cargo test --package ironhermes-core` | Wave 0 |
| MEM-08 | MemoryStore implements MemoryProvider | unit | `cargo test --package ironhermes-core` | Wave 0 |
| MEM-08 | All existing MemoryStore tests pass unchanged | regression | `cargo test --package ironhermes-core memory_store` | ✅ exists |
| MEM-08 | prefetch returns correct entries after load_from_disk | unit | `cargo test --package ironhermes-core` | Wave 0 |
| MEM-12 | Config `memory.provider = "file"` selects file backend | unit | `cargo test --package ironhermes-core config` | Wave 0 |
| MEM-12 | Unknown provider name returns hard error | unit | `cargo test --package ironhermes-core` | Wave 0 |
| MEM-12 | Config without `memory:` key parses with default "file" | unit | `cargo test --package ironhermes-core config` | Wave 0 |
| — | MemoryTool compiles with `dyn MemoryProvider` | compile | `cargo build --package ironhermes-tools` | ✅ will verify |
| — | PromptBuilder compiles with `dyn MemoryProvider` | compile | `cargo build --package ironhermes-agent` | ✅ will verify |

### Sampling Rate

- **Per task commit:** `cargo test --package ironhermes-core`
- **Per wave merge:** `cargo test --package ironhermes-core --package ironhermes-tools --package ironhermes-agent`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-core/src/memory_provider.rs` — new file; needs tests for:
  - `MemoryEntries` default is empty
  - `MemoryProviderConfig` serializes/deserializes cleanly
  - `build_memory_provider("file")` returns Ok
  - `build_memory_provider("unknown")` returns Err with provider name in message
  - `MemoryStore` as `dyn MemoryProvider`: initialize + prefetch round-trip
- [ ] `crates/ironhermes-core/src/config.rs` — extend existing config tests:
  - `test_config_parses_without_memory_section` (backward compat)
  - `test_memory_config_default_is_file`

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes (inherited) | `scan_context_content()` already called in MemoryStore; trait wrapping does not bypass it |
| V6 Cryptography | no | — |

**Security note:** The `MemoryProvider` trait is a structural abstraction. It does not
introduce new input surfaces. The existing security scanning in `MemoryStore::add()` and
`replace()` is called before data reaches the provider hooks, so `sync_turn` and
`on_session_end` receive already-scanned entries. Future external providers (Phase 17)
must enforce their own input validation — that is a Phase 17 concern.

## Sources

### Primary (HIGH confidence)

- [VERIFIED: crates/ironhermes-core/src/memory_store.rs] — Full MemoryStore implementation, fields, methods, tests
- [VERIFIED: crates/ironhermes-tools/src/registry.rs] — Tool trait pattern (async_trait + Send + Sync), ToolRegistry
- [VERIFIED: crates/ironhermes-core/src/config.rs] — Config struct, serde default pattern, all existing sub-configs
- [VERIFIED: crates/ironhermes-tools/src/memory_tool.rs] — MemoryTool wiring, Arc<Mutex<MemoryStore>>
- [VERIFIED: crates/ironhermes-cli/src/main.rs grep] — 5 confirmed call sites for MemoryStore
- [VERIFIED: crates/ironhermes-gateway/src/runner.rs grep] — runner.rs wiring
- [VERIFIED: Cargo.lock] — async-trait 0.1.89
- [VERIFIED: rustc --version] — Rust 1.94 stable (edition 2024)
- [VERIFIED: cargo test] — 99 tests pass on ironhermes-core baseline

### Secondary (MEDIUM confidence)

- [VERIFIED: .planning/codebase/ARCH.md] — Crate dependency graph; ironhermes-core is the correct placement for new trait

### Tertiary (LOW confidence)

- None

## Metadata

**Confidence breakdown:**

- Standard stack: HIGH — all dependencies already in workspace, confirmed in Cargo.lock
- Architecture: HIGH — trait pattern directly mirrors existing Tool/PlatformAdapter in codebase
- Pitfalls: HIGH — identified from direct code inspection of affected files
- Config pattern: HIGH — four prior config subsections use identical serde pattern
- Call site migration: HIGH — all 5 sites confirmed by grep

**Research date:** 2026-04-11
**Valid until:** 2026-05-11 (stable tech, no fast-moving dependencies)
