---
phase: 20
plan: 04
type: execute
wave: 3
depends_on: [20-02]
files_modified:
  - crates/ironhermes-core/src/memory_store.rs
  - crates/ironhermes-core/src/memory_provider.rs
  - providers/memory-sqlite/src/lib.rs
  - providers/memory-duckdb/src/lib.rs
  - providers/memory-grafeo/src/lib.rs
  - providers/memory-sqlite/tests/config_schema.rs
  - providers/memory-duckdb/tests/config_schema.rs
  - providers/memory-grafeo/tests/config_schema.rs
  - crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs
autonomous: true
requirements: [MEM-08, MEM-09, MEM-10, MEM-11]
tags: [memory, provider, plugin-contract, config-schema, hooks]

must_haves:
  truths:
    - "Every compiled-in MemoryProvider returns a stable, non-empty `name()` literal"
    - "Every compiled-in MemoryProvider returns a non-empty `get_config_schema()` describing its real config surface"
    - "Every ConfigField with a secret uses an env_var name (no plaintext in config files)"
    - "At least one provider (sqlite) demonstrates on_memory_write mirror behavior end-to-end"
    - "File provider default is preserved when no external provider is available (is_available=false fallback)"
  artifacts:
    - path: "crates/ironhermes-core/src/memory_store.rs"
      provides: "MemoryStore impl overrides name() and get_config_schema()"
      contains: 'fn name(&self) -> &\'static str'
    - path: "providers/memory-sqlite/src/lib.rs"
      provides: "SqliteMemoryProvider impl overrides name(), get_config_schema(), optionally on_memory_write fixture hook"
      contains: '"sqlite"'
    - path: "providers/memory-duckdb/src/lib.rs"
      provides: "DuckDbMemoryProvider impl overrides name() and get_config_schema()"
      contains: '"duckdb"'
    - path: "providers/memory-grafeo/src/lib.rs"
      provides: "GrafeoMemoryProvider impl overrides name() and get_config_schema()"
      contains: '"grafeo"'
    - path: "providers/memory-sqlite/tests/config_schema.rs"
      provides: "Unit test asserting sqlite config_schema content"
    - path: "providers/memory-duckdb/tests/config_schema.rs"
      provides: "Unit test asserting duckdb config_schema content"
    - path: "providers/memory-grafeo/tests/config_schema.rs"
      provides: "Unit test asserting grafeo config_schema content"
    - path: "crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs"
      provides: "Integration test demonstrating on_memory_write fires to a mock mirror when sqlite is the primary"
  key_links:
    - from: "MemoryProvider trait (20-01)"
      to: "concrete provider impls"
      via: "default-method overrides"
      pattern: "fn name|get_config_schema|on_memory_write"
    - from: "sqlite primary + mock mirror"
      to: "MemoryManager.on_memory_write (20-02)"
      via: "MemoryManager::handle_tool_call success path"
      pattern: "mirror.on_memory_write"
---

<objective>
Every provider crate picks up the hooks it cares about — specifically `name()` (required, no default from 20-01) and `get_config_schema()` (default empty; each provider must override with its real config surface). One provider (sqlite) demonstrates `on_memory_write` end-to-end as a fixture, proving the plugin-contract surface composes with the MemoryManager mirror layer.

Purpose: Close the plugin-contract loop. After 20-01 defined the trait and 20-02 introduced the manager, the providers themselves must expose their identity and config so that the wizard (20-03) and any future introspection tooling can enumerate them without hard-coded knowledge. Mirror fixture proves the composition.

Output: Four provider impls (file, sqlite, duckdb, grafeo) with `name()` + `get_config_schema()` overrides; four unit tests pinning the schema contract; one integration test proving on_memory_write fires through MemoryManager when sqlite is the primary.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/STATE.md
@.planning/REQUIREMENTS.md
@.planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md
@.planning/phases/20-memory-provider-plugin-contract/20-RESEARCH.md
@.planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md
@.planning/phases/20-memory-provider-plugin-contract/20-01-SUMMARY.md
@.planning/phases/20-memory-provider-plugin-contract/20-02-SUMMARY.md

<interfaces>
<!-- Key contracts this plan consumes. Do not re-derive from codebase — use these directly. -->

From `crates/ironhermes-core/src/memory_provider.rs` (after 20-01):

```rust
#[async_trait]
pub trait MemoryProvider: Send + Sync + 'static {
    fn name(&self) -> &'static str;  // REQUIRED — no default

    fn is_available(&self) -> bool { true }
    fn unavailable_reason(&self) -> Option<String> { None }

    fn get_config_schema(&self) -> Vec<ConfigField> { Vec::new() }
    fn save_config(
        &self,
        _values: &std::collections::HashMap<String, serde_json::Value>,
        _hermes_home: &std::path::Path,
    ) -> anyhow::Result<()> { Ok(()) }

    fn system_prompt_block(&self) -> Option<String> { None }

    async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> { Ok(()) }
    async fn on_pre_compress(
        &self,
        _messages: &[crate::chat::ChatMessage],
    ) -> anyhow::Result<()> { Ok(()) }
    async fn on_memory_write(
        &mut self,
        _action: MemoryAction,
        _target: MemoryTarget,
        _content: &str,
    ) -> anyhow::Result<()> { Ok(()) }

    // ... other methods (initialize, prefetch, sync_turn, add, replace, remove, ...)
}
```

From `crates/ironhermes-core/src/config_schema.rs` (created in 20-01):

```rust
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConfigField {
    pub key: String,
    pub description: String,
    pub secret: bool,
    pub required: bool,
    pub default: Option<serde_json::Value>,
    pub choices: Option<Vec<String>>,
    pub env_var: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryAction { Add, Replace, Remove }
```

