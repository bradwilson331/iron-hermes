# Skills System Research: IronHermes v1.1

**Researched:** 2026-04-08
**Confidence:** HIGH (official spec fetched from agentskills.io, Python hermes-agent docs fetched, codebase read directly)

---

## 1. The agentskills.io Open Standard

### What It Is

Agent Skills is an open standard originally developed by Anthropic, published at agentskills.io. It defines a portable skill format that works across 30+ agent products (Claude Code, GitHub Copilot, VS Code, Cursor, OpenAI Codex, Gemini CLI, OpenHands, etc.). The spec is minimal by design — it only defines what lives inside a skill directory, not where skill directories live.

### SKILL.md File Format

Every skill is a **directory** containing at minimum a `SKILL.md` file. The file uses YAML frontmatter followed by Markdown body content.

**Frontmatter fields:**

| Field | Required | Constraints |
|-------|----------|-------------|
| `name` | Yes | 1-64 chars, lowercase alphanumeric + hyphens only, no leading/trailing/consecutive hyphens, must match parent directory name |
| `description` | Yes | 1-1024 chars. Must describe both what the skill does AND when to use it. This is the only field the agent sees at startup. |
| `license` | No | Short string or reference to bundled LICENSE file |
| `compatibility` | No | 1-500 chars, describes environment requirements (OS, packages, network access) |
| `metadata` | No | Arbitrary key-value map for additional properties |
| `allowed-tools` | No | Space-delimited pre-approved tool list (experimental) |

**Minimal valid SKILL.md:**
```markdown
---
name: pdf-processing
description: Extract PDF text, fill forms, merge files. Use when handling PDFs.
---

## When to use this skill
Use when the user asks about PDFs, forms, or document extraction.

## Procedure
1. ...
```

**Full example with optional fields:**
```markdown
---
name: data-analysis
description: Analyze datasets, generate charts, summary reports. Use when working with CSVs or tabular data.
license: MIT
compatibility: Requires Python 3.10+
metadata:
  author: example-org
  version: "1.0"
allowed-tools: Bash(python:*) Read Write
---
```

### Directory Structure

```
~/.ironhermes/skills/
├── category/              # Optional grouping (hermes-agent convention)
│   └── skill-name/
│       ├── SKILL.md       # Required
│       ├── scripts/       # Optional: executable helpers
│       │   └── extract.py
│       ├── references/    # Optional: detailed docs
│       │   └── REFERENCE.md
│       └── assets/        # Optional: templates, data files
└── .agents/skills/        # Cross-client interoperability path
    └── skill-name/
        └── SKILL.md
```

---

## 2. Progressive Disclosure Pattern

The spec defines a three-tier loading strategy. This is the core design principle — the agent never pays the token cost of all skills upfront.

| Tier | What Loads | When | Approx Token Cost |
|------|-----------|------|------------------|
| 1. Catalog | `name` + `description` only | Session startup, always | ~50-100 tokens per skill |
| 2. Instructions | Full `SKILL.md` body | When agent decides skill is relevant | <5000 tokens recommended |
| 3. Resources | Files in `scripts/`, `references/`, `assets/` | When instructions reference them | Varies per file |

**Tier 1 behavior:** At session start, the agent receives a compact catalog of all available skills — just names and descriptions. With 20 skills installed, this costs 1-2k tokens total, not 20x the full instruction sets.

**Tier 2 behavior:** When the agent determines a skill matches the current task (based on description), it activates the skill. This delivers the full SKILL.md body into context. The YAML frontmatter can be stripped (most dedicated-tool implementations do this) or included (file-read implementations pass the raw file).

**Tier 3 behavior:** Scripts and reference files are NOT eagerly read. The skill body references them with relative paths (e.g., `scripts/extract.py`). The agent reads specific files on demand using its file-reading tool.

**What to load upfront vs on-demand:**
- Upfront: `name` + `description` for every discovered skill (catalog)
- On activation: full `SKILL.md` body (markdown after the frontmatter)
- On demand: anything in `scripts/`, `references/`, `assets/`

