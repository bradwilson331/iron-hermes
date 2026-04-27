# Phase 23: Configuration CLI and Setup Wizard — Pattern Map

**Mapped:** 2026-04-27
**Files analyzed:** 9 new/modified files
**Analogs found:** 9 / 9

> Maps each new file/module Phase 23 introduces to the closest existing analog
> in the codebase. Planner uses this to ensure new code follows established
> conventions.

---

## File Classification

| New / Modified File | Role | Data Flow | Closest Analog | Match Quality |
|---|---|---|---|---|
| `crates/ironhermes-core/src/wizard.rs` | utility | transform | `crates/ironhermes-cli/src/memory_setup.rs` | role-match |
| `crates/ironhermes-core/src/config_validate.rs` | utility | transform | `crates/ironhermes-core/src/config.rs` (Config impl block) | role-match |
| `crates/ironhermes-core/src/config_setter.rs` | utility | CRUD | `crates/ironhermes-cli/src/memory_setup.rs` `update_config_yaml_memory_provider` | exact |
| `crates/ironhermes-cli/src/setup.rs` | service | request-response | `crates/ironhermes-cli/src/memory_setup.rs` | exact |
| `crates/ironhermes-cli/src/config_cli.rs` | controller | CRUD | `crates/ironhermes-cli/src/cron.rs` + `handlers.rs` cmd_config stub | role-match |
| `crates/ironhermes-cli/src/preflight.rs` | middleware | request-response | `crates/ironhermes-cli/src/main.rs` (pre-dispatch logic) | partial |
| `crates/ironhermes-core/src/config_schema.rs` | model | — | itself (EXTEND) | — |
| `crates/ironhermes-core/src/commands/handlers.rs` | controller | request-response | itself (EXTEND) | — |
| `crates/ironhermes-cli/src/main.rs` | controller | request-response | itself (EXTEND) | — |

---

## Pattern Assignments — New Files

---

### `crates/ironhermes-core/src/wizard.rs` (NEW)

**Purpose:** Pure-function wizard helpers — `apply_*_answer` functions that map a raw string input + default onto a `Config` mutation. No rustyline or I/O dependency; all I/O stays in `setup.rs`.

**Closest analog:** `crates/ironhermes-cli/src/memory_setup.rs`

The `prompt_line` / `run_memory_setup_with_io` separation in `memory_setup.rs` is the direct precedent: pure logic is extracted into a testable function that takes `&mut Config` rather than touching stdin. Phase 23's `wizard.rs` takes that pattern one level further by moving the pure mutation functions into `ironhermes-core` so they can be unit-tested without the CLI crate.

**Imports pattern** (`memory_setup.rs` lines 19–30, adapted for core):
```rust
// In ironhermes-core — no CLI or rustyline imports
use crate::config::Config;
use anyhow::Result;
```

**Core pattern — pure apply function** (from RESEARCH.md Q2 + `memory_setup.rs` pattern):
```rust
/// Apply a wizard answer for `model.default`.
/// Empty input accepts `default`; non-empty input trims and sets.
pub fn apply_model_answer(config: &mut Config, raw_input: &str, default: &str) {
    let val = if raw_input.trim().is_empty() { default } else { raw_input.trim() };
    config.model.default = val.to_string();
}

/// Apply Learning Loop opt-in answer.
/// "y" or empty (default YES) writes the full memory.* + learning.* block.
pub fn apply_learning_loop_answer(config: &mut Config, raw_input: &str) {
    let enabled = raw_input.trim().is_empty()           // Enter = default YES
        || raw_input.trim().eq_ignore_ascii_case("y")
        || raw_input.trim().eq_ignore_ascii_case("yes");
    config.memory.memory_enabled = enabled;
    config.memory.user_profile_enabled = enabled;
    // learning.* keys are always written explicitly (never absent — per D-14)
    // ... set all learning.* fields to defaults or disabled sentinels
}
```

**Error handling pattern:** Functions return `()` (infallible) or `Result<()>` when validation is required. Validation errors are separate from mutation — validation lives in `config_validate.rs`, mutation lives here.

**Doc-comment pattern** (`memory_setup.rs` lines 1–18):
```rust
//! `wizard.rs` — pure-function wizard helpers (apply_*_answer).
//!
//! No I/O dependency. All rustyline / stdin interaction lives in `setup.rs`.
//! Import and call these functions from the rustyline-driven wizard runner.
```

