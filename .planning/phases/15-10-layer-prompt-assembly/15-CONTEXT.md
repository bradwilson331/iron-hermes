# Phase 15: 10-Layer Prompt Assembly - Context

**Gathered:** 2026-04-12
**Status:** Ready for planning

<domain>
## Phase Boundary

Restructure the system prompt into an ordered slot-based assembly using a `PromptSlot` enum with `BTreeMap` storage, implementing 9 slots matching hermes-agent's architecture. Includes frozen memory snapshots, durable/ephemeral layer separation (cache breakpoint between slots 5 and 6), and a /personality command for session-level identity overlays with 14 built-in presets plus custom presets from config.yaml and HERMES_HOME/personalities/ directory.

</domain>

<decisions>
## Implementation Decisions

### Slot ordering (replaces PRMT-01's 10-layer spec)
- **D-01:** Follow the 9-slot PromptSlot enum from hermes-agent reference, NOT the 10-layer spec in PRMT-01. The authoritative ordering is: (1) Identity, (2) ToolGuidance, (3) Memory, (4) Skills, (5) ContextFiles, (6) Timestamp, (7) PlatformHints, (8) SessionOverlay, (9) UserMessage.
- **D-02:** Provider block (PRMT-01 layer 3) and optional system message (PRMT-01 layer 4) are NOT separate slots. Provider info folds into Identity or ToolGuidance. Config-driven system message folds into SessionOverlay.
- **D-03:** `PromptSlot` is a `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]` enum with discriminant values 1-9. `PromptBuilder` uses `BTreeMap<PromptSlot, String>` for ordered storage.

### Durable vs ephemeral split
- **D-04:** Cache breakpoint is between slot 5 (ContextFiles) and slot 6 (Timestamp). Slots 1-5 are durable (stable across turns, cacheable). Slots 6-9 are ephemeral (regenerated per turn).
- **D-05:** `build()` returns a `(String, String)` tuple: `(durable, ephemeral)`. Split logic: `slot >= PromptSlot::Timestamp` goes to ephemeral. Phase 16 will place `cache_control` breakpoint between the two parts.
- **D-06:** Durable slots are frozen at session start — mid-session file edits to SOUL.md, MEMORY.md, skills, or context files do NOT change the active prompt (frozen-snapshot pattern from Phase 12).

### Personality overlay system
- **D-07:** /personality applies a session-level overlay as slot 8 (SessionOverlay), NOT prepended to slot 1 (Identity). SOUL.md remains the stable identity foundation in the durable layer; personality overlays live in the ephemeral layer so they can change mid-session without invalidating the prompt cache.
- **D-08:** 14 built-in personality presets: helpful, concise, technical, creative, teacher, kawaii, catgirl, pirate, shakespeare, surfer, noir, uwu, philosopher, hype.
- **D-09:** Custom presets from two merged sources: (1) config.yaml under `agent.personalities` namespace for quick inline presets, (2) HERMES_HOME/personalities/ directory as separate .md files for longer presets. Both sources merged at load time, config.yaml takes precedence on name collision.
- **D-10:** /personality with no argument lists available presets (built-in + custom). /personality <name> activates a preset. /personality off removes the overlay. Only one overlay active at a time.

### Layer content
- **D-11:** Slot 3 (Memory): Frozen MEMORY.md and USER.md snapshots with capacity headers. Frozen at session start per MEM-06.
- **D-12:** Slot 6 (Timestamp): Current UTC date/time, session identifier, current turn number, and active personality overlay name (if any).
- **D-13:** Slot 7 (PlatformHints): Platform-specific formatting guidance (cli/telegram/discord/slack). Already implemented in current PromptBuilder — moves from current position to ephemeral slot 7.
- **D-14:** Slot 2 (ToolGuidance): Includes model identity and provider context (model name, provider name, known context window size). Folds the "provider block" concept into tool guidance.

### Subagent prompt building
- **D-15:** Subagents get Identity (DEFAULT_AGENT_IDENTITY) + ToolGuidance only — slots 3-8 are skipped entirely. Subagents know nothing from the parent conversation; their only context comes from the goal and context fields passed via delegate_task. No SOUL.md, no memory, no skills, no project context, no personality overlay.
- **D-16:** Blocked tools for subagents: delegation, clarify, memory, send_message, execute_code. This is already implemented in Phase 9 — PromptBuilder just needs to respect skip_context_files to skip slots 3-8.

