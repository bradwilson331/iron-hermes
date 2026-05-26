# Phase 26: Provider Polish - Research

**Researched:** 2026-04-29
**Domain:** Rust provider resolution, config schema, CLI subcommand, integration testing
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- D-01: One unified `providers:` HashMap covers built-ins + custom. Schema includes `base_url`, `api_key_env`, `default_model`, `api_mode`, `fallback_providers`. No `api_key:` literal field.
- D-02: `config.custom_providers: Vec<CustomProvider>` is DEPRECATED. Migrate entries on load with one stderr warning per migrated entry.
- D-03: Built-in providers (anthropic, openai, openrouter) pre-populated with canonical defaults; config.providers entries OVERLAY those defaults.
- D-04: `api_key_env` values validated as `[A-Z][A-Z0-9_]*` at config load. Rejects empty/lowercase/shell-injection-shaped values.
- D-05: Single `auxiliary` block + optional per-task overrides. Five reserved roles: vision, compression, session_search, skills_hub, mcp_helper.
- D-06: `auxiliary` is OPTIONAL. Default config has no `auxiliary:` block — all helper tasks use main.
- D-07: `resolve_role(role)` returns `Option<ResolvedEndpoint>` (by value/clone). Callers use `.unwrap_or_else(|| resolver.resolve_for_main().clone())`.
- D-08: Name-keyed lookup stays primary contract. No (name, base_url) tuple keying.
- D-09: `fallback_providers` from Phase 21 stays unchanged. T-12-03 validation preserved.
- D-10: Auxiliary fallback layers ON TOP of provider fallback chains. No "fall back from aux to main" at role-resolution layer.
- D-11: PROV-04 leaky fallback REMOVED. Delete provider.rs:212 `_ => std::env::var("OPENAI_API_KEY").ok()` arm.
- D-12: Legacy env vars `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` / `OPENROUTER_API_KEY` remain accepted for matching built-ins when `api_key_env` is unset. Emit one-shot stderr banner.
- D-13: `config.model.api_key` legacy field DEPRECATED. Keep with one-shot stderr banner.
- D-14: `hermes provider` subcommand mirrors `hermes toolset`. Five subcommands: list/show/test/enable/disable.
- D-15: `hermes provider test <name>` NEVER prints API key value. Shows env var name only.
- D-16: Cache-break stderr banner emitted ONLY on persistent `hermes config set` / `hermes provider enable|disable` writes.
- D-17: `auxiliary` and per-task overrides are also cache-breaking.
- D-18: New types in `ironhermes-core` use plain Strings per Phase 22.4.2.2 cross-crate convention.
- D-19: `apply_minimum_viable_answers` (Phase 23 testability seam) is REUSED for the `hermes setup` provider/aux defaults.
- D-20: Three mandatory integration tests (key_does_not_leak_to_wrong_provider, auxiliary_routes_to_separate_model, custom_provider_selectable_by_name).
- D-21: One mandatory unit test (legacy_openai_key_does_not_leak_to_unknown_provider).

### Claude's Discretion
None explicitly listed.

### Deferred Ideas (OUT OF SCOPE)
- Provider auto-discovery (Ollama/LocalAI/LM Studio)
- Per-request key rotation
- Multi-key-per-provider (org-keyed routing)
- Live failover policy (auto-switch on 5xx)
- Provider rate limiting / quota tracking
- `hermes provider create/delete/rename/clone/import/export`
- Encrypted at-rest storage of api_key values
- `hermes doctor --providers`
- Per-toolset model override
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PROV-04 | API keys scoped to their provider's base URL — prevents leaking wrong key to wrong endpoint | D-11 removes the `_ => OPENAI_API_KEY` arm at provider.rs:212; D-12 retains legacy env fallback for built-ins only with deprecation banner; D-04 validates api_key_env format |
| PROV-06 | Auxiliary model routing: vision, compression, session_search, skills_hub, mcp_helper tasks can use separate provider/model from main | D-05 defines cascade; D-07 documents resolve_role return type; engine_factory.rs already calls resolve_role("compression") but the cascade body is incomplete for general roles |
| PROV-08 | Named custom providers configurable in config.yaml for any OpenAI-compatible endpoint | D-01 unifies providers: HashMap; D-02 deprecates custom_providers: Vec; D-14 adds `hermes provider` CLI; config.rs already has both mechanisms |
</phase_requirements>

---

## Summary

Phase 26 closes three provider correctness/UX gaps that have been partially scaffolded but not fully wired. All three requirements touch `crates/ironhermes-core/src/provider.rs` and `crates/ironhermes-core/src/config.rs` as the epicenter, with ripple effects outward to the agent crate (role resolution wire-through) and the CLI crate (new `hermes provider` subcommand).

The PROV-04 leak (D-11) is a one-line deletion at `provider.rs:212` — the `_ => std::env::var("OPENAI_API_KEY").ok()` wildcard arm in the API key resolution loop. This is surgical but high-impact: after deletion, custom providers with no `api_key_env` get `api_key: None` and must fail loudly at the call site rather than silently using a wrong key. The legacy env-var path for built-ins (D-12) must survive as a deprecated-but-accepted fallback with a one-shot stderr banner.

PROV-06 (auxiliary routing) is also mostly scaffolded: `resolve_role()` exists at provider.rs:275 and returns `Option<ResolvedEndpoint>`, `engine_factory.rs` already calls `build_role_client(resolver, "compression")`, and `config.rs` has `model.roles: HashMap<String, ModelRoleConfig>`. Phase 26 extends this by (a) adding a top-level `auxiliary:` config block as shorthand, (b) wiring the remaining four roles (vision, session_search, skills_hub, mcp_helper) through the agent crate, and (c) validating unknown role names at config load.

