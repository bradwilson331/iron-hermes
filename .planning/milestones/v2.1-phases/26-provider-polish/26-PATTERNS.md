# Phase 26: Provider Polish - Pattern Map

**Mapped:** 2026-04-29
**Files analyzed:** 10 new/modified files
**Analogs found:** 10 / 10

---

## File Classification

| New/Modified File | Role | Data Flow | Closest Analog | Match Quality |
|-------------------|------|-----------|----------------|---------------|
| `crates/ironhermes-core/src/config.rs` | config schema | transform | `crates/ironhermes-core/src/config.rs` (ToolsConfig block, Phase 25) | exact — same file, same section-per-feature pattern |
| `crates/ironhermes-core/src/provider.rs` | resolver internals | request-response | `crates/ironhermes-core/src/provider.rs` (existing build + resolve_role) | exact — surgery on the same file |
| `crates/ironhermes-core/src/commands/provider_display.rs` | display helper | transform | `crates/ironhermes-core/src/commands/toolset_display.rs` | exact role-match |
| `crates/ironhermes-cli/src/provider_cmd.rs` | CLI subcommand | request-response | `crates/ironhermes-cli/src/toolset_cmd.rs` | exact role-match |
| `crates/ironhermes-cli/src/main.rs` | CLI wiring | request-response | `crates/ironhermes-cli/src/main.rs` (Toolset variant, line 169) | exact — same enum, same pattern |
| `crates/ironhermes-agent/src/engine_factory.rs` | agent wire-through | request-response | `crates/ironhermes-agent/src/engine_factory.rs` (compression role, lines 84-114) | exact — same file, extend existing pattern |
| `crates/ironhermes-cli/src/setup.rs` | wizard stage | request-response | `crates/ironhermes-cli/src/setup.rs` (run_minimum_viable_flow, lines 101-120) | exact — same file, add auxiliary stage |
| `crates/ironhermes-core/src/wizard.rs` | wizard apply-fn | transform | `crates/ironhermes-core/src/wizard.rs` (apply_provider_answer, lines 37-44) | exact role-match |
| `crates/ironhermes-cli/tests/provider_integration.rs` | integration test | request-response | `crates/ironhermes-cli/tests/toolset_integration.rs` (env_lock + CARGO_BIN_EXE pattern) | exact role-match |
| `crates/ironhermes-core/src/provider.rs` (unit tests) | unit test | CRUD | `crates/ironhermes-hub/tests/audit_test.rs` (wiremock + ENV_LOCK RAII guard) | role-match |

---

## Pattern Assignments

### `crates/ironhermes-core/src/config.rs` (config schema, transform)

**Analog:** `crates/ironhermes-core/src/config.rs` — ToolsConfig block (lines 1–53) + ProviderConfig struct (lines 68–89)

**Existing ProviderConfig shape to extend** (lines 69–89):
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,      // Phase 26: deprecate, keep with banner
    pub api_mode: Option<ApiMode>,
    pub default_model: Option<String>,
    pub fallback_providers: Vec<String>,
    // Phase 26 adds:
    // pub api_key_env: Option<String>,
    // pub disabled: Option<bool>,
}
```

**New struct shape to add — AuxiliaryConfig** (model from ToolsConfig / ToolsetEntry pattern, lines 10–16):
```rust
/// Phase 26 D-05: top-level auxiliary routing block.
/// Plain Strings per D-18 / Phase 22.4.2.2 cross-crate rule.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuxiliaryConfig {
    pub provider: String,
    pub model: String,
}
```

**Config struct field pattern — add alongside `tools:` field** (lines 148–151):
```rust
// Phase 25 D-22: toolset enable/disable configuration.
#[serde(default)]
pub tools: ToolsConfig,
// Phase 26 D-05: auxiliary model routing (PROV-06).
// #[serde(default)]
// pub auxiliary: Option<AuxiliaryConfig>,
```

**What the planner should copy:**
- `ToolsetEntry` / `ToolsConfig` for the new `AuxiliaryConfig` / `RoleOverride` struct shapes (all `String` / `Option<String>` fields, `#[serde(default)]` on struct, `Default` derived)
- The `Config` struct field declaration pattern: `#[serde(default)]` on each new field, one comment per feature
- `ModelRoleConfig` (lines 102–108) is the existing `RoleOverride` shape — use it directly as a type alias or add `pub type RoleOverride = ModelRoleConfig;` rather than creating a new struct

