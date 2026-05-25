# Phase 25: Toolset Management - Research

**Researched:** 2026-04-29
**Domain:** Rust — tool registry expansion, CLI subcommand, slash command integration, setup wizard hook
**Confidence:** HIGH (all key claims verified against live codebase)

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions
- D-01: Six concrete toolsets: `web`, `code`, `memory`, `agent`, `skills`, `session`
- D-02: Toolset names validated as slugs via `validate_profile_name`-style regex
- D-03: Toolset membership read at runtime from `Tool::toolset()`; no separate table
- D-04: `hermes toolset list/enable/disable/show` + `setup` subcommands
- D-05: Enable/disable persistent per-profile via Phase 24 pivot (automatic)
- D-06: Slash commands mirror CLI but session-only (no config.yaml write)
- D-07: No `--toolset` global flag
- D-08: `is_available()` pure synchronous bool; env var + config field checks only
- D-09: `prerequisites()` method on `Tool` trait returning `Vec<Prerequisite>`; default empty
- D-10: Schema exclusion silent; no stderr on excluded tools
- D-11: Check-time is `get_definitions()` call-time (fast, catches mid-session env changes)
- D-12: `dispatch_intercepts(name, args) -> Option<InterceptResult>` on `ToolRegistry`; called BEFORE `dispatch()`
- D-13: Five intercepted tools: `memory`, `session_search`, `delegate_task`, `todo_write`, `todo_read`, `cronjob`
- D-14: Intercepted tools NOT in `tools` HashMap; separate `intercepts` map; schemas from BOTH maps
- D-15: Duplicate tool name in both maps = `panic!()` at registry build
- D-16: `with_intercepts(...)` builder on `AgentLoop`; default registers no intercepts
- D-17: `preflight::run_preflight_check` gains tool-prereq check; missing required prereqs emit stderr banner; no auto-wizard launch
- D-18: `hermes toolset setup` subcommand walks unsatisfied required prereqs
- D-19: `hermes setup` gains final opt-in "optional tool prerequisites" stage
- D-20: Default enabled toolsets on fresh install: `[memory, session, agent, skills]`; `web` and `code` disabled
- D-21: Per-profile override automatic via Phase 24; `DEFAULT_TOOLSETS` constant in `ironhermes-core::constants`
- D-22: Config shape: `tools.toolsets.<name>.enabled` block per toolset + `tools.skip_prompts: []`
- D-23: Existing `enabled_tools` param on `get_definitions()` repurposed as per-tool override layer
- D-24: Config schema migration automatic and silent
- D-25: New cross-crate types use plain Strings; `Prerequisite` is plain struct not enum
- D-26: Three mandatory integration tests: `toolset_enable_disable_persists`, `tool_excluded_when_prereq_missing`, `intercepted_tool_no_schema_duplicate`

### Claude's Discretion
None — user opted into "Skip — Claude picks defaults" across all gray areas.

### Deferred Ideas (OUT OF SCOPE)
- `hermes toolset create/delete/rename/alias/import/export`
- Per-tool-call permission prompts
- Per-toolset rate limiting / quota
- `--toolset` per-invocation override flag
- Custom user-defined toolset definitions
- Toolset versioning / compatibility ranges
- MCP-server-as-toolset auto-grouping management
- `hermes doctor --tools`
- First-run "all tools on" default
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| TOOL-01 | Tool trait includes `is_available()` check; tools silently excluded from schema when prerequisites absent | `is_available()` default `true` already exists at `registry.rs:17-19`; `web_search.rs:67` has env-var impl; `web_read.rs:521` has `true` stub. `prerequisites()` method is additive with default empty vec. |
| TOOL-02 | Tools organized into named toolsets with platform-specific presets | `Tool::toolset()` already returns `&str` on all 12 built-ins. `ToolRegistry` needs `list_toolsets()`, toolset-level filter in `get_definitions()`, and `ToolsConfig` in `Config`. |
| TOOL-03 | Adding a new tool requires only a registration call — no dispatch logic changes | Registry's single `tools` HashMap already does this for normal tools; intercept map adds same guarantee for intercepted tools via `register_intercepted()`. |
| TOOL-04 | Agent-intercepted tools handled before registry dispatch without schema duplication | Single hardcoded `session_search` block at `agent_loop.rs:951-961` is the migration target; memory_provider interception at lines ~981+ also in scope. |
| TOOL-05 | Setup wizard detects missing prerequisites and guides user through configuring them | `setup.rs:239` has `run_tools_section()` stub already reserved for Phase 25; `apply_minimum_viable_answers` at `setup.rs:250` is the testability seam. |
</phase_requirements>

---

## Summary

Phase 25 is a registry expansion and plumbing consolidation phase. The codebase already has almost all the structural prerequisites: `Tool::toolset()` returns `&'static str` on all 12 built-in tools; `is_available()` exists with a default `true` impl; `web_search.rs` already checks `FIRECRAWL_API_KEY`; there is a reserved `run_tools_section()` stub in `setup.rs`; and a `/toolsets` entry already exists in the slash command registry. The core additions are: (1) `prerequisites()` method on the `Tool` trait, (2) `intercepts` HashMap + three new registry methods, (3) toolset-level filtering in `get_definitions()`, (4) `ToolsConfig` in `Config`, (5) `hermes toolset` CLI subcommand, and (6) wiring the setup wizard hook.

