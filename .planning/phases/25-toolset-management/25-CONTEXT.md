# Phase 25: Toolset Management - Context

**Gathered:** 2026-04-29
**Status:** Ready for planning
**Source:** Discuss-phase with Claude's defaults (user opted into "Skip — Claude picks defaults" alongside selecting all gray areas)

<domain>
## Phase Boundary

Tools are organized into **named toolsets** with operator-controllable enable/disable, **prerequisite checks** (`is_available()`) that silently exclude unavailable tools from the LLM-visible schema, and a **setup-wizard hook** that surfaces missing prerequisites. Phase 25 ships exactly one new operator-facing primitive — `hermes toolset` — plus the supporting plumbing to make every tool route through a single registration → availability → enablement decision point.

This phase covers **TOOL-01..TOOL-05** only:
- TOOL-01: `is_available()` implementations on tools that need env vars / API keys
- TOOL-02: Named toolsets + operator enable/disable
- TOOL-03: Single-call registration (no dispatch-layer changes when adding a tool)
- TOOL-04: Agent-intercepted tools (memory, session_search, delegate_task, todo) standardized
- TOOL-05: Setup-wizard prerequisite probing

**Out of scope:**
- Per-tool-call permission prompts (separate concern; not in REQUIREMENTS.md for v2.1)
- Plugin/extension system for runtime tool loading (PROJECT.md "Out of Scope")
- Toolset versioning / compatibility ranges
- Per-toolset rate limiting

</domain>

<decisions>
## Implementation Decisions

### Toolset Presets & Membership (TOOL-02)

- **D-01: Six concrete toolsets ship in v2.1, named by capability domain.** The names map 1:1 to the existing `Tool::toolset()` return values plus two new groupings:
  - `web` — `web_search`, `web_read` (Firecrawl/Brave fallback)
  - `code` — `execute_code` (Python sandbox), `terminal` (shell), `file_tools::*` (read_file/write_file/list_dir/grep_files)
  - `memory` — `memory` tool (intercepted, see D-13)
  - `agent` — `delegate_task` (intercepted), `cronjob`
  - `skills` — `skills` tool (skills system)
  - `session` — `session_search` (intercepted, already in tree)
  Toolsets are **flat** (no nesting). A tool belongs to exactly one toolset (its `toolset()` return value is the source of truth — already a `&'static str` on the trait).

- **D-02: Toolset names are validated as slugs** using the same `validate_profile_name`-style regex from Phase 24 D-03 (`[a-z0-9][a-z0-9-]*`). Rejection of unknown toolset names happens at config load and on `hermes toolset enable` calls. No reserved names beyond the six built-ins. Custom toolset definitions are NOT supported in Phase 25 — extending requires adding a tool with a new `toolset()` return value AND updating the toolset enumeration in core (compiled-in, single-binary).

- **D-03: Toolset membership is read at runtime from the `Tool::toolset()` method.** No separate registry table. `ToolRegistry::list_toolsets()` returns the unique set of `toolset()` values across all currently-registered tools. This means MCP tools that report `toolset() = "mcp"` (or per-server, e.g., `"mcp__github"`) integrate without code changes — Phase 25 D-13 expands the trait contract here.

### Operator Control Surface (TOOL-02)

- **D-04: New `hermes toolset` subcommand namespace** with three minimum-viable subcommands:
  - `hermes toolset list` — print all toolsets, their tools, enabled/disabled status, and per-tool availability (✓ available / ✗ missing prerequisites)
  - `hermes toolset enable <name>` / `hermes toolset disable <name>` — persistent enable/disable, writes to active profile's config.yaml
  - `hermes toolset show <name>` — detailed view of one toolset (members, schemas, prerequisites)
  Mirrors the `hermes config` / `hermes status` namespace style from Phase 23/21.7. NO `hermes toolset create/delete/rename/alias/import/export` — toolsets are compiled-in (D-01).

- **D-05: Enable/disable is persistent and per-profile.** `hermes --profile work toolset disable web` writes to `~/.ironhermes/profiles/work/config.yaml`; bare `hermes toolset disable web` writes to `~/.ironhermes/config.yaml`. Phase 24's `IRONHERMES_HOME` pivot makes this automatic — no special-casing in toolset commands.

- **D-06: Slash commands mirror the CLI subcommands at runtime.** `/toolset list`, `/toolset enable web`, `/toolset disable web`, `/toolset show web` register through Phase 21.1's CommandRouter, dispatching to the same handler functions used by the CLI. Slash commands take effect for the **current session only** — they do NOT persist to config.yaml. Persistent changes require the CLI subcommand. Mirrors the `/personality` overlay vs `hermes config set personality.default` distinction from prior phases.