From `crates/ironhermes-agent/src/memory/manager.rs` (created in 20-02):

```rust
pub struct MemoryManager {
    primary: Arc<tokio::sync::Mutex<Box<dyn MemoryProvider>>>,
    mirror:  Option<Arc<tokio::sync::Mutex<Box<dyn MemoryProvider>>>>,
}

impl MemoryManager {
    pub fn new(
        primary: Box<dyn MemoryProvider>,
        mirror: Option<Box<dyn MemoryProvider>>,
    ) -> anyhow::Result<Self>;

    pub async fn handle_tool_call(
        &self,
        tool_name: &str,
        args: serde_json::Value,
    ) -> ironhermes_core::memory_store::MemoryResult;
}
```

From `crates/ironhermes-core/src/memory_store.rs` (pre-existing, for char-limit facts):

```rust
pub const MEMORY_CHAR_LIMIT: usize = 2200;
pub const USER_CHAR_LIMIT: usize = 1375;
// MemoryTarget::char_limit() returns these per-target.
// MemoryStore::new(memory_dir: PathBuf) — constructs a file-based provider.
```

From `crates/ironhermes-core/src/context_scanner.rs`:

```rust
pub const CONTEXT_FILE_MAX_CHARS: usize = 20_000;
```

</interfaces>

<constraints>
<!-- Non-negotiable rules for this plan. -->

- **No new trait methods.** This plan only overrides defaults from 20-01 and supplies the one required method (`name`). If a method needs adding, stop and escalate — that belongs in 20-01.
- **No secrets in config files.** Every `ConfigField { secret: true, .. }` MUST set `env_var = Some("IRONHERMES_<PROVIDER>_<KEY>")`. `save_config` never writes secret keys.
- **Config schema must be a stable literal**, not derived from runtime state. `get_config_schema()` is queried by the wizard before the provider is initialized — it must not depend on `&self` state beyond the `name()` literal.
- **`name()` return values are stable identifiers** already used by the factory: `"file"`, `"sqlite"`, `"duckdb"`, `"grafeo"`. Do not invent new names.
- **No path traversal in `name()`** — debug_assert per T-20-04 (already enforced in 20-01 factory boundary). Here we just pin the literals.
- **Preserve `on_memory_write` mirror-failure semantics** (D-14, D-29): mirror errors are logged, not returned. The fixture test asserts both sides of this.
- **No changes to `add`/`replace`/`remove` bodies.** This plan only touches trait-hook overrides and tests.
- **MEM-12 single-primary invariant** is enforced by MemoryManager (20-02); this plan does not re-enforce it, but the sqlite_mirror_fixture test MUST use `MemoryManager::new(primary=sqlite, mirror=Some(mock))` (not two primaries).
- **Feature gating is preserved.** Each provider crate's test file is gated by the same features as the crate itself (e.g. `memory-sqlite` is an optional feature at the workspace level); the sqlite_mirror_fixture test in the agent crate is gated by `#[cfg(feature = "memory-sqlite")]`.
- **Rust 2024 edition, tokio 1.x, async-trait 0.1.x** — no new dependencies introduced.
</constraints>
</context>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| provider crate -> MemoryManager | Provider code crosses the trait boundary; hostile/buggy provider must not corrupt manager state |
| disk -> provider | Provider reads files (db_path, memory_dir, graph_dir) owned by $HERMES_HOME — traversal must be prevented at factory/wizard boundary (already enforced in 20-01 / 20-03) |
| mirror -> primary | Mirror is write-only; it MUST NOT be queried on read paths (enforced at MemoryManager in 20-02, re-asserted by fixture test) |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-20-01 | Tampering | provider `get_config_schema` | mitigate | Each schema pins `env_var` for secrets (never plaintext in `save_config`); unit tests assert `secret=true` fields have `env_var.is_some()` |
| T-20-04 | Spoofing  | provider `name()` | mitigate | Pin literals in tests: `assert_eq!(provider.name(), "sqlite")` etc. Reject any runtime-computed names (would be a refactor-time regression the test catches) |
| T-20-06 | Info Disclosure | `ConfigField.description` / `default` logged by wizard | mitigate | Wizard (20-03) uses `RedactedValue` for secret fields; this plan ensures `secret=true` flag is set correctly on every sensitive field so the redaction triggers |
| T-20-07 | Repudiation | on_memory_write mirror audit | accept | Fixture test asserts action/target/content propagation; full audit trail is Phase 23+ concern |
| T-20-08 | DoS | Malformed `get_config_schema` (infinite loop) | accept | `get_config_schema` returns owned `Vec<ConfigField>` with no I/O; schema is a compile-time-ish constant per provider. Low risk. |

All HIGH-severity threats (T-20-01, T-20-04, T-20-06) are mitigated with automated test assertions in this plan's test tasks.
</threat_model>

<tasks>

