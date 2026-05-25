---
phase: 34a-read-side-memory-parity
plan: 01
type: execute
wave: 1
depends_on: []
files_modified:
  - crates/ironhermes-core/src/memory_provider.rs
  - crates/ironhermes-agent/src/memory/manager.rs
  - crates/ironhermes-agent/src/memory_context.rs
  - crates/ironhermes-agent/src/lib.rs
autonomous: true
requirements: [MEM-READ-01, MEM-READ-02]

must_haves:
  truths:
    - "A memory provider can be asked for query-scoped recall text via prefetch_with_query; the default impl returns an empty string so the file provider is unaffected"
    - "MemoryManager proxies prefetch_with_query to the primary provider only (mirror is write-only)"
    - "sanitize_context strips full <memory-context> blocks, orphaned [System note] lines, and bare fence tags — in that order — case-insensitively"
    - "build_memory_context_block returns None for empty/whitespace input and otherwise wraps sanitized text in a <memory-context> block with the byte-exact Python system note (em dash U+2014)"
    - "sanitize(build(x)) is idempotent against re-wrapping — feeding a provider's already-wrapped output through build does not double-nest"
  artifacts:
    - path: "crates/ironhermes-core/src/memory_provider.rs"
      provides: "prefetch_with_query defaulted trait method (no-op returns Ok(String::new()))"
      contains: "async fn prefetch_with_query"
    - path: "crates/ironhermes-agent/src/memory/manager.rs"
      provides: "MemoryManager::prefetch_with_query primary-only proxy"
      contains: "pub async fn prefetch_with_query"
    - path: "crates/ironhermes-agent/src/memory_context.rs"
      provides: "sanitize_context + build_memory_context_block + 8 unit tests"
      min_lines: 80
    - path: "crates/ironhermes-agent/src/lib.rs"
      provides: "pub mod memory_context declaration"
      contains: "pub mod memory_context"
  key_links:
    - from: "crates/ironhermes-agent/src/memory/manager.rs"
      to: "MemoryProvider::prefetch_with_query"
      via: "self.primary.lock().await then delegate"
      pattern: "prefetch_with_query"
    - from: "crates/ironhermes-agent/src/memory_context.rs::build_memory_context_block"
      to: "sanitize_context"
      via: "calls sanitize_context on raw before wrapping"
      pattern: "sanitize_context"
---

<objective>
Add the read-side recall query primitive (MEM-READ-01) and the context-block
transform module (MEM-READ-02). This is pure-logic + trait work: a defaulted
trait method, a primary-only proxy on MemoryManager, and a new
`memory_context.rs` module porting `sanitize_context` + `build_memory_context_block`
byte-for-byte from the Python reference. No agent-loop changes, no streaming
changes — those land in 34a-02.

Purpose: Establish the contracts that 34a-02 consumes (the agent loop calls
`MemoryManager::prefetch_with_query` and `build_memory_context_block`). Defining
them first means 34a-02's executor receives working interfaces, not stubs.

Output: prefetch_with_query trait method + MemoryManager proxy + memory_context.rs
(8 passing unit tests) + lib.rs module declaration.
</objective>

<execution_context>
@$HOME/.claude/get-shit-done/workflows/execute-plan.md
@$HOME/.claude/get-shit-done/templates/summary.md
</execution_context>

<context>
@.planning/PROJECT.md
@.planning/ROADMAP.md
@.planning/STATE.md
@.planning/phases/34a-read-side-memory-parity/34A-CONTEXT.md
@.planning/phases/34a-read-side-memory-parity/34A-RESEARCH.md
@.planning/phases/34a-read-side-memory-parity/34A-PATTERNS.md

<interfaces>
<!-- Contracts the executor needs. Extracted from codebase + RESEARCH/PATTERNS. No exploration needed. -->

Existing defaulted no-op trait method to mirror (memory_provider.rs ~line 149):
  async fn queue_prefetch(&self, _query: &str) -> anyhow::Result<()> { Ok(()) }
  - trait-level #[async_trait] at line 55 covers the new method; no per-method attribute
  - leading underscore on unused params; takes &self (immutable)
  - MemoryStore's impl block (lines 193-293) does NOT change — inherits the default