---

### `crates/ironhermes-core/src/provider.rs` — resolver changes (resolver internals, request-response)

**Analog:** Same file — `ProviderResolver::build` (lines 100–256) + `resolve_role` (lines 275–287)

**Step 4 API key resolution loop to replace** (lines 200–222):
```rust
// --- 4. Resolve API keys with precedence (D-03, PROV-03, PROV-04) ---
let main = &config.model.provider;

for (name, endpoint) in endpoints.iter_mut() {
    let config_key = config.providers.get(name).and_then(|p| p.api_key.clone());

    let env_key: Option<String> = match name.as_str() {
        "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
        "anthropic"  => std::env::var("ANTHROPIC_API_KEY").ok(),
        "openai"     => std::env::var("OPENAI_API_KEY").ok(),
        // custom providers: try OPENAI_API_KEY as generic fallback
        _ => std::env::var("OPENAI_API_KEY").ok(),    // <-- D-11 DELETES THIS ARM
    };

    let model_key = if name == main { config.model.api_key.clone() } else { None };

    endpoint.api_key = config_key
        .or(env_key)
        .or(model_key)
        .or_else(|| endpoint.api_key.clone());
}
```

**D-11 replacement pattern** (delete wildcard arm; replace with api_key_env lookup):
```rust
// Phase 26 D-11: per-provider api_key_env lookup; no wildcard fallback.
let env_key: Option<String> = if let Some(ref env_name) =
    config.providers.get(name).and_then(|p| p.api_key_env.as_ref().cloned())
{
    std::env::var(env_name).ok()
} else {
    // Built-in legacy fallback only (D-12) — custom providers get None.
    match name.as_str() {
        "openrouter" => legacy_env_with_banner(name, "OPENROUTER_API_KEY"),
        "anthropic"  => legacy_env_with_banner(name, "ANTHROPIC_API_KEY"),
        "openai"     => legacy_env_with_banner(name, "OPENAI_API_KEY"),
        _            => None,   // D-11: no wildcard; custom providers with no api_key_env => None
    }
};
```

**resolve_role body to extend** (lines 275–287):
```rust
pub fn resolve_role(&self, role: &str) -> Option<ResolvedEndpoint> {
    let role_cfg = self.roles.get(role)?;
    let base_endpoint = if role_cfg.provider == "main" {
        self.endpoints.get(&self.main_provider)?
    } else {
        self.endpoints.get(&role_cfg.provider)?
    };
    let mut ep = base_endpoint.clone();
    if let Some(ref model) = role_cfg.model {
        ep.default_model = model.clone();
    }
    Some(ep)
    // Phase 26 D-05: after the roles.get() returns None, fall through to
    // self.auxiliary config before returning None to caller.
}
```

**D-12 once-only deprecation banner pattern** (implement as module-level fn in provider.rs):
```rust
// Source: derived from RESEARCH.md Pattern 1 + audit_test.rs RAII guard approach.
// Place at module level in provider.rs, called only from build().
fn legacy_env_with_banner(provider_name: &str, var_name: &str) -> Option<String> {
    use std::sync::OnceLock;
    static WARNED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>>
        = OnceLock::new();
    let warned = WARNED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    let key = std::env::var(var_name).ok()?;
    let mut set = warned.lock().unwrap();
    if set.insert(provider_name.to_string()) {
        eprintln!(
            "[provider:{}] using deprecated env var {} \u{2014} \
             set providers.{}.api_key_env in config.yaml to silence this warning",
            provider_name, var_name, provider_name
        );
    }
    Some(key)
}
```

**What the planner should copy:**
- The existing step-numbered comment structure in `build()` — keep the pattern, add a Step 3b for D-02 migration and update Step 4 in place
- `resolve_role` returns `Option<ResolvedEndpoint>` (by value / clone) — keep signature; extend body to check `self.auxiliary` after `self.roles.get(role)` fails
- The `is_provider_url_safe()` function (lines 71–80) for validating new `providers.*.base_url` writes — reuse as-is
- `ResolvedEndpoint` Debug impl (lines 46–58) already redacts `api_key` — never add new Debug paths that bypass this

---

