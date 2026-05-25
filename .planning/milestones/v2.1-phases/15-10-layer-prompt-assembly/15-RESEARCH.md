# Phase 15: 10-Layer Prompt Assembly - Research

**Researched:** 2026-04-12
**Domain:** Rust prompt assembly refactor — PromptSlot enum, BTreeMap storage, durable/ephemeral split, personality overlay system
**Confidence:** HIGH

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

**D-01:** Follow the 9-slot PromptSlot enum from hermes-agent reference, NOT the 10-layer spec in PRMT-01. Authoritative ordering: (1) Identity, (2) ToolGuidance, (3) Memory, (4) Skills, (5) ContextFiles, (6) Timestamp, (7) PlatformHints, (8) SessionOverlay, (9) UserMessage.

**D-02:** Provider block and optional system message are NOT separate slots. Provider info folds into Identity or ToolGuidance. Config-driven system message folds into SessionOverlay.

**D-03:** `PromptSlot` is a `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]` enum with discriminant values 1-9. `PromptBuilder` uses `BTreeMap<PromptSlot, String>` for ordered storage.

**D-04:** Cache breakpoint is between slot 5 (ContextFiles) and slot 6 (Timestamp). Slots 1-5 are durable (stable across turns, cacheable). Slots 6-9 are ephemeral (regenerated per turn).

**D-05:** `build()` returns a `(String, String)` tuple via `build_split()` — actually: `build_split() -> (String, String)` is the new primary; `build() -> String` is kept as a convenience wrapper joining the two parts. Split logic: `slot >= PromptSlot::Timestamp` goes to ephemeral.

**D-06:** Durable slots are frozen at session start — mid-session file edits do NOT change the active prompt.

**D-07:** /personality applies overlay as slot 8 (SessionOverlay), NOT prepended to slot 1 (Identity). SOUL.md stays durable; personality overlays live in the ephemeral layer.

**D-08:** 14 built-in personality presets: helpful, concise, technical, creative, teacher, kawaii, catgirl, pirate, shakespeare, surfer, noir, uwu, philosopher, hype.

**D-09:** Custom presets from two merged sources: (1) `config.yaml` under `agent.personalities` namespace, (2) `HERMES_HOME/personalities/` directory as `.md` files. Both merged at load time; config.yaml takes precedence on name collision.

**D-10:** /personality with no argument lists presets. /personality <name> activates. /personality off removes overlay. Only one overlay active at a time.

**D-11:** Slot 3 (Memory): Frozen MEMORY.md and USER.md snapshots with capacity headers. Frozen at session start per MEM-06.

**D-12:** Slot 6 (Timestamp): Current UTC date/time, session identifier, current turn number, and active personality overlay name (if any).

**D-13:** Slot 7 (PlatformHints): Platform-specific formatting guidance — moves from current position to ephemeral slot 7.

**D-14:** Slot 2 (ToolGuidance): Includes model identity and provider context (model name, provider name, known context window size).

**D-15:** Subagents get Identity (DEFAULT_AGENT_IDENTITY) + ToolGuidance only — slots 3-8 skipped entirely.

**D-16:** Blocked tools for subagents already implemented in Phase 9 — PromptBuilder respects `skip_context_files` to skip slots 3-8.

**D-17:** No separate `agent.system_message` config key. SessionOverlay slot (8) is exclusively for /personality overlays.

**D-18:** `HERMES.md` is also a valid context file name alongside `.hermes.md` — add to priority chain candidates.

**D-19:** `.cursor/rules/*.mdc` rule modules are supported in addition to `.cursorrules`.

**D-20:** Subdirectory discovery truncation cap is 8,000 chars per file.

**D-21:** Context files assembled under a `# Project Context` header. SOUL.md content inserted directly without wrapper text.

**D-22:** Add `build_split() -> (String, String)` as the new primary method.

**D-23:** Refactor existing `build() -> String` to call `build_split()` internally. No breaking change.

**D-24:** Agent loop checks if LLM adapter supports multi-block system prompts; if so, passes split parts separately. Otherwise concatenates via `build()`.

### Claude's Discretion

