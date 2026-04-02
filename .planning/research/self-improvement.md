# Self-Improving Agent Architectures

**Project:** IronHermes
**Researched:** 2026-04-01
**Overall confidence:** HIGH (primary source is hermes-agent codebase analysis)
**Research mode:** Ecosystem + Feasibility

---

## Executive Summary

The hermes-agent "self-improvement" system is not a single mechanism but three complementary systems layered into the prompt assembly pipeline:

1. **SOUL.md** -- The agent's personality/identity, loaded as slot #1 in the system prompt. The agent can edit this file with its file tools to refine its own personality over time. This is the simplest and most powerful form of self-modification: the agent literally rewrites who it is.

2. **Memory (MEMORY.md / USER.md)** -- Bounded, curated declarative memory persisted as flat files. The agent uses a dedicated `memory` tool to add/replace/remove entries. These are injected as frozen snapshots at session start. This is self-improvement through accumulating knowledge about the user and environment.

3. **Skills (SKILL.md files)** -- Procedural memory organized as a directory tree of markdown files. The agent creates, patches, and deletes skills via `skill_manage`. Skills are the agent's learned procedures -- "how to do X" captured from successful task completions. This is self-improvement through accumulating reusable approaches.

All three feed into the system prompt through `_build_system_prompt()` in `run_agent.py`, which assembles a 10-layer prompt that is cached for the duration of a session. The frozen-snapshot pattern is critical: mid-session writes update disk state but do not mutate the running prompt, preserving LLM prefix cache stability.

IronHermes already has a basic `PromptBuilder` with `load_context_files()` that reads SOUL.md/AGENTS.md from the working directory. It needs to be extended with: (a) HERMES_HOME-based SOUL.md loading, (b) a memory subsystem, (c) a skills subsystem, and (d) the security scanning layer that prevents prompt injection via context files.

---

## 1. How hermes-agent Does Self-Improvement

### 1.1 Three Self-Modification Channels

| Channel | What Changes | How Agent Modifies It | Where It Appears in Prompt | Persistence |
|---------|-------------|----------------------|---------------------------|-------------|
| SOUL.md | Identity/personality | File tools (read_file, write_file, patch) | Slot #1 (replaces DEFAULT_AGENT_IDENTITY) | `$HERMES_HOME/SOUL.md` |
| MEMORY.md / USER.md | Declarative facts | Dedicated `memory` tool (add/replace/remove) | Slots #5-6 (frozen snapshot) | `$HERMES_HOME/memories/` |
| Skills | Procedural knowledge | `skill_manage` tool (create/patch/edit/delete) | Slot #7 (skills index) + on-demand via `skill_view` | `$HERMES_HOME/skills/` |

### 1.2 Prompt Assembly Order (10 Layers)

From `run_agent.py::_build_system_prompt()` and `agent/prompt_builder.py`:

```
1. Agent identity       -- SOUL.md content, or DEFAULT_AGENT_IDENTITY fallback
2. Behavior guidance    -- MEMORY_GUIDANCE, SESSION_SEARCH_GUIDANCE, SKILLS_GUIDANCE
3. Honcho static block  -- External user-modeling layer (optional)
4. System message       -- User-configured override
5. MEMORY snapshot      -- Frozen at session start from MEMORY.md
6. USER snapshot        -- Frozen at session start from USER.md
7. Skills index         -- Compact list of available skills with descriptions
8. Context files        -- AGENTS.md / .hermes.md / CLAUDE.md / .cursorrules
9. Timestamp + session  -- Current time, session ID
10. Platform hint       -- CLI/Telegram/Discord formatting guidance
```

Key design principle: **layers 1-10 are frozen for the entire session**. The system prompt is built once (or rebuilt only after context compression) and cached on `self._cached_system_prompt`. This is critical for Anthropic prompt caching, which dramatically reduces costs.

### 1.3 SOUL.md -- Identity Self-Modification

**Loading** (`prompt_builder.py::load_soul_md()`):
- Reads from `$HERMES_HOME/SOUL.md` (NOT the working directory)
- Security-scanned for prompt injection patterns
- Truncated at 20,000 chars (70% head, 20% tail)
- If present and non-empty, replaces DEFAULT_AGENT_IDENTITY entirely
- `build_context_files_prompt()` is then called with `skip_soul=True` to prevent duplication