### `crates/ironhermes-core/src/commands/provider_display.rs` (display helper, transform)

**Analog:** `crates/ironhermes-core/src/commands/toolset_display.rs` (full file, 133 lines)

**Row struct pattern** (lines 9–17):
```rust
pub struct ToolsetRow {
    pub name: String,
    pub enabled: bool,
    pub member_count: usize,
    pub available_count: usize,
    pub member_summary: String,
}
```

**Render function pattern** (lines 31–52):
```rust
pub fn render_toolset_list(rows: Vec<ToolsetRow>) -> String {
    let header = format!(
        "{:<10} {:<10} {:<7} {}\n",
        "TOOLSET", "STATUS", "TOOLS", "AVAILABLE"
    );
    let mut out = header;
    for row in &rows {
        let status = if row.enabled { "enabled" } else { "disabled" };
        let avail = format!("{}/{}", row.available_count, row.member_count);
        // ...
        out.push_str(&format!(
            "{:<10} {:<10} {:<7} {}{}\n",
            row.name, status, row.member_count, avail, detail
        ));
    }
    out
}
```

**File-level doc comment pattern** (lines 1–7):
```rust
//! Shared toolset display helpers — Phase 25, D-06 / Critical Constraint 1.
//!
//! Lives in `ironhermes-core` so BOTH the CLI subcommand (`toolset_cmd.rs`)
//! and the slash command handler (`handlers.rs`) can call it without creating
//! a circular dependency (ironhermes-cli → ironhermes-core, NEVER the reverse).
//!
//! Pure functions: no I/O, no environment access — only rendering.
```

**What the planner should copy:**
- File-level doc comment: substitute "toolset" → "provider" and reference Phase 26 D-14
- `ToolsetRow` → `ProviderRow` with fields: `name: String`, `base_url: String`, `api_key_status: String` (e.g., `"✓ $OPENAI_API_KEY"` or `"✗ missing $VAR"`), `default_model: String`, `role: String`, `fallbacks: String`, `disabled: bool`
- `render_toolset_list` → `render_provider_list`: same `{:<N}` left-aligned column format; header = `NAME / BASE_URL / API_KEY / MODEL / ROLE / FALLBACKS` per CONTEXT.md §Specifics
- `render_toolset_show` → `render_provider_show`: same header block format
- Tests follow the same `make_row` helper + assert-contains pattern (lines 82–132)
- Widths: NAME=18, BASE_URL=36, API_KEY=22, MODEL=20, ROLE=10, FALLBACKS=remainder

---

### `crates/ironhermes-cli/src/provider_cmd.rs` (CLI subcommand, request-response)

**Analog:** `crates/ironhermes-cli/src/toolset_cmd.rs` (full file, 381 lines)

**Subcommand enum pattern** (lines 17–31):
```rust
#[derive(Subcommand)]
pub enum ToolsetSubcommand {
    List,
    Enable { name: String },
    Disable { name: String },
    Show { name: String },
    Setup,
}
```

**Dispatcher pattern** (lines 33–45):
```rust
pub async fn handle_toolset_command(cmd: ToolsetSubcommand, _profile_name: &str) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ToolsetSubcommand::List => cmd_toolset_list(&hermes_home).await,
        ToolsetSubcommand::Enable { name } => cmd_toolset_enable(&hermes_home, &name).await,
        // ...
    }
}
```

**Enable/disable + cache-break banner pattern** (lines 77–113):
```rust
pub async fn cmd_toolset_enable(hermes_home: &Path, name: &str) -> Result<()> {
    let validated = validate_toolset_name(name)?;
    check_known_toolset(&validated)?;
    config_setter::config_set(
        hermes_home,
        &format!("tools.toolsets.{}.enabled", validated),
        "true",
    )
    .with_context(|| format!("failed to enable toolset {}", validated))?;
    // T-25-03: cache-break banner on stderr (not stdout).
    eprintln!(
        "{} [toolset: {}] enabled \u{2014} schema cache will rebuild on next LLM call",
        "\u{26a0}".yellow(),
        validated,
    );
    Ok(())
}
```