**Test pattern** (`memory_setup.rs` lines 319–333):
```rust
#[test]
fn apply_model_uses_default_on_empty_input() {
    let mut config = Config::default();
    apply_model_answer(&mut config, "", "openrouter/qwen-2.5-coder-32b");
    assert_eq!(config.model.default, "openrouter/qwen-2.5-coder-32b");
}
```

---

### `crates/ironhermes-core/src/config_validate.rs` (NEW)

**Purpose:** `ConfigValidationError` struct + `Config::validate() -> Vec<ConfigValidationError>` method (D-06). Used by the preflight check to determine whether fix-mode wizard must run.

**Closest analog:** `crates/ironhermes-core/src/config.rs` lines 602–668 (Config impl block — existing `load`, `save`, `save_to`, `config_path`, `env_path`, `telegram_default_origin` methods show the impl block extension pattern).

The `telegram_default_origin` method (config.rs lines 650–667) is the closest behavioral analog: it inspects `self` fields and returns a computed result type. `validate()` follows the same shape but returns `Vec<ConfigValidationError>` instead of `OriginDecision`.

**Struct definition** (RESEARCH.md Q4):
```rust
/// A single config validation failure, keyed by dotted path (D-06, D-08).
#[derive(Debug, Clone)]
pub struct ConfigValidationError {
    pub path: String,              // dotted path, e.g. "model.api_key"
    pub reason: String,            // human-readable description
    pub suggested_fix: Option<String>, // e.g. "Run `hermes setup model` to set"
}
```

**impl Config extension pattern** (config.rs lines 602–634):
```rust
impl Config {
    // existing: load(), load_from(), save(), save_to(), config_path(), env_path()

    /// Validate this config and return all detected problems.
    /// Returns an empty Vec when the config is ready to use.
    pub fn validate(&self) -> Vec<ConfigValidationError> {
        let mut errors = Vec::new();
        if self.model.api_key.as_deref().unwrap_or("").is_empty() {
            errors.push(ConfigValidationError {
                path: "model.api_key".into(),
                reason: "API key is required".into(),
                suggested_fix: Some("hermes setup model".into()),
            });
        }
        // ... additional field checks per D-06 spec
        errors
    }
}
```

**Error handling:** No `Result` — validation is infallible. Errors are data, not exceptions.

**Test pattern** (`crates/ironhermes-core/tests/` files, e.g. `handlers_cron.rs` lines 124–138):
```rust
#[test]
fn config_missing_api_key_surfaces_error() {
    let mut config = Config::default();
    config.model.api_key = None;
    let errors = config.validate();
    assert!(errors.iter().any(|e| e.path == "model.api_key"));
}

#[test]
fn config_all_defaults_validates_clean() {
    // Config::default() has empty api_key — populate a valid one.
    let mut config = Config::default();
    config.model.api_key = Some("sk-test".into());
    assert!(config.validate().is_empty());
}
```

---

### `crates/ironhermes-core/src/config_setter.rs` (NEW)

**Purpose:** Dotted-path get/set traversal over `Config` using `serde_yaml::Value`. Drives `hermes config set <dotted.key> <value>` and `hermes config get <dotted.key>` (D-08). Also owns the `cache_breaking` warning logic (D-10).

**Closest analog:** `crates/ironhermes-cli/src/memory_setup.rs` `update_config_yaml_memory_provider` (lines 273–301) — this is the only existing function that does a `serde_yaml::Value` load → mutate → overwrite round-trip on `config.yaml`.

**Imports pattern** (from `memory_setup.rs` lines 19–30):
```rust
use anyhow::{Context, Result};
use ironhermes_core::config::Config;
use ironhermes_core::config_schema::ConfigField;  // for cache_breaking lookup
use std::path::Path;
```

