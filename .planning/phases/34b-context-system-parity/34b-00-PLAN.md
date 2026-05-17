---
phase: 34b-context-system-parity
plan: 00
type: execute
wave: 0
depends_on: []
files_modified:
  - crates/ironhermes-agent/src/context_refs.rs
  - crates/ironhermes-agent/src/lib.rs
  - crates/ironhermes-agent/src/context_compressor.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-agent/tests/invariants_34b.rs
autonomous: true
requirements:
  - CTX-REF-W0
  - CTX-ENG-W0
tags:
  - wave-0
  - test-scaffolding
  - nyquist

must_haves:
  truths:
    - "Every Wave 1/2 task in 34b-01 and 34b-02 has a runnable cargo-test target before any implementation begins."
    - "`cargo test -p ironhermes-agent --lib context_refs::tests` compiles (and exits 0 with an empty pass list before implementation lands)."
    - "`cargo test -p ironhermes-agent --test invariants_34b` compiles (and exits 0 with an empty pass list before implementation lands)."
    - "`cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter` compiles to a `#[ignore]`/`todo!()` placeholder, surfacing as a known-pending test."
    - "`cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header` compiles to a `#[ignore]`/`todo!()` placeholder."
  artifacts:
    - path: "crates/ironhermes-agent/src/context_refs.rs"
      provides: "Empty module stub with `#[cfg(test)] mod tests {}` so 34b-01 Task 1 has a target file."
      min_lines: 5
    - path: "crates/ironhermes-agent/tests/invariants_34b.rs"
      provides: "Empty integration test file; tests added by 34b-01 Task 3 and 34b-02 Task 3."
      min_lines: 3
    - path: "crates/ironhermes-agent/src/context_compressor.rs"
      provides: "Existing file — adds `#[cfg(test)] mod tests` (if absent) with a `#[ignore]` placeholder for `test_context_compressor_reset_zeroes_counter`."
      contains: "test_context_compressor_reset_zeroes_counter"
    - path: "crates/ironhermes-agent/src/summarizing_engine.rs"
      provides: "Existing file — adds `#[cfg(test)] mod tests` (if absent) with a `#[ignore]` placeholder for `test_memory_authority_header`."
      contains: "test_memory_authority_header"
    - path: "crates/ironhermes-agent/src/lib.rs"
      provides: "Registers `pub mod context_refs;` so the stub compiles into the crate."
      contains: "pub mod context_refs"
  key_links:
    - from: "crates/ironhermes-agent/src/lib.rs"
      to: "crates/ironhermes-agent/src/context_refs.rs"
      via: "pub mod context_refs;"
      pattern: "pub mod context_refs"
    - from: "Plan 34b-01 Task 1"
      to: "crates/ironhermes-agent/src/context_refs.rs"
      via: "extends the stub with parser implementation"
      pattern: "parse_context_references"
    - from: "Plan 34b-02 Task 1"
      to: "crates/ironhermes-agent/src/context_compressor.rs"
      via: "replaces the `#[ignore] todo!()` placeholder with the real reset test"
      pattern: "test_context_compressor_reset_zeroes_counter"
---

<objective>
Create the four Wave 0 test scaffolding artifacts required by VALIDATION.md so Plans 34b-01 and 34b-02 are Nyquist-compliant before they execute. This plan establishes the test-file skeletons; the implementation plans then attach real assertions.

Purpose: satisfy `nyquist_compliant: true` and `wave_0_complete: true` per GSD planner contract. Every Wave 1 task in this phase already has a `<verify><automated>` command, but the target test files do not yet exist — running those commands today returns "no such test". This plan fixes the gap.

Output: 2 new files + 2 edits to existing files. Total expected size after this plan: ~30 added lines across 5 files. No production logic is added — only `#[cfg(test)] mod tests {}` skeletons and one new integration test file.

Security: no new external code paths, no new packages, no new public APIs. The `context_refs.rs` stub exposes no items. Threat model deferred to 34b-01.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/STATE.md
@.planning/phases/34b-context-system-parity/34B-CONTEXT.md
@.planning/phases/34b-context-system-parity/34B-VALIDATION.md
@.planning/phases/34b-context-system-parity/34B-PATTERNS.md

