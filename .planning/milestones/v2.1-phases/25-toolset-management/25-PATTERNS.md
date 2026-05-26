# Phase 25: Toolset Management - Pattern Map

**Mapped:** 2026-04-29
**Files analyzed:** 12 new/modified files across 4 crates
**Analogs found:** 12 / 12

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `crates/ironhermes-tools/src/registry.rs` | registry | CRUD | self (expand in-place) | exact |
| `crates/ironhermes-tools/src/web_search.rs` | tool impl | request-response | self (add `prerequisites()`) | exact |
| `crates/ironhermes-tools/src/web_read.rs` | tool impl | request-response | `web_search.rs:67-69` | exact |
| `crates/ironhermes-tools/src/terminal.rs` | tool impl | CRUD | `web_search.rs` toolset() pattern | role-match |
| `crates/ironhermes-tools/src/file_tools.rs` | tool impl | CRUD | `web_search.rs` toolset() pattern | role-match |
| `crates/ironhermes-tools/src/cronjob_tool.rs` | tool impl | CRUD | `web_search.rs` toolset() pattern | role-match |
| `crates/ironhermes-core/src/config.rs` | config schema | CRUD | `config.rs:528-560` SubagentConfig | exact |
| `crates/ironhermes-core/src/constants.rs` | config | config | existing constants pattern | role-match |
| `crates/ironhermes-cli/src/toolset_cmd.rs` | cli subcommand | request-response | `config_cli.rs` | exact |
| `crates/ironhermes-cli/src/main.rs` | cli entry | request-response | self (add Commands variant + dispatch arm) | exact |
| `crates/ironhermes-core/src/commands/registry.rs` | slash command | request-response | `registry.rs:111` "toolsets" stub | exact |
| `crates/ironhermes-cli/src/setup.rs` | setup wizard | request-response | `setup.rs:239-245` `run_tools_section` stub | exact |
| `crates/ironhermes-cli/src/preflight.rs` | middleware | request-response | self (extend in-place) | exact |
| `crates/ironhermes-agent/src/agent_loop.rs` | agent loop | event-driven | self (migrate + add builder) | exact |
| `crates/ironhermes-cli/tests/toolset_integration.rs` | integration test | batch | `tests/profile_isolation.rs` | exact |

---

## Pattern Assignments

### `crates/ironhermes-tools/src/registry.rs` — Plan 1 + Plan 2 + Plan 3 (registry expansion)

**Analog:** self — expand in-place

**Existing `Tool` trait** (`registry.rs:10-22`):
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```
**What to copy (Plan 1):** Add `prerequisites()` method with default `vec![]` return AFTER `is_available()`. Update `is_available()` default body to walk `self.prerequisites()` filtering `required: true` and checking env vars. Add `Prerequisite` struct above the trait. Keep `is_available()` signature unchanged so every existing impl compiles without modification.

**Existing `ToolRegistry` struct** (`registry.rs:24-28`):
```rust
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>,
    error_detail: ironhermes_hooks::ErrorDetailLevel,
}
```
**What to copy (Plan 2):** Add two new fields after `error_detail`:
```rust
intercepts: HashMap<String, (ToolSchema, InterceptHandler)>,
toolset_config: Option<ToolsConfig>,
```
`InterceptHandler` type alias = `Arc<dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, anyhow::Result<String>> + Send + Sync>`.

**Existing `register()` method** (`registry.rs:39-41`):
```rust
pub fn register(&mut self, tool: Box<dyn Tool>) {
    self.tools.insert(tool.name().to_string(), tool);
}
```
**What to copy (Plan 2 — D-15 panic guard):** New `register_intercepted()` must check `self.tools.contains_key(name)` and `panic!()` if true. `register()` must gain a reciprocal check: `assert!(!self.intercepts.contains_key(tool.name()), ...)`.

**Existing `get_definitions()` method** (`registry.rs:76-87`):
```rust
pub fn get_definitions(&self, enabled_tools: Option<&[String]>) -> Vec<ToolSchema> {
    self.tools
        .values()
        .filter(|t| t.is_available())
        .filter(|t| {
            enabled_tools
                .map(|list| list.iter().any(|name| name == t.name()))
                .unwrap_or(true)
        })
        .map(|t| t.schema())
        .collect()
}
```
**What to copy (Plan 3 — toolset filter + intercept schemas):** Signature stays identical. Add a leading filter clause before `is_available()`:
```rust
.filter(|t| self.toolset_enabled(t.toolset()))
```
After collecting from `self.tools`, extend with schemas from `self.intercepts`:
```rust
schemas.extend(self.intercepts.values().map(|(schema, _)| schema.clone()));
```
Apply per-tool `disabled` list from `ToolsConfig` as a final filter. `None` toolset_config = no toolset filtering (D-A2, preserves existing test behavior).

**Existing test mock** (`registry.rs:394-419`): Copy `MockTool` pattern verbatim for all new registry unit tests. The struct + `#[async_trait] impl Tool` shape is the canonical test double.