**Critical mismatch (VERIFIED):** The existing `toolset()` return values do not fully match CONTEXT.md D-01's six compiled-in names. `terminal` returns `"system"` (not `"code"`), and all four file tools (`read_file`, `write_file`, `patch_file`, `search_files`) return `"file"` (not `"code"`). D-01 lists the `code` toolset as containing `execute_code`, `terminal`, and `file_tools::*`. This means **all five of those tool implementations need `toolset()` updated to return `"code"`** before the membership logic can work. This is a Plan 0 / Wave 0 code change, not a new type — it is a one-liner per tool.

**Second mismatch (VERIFIED):** `web_read.rs:521` has `is_available()` returning hard-coded `true`. D-01 puts `web_read` in the `web` toolset whose availability should depend on the same prereq as `web_search`. Per D-09, `web_read` needs a `prerequisites()` impl listing `FIRECRAWL_API_KEY` as `required: false` (it has a plain-text fallback), and `web_search` needs `FIRECRAWL_API_KEY` as `required: true`. The planner must decide whether `web_read` in a disabled toolset skips `is_available()` anyway (it does — disabled toolset excludes all tools regardless).

**Primary recommendation:** Split into 5 plans: (1) trait surface + toolset() fixes, (2) registry expansion, (3) config + `get_definitions()` wiring, (4) CLI subcommand, (5) setup wizard + preflight hook. The agent loop migration (D-12) belongs in Plan 2 because it touches `registry.rs` first.

---

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| `is_available()` / `prerequisites()` trait surface | `ironhermes-tools` library | — | Per-tool knowledge; lives with each tool's impl |
| Toolset membership (`toolset()` method) | `ironhermes-tools` library | — | Source of truth per D-03; already on `Tool` trait |
| Registry intercept dispatch | `ironhermes-tools` library | — | `ToolRegistry` owns both maps; intercept logic is registry-internal |
| Toolset-level config persistence | `ironhermes-core` library | — | `Config` struct lives in core; dotted-path setter already there |
| `ToolsConfig` struct + defaults | `ironhermes-core` library | — | Follows all other `*Config` structs in `config.rs` |
| `hermes toolset` CLI subcommand | `ironhermes-cli` binary | `ironhermes-core` (config setter) | Mirrors `config_cli.rs` pattern; pure I/O + config write |
| `/toolset` slash commands | `ironhermes-core` (commands registry) | — | `build_registry()` in `commands/registry.rs`; already has `/toolsets` stub |
| Setup wizard tool prereq stage | `ironhermes-cli` library (`setup.rs`) | `ironhermes-core` (wizard helpers) | `run_tools_section()` stub already exists |
| Preflight prereq check | `ironhermes-cli` library (`preflight.rs`) | `ironhermes-tools` (list_unavailable) | Insertion point: after config validation, before wizard launch |
| Agent loop intercept wiring | `ironhermes-agent` library | `ironhermes-tools` (registry) | `execute_tool_call()` calls `dispatch_intercepts()` first |

---

## Standard Stack

### Core (all verified in Cargo.toml / existing usage)

| Library | Purpose | Already in use |
|---------|---------|---------------|
| `async_trait` | `Tool` trait with async `execute()` | Yes — `registry.rs:4` |
| `serde` / `serde_yaml` | `ToolsConfig` serialization | Yes — all `*Config` structs |
| `clap` (derive) | `ToolsetSubcommand` enum | Yes — `config_cli.rs`, `cron.rs` |
| `anyhow` | Error handling in subcommand handlers | Yes — universal |
| `rustyline` | Setup wizard prompts | Yes — `setup.rs` |
| `colored` | Stderr banners | Yes — `config_cli.rs:7` |
| `tokio` + `RwLock` | Registry behind `Arc<RwLock<ToolRegistry>>` | Yes — `agent_loop.rs:478` |
| `tempfile` | Integration test isolation | Yes — `profile_isolation.rs` |

No new external dependencies required for Phase 25.

---

## Architecture Patterns

### System Architecture Diagram

```
hermes toolset enable web
        |
        v
[CLI: ToolsetSubcommand::Enable]
        |
        v
[config_setter::config_set("tools.toolsets.web.enabled", "true")]
        |
        v
[~/.ironhermes/config.yaml] <-- per-profile via Phase 24 IRONHERMES_HOME pivot
        |
        v (next session start)
[ToolRegistry::build()]
   register() ─────────────────────> tools: HashMap<String, Box<dyn Tool>>
   register_intercepted() ─────────> intercepts: HashMap<String, InterceptHandler>
        |
        v
[get_definitions(enabled_tools)]
   1. filter: toolset enabled? (ToolsConfig)
   2. filter: is_available()?
   3. filter: tools.disabled list?
   4. collect schemas from tools + intercepts
        |
        v
[LLM sees tool schema list]

execute_tool_call(name, args)
   1. dispatch_intercepts(name, args) -> Option<InterceptResult>
      Some(r) -> return r   (memory, session_search, delegate_task, todo_*, cronjob)
      None    -> dispatch(name, args) via guardrail chain
```

### Recommended Project Structure