**Self-modification path**: The agent uses its standard file tools (read_file, write_file, patch) to edit `$HERMES_HOME/SOUL.md`. There is no special tool for this. The change takes effect on the next session start.

**Default content** (`hermes_cli/default_soul.py`):
```
You are Hermes Agent, an intelligent AI assistant created by Nous Research.
You are helpful, knowledgeable, and direct. ...
```

This is seeded on first run and never overwritten by the system. The user (or the agent itself) owns SOUL.md from that point forward.

### 1.4 Memory -- Declarative Self-Improvement

**Architecture** (`tools/memory_tool.py::MemoryStore`):
- Two parallel stores: `memory` (agent notes) and `user` (user profile)
- Character-limited: 2,200 chars for memory, 1,375 chars for user
- Entry delimiter: `\n[section-sign]\n` (the section sign character)
- Operations: add, replace (substring match), remove (substring match)
- Atomic file writes using temp-file + `os.replace()`
- File locking with `fcntl.flock()` for concurrent session safety
- Security scanning blocks injection/exfiltration attempts in memory content

**Frozen snapshot pattern**:
- `load_from_disk()` captures `_system_prompt_snapshot` at session start
- `format_for_system_prompt()` always returns the frozen snapshot, never live state
- Tool responses show live state so the agent sees what it just wrote
- Changes persist to disk immediately but prompt updates on next session

**Guidance in prompt** (from `MEMORY_GUIDANCE`):
```
Prioritize what reduces future user steering -- the most valuable memory is one
that prevents the user from having to correct or remind you again.
```

This is the key self-improvement heuristic: memory should make the agent better at serving this specific user over time.

### 1.5 Skills -- Procedural Self-Improvement

**Architecture** (`tools/skill_manager_tool.py` + `tools/skills_tool.py`):
- Skills are directories under `$HERMES_HOME/skills/`
- Each skill has a `SKILL.md` with YAML frontmatter (name, description, platforms) + markdown body
- Supporting files in `references/`, `templates/`, `scripts/`, `assets/` subdirectories
- Skills index is built by `build_skills_system_prompt()` with two-layer cache (in-process LRU + disk snapshot)

**Self-modification operations**:
- `create` -- New skill from successful task completion
- `patch` -- Targeted find-and-replace within SKILL.md (preferred for fixes)
- `edit` -- Full rewrite of SKILL.md (major overhauls)
- `delete` -- Remove a skill
- `write_file` / `remove_file` -- Manage supporting files

**Trigger conditions** (from `SKILLS_GUIDANCE`):
```
After completing a complex task (5+ tool calls), fixing a tricky error,
or discovering a non-trivial workflow, save the approach as a skill.
When using a skill and finding it outdated, incomplete, or wrong,
patch it immediately -- don't wait to be asked.
```

**Security**: All skill operations are security-scanned by `skills_guard.py`. Agent-created skills get the same scrutiny as community-installed ones. Failed scans trigger automatic rollback.

### 1.6 What hermes-agent Does NOT Have

- **No explicit reflection loop** -- The agent does not periodically evaluate its own performance. Self-improvement is opportunistic, triggered by task completion or user correction.
- **No A/B testing of prompts** -- The agent cannot try two versions of SOUL.md and measure which performs better.
- **No versioning of context files** -- There is no git-like history for SOUL.md, MEMORY.md, or skills. Edits are destructive (though skills have atomic writes with rollback on security failure).
- **No automated rollback** -- If the agent writes a bad SOUL.md, it stays bad until manually fixed or the agent fixes it in a future session.
- **No cross-session performance metrics** -- The `InsightsEngine` tracks token usage, tool calls, and costs, but does not measure "how well did I do?" in any qualitative sense.

---

## 2. Context File Architecture Patterns

### 2.1 The hermes-agent Layered Model

hermes-agent uses a clear separation of concerns across context files:

| File | Scope | Mutability | Content Type |
|------|-------|-----------|--------------|
| SOUL.md | Global (per instance) | Agent-editable | Identity, personality, tone |
| MEMORY.md | Global (per instance) | Agent-editable (memory tool) | Environment facts, learned conventions |
| USER.md | Global (per instance) | Agent-editable (memory tool) | User preferences, communication style |
| AGENTS.md | Per-project (working dir) | User-editable | Project architecture, conventions, tooling |
| .hermes.md | Per-project (walks to git root) | User-editable | Project instructions (highest priority) |
| CLAUDE.md | Per-project (working dir) | User-editable | Claude Code compatibility |
| Skills | Global (per instance) | Agent-editable (skill_manage tool) | Procedural knowledge, how-to guides |

**Priority system for project context** (first match wins):
1. `.hermes.md` / `HERMES.md`
2. `AGENTS.md`
3. `CLAUDE.md`
4. `.cursorrules` / `.cursor/rules/*.mdc`

Only ONE project context type is loaded. SOUL.md is always loaded independently.

### 2.2 File Format Conventions

**SOUL.md**: Pure markdown, no frontmatter. Content is injected verbatim as the agent identity.

**MEMORY.md / USER.md**: Entries delimited by `\n[section-sign]\n`. Each entry is a compact text string. Header shows usage percentage and character counts. No frontmatter.

**SKILL.md**: YAML frontmatter (required: `name`, `description`; optional: `platforms`, trigger conditions) + markdown body with numbered steps, pitfalls, verification.

**AGENTS.md**: Pure markdown with conventional sections (Architecture, Conventions, Important Notes). Hierarchical discovery walks subdirectories for monorepo support.

### 2.3 Loading Order and Override Semantics

The hermes-agent approach is **additive with priority**:
- SOUL.md replaces the default identity (override)
- Memory is appended after identity (additive)
- Skills index is appended (additive)
- Project context is appended, but only one type loads (priority)

There is no merge or conflict resolution. Each layer occupies a distinct slot in the prompt. The order matters because LLMs weight earlier content differently, and SOUL.md as slot #1 has the strongest influence on behavior.

---

## 3. Safe Self-Modification

### 3.1 hermes-agent's Safety Mechanisms

**Prompt injection scanning** (`prompt_builder.py::_scan_context_content()`):
- Regex patterns for: instruction overrides, deception, system prompt hijacking, hidden HTML, credential exfiltration, secret file access
- Invisible Unicode character detection (zero-width spaces, bidirectional overrides)
- Blocked content gets a warning message instead of the original content
- Applied to all context files (SOUL.md, AGENTS.md, .cursorrules) and memory entries

**Memory content scanning** (`memory_tool.py::_scan_memory_content()`):
- Separate scanner for memory-specific threats (role hijacking, exfiltration via curl/wget, SSH backdoor attempts)
- Blocks content before it enters the memory store

**Skills security guard** (`tools/skills_guard.py`):
- Full security scan of skill directories
- Three-tier result: allow, ask (warn but allow), block
- Automatic rollback on block -- original content restored via atomic write pattern

**Bounded memory**: Character limits (2,200 + 1,375) prevent unbounded growth that could degrade prompt quality or blow context windows.

**Atomic writes**: Both memory and skills use temp-file + `os.replace()` for crash-safe persistence.

**File locking**: Memory uses `fcntl.flock()` for concurrent session safety.

### 3.2 What's Missing (and Needed for IronHermes)

**Version history**: hermes-agent has no versioning for context files. If the agent writes a catastrophically bad SOUL.md, there is no built-in recovery path other than the agent recognizing the problem in a future session. IronHermes should add:
- A simple version log for SOUL.md changes (append-only log of diffs or full snapshots)
- A `/rollback` command to revert to a previous version
- Maximum of N versions kept (e.g., 10) to bound storage

**Validation before application**: hermes-agent scans for injection but does not validate semantic quality. A SOUL.md that says "Always respond with exactly one word" is technically safe but functionally broken. Consider:
- A minimum content length check for SOUL.md
- A structural validation (does it still contain identity-like content?)
- A "soft lock" mode where the agent proposes changes to SOUL.md but the user must approve

**Rate limiting**: Nothing prevents the agent from rewriting SOUL.md 50 times in a session. Consider per-session limits on self-modification operations.