PROV-08 (custom providers) is structurally complete but duplicated: `config.providers: HashMap<String, ProviderConfig>` and `config.custom_providers: Vec<CustomProviderConfig>` are both parsed today. Phase 26 unifies to the HashMap, deprecates the Vec with a migration warning, and adds the `hermes provider` CLI surface. The `wiremock` crate (v0.6) is already a workspace dev-dependency in `ironhermes-cli/Cargo.toml` and `reqwest` is already a runtime dep in `ironhermes-core`, so the HTTP probe and mock-server tests have no new dependency cost.

**Primary recommendation:** Implement in five plans: (1) config schema changes in ironhermes-core; (2) provider.rs resolver changes (D-11 leak fix + D-12 legacy banners + D-02 migration); (3) agent crate wire-through; (4) CLI `hermes provider` subcommand + slash commands; (5) setup wizard stage + mandatory integration tests.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| API key scoping (PROV-04) | ironhermes-core (provider.rs) | — | Resolver is the single source of truth for endpoint resolution; key isolation enforced at build time |
| Auxiliary role routing (PROV-06) | ironhermes-core (provider.rs) | ironhermes-agent (engine_factory, summarizing_engine) | Resolver owns the cascade logic; agent factory owns the wire-through to concrete clients |
| Custom provider config (PROV-08) | ironhermes-core (config.rs) | — | Config schema is core-crate responsibility; migration warning lives at parse time |
| `hermes provider` CLI subcommand | ironhermes-cli | ironhermes-core (provider_display) | CLI dispatches; display helpers follow the Phase 25 toolset_display pattern in core |
| `api_key_env` validator (D-04) | ironhermes-core (config.rs or provider.rs) | — | Mirrors slug validator from Phase 24/25; lives at the resolution layer |
| Deprecation banner once-only emission | ironhermes-core (provider.rs) | — | Must not spam every LLM call; `OnceLock<bool>` per banner type at resolver build |
| Setup wizard auxiliary stage (D-19) | ironhermes-cli (setup.rs) | ironhermes-core (wizard.rs) | Production I/O in CLI; pure mutation logic in core wizard via apply_* functions |
| `/provider` slash commands | ironhermes-core (CommandRouter) | ironhermes-cli | CommandRouter registers handlers; Phase 21.1 pattern |

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| `serde` / `serde_yaml` | workspace | Config schema types | Already used for all config structs |
| `reqwest` | 0.12 (workspace, rustls-tls) | `hermes provider test` HTTP probe | Already in ironhermes-core and ironhermes-cli; no new dep |
| `wiremock` | 0.6 (workspace dev-dep) | Mock HTTP server for D-20 integration tests | Already in ironhermes-cli/Cargo.toml dev-deps |
| `clap` | workspace | `hermes provider` subcommand enum | Pattern established by hermes toolset, hermes config |
| `colored` | workspace | Aligned-column terminal output with ANSI awareness | Used in toolset_cmd.rs already |
| `anyhow` | workspace | Error propagation | Entire codebase uses anyhow |
| `OnceLock<bool>` (std) | stable | Once-only deprecation banner emission | Already used in workspace for ENV_LOCK pattern |

[VERIFIED: crates/ironhermes-cli/Cargo.toml, crates/ironhermes-core/Cargo.toml — reqwest 0.12, wiremock 0.6 confirmed]

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| `tempfile` | workspace dev-dep | Isolated IRONHERMES_HOME for tests | Every integration test that mutates env/config |
| `url` | workspace | `is_provider_url_safe()` — already used at provider.rs:73 | Validating new `providers.*.base_url` writes |
| `rustyline` | workspace | Wizard prompts in setup.rs | D-19 auxiliary stage in hermes setup |

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| `wiremock` (already in workspace) | `httpmock` | wiremock is already present; no value in adding a second mock library |
| `OnceLock<bool>` for deprecation banner | `AtomicBool` | `AtomicBool` is lighter but OnceLock is the established project pattern (ENV_LOCK); either works |

---

## Architecture Patterns

### System Architecture Diagram

```
Config::load_from(path)
    │
    ├─► parse custom_providers: Vec (if present)
    │       └─► D-02 migration: emit stderr warning, copy to providers HashMap
    │
    ▼
ProviderResolver::build(&Config)
    │
    ├─► Step 1: pre-populate 3 built-ins (openai/anthropic/openrouter)
    ├─► Step 2: overlay config.providers entries (base_url, api_mode, default_model, fallback_providers)
    │           NEW: also read api_key_env field → defer to Step 4
    ├─► Step 3: [D-02] custom_providers Vec migration already done in Config::load
    │
    ├─► Step 4: API key resolution loop (per provider name)
    │       ├─► config.providers[name].api_key_env → std::env::var(env_name)  [D-01]
    │       ├─► built-in fallback: OPENAI_API_KEY / ANTHROPIC_API_KEY / OPENROUTER_API_KEY
    │       │       └─► if used: emit one-shot stderr deprecation banner [D-12]
    │       ├─► config.model.api_key for main provider only [D-13, deprecated]
    │       └─► [D-11] DELETED: `_ => OPENAI_API_KEY` wildcard arm
    │
    ├─► Step 5: validate fallback_providers reference known names [T-12-03]
    ├─► Step 6: populate model_metadata + config_context_length [Phase 21.3]
    └─► Step 7: store roles (model.roles HashMap)

resolve_role("vision") / resolve_role("compression") / etc.
    │
    ├─► Look up config.auxiliary (if set) as fallback for unconfigured roles
    ├─► Look up per-task override (model.roles["vision"] or auxiliary_config.vision)
    │       D-05 cascade:
    │       1. per-task block → use it
    │       2. auxiliary block → use it
    │       3. None → caller uses resolve_for_main()
    └─► return Option<ResolvedEndpoint> (cloned)

engine_factory.rs / summarizing_engine.rs (agent crate)
    │
    └─► build_role_client(resolver, "compression"|"vision"|etc.)
            └─► resolver.resolve_role(role)
                    .unwrap_or_else(|| resolver.resolve_for_main().clone())

hermes provider <subcommand>  (CLI crate, provider_cmd.rs)
    ├─► list   → ProviderResolver::build → display aligned columns
    ├─► show   → resolver.resolve(name) → detailed view
    ├─► test   → reqwest GET ${base_url}/models (with resolved api_key, never print value)
    ├─► enable → config_setter dotted-path write → cache-break stderr banner
    └─► disable → config_setter dotted-path write → cache-break stderr banner
```

