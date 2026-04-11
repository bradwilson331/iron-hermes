# Phase 7: Skills System - Research

**Researched:** 2026-04-09
**Domain:** Rust skill discovery, YAML frontmatter parsing, progressive disclosure tool pattern
**Confidence:** HIGH

## Summary

Phase 7 adds a skill discovery and activation system to IronHermes. All user decisions are locked via CONTEXT.md. The implementation follows well-established codebase patterns: SkillRegistry in `ironhermes-core` (same home as Config and MemoryStore), SkillsTool in `ironhermes-tools` mirroring CronjobTool's action-based pattern, catalog injection in PromptBuilder following the existing context-loading pattern, and skill resolution in the cron tick runner.

The primary technical challenge is YAML frontmatter parsing. The workspace already depends on `serde_yaml = "0.9"` (used in `ironhermes-core/src/config.rs`), so no new crate dependency is needed for `ironhermes-core`. The frontmatter format is straightforward: a `---` block at the top of the file followed by markdown body — this needs a simple string splitter, not a dedicated library.

The cron-skill wiring point is `ironhermes-gateway` (the GatewayRunner that runs the tick loop). The cron tick runner currently returns due jobs; the gateway must resolve skill content before constructing the agent_input passed to AgentLoop. This requires `ironhermes-cron` to gain a dependency on `ironhermes-core` (for SkillRegistry) — but SkillRegistry lives in `ironhermes-core` which `ironhermes-cron` already depends on, so the dependency graph remains clean.

**Primary recommendation:** Implement SkillRegistry in `ironhermes-core`, SkillsTool in `ironhermes-tools`, catalog injection in PromptBuilder, and cron wiring in the gateway runner — all following existing patterns with no new crate dependencies.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**Skill Discovery**
- D-01: Flat directory layout — `{skills_dir}/{skill-name}/SKILL.md`. No nested category hierarchy. Categories are expressed as tags in YAML frontmatter instead of directory structure.
- D-02: Three scan paths in priority order: (1) `{cwd}/.ironhermes/skills/` (project-level, highest precedence), (2) `~/.ironhermes/skills/` (user global), (3) `~/.agents/skills/` (agentskills.io standard path). On name conflict, earlier path wins.
- D-03: Scanner walks each path looking for `SKILL.md` files. Each SKILL.md uses the agentskills.io format: YAML frontmatter with `name` and `description` (required), plus optional `version`, `author`, `license`, `metadata`. Markdown body contains the full skill content.

**Catalog Format**
- D-04: Compact one-line-per-skill catalog injected into the system prompt at session start: `"- {name}: {description}"`. No tags, version, or categories in the prompt — minimal token usage.
- D-05: Include a brief usage hint after the catalog: `"Use the skills tool to view or activate a skill before using it."` Ensures the agent knows how to load full content.

**Skills Tool API**
- D-06: Single `skills` tool with `action` parameter — actions: `list` (show catalog with descriptions), `view` (show full SKILL.md content without activation), `activate` (load full content and return it). Matches the cronjob tool's compressed action pattern.
- D-07: `activate` returns the full SKILL.md markdown body as the tool result. No system prompt mutation or session state tracking — the agent receives skill content like any tool output and follows the instructions.

**Cron-Skill Wiring**
- D-08: At cron tick time, resolve each skill name in the job's `skills: Vec<String>` against the SkillRegistry. Read full SKILL.md content for each, prepend to the job's `agent_input` as context. Agent sees skill content before the task prompt.
- D-09: Missing skill names at tick time produce a tracing warning and are skipped. The job runs with whatever skills resolved successfully. Jobs should not break because a skill was removed.

### Claude's Discretion
- Internal SkillRegistry data structure and caching strategy
- SKILL.md YAML parsing approach (serde_yaml or manual)
- How SkillRegistry is shared across components (Arc pattern)
- Whether to fire hook events on skill activation (leveraging Phase 6 hooks)
- Error message formatting for the skills tool