---

### `crates/ironhermes-tools/src/web_search.rs` — Plan 1 (add `prerequisites()`)

**Analog:** self

**Existing `is_available()`** (`web_search.rs:67-69`):
```rust
fn is_available(&self) -> bool {
    std::env::var("FIRECRAWL_API_KEY").is_ok()
}
```
**What to copy:** Keep this override as-is (D-09 allows custom `is_available()` when logic can't be expressed via prereqs alone). Add alongside it:
```rust
fn prerequisites(&self) -> Vec<Prerequisite> {
    vec![Prerequisite {
        kind: "env_var".to_string(),
        name: "FIRECRAWL_API_KEY".to_string(),
        description: "Firecrawl API key for web search".to_string(),
        required: true,
    }]
}
```

---

### `crates/ironhermes-tools/src/web_read.rs` — Plan 1 (add `prerequisites()`)

**Analog:** `web_search.rs:67-69`

**Current stub** (`web_read.rs:521`): `fn is_available(&self) -> bool { true }`

**What to copy:** Replace the hard-coded `true` stub with the default impl (remove the override, or keep it as `true` since `web_read` has a plain-text fallback). Add `prerequisites()` with `required: false`:
```rust
fn prerequisites(&self) -> Vec<Prerequisite> {
    vec![Prerequisite {
        kind: "env_var".to_string(),
        name: "FIRECRAWL_API_KEY".to_string(),
        description: "Firecrawl API key (optional — plain-text fallback used without it)".to_string(),
        required: false,
    }]
}
```
Per D-09: `required: false` means this prereq is advisory only and does NOT block `is_available()`.

---

### `crates/ironhermes-tools/src/terminal.rs`, `file_tools.rs`, `cronjob_tool.rs` — Plan 1 (toolset() fixes)

**Analog:** any existing `toolset()` impl (e.g., `web_search.rs`)

**What to change:** One-liner per impl. These are the only changes in these files for Phase 25:
- `terminal.rs`: `fn toolset(&self) -> &str { "system" }` → `"code"`
- `file_tools.rs` (all four impls: `ReadFileTool`, `WriteFileTool`, `PatchFileTool`, `SearchFilesTool`): `fn toolset(&self) -> &str { "file" }` → `"code"`
- `cronjob_tool.rs`: `fn toolset(&self) -> &str { "cronjob" }` → `"agent"`

No other changes to these files in Phase 25.

---

### `crates/ironhermes-core/src/config.rs` — Plan 3 (add `ToolsConfig`)

**Analog:** `config.rs:528-560` — `SubagentConfig`

**Existing `SubagentConfig` pattern** (`config.rs:526-560`):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubagentConfig {
    pub timeout_secs: u64,
    pub max_subagents: usize,
    // ...
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 300,
            max_subagents: 3,
            // ...
        }
    }
}
```
**What to copy:** Follow this exact shape for two new structs:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsetEntry {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub toolsets: HashMap<String, ToolsetEntry>,
    pub skip_prompts: Vec<String>,
    pub disabled: Vec<String>,
}

impl Default for ToolsConfig {
    fn default() -> Self {
        let mut toolsets = HashMap::new();
        for name in ["memory", "session", "agent", "skills"] {
            toolsets.insert(name.to_string(), ToolsetEntry { enabled: true });
        }
        for name in ["web", "code"] {
            toolsets.insert(name.to_string(), ToolsetEntry { enabled: false });
        }
        Self { toolsets, skip_prompts: vec![], disabled: vec![] }
    }
}
```
Add a `tools: ToolsConfig` field to the top-level `Config` struct using the same `#[serde(default)]` pattern as all other `*Config` fields.

---

### `crates/ironhermes-core/src/constants.rs` — Plan 3 (add `DEFAULT_TOOLSETS`)

**Analog:** existing constants in `constants.rs`