**Config load helper pattern** (lines 195–206):
```rust
fn load_tools_config(hermes_home: &Path) -> ToolsConfig {
    let cfg_path = hermes_home.join("config.yaml");
    if !cfg_path.exists() { return ToolsConfig::default(); }
    let text = match std::fs::read_to_string(&cfg_path) {
        Ok(t) => t,
        Err(_) => return ToolsConfig::default(),
    };
    let config: ironhermes_core::Config = serde_yaml::from_str(&text).unwrap_or_default();
    config.tools
}
```

**What the planner should copy:**
- Subcommand enum: `ProviderSubcommand` with `List { json: bool }`, `Show { name: String }`, `Test { name: String }`, `Enable { name: String }`, `Disable { name: String }` (no `Setup` — that lives in `hermes setup`)
- `validate_toolset_name` → `validate_provider_name`: call `profile::validate_profile_name` (same function — provider names use same slug format `[a-z0-9][a-z0-9-]*`)
- Enable/disable: `config_setter::config_set(hermes_home, &format!("providers.{}.disabled", validated), "false"/"true")`
- Cache-break banner on stderr: `eprintln!("{} [provider: {}] config changed \u{2014} schema cache will rebuild on next LLM call", "\u{26a0}".yellow(), validated)`
- Config load helper: load full `Config` (not just a sub-struct), call `ProviderResolver::build(&config)` for list/show/test
- `cmd_provider_test`: use `reqwest::Client` → `GET {base_url}/models`; on 404 fall back to `POST {base_url}/chat/completions`; report HTTP status + latency; NEVER include `endpoint.api_key` value in output (D-15); show env var name only

---

### `crates/ironhermes-cli/src/main.rs` (CLI wiring, request-response)

**Analog:** Same file — `Toolset` variant (lines 168–172)

**Toolset variant to mirror** (lines 168–172):
```rust
/// Manage toolsets — enable/disable, list, show (Phase 25, D-04).
Toolset {
    #[command(subcommand)]
    subcommand: toolset_cmd::ToolsetSubcommand,
},
```

**What the planner should copy:**
- Add immediately after the `Toolset` block:
  ```rust
  /// Manage providers — list/show/test/enable/disable (Phase 26, D-14).
  Provider {
      #[command(subcommand)]
      subcommand: provider_cmd::ProviderSubcommand,
  },
  ```
- Add `Commands::Provider { subcommand } => provider_cmd::handle_provider_command(subcommand, &profile_name).await` to the match arm in the main dispatch block (same pattern as the `Toolset` arm nearby)

---

### `crates/ironhermes-agent/src/engine_factory.rs` (agent wire-through, request-response)

**Analog:** Same file — `build_role_client(resolver, "compression")` pattern (lines 84–114)

**Compression role pattern to extend** (lines 83–108):
```rust
"summarizing" => {
    let client = match build_role_client(resolver, "compression") {
        Ok(Some(c)) => c,
        Ok(None) => {
            tracing::warn!("compression role unconfigured, falling back to main client");
            match build_main_client(resolver) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = ?e, "main client resolution failed, falling back to local_prune");
                    return build_local(hooks, tracker, memory_manager, &sid);
                }
            }
        }
        Err(e) => { /* ... same fallback chain ... */ }
    };
    let model = resolver
        .resolve_role("compression")
        .map(|ep| ep.default_model.clone())
        .or_else(|| Some(resolver.resolve_for_main().default_model.clone()));
```

**D-07 caller pattern** (from RESEARCH.md — applied at each new role call site):
```rust
// Phase 26 D-07: resolve role with fallback to main.
let endpoint = resolver
    .resolve_role("vision")   // or "session_search" / "skills_hub" / "mcp_helper"
    .unwrap_or_else(|| resolver.resolve_for_main().clone());
```

**What the planner should copy:**
- The three-branch `Ok(Some(c))` / `Ok(None)` / `Err(e)` fallback chain from lines 84–108 for each new role wire-through site
- `tracing::warn!` at each fallback point (not `eprintln!`) — agent crate uses tracing
- `resolver.resolve_role(role).map(|ep| ep.default_model.clone()).or_else(|| Some(resolver.resolve_for_main().default_model.clone()))` for model-identifier derivation
- Plan 3 must grep for `"vision_model"`, `"mcp_helper"`, `"skills_hub"`, `"session_search"` in `crates/ironhermes-agent/src/` to find the actual call sites before implementing

---

### `crates/ironhermes-cli/src/setup.rs` (wizard stage, request-response)