Existing read proxy to mirror (manager.rs lines 180-183):
  pub async fn prefetch(&self, session_id: &str) -> anyhow::Result<MemoryEntries> {
      let p = self.primary.lock().await;
      p.prefetch(session_id).await
  }
  - primary-only; mirror is write-only (D-26/D-28). No fan-out loop.

Python source of truth (read byte-for-byte before writing memory_context.rs):
  /Users/twilson/code/hermes-agent/agent/memory_manager.py
  - sanitize_context: 3-regex sequence (block, note, bare tag) — ORDER MATTERS
  - build_memory_context_block: empty->None; else wrap sanitized text
  - system note em dash is U+2014 (literal — in Python, \u{2014} in Rust)

regex crate: already a direct workspace dep of ironhermes-agent
  (crates/ironhermes-agent/Cargo.toml line 38: `regex = { workspace = true }`).
  Do NOT add it. Use regex::Regex + std::sync::OnceLock.
</interfaces>
</context>

<tasks>

<task type="auto">
  <name>Task 1: Add prefetch_with_query to MemoryProvider trait + MemoryManager proxy (MEM-READ-01)</name>
  <files>crates/ironhermes-core/src/memory_provider.rs, crates/ironhermes-agent/src/memory/manager.rs</files>
  <read_first>
    - crates/ironhermes-core/src/memory_provider.rs — read the queue_prefetch default no-op (~line 149-151), the trait-level #[async_trait] (line 55), the MemoryStore impl block (193-293), and the existing default-hook test `default_hook_methods_return_defaults` (~lines 328-426)
    - crates/ironhermes-agent/src/memory/manager.rs — read the prefetch() proxy (lines 180-183), queue_prefetch() proxy (lines 201-204), and the `read_paths_hit_primary_only` test (~lines 601-618)
  </read_first>
  <action>
    In memory_provider.rs, add a defaulted async trait method `prefetch_with_query(&self, _query: &str, _session_id: &str) -> anyhow::Result<String>` returning `Ok(String::new())`, placed immediately after `queue_prefetch`. Per MEM-READ-01 and the established defaulted-no-op pattern: &self (immutable), leading underscores on unused params, no per-method async_trait attribute. The MemoryStore impl block MUST NOT be touched — it inherits the no-op (verify by NOT editing lines 193-293). Extend the `default_hook_methods_return_defaults` test to assert `p.prefetch_with_query("q", "sid").await.unwrap() == ""` alongside the existing `queue_prefetch` assertion.
    In manager.rs, add `pub async fn prefetch_with_query(&self, query: &str, session_id: &str) -> anyhow::Result<String>` in the "Read paths" section near the prefetch() proxy, copying its shape exactly: lock primary, delegate, return. Primary-only — do NOT fan out to the mirror (D-26/D-28; mirror is write-only). Extend (or add a sibling to) `read_paths_hit_primary_only` to assert prefetch_with_query returns `Ok("")` on the file provider and the mock recorder records no extra read on the mirror.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-core --lib memory_provider 2>&1 | tail -5</automated>
    <automated>cargo test -p ironhermes-agent --lib memory::manager 2>&1 | tail -5</automated>
  </verify>
  <acceptance_criteria>
    - `cargo build -p ironhermes-core -p ironhermes-agent` is clean (no errors, no new warnings)
    - `grep -c "async fn prefetch_with_query" crates/ironhermes-core/src/memory_provider.rs` == 1
    - `grep -c "pub async fn prefetch_with_query" crates/ironhermes-agent/src/memory/manager.rs` == 1
    - `cargo test -p ironhermes-core --lib memory_provider` passes (includes the extended default-hook assertion)
    - `cargo test -p ironhermes-agent --lib memory::manager` passes (includes the primary-only assertion)
    - The diff to memory_provider.rs touches ONLY the trait body + its test — `git diff crates/ironhermes-core/src/memory_provider.rs` shows zero changes inside `impl MemoryProvider for MemoryStore` (lines ~193-293)
  </acceptance_criteria>
  <done>prefetch_with_query exists as a defaulted trait method (no-op) and a primary-only MemoryManager proxy; file provider inherits the no-op unchanged; both crates compile and their lib tests pass.</done>
