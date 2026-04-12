# Phase 15: 10-Layer Prompt Assembly - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-12
**Phase:** 15-10-layer-prompt-assembly
**Areas discussed:** Personality overlays, Layer content details, Cached vs ephemeral split, Subagent prompts, Config system_message, Build API migration

---

## Personality overlays

| Option | Description | Selected |
|--------|-------------|----------|
| Prepend to SOUL.md | Overlay text prepended before SOUL.md in layer 1. Clean separation. | |
| Replace SOUL.md entirely | Overlay replaces SOUL.md for the session. Simpler but loses base identity. | |
| Append after SOUL.md | Overlay appended after SOUL.md. Risk: model may weight later instructions more. | |

**User's choice:** Prepend to SOUL.md (initial selection), later overridden by hermes-agent reference showing overlay belongs in ephemeral slot 8 (SessionOverlay), not in identity slot 1.
**Notes:** User provided comprehensive hermes-agent documentation showing /personality is a session-level overlay in the ephemeral layer, separate from SOUL.md identity.

### Custom preset storage

| Option | Description | Selected |
|--------|-------------|----------|
| config.yaml only | Presets under personality.presets in config.yaml | |
| HERMES_HOME/personalities/ | Each preset as a separate .md file | |
| Both sources merged | config.yaml for inline, personalities/ for longer ones | ✓ |

**User's choice:** Both sources merged
**Notes:** User also provided list of 14 built-in presets from hermes-agent: helpful, concise, technical, creative, teacher, kawaii, catgirl, pirate, shakespeare, surfer, noir, uwu, philosopher, hype. Custom presets in `agent.personalities` config namespace.

---

## Layer content details

### Provider block (layer 3)

| Option | Description | Selected |
|--------|-------------|----------|
| Model identity + limits | Tell model its name, provider, context window size | ✓ |
| Provider-specific instructions | Behavioral rules per provider | |
| You decide | Claude's discretion | |

**User's choice:** Model identity + limits
**Notes:** Folded into ToolGuidance slot per 9-slot reference (no separate provider block slot).

### Optional system message (layer 4)

**User's choice:** User provided hermes-agent SOUL.md documentation instead of directly answering. Confirmed SOUL.md is for durable identity, AGENTS.md for project instructions. System message concept folds into SessionOverlay slot.
**Notes:** Detailed reference on SOUL.md best practices: use for tone/style/directness, not project instructions.

### Timestamp layer (layer 9)

| Option | Description | Selected |
|--------|-------------|----------|
| Date + session ID + turn count | Current UTC, session ID, turn number | ✓ |
| Date only | Just current date/time | |
| You decide | Claude's discretion | |

**User's choice:** Date + session ID + turn count
**Notes:** User added that active personality overlay info also belongs in this ephemeral layer.

---

## Cached vs ephemeral split

### API shape

| Option | Description | Selected |
|--------|-------------|----------|
| Two-part build | build() returns (durable, ephemeral) tuple | |
| Single string with marker | build() returns one string with marker | |
| You decide | Claude's discretion | |

**User's choice:** User provided complete PromptSlot enum implementation from hermes-agent reference. 9-slot BTreeMap with `>= Timestamp` as ephemeral boundary. build() returns (String, String) tuple.

### Slot count

| Option | Description | Selected |
|--------|-------------|----------|
| Follow 9-slot reference | PromptSlot enum from hermes-agent. Simpler. | ✓ |
| Keep all 10 layers from PRMT-01 | Add ProviderBlock and SystemMessage as separate slots | |
| You decide | Claude reconciles the two specs | |

**User's choice:** Follow 9-slot reference
**Notes:** User-provided hermes-agent reference is authoritative, overrides PRMT-01's 10-layer spec.

---

## Subagent prompts (Round 2)

| Option | Description | Selected |
|--------|-------------|----------|
| Identity + ToolGuidance only | Subagents get DEFAULT_AGENT_IDENTITY + tool guidance. Minimal focused prompt. | ✓ |
| Identity + ToolGuidance + Memory | Also include frozen memory snapshot. | |
| All durable except ContextFiles | Identity, ToolGuidance, Memory, Skills. | |
| You decide | Claude's discretion | |

**User's choice:** Identity + ToolGuidance only (confirmed by hermes-agent delegation docs)
**Notes:** User provided comprehensive hermes-agent subagent documentation. Subagents know nothing from parent conversation — fresh context from goal/context fields only. Blocked tools: delegation, clarify, memory, send_message, execute_code. Max concurrency 3, depth limit 2.

---

## Config system_message (Round 2)

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, in SessionOverlay slot | agent.system_message in config.yaml for persistent custom instructions | |
| No, use SOUL.md instead | Custom instructions belong in SOUL.md or AGENTS.md | ✓ |
| You decide | Claude's discretion | |

**User's choice:** No separate system_message config (confirmed by hermes-agent context file docs)
**Notes:** User provided hermes-agent context file documentation. No `agent.system_message` key exists in hermes-agent. SOUL.md is for identity, AGENTS.md for project instructions. Also confirmed: HERMES.md is a valid context file name, .cursor/rules/*.mdc supported, subdirectory truncation at 8,000 chars.

---

## Build API migration (Round 2)

| Option | Description | Selected |
|--------|-------------|----------|
| Clean break | Change build() to return (String, String). Update all callers. | |
| Struct return type | Return SystemPrompt struct with .durable/.ephemeral/.combined() | |
| You decide | Claude's discretion | |

**User's choice:** User provided migration checklist from hermes-agent reference:
1. Add `build_split() -> (String, String)` as new primary method
2. Refactor `build() -> String` to call `build_split()` and join (backwards-compatible)
3. Agent loop checks if LLM adapter supports multi-block system prompts; if so, passes split parts separately

**Notes:** No breaking change — build() remains as convenience. build_split() is the canonical new method.

---

## Claude's Discretion

- Exact text content of each of the 14 built-in personality presets
- Whether PromptSlot::UserMessage is populated by PromptBuilder or callers
- Internal API for populating individual slots
- Personality preset loading timing (eager vs lazy)

## Deferred Ideas

None — discussion stayed within phase scope.