```
crates/ironhermes-tools/src/
├── registry.rs          # Tool trait (+ prerequisites()), ToolRegistry (+ intercepts map)
├── web_search.rs        # toolset() = "web"; prerequisites() with FIRECRAWL_API_KEY required:true
├── web_read.rs          # toolset() = "web"; prerequisites() with FIRECRAWL_API_KEY required:false
├── terminal.rs          # toolset() CHANGE "system" -> "code"
├── execute_code.rs      # toolset() already "code"
├── file_tools.rs        # toolset() CHANGE "file" -> "code" (all 4 tools)
├── memory_tool.rs       # toolset() = "memory"
├── delegate_task.rs     # toolset() = "agent"
├── skills_tool.rs       # toolset() = "skills"
├── cronjob_tool.rs      # toolset() = "cronjob" (NOTE: see mismatch below)
└── session_search.rs    # no toolset() — intercepted only; schema registered via register_intercepted()

crates/ironhermes-core/src/
├── config.rs            # + ToolsConfig struct, + ToolsetEntry { enabled: bool }, + tools field on Config
└── constants.rs         # + DEFAULT_TOOLSETS: &[&str] = &["memory", "session", "agent", "skills"]

crates/ironhermes-cli/src/
├── main.rs              # + Commands::Toolset variant
├── toolset_cmd.rs       # NEW — mirrors config_cli.rs pattern
└── setup.rs             # run_tools_section() stub replaced with real prompts
                         # apply_minimum_viable_answers signature UNCHANGED

crates/ironhermes-cli/tests/
└── toolset_integration.rs  # D-26 three mandatory integration tests
```

### Pattern 1: Existing CLI Subcommand (reference — config_cli.rs)

```rust
// Source: crates/ironhermes-cli/src/config_cli.rs (VERIFIED)
#[derive(Subcommand)]
pub enum ConfigSubcommand {
    Set { key: String, value: String },
    Get { key: String },
    Show,
    // ...
}

pub async fn handle_config_command(cmd: ConfigSubcommand, profile_name: &str) -> Result<()> {
    let hermes_home = ironhermes_core::constants::get_hermes_home();
    match cmd {
        ConfigSubcommand::Set { key, value } => cmd_config_set(&hermes_home, &key, &value).await,
        // ...
    }
}
```

Toolset subcommand mirrors this structure exactly.

### Pattern 2: Existing Slash Command Registration (reference — registry.rs)

```rust
// Source: crates/ironhermes-core/src/commands/registry.rs (VERIFIED)
// Existing stub in build_registry():
CommandDef::new("toolsets", "List available toolsets", ToolsAndSkills).platform(CliOnly),

// Phase 25: expand to full /toolset command with subcommands
CommandDef::new("toolset", "Manage toolsets", ToolsAndSkills)
    .args_hint("[list|enable|disable|show] [name]")
    .platform(Universal),
```

NOTE: The existing entry uses `"toolsets"` (plural) at CliOnly. Phase 25 D-06 calls the slash command `"/toolset"` (singular). The existing `"toolsets"` entry must be replaced or aliased. This is a minor registry change in `build_registry()`.

### Pattern 3: Subprocess Integration Test Pattern (reference — profile_isolation.rs)

```rust
// Source: crates/ironhermes-cli/tests/profile_isolation.rs (VERIFIED)
let bin = match std::env::var("CARGO_BIN_EXE_ironhermes") {
    Ok(p) => p,
    Err(_) => { eprintln!("Skipping: CARGO_BIN_EXE_ironhermes not set"); return; }
};
let tmp = tempfile::TempDir::new().unwrap();
let out = std::process::Command::new(&bin)
    .env("IRONHERMES_HOME", tmp.path())
    .args(["toolset", "list"])
    .output()
    .expect("failed to run binary");
```

D-26 integration tests use this exact pattern. `CARGO_BIN_EXE_ironhermes` is set by cargo's test harness for integration tests in the `ironhermes-cli` crate.

### Pattern 4: AgentLoop Builder Chain (reference — agent_loop.rs)

```rust
// Source: crates/ironhermes-agent/src/agent_loop.rs (VERIFIED — existing builder methods)
pub fn with_state_store(mut self, store: Arc<Mutex<StateStore>>) -> Self { ... }
pub fn with_memory_manager(mut self, manager: Arc<Mutex<MemoryManager>>) -> Self { ... }
pub fn with_active_skills(mut self, skills: Arc<Mutex<Vec<SkillRecord>>>) -> Self { ... }

// Phase 25 D-16: add to this chain
pub fn with_intercepts(mut self, ...) -> Self { ... }
```

### Anti-Patterns to Avoid

- **Returning session_search from get_definitions()**: Currently `session_search` schema is injected in `agent_loop.rs:run()` (line 478+), not from the registry. After migration, `register_intercepted()` owns the schema — the agent_loop injection block MUST be removed. Leaving both in place would violate D-15 at runtime.
- **Calling dispatch() for intercepted tool names**: `execute_tool_call()` currently dispatches `session_search` via the hardcoded check and falls through to `registry.dispatch()` if no state_store. After migration, `dispatch_intercepts()` must fully own these tools; `dispatch()` must never be called for intercepted names.
- **Toolset filtering after is_available() in get_definitions()**: Filter order matters. Toolset-disabled tools should be excluded before `is_available()` is even called (performance). Order: disabled-toolset → is_available() → per-tool disabled list.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Config file atomic write | Custom write-and-rename | Existing `config_setter::config_set()` (Phase 23) | Already atomic via tempfile+rename per Phase 21.5/21.8 pattern |
| Toolset name validation | Custom regex | `validate_profile_name()` from `ironhermes_core::profile` | Already battle-tested with identical slug requirements |
| Dotted-path config read/write | String parsing | `config_setter::config_set()` / `config_getter::config_get()` | Phase 23 D-15; handles nested YAML, type coercion |
| Readline prompts in setup wizard | Raw stdin | `rustyline::DefaultEditor` + `prompt_with_default()` helper in `setup.rs` | Anti-Pattern #3 already implemented: no history bleed |
| Binary subprocess tests | Custom exec harness | `std::process::Command` + `CARGO_BIN_EXE_ironhermes` | Established pattern in `profile_isolation.rs` |
| Env var isolation in tests | Per-test env set/unset | `OnceLock<Mutex<()>>` env_lock pattern from `profile_isolation.rs:10-13` | Threading safety already solved |

