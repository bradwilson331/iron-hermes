---
status: diagnosed
phase: 10-batch-processing
source: [10-01-SUMMARY.md, 10-02-SUMMARY.md]
started: 2026-04-10T12:00:00Z
updated: 2026-04-10T12:15:00Z
---

## Current Test

[testing complete]

## Tests

### 1. Cold Start Smoke Test
expected: Kill any running ironhermes process. Run `cargo build` from clean state. Build completes with 0 errors. Run `ironhermes batch --help` — shows subcommands: run, status, cancel, list. No panics or missing module errors.
result: pass

### 2. Batch Run Basic Execution
expected: Create a test input JSONL file with 2-3 simple prompts. Run `ironhermes batch run input.jsonl -o test_output`. Command executes prompts in parallel, prints progress, and produces `test_output_output.jsonl` with ShareGPT-formatted trajectories.
result: pass

### 3. Batch Run Worker/Model Flags
expected: Run `ironhermes batch run input.jsonl -w 2 -m some-model`. The `-w` flag limits concurrency to 2 workers, `-m` selects the model. Command accepts both flags without error.
result: pass

### 4. Batch Status
expected: After running a batch, run `ironhermes batch status`. Shows the latest/most recent batch run with progress info (completed count, total, pass/reject counts, status).
result: pass

### 5. Batch Cancel
expected: Start a batch run with many prompts. In another terminal, run `ironhermes batch cancel`. The running batch stops dispatching new prompts and exits gracefully.
result: pass

### 6. Batch List
expected: After running one or more batches, run `ironhermes batch list`. Shows a tabular history of past runs with timestamps, status, and pass/reject counts.
result: pass

### 7. Checkpoint Resume
expected: Start a batch run with several prompts, then cancel or interrupt it mid-run. Re-run the same command. The second run skips prompts that already completed and only processes remaining prompts. Output file contains all results without duplicates.
result: issue
reported: "fail it said it re-ran but the output is not in the file."
severity: major

### 8. ShareGPT Output Format
expected: After a successful batch run, inspect the output JSONL. Each line is valid JSON with ShareGPT conversations array containing turns with from (human/gpt/tool_call/tool_response) and value fields. System messages are excluded.
result: pass

### 9. Quality Filter: Reject Separation
expected: After a batch run that produces some rejected trajectories, check that a *_rejected.jsonl file exists alongside the output file. Rejected entries have a rejection_reason field explaining why they failed quality checks.
result: issue
reported: "the batch did not run but there was no test_reject_rejected.jsonl"
severity: major

### 10. Secrets Detection Filter
expected: If a trajectory contains patterns like API keys (sk-..., AKIA...), Bearer tokens, or PEM blocks in tool output, it should be routed to the rejected file with a secrets-related rejection reason.
result: issue
reported: "file did not run,good - failed because no output file with rejection reason created"
severity: major

## Summary

total: 10
passed: 7
issues: 3
pending: 0
skipped: 0
blocked: 0

## Gaps

- truth: "Second batch run appends resumed results to output file"
  status: failed
  reason: "User reported: fail it said it re-ran but the output is not in the file."
  severity: major
  test: 7
  root_cause: "Cancel sentinel file (hermes_home/batch/cancel) is never deleted after cmd_cancel writes it. On resume, cmd_run checks cancel_path.exists() at runner.rs:201 on the first loop iteration and breaks immediately — zero prompts dispatched."
  artifacts:
    - path: "crates/ironhermes-cli/src/batch/runner.rs"
      issue: "cancel sentinel never cleaned up; stale file blocks all future runs"
  missing:
    - "Add `let _ = std::fs::remove_file(&cancel_path);` at runner.rs:118 after cancel_path is defined"
  debug_session: ""

- truth: "Rejected trajectories written to separate *_rejected.jsonl file"
  status: failed
  reason: "User reported: the batch did not run but there was no test_reject_rejected.jsonl"
  severity: major
  test: 9
  root_cause: "filter_no_reasoning (filters.rs:57) has a 10-char threshold that any coherent LLM reply exceeds. Empty prompts still get >10 char responses like 'How can I help?', so has_text=true and filter returns None. No trajectories ever get rejected."
  artifacts:
    - path: "crates/ironhermes-cli/src/batch/filters.rs"
      issue: "filter_no_reasoning threshold too permissive — any non-trivial model reply passes"
  missing:
    - "Tighten filter_no_reasoning: require tool calls for trajectory to pass, or raise threshold significantly"
  debug_session: ""

- truth: "Trajectories with secret patterns in tool output routed to rejected file with secrets_in_output reason"
  status: failed
  reason: "User reported: file did not run,good - failed because no output file with rejection reason created"
  severity: major
  test: 10
  root_cause: "filter_secrets_in_output (filters.rs:91-102) only scans Role::Tool messages. When model echoes secrets in assistant text (Role::Assistant), the pattern is never matched. Secrets in non-tool output are invisible to this filter."
  artifacts:
    - path: "crates/ironhermes-cli/src/batch/filters.rs"
      issue: "filter_secrets_in_output only checks Role::Tool, misses Role::Assistant content"
  missing:
    - "Extend filter_secrets_in_output to also scan Role::Assistant messages"
  debug_session: ""
