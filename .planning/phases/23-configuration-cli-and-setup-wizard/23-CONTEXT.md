# Phase 23: Configuration CLI and Setup Wizard - Context

**Gathered:** 2026-04-27
**Status:** Ready for planning

<domain>
## Phase Boundary

Users can configure IronHermes interactively on first run AND manage `config.yaml` values from the command line. Phase 23 ships three CLI surfaces:

1. **`hermes setup [section]`** — interactive first-run wizard with section-based subcommand routing. Bare `hermes setup` runs the minimum-viable flow (provider + API key + model). Section variants (`hermes setup model|memory|gateway|tools`) configure one section each.
2. **`hermes config <subcommand>`** — manage live config: `set <dotted.key> <value>`, `get <dotted.key>`, `show`, `migrate`, `path`, `env-path`.
3. **First-run auto-launch** — running `hermes` (no subcommand) with missing OR invalid config triggers the wizard before chat starts; valid config drops straight into chat.

This phase covers **CFG-01, CFG-02, CFG-03**. CFG-04 (profile isolation) is Phase 24 — out of scope here. The agent + skills config sections are deferred to Phases 26 and 28 respectively (those phases own the underlying config schema).

</domain>

<decisions>
## Implementation Decisions

### Wizard Rendering & UX

- **D-01:** The setup wizard renders via **inline rustyline prompts** (sequential question-by-question). Reuses the rustyline 15 infrastructure landed in Phase 22.3 (history activation, `set_history_ignore_dups`, `set_max_history_size(1000)`). No ratatui form rendering for v2.1. Rationale: lightweight, scriptable (can be piped/automated), zero new deps, and works in pre-TTY contexts where the wizard auto-launches before any chat session.
- **D-02:** The wizard uses **section-based subcommand routing**. `hermes setup` (no arg) runs the minimum-viable flow (provider + API key + model). `hermes setup model|memory|gateway|tools` configures one section. Mirrors hermes-agent's `hermes setup [section]` design exactly. Phase 25 (Toolset Management) and Phase 26 (Provider Polish) MAY plug additional questions into their respective sections later — Phase 23 establishes the dispatch surface.
- **D-03:** Sections accepted in v2.1: **`model`**, **`memory`**, **`gateway`**, **`tools`** (4 sections). `agent` and `skills` sections are explicitly deferred to Phases 26 and 28 respectively, since those phases own the corresponding config schema additions. `hermes setup agent` should error cleanly with "section deferred to Phase 26" or be omitted from the help text entirely.
- **D-04:** Each section's question flow shows **defaults inline** (e.g., `Model [openrouter/qwen-2.5-coder-32b]:`) and **validates per-answer** (provider exists in registry; API key non-empty; model resolves). On invalid answer: re-prompt with the validation error inline. No silent acceptance.

### First-Run Auto-Launch

- **D-05:** Auto-launch trigger: missing `~/.ironhermes/config.yaml` OR validation failure (e.g., config exists but selected provider has no API key, or `Config::load()` returns a parse error). On auto-launch, the wizard runs in **fix mode** — preserves existing valid sections and only re-prompts for the broken/missing portions. Distinct from explicit `hermes setup` which always runs the full minimum-viable flow.
- **D-06:** Validation source of truth is `Config::load()` plus a new `Config::validate()` method that returns a structured `Vec<ConfigValidationError>` per failed field. Wizard reads that to know which sections to repair.
- **D-07:** When wizard auto-launch completes, drop into the originally-requested command (`hermes` → chat; `hermes chat -q ...` → chat with query; `hermes gateway run` → gateway start). The wizard interruption is transparent.

### `hermes config set/get/show`