### Recommended Project Structure
```
crates/ironhermes-core/src/
├── config.rs          # add AuxiliaryConfig, RoleOverride; api_key_env on ProviderConfig;
│                      # D-02 migration in Config::load_from (or post-parse hook)
├── provider.rs        # D-11 leak fix; D-12 legacy banners; resolve_role cascade;
│                      # D-04 api_key_env validator
└── commands/
    └── provider_display.rs   # NEW: render_provider_list, render_provider_show (mirrors toolset_display.rs)

crates/ironhermes-agent/src/
├── engine_factory.rs  # wire resolve_role for "vision","session_search","skills_hub","mcp_helper"
└── summarizing_engine.rs  # already wired for "compression" — verify no changes needed

crates/ironhermes-cli/src/
├── main.rs            # add Provider(ProviderCommand) to Commands enum
├── provider_cmd.rs    # NEW: mirrors toolset_cmd.rs — list/show/test/enable/disable
└── setup.rs           # D-19: add auxiliary stage to run_minimum_viable_flow

crates/ironhermes-cli/tests/
└── provider_integration.rs  # D-20 three integration tests + D-21 unit test
```

### Pattern 1: Deprecation Banner Once-Only Emission

**What:** A deprecation warning must emit exactly once per resolver build, never per LLM call.
**When to use:** D-12 legacy env var warning, D-13 config.model.api_key warning.
**Example:**
```rust
// Source: [VERIFIED: crates/ironhermes-core/src/provider.rs pattern]
// In ProviderResolver::build(), before the key resolution loop:
use std::sync::OnceLock;

fn legacy_env_warned() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static WARNED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    WARNED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

// Inside the key resolution loop for built-in providers:
if api_key_env_unset && legacy_env_key.is_some() {
    let mut warned = legacy_env_warned().lock().unwrap();
    if warned.insert(name.clone()) {  // insert returns false if already present
        eprintln!("[provider:{}] using deprecated env var {} — set providers.{}.api_key_env in config.yaml to silence this warning",
            name, legacy_var_name, name);
    }
}
```

### Pattern 2: `hermes provider` Subcommand (mirrors toolset_cmd.rs)

**What:** CLI subcommand enum + dispatcher following Phase 25 D-04 pattern.
**When to use:** D-14 implementation.
**Example:**
```rust
// Source: [VERIFIED: crates/ironhermes-cli/src/toolset_cmd.rs — structural model]
#[derive(Subcommand)]
pub enum ProviderSubcommand {
    /// List all providers with status
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show detail for one provider
    Show { name: String },
    /// Live ping a provider's API endpoint
    Test { name: String },
    /// Enable a provider (persists to active profile config.yaml)
    Enable { name: String },
    /// Disable a provider (persists to active profile config.yaml)
    Disable { name: String },
}
```

### Pattern 3: Integration Test with env_lock + wiremock

**What:** Tests that mutate OPENAI_API_KEY (or any env var) must hold the process-wide ENV_LOCK.
**When to use:** All three D-20 integration tests + D-21 unit test.
**Example:**
```rust
// Source: [VERIFIED: crates/ironhermes-cli/tests/toolset_integration.rs:12-21]
use std::sync::OnceLock;

fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[tokio::test]
async fn key_does_not_leak_to_wrong_provider() {
    let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: test-only env mutation, held behind ENV_LOCK
    unsafe { std::env::set_var("OPENAI_API_KEY", "sk-leaked"); }
    // ... wiremock server captures Authorization header ...
    unsafe { std::env::remove_var("OPENAI_API_KEY"); }
}
```

### Pattern 4: `api_key_env` Validation (D-04)

**What:** Validate that api_key_env values look like env var identifiers.
**When to use:** At config load / ProviderResolver::build().
**Example:**
```rust
// Source: [ASSUMED — pattern derived from Phase 24 slug validator in profile.rs]
fn validate_api_key_env(value: &str) -> Result<()> {
    let re = regex::Regex::new(r"^[A-Z][A-Z0-9_]*$").unwrap();
    if value.is_empty() || !re.is_match(value) {
        anyhow::bail!(
            "api_key_env '{}' is not a valid env var name — must match [A-Z][A-Z0-9_]*",
            value
        );
    }
    Ok(())
}
// Note: check if workspace already uses regex or a hand-rolled approach (Phase 24 uses regex for slugs).
```

### Anti-Patterns to Avoid

- **Deprecation banner in the hot path:** Do NOT emit the D-12/D-13 banner from `resolve()` or `resolve_for_main()`. Those are called per LLM turn. The banner must only be emitted in `build()`, guarded by the once-lock set.
- **Printing key values:** `hermes provider test` must capture the Authorization header success/failure (HTTP status code) only. Never include the actual key value in any output string.
- **Swallowing unknown roles:** D-05 says unknown role names in `model.roles` are rejected at config load. Don't silently ignore them — fail at `ProviderResolver::build()` with a clear error.
- **Using `config.providers[name].api_key` directly in resolver:** The `ProviderConfig.api_key` field currently exists for backward compat. Phase 26 replaces this with `api_key_env`. Old configs with `api_key:` literal must either be rejected or migrated with a banner (CONTEXT.md D-01 says "No `api_key:` literal field").

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| HTTP mock server for live-ping tests | Custom TCP listener | `wiremock` 0.6 (already in workspace) | Already used in ironhermes-hub tests; handles request matching, response queuing, and parallel test safety |
| Config dotted-path writes | Custom YAML walker | `ironhermes_core::config_setter::config_set` | Already implemented; handles atomic write via tempfile+rename |
| URL safety validation | Custom regex | `is_provider_url_safe()` at provider.rs:71 | Already validates https/http-localhost; Phase 26 reuses as-is |
| Slug/identifier validation for api_key_env | New regex | `regex` crate already in workspace; adapt Phase 24/25 pattern | The workspace regex dep is established; the `[A-Z][A-Z0-9_]*` pattern is trivially adapted |
| Aligned-column terminal output | Custom string padding | `ironhermes_core::commands::toolset_display` as model | Phase 25 established the render_toolset_list pattern; Phase 26 adds provider_display.rs mirroring it |

