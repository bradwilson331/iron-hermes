---
phase: 34b-context-system-parity
plan: "01"
subsystem: ironhermes-agent
tags: [context-refs, parser, expander, budget, argv-only, security, cwe-78, centralization]
dependency_graph:
  requires: [34b-00]
  provides: [context_refs-module-full, preprocess_context_references_async, AgentResult-context_warnings, run_turn-centralization]
  affects:
    - crates/ironhermes-agent/src/context_refs.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
tech_stack:
  added:
    - dirs workspace dep added to ironhermes-agent Cargo.toml (was only in ironhermes-core)
  patterns:
    - OnceLock<Regex> for compile-once regex patterns (no once_cell dep)
    - lookbehind emulation via optional pre capture group
    - argv-only subprocess discipline (Command::new + .arg() per argument, no shell)
    - injected UrlFetcher (boxed async fn) for hermetic test isolation
    - D-11 carrier pattern: warnings bubbled on AgentResult, not per-surface
key_files:
  created: []
  modified:
    - crates/ironhermes-agent/src/context_refs.rs
    - crates/ironhermes-agent/src/agent_runtime.rs
    - crates/ironhermes-agent/src/agent_loop.rs
    - crates/ironhermes-agent/tests/invariants_34b.rs
    - crates/ironhermes-agent/Cargo.toml
decisions:
  - "Used std::sync::OnceLock instead of once_cell::sync::Lazy — stdlib-only, no new dep"
  - "Regex crate backreference (?P=q) not supported — split into three separate quoted-form patterns (backtick/double/single)"
  - "Lookbehind (?<![\w/]) emulated via optional pre capture group + skip in parser loop"
  - "rg --files in try_rg_listing uses std::process::Command (sync) not block_in_place — avoids multi-threaded runtime requirement in unit tests"
  - "UrlFetcher boxed async fn type alias — production wires registry.execute_tool('web_extract'), tests inject hermetic fakes"
  - "cwd stored on AgentRuntime struct (cloned before passing to bundle builder) for D-05 fixed-root resolution"
  - "context_warnings defaulted to Vec::new() at all 4 Ok(AgentResult{..}) sites and budget_exhausted"
  - "handler.rs lives in ironhermes-gateway (not iron_hermes_ui) — invariants_34b paths corrected"
metrics:
  duration: "~18 min"
  completed: "2026-05-22T11:15:21Z"
  tasks_completed: 3
  files_created: 0
  files_modified: 5
---

# Phase 34b Plan 01: @-reference Context Expansion — Full Implementation Summary

Port of `hermes-agent/agent/context_references.py` to `context_refs.rs` with central wiring in `AgentRuntime::run_turn`. Provides `@file:/@folder:/@diff/@staged/@git:N/@url:` expansion with sensitive-path blocklist, 50%/25% token budget, argv-only subprocess discipline (CWE-78 mitigated), and a `context_warnings` carrier on `AgentResult` for all 3 surfaces.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Parser + types + sensitive-path blocklist | 964b4c57 | context_refs.rs, Cargo.toml, Cargo.lock |
| 2 | Expander + budget + preprocess_context_references_async | a2732d23 | context_refs.rs |
| 3 | Centralize in run_turn + AgentResult.context_warnings carrier | 526b44ae | agent_runtime.rs, agent_loop.rs, invariants_34b.rs |

## Verification Results

```
cargo build --workspace
  → Finished dev profile (0 errors, pre-existing warnings only)

cargo test -p ironhermes-agent --lib context_refs::tests
  → ok. 15 passed; 0 failed; 0 ignored
  (6 parser + 1 blocklist + 5 expander + hard-limit + soft-limit + url-error + git:N validation)

cargo test -p ironhermes-agent --test invariants_34b
  → ok. 2 passed; 0 failed; 0 ignored
  (preprocess_before_attach_context_engine_in_run_turn, preprocess_not_called_in_surfaces)

Centralization gate — must sum to 0:
  grep -c preprocess_context_references_async main.rs handler.rs state.rs
  → 0 0 0 ✓

runtime has call — must be >= 1:
  grep -c preprocess_context_references_async agent_runtime.rs → 3 ✓

NO-SHELL gate — CWE-78:
  grep -nE 'sh -c|/bin/sh|...' context_refs.rs → CLEAN (0 matches) ✓

.arg() argv-only present:
  grep -c '\.arg(' context_refs.rs → 3 ✓

cargo test -p ironhermes-agent --test invariants_33 → ok. 8 passed ✓
```

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] Regex crate backreference not supported**
- **Found during:** Task 1 (RED compile)
- **Issue:** Python's quoted regex uses `(?P=q)` backreference; Rust `regex` crate rejects it at runtime
- **Fix:** Split into three separate patterns — `quoted_backtick_re()`, `quoted_double_re()`, `quoted_single_re()` — each matching one quote style explicitly
- **Files modified:** context_refs.rs
- **Commit:** 964b4c57

