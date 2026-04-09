# Phase 7: Skills System - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-09
**Phase:** 07-skills-system
**Areas discussed:** Skill discovery & directory structure, Catalog format in system prompt, Skills tool API design, Cron-skill wiring

---

## Skill Discovery & Directory Structure

### Directory Layout

| Option | Description | Selected |
|--------|-------------|----------|
| Flat | ~/.ironhermes/skills/{skill-name}/SKILL.md — single level, categories as tags | ✓ |
| Nested categories | {category}/{skill-name}/SKILL.md with DESCRIPTION.md per category | |
| Both flat and nested | Recursive scanner finds SKILL.md at any depth | |

**User's choice:** Flat layout
**Notes:** Simpler than Python's nested categories. Categories expressed via frontmatter tags.

### Scan Paths

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, both paths | ~/.ironhermes/skills/ AND ~/.agents/skills/ per SKILL-01 | ✓ |
| Only ~/.ironhermes/skills/ | Single canonical path | |
| Configurable paths | Default to both, allow extra paths in config | |

**User's choice:** Both standard paths
**Notes:** Interop with agentskills.io ecosystem. ~/.ironhermes/ takes precedence on name conflicts.

### Project-Level Skills

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, .ironhermes/skills/ in cwd | Project-specific skills, override global on conflict | ✓ |
| No, global only | Skills are global to agent instance | |
| You decide | Claude's discretion | |

**User's choice:** Yes, project-level discovery
**Notes:** Matches how SOUL.md/AGENTS.md already load from project directory.

---

## Catalog Format in System Prompt

### Catalog Style

| Option | Description | Selected |
|--------|-------------|----------|
| Compact list | One line per skill: "- {name}: {description}" | ✓ |
| Categorized list | Group skills by category tag | |
| Name-only list | Just skill names, no descriptions | |

**User's choice:** Compact list
**Notes:** Minimal token usage, agent sees what's available at a glance.

### Usage Hint

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, brief instruction | One line: "Use the skills tool to view or activate a skill" | ✓ |
| No hint | Agent discovers from tool list | |
| You decide | Claude's discretion | |

**User's choice:** Yes, brief instruction

---

## Skills Tool API Design

### Tool Structure

| Option | Description | Selected |
|--------|-------------|----------|
| Single tool, action param | One `skills` tool with list/view/activate actions | ✓ |
| Separate tools | skills_list, skills_view, skills_activate | |
| You decide | Claude's discretion | |

**User's choice:** Single tool with action parameter
**Notes:** Matches cronjob tool pattern. Fewer tools in LLM schema.

### Activate Behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Return full SKILL.md content | Returns markdown body as tool result | ✓ |
| Inject into system prompt | Appends to system prompt for session remainder | |
| Return + mark as active | Return content AND track activation state | |

**User's choice:** Return full content
**Notes:** Simple, no magic. Agent receives it like any tool output.

---

## Cron-Skill Wiring

### Tick-Time Skill Resolution

| Option | Description | Selected |
|--------|-------------|----------|
| Prepend to cron prompt | Resolve skills, prepend content to agent_input | ✓ |
| Inject into system prompt | Add to system prompt when spawning cron session | |
| You decide | Claude's discretion | |

**User's choice:** Prepend to cron prompt

### Missing Skill Handling

| Option | Description | Selected |
|--------|-------------|----------|
| Warn and continue | Log warning, skip missing skill, run job anyway | ✓ |
| Fail the job | Return error, user must fix skill list | |
| You decide | Claude's discretion | |

**User's choice:** Warn and continue
**Notes:** Jobs shouldn't break because a skill was removed.

---

## Claude's Discretion

- Internal SkillRegistry data structure and caching strategy
- SKILL.md YAML parsing approach
- How SkillRegistry is shared across components
- Whether to fire hook events on skill activation
- Error message formatting for the skills tool

## Deferred Ideas

None — discussion stayed within phase scope.