### Deferred Ideas (OUT OF SCOPE)
None — discussion stayed within phase scope.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SKILL-01 | Agent discovers skill documents from skills directories (~/.ironhermes/skills/, ~/.agents/skills/, project-level) | D-02/D-03: SkillRegistry scans three priority paths at startup; `get_hermes_home()` gives base path |
| SKILL-02 | Skills use progressive disclosure — catalog (name+description) loaded at session start, full content loaded only on activation | D-04/D-07: PromptBuilder injects compact catalog; full content returned only on `activate` tool call |
| SKILL-03 | Skill documents follow the agentskills.io open standard (SKILL.md with name/description frontmatter, Markdown body) | D-03: YAML frontmatter with required `name`/`description`; markdown body; verified from reference SKILL.md files |
| SKILL-04 | Agent can list, view, and activate skills via a dedicated skills tool during conversation | D-06: Single `skills` tool with list/view/activate actions, mirroring CronjobTool pattern |
</phase_requirements>

---

## Standard Stack

### Core
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| serde_yaml | 0.9 | YAML frontmatter parsing | Already in workspace dependencies — no new dep needed |
| serde | 1 | Derive for SkillFrontmatter struct | Already in workspace |
| serde_json | 1 | Tool response serialization | Already in workspace |
| async-trait | 0.1 | Tool trait implementation | Already in workspace |
| tracing | 0.1 | Warning on missing skills at tick time | Already in workspace |

[VERIFIED: /Users/twilson/code/ironhermes/Cargo.toml — all dependencies confirmed in workspace]

### Supporting
| Library | Version | Purpose | When to Use |
|---------|---------|---------|-------------|
| glob | 0.3 | Directory walking for skill discovery | If std::fs::read_dir recursion becomes verbose; optional since layout is flat (one level deep) |

[VERIFIED: /Users/twilson/code/ironhermes/Cargo.toml — glob is already a workspace dependency]

### Alternatives Considered
| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| serde_yaml for frontmatter | Manual string split on `---` | Manual split is simpler for the narrow case (parse only `name` and `description` required fields); serde_yaml gives typed deserialization and handles edge cases — prefer serde_yaml |
| `Arc<SkillRegistry>` | `Arc<Mutex<SkillRegistry>>` | Registry is read-only after startup (no writes); plain `Arc<SkillRegistry>` with interior immutability is sufficient and avoids lock contention |

**Installation:** No new dependencies required. All needed crates are already in workspace.

---

## Architecture Patterns

### Recommended Project Structure

```
crates/ironhermes-core/src/
├── skills.rs               # NEW: SkillRegistry, SkillRecord, YAML parsing

crates/ironhermes-tools/src/
├── skills_tool.rs          # NEW: SkillsTool implementing Tool trait

crates/ironhermes-agent/src/
└── prompt_builder.rs       # MODIFIED: inject skill catalog in build()

crates/ironhermes-cron/ (no change to crate — wiring in gateway)

crates/ironhermes-gateway/src/
└── runner.rs               # MODIFIED: resolve skills at tick time
```

### Pattern 1: SkillRegistry in ironhermes-core

**What:** A struct that scans skill paths at construction time, stores `Vec<SkillRecord>` (name, description, full_path), and exposes read-only methods. Loaded once at startup, shared via `Arc<SkillRegistry>`.

**When to use:** Follows MemoryStore and JobStore patterns — one authoritative store, shared by reference.

**Example:**
```rust
// Source: [ASSUMED] — pattern inferred from existing MemoryStore/JobStore in codebase

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SkillRecord {
    pub name: String,
    pub description: String,
    pub path: PathBuf,  // full path to SKILL.md
}

pub struct SkillRegistry {
    skills: Vec<SkillRecord>,
}

impl SkillRegistry {
    /// Load from three priority-ordered scan paths.
    pub fn load(cwd: &Path) -> Self { ... }

    /// Compact catalog for system prompt injection.
    pub fn catalog_text(&self) -> String { ... }

    /// Read full SKILL.md content (body only, not frontmatter).
    pub fn read_content(&self, name: &str) -> Option<String> { ... }

    /// Find a skill by name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<&SkillRecord> { ... }

    pub fn list(&self) -> &[SkillRecord] { ... }
}
```