**Core dotted-path set pattern** (adapted from `memory_setup.rs` lines 273–301):
```rust
/// Load config.yaml as a serde_yaml::Value, set the value at `dotted_path`,
/// and overwrite the file. Returns the old value (as String) for display.
///
/// This is the ONLY write path for `hermes config set` — do not call
/// Config::save() from this function (that round-trips through Rust structs
/// and drops all serde_yaml::Value-level keys that are not in Config).
pub fn config_set(hermes_home: &Path, dotted_path: &str, value: &str) -> Result<Option<String>> {
    let cfg_path = hermes_home.join("config.yaml");
    let mut doc: serde_yaml::Value = if cfg_path.exists() {
        let text = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        serde_yaml::from_str(&text)
            .unwrap_or(serde_yaml::Value::Mapping(Default::default()))
    } else {
        serde_yaml::Value::Mapping(Default::default())
    };

    let keys: Vec<&str> = dotted_path.split('.').collect();
    // ... walk keys, set leaf, capture old value ...

    let text = serde_yaml::to_string(&doc)?;
    std::fs::write(&cfg_path, text)?;
    Ok(old_value)
}
```

**Cache-breaking warning** (D-10 — display concern, not a block):
```rust
/// Return true if `dotted_path` is tagged `cache_breaking: true` in the
/// ConfigField registry. Caller emits the warning; this function is pure.
pub fn is_cache_breaking(dotted_path: &str, schema: &[ConfigField]) -> bool {
    schema.iter().any(|f| f.key == dotted_path && f.cache_breaking)
}
```

**Error handling** (`anyhow::Result`, same as `memory_setup.rs`): propagate with `.context(...)` at each I/O boundary.

**Test pattern** (from `memory_setup.rs` lines 378–402 — tempdir + YAML assertions):
```rust
#[test]
fn config_set_dotted_path_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    config_set(tmp.path(), "model.default", "openrouter/qwen-2.5-coder-32b").unwrap();
    let text = std::fs::read_to_string(tmp.path().join("config.yaml")).unwrap();
    let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    assert_eq!(parsed["model"]["default"].as_str(), Some("openrouter/qwen-2.5-coder-32b"));
}
```

---

### `crates/ironhermes-cli/src/setup.rs` (NEW)

**Purpose:** Interactive setup wizard runner using rustyline. Dispatches to section question flows. Entry point: `pub async fn run_setup(section: Option<&str>, mode: WizardMode) -> anyhow::Result<()>`.

**Closest analog:** `crates/ironhermes-cli/src/memory_setup.rs` — exact match. `run_memory_setup` is the existing wizard runner; `setup.rs` follows the same structure at the top level, but replaces `std::io::BufRead` prompts with `rustyline::DefaultEditor::readline_with_initial` (D-01 / D-02). The `run_memory_setup_with_io` testability seam pattern must be replicated: the I/O-free inner function takes `&mut Config` and input answers as strings so tests can drive it without a real TTY.

**Imports pattern** (`memory_setup.rs` lines 19–30 + `repl_input.rs` lines 57–58):
```rust
use anyhow::{Context, Result};
use ironhermes_core::config::Config;
use ironhermes_core::constants::get_hermes_home;
use ironhermes_core::wizard::{apply_model_answer, apply_learning_loop_answer, /* ... */};
// Phase 22.3: Configurer trait required for set_max_history_size / set_history_ignore_dups
use rustyline::config::Configurer;
```

**rustyline editor construction — wizard variant** (RESEARCH.md Q2; `repl_input.rs` lines 213–255 for the chat editor, wizard uses simpler variant WITHOUT history persistence per D-01):
```rust
/// Build a fresh rustyline editor for wizard use.
/// NO history persistence — wizard answers must not bleed into chat history.
fn make_wizard_editor() -> Result<rustyline::DefaultEditor> {
    let mut rl = rustyline::DefaultEditor::new()
        .context("failed to initialize rustyline for wizard")?;
    // Do NOT call set_max_history_size or load_history — wizard has no history.
    Ok(rl)
}
```

**readline_with_initial inline-default pattern** (RESEARCH.md Q2, verified from rustyline-15.0.0/src/lib.rs:648):
```rust
/// Prompt with an inline pre-populated default. Empty submission accepts default.
fn prompt_with_default(rl: &mut rustyline::DefaultEditor, prompt: &str, default: &str) -> Result<String> {
    let full_prompt = format!("{} [{}]: ", prompt, default);
    let raw = match rl.readline_with_initial(&full_prompt, (default, "")) {
        Ok(s) => s,
        Err(rustyline::error::ReadlineError::Interrupted) => return Err(anyhow::anyhow!("interrupted")),
        Err(rustyline::error::ReadlineError::Eof) => return Err(anyhow::anyhow!("EOF")),
        Err(e) => return Err(anyhow::anyhow!("readline error: {}", e)),
    };
    let chosen = if raw.trim().is_empty() { default.to_string() } else { raw.trim().to_string() };
    Ok(chosen)
}
```