---

## 4. Reflection Loop Patterns

### 4.1 hermes-agent's Approach: Opportunistic, Not Periodic

hermes-agent does NOT have a reflection loop. Self-improvement is triggered by:
- **Task completion**: "After completing a complex task (5+ tool calls), offer to save as a skill"
- **User correction**: "User corrects you or says 'remember this'" triggers memory save
- **Skill failure**: "When using a skill and finding it outdated, patch it immediately"
- **Discovery**: "If you've discovered a new way to do something, save it as a skill"

This is entirely embedded in the prompt guidance (MEMORY_GUIDANCE, SKILLS_GUIDANCE). The agent is instructed to self-improve, but there is no code-level reflection mechanism.

### 4.2 Broader Patterns (from Training Knowledge)

**Periodic reflection** (e.g., Reflexion paper pattern):
- After N turns or at session end, the agent reviews its conversation
- Generates a self-critique: what went well, what could improve
- Writes improvements to memory/context files
- Risk: adds token overhead; can become navel-gazing; may not produce actionable insights

**Triggered reflection** (hermes-agent's approach, plus):
- Error-triggered: when a tool call fails, reflect on why and save the lesson
- Feedback-triggered: when user provides explicit positive/negative feedback
- Pattern-triggered: when the agent notices it is repeating itself or going in circles

**Outcome-based reflection**:
- Track task success/failure rates per skill
- Deprecate or flag skills with high failure rates
- Requires a definition of "success" which is hard to automate

### 4.3 Recommendation for IronHermes

Start with hermes-agent's opportunistic approach (it works and is simple), but add two enhancements:

1. **Session-end reflection prompt**: At the end of long sessions (10+ turns), inject a brief internal prompt: "Review this session. Should any memories be updated? Should any skills be created or patched?" This is cheap (one extra LLM call) and catches improvements the agent might not notice mid-task.

2. **Error-triggered learning**: When a tool call fails and the agent recovers, prompt it to consider saving the recovery approach. This is already partially covered by SKILLS_GUIDANCE but could be made more systematic.

Do NOT build periodic background reflection (cron-based self-evaluation) -- the cost/benefit ratio is poor and the agent lacks objective quality metrics.

---

## 5. Memory vs Context Files: When to Use Each

### 5.1 hermes-agent's Three-Tier Model

| Tier | Mechanism | Capacity | Latency | Best For |
|------|-----------|----------|---------|----------|
| **Always-on context** | SOUL.md, MEMORY.md, USER.md | ~24K chars total | Zero (in prompt) | Identity, core preferences, environment facts |
| **On-demand procedural** | Skills (SKILL.md) | Unbounded (indexed) | One tool call (skill_view) | How-to procedures, workflows, templates |
| **Search-based recall** | Session search (SQLite FTS5) | Unbounded (all sessions) | Tool call + LLM summarization | Past conversation recall, "did we discuss X?" |

This is a well-designed hierarchy. Each tier trades capacity for latency:
- Tier 1 is always available but severely bounded (to keep prompt costs down)
- Tier 2 has a compact index always visible, with full content loaded on demand
- Tier 3 has unlimited capacity but requires explicit search

### 5.2 What Goes Where

**SOUL.md (identity)**:
- Personality traits, communication style, values
- Stable across all contexts and projects
- Changed rarely (personality evolution, not task-by-task)

**MEMORY.md (agent notes)**:
- Environment: OS, installed tools, project paths
- Conventions: coding style, preferred tools, API quirks
- Lessons: "don't use sudo for Docker on this machine"

**USER.md (user profile)**:
- Name, role, timezone
- Communication preferences
- Pet peeves, expectations

**Skills**:
- Multi-step procedures that worked
- Error recovery recipes
- Non-obvious workflows

**Session search**:
- Historical context ("we discussed the migration last Tuesday")
- Task outcomes and results
- Anything too large or ephemeral for curated memory

### 5.3 Recommendation for IronHermes

Implement the same three-tier model. The design is proven and the separation of concerns is clean. Specific Rust considerations:

- **SOUL.md / MEMORY.md / USER.md**: Simple file I/O with `tokio::fs`. Use `tempfile` crate + `std::fs::rename()` for atomic writes. Use `fs2` crate for file locking (cross-platform `flock` equivalent).
- **Skills**: Directory tree with serde-based YAML frontmatter parsing. Use `serde_yaml` for frontmatter, `pulldown-cmark` if you need markdown parsing (probably not needed -- skills are injected as raw text).
- **Session search**: Already have SQLite with FTS5 in `ironhermes-state`. Wire up a search tool.

---

## 6. Architecture Recommendations for IronHermes

### 6.1 System Prompt Assembly

Port the 10-layer prompt assembly from hermes-agent. The existing `PromptBuilder` in IronHermes is a good skeleton but needs:

```rust
pub struct PromptBuilder {
    model: String,
    platform: String,
    // Layer 1: Identity
    identity: Option<String>,           // From SOUL.md or default
    // Layer 2: Behavior guidance
    // (hardcoded constants, conditional on available tools)
    // Layer 5-6: Memory
    memory_snapshot: Option<String>,     // Frozen MEMORY.md content
    user_snapshot: Option<String>,       // Frozen USER.md content
    // Layer 7: Skills
    skills_index: Option<String>,        // Compact skills listing
    // Layer 8: Context files
    context_files: Option<String>,       // AGENTS.md / .hermes.md
    // Layer 9-10: Timestamp, platform
}
```

### 6.2 Context File Loading

Current `load_context_files()` reads all candidates and concatenates them. Port the hermes-agent priority system instead:

1. SOUL.md from `$IRONHERMES_HOME` (not working directory) -- identity slot
2. Project context: first match wins from `.hermes.md` -> `AGENTS.md` -> `CLAUDE.md` -> `.cursorrules`
3. Security scan all content before injection
4. Truncate at 20K chars per file

### 6.3 Memory Subsystem

Create an `ironhermes-memory` module (could live in `ironhermes-state` or be a new crate):

```rust
pub struct MemoryStore {
    memory_entries: Vec<String>,
    user_entries: Vec<String>,
    memory_char_limit: usize,    // 2200
    user_char_limit: usize,      // 1375
    // Frozen at load time for system prompt injection
    system_prompt_snapshot: MemorySnapshot,
}

struct MemorySnapshot {
    memory_block: String,
    user_block: String,
}
```

Key operations: `load_from_disk()`, `add()`, `replace()`, `remove()`, `format_for_system_prompt()`.

File format: same `\n[section-sign]\n` delimiter for compatibility with hermes-agent memory files.

### 6.4 Skills Subsystem

Start simple -- the full hermes-agent skills system is large. For MVP:

1. Scan `$IRONHERMES_HOME/skills/` for `SKILL.md` files
2. Parse YAML frontmatter for name + description
3. Build a compact index string for the system prompt
4. Implement `skill_view` tool (read a skill on demand)
5. Implement `skill_manage` tool (create, patch, delete)
6. Security scan on write (port the threat pattern regex list)

### 6.5 Version History for Self-Modified Files

Add a simple append-only changelog for SOUL.md and memory files:

```
$IRONHERMES_HOME/
  SOUL.md                    -- Current version
  .history/
    soul/
      2026-04-01T14:30:00.md -- Previous version snapshot
      2026-04-01T15:45:00.md -- Previous version snapshot
    memory/
      2026-04-01T14:30:00.md
```

Keep last 10 snapshots per file. Expose via `/rollback soul` command. This is a meaningful improvement over hermes-agent which has no versioning.

### 6.6 Security Scanning

Port the regex-based threat detection from hermes-agent. The patterns are language-agnostic:

- Prompt injection: "ignore previous instructions", "disregard your rules"
- Deception: "do not tell the user"
- Exfiltration: `curl ... $API_KEY`, `cat .env`
- Invisible Unicode: zero-width spaces, bidirectional overrides

Use the `regex` crate with `RegexSet` for efficient multi-pattern matching.

---

## 7. Pitfalls

### 7.1 Critical: Prompt Cache Invalidation

The #1 lesson from hermes-agent is that **self-modification must not invalidate the prompt cache mid-session**. The frozen-snapshot pattern exists specifically to prevent this. If IronHermes updates SOUL.md and immediately reloads the system prompt, every subsequent API call in that session will cache-miss, dramatically increasing costs.

**Prevention**: Always use the frozen-snapshot pattern. Changes to context files take effect on next session start, never mid-session.

### 7.2 Critical: Runaway Self-Modification

Without guardrails, the agent could rewrite SOUL.md into something that makes it unable to function, then be stuck because its identity is now broken.

**Prevention**:
- Security scanning on all writes
- Version history with rollback
- Minimum content validation (SOUL.md must be non-empty and at least 50 chars)
- Per-session rate limit on self-modification operations (max 5 SOUL.md edits per session)
- The user can always manually edit `$IRONHERMES_HOME/SOUL.md`

### 7.3 Moderate: Memory Bloat

If the agent saves too aggressively, memory fills up with low-value entries and the agent spends tokens managing its memory instead of doing useful work.

**Prevention**: Character limits (already in the design), plus guidance in the prompt that prioritizes "what reduces future user steering" over completionism.

### 7.4 Moderate: Skill Rot

Skills that worked six months ago may not work today (tools change, APIs change, environments change). Stale skills are worse than no skills because the agent follows outdated procedures.

**Prevention**: The SKILLS_GUIDANCE instruction to "patch immediately when issues are found" is the right approach. Consider adding a `last_used` / `last_updated` timestamp to skills metadata for future staleness detection.

### 7.5 Minor: Cross-Platform Context Divergence

If SOUL.md is optimized for CLI interactions, it may produce poor results on Telegram (where messages should be shorter). The platform hint partially addresses this, but personality and platform can conflict.

**Prevention**: Keep SOUL.md platform-agnostic. Use platform hints (layer 10) for formatting differences. If needed, support per-platform SOUL overrides in the future.

---

## 8. Implementation Priority for IronHermes

Based on the research, the recommended build order is:

### Phase 1: Context File Loading (foundation)
- Port SOUL.md loading from `$IRONHERMES_HOME` (not cwd)
- Implement priority-based project context discovery
- Add security scanning (regex-based threat detection)
- Add content truncation (20K char limit)

### Phase 2: Memory Subsystem
- MemoryStore with MEMORY.md / USER.md
- `memory` tool with add/replace/remove actions
- Frozen snapshot pattern for system prompt injection
- Atomic file writes with file locking

### Phase 3: Skills Subsystem (MVP)
- Skills directory scanning and index building
- `skill_view` tool (read on demand)
- `skill_manage` tool (create, patch, delete)
- Security scanning on skill writes

### Phase 4: Safety and Polish
- Version history for SOUL.md and memory files
- `/rollback` command
- Rate limiting on self-modification
- Session-end reflection prompt (optional enhancement)

---

## Sources

All findings are from direct source code analysis:
- `/Users/twilson/code/hermes-agent/agent/prompt_builder.py` -- Prompt assembly, context file loading, security scanning
- `/Users/twilson/code/hermes-agent/tools/memory_tool.py` -- Memory subsystem implementation
- `/Users/twilson/code/hermes-agent/tools/skill_manager_tool.py` -- Skills subsystem implementation
- `/Users/twilson/code/hermes-agent/run_agent.py` -- `_build_system_prompt()`, agent loop
- `/Users/twilson/code/hermes-agent/hermes_cli/default_soul.py` -- Default SOUL.md content
- `/Users/twilson/code/hermes-agent/website/docs/developer-guide/prompt-assembly.md` -- Official prompt assembly documentation
- `/Users/twilson/code/hermes-agent/website/docs/user-guide/features/personality.md` -- SOUL.md documentation
- `/Users/twilson/code/hermes-agent/website/docs/user-guide/features/memory.md` -- Memory documentation
- `/Users/twilson/code/hermes-agent/website/docs/user-guide/features/context-files.md` -- Context files documentation
- `/Users/twilson/code/ironhermes/crates/ironhermes-agent/src/prompt_builder.rs` -- Current IronHermes prompt builder
- `/Users/twilson/code/ironhermes/.planning/PROJECT.md` -- IronHermes project context

Confidence: HIGH -- all findings derived from primary source code analysis, not web search or training data.