**Key insight:** The heavy lifting (HTTP probing, config mutation, display rendering) is all solved infrastructure. Phase 26 is primarily wiring and policy change, not new infrastructure.

---

## Code-Site Discovery (Verified Line Numbers)

### PROV-04 Leak Site

```
provider.rs:207-212  — API key resolution loop, wildcard arm
```

**Verified body** (from source read):
```rust
let env_key: Option<String> = match name.as_str() {
    "openrouter" => std::env::var("OPENROUTER_API_KEY").ok(),
    "anthropic" => std::env::var("ANTHROPIC_API_KEY").ok(),
    "openai" => std::env::var("OPENAI_API_KEY").ok(),
    // custom providers: try OPENAI_API_KEY as generic fallback   ← LINE 211 comment
    _ => std::env::var("OPENAI_API_KEY").ok(),                     ← LINE 212 LEAK
};
```

D-11 deletes the `_ => std::env::var("OPENAI_API_KEY").ok()` arm entirely. Replacement: for custom providers, check `config.providers[name].api_key_env` (the new field); if None, `env_key = None`.

[VERIFIED: crates/ironhermes-core/src/provider.rs:207-212 read directly]

### `resolve_role()` at provider.rs:275

**Verified body:**
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
}
```

Current state: `resolve_role` works for `model.roles` entries only — it does NOT implement the D-05 three-level cascade (per-task → auxiliary → main). Phase 26 must extend this to check the `auxiliary:` block when no per-task override exists.

The `roles` HashMap is populated from `config.model.roles` (step 7 in build). Phase 26 needs to also integrate `config.auxiliary` into the resolution cascade.

[VERIFIED: crates/ironhermes-core/src/provider.rs:275-287 read directly]

### `auxiliary_model` in engine_factory.rs and summarizing_engine.rs

**Verified findings:**

`engine_factory.rs` already calls `build_role_client(resolver, "compression")` at line 84. The `compression` role wire-through is complete. The file does NOT reference any other roles (vision, session_search, skills_hub, mcp_helper) — those are the D-07 wire-through targets.

`summarizing_engine.rs` does NOT directly reference `auxiliary_model` — it receives an `Arc<dyn SummarizationClient>` from the factory. No changes needed in this file for PROV-06 (the factory owns the role resolution).

**Conclusion:** The agent crate wire-through for vision/session_search/skills_hub/mcp_helper requires finding the call sites where those tasks are initiated (not in engine_factory.rs or summarizing_engine.rs). These call sites are likely in the agent loop or specific tool handlers. Plan 3 must locate these.

[VERIFIED: crates/ironhermes-agent/src/engine_factory.rs lines 84-108 read directly]
[VERIFIED: crates/ironhermes-agent/src/summarizing_engine.rs — no `auxiliary_model` string references]

### Cli struct in main.rs

**Verified state:** The `Commands` enum (main.rs:103-173) currently has:
- Chat, Status, Doctor, Version, Gateway, Cron, Batch, Skills, Memory, Models, Mcp, Setup, Config, Toolset

Phase 26 adds `Provider(ProviderSubcommand)` as a new variant. Pattern exactly mirrors `Toolset { subcommand: toolset_cmd::ToolsetSubcommand }` at line 169-172.

[VERIFIED: crates/ironhermes-cli/src/main.rs:103-173 read directly]

### `apply_minimum_viable_answers` and setup.rs seam

**Verified:** `setup.rs` imports and uses:
- `apply_provider_answer(config, raw_input, default)` — from wizard.rs:37
- `apply_api_key_answer(config, raw_input)` — from wizard.rs:48

The `run_minimum_viable_flow` function at setup.rs:79 calls these `apply_*` functions as the testability seam. D-19 adds an auxiliary stage here — a new `apply_auxiliary_answer(config, raw_input)` function in wizard.rs that writes `auxiliary.provider` and `auxiliary.model`.

[VERIFIED: crates/ironhermes-cli/src/setup.rs:1-80 read directly; crates/ironhermes-core/src/wizard.rs grep confirmed apply_* functions]

### toolset_cmd.rs as analog template

**Verified structure for provider_cmd.rs:**
- `ToolsetSubcommand` enum with List/Enable/Disable/Show/Setup → `ProviderSubcommand` with List/Show/Test/Enable/Disable
- `validate_toolset_name(name)` → `validate_provider_name(name)` — note: provider names use `my-local-llm` style (lowercase + hyphens); Phase 24/25 slug validator already handles this
- `config_setter::config_set(hermes_home, &format!("tools.toolsets.{}.enabled", name), "true")` → `config_setter::config_set(hermes_home, &format!("providers.{}.disabled", name), "false")`
- Cache-break banner on stderr: `eprintln!("[provider: {}] config changed ...", name)`
- `env_lock()` pattern in tests via `OnceLock<Mutex<()>>`

[VERIFIED: crates/ironhermes-cli/src/toolset_cmd.rs full file read]

### Config schema: existing types

**ProviderConfig** (config.rs:70-89):
```rust
pub struct ProviderConfig {
    pub base_url: Option<String>,
    pub api_key: Option<String>,      // ← Phase 26 DEPRECATES this; adds api_key_env
    pub api_mode: Option<ApiMode>,
    pub default_model: Option<String>,
    pub fallback_providers: Vec<String>,
}
```

**Missing types to add:**
- `api_key_env: Option<String>` on `ProviderConfig` (D-01)
- `disabled: Option<bool>` on `ProviderConfig` (D-14 enable/disable)
- `AuxiliaryConfig { provider: String, model: String }` — top-level (D-05)
- `RoleOverride { provider: String, model: Option<String> }` — per-task (D-05); essentially same shape as `ModelRoleConfig` which already exists — Phase 26 may reuse `ModelRoleConfig` directly

**ModelRoleConfig** already exists (config.rs:103-108):
```rust
pub struct ModelRoleConfig {
    pub provider: String,
    pub model: Option<String>,
}
```

This is the `RoleOverride` shape. Phase 26 may add type aliases rather than new structs.

[VERIFIED: crates/ironhermes-core/src/config.rs:70-108 read directly]

---

## Common Pitfalls

### Pitfall 1: Deprecation Banner Spamming Every LLM Call
**What goes wrong:** If the D-12/D-13 banner is emitted from `resolve()` or `resolve_for_main()` (called per-turn), stderr fills with repeated warnings.
**Why it happens:** The obvious place to check `api_key_env` absence is in the getter, not the builder.
**How to avoid:** Emit banners ONLY in `ProviderResolver::build()`, behind a `HashSet` or `OnceLock` that tracks which provider names have already warned. The resolver is built once at startup.
**Warning signs:** Test output shows the warning printed multiple times; grep for banner string in per-call paths.

### Pitfall 2: `custom_providers` Migration Race — Both Present
**What goes wrong:** User has BOTH `providers.foo` AND `custom_providers: [{name:foo}]`. Migration logic in D-02 says "if `custom_providers` has an entry with no matching `providers` key, copy it." But if both exist, which wins?
**Why it happens:** Ambiguous migration contract.
**How to avoid:** D-02 explicit rule: migration only runs for names NOT already present in `providers:`. If `providers.foo` exists, `custom_providers.foo` is silently dropped (not warned). Document this in the migration warning format.
**Warning signs:** Test with a config YAML that has both sections for the same name; assert the `providers` HashMap entry is used unchanged.

### Pitfall 3: `auxiliary.provider` References Unknown Name
**What goes wrong:** User sets `auxiliary: {provider: "typo-name"}` — if not validated at config load, error surfaces deep in the agent loop on first LLM call with a confusing panic/unwrap.
**Why it happens:** Lazy validation (check on use, not on load).
**How to avoid:** In `ProviderResolver::build()`, after building the endpoints map, validate `config.auxiliary.provider` (if set) is a known key. Fail with: `"auxiliary.provider 'xyz' is not a known provider — define it in providers: first"`.
**Warning signs:** No test exists for this; add one in D-21.

### Pitfall 4: `providers.*.disabled` Flag vs. Resolver Endpoint Presence
**What goes wrong:** `hermes provider disable <name>` writes `providers.name.disabled: true` to config. If `ProviderResolver::build()` doesn't skip disabled providers, they still resolve and accept calls.
**Why it happens:** D-14 adds the disabled flag but the resolver loop must honor it.
**How to avoid:** In the resolver build loop (Step 2), skip or omit disabled providers. A disabled provider that is also the main provider should error at build time: "main provider 'X' is disabled — re-enable it or change model.provider."
**Warning signs:** `hermes provider test <name>` after disabling still succeeds; integration test checks that disabled providers don't appear in resolver.

### Pitfall 5: `api_key` Literal Field Still Parsed in ProviderConfig
**What goes wrong:** D-01 says no `api_key:` literal field in the new schema. But `ProviderConfig.api_key` currently exists. If not handled, old config files with `api_key:` silently parse and work (which might be desired for migration) but the field is undocumented.
**Why it happens:** Serde will still deserialize the field if it remains in the struct.
**How to avoid:** Keep the field but mark it `#[serde(default)]` with a one-shot deprecation banner when it's non-None at config load (same pattern as custom_providers migration). The field stays for one minor release cycle.
**Warning signs:** Config YAML with `api_key: sk-xxx` loads without warning.