**Top-level entry and section dispatch** (`memory_setup.rs` lines 112–122 + `cron.rs` pattern):
```rust
pub async fn run_setup(section: Option<&str>, mode: WizardMode) -> Result<()> {
    let hermes_home = get_hermes_home();
    std::fs::create_dir_all(&hermes_home).context("creating HERMES_HOME")?;
    let mut config = Config::load().unwrap_or_default();

    match section {
        None => run_minimum_viable_flow(&mut config, &hermes_home).await?,
        Some("model") => run_model_section(&mut config, &hermes_home).await?,
        Some("memory") => run_memory_section(&mut config, &hermes_home).await?,
        Some("gateway") => run_gateway_section(&mut config, &hermes_home).await?,
        Some("tools") => run_tools_section(&mut config, &hermes_home).await?,
        Some(other) => anyhow::bail!("unknown setup section: {other}"),
    }
    config.save_to(&hermes_home.join("config.yaml"))?;
    Ok(())
}
```

**Testability seam** (`memory_setup.rs` lines 127–131 — inner `_with_io` function):
```rust
/// Testable core — replaces rustyline calls with pre-scripted answer strings.
pub(crate) fn apply_minimum_viable_answers(
    config: &mut Config,
    provider: &str,
    api_key: &str,
    model: &str,
    learning_loop: &str,
) {
    ironhermes_core::wizard::apply_provider_answer(config, provider, "openrouter");
    ironhermes_core::wizard::apply_api_key_answer(config, api_key);
    ironhermes_core::wizard::apply_model_answer(config, model, &ironhermes_core::constants::DEFAULT_MODEL);
    ironhermes_core::wizard::apply_learning_loop_answer(config, learning_loop);
}
```

**env-var naming:** `IRONHERMES_HOME` (not `HERMES_HOME`) per `cron_default_deliver.rs` line 46 and `memory_setup.rs` lines 418/509.

**Error handling:** `anyhow::Result<()>` throughout; `.context(...)` at every I/O call (same as `memory_setup.rs`).

---

### `crates/ironhermes-cli/src/config_cli.rs` (NEW)

**Purpose:** `hermes config set/get/show/migrate/path/env-path` CLI subcommand handlers. These are `async fn` dispatched from `main.rs` like `cron::handle_cron_command`.

**Closest analog:** `crates/ironhermes-cli/src/cron.rs` — the `CronCommands` enum + `handle_cron_command` dispatcher is the direct structural model for a multi-subcommand CLI module. Also `handlers.rs` lines 656–660 (the `cmd_config` stub that is being fleshed out).

**Subcommand enum pattern** (`cron.rs` lines 17–80):
```rust
#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Set a config value by dotted path
    Set {
        /// Dotted key (e.g. model.default)
        key: String,
        /// New value
        value: String,
    },
    /// Get a config value by dotted path
    Get {
        /// Dotted key
        key: String,
    },
    /// Show full active config (secrets masked)
    Show,
    /// Scan installed skills and prompt for missing config/env gaps
    Migrate,
    /// Print path to config.yaml
    Path,
    /// Print path to .env
    EnvPath,
}
```

**Handler dispatcher pattern** (`cron.rs` / `main.rs` dispatch style):
```rust
pub async fn handle_config_command(cmd: ConfigSubcommand) -> anyhow::Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ConfigSubcommand::Set { key, value } => cmd_config_set(&hermes_home, &key, &value).await,
        ConfigSubcommand::Get { key } => cmd_config_get(&hermes_home, &key).await,
        ConfigSubcommand::Show => cmd_config_show(&hermes_home).await,
        ConfigSubcommand::Migrate => cmd_config_migrate(&hermes_home).await,
        ConfigSubcommand::Path => { println!("{}", hermes_home.join("config.yaml").display()); Ok(()) }
        ConfigSubcommand::EnvPath => { println!("{}", hermes_home.join(".env").display()); Ok(()) }
    }
}
```