### Pattern 2: YAML Frontmatter Parsing

**What:** SKILL.md files begin with `---\n`, a YAML block, then `---\n`, then markdown body. Split on the second `---` delimiter to extract frontmatter and body separately.

**When to use:** For all SKILL.md loading in both SkillRegistry (description extraction) and SkillsTool `activate` action (body content).

**Example:**
```rust
// Source: [ASSUMED] — pattern derived from agentskills.io format + serde_yaml usage in config.rs

fn parse_skill_md(content: &str) -> Option<(SkillFrontmatter, String)> {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return None;  // no frontmatter
    }
    // Skip opening ---
    let rest = content.trim_start_matches("---").trim_start_matches('\n');
    // Find closing ---
    let end = rest.find("\n---")?;
    let yaml_block = &rest[..end];
    let body = rest[end..].trim_start_matches("\n---").trim_start_matches('\n');
    let frontmatter: SkillFrontmatter = serde_yaml::from_str(yaml_block).ok()?;
    Some((frontmatter, body.to_string()))
}
```

### Pattern 3: SkillsTool — Action-Based Tool (mirrors CronjobTool)

**What:** Single `skills` tool with `action` parameter dispatching to `list`, `view`, `activate` handlers. Holds `Arc<SkillRegistry>`.

**When to use:** Exact mirror of CronjobTool pattern — JSON args, match on action string, return `serde_json::to_string()` of result.

**Example:**
```rust
// Source: [VERIFIED: crates/ironhermes-tools/src/cronjob_tool.rs]

pub struct SkillsTool {
    registry: Arc<SkillRegistry>,
}

#[async_trait]
impl Tool for SkillsTool {
    fn name(&self) -> &str { "skills" }
    fn toolset(&self) -> &str { "skills" }

    async fn execute(&self, args: Value) -> anyhow::Result<String> {
        let action = args.get("action").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'action'"))?;
        let result = match action {
            "list"     => handle_list(&self.registry),
            "view"     => handle_view(&self.registry, &args),
            "activate" => handle_activate(&self.registry, &args),
            other      => json!({"status":"error","message":format!("Unknown action '{}'. Valid: list, view, activate", other)}),
        };
        Ok(serde_json::to_string(&result)?)
    }
}
```

### Pattern 4: PromptBuilder Catalog Injection

**What:** Add an optional `skill_registry` field to `PromptBuilder`. In `build()`, if the registry has skills, append a `## Skills` section after AGENTS.md and before memory (or as the last non-memory section).

**When to use:** Follows existing context-loading pattern — `set_memory_store()` shows the exact pattern for adding optional late-bound context.

**Example:**
```rust
// Source: [VERIFIED: crates/ironhermes-agent/src/prompt_builder.rs]

// In PromptBuilder::build():
// After section 5 (AGENTS.md), before section 6 (Memory):
if let Some(ref registry) = self.skill_registry {
    let catalog = registry.catalog_text();
    if !catalog.is_empty() {
        let block = format!(
            "## Available Skills\n\n{}\n\nUse the skills tool to view or activate a skill before using it.",
            catalog
        );
        parts.push(block);
    }
}
```

### Pattern 5: Cron-Skill Wiring in Gateway Runner

**What:** The gateway runner (or wherever the cron tick fires agent runs) resolves skill names from `job.skills` against `Arc<SkillRegistry>`, reads full content for each, and prepends to `agent_input` before constructing the AgentLoop messages.

**When to use:** D-08 requires skill content to appear before the task prompt. The gateway runner is the integration point that has both the CronJob and the AgentLoop.

**Example:**
```rust
// Source: [ASSUMED] — pattern derived from tick.rs + delivery.rs review

fn resolve_skill_context(registry: &SkillRegistry, skill_names: &[String]) -> String {
    let mut parts = Vec::new();
    for name in skill_names {
        match registry.read_content(name) {
            Some(content) => parts.push(format!("## Skill: {}\n\n{}", name, content)),
            None => tracing::warn!(skill = %name, "Skill not found at tick time — skipping"),
        }
    }
    parts.join("\n\n---\n\n")
}

// Then prepend to agent_input:
let skill_ctx = resolve_skill_context(&registry, &job.skills);
let full_input = if skill_ctx.is_empty() {
    job.prompt.clone()
} else {
    format!("{}\n\n---\n\n{}", skill_ctx, job.prompt)
};
```