### Config system_message
- **D-17:** No separate `agent.system_message` config key. hermes-agent uses SOUL.md for durable identity and AGENTS.md/project context files for project instructions. There is no third config-driven instruction slot. The SessionOverlay slot (8) is exclusively for /personality overlays.

### Additional context file details (from hermes-agent reference)
- **D-18:** `HERMES.md` is also a valid context file name alongside `.hermes.md` — add to the priority chain candidates.
- **D-19:** `.cursor/rules/*.mdc` rule modules are supported in addition to `.cursorrules`.
- **D-20:** Subdirectory discovery truncation cap is **8,000 chars** per file (not 20,000 like startup context files).
- **D-21:** Context files assembled under a `# Project Context` header. SOUL.md content inserted directly without wrapper text (already captured in D-11 from Phase 14, confirmed here).

### Build API migration
- **D-22:** Add `build_split() -> (String, String)` as the new primary method returning `(durable, ephemeral)`.
- **D-23:** Refactor existing `build() -> String` to call `build_split()` internally and join the two parts. No breaking change — `build()` remains as a convenience method for callers that don't need the split.
- **D-24:** Agent loop checks if the LLM adapter supports multi-block system prompts; if so, passes the split parts separately. Otherwise, concatenates via `build()`. This prepares for Phase 16's cache_control breakpoint placement.

### Claude's Discretion
- Exact text content of each of the 14 built-in personality presets
- Whether PromptSlot::UserMessage (slot 9) is populated by PromptBuilder or by callers
- Internal API for populating individual slots (setter methods vs builder pattern)
- How /personality command integrates with the slash command system (Phase 20 scope, but the overlay mechanism is Phase 15)
- Personality preset loading: eager at startup vs lazy on first /personality call

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Prompt assembly requirements
- `.planning/REQUIREMENTS.md` — PRMT-01 (layer ordering — note: user overrode to 9-slot model), PRMT-02 (cached/ephemeral separation), PRMT-03 (SOUL.md from HERMES_HOME), PRMT-04 (SOUL.md security scan + 20K cap), PRMT-05 (skip_context_files for subagents), PRMT-06 (/personality session overlay), PRMT-07 (built-in + custom presets), MEM-06 (frozen memory snapshots)

