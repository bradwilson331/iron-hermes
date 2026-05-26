---
phase: 20
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - crates/ironhermes-core/src/memory_provider.rs
  - crates/ironhermes-core/src/config_schema.rs
  - crates/ironhermes-core/src/lib.rs
  - crates/ironhermes-core/src/config.rs
  - providers/memory-sqlite/src/lib.rs
  - providers/memory-duckdb/src/lib.rs
  - providers/memory-grafeo/src/lib.rs
  - crates/ironhermes-agent/src/memory/factory.rs
  - crates/ironhermes-agent/src/memory_flush_handler.rs
  - crates/ironhermes-cli/src/main.rs
autonomous: true
requirements: [MEM-07, MEM-08, MEM-10, MEM-11]
must_haves:
  truths:
    - "MemoryProvider trait declares all 11 new hook methods plus the reshaped async initialize"
    - "MemoryProviderConfig no longer exists anywhere in the workspace"
    - "ConfigField and MemoryAction types exist in ironhermes-core and are re-exported"
    - "build_memory_provider is async, calls provider.initialize(...).await, then load_from_disk(), then evaluates is_available"
    - "Factory falls back to the file provider (with tracing::warn) when the selected external provider reports is_available=false"
    - "Sqlite, DuckDB, and Grafeo provider crates compile under the new initialize signature"
    - "Factory persistence round-trip: provider built -> add entry -> drop -> rebuild via factory at same HERMES_HOME -> entry visible via format_for_system_prompt"
  artifacts:
    - path: "crates/ironhermes-core/src/memory_provider.rs"
      provides: "MemoryProvider trait with enriched hook surface, new async initialize signature, no MemoryProviderConfig"
      contains: "fn name(&self) -> &'static str"
    - path: "crates/ironhermes-core/src/config_schema.rs"
      provides: "ConfigField struct and MemoryAction enum"
      contains: "pub struct ConfigField"
    - path: "crates/ironhermes-agent/src/memory/factory.rs"
      provides: "async factory that runs initialize + load_from_disk + is_available fallback"
      contains: "async fn build_memory_provider"
  key_links:
    - from: "crates/ironhermes-agent/src/memory/factory.rs"
      to: "MemoryProvider::initialize"
      via: "await call after construction, before load_from_disk"
      pattern: "provider\\.initialize\\([^)]*\\)\\.await"
    - from: "crates/ironhermes-agent/src/memory/factory.rs"
      to: "MemoryProvider::load_from_disk"
      via: "unconditional call for every provider arm (closes Fix 1)"
      pattern: "load_from_disk\\(\\)"
    - from: "crates/ironhermes-cli/src/main.rs"
      to: "build_memory_provider"
      via: ".await at every call site (run_gateway, run_chat, run_single)"
      pattern: "build_memory_provider\\([^)]*\\)\\.await"
---

<objective>
Land the trait-shape change and factory persistence fix as a single atomic plan. This plan is breaking: `initialize` signature flips to `async fn initialize(&mut self, session_id: &str, hermes_home: &Path, provider_config: &Value) -> anyhow::Result<()>` and `MemoryProviderConfig` is deleted from the workspace. Every current provider crate (file/sqlite/duckdb/grafeo) is migrated in this plan — no compat shim (per D-10, D-20). Adds defaulted trait methods (`name` required; 10 others defaulted per D-01..D-05, D-11..D-14), introduces `ConfigField` + `MemoryAction` in `ironhermes-core/src/config_schema.rs` (D-06), adds optional `memory.mirror_provider` to `MemoryConfig` (D-27), fixes the pending-todo "gateway memory does not persist across restart" bug by making the factory async, calling `initialize` then `load_from_disk` for every provider, and evaluating `is_available` with file-provider fallback (D-16, D-17). Regression tests cover the sqlite round-trip (mandatory) plus feature-gated duckdb and grafeo round-trips.

Purpose: Brings the Rust `MemoryProvider` trait to API parity with hermes-agent's Python `MemoryProvider` ABC without introducing runtime plugin loading (compile-time features stay — per PROJECT.md:52). Unblocks Plans 20-02, 20-03, 20-04.
Output: Trait + four provider impls + async factory + one new module (`config_schema.rs`) + async wiring at the three CLI call sites.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md
@.planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md
@.planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md

<interfaces>
<!-- Current trait shape (ironhermes-core/src/memory_provider.rs) — target of rewrite. -->
```rust
// Current (to be replaced):
#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    async fn initialize(&mut self, config: &MemoryProviderConfig) -> anyhow::Result<()>;
    async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries>;
    async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
    async fn on_session_end(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;
    fn load_from_disk(&mut self) -> anyhow::Result<()>;
    fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult;
    fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult;
    fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult;
    fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>;
    fn to_memory_entries(&self) -> MemoryEntries;
}
```

<!-- ToolSchema already exists in ironhermes-core (re-exported). Reused by get_tool_schemas. -->
From `crates/ironhermes-core/src/types.rs`:
```rust
pub struct ToolSchema { /* ... name/description/parameters ... */ }
```

<!-- MemoryTarget enum already defined in memory_store.rs. -->
From `crates/ironhermes-core/src/memory_store.rs`:
```rust
pub enum MemoryTarget { Memory, User }
pub type MemoryResult = anyhow::Result<MemoryWriteOutcome>;
```