### Anti-Patterns to Avoid

- **Loading full SKILL.md body at startup:** Only frontmatter (name + description) is needed for the catalog. Full body is disk I/O on demand only (view/activate/cron tick). Do not preload all bodies into memory.
- **Mutating system prompt mid-session:** D-07 explicitly prohibits this. `activate` returns skill content as a tool result — the agent incorporates it naturally without any prompt mutation.
- **Arc<Mutex<SkillRegistry>> when read-only:** The registry is loaded once and never mutated. Plain `Arc<SkillRegistry>` is sufficient. Adding a Mutex creates false contention.
- **Scanning skills on every tool call:** Scan once at startup (or at SkillRegistry construction), cache in the struct. Re-scanning on every `list`/`view`/`activate` call is wasteful and inconsistent.
- **Breaking the existing PromptBuilder section order:** Insert skills section after AGENTS.md (section 5), before memory (section 6). This preserves the established priority ordering: SOUL > platform > tool guidance > project context > AGENTS > skills > memory.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| YAML parsing | Custom YAML parser | `serde_yaml::from_str` | Edge cases in YAML strings, multiline values, special characters |
| Directory walking | Custom recursive walker | `std::fs::read_dir` (flat layout — one level deep) | Skills layout is flat by D-01; `{skills_dir}/{name}/SKILL.md` needs only one level |
| Name conflict resolution | Complex merge strategy | First-path-wins at scan time | D-02 specifies priority order; store names in `HashSet` during scan, skip duplicates |
| Skill content caching | LRU cache or TTL cache | None — read on demand | Skills are small markdown files; disk reads are fast; caching adds complexity with no measurable benefit |

**Key insight:** The skills system is intentionally minimal — discovery + catalog + on-demand reads. Every complexity beyond that is premature.

---

## Common Pitfalls

### Pitfall 1: YAML Frontmatter Delimiter Ambiguity
**What goes wrong:** SKILL.md body contains `---` (e.g., a markdown horizontal rule), causing the frontmatter parser to split at the wrong boundary.
**Why it happens:** Naive `content.split("---")` splits on ALL occurrences, not just the second one.
**How to avoid:** Use `find("\n---")` to locate the closing frontmatter delimiter precisely — find the FIRST occurrence of `\n---` after the opening `---` block. The opening `---` must be at the very start of the file (after trim).
**Warning signs:** Skill descriptions truncated or containing markdown content; `serde_yaml::from_str` returning errors on valid frontmatter.

### Pitfall 2: Name Conflict Across Scan Paths
**What goes wrong:** A skill named `"focus"` exists in both `~/.ironhermes/skills/` and `~/.agents/skills/`. The registry contains two entries with the same name, and `find("focus")` returns an unpredictable one.
**Why it happens:** Failing to track which names have already been registered during the multi-path scan.
**How to avoid:** Use a `HashSet<String>` (lowercase names) during scanning. When a skill name is already in the set, skip it. Log a `tracing::debug!` for the skipped duplicate. D-02 specifies first path wins.

### Pitfall 3: Missing `register_skills_tool` Call
**What goes wrong:** SkillsTool is implemented but never registered; agent cannot see or call it.
**Why it happens:** The pattern for memory and cronjob tools uses separate `register_*` methods that must be called explicitly from CLI/gateway entry points (not in `register_defaults()`).
**How to avoid:** Add `register_skills_tool(&mut self, registry: Arc<SkillRegistry>)` to `ToolRegistry` following the exact same pattern as `register_cronjob_tool` and `register_memory_tool`. Call it from both CLI (`main.rs`) and gateway (`runner.rs`) entry points.