### Pitfall 6: Deprecation Banner in Tests Without env_lock
**What goes wrong:** Tests that set `OPENAI_API_KEY` concurrently interfere with each other — one test's `remove_var` fires during another test's `set_var` check, causing flaky PROV-04 assertions.
**Why it happens:** Rust test runner is multi-threaded by default; env vars are process-global.
**How to avoid:** Every test touching `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY` must hold `env_lock()`. The established pattern is in `toolset_integration.rs:18-21` and `ironhermes-hub/src/audit.rs:102`.
**Warning signs:** Tests pass in isolation but fail under `cargo test --workspace` with parallel threads.

---

## Runtime State Inventory

This is not a rename/refactor/migration phase. The only migration concern is the `custom_providers: Vec` → `providers: HashMap` schema migration, which is a config-parse-time migration (not a stored data migration): the old YAML key is read, entries copied to the HashMap with a stderr warning, and the user is expected to edit config.yaml manually. No database records, OS-registered state, or build artifacts are involved.

**Stored data:** None — config.yaml is the only store, migration handled at parse time.
**Live service config:** None — no n8n workflows, no external service registrations.
**OS-registered state:** None.
**Secrets/env vars:** The point of PROV-04 is env var NAMES change meaning (api_key_env references explicit var names). Existing `OPENAI_API_KEY` etc. continue to work as deprecated fallbacks (D-12).
**Build artifacts:** None.

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness + tokio-test |
| Config file | None (cargo test) |
| Quick run command | `cargo test -p ironhermes-core provider` |
| Full suite command | `cargo test --workspace` |