- **D-07: NO `--toolset` global flag.** Operator chooses toolsets by editing config or running enable/disable; bare `hermes chat` honors the active profile's enabled toolsets. Per-invocation override would invite scope creep and complicate the prompt-cache story (Principle #2 from PROJECT.md "Architectural Principles" — switching toolsets is cache-breaking via the schema delta).

### `is_available()` Semantics (TOOL-01)

- **D-08: `is_available()` is a pure synchronous function returning `bool`.** No async, no network probes, no caching layer. Implementations check **only**: (a) presence of named env vars, (b) presence of named config fields. Tools that need network connectivity test it lazily on `execute()` and return a clear error string — they don't gate `is_available()` on it (matches the Phase 23 wizard's "config completeness" framing, not "service liveness").

- **D-09: Per-tool prerequisite reporting via a new `prerequisites()` method on the `Tool` trait.** Returns `Vec<Prerequisite>` where `Prerequisite` is a plain-String struct (D-17 cross-crate convention from Phase 22.4.2.2):
  ```rust
  pub struct Prerequisite {
      pub kind: String,       // "env_var" | "config_field"
      pub name: String,       // "FIRECRAWL_API_KEY" or "search.brave_api_key"
      pub description: String,// "Firecrawl API key for web search"
      pub required: bool,     // true = blocks; false = optional/fallback
  }
  ```
  Default impl returns empty Vec (most tools have no prereqs). `is_available()` default impl walks `prerequisites()` and returns true iff every `required: true` prerequisite is satisfied. Tools can override `is_available()` for custom logic (e.g., "either FIRECRAWL_API_KEY or BRAVE_API_KEY") and they MUST still implement `prerequisites()` for setup-wizard guidance.

- **D-10: Schema exclusion is silent.** `ToolRegistry::get_definitions()` already filters by `is_available()` — Phase 25 keeps this behavior verbatim. NO stderr warnings on excluded tools (would spam every CLI invocation). NO tool-listing stutter for the LLM. Operator-facing `hermes toolset list` is the single discovery surface for "what's available vs blocked."

- **D-11: The check-time is registry-build-time + each `get_definitions()` call.** Since `is_available()` is pure and synchronous, calling it on every schema build is fast (microseconds for env var lookups). This catches mid-session env changes (e.g., operator runs `export FIRECRAWL_API_KEY=...` in another terminal and restarts hermes) without needing a cache-invalidation story.

### Agent Interception (TOOL-04)

- **D-12: Interception is a `dispatch_intercepts(name, args) -> Option<InterceptResult>` method on `ToolRegistry`.** Called by `agent_loop::execute_tool_call` BEFORE the normal `dispatch()` path. Returns `Some(result)` when the tool is intercepted, `None` to fall through to registry dispatch. The current hardcoded session_search match block at `agent_loop.rs:951` migrates into this method (it stays a match block, but lives in the registry and is reachable by all callers).

- **D-13: Five tools are intercepted in v2.1:**
  - `memory` — routed to `MemoryManager` (already exists; needs to move out of `dispatch()` registration or stay registered with intercept-priority — see D-14)
  - `session_search` — routed to `StateStore::search` (already intercepted at agent_loop.rs:951; migration only)
  - `delegate_task` — routed to `SubagentRunner` (depth + concurrency control)
  - `todo_write` / `todo_read` — routed to in-session todo state (one logical "todo" surface, two tool names)
  - `cronjob` — routed via the OriginDecision (Phase 22.4.2.2) so CLI vs LLM call sites get the right `deliver` default
  Each intercepted tool's schema MUST be added to the LLM-visible tool list exactly once (see D-15).

- **D-14: Intercepted tools are NOT registered in the regular `tools` HashMap.** Instead, the registry exposes `register_intercepted(name, schema, handler)` that stores them in a separate `intercepts: HashMap<String, InterceptHandler>` map. `get_definitions()` returns schemas from BOTH maps so the LLM sees the full surface, but `dispatch()` checks `intercepts` first. This sidesteps schema duplication entirely — there is one source of truth per tool name. The `intercepted` boolean on `Tool` is NOT added to the trait (per D-12, interception lives in the registry, not on the tool).

