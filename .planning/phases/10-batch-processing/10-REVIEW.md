---
phase: 10-batch-processing
type: code-review
depth: standard
status: findings
files_reviewed: 10
files_reviewed_list:
  - crates/ironhermes-cli/src/batch/runner.rs
  - crates/ironhermes-cli/src/batch/filters.rs
  - crates/ironhermes-cli/src/batch/tests.rs
  - crates/ironhermes-cli/src/batch/mod.rs
  - crates/ironhermes-cli/src/batch/types.rs
  - crates/ironhermes-cli/src/batch/checkpoint.rs
  - crates/ironhermes-cli/src/batch/sharegpt.rs
  - crates/ironhermes-cli/Cargo.toml
  - crates/ironhermes-cli/src/main.rs
  - crates/ironhermes-core/src/config.rs
findings: 10
by_severity:
  critical: 0
  high: 2
  medium: 4
  low: 2
  info: 2
created: 2026-04-10T12:00:00Z
---

# Phase 10: Batch Processing Code Review

**Reviewed:** 2026-04-10
**Depth:** standard (per-file analysis with language-specific checks)
**Files Reviewed:** 10
**Status:** findings

## Summary

The batch processing module is well-structured with good separation of concerns: types, checkpoint persistence, ShareGPT conversion, quality filters, and the main runner. The cancel sentinel timestamp-guard and select!-based polling are solid concurrency patterns. Quality filters are comprehensive. However, there are two high-severity bugs around silent error swallowing and a race condition in run record persistence, plus several medium-severity issues around error handling and filter accuracy.

## High

### HI-01: Silent swallowing of agent errors loses trajectory data

**File:** `crates/ironhermes-cli/src/batch/runner.rs:282-284`
**Category:** bug
**Issue:** When `agent.run(messages)` returns an `Err`, the error is printed to stderr but the prompt hash is never written to the checkpoint. This means the failed prompt will be retried on resume, which is correct. However, the entry is also never counted in `completed`, `passed`, or `rejected` -- so `total_entries` vs `completed` will be permanently mismatched in the final run record. More critically, the worker also silently drops the semaphore permit and tx channel without sending anything, which is fine mechanically but means errors are invisible in the output files. There is no way to audit which prompts failed vs were simply not reached during a cancel.
**Fix:** Send a rejected trajectory for agent errors so they appear in the reject file and checkpoint, or at minimum track error count in the run record:
```rust
Err(e) => {
    eprintln!("{} Agent error for prompt hash {}: {}", "Error:".red(), &hash_clone[..8], e);
    let trajectory = TrajectoryLine {
        id: hash_clone.clone(),
        model: model_for_traj,
        timestamp: Utc::now().to_rfc3339(),
        usage: UsageInfo { prompt_tokens: 0, completion_tokens: 0 },
        turns: 0,
        quality: QualityResult { passed: false, reasons: vec!["agent_error".to_string()] },
        conversations: vec![],
        rejection_reason: Some(format!("agent_error: {}", e)),
    };
    let _ = tx.send((trajectory, hash_clone)).await;
}
```

### HI-02: Race condition in run record file persistence

**File:** `crates/ironhermes-cli/src/batch/runner.rs:485-506`
**Category:** bug
**Issue:** `save_run_record` performs a read-modify-write cycle (`load_run_records` -> find/update -> write) without any file locking. If multiple batch runs are started concurrently (different input files), they will race on `runs.json`. One run's update could overwrite another's, losing run records. The atomic temp+rename protects against partial writes but not concurrent read-modify-write.
**Fix:** Use file locking (e.g., `fs2::FileExt::lock_exclusive` or `fd-lock`) around the read-modify-write cycle. Alternatively, use per-run record files instead of a single `runs.json` to eliminate contention:
```rust
// Option A: file lock
use fs2::FileExt;
let lock_file = std::fs::File::create(path.with_extension("lock"))?;
lock_file.lock_exclusive()?;
// ... read, modify, write ...
lock_file.unlock()?;
```

## Medium

### MD-01: filter_error_only false positives on benign content containing "failed"

**File:** `crates/ironhermes-cli/src/batch/filters.rs:96-101`
**Category:** bug
**Issue:** The `filter_error_only` check uses substring matching (`content.contains("failed")`) which will produce false positives on legitimate tool output that happens to contain the word "failed". For example, a file read that contains text like "The experiment failed to replicate the results" would be flagged as an error. Combined with the `all_errors` logic, a single tool call returning content with the word "failed" anywhere would reject the entire trajectory.
**Fix:** Use more precise error detection -- either match only at the start of the string (like the `Error:` check does) or use a structured error field rather than string matching:
```rust
let all_errors = tool_results.iter().all(|content| {
    content.starts_with("Error:")
        || content.starts_with("error:")
        || content.starts_with("BLOCKED")
        // Remove the loose .contains("failed") check
});
```

### MD-02: Writer task silently ignores file I/O errors

