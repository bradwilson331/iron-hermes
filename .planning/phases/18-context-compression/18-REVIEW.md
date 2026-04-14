---
status: issues
phase: 18
depth: standard
reviewed_at: 2026-04-13
scope: 59dfb2f..HEAD (plans 18-11 and 18-12)
files_reviewed:
  - crates/ironhermes-agent/src/tool_pair.rs
  - crates/ironhermes-agent/src/summarizing_engine.rs
  - crates/ironhermes-core/src/config.rs
findings:
  blocking: 0
  high: 0
  medium: 2
  low: 3
  info: 2
  total: 7
---

# Phase 18 Plans 11 & 12 — Code Review

Scope: diffs introduced by `59dfb2f..b0f7a5c` — `compute_effective_protect_first_n`,
`SummarizingEngine::compress` wiring, `COMPLETED_TOOLS_SENTINEL` + completion header,
enriched prompt template, and the `protect_first_n` doc comment.

Correctness, safety, and PRMT-15 preservation look solid. Issues below are maintenance-
grade — none block ship. The atomic-rollback path, single-pin invariant, and `configured
== 0` short-circuit are all exercised by new tests. The live UAT re-run still gates
closure of the UAT gaps as documented in the two SUMMARYs.

## Blocking

None.

## High

None.

## Medium

### M-01 — `tool_names` has no dedup; same-name repetition on parallel/iterative pruning

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs:475-488`

The `tool_names` vector is collected via `flat_map` across every `tool_calls` entry in
`pruned_blocks`, then joined with `", "`. When a pair has multiple parallel tool_calls to
the same tool (e.g., two `web_read` calls), or when several assistant turns call the same
tool before compression fires, the pinned body prints `"Tools: web_read, web_read, web_read"`.

This is not a correctness bug — the sentinel still provides a valid "already completed"
signal — but it bloats the pinned body, wastes characters against `HISTORY_SUMMARY_MAX_CHARS
= 8_000`, and can mildly confuse the main model ("why three of the same?").

**Fix:** De-dup while preserving first-seen order:

```rust
let mut seen = std::collections::HashSet::new();
let tool_names: Vec<String> = pruned_blocks
    .iter()
    .filter_map(|m| m.tool_calls.as_ref())
    .flat_map(|calls| calls.iter().map(|c| c.function.name.clone()))
    .filter(|n| seen.insert(n.clone()))
    .collect();
```

### M-02 — `enriched_summary` length cap relies on preexisting byte-slice truncation in `make_history_message`; risk grows as header grows

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs:550` (call site) and
`crates/ironhermes-agent/src/summarizing_engine.rs:52-57` (preexisting truncation)

`enriched_summary = completion_header + new_summary` is passed to
`make_history_message`, which truncates via `&summary_body[..HISTORY_SUMMARY_MAX_CHARS]`.
The byte-index slice is a preexisting UTF-8 panic hazard (out of 18-11/18-12 scope) but
18-12 increases the probability of the body reaching the cap because we now prepend
arbitrary tool names. If a future skill registers a tool with a non-ASCII name, the
combined `enriched_summary` could hit 8_000 bytes mid-codepoint and panic.

**Fix (18-12 scope):** cap the header length explicitly so the 8_000-byte tail is never
reached due to header bloat. Example:

```rust
const COMPLETION_HEADER_MAX_CHARS: usize = 512;
let completion_header = if tool_names.is_empty() {
    String::new()
} else {
    let mut h = format!("{}\nTools: {}\n\n", COMPLETED_TOOLS_SENTINEL, tool_names.join(", "));
    if h.len() > COMPLETION_HEADER_MAX_CHARS {
        h.truncate(
            h.char_indices()
                .take_while(|(i, _)| *i <= COMPLETION_HEADER_MAX_CHARS)
                .last()
                .map(|(i, c)| i + c.len_utf8())
                .unwrap_or(0),
        );
    }
    h
};
```

**Follow-up (separate):** also fix `make_history_message`'s byte-slice truncation to a
char-boundary-safe cut. File a dedicated ticket; out of 18-11/18-12 scope.

## Low

### L-01 — `detect_tool_pairs(messages)` called three times per `compress()`

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs:306, 334, 396`

`detected_pairs_early`, `pairs`, and `pairs_after_shift` all call `detect_tool_pairs`
on the same (unmutated-up-to-that-point) `messages` slice. The first and second calls
operate on the identical slice — only `pairs_after_shift` runs after any mutation would
have happened (though in the current code path, `messages` is not mutated before line
396 either).

Pair detection is O(n) and typically cheap, but this is an obvious redundancy.

**Fix:** hoist the first detection and reuse. Example:

```rust
let pairs = tool_pair::detect_tool_pairs(messages);
let effective_first_n =
    tool_pair::compute_effective_protect_first_n(messages, self.protect_first_n, &pairs);