- Exact text content of each of the 14 built-in personality presets
- Whether PromptSlot::UserMessage (slot 9) is populated by PromptBuilder or by callers
- Internal API for populating individual slots (setter methods vs builder pattern)
- How /personality command integrates with the slash command system (Phase 20 scope, but overlay mechanism is Phase 15)
- Personality preset loading: eager at startup vs lazy on first /personality call

### Deferred Ideas (OUT OF SCOPE)

None from discussion. "Add setup wizard and config scaffolding for gateway testing" deferred to Phase 23.
</user_constraints>

---

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| PRMT-01 | System prompt assembles layers in order (9-slot model per D-01) | PromptSlot enum + BTreeMap pattern; current `build()` lacks ordered enum — must restructure |
| PRMT-02 | Cached layers (1-5) stable across turns; dynamic layers (6-9) ephemeral | `build_split()` returning `(durable, ephemeral)`; durable/ephemeral split at slot 5/6 boundary |
| PRMT-03 | SOUL.md loads from HERMES_HOME as slot 1; falls back to DEFAULT_AGENT_IDENTITY | `load_soul_md()` already exists in PromptBuilder — needs to be mapped to slot 1 in new BTreeMap |
| PRMT-04 | SOUL.md security scanned + truncated at 20K chars | `scan_context_content()` + `truncate_content()` already implement this; wiring preserved |
| PRMT-05 | skip_context_files for subagent delegation skips slots 3-8 | `skip_context_files` flag already exists; must be updated to skip slots 3-8 (not just context files) |
| PRMT-06 | /personality command applies session-level overlay in slot 8 | New SessionOverlay slot mechanism; overlay stored in PromptBuilder state; ephemeral layer |
| PRMT-07 | Built-in presets + custom presets from config | PersonalityRegistry (new); 14 built-ins + config.yaml `agent.personalities` + HERMES_HOME/personalities/ |
| MEM-06 | Memory snapshots frozen at session start | Already implemented via `MemoryStore::snapshot` + `format_for_system_prompt()` — maps to slot 3 |
</phase_requirements>

---

## Summary

Phase 15 restructures the existing `PromptBuilder` from an ad-hoc `Vec<String>` assembly to a `BTreeMap<PromptSlot, String>` ordered-slot model matching the hermes-agent reference architecture. The core work is: (1) define the `PromptSlot` enum with `Ord` derived for BTreeMap ordering, (2) migrate all existing content loading into slot setters, (3) implement the durable/ephemeral split at the slot 5/6 boundary returning `(String, String)` from `build_split()`, (4) add the personality overlay system as slot 8 with a `PersonalityRegistry` containing 14 built-ins and custom preset loading from config + HERMES_HOME/personalities/, and (5) update all call sites to handle the new API.

The existing codebase is well-positioned for this refactor. `MemoryStore` already has a frozen snapshot pattern (`self.snapshot` captured at `load_from_disk()` and never mutated). `scan_context_content()` and `truncate_content()` are reusable as-is. `ContextLoader` from Phase 14 provides the context file loading for slot 5. The primary structural change is replacing the `Vec<String>` + ad-hoc ordering with `BTreeMap<PromptSlot, String>` + discriminant ordering.

The personality overlay system is new infrastructure. `PersonalityRegistry` holds 14 built-in `&'static str` presets plus runtime-loaded custom presets. The active overlay is stored in `PromptBuilder` state and written to slot 8 (SessionOverlay) — because slot 8 is ephemeral, toggling personality does NOT invalidate the durable prompt cache.

**Primary recommendation:** Implement in three logical waves: (1) PromptSlot enum + BTreeMap migration + build_split(), (2) personality overlay system + PersonalityRegistry, (3) call site updates in agent_loop/handler/main/runner + CONTEXT_CANDIDATES update.

---

## Standard Stack

### Core (all existing — no new dependencies)

| Component | Version | Purpose | Status |
|-----------|---------|---------|--------|
| `BTreeMap<K,V>` | std | Ordered slot storage — Ord-derived enum keys give deterministic ordering | stdlib, no dep needed |
| `scan_context_content()` | codebase | Security scanning for SOUL.md + personality presets | Already in `ironhermes-core::context_scanner` |
| `truncate_content()` | codebase | 20K char cap with 70/20 head/tail | Already in `ironhermes-core::context_scanner` |
| `MemoryStore::format_for_system_prompt()` | codebase | Frozen snapshot for slot 3 | Already in `ironhermes-core::memory_store` |
| `SkillRegistry::catalog_text()` | codebase | Skill catalog for slot 4 | Already in `ironhermes-core` |
| `ContextLoader` | Phase 14 | Context file loading for slot 5 | Phase 14 output |
| `serde_yaml` | existing | Parsing `agent.personalities` config section | Already in Cargo.toml via config.rs |

