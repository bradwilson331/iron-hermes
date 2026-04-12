# Phase 15: 10-Layer Prompt Assembly - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-12
**Phase:** 15-10-layer-prompt-assembly
**Areas discussed:** Personality overlays, Layer content details, Cached vs ephemeral split

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

## Claude's Discretion

- Exact text content of each of the 14 built-in personality presets
- PromptBuilder migration strategy (incremental vs clean rewrite)
- Whether PromptSlot::UserMessage is populated by PromptBuilder or callers
- Internal API for populating individual slots
- Personality preset loading timing (eager vs lazy)

## Deferred Ideas

None — discussion stayed within phase scope.