- **D-15: Schema duplication is structurally prevented.** A tool name registered in BOTH `tools` and `intercepts` causes a `panic!()` at registry build (development-time error, never reachable in shipped binary). `register()` rejects names that already exist in `intercepts`; `register_intercepted()` rejects names that already exist in `tools`. This makes "same tool seen twice by the LLM" impossible.

- **D-16: Interception is opt-in per-callsite via a new `with_intercepts(...)` builder method on `AgentLoop`.** Default `AgentLoop::new()` registers no intercepts (CLI test harnesses, batch processing, and offline subagent runs benefit from a no-magic registry). Callers that want the full intercept set call `agent.with_intercepts(memory_manager, state_store, subagent_runner, todo_state, cron_router)` — same pattern as `with_state_store()` from prior phases. This keeps the trait surface stable and avoids breaking existing tests.

### Setup Wizard Integration (TOOL-05)

- **D-17: Phase 23's `preflight::run_preflight_check` gains a "tool prerequisites" check phase.** After validating config.yaml structure, the preflight calls `ToolRegistry::list_unavailable()` (returns `Vec<(tool_name, Vec<unsatisfied_Prerequisite>)>`). For tools with `required: true` prereqs missing, the preflight emits a warning banner to stderr listing them and exits the preflight without auto-launching the wizard — the operator can always run `hermes toolset setup` to fix prereqs (D-19). For tools with `required: false` prereqs missing, no banner. Mirrors Phase 23 D-13 cache-breaking warning style.

- **D-18: `hermes toolset setup` is a new subcommand** that walks the operator through every unsatisfied required prerequisite, one tool at a time. For each prerequisite:
  - Display tool name, description, prerequisite kind/name, and description
  - For `kind: "env_var"`: prompt for the value, optionally write to `~/.ironhermes/profiles/<active>/.env` (matches Phase 23 `apply_minimum_viable_answers` `.env` write pattern)
  - For `kind: "config_field"`: prompt for the value, write to active profile's `config.yaml` via the dotted-path config setter (Phase 23 D-15)
  - Skip option: "Mark this tool as never-prompt" — writes a `tools.skip_prompts: [<tool_name>]` config entry; tool stays unavailable but no banner
  Mirrors the rustyline-driven UX from Phase 23's setup wizard. Reuses `apply_minimum_viable_answers` testability seam where possible.

- **D-19: First-run integration with `hermes setup`.** The existing `hermes setup` command (Phase 23) gains a final stage AFTER the model/key wizard: "Optional tool prerequisites" — same prompts as `hermes toolset setup`, but presented as opt-in "want to enable additional tools now?" rather than forced gating. Operator can decline ("No, I'll set these up later") and `hermes setup` completes with the minimum-viable config from Phase 23. NO change to Phase 23's `apply_minimum_viable_answers` minimum — tool prereqs are explicitly above the floor.

### Default Toolset & Per-Profile Override

- **D-20: Default toolset on a fresh install is `[memory, session, agent, skills]`** — the four "internal" toolsets that have no external prerequisites. `web` and `code` are DISABLED by default because:
  - `web` requires `FIRECRAWL_API_KEY` (or fallback) and operators may not have one
  - `code` (terminal + execute_code + file_tools) is high-blast-radius — opt-in is safer for a fresh install
  This is a behavior change from the current "all-tools-on" default; it's locked in v2.1 because Phase 25 introduces the toolset notion and this is the install-time decision point. Operators who want everything: `hermes toolset enable web && hermes toolset enable code`.

- **D-21: Per-profile override is automatic via Phase 24.** `~/.ironhermes/profiles/work/config.yaml` can have `tools.toolsets.web.enabled: true` while `~/.ironhermes/profiles/personal/config.yaml` has it disabled. No special handling in toolset commands — they read/write the active profile's config like every other Phase 23 D-15 dotted-path setter. The "default" toolset list lives in `ironhermes-core::constants` as `DEFAULT_TOOLSETS: &[&str] = &["memory", "session", "agent", "skills"]`.

### Config Persistence Shape

- **D-22: Toolset state lives under `tools` in config.yaml as a per-toolset block:**
  ```yaml
  tools:
    toolsets:
      web: { enabled: false }
      code: { enabled: false }
      memory: { enabled: true }
      session: { enabled: true }
      agent: { enabled: true }
      skills: { enabled: true }
    skip_prompts: []   # tool names to never re-prompt for prereqs
  ```
  The block-per-toolset shape (vs flat list `enabled_toolsets: [...]`) is more extensible — Phase 25 can add `enabled: true/false` only, but future phases (e.g., per-toolset rate limit, per-toolset model override) can extend the block without breaking the schema.