**Key insight:** Every infrastructure primitive Phase 25 needs already exists in the codebase. This phase assembles existing parts, not invents new primitives.

---

## Common Pitfalls

### Pitfall 1: Toolset Name Mismatch (VERIFIED — HIGH RISK)
**What goes wrong:** Plan executes assuming `terminal` returns `"code"` and file tools return `"code"`, but they currently return `"system"` and `"file"` respectively. All toolset membership logic silently misbehaves — the `code` toolset shows 0 members.
**Why it happens:** CONTEXT.md D-01 describes the desired end-state, not the current state.
**How to avoid:** Plan 1 (or Wave 0) must include explicit one-liner changes to `toolset()` return values in `terminal.rs` (→ `"code"`) and all four `file_tools.rs` impls (→ `"code"`). These are cargo test-breaking changes if any test hardcodes `"system"` or `"file"`.
**Warning signs:** `hermes toolset list` shows `code` toolset with only `execute_code`; `terminal` and file tools appear under unknown/missing toolset.

### Pitfall 2: `cronjob` Toolset Name (NEEDS DECISION)
**What goes wrong:** `cronjob_tool.rs` returns `toolset() = "cronjob"`, but D-01 lists `cronjob` as a member of the `agent` toolset. If not updated, the cron tool will appear as a standalone toolset not in D-01's six compiled-in names.
**Why it happens:** The CONTEXT.md D-01 description says `agent — delegate_task, cronjob` but the source of truth (`toolset()` return) says `"cronjob"`.
**How to avoid:** Plan 1 must also update `cronjob_tool.rs` to return `"agent"`. This should be flagged as an explicit decision point — if the planner keeps `"cronjob"` as a standalone toolset name, D-01's six-toolset enumeration changes.
**Warning signs:** `hermes toolset list` shows a seventh `cronjob` toolset with no docs.

### Pitfall 3: `todo_write` / `todo_read` Don't Exist Yet (VERIFIED)
**What goes wrong:** D-13 lists `todo_write` and `todo_read` as intercepted tools, but no `todo_tool.rs` exists in `crates/ironhermes-tools/src/`. Attempting `register_intercepted("todo_write", ...)` with no schema source fails to give the LLM the tool definition.
**Why it happens:** These tools are new additions in Phase 25, not migrations of existing tools.
**How to avoid:** Plan 2 (registry expansion) must include creating the todo tool schema (minimal: `todo_write` with `content: String`, `todo_read` with no required params). The in-session state is a `Vec<String>` held in the intercept handler closure. This is a greenfield implementation, not a migration.
**Warning signs:** Integration test `intercepted_tool_no_schema_duplicate` fails to find `todo_write` or `todo_read` in schema output.

### Pitfall 4: `session_search` Schema Double-Registration
**What goes wrong:** `agent_loop.rs:run()` line 478+ still pushes `session_search_schema()` AFTER `get_definitions()` when `state_store` is Some. After migration, `register_intercepted("session_search", schema, handler)` also adds it. LLM sees the tool twice.
**Why it happens:** The migration removes one injection point but forgets the other.
**How to avoid:** The agent_loop.rs schema injection block (lines 478-484 in the verified code) must be deleted in the same plan that adds `register_intercepted("session_search", ...)`. The D-15 panic guard catches it at registry build — which means tests will fail loudly if both paths coexist.
**Warning signs:** D-26 integration test `intercepted_tool_no_schema_duplicate` fails; or D-15 panic fires in tests.

### Pitfall 5: `get_definitions()` Signature Change
**What goes wrong:** The existing `get_definitions(enabled_tools: Option<&[String]>)` signature is called with `None` at `agent_loop.rs:478` (all tools). Phase 25 D-23 layers toolset filtering on top. If the signature changes to add toolset config, all 3+ existing call sites break.
**Why it happens:** Adding a new parameter vs. reading config inside the method.
**How to avoid:** D-23 says toolset filtering uses the same parameter as per-tool filtering — the resolution order is applied inside `get_definitions()` using the active `ToolsConfig` which should be stored in the registry at build time (set once, not per-call). Callers continue passing `None` for `enabled_tools` (or a specific list). No signature change needed.
**Warning signs:** Compile errors at all `get_definitions(None)` call sites.

### Pitfall 6: Env Var Test Isolation
**What goes wrong:** Tests that set/unset `FIRECRAWL_API_KEY` to verify `is_available()` behavior race with other tests in the same process.
**Why it happens:** Rust runs `#[test]` functions in parallel threads in the same process by default.
**How to avoid:** Use the same `OnceLock<Mutex<()>>` env_lock pattern established in `profile_isolation.rs:10-13`. Every test that mutates env vars must hold this lock. The D-26 test `tool_excluded_when_prereq_missing` absolutely must use this pattern.
**Warning signs:** Test passes in isolation but flakes in parallel test run.