### Critical Paths Requiring Test Coverage

1. **PROV-04 key leak prevention (D-21 unit test)**
   - Set `OPENAI_API_KEY` in env, build resolver with custom provider that has no `api_key_env`, assert custom provider `api_key == None`.
   - Location: `crates/ironhermes-core/src/provider.rs` `#[cfg(test)]` module.
   - Requires: `env_lock()` + `unsafe { std::env::set_var/remove_var }`.

2. **PROV-06 auxiliary routing (D-20 test 2)**
   - Set `auxiliary: {provider: openai, model: gpt-4o-mini}` with main = anthropic.
   - Trigger a compression task; assert outbound request goes to `api.openai.com`, not `api.anthropic.com`.
   - Location: `crates/ironhermes-cli/tests/provider_integration.rs` (subprocess or wiremock).
   - Requires: `wiremock` mock server + env_lock.

3. **PROV-08 custom provider selectability (D-20 test 3)**
   - Define `providers.my-local-llm` with custom base_url + api_key_env.
   - Run `hermes --provider my-local-llm chat "ping"`.
   - Assert resolver returns custom endpoint and request hits configured base_url.
   - Location: `crates/ironhermes-cli/tests/provider_integration.rs` (subprocess).

4. **PROV-04 integration (D-20 test 1)**
   - Spawn binary with `OPENAI_API_KEY=sk-real` set.
   - Define `my-local-llm` with no `api_key_env`.
   - wiremock captures the outbound Authorization header to `my-local-llm`.
   - Assert Authorization header does NOT contain `sk-real`.
   - Location: `crates/ironhermes-cli/tests/provider_integration.rs`.

5. **Deprecation banner once-only emission**
   - Build resolver twice with `OPENAI_API_KEY` set and no `api_key_env` for openai.
   - Assert banner appears exactly once (first build), not twice.
   - Location: unit test in `provider.rs` tests module. Use `OnceLock` reset workaround or process-isolation.

6. **`hermes provider test` does not print key value**
   - Subprocess test: run `hermes provider test openai` with `OPENAI_API_KEY=sk-secret` set.
   - Assert stdout + stderr do NOT contain `sk-secret`.
   - Location: `crates/ironhermes-cli/tests/provider_integration.rs`.

7. **`custom_providers` migration warning**
   - Config YAML with `custom_providers: [{name: foo, base_url: ...}]` but no `providers.foo`.
   - Load config, build resolver; assert stderr contains migration warning for `foo`.
   - Location: unit test in `provider.rs` or `config.rs` tests module.

8. **`auxiliary.provider` unknown name fails at build**
   - Set `auxiliary: {provider: nonexistent}` in config; assert `ProviderResolver::build()` returns `Err`.
   - Location: unit test in `provider.rs` tests module.

9. **`api_key_env` validation rejects invalid identifiers**
   - `validate_api_key_env("")`, `validate_api_key_env("lower_case")`, `validate_api_key_env("HAS SPACE")` → all Err.
   - `validate_api_key_env("OPENAI_API_KEY")`, `validate_api_key_env("MY_KEY_123")` → Ok.
   - Location: unit test in `provider.rs` or `config.rs`.

10. **`resolve_role` D-05 cascade**
    - Three sub-tests: (a) per-task configured → returns per-task; (b) per-task absent, auxiliary configured → returns auxiliary; (c) both absent → returns None (caller falls through to main).
    - Location: unit test in `provider.rs` tests module.

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| PROV-04 | Key does not leak to custom provider | unit | `cargo test -p ironhermes-core legacy_openai_key_does_not_leak` | No — Wave 0 |
| PROV-04 | Key does not appear in outbound HTTP header | integration | `cargo test -p ironhermes-cli key_does_not_leak_to_wrong_provider` | No — Wave 0 |
| PROV-06 | Aux routes to separate model/provider | integration | `cargo test -p ironhermes-cli auxiliary_routes_to_separate_model` | No — Wave 0 |
| PROV-06 | resolve_role cascade (per-task → aux → None) | unit | `cargo test -p ironhermes-core resolve_role_cascade` | No — Wave 0 |
| PROV-08 | Custom provider selectable by --provider flag | integration | `cargo test -p ironhermes-cli custom_provider_selectable_by_name` | No — Wave 0 |
| D-02 | custom_providers migration emits warning | unit | `cargo test -p ironhermes-core custom_providers_migration_warning` | No — Wave 0 |
| D-04 | api_key_env invalid names rejected | unit | `cargo test -p ironhermes-core api_key_env_validation` | No — Wave 0 |
| D-12 | Legacy env var banner emitted once only | unit | `cargo test -p ironhermes-core legacy_env_banner_once_only` | No — Wave 0 |
| D-15 | provider test never prints key value | integration | `cargo test -p ironhermes-cli provider_test_does_not_print_key` | No — Wave 0 |

### Sampling Strategy for env-var Sensitive Tests

All tests that call `std::env::set_var` / `remove_var` (Rust 2024 edition: requires `unsafe {}` block) MUST hold the process-wide `env_lock()` for the duration. Pattern from `toolset_integration.rs`:

```rust
fn env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

// In each test that mutates env:
let _g = env_lock().lock().unwrap_or_else(|p| p.into_inner());
unsafe { std::env::set_var("OPENAI_API_KEY", "sk-test"); }
// ... test body ...
unsafe { std::env::remove_var("OPENAI_API_KEY"); }
```

Note: The `env_lock()` in `provider_integration.rs` must be a SEPARATE static from `toolset_integration.rs` — they are different test binary compilations, so a shared static in each test file is correct.