**What to add:**
```rust
/// D-20: toolsets enabled on a fresh install.
pub const DEFAULT_TOOLSETS: &[&str] = &["memory", "session", "agent", "skills"];
```
Single constant addition; no other changes.

---

### `crates/ironhermes-cli/src/toolset_cmd.rs` — Plan 4 (new file, CLI subcommand)

**Analog:** `crates/ironhermes-cli/src/config_cli.rs` (full file, 255 lines)

**Imports pattern** (`config_cli.rs:1-9`):
```rust
use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_core::{config_schema, config_setter};
use std::path::Path;
```
Copy these imports, substituting `config_schema`/`config_setter` for `ironhermes_core::constants` and `ironhermes_tools::ToolRegistry`.

**Subcommand enum pattern** (`config_cli.rs:13-27`):
```rust
#[derive(Subcommand)]
pub enum ConfigSubcommand {
    Set { key: String, value: String },
    Get { key: String },
    Show,
    Migrate,
    Path,
    #[command(name = "env-path")]
    EnvPath,
}
```
Copy this shape for `ToolsetSubcommand`:
```rust
#[derive(Subcommand)]
pub enum ToolsetSubcommand {
    /// List all toolsets with status and availability
    List,
    /// Enable a toolset (persists to active profile config.yaml)
    Enable { name: String },
    /// Disable a toolset (persists to active profile config.yaml)
    Disable { name: String },
    /// Show detail for one toolset (members, schemas, prerequisites)
    Show { name: String },
    /// Walk through missing required prerequisites interactively
    Setup,
}
```

**Dispatcher function pattern** (`config_cli.rs:29-44`):
```rust
pub async fn handle_config_command(cmd: ConfigSubcommand, profile_name: &str) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ConfigSubcommand::Set { key, value } => cmd_config_set(&hermes_home, &key, &value).await,
        // ...
    }
}
```
Copy this dispatcher shape for `handle_toolset_command(cmd: ToolsetSubcommand, profile_name: &str) -> Result<()>`.

**Cache-break banner pattern** (`config_cli.rs:47-60`):
```rust
async fn cmd_config_set(hermes_home: &Path, key: &str, value: &str) -> Result<()> {
    let schema = config_schema::schema();
    if config_setter::is_cache_breaking(key, &schema) {
        eprintln!(
            "{} Changing {} invalidates the prompt cache. Active sessions will pay full cache-miss cost on next turn.",
            "⚠".yellow(),
            key
        );
    }
    // ...
    println!("Persisted: {} = {}", key, value);
    Ok(())
}
```
For `hermes toolset enable/disable`, emit this cache-break banner using `eprintln!` with `.yellow()` colored prefix. Use the exact banner text from CONTEXT.md Specifics: `[toolset: {name}] enabled — schema cache will rebuild on next LLM call`. Use `"⚠".yellow()` for the prefix.

**Config write pattern** (uses `config_setter::config_set`):
```rust
// enable: write tools.toolsets.<name>.enabled = "true"
config_setter::config_set(&hermes_home, &format!("tools.toolsets.{}.enabled", name), "true")
    .with_context(|| format!("failed to enable toolset {}", name))?;
```
Validation: call `ironhermes_core::profile::validate_profile_name`-style slug check (regex `[a-z0-9][a-z0-9-]*`) on `name` before writing. Return a clear error for unknown toolset names.

---

### `crates/ironhermes-cli/src/main.rs` — Plan 4 (add `Toolset` variant)

**Analog:** self — existing `Config` variant and dispatch arm

**Commands enum addition** (`main.rs:162-167`):
```rust
/// Manage configuration values (Phase 23, D-08/D-09/D-10/D-11).
Config {
    #[command(subcommand)]
    subcommand: config_cli::ConfigSubcommand,
},
```
Copy this shape. Add after `Config`:
```rust
/// Manage toolsets — enable/disable, list, show, setup (Phase 25, D-04).
Toolset {
    #[command(subcommand)]
    subcommand: toolset_cmd::ToolsetSubcommand,
},
```

**Dispatch arm** (`main.rs:378-384`):
```rust
Some(Commands::Config { subcommand }) => {
    let profile_name = cli.profile.as_deref().unwrap_or("default").to_string();
    config_cli::handle_config_command(subcommand, &profile_name).await
}
```
Copy verbatim, substituting `Toolset`/`toolset_cmd::handle_toolset_command`.

**Module declaration** (`main.rs:29-38`, `mod config_cli;` line): add `mod toolset_cmd;` in the same block.

---

