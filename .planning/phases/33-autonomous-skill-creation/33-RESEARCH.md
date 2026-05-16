# Phase 33: Autonomous Skill Creation & Self-Improvement - Research

**Researched:** 2026-05-15
**Domain:** Rust agent tool implementation — skill_manage tool, trigger heuristic detection, SKILL.md scaffolding
**Confidence:** HIGH

## Summary

Phase 33 lands the agent-curated side of the Learning Loop: at task completion, the agent detects whether the work was worth documenting (via a 4-condition heuristic), then autonomously writes and maintains SKILL.md files via a new `skill_manage` tool. The tool follows the exact same action-dispatch pattern as the existing `memory` tool (Phase 17, `crates/ironhermes-tools/src/memory_tool.rs`) and integrates with the existing SkillRegistry discovery path — no new crate, no new registry discovery logic.

Three concrete implementation challenges define the phase:

1. **Trigger heuristic placement**: The heuristic fires post-run at the call site that processes `AgentResult`. The cleanest location is immediately after `agent.run(messages).await?` returns in the three platform wiring sites (CLI `run_agent_turn`, gateway `handle()`, cron runner). The heuristic reads `result.appended` to count tool calls and scans for error indicators. Since the trigger is behavioral guidance that causes the agent to call `skill_manage`, the preferred approach is a **system-prompt guidance block** that tells the agent to use `skill_manage` when it detects any trigger condition — this mirrors how memory persistence is handled (guidance in SOUL.md/prompt, not hardcoded code logic). A lightweight code-side counter can be included as metadata for honesty but is not load-bearing.

2. **skill_manage tool implementation**: A new `SkillManageTool` struct in `crates/ironhermes-tools/src/skill_manage.rs`, registered as toolset `"learning"`. The 6 actions (create/patch/edit/delete/write_file/remove_file) follow the `memory_tool.rs` pattern exactly: JSON action dispatch, `old_string`/`new_string` for `patch`, full content for `edit`, path-scoped operations for `write_file`/`remove_file`. The tool operates on `~/.ironhermes/skills/<category>/<slug>/SKILL.md` (using `get_hermes_home()` from `ironhermes-core::constants`).

3. **Self-created trust tier**: Phase 28 (SKILL-09) is deferred. Adding `SelfCreated` to `SkillSource` enum in this phase is the right call — it's a 5-line addition to `ironhermes-core::skills`. The enum is `#[derive(Copy, Clone, PartialEq, Eq)]` so adding a variant is non-breaking. Skills written by `skill_manage create` get `SkillSource::SelfCreated` in their frontmatter as the string `"Self-created"` via the `metadata.hermes` block. The SkillRegistry scans `~/.ironhermes/skills/` on next session start and will discover them automatically.

**Primary recommendation:** Implement in 3 plans — (1) `SkillSource::SelfCreated` + system-prompt trigger guidance, (2) `skill_manage` tool with 6 actions, (3) toolset registration + discovery verification + tests.

## Architectural Responsibility Map

| Capability | Primary Tier | Secondary Tier | Rationale |
|------------|-------------|----------------|-----------|
| Trigger heuristic evaluation | Agent behavioral guidance (system prompt) | Code-side post-run counter (supporting) | Agent decides; code can count tool calls as a metric but the decision to call skill_manage is the agent's |
| skill_manage tool dispatch | API / Tool layer (`ironhermes-tools`) | — | Same tier as `memory_tool`, `skills_tool`, `cronjob_tool` |
| SKILL.md file I/O | skill_manage tool (writes to `~/.ironhermes/skills/`) | — | Scoped file writes within HERMES_HOME |
| Security scanning of new skills | SkillRegistry load path | skill_manage pre-write scan | Scan happens at SkillRegistry load; pre-write scan in skill_manage is defense-in-depth |
| Skill discovery on next session | SkillRegistry (existing, no changes) | — | `load_with_config` already walks `~/.ironhermes/skills/`; new skills appear automatically |
| Self-created trust tier | `SkillSource` enum in `ironhermes-core::skills` | — | Source of truth for trust is the enum; string label in frontmatter is advisory only |
| Toolset registration | `toolset_cmd.rs` KNOWN_TOOLSETS + `toolset_session.rs` members_map | `config.rs` DEFAULT_TOOLSETS | Phase 25 pattern — add "learning" to all three lists |