### Pitfall 4: Skill Catalog Injected When No Skills Exist
**What goes wrong:** System prompt contains an empty `## Available Skills` section with just the usage hint, wasting tokens and potentially confusing the agent.
**Why it happens:** Catalog injection not guarded against empty registry.
**How to avoid:** Only inject the catalog section if `registry.list().is_empty()` is false. Guard: `if !registry.list().is_empty() { parts.push(catalog_block); }`.

### Pitfall 5: SkillRegistry Not Available at Cron Tick Time
**What goes wrong:** The gateway runner runs the cron tick but has no reference to SkillRegistry, so `job.skills` cannot be resolved.
**Why it happens:** SkillRegistry is constructed in the CLI/gateway startup and not threaded through to the tick execution path.
**How to avoid:** The gateway runner already has `Arc<ToolRegistry>` which contains `SkillsTool` which holds `Arc<SkillRegistry>`. One option: store `Arc<SkillRegistry>` directly on the gateway runner struct alongside existing Arc fields. This is the cleanest approach given the existing runner pattern.

### Pitfall 6: serde_yaml Version Mismatch
**What goes wrong:** `serde_yaml = "0.9"` uses `serde_yaml::from_str`, but it was deprecated in favor of `serde_yml` in some newer guidance.
**Why it happens:** Ecosystem confusion between `serde_yaml` (original) and `serde_yml` (fork).
**How to avoid:** The workspace already uses `serde_yaml = "0.9"` in `ironhermes-core/src/config.rs`. Use the same crate consistently — no migration needed. `serde_yaml 0.9` is stable and appropriate for this use case. [VERIFIED: /Users/twilson/code/ironhermes/Cargo.toml]

---

## Code Examples

Verified patterns from the codebase:

### Registering a tool that requires a shared store (CronjobTool pattern)
```rust
// Source: [VERIFIED: crates/ironhermes-tools/src/registry.rs lines 144-147]
pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
    use crate::cronjob_tool::CronjobTool;
    self.register(Box::new(CronjobTool::new(store)));
}
// Skills follows identical pattern:
pub fn register_skills_tool(&mut self, registry: Arc<SkillRegistry>) {
    use crate::skills_tool::SkillsTool;
    self.register(Box::new(SkillsTool::new(registry)));
}
```

### Optional context injection in PromptBuilder
```rust
// Source: [VERIFIED: crates/ironhermes-agent/src/prompt_builder.rs lines 50-52, 165-174]
// Pattern for adding late-bound optional context:
pub fn set_skill_registry(&mut self, registry: Arc<SkillRegistry>) {
    self.skill_registry = Some(registry);
}
// In build(), after section 5 (AGENTS.md):
if let Some(ref registry) = self.skill_registry {
    let catalog = registry.catalog_text();
    if !catalog.is_empty() {
        parts.push(format!(
            "## Available Skills\n\n{}\n\nUse the skills tool to view or activate a skill before using it.",
            catalog
        ));
    }
}
```

### HookEventKind extension for skill activation (Claude's discretion)
```rust
// Source: [VERIFIED: crates/ironhermes-hooks/src/event.rs]
// If hook firing is desired, add to HookEventKind enum:
SkillActivated {
    skill_name: String,
    source: String,  // "tool" or "cron"
},
// Then fire from SkillsTool::execute() and cron skill resolution
```

### Tracing warning for missing skills (D-09)
```rust
// Source: [ASSUMED] — pattern consistent with existing tracing usage in tick.rs
for name in &job.skills {
    match registry.find(name) {
        Some(record) => { /* load content */ }
        None => tracing::warn!(
            skill = %name,
            job_id = %job.id,
            "Skill not found in registry at tick time — skipping"
        ),
    }
}
```