### New Structures (no external deps)

| Structure | Purpose |
|-----------|---------|
| `PromptSlot` enum | Ordered slot discriminants 1-9 with `#[derive(Ord)]` |
| `PersonalityRegistry` | Holds built-in presets as `HashMap<&'static str, &'static str>` + custom presets |
| `AgentConfig::personalities` field | `HashMap<String, String>` for inline config.yaml presets |

**Installation:** No new cargo dependencies required. [VERIFIED: codebase inspection — all needed types already exist]

---

## Architecture Patterns

### Recommended Structure

```
crates/ironhermes-agent/src/
├── prompt_builder.rs        # PRIMARY CHANGE: PromptSlot enum + BTreeMap + build_split()
├── personality.rs           # NEW: PersonalityRegistry with built-ins + custom loading
├── context_loader.rs        # UPDATE: add HERMES.md + .cursor/rules/*.mdc to CONTEXT_CANDIDATES
└── lib.rs                   # UPDATE: pub use personality::PersonalityRegistry

crates/ironhermes-core/src/
└── config.rs                # UPDATE: AgentConfig gets personalities: HashMap<String, String>
```

### Pattern 1: PromptSlot Enum with BTreeMap

**What:** Replace `Vec<String>` with `BTreeMap<PromptSlot, String>`. Slot enum uses `#[derive(Ord)]` so BTreeMap iteration is always in discriminant order.

**When to use:** Any time a slot needs to be set or updated. BTreeMap gives idempotent slot setting — calling `set_slot(PromptSlot::Identity, content)` twice is safe; second call overwrites first.

**Example:**
```rust
// Source: hermes-agent reference + CONTEXT.md D-03
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum PromptSlot {
    Identity       = 1,
    ToolGuidance   = 2,
    Memory         = 3,
    Skills         = 4,
    ContextFiles   = 5,
    Timestamp      = 6,
    PlatformHints  = 7,
    SessionOverlay = 8,
    UserMessage    = 9,
}

impl PromptSlot {
    /// Returns true if this slot belongs to the ephemeral (per-turn) section.
    /// Cache breakpoint is BETWEEN slot 5 and slot 6 (D-04).
    pub fn is_ephemeral(self) -> bool {
        self >= PromptSlot::Timestamp
    }
}
```

### Pattern 2: build_split() Durable/Ephemeral Split

**What:** `build_split()` iterates `BTreeMap` in key order, routes to durable or ephemeral vec based on `slot.is_ephemeral()`, joins each vec with double newlines, returns `(durable, ephemeral)`.

**When to use:** Called by agent_loop when LLM adapter supports multi-block system prompts (Phase 16). Called by `build()` which joins the two halves for single-string callers.

**Example:**
```rust
// Source: CONTEXT.md D-22, D-23, D-05
pub fn build_split(&self) -> (String, String) {
    let mut durable_parts: Vec<String> = Vec::new();
    let mut ephemeral_parts: Vec<String> = Vec::new();

    for (slot, content) in &self.slots {
        if slot.is_ephemeral() {
            ephemeral_parts.push(content.clone());
        } else {
            durable_parts.push(content.clone());
        }
    }

    (durable_parts.join("\n\n"), ephemeral_parts.join("\n\n"))
}

pub fn build(&self) -> String {
    let (durable, ephemeral) = self.build_split();
    if ephemeral.is_empty() {
        durable
    } else if durable.is_empty() {
        ephemeral
    } else {
        format!("{}\n\n{}", durable, ephemeral)
    }
}
```

### Pattern 3: PersonalityRegistry

**What:** A struct owning built-in presets (static map) + runtime-loaded custom presets. Loaded once at startup. The active overlay is stored in `PromptBuilder` as `Option<String>`.