### Pitfall 7: `with_intercepts()` vs Existing Intercept Pattern
**What goes wrong:** The existing memory provider interception (lines ~981+) and session_search interception (lines ~951+) in `execute_tool_call()` are currently ad-hoc. The new `with_intercepts()` builder needs to wire ALL intercepted tools through `dispatch_intercepts()`. If memory provider tools (`memory_recall` etc.) are injected via `memory_manager.get_tool_schemas()` (line ~487+), they are NOT in the regular `tools` map — they have their own interception path that must remain separate from the D-14 intercepts map.
**Why it happens:** Two separate interception patterns: (a) `memory_provider_tool_names` set from `get_tool_schemas()` at session start, (b) named tools in `intercepts` HashMap. They are different mechanisms for different purposes.
**How to avoid:** D-12/D-14 only migrate `session_search` to the intercepts map. Memory provider tools stay on their existing path. `delegate_task`, `todo_write`, `todo_read`, `cronjob` are registered with `register_intercepted()` because their schemas are known at registry build time. Document this split clearly in the plan.

### Pitfall 8: Default Toolsets Break Existing Tests
**What goes wrong:** Existing tests build a `ToolRegistry` and call `get_definitions(None)` expecting all tools including web tools. After Phase 25, `get_definitions()` also applies toolset filtering — and the default is `web` disabled. Tests that expect `web_search` in the schema will fail.
**Why it happens:** D-20 changes the default from "all on" to "memory+session+agent+skills on."
**How to avoid:** `ToolRegistry::new()` should NOT apply toolset filtering unless a `ToolsConfig` is explicitly set on it. A separate `ToolRegistry::new_with_config(config)` or `registry.set_toolset_config(config)` lets tests remain default-all-on. Integration tests that test the D-20 default use a config with the defaults applied.
**Warning signs:** `delegate_task_semaphore_warn.rs` and `delegate_task_timeout_cancel.rs` tests fail because `delegate_task` is filtered out by the `agent` toolset being disabled by default.

---

## Code Examples

### Prerequisite Struct

```rust
// Source: CONTEXT.md D-09 + D-25 (plain-String cross-crate pattern)
// Location: crates/ironhermes-tools/src/registry.rs (or new prereq.rs)
pub struct Prerequisite {
    pub kind: String,        // "env_var" | "config_field"
    pub name: String,        // "FIRECRAWL_API_KEY" or "search.brave_api_key"
    pub description: String, // "Firecrawl API key for web search"
    pub required: bool,      // true = blocks; false = optional/fallback
}
```

### Tool Trait After Phase 25

```rust
// Source: registry.rs:10-22 (VERIFIED existing) + CONTEXT.md D-09 additions
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn toolset(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> ToolSchema;

    // EXISTING — default true
    fn is_available(&self) -> bool {
        // Default: walk prerequisites(), return true iff all required ones satisfied
        self.prerequisites()
            .iter()
            .filter(|p| p.required)
            .all(|p| match p.kind.as_str() {
                "env_var" => std::env::var(&p.name).is_ok(),
                "config_field" => true, // checked via config at call site
                _ => true,
            })
    }

    // NEW D-09 — default empty
    fn prerequisites(&self) -> Vec<Prerequisite> {
        vec![]
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String>;
}
```

NOTE: `web_search.rs:67` currently has a manual `is_available()` checking FIRECRAWL. After adding `prerequisites()`, either: (a) remove the manual override and let the default walk prereqs, or (b) keep the manual override for custom logic (D-09 allows both). The CONTEXT.md says "Tools can override is_available() for custom logic and MUST still implement prerequisites()." So web_search keeps its override but adds `prerequisites()`.

### ToolRegistry New Fields

```rust
// Source: CONTEXT.md D-12/D-14, based on verified registry.rs:24-37
pub type InterceptHandler = Arc<dyn Fn(serde_json::Value) -> anyhow::Result<String> + Send + Sync>;

pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,           // EXISTING
    guardrails: Vec<Box<dyn GuardrailHook>>,         // EXISTING
    error_detail: ErrorDetailLevel,                  // EXISTING
    intercepts: HashMap<String, (ToolSchema, InterceptHandler)>, // NEW D-14
    toolset_config: Option<ToolsConfig>,             // NEW D-22/D-23
}
```

### ToolsConfig in ironhermes-core config.rs