**Analog:** Same file — `run_minimum_viable_flow` (lines 101–120) + `prompt_with_default` (lines 31–50)

**Minimum viable flow pattern to extend** (lines 101–117):
```rust
async fn run_minimum_viable_flow(
    config: &mut Config,
    hermes_home: &Path,
    rl: &mut rustyline::DefaultEditor,
    _mode: WizardMode,
) -> Result<()> {
    use ironhermes_core::config_setter;

    // 1. Provider
    let provider = prompt_with_default(rl, "Provider", "openrouter")?;
    apply_provider_answer(config, &provider, "openrouter");

    // 2. API key
    let api_key = prompt_required(rl, &format!("API key for {}", provider))?;
    apply_api_key_answer(config, &api_key);
    // ...
}
```

**What the planner should copy:**
- Add an optional auxiliary stage after the model prompt:
  ```rust
  // Phase 26 D-19: optional auxiliary routing stage.
  let aux_provider = prompt_with_default(rl, "Auxiliary provider (cheaper model, optional — Enter to skip)", "")?;
  if !aux_provider.trim().is_empty() {
      let aux_model = prompt_with_default(rl, "Auxiliary model", "gpt-4o-mini")?;
      apply_auxiliary_answer(config, &aux_provider, &aux_model);
  }
  ```
- `apply_auxiliary_answer` lives in `wizard.rs` (same pattern as `apply_provider_answer` / `apply_api_key_answer`)
- Import via `use ironhermes_core::wizard::apply_auxiliary_answer;` in the imports block (lines 12–15)
- The `setup.rs` section dispatch (`Some("agent")` line 84) already has a placeholder comment — replace `"section deferred to Phase 26"` with actual provider/auxiliary handling

---

### `crates/ironhermes-core/src/wizard.rs` (wizard apply-fn, transform)

**Analog:** Same file — `apply_provider_answer` (lines 37–44) and `apply_api_key_answer` (lines 48–54)

**apply_provider_answer pattern to copy** (lines 37–44):
```rust
pub fn apply_provider_answer(config: &mut Config, raw_input: &str, default: &str) {
    let trimmed = raw_input.trim();
    if !trimmed.is_empty() {
        config.model.provider = trimmed.to_string();
    } else if config.model.provider.is_empty() {
        config.model.provider = default.to_string();
    }
}
```

**What the planner should copy:**
- New function signature: `pub fn apply_auxiliary_answer(config: &mut Config, provider: &str, model: &str)`
- Body: set `config.auxiliary = Some(AuxiliaryConfig { provider: provider.trim().to_string(), model: model.trim().to_string() })`; only call when `!provider.trim().is_empty()`
- No I/O — pure mutation, identical pattern to existing `apply_*` functions
- Import `crate::config::AuxiliaryConfig` at top of wizard.rs

---

### `crates/ironhermes-cli/tests/provider_integration.rs` (integration test, request-response)

**Analog:** `crates/ironhermes-cli/tests/toolset_integration.rs` (lines 1–80)

**env_lock pattern** (lines 12–21):
```rust
use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}
```

**CARGO_BIN_EXE subprocess pattern** (lines 33–48):
```rust
#[test]
fn toolset_enable_disable_persists() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
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
        .args(["toolset", "enable", "web"])
        .output()
        .expect("failed to run ironhermes binary");
    assert!(out.status.success(), "...: stderr={}", String::from_utf8_lossy(&out.stderr));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[toolset: web] enabled"), "...: {}", stderr);
}
```

**wiremock pattern** (from `crates/ironhermes-hub/tests/audit_test.rs` lines 9–60):
```rust
use wiremock::matchers::{method, path, header};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn key_does_not_leak_to_wrong_provider() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let server = MockServer::start().await;
    // Server captures the Authorization header from the outbound request.
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "pong"}}]
        })))
        .mount(&server)
        .await;

    // Set leak env var and point custom provider at mock server.
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-leaked"); }
    // ... build config with my-local-llm pointing at server.uri() ...
    // ... assert captured request Authorization header does NOT contain "sk-leaked" ...
    unsafe { std::env::remove_var("OPENAI_API_KEY"); }
}
```