### `crates/ironhermes-core/src/commands/registry.rs` — Plan 4 (replace `"toolsets"` stub)

**Analog:** self — existing `"toolsets"` entry at line 111

**Existing stub** (`registry.rs:111`):
```rust
CommandDef::new("toolsets", "List available toolsets", ToolsAndSkills).platform(CliOnly),
```
**What to copy:** Replace this single line with:
```rust
CommandDef::new("toolset", "Manage toolsets (list/enable/disable/show)", ToolsAndSkills)
    .args_hint("[list|enable|disable|show|setup] [name]")
    .platform(Universal),
```
Note: `"toolsets"` (plural) → `"toolset"` (singular) per D-06. `CliOnly` → `Universal` because slash commands are runtime-only (D-06). This is the only change to `registry.rs` in Phase 25.

---

### `crates/ironhermes-cli/src/setup.rs` — Plan 5 (replace `run_tools_section` stub)

**Analog:** `setup.rs:172-228` — `run_memory_section` (full prompt loop with rustyline)

**Existing stub to replace** (`setup.rs:239-245`):
```rust
async fn run_tools_section(
    _config: &mut Config,
    _rl: &mut rustyline::DefaultEditor,
) -> Result<()> {
    println!("Tools setup will gain prerequisite-check prompts in Phase 25 (TOOL-05).");
    Ok(())
}
```

**Prompt loop pattern** (`setup.rs:53-67` — `prompt_required`):
```rust
fn prompt_required(rl: &mut rustyline::DefaultEditor, prompt: &str) -> Result<String> {
    use rustyline::error::ReadlineError;
    loop {
        let raw = match rl.readline(&format!("{}: ", prompt)) {
            Ok(s) => s,
            Err(ReadlineError::Interrupted) => return Err(anyhow!("interrupted")),
            Err(ReadlineError::Eof) => return Err(anyhow!("EOF on stdin")),
            Err(e) => return Err(anyhow!("readline error: {}", e)),
        };
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
        eprintln!("Required — please enter a value (Ctrl-C to abort).");
    }
}
```
Copy this error-matching pattern for all readline calls inside `run_tools_section`.

**Config setter splicing pattern** (`setup.rs:129-146`):
```rust
config.save_to(&hermes_home.join("config.yaml")).context("writing config.yaml")?;
for (key, value) in &learning_block {
    let dotted = format!("learning.{}", key_str);
    config_setter::config_set(hermes_home, &dotted, &value_str)?;
}
```
For tool prereq env var writes: write to `hermes_home.join(".env")` (same path as `cmd_config_env_path` exposes). For config field writes: use `config_setter::config_set(hermes_home, prereq.name, value)`.

**`apply_minimum_viable_answers` testability seam** (`setup.rs:250-261`):
```rust
pub fn apply_minimum_viable_answers(
    config: &mut Config,
    provider: &str,
    api_key: &str,
    model: &str,
    learning_loop: &str,
) -> serde_yaml::Mapping {
    // ...
}
```
**What to copy:** Add a parallel seam `apply_tool_prereq_answers(hermes_home, prereqs: &[(name, value)])` that drives the tool-prereq stage without rustyline. Integration tests for `hermes toolset setup` call this seam directly.

---

### `crates/ironhermes-cli/src/preflight.rs` — Plan 5 (add tool-prereq stage)

**Analog:** self (expand in-place)

**Existing `run_preflight_check`** (`preflight.rs:10-27`):
```rust
pub async fn run_preflight_check(_cli: &Cli) -> Result<()> {
    let cfg_path = Config::config_path();
    if !cfg_path.exists() {
        return crate::setup::run_setup(None, WizardMode::FirstRun).await;
    }
    match Config::load() {
        Err(_) => crate::setup::run_setup(None, WizardMode::FixMode).await,
        Ok(config) => {
            if !config.validate().is_empty() {
                crate::setup::run_setup(None, WizardMode::FixMode).await
            } else {
                Ok(())
            }
        }
    }
}
```
**What to copy (D-17 insertion):** After the `config.validate().is_empty()` check passes (i.e., config is valid), add the tool-prereq probe BEFORE returning `Ok(())`:
```rust
Ok(config) => {
    if !config.validate().is_empty() {
        return crate::setup::run_setup(None, WizardMode::FixMode).await;
    }
    // Phase 25 D-17: tool-prereq stage
    // (build registry, call list_unavailable(), emit banner if required prereqs missing)
    // INSERT HERE — before Ok(())
    Ok(())
}
```
Banner style mirrors Phase 24 D-08 profile banner (`eprintln!("[toolset: {}] missing prereq: {}", ...)`) and Phase 23 D-13 cache-break warning. NO auto-wizard launch — operator uses `hermes toolset setup` to fix. Stderr only; stdout untouched.

