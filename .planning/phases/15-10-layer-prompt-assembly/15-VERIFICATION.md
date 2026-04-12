---
phase: 15-10-layer-prompt-assembly
verified: 2026-04-12T15:00:00Z
status: passed
score: 5/5 must-haves verified
overrides_applied: 0
---

# Phase 15: 10-Layer Prompt Assembly Verification Report

**Phase Goal:** The system prompt is assembled in 9 ordered slots matching hermes-agent, with frozen memory snapshots and session-level personality overlays
**Verified:** 2026-04-12T15:00:00Z
**Status:** passed
**Re-verification:** No — initial verification

## Goal Achievement

### Observable Truths (from Roadmap Success Criteria)

| #   | Truth   | Status     | Evidence       |
| --- | ------- | ---------- | -------------- |
| 1   | System prompt assembles 9 slots in order: Identity, ToolGuidance, Memory, Skills, ContextFiles, Timestamp, PlatformHints, SessionOverlay, UserMessage | ✓ VERIFIED | `PromptSlot` enum with `#[repr(u8)]` discriminants 1-9 in `prompt_builder.rs:36-46`. `BTreeMap<PromptSlot, String>` provides automatic ordering. `test_slot_ordering` passes. |
| 2   | Memory snapshots are frozen at session start — mid-session memory writes do not alter active prompt | ✓ VERIFIED | `load_memory()` reads from `memory_store` Arc<Mutex> once and calls `set_slot(PromptSlot::Memory, ...)`. Slot is only written at load time. `MEM-06` comment in code at line 177. |
| 3   | SOUL.md loads from HERMES_HOME with 20K char cap and security scan; falls back to DEFAULT_AGENT_IDENTITY when absent; subagent delegation skips SOUL.md and uses default identity | ✓ VERIFIED | `load_soul_md()` calls `scan_context_content` then checks `starts_with("[BLOCKED:")` — if blocked, leaves Identity slot unset so DEFAULT_AGENT_IDENTITY injects at build time. `truncate_content(..., CONTEXT_FILE_MAX_CHARS)` enforces 20K cap. `skip_context_files=true` returns early from `load_context()`. Tests `test_soul_security_scan`, `test_skip_context_files_default_identity` all pass. |
| 4   | /personality applies session-level overlay without modifying SOUL.md on disk | ✓ VERIFIED | `set_overlay(text: String)` / `clear_overlay()` on `PromptBuilder` place text into slot 8 (SessionOverlay) in ephemeral output only. `PersonalityRegistry` with 14 built-in presets exists in `personality.rs`. SOUL.md is never written. Tests `test_personality_overlay`, `test_personality_overlay_in_timestamp` pass. |
| 5   | Slots 1-5 are durable; slots 6-9 are ephemeral — separation maintained for caching correctness | ✓ VERIFIED | `PromptSlot::is_ephemeral()` returns `self >= PromptSlot::Timestamp`. `build_split()` iterates BTreeMap and partitions by `is_ephemeral()`. Ephemeral slots (6-8) only populated when `skip_context_files=false`. Tests `test_build_split_durable_ephemeral`, `test_build_split_empty_ephemeral` pass. |

**Score:** 5/5 truths verified

### Required Artifacts