---

## 3. Skill Discovery Architecture

### Discovery Paths (Standard Conventions)

Per agentskills.io spec, agents should scan at minimum:

| Scope | Primary Path | Cross-client Path |
|-------|-------------|------------------|
| User | `~/.ironhermes/skills/` | `~/.agents/skills/` |
| Project | `<cwd>/.<agent>/skills/` | `<cwd>/.agents/skills/` |

**Precedence:** Project-level skills override user-level skills when names collide.

**For IronHermes specifically:** Scan `~/.ironhermes/skills/` (primary) and `~/.agents/skills/` (cross-client compat). Project-level skill scanning is optional for v1.1 but the `~/.agents/skills/` path enables skill sharing with Claude Code.

### Discovery Algorithm

1. Walk each skills directory (max depth 4, skip `.git`, `node_modules`)
2. Find subdirectories containing `SKILL.md`
3. Parse frontmatter: extract `name`, `description`, optional fields
4. Validate: warn-but-load on non-critical issues (name mismatch, excess length); skip on missing description or unparseable YAML
5. Build in-memory catalog keyed by `name` → `SkillRecord { name, description, path, base_dir }`
6. Handle name collisions: project > user precedence, log warning

### What to Store Per Skill

```rust
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub path: PathBuf,        // absolute path to SKILL.md
    pub base_dir: PathBuf,    // parent dir of SKILL.md (for relative path resolution)
    // Optional fields:
    pub compatibility: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, String>>,
}
```

Store only metadata at discovery time. Read the body at activation time (picks up file edits without restart, and avoids memory cost for skills that are never activated in a session).

---

## 4. How Python hermes-agent Handles Skills

The Python original has a mature skills system that IronHermes should replicate at parity.

### Three-Level API

The Python agent exposes three skill tool operations that map directly to the progressive disclosure tiers:

| Level | Function | Returns | Token Cost |
|-------|----------|---------|-----------|
| 0 | `skills_list()` | All skill names + descriptions + categories | ~3k tokens for full catalog |
| 1 | `skill_view(name)` | Full SKILL.md content + metadata | Variable |
| 2 | `skill_view(name, path)` | Specific reference file within a skill | Variable |

### Directory Layout

Skills live in `~/.hermes/skills/` organized by category:

```
~/.hermes/skills/
├── media/
│   └── gif-search/
│       ├── SKILL.md
│       └── scripts/
├── ai/
│   └── axolotl/
│       └── SKILL.md
└── .hub/                  # Hub registry state
```

### Extended Frontmatter (hermes-specific)

Python hermes-agent extends the agentskills.io spec with additional fields under `metadata.hermes`:

```yaml
---
name: skill-name
description: Brief summary
version: 1.0.0
platforms: [macos, linux]
metadata:
  hermes:
    tags: [category, subcategory]
    category: devops
    fallback_for_toolsets: [web]      # show when premium tools unavailable
    requires_toolsets: [terminal]      # only show when toolset available
    requires_tools: [specific_tool]    # only show when tool available
    related_skills: [other-skill]
    config:
      - key: setting.path
        description: What it does
        default: value
        prompt: "Setup prompt shown to user"
---
```

**Key hermes-specific behaviors:**
- **Platform filtering:** Skip skills that list platforms not matching current OS
- **Toolset filtering:** Skills with `requires_toolsets` only appear in catalog when those toolsets are available
- **Fallback skills:** Skills with `fallback_for_toolsets` appear automatically when listed toolsets are unavailable (e.g., `duckduckgo-search` appears when premium web search is not configured)

### Skill Management Tool (`skill_manage`)

Python hermes-agent also has a `skill_manage` tool that lets the agent create and edit its own skills — this is part of the "self-improving" story. Actions: `create`, `patch`, `edit`, `delete`, `write_file`, `remove_file`.