### Catalog text format (D-04)
```rust
// Source: [ASSUMED] — format specified by D-04
pub fn catalog_text(&self) -> String {
    self.skills
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect::<Vec<_>>()
        .join("\n")
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| All context loaded upfront | Progressive disclosure (catalog only at start) | agentskills.io standard | Saves significant tokens for agents with many skills |
| Skills in AGENTS.md | Dedicated SKILL.md with frontmatter | agentskills.io v1 | Structured metadata enables tooling, search, versioning |

**Deprecated/outdated:**
- Embedding full skill content in AGENTS.md: replaced by dedicated SKILL.md with progressive disclosure.

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | SkillRegistry is best placed in ironhermes-core (not a new crate) | Architecture Patterns | Low — confirmed by STATE.md decision "SkillRegistry in ironhermes-core" |
| A2 | Frontmatter parsing should use serde_yaml rather than manual split | Architecture Patterns | Low — both work; serde_yaml is more robust |
| A3 | Catalog injection position is after AGENTS.md (section 5), before memory (section 6) | Code Examples | Low — ordering is a planner/implementer choice; position affects token budget but not correctness |
| A4 | Gateway runner is the correct injection point for cron-skill wiring (vs. delivery.rs or tick.rs) | Architecture Patterns | Medium — if cron tick runs outside the gateway (e.g., standalone CLI tick command), wiring must happen there too |
| A5 | Hook event firing on skill activation is optional (Claude's discretion) | Code Examples | Low — no requirement mandates it; adds observability value |

---

## Open Questions

1. **Cron tick invocation path**
   - What we know: `run_tick_check` and `complete_job_run` are in `ironhermes-cron/src/tick.rs`; the gateway runner calls these
   - What's unclear: Whether a CLI `tick` subcommand exists or is planned — if so, it also needs SkillRegistry access
   - Recommendation: Planner should check `crates/ironhermes-cli/src/main.rs` for any existing tick subcommand; if present, add SkillRegistry there too

2. **PromptBuilder mutation pattern**
   - What we know: `set_memory_store` mutates `self` (takes `&mut self`); `load_context` consumes self (builder pattern)
   - What's unclear: Whether to follow the `&mut self` pattern (`set_skill_registry`) or the consuming builder pattern (`with_skill_registry`)
   - Recommendation: Follow `set_memory_store` pattern (`&mut self`) for consistency with the existing code

3. **Case sensitivity for skill name lookup**
   - What we know: D-02 specifies "name conflict" resolution by path priority but doesn't specify case sensitivity
   - What's unclear: Should `find("Focus")` match a skill named `"focus"`?
   - Recommendation: Normalize to lowercase during scanning and lookup; simpler and more user-friendly

---

## Environment Availability

Step 2.6: SKIPPED (no external dependencies — this is a pure Rust code addition using existing workspace crates).

---

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in test + `#[tokio::test]` for async |
| Config file | `Cargo.toml` per-crate (no separate config file) |
| Quick run command | `cargo test -p ironhermes-core skills 2>&1 | tail -20` |
| Full suite command | `cargo test --workspace 2>&1 | tail -40` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SKILL-01 | SkillRegistry scans three paths, returns skills, first-path-wins on conflict | unit | `cargo test -p ironhermes-core skill_registry` | ❌ Wave 0 |
| SKILL-01 | Scanner skips paths that don't exist (no panic) | unit | `cargo test -p ironhermes-core skill_registry_missing_paths` | ❌ Wave 0 |
| SKILL-02 | catalog_text() returns compact one-line-per-skill format | unit | `cargo test -p ironhermes-core catalog_text` | ❌ Wave 0 |
| SKILL-02 | PromptBuilder.build() includes skill catalog section | unit | `cargo test -p ironhermes-agent prompt_builder_skills` | ❌ Wave 0 |
| SKILL-03 | parse_skill_md() parses valid frontmatter + body | unit | `cargo test -p ironhermes-core parse_skill_md` | ❌ Wave 0 |
| SKILL-03 | parse_skill_md() handles missing frontmatter gracefully | unit | `cargo test -p ironhermes-core parse_skill_md_no_frontmatter` | ❌ Wave 0 |
| SKILL-04 | SkillsTool `list` action returns JSON with all skills | unit | `cargo test -p ironhermes-tools skills_tool_list` | ❌ Wave 0 |
| SKILL-04 | SkillsTool `view` action returns full SKILL.md content | unit | `cargo test -p ironhermes-tools skills_tool_view` | ❌ Wave 0 |
| SKILL-04 | SkillsTool `activate` action returns markdown body only | unit | `cargo test -p ironhermes-tools skills_tool_activate` | ❌ Wave 0 |
| SKILL-04 | SkillsTool unknown action returns error JSON | unit | `cargo test -p ironhermes-tools skills_tool_unknown_action` | ❌ Wave 0 |
| D-08/D-09 | Cron skill resolution prepends content, warns on missing | unit | `cargo test -p ironhermes-gateway cron_skill_resolution` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-core 2>&1 | tail -20` and `cargo test -p ironhermes-tools 2>&1 | tail -20`
- **Per wave merge:** `cargo test --workspace 2>&1 | tail -40`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-core/src/skills.rs` — SkillRegistry, SkillRecord, parse_skill_md tests
- [ ] `crates/ironhermes-tools/src/skills_tool.rs` — SkillsTool with list/view/activate tests
- [ ] `crates/ironhermes-agent/src/prompt_builder.rs` — extend with skill_registry field + tests
- [ ] `crates/ironhermes-gateway/src/runner.rs` — skill resolution at tick time + tests

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes | Skill names from tool args validated against registry (no path traversal); skill file paths constructed from trusted registry, not user input |
| V6 Cryptography | no | — |