</task>

<task type="auto" tdd="true">
  <name>Task 2: Create memory_context.rs (sanitize_context + build_memory_context_block) with 8 tests (MEM-READ-02)</name>
  <files>crates/ironhermes-agent/src/memory_context.rs, crates/ironhermes-agent/src/lib.rs</files>
  <read_first>
    - /Users/twilson/code/hermes-agent/agent/memory_manager.py — the canonical source. Read `sanitize_context` (the 3-regex set + their application order), `build_memory_context_block` (empty->None, system-note wording, the literal em dash), and confirm the exact system-note string for both variant phrasings ("informational background data" and "authoritative reference data ...")
    - crates/ironhermes-agent/src/nudge.rs — structural template: file-level doc comment, top-level fns, inline `#[cfg(test)] mod tests` with sync `#[test]` fns (lines 1-46 header shape, 154-238 test-block shape)
    - crates/ironhermes-agent/src/lib.rs — read the existing `pub mod nudge;` declaration to mirror placement
    - 34A-RESEARCH.md "Code Examples" section (regex set + build_memory_context_block) and "Common Pitfalls" 6 (regex order) and 34A-PATTERNS.md memory_context.rs section
  </read_first>
  <behavior>
    - Test 1 (empty_input_returns_none): build_memory_context_block("") and build_memory_context_block("   \n ") both return None
    - Test 2 (wraps_non_empty): build_memory_context_block("fact A") is Some(s) where s starts with "<memory-context>" and ends with "</memory-context>" and contains "fact A"
    - Test 3 (system_note_present_with_em_dash): the wrapped block contains the U+2014 em dash and the literal "[System note: The following is recalled memory context, NOT new user input."
    - Test 4 (double_wrap_idempotency): sanitize_context(build_memory_context_block("fact A").unwrap()) contains "fact A" but contains NO "<memory-context>" tag and NO "[System note:" line — i.e. feeding a wrapped block back through sanitize fully unwraps it
    - Test 5 (strip_full_block): sanitize_context("before <memory-context>x</memory-context> after") == "before  after" (block content removed entirely, not just tags)
    - Test 6 (strip_orphan_system_note): a string containing only the "[System note: ... authoritative reference data ...]" line is reduced to empty/whitespace by sanitize_context
    - Test 7 (case_insensitive_tags): sanitize_context strips "<MEMORY-CONTEXT>...</Memory-Context>" the same as lowercase
    - Test 8 (multi_block_in_one_input): two back-to-back <memory-context>...</memory-context> blocks in one string are both removed
  </behavior>
  <action>
    Create crates/ironhermes-agent/src/memory_context.rs. Add a file-level doc comment tagging Phase 34a Plan 01 / MEM-READ-02 and citing the Python source. Use `use regex::Regex;` and `use std::sync::OnceLock;` (regex is already a workspace dep — do NOT add it). Define three `static ... : OnceLock<Regex>` plus get_or_init accessor fns: `internal_context_re` (matches a full `<memory-context>...</memory-context>` block, dotall+case-insensitive, non-greedy), `internal_note_re` (matches the `[System note: ...]` line for BOTH variant phrasings), `fence_tag_re` (matches a bare open OR close fence tag, case-insensitive). Use the exact patterns from 34A-RESEARCH.md "Code Examples". Implement `pub fn sanitize_context(text: &str) -> String` applying the three regexes in EXACTLY this order (per RESEARCH Pitfall 6): internal_context_re -> internal_note_re -> fence_tag_re, returning an owned String. Implement `pub fn build_memory_context_block(raw: &str) -> Option<String>`: return None if `raw.trim().is_empty()`; else `sanitize_context(raw)` then wrap in the byte-exact Python format with the em dash as `\u{2014}` (NOT a literal — keep the source ASCII-safe but emit the same UTF-8 bytes). The wrapper text must match Python so internal_note_re strips it on a re-wrap (idempotency). Write the 8 sync `#[test]` fns in an inline `#[cfg(test)] mod tests` per the behavior block. In lib.rs, add `pub mod memory_context;` alongside the existing `pub mod nudge;` declaration.
  </action>
  <verify>
    <automated>cargo test -p ironhermes-agent --lib memory_context::tests 2>&1 | tail -8</automated>
  </verify>
  <acceptance_criteria>
    - `cargo test -p ironhermes-agent --lib memory_context::tests` reports 8 passed; 0 failed
    - `cargo build -p ironhermes-agent` is clean
    - `grep -c "<memory-context>" crates/ironhermes-agent/src/memory_context.rs` >= 4
    - `grep -c "pub mod memory_context" crates/ironhermes-agent/src/lib.rs` == 1
    - `grep -c "u{2014}" crates/ironhermes-agent/src/memory_context.rs` >= 1 (em dash emitted via escape)
    - Regex application order in sanitize_context body is internal_context_re THEN internal_note_re THEN fence_tag_re (verify by reading the fn body; reversed order fails the double_wrap_idempotency test)
    - `grep -v '^//' crates/ironhermes-agent/src/memory_context.rs | grep -c "lazy_static"` == 0 (uses OnceLock, not lazy_static)
  </acceptance_criteria>
  <done>memory_context.rs exists with sanitize_context + build_memory_context_block; 8 unit tests pass; module is declared in lib.rs; idempotency (sanitize ∘ build unwraps) holds.</done>
