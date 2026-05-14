<!-- generated-by: gsd-doc-writer -->
# Skills System — Internals and Authoring Guide

This document covers the skills system architecture: how skills are discovered and loaded by the agent, the SKILL.md file format and frontmatter schema, how to write a custom skill, and how skills differ from tools. For CLI commands (`ironhermes skills install`, `search`, `list`, etc.) see [docs/skills-cli.md](skills-cli.md).

---

## Overview

Skills are Markdown documents that inject reusable knowledge, workflows, and domain context into the agent's system prompt. When a skill is active, the agent gains awareness of specialized procedures — multi-step workflows, protocol references, domain rules — without requiring those instructions to be repeated in every conversation.

Skills differ from tools in a fundamental way:

| | Skills | Tools |
|--|--------|-------|
| What they are | Markdown knowledge documents | Executable Rust/Python functions |
| How they work | Injected into the system prompt | Registered in the tool-call schema |
| What they do | Guide the agent's reasoning and behavior | Perform concrete actions (web search, file I/O, TCP calls) |
| Who writes them | Anyone with a text editor | Developers who extend the Rust or Python codebase |
| Requires code changes | No | Yes |

A skill cannot call a tool, but a skill can document how to call tools — the agent reads the skill body and then uses its tools accordingly.

---

## Directory Structure

The bundled skills library lives at `skills/` in the project root. It is organized into category directories, each containing individual skill subdirectories:

```
skills/
├── <category>/               # Category directory (contains DESCRIPTION.md)
│   ├── DESCRIPTION.md        # One-line category description (optional)
│   └── <skill-name>/
│       ├── SKILL.md          # Required: skill definition file
│       ├── references/       # Optional: reference documents the skill loads
│       ├── scripts/          # Optional: helper scripts the agent can invoke
│       └── templates/        # Optional: output templates referenced in the skill body
├── index-cache/              # Cached remote skill index files (not agent-loaded)
└── <flat-skill>/             # Flat layout: skill at root level (legacy)
    └── SKILL.md
```

### Categories

The following top-level categories are present in the bundled library:

| Category | Description |
|----------|-------------|
| `apple` | Apple/macOS-specific skills — iMessage, Reminders, Notes, FindMy, macOS automation |
| `autonomous-ai-agents` | Spawning and orchestrating autonomous AI coding agents and multi-agent workflows |
| `creative` | Creative writing, storytelling, and content generation |
| `data-science` | Data science workflows — interactive exploration, Jupyter notebooks, data analysis |
| `devops` | DevOps skills — webhooks, infrastructure automation |
| `diagramming` | Diagram creation and visualization |
| `dogfood` | Systematic exploratory QA testing of web applications |
| `domain` | Domain registration and DNS management |
| `email` | Email workflows and automation |
| `feeds` | RSS/Atom feed reading and monitoring |
| `gaming` | Gaming and game-server skills |
| `gifs` | GIF search and generation |
| `github` | GitHub workflow automation |
| `hermes-agent` | Complete guide to using and extending Hermes Agent |
| `hexapod` | Protocol reference for the Freenove hexapod robot |
| `inference-sh` | Remote inference and model hosting via inference.sh |
| `leisure` | Leisure and entertainment skills |
| `mcp` | MCP server setup and tool integration |
| `media` | Media skills — YouTube, audio processing |
| `mlops` | Machine learning operations and model deployment |
| `note-taking` | Note-taking and knowledge capture workflows |
| `productivity` | Productivity tools — Linear, Notion, Google Workspace, PowerPoint, OCR |
| `red-teaming` | Adversarial testing and red-team workflows |
| `research` | Research workflows — arXiv, Polymarket, blog watching, paper writing |
| `software-development` | Software development practices — TDD, code review, planning, debugging |
| `social-media` | Social media automation |
| `smart-home` | Home automation skills |

---

## SKILL.md File Format

Every skill is defined by a `SKILL.md` file with a YAML frontmatter block followed by a Markdown body.

### Structure

```
---
<YAML frontmatter>
---

<Markdown body>
```

The file must begin with `---` on the first line. The frontmatter block is closed by a second `---` line. Everything after the closing `---` is the skill body — plain Markdown that the agent reads.

### Frontmatter Schema