**Loading precedence:** config.yaml `agent.personalities` beats HERMES_HOME/personalities/ on name collision (D-09). Custom presets are NOT security scanned for injection (they come from trusted config/home directory, same trust level as SOUL.md). [ASSUMED — the CONTEXT.md does not explicitly state whether custom personality presets are security scanned. The same scan that applies to SOUL.md may apply here.]

**Example:**
```rust
// Source: CONTEXT.md D-08, D-09
pub struct PersonalityRegistry {
    presets: HashMap<String, String>,  // name -> overlay text
}

impl PersonalityRegistry {
    pub fn load(config_personalities: &HashMap<String, String>, hermes_home: &Path) -> Self {
        let mut presets: HashMap<String, String> = builtin_presets();

        // Load from HERMES_HOME/personalities/*.md (lower precedence)
        if let Ok(entries) = std::fs::read_dir(hermes_home.join("personalities")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("md") {
                    if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                        if let Ok(content) = std::fs::read_to_string(&path) {
                            presets.entry(name.to_string()).or_insert(content);
                        }
                    }
                }
            }
        }

        // config.yaml takes precedence (overwrite)
        for (name, text) in config_personalities {
            presets.insert(name.clone(), text.clone());
        }

        Self { presets }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.presets.get(name).map(|s| s.as_str())
    }

    pub fn list(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.presets.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }
}
```

### Pattern 4: Subagent Skip via slot filter

**What:** When `skip_context_files` is true, only slots 1 (Identity) and 2 (ToolGuidance) are populated. Slots 3-8 are never set. The check happens at content-loading time, not at build time.

**Why:** Simpler and safer to simply not call the slot setters for slots 3-8 when `skip_context_files=true`, rather than filtering at `build_split()`.

### Anti-Patterns to Avoid

- **Setting slots after calling build_split():** The design is load-once (frozen snapshot). Call all setters before the first build.
- **Storing active_personality inside BTreeMap slot 8 at load time:** Slot 8 is the SESSION OVERLAY — it changes when /personality is called mid-session. Store `active_overlay: Option<String>` in PromptBuilder and write to slot 8 in `build_split()` dynamically. This is the correct model for ephemeral slots that change per-call.
- **Making build_split() mutate self:** `build_split()` should be `&self` — it reads the stored overlay and writes it into the output at call time.
- **Forgetting that ContextLoader (Phase 14) is separate from the context candidates list in context_loader.rs:** The `CONTEXT_CANDIDATES` constant in `context_loader.rs` needs HERMES.md added (D-18) and `.cursor/rules/*.mdc` glob added (D-19), even though ContextLoader is Phase 14's output.

---

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Ordered slot storage | Custom sorted Vec | `BTreeMap<PromptSlot, String>` | Ord-derived enum gives free deterministic ordering; BTreeMap iteration is always sorted |
| Security scanning for personality text | Custom regex | `scan_context_content()` | Already handles 10 threat patterns + invisible unicode; consistent with SOUL.md scanning |
| Timestamp formatting | Manual UTC formatting | `chrono::Utc::now()` (already in Cargo.toml) | Already used elsewhere in codebase |
| YAML config deserialization | Manual parsing | `serde_yaml` + `#[serde(default)]` | Already in use for all config sections; `agent.personalities: HashMap<String, String>` just needs adding |

**Key insight:** The entire pattern for this phase is already present in the codebase. The work is restructuring and wiring existing components into the new slot model, not building new infrastructure from scratch.

---

## Common Pitfalls

### Pitfall 1: Rust BTreeMap key ordering requires Ord, not just PartialOrd
**What goes wrong:** `BTreeMap<PromptSlot, String>` requires `PromptSlot: Ord`. If only `PartialOrd` is derived, the compiler errors. All four — `PartialEq`, `Eq`, `PartialOrd`, `Ord` — must be in the derive list.
**Why it happens:** Rust's `#[derive(Ord)]` requires `Eq` + `PartialOrd` to already be derived.
**How to avoid:** Use the full derive: `#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]` as specified in D-03. [VERIFIED: Rust stdlib BTreeMap requires K: Ord]