**Cache-breaking warning emission** (D-10 — display-only, follows `colored` crate pattern used elsewhere in CLI):
```rust
async fn cmd_config_set(hermes_home: &Path, key: &str, value: &str) -> anyhow::Result<()> {
    use colored::Colorize;
    // Check cache-breaking BEFORE writing (warn then persist per D-10)
    if ironhermes_core::config_setter::is_cache_breaking(key, &SCHEMA) {
        println!("{} Changing {} invalidates the prompt cache. Active sessions will pay full cache-miss cost on next turn.",
            "⚠".yellow(), key);
    }
    ironhermes_core::config_setter::config_set(hermes_home, key, value)?;
    println!("Persisted: {} = {}", key, value);
    Ok(())
}
```

**Secret masking for `show`** (D-09 — formatter walk per RESEARCH.md Q3):
```rust
fn mask_secret(value: &str) -> String {
    let prefix_len = value.len().min(6).max(4);
    format!("{}***", &value[..prefix_len])
}
```

**Error handling:** `anyhow::Result<()>`; errors propagate to `main()` which prints them via `eprintln!` (existing main.rs pattern).

---

### `crates/ironhermes-cli/src/preflight.rs` (NEW)

**Purpose:** Pre-flight check function called from `main()` after `Cli::parse()` but before command dispatch (D-05/D-07). Detects missing config or `Config::validate()` failures and launches fix-mode wizard.

**Closest analog:** `crates/ironhermes-cli/src/main.rs` — the existing pre-dispatch guard at lines 207–233 (`is_interactive`, `is_chat_or_bare` checks, yolo flag merging) is the structural model. No single-file analog exists yet — this is the first pre-dispatch middleware module.

**Call-site pattern** (RESEARCH.md Q5; main.rs lines 207–243 for the existing guard shape):
```rust
// In main.rs, after Cli::parse(), before match cli.command:
let skip_preflight = matches!(
    &cli.command,
    Some(Commands::Setup { .. }) | Some(Commands::Config { .. }) | None
);
if !skip_preflight {
    preflight::run_preflight_check(&cli).await?;
}
```

**Module contents:**
```rust
use anyhow::Result;
use ironhermes_core::config::Config;
use crate::Cli;

pub async fn run_preflight_check(_cli: &Cli) -> Result<()> {
    let config_path = ironhermes_core::config::Config::config_path();

    // Missing config.yaml → full wizard
    if !config_path.exists() {
        return crate::setup::run_setup(None, crate::setup::WizardMode::FirstRun).await;
    }

    // Config exists but fails validation → fix mode
    match Config::load() {
        Err(_) => {
            return crate::setup::run_setup(None, crate::setup::WizardMode::FixMode).await;
        }
        Ok(config) => {
            let errors = config.validate();
            if !errors.is_empty() {
                return crate::setup::run_setup(None, crate::setup::WizardMode::FixMode).await;
            }
        }
    }
    Ok(())
}
```

**Error handling:** `anyhow::Result<()>` — any wizard error propagates to `main()`.

---

## Pattern Assignments — Existing Files Modified

---

### `crates/ironhermes-core/src/config_schema.rs` (EXTEND)

**Add:** `cache_breaking: bool` field to `ConfigField`.

**Pattern continuity:** `config_schema.rs` lines 11–28 show Phase 20's extension pattern exactly — every field uses `#[serde(default)]` so existing serialized configs that lack the field parse cleanly (backward-compatible). The existing `secret: bool` field (line 17) is already present and uses `#[serde(default)]`. Add `cache_breaking` identically:

```rust
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
    pub cache_breaking: bool,      // ADD — Phase 23 D-10/D-13
    #[serde(default)]
    pub default: Option<serde_json::Value>,
    #[serde(default)]
    pub choices: Option<Vec<String>>,
    #[serde(default)]
    pub env_var: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}
```

The existing `config_field_roundtrip_all_fields` test (lines 43–57) must be updated to include `cache_breaking: true` in the fixture.

---

### `crates/ironhermes-core/src/commands/handlers.rs` (EXTEND)