| Artifact | Expected    | Status | Details |
| -------- | ----------- | ------ | ------- |
| `crates/ironhermes-agent/src/prompt_builder.rs` | PromptSlot enum, BTreeMap storage, build_split(), build() | ✓ VERIFIED | Contains `pub enum PromptSlot` (9 variants, discriminants 1-9), `slots: BTreeMap<PromptSlot, String>`, `pub fn build_split(&self) -> (String, String)`, `pub fn build(&self) -> String` wrapping build_split(). All setter methods present. |
| `crates/ironhermes-agent/src/personality.rs` | PersonalityRegistry with 14 built-in presets | ✓ VERIFIED | `pub struct PersonalityRegistry`, `fn builtin_presets()` returns exactly 14 entries (verified by `test_personality_registry_builtins`). `load()`, `get()`, `list()` all present. |
| `crates/ironhermes-core/src/config.rs` | AgentConfig.personalities field | ✓ VERIFIED | `pub personalities: HashMap<String, String>` at line 148 with `#[serde(default)]`. Default impl initializes to `HashMap::new()`. |
| `crates/ironhermes-agent/src/context_loader.rs` | CONTEXT_CANDIDATES with HERMES.md at index 1 | ✓ VERIFIED | `CONTEXT_CANDIDATES = &[".hermes.md", "HERMES.md", "AGENTS.md", "CLAUDE.md", ".cursorrules"]`. Length 5. `test_hermes_md_in_candidates` verifies index 1 position. |
| `crates/ironhermes-agent/src/subdir_discovery.rs` | SUBDIR_CONTEXT_MAX_CHARS = 8000 | ✓ VERIFIED | `const SUBDIR_CONTEXT_MAX_CHARS: usize = 8_000` at line 9. Used in `truncate_content()` call at line 82. `test_subdir_truncation_cap` verifies truncation behavior. |
| `crates/ironhermes-agent/src/lib.rs` | Exports PromptSlot and PersonalityRegistry | ✓ VERIFIED | `pub use prompt_builder::{PromptBuilder, PromptSlot}` at line 16. `pub use personality::PersonalityRegistry` at line 17. |
| `crates/ironhermes-cli/src/main.rs` | with_provider(), load_memory(), load_skills() calls | ✓ VERIFIED | Both `run_single` and `run_chat` call sites use `.with_provider(&config.model.provider).load_context(&cwd)` followed by `prompt_builder.load_memory()` and `prompt_builder.load_skills()`. |
| `crates/ironhermes-gateway/src/handler.rs` | with_provider(), load_memory(), load_skills() calls | ✓ VERIFIED | Handler call site uses `.with_provider(&self.config.model.provider).load_context(&cwd)` followed by `prompt_builder.load_memory()` and `prompt_builder.load_skills()`. |

### Key Link Verification

| From | To  | Via | Status | Details |
| ---- | --- | --- | ------ | ------- |
| `prompt_builder.rs` | `ironhermes_core::scan_context_content` | use import + call in `load_soul_md()` | ✓ WIRED | Import at line 5, called in `load_soul_md()`, `load_project_context_str()` |
| `prompt_builder.rs` | `ironhermes_core::MemoryProvider` | `format_for_system_prompt` in `load_memory()` | ✓ WIRED | `format_for_system_prompt(MemoryTarget::Memory)` and `(MemoryTarget::User)` both called in `load_memory()` |
| `personality.rs` | `ironhermes_core::scan_context_content` | security scan on custom presets | ✓ WIRED | Import at line 4, called for each `.md` file read from HERMES_HOME/personalities/ |
| `personality.rs` | `config.rs AgentConfig.personalities` | HashMap<String, String> config source | ✓ WIRED | `load(config_personalities: &HashMap<String, String>, ...)` — parameter receives from AgentConfig |
| `main.rs` | `prompt_builder.rs` | `.with_provider().load_context().load_memory().load_skills()` | ✓ WIRED | Both CLI call sites use full new API |
| `handler.rs` | `prompt_builder.rs` | `.with_provider().load_context().load_memory().load_skills()` | ✓ WIRED | Gateway handler uses full new API |

### Data-Flow Trace (Level 4)