### Pitfall 2: Existing tests call `builder.build()` expecting a `String`
**What goes wrong:** If `build()` is changed to return `(String, String)` (D-05 first read), all 15+ test call sites break.
**Why it happens:** D-05 says `build_split()` returns `(String, String)`. D-23 clarifies `build()` remains `-> String` as a convenience method calling `build_split()` internally.
**How to avoid:** Per D-22/D-23: add `build_split() -> (String, String)`, refactor `build() -> String` to call `build_split()` and join. All existing tests continue to pass unchanged. [VERIFIED: codebase — 15+ test call sites use `.build()` returning String]

### Pitfall 3: PromptBuilder assembly order breaks existing tests
**What goes wrong:** Current assembly order is: identity, platform_hint, tool_guidance, project_context, agents_md, skills, memory. New slot order is: 1-Identity, 2-ToolGuidance, 3-Memory, 4-Skills, 5-ContextFiles, 6-Timestamp, 7-PlatformHints, 8-SessionOverlay, 9-UserMessage. Tests that check relative positions (`soul_pos < project_pos < agents_pos`) will break if slot ordering changes.
**Why it happens:** The `test_assembly_order` test asserts `SOUL < PROJECT < AGENTS`. Under new ordering, AGENTS.md becomes part of ContextFiles (slot 5), which is after Memory (slot 3). The relative position of SOUL, PROJECT CONTEXT, AGENTS.md is preserved (1, 5, 5 — all durable), but memory now appears before skills and context files.
**How to avoid:** Update assembly order tests to match new slot ordering. The test checking SOUL < PROJECT < AGENTS remains valid since both project context and AGENTS.md are in slot 5 (ContextFiles) under the `# Project Context` header. [ASSUMED — exact sub-ordering of AGENTS.md within slot 5 needs to be confirmed against hermes-agent reference.]

### Pitfall 4: context_loader.rs CONTEXT_CANDIDATES missing HERMES.md
**What goes wrong:** D-18 adds `HERMES.md` to the priority chain. Currently `CONTEXT_CANDIDATES = [".hermes.md", "AGENTS.md", "CLAUDE.md", ".cursorrules"]`. Test `test_context_candidates_case_sensitive` asserts `CONTEXT_CANDIDATES.len() == 4` and explicitly asserts `!CONTEXT_CANDIDATES.contains(&"HERMES.md")`.
**Why it happens:** Phase 14 decision explicitly excluded HERMES.md; Phase 15 decision D-18 re-adds it.
**How to avoid:** Add `HERMES.md` after `.hermes.md` in the candidates list AND update `test_context_candidates_case_sensitive` to expect `len() == 5` and to contain `HERMES.md`. [VERIFIED: context_loader.rs line 6 has exactly 4 candidates; test line 189 asserts len()==4]

### Pitfall 5: Personality overlay at slot 8 but slot 8 is ephemeral — must recalculate every build
**What goes wrong:** If `active_overlay` is stored as a BTreeMap entry set once, it won't update when /personality changes the overlay mid-session.
**Why it happens:** Ephemeral slots (6-9) are "regenerated per turn" (D-04). The overlay name can change between turns.
**How to avoid:** Store `active_overlay: Option<String>` as a separate field in `PromptBuilder`. In `build_split()`, write slot 8 dynamically from `self.active_overlay` rather than pre-populating BTreeMap entry 8.

### Pitfall 6: Config backward compatibility for `agent.personalities`
**What goes wrong:** Adding `personalities: HashMap<String, String>` to `AgentConfig` causes parse failure for existing config.yaml files that have no `agent.personalities` section.
**Why it happens:** `serde_yaml` requires explicit `#[serde(default)]` on the field (or on the struct) to skip missing keys.
**How to avoid:** Use `#[serde(default)]` on the new `personalities` field. Pattern is already established in `AgentConfig`. [VERIFIED: config.rs uses `#[serde(default)]` consistently on all config structs]

---

## Code Examples

### Slot population (setter pattern)

```rust
// Source: CONTEXT.md D-03, D-06 — slots populated at load time (frozen snapshot)
impl PromptBuilder {
    fn set_slot(&mut self, slot: PromptSlot, content: String) {
        if !content.trim().is_empty() {
            self.slots.insert(slot, content);
        }
    }

    // Called from load_context() for slot 1
    fn load_soul_md(&mut self) {
        let path = ironhermes_core::get_hermes_home().join("SOUL.md");
        match std::fs::read_to_string(&path) {
            Ok(content) if !content.trim().is_empty() => {
                let scanned = scan_context_content(&content, "SOUL.md");
                let truncated = truncate_content(&scanned, "SOUL.md", CONTEXT_FILE_MAX_CHARS);
                self.set_slot(PromptSlot::Identity, truncated);
            }
            _ => { /* fallback to DEFAULT_AGENT_IDENTITY stays in Identity slot default */ }
        }
    }
}
```