For IronHermes v1.1, the `skill_manage` capability (agent writes its own skills) is desirable but can be deferred. The discovery and activation system is the priority.

---

## 5. Tool Interface Design for IronHermes

### Recommended: Single `skills` Tool with Action Parameter

Rather than three separate tools, use one tool with an `action` parameter. This matches the hermes-agent pattern and keeps the tool list compact.

```
Tool name: skills
Actions: list | view | activate
```

**`list` action:**
Returns the full skills catalog (names + descriptions). This is what backs the Tier 1 catalog in the system prompt, but also allows the agent to re-query at runtime.

```json
{ "action": "list" }
```

Returns:
```
Available skills (3):
- pdf-processing: Extract PDF text, fill forms, merge files. Use when handling PDFs.
- data-analysis: Analyze datasets, generate charts. Use when working with tabular data.
- git-flow: Manage git branches and PRs following team conventions.
```

**`view` action:**
Returns full SKILL.md body for one skill (Tier 2 activation). Optionally accepts a `path` for Tier 3 resource loading.

```json
{ "action": "view", "name": "pdf-processing" }
{ "action": "view", "name": "pdf-processing", "path": "scripts/extract.py" }
```

Returns the skill body wrapped in identifying tags:
```
<skill_content name="pdf-processing">
[full SKILL.md body, frontmatter stripped]

Skill directory: /home/user/.ironhermes/skills/pdf-processing

Available resources:
- scripts/extract.py
- references/pdf-spec-summary.md
</skill_content>
```

**`activate` action (alias for view):**
Activates the skill — same as `view` but explicit. Most agent implementations use `activate_skill` as the verb for the dedicated tool.

### Tool Schema (Rust)

```rust
// In ironhermes-tools/src/skills_tool.rs
pub struct SkillsTool {
    pub registry: Arc<SkillRegistry>,
}

// JSON schema:
// {
//   "type": "object",
//   "properties": {
//     "action": { "type": "string", "enum": ["list", "view", "activate"] },
//     "name": { "type": "string", "description": "Skill name (required for view/activate)" },
//     "path": { "type": "string", "description": "Relative path to resource file (optional)" }
//   },
//   "required": ["action"]
// }
```

### System Prompt Injection (Tier 1 Catalog)

At session build time, `PromptBuilder` should inject the skills catalog. The catalog is built from `SkillRegistry` at startup (frozen snapshot, same pattern as SOUL.md/AGENTS.md):

```
## Available Skills

The following skills provide specialized knowledge and workflows. When a task
matches a skill's description, call the `skills` tool with action "activate"
and the skill name to load full instructions before proceeding.

- pdf-processing: Extract PDF text, fill forms, merge files. Use when handling PDFs.
- data-analysis: Analyze datasets, generate charts. Use when working with tabular data.
```

