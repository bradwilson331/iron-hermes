---
status: complete
phase: 10-batch-processing
source: [10-01-SUMMARY.md, 10-02-SUMMARY.md, 10-03-SUMMARY.md]
started: 2026-04-10T12:00:00Z
updated: 2026-04-10T14:30:00Z
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

### 7. Checkpoint Resume (re-verify)
expected: Start a batch run with several prompts, then cancel or interrupt it mid-run. Re-run the same command. The second run skips already-completed prompts and only processes remaining ones. Output file contains all results without duplicates. (Fix: stale cancel sentinel is now cleaned up at run start.)
result: issue
reported: "fail - ignores the cancel"
severity: major

### 8. ShareGPT Output Format
expected: After a successful batch run, inspect the output JSONL. Each line is valid JSON with ShareGPT conversations array containing turns with from (human/gpt/tool_call/tool_response) and value fields. System messages are excluded.
result: pass

### 9. Quality Filter: Reject Separation (re-verify)
expected: Run a batch where some prompts produce text-only responses (no tool calls). Check that a *_rejected.jsonl file exists alongside the output. Rejected entries have a rejection_reason field. (Fix: filter_no_reasoning now requires tool calls — text-only responses are rejected.)
result: issue
reported: "pass on function is too strict, it now rejects prompts that would normally pass: {\"passed\":false,\"reasons\":[\"no_reasoning_steps\"]},\"conversations\":[{\"from\":\"human\",\"value\":\"why is the sky blue?\"}"
severity: major

### 10. Secrets Detection Filter (re-verify)
expected: If a trajectory contains patterns like API keys (sk-..., AKIA...), Bearer tokens, or PEM blocks in assistant or tool output, it should be routed to the rejected file with a secrets-related rejection reason. (Fix: filter now scans Role::Assistant messages too.)
result: pass

## Summary

total: 10
passed: 8
issues: 2
pending: 0
skipped: 0
blocked: 0

## Gaps

- truth: "Cancel stops dispatching new prompts; resume skips completed and processes remaining"
  status: failed
  reason: "User reported: fail - ignores the cancel"
  severity: major
  test: 7
  root_cause: ""
  artifacts: []
  missing: []
  debug_session: ""

- truth: "no_reasoning filter rejects low-quality responses without rejecting valid text-only answers"
  status: failed
  reason: "User reported: pass on function is too strict, it now rejects prompts that would normally pass: no_reasoning_steps for 'why is the sky blue?'"
  severity: major
  test: 9
  root_cause: ""
  artifacts: []
  missing: []
  debug_session: ""