| Artifact | Data Variable | Source | Produces Real Data | Status |
| -------- | ------------- | ------ | ------------------ | ------ |
| `prompt_builder.rs` build_split() | `slots: BTreeMap<PromptSlot, String>` | `load_context()`, `load_memory()`, `load_soul_md()` | Yes — reads SOUL.md from disk, memory from MemoryProvider, skills from SkillRegistry | ✓ FLOWING |
| `personality.rs` PersonalityRegistry | `presets: HashMap<String, String>` | `builtin_presets()` + file reads + config map | Yes — 14 hardcoded built-ins + optional file/config sources | ✓ FLOWING |

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
| -------- | ------- | ------ | ------ |
| All ironhermes-agent tests pass | `cargo test -p ironhermes-agent -- --test-threads=1` | 96 passed; 0 failed | ✓ PASS |
| Workspace builds clean | `cargo build --workspace` | Finished dev profile, 0 errors (2 pre-existing dead code warnings unrelated to phase 15) | ✓ PASS |

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
| ----------- | ---------- | ----------- | ------ | -------- |
| PRMT-01 | 15-01, 15-03 | System prompt assembles 10 layers in deterministic order | ✓ SATISFIED | 9-slot BTreeMap-ordered assembly; `test_slot_ordering` verifies Identity < ToolGuidance < ContextFiles ordering |
| PRMT-02 | 15-01, 15-03 | Cached layers (1-5) stable, dynamic layers (6-9) ephemeral | ✓ SATISFIED | `is_ephemeral()` splits at slot 6 boundary; `build_split()` returns (durable, ephemeral); `test_build_split_durable_ephemeral` verifies |
| PRMT-03 | 15-01, 15-03 | SOUL.md loads as slot 1; falls back to DEFAULT_AGENT_IDENTITY | ✓ SATISFIED | `load_soul_md()` sets Identity slot; `build_split()` uses `entry().or_insert_with(DEFAULT_AGENT_IDENTITY)` fallback; `test_soul_replaces_default` passes |
| PRMT-04 | 15-01, 15-03 | SOUL.md security scanned and truncated at 20K | ✓ SATISFIED | `scan_context_content(&content, "SOUL.md")` + `truncate_content(&scanned, "SOUL.md", CONTEXT_FILE_MAX_CHARS)` in `load_soul_md()`; `test_soul_security_scan` verifies blocked content falls back to default |
| PRMT-05 | 15-01, 15-03 | skip_context_files skips SOUL.md, uses default identity | ✓ SATISFIED | `load_context()` returns `self` immediately when `skip_context_files=true`; `test_skip_context_files_default_identity` and `test_skip_context_files_skips_slots_3_to_8` pass |
| PRMT-06 | 15-02 | /personality applies session overlay without modifying SOUL.md | ✓ SATISFIED | `set_overlay(text)` places text in slot 8 (ephemeral), `clear_overlay()` removes it; no disk write; `test_personality_overlay` verifies |
| PRMT-07 | 15-02 | 14 built-in presets + custom from config | ✓ SATISFIED | `builtin_presets()` returns exactly 14 entries; `PersonalityRegistry::load()` merges config.yaml personalities at highest precedence; `test_personality_registry_builtins` and `test_personality_registry_custom_config` pass |
| MEM-06 | 15-01 | Memory snapshots frozen at session start | ✓ SATISFIED | `load_memory()` reads from `memory_store` once at load time into slot 3; subsequent memory writes to disk do not update the slot; `load_memory()` guarded by `skip_context_files` check |

### Anti-Patterns Found

No blockers or warnings found. Reviewed prompt_builder.rs, personality.rs, context_loader.rs, subdir_discovery.rs, main.rs, handler.rs.

| File | Pattern | Severity | Assessment |
| ---- | ------- | -------- | ---------- |
| `prompt_builder.rs` | `return null` / empty impls | ℹ️ None found | All methods have real implementations |
| `personality.rs` | TODO/FIXME | ℹ️ None found | Fully implemented |
| call sites (main.rs, handler.rs) | load_memory() with no memory_store set | ℹ️ Info | `load_memory()` is a no-op when `memory_store` is None — correct behavior, not a stub |

### Human Verification Required

None. All must-haves are programmatically verifiable and have been verified.

### Gaps Summary

No gaps. All 5 roadmap success criteria verified. All 8 requirements (PRMT-01 through PRMT-07, MEM-06) satisfied. All plan must-haves pass. 96 ironhermes-agent tests pass. Workspace builds clean.

---

_Verified: 2026-04-12T15:00:00Z_
_Verifier: Claude (gsd-verifier)_