# Files this plan touches
@crates/ironhermes-agent/src/lib.rs
@crates/ironhermes-agent/src/context_compressor.rs
@crates/ironhermes-agent/src/summarizing_engine.rs

<interfaces>
<!-- No new public interfaces. All additions are test-only. -->
<!-- Plan 34b-01 Task 1 fills in `context_refs.rs` with the parser. -->
<!-- Plan 34b-02 Task 1 fills in `context_compressor::reset` and the test body. -->
<!-- Plan 34b-02 Task 2 fills in `MEMORY_AUTHORITY_REMINDER` and the test body. -->
</interfaces>

</context>

<tasks>

<task type="auto">
  <name>Task 1: Create context_refs.rs stub + register in lib.rs</name>
  <files>crates/ironhermes-agent/src/context_refs.rs, crates/ironhermes-agent/src/lib.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/lib.rs (module list — find the alphabetical insertion point between `context_loader` and `engine_factory`)
    - crates/ironhermes-agent/src/nudge.rs (module-doc-header template — Plan 34b-01 Task 1 will replace the stub header with the full one)
  </read_first>
  <action>
    Create `crates/ironhermes-agent/src/context_refs.rs` containing ONLY:
    - A one-line module doc comment: `//! Phase 34b Plan 01 stub — implementation lands in Plan 34b-01 Task 1.`
    - An empty `#[cfg(test)] mod tests {}` block.

    Total file size: ~5 lines.

    Edit `crates/ironhermes-agent/src/lib.rs`. Add `pub mod context_refs;` in alphabetical order between the existing `pub mod context_loader;` and `pub mod engine_factory;` lines. Do not add a `pub use` re-export — that decision is owned by Plan 34b-01.

    Do NOT add the parser, types, regex, or any other implementation. Plan 34b-01 Task 1 will replace the body of this file. The purpose of this task is solely to make `cargo test -p ironhermes-agent --lib context_refs::tests` compile and exit 0 (with zero tests) BEFORE Plan 34b-01 begins.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent 2>&1 | tee /tmp/34b-00-task1.log; grep -E "error\[|^error:" /tmp/34b-00-task1.log && echo "BUILD ERROR" || echo "BUILD OK"; cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast 2>&1 | tee -a /tmp/34b-00-task1.log; grep -q "^pub mod context_refs;" crates/ironhermes-agent/src/lib.rs && echo "lib.rs registration OK" || echo "lib.rs registration MISSING"; [ -f crates/ironhermes-agent/src/context_refs.rs ] && echo "stub file exists OK" || echo "stub file MISSING"</automated>
  </verify>
  <acceptance_criteria>
    - `crates/ironhermes-agent/src/context_refs.rs` exists
    - `grep -c "^pub mod context_refs;" crates/ironhermes-agent/src/lib.rs` returns 1
    - `cargo build -p ironhermes-agent` exits 0
    - `cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast` exits 0 (with "running 0 tests" or equivalent)
    - File line count is ≤ 10 (it is a stub, not implementation)
  </acceptance_criteria>
  <done>The `context_refs` module stub exists, is registered in `lib.rs`, and the test target `context_refs::tests` is now runnable. Plan 34b-01 Task 1 can begin and will replace the entire file body.</done>
</task>