```yaml
---
# Required fields
name: my-skill-name          # Lowercase alphanumeric + hyphens, 1–64 chars
description: One sentence describing what this skill does and when to load it.  # 1–1024 chars

# Optional fields
version: 1.0.0               # Semantic version string
author: Author Name
license: MIT

# Platform filter — omit to load on all platforms
platforms:
  - macos                    # Valid values: macos, linux, windows (exact — no aliases)
  - linux

# agentskills.io compatibility fields (parsed, not enforced)
compatibility: "Requires browser toolset"
allowed-tools:               # Advisory pre-approved tool list
  - browser_navigate
  - browser_snapshot

# IronHermes-specific metadata
metadata:
  hermes:
    tags: [tag1, tag2]
    related_skills: [other-skill-name]

    # Catalog-render filter — controls when this skill appears in the agent's skill list
    requires_toolsets:        # Show only when ALL listed toolsets are enabled
      - browser
    requires_tools:           # Show only when ALL listed tools are active
      - my_custom_tool
    fallback_for_toolsets:    # Hide when ANY listed toolset is active (fallback behavior)
      - native-browser
    fallback_for_tools:       # Hide when ANY listed tool is active
      - preferred_tool

    # Environment variable declarations
    required_environment_variables:
      - name: MY_API_KEY
        prompt: "Enter your API key"
        help: "Obtain from https://example.com/settings"
        required_for: "API calls"

    # Credential file declarations
    required_credential_files:
      - path: credentials.json
        description: "OAuth credentials from Google Cloud Console"

    # Config schema (consumed by `ironhermes config migrate`)
    config:
      - key: timeout_seconds
        default: 30
        description: "Request timeout in seconds"
        type: integer
---
```

### Name Validation Rules

Skill names are validated against the agentskills.io specification:

- Length: 1 to 64 characters
- Characters: lowercase alphanumeric (`a-z`, `0-9`) and hyphens only
- No leading or trailing hyphens
- No consecutive hyphens (`--`)
- Pattern: `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`

Names that contain uppercase letters or spaces are **normalized at load time** — converted to lowercase with spaces replaced by hyphens. The on-disk directory name is never changed; normalization is in-memory only. A warning is logged when normalization changes the name.

Names that fail validation after normalization cause the skill to be silently skipped.

### Description Validation

Descriptions must be 1–1024 characters. Out-of-range descriptions produce a warning but the skill still loads (warn-but-load behavior).

---

## How Skills Are Discovered and Loaded

### Search Paths

The `SkillRegistry` scans the following directories in priority order:

1. `{cwd}/.ironhermes/skills/` — project-local skills
2. `~/.ironhermes/skills/` (or `$IRONHERMES_HOME/skills/`) — user skills
3. `~/.agents/skills/` — shared agent skills directory
4. Any paths listed in `config.skills.extra_paths` — appended after defaults

First-path-wins: if the same skill name (case-insensitive) appears in multiple directories, only the first occurrence is loaded. This lets project-local skills override user-level skills.

### Scan Depth

The registry scans at two levels:

- **Level 1:** `<search_path>/<dir>/SKILL.md` — the legacy flat layout
- **Level 2:** `<search_path>/<dir>/<subdir>/SKILL.md` — the Phase 21.8 installer layout used by `ironhermes skills install`

Scanning never descends beyond two levels. Hidden directories (names starting with `.`) and the `.hub` state directory are never traversed. `node_modules` is skipped at level 2.

### Load Pipeline

For each `SKILL.md` found:

1. **Read** the file from disk.
2. **Parse** the YAML frontmatter with `parse_skill_md`. Skip the file if frontmatter is missing or invalid.
3. **Normalize** the skill name (Title Case / spaces → kebab-case). Log a warning if the name changed.
4. **Validate** the name against the name rules. Reject and skip if invalid.
5. **Security scan** the frontmatter text and body combined. Community-sourced skills that trigger a scan hit are hard-rejected. Builtin/Official/Trusted skills log a warning but still load.
6. **Platform filter** — if `platforms` is specified and does not include the current OS, skip.
7. **Deduplication** — if the name (lowercase) has already been seen in a higher-priority path, skip.
8. **Extract hermes metadata** from the opaque `metadata` blob into typed `HermesMetadata`. Unknown fields inside `metadata.hermes.*` are preserved in `extras` for forward compatibility.
9. **Add to registry** with source label `Builtin` (default for locally-discovered skills).