### hermes-agent architecture (user-provided during discussion)
- **Prompt stack & PromptSlot enum:** 9-slot ordering (Identity, ToolGuidance, Memory, Skills, ContextFiles | CACHE BREAK | Timestamp, PlatformHints, SessionOverlay, UserMessage). Cache breakpoint after slot 5. `build_split()` returns `(durable, ephemeral)`, `build()` joins them.
- **Personality system:** 14 built-in presets (helpful, concise, technical, creative, teacher, kawaii, catgirl, pirate, shakespeare, surfer, noir, uwu, philosopher, hype). Custom presets in `agent.personalities` config namespace. /personality is session-level overlay in ephemeral slot 8.
- **SOUL.md:** Injected raw as slot 1, no wrapper. For durable identity/voice only. Hermes seeds a default SOUL.md if absent. Security scanned + truncated at 20K chars.
- **Context files:** Priority chain .hermes.md/HERMES.md > AGENTS.md > CLAUDE.md > .cursorrules > .cursor/rules/*.mdc. Subdirectory discovery truncation at 8,000 chars. Assembled under `# Project Context` header.
- **Subagent delegation:** Subagents get DEFAULT_AGENT_IDENTITY + ToolGuidance only. Fresh conversation, zero parent context. Blocked tools: delegation, clarify, memory, send_message, execute_code. Max concurrency 3, depth limit 2.

### Existing implementation (primary code references)
- `crates/ironhermes-agent/src/prompt_builder.rs` — Current PromptBuilder with `build()` method assembling identity, platform hint, tool guidance, project context, AGENTS.md, skills catalog, memory snapshots. Needs restructuring to PromptSlot/BTreeMap pattern.
- `crates/ironhermes-agent/src/context_loader.rs` — ContextLoader with priority chain walk, frontmatter stripping. Phase 14 output — feeds into slot 5 (ContextFiles).
- `crates/ironhermes-core/src/memory_store.rs` — MemoryStore with `format_for_system_prompt()` — feeds into slot 3 (Memory).
- `crates/ironhermes-core/src/context_scanner.rs` — `scan_context_content()` and `truncate_content()` — used for SOUL.md and all content injection.

### Integration points
- `crates/ironhermes-agent/src/agent_loop.rs` — Agent loop where `build()` is called and system message is constructed
- `crates/ironhermes-gateway/src/handler.rs` — Gateway handler that constructs PromptBuilder
- `crates/ironhermes-cli/src/main.rs` — CLI entry point that constructs PromptBuilder
- `crates/ironhermes-core/src/config.rs` — Config struct needs `agent.personalities` and `agent.system_message` sections

### Architecture
- `.planning/codebase/ARCH.md` — Crate dependency graph, key abstractions
- `.planning/ROADMAP.md` — Phase 15 success criteria, downstream dependencies (Phase 16 depends on 15 for cache breakpoint)

### Prior phase context
- `.planning/phases/11-memory-provider-trait/11-CONTEXT.md` — Frozen-snapshot pattern, async_trait + Send + Sync, MemoryProvider trait
- `.planning/phases/12-provider-resolution/12-CONTEXT.md` — Frozen-snapshot pattern (resolve once at startup), ProviderResolver struct
- `.planning/phases/14-context-files-soul-md/14-CONTEXT.md` — ContextLoader, priority chain, SOUL.md loading, skip_context_files, DEFAULT_AGENT_IDENTITY

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `PromptBuilder` (`prompt_builder.rs`): Already loads SOUL.md, tool guidance, project context, AGENTS.md, skills catalog, memory snapshots, platform hint. Core logic preserved — restructured into PromptSlot/BTreeMap pattern.
- `DEFAULT_AGENT_IDENTITY` const: Hardcoded fallback identity (Phase 14 D-12). Stays as-is.
- `TOOL_USE_GUIDANCE` const: Existing tool guidance text for slot 2.
- `MemoryProvider::format_for_system_prompt()`: Already formats memory for injection into slot 3.
- `SkillRegistry::catalog_text()`: Already formats skill catalog for injection into slot 4.
- `ContextLoader` + `SubdirDiscovery` (Phase 14): Context file loading for slot 5.

### Established Patterns
- Frozen-snapshot: all content loaded once, immutable for session duration (Phases 11, 12, 14)
- `Arc<Mutex<>>` sharing for stateful session resources
- Security scanning via `scan_context_content()` for all user-controlled content
- `truncate_content()` with 70/20 head/tail ratio at 20K chars

### Integration Points
- `PromptBuilder::build()` callers in agent_loop.rs, handler.rs, main.rs — need to handle `(durable, ephemeral)` return type
- `Config` struct — needs `agent.personalities` map and personality file loading from HERMES_HOME/personalities/
- Future Phase 16 — will consume the durable/ephemeral split for cache_control breakpoint placement
- Future Phase 20 — /personality slash command will call into the overlay mechanism built here

</code_context>

<specifics>
## Specific Ideas

- The user provided detailed hermes-agent documentation showing the exact slot ordering, personality system, and SOUL.md behavior. This is the authoritative reference — port faithfully.
- SOUL.md is for durable identity/voice (tone, style, directness). AGENTS.md is for project-specific instructions. /personality is for temporary mode switches. Three distinct layers with clear separation of concerns.
- The PromptSlot enum with BTreeMap storage is the user's preferred implementation pattern — use discriminant-based ordering with `>= Timestamp` as the ephemeral boundary.
- Personality overlay goes in the ephemeral layer (slot 8), not prepended to SOUL.md in the durable layer. This preserves prompt cache across /personality switches.

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

### Reviewed Todos (not folded)
- "Add setup wizard and config scaffolding for gateway testing" — belongs in Phase 23 (Configuration & Setup Wizard). Already reviewed and deferred in Phases 12, 13, and 14.

</deferred>

---

*Phase: 15-10-layer-prompt-assembly*
*Context gathered: 2026-04-12*