## Standard Stack

### Core (no new crates required)

| Component | Location | Purpose | Why Standard |
|-----------|----------|---------|--------------|
| `async_trait` | workspace dep | `impl Tool for SkillManageTool` | All tools use this |
| `serde_json` | workspace dep | JSON action dispatch | All tools use this |
| `tokio` | workspace dep | async execute | All tools use this |
| `anyhow` | workspace dep | error handling | Project standard |
| `ironhermes_core::skills` | crate | `SkillSource::SelfCreated`, `parse_skill_md`, `scan_skill_content` | Existing skill infrastructure |
| `ironhermes_core::constants::get_hermes_home` | crate | resolve `~/.ironhermes/` | Established pattern |
| `ironhermes_core::context_scanner::scan_skill_content` | crate | pre-write threat scan | Same scanner used by SkillRegistry |

### New files to create

| File | Crate | Purpose |
|------|-------|---------|
| `crates/ironhermes-tools/src/skill_manage.rs` | ironhermes-tools | `SkillManageTool` struct + 6 actions |
| `crates/ironhermes-agent/tests/invariants_33.rs` | ironhermes-agent | static-grep regression tests |

### Alternatives Considered

| Instead of | Could Use | Tradeoff |
|------------|-----------|----------|
| System-prompt guidance for trigger | Code-side AgentResult analysis | Code analysis can count tool calls but cannot detect "non-obvious workflow" — agent judgment needed; guidance approach matches Phase 32 nudge pattern |
| SkillSource::SelfCreated in enum | String in frontmatter only | Enum variant enables type-safe dispatch in scan enforcement; Phase 28 builds on it |
| Tool registered in `ironhermes-tools` | Intercept in agent_loop.rs | Tools belong in tools crate; intercept pattern only for cross-crate handles (memory_manager, state_store) |

**Installation:** No new cargo deps. All required crates are already workspace members.

## Package Legitimacy Audit

> No new external packages introduced in this phase. All implementation uses existing workspace dependencies.

**Packages removed due to slopcheck [SLOP] verdict:** none
**Packages flagged as suspicious [SUS]:** none

## Architecture Patterns

### System Architecture Diagram

```
User message → AgentLoop.run() → [tool calls tracked in result.appended]
                                          ↓
                              Natural completion (StopReason::Natural)
                                          ↓
                 System-prompt trigger guidance fires agent judgment:
                 "Did this task hit any trigger condition?"
                   (a) count tool_calls in result.appended >= 5
                   (b) any tool result contained "[BLOCKED:" or "Error:"
                   (c) user message contained correction language
                   (d) agent recognizes non-obvious workflow
                                          ↓ trigger fires
                         Agent calls skill_manage(action="create", ...)
                                          ↓
                         SkillManageTool.execute()
                           - validate slug (validate_skill_name)
                           - security scan content (scan_skill_content)
                           - resolve path: get_hermes_home()/skills/<category>/<slug>/SKILL.md
                           - fs::create_dir_all + fs::write
                                          ↓
                         Next session: SkillRegistry.load_with_config()
                         walks ~/.ironhermes/skills/ → discovers new skill
                         → appears in skill index with source=SelfCreated
```

### Recommended Project Structure

```
crates/ironhermes-tools/src/
├── skill_manage.rs          # NEW: SkillManageTool (6 actions)
├── memory_tool.rs           # REFERENCE pattern to replicate
└── lib.rs                   # ADD: pub mod skill_manage;

crates/ironhermes-core/src/
└── skills.rs                # MODIFY: add SkillSource::SelfCreated variant

crates/ironhermes-cli/src/
└── toolset_cmd.rs           # MODIFY: add "learning" to KNOWN_TOOLSETS + members_map

crates/ironhermes-agent/tests/
└── invariants_33.rs         # NEW: static-grep gates

~/.ironhermes/skills/        # RUNTIME: where self-created skills land
└── <category>/
    └── <slug>/
        └── SKILL.md
```

### Pattern 1: Tool Action Dispatch (mirror of memory_tool.rs)