<task type="auto">
  <name>Task 2: Create invariants_34b.rs integration-test stub</name>
  <files>crates/ironhermes-agent/tests/invariants_34b.rs</files>
  <read_first>
    - crates/ironhermes-agent/tests/ (list existing integration test files for naming convention; expect `invariants_33.rs` to be present)
  </read_first>
  <action>
    Create `crates/ironhermes-agent/tests/invariants_34b.rs` containing ONLY:
    - A one-line module doc comment: `//! Phase 34b integration tests — populated by Plan 34b-01 Task 3 and Plan 34b-02 Task 3.`
    - A single ignored placeholder test:
      `#[test] #[ignore = "Phase 34b: stub — real tests land in 34b-01/34b-02 Task 3"] fn placeholder() {}`

    Total file size: ~4 lines. Do NOT add any real test logic. The purpose is solely to make `cargo test -p ironhermes-agent --test invariants_34b` discover and exit 0.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent --tests 2>&1 | tee /tmp/34b-00-task2.log; grep -E "error\[|^error:" /tmp/34b-00-task2.log && echo "BUILD ERROR" || echo "BUILD OK"; cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast 2>&1 | tee -a /tmp/34b-00-task2.log; [ -f crates/ironhermes-agent/tests/invariants_34b.rs ] && echo "integration test file exists OK" || echo "integration test file MISSING"; grep -q "#\[ignore" crates/ironhermes-agent/tests/invariants_34b.rs && echo "placeholder is #[ignore] OK"</automated>
  </verify>
  <acceptance_criteria>
    - `crates/ironhermes-agent/tests/invariants_34b.rs` exists
    - `cargo build -p ironhermes-agent --tests` exits 0
    - `cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast` exits 0 with one ignored test (output contains `1 ignored` or `0 passed; 1 ignored`)
    - File contains `#[ignore` (the placeholder is ignored, not failing)
  </acceptance_criteria>
  <done>The integration-test stub exists and is discoverable. Plans 34b-01 Task 3 and 34b-02 Task 3 will append real tests; the placeholder can be removed at that time.</done>
</task>

<task type="auto">
  <name>Task 3: Add #[ignore] test placeholders to context_compressor.rs and summarizing_engine.rs</name>
  <files>crates/ironhermes-agent/src/context_compressor.rs, crates/ironhermes-agent/src/summarizing_engine.rs</files>
  <read_first>
    - crates/ironhermes-agent/src/context_compressor.rs (look for an existing `#[cfg(test)] mod tests` — append to it if present, create if absent; verify it does NOT already define `test_context_compressor_reset_zeroes_counter`)
    - crates/ironhermes-agent/src/summarizing_engine.rs (look for existing `#[cfg(test)] mod tests`; verify it does NOT already define `test_memory_authority_header`)
  </read_first>
  <action>
    In `crates/ironhermes-agent/src/context_compressor.rs`:
    - If `#[cfg(test)] mod tests { ... }` exists (confirmed at ~lines 241-276 per PATTERNS §context_compressor), append the placeholder test inside it. If it does NOT exist, add the module at the end of the file.
    - The placeholder test signature:
      `#[test] #[ignore = "Phase 34b-02 Task 1 fills this in"] fn test_context_compressor_reset_zeroes_counter() { todo!("Plan 34b-02 Task 1: assert reset() zeroes compression_count and last_*_tokens"); }`

    In `crates/ironhermes-agent/src/summarizing_engine.rs`:
    - If a `#[cfg(test)] mod tests { ... }` exists, append to it. If not, add the module at the end of the file. PATTERNS §summarizing_engine notes the mod may NOT exist today — create it if absent.
    - The placeholder test signature:
      `#[test] #[ignore = "Phase 34b-02 Task 2 fills this in"] fn test_memory_authority_header() { todo!("Plan 34b-02 Task 2: assert make_history_message output contains MEMORY.md and ALWAYS authoritative"); }`

    Do NOT implement the assertions. Do NOT touch any non-test code. The `todo!()` macro inside an `#[ignore]` test never executes during a normal `cargo test` run because the test is filtered out, so the `todo!` is just documentation for the future implementor.
  </action>
  <verify>
    <automated>cargo build -p ironhermes-agent 2>&1 | tee /tmp/34b-00-task3.log; grep -E "error\[|^error:" /tmp/34b-00-task3.log && echo "BUILD ERROR" || echo "BUILD OK"; cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast 2>&1 | tee -a /tmp/34b-00-task3.log; grep -q "test_context_compressor_reset_zeroes_counter" crates/ironhermes-agent/src/context_compressor.rs && echo "compressor placeholder OK"; grep -q "test_memory_authority_header" crates/ironhermes-agent/src/summarizing_engine.rs && echo "summarizing placeholder OK"; cargo test -p ironhermes-agent --lib 2>&1 | tail -10</automated>
  </verify>
  <acceptance_criteria>
    - `grep -c "fn test_context_compressor_reset_zeroes_counter" crates/ironhermes-agent/src/context_compressor.rs` returns 1
    - `grep -c "fn test_memory_authority_header" crates/ironhermes-agent/src/summarizing_engine.rs` returns 1
    - Both placeholders are annotated `#[ignore` (confirmed by grep within ~3 lines of each test fn declaration)
    - `cargo build -p ironhermes-agent` exits 0
    - `cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter summarizing_engine::tests::test_memory_authority_header --no-fail-fast` exits 0 (both tests reported as ignored, not failed)
    - Regression: `cargo test -p ironhermes-agent --lib` still exits 0 (existing tests unaffected)
  </acceptance_criteria>
  <done>Both implementation-target test names are discoverable by name. Plan 34b-02 Task 1 will remove the `#[ignore]` from `test_context_compressor_reset_zeroes_counter` and fill in the body. Plan 34b-02 Task 2 will do the same for `test_memory_authority_header`. Wave 0 is complete.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| (none) | This plan introduces no new code paths, no new dependencies, no new public APIs. Pure test scaffolding. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34b-00-SC | Tampering | npm/pip/cargo installs | accept | No new packages introduced. |
