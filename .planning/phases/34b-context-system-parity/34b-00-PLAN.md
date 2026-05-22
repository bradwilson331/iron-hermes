---
phase: 34b-context-system-parity
plan: 00
type: execute
wave: 0
depends_on: []
files_modified:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
autonomous: true
requirements: [CTX-REF-W0, CTX-ENG-W0]
must_haves:
  truths:
    - "cargo test -p ironhermes-agent --lib context_refs::tests compiles and runs (0 tests, no error)"
    - "cargo test -p ironhermes-agent --test invariants_34b compiles and runs"
    - "Two #[ignore] placeholder tests exist so waves 1/2 have failing-test targets to flip"
  artifacts:
    - path: crates/ironhermes-agent/src/context_refs.rs
      provides: "Empty context_refs module with #[cfg(test)] mod tests so wave 1 can grow it in place"
      contains: "mod tests"
    - path: crates/ironhermes-agent/tests/invariants_34b.rs
      provides: "Integration-test scaffold for phase 34b grep/source invariants"
    - path: crates/ironhermes-agent/src/lib.rs
      provides: "pub mod context_refs declaration"
      contains: "pub mod context_refs"
  key_links:
    - from: crates/ironhermes-agent/src/lib.rs
      to: crates/ironhermes-agent/src/context_refs.rs
      via: "pub mod context_refs"
      pattern: "pub mod context_refs"
---

<objective>
Wave 0 test scaffolding for Phase 34b. Create the empty module + test
placeholders that Waves 1 and 2 grow into real implementations, so every later
task has an `<automated>` target that exists from the start (Nyquist
compliance, per 34B-VALIDATION.md). NOTE: a prior planning pass marked these
scaffolds "✅ created" but they are NOT on disk — this plan actually creates them.

This plan creates NO production behavior. It establishes:
1. `crates/ironhermes-agent/src/context_refs.rs` as a compiling empty module
   with a `#[cfg(test)] mod tests {}` block (Wave 1 fills it).
2. `crates/ironhermes-agent/tests/invariants_34b.rs` as a compiling
   integration-test file with an `#[ignore]`d placeholder.
3. Two `#[ignore]`d unit-test placeholders — `test_context_compressor_reset_zeroes_counter`
   in `context_compressor.rs` and `test_memory_authority_header` in
   `summarizing_engine.rs` — that Wave 2 un-ignores and makes assert real behavior.

Purpose: give Waves 1/2 a red→green target the moment they start.
Output: 1 new module, 1 new integration-test file, 2 placeholder tests, lib wiring.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/ROADMAP.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-VALIDATION.md

<interfaces>
From crates/ironhermes-agent/src/lib.rs (existing module cluster — insert after context_loader):
```rust
pub mod context_compressor;
pub mod context_engine;
pub mod context_loader;
```
The new `pub mod context_refs;` belongs in this cluster.