```rust
// Source: CONTEXT.md D-22; follows SubagentConfig pattern at config.rs:528
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
    pub disabled: Vec<String>,  // per-tool override within enabled toolset (D-23)
}

impl Default for ToolsConfig {
    fn default() -> Self {
        // D-20: fresh install defaults
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

### ToolsetSubcommand (new toolset_cmd.rs)

```rust
// Source: config_cli.rs pattern (VERIFIED) + CONTEXT.md D-04
#[derive(Subcommand)]
pub enum ToolsetSubcommand {
    /// List all toolsets with status and availability
    List,
    /// Enable a toolset (persists to config.yaml)
    Enable { name: String },
    /// Disable a toolset (persists to config.yaml)
    Disable { name: String },
    /// Show detail for one toolset
    Show { name: String },
    /// Walk through missing prerequisites interactively
    Setup,
}
```

### Commands enum addition (main.rs)

```rust
// Source: main.rs:102-164 (VERIFIED existing Commands enum)
// Add after Config variant:
/// Manage toolsets (Phase 25, D-04).
Toolset {
    #[command(subcommand)]
    subcommand: toolset_cmd::ToolsetSubcommand,
},
```

---

## Runtime State Inventory

> Phase 25 is greenfield feature addition, not a rename/refactor. No runtime state migration required.

| Category | Items Found | Action Required |
|----------|-------------|------------------|
| Stored data | No toolset names stored in any database | None |
| Live service config | None — toolset config is file-based | None |
| OS-registered state | None | None |
| Secrets/env vars | `FIRECRAWL_API_KEY` env var checked in `web_search.rs:67` (existing) | None — D-09 adds `prerequisites()` alongside, does not rename the var |
| Build artifacts | None | None |

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in test harness + `tokio::test` for async |
| Config file | `Cargo.toml` per crate; no separate test config |
| Quick run command | `cargo test --workspace --lib` |
| Full suite command | `cargo test --workspace` |
| Integration test location | `crates/ironhermes-cli/tests/toolset_integration.rs` (new), plus unit tests in `registry.rs` and `config.rs` |

### Critical Test Surfaces (D-26 mandatory + supporting)

**Integration Test 1 — `toolset_enable_disable_persists`**
- Spawn binary with fresh tempdir as `IRONHERMES_HOME`
- Run `hermes toolset enable web`
- Assert stdout contains `[toolset: web] enabled` (or similar confirmation)
- Assert `config.yaml` now contains `tools.toolsets.web.enabled: true`
- Run `hermes toolset list`; assert `web` shows as `enabled`
- Restart binary (new `Command::new(bin)` invocation, same `IRONHERMES_HOME`)
- Run `hermes toolset list`; assert `web` STILL shows as `enabled`
- Test location: `crates/ironhermes-cli/tests/toolset_integration.rs`
- Pattern: same subprocess pattern as `profile_isolation.rs`

**Integration Test 2 — `tool_excluded_when_prereq_missing`**
- Hold env_lock (MUST — env var mutation)
- Ensure `FIRECRAWL_API_KEY` is unset
- Build ToolRegistry with defaults (web toolset enabled in config)
- Call `registry.get_definitions(None)`
- Assert `web_search` schema NOT present in result
- Set `FIRECRAWL_API_KEY=test_key`
- Call `registry.get_definitions(None)` again
- Assert `web_search` schema IS present
- Unset `FIRECRAWL_API_KEY`, release env_lock
- Test location: `crates/ironhermes-tools/src/registry.rs` `#[cfg(test)]` or `crates/ironhermes-tools/tests/`

**Integration Test 3 — `intercepted_tool_no_schema_duplicate`**
- Build full registry with `register_intercepted()` for all D-13 tools
- Call `registry.get_definitions(None)`
- Collect all schema names
- For each of: `memory`, `session_search`, `delegate_task`, `todo_write`, `todo_read`, `cronjob`
  - Assert schema name appears **exactly once** in the combined list
- Test location: `crates/ironhermes-tools/src/registry.rs` `#[cfg(test)]`

**Supporting Unit Tests (not D-26 but needed for confidence)**

| Test | What it covers | Location |
|------|---------------|----------|
| `prerequisite_default_impl_returns_empty` | `Tool` default `prerequisites()` | `registry.rs` tests |
| `is_available_default_walks_prerequisites` | Default `is_available()` uses prereq list | `registry.rs` tests |
| `register_intercepted_panics_on_duplicate_with_tools` | D-15 panic guard | `registry.rs` tests |
| `register_tools_panics_on_duplicate_with_intercepts` | D-15 reverse guard | `registry.rs` tests |
| `list_unavailable_returns_missing_required_prereqs` | `list_unavailable()` correctness | `registry.rs` tests |
| `list_toolsets_returns_unique_set` | `list_toolsets()` deduplication | `registry.rs` tests |
| `toolset_disabled_excludes_all_member_tools` | toolset-level filter in `get_definitions()` | `registry.rs` tests |
| `dispatch_intercepts_returns_some_for_known` | `dispatch_intercepts()` routes correctly | `registry.rs` tests |
| `dispatch_intercepts_returns_none_for_unknown` | `dispatch_intercepts()` fallthrough | `registry.rs` tests |
| `tools_config_default_has_correct_enabled_set` | D-20 defaults | `config.rs` tests |
| `toolset_name_slug_validation` | D-02 regex | `profile.rs` or `toolset_cmd.rs` |

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command |
|--------|----------|-----------|-------------------|
| TOOL-01 | `is_available()` excludes tools with missing prereqs | unit | `cargo test -p ironhermes-tools is_available` |
| TOOL-01 | `prerequisites()` returns structured prereq list | unit | `cargo test -p ironhermes-tools prerequisite` |
| TOOL-02 | `hermes toolset enable` persists to config | integration | `cargo test -p ironhermes-cli toolset_enable_disable` |
| TOOL-02 | Toolset-level filter in `get_definitions()` | unit | `cargo test -p ironhermes-tools toolset_disabled` |
| TOOL-03 | `register_intercepted()` adds schema to output | unit | `cargo test -p ironhermes-tools intercepted_tool_no_schema` |
| TOOL-04 | No schema duplication for intercepted tools | integration | `cargo test -p ironhermes-tools intercepted_tool_no_schema_duplicate` |
| TOOL-05 | `hermes toolset setup` walks missing prereqs | integration | `cargo test -p ironhermes-cli toolset_setup` |

### Sampling Rate

- **Per task commit:** `cargo test --workspace --lib` (unit tests only, ~5s)
- **Per wave merge:** `cargo test --workspace` (includes integration tests, ~30-60s)
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `crates/ironhermes-cli/tests/toolset_integration.rs` — covers D-26 tests 1, 3
- [ ] `crates/ironhermes-tools/tests/toolset_prereq.rs` — covers D-26 test 2 + env_lock pattern
- [ ] No new framework install needed — standard cargo test harness already in place

---

## Environment Availability