**File:** `crates/ironhermes-cli/src/batch/runner.rs:162-178`
**Category:** bug
**Issue:** Both the output file write (line 167) and reject file write (line 178) use `let _ = file.write_all(...)` which silently discards write errors (disk full, permission denied, etc.). The trajectory would be lost -- it is counted in passed/rejected but never persisted to disk. The checkpoint would still be updated, so on resume the prompt would be skipped, meaning the data is permanently lost.
**Fix:** At minimum, log the error. Ideally, propagate it or use a counter to track write failures:
```rust
if let Err(e) = file.write_all(line.as_bytes()).await {
    eprintln!("Error: failed to write trajectory: {}", e);
    // Consider: don't update checkpoint for this entry
}
```

### MD-03: Checkpoint temp file extension collision

**File:** `crates/ironhermes-cli/src/batch/checkpoint.rs:30`
**Category:** bug
**Issue:** `save_checkpoint` uses `path.with_extension("checkpoint.tmp")`. If the checkpoint path is `foo.checkpoint.json`, then `with_extension` replaces `.json` with `.checkpoint.tmp`, producing `foo.checkpoint.checkpoint.tmp`. Meanwhile in `runner.rs:501`, `save_run_record` uses `path.with_extension("runs.tmp")` on a path that ends in `.json`, producing the correct `runs.runs.tmp` -- wait, that is also wrong. The `with_extension` method replaces only the last extension. So `foo.checkpoint.json` -> `foo.checkpoint.checkpoint.tmp` (double "checkpoint"). This works correctly in practice only because the checkpoint path is `output.checkpoint.json` (line 57 of runner.rs), but the naming is confusing and fragile.
**Fix:** Use a more explicit temp path construction:
```rust
let tmp = path.parent().unwrap_or(Path::new(".")).join(
    format!("{}.tmp", path.file_name().unwrap_or_default().to_string_lossy())
);
```

### MD-04: `started_at` field overwritten then patched back from disk

**File:** `crates/ironhermes-cli/src/batch/runner.rs:305-322`
**Category:** bug
**Issue:** The final run record is created with `started_at: Utc::now().to_rfc3339()` (line 313, commented "approximate") and then immediately patched by re-reading from disk (lines 318-322). This is an unnecessary round-trip that also has a subtle bug: if `load_run_records` fails (returns Err), the `started_at` remains the incorrect "now" value instead of the actual start time. The original `run_record` created at line 127 already has the correct `started_at` -- it should be reused.
**Fix:** Preserve the original `started_at` from the initial record rather than reconstructing:
```rust
let mut final_record = run_record; // reuse the original
final_record.completed = passed_count + rejected_count;
final_record.passed = passed_count;
final_record.rejected = rejected_count;
final_record.finished_at = Some(Utc::now().to_rfc3339());
final_record.status = final_status.to_string();
```

## Low

### LO-01: `tools` field in BatchEntry is parsed but never used

**File:** `crates/ironhermes-cli/src/batch/types.rs:12`
**Category:** quality
**Issue:** `BatchEntry` has a `tools: Option<Vec<String>>` field that is deserialized from input but never referenced anywhere in the runner or filters. The runner always creates a full default `ToolRegistry` (runner.rs:239-243) regardless of this field. This is dead data -- it suggests an incomplete feature (tool allowlisting per prompt).
**Fix:** Either remove the field to avoid confusion, or add a comment documenting it as a planned feature:
```rust
/// Optional tool allowlist (planned, not yet enforced).
#[serde(default)]
pub tools: Option<Vec<String>>,
```

### LO-02: Unused variable `_run_id_clone`

**File:** `crates/ironhermes-cli/src/batch/runner.rs:148`
**Category:** quality
**Issue:** `_run_id_clone` is assigned but never used. The underscore prefix suppresses the compiler warning, but the clone is wasted work.
**Fix:** Remove the unused variable:
```rust
// Remove this line:
// let _run_id_clone = run_id.clone();
```

## Info

### IN-01: `unwrap_or_default()` on serde_json serialization in writer task

**File:** `crates/ironhermes-cli/src/batch/runner.rs:157`
**Category:** quality
**Issue:** `serde_json::to_string(&trajectory).unwrap_or_default()` would produce an empty string if serialization somehow fails, resulting in a blank line written to the output file. Serialization of these types should never fail (no maps with non-string keys, no recursive structures), so this is low-risk, but the silent fallback to empty string is surprising.
**Fix:** Use `.expect("TrajectoryLine serialization is infallible")` or handle the error explicitly.

### IN-02: Magic number 100 in filter_no_reasoning threshold

**File:** `crates/ironhermes-cli/src/batch/filters.rs:67,75`
**Category:** quality
**Issue:** The threshold of 100 characters for "substantive text" is a magic number used in two places. It works but would benefit from being a named constant for clarity and single-point-of-change.
**Fix:**
```rust
/// Minimum character count for a text-only response to be considered substantive.
const MIN_SUBSTANTIVE_TEXT_LEN: usize = 100;
```

---

_Reviewed: 2026-04-10_
_Reviewer: Claude (gsd-code-reviewer)_
_Depth: standard_