**2. [Rule 1 - Bug] Lookbehind `(?<![\w/])` not supported**
- **Found during:** Task 1 (RED compile)
- **Issue:** Rust `regex` crate has no lookbehind support; the Python pattern uses `(?<![\w/])@`
- **Fix:** Prepend optional `(?P<pre>[\w/])` capture group; skip any match where `pre` is captured in parser loop — semantically equivalent
- **Files modified:** context_refs.rs
- **Commit:** 964b4c57

**3. [Rule 1 - Bug] `block_in_place` requires multi-threaded Tokio runtime**
- **Found during:** Task 2 test run (test_expand_folder panicked)
- **Issue:** `tokio::task::block_in_place` panics in single-threaded test runtime
- **Fix:** Use `std::process::Command` (sync, no Tokio runtime requirement) for the `rg --files` fallback in `try_rg_listing`
- **Files modified:** context_refs.rs
- **Commit:** a2732d23

**4. [Rule 1 - Bug] handler.rs path in invariants_34b was wrong**
- **Found during:** Task 3 (test compile error)
- **Issue:** Plan specified `iron_hermes_ui/src/server/handler.rs`; actual file is `ironhermes-gateway/src/handler.rs`
- **Fix:** Updated `include_str!` path to `../../ironhermes-gateway/src/handler.rs`
- **Files modified:** invariants_34b.rs
- **Commit:** 526b44ae

**5. [Rule 2 - Missing field] AgentRuntime test constructor missing cwd field**
- **Found during:** Task 3 (lib test compile error)
- **Issue:** Test-only `Self { }` constructor in agent_runtime.rs didn't include new `cwd` field
- **Fix:** Added `cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))` to test constructor
- **Files modified:** agent_runtime.rs
- **Commit:** 526b44ae

## Security Gate Verification (Threat Register)

| Threat | Disposition | Verified |
|--------|-------------|---------|
| T-34b-01-PATH (path traversal) | mitigated | resolve_within_root enforces allowed_root (fixed to cwd per D-03/D-04) |
| T-34b-01-SC (sensitive-path blocklist) | mitigated | is_sensitive_path covers all 24 entries; parameterised test covers every entry |
| T-34b-01-SHELL (CWE-78 command injection) | mitigated | NO-SHELL grep gate: 0 matches; .arg() count: 3; @git:N validated u32 [1,10] before command |
| T-34b-01-SSRF (@url: fetch) | mitigated | Routes through registry.execute_tool("web_extract"); D-02 fallback warning on failure |
| T-34b-01-DOS (token budget DoS) | mitigated | hard_limit=50%, soft_limit=25%; hard → blocked + message reverts to original |

## Known Stubs

None — all must-haves from the plan are fully implemented and verified.

## Threat Flags

None — no new network endpoints or trust boundaries introduced beyond what the plan's threat model covers.

## Self-Check: PASSED

- [x] crates/ironhermes-agent/src/context_refs.rs exists (1237 lines, >= 300 min)
- [x] crates/ironhermes-agent/src/agent_runtime.rs contains preprocess_context_references_async
- [x] crates/ironhermes-agent/src/agent_loop.rs contains context_warnings field
- [x] crates/ironhermes-agent/tests/invariants_34b.rs is not #[ignore] (2 real tests)
- [x] Commit 964b4c57 exists (Task 1)
- [x] Commit a2732d23 exists (Task 2)
- [x] Commit 526b44ae exists (Task 3)
- [x] 15/15 context_refs::tests pass
- [x] 2/2 invariants_34b tests pass
- [x] workspace build clean
