---
phase: 01-context-file-loading
verified: 2026-04-01T00:00:00Z
status: passed
score: 11/11 must-haves verified
re_verification: false
---

# Phase 1: Context File Loading — Verification Report

**Phase Goal:** Agent loads personality and project context files into the system prompt so every conversation reflects the configured identity and project awareness
**Verified:** 2026-04-01
**Status:** PASSED
**Re-verification:** No — initial verification

---

## Goal Achievement

### Observable Truths

| # | Truth | Status | Evidence |
|---|-------|--------|----------|
| 1 | SOUL.md is loaded from `$IRONHERMES_HOME`, not the working directory | VERIFIED | `load_soul_md()` calls `ironhermes_core::get_hermes_home().join("SOUL.md")` — prompt_builder.rs:55 |
| 2 | Project context uses priority chain: `.hermes.md > AGENTS.md > CLAUDE.md > .cursorrules` (first match wins) | VERIFIED | `candidates` slice in `load_project_context()` ordered exactly per spec, `return` on first non-empty match — prompt_builder.rs:91–123 |
| 3 | Content exceeding 20K chars is truncated with head/tail split (70%/20%) and a marker | VERIFIED | `CONTEXT_FILE_MAX_CHARS = 20_000`, ratios 0.7/0.2, marker `[...truncated ...]` — context_scanner.rs:4–6, 76–94 |
| 4 | Assembly order is: identity (SOUL.md or default) > project context > AGENTS.md from IRONHERMES_HOME | VERIFIED | `build()` assembles parts in order: identity → platform_hint → TOOL_USE_GUIDANCE → project_context → agents_md_content — prompt_builder.rs:127–157 |
| 5 | Context files with prompt injection patterns are blocked with a warning message | VERIFIED | 10 `THREAT_PATTERNS` + invisible unicode; `warn!()` emitted and `[BLOCKED: ...]` returned — context_scanner.rs:44–72 |
| 6 | System prompt is built once and frozen for the session duration | VERIFIED | `cwd` captured once per entry point; `build_system_message()` called once; `run_agent_turn()` reuses `messages` vec without rebuilding — main.rs:211–214, 253–256 |
| 7 | CLI loads SOUL.md from IRONHERMES_HOME into the system prompt at session start | VERIFIED | `run_single()` and `run_chat()` both call `.load_context(&cwd)` before `.build_system_message()` — main.rs:212–213, 254–255 |
| 8 | CLI loads project context from cwd using priority chain at session start | VERIFIED | `load_context(&cwd)` dispatches to `load_project_context(cwd)` — prompt_builder.rs:47–52 |
| 9 | CLI loads AGENTS.md from IRONHERMES_HOME into the system prompt after project context | VERIFIED | `load_agents_md()` reads `get_hermes_home().join("AGENTS.md")`, wraps as `## AGENTS.md\n\n{content}`, assembled last — prompt_builder.rs:72–89, 151–155 |
| 10 | Context is frozen at session start — mid-session file edits do not change the prompt | VERIFIED | `system_msg` stored as first element of `messages` vec and never rebuilt; `run_agent_turn()` takes the existing vec — main.rs:258, 335–360 |
| 11 | `cargo build --bin ironhermes` compiles and runs successfully | VERIFIED | Build exits 0 in 0.49s, no errors |

**Score:** 11/11 truths verified

---

### Required Artifacts

| Artifact | Expected | Status | Details |
|----------|----------|--------|---------|
| `crates/ironhermes-agent/src/context_scanner.rs` | Security scanning for context file content | VERIFIED | 192 lines; exports `scan_context_content`, `truncate_content`; defines `THREAT_PATTERNS`, `THREAT_NAMES`, `INVISIBLE_CHARS`; 12 tests |
| `crates/ironhermes-agent/src/prompt_builder.rs` | System prompt assembly with layered context loading | VERIFIED | 323 lines; `PromptBuilder` struct with `load_context`, `load_soul_md`, `load_project_context`, `load_agents_md`, `build`, `build_system_message`; 6 tests |
| `crates/ironhermes-agent/src/lib.rs` | Module exports | VERIFIED | Declares `pub mod context_scanner`; re-exports `scan_context_content`, `truncate_content`, `CONTEXT_FILE_MAX_CHARS` |
| `crates/ironhermes-cli/src/main.rs` | CLI entry point wiring `PromptBuilder.load_context()` | VERIFIED | `load_context(&cwd)` called at lines 213 and 255; `current_dir()` captured at lines 211 and 253 |

---

### Key Link Verification

| From | To | Via | Status | Details |
|------|----|-----|--------|---------|
| `prompt_builder.rs` | `context_scanner.rs` | `scan_context_content()` call before injecting any context file content | VERIFIED | Called in `load_soul_md` (line 58), `load_agents_md` (line 76), `load_project_context` (line 108) — every read path goes through scan |
| `prompt_builder.rs` | `ironhermes-core/src/constants.rs` | `get_hermes_home()` for SOUL.md path resolution | VERIFIED | `ironhermes_core::get_hermes_home()` called at prompt_builder.rs:55 (SOUL.md) and :73 (AGENTS.md) |
| `main.rs` | `prompt_builder.rs` | `PromptBuilder::new().load_context(&cwd).build_system_message()` | VERIFIED | Chained call present at both `run_single` (lines 212–214) and `run_chat` (lines 254–256) |