**What the planner should copy:**
- `env_lock()` definition verbatim (this is a new static, not shared with toolset_integration.rs — separate test binary)
- `CARGO_BIN_EXE_ironhermes` guard pattern verbatim (early return with eprintln! on missing)
- `tempfile::TempDir::new()` + `.env("IRONHERMES_HOME", tmp.path())` for all subprocess tests
- `MockServer::start().await` for wiremock tests; use `server.uri()` as the `base_url` for the custom provider
- `unsafe { std::env::set_var(...) }` / `unsafe { std::env::remove_var(...) }` — Rust 2024 edition requires `unsafe` block for env mutation
- D-15 assertion pattern: `assert!(!stdout.contains("sk-") && !stderr.contains("sk-"), "key must not appear in output")`

---

### `crates/ironhermes-core/src/provider.rs` unit tests (unit test, CRUD)

**Analog:** `crates/ironhermes-hub/tests/audit_test.rs` RAII guard pattern (lines 21–46) + `toolset_integration.rs` env_lock

**Unit test env isolation pattern** (from audit_test.rs lines 21–46):
```rust
// RAII guard restores env var on drop — cleaner than manual remove_var in tests.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}
impl EnvGuard {
    fn set(key: &'static str, val: &str) -> Self {
        let prev = std::env::var(key).ok();
        unsafe { std::env::set_var(key, val); }
        Self { key, prev }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}
```

**D-21 unit test pseudocode** (from CONTEXT.md §Specifics, adapted):
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, ProviderConfig};
    use std::sync::OnceLock;

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn legacy_openai_key_does_not_leak_to_unknown_provider() {
        let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-leaked"); }
        let mut config = Config::default();
        config.providers.insert("my-local-llm".to_string(), ProviderConfig {
            base_url: Some("http://localhost:8080/v1".to_string()),
            api_key_env: None,
            ..Default::default()
        });
        let resolver = ProviderResolver::build(&config).unwrap();
        let endpoint = resolver.resolve("my-local-llm").unwrap();
        assert_eq!(endpoint.api_key, None, "OPENAI_API_KEY MUST NOT leak to my-local-llm");
        unsafe { std::env::remove_var("OPENAI_API_KEY"); }
    }
}
```

**What the planner should copy:**
- `env_lock()` pattern inside `#[cfg(test)]` module — same `OnceLock<Mutex<()>>` static
- RAII `EnvGuard` from audit_test.rs for tests that need guaranteed cleanup on panic
- `unsafe { std::env::set_var / remove_var }` — required by Rust 2024 edition
- Banner once-only test: implement as subprocess integration test (not unit test) because `OnceLock` cannot be reset between tests in the same process

---

## Shared Patterns

### env_lock — process-wide env mutation serialization

**Source:** `crates/ironhermes-cli/tests/toolset_integration.rs` lines 12–21
**Apply to:** All test files that call `std::env::set_var` / `remove_var` (provider_integration.rs, provider.rs unit tests)

```rust
use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// In each test:
let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
```

Note: each test binary has its own static. `provider_integration.rs` and `provider.rs` tests each declare their own `env_lock()` — they do not share across crate boundaries.

---

### CARGO_BIN_EXE subprocess pattern

**Source:** `crates/ironhermes-cli/tests/toolset_integration.rs` lines 33–48
**Apply to:** `provider_integration.rs` for D-20 tests 1 (key_does_not_leak), 3 (custom_provider_selectable_by_name), and D-15 (provider_test_does_not_print_key)

```rust
let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
    Ok(p) => p,
    Err(_) => {
        eprintln!("Skipping <test_name>: CARGO_BIN_EXE_ironhermes not set");
        return;
    }
};
let tmp = tempfile::TempDir::new().unwrap();
let out = std::process::Command::new(&bin)
    .env("IRONHERMES_HOME", tmp.path())
    .args([/* ... */])
    .output()
    .expect("failed to run ironhermes binary");
```

---

### wiremock MockServer setup

**Source:** `crates/ironhermes-hub/tests/audit_test.rs` lines 52–60
**Apply to:** `provider_integration.rs` D-20 tests 1 and 2 (wiremock-based)

```rust
use wiremock::matchers::{method, path, header};
use wiremock::{Mock, MockServer, ResponseTemplate};

// In #[tokio::test]:
let server = MockServer::start().await;
Mock::given(method("POST"))
    .and(path("/chat/completions"))
    .respond_with(ResponseTemplate::new(200).set_body_json(/*...*/))
    .mount(&server)
    .await;
// Use server.uri() as base_url in the provider config under test.
```