```rust
// Source: crates/ironhermes-tools/src/memory_tool.rs (Phase 17 pattern)
#[async_trait]
impl Tool for SkillManageTool {
    fn name(&self) -> &str { "skill_manage" }
    fn toolset(&self) -> &str { "learning" }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args.get("action").and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter 'action'"))?;
        match action {
            "create" => self.action_create(&args).await,
            "patch"  => self.action_patch(&args).await,
            "edit"   => self.action_edit(&args).await,
            "delete" => self.action_delete(&args).await,
            "write_file"  => self.action_write_file(&args).await,
            "remove_file" => self.action_remove_file(&args).await,
            other => Err(anyhow::anyhow!(
                "Unknown action '{}'. Valid: create, patch, edit, delete, write_file, remove_file",
                other
            )),
        }
    }
}
```

### Pattern 2: patch Action — Substring Replace (mirror of MEM-03)

```rust
// Source: memory_tool.rs replace action; old_text/new_content pattern
// For skill_manage patch: old_string/new_string in the SKILL.md file
async fn action_patch(&self, args: &serde_json::Value) -> anyhow::Result<String> {
    let slug = required_str(args, "name")?;
    let category = required_str(args, "category")?;
    let old_string = required_str(args, "old_string")?;
    let new_string = required_str(args, "new_string")?;

    let path = self.skill_path(category, slug);
    let content = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("skill '{}' not found: {}", slug, e))?;

    if !content.contains(old_string) {
        return Ok(format!("{{\"error\":\"not_found\",\"reason\":\"old_string not found in {}/SKILL.md\"}}", slug));
    }
    // Single replace — same semantics as memory replace
    let new_content = content.replacen(old_string, new_string, 1);

    // Security scan new content before writing (defense-in-depth)
    let scan = ironhermes_core::context_scanner::scan_skill_content(&new_content, &path.display().to_string());
    if scan.starts_with("[BLOCKED:") {
        return Ok(format!("{{\"error\":\"content_rejected\",\"reason\":\"injection_pattern_detected\"}}"));
    }
    std::fs::write(&path, new_content)?;
    Ok(format!("Patched {}/SKILL.md", slug))
}
```

### Pattern 3: create Action — SKILL.md Scaffold

```rust
// Source: agentskills.io specification + existing hexapod SKILL.md example
async fn action_create(&self, args: &serde_json::Value) -> anyhow::Result<String> {
    let slug = required_str(args, "name")?; // validated via validate_skill_name
    let category = required_str(args, "category")?;
    let description = required_str(args, "description")?;
    let tags: Vec<String> = args.get("tags")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let content_body = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    // Validate name per agentskills.io spec
    ironhermes_core::skills::validate_skill_name_pub(slug)?;

    let frontmatter = format!(
        "---\nname: {}\ndescription: {}\nversion: 1.0.0\nmetadata:\n  hermes:\n    tags: {}\n    category: {}\n    trust_tier: Self-created\n---\n",
        slug, description,
        if tags.is_empty() { "[]".to_string() } else { format!("[{}]", tags.iter().map(|t| format!("\"{}\"", t)).collect::<Vec<_>>().join(", ")) },
        category
    );
    let full_content = format!("{}{}", frontmatter, content_body);

    // Security scan before write
    let scan = ironhermes_core::context_scanner::scan_skill_content(&full_content, slug);
    if scan.starts_with("[BLOCKED:") {
        return Ok("{\"error\":\"content_rejected\",\"reason\":\"injection_pattern_detected\"}".to_string());
    }

    let dir = self.skill_dir(category, slug);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("SKILL.md");
    if path.exists() {
        return Ok(format!("{{\"error\":\"already_exists\",\"reason\":\"use patch or edit to update {}\"}}", slug));
    }
    std::fs::write(&path, full_content)?;
    Ok(format!("Created skill '{}' at {}", slug, path.display()))
}
```

### Pattern 4: write_file / remove_file Security Boundary

```rust
// Security: all paths scoped to get_hermes_home()/skills/<category>/<slug>/
fn skill_dir(&self, category: &str, slug: &str) -> PathBuf {
    get_hermes_home().join("skills").join(category).join(slug)
}

fn resolve_skill_file_path(&self, category: &str, slug: &str, rel_path: &str) -> anyhow::Result<PathBuf> {
    let base = self.skill_dir(category, slug);
    // Reject any path traversal: ../  or absolute paths
    if rel_path.contains("..") || rel_path.starts_with('/') {
        anyhow::bail!("path traversal rejected: '{}'", rel_path);
    }
    Ok(base.join(rel_path))
}
```