After all paths are scanned, trust labels are recomputed by consulting `.hub/lock.json` and `config.hub.trusted_repos`. Community skills that now fail a re-scan are removed from the registry.

### Platform Filtering

The `platforms` field restricts which operating systems load a skill. Valid values are `macos`, `linux`, and `windows` — exact strings, no aliases (`darwin`, `osx`, `win32` are not recognized and produce a non-match). Skills that list only unrecognized platform strings are filtered out on every OS.

Omitting `platforms` (or providing an empty list) means the skill loads on all platforms.

### Kill Switch

Setting `config.skills.enabled = false` disables the entire skill system. `SkillRegistry::load_with_config` returns an empty registry immediately without scanning the filesystem.

---

## Catalog-Render Filter

The catalog-render filter controls which skills appear in the agent's `## Available Skills` system prompt section. It runs at session-freeze time and is a pure in-memory operation — no filesystem or environment access.

### Filter Rules

Skills without any `metadata.hermes` block are always shown.

For skills with hermes metadata, the following rules apply in order:

| Metadata field | Rule |
|---------------|------|
| `requires_toolsets` | Skill is hidden unless **all** listed toolsets are active |
| `requires_tools` | Skill is hidden unless **all** listed tools are active |
| `fallback_for_toolsets` | Skill is hidden if **any** listed toolset is active |
| `fallback_for_tools` | Skill is hidden if **any** listed tool is active |

The active toolset/tool snapshot is captured at session start from the merged tool configuration and does not change mid-session.

**Example:** The `hexapod` skill declares `requires_toolsets: [robotics]`. If the `robotics` toolset is disabled, the skill does not appear in the catalog. When the user enables the `robotics` toolset and starts a new session, the skill appears.

---

## Prompt Slot Placement

Skills occupy **slot 5** in the 10-layer system prompt assembly model:

| Slot | Name | Content |
|------|------|---------|
| 1 | Identity | SOUL.md or default agent identity |
| 2 | SystemMessage | `config.agent.system_message` (if set) |
| 3 | ToolGuidance | Tool use instructions, model/provider context |
| 4 | Memory | Frozen memory snapshot |
| **5** | **Skills** | **`## Available Skills` catalog** |
| 6 | ContextFiles | `.hermes.md`, AGENTS.md, project context |
| 7 | Timestamp | Current time, turn number, session ID |
| 8 | PlatformHints | CLI/Telegram/Discord/Slack platform notes |
| 9 | SessionOverlay | Active personality overlay |
| 10 | UserMessage | Current user turn content |

Slots 1–6 are durable (stable across turns, Anthropic prompt cache hits them). Slots 7–10 are ephemeral (regenerated per turn).

The catalog text format is:

```
## Available Skills

- skill-name: One-line description
- other-skill: Another description

Use the skills tool to view or activate a skill before using it.
```

### Skill Activation (Mid-Session)

When the user runs `/skill <name>` in an active session, the skill body is injected into the **ephemeral** portion of the prompt as an activated skill overlay:

```
# Activated Skill: <name>

<body content>
```

Multiple skills can be activated in the same session. Overlays accumulate in activation order. `/clear` removes all activated skill overlays (`clear_skill_overlays()`).

---

## Source Trust Levels

Every skill record carries a provenance label that determines how security scan results are treated:

| Source | Assigned when | Scan hit behavior |
|--------|--------------|-------------------|
| `Builtin` | Locally-discovered skill with no hub manifest entry | Warn-but-load |
| `Official` | Skill path contains an `optional-skills/` component | Warn-but-load |
| `Trusted` | Hub-installed from a GitHub repo listed in `config.hub.trusted_repos` | Warn-but-load |
| `Community` | Hub-installed from well-known catalog, skills.sh, or untrusted GitHub repo | **Hard-reject** |

The bundled `skills/` directory in this repository always loads as `Builtin`. Skills installed via `ironhermes skills install` start as `Community` unless their source repository is on the `hub.trusted_repos` allowlist.

---

## Writing a Custom Skill

### Minimal Example

```
skills/
└── my-skills/
    └── my-workflow/
        └── SKILL.md
```

`SKILL.md`:

```markdown
---
name: my-workflow
description: Step-by-step guide for performing my-workflow tasks efficiently.
version: 1.0.0
author: Your Name
---

# My Workflow

## Overview

Brief description of what this skill covers and when to use it.

## Steps

1. First, do this:
   ```
   terminal(command="echo hello")
   ```

2. Then do this...

## Tips

- Use X when Y happens.
- Avoid Z because it causes W.
```

Save the file at `$IRONHERMES_HOME/skills/my-skills/my-workflow/SKILL.md` (user-level) or `{project}/.ironhermes/skills/my-skills/my-workflow/SKILL.md` (project-level). Restart the agent or start a new session to load it.

### Platform-Specific Skill

To create a skill that only loads on macOS:

```yaml
---
name: apple-reminders-sync
description: Sync tasks with Apple Reminders via osascript.
platforms:
  - macos
---
```

### Toolset-Gated Skill

To show a skill only when the `browser` toolset is active:

```yaml
---
name: web-scraper
description: Systematic multi-page web scraping workflow with browser toolset.
metadata:
  hermes:
    requires_toolsets: [browser]
---
```

### Skill with Reference Files

For complex skills, companion files can live alongside `SKILL.md` and be referenced in the body:

```
skills/productivity/my-tool/
├── SKILL.md
├── references/
│   └── api-reference.md     # Loaded by agent when needed
└── scripts/
    └── helper.py            # Script the agent can run via terminal tool
```

Reference the companion files in the skill body so the agent knows to read them:

```markdown
## API Reference

For the full API reference, read `references/api-reference.md` in this skill's directory.
```

### Naming Conventions

- Use lowercase kebab-case: `my-skill-name`
- Name the skill after what it does, not after a technology: `web-scraping` not `playwright`
- Keep the description to one sentence: what the skill covers and when to activate it
- Match the directory name to the `name:` frontmatter field

---

## Skills vs Tools — When to Use Each

**Write a skill when:**
- You want the agent to follow a specific procedure or workflow
- You have domain knowledge (API protocols, business rules, style guides) to encode
- You want to reuse a complex multi-step process across sessions
- No code changes to the agent are needed

**Write a tool (in Rust or via the Python tool registry) when:**
- You need the agent to perform an action it cannot accomplish with existing tools
- You need to call an external API, read hardware, or execute a subprocess
- The capability requires returning structured data to the agent's reasoning loop
- You are a developer with access to the agent's codebase

Skills guide the agent's reasoning. Tools extend the agent's capabilities. The two compose naturally: a skill documents how to use a tool, and a tool implements what a skill describes.

---

## Security Considerations

All skill content (frontmatter and body combined) is passed through a content security scanner at load time via `scan_skill_content`. The scanner looks for prompt injection patterns and other threat indicators.

Additional sanitization rules enforced during install (not local discovery):

- File path traversal is rejected (`..`, NUL bytes, absolute paths, Windows drive prefixes)
- YAML-only frontmatter modes are blocked (`---js`, `---javascript`)
- Every line of terminal output that quotes skill names or server data passes through a terminal-escape stripper before display

Community-sourced skills (installed from the hub) are hard-rejected if any scan hit is detected. Locally-authored skills (Builtin) log a warning but still load.

---

## Source Pointers

| Concern | File |
|---------|------|
| `SkillRegistry`, `SkillRecord`, `SkillFrontmatter`, `HermesMetadata` | `crates/ironhermes-core/src/skills.rs` |
| Catalog-render filter (`skill_passes_filter`) | `crates/ironhermes-core/src/skills.rs` |
| Prompt slot assembly and skill catalog injection | `crates/ironhermes-agent/src/prompt_builder.rs` |
| CLI surface (`ironhermes skills install`, `search`, etc.) | `crates/ironhermes-cli/src/skills_cmd.rs` |
| Install pipeline (8-step) | `crates/ironhermes-hub/src/installer.rs` |
| Lock schema and folder hash | `crates/ironhermes-hub/src/lock.rs` |
| Bundled skills library | `skills/` |

---

## Related Documentation

- [skills-cli.md](skills-cli.md) — CLI reference for `ironhermes skills` commands
- [CONFIGURATION.md](CONFIGURATION.md) — `config.skills.*` configuration options
- [ARCHITECTURE.md](ARCHITECTURE.md) — System architecture and prompt assembly model
