# Hermes Agent Persistent Memory Reference

Source: User-provided documentation for Phase 21.4 gap analysis.

## Overview

Hermes Agent has bounded, curated memory that persists across sessions. Two files:

| File | Purpose | Char Limit |
|------|---------|------------|
| MEMORY.md | Agent's personal notes -- environment facts, conventions, things learned | 2,200 chars (~800 tokens) |
| USER.md | User profile -- preferences, communication style, expectations | 1,375 chars (~500 tokens) |

Both stored in `~/.hermes/memories/` and injected into system prompt as frozen snapshot at session start.

## How Memory Appears in System Prompt

```
══════════════════════════════════════════════
MEMORY (your personal notes) [67% -- 1,474/2,200 chars]
══════════════════════════════════════════════
User's project is a Rust web service at ~/code/myapi using Axum + SQLx
§
This machine runs Ubuntu 22.04, has Docker and Podman installed
§
User prefers concise responses, dislikes verbose explanations
```

Format: header with store name + usage percentage + char counts, entries separated by `§` (section sign).

**Frozen snapshot pattern:** System prompt injection captured once at session start, never changes mid-session. Preserves LLM prefix cache. Mid-session changes persisted to disk immediately but appear in prompt next session only.

## Memory Tool Actions

- **add** -- Add a new memory entry
- **replace** -- Replace existing entry (substring matching via `old_text`)
- **remove** -- Remove entry (substring matching via `old_text`)
- No **read** action -- memory auto-injected into system prompt

### Substring Matching

`old_text` needs to be a unique substring identifying exactly one entry. Multiple matches return error.

## Two Targets

- **memory** -- Agent's personal notes (environment, conventions, tool quirks, completed tasks, techniques)
- **user** -- User profile (name, role, timezone, communication prefs, pet peeves, workflow habits, skill level)

## What to Save vs Skip

**Save:** User preferences, environment facts, corrections, conventions, completed work, explicit requests.
**Skip:** Trivial/obvious info, easily re-discovered facts, raw data dumps, session ephemera, info already in context files.

## Capacity Management

| Store | Limit | Typical entries |
|-------|-------|-----------------|
| memory | 2,200 chars | 8-15 entries |
| user | 1,375 chars | 5-10 entries |

When full, tool returns error with current entries and usage. Agent should consolidate/replace before adding.

## Duplicate Prevention

Automatically rejects exact duplicate entries.

## Security Scanning

Memory entries scanned for injection/exfiltration patterns before acceptance (prompt injection, credential exfiltration, SSH backdoors, invisible Unicode).

## Session Search

- All sessions stored in SQLite (`~/.hermes/state.db`) with FTS5 full-text search
- `session_search` tool queries past conversations with Gemini Flash summarization
- Complement to memory: memory = always in context, session_search = on-demand recall

## Configuration

```yaml
# ~/.hermes/config.yaml
memory:
  memory_enabled: true
  user_profile_enabled: true
  memory_char_limit: 2200
  user_char_limit: 1375
```

## External Memory Providers

8 plugins: Honcho, OpenViking, Mem0, Hindsight, Holographic, RetainDB, ByteRover, Supermemory.

Run alongside built-in memory (never replacing). Add: knowledge graphs, semantic search, auto fact extraction, cross-session user modeling.

**Note for gap analysis:** IronHermes currently implements 3 providers (SQLite, Grafeo, DuckDB) per v2.0 scope decision. The 8-provider Python ecosystem is out of scope.

```
hermes memory setup      # pick a provider and configure it
hermes memory status     # check what's active
```