<task type="auto" tdd="true">
  <name>Task 20-04-01: File + SQLite — name() and get_config_schema() overrides with unit tests</name>
  <files>
    - crates/ironhermes-core/src/memory_store.rs
    - providers/memory-sqlite/src/lib.rs
    - providers/memory-sqlite/tests/config_schema.rs
  </files>

  <read_first>
    - crates/ironhermes-core/src/memory_store.rs (full file — current MemoryProvider impl for MemoryStore; 20-01 already migrated initialize signature)
    - crates/ironhermes-core/src/memory_provider.rs (for trait default signatures from 20-01)
    - crates/ironhermes-core/src/config_schema.rs (for ConfigField shape from 20-01)
    - crates/ironhermes-core/src/context_scanner.rs (for CONTEXT_FILE_MAX_CHARS)
    - providers/memory-sqlite/src/lib.rs (full file — current MemoryProvider impl; 20-01 migrated initialize)
    - providers/memory-sqlite/Cargo.toml (for dev-deps / features)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md sections D-02, D-06, D-08 (authoritative on `name()` required, ConfigField shape, wizard contract)
  </read_first>

  <behavior>
    - Test 1 (file, unit in memory_store.rs): `MemoryStore::new(tmp).name()` returns `"file"` (exact literal).
    - Test 2 (file, unit in memory_store.rs): `MemoryStore::new(tmp).get_config_schema()` returns a Vec with exactly 3 fields keyed `"memory_dir"`, `"memory_char_limit"`, `"user_char_limit"`. The `memory_dir` field has `required=false`, `default = Some(json!("$HERMES_HOME/memory"))`, `secret=false`, `env_var=None`. The `memory_char_limit` and `user_char_limit` fields have `required=false`, `default=Some(json!(2200))` and `Some(json!(1375))` respectively, `secret=false`.
    - Test 3 (file, unit in memory_store.rs): Every field with `secret=true` has `env_var.is_some()` — vacuously passes for file provider (no secrets). Codified as a general invariant helper re-used across providers.
    - Test 4 (sqlite, integration test file): `SqliteMemoryProvider::new(tmp.path().join("mem.db")).name()` returns `"sqlite"`.
    - Test 5 (sqlite, integration test file): `get_config_schema()` returns a Vec with exactly one field: `{ key: "db_path", description: non-empty, secret: false, required: false, default: Some(json!("$HERMES_HOME/memory.db")), env_var: None, url: None, choices: None }`.
    - Test 6 (sqlite, integration test file): Secret-implies-env_var invariant holds (vacuous for sqlite; same helper from Test 3).
  </behavior>

  <action>
    1. **In `crates/ironhermes-core/src/memory_store.rs`** — inside the existing `impl MemoryProvider for MemoryStore` block (from 20-01), add these override methods at the top of the impl (after `initialize`, before `prefetch`):

       ```rust
       fn name(&self) -> &'static str {
           "file"
       }

       fn get_config_schema(&self) -> Vec<crate::config_schema::ConfigField> {
           use crate::config_schema::ConfigField;
           use serde_json::json;
           vec![
               ConfigField {
                   key: "memory_dir".to_string(),
                   description: "Directory holding MEMORY.md and USER.md files".to_string(),
                   secret: false,
                   required: false,
                   default: Some(json!("$HERMES_HOME/memory")),
                   choices: None,
                   env_var: None,
                   url: None,
               },
               ConfigField {
                   key: "memory_char_limit".to_string(),
                   description: "Character limit for the MEMORY.md scope (default 2200)".to_string(),
                   secret: false,
                   required: false,
                   default: Some(json!(crate::memory_store::MEMORY_CHAR_LIMIT)),
                   choices: None,
                   env_var: None,
                   url: None,
               },
               ConfigField {
                   key: "user_char_limit".to_string(),
                   description: "Character limit for the USER.md scope (default 1375)".to_string(),
                   secret: false,
                   required: false,
                   default: Some(json!(crate::memory_store::USER_CHAR_LIMIT)),
                   choices: None,
                   env_var: None,
                   url: None,
               },
           ]
       }
       ```

       If the constants `MEMORY_CHAR_LIMIT` / `USER_CHAR_LIMIT` are not yet module-level `pub const`s (they may be encoded in `MemoryTarget::char_limit()` returning literals 2200/1375), promote them to `pub const MEMORY_CHAR_LIMIT: usize = 2200;` and `pub const USER_CHAR_LIMIT: usize = 1375;` at the top of `memory_store.rs`, and update `MemoryTarget::char_limit` to reference them. This is a zero-behavior refactor — confirm the values via grep before changing.

    2. **In the same file, `#[cfg(test)] mod tests { ... }` block** — append these unit tests (do not remove existing tests):

       ```rust
       #[test]
       fn file_provider_name_is_file() {
           let tmp = tempfile::tempdir().unwrap();
           let store = super::MemoryStore::new(tmp.path().to_path_buf());
           assert_eq!(
               <super::MemoryStore as crate::memory_provider::MemoryProvider>::name(&store),
               "file",
           );
       }

       #[test]
       fn file_provider_config_schema_shape() {
           let tmp = tempfile::tempdir().unwrap();
           let store = super::MemoryStore::new(tmp.path().to_path_buf());
           let schema = <super::MemoryStore as crate::memory_provider::MemoryProvider>::get_config_schema(&store);

           let keys: Vec<&str> = schema.iter().map(|f| f.key.as_str()).collect();
           assert_eq!(keys, vec!["memory_dir", "memory_char_limit", "user_char_limit"]);

           let memory_dir = schema.iter().find(|f| f.key == "memory_dir").unwrap();
           assert!(!memory_dir.required);
           assert!(!memory_dir.secret);
           assert!(memory_dir.env_var.is_none());
           assert_eq!(memory_dir.default, Some(serde_json::json!("$HERMES_HOME/memory")));

           let mem_limit = schema.iter().find(|f| f.key == "memory_char_limit").unwrap();
           assert_eq!(mem_limit.default, Some(serde_json::json!(2200)));

           let user_limit = schema.iter().find(|f| f.key == "user_char_limit").unwrap();
           assert_eq!(user_limit.default, Some(serde_json::json!(1375)));
       }

       #[test]
       fn file_provider_secret_implies_env_var() {
           let tmp = tempfile::tempdir().unwrap();
           let store = super::MemoryStore::new(tmp.path().to_path_buf());
           let schema = <super::MemoryStore as crate::memory_provider::MemoryProvider>::get_config_schema(&store);
           for field in &schema {
               if field.secret {
                   assert!(
                       field.env_var.is_some(),
                       "secret field {:?} must declare env_var",
                       field.key,
                   );
               }
           }
       }
       ```

    3. **In `providers/memory-sqlite/src/lib.rs`** — inside the existing `impl MemoryProvider for SqliteMemoryProvider` block (from 20-01 migration), add at the top of the impl:

       ```rust
       fn name(&self) -> &'static str {
           "sqlite"
       }

       fn get_config_schema(&self) -> Vec<ironhermes_core::config_schema::ConfigField> {
           use ironhermes_core::config_schema::ConfigField;
           use serde_json::json;
           vec![ConfigField {
               key: "db_path".to_string(),
               description: "SQLite database file path. Created on first run if absent.".to_string(),
               secret: false,
               required: false,
               default: Some(json!("$HERMES_HOME/memory.db")),
               choices: None,
               env_var: None,
               url: None,
           }]
       }
       ```

    4. **Create `providers/memory-sqlite/tests/config_schema.rs`**:

       ```rust
       //! Phase 20-04 Task 20-04-01: pin sqlite provider name() and get_config_schema().
       //!
       //! These assertions are the plugin-contract surface. Changing them is a
       //! breaking change for the setup wizard (Phase 20-03) and must be done
       //! with a corresponding wizard update.

       use ironhermes_core::memory_provider::MemoryProvider;
       use memory_sqlite::SqliteMemoryProvider;

       #[test]
       fn sqlite_provider_name_is_sqlite() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
           assert_eq!(provider.name(), "sqlite");
       }

       #[test]
       fn sqlite_provider_config_schema_shape() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
           let schema = provider.get_config_schema();

           assert_eq!(schema.len(), 1, "expected one field (db_path)");
           let db_path = &schema[0];
           assert_eq!(db_path.key, "db_path");
           assert!(!db_path.description.is_empty());
           assert!(!db_path.required);
           assert!(!db_path.secret);
           assert!(db_path.env_var.is_none());
           assert_eq!(
               db_path.default,
               Some(serde_json::json!("$HERMES_HOME/memory.db")),
           );
       }

       #[test]
       fn sqlite_provider_secret_implies_env_var() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = SqliteMemoryProvider::new(&tmp.path().join("mem.db")).unwrap();
           for field in provider.get_config_schema() {
               if field.secret {
                   assert!(field.env_var.is_some(), "secret field {} must declare env_var", field.key);
               }
           }
       }
       ```

    5. If `providers/memory-sqlite/Cargo.toml` does not already have `tempfile` in `[dev-dependencies]`, add it: `tempfile = "3"`. Use `cargo add --dev --package memory-sqlite tempfile` from the workspace root. Do NOT add any production dependencies.
  </action>

  <verify>
    <automated>cargo test -p ironhermes-core memory_store::tests::file_provider_ --lib && cargo test -p memory-sqlite --test config_schema</automated>
  </verify>

  <acceptance_criteria>
    - `grep -n 'fn name(&self) -> &.\'static str' crates/ironhermes-core/src/memory_store.rs` returns exactly 1 match inside `impl MemoryProvider for MemoryStore`.
    - `grep -n '"file"' crates/ironhermes-core/src/memory_store.rs` returns a match within 3 lines of the `fn name` signature.
    - `grep -n 'fn name(&self) -> &.\'static str' providers/memory-sqlite/src/lib.rs` returns exactly 1 match.
    - `grep -n '"sqlite"' providers/memory-sqlite/src/lib.rs` returns a match within 3 lines of the `fn name` signature.
    - `grep -n '"memory_dir"\|"memory_char_limit"\|"user_char_limit"' crates/ironhermes-core/src/memory_store.rs` returns at least 3 matches.
    - `grep -n '"db_path"' providers/memory-sqlite/src/lib.rs` returns at least 1 match.
    - `providers/memory-sqlite/tests/config_schema.rs` exists and contains `#[test]` on at least 3 distinct functions.
    - `cargo test -p ironhermes-core memory_store::tests::file_provider_` passes (3 tests green).
    - `cargo test -p memory-sqlite --test config_schema` passes (3 tests green).
    - `cargo clippy -p ironhermes-core -p memory-sqlite --all-features -- -D warnings` exits 0.
  </acceptance_criteria>

  <done>
    File + sqlite providers expose name() literal and a pinned config_schema. Unit + integration tests prevent silent schema drift. Secret-implies-env_var invariant helper established for other providers to reuse.
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-04-02: DuckDB + Grafeo — name() and get_config_schema() overrides with unit tests</name>
  <files>
    - providers/memory-duckdb/src/lib.rs
    - providers/memory-duckdb/tests/config_schema.rs
    - providers/memory-grafeo/src/lib.rs
    - providers/memory-grafeo/tests/config_schema.rs
  </files>

  <read_first>
    - providers/memory-duckdb/src/lib.rs (full file — current MemoryProvider impl; 20-01 migrated initialize)
    - providers/memory-duckdb/Cargo.toml (dev-deps + features)
    - providers/memory-grafeo/src/lib.rs (full file — current MemoryProvider impl; 20-01 migrated initialize)
    - providers/memory-grafeo/Cargo.toml (dev-deps + features)
    - crates/ironhermes-core/src/config_schema.rs (for ConfigField shape)
    - providers/memory-sqlite/tests/config_schema.rs (test pattern established in Task 20-04-01; replicate, do not re-invent)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md sections D-02, D-06 (name() required; default empty schema — override with real fields)
    - .planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md rows 20-04-03 / 20-04-04 (expected schema keys per provider)
  </read_first>

  <behavior>
    - Test 1 (duckdb): `DuckDbMemoryProvider::new(tmp.path().join("mem.duckdb")).name()` returns `"duckdb"`.
    - Test 2 (duckdb): `get_config_schema()` returns a Vec with exactly 2 fields: `{key: "db_path", secret: false, required: false, default: Some(json!("$HERMES_HOME/memory.duckdb")), env_var: None}` and `{key: "threads", secret: false, required: false, default: Some(json!(1)), env_var: None}` (threads default 1 per VALIDATION row 20-04-03).
    - Test 3 (duckdb): Secret-implies-env_var invariant holds (vacuous).
    - Test 4 (grafeo): `GrafeoMemoryProvider::new_in_memory().name()` returns `"grafeo"`.
    - Test 5 (grafeo): `get_config_schema()` returns a Vec with exactly 1 field: `{key: "graph_dir", secret: false, required: false, default: Some(json!("$HERMES_HOME/grafeo")), env_var: None}`.
    - Test 6 (grafeo): Secret-implies-env_var invariant holds (vacuous).
  </behavior>

  <action>
    1. **In `providers/memory-duckdb/src/lib.rs`** — inside the existing `impl MemoryProvider for DuckDbMemoryProvider` block (post-20-01 migration), add at the top of the impl:

       ```rust
       fn name(&self) -> &'static str {
           "duckdb"
       }

       fn get_config_schema(&self) -> Vec<ironhermes_core::config_schema::ConfigField> {
           use ironhermes_core::config_schema::ConfigField;
           use serde_json::json;
           vec![
               ConfigField {
                   key: "db_path".to_string(),
                   description: "DuckDB database file path. Created on first run if absent.".to_string(),
                   secret: false,
                   required: false,
                   default: Some(json!("$HERMES_HOME/memory.duckdb")),
                   choices: None,
                   env_var: None,
                   url: None,
               },
               ConfigField {
                   key: "threads".to_string(),
                   description: "Number of worker threads DuckDB may use (default 1 for deterministic single-user workloads).".to_string(),
                   secret: false,
                   required: false,
                   default: Some(json!(1)),
                   choices: None,
                   env_var: None,
                   url: None,
               },
           ]
       }
       ```

       The `threads` field is purely declarative here — actually applying it requires the bridge to `PRAGMA threads = N` on startup, which is NOT in scope for this plan (the bridge wiring is a follow-on). The wizard will still prompt for it and persist it; the DuckDB provider will pick it up from `provider_config` in a future phase.

    2. **Create `providers/memory-duckdb/tests/config_schema.rs`** mirroring the sqlite test structure:

       ```rust
       //! Phase 20-04 Task 20-04-02: pin duckdb provider name() and get_config_schema().

       use ironhermes_core::memory_provider::MemoryProvider;
       use memory_duckdb::DuckDbMemoryProvider;

       #[test]
       fn duckdb_provider_name_is_duckdb() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
           assert_eq!(provider.name(), "duckdb");
       }

       #[test]
       fn duckdb_provider_config_schema_shape() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
           let schema = provider.get_config_schema();

           let keys: Vec<&str> = schema.iter().map(|f| f.key.as_str()).collect();
           assert_eq!(keys, vec!["db_path", "threads"]);

           let db_path = schema.iter().find(|f| f.key == "db_path").unwrap();
           assert!(!db_path.required);
           assert!(!db_path.secret);
           assert!(db_path.env_var.is_none());
           assert_eq!(db_path.default, Some(serde_json::json!("$HERMES_HOME/memory.duckdb")));

           let threads = schema.iter().find(|f| f.key == "threads").unwrap();
           assert_eq!(threads.default, Some(serde_json::json!(1)));
       }

       #[test]
       fn duckdb_provider_secret_implies_env_var() {
           let tmp = tempfile::tempdir().unwrap();
           let provider = DuckDbMemoryProvider::new(&tmp.path().join("mem.duckdb")).unwrap();
           for field in provider.get_config_schema() {
               if field.secret {
                   assert!(field.env_var.is_some(), "secret field {} must declare env_var", field.key);
               }
           }
       }
       ```

    3. **In `providers/memory-grafeo/src/lib.rs`** — inside `impl MemoryProvider for GrafeoMemoryProvider` (post-20-01), add at the top:

       ```rust
       fn name(&self) -> &'static str {
           "grafeo"
       }

       fn get_config_schema(&self) -> Vec<ironhermes_core::config_schema::ConfigField> {
           use ironhermes_core::config_schema::ConfigField;
           use serde_json::json;
           vec![ConfigField {
               key: "graph_dir".to_string(),
               description: "Directory holding the Grafeo graph database (file or directory). Created on first run if absent.".to_string(),
               secret: false,
               required: false,
               default: Some(json!("$HERMES_HOME/grafeo")),
               choices: None,
               env_var: None,
               url: None,
           }]
       }
       ```

    4. **Create `providers/memory-grafeo/tests/config_schema.rs`**:

       ```rust
       //! Phase 20-04 Task 20-04-02: pin grafeo provider name() and get_config_schema().

       use ironhermes_core::memory_provider::MemoryProvider;
       use memory_grafeo::GrafeoMemoryProvider;

       #[test]
       fn grafeo_provider_name_is_grafeo() {
           let provider = GrafeoMemoryProvider::new_in_memory();
           assert_eq!(provider.name(), "grafeo");
       }

       #[test]
       fn grafeo_provider_config_schema_shape() {
           let provider = GrafeoMemoryProvider::new_in_memory();
           let schema = provider.get_config_schema();

           assert_eq!(schema.len(), 1, "expected one field (graph_dir)");
           let graph_dir = &schema[0];
           assert_eq!(graph_dir.key, "graph_dir");
           assert!(!graph_dir.description.is_empty());
           assert!(!graph_dir.required);
           assert!(!graph_dir.secret);
           assert!(graph_dir.env_var.is_none());
           assert_eq!(graph_dir.default, Some(serde_json::json!("$HERMES_HOME/grafeo")));
       }

       #[test]
       fn grafeo_provider_secret_implies_env_var() {
           let provider = GrafeoMemoryProvider::new_in_memory();
           for field in provider.get_config_schema() {
               if field.secret {
                   assert!(field.env_var.is_some(), "secret field {} must declare env_var", field.key);
               }
           }
       }
       ```

    5. If `tempfile` is missing from `[dev-dependencies]` in `providers/memory-duckdb/Cargo.toml`, add it (`cargo add --dev --package memory-duckdb tempfile`). Grafeo test uses `new_in_memory()`, no temp dir needed.
  </action>

  <verify>
    <automated>cargo test -p memory-duckdb --test config_schema && cargo test -p memory-grafeo --test config_schema</automated>
  </verify>

  <acceptance_criteria>
    - `grep -n 'fn name(&self) -> &.\'static str' providers/memory-duckdb/src/lib.rs` returns exactly 1 match.
    - `grep -n '"duckdb"' providers/memory-duckdb/src/lib.rs` returns a match within 3 lines of `fn name`.
    - `grep -n '"db_path"\|"threads"' providers/memory-duckdb/src/lib.rs` returns at least 2 matches.
    - `grep -n 'fn name(&self) -> &.\'static str' providers/memory-grafeo/src/lib.rs` returns exactly 1 match.
    - `grep -n '"grafeo"' providers/memory-grafeo/src/lib.rs` returns a match within 3 lines of `fn name`.
    - `grep -n '"graph_dir"' providers/memory-grafeo/src/lib.rs` returns at least 1 match.
    - Both new test files exist with ≥3 `#[test]` functions each.
    - `cargo test -p memory-duckdb --test config_schema` passes (3 tests green).
    - `cargo test -p memory-grafeo --test config_schema` passes (3 tests green).
    - `cargo clippy -p memory-duckdb -p memory-grafeo --all-features -- -D warnings` exits 0.
  </acceptance_criteria>

  <done>
    DuckDB and Grafeo providers expose name() literal and a pinned config_schema. All four providers (file, sqlite, duckdb, grafeo) now satisfy the plugin-contract surface required by the wizard (20-03).
  </done>