- **D-23: Existing `enabled_tools: Option<&[String]>` field on `get_definitions()` is REPURPOSED as a per-tool override layer.** Resolution order:
  1. If toolset is disabled → exclude all tools in toolset
  2. If toolset is enabled → include all tools in toolset
  3. If `tools.disabled: [<tool_name>]` config field present → exclude that specific tool (within an enabled toolset)
  4. If `is_available()` returns false → exclude (already filtered, D-10)
  This gives operators "I want web toolset enabled but specifically the slow `web_read` tool disabled" without inventing a new field.

- **D-24: Config schema migration is automatic and silent.** Existing installs without a `tools` block: load defaults from D-20 on first read, write back on next config save. No migration banner — Phase 23 D-13 cache-breaking warnings cover the model.default case; toolsets are not cache-breaking (the LLM doesn't see the toolset boundary, only the per-tool schema list).

### Cross-Crate Type Pattern (carry-forward)

- **D-25: New types in `ironhermes-tools` crossing crate boundaries use plain Strings** per Phase 22.4.2.2 / Phase 23 D-12 / Phase 24 D-17. `Prerequisite { kind: String, name: String, description: String, required: bool }` is a plain struct, not an enum. Consumers (CLI subcommand dispatch, setup wizard, MCP integration) match on `kind` strings at the call site.

### Test Strategy (locked at this stage)

- **D-26: Three integration tests are mandatory.** Plan must lock all three:
  1. `toolset_enable_disable_persists` — `hermes toolset enable web` writes config, `hermes toolset list` shows enabled, restart binary, list still shows enabled.
  2. `tool_excluded_when_prereq_missing` — Spawn binary with `FIRECRAWL_API_KEY` unset; assert `web_search` schema is NOT in the get-definitions output; set the env, restart, assert it IS in the output.
  3. `intercepted_tool_no_schema_duplicate` — Boot full registry with intercepts; for each intercepted tool name (memory, session_search, delegate_task, todo_write, todo_read, cronjob), assert exactly ONE schema entry across all sources combined.

### Folded Todos

- *(none — no pending todos in `.planning/todos/` match Phase 25 scope; will be re-checked at plan-phase)*

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### v2.1 Milestone Architectural Principles
- `.planning/PROJECT.md` §"Architectural Principles (carried through every v2.1 phase)" — Principle #2 (cache-awareness): toolset enable/disable IS schema-altering and therefore cache-breaking on the next LLM call. Phase 25 must surface this in `hermes toolset enable/disable` output (one-line stderr warning, mirrors Phase 23 D-13).
- `.planning/REQUIREMENTS.md` §"Tool Registry" lines 121-125 — TOOL-01..05 verbatim text.
- `.planning/ROADMAP.md` §"Phase 25: Toolset Management" lines 480-498 — full success criteria + dependencies on Phase 23 (CFG-01 wizard) and Phase 21.1 (slash command registry).

### Phase 23 Carry-Forward (REQUIRED reading)
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-CONTEXT.md` — preflight middleware semantics (Phase 25 D-17 extends), dotted-path config setter (D-22), `apply_minimum_viable_answers` testability seam (D-18 reuse), Learning Loop banner stack ordering.
- `.planning/phases/23-configuration-cli-and-setup-wizard/23-VERIFICATION.md` — locks the preflight gate location at `crates/ironhermes-cli/src/main.rs:213-223`. Phase 25 D-17 inserts the tool-prereq stage AFTER the existing config check, BEFORE the wizard launch decision.

### Phase 24 Carry-Forward (REQUIRED reading)
- `.planning/phases/24-profile-isolation/24-CONTEXT.md` — `IRONHERMES_HOME` pivot makes per-profile toolset config automatic (D-05). NO special-casing needed in toolset commands.
- `.planning/phases/24-profile-isolation/24-01-SUMMARY.md` — `validate_profile_name` slug validator pattern reused for D-02 toolset-name validation.

### Phase 21.1 Carry-Forward (REQUIRED reading)
- `.planning/phases/21.1-slash-commands/` — CommandRouter three-stage resolve. Phase 25 D-06 registers `/toolset` slash commands through this router.

### Phase 22.4.2.2 Carry-Forward
- `.planning/PROJECT.md` Key Decisions row "Cross-crate transport types use plain Strings" — Phase 25 D-25 follows verbatim.
- `crates/ironhermes-cron/src/lib.rs` — `OriginDecision` pattern. Phase 25 D-13 reuses for `cronjob` interception routing.

### Codebase Code Sites (verified via grep)
- `crates/ironhermes-tools/src/registry.rs:11-22` — current `Tool` trait. Phase 25 ADDS `prerequisites()` method (D-09); does NOT modify existing methods.
- `crates/ironhermes-tools/src/registry.rs:30-95` — current `ToolRegistry`. Phase 25 ADDS `intercepts: HashMap<String, InterceptHandler>`, `register_intercepted()`, `dispatch_intercepts()`, `list_unavailable()`, `list_toolsets()`. Does NOT modify `register()` or `get_definitions()` semantics beyond schema sourcing from both maps.
- `crates/ironhermes-agent/src/agent_loop.rs:479` — D-07 session_search schema injection. Phase 25 D-14 migrates this into the registry; agent_loop becomes a one-line `tool_schemas.extend(registry.get_definitions(enabled))` call.
- `crates/ironhermes-agent/src/agent_loop.rs:951-961` — D-07 session_search interception block. Phase 25 D-12 migrates this into `registry.dispatch_intercepts()`.
- `crates/ironhermes-tools/src/web_search.rs:68` — existing `is_available()` checking `FIRECRAWL_API_KEY`. Phase 25 D-09 generalizes this via `prerequisites()` for setup-wizard discovery.
- `crates/ironhermes-tools/src/web_read.rs:172-173` — same pattern. Phase 25 D-09 adds `prerequisites()` here too.
- `crates/ironhermes-tools/src/{cronjob_tool,delegate_task,execute_code,file_tools,memory_tool,skills_tool,terminal,web_read,web_search}.rs` — all have existing `toolset()` returns. Phase 25 D-03 reads these as the source of truth.

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets

- **`Tool::is_available()` (`registry.rs:17-19`)** — already exists with default `true` impl. Phase 25 keeps the default and adds `prerequisites()` alongside it (D-09).
- **`ToolRegistry::get_definitions(enabled_tools)` (`registry.rs:76-87`)** — already filters by `is_available()` AND a per-tool name list. Phase 25 D-23 layers toolset-level filtering on top without touching this method's signature.
- **`Tool::toolset()` (`registry.rs:13`)** — already exists, returns `&str`. All 12 built-in tool modules implement it. Phase 25 D-03 uses this as the membership source of truth — no separate registry table.
- **Existing `is_available()` impl on web tools (`web_search.rs:68`, `web_read.rs:172`)** — env-var-presence check. Phase 25 generalizes the pattern via `prerequisites()`.
- **Phase 23's `apply_minimum_viable_answers` testability seam (`setup_wizard.rs:250`)** — Phase 25 D-18 reuses this for tool-prereq setup tests.
- **Phase 21.1 `CommandRouter`** — Phase 25 D-06 registers `/toolset list/enable/disable/show` here.
- **Phase 24's profile pivot (`main.rs::resolve_and_set_profile`)** — Phase 25 D-21 inherits per-profile config without any new code.

### Established Patterns

- **Cross-crate plain-String pattern (Phase 22.4.2.2 → 23 D-12 → 24 D-17)** — Phase 25 D-25 follows verbatim for `Prerequisite`.
- **Block-per-config-section shape (Phase 23 D-15 dotted-path setter)** — Phase 25 D-22 uses block-per-toolset rather than flat list for forward-compat.
- **Stderr-banner UX convention (Phase 21.7 D-11/D-12 yolo, Phase 24 D-08 profile)** — Phase 25 D-17 mirrors for tool-prereq warnings; D-04 mirrors for cache-break warning on `hermes toolset enable/disable`.
- **Subcommand-namespace minimum surface (Phase 23 `hermes config`, Phase 24 `--profile`)** — Phase 25 D-04 stays minimum: list/enable/disable/show + `setup`. NO create/delete/rename/alias/import/export.
- **Atomic file writes via tempfile + rename (Phase 21.5/21.8/24 D-10)** — Phase 25 D-22 reuses for config.yaml writes (the existing config setter already does this).

### Integration Points

- **`crates/ironhermes-cli/src/main.rs` after Phase 24 pivot, before preflight** — Phase 25 D-17 inserts the tool-prereq probe call between `ensure_home_dirs` and `preflight::run_preflight_check`. Single insertion point.
- **`crates/ironhermes-cli/src/main.rs` Cli struct** — Phase 25 D-04 adds `Toolset(ToolsetCommand)` to the `Commands` enum (mirrors Phase 23's `Config(ConfigCommand)`).
- **`crates/ironhermes-tools/src/registry.rs`** — Phase 25 expands the `ToolRegistry` API but the existing `Tool` trait gains exactly one method (`prerequisites()`) with a default impl, so existing impls compile without changes.
- **`crates/ironhermes-agent/src/agent_loop.rs`** — Phase 25 D-12/D-14 collapses two hardcoded session_search blocks into the registry's intercept dispatch.
- **`crates/ironhermes-cli/src/setup.rs`** — Phase 25 D-19 adds an "optional tool prerequisites" stage to the existing wizard.

</code_context>

<specifics>
## Specific Ideas

- **`Prerequisite::kind` is a string union, not an enum.** The two values for v2.1 are `"env_var"` and `"config_field"`. This is the D-25 cross-crate convention applied — keeps `ironhermes-tools` from needing a downstream type. Future kinds (e.g., `"network"`, `"binary_present"`) extend without breaking.
- **Banner format on `hermes toolset enable web`**: `[toolset: web] enabled — schema cache will rebuild on next LLM call`. Mirrors Phase 24 D-08 banner style. Operator-facing signal that the change is cache-breaking.
- **`hermes toolset list` output format**: aligned columns showing toolset / status / member count / availability — like `hermes status --all` from Phase 21.7.
  ```
  TOOLSET   STATUS    TOOLS  AVAILABLE
  web       enabled   2      1/2 (web_search ✓, web_read ✗ FIRECRAWL_API_KEY)
  code      disabled  4      4/4
  memory    enabled   1      1/1
  ...
  ```
  `--json` output adds full per-tool detail (name, schema summary, prereqs).
- **Slash command naming**: `/toolset` (singular), not `/toolsets`. Mirrors `/personality` (not `/personalities`). Subcommands: `/toolset list`, `/toolset enable web`, `/toolset disable web`, `/toolset show web`.
- **Documentation for `hermes toolset setup`**: include the line "this is the per-tool prerequisite walkthrough; for the model+key initial setup use `hermes setup` instead." Avoids operator confusion about which wizard does what.
- **The `delegate_task` tool's interception** is already partially in the agent_wiring.rs — Phase 25 D-12 standardizes this without changing the depth/concurrency control logic. The intercept handler IS `SubagentRunner::run`.

</specifics>

<deferred>
## Deferred Ideas

- **`hermes toolset create/delete/rename/alias/import/export`** — full toolset lifecycle. Skipped; toolsets are compiled-in for v2.1 (D-01). Re-open in v2.2 if the Skills Hub or MCP-driven discovery needs runtime toolset definitions.
- **Per-tool-call permission prompts** ("Allow `terminal` to run `rm -rf /`? [y/n]") — separate concern, not in REQUIREMENTS.md. Punt to a future "Tool Sandboxing" phase.
- **Per-toolset rate limiting / quota** — not in REQUIREMENTS.md for v2.1. Add when usage data justifies it.
- **`--toolset web` per-invocation override flag** — explicitly NOT chosen (D-07). Cache-break implications and scope creep.
- **Custom user-defined toolset definitions** (e.g., `tools.custom_toolsets.my_combo: [web_search, terminal, memory]`) — deferred to v2.2 alongside Plugin/Extension if it materializes. Phase 25 v2.1 is compiled-in only.
- **Toolset versioning / compatibility ranges** — overkill for single-binary deployment.
- **MCP-server-as-toolset auto-grouping** — interesting (each MCP server's tools become an auto-toolset named `mcp__{server}`). The `Tool::toolset()` method already supports it, but the `hermes toolset` CLI surface in Phase 25 only enumerates the six built-in toolsets — MCP toolsets show up in `hermes toolset list` but cannot be enabled/disabled via the CLI yet. Re-open when MCP gets a proper management layer.
- **`hermes doctor --tools`** — cross-tool prerequisite check (would walk all tools and report missing prereqs). Skipped per "active scope only" pattern from Phase 24 D-16. Operator uses `hermes toolset list` + `hermes toolset setup` instead.
- **First-run "all tools on" default** — REJECTED in favor of D-20 safe-by-default (memory + session + agent + skills enabled; web + code opt-in). v2.0 behavior was "all on" but v2.1 introduces toolsets and this is the right inflection point to lock the safer default.

### Reviewed Todos (not folded)

None.

</deferred>

---

*Phase: 25-toolset-management*
*Context gathered: 2026-04-29 via /gsd-discuss-phase with Claude's defaults*
*All gray areas decided; user reserves the right to object before plan-phase or execute-phase.*