---

### `crates/ironhermes-agent/src/agent_loop.rs` — Plan 3 (migrate intercepts + builder)

**Analog:** self — existing builder chain and session_search intercept block

**Existing builder chain** (`agent_loop.rs:202-291`, representative sample):
```rust
pub fn with_memory_manager(mut self, manager: Arc<Mutex<MemoryManager>>) -> Self {
    self.memory_manager = Some(manager);
    self
}

pub fn with_state_store(mut self, store: Arc<std::sync::Mutex<StateStore>>) -> Self {
    // ...
    self
}
```
**What to copy (D-16):** Add `with_intercepts(mut self, ...) -> Self` following the identical signature pattern. Takes handles for `memory_manager` (already has its own builder), `state_store` (already has its own builder), `subagent_runner`, `todo_state`, and `cron_router`. Stores them and wires `register_intercepted()` at session start. Default `new()` registers NO intercepts (existing tests remain unaffected).

**Existing session_search interception block** (`agent_loop.rs:951-971`):
```rust
if name == "session_search" {
    if let Some(ref state) = self.state_store {
        let state_clone = state.clone();
        let args_clone = args.clone();
        let result = tokio::task::spawn_blocking(move || {
            let store = state_clone.lock().unwrap();
            crate::session_search::handle_session_search(&args_clone, &store)
        }).await;
        return match result {
            Ok(s) => s,
            Err(e) => format!(r#"{{"error":"internal","reason":"{}"}}"#, ...),
        };
    }
    return r#"{"error":"unavailable","reason":"state store not configured"}"#.to_string();
}
```
**What to copy (D-12 migration):** This block moves into `registry.dispatch_intercepts()`. After migration, the call site becomes:
```rust
if let Some(result) = registry.dispatch_intercepts(name, args.clone()).await {
    return result;
}
```
The `spawn_blocking` pattern stays inside the closure registered with `register_intercepted("session_search", schema, handler)`.

**Schema injection block** (`agent_loop.rs:478-484`):
```rust
let mut tool_schemas = self.registry.read().await.get_definitions(None);
if self.state_store.is_some() {
    tool_schemas.push(crate::session_search::session_search_schema());
}
```
**What to copy (D-14 migration):** After `register_intercepted("session_search", ...)` owns the schema, this push MUST be deleted. The `get_definitions()` call stays; it now includes intercept schemas automatically. Leaving both causes the D-15 panic at registry build in tests.

---

### `crates/ironhermes-cli/tests/toolset_integration.rs` — Plan 4 + Plan 5 (new integration tests)

**Analog:** `crates/ironhermes-cli/tests/profile_isolation.rs` (full file)

**env_lock pattern** (`profile_isolation.rs:7-15`):
```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
```
Copy verbatim. Every test that mutates `IRONHERMES_HOME` or `FIRECRAWL_API_KEY` MUST hold this lock.

**Binary subprocess pattern** (`profile_isolation.rs:81-97`):
```rust
let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
    Ok(p) => p,
    Err(_) => {
        eprintln!("Skipping ...: CARGO_BIN_EXE_ironhermes not set");
        return;
    }
};
let tmp = tempfile::TempDir::new().unwrap();
let out = std::process::Command::new(&bin)
    .env("IRONHERMES_HOME", tmp.path())
    .args(["--profile", "testbanner", "doctor"])
    .output()
    .expect("failed to run ironhermes binary");
let stderr = String::from_utf8_lossy(&out.stderr);
let stdout = String::from_utf8_lossy(&out.stdout);
```
Copy this shape for all three D-26 integration tests. Key adaptations:
- D-26 Test 1 (`toolset_enable_disable_persists`): `.args(["toolset", "enable", "web"])` then `.args(["toolset", "list"])`. Assert stdout contains `web` and `enabled`. Then new `Command::new(&bin)` same `IRONHERMES_HOME` to verify persistence.
- D-26 Test 2 (`tool_excluded_when_prereq_missing`): unit test in `crates/ironhermes-tools/` using env_lock + `unsafe { std::env::remove_var("FIRECRAWL_API_KEY") }`. Build ToolRegistry with `toolset_config` having `web: { enabled: true }`. Assert `web_search` absent from `get_definitions(None)`.
- D-26 Test 3 (`intercepted_tool_no_schema_duplicate`): unit test in `crates/ironhermes-tools/registry.rs` `#[cfg(test)]`. Build registry + `register_intercepted()` for all D-13 tools. Call `get_definitions(None)`. Assert each intercepted name appears exactly once.