</task>

<task type="auto" tdd="true">
  <name>Task 20-04-03: SQLite mirror fixture — end-to-end on_memory_write through MemoryManager</name>
  <files>
    - crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs
    - crates/ironhermes-agent/Cargo.toml (dev-dependencies only if missing)
  </files>

  <read_first>
    - crates/ironhermes-agent/src/memory/manager.rs (full file — from 20-02; the `MemoryManager::new`, `handle_tool_call`, and on_memory_write-firing paths)
    - crates/ironhermes-agent/src/memory/factory.rs (for `build_memory_manager` signature from 20-01/20-02)
    - crates/ironhermes-core/src/memory_provider.rs (trait + MemoryAction from 20-01)
    - crates/ironhermes-core/tests/memory_provider_contract.rs (MockMemoryProvider created in 20-02; reuse its invocation recorder pattern)
    - providers/memory-sqlite/src/lib.rs (SqliteMemoryProvider::new constructor)
    - .planning/phases/20-memory-provider-plugin-contract/20-CONTEXT.md sections D-14, D-25, D-26, D-28, D-29 (authoritative on mirror semantics + test requirements)
    - .planning/phases/20-memory-provider-plugin-contract/20-02-SUMMARY.md (exact MemoryManager API shape as delivered by 20-02)
    - .planning/phases/20-memory-provider-plugin-contract/20-VALIDATION.md row 20-04-05 (fixture test expected shape)
  </read_first>

  <behavior>
    - Test 1 (`sqlite_primary_fires_on_memory_write_to_mirror`): Build a `MemoryManager` with sqlite primary (temp DB) and a `MockMirrorProvider` as mirror. Call `handle_tool_call("memory_add", {target: "memory", content: "fact-1"})`. Assert the mirror recorded exactly one `on_memory_write` invocation with `action = MemoryAction::Add`, `target = MemoryTarget::Memory`, `content = "fact-1"`. Assert the primary sqlite provider has the entry (query via `format_for_system_prompt(MemoryTarget::Memory)` after a `load_from_disk`).
    - Test 2 (`mirror_observes_replace_and_remove`): Same setup; perform `memory_add` → `memory_replace` → `memory_remove` in sequence; assert the mirror observed three invocations in order with action Add / Replace / Remove and consistent target/content.
    - Test 3 (`failing_mirror_does_not_block_sqlite_writes`): Build a `MemoryManager` with sqlite primary and a `MockMirrorProvider` whose `on_memory_write` returns `Err(anyhow!("mirror kaput"))`. Call `memory_add`. Assert: (a) the outer `handle_tool_call` result is `Ok(_)` — primary write succeeded; (b) the sqlite primary actually persisted the entry; (c) the mirror's counter of `on_memory_write` invocations is still 1 (the call was made, the error was swallowed); (d) `tracing` captured an error-level log (use `tracing_test::traced_test` attribute if available, else assert presence via the mirror's own recorded error message).
    - Test 4 (`mirror_never_receives_reads`): Same setup; call a read-path op on the MemoryManager (`prefetch("session-id")` or equivalent read entry point); assert mirror's on-read counter is 0 — reads MUST NOT fan out to the mirror per D-26/D-28.
  </behavior>

  <action>
    1. **Create `crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs`**. The test file is feature-gated so it only compiles when sqlite is available:

       ```rust
       //! Phase 20-04 Task 20-04-03: End-to-end fixture proving MemoryManager fires
       //! on_memory_write through to a mirror provider when sqlite is primary.
       //!
       //! Covers D-14, D-25..D-29 (mirror semantics) and T-20-07 (observability).

       #![cfg(feature = "memory-sqlite")]

       use std::sync::atomic::{AtomicUsize, Ordering};
       use std::sync::Arc;

       use anyhow::anyhow;
       use async_trait::async_trait;
       use ironhermes_core::config_schema::MemoryAction;
       use ironhermes_core::memory_provider::{MemoryEntries, MemoryProvider};
       use ironhermes_core::memory_store::{MemoryResult, MemoryTarget};

       use ironhermes_agent::memory::manager::MemoryManager;
       use memory_sqlite::SqliteMemoryProvider;

       /// Records every hook invocation with full argument fidelity. Optionally
       /// returns Err from on_memory_write to exercise the failure-swallow path.
       #[derive(Default)]
       struct MockMirrorProvider {
           writes: std::sync::Mutex<Vec<(MemoryAction, MemoryTarget, String)>>,
           reads: AtomicUsize,
           fail_on_write: bool,
       }

       impl MockMirrorProvider {
           fn new() -> Self { Self::default() }
           fn failing() -> Self { Self { fail_on_write: true, ..Default::default() } }
           fn write_log(&self) -> Vec<(MemoryAction, MemoryTarget, String)> {
               self.writes.lock().unwrap().clone()
           }
           fn read_count(&self) -> usize { self.reads.load(Ordering::SeqCst) }
       }

       #[async_trait]
       impl MemoryProvider for MockMirrorProvider {
           fn name(&self) -> &'static str { "mock-mirror" }

           async fn initialize(
               &mut self,
               _session_id: &str,
               _hermes_home: &std::path::Path,
               _provider_config: &serde_json::Value,
           ) -> anyhow::Result<()> { Ok(()) }

           async fn prefetch(&self, _session_id: &str) -> anyhow::Result<MemoryEntries> {
               self.reads.fetch_add(1, Ordering::SeqCst);
               Ok(MemoryEntries::default())
           }

           async fn sync_turn(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
           async fn on_session_end(&self, _s: &str, _e: &MemoryEntries) -> anyhow::Result<()> { Ok(()) }
           async fn shutdown(&mut self) -> anyhow::Result<()> { Ok(()) }

           fn load_from_disk(&mut self) -> anyhow::Result<()> { Ok(()) }

           fn add(&mut self, _t: MemoryTarget, _c: &str) -> MemoryResult { Ok(String::new()) }
           fn replace(&mut self, _t: MemoryTarget, _o: &str, _n: &str) -> MemoryResult { Ok(String::new()) }
           fn remove(&mut self, _t: MemoryTarget, _o: &str) -> MemoryResult { Ok(String::new()) }

           fn format_for_system_prompt(&self, _t: MemoryTarget) -> Option<String> { None }
           fn to_memory_entries(&self) -> MemoryEntries { MemoryEntries::default() }

           async fn on_memory_write(
               &mut self,
               action: MemoryAction,
               target: MemoryTarget,
               content: &str,
           ) -> anyhow::Result<()> {
               self.writes.lock().unwrap().push((action, target, content.to_string()));
               if self.fail_on_write {
                   Err(anyhow!("mirror kaput"))
               } else {
                   Ok(())
               }
           }
       }

       fn tmp_db() -> (tempfile::TempDir, std::path::PathBuf) {
           let dir = tempfile::tempdir().unwrap();
           let path = dir.path().join("mem.db");
           (dir, path)
       }

       #[tokio::test]
       async fn sqlite_primary_fires_on_memory_write_to_mirror() {
           let (_guard, db_path) = tmp_db();
           let primary: Box<dyn MemoryProvider> =
               Box::new(SqliteMemoryProvider::new(&db_path).unwrap());
           let mirror_arc = Arc::new(parking_lot::Mutex::new(MockMirrorProvider::new()));
           // Construction: use the MemoryManager API from 20-02. If the real API
           // takes an owned Box for the mirror, keep a thin proxy that holds an
           // Arc so the test can still observe. See comment at end of file.
           let mirror_box: Box<dyn MemoryProvider> = todo!("EXECUTOR: wrap mirror_arc per 20-02 API");
           let manager = MemoryManager::new(primary, Some(mirror_box)).unwrap();

           let args = serde_json::json!({ "target": "memory", "content": "fact-1" });
           let res = manager.handle_tool_call("memory_add", args).await;
           assert!(res.is_ok(), "primary write must succeed: {:?}", res);

           let log = mirror_arc.lock().write_log();
           assert_eq!(log.len(), 1);
           assert_eq!(log[0].0, MemoryAction::Add);
           assert_eq!(log[0].1, MemoryTarget::Memory);
           assert_eq!(log[0].2, "fact-1");
       }

       #[tokio::test]
       async fn mirror_observes_replace_and_remove() {
           // ... add, replace, remove sequence; assert 3 log entries with correct actions.
       }

       #[tokio::test]
       async fn failing_mirror_does_not_block_sqlite_writes() {
           // MockMirrorProvider::failing() returns Err from on_memory_write.
           // Assert outer handle_tool_call is Ok(_); primary has the entry;
           // mirror log has 1 entry (call was made, error swallowed).
       }

       #[tokio::test]
       async fn mirror_never_receives_reads() {
           // Read op on manager; assert mirror.read_count() == 0.
       }
       ```

       **EXECUTOR NOTE (wrapping the mirror for observability):** The 20-02 `MemoryManager::new(primary, mirror)` signature takes owned `Box<dyn MemoryProvider>` for both. To let the test observe the mirror's internal state after construction, use one of:
       - **Preferred:** Wrap the recorder in `Arc<parking_lot::Mutex<_>>` and write a thin `ObservableMirror` adapter that holds the `Arc` and whose `MemoryProvider` impl forwards to the inner recorder. The adapter is what goes into the `Box`; the `Arc` clone stays with the test for assertions.
       - **Alternative:** If 20-02 exposes a `MemoryManager::mirror_handle()` accessor returning an `Arc<Mutex<Box<dyn MemoryProvider>>>`, downcast via `Any` (add `fn as_any(&self) -> &dyn Any` to the trait — NO, do not extend the trait; use the adapter approach instead).

       Replace the `todo!(...)` with the adapter wiring. Pattern:

       ```rust
       struct ObservableMirror(Arc<parking_lot::Mutex<MockMirrorProvider>>);

       #[async_trait]
       impl MemoryProvider for ObservableMirror {
           fn name(&self) -> &'static str { "observable-mirror" }
           async fn on_memory_write(&mut self, a: MemoryAction, t: MemoryTarget, c: &str) -> anyhow::Result<()> {
               self.0.lock().on_memory_write(a, t, c).await
           }
           // ... forward the rest as no-ops or to self.0
       }
       ```

       The adapter is test-only and lives entirely in this file.

    2. **Dev dependencies** — ensure `crates/ironhermes-agent/Cargo.toml` `[dev-dependencies]` contains: `tempfile`, `tokio = { version = "1", features = ["macros", "rt-multi-thread"] }`, `parking_lot`, `anyhow`, `async-trait`, and `memory-sqlite = { path = "../../providers/memory-sqlite", optional = false }` gated under a dev feature if needed. Prefer `tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }` if the workspace already declares it. Add missing items only — do not duplicate.

    3. **Feature wiring** — the test file is `#![cfg(feature = "memory-sqlite")]`. Ensure the agent crate has `memory-sqlite = ["dep:memory-sqlite"]` in `[features]` (should already exist from prior phases). Running `cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture` must compile and run all 4 tests.
  </action>

  <verify>
    <automated>cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture</automated>
  </verify>

  <acceptance_criteria>
    - `crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs` exists.
    - File begins with `#![cfg(feature = "memory-sqlite")]`.
    - Contains exactly 4 `#[tokio::test]` functions matching the behavior spec: `sqlite_primary_fires_on_memory_write_to_mirror`, `mirror_observes_replace_and_remove`, `failing_mirror_does_not_block_sqlite_writes`, `mirror_never_receives_reads`.
    - No `todo!()` or `unimplemented!()` remains in the committed file.
    - `grep -n 'MemoryAction::Add\|MemoryAction::Replace\|MemoryAction::Remove' crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs` returns at least 3 matches (one per action).
    - `grep -n 'on_memory_write' crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs` returns at least 2 matches (impl + assertion use).
    - `cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture` passes all 4 tests.
    - `cargo clippy -p ironhermes-agent --features memory-sqlite --tests -- -D warnings` exits 0.
    - Failing-mirror test explicitly asserts (a) outer Ok, (b) primary entry persisted, (c) mirror call counted.
    - `mirror_never_receives_reads` asserts `read_count() == 0`.
  </acceptance_criteria>

  <done>
    End-to-end proof that the plugin-contract mirror composition works: sqlite primary + mock mirror, four scenarios green (success, multi-op sequence, failure swallow, no-read-fanout). MEM-12 single-primary + mirror-as-observational invariant locked in by test.
  </done>
