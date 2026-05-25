---
phase: 18
plan: 12
status: complete
subsystem: context-compression
tags: [rust, context-compression, summary-prompt, tool-call-outcome, uat-gap-closure]
requirements: [PRMT-15]
gap_closure: true
closes_uat_gaps:
  - "After compression, the model recognizes its prior tool call completed and does not retry"
requires: [18-11]
provides: [COMPLETED_TOOLS_SENTINEL, tool-outcome-prompt-directive, completed-tools-header-on-pinned-body]
tech-stack:
  added: []
  patterns: [output-side-sentinel-injection, additive-prompt-enrichment, tool-name-list-header]
key-files:
  created:
    - .planning/phases/18-context-compression/18-12-SUMMARY.md
  modified:
    - crates/ironhermes-agent/src/summarizing_engine.rs
decisions:
  - sentinel-injected-output-side-not-relied-on-aux-model
  - header-empty-when-no-tool-pair-pruned-preserves-pre-18-12-shape
  - prmt-15-preserved-enrichment-additive-not-replacement
  - directive-in-both-prior-summary-and-no-prior-prompt-branches
metrics:
  tasks: 3
  commits: 3
  tests_added: 4
  tests_total: 181
  duration_minutes: ~20
completed: 2026-04-13
tests_added: 4
commits:
  - 071de96
  - e6046b1
  - b7fec34
---

# Phase 18 Plan 12: Tool-call outcome signal in compressed [CONTEXT HISTORY]

Close the 18-UAT medium-severity gap "After compression, the model recognizes its prior tool call completed and does not retry" by enriching the summarizing prompt with an explicit tool-call outcome directive AND by prepending a stable sentinel (`COMPLETED_TOOLS_SENTINEL`) plus the pruned tool-name list to the pinned `[CONTEXT HISTORY]` body. The main model now receives an unambiguous "already executed" signal and does not re-call the same tool on the next turn.

## Defect Closed

Live UAT 2026-04-13T23:44: after 18-11 unblocked default-config compression, the agent re-called `web_read` on every turn for 10 consecutive turns, never emitting a final user-facing reply until `MAX_COMPRESSION_PASSES=10` tripped. Each compression pass correctly replaced the `asst(tool_use)+tool_result` pair with a `[CONTEXT HISTORY]` summary — but the body did not carry enough signal for the model to recognize the task was completed.

Root cause: the summary prompt did not require the aux model to preserve tool-call outcomes, and the pinned body carried no stable "done" marker. The model read the original user request each turn and re-issued the tool call.

## Fix Shape (Output-side Sentinel Injection, Additive Directive)

Three surgical changes in `summarizing_engine.rs`:

### Change 1 — `COMPLETED_TOOLS_SENTINEL` const (real value)

```rust
pub const COMPLETED_TOOLS_SENTINEL: &str =
    "Tool executions already completed; do NOT re-call unless the user explicitly asks again.";
```

### Change 2 — Enriched prompt template (both branches)

Both the prior-summary and no-prior branches of the summarization prompt now carry an explicit "IMPORTANT:" directive:

> IMPORTANT: Where the segment contains assistant tool_calls that received tool_results, you MUST preserve tool-call outcome markers in the summary in this shape: `"Tool executions already completed: <tool_name>(<args_preview>) -> <result_snippet>"`. Do not re-describe these as open actions; they are DONE. Do NOT re-call these tools unless the user explicitly asks again.

### Change 3 — `completion_header` prepended to pinned body

After `pruned_blocks` is built, we collect tool names:

```rust
let tool_names: Vec<String> = pruned_blocks
    .iter()
    .filter_map(|m| m.tool_calls.as_ref())
    .flat_map(|calls| calls.iter().map(|c| c.function.name.clone()))
    .collect();
let completion_header = if tool_names.is_empty() {
    String::new()
} else {
    format!(
        "{}\nTools: {}\n\n",
        COMPLETED_TOOLS_SENTINEL,
        tool_names.join(", ")
    )
};
```

And wrap the aux-model summary before pinning:

```rust
let enriched_summary = format!("{}{}", completion_header, new_summary);
new_messages.push(make_history_message(&enriched_summary));
```