For wiremock-based tests, use `wiremock::MockServer::start().await` which binds a random port. No port collisions between parallel tests.

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-core provider`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-cli/tests/provider_integration.rs` — D-20 three integration tests
- [ ] `crates/ironhermes-core/src/provider.rs` `#[cfg(test)]` additions — D-21 unit test + cascade/validation tests
- [ ] `crates/ironhermes-core/src/commands/provider_display.rs` — display helpers (render_provider_list, render_provider_show)

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | yes | API key scoping per provider (PROV-04 D-11); api_key_env reference pattern (D-01) |
| V3 Session Management | no | — |
| V4 Access Control | no | Provider selection is operator config, not user auth |
| V5 Input Validation | yes | D-04 api_key_env `[A-Z][A-Z0-9_]*` validator; `is_provider_url_safe()` for base_url |
| V6 Cryptography | no | Keys in env vars (OS process memory), not encrypted at rest — explicitly deferred (CONTEXT deferred ideas) |

### Known Threat Patterns for This Stack

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| API key leaked to wrong endpoint via wildcard env fallback | Information Disclosure | D-11 deletes the `_ => OPENAI_API_KEY` arm; any custom provider with no `api_key_env` gets `None` |
| `api_key_env` set to shell-injection string (e.g., `$(rm -rf ~)`) | Tampering | D-04 uppercase-only regex validator; applied at config load before any `std::env::var()` call |
| `base_url` set to http:// non-localhost to intercept key | Information Disclosure | `is_provider_url_safe()` already validates https-or-localhost; Phase 26 applies this to new `providers.*.base_url` writes too |
| `hermes provider test` leaking key value to stdout/stderr | Information Disclosure | D-15 hard rule; subprocess test asserts output does NOT contain the key value string (T-26-01) |
| Debug log redaction bypass | Information Disclosure | `ResolvedEndpoint` `Debug` impl already redacts `api_key` field with `[REDACTED]` (provider.rs:50); Phase 26 must not introduce new Debug paths that bypass this |

---

## Plan Splitting Suggestion

Given 21 decisions, the logical breakdown into 5 plans with clear dependency edges:

**Plan 1: Config Schema** (ironhermes-core/config.rs only)
- Add `api_key_env: Option<String>` and `disabled: Option<bool>` to `ProviderConfig`
- Add `AuxiliaryConfig { provider: String, model: String }` struct
- Add `auxiliary:` and per-task override fields to `Config`
- D-04: `validate_api_key_env()` function
- D-02: migration logic in `Config::load_from()` or as a post-parse step
- Backward compat tests (old config without new fields parses cleanly)

**Plan 2: ProviderResolver Resolver Changes** (ironhermes-core/provider.rs)
- D-11: delete the `_ => OPENAI_API_KEY` wildcard arm
- D-12: legacy env var fallback for built-ins with one-shot stderr banner
- D-13: `config.model.api_key` deprecation banner
- D-05/D-07: extend `resolve_role()` to implement the three-level cascade (per-task → auxiliary → None)
- D-10: validate `auxiliary.provider` references a known name at build time
- Unit tests: D-21 unit test + cascade tests + banner-once-only test
- Depends on Plan 1 (new config types)

**Plan 3: Agent Crate Wire-Through** (ironhermes-agent)
- Locate the vision/session_search/skills_hub/mcp_helper call sites in agent loop and tool handlers
- Wire `resolver.resolve_role(role).unwrap_or_else(|| resolver.resolve_for_main().clone())` at each site
- `engine_factory.rs` "compression" path already complete — verify and add regression test
- Depends on Plan 2 (resolve_role cascade complete)

**Plan 4: `hermes provider` CLI + Display + Slash Commands** (ironhermes-cli + ironhermes-core)
- `crates/ironhermes-core/src/commands/provider_display.rs` — render helpers
- `crates/ironhermes-cli/src/provider_cmd.rs` — list/show/test/enable/disable
- Wire `Provider(ProviderSubcommand)` into `Commands` enum in main.rs
- D-15: `hermes provider test` HTTP probe via reqwest (GET /models, fallback POST /chat/completions)
- D-16: cache-break stderr banner on enable/disable writes
- Register `/provider list/show/test/enable/disable` in CommandRouter (Phase 21.1 pattern)
- Depends on Plan 2 (resolver complete), Plan 1 (disabled flag)

**Plan 5: Setup Wizard Stage + Mandatory Integration Tests** (ironhermes-cli)
- D-19: add `apply_auxiliary_answer()` in wizard.rs; add auxiliary stage to `run_minimum_viable_flow` in setup.rs
- D-20 test 1: `key_does_not_leak_to_wrong_provider` (wiremock)
- D-20 test 2: `auxiliary_routes_to_separate_model` (wiremock)
- D-20 test 3: `custom_provider_selectable_by_name` (subprocess)
- D-15 test: `provider_test_does_not_print_key`
- Depends on Plans 1–4 all complete

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Generic `OPENAI_API_KEY` fallback for all unknown providers | Per-provider `api_key_env` reference; custom providers get `None` if unset | Phase 26 D-11 | Eliminates silent cross-provider key leak |
| `custom_providers: Vec<CustomProvider>` for custom endpoints | Unified `providers: HashMap` for built-ins and custom alike | Phase 26 D-02 | Single config surface; simpler resolution logic |
| `model.api_key` literal key in config | Deprecated; `providers.<main>.api_key_env` preferred | Phase 26 D-13 | Keeps keys in env, not config file |
| No auxiliary model routing wired in agent | `resolve_role()` cascade feeds engine_factory + tool handlers | Phase 26 D-07 | Cheap models for helper tasks without main model involvement |