<!-- The provider factory currently returns Arc<Mutex<dyn MemoryProvider + Send>>. -->
<!-- That sharing shape is preserved; only the function becomes `async fn`. -->
</interfaces>
</context>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| CLI user -> factory | `HERMES_HOME` path and `provider_config: &Value` (loaded from `$HERMES_HOME/<provider>.json` in future wizard calls) cross here. Provider-declared `name()` is `&'static str` (literal), so safe. |
| Factory -> provider | `initialize(session_id, hermes_home, &Value)` — `hermes_home` must not be interpreted as attacker-controlled (it's derived from `constants::get_hermes_home()`), but `provider_config` could contain untrusted path-like strings. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-20-01 | Tampering | `initialize(hermes_home: &Path, provider_config: &Value)` in all four provider impls | mitigate | Provider impls that consume a path from `provider_config` (e.g., future sqlite `db_path` override) MUST call `std::path::Path::new(s).canonicalize()` and assert `canonical.starts_with(hermes_home)` before use. Default file-provider `initialize` is a no-op so inherits no surface. Document the contract in a module-level comment on `memory_provider.rs`. |
| T-20-04 | Tampering | `save_config` default impl (JSON file path construction) | mitigate | Default trait impl for `save_config` must `debug_assert!(!self.name().contains(['/', '\\', '.']))` before `hermes_home.join(format!("{}.json", self.name()))`. `name()` returns `&'static str`, but the assert prevents a future provider from accidentally returning a `format!()`-built name. |

High-severity threats: T-20-01 is the only one that touches Plan 20-01. The mitigation is a module-level comment + enforcement in each provider's `initialize` body where applicable. Sqlite/duckdb/grafeo today take `db_path` from `Provider::new(...)` (not `initialize`), so Plan 20-01 only needs to document the contract; Plan 20-04 enforces it when providers actually read paths from `provider_config`.
</threat_model>

<tasks>

<task type="auto" tdd="true">
  <name>Task 20-01-01: Introduce config_schema module + enrich trait + delete MemoryProviderConfig + migrate MemoryStore file-provider impl</name>
  <read_first>
    - crates/ironhermes-core/src/memory_provider.rs (current trait + MemoryStore impl — entire file)
    - crates/ironhermes-core/src/memory_store.rs (MemoryStore struct + MemoryTarget/MemoryResult — entire file)
    - crates/ironhermes-core/src/types.rs (ToolSchema — lines 94-118)
    - crates/ironhermes-core/src/skills.rs (lines 85-107 — SkillConfigField prior-art for serde pattern)
    - crates/ironhermes-core/src/lib.rs (module declarations + pub use list)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-01..D-15, D-27)
    - External-reference-only (do NOT attempt to open — cited for API parity): hermes-agent/agent/memory_provider.py
  </read_first>
  <files>
    crates/ironhermes-core/src/config_schema.rs (NEW),
    crates/ironhermes-core/src/memory_provider.rs (rewrite),
    crates/ironhermes-core/src/lib.rs (add `pub mod config_schema;` + `pub use config_schema::{ConfigField, MemoryAction};`),
    crates/ironhermes-core/src/config.rs (add `pub mirror_provider: Option<String>` to `MemoryConfig`)
  </files>
  <behavior>
    - Test: `ConfigField` with all fields populated serializes to JSON and round-trips via serde_json (key/description/secret/required/default/choices/env_var/url).
    - Test: `MemoryAction::Add | Replace | Remove` serializes to lowercase strings `"add" | "replace" | "remove"`.
    - Test: Trait default methods on a minimal test-local provider:
        - `name()` is required (no default — compile fails if missing; implementer writes `fn name(&self) -> &'static str { "test" }`).
        - `is_available(&self) -> bool` default returns `true`.
        - `unavailable_reason(&self) -> Option<String>` default returns `None`.
        - `get_tool_schemas(&self) -> Vec<ToolSchema>` default returns a non-empty Vec containing exactly the three memory-action schemas (add/replace/remove), matching today's shape.
        - `handle_tool_call(&mut self, name: &str, args: serde_json::Value) -> MemoryResult` default dispatches on `name` ("memory_add"/"memory_replace"/"memory_remove") to `self.add/replace/remove` by parsing args for `target` and `content`/`old_text`/`new_content`. Unknown name returns `Err(anyhow!("unknown memory tool: {name}"))`.
        - `get_config_schema(&self) -> Vec<ConfigField>` default returns `vec![]`.
        - `save_config(&self, values: &HashMap<String, serde_json::Value>, hermes_home: &Path) -> anyhow::Result<()>` default is no-op (`Ok(())`) — providers override to write JSON.
        - `system_prompt_block(&self) -> Option<String>` default returns `None`.
        - `async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()>` default is `Ok(())`.
        - `async fn on_pre_compress(&self, _messages: &[ironhermes_core::types::ChatMessage]) -> anyhow::Result<()>` default is `Ok(())`.
        - `async fn on_memory_write(&mut self, _action: MemoryAction, _target: MemoryTarget, _content: &str) -> anyhow::Result<()>` default is `Ok(())`.
    - Test: `MemoryStore` (file provider) `name()` returns `"file"`; `is_available()` returns `true`; new `initialize(session_id, hermes_home, provider_config)` is a no-op and returns `Ok(())` (per Pitfall 5 — file provider stays constructed by `new(memory_dir)`).
    - Test: `MemoryConfig::default().mirror_provider` is `None`.
    - Negative compile check (documented, not automated): `grep -R "MemoryProviderConfig" crates/ providers/` returns zero results after this task.
  </behavior>
  <action>
    1. CREATE `crates/ironhermes-core/src/config_schema.rs` with EXACTLY:

    ```rust
    //! Config schema types for memory (and future) provider plugins (D-06).
    //!
    //! `ConfigField` mirrors the hermes-agent plugin contract 1:1 so providers
    //! can describe their own configuration surface to setup wizards.
    //! `MemoryAction` is the small enum fired via `MemoryProvider::on_memory_write`
    //! so a mirror subscriber can observe primary writes without owning the
    //! dispatch semantics.

    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
    pub enum MemoryAction {
        Add,
        Replace,
        Remove,
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn config_field_roundtrip_all_fields() {
            let f = ConfigField {
                key: "API_KEY".into(),
                description: Some("Service key".into()),
                secret: true,
                required: true,
                default: Some(serde_json::json!("sk-...")),
                choices: Some(vec!["a".into(), "b".into()]),
                env_var: Some("MY_API_KEY".into()),
                url: Some("https://example.com".into()),
            };
            let s = serde_json::to_string(&f).unwrap();
            let back: ConfigField = serde_json::from_str(&s).unwrap();
            assert_eq!(f, back);
        }

        #[test]
        fn memory_action_lowercase_serde() {
            assert_eq!(serde_json::to_string(&MemoryAction::Add).unwrap(), "\"add\"");
            assert_eq!(serde_json::to_string(&MemoryAction::Replace).unwrap(), "\"replace\"");
            assert_eq!(serde_json::to_string(&MemoryAction::Remove).unwrap(), "\"remove\"");
            let a: MemoryAction = serde_json::from_str("\"add\"").unwrap();
            assert_eq!(a, MemoryAction::Add);
        }
    }
    ```

    2. REWRITE `crates/ironhermes-core/src/memory_provider.rs` ENTIRELY. New shape:

    ```rust
    //! MemoryProvider trait and supporting types for pluggable memory backends.
    //!
    //! MEM-07: Trait with Send + Sync + 'static bounds and async lifecycle hooks.
    //! MEM-08: MemoryStore implements the trait as the default file-based backend.
    //!
    //! Phase 20 (D-01..D-15): Enriched hook surface — `name`, `is_available`,
    //! `unavailable_reason`, `get_tool_schemas`, `handle_tool_call`,
    //! `get_config_schema`, `save_config`, `system_prompt_block`, `queue_prefetch`,
    //! `on_pre_compress`, `on_memory_write`. `initialize` is a breaking signature
    //! change (D-10); `MemoryProviderConfig` is deleted.
    //!
    //! Security contract (T-20-01): providers that consume path-like strings
    //! from `provider_config: &Value` MUST canonicalize and assert
    //! `starts_with(hermes_home)` before use. The default file-provider
    //! `initialize` is a no-op and inherits no surface.

    use std::collections::HashMap;
    use std::path::Path;

    use async_trait::async_trait;
    use serde_json::Value;

    use crate::config_schema::{ConfigField, MemoryAction};
    use crate::memory_store::{MemoryResult, MemoryStore, MemoryTarget};
    use crate::types::{ChatMessage, ToolSchema};

    // =============================================================================
    // MemoryEntries wrapper
    // =============================================================================

    #[derive(Debug, Clone, Default)]
    pub struct MemoryEntries {
        pub entries: HashMap<MemoryTarget, Vec<String>>,
    }

    // =============================================================================
    // Default tool schemas (used by `get_tool_schemas` default impl)
    // =============================================================================

    fn default_memory_tool_schemas() -> Vec<ToolSchema> {
        // Returns the three-action schema set matching today's behavior. Providers
        // that surface extra tools (e.g. memory_search) override `get_tool_schemas`.
        // Construction mirrors the existing `MemoryTool` registration shape so the
        // default stays wire-compatible.
        // EXECUTOR: consult crates/ironhermes-tools/src/memory_tool.rs for the
        // canonical schema values when filling in this helper.
        vec![]
    }

    // =============================================================================
    // MemoryProvider trait (MEM-07) — enriched surface
    // =============================================================================

    #[async_trait]
    pub trait MemoryProvider: Send + Sync + 'static {
        // ---- Identity (D-02, D-03) ----
        /// Stable provider identifier used for config file naming and logs.
        /// Must be a filename-safe literal — no slashes, dots, or `..`.
        fn name(&self) -> &'static str;

        fn is_available(&self) -> bool { true }
        fn unavailable_reason(&self) -> Option<String> { None }

        // ---- Tool surface (D-04, D-05) ----
        fn get_tool_schemas(&self) -> Vec<ToolSchema> {
            default_memory_tool_schemas()
        }

        fn handle_tool_call(&mut self, name: &str, args: Value) -> MemoryResult {
            // Default dispatch: today's memory tool is a single tool with
            // action arg, but for trait-level parity we also accept the
            // hermes-agent naming `memory_add / memory_replace / memory_remove`.
            let target = parse_target(&args)?;
            match name {
                "memory_add" | "add" => {
                    let content = args.get("content").and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("missing `content`"))?;
                    self.add(target, content)
                }
                "memory_replace" | "replace" => {
                    let old_text = args.get("old_text").and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("missing `old_text`"))?;
                    let new_content = args.get("new_content").and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("missing `new_content`"))?;
                    self.replace(target, old_text, new_content)
                }
                "memory_remove" | "remove" => {
                    let old_text = args.get("old_text").and_then(|v| v.as_str())
                        .ok_or_else(|| anyhow::anyhow!("missing `old_text`"))?;
                    self.remove(target, old_text)
                }
                other => Err(anyhow::anyhow!("unknown memory tool: {other}").into()),
            }
        }

        // ---- Config schema (D-06, D-07) ----
        fn get_config_schema(&self) -> Vec<ConfigField> { vec![] }

        fn save_config(
            &self,
            _values: &HashMap<String, Value>,
            _hermes_home: &Path,
        ) -> anyhow::Result<()> {
            // T-20-04: default guard against accidental traversal in future
            // overrides. `name()` is `&'static str` so this is a programmer
            // safeguard rather than a runtime validation.
            debug_assert!(
                !self.name().contains(['/', '\\']) && !self.name().contains(".."),
                "MemoryProvider::name() must be a filename-safe literal, got: {}",
                self.name()
            );
            Ok(())
        }

        // ---- Prompt integration (D-11) ----
        fn system_prompt_block(&self) -> Option<String> { None }

        // ---- Async lifecycle (D-10, D-12..D-15) ----
        async fn initialize(
            &mut self,
            session_id: &str,
            hermes_home: &Path,
            provider_config: &Value,
        ) -> anyhow::Result<()>;

        async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries>;
        async fn sync_turn(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;

        async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> { Ok(()) }
        async fn on_pre_compress(&self, _messages: &[ChatMessage]) -> anyhow::Result<()> { Ok(()) }
        async fn on_memory_write(
            &mut self,
            _action: MemoryAction,
            _target: MemoryTarget,
            _content: &str,
        ) -> anyhow::Result<()> { Ok(()) }

        async fn on_session_end(&self, session_id: &str, entries: &MemoryEntries) -> anyhow::Result<()>;
        async fn shutdown(&mut self) -> anyhow::Result<()>;

        // ---- Sync operations (unchanged) ----
        fn load_from_disk(&mut self) -> anyhow::Result<()>;
        fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult;
        fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult;
        fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult;
        fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String>;
        fn to_memory_entries(&self) -> MemoryEntries;
    }

    fn parse_target(args: &Value) -> anyhow::Result<MemoryTarget> {
        let raw = args.get("target").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing `target`"))?;
        match raw {
            "memory" => Ok(MemoryTarget::Memory),
            "user" => Ok(MemoryTarget::User),
            other => Err(anyhow::anyhow!("invalid target: {other}")),
        }
    }

    // =============================================================================
    // MemoryProvider impl for MemoryStore (MEM-08) — file-based default
    // =============================================================================

    #[async_trait]
    impl MemoryProvider for MemoryStore {
        fn name(&self) -> &'static str { "file" }

        async fn initialize(
            &mut self,
            _session_id: &str,
            _hermes_home: &Path,
            _provider_config: &Value,
        ) -> anyhow::Result<()> {
            // Pitfall 5: file provider is constructed by `MemoryStore::new(memory_dir)`.
            // Keep `initialize` a no-op to avoid double-construction.
            Ok(())
        }

        async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
            Ok(self.to_memory_entries())
        }

        async fn sync_turn(&self, _session_id: &str, _entries: &MemoryEntries) -> anyhow::Result<()> {
            Ok(())
        }

        async fn on_session_end(
            &self,
            _session_id: &str,
            _entries: &MemoryEntries,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }

        fn load_from_disk(&mut self) -> anyhow::Result<()> { MemoryStore::load_from_disk(self) }
        fn add(&mut self, target: MemoryTarget, content: &str) -> MemoryResult {
            MemoryStore::add(self, target, content)
        }
        fn replace(&mut self, target: MemoryTarget, old_text: &str, new_content: &str) -> MemoryResult {
            MemoryStore::replace(self, target, old_text, new_content)
        }
        fn remove(&mut self, target: MemoryTarget, old_text: &str) -> MemoryResult {
            MemoryStore::remove(self, target, old_text)
        }
        fn format_for_system_prompt(&self, target: MemoryTarget) -> Option<String> {
            MemoryStore::format_for_system_prompt(self, target)
        }
        fn to_memory_entries(&self) -> MemoryEntries {
            MemoryEntries { entries: self.entries().clone() }
        }
    }

    // Note: The `MemoryProviderConfig` struct has been REMOVED in Phase 20 (D-10).
    // Provider-specific config is passed via `initialize(_, _, &Value)`.
    ```

    IMPORTANT: The placeholder `default_memory_tool_schemas` returning `vec![]` is NOT acceptable — fill it in with the three-schema Vec. Open `crates/ironhermes-tools/src/memory_tool.rs` and copy the exact `ToolSchema { name, description, parameters }` values that the existing single `"memory"` tool exposes via its action-based schema; decompose into three discrete schemas named `memory_add`, `memory_replace`, `memory_remove`. If decomposition requires a larger change, surface as `TODO(20-04)` and leave `vec![]` as a conservative default — Plan 20-04 will fill provider-specific schemas; the default never needs to be non-empty because the tool registry already owns a `"memory"` tool.

    3. EDIT `crates/ironhermes-core/src/lib.rs`: add `pub mod config_schema;` near the other `pub mod` lines. Add `pub use config_schema::{ConfigField, MemoryAction};` to the re-export list. Remove any `pub use memory_provider::MemoryProviderConfig;` that may exist.

    4. EDIT `crates/ironhermes-core/src/config.rs`: inside `struct MemoryConfig`, add after the existing `provider` field:
    ```rust
        /// Optional mirror provider (D-27). When set, the factory builds a
        /// secondary provider that receives `on_memory_write` events but does
        /// not serve reads. Preserves MEM-12 (single primary).
        #[serde(default)]
        pub mirror_provider: Option<String>,
    ```
    Update `MemoryConfig::default()` (if it implements `Default` manually) to set `mirror_provider: None`. If it derives `Default`, `Option::default()` already yields `None`.

    5. RUN `cargo check -p ironhermes-core --all-features`. If the `ChatMessage` import fails (it lives at `crates/ironhermes-core/src/types.rs`), verify the `pub use types::ChatMessage;` re-export in `lib.rs`; add it if missing.
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-core --all-features &&
      cargo test -p ironhermes-core config_schema::tests &&
      cargo test -p ironhermes-core memory_provider &&
      ! grep -R "MemoryProviderConfig" crates/ironhermes-core/src/
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "fn name(&self) -> &'static str;" crates/ironhermes-core/src/memory_provider.rs` (required method declared without default).
    - `grep -q "async fn initialize" crates/ironhermes-core/src/memory_provider.rs && grep -q "hermes_home: &Path" crates/ironhermes-core/src/memory_provider.rs && grep -q "provider_config: &Value" crates/ironhermes-core/src/memory_provider.rs` (new signature).
    - `grep -q "fn on_memory_write" crates/ironhermes-core/src/memory_provider.rs && grep -q "MemoryAction" crates/ironhermes-core/src/memory_provider.rs`.
    - `grep -q "pub struct ConfigField" crates/ironhermes-core/src/config_schema.rs`.
    - `grep -q "pub enum MemoryAction" crates/ironhermes-core/src/config_schema.rs`.
    - `grep -q "pub mod config_schema" crates/ironhermes-core/src/lib.rs`.
    - `grep -q "pub mirror_provider: Option<String>" crates/ironhermes-core/src/config.rs`.
    - `! grep -q "MemoryProviderConfig" crates/ironhermes-core/src/memory_provider.rs` (struct gone from its defining file).
    - `cargo check -p ironhermes-core --all-features` exits 0.
    - `cargo test -p ironhermes-core config_schema::tests::config_field_roundtrip_all_fields memory_action_lowercase_serde` exits 0 (both tests present and passing).
  </acceptance_criteria>
  <done>
    Trait enriched; MemoryProviderConfig deleted from ironhermes-core; ConfigField/MemoryAction available via `use ironhermes_core::{ConfigField, MemoryAction};`; MemoryConfig carries the optional mirror_provider field; ironhermes-core compiles clean under `--all-features`.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-01-02: Migrate three external provider crates (sqlite/duckdb/grafeo) to the new initialize signature and purge MemoryProviderConfig</name>
  <read_first>
    - providers/memory-sqlite/src/lib.rs (current impl — lines 1-130 for construction + initialize; lines 560-660 for existing tests)
    - providers/memory-duckdb/src/lib.rs (current impl — lines 1-170 for construction + initialize; lines 380-450 for tests)
    - providers/memory-grafeo/src/lib.rs (current impl — lines 1-200 for construction + initialize; lines 590-700 for tests)
    - crates/ironhermes-core/src/memory_provider.rs (post-Task 20-01-01 trait)
    - crates/ironhermes-agent/src/memory_flush_handler.rs (any test-mock impls of MemoryProvider — lines 40-90)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-02, D-10, D-20)
  </read_first>
  <files>
    providers/memory-sqlite/src/lib.rs,
    providers/memory-duckdb/src/lib.rs,
    providers/memory-grafeo/src/lib.rs,
    crates/ironhermes-agent/src/memory_flush_handler.rs (if the test mock impls MemoryProvider and referenced MemoryProviderConfig)
  </files>
  <behavior>
    - Test: each provider crate compiles under `cargo check -p memory-sqlite`, `-p memory-duckdb`, `-p memory-grafeo` after the migration.
    - Test: each provider's existing test suite passes (`cargo test -p memory-sqlite`, etc.).
    - Test: each provider's `name()` returns the expected literal:
        - sqlite -> `"sqlite"`
        - duckdb -> `"duckdb"`
        - grafeo -> `"grafeo"`
      (These are the Plan 20-04 schema targets, but Plan 20-01 requires `name()` as a required method — so implementations land here; only the schemas are deferred to Plan 20-04.)
    - Test: each provider's `initialize` accepts the new `(session_id: &str, hermes_home: &Path, provider_config: &Value)` signature and is a no-op (`Ok(())`) — existing construction via `Provider::new(db_path)` stays unchanged.
    - Audit: `grep -R "MemoryProviderConfig" crates/ providers/` returns zero results after this task.
  </behavior>
  <action>
    For EACH of the three provider crates (sqlite, duckdb, grafeo) do the following:

    1. REMOVE the import line `use ironhermes_core::memory_provider::MemoryProviderConfig;` (or the equivalent `use ironhermes_core::MemoryProviderConfig;`).
    2. ADD (or ensure present):
       ```rust
       use std::path::Path;
       use serde_json::Value;
       ```
    3. REPLACE the existing `initialize` method. Current shape in all three is:
       ```rust
       async fn initialize(&mut self, _config: &MemoryProviderConfig) -> anyhow::Result<()> {
           // existing body (usually a no-op or a self.load_from_disk() call)
       }
       ```
       Change to EXACTLY:
       ```rust
       async fn initialize(
           &mut self,
           _session_id: &str,
           _hermes_home: &Path,
           _provider_config: &Value,
       ) -> anyhow::Result<()> {
           // Existing construction happens in Provider::new(db_path). Provider-specific
           // config derived from `_provider_config` is wired in Plan 20-04 when each
           // provider adopts `get_config_schema`. Phase 20-01 keeps this a no-op.
           Ok(())
       }
       ```
       If the current body does more than `Ok(())` (e.g. sqlite calls `self.load_from_disk()`), REMOVE that call from `initialize` — the factory (Task 20-01-03) will call `load_from_disk` explicitly after `initialize`, so doing it twice is wasted I/O.
    4. ADD a `name()` method directly above `initialize`:
       - sqlite: `fn name(&self) -> &'static str { "sqlite" }`
       - duckdb: `fn name(&self) -> &'static str { "duckdb" }`
       - grafeo: `fn name(&self) -> &'static str { "grafeo" }`
    5. VERIFY the provider's test module. Any test that called `provider.initialize(&MemoryProviderConfig { ... }).await.unwrap();` must be rewritten to:
       ```rust
       let tmp = tempfile::TempDir::new().unwrap();
       provider.initialize("test-session", tmp.path(), &serde_json::Value::Null).await.unwrap();
       ```
       Use `tempfile` (already in workspace per Cargo.toml:85) for `hermes_home`. Keep construction via `Provider::new(db_path)` unchanged.

    6. SEARCH `crates/ironhermes-agent/src/memory_flush_handler.rs` for test-only `impl MemoryProvider for MockXxx` blocks. If any reference `MemoryProviderConfig`, rewrite the `initialize` signature as above and remove the import. Add `fn name(&self) -> &'static str { "mock" }` to each mock impl.

    7. AUDIT: `grep -Rn "MemoryProviderConfig" crates/ providers/` — the result MUST be empty. If any match survives, fix it in this task (do not defer).
  </action>
  <verify>
    <automated>
      cargo check -p memory-sqlite &&
      cargo check -p memory-duckdb &&
      cargo check -p memory-grafeo &&
      cargo test -p memory-sqlite &&
      cargo test -p memory-duckdb &&
      cargo test -p memory-grafeo &&
      ! grep -R "MemoryProviderConfig" crates/ providers/
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q 'fn name(&self) -> &.static str { "sqlite" }' providers/memory-sqlite/src/lib.rs`.
    - `grep -q 'fn name(&self) -> &.static str { "duckdb" }' providers/memory-duckdb/src/lib.rs`.
    - `grep -q 'fn name(&self) -> &.static str { "grafeo" }' providers/memory-grafeo/src/lib.rs`.
    - `grep -cE "async fn initialize\\(" providers/memory-sqlite/src/lib.rs providers/memory-duckdb/src/lib.rs providers/memory-grafeo/src/lib.rs` returns at least one match per file.
    - `! grep -R "MemoryProviderConfig" crates/ providers/` (zero matches workspace-wide).
    - `cargo check -p memory-sqlite -p memory-duckdb -p memory-grafeo` exits 0.
    - `cargo test -p memory-sqlite && cargo test -p memory-duckdb && cargo test -p memory-grafeo` all exit 0.
  </acceptance_criteria>
  <done>
    All three provider crates build + test clean under the new trait; MemoryProviderConfig has zero references anywhere in the workspace; provider `name()` literals are in place for Plan 20-04 to extend with schemas.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-01-03: Make factory async, call initialize + load_from_disk + is_available fallback, and update all three CLI call sites; add round-trip regression test</name>
  <read_first>
    - crates/ironhermes-agent/src/memory/factory.rs (current sync factory — entire file, lines 1-137)
    - crates/ironhermes-agent/src/memory/mod.rs (module exports)
    - crates/ironhermes-cli/src/main.rs (lines 110-130 main dispatch; lines 243-350 run_single; lines 348-500 run_chat; lines 605-700 run_gateway — identify every `build_memory_provider` call site)
    - crates/ironhermes-core/src/memory_provider.rs (new trait — post-Task 20-01-01)
    - crates/ironhermes-core/src/constants.rs (get_hermes_home definition)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md (D-16, D-17, D-24, D-27)
    - .planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md (Pitfall 2, 7; Assumption A6)
  </read_first>
  <files>
    crates/ironhermes-agent/src/memory/factory.rs,
    crates/ironhermes-cli/src/main.rs
  </files>
  <behavior>
    - Test: `build_memory_provider(&cfg("file")).await.is_ok()` — file provider round-trips through the async factory.
    - Test: `build_memory_provider(&cfg("totally-unknown")).await.is_err()` — unknown provider errors with a message naming available providers.
    - Test (feature-gated `memory-sqlite`): `sqlite_round_trip_via_factory` — factory builds sqlite provider, adds an entry via `provider.add(MemoryTarget::Memory, "integration-fact-XYZ")`, drops, rebuilds via factory at the same `HERMES_HOME`, asserts `format_for_system_prompt(MemoryTarget::Memory)` contains `"integration-fact-XYZ"`. This is the D-24 regression for the pending todo Fix 1.
    - Test (feature-gated `memory-duckdb`): analogous round-trip.
    - Test (feature-gated `memory-grafeo`): analogous round-trip.
    - Test: `is_available_false_falls_back_to_file_with_warn` — wire a `FailingMockProvider` whose `is_available()` returns `false` and whose `unavailable_reason()` returns `Some("deliberate test failure".into())`; assert the factory returns the file provider and emits a `tracing::warn!` event (capture via `tracing_test::traced_test` if available, else observe behavior by asserting `provider.name() == "file"` post-fallback). If `tracing_test` is not already in the workspace, skip the log-capture assertion and keep the `name()` assertion — do NOT add a new dev dep.
    - Test: All three CLI call sites (run_gateway, run_chat, run_single) compile with `.await` on `build_memory_provider`.
  </behavior>
  <action>
    1. REWRITE `crates/ironhermes-agent/src/memory/factory.rs` to EXACTLY the following shape (preserving the feature-gating pattern):

    ```rust
    use std::sync::Arc;
    use tokio::sync::Mutex;

    use ironhermes_core::MemoryProvider;
    use ironhermes_core::memory_store::MemoryStore;
    use ironhermes_core::constants::get_hermes_home;

    /// Build a memory provider from config. Returns `Arc<tokio::sync::Mutex<...>>`
    /// for direct use with `MemoryManager` (Plan 20-02) and `MemoryTool`.
    ///
    /// Phase 20 (D-10, D-16, D-17): async factory that
    /// 1. constructs the provider via `Provider::new(db_path)`,
    /// 2. calls `provider.initialize(session_id, hermes_home, provider_config).await`,
    /// 3. calls `provider.load_from_disk()` unconditionally (Fix 1 for the pending
    ///    todo "gateway memory does not persist across restart"),
    /// 4. if `is_available()` returns false, logs `tracing::warn!` with
    ///    `unavailable_reason()` and falls back to the file-based provider.
    ///
    /// Feature-gated per D-16 — external providers require their respective
    /// cargo feature. PROJECT.md:52 — compile-time plugin selection only.
    pub async fn build_memory_provider(
        config: &ironhermes_core::config::MemoryConfig,
    ) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
        let hermes_home = get_hermes_home();
        let provider_config = serde_json::Value::Null; // Plan 20-03 will load
                                                       // `$HERMES_HOME/<name>.json`
                                                       // here; Phase 20-01 passes Null.

        let provider: Arc<Mutex<dyn MemoryProvider + Send>> = match config.provider.as_str() {
            "file" => build_file_provider(&hermes_home).await?,
            #[cfg(feature = "memory-sqlite")]
            "sqlite" => {
                let db_path = hermes_home.join("memory.db");
                let mut p = memory_sqlite::SqliteMemoryProvider::new(&db_path)?;
                p.initialize("factory-boot", &hermes_home, &provider_config).await?;
                p.load_from_disk()?;
                if !p.is_available() {
                    let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                    tracing::warn!(provider = "sqlite", reason = %reason,
                        "memory provider reported is_available=false; falling back to file provider");
                    return build_file_provider(&hermes_home).await;
                }
                Arc::new(Mutex::new(p))
            }
            #[cfg(not(feature = "memory-sqlite"))]
            "sqlite" => {
                anyhow::bail!(
                    "Memory provider 'sqlite' requires the 'memory-sqlite' feature. \
                     Rebuild with: cargo build --features memory-sqlite"
                );
            }
            #[cfg(feature = "memory-duckdb")]
            "duckdb" => {
                let db_path = hermes_home.join("memory_duckdb.db");
                let mut p = memory_duckdb::DuckDbMemoryProvider::new(&db_path)?;
                p.initialize("factory-boot", &hermes_home, &provider_config).await?;
                p.load_from_disk()?;
                if !p.is_available() {
                    let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                    tracing::warn!(provider = "duckdb", reason = %reason,
                        "memory provider reported is_available=false; falling back to file provider");
                    return build_file_provider(&hermes_home).await;
                }
                Arc::new(Mutex::new(p))
            }
            #[cfg(not(feature = "memory-duckdb"))]
            "duckdb" => {
                anyhow::bail!(
                    "Memory provider 'duckdb' requires the 'memory-duckdb' feature. \
                     Rebuild with: cargo build --features memory-duckdb"
                );
            }
            #[cfg(feature = "memory-grafeo")]
            "grafeo" => {
                let db_path = hermes_home.join("memory_graph");
                let mut p = memory_grafeo::GrafeoMemoryProvider::new(&db_path)?;
                p.initialize("factory-boot", &hermes_home, &provider_config).await?;
                p.load_from_disk()?;
                if !p.is_available() {
                    let reason = p.unavailable_reason().unwrap_or_else(|| "unknown".into());
                    tracing::warn!(provider = "grafeo", reason = %reason,
                        "memory provider reported is_available=false; falling back to file provider");
                    return build_file_provider(&hermes_home).await;
                }
                Arc::new(Mutex::new(p))
            }
            #[cfg(not(feature = "memory-grafeo"))]
            "grafeo" => {
                anyhow::bail!(
                    "Memory provider 'grafeo' requires the 'memory-grafeo' feature. \
                     Rebuild with: cargo build --features memory-grafeo"
                );
            }
            other => {
                anyhow::bail!(
                    "Unknown memory provider '{}'. Available providers: file{}{}{}",
                    other,
                    if cfg!(feature = "memory-sqlite") { ", sqlite" } else { "" },
                    if cfg!(feature = "memory-grafeo") { ", grafeo" } else { "" },
                    if cfg!(feature = "memory-duckdb") { ", duckdb" } else { "" }
                );
            }
        };

        Ok(provider)
    }

    async fn build_file_provider(
        hermes_home: &std::path::Path,
    ) -> anyhow::Result<Arc<Mutex<dyn MemoryProvider + Send>>> {
        let memory_dir = hermes_home.join("memories");
        let mut store = MemoryStore::new(memory_dir);
        // initialize is a no-op for file provider but we call it for symmetry.
        store.initialize("factory-boot", hermes_home, &serde_json::Value::Null).await?;
        if let Err(e) = store.load_from_disk() {
            tracing::warn!("Failed to load memory from disk: {}", e);
        }
        Ok(Arc::new(Mutex::new(store)))
    }
    ```

    IMPORTANT Mutex-flavor note (resolves research open question #2): Use `tokio::sync::Mutex` — all MemoryProvider async methods are awaited with the guard held (e.g. `queue_prefetch`, `on_pre_compress`, `on_memory_write`). `std::sync::MutexGuard` cannot cross `.await` safely. This is a plan-wide constraint and propagates through Plans 20-02 and 20-04. The previous `Arc<std::sync::Mutex<...>>` usage at `memory_tool.rs:10` and `memory_tool.rs:212` will migrate to `tokio::sync::Mutex` in Plan 20-02 when the tool delegates to the MemoryManager.

    2. EXTEND the `#[cfg(test)] mod tests` block in `factory.rs`. Replace existing `#[test] fn file_provider_returns_ok` with `#[tokio::test]`; add the fallback test and round-trip tests. Full new test module:

    ```rust
    #[cfg(test)]
    mod tests {
        use super::*;
        use ironhermes_core::config::MemoryConfig;
        use ironhermes_core::memory_store::MemoryTarget;

        fn cfg(provider: &str) -> MemoryConfig {
            let mut c = MemoryConfig::default();
            c.provider = provider.to_string();
            c
        }

        #[tokio::test]
        async fn file_provider_returns_ok() {
            let _tmp = tempfile::TempDir::new().unwrap();
            unsafe { std::env::set_var("HERMES_HOME", _tmp.path()); }
            let result = build_memory_provider(&cfg("file")).await;
            assert!(result.is_ok(), "file provider should build, got {:?}", result.err());
        }

        #[tokio::test]
        async fn unknown_provider_returns_err_with_message() {
            let result = build_memory_provider(&cfg("totally-unknown")).await;
            assert!(result.is_err());
            let msg = result.err().unwrap().to_string();
            assert!(msg.contains("Unknown memory provider 'totally-unknown'"), "got: {msg}");
            assert!(msg.contains("file"), "got: {msg}");
        }

        #[cfg(feature = "memory-sqlite")]
        #[tokio::test]
        async fn sqlite_round_trip_via_factory() {
            // D-24 regression: pending todo Fix 1 — factory must call load_from_disk
            // for external providers so gateway/chat memory persists across restart.
            let tmp = tempfile::TempDir::new().unwrap();
            unsafe { std::env::set_var("HERMES_HOME", tmp.path()); }

            {
                let p = build_memory_provider(&cfg("sqlite")).await.unwrap();
                let mut guard = p.lock().await;
                guard.add(MemoryTarget::Memory, "integration-fact-XYZ").unwrap();
            } // drop provider

            let p2 = build_memory_provider(&cfg("sqlite")).await.unwrap();
            let guard2 = p2.lock().await;
            let block = guard2.format_for_system_prompt(MemoryTarget::Memory)
                .expect("memory block should be populated after reload");
            assert!(block.contains("integration-fact-XYZ"),
                "factory reload lost the entry; block was: {block}");
        }

        #[cfg(feature = "memory-duckdb")]
        #[tokio::test]
        async fn duckdb_round_trip_via_factory() {
            let tmp = tempfile::TempDir::new().unwrap();
            unsafe { std::env::set_var("HERMES_HOME", tmp.path()); }
            {
                let p = build_memory_provider(&cfg("duckdb")).await.unwrap();
                let mut guard = p.lock().await;
                guard.add(MemoryTarget::Memory, "duckdb-fact-XYZ").unwrap();
            }
            let p2 = build_memory_provider(&cfg("duckdb")).await.unwrap();
            let guard2 = p2.lock().await;
            let block = guard2.format_for_system_prompt(MemoryTarget::Memory)
                .expect("duckdb reload should populate");
            assert!(block.contains("duckdb-fact-XYZ"), "got: {block}");
        }

        #[cfg(feature = "memory-grafeo")]
        #[tokio::test]
        async fn grafeo_round_trip_via_factory() {
            let tmp = tempfile::TempDir::new().unwrap();
            unsafe { std::env::set_var("HERMES_HOME", tmp.path()); }
            {
                let p = build_memory_provider(&cfg("grafeo")).await.unwrap();
                let mut guard = p.lock().await;
                guard.add(MemoryTarget::Memory, "grafeo-fact-XYZ").unwrap();
            }
            let p2 = build_memory_provider(&cfg("grafeo")).await.unwrap();
            let guard2 = p2.lock().await;
            let block = guard2.format_for_system_prompt(MemoryTarget::Memory)
                .expect("grafeo reload should populate");
            assert!(block.contains("grafeo-fact-XYZ"), "got: {block}");
        }
    }
    ```

    3. UPDATE every `build_memory_provider` call site in `crates/ironhermes-cli/src/main.rs`:
       - `run_gateway` (approx line 613): currently `build_memory_provider(&config.memory)?;` — change to `build_memory_provider(&config.memory).await?;`.
       - `run_chat` and `run_single`: Plan 20-03 adds wiring; for THIS task, if `build_memory_provider` is already called there, add `.await`. If not yet called, Plan 20-03 will call it with `.await`.
       - Any other call in `crates/ironhermes-agent/` (scan via `grep -rn build_memory_provider crates/`): add `.await` at each site.
       - Any test in any crate that called the sync factory: add `#[tokio::test]` + `.await`.

    4. RUN `grep -rn 'build_memory_provider' crates/ providers/` — every call must be followed by `.await` (or be inside an `async fn` awaiting the result). Ensure all test modules that touch the factory are `#[tokio::test]`.

    5. RUN `cargo clippy -p ironhermes-agent --all-features -- -D warnings` to catch any missed `.await` that manifests as "unused Future must be used" warnings.
  </action>
  <verify>
    <automated>
      cargo check -p ironhermes-agent --all-features &&
      cargo check -p ironhermes-cli --all-features &&
      cargo test -p ironhermes-agent memory::factory::tests &&
      cargo test -p ironhermes-agent --features memory-sqlite memory::factory::tests::sqlite_round_trip_via_factory &&
      cargo clippy -p ironhermes-agent --all-features -- -D warnings
    </automated>
  </verify>
  <acceptance_criteria>
    - `grep -q "pub async fn build_memory_provider" crates/ironhermes-agent/src/memory/factory.rs` (factory is async).
    - `grep -q "tokio::sync::Mutex" crates/ironhermes-agent/src/memory/factory.rs` (tokio Mutex chosen per open question #2).
    - `grep -c "\.initialize(" crates/ironhermes-agent/src/memory/factory.rs` returns at least 4 (file + sqlite + duckdb + grafeo arms).
    - `grep -c "\.load_from_disk()" crates/ironhermes-agent/src/memory/factory.rs` returns at least 3 external-arm matches (D-16 fix) — file arm may call via helper.
    - `grep -q "is_available()" crates/ironhermes-agent/src/memory/factory.rs` AND `grep -q "unavailable_reason" crates/ironhermes-agent/src/memory/factory.rs` AND `grep -q 'falling back to file provider' crates/ironhermes-agent/src/memory/factory.rs` (D-17 implemented).
    - `grep -q "sqlite_round_trip_via_factory" crates/ironhermes-agent/src/memory/factory.rs` (D-24 regression test present).
    - Every `build_memory_provider` callsite in `crates/ironhermes-cli/src/main.rs` is followed by `.await` (run: `grep -n build_memory_provider crates/ironhermes-cli/src/main.rs` — every match must appear in an `.await` context).
    - `cargo test -p ironhermes-agent --features memory-sqlite memory::factory::tests::sqlite_round_trip_via_factory` exits 0.
    - `cargo test -p ironhermes-agent --features memory-duckdb memory::factory::tests::duckdb_round_trip_via_factory` exits 0.
    - `cargo test -p ironhermes-agent --features memory-grafeo memory::factory::tests::grafeo_round_trip_via_factory` exits 0.
    - `cargo check -p ironhermes-cli --all-features` exits 0.
  </acceptance_criteria>
  <done>
    Factory is async; calls `initialize` then `load_from_disk` for every provider; `is_available=false` triggers file-provider fallback with `tracing::warn!`; all CLI call sites have `.await`; sqlite/duckdb/grafeo round-trip tests pass; Fix 1 of the pending todo is closed at the factory layer (chat-mode wiring lands in Plan 20-03).
  </done>
</task>

</tasks>

<verification>
**Full-plan automated verification (run after all three tasks land):**

```bash
cargo check --workspace --all-features &&
cargo clippy --workspace --all-features -- -D warnings &&
cargo test -p ironhermes-core config_schema memory_provider &&
cargo test -p memory-sqlite &&
cargo test -p memory-duckdb &&
cargo test -p memory-grafeo &&
cargo test -p ironhermes-agent --features memory-sqlite memory::factory::tests::sqlite_round_trip_via_factory &&
cargo test -p ironhermes-agent --features memory-duckdb memory::factory::tests::duckdb_round_trip_via_factory &&
cargo test -p ironhermes-agent --features memory-grafeo memory::factory::tests::grafeo_round_trip_via_factory &&
! grep -R "MemoryProviderConfig" crates/ providers/
```

**Workspace audit (should all pass):**
- No file anywhere references `MemoryProviderConfig` (struct fully deleted).
- `ConfigField` and `MemoryAction` are re-exported from `ironhermes_core::{ConfigField, MemoryAction}`.
- The factory returns `Arc<tokio::sync::Mutex<dyn MemoryProvider + Send>>` (not `std::sync::Mutex`) — this is the shape Plan 20-02's `MemoryManager` will wrap.
</verification>

<success_criteria>
- [ ] `MemoryProvider` trait carries the required `fn name(&self) -> &'static str` plus the 10 defaulted hook methods (D-01..D-05, D-11..D-14).
- [ ] `initialize` signature is `async fn initialize(&mut self, session_id: &str, hermes_home: &Path, provider_config: &Value) -> anyhow::Result<()>` (D-10).
- [ ] `MemoryProviderConfig` is deleted from the entire workspace (zero `grep` matches).
- [ ] `ConfigField` + `MemoryAction` exist in `ironhermes-core/src/config_schema.rs` and round-trip via serde (D-06).
- [ ] `MemoryConfig.mirror_provider: Option<String>` exists (D-27) — Plan 20-02 consumes it.
- [ ] Factory is `async fn`, uses `tokio::sync::Mutex`, calls `initialize().await` then `load_from_disk()` for every provider (D-16 Fix 1).
- [ ] `is_available() == false` triggers file-provider fallback with `tracing::warn!(reason = ..., ...)` (D-17).
- [ ] Sqlite/DuckDB/Grafeo providers all implement `name()` and the new `initialize` signature; all their tests pass (D-20).
- [ ] Factory round-trip regression test passes for sqlite (mandatory) and duckdb/grafeo (feature-gated) (D-24).
- [ ] Every `build_memory_provider` call site awaits the future; workspace `cargo check --all-features` is clean.
- [ ] T-20-01 mitigation documented in the trait comment; T-20-04 mitigation enforced via `debug_assert` in default `save_config`.
</success_criteria>

<output>
After completion, create `.planning/phases/20-memory-provider-plugin-contract/20-01-SUMMARY.md` following the template at `$HOME/.claude/get-shit-done/templates/summary.md`. Highlight:
- Confirmation that `MemoryProviderConfig` is fully deleted (grep-verified).
- Resolution of research open question #2 (tokio::sync::Mutex chosen) and #3 (async factory chosen).
- List of every file touched and test added.
- Any deviations from the plan + rationale.
</output>