**Add:** Replace the `cmd_config` stub (line 656) with a real dispatcher. Add `cmd_setup` as a companion stub that defers to `ironhermes_cli::setup` (not wired here — core handlers are slash-command handlers, not top-level CLI dispatchers; the slash `/config` handler stays as a one-liner per the existing stub pattern).

**Pattern continuity:** Lines 640–660 show the guard pattern for handlers that need optional context:

```rust
// Existing pattern (lines 640–654): guard on optional context field, return informational text if missing
fn cmd_fast(ctx: &CommandContext) -> CommandResult {
    let resolver = match &ctx.provider_resolver {
        Some(r) => r.clone(),
        None => return CommandResult::Output("Provider resolver not configured.".to_string()),
    };
    // ... real logic
}

// Phase 23 extension — stub kept minimal; real work is in config_cli.rs
fn cmd_config(_ctx: &CommandContext) -> CommandResult {
    CommandResult::Output(
        "Use `hermes config show` to inspect configuration, or `hermes config set <key> <value>` to change it.".to_string(),
    )
}
```

The dispatch table (lines 18–81) adds `"config"` routing to the updated `cmd_config` — no new dispatch entry needed since the key already exists at line 50.

---

### `crates/ironhermes-cli/src/main.rs` (EXTEND)

**Add:** `Setup { section: Option<String> }` and `Config { subcommand: ConfigSubcommand }` variants to the `Commands` enum; `preflight::run_preflight_check` call after `Cli::parse()`.

**Pattern continuity:** `Commands` enum (lines 91–147) — all variants follow the same clap derive shape. The `Memory` variant (lines 132–136) with an inner `MemorySubcommand` is the exact model for the new `Config` variant:

```rust
// Existing pattern (lines 132–136):
/// Memory provider management (Plan 20-03, D-08).
Memory {
    #[command(subcommand)]
    action: MemorySubcommand,
},

// Phase 23 additions follow the same shape:
/// Interactive first-run setup wizard
Setup {
    /// Section to configure: model, memory, gateway, tools (default: all)
    section: Option<String>,
},
/// Manage configuration values
Config {
    #[command(subcommand)]
    subcommand: config_cli::ConfigSubcommand,
},
```

Dispatch arm pattern (lines 284–293 for the `Memory` dispatch):
```rust
// Existing:
Some(Commands::Memory { action: MemorySubcommand::Setup }) => {
    memory_setup::run_memory_setup(&cli).await
}

// Phase 23:
Some(Commands::Setup { section }) => {
    setup::run_setup(section.as_deref(), setup::WizardMode::Explicit).await
}
Some(Commands::Config { subcommand }) => {
    config_cli::handle_config_command(subcommand).await
}
```

Module declaration follows `mod memory_setup;` pattern (line 32):
```rust
mod config_cli;
mod preflight;
mod setup;
```

---

## Test File Patterns

---

### `crates/ironhermes-core/tests/wizard_flow.rs` (NEW)

**Closest test analog:** `crates/ironhermes-core/tests/handlers_cron.rs`

`handlers_cron.rs` is the best pure-function test analog in `ironhermes-core/tests/`: it drives `dispatch()` with minimal fakes, no tempdir, no async (lines 124–138 show the synchronous `#[test]` shape). `wizard_flow.rs` follows the same shape but calls `apply_*_answer` functions directly.

**Pattern** (from `handlers_cron.rs` lines 124–138):
```rust
//! Pure-function tests for wizard.rs `apply_*_answer` functions.
//! No rustyline, no I/O, no async.

use ironhermes_core::config::Config;
use ironhermes_core::wizard::*;

#[test]
fn apply_model_uses_default_on_empty_input() {
    let mut config = Config::default();
    apply_model_answer(&mut config, "", "openrouter/qwen-2.5-coder-32b");
    assert_eq!(config.model.default, "openrouter/qwen-2.5-coder-32b");
}

#[test]
fn apply_learning_loop_yes_sets_all_keys() {
    let mut config = Config::default();
    apply_learning_loop_answer(&mut config, "y");
    assert!(config.memory.memory_enabled);
    assert!(config.memory.user_profile_enabled);
    // learning.* keys verified similarly
}

#[test]
fn apply_learning_loop_empty_input_defaults_to_yes() {
    let mut config = Config::default();
    apply_learning_loop_answer(&mut config, "");
    assert!(config.memory.memory_enabled, "empty = default YES per D-14");
}

#[test]
fn apply_learning_loop_no_writes_explicit_false() {
    let mut config = Config::default();
    apply_learning_loop_answer(&mut config, "n");
    assert!(!config.memory.memory_enabled, "n must write explicit false, not absent");
}
```