### Pattern 5: Tool JSON Schema (mirrors memory_tool.rs schema())

```rust
// Source: memory_tool.rs schema() pattern
fn schema(&self) -> ToolSchema {
    ToolSchema::new("skill_manage", self.description(), json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["create", "patch", "edit", "delete", "write_file", "remove_file"],
                "description": "Action to perform. Prefer 'patch' for updates (token-efficient substring replace). Use 'edit' only for full rewrites."
            },
            "name": {
                "type": "string",
                "description": "Skill slug: lowercase letters, numbers, hyphens only (e.g. 'git-workflow'). Required for all actions."
            },
            "category": {
                "type": "string",
                "description": "Skill category subdirectory (e.g. 'development', 'automation', 'data'). Required for all actions."
            },
            "description": { "type": "string", "description": "Skill description (required for 'create')." },
            "content": { "type": "string", "description": "Full SKILL.md body for 'create' or 'edit' actions." },
            "old_string": { "type": "string", "description": "Unique substring to replace (required for 'patch')." },
            "new_string": { "type": "string", "description": "Replacement text (required for 'patch')." },
            "tags": { "type": "array", "items": {"type": "string"}, "description": "Tags for the skill (used in 'create')." },
            "file_path": { "type": "string", "description": "Relative file path within skill directory (required for 'write_file'/'remove_file')." },
            "file_content": { "type": "string", "description": "File content (required for 'write_file')." }
        },
        "required": ["action", "name", "category"]
    }))
}
```

### Pattern 6: System-Prompt Trigger Guidance

The trigger guidance lives in the system prompt as behavioral instructions (the same layer as memory guidance in SOUL.md). It does NOT require a new code hook — it is text that the agent reads:

```
## Skill Creation (Learning Loop)

After completing a task, evaluate whether the approach is worth documenting.
Write a SKILL.md if ANY of these conditions is true:
- You made 5 or more tool calls to complete the task
- You recovered from a tool error or unexpected result during the task
- The user corrected your approach mid-task
- You discovered a non-obvious workflow that worked well

When creating a skill, call `skill_manage(action="create", ...)`. For subsequent
improvements to an existing skill, prefer `skill_manage(action="patch", ...)` —
pass only the changed substring, not the full file. Full rewrites use `action="edit"`.

Self-created skills appear in your skill index next session. Choose a descriptive
category (e.g. "development", "automation", "data", "research") and a kebab-case name.
```

This guidance is injected as part of the system prompt when the `learning` toolset is enabled and `skill_manage` is registered.

### Anti-Patterns to Avoid