---

### Data-Flow Trace (Level 4)

Not applicable — this phase produces no components that render dynamic data from a database or API. All data flows are file-system reads → string assembly → `ChatMessage::system()`.

---

### Behavioral Spot-Checks

| Behavior | Command | Result | Status |
|----------|---------|--------|--------|
| All context_scanner and prompt_builder tests pass | `cargo test -p ironhermes-agent -- --test-threads=1` | 20 passed; 0 failed | PASS |
| Binary compiles clean | `cargo build --bin ironhermes` | Finished in 0.49s, exit 0 | PASS |
| Workspace clippy clean | `cargo clippy --workspace -- -D warnings` | Finished in 1.23s, exit 0 | PASS |
| Old `load_context_files` static removed | `grep "fn load_context_files" prompt_builder.rs` | No matches | PASS |
| `load_context` wired at both CLI entry points | `grep "load_context" main.rs` | Lines 213, 255 | PASS |

---

### Requirements Coverage

| Requirement | Source Plan | Description | Status | Evidence |
|-------------|------------|-------------|--------|----------|
| CTX-01 | 01-01-PLAN.md, 01-02-PLAN.md | Agent loads SOUL.md from IRONHERMES_HOME into system prompt as personality/identity | SATISFIED | `load_soul_md()` reads `get_hermes_home()/SOUL.md`; wired in CLI via `load_context(&cwd)` |
| CTX-02 | 01-01-PLAN.md | Agent loads AGENTS.md from IRONHERMES_HOME into system prompt as capability definitions | SATISFIED | `load_agents_md()` reads `get_hermes_home()/AGENTS.md`, wrapped as `## AGENTS.md\n\n{content}`, assembled in `build()` |
| CTX-03 | 01-01-PLAN.md, 01-02-PLAN.md | Agent loads project-level context files from working directory | SATISFIED | `load_project_context(cwd)` checks `.hermes.md`, `HERMES.md`, `AGENTS.md`, `agents.md`, `CLAUDE.md`, `claude.md`, `.cursorrules` |
| CTX-04 | 01-01-PLAN.md, 01-02-PLAN.md | Context files loaded once at session start (frozen-snapshot) | SATISFIED | `cwd` captured once per entry; `system_msg` built once and stored in `messages[0]`; never rebuilt per turn |
| CTX-05 | 01-01-PLAN.md, 01-02-PLAN.md | Priority-based context assembly: SOUL.md > project context > AGENTS.md | SATISFIED | `build()` assembly order verified; `test_assembly_order` test validates positional ordering |

**Coverage: 5/5 Phase 1 requirements satisfied. No orphaned requirements.**

Note: 01-01-PLAN.md declares `[CTX-01, CTX-02, CTX-03, CTX-04, CTX-05]` and 01-02-PLAN.md declares `[CTX-01, CTX-03, CTX-04, CTX-05]`. CTX-02 is absent from 01-02-PLAN.md requirements list but is implemented via the `load_agents_md()` path wired through `load_context()` in both CLI entry points. CTX-02 is fully satisfied.

---

### Anti-Patterns Found

| File | Line | Pattern | Severity | Impact |
|------|------|---------|----------|--------|
| `prompt_builder.rs` | 175 | `#[allow(dead_code)]` on `model` field | Info | `model` field retained per spec but unused in current code; suppressed intentionally per SUMMARY decision |

No blockers, no stubs, no hardcoded empty returns, no TODO/FIXME/placeholder comments in phase files.

---

### Human Verification Required

#### 1. Live personality injection with SOUL.md

**Test:** Create `~/.ironhermes/SOUL.md` with a distinctive personality, run `ironhermes -e "who are you?"`, observe response.
**Expected:** Agent response reflects the SOUL.md content, not the default "IronHermes, an AI assistant created by Nous Research" identity.
**Why human:** Requires a configured API key and a real LLM call to observe personality in the response.

#### 2. Frozen-snapshot behavior under file mutation

**Test:** Start `ironhermes` in chat mode, edit `~/.ironhermes/SOUL.md` mid-session, continue the conversation.
**Expected:** The agent's identity does not change — responses still reflect the SOUL.md content from session start.
**Why human:** Requires interactive chat session; cannot test stateful session behavior programmatically.

---

### Gaps Summary

No gaps. All 11 observable truths verified, all 4 artifacts substantive and wired, all 3 key links confirmed, all 5 requirements satisfied, no blocker anti-patterns, all behavioral spot-checks pass.

---

_Verified: 2026-04-01_
_Verifier: Claude (gsd-verifier)_