---

### Cache-break stderr banner format

**Source:** `crates/ironhermes-cli/src/toolset_cmd.rs` lines 87–93
**Apply to:** `provider_cmd.rs` `cmd_provider_enable` / `cmd_provider_disable`; also `hermes config set providers.*` paths in config_cli.rs

```rust
use colored::Colorize;
eprintln!(
    "{} [toolset: {}] enabled \u{2014} schema cache will rebuild on next LLM call",
    "\u{26a0}".yellow(),
    validated,
);
// Phase 26 variant:
// eprintln!("{} [provider: {}] config changed \u{2014} schema cache will rebuild on next LLM call",
//     "\u{26a0}".yellow(), validated);
```

---

### dotted-path config setter

**Source:** `crates/ironhermes-core/src/config_setter.rs` lines 73–88
**Apply to:** `provider_cmd.rs` enable/disable write paths

```rust
// Signature:
pub fn config_set(hermes_home: &Path, dotted_path: &str, value: &str) -> Result<Option<String>>

// Phase 26 usage:
config_setter::config_set(hermes_home, &format!("providers.{}.disabled", name), "true")
    .with_context(|| format!("failed to disable provider {}", name))?;
```

---

### Cross-crate plain-String type rule

**Source:** Phase 22.4.2.2 → 23 D-12 → 24 D-17 → 25 D-25 → 26 D-18
**Apply to:** All new structs in `ironhermes-core`: `AuxiliaryConfig`, additions to `ProviderConfig`, `RoleOverride`

Rule: all fields are `String` / `Option<String>` / `Vec<String>` / `bool`. No enums that cross crate boundaries. `ApiMode` already exists in `ironhermes-core` — it stays there and is fine to use within-crate.

---

### slug/identifier validator reuse

**Source:** `crates/ironhermes-core/src/profile.rs` lines 63–88 (`validate_profile_name`)
**Apply to:** `provider_cmd.rs` `validate_provider_name` (same function call, same slug format `[a-z0-9][a-z0-9-]*`)

```rust
// In provider_cmd.rs:
pub fn validate_provider_name(name: &str) -> Result<String> {
    profile::validate_profile_name(name)
        .map_err(|e| anyhow::anyhow!("invalid provider name: {}", e))
}
```

For `api_key_env` validation (D-04 — uppercase only), implement separately:
```rust
fn validate_api_key_env(value: &str) -> Result<()> {
    if value.is_empty() { anyhow::bail!("api_key_env must not be empty"); }
    let valid = value.chars().next().map(|c| c.is_ascii_uppercase()).unwrap_or(false)
        && value.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if !valid {
        anyhow::bail!(
            "api_key_env '{}' is not a valid env var name — must match [A-Z][A-Z0-9_]*",
            value
        );
    }
    Ok(())
}
// Note: hand-rolled char loop avoids needing the `regex` crate dep in ironhermes-core.
// If regex is already a direct dep, use Regex::new(r"^[A-Z][A-Z0-9_]*$") instead.
```

---

### `apply_*_answer` wizard pure-function pattern

**Source:** `crates/ironhermes-core/src/wizard.rs` lines 37–54
**Apply to:** New `apply_auxiliary_answer` in `wizard.rs`

```rust
// Pattern: mutate &mut Config, no I/O, no error return.
pub fn apply_provider_answer(config: &mut Config, raw_input: &str, default: &str) {
    let trimmed = raw_input.trim();
    if !trimmed.is_empty() {
        config.model.provider = trimmed.to_string();
    } else if config.model.provider.is_empty() {
        config.model.provider = default.to_string();
    }
}
```

---

## No Analog Found

All files have close analogs. No entries in this section.

---

## Metadata

**Analog search scope:** `crates/ironhermes-core/src/`, `crates/ironhermes-cli/src/`, `crates/ironhermes-cli/tests/`, `crates/ironhermes-agent/src/`, `crates/ironhermes-hub/tests/`
**Files scanned:** 14 files read directly; 4 grep queries
**Pattern extraction date:** 2026-04-29

---

## PATTERN MAPPING COMPLETE