> Step 2.6: All dependencies are Rust crates in-workspace or already in Cargo.toml. No external service dependencies.

| Dependency | Required By | Available | Notes |
|------------|------------|-----------|-------|
| `cargo test --workspace` | All tests | Yes | Verified: Rust workspace project |
| `CARGO_BIN_EXE_ironhermes` | Integration tests | Set by cargo at test time | Not a real env var; injected by test harness |
| `tempfile` crate | Integration tests | Yes — already in `Cargo.toml` for tests | Used in `profile_isolation.rs` |
| `FIRECRAWL_API_KEY` | `tool_excluded_when_prereq_missing` | Must be UNSET for negative case | Test controls this via env_lock |

---

## Open Questions (RESOLVED)

1. **`cronjob` toolset membership**
   - What we know: `cronjob_tool.rs` currently returns `toolset() = "cronjob"`. D-01 lists `cronjob` as a member of the `agent` toolset.
   - What's unclear: Is this a deliberate separation (cronjob is its own toolset) or should it be folded into `agent`?
   - Recommendation: Plan must explicitly update `cronjob_tool.rs` to return `"agent"` per D-01 mapping, OR planner creates a seventh `cronjob` toolset. Bring to user attention before Plan 1 executes.
   - **RESOLVED:** Plan 1 updates `crates/ironhermes-tools/src/cronjob_tool.rs` `toolset()` to return `"agent"` per D-01.

2. **`todo_write` / `todo_read` schema design**
   - What we know: These tools don't exist in the codebase yet. D-13 lists them as intercepted, D-09 requires `prerequisites()` on all tools.
   - What's unclear: Exact schema shape (fields, types) and in-session state structure for the todo list.
   - Recommendation: Plan 2 must include minimal schema definitions. Suggested: `todo_write(items: Vec<String>)` replaces the current list; `todo_read()` returns current list. Intercepted by an `Arc<Mutex<Vec<String>>>` in the AgentLoop state.
   - **RESOLVED:** Plan 2 creates `todo_write({"items": [string]})` and `todo_read({})` greenfield. State lives in `Arc<tokio::sync::Mutex<Vec<String>>>` owned by AgentLoop, passed via `with_intercepts()`.

3. **`InterceptHandler` async vs sync**
   - What we know: `dispatch_intercepts()` is called from `execute_tool_call()` which is `async fn`. Session_search currently uses `spawn_blocking` because StateStore is sync.
   - What's unclear: Should `InterceptHandler` be `async` (requiring `Box<dyn Future>`) or sync (requiring `spawn_blocking` inside handlers)?
   - Recommendation: Make `dispatch_intercepts` async; `InterceptHandler = Arc<dyn Fn(serde_json::Value) -> BoxFuture<'static, anyhow::Result<String>> + Send + Sync>`. The `spawn_blocking` for StateStore stays inside the closure. This avoids propagating the sync/async split into the trait surface.
   - **RESOLVED:** `InterceptHandler = Arc<dyn Fn(serde_json::Value) -> futures::future::BoxFuture<'static, anyhow::Result<String>> + Send + Sync>`. `spawn_blocking` for sync StateStore stays inside the closure.

4. **`enabled_tools` parameter interpretation after toolset layer**
   - What we know: Existing call at `agent_loop.rs:478` passes `None` (all tools). D-23 says this parameter becomes the per-tool override layer. The toolset config is a separate layer read from the registry's stored `ToolsConfig`.
   - What's unclear: Should `get_definitions(None)` mean "no per-tool override filter" (apply all toolset+prereq filters) or "truly all tools"?
   - Recommendation: `None` = "apply all filters (toolset + prereq + per-tool disabled list)." Passing `Some(list)` narrows further. This is the only interpretation consistent with D-23.
   - **RESOLVED:** `get_definitions(None)` means "no per-tool override list; apply all OTHER filters that the registry has configured (toolset filter if `toolset_config: Some(...)`, prereq filter via `is_available()`)". When `toolset_config: None` (pre-Phase-25 default state), no toolset filter is applied — preserves existing behavior per Pitfall 8 / Assumption A2. Document this in the rustdoc on `get_definitions` (Plan 03 Task 2 covers writing the rustdoc).

---

## Plan Split Suggestion

Given 26 decisions spanning 5 requirements across 4 crates, a logical 5-plan breakdown with clear dependency edges:

**Plan 1 — Trait Surface + Toolset Name Fixes** (`ironhermes-tools`)
- Add `Prerequisite` struct to `registry.rs`
- Add `prerequisites()` default method to `Tool` trait
- Update `is_available()` default to walk prerequisites
- Fix `toolset()` return values: `terminal.rs` "system"→"code", all 4 `file_tools.rs` "file"→"code", `cronjob_tool.rs` "cronjob"→"agent" (per resolution of Open Question 1)
- Add `prerequisites()` impls to `web_search.rs` (FIRECRAWL required:true) and `web_read.rs` (FIRECRAWL required:false)
- Covers: TOOL-01 partial, TOOL-02 foundation
- Dependencies: none (pure additive trait change)

**Plan 2 — Registry Expansion + Intercept Infrastructure** (`ironhermes-tools`)
- Add `intercepts` HashMap to `ToolRegistry`
- Add `register_intercepted()`, `dispatch_intercepts()`, `list_unavailable()`, `list_toolsets()`
- Add D-15 panic guards
- Create stub schemas for `todo_write`, `todo_read` (schemas only; handlers are no-op stubs)
- Unit tests: D-26 test 3, all `dispatch_intercepts` unit tests, `list_unavailable` unit tests
- Covers: TOOL-03, TOOL-04 partial
- Dependencies: Plan 1