- **Hardcoded tool-call counting in AgentResult**: The `result.appended` slice can be iterated to count `role == Assistant && tool_calls.is_some()`, but this only detects condition (a). Conditions (c) and (d) require agent judgment — use system-prompt guidance as the primary mechanism.
- **Network call at SkillRegistry load time**: The existing registry explicitly avoids network calls (`skills-sh` source stays Community because "doing so would require a network call at registry-load time"). Self-created skills must write to disk; no network validation at load.
- **Writing skills outside get_hermes_home()/skills/**: Path traversal protection is mandatory on `write_file` and `remove_file`. Use `canonicalize` after join and verify the canonical path starts with the skill dir prefix.
- **Skipping security scan on create**: Even self-created content must pass `scan_skill_content` before write. The agent could be manipulated via an injected user message to create a malicious skill.
- **Adding SelfCreated to SkillSource without updating the D-15 scan enforcement match**: The `match source` block in `try_register_skill_from_dir` (skills.rs:578-596) must handle `SelfCreated` — treat same as `Builtin` (WARN-BUT-LOAD, not hard-reject).

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| SKILL.md name validation | Custom regex | `validate_skill_name()` in `ironhermes-core::skills` | Already implements agentskills.io spec exactly |
| Security scanning | Custom pattern match | `scan_skill_content()` in `ironhermes-core::context_scanner` | Existing scanner covers SKILL_THREAT_PATTERNS + THREAT_PATTERNS |
| Home directory resolution | `std::env::var("HOME")` | `get_hermes_home()` in `ironhermes-core::constants` | Handles `IRONHERMES_HOME` override + fallback |
| SKILL.md parsing | Custom YAML parser | `parse_skill_md()` in `ironhermes-core::skills` | Already handles frontmatter normalization, name validation |
| Tool schema definition | Custom JSON | `ToolSchema::new()` + `serde_json::json!()` | Established workspace pattern |

**Key insight:** Every building block already exists. Phase 33 is primarily wiring — a new tool that uses existing validators, scanners, and path helpers.

## Common Pitfalls

### Pitfall 1: validate_skill_name is private
**What goes wrong:** `validate_skill_name` in `skills.rs` is `fn` (not `pub fn`). SkillManageTool cannot call it directly from a different crate.
**Why it happens:** It was written for internal use by `parse_skill_md`.
**How to avoid:** Either (a) make `validate_skill_name` pub in this phase, or (b) use `parse_skill_md` as an indirect validator by constructing a minimal SKILL.md string and parsing it. Option (a) is cleaner — add `pub(crate)` or `pub` with a doc comment.
**Warning signs:** Compiler error "function `validate_skill_name` is private" when implementing SkillManageTool.

### Pitfall 2: SkillSource::SelfCreated must be added to ALL match arms
**What goes wrong:** Adding `SelfCreated` to the `SkillSource` enum causes exhaustive-match compile errors in `try_register_skill_from_dir` (3 match arms), `resolve_source` (return values), and any test that matches on `SkillSource`.
**Why it happens:** `SkillSource` derives `Copy, Clone, PartialEq, Eq` — adding a variant requires updating all match sites.
**How to avoid:** Search for all `SkillSource::` occurrences before writing. The scan enforcement match at `skills.rs:580-596` must add `SkillSource::SelfCreated` alongside `Builtin | Official | Trusted` in the WARN-BUT-LOAD arm (not the hard-reject arm).
**Warning signs:** `non-exhaustive patterns` compile error.

### Pitfall 3: Directory layout must be two-level (category/slug/) not flat
**What goes wrong:** Writing to `~/.ironhermes/skills/<slug>/SKILL.md` (one level) works for legacy layout but not for the Phase 21.8 installer layout that `load_with_paths` now traverses.
**Why it happens:** `load_with_paths` checks `<root>/<dir>/SKILL.md` (level 1) AND `<root>/<category>/<subdir>/SKILL.md` (level 2). Self-created skills should use two-level layout since they have a category.
**How to avoid:** Always write to `get_hermes_home()/skills/<category>/<slug>/SKILL.md`. The SkillRegistry will discover it at level 2.
**Warning signs:** Skill not appearing in next session's skill index.

### Pitfall 4: patch with old_string that appears multiple times
**What goes wrong:** `content.replacen(old_string, new_string, 1)` replaces only the first occurrence. If the old_string appears multiple times (e.g., a common word), the wrong instance is replaced.
**Why it happens:** Same issue exists in the memory tool's `replace` action. The fix is the same: the schema description should instruct the agent to use a unique substring as `old_string`.
**How to avoid:** Schema description: "Must be a unique substring — include enough surrounding context to identify exactly one location."
**Warning signs:** Agent complains that patch changed the wrong section.

### Pitfall 5: KNOWN_TOOLSETS count test will fail if "learning" is not added
**What goes wrong:** `toolset_cmd.rs` has a test asserting `KNOWN_TOOLSETS.len() == 7` exactly. Adding "learning" without updating this assertion breaks the test.
**Why it happens:** The test was written to catch silent additions. See test at line ~479: `assert_eq!(KNOWN_TOOLSETS.len(), 7, ...)`.
**How to avoid:** When adding "learning" to KNOWN_TOOLSETS, update the count assertion to 8. Also add "learning" to `toolset_members_map()` and `DEFAULT_TOOLSETS` in `constants.rs`.
**Warning signs:** `assertion failed: KNOWN_TOOLSETS.len() == 7`.

### Pitfall 6: Security scan must run BEFORE write, not after
**What goes wrong:** Writing first then scanning leaves a window where a malicious SKILL.md exists on disk. If the scanner blocks it, deletion may fail or race.
**Why it happens:** Temptation to "check after" since SkillRegistry scans on load anyway.
**How to avoid:** Always: (1) construct full content string, (2) scan_skill_content, (3) if blocked return error, (4) only then write to disk. This is defense-in-depth — the SkillRegistry scan on load is the second gate, not the first.

### Pitfall 7: robotics toolset is missing from KNOWN_TOOLSETS in toolset_cmd.rs
**What goes wrong:** The existing `KNOWN_TOOLSETS` in `toolset_cmd.rs` has 7 entries but does NOT include "robotics" — only `DEFAULT_TOOLSETS` in `constants.rs` does. The count test and toolset_cmd tests may not catch "learning" additions correctly if you assume KNOWN_TOOLSETS == DEFAULT_TOOLSETS.
**Why it happens:** The two lists serve different purposes: KNOWN_TOOLSETS (toolset_cmd.rs) is for CLI enable/disable validation; DEFAULT_TOOLSETS (constants.rs) is for toolsets enabled on fresh install. They are not required to be identical.
**How to avoid:** Add "learning" to KNOWN_TOOLSETS in toolset_cmd.rs AND to the members_map. Separately decide whether "learning" should be in DEFAULT_TOOLSETS (it should — skill_manage has no external prerequisites, same as "memory" and "session").

## Code Examples

Verified patterns from the actual codebase:

### Existing SkillSource enum (ironhermes-core/src/skills.rs:117-128)
```rust
// Source: crates/ironhermes-core/src/skills.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    Builtin,
    Official,
    Trusted,
    Community,
    // ADD IN PHASE 33:
    // SelfCreated,
}

impl Default for SkillSource {
    fn default() -> Self { SkillSource::Builtin }
}
```

### Existing scan enforcement match (skills.rs:578-596) — must add SelfCreated arm
```rust
// Source: crates/ironhermes-core/src/skills.rs try_register_skill_from_dir
if scan_result.starts_with("[BLOCKED:") {
    match source {
        SkillSource::Community => {
            warn!("hard-rejecting community skill — scan hit");
            return; // D-15 community hard-reject
        }
        SkillSource::Builtin | SkillSource::Official | SkillSource::Trusted => {
            warn!("WARN-BUT-LOAD — scan hit on builtin/official/trusted skill");
            // proceed — D-15 WARN-BUT-LOAD
        }
        // Phase 33: SelfCreated — WARN-BUT-LOAD (agent-authored, not untrusted external)
        // SkillSource::SelfCreated => { warn!(...); }
    }
}
```

### Tool registration pattern (from register_defaults in registry.rs)
```rust
// Source: crates/ironhermes-tools/src/registry.rs — add to register_defaults()
// or wire via with_intercepts() builder in agent_loop.rs
registry.register(Box::new(SkillManageTool::new()));
```

### get_hermes_home usage
```rust
// Source: crates/ironhermes-core/src/constants.rs:57-66
use ironhermes_core::constants::get_hermes_home;
let skill_path = get_hermes_home()
    .join("skills")
    .join(category)
    .join(slug)
    .join("SKILL.md");
```

### Existing agentskills.io SKILL.md format (from hexapod skill — verified)
```yaml
---
name: hexapod
description: Protocol reference and action guide for the Freenove hexapod robot...
version: 1.0.0
metadata:
  hermes:
    requires_toolsets: [robotics]
    tags: [robotics, hexapod, freenove, tcp, video]
---
```

Self-created skill format adds `trust_tier: Self-created` and `category` to the hermes block:
```yaml
---
name: git-workflow
description: Step-by-step workflow for resolving merge conflicts using interactive rebase. Use when handling git merge conflicts or when the user asks about rebasing.
version: 1.0.0
metadata:
  hermes:
    tags: [git, workflow, vcs]
    category: development
    trust_tier: Self-created
---
```

Note: `metadata.hermes.tags` is stored in `HermesMetadata.extras` (the catch-all `#[serde(flatten)]` field) — it is NOT a typed field today. `trust_tier` and `category` are also extras. This is correct — unknown hermes fields are preserved per D-18 WARN-BUT-LOAD design.

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Skills only come from installed/discovered files | Agent writes skills via tool call | Phase 33 (this phase) | Closes the self-improvement loop |
| SkillSource: Builtin/Official/Trusted/Community | + SelfCreated | Phase 33 (Phase 28 deferred) | Phase 28 builds on this; no blocking dep |
| Memory tool as only agent-facing write tool | memory + skill_manage | Phase 33 | Second write tool follows same action-dispatch pattern |

**Deprecated/outdated:**
- Manual skill creation only: After Phase 33, the agent can author skills autonomously. The manual path remains valid.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `validate_skill_name` needs to be made `pub` (currently private) | Architecture Patterns / Pitfall 1 | If it's already pub-accessible, no change needed |
| A2 | System-prompt guidance is preferred over code-side trigger counter | Architecture Patterns / Pattern 6 | If the user wants pure code-side triggering, the prompt guidance approach would need a post-run injection site instead |
| A3 | `SelfCreated` trust tier is added in Phase 33, not Phase 28 | Standard Stack / Architecture | Phase 28 could conflict if it adds its own variant; coordinate naming |
| A4 | "learning" should be in DEFAULT_TOOLSETS (enabled by default on fresh install) | Common Pitfalls 7 | If learning is opt-in only, add to KNOWN_TOOLSETS but not DEFAULT_TOOLSETS |
| A5 | `metadata.hermes.trust_tier` as a string field in frontmatter is sufficient for Phase 33 | Code Examples | If SkillRegistry needs to read trust_tier at load time for enforcement, a typed field would be needed |

## Open Questions

1. **Where exactly does trigger guidance live in the system prompt?**
   - What we know: Phase 32 injects nudge guidance via `config.learning.periodic_nudge_interval_seconds` — there's a `learning:` config block already in `cli-config.yaml.example`. The nudge fires as a system message injection (from cron runner).
   - What's unclear: Does skill creation guidance go in the same `learning:` config-controlled block, or is it always injected when `skill_manage` is available?
   - Recommendation: Add a `skill_creation_guidance` flag under `config.learning`. Default true. Inject the guidance block into the system prompt when `skill_manage` is registered and the flag is enabled. This follows the "progressive disclosure" philosophy.

2. **Should skill_manage be an intercepted tool or a registry tool?**
   - What we know: Memory tool is a registered tool. Intercepted tools (session_search, delegate_task) require cross-crate handles. skill_manage needs only `get_hermes_home()` and `scan_skill_content` — both accessible from `ironhermes-core` without a special handle.
   - What's unclear: Nothing — this is clearly a registered tool, not intercepted.
   - Recommendation: Register as a normal `Box<dyn Tool>` in `register_defaults()`, same as memory_tool.

3. **Does write_file/remove_file need to support arbitrary files in the skill directory?**
   - What we know: LEARN-05 explicitly lists `write_file` and `remove_file` as actions. The agentskills.io spec shows skills can have `scripts/`, `references/`, `assets/` subdirectories.
   - What's unclear: Should write_file be restricted to only SKILL.md? Or can the agent write `references/setup.md` too?
   - Recommendation: Allow any file within `<skill_dir>/` with path traversal protection. The agent needs this to create companion files referenced from SKILL.md.

## Environment Availability

> This phase is code-only — no external tools, services, or databases are required beyond the existing workspace.

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust 2024 edition | all crates | ✓ | workspace | — |
| `~/.ironhermes/skills/` dir | skill_manage write path | ✓ (created by `ensure_home_dirs()`) | — | skill_manage creates it with `create_dir_all` |
| `scan_skill_content` function | pre-write scan | ✓ | ironhermes-core | — |
| `get_hermes_home()` | path resolution | ✓ | ironhermes-core::constants | — |

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | Cargo workspace |
| Quick run command | `cargo test -p ironhermes-tools skill_manage -- --nocapture` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| LEARN-03 | Trigger heuristic fires on 5+ tool calls | unit | `cargo test -p ironhermes-agent test_trigger_heuristic` | ❌ Wave 0 |
| LEARN-03 | Trigger fires on error recovery signal | unit | `cargo test -p ironhermes-agent test_trigger_error_recovery` | ❌ Wave 0 |
| LEARN-04 | Created SKILL.md has valid frontmatter with Self-created tier | unit | `cargo test -p ironhermes-tools test_skill_manage_create_frontmatter` | ❌ Wave 0 |
| LEARN-04 | Created skill discovered by SkillRegistry on next load | integration | `cargo test -p ironhermes-core test_skill_registry_discovers_self_created` | ❌ Wave 0 |
| LEARN-05 | patch action replaces substring without full rewrite | unit | `cargo test -p ironhermes-tools test_skill_manage_patch` | ❌ Wave 0 |
| LEARN-05 | All 6 actions exposed in JSON schema | unit | `cargo test -p ironhermes-tools test_skill_manage_schema_actions` | ❌ Wave 0 |
| LEARN-05 | write_file scoped to skill dir (traversal rejected) | unit | `cargo test -p ironhermes-tools test_skill_manage_path_traversal_rejected` | ❌ Wave 0 |
| LEARN-05 | Security scan blocks injected content in create | unit | `cargo test -p ironhermes-tools test_skill_manage_create_blocked_content` | ❌ Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test -p ironhermes-tools -- skill_manage`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full suite green before `/gsd:verify-work`

### Wave 0 Gaps
- [ ] `crates/ironhermes-tools/src/skill_manage.rs` — new tool (covers LEARN-05)
- [ ] `crates/ironhermes-agent/tests/invariants_33.rs` — static-grep gates
- [ ] `crates/ironhermes-core/src/skills.rs` — add `SkillSource::SelfCreated` (covers LEARN-04 trust tier)

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | n/a — tool runs within authenticated agent session |
| V3 Session Management | no | n/a |
| V4 Access Control | yes | Path traversal protection on write_file/remove_file; operations scoped to HERMES_HOME |
| V5 Input Validation | yes | `validate_skill_name` for slug; `scan_skill_content` for content; `parse_skill_md` for frontmatter |
| V6 Cryptography | no | No secrets in skill content |

### Known Threat Patterns for skill_manage

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Path traversal via `write_file` `file_path` | Tampering | Reject paths containing `..` or starting with `/`; verify canonical path starts with skill_dir |
| Prompt injection embedded in SKILL.md content | Tampering | `scan_skill_content()` pre-write (SKILL_THREAT_PATTERNS + THREAT_PATTERNS) |
| Privilege escalation via `allowed-tools` in SKILL.md | Elevation | `scan_skill_content()` catches `allowed-tools` privilege escalation pattern (existing test in context_scanner.rs) |
| Overwrite arbitrary files via category traversal | Tampering | Category name validated same as slug (no `/`, no `..`); path built from `get_hermes_home().join(validated_category).join(validated_slug)` |
| delete action removing non-skill-managed files | Tampering | delete only removes `<skill_dir>/SKILL.md` and the slug directory; validate that path is within `~/.ironhermes/skills/` |

## Sources

### Primary (HIGH confidence)
- Codebase: `crates/ironhermes-tools/src/memory_tool.rs` — action-dispatch pattern verified directly
- Codebase: `crates/ironhermes-core/src/skills.rs` — SkillSource enum, validate_skill_name, scan enforcement match, SkillRegistry.load_with_paths
- Codebase: `crates/ironhermes-agent/src/agent_loop.rs` — AgentResult, run() structure, execute_tool_call pattern
- Codebase: `crates/ironhermes-core/src/constants.rs` — DEFAULT_TOOLSETS, get_hermes_home
- Codebase: `crates/ironhermes-cli/src/toolset_cmd.rs` — KNOWN_TOOLSETS (7 entries), toolset_members_map
- Codebase: `skills/hexapod/SKILL.md` — verified real-world SKILL.md format with metadata.hermes block
- Codebase: `cli-config.yaml.example` — learning: config section already exists
- [agentskills.io/specification](https://agentskills.io/specification) — verified SKILL.md frontmatter fields, name validation rules, metadata structure

### Secondary (MEDIUM confidence)
- REQUIREMENTS.md LEARN-03/04/05 — requirement text used as specification
- ROADMAP.md Phase 33 section — success criteria confirmed

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — no new deps; all crates already in workspace
- Architecture: HIGH — memory_tool.rs pattern verified; SkillRegistry discovery verified; agentskills.io spec verified
- Pitfalls: HIGH — most pitfalls derived from reading actual code (private fn, exhaustive match, KNOWN_TOOLSETS count test)
- Trigger heuristic approach: MEDIUM — system-prompt guidance is the right approach per hermes-agent philosophy, but the exact injection site for the guidance text is not yet determined (Open Question 1)

**Research date:** 2026-05-15
**Valid until:** 2026-06-15 (stable Rust codebase; agentskills.io spec is stable)
