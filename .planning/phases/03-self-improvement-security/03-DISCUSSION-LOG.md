# Phase 3: Self-Improvement + Security - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-07
**Phase:** 03-self-improvement-security
**Areas discussed:** Self-edit guardrails, Memory subsystem design, SSRF validation scope, Rate limiting strategy

---

## Self-edit guardrails

### Write blocking behavior

| Option | Description | Selected |
|--------|-------------|----------|
| Block the write entirely | write_file/patch returns error, file not modified | ✓ |
| Write but strip the threat | Remove/redact matching pattern, write sanitized version | |
| Write but warn | Allow the write, return warning to agent | |

**User's choice:** Block the write entirely
**Notes:** Matches Python hermes-agent behavior

### Scan scope

| Option | Description | Selected |
|--------|-------------|----------|
| Context files only | SOUL.md, AGENTS.md, MEMORY.md, USER.md — files injected into system prompt | ✓ |
| All files written by agent | Every write_file/patch goes through scanning | |
| Context files + configurable paths | Scan context files plus additional paths from config | |

**User's choice:** Context files only

### File permissions

| Option | Description | Selected |
|--------|-------------|----------|
| All writable | Agent can edit all context files, protected by scanning | ✓ |
| SOUL.md read-only | Personality file sacred, only user can edit | |
| Configurable per-file | Config YAML specifies which files are writable | |

**User's choice:** All writable

### Architecture

| Option | Description | Selected |
|--------|-------------|----------|
| Move scanner to core | Move context_scanner.rs to ironhermes-core for shared access | ✓ |
| Callback/hook on tools | Tools accept optional scan callback, agent wires it | |
| You decide | Claude picks cleanest approach | |

**User's choice:** Move scanner to core

---

## Memory subsystem design

### Store count

| Option | Description | Selected |
|--------|-------------|----------|
| Two stores | MEMORY.md (2,200 chars) + USER.md (1,375 chars), matching hermes-agent | ✓ |
| Single MEMORY.md | One file with all entries | |

**User's choice:** Two stores
**Notes:** User referenced `/Users/twilson/code/hermes-agent/hermes_state.py` for memory guidelines; actual memory implementation found in `tools/memory_tool.py`

### File locking

| Option | Description | Selected |
|--------|-------------|----------|
| Advisory flock via fs2 crate | Cross-platform file locking, matches Python fcntl pattern | ✓ |
| Atomic-only, no locking | Rely purely on atomic temp+rename | |
| You decide | Claude picks based on Rust patterns | |

**User's choice:** Advisory flock via fs2 crate

### Read action

| Option | Description | Selected |
|--------|-------------|----------|
| No read action | Memory injected via frozen snapshot in system prompt | ✓ |
| Include read action | Agent can explicitly read live memory state mid-session | |
| You decide | Claude decides based on frozen-snapshot pattern | |

**User's choice:** No read action

### Crate location

| Option | Description | Selected |
|--------|-------------|----------|
| New module in ironhermes-core | Shared leaf crate, both agent and tools can depend on it | ✓ |
| In ironhermes-agent | Next to PromptBuilder but creates circular dep for tools | |
| New ironhermes-memory crate | Dedicated crate, clean but adds workspace complexity | |

**User's choice:** New module in ironhermes-core

---

## SSRF validation scope

### Port depth

| Option | Description | Selected |
|--------|-------------|----------|
| Direct port | Match Python exactly: resolve + check private ranges, fail closed | ✓ |
| Add connection-level check | Re-validate IPs at connection time, mitigating DNS rebinding | |
| You decide | Claude picks based on threat model | |

**User's choice:** Direct port
**Notes:** DNS rebinding documented as known limitation, same as Python

### Crate location

| Option | Description | Selected |
|--------|-------------|----------|
| In ironhermes-core | Shared foundation, any crate can import | ✓ |
| In ironhermes-tools | Co-located with web tools | |
| You decide | Claude picks based on dependency graph | |

**User's choice:** In ironhermes-core

---

## Rate limiting strategy

### Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Per-user | Each Telegram user_id gets independent rate limits | ✓ |
| Global | Single rate limit across all users | |
| Per-chat | Rate limit by chat_id | |

**User's choice:** Per-user

### Excess handling

| Option | Description | Selected |
|--------|-------------|----------|
| Silent drop | Messages over limit silently ignored | ✓ |
| Error message | Reply with rate limit warning | |
| Queue with cap | Queue excess up to depth limit | |

**User's choice:** Silent drop

### Configuration

| Option | Description | Selected |
|--------|-------------|----------|
| Configurable in config.yaml | messages_per_minute + burst_size fields | ✓ |
| Hardcoded defaults only | Sensible defaults baked in | |
| You decide | Claude picks defaults and config exposure | |

**User's choice:** Configurable in config.yaml

---

## Claude's Discretion

- Token bucket vs sliding window for rate limiting
- Extended threat patterns for memory scanning
- Memory tool schema description wording
- fsync before rename in atomic writes
- Deduplication strategy details

## Deferred Ideas

None — discussion stayed within phase scope