---

### `crates/ironhermes-cli/tests/setup_wizard.rs` (NEW)

**Closest test analog:** `crates/ironhermes-cli/tests/cron_default_deliver.rs`

`cron_default_deliver.rs` is the canonical tempdir + `IRONHERMES_HOME` redirect + `Config::load()` pattern for integration tests that exercise config file I/O without touching the real home directory. `setup_wizard.rs` follows the same structure.

**Pattern** (from `cron_default_deliver.rs` lines 16–58):
```rust
//! Integration tests for `hermes setup` wizard flow.
//! IRONHERMES_HOME is redirected to a TempDir for each test.
//!
//! Run: `cargo test -p ironhermes-cli --test setup_wizard`

use ironhermes_core::config::Config;
use tempfile::TempDir;

fn env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
async fn minimum_viable_flow_writes_config_yaml() {
    let _guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let tmp = TempDir::new().unwrap();
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }

    // Drive the testability seam (not the real rustyline path)
    let mut config = Config::default();
    ironhermes_cli::setup::apply_minimum_viable_answers(
        &mut config, "openrouter", "sk-test-key",
        "openrouter/qwen-2.5-coder-32b", "y",
    );
    config.save_to(&tmp.path().join("config.yaml")).unwrap();

    assert!(tmp.path().join("config.yaml").exists());
    let loaded = Config::load().expect("config must load after wizard");
    assert!(loaded.memory.memory_enabled, "Learning Loop must default ON");
}
```

Note: test rustyline TTY interaction is NOT tested directly (per RESEARCH.md "What NOT to Test") — only the pure-function seam is exercised.

---

### `crates/ironhermes-cli/tests/config_migrate_discovery.rs` (NEW)

**Closest test analog:** `crates/ironhermes-cli/tests/skills_cmd_test.rs`

`skills_cmd_test.rs` (lines 13–25) uses the `with_hermes_home` tempdir helper and exercises skills-related file I/O — the closest existing test to `config migrate`'s skill-frontmatter scanning.

**Pattern** (from `skills_cmd_test.rs` lines 13–25):
```rust
//! Tests for `hermes config migrate` skill-gap discovery.
//!
//! Run: `cargo test -p ironhermes-cli --test config_migrate_discovery`

use tempfile::TempDir;

fn with_ironhermes_home<F: FnOnce(&std::path::Path)>(f: F) {
    let tmp = TempDir::new().unwrap();
    let prev = std::env::var("IRONHERMES_HOME").ok();
    unsafe { std::env::set_var("IRONHERMES_HOME", tmp.path()); }
    f(tmp.path());
    match prev {
        Some(v) => unsafe { std::env::set_var("IRONHERMES_HOME", v) },
        None => unsafe { std::env::remove_var("IRONHERMES_HOME") },
    }
}

#[test]
fn migrate_with_no_skills_installed_prints_nothing_to_fill() {
    with_ironhermes_home(|home| {
        std::fs::create_dir_all(home.join("skills")).unwrap();
        // invoke config_migrate discovery scan — asserts empty gap list
    });
}
```

---

## Workspace / Cargo.toml Deltas

**New dev-deps:**
- `ironhermes-cli/Cargo.toml` dev-deps: `tempfile = "3"` is already present (line 65), `insta` is already present (line 70), `assert_cmd` is already present (line 71). No new dev-deps needed for CLI tests.
- `ironhermes-core/Cargo.toml` dev-deps: `tempfile = "3"` is already present (line 29). No new dev-dep needed.

**New runtime deps:** NONE. All runtime dependencies (`rustyline = "15"`, `serde_yaml = "0.9"`, `anyhow`, `colored`) are already workspace-pinned and present in `ironhermes-cli/Cargo.toml`.

**Module wiring required in `ironhermes-core/src/lib.rs`:** Add `pub mod wizard;` and `pub mod config_validate;` and `pub mod config_setter;` (following the existing `pub mod config;` / `pub mod config_schema;` pattern).

