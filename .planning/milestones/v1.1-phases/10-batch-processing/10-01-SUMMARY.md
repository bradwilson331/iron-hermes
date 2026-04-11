---
phase: 10-batch-processing
plan: "01"
subsystem: batch-processing
tags: [batch, sharegpt, checkpoint, parallel, jsonl]
dependency_graph:
  requires:
    - ironhermes-core/config (BatchConfig)
    - ironhermes-agent (AgentLoop, LlmClient)
    - ironhermes-tools (ToolRegistry)
  provides:
    - ironhermes-cli/batch (BatchCommands, handle_batch_command)
    - ironhermes-cli/batch/checkpoint (prompt_hash, load_checkpoint, save_checkpoint)
    - ironhermes-cli/batch/sharegpt (messages_to_sharegpt)
    - ironhermes-cli/batch/runner (cmd_run, cmd_status, cmd_cancel, cmd_list)
  affects:
    - ironhermes-cli/main.rs (Commands::Batch variant added)
    - ironhermes-core/lib.rs (BatchConfig re-exported)
tech_stack:
  added:
    - sha2 = "0.10" (SHA-256 hashing for checkpoint keys)
    - regex = "1" (available for Plan 02 quality filtering)
  patterns:
    - JoinSet + Semaphore + mpsc for bounded parallel execution
    - atomic rename (tmp + rename) for checkpoint durability
    - JSONL streaming (one entry per line, line-by-line reader)
    - Cancel sentinel file for cross-process cancellation
key_files:
  created:
    - crates/ironhermes-cli/src/batch/mod.rs
    - crates/ironhermes-cli/src/batch/types.rs
    - crates/ironhermes-cli/src/batch/checkpoint.rs
    - crates/ironhermes-cli/src/batch/sharegpt.rs
    - crates/ironhermes-cli/src/batch/runner.rs
    - crates/ironhermes-cli/src/batch/tests.rs
  modified:
    - crates/ironhermes-core/src/config.rs (BatchConfig struct + Config.batch field)
    - crates/ironhermes-core/src/lib.rs (BatchConfig re-export)
    - crates/ironhermes-cli/Cargo.toml (sha2, regex deps)
    - crates/ironhermes-cli/src/main.rs (Commands::Batch + dispatch)
decisions:
  - "Cancel uses sentinel file (hermes_home/batch/cancel) — no IPC needed, cmd_run polls before each dispatch"
  - "Checkpoint keyed by SHA-256 of prompt text (content hash, not line number) per D-05"
  - "Writer task owns all file I/O over mpsc channel — prevents concurrent write corruption"
  - "Quality always passes (passed=true) in Plan 01; Plan 02 adds real filtering (D-13)"
  - "runs.json at hermes_home/batch/runs.json for batch list persistence"
metrics:
  duration: "~15 minutes"
  completed_date: "2026-04-10"
  tasks_completed: 2
  files_changed: 10
---

# Phase 10 Plan 01: Batch Processing Core Summary

Parallel batch execution pipeline with SHA-256 checkpoint/resume and ShareGPT JSONL output via JoinSet+Semaphore+mpsc writer pattern.

## What Was Built

### BatchConfig (ironhermes-core)
Added `BatchConfig` struct with `workers` (default: 4), `max_turns` (default: 20), and `output_dir` (default: "batch_output") fields. Wired into the top-level `Config` struct with `#[serde(default)]` for backward compatibility.

### CLI Batch Subcommand Group
Added `Commands::Batch` to main.rs with four subcommands via `BatchCommands` enum:
- `run <input.jsonl>` — parallel execution with `-o/--output`, `-w/--workers`, `-m/--model` flags
- `status` — show latest/running batch progress
- `cancel` — write sentinel file to stop running batch
- `list` — tabular history of batch runs