- **D-08:** `hermes config set` and `hermes config get` use **dotted-path syntax** for nested keys: `hermes config set model.default openrouter/qwen-2.5-coder-32b`, `hermes config get gateway.telegram.allowed_chats`. Mirrors git/npm/cargo conventions and maps directly to YAML structure. `hermes config get` returns the raw value (no JSON wrapping unless `--json` is passed in a future polish phase).
- **D-09:** `hermes config show` prints the full active config in YAML form, with secrets **masked using prefix preservation**: API keys shown as `sk-abc***` (first 4–6 chars + asterisks), bot tokens / OAuth tokens similarly masked. Helps user verify the right key is loaded without revealing the full secret. The list of "secret" fields is determined by a `secret: bool` flag added to `ConfigField` (Phase 20's schema). `.env`-stored secrets are never inlined into `hermes config show` output — they're surfaced via `hermes config env-path` (path only) which is a separate subcommand.
- **D-10:** Cache-breaker behavior on `hermes config set`: when a user changes a field tagged as `cache_breaking: bool` in the ConfigField schema (e.g., `model.default`, `model.base_url`, `agent.system_prompt`, `memory.provider`, context-file paths), the command **warns and persists**:
  ```
  ⚠ Changing model.default invalidates the prompt cache. Active sessions will pay full cache-miss cost on next turn.
  Persisted: model.default = openrouter/qwen-2.5-coder-32b
  ```
  The change always lands; the user is informed. PRMT-06's frozen-at-session-start property already prevents mid-session mutation of the active prompt — the warning is informational, not blocking. Aligns with v2.1 architectural principle #2 (cache-awareness is load-bearing).

### `hermes config migrate`

- **D-11:** `hermes config migrate` is **manual-only** — runs only when the user explicitly invokes it. Scans installed skills' `requires_config` / `requires_env` frontmatter (`crates/ironhermes-core/src/skills.rs:750` is the pre-coordination point), finds gaps relative to the live `config.yaml` and `.env`, and prompts the user to fill them. No auto-trigger on `hermes skills install`, no auto-trigger on `hermes` startup. Rationale: surprise behavior is a UX hazard; users discover gaps either via skill failure messages or by running `hermes doctor`. Phase 23 publishes the `migrate` command; downstream phases (Phase 28 skills trust tiers, Phase 30 ACP) MAY add hooks if they prove necessary.

### Cross-Crate Type Pattern (carry-forward from Phase 22.4.2.2)

- **D-12:** Any new types in `ironhermes-core::config` that cross crate boundaries use **plain Strings, not embedded downstream enums**. Example: a setup-wizard `WizardSection` enum, if introduced, lives in `ironhermes-core`; consumers (CLI subcommand dispatch, gateway setup hooks) construct their own enums at the call site. Avoids the circular-crate-dep problem documented in PROJECT.md Key Decisions (Phase 22.4.2.2 D-decision).

### Cache-Breaker Field Inventory

- **D-13:** Phase 23 adds a `cache_breaking: bool` field to `ConfigField` and tags the following fields as cache-breaking:
  - `model.default` (model switch breaks cache)
  - `model.base_url` (provider switch breaks cache)
  - `model.api_key` (only when key changes provider routing — annotate carefully)
  - `agent.system_prompt` (prompt structure change)
  - `agent.personality` (slot 1 of 10-layer prompt)
  - `memory.provider` (changes which memory tool is registered)
  - Any path field for SOUL.md / AGENTS.md / context files (changes 10-layer prompt source)
  Implementation tagged in Phase 23 plans; Phase 27 (Prompt Caching) MAY refine the list once the system_and_3 cache strategy lands.

### Claude's Discretion

- Wizard question phrasing, exact validation error messages, and the order within each section's question list — Claude can pick reasonable phrasing.
- Whether `hermes config show --section <X>` (filter to one section) lands in v2.1 or is deferred — small ergonomic add-on; planner can include if it fits the plan budget.
- Whether `hermes config get` returns YAML, raw scalar, or both — planner picks; default raw scalar is simplest.

### Folded Todos

- **`2026-04-17-configuration-setup-wizard-improvements.md`** (todo from .planning/todos/pending/, score 0.9) — exact match to phase scope. Folded into the decisions above; the todo's "Solution" sketch ("`ironhermes setup` interactive wizard. Add `ironhermes config set/get/show` subcommands. Add `ironhermes config migrate` for skills settings discovery. Implement profile isolation with per-profile HERMES_HOME directories.") is reflected in D-01..D-11 with profile isolation (CFG-04) split out to Phase 24. Tag todo with `resolves_phase: 23` (and `resolves_phase: 24` for the CFG-04 portion — duplicate todo OR move CFG-04 portion to a separate todo at next session).

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Phase scope and milestone context
- `.planning/ROADMAP.md` §"Phase 23: Configuration CLI and Setup Wizard" — phase goal, requirements (CFG-01, CFG-02, CFG-03), success criteria
- `.planning/REQUIREMENTS.md` lines 167-169 (CFG-01..03 body) and traceability table — requirement text and v2.1 phase assignment
- `.planning/PROJECT.md` §"Current Milestone: v2.1 Carry-Overs + Learning Loop" — milestone goal and 7 architectural principles that must be honored
- `.planning/v2.0-MILESTONE-AUDIT.md` — origin context for why CFG-01..04 are v2.1 carry-overs (Phase 23 was originally numbered Phase 23 in v2.0 but never executed)

### Existing config infrastructure (must integrate, not replace)
- `crates/ironhermes-core/src/config.rs` — 25+ section structs (`Config`, `ProviderConfig`, `ModelConfig`, `MemoryConfig`, `GatewayConfig`, `TerminalConfig`, `WebConfig`, `CronConfig`, `SecurityConfig`, `HubConfig`, `SkillsConfig`, `ExecConfig`, `SubagentConfig`, `BatchConfig`, `OriginDecision`, etc.)
- `crates/ironhermes-core/src/config_schema.rs` — `ConfigField` schema introduced in Phase 20. Phase 23 adds `secret: bool` and `cache_breaking: bool` fields here.
- `crates/ironhermes-core/src/commands/handlers.rs:50,656-658` — existing `cmd_config` stub. Phase 23 fleshes out this handler (and adds `cmd_setup`, `cmd_config_set`, `cmd_config_get`, `cmd_config_show`, `cmd_config_migrate` siblings).
- `crates/ironhermes-core/src/skills.rs:750` — pre-coordination comment: "Phase 23's `hermes config migrate` CLI consumes this to seed". Read this site to understand what skills surface for migrate to consume.
- `crates/ironhermes-cli/src/main.rs` — CLI entry point. Phase 23 adds `Setup { section: Option<String> }` and expanded `Config { subcommand: ConfigSubcommand }` variants to the `Commands` enum.

### Cross-crate type pattern (must follow)
- `.planning/phases/22.4.2.2-cron-create-defaults-to-tg-origin-when-gateway-active/22.4.2.2-CONTEXT.md` — `OriginDecision` enum precedent for plain-String cross-crate types
- `.planning/PROJECT.md` Key Decisions table, "Cross-crate transport types use plain Strings (no embedded downstream types)" row

### Rustyline integration pattern (reuse for wizard prompts)
- `crates/ironhermes-cli/src/repl_input.rs` — rustyline 15 wiring including `set_history_ignore_dups(true)`, `set_max_history_size(1000)`, history persistence to `$HERMES_HOME/repl_history`. Wizard uses bare rustyline (no history persistence — wizard answers should not bleed into chat history).
- `.planning/phases/22.3-repl-ux-hardening-visual-stability-reset-unified-history/22.3-CONTEXT.md` §D-08 — rustyline 15 API correction (`set_history_ignore_dups`, NOT `set_history_duplicates`)

### Cache-awareness contract (D-10 + D-13 enforcement)
- `.planning/REQUIREMENTS.md` PRMT-06 — frozen-at-session-start memory snapshot; mid-session writes don't mutate active prompt. The basis for "warn and persist".
- Phase 27 (Prompt Caching, downstream) — owns the system_and_3 cache strategy that Phase 23's cache_breaking flags must align with. Phase 23 establishes the warning surface; Phase 27 may refine the field-tagging list.

### Skills integration (CFG-03 migrate consumer)
- `crates/ironhermes-core/src/skills.rs` (full file) — skill loading, frontmatter parsing, the line-750 pre-coordination point
- `.planning/REQUIREMENTS.md` SKILL-01..08 (validated v2.0 reqs) — agentskills.io frontmatter shape that migrate scans

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- **`Config::load()`** (`config.rs`): already used by 8+ call sites (cronjob_tool, web_read, gateway/runner, mcp/manager, etc.). Phase 23's `set/get/show` operations should NOT introduce a parallel load path — extend or wrap `Config::load()`.
- **`ConfigField` schema** (`config_schema.rs`, Phase 20): existing schema-aware introspection. Phase 23 adds `secret: bool` and `cache_breaking: bool` fields to enable show-redaction (D-09) and warn-on-set (D-10) behavior without per-callsite handcoding.
- **rustyline 15 history infra** (`repl_input.rs`, Phase 22.3): wizard prompts reuse the rustyline editor. Wizard creates a fresh editor without history persistence (so wizard inputs don't bleed into chat history); chat session keeps its own editor with persistence.
- **`cmd_config` stub** (`handlers.rs:656`): existing CLI entry point that returns a one-line message. Phase 23 fleshes this out into a real subcommand dispatcher.
- **`OriginDecision` precedent** (Phase 22.4.2.2): pattern for new cross-crate types. Phase 23 mints any new wizard/config dispatch types in `ironhermes-core` with plain-String fields.

### Established Patterns
- **YAML at `~/.ironhermes/config.yaml` + `.env` for secrets** (PROJECT.md Constraint): Phase 23 keeps this split — `hermes config show` reads both, masks .env values, displays YAML inline; `hermes config env-path` returns the .env path only.
- **`gsd-sdk query state.*`-style command dispatch**: every CLI subcommand routes through a uniform handler signature. Phase 23 adds setup + config subcommands following the same pattern.
- **Config validation via `Config::load()` Result**: load returns `Result<Config, ConfigError>`. Phase 23 introduces `Config::validate(&self) -> Vec<ConfigValidationError>` to enable fix-mode wizard auto-launch (D-05).

### Integration Points
- **CLI entry point (`main.rs`)**: extend `Commands` enum with `Setup { section: Option<String> }` and expanded `Config { subcommand: ConfigSubcommand }` variants. Pre-flight check (D-05) runs in the bare `hermes` and `hermes chat` arms before normal startup.
- **Gateway**: `hermes gateway setup` already exists conceptually as a separate path; Phase 23's `hermes setup gateway` may delegate to or replace it. Verify against `crates/ironhermes-gateway/src/runner.rs:1228` config-load site.
- **MCP manager** (`mcp/manager.rs:347-354`): consumes fresh `Config::load()` on `/reload-mcp`. Phase 23's `config set` mutations are visible after `mcp_servers` is re-read — no Phase 23 wiring needed, but document the timing.
- **Skills system** (`skills.rs:750`): the pre-coordination point. `hermes config migrate` reads installed skills' frontmatter, diffs `requires_config` / `requires_env` against the live config, and prompts to fill gaps.

</code_context>

<specifics>
## Specific Ideas

- **Wizard `hermes setup` (no section) flow:** prompts in order — (1) Provider [OpenRouter]: , (2) API key: , (3) Default model [openrouter/qwen-2.5-coder-32b]: , (4) "Configure additional sections now? [y/N]" — if yes, dispatches to `hermes setup memory` etc. in turn; if no, exits to next user command (chat or whatever was originally requested).
- **`hermes config show` redaction example:**
  ```yaml
  model:
    default: openrouter/qwen-2.5-coder-32b
    api_key: sk-abc1***
    base_url: https://openrouter.ai/api/v1
  gateway:
    telegram:
      bot_token: 1234***
  ```
  First 4–6 chars preserved, rest masked with `***`.
- **`hermes config migrate` UX:** scans skills, prints a table of gaps, prompts per-gap with optional "skip" and "skip all" affordances. Resolves both `.env` keys and `config.yaml` keys.
- **`hermes config path` and `hermes config env-path`:** simple subcommands returning paths only — useful for shell scripting and editor integration. No new infrastructure.

</specifics>

<deferred>
## Deferred Ideas

These came up during discussion but belong elsewhere — not lost, just routed.

- **`hermes setup agent`** — agent-loop config (max_turns, tool_use_enforcement, BudgetHandle thresholds for PROV-09/10). Deferred to **Phase 26 (Provider Polish)** since PROV-04/06/08 modify the relevant config schemas.
- **`hermes setup skills`** — skill hub config (extra_taps, default trust tier policy). Deferred to **Phase 28 (Skills Trust Tiers)** since SKILL-09 introduces the trust-tier discrimination this section would need.
- **`hermes setup voice` / STT / TTS sections** — not in v2.1 scope. Voice is a GAP-NEW item (VOICE-01..N) parked in Future Requirements.
- **`hermes config edit`** (open `$EDITOR`) — useful but not required. Planner can include if budget allows; otherwise v2.2 polish.
- **`hermes config show --json` output format** — JSON-formatted view. Defer to v2.2 polish unless planner sees a free-rider opportunity.
- **`hermes config show --section <X>` filter** — Claude's Discretion: planner picks if it fits.
- **Auto-trigger `config migrate` on `hermes skills install`** — D-11 explicitly chose manual-only. Reconsider in Phase 28 if SKILL-09 trust-tier work surfaces a user-experience gap.
- **`hermes doctor --fix` integration** — `hermes doctor` is documented in v2.0; the `--fix` flag is deferred to v2.2 (Production Polish reservation). Phase 23's auto-launch fix-mode is a partial overlap but specifically scoped to first-run + config validation, not general health checks.

### Reviewed Todos (not folded)

- `2026-04-17-cli-feature-parity.md` — already validated by v2.0 Phase 22; not relevant to Phase 23.
- `2026-04-17-slash-command-integration-skill-13.md` — already validated by v2.0 Phase 21.1; not relevant to Phase 23.
- `2026-04-18-skill-registry-name-validation-rejects-title-case-skills.md` — defect in v2.0 skill registry; should be filed as a v2.0 bugfix or rolled into Phase 28 if scope-matches.

</deferred>

---

*Phase: 23-configuration-cli-and-setup-wizard*
*Context gathered: 2026-04-27*