---

## Shared Patterns

### Cache-Break Banner (stderr, no stdout contamination)
**Source:** `crates/ironhermes-cli/src/config_cli.rs:47-60` + `main.rs:224-229`
**Apply to:** `toolset_cmd.rs` `cmd_toolset_enable`, `cmd_toolset_disable`
```rust
eprintln!(
    "{} [toolset: {}] {} — schema cache will rebuild on next LLM call",
    "⚠".yellow(),
    name,
    action  // "enabled" or "disabled"
);
```
Pattern: `eprintln!` only. `stdout` stays clean for pipes. `.yellow()` from `colored` crate (already in `Cargo.toml`).

### Dotted-Path Config Read/Write
**Source:** `config_cli.rs:57-59` + `setup.rs:145`
**Apply to:** `toolset_cmd.rs` enable/disable handlers
```rust
config_setter::config_set(&hermes_home, "tools.toolsets.web.enabled", "true")
    .with_context(|| format!("failed to enable toolset web"))?;
```
Never hand-roll YAML writes. Always use `config_setter::config_set` — it is atomic (tempfile+rename per Phase 21.5/21.8/24 D-10).

### rustyline Prompt Loop (no history bleed)
**Source:** `setup.rs:22-28` + `setup.rs:53-67`
**Apply to:** `setup.rs run_tools_section`, `toolset_cmd.rs cmd_toolset_setup`
```rust
fn make_wizard_editor() -> Result<rustyline::DefaultEditor> {
    use rustyline::config::Configurer;
    let mut rl = rustyline::DefaultEditor::new().context("initializing rustyline for wizard")?;
    rl.set_history_ignore_dups(true).ok();
    // Anti-Pattern #3: no history file persistence
    Ok(rl)
}
```
Always construct via `make_wizard_editor()`. Never use `rustyline::DefaultEditor::new()` directly in wizard code (bypasses Anti-Pattern #3 guard).

### AgentLoop Builder Chain
**Source:** `agent_loop.rs:202-205`
**Apply to:** `agent_loop.rs with_intercepts`
```rust
pub fn with_memory_manager(mut self, manager: Arc<Mutex<MemoryManager>>) -> Self {
    self.memory_manager = Some(manager);
    self
}
```
Every builder method: takes `mut self`, sets one `Option<T>` field, returns `self`. No `&mut self` builders — the chain is consumed and rebuilt.

### Cross-Crate Plain-String Type
**Source:** Phase 22.4.2.2 / Phase 23 D-12 / Phase 24 D-17 pattern (carried forward)
**Apply to:** `Prerequisite` struct in `registry.rs`
```rust
pub struct Prerequisite {
    pub kind: String,        // "env_var" | "config_field"
    pub name: String,
    pub description: String,
    pub required: bool,
}
```
No enums. Consumers match on `kind` strings at their call site. `#[derive(Debug, Clone)]` only — no `Serialize`/`Deserialize` needed unless stored in config (they are not).

### OnceLock env_lock for Test Isolation
**Source:** `profile_isolation.rs:7-15`
**Apply to:** All tests in `toolset_integration.rs` and `toolset_prereq.rs` that mutate env vars
```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
// In each test:
let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
```
This is mandatory for `tool_excluded_when_prereq_missing` (D-26 Test 2). Rust runs tests in parallel threads; any test that mutates `FIRECRAWL_API_KEY` without this lock causes Pitfall 6 races.

---

## No Analog Found

All Phase 25 files have close analogs in the codebase. No files require falling back to RESEARCH.md patterns exclusively.

| File | Role | Note |
|------|------|-------|
| `crates/ironhermes-tools/tests/toolset_prereq.rs` | integration test | New file; uses `profile_isolation.rs` subprocess + env_lock pattern (analog: role-match) |

---

## Metadata

**Analog search scope:** `crates/ironhermes-tools/`, `crates/ironhermes-cli/`, `crates/ironhermes-core/`, `crates/ironhermes-agent/`
**Files scanned:** 15 source files read directly
**Pattern extraction date:** 2026-04-29

---

## PATTERN MAPPING COMPLETE