### Timestamp slot (slot 6, ephemeral)

```rust
// Source: CONTEXT.md D-12 — regenerated per build_split() call
fn build_timestamp_block(&self) -> String {
    let now = chrono::Utc::now();
    let mut parts = vec![
        format!("Current time: {}", now.format("%Y-%m-%d %H:%M:%S UTC")),
    ];
    if let Some(ref session_id) = self.session_id {
        parts.push(format!("Session: {}", session_id));
    }
    if let Some(ref overlay) = self.active_overlay {
        parts.push(format!("Active personality: {}", overlay));
    }
    parts.join("\n")
}
```

### Config extension for personalities

```rust
// Source: CONTEXT.md D-09, D-17 — config.yaml under agent.personalities namespace
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_turns: usize,
    pub context_compression: f64,
    pub tool_delay_secs: f64,
    #[serde(default)]
    pub personalities: HashMap<String, String>,  // NEW for Phase 15
}
```

### PromptBuilder new fields

```rust
// Source: CONTEXT.md D-03, D-07, D-12
pub struct PromptBuilder {
    model: String,
    platform: String,
    provider: String,              // NEW: for slot 2 ToolGuidance
    context_length: Option<usize>, // NEW: for slot 2 ToolGuidance
    session_id: Option<String>,    // NEW: for slot 6 Timestamp
    turn_number: Option<usize>,    // NEW: for slot 6 Timestamp
    active_overlay: Option<String>,// NEW: for slot 8 SessionOverlay (ephemeral)
    skip_context_files: bool,
    slots: BTreeMap<PromptSlot, String>,   // NEW: replaces all separate Option<String> fields
    memory_store: Option<Arc<Mutex<dyn MemoryProvider + Send>>>,
    skill_registry: Option<Arc<SkillRegistry>>,
}
```

---

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| Vec<String> append | BTreeMap<PromptSlot, String> | Phase 15 | Idempotent slot setting, deterministic order, clean durable/ephemeral split |
| build() -> String | build_split() -> (String, String) + build() -> String wrapper | Phase 15 | Enables Phase 16 cache_control breakpoint placement |
| Platform hint at position 2 (early) | Platform hint at slot 7 (ephemeral) | Phase 15 | Platform hint regenerated per turn; doesn't affect durable prompt cache |
| Personality: not implemented | PersonalityRegistry + slot 8 SessionOverlay | Phase 15 | Session-level persona switching without cache invalidation |
| Memory injected after skills | Memory at slot 3 (before skills at 4) | Phase 15 | Matches hermes-agent reference ordering |

---

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | Custom personality presets from HERMES_HOME/personalities/ are NOT security scanned (same trust as SOUL.md) | Architecture Patterns — PersonalityRegistry | If scan IS required, add `scan_context_content()` call in PersonalityRegistry::load() before inserting custom presets |
| A2 | AGENTS.md from HERMES_HOME is assembled as part of slot 5 (ContextFiles) under a `# Project Context` header, maintaining relative order after project context | Common Pitfalls — Pitfall 3 | If AGENTS.md should be a separate slot between Memory and Skills, the slot numbering changes; this is low-risk since D-21 says "context files assembled under `# Project Context` header" |
| A3 | PromptSlot::UserMessage (slot 9) is populated by callers (not PromptBuilder) | Architecture — this is left as Claude's Discretion | If PromptBuilder owns slot 9, it needs access to user messages; callers currently pass full messages array to agent_loop, not to PromptBuilder |

---

## Open Questions