| T-34b-00-COMPAT | Tampering | existing test module shapes | mitigate | Task 3 appends to existing `#[cfg(test)] mod tests` blocks if present; the `#[ignore]` annotation guarantees no behavior change at `cargo test` time. |
</threat_model>

<verification>
After all three tasks complete, the following four commands MUST all exit 0:

```bash
cargo test -p ironhermes-agent --lib context_refs::tests --no-fail-fast
cargo test -p ironhermes-agent --test invariants_34b --no-fail-fast
cargo test -p ironhermes-agent --lib context_compressor::tests::test_context_compressor_reset_zeroes_counter --no-fail-fast
cargo test -p ironhermes-agent --lib summarizing_engine::tests::test_memory_authority_header --no-fail-fast
```

Each command should report tests as either "0 passed; 0 ignored" (Task 1, Task 2 placeholder is ignored) or "0 passed; 1 ignored" — never a failure. With Wave 0 in place, VALIDATION.md can be updated to `wave_0_complete: true` and `nyquist_compliant: true`.

Regression: `cargo test -p ironhermes-agent --lib` continues to exit 0 (no existing test is renamed, removed, or modified).
</verification>

<success_criteria>
1. `crates/ironhermes-agent/src/context_refs.rs` exists as a ~5-line stub registered in `lib.rs`.
2. `crates/ironhermes-agent/tests/invariants_34b.rs` exists as a ~4-line stub with one `#[ignore]` placeholder test.
3. `crates/ironhermes-agent/src/context_compressor.rs` contains a `#[cfg(test)] mod tests` block with an `#[ignore]` `test_context_compressor_reset_zeroes_counter` placeholder.
4. `crates/ironhermes-agent/src/summarizing_engine.rs` contains a `#[cfg(test)] mod tests` block with an `#[ignore]` `test_memory_authority_header` placeholder.
5. All four `<verify>` commands from `<verification>` exit 0.
6. VALIDATION.md is updated post-plan (out of scope for the executor — the planner has already updated it) to `wave_0_complete: true` and `nyquist_compliant: true`.
</success_criteria>

<output>
Create `.planning/phases/34b-context-system-parity/34b-00-SUMMARY.md` when done, including:
- Final file sizes for each created/modified file
- Confirmation that all four ignored-placeholder tests are discoverable by name
- Any deviation from the planned `#[ignore]` annotation strategy (none expected)
- Note for Plan 34b-01 Task 1: this task replaces the entire body of `context_refs.rs`
- Note for Plan 34b-02 Task 1: this task removes `#[ignore]` and implements `test_context_compressor_reset_zeroes_counter`
- Note for Plan 34b-02 Task 2: this task removes `#[ignore]` and implements `test_memory_authority_header`
</output>