If no skills are discovered, omit the section entirely (don't show an empty block).

---

## 6. Integration with Existing IronHermes Architecture

### Where Skills Fit

Skills integrate at two points in the existing architecture:

1. **Session startup (PromptBuilder):** Discover skills, build catalog, inject into system prompt as a frozen section. Same pattern as SOUL.md loading.
2. **Agent loop (ToolRegistry):** Register `SkillsTool` with the registry. Agent calls it during turns to activate skills and load resources.

### New Component: `SkillRegistry`

Lives in `ironhermes-core` (it's shared data, not a tool). Loaded once at startup, passed to both `PromptBuilder` and `SkillsTool`.

```rust
// ironhermes-core/src/skills.rs
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub base_dir: PathBuf,
    pub compatibility: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub metadata: Option<HashMap<String, String>>,
}

pub struct SkillRegistry {
    skills: HashMap<String, SkillRecord>,
}

impl SkillRegistry {
    pub fn load(home_dir: &Path) -> Self { ... }  // scans ~/.ironhermes/skills/ and ~/.agents/skills/
    pub fn list(&self) -> Vec<&SkillRecord> { ... }
    pub fn get(&self, name: &str) -> Option<&SkillRecord> { ... }
    pub fn format_catalog(&self) -> Option<String> { ... }  // returns None if empty
}
```

### Dependency Direction

```
ironhermes-core
  └── SkillRegistry, SkillRecord  [NEW — no new deps, just fs + serde_yaml]
        ↑
ironhermes-tools
  └── SkillsTool  [NEW — implements Tool, takes Arc<SkillRegistry>]
        ↑
ironhermes-agent
  └── PromptBuilder  [MODIFY — inject catalog from Arc<SkillRegistry>]
```

No new crate needed. `SkillRegistry` belongs in `ironhermes-core` (like `MemoryStore`). `SkillsTool` belongs in `ironhermes-tools`.

### PromptBuilder Changes

Add `Arc<SkillRegistry>` field to `PromptBuilder`, following the same pattern as `memory_store`:

```rust
pub fn set_skill_registry(&mut self, registry: Arc<SkillRegistry>) {
    self.skill_registry = Some(registry);
}
```

In `build()`, after the memory section, append the skills catalog:

```rust
if let Some(ref registry) = self.skill_registry {
    if let Some(catalog) = registry.format_catalog() {
        parts.push(catalog);
    }
}
```

### Context Compressor Protection

Skills content injected via the `skills` tool during a session must be protected from context compression. The existing `ContextCompressor` prunes tool results. Add a flag to mark skill tool results as protected (same approach as the spec recommends for skill content).

Alternative: wrap skill content in identifiable XML tags (`<skill_content name="...">`) and teach the compressor to skip messages containing those tags.

---

## 7. Skill File Format Specification for IronHermes

IronHermes should be fully compatible with agentskills.io and partially compatible with hermes-agent's extensions.

### Required Fields (agentskills.io standard)
- `name`: lowercase alphanumeric + hyphens, matches directory name
- `description`: describes what + when, 1-1024 chars

### Optional Fields (agentskills.io standard)
- `license`
- `compatibility`
- `metadata` (key-value map)
- `allowed-tools` (space-delimited list, experimental)

### Optional Fields (hermes-agent extensions, support as best-effort)
- `version`: semantic version string
- `platforms`: list of `[macos, linux, windows]` — filter at discovery
- `metadata.hermes.tags`: categorization
- `metadata.hermes.requires_toolsets`: hide if toolsets unavailable
- `metadata.hermes.fallback_for_toolsets`: show only when toolsets unavailable
- `metadata.hermes.category`: display grouping

IronHermes v1.1 should parse these extended fields where present but only act on `platforms` filtering (skip skills for wrong platform) and store the rest for display. Toolset-based filtering can come in a later phase.

### Parsing Strategy

Use `serde_yaml` (already in workspace deps via `config.rs`). Parse the YAML frontmatter by:
1. Detect opening `---` at start of file
2. Find closing `---`  
3. Parse YAML block between them
4. Treat remainder as body content

Handle the common malformed YAML case (unquoted colon in description): wrap value in quotes and retry parse.

---

## 8. Directory Structure for IronHermes

```
~/.ironhermes/             # IRONHERMES_HOME
├── SOUL.md
├── AGENTS.md
├── MEMORY.md
├── config.yaml
└── skills/                # NEW: skill install location
    ├── skill-name/
    │   ├── SKILL.md
    │   ├── scripts/
    │   └── references/
    └── category/
        └── skill-name/
            └── SKILL.md
```

Cross-client compatibility path (read-only, IronHermes writes to `~/.ironhermes/skills/`):
```
~/.agents/skills/           # scanned but not written to
    └── skill-name/
        └── SKILL.md
```

---

## 9. Pitfalls Specific to Skills Integration

### Pitfall 1: Skill content pruned by context compressor
**What:** The context compressor drops older tool results to manage context window. If the `skills` tool result gets dropped, the agent loses its loaded skill instructions mid-session.
**Prevention:** Wrap skill content in `<skill_content name="...">` tags and add logic to `ContextCompressor` to protect messages containing those tags. Alternatively, mark skill tool results with a `protected` flag in a wrapper struct.

### Pitfall 2: Catalog injected even when empty
**What:** `PromptBuilder` adds a `## Available Skills` block even when no skills are installed. This wastes tokens and may confuse the agent.
**Prevention:** `SkillRegistry::format_catalog()` returns `Option<String>`. Return `None` when catalog is empty. `PromptBuilder` only appends when `Some`.

### Pitfall 3: Skills directory path hardcoded
**What:** Discovery only scans `~/.ironhermes/skills/` and misses user-installed skills in `~/.agents/skills/`.
**Prevention:** Always scan both paths. Make the scan paths configurable via `config.yaml` with sensible defaults.

### Pitfall 4: YAML parse failure on valid hermes-agent skills
**What:** Skills authored for hermes-agent may use unquoted colons in description values (technically invalid YAML). Hard failing on these breaks cross-client compat.
**Prevention:** Implement fallback parse: on YAML error, wrap the `description:` value in quotes and retry. Log the fix at debug level.

### Pitfall 5: Name collision silent override
**What:** Two skills named `git-flow` (one user-level, one project-level) — project wins silently. User doesn't know their user skill was shadowed.
**Prevention:** Log `tracing::warn!` when a name collision is resolved. Surface in `skills list` output with a `[shadowed]` indicator.

### Pitfall 6: Skill body injected with frontmatter included
**What:** Returning raw `SKILL.md` content including YAML frontmatter to the LLM wastes tokens on metadata the agent doesn't need in context.
**Prevention:** Strip frontmatter in `SkillsTool::execute` for the `view`/`activate` action. The body starts after the closing `---`.

---

## 10. Integration Points Summary

| What | Where | Change Type |
|------|-------|-------------|
| `SkillRecord`, `SkillRegistry` | `ironhermes-core/src/skills.rs` | New module |
| `SkillsTool` | `ironhermes-tools/src/skills_tool.rs` | New tool |
| `register_skills_tool()` | `ironhermes-tools/src/registry.rs` | New method (mirrors `register_memory_tool`) |
| `PromptBuilder::set_skill_registry()` | `ironhermes-agent/src/prompt_builder.rs` | New optional field + method |
| Skills catalog section in `build()` | `ironhermes-agent/src/prompt_builder.rs` | Additive change in `build()` |
| Context compressor protection | `ironhermes-agent/src/context_compressor.rs` | Protect `<skill_content>` tagged messages |
| `Config::skills` section | `ironhermes-core/src/config.rs` | Optional `SkillsConfig` with scan paths |
| Wiring in CLI and gateway | `ironhermes-cli/src/main.rs`, `ironhermes-gateway` | Pass `Arc<SkillRegistry>` at startup |

---

## Sources

- agentskills.io specification (fetched 2026-04-08): https://agentskills.io/specification
- agentskills.io client implementation guide (fetched 2026-04-08): https://agentskills.io/client-implementation/adding-skills-support
- hermes-agent skills system docs (fetched 2026-04-08): https://hermes-agent.nousresearch.com/docs/user-guide/features/skills/
- hermes-agent skill creation guide (fetched 2026-04-08): https://hermes-agent.nousresearch.com/docs/developer-guide/creating-skills/
- Direct codebase analysis: `crates/ironhermes-agent/src/prompt_builder.rs` — PromptBuilder pattern, load_context, frozen snapshot
- Direct codebase analysis: `crates/ironhermes-tools/src/registry.rs` — Tool trait, ToolRegistry, register_memory_tool pattern
- Direct codebase analysis: `crates/ironhermes-core/src/constants.rs` — get_hermes_home(), IRONHERMES_HOME
- Direct codebase analysis: `crates/ironhermes-agent/src/agent_loop.rs` — AgentLoop, execute_tool_call, ContextCompressor usage