1. **Does `active_overlay` in slot 8 get set in `build_split()` dynamically or pre-stored in slots map?**
   - What we know: Slot 8 is ephemeral (regenerated per turn). Active overlay changes mid-session.
   - What's unclear: Whether overlay content is computed in `build_split()` or written to `slots[SessionOverlay]` by `set_overlay()`.
   - Recommendation: Write to `slots[SessionOverlay]` at `set_overlay()` call time AND re-write on every `build_split()` call from `self.active_overlay`. Both work; the ephemeral-field approach (writing in build_split) is cleaner because it avoids stale state if `set_overlay` and `build_split` calls interleave. Use the dynamic approach.

2. **Where does `session_id` and `turn_number` come from at PromptBuilder time?**
   - What we know: Slot 6 (Timestamp) includes session ID and turn number (D-12).
   - What's unclear: PromptBuilder is constructed before the session loop starts. Session ID is known at construction time; turn number changes each turn.
   - Recommendation: Add `with_session_id(id)` setter. Turn number is NOT stored in PromptBuilder — it's passed to `build_split(turn: usize)` as a parameter, OR PromptBuilder has a `increment_turn()` method called by agent_loop each iteration. The simpler approach: pass turn number as parameter to `build_split()`.

---

## Environment Availability

Step 2.6: SKIPPED — this phase is a pure code refactor. No external dependencies beyond the existing Rust toolchain. [VERIFIED: codebase — all required types exist in-tree]

---

## Validation Architecture

### Test Framework

| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `tempfile` crate |
| Config file | none (inline `#[cfg(test)]` modules) |
| Quick run command | `cargo test -p ironhermes-agent -- --test-threads=1 2>&1` |
| Full suite command | `cargo test --workspace -- --test-threads=1 2>&1` |

Note: `--test-threads=1` is required because prompt_builder tests manipulate environment variables (`IRONHERMES_HOME`) with a static `ENV_MUTEX` lock. [VERIFIED: prompt_builder.rs lines 279-281]

### Phase Requirements → Test Map

| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| PRMT-01 | Slots 1-9 assemble in correct order | unit | `cargo test -p ironhermes-agent test_slot_ordering -- --test-threads=1` | Wave 0 |
| PRMT-02 | build_split() returns durable (slots 1-5) and ephemeral (slots 6-9) | unit | `cargo test -p ironhermes-agent test_build_split -- --test-threads=1` | Wave 0 |
| PRMT-03 | SOUL.md -> slot 1; fallback to DEFAULT_AGENT_IDENTITY | unit | `cargo test -p ironhermes-agent test_soul_replaces_default -- --test-threads=1` | ✅ (update) |
| PRMT-04 | SOUL.md security scanned + 20K truncation | unit | `cargo test -p ironhermes-agent test_soul_security_scan -- --test-threads=1` | Wave 0 |
| PRMT-05 | skip_context_files skips slots 3-8 | unit | `cargo test -p ironhermes-agent test_skip_context_files_default_identity -- --test-threads=1` | ✅ (update) |
| PRMT-06 | set_overlay() places content in slot 8; build returns overlay in ephemeral | unit | `cargo test -p ironhermes-agent test_personality_overlay -- --test-threads=1` | Wave 0 |
| PRMT-07 | PersonalityRegistry lists 14 built-ins + custom presets | unit | `cargo test -p ironhermes-agent test_personality_registry -- --test-threads=1` | Wave 0 |
| MEM-06 | Memory snapshot frozen: mid-session add does not change build output | unit | `cargo test -p ironhermes-core test_snapshot_frozen_after_load -- --test-threads=1` | ✅ exists |

### Sampling Rate

- **Per task commit:** `cargo test -p ironhermes-agent -- --test-threads=1 2>&1`
- **Per wave merge:** `cargo test --workspace -- --test-threads=1 2>&1`
- **Phase gate:** Full workspace test suite green before `/gsd-verify-work`

### Wave 0 Gaps

- [ ] `test_slot_ordering` — verify slots 1-9 appear in correct order in `build()` output — covers PRMT-01
- [ ] `test_build_split_durable_ephemeral` — verify `build_split()` partitions correctly — covers PRMT-02
- [ ] `test_soul_security_scan` — verify blocked SOUL.md falls back to default identity — covers PRMT-04
- [ ] `test_personality_overlay` — verify `set_overlay()` / `clear_overlay()` affect slot 8 — covers PRMT-06
- [ ] `test_personality_registry_builtins` — verify all 14 presets present — covers PRMT-07
- [ ] `test_personality_registry_custom_config` — verify config.yaml presets loaded + override HERMES_HOME — covers PRMT-07
- [ ] `test_hermes_md_in_candidates` — verify HERMES.md is in CONTEXT_CANDIDATES (D-18) — covers D-18
- [ ] `test_skip_context_files_skips_slots_3_to_8` — verify subagent gets only slots 1-2 — covers PRMT-05 (update of existing test)