**Plan 3 — Config + get_definitions() Wiring** (`ironhermes-core`, `ironhermes-tools`, `ironhermes-agent`)
- Add `ToolsConfig` + `ToolsetEntry` structs to `config.rs`
- Add `DEFAULT_TOOLSETS` to `constants.rs`
- Add `toolset_config: Option<ToolsConfig>` field to `ToolRegistry`
- Update `get_definitions()` to apply toolset filter (resolution order: disabled-toolset → is_available() → per-tool disabled list)
- Migrate `agent_loop.rs`: remove session_search schema injection block (line 478+); add `with_intercepts()` builder; call `dispatch_intercepts()` before `dispatch()` in `execute_tool_call()`
- Unit tests: D-26 test 2, toolset-filter tests, config-default tests
- Covers: TOOL-02 complete, TOOL-04 complete
- Dependencies: Plans 1 + 2

**Plan 4 — CLI Subcommand** (`ironhermes-cli`)
- Create `crates/ironhermes-cli/src/toolset_cmd.rs`
- Add `Commands::Toolset` variant to `main.rs`
- Implement `list`, `enable`, `disable`, `show` handlers (using existing config_setter)
- Add stderr cache-break banner on enable/disable (D-04 / D-06 banner spec)
- Update slash command registry: replace `"toolsets"` stub with full `"toolset"` entry
- Integration tests: D-26 test 1
- Covers: TOOL-02 operator surface
- Dependencies: Plan 3

**Plan 5 — Setup Wizard Hook + Preflight** (`ironhermes-cli`)
- Replace `run_tools_section()` stub with real prereq-walking prompts (D-18)
- Add `hermes toolset setup` routing in `toolset_cmd.rs`
- Update `preflight::run_preflight_check()` to call `registry.list_unavailable()` and emit stderr banner for required missing prereqs (D-17)
- Add opt-in "optional tool prerequisites" final stage to `hermes setup` (D-19)
- Integration test: toolset_setup subprocess test
- Covers: TOOL-05
- Dependencies: Plans 3 + 4

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `InterceptHandler` should be async (`BoxFuture`) | Code Examples, Open Q 3 | If sync, `spawn_blocking` propagates into all intercept closures; design gets messier |
| A2 | `ToolRegistry::new()` without `ToolsConfig` should NOT apply toolset filtering | Pitfall 8 | Breaking change to all existing tests that use `get_definitions(None)` and expect all tools |
| A3 | `cronjob_tool.rs` should return `"agent"` per D-01 | Pitfall 2, Plan Split | If kept as `"cronjob"`, seventh toolset appears outside D-01's six-toolset enumeration |

---

## Sources

### Primary (HIGH confidence — VERIFIED in this session)

- `crates/ironhermes-tools/src/registry.rs` — full `Tool` trait and `ToolRegistry` (lines 1-601 read)
- `crates/ironhermes-agent/src/agent_loop.rs:478-484` — session_search schema injection (VERIFIED)
- `crates/ironhermes-agent/src/agent_loop.rs:826-1000` — `execute_tool_call()` full body (VERIFIED)
- `crates/ironhermes-tools/src/web_search.rs:67` — `is_available()` checking FIRECRAWL_API_KEY (VERIFIED)
- `crates/ironhermes-tools/src/web_read.rs:521` — `is_available()` returning `true` (VERIFIED)
- `crates/ironhermes-tools/src/terminal.rs` — `toolset()` returns `"system"` (VERIFIED)
- `crates/ironhermes-tools/src/file_tools.rs` — `toolset()` returns `"file"` (VERIFIED)
- `crates/ironhermes-tools/src/cronjob_tool.rs` — `toolset()` returns `"cronjob"` (VERIFIED)
- `crates/ironhermes-cli/src/main.rs:200-300` — preflight gate at lines 267-268 (VERIFIED)
- `crates/ironhermes-cli/src/main.rs:102-164` — `Commands` enum (VERIFIED — no Toolset variant yet)
- `crates/ironhermes-cli/src/setup.rs:83,239-242` — `run_tools_section()` stub (VERIFIED)
- `crates/ironhermes-cli/src/setup.rs:250-261` — `apply_minimum_viable_answers` signature (VERIFIED)
- `crates/ironhermes-cli/src/config_cli.rs` — reference subcommand pattern (VERIFIED)
- `crates/ironhermes-core/src/commands/registry.rs` — `/toolsets` stub already in `build_registry()` (VERIFIED)
- `crates/ironhermes-core/src/config.rs:528-568` — `SubagentConfig` + `default_toolsets` field (VERIFIED)
- `crates/ironhermes-cli/tests/profile_isolation.rs` — subprocess integration test pattern (VERIFIED)
- `crates/ironhermes-agent/src/agent_wiring.rs` — wiring helper (does NOT contain delegate_task intercept; VERIFIED)

### Secondary (MEDIUM confidence)

- CONTEXT.md D-01..D-26 — locked decisions (user-provided; cross-checked against codebase)

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all Rust in-workspace; no new dependencies
- Architecture: HIGH — all key code sites verified line-by-line
- Pitfalls: HIGH — most sourced from actual verified mismatches in live code
- Plan split: MEDIUM — logical grouping based on code evidence; planner may adjust boundaries

**Research date:** 2026-04-29
**Valid until:** 2026-05-29 (stable Rust workspace; 30-day window appropriate)

---

## RESEARCH COMPLETE