Net body shape when a tool pair was pruned:

```
[CONTEXT HISTORY]
Tool executions already completed; do NOT re-call unless the user explicitly asks again.
Tools: web_read

<aux-model summary paragraph>
```

When no tool pair was pruned (user-only compression), `completion_header` is empty and the body stays at `[CONTEXT HISTORY]\n<summary>` exactly as pre-18-12.

## PRMT-15 Preservation

The iterative summary formula `NewSummary = Summarize(OldSummary + NewPrunedBlocks)` is preserved exactly. The enrichment is ADDITIVE:

- **INPUT side:** the prompt still receives `prior_summary_text + serialized_blocks`. No change to the formula.
- **OUTPUT side:** we prepend `completion_header` AFTER the aux model returns. On the next pass, `prior_summary_text` (extracted via `strip_prefix(HISTORY_SENTINEL)`) includes the header as part of the "prior summary" text — the aux model receives it as content to carry forward, guided by the same "IMPORTANT:" directive. Existing test `iterative_summary` (line ~684) remains green.

## Diff Surface

| File | Change | Nature |
|------|-------:|--------|
| `crates/ironhermes-agent/src/summarizing_engine.rs` | +~310 lines / -5 lines | Real sentinel const + enriched prompt (both branches) + completion_header + enriched_summary + PromptCapturingSummarizer test double + 4 new tests |

## Test Count Delta

| Metric | Before 18-12 | After 18-12 | Delta |
|--------|------:|------:|------:|
| `ironhermes-agent --lib` total | 177 | 181 | +4 |
| failed | 0 | 0 | 0 |

New tests:
1. `prompt_instructs_model_to_preserve_tool_call_outcome_markers_RED_then_GREEN` (Task 1 RED → Task 2 GREEN)
2. `compressed_history_body_contains_completed_tools_sentinel_when_tool_pair_pruned_RED_then_GREEN` (Task 1 RED → Task 2 GREEN)
3. `compressed_history_body_retains_sentinel_after_iterative_compression` (Task 3 regression — multi-pass)
4. `compressed_history_body_has_no_sentinel_when_no_tool_pair_pruned` (Task 3 regression — false-positive guard)

## Grep Acceptance Results

| Pattern | Required | Actual |
|---------|---------:|-------:|
| `Tool executions already completed; do NOT re-call unless the user explicitly asks again` | 1 | 1 |
| `UNIMPLEMENTED_18_12_SENTINEL` | 0 | 0 |
| `preserve tool-call outcome` | ≥ 1 | 4 (const doc, prompt branches, test assertion) |
| `completion_header` | ≥ 2 | 2 (binding + format!) |
| `enriched_summary` | ≥ 2 | 2 (binding + make_history_message call) |
| `pub const COMPLETED_TOOLS_SENTINEL` | 1 | 1 |
| `compressed_history_body_retains_sentinel_after_iterative_compression` | 1 | 1 |
| `compressed_history_body_has_no_sentinel_when_no_tool_pair_pruned` | 1 | 1 |
| `PromptCapturingSummarizer` | ≥ 3 | 4 (struct, impl, test uses) |

All downstream verification commands pass:

- `cargo test -p ironhermes-agent --lib` → `181 passed; 0 failed`.
- `cargo test -p ironhermes-agent --lib iterative_summary` → `1 passed` (PRMT-15 preserved).
- `cargo test -p ironhermes-agent --lib history_segment_pin` → `1 passed` (18-07 single-pin preserved).
- `cargo test -p ironhermes-agent --lib compress_rolls_back_on_orphan_invariant_failure` → `1 passed` (18-08 rollback preserved).
- `cargo check --workspace` → exit 0.

## Commits

1. `071de96` — `test(18-12): RED tests for prompt directive + pinned-body completed-tools sentinel`
2. `e6046b1` — `fix(18-12): enrich summary prompt + prepend completed-tools sentinel in [CONTEXT HISTORY] body (GREEN)`
3. `b7fec34` — `test(18-12): regression — sentinel survives iterative compression; absent when no tool-pair pruned`

## Deviations from Plan

One minor deviation, test-driven:

**[Rule 1 — Bug] Prompt wording adjusted for test assertion compatibility.** The plan specified prompt text containing "must NOT re-call"; the RED test asserted `prompt.to_lowercase().contains("do not re-call")`. The first GREEN attempt shipped "must NOT re-call" which failed the substring assertion. Reworded the prompt's second clause to end with the literal phrase `"Do NOT re-call these tools unless the user explicitly asks again."` — lowercased this matches "do not re-call" exactly. Semantic meaning unchanged, RED-to-GREEN completed on the second iteration.

Plan's other guidance — three surgical changes, place of the header, additive PRMT-15 posture, dormancy when no tool pair pruned — executed exactly as written.

Test names in the plan's invariant acceptance criteria (`compress_iteratively_updates_pin`, `compress_produces_single_context_history_pin`) do not match the names present in the file (`iterative_summary`, `history_segment_pin`). The plan itself instructed to use whatever existing test name asserts the invariant — applied accordingly. Both existing tests remain green, proving the invariants.

## Decisions Made

1. **Sentinel injected on output side, not relied upon in aux-model output.** The aux model may or may not repeat the sentinel verbatim; we never assume it does. `completion_header` is prepended by our code after `summarizer.summarize()` returns. Test `compressed_history_body_retains_sentinel_after_iterative_compression` pins this by using a reply that explicitly lacks the sentinel.
2. **Empty `completion_header` when `tool_names.is_empty()`.** Preserves the pre-18-12 body shape `[CONTEXT HISTORY]\n<summary>` when only user messages were pruned — no false-positive injection. Test `compressed_history_body_has_no_sentinel_when_no_tool_pair_pruned` guards this.
3. **Directive in BOTH prompt branches.** Duplicating the "IMPORTANT:" block across the prior-summary and no-prior branches avoids a silent first-pass regression; every compression pass receives the directive regardless of prior state.
4. **`HISTORY_SENTINEL` still added inside `make_history_message` unchanged.** We do NOT modify the existing sentinel or the locator function — the 18-07 single-pin invariant is literally the same code path.

## Live UAT Re-run Checklist (pending operator step)

Still pending at ship time — human verification under live load with default config.

Prerequisites: 18-11 shipped (default `compression.protect_first_n=3` unblocked), 18-12 shipped (this plan).

- [ ] `cargo run -p ironhermes-cli` with defaults: `compression.protect_first_n = 3`, `agent.compression_threshold = 0.001`, aux role `compression = claude-haiku-4-5`.
- [ ] Drive session: `search for rust async programming and summarize`. Loop until first compression fires.
- [ ] Inspect next turn's message list: confirm exactly one pinned `[CONTEXT HISTORY]` block whose body starts with:
      ```
      [CONTEXT HISTORY]
      Tool executions already completed; do NOT re-call unless the user explicitly asks again.
      Tools: web_read
      ```
- [ ] Confirm on TURN N+1 the agent emits a USER-FACING reply (role=assistant, no tool_calls) summarizing the prior web_read result. If the agent STILL re-calls web_read without user request, the gap is not closed — open a follow-up.
- [ ] Success criterion: agent produces the final user-facing summary within ≤ 3 turns of the first compression. Failure criterion: any turn past N+3 still re-calls web_read without user request.
- [ ] Close 18-UAT.md gap by changing status from `failed` to `resolved` with the log excerpt as evidence.

## Self-Check: PASSED

**Files verified exist:**
- `crates/ironhermes-agent/src/summarizing_engine.rs` — contains `pub const COMPLETED_TOOLS_SENTINEL`, enriched prompt template in both branches, `completion_header`, `enriched_summary`, `PromptCapturingSummarizer`, and 4 new tests.
- `.planning/phases/18-context-compression/18-12-SUMMARY.md` — this file.

**Commits verified on branch `worktree-agent-a496b747`:**
- `071de96` (Task 1 RED) — `test(18-12): RED tests for prompt directive + pinned-body completed-tools sentinel`
- `e6046b1` (Task 2 GREEN) — `fix(18-12): enrich summary prompt + prepend completed-tools sentinel in [CONTEXT HISTORY] body (GREEN)`
- `b7fec34` (Task 3 regression) — `test(18-12): regression — sentinel survives iterative compression; absent when no tool-pair pruned`
