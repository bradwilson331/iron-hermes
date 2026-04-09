# Phase 7: Skills System - Context

**Gathered:** 2026-04-09
**Status:** Ready for planning

<domain>
## Phase Boundary

Add a skill discovery, cataloging, and activation system to IronHermes so the agent can find skill documents at startup, present a compact catalog in the system prompt, and load full skill content on demand via a dedicated skills tool. Wire skill resolution into the cron tick runner so scheduled tasks can use attached skills.

</domain>

<decisions>
## Implementation Decisions

### Skill Discovery
- **D-01:** Flat directory layout — `{skills_dir}/{skill-name}/SKILL.md`. No nested category hierarchy. Categories are expressed as tags in YAML frontmatter instead of directory structure.
- **D-02:** Three scan paths in priority order: (1) `{cwd}/.ironhermes/skills/` (project-level, highest precedence), (2) `~/.ironhermes/skills/` (user global), (3) `~/.agents/skills/` (agentskills.io standard path). On name conflict, earlier path wins.
- **D-03:** Scanner walks each path looking for `SKILL.md` files. Each SKILL.md uses the agentskills.io format: YAML frontmatter with `name` and `description` (required), plus optional `version`, `author`, `license`, `metadata`. Markdown body contains the full skill content.

### Catalog Format
- **D-04:** Compact one-line-per-skill catalog injected into the system prompt at session start: `"- {name}: {description}"`. No tags, version, or categories in the prompt — minimal token usage.
- **D-05:** Include a brief usage hint after the catalog: `"Use the skills tool to view or activate a skill before using it."` Ensures the agent knows how to load full content.

### Skills Tool API
- **D-06:** Single `skills` tool with `action` parameter — actions: `list` (show catalog with descriptions), `view` (show full SKILL.md content without activation), `activate` (load full content and return it). Matches the cronjob tool's compressed action pattern.
- **D-07:** `activate` returns the full SKILL.md markdown body as the tool result. No system prompt mutation or session state tracking — the agent receives skill content like any tool output and follows the instructions.

### Cron-Skill Wiring
- **D-08:** At cron tick time, resolve each skill name in the job's `skills: Vec<String>` against the SkillRegistry. Read full SKILL.md content for each, prepend to the job's `agent_input` as context. Agent sees skill content before the task prompt.
- **D-09:** Missing skill names at tick time produce a tracing warning and are skipped. The job runs with whatever skills resolved successfully. Jobs should not break because a skill was removed.

### Claude's Discretion
- Internal SkillRegistry data structure and caching strategy
- SKILL.md YAML parsing approach (serde_yaml or manual)
- How SkillRegistry is shared across components (Arc pattern)
- Whether to fire hook events on skill activation (leveraging Phase 6 hooks)
- Error message formatting for the skills tool

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Python Reference Implementation
- `~/code/hermes-agent/tools/skills_hub.py` — SkillMeta/SkillBundle models, source adapters, hub state management
- `~/code/hermes-agent/skills/hermes-agent/SKILL.md` — Example skill document with full YAML frontmatter (name, description, version, author, license, metadata)
- `~/code/hermes-agent/skills/creative/ascii-art/SKILL.md` — Another example skill document

### Existing Rust Codebase
- `crates/ironhermes-tools/src/registry.rs` — Tool trait and ToolRegistry pattern (skills tool follows same pattern)
- `crates/ironhermes-tools/src/cronjob_tool.rs` — Single tool with action parameter pattern (skills tool mirrors this)
- `crates/ironhermes-core/src/config.rs` — Config struct where SkillsConfig may be added
- `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop where skill catalog is injected into system prompt
- `crates/ironhermes-cron/src/delivery.rs` — Cron tick execution where skill content is prepended to agent_input
- `crates/ironhermes-hooks/src/registry.rs` — HookRegistry for optional skill activation events

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph. SkillRegistry in ironhermes-core, SkillsTool in ironhermes-tools (no new crate deps per prior decision)

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `Tool` trait in `ironhermes-tools/src/registry.rs` — SkillsTool implements this with name/description/schema/execute
- `CronjobTool` in `ironhermes-tools/src/cronjob_tool.rs` — action-based tool pattern to mirror for SkillsTool
- `get_hermes_home()` in `ironhermes-core/src/constants.rs` — resolves `~/.ironhermes/` path for skill directory
- `HookRegistry.fire()` — can emit skill activation events (HookEventKind extension)

### Established Patterns
- Action-based tool with JSON args parsed via serde_json (cronjob_tool pattern)
- Arc<Mutex<T>> for shared mutable state (MemoryStore, JobStore patterns)
- YAML frontmatter parsing — will need `serde_yaml` or similar (new dep for ironhermes-core)
- System prompt assembly in AgentLoop — context files loaded at session start, skill catalog follows same pattern

### Integration Points
- `AgentLoop::new()` or `AgentLoop::run()` — inject skill catalog into system messages
- `ToolRegistry::register()` — register SkillsTool with Arc<SkillRegistry>
- `CronJob.skills: Vec<String>` — already stores skill name references, needs SkillRegistry at tick time
- `ironhermes-cron` tick runner — needs access to SkillRegistry to resolve skill content

</code_context>

<specifics>
## Specific Ideas

- Follow the agentskills.io SKILL.md format exactly — YAML frontmatter with `name` and `description` required, markdown body
- SkillRegistry should be cheap to share (Arc) and loaded once at startup, not per-request
- The compact catalog format matches how Claude Code presents its skills — one line per skill with name and description

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" — tooling/DX improvement, not directly related to skills system. Belongs in a future DX phase.

</deferred>

---

*Phase: 07-skills-system*
*Context gathered: 2026-04-09*