---

## Anti-Patterns to Avoid (codebase-derived)

1. **Calling `Config::save()` from `config_setter.rs`.** `Config::save()` round-trips through Rust struct serialization, which drops any `serde_yaml::Value`-level keys not represented in the current `Config` struct (e.g., `learning.*` keys Phase 23 adds that Phase 32/33 haven't wired into the struct yet). The `config_setter` must do `serde_yaml::Value` load → mutate → `serde_yaml::to_string` → `std::fs::write` directly, exactly as `update_config_yaml_memory_provider` does (memory_setup.rs lines 273–301).

2. **Touching `HERMES_HOME` env var in tests instead of `IRONHERMES_HOME`.** The real env var is `IRONHERMES_HOME` (confirmed: `cron_default_deliver.rs` line 46, `memory_setup.rs` lines 418/509, `constants.rs` `get_hermes_home()`). `skills_cmd_test.rs` uses `HERMES_HOME` — that is a legacy inconsistency specific to the skills module. Phase 23 tests MUST use `IRONHERMES_HOME`.

3. **Creating a rustyline history file for the wizard.** `repl_input.rs` lines 254–275 show the correct history-load pattern. The wizard MUST NOT call `rl.set_max_history_size`, `rl.load_history`, or `rl.save_history` — wizard answers must not bleed into REPL chat history. Only the REPL's `ReplInputChannel` owns history persistence (D-01/Q2).

4. **Using `std::io::BufRead` prompts instead of `rustyline::readline_with_initial` in the real wizard path.** `memory_setup.rs` uses `BufRead`/`Write` for its testability seam — that is the test abstraction, not the production path. The production `setup.rs` must use `rustyline::DefaultEditor::readline_with_initial` for inline defaults (D-01/Q2).

5. **Placing cross-crate types (WizardMode, ConfigSubcommand) in `ironhermes-cli`.** Per D-12 and the Phase 22.4.2.2 cross-crate type precedent (`OriginDecision` lives in `ironhermes-core/src/config.rs` lines 550–600): any type that `ironhermes-core` or a test in `ironhermes-core/tests/` needs to reference must live in `ironhermes-core`. `WizardMode` (if used in the testability seam) belongs in `ironhermes-core::wizard`.

6. **Embedding `impl Default` directly on the new `LearningConfig` struct (if introduced) without `#[serde(default)]` on the field in Config.** Every Config field uses `#[serde(default)]` (e.g. `config.rs` lines 62–98). Missing `#[serde(default)]` on a new `Config.learning` field will break deserialization of existing `config.yaml` files that lack the `learning:` section.

---

## Cross-References

- **Phase 20:** `ConfigField` + schema extension pattern — `config_schema.rs` lines 11–28. Phase 23 adds `cache_breaking: bool` following the same `#[serde(default)]` pattern as `secret: bool`.
- **Phase 21.6 (deployment setup):** `memory_setup.rs` is the direct predecessor — the `.env`-write pattern (lines 206–227), the `update_config_yaml_*` pattern (lines 273–301), and the testability seam pattern (lines 127–131) all carry forward to Phase 23. `tempfile` dev-dep was confirmed present in `ironhermes-cli/Cargo.toml` line 65.
- **Phase 22.3 (rustyline 15 wiring):** `repl_input.rs` lines 245–275 — `set_history_ignore_dups(true)`, NOT `set_history_duplicates`. The wizard must NOT replicate this; it uses a no-history editor. The `Configurer` trait import (line 57) is required to call these methods.
- **Phase 22.4.2.2 (cross-crate plain-String type pattern):** `OriginDecision` enum in `config.rs` lines 550–600. `WizardMode` and any wizard dispatch enum introduced in Phase 23 follow the same rule: defined in `ironhermes-core`, consumers use plain-String fields at crate boundaries. No downstream enums embedded in core types.
- **Phase 27 (Prompt Caching, downstream):** `cache_breaking: bool` tags established in Phase 23 will be refined by Phase 27's system_and_3 cache strategy. Phase 23 establishes the warning surface; Phase 27 may extend the field-tagging list. Do not over-engineer the `is_cache_breaking` lookup — a simple `schema.iter().any(...)` is sufficient.