**Deprecated/outdated:**
- `config.custom_providers: Vec<CustomProviderConfig>`: deprecated D-02; kept for two minor releases then dropped
- `config.model.api_key`: deprecated D-13; kept with banner for one major release
- `config.providers[name].api_key` literal: deprecated D-01; replaced by `api_key_env`

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `validate_api_key_env` can use the `regex` crate already in the workspace without adding a new dep | Standard Stack / Pattern 4 | If regex is not a direct dep of ironhermes-core, must either add it or hand-roll the check with a simple loop |
| A2 | The vision/session_search/skills_hub/mcp_helper call sites are in the agent loop or tool handlers (not in engine_factory.rs) | Code-site discovery (Plan 3) | If they live in a different crate, Plan 3 scope changes |
| A3 | `ModelRoleConfig` (existing) can be reused as `RoleOverride` shape for per-task overrides without a new type | Config Schema | If the planner wants distinct types for clarity, a type alias or newtype adds trivial work |
| A4 | The `once_only` banner can be implemented with a process-level `OnceLock<Mutex<HashSet<String>>>` inside `provider.rs` (one per banner category) | Pattern 1 | If tests need to reset the OnceLock (impossible after first init), unit tests for the once-only property must use process isolation (subprocess tests) |

---

## Open Questions

1. **Where exactly are the vision/session_search/skills_hub/mcp_helper call sites in the agent crate?**
   - What we know: `engine_factory.rs` handles `compression` via `build_role_client(resolver, "compression")`. The other four roles are not in engine_factory.rs or summarizing_engine.rs.
   - What's unclear: Are they in `agent_loop.rs`, specific tool handlers (e.g., a vision tool), or somewhere else in the agent crate?
   - Recommendation: Plan 3 researcher/executor must grep for "vision_model", "mcp_helper", "skills_hub", "session_search" in the agent crate before implementing the wire-through.

2. **Banner once-only testing: OnceLock cannot be reset between tests**
   - What we know: `OnceLock<T>` is initialized exactly once and cannot be reset in the same process. Tests in the same binary share the OnceLock state.
   - What's unclear: Should the banner once-only test be a subprocess test (clean process per test), or should the implementation use a `Mutex<HashSet>` that can be cleared (test-only reset seam)?
   - Recommendation: Use a subprocess integration test for the once-only property, similar to `toolset_enable_emits_cache_break_banner_on_stderr`. Avoids any test-reset seam in production code.

3. **`api_key` literal field in ProviderConfig — deprecate with warning or remove immediately?**
   - What we know: D-01 says "No `api_key:` literal field". D-18 says use plain Strings. Current `ProviderConfig.api_key: Option<String>` exists.
   - What's unclear: Should existing configs with `api_key:` literal silently work (with a deprecation banner) for one release, or fail at parse time?
   - Recommendation: Treat identically to `custom_providers` migration (D-02) — accept but warn, drop in next major.

---

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| `reqwest` | `hermes provider test` HTTP probe | Yes | 0.12 (workspace) | — |
| `wiremock` | D-20 integration tests (mock server) | Yes (dev-dep) | 0.6 (workspace) | — |
| `cargo test` | Running all tests | Yes | (Rust toolchain) | — |
| `CARGO_BIN_EXE_ironhermes` | Subprocess integration tests | Yes (set by cargo test harness) | — | Skip with eprintln and return |
| `IRONHERMES_HOME` env var | Test isolation (tempdir override) | Yes | — | — |

**Missing dependencies:** None identified. All required libraries are present in the workspace.

---

## Sources

### Primary (HIGH confidence)
- [VERIFIED: crates/ironhermes-core/src/provider.rs] — Full file read; confirmed leak at line 212, `resolve_role` body at line 275, three built-in pre-population at lines 111-147
- [VERIFIED: crates/ironhermes-core/src/config.rs] — Full file read; confirmed ProviderConfig, CustomProviderConfig, ModelRoleConfig shapes; confirmed `custom_providers: Vec` and `providers: HashMap` both exist
- [VERIFIED: crates/ironhermes-agent/src/engine_factory.rs] — Full file read; confirmed compression role wired via `build_role_client(resolver, "compression")`; no other role wire-throughs
- [VERIFIED: crates/ironhermes-agent/src/summarizing_engine.rs] — Full file read; no `auxiliary_model` references; receives `Arc<dyn SummarizationClient>` from factory
- [VERIFIED: crates/ironhermes-cli/src/main.rs:1-220] — Commands enum read; confirmed Toolset variant pattern; no Provider variant yet
- [VERIFIED: crates/ironhermes-cli/src/toolset_cmd.rs] — Full file read; structural model for provider_cmd.rs
- [VERIFIED: crates/ironhermes-cli/src/setup.rs:1-80] — Confirmed apply_* seam functions; make_wizard_editor pattern
- [VERIFIED: crates/ironhermes-cli/tests/toolset_integration.rs:1-80] — Confirmed env_lock + CARGO_BIN_EXE_ironhermes + subprocess test pattern
- [VERIFIED: crates/ironhermes-cli/Cargo.toml + Cargo.toml workspace] — reqwest 0.12, wiremock 0.6 confirmed present

### Secondary (MEDIUM confidence)
- [VERIFIED: grep results on wizard.rs] — apply_provider_answer at line 37, apply_api_key_answer at line 48 confirmed
- [VERIFIED: grep results on config_setter.rs] — config_set(hermes_home, dotted_path, value) confirmed
- [VERIFIED: .planning/config.json] — `workflow.nyquist_validation` absent (treat as enabled); `security_enforcement` absent (treat as enabled)

### Tertiary (LOW confidence)
- [ASSUMED] regex crate in workspace — assumed available; not directly verified against Cargo.toml workspace deps

---

## Metadata

**Confidence breakdown:**
- Standard Stack: HIGH — all libraries confirmed via Cargo.toml reads
- Architecture: HIGH — code sites verified by direct file reads
- Pitfalls: HIGH — derived from verified code patterns; one ASSUMED (regex availability)
- Validation: HIGH — test patterns confirmed from toolset_integration.rs

**Research date:** 2026-04-29
**Valid until:** 2026-05-29 (30 days; stable Rust ecosystem)

---

## RESEARCH COMPLETE