### Type Definitions (types.rs)
Complete type set for the batch pipeline:
- `BatchEntry` — input JSONL record (prompt, optional system/tools)
- `TrajectoryLine` — output JSONL record with ShareGPT conversations, usage, quality metadata
- `ShareGptTurn` — `{from, value}` turn with human/gpt/tool_call/tool_response roles
- `CheckpointEntry` — per-prompt completion record (status, timestamp)
- `QualityResult` — passed flag + reasons (populated by Plan 02)
- `BatchRunRecord` — persistent run history record

### Checkpoint Module (checkpoint.rs)
- `prompt_hash(prompt)` — SHA-256 hex string (64 chars), deterministic, content-addressed
- `load_checkpoint(path)` — empty HashMap if missing/empty, JSON parse with context errors
- `save_checkpoint(path, data)` — atomic write via tmp file + rename (T-10-02 mitigation)

### ShareGPT Conversion (sharegpt.rs)
`messages_to_sharegpt` maps `Vec<ChatMessage>` to `Vec<ShareGptTurn>`:
- `Role::User` → `from: "human"`
- `Role::Assistant` text → `from: "gpt"`
- `Role::Assistant` tool_calls → `from: "tool_call"` (one turn per call, JSON-serialized)
- `Role::Tool` → `from: "tool_response"`
- `Role::System` → skipped

### Batch Runner (runner.rs)
`cmd_run` implements the full execution pipeline:
1. Config load, output path resolution, checkpoint load
2. JSONL streaming line-by-line (T-10-03: no full load into memory)
3. Skip entries already in checkpoint (BATCH-03 resume)
4. `JoinSet` + `Arc<Semaphore>` for bounded parallelism (T-10-03 mitigation)
5. `mpsc::channel(256)` — single writer task receives trajectories, appends JSONL, saves checkpoint
6. Passed trajectories → `*_output.jsonl`, rejected → `*_rejected.jsonl` (D-11)
7. Cancel sentinel check before each dispatch (D-03)
8. `runs.json` persistence for `batch list`
9. Checkpoint deletion on clean completion (D-06)

## Tests

8 unit tests all passing:
- `test_prompt_hash_deterministic` — same input → same 64-char hex
- `test_prompt_hash_different_inputs` — different inputs → different hashes
- `test_sharegpt_user_message` — User → "human" turn
- `test_sharegpt_skips_system` — System messages excluded
- `test_trajectory_line_serializes_to_json` — correct JSON shape
- `test_checkpoint_entry_roundtrip` — serialize/deserialize identity
- `test_batch_entry_minimal_parse` — minimal JSONL input parses
- `test_batch_entry_with_optional_fields` — optional system/tools fields work

## Deviations from Plan

None — plan executed exactly as written.

## Known Stubs

- `quality: QualityResult { passed: true, reasons: vec![] }` in runner.rs:197 — all trajectories pass in Plan 01. Real filtering (D-13: min turns, tool use ratio, secrets detection) deferred to Plan 02 Task 1. This is intentional per plan scope.

## Threat Flags

None. All T-10-02 (atomic checkpoint), T-10-03 (bounded memory/concurrency) mitigations applied as designed. T-10-01 (secrets in output) deferred to Plan 02 per threat register.

## Self-Check: PASSED

Files exist:
- crates/ironhermes-cli/src/batch/mod.rs — FOUND
- crates/ironhermes-cli/src/batch/types.rs — FOUND
- crates/ironhermes-cli/src/batch/checkpoint.rs — FOUND
- crates/ironhermes-cli/src/batch/sharegpt.rs — FOUND
- crates/ironhermes-cli/src/batch/runner.rs — FOUND
- crates/ironhermes-cli/src/batch/tests.rs — FOUND

Commits exist:
- e73c99e — Task 1: BatchConfig, CLI batch subcommand skeleton, type definitions
- e857114 — Task 2: implement checkpoint, sharegpt, and batch runner

Tests: 10 passed, 0 failed
Workspace: Finished dev profile, 0 errors