</task>

</tasks>

<verification>
All four provider crates expose the plugin-contract surface:

1. **Schema pinning:** `cargo test --workspace --all-features` includes the three `tests/config_schema.rs` files and the in-crate `memory_store::tests::file_provider_*` tests — total 12 new assertions covering name() literal + schema shape + secret-implies-env_var invariant across all four providers.

2. **End-to-end mirror:** `cargo test -p ironhermes-agent --features memory-sqlite --test sqlite_mirror_fixture` green (4 tests).

3. **Lint:** `cargo clippy --workspace --all-features --tests -- -D warnings` exits 0.

4. **Wizard unblocked:** With this plan landed, the Plan 20-03 wizard can iterate compiled-in providers, call `get_config_schema()`, and branch on the returned fields without any provider-specific code — proving the contract is sufficient for its consumer.

5. **No regressions:** `cargo test --workspace --all-features` still passes (providers' existing tests untouched; only additive overrides + new test files).
</verification>

<success_criteria>
- All four compiled-in `MemoryProvider` implementations override `name()` with the exact literal used by the factory (`"file"`, `"sqlite"`, `"duckdb"`, `"grafeo"`).
- All four override `get_config_schema()` with provider-appropriate fields (file: 3 fields; sqlite: 1 field; duckdb: 2 fields; grafeo: 1 field).
- Three new `tests/config_schema.rs` files exist (sqlite, duckdb, grafeo) plus three new unit tests added to `memory_store.rs` (file); each file asserts: name literal, schema shape, secret-implies-env_var invariant.
- `crates/ironhermes-agent/tests/sqlite_mirror_fixture.rs` exists with 4 passing `#[tokio::test]`s proving: success propagation, multi-op sequence ordering, failure swallow, read-path isolation.
- `cargo test --workspace --all-features` passes.
- `cargo clippy --workspace --all-features --tests -- -D warnings` passes.
- No new production dependencies introduced.
- Requirements MEM-08, MEM-09, MEM-10, MEM-11 each have at least one automated assertion tying them to a provider.
</success_criteria>

<output>
After completion, create `.planning/phases/20-memory-provider-plugin-contract/20-04-SUMMARY.md` capturing:
- Exact schema Vec<ConfigField> returned by each provider (key list + defaults).
- Mirror fixture test results (4/4 green).
- Any Cargo.toml dev-dep additions.
- Confirmation that the wizard (20-03) now sees non-empty schemas from every compiled-in provider.
</output>