</task>

</tasks>

<threat_model>
## Trust Boundaries

| Boundary | Description |
|----------|-------------|
| memory provider -> agent context | Recall text returned by a provider's `prefetch_with_query` is untrusted; it will (in 34a-02) be injected into the model context wrapped in a `<memory-context>` fence. A malicious or buggy provider could embed its own fence tags or a forged `[System note]` to spoof the recall-context boundary. |

## STRIDE Threat Register

| Threat ID | Category | Component | Disposition | Mitigation Plan |
|-----------|----------|-----------|-------------|-----------------|
| T-34a-01 | Tampering | provider recall text wrapped by `build_memory_context_block` | mitigate | `build_memory_context_block` calls `sanitize_context` BEFORE wrapping, stripping any embedded `<memory-context>` blocks, orphan `[System note]` lines, and bare fence tags. A provider cannot forge a recall boundary or nest a fake system note — the idempotency test (Test 4) locks this. |
| T-34a-02 | Spoofing | forged `[System note]` in recall text | mitigate | `internal_note_re` matches both variant phrasings of the system note and strips them in `sanitize_context`, so a provider cannot inject a counterfeit authority preamble that survives wrapping. |
| T-34a-03 | Information Disclosure | regex catastrophic backtracking on adversarial recall text | accept | Patterns use non-greedy `[\s\S]*?` and bounded character classes; the `regex` crate (RE2-style, no backtracking) guarantees linear-time matching. No DoS surface. |
</threat_model>

<verification>
After both tasks:
```bash
cargo build -p ironhermes-core -p ironhermes-agent
cargo test -p ironhermes-core --lib memory_provider
cargo test -p ironhermes-agent --lib memory_context::tests
cargo test -p ironhermes-agent --lib memory::manager
# Cross-phase regression gates (must stay green — Plan 01 should not affect them):
cargo test -p ironhermes-agent --lib nudge::tests
cargo test -p ironhermes-core --lib test_snapshot_frozen_after_load
```
</verification>

<success_criteria>
- `MemoryProvider::prefetch_with_query` exists with a default no-op; file provider unchanged
- `MemoryManager::prefetch_with_query` is a primary-only proxy
- `memory_context.rs` exists; `sanitize_context` + `build_memory_context_block` ported byte-exact; 8 tests pass
- `pub mod memory_context;` declared in lib.rs
- regex order in sanitize_context is block -> note -> fence (idempotency holds)
- D-12 gate (`test_snapshot_frozen_after_load`) and Phase 32 `nudge::tests` stay green (this plan touches neither path)
</success_criteria>

<output>
Create `.planning/phases/34a-read-side-memory-parity/34a-01-SUMMARY.md` when done.
</output>
