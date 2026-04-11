---
phase: 10-batch-processing
plan: "02"
subsystem: batch-processing
tags: [quality-filters, secrets-detection, batch-management, rejection]
dependency_graph:
  requires:
    - ironhermes-cli/batch/runner (worker pipeline)
    - ironhermes-cli/batch/types (QualityResult, TrajectoryLine)
    - ironhermes-tools (ToolRegistry for hallucination detection)
  provides:
    - ironhermes-cli/batch/filters (quality filter pipeline)
    - batch status/cancel/list subcommands
---

# Plan 10-02 Summary: Quality Filtering & Batch Management

## What Was Built

### Quality Filter Pipeline (BATCH-04)
Created `filters.rs` with four rejection criteria as pure functions:

1. **`filter_hallucinated_tools`** — Detects tool calls to tools not in the ToolRegistry
2. **`filter_no_reasoning`** — Rejects trajectories with no tool calls AND no substantive response
3. **`filter_error_only`** — Rejects trajectories where every tool result is an error
4. **`filter_secrets_in_output`** — Detects API keys, JWTs, AWS keys, Slack tokens, PEM keys via `RegexSet`

`run_filters` orchestrates all four without short-circuiting — collects all applicable reasons (D-13).

### Reject File Routing (D-11)
- Failed trajectories route to `{output}_rejected.jsonl` with `rejection_reason` field
- Nothing silently discarded — both passed and rejected have `quality` metadata

### Batch Management (D-02, D-03, D-04)
- **`batch status`** — Shows latest run progress with colored status
- **`batch cancel`** — Writes cancel sentinel file; running batch stops dispatching
- **`batch list`** — Tabular view of past runs with pass/reject counts

## Test Coverage

25 tests total (all passing):
- 3 filter_hallucinated_tools tests (detect unknown, pass known)
- 4 filter_no_reasoning tests (reject empty, pass with tools/text)
- 2 filter_error_only tests (reject all errors, pass mixed)
- 3 filter_secrets tests (API key, Bearer JWT, AWS key, clean output)
- 2 run_filters integration tests (collects all reasons, passes clean)
- 1 reject_file_path derivation test
- 1 BatchRunRecord serialization test
- Plus 9 tests from Plan 01

## Requirements Satisfied

- **BATCH-04**: Quality filtering with four criteria and reject separation ✓
- **D-02**: `batch status` shows progress ✓
- **D-03**: `batch cancel` graceful shutdown ✓
- **D-04**: `batch list` past runs summary ✓
- **D-11**: Separate reject file with rejection_reason ✓
- **D-13**: Quality metadata on both passed and rejected ✓

## Threat Mitigations

- **T-10-01** (Information Disclosure): SECRET_PATTERNS RegexSet catches credential formats in tool output
- **T-10-05** (Repudiation): Rejected trajectories written to separate file with reasons, nothing discarded

## Files Changed

| File | Change |
|------|--------|
| `crates/ironhermes-cli/src/batch/filters.rs` | NEW — Quality filter pipeline with SECRET_PATTERNS |
| `crates/ironhermes-cli/src/batch/runner.rs` | Wire `run_filters` into worker, add reject routing |
| `crates/ironhermes-cli/src/batch/mod.rs` | Add `pub mod filters` |
| `crates/ironhermes-cli/src/batch/tests.rs` | 14 new filter + integration tests |
| `crates/ironhermes-cli/Cargo.toml` | Add `regex` dependency |
| `crates/ironhermes-agent/src/lib.rs` | Re-export `AgentResult` for filter function signatures |