// ... later, reuse `pairs` for the adaptive-shift loop and atomicity guard.
```

Only re-detect if and when `messages` has actually been mutated (which, inside
`compress()` pre-assignment, it is not).

### L-02 — `_messages` parameter on `compute_effective_protect_first_n` is dead

**File:** `crates/ironhermes-agent/src/tool_pair.rs:141, 166`

The function takes `_messages: &[ChatMessage]` but never reads it. The doc comment
claims it is "reserved for future extensions." This is fine as a deliberate choice,
but callers pay a parameter-passing cost and the API is misleading.

**Fix:** drop the parameter (all 9 call sites in the diff pass `&msgs` / `messages`
but the function ignores it). If genuinely needed later, add it back in a focused
commit. Keeps the API honest today.

### L-03 — Prompt directive ships the string `<tool_name>(<args_preview>) -> <result_snippet>` as a literal template

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs:499, 507`

The enriched prompt shows the aux model a shape template containing literal `<...>`
placeholders. Some models will echo the placeholders verbatim ("tool `<tool_name>` was
called with `<args_preview>`..."). This is a low-probability output-quality risk, not
correctness. The output-side sentinel injection in change 3 (`completion_header`)
compensates — the main model always sees the real tool name regardless of what the aux
model emits.

**Fix (optional):** reword to descriptive rather than template prose:

> Where the segment contains assistant tool_calls that received tool_results, you MUST
> preserve tool-call outcome markers naming the tool, a short argument preview, and a
> short result snippet. Do not re-describe these as open actions — they are DONE.
> Do NOT re-call these tools unless the user explicitly asks again.

## Info

### I-01 — Sentinel-injection ordering depends on implicit short-circuit of the `prior_summary_text` extraction

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs:351-356`

On iterative compression, `prior_summary_text` strips `HISTORY_SENTINEL` but not the
`COMPLETED_TOOLS_SENTINEL` header. This is intentional (the sentinel becomes part of the
prior-summary body, which the aux model is instructed to carry forward, then we prepend a
fresh header on the output side). Worth noting explicitly in a comment so a future
refactor does not "clean this up" and break the iterative-survival test.

**Fix:** add a one-line comment at the `prior_summary_text` construction explaining that
only `HISTORY_SENTINEL` is stripped; `COMPLETED_TOOLS_SENTINEL` intentionally remains in
the summarization input.

### I-02 — Two RED-to-GREEN test names use `#[allow(non_snake_case)]` for readability

**File:** `crates/ironhermes-agent/src/summarizing_engine.rs` (multiple)

Tests are suffixed `_RED_then_GREEN` in UPPER case to mark intent. Acceptable convention
for this phase's workflow, just flagging for consistency — future reviewers should know
this is not accidental `SCREAMING_SNAKE_CASE`.

---

## What Was Checked

- `compute_effective_protect_first_n` invariants: zero-short-circuit, monotonic-downward,
  min-of-conflicting-pairs, orphaned-asst-no-shrink. All covered by unit tests.
- `SummarizingEngine::compress` substitution of `effective_first_n` at all four sites
  (`compute_protect_start`, early-return guard, `prune_start` init, `pin_idx` clamp).
  Substitutions are complete and consistent.
- PRMT-15 iterative-formula preservation. The prompt still receives
  `prior_summary_text + serialized_blocks`; the enrichment is additive on input (directive
  prose) and additive on output (completion header). `iterative_summary` test covers this.
- 18-07 single-pin invariant. `make_history_message` unchanged; `locate_history_segment`
  unchanged. `compressed_history_body_retains_sentinel_after_iterative_compression`
  asserts exactly one pin after two passes.
- 18-08 atomic-rollback path. `snapshot` is taken before the shrink computation;
  summarizer failure still falls back to `LocalPruningEngine`; no half-updated state is
  reachable.
- 18-10 front-straddle guard still reachable as a warn path for genuinely-unsolvable
  malformed inputs (good — defensive log retained).
- Threat model (T-18-12-01 … T-18-12-05): tool names come from structured
  `ToolCall.function.name`, no arg interpolation into the header, no new injection
  surface.
- Config doc comment on `protect_first_n` correctly documents configured-vs-effective
  split.

---

_Reviewed: 2026-04-13_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