Existing tests that need **updating** (not replacing):
- `test_assembly_order` — assert new slot ordering (SOUL < MEMORY < SKILLS < PROJECT CONTEXT)
- `test_context_candidates_case_sensitive` — update len() assertion from 4 to 5, add HERMES.md

---

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | — |
| V3 Session Management | no | — |
| V4 Access Control | no | — |
| V5 Input Validation | yes | `scan_context_content()` for all user-controlled content injected into system prompt |
| V6 Cryptography | no | — |

### Known Threat Patterns for prompt assembly

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via SOUL.md | Tampering | `scan_context_content()` + `truncate_content()` — already applied to SOUL.md |
| Prompt injection via custom personality .md files | Tampering | Security scan before injection (see A1 in Assumptions Log — clarify whether to apply) |
| Invisible unicode in personality overlays | Tampering | `scan_context_content()` detects 10 invisible unicode code points |
| Context length exhaustion via large personality file | Denial of Service | Apply same 20K char truncation used for SOUL.md to custom personality files |

---

## Project Constraints (from CLAUDE.md)

No `CLAUDE.md` was found at the repository root. [VERIFIED: `ls /Users/twilson/code/ironhermes/CLAUDE.md` → file does not exist]

Established project patterns observed in codebase:
- `#[serde(default)]` on all config structs for backward compatibility
- `unsafe { std::env::set_var }` in tests with static `Mutex` guards to prevent env var races
- `Arc<Mutex<dyn Trait + Send>>` for shared stateful resources
- Frozen-snapshot pattern for all session-start content (established in Phases 11, 12, 14)
- `scan_context_content()` + `truncate_content()` applied to ALL user-controlled text injected into system prompt

---

## Sources

### Primary (HIGH confidence)
- `crates/ironhermes-agent/src/prompt_builder.rs` — Current PromptBuilder implementation, all field names, test names, existing assembly order [VERIFIED: read in full]
- `crates/ironhermes-agent/src/context_loader.rs` — CONTEXT_CANDIDATES, find_git_root, strip_yaml_frontmatter [VERIFIED: read in full]
- `crates/ironhermes-core/src/memory_store.rs` — Frozen snapshot via `self.snapshot`, format_for_system_prompt() [VERIFIED: read in full]
- `crates/ironhermes-core/src/config.rs` — AgentConfig, serde(default) patterns, existing config structure [VERIFIED: read in full]
- `crates/ironhermes-core/src/context_scanner.rs` — scan_context_content(), truncate_content(), CONTEXT_FILE_MAX_CHARS=20000 [VERIFIED: read in full]
- `.planning/phases/15-10-layer-prompt-assembly/15-CONTEXT.md` — All locked decisions D-01 through D-24 [VERIFIED: read in full]
- `crates/ironhermes-cli/src/main.rs` lines 264-267 — PromptBuilder call site pattern [VERIFIED: read]
- `crates/ironhermes-gateway/src/handler.rs` lines 284-294 — PromptBuilder call site pattern [VERIFIED: read]

### Secondary (MEDIUM confidence)
- `crates/ironhermes-agent/src/agent_loop.rs` lines 1-80 — AgentLoop struct fields, AnyClient usage [VERIFIED: partial read]
- Rust stdlib BTreeMap documentation — requires K: Ord; iteration order is key-sorted [ASSUMED based on training + consistent with stdlib docs]

### Tertiary (LOW confidence)
- None

---

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all components verified in codebase
- Architecture: HIGH — patterns directly derived from CONTEXT.md decisions + existing code
- Pitfalls: HIGH — derived from direct code inspection of test names, assertion values, existing slot ordering
- Personality registry: MEDIUM — design derived from CONTEXT.md decisions; exact preset text is Claude's discretion

**Research date:** 2026-04-12
**Valid until:** 2026-05-12 (stable Rust project, no fast-moving external dependencies)