Existing test idiom in this crate — `#[ignore]`d placeholder tests compile and
are skipped until a later wave removes the attribute and adds assertions.
`invariants_33.rs` is the integration-test analog using `include_str!` source guards.
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Create context_refs.rs empty module + lib wiring</name>
  <files>crates/ironhermes-agent/src/context_refs.rs, crates/ironhermes-agent/src/lib.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/lib.rs (place `pub mod context_refs;` in the existing context_* cluster, after `pub mod context_loader;`)
    - crates/ironhermes-agent/src/context_loader.rs (small existing module, for the module-doc + test-mod idiom)
  </read_first>
  <action>
    Create `crates/ironhermes-agent/src/context_refs.rs` containing only: a
    module-level doc comment stating this is the `@`-reference expansion module
    (port of `../hermes-agent/agent/context_references.py`, Phase 34b Plan 01),
    and an empty `#[cfg(test)] mod tests { }` block. Do NOT add any types or
    functions — Wave 1 (Plan 01) adds `ContextReference`,
    `ContextReferenceResult`, `parse_context_references`,
    `preprocess_context_references_async`. Add `pub mod context_refs;` to
    `crates/ironhermes-agent/src/lib.rs` after `pub mod context_loader;`.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast 2>&1 | tail -5</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build -p ironhermes-agent` succeeds.
    - `grep -c 'pub mod context_refs' crates/ironhermes-agent/src/lib.rs` returns 1.
    - The test command runs and reports 0 tests (module + empty test mod compile).
    - `context_refs.rs` contains no `pub fn`/`pub struct` yet (it is a stub).
  </acceptance_criteria>
  <done>context_refs module compiles, is exported from lib.rs; empty test mod present.</done>
</task>

<task type="auto">
  <name>Task 2: Create invariants_34b.rs integration-test scaffold</name>
  <files>crates/ironhermes-agent/tests/invariants_34b.rs</files>
  <read_first>
    - crates/ironhermes-agent/tests/invariants_33.rs (existing integration-test file — copy its file/module shape and include_str! source-guard idiom)
  </read_first>
  <action>
    Create `crates/ironhermes-agent/tests/invariants_34b.rs` with a file-level
    doc comment naming it the Phase 34b cross-surface wiring invariant suite,
    and a single `#[test] #[ignore]` placeholder `placeholder_34b_wiring` whose
    body is the comment `// Wave 1/2 replace this with the 3-surface preprocess
    wiring grep-gate and the run_turn-centralization source guard.`. Match the
    `include_str!`-based source-guard convention used by `invariants_33.rs` so
    Wave 1/2 can assert against handler.rs / state.rs / main.rs source text.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast 2>&1 | tail -5</automated>
  </verify>
  <acceptance_criteria>
    - File compiles as an integration test target.
    - The test command runs and reports 1 ignored test, 0 failures.
  </acceptance_criteria>
  <done>invariants_34b integration target compiles and runs with one ignored placeholder.</done>
</task>

<task type="auto">
  <name>Task 3: Add #[ignore] placeholder tests for reset + memory-authority header</name>
  <files>crates/ironhermes-agent/src/context_compressor.rs, crates/ironhermes-agent/src/summarizing_engine.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_compressor.rs (existing `#[cfg(test)] mod tests`; note `ContextCompressor::new(context_length, threshold_percent)` at ~:48, `compression_count` field at ~:44)
    - crates/ironhermes-agent/src/summarizing_engine.rs (existing `#[cfg(test)] mod tests` at ~:700; note `HISTORY_SENTINEL`/`make_history_message` at ~:54)
  </read_first>
  <action>
    In `context_compressor.rs` `mod tests`, add `#[test] #[ignore]` named
    `test_context_compressor_reset_zeroes_counter` with body comment `// Wave 2
    (Plan 02 Task 1) un-ignores: build a ContextCompressor, drive
    compression_count up, call on_session_reset(), assert all token counters +
    compression_count are zero.`. In `summarizing_engine.rs` `mod tests`, add
    `#[test] #[ignore]` named `test_memory_authority_header` with body comment
    `// Wave 2 (Plan 02 Task 2) un-ignores: assert the compaction
    history-segment header contains "MEMORY.md" and "ALWAYS authoritative".`.
    Change NO production code in this task.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast 2>&1 | tail -8</automated>
  </verify>
  <acceptance_criteria>
    - Both placeholder tests compile and report as ignored (0 failures).
    - No production code changed (only `#[cfg(test)]` blocks touched).
    - `grep -c 'fn test_context_compressor_reset_zeroes_counter' crates/ironhermes-agent/src/context_compressor.rs` returns 1.
    - `grep -c 'fn test_memory_authority_header' crates/ironhermes-agent/src/summarizing_engine.rs` returns 1.
  </acceptance_criteria>
  <done>Two ignored placeholder tests exist as red→green targets for Wave 2.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| (none in scope) | This plan creates only test scaffolds and an empty module — no untrusted input processed, no path/URL/network touched. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-00-SC | Tampering | cargo (no new package installs) | accept | This plan adds no dependencies and runs no package-manager installs. The @-ref security surface lands in Plan 01's threat model. |
</threat_model>

<verification>
```bash
cargo build -p ironhermes-agent
cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast 2>&1 | tail -5
cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast 2>&1 | tail -5
cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast 2>&1 | tail -8
```
All commands succeed; the placeholder tests report as ignored.
</verification>

<success_criteria>
- context_refs.rs exists, compiles, exported from lib.rs, contains an empty test mod.
- invariants_34b.rs exists as a compiling integration target with one ignored test.
- Two #[ignore] unit-test placeholders exist (compressor reset, memory-authority header).
- No production behavior added.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34B-00-SUMMARY.md` when done.
</output>
