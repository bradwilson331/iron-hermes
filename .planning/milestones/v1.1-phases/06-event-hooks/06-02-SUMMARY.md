---
phase: 06-event-hooks
plan: 02
subsystem: hooks, tools
tags: [guardrails, security, tool-interception, hook-02]
dependency_graph:
  requires: [06-01 (ironhermes-hooks crate, HooksConfig)]
  provides: [GuardrailHook trait, GuardrailDecision enum, BlocklistGuardrail, ToolRegistry guardrail intercept]
  affects: [ironhermes-tools (ToolRegistry), future plans using ToolRegistry dispatch]
tech_stack:
  added: [ironhermes-hooks dep in ironhermes-tools]
  patterns: [trait-object guardrail chain, three-outcome decision enum, error-detail level abstraction]
key_files:
  created:
    - crates/ironhermes-hooks/src/guardrail.rs
  modified:
    - crates/ironhermes-hooks/src/lib.rs
    - crates/ironhermes-tools/Cargo.toml
    - crates/ironhermes-tools/src/registry.rs
decisions:
  - "GuardrailHook::check() is synchronous — no I/O in guardrail logic, keeping dispatch hot path fast"
  - "Guardrails stored as Vec<Box<dyn GuardrailHook>> — checked in registration order, blocklist first per D-05"
  - "Empty guardrail vec is the default — zero behavioral change for all existing ToolRegistry users"
  - "T-06-04: same &args reference used for guardrail check and tool.execute(), eliminating copy-after-check gap"
metrics:
  duration: "~20 minutes"
  completed: "2026-04-08"
  tasks_completed: 2
  files_created: 1
  files_modified: 3
---

# Phase 6 Plan 2: Guardrail Interception System Summary

GuardrailHook trait with three-outcome GuardrailDecision (Allow/Warn/Block), BlocklistGuardrail for config-driven tool blocking by name, and ToolRegistry::dispatch() intercept that checks all registered guardrails before tool.execute().

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 1 | Define GuardrailHook trait, GuardrailDecision enum, BlocklistGuardrail | 96db428 | guardrail.rs (new), lib.rs |
| 2 | Wire guardrail checks into ToolRegistry::dispatch() | 7987f3d | ironhermes-tools/Cargo.toml, registry.rs |

## What Was Built

### Task 1: GuardrailHook trait and BlocklistGuardrail

**guardrail.rs** — New file in `crates/ironhermes-hooks/src/`:

- `GuardrailDecision` enum: `Allow`, `Warn { reason }`, `Block { reason }` — three-outcome design per D-07.
- `GuardrailHook` trait: `check(&self, tool_name: &str, args: &serde_json::Value) -> GuardrailDecision` and `name() -> &str`. Synchronous — no I/O in guardrail logic.
- `BlocklistGuardrail` struct: checks `blocked_tools: Vec<String>` by exact name match. `from_config()` constructor reads from `HooksConfig.blocked_tools`.
- `format_guardrail_error()`: formats block error message respecting `ErrorDetailLevel` (T-06-05 mitigation). `Full` includes tool name, guardrail name, and reason; `Minimal` returns generic "Tool call blocked by security policy".

7 unit tests: block named tool, allow unlisted tool, empty blocklist allows all, format error Full, format error Minimal, Warn variant identity, from_config constructor.

**lib.rs** — Added `pub mod guardrail` and re-exported all four public items.

### Task 2: ToolRegistry guardrail wiring

**Cargo.toml** — Added `ironhermes-hooks = { path = "../ironhermes-hooks" }`.

**registry.rs** — Modified `ToolRegistry`:
- Added `guardrails: Vec<Box<dyn ironhermes_hooks::GuardrailHook>>` field (empty by default).
- Added `error_detail: ironhermes_hooks::ErrorDetailLevel` field (defaults to `Full`).
- `add_guardrail(hook)` — appends to the guardrail chain. Caller registers `BlocklistGuardrail` first per D-05.
- `set_error_detail(level)` — configures block error verbosity.
- `dispatch()` — guardrail loop before `tool.execute(args)`: `Allow` continues, `Warn` calls `tracing::warn!` and continues, `Block` calls `format_guardrail_error()` and returns `Err`.

5 integration tests: no-guardrail passes, block returns Err containing "blocked", allow lets through, warn proceeds, minimal detail hides tool name.

## Verification Results

- `cargo test -p ironhermes-hooks`: 21/21 passed (14 existing + 7 new guardrail tests)
- `cargo test -p ironhermes-tools`: 59/59 passed (54 existing + 5 new guardrail integration tests)
- `cargo test --workspace`: all tests green, no regressions

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] MockTool schema() used wrong ToolSchema constructor**
- **Found during:** Task 2 compilation
- **Issue:** Test's `MockTool::schema()` used struct literal `ToolSchema { name, description, parameters }` but the actual `ToolSchema` struct has `schema_type` and `function: FunctionSchema` fields
- **Fix:** Replaced with `ToolSchema::new(name, description, parameters)` which is the correct constructor
- **Files modified:** crates/ironhermes-tools/src/registry.rs (tests section only)
- **Commit:** 7987f3d (included in same commit)

## Known Stubs

None. All guardrail logic is fully wired. `BlocklistGuardrail::from_config()` connects directly to `HooksConfig.blocked_tools`. The `ToolRegistry` caller is responsible for constructing and registering guardrails — no auto-wiring from config yet (that is a caller-side concern, expected to be done in the agent or gateway setup code).

## Threat Surface Scan

No new network endpoints or auth paths introduced. The guardrail system is purely in-process synchronous logic.

Threat mitigations implemented as designed:
- **T-06-04** (Tampering — guardrail bypass via arg mutation): Same `&args` reference is checked by guardrails and then passed to `tool.execute()` in the same `dispatch()` call with no intermediate mutation point.
- **T-06-05** (Information Disclosure — error detail leaks tool name): `ErrorDetailLevel::Minimal` verified by test `test_guardrail_error_detail_minimal` to return exactly "Tool call blocked by security policy" without tool name.

## Self-Check: PASSED

- crates/ironhermes-hooks/src/guardrail.rs: FOUND
- crates/ironhermes-hooks/src/lib.rs contains `pub mod guardrail`: FOUND
- crates/ironhermes-tools/Cargo.toml contains `ironhermes-hooks`: FOUND
- crates/ironhermes-tools/src/registry.rs contains `guardrails`: FOUND
- commit 96db428: FOUND
- commit 7987f3d: FOUND
