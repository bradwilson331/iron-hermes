# Phase 14: Context Files & SOUL.md - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-12
**Phase:** 14-context-files-soul-md
**Areas discussed:** .hermes.md walk behavior, Subdirectory discovery, Priority chain edge cases, SOUL.md identity system

---

## .hermes.md Walk Behavior

| Option | Description | Selected |
|--------|-------------|----------|
| First match wins | Walk upward from CWD, load the FIRST .hermes.md found. Matches hermes-agent behavior. | ✓ |
| Merge all found | Walk upward, merge ALL .hermes.md files (closest highest priority). | |
| Git root only | Only check git root for .hermes.md, skip intermediate directories. | |

**User's choice:** First match wins
**Notes:** Matches hermes-agent behavior — closest context file wins, one file loaded.

| Option | Description | Selected |
|--------|-------------|----------|
| Strip before injection | Parse and remove YAML frontmatter before injecting content. Reserved for future config. | ✓ |
| Pass through as-is | Include frontmatter in system prompt. | |
| Parse and use | Parse fields for config overrides. | |

**User's choice:** Strip before injection
**Notes:** Per CTX-07, frontmatter reserved for future config overrides.

| Option | Description | Selected |
|--------|-------------|----------|
| Stop at filesystem root | Walk all the way to / if no git root. Cap at 10 dirs. | |
| CWD only fallback | If no git root, only check CWD. | |
| Stop at home dir | Walk up from CWD but stop at $HOME if no git root found. | ✓ |

**User's choice:** Stop at home dir
**Notes:** Prevents loading system-level context files.

---

## Subdirectory Discovery

| Option | Description | Selected |
|--------|-------------|----------|
| Inject into tool results | Append context file content to the triggering tool's result output. | ✓ |
| Add as system message | Insert new system message with discovered context. | |
| You decide | Let Claude choose injection mechanism. | |

**User's choice:** Inject into tool results
**Notes:** Matches hermes-agent pattern. Agent sees context naturally as part of tool output.

| Option | Description | Selected |
|--------|-------------|----------|
| File-access tools only | Only read_file, write_file, list_directory trigger discovery. | ✓ |
| Any tool with path argument | Broader coverage including non-file tools. | |
| Explicit navigate tool | Dedicated cd/navigate tool for context changes. | |

**User's choice:** File-access tools only
**Notes:** Focused and predictable trigger set.

| Option | Description | Selected |
|--------|-------------|----------|
| Upward from accessed file | Check up to 5 parent directories from accessed file location. | ✓ |
| Between CWD and accessed file | Only check directories between CWD and accessed file path. | |
| You decide | Let Claude choose traversal strategy. | |

**User's choice:** Upward from accessed file
**Notes:** Discovers context closest to the work.

| Option | Description | Selected |
|--------|-------------|----------|
| Full priority chain | Check .hermes.md > AGENTS.md > CLAUDE.md > .cursorrules in subdirectories. | ✓ |
| .hermes.md only | Subdirectory discovery only checks for .hermes.md. | |
| You decide | Let Claude choose based on hermes-agent behavior. | |

**User's choice:** Full priority chain
**Notes:** Consistent behavior, picks up CLAUDE.md files in monorepo subprojects.

---

## Priority Chain Edge Cases

| Option | Description | Selected |
|--------|-------------|----------|
| Both load, separate roles | HERMES_HOME/AGENTS.md = global config. CWD/AGENTS.md = project context (priority chain). Both injected. | ✓ |
| CWD wins, skip HERMES_HOME | If CWD AGENTS.md found, don't load HERMES_HOME version. | |
| HERMES_HOME wins, skip CWD | Always load from HERMES_HOME only. | |

**User's choice:** Both load, separate roles
**Notes:** Two different purposes — global agent config vs project context.

| Option | Description | Selected |
|--------|-------------|----------|
| Case-sensitive | Only exact names match: .hermes.md, AGENTS.md, CLAUDE.md, .cursorrules. | ✓ |
| Case-insensitive | Match regardless of case. | |
| Match both cases explicitly | Check known variants like CLAUDE.md and claude.md. | |

**User's choice:** Case-sensitive
**Notes:** Drop current lowercase variants from candidate list.

---

## SOUL.md Identity System

| Option | Description | Selected |
|--------|-------------|----------|
| Skip SOUL.md, use default | When skip_context_files set, use DEFAULT_AGENT_IDENTITY. Skip all context. | ✓ |
| SOUL.md always loads | SOUL.md loads regardless. Only project context skipped. | |
| You decide | Let Claude choose based on hermes-agent behavior. | |

**User's choice:** Skip SOUL.md, use default
**Notes:** Subagents get a clean, focused identity.

| Option | Description | Selected |
|--------|-------------|----------|
| Raw injection | SOUL.md injected as-is (after scan+truncate) as first prompt layer. No header. | ✓ |
| Wrap with header | Add ## SOUL.md or ## Identity header before content. | |

**User's choice:** Raw injection
**Notes:** SOUL.md IS the identity — no wrapper needed. Matches current behavior.

| Option | Description | Selected |
|--------|-------------|----------|
| Hardcoded constant | Keep DEFAULT_AGENT_IDENTITY as const &str in prompt_builder.rs. | ✓ |
| Bundled via include_str! | Embed a default_soul.md file at compile time. | |
| You decide | Let Claude choose. | |

**User's choice:** Hardcoded constant
**Notes:** Simple, no I/O, always available. Current approach preserved.

---

## Claude's Discretion

- Visited-dirs set implementation (HashSet<PathBuf> on session, or shared Arc structure)
- File-access tool discovery trigger mechanism (interceptor vs per-tool check)
- YAML frontmatter parsing approach
- Subdirectory context injection formatting within tool results
- Whether to add ContextLoader struct or extend PromptBuilder
- Git root detection method

## Deferred Ideas

None — discussion stayed within phase scope.