### Known Threat Patterns for Skill Loading

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Path traversal via skill name | Tampering | Skill paths are resolved only from the SkillRegistry (pre-scanned trusted paths), never constructed from user-supplied skill names directly |
| Prompt injection via SKILL.md content | Tampering | Out of scope for Phase 7 — SKILL.md files are locally installed by the operator; same trust level as SOUL.md and AGENTS.md which have no scan |
| Skill name spoofing via `activate` args | Tampering | `activate` looks up by name in registry; returns error if not found; no filesystem access from the name string |

**Key insight:** The security surface is minimal because skill file paths are always resolved through the SkillRegistry (built from trusted scan paths at startup), never constructed directly from tool call arguments. This mirrors how read_file tool uses absolute paths checked against the registry.

---

## Sources

### Primary (HIGH confidence)
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-tools/src/registry.rs] — Tool trait, ToolRegistry, register_* pattern
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-tools/src/cronjob_tool.rs] — Action-based tool pattern to mirror for SkillsTool
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-agent/src/prompt_builder.rs] — Context injection pattern, set_memory_store, build() section order
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-core/src/config.rs] — serde_yaml usage, Config struct pattern
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-core/src/constants.rs] — get_hermes_home() path resolution
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-hooks/src/event.rs] — HookEventKind enum for optional skill activation events
- [VERIFIED: /Users/twilson/code/ironhermes/crates/ironhermes-cron/src/tick.rs] — Tick runner, CronJob fields including skills Vec<String>
- [VERIFIED: /Users/twilson/code/ironhermes/Cargo.toml] — workspace dependencies confirming serde_yaml 0.9, glob 0.3 available
- [VERIFIED: /Users/twilson/code/hermes-agent/skills/hermes-agent/SKILL.md] — agentskills.io SKILL.md format (YAML frontmatter with name, description, version, author, license, metadata + markdown body)
- [VERIFIED: /Users/twilson/code/hermes-agent/skills/creative/ascii-art/SKILL.md] — Second example SKILL.md confirming format
- [VERIFIED: .planning/phases/07-skills-system/07-CONTEXT.md] — All locked decisions

### Secondary (MEDIUM confidence)
- [VERIFIED: /Users/twilson/code/ironhermes/.planning/codebase/ARCH.md] — Crate dependency graph confirming no circular deps if SkillRegistry in ironhermes-core

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all crates verified in workspace Cargo.toml, no new dependencies needed
- Architecture: HIGH — patterns verified directly from existing Rust source (CronjobTool, PromptBuilder, ToolRegistry)
- Pitfalls: MEDIUM — derived from code review and known YAML parsing edge cases; not from observed runtime failures
- Cron wiring: MEDIUM — tick.rs verified, but exact gateway integration point (runner.rs) not fully read; planner should verify runner.rs structure

**Research date:** 2026-04-09
**Valid until:** 2026-05-09 (stable Rust codebase; no fast-moving external dependencies)
