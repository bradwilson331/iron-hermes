---
phase: 05-scheduled-tasks
plan: 02
subsystem: ironhermes-tools, ironhermes-cron
tags: [cron, tool, security, scanner, prompt-injection]
dependency_graph:
  requires: [05-01]
  provides: [CronjobTool, scan_cron_prompt]
  affects: [ironhermes-tools, ironhermes-cron]
tech_stack:
  added: [ironhermes-cron dependency in ironhermes-tools]
  patterns: [Arc<Mutex<JobStore>> shared ownership, RegexSet threat matching, LazyLock static patterns]
key_files:
  created:
    - crates/ironhermes-tools/src/cronjob_tool.rs
    - crates/ironhermes-cron/src/scanner.rs
  modified:
    - crates/ironhermes-tools/src/lib.rs
    - crates/ironhermes-tools/src/registry.rs
    - crates/ironhermes-tools/Cargo.toml
    - crates/ironhermes-cron/src/lib.rs
decisions:
  - "Task 2 (scanner) implemented before Task 1 (tool) to resolve compile dependency order"
  - "scan_cron_prompt returns Err(String) not anyhow::Error to avoid anyhow dependency in ironhermes-cron"
  - "handle_run verifies job exists but returns triggered without executing -- execution deferred to Plan 03 tick runner"
  - "job_to_json helper serializes CronJob to UI-SPEC-compatible shape with schedule_kind discriminator"
metrics:
  duration: "~15 minutes"
  completed: "2026-04-08"
  tasks_completed: 2
  tasks_total: 2
  files_created: 2
  files_modified: 4
---

# Phase 05 Plan 02: CronjobTool and Security Scanner Summary

One-liner: CronjobTool with 8-action dispatch over Arc<Mutex<JobStore>> and RegexSet-based prompt injection scanner blocking injection, exfiltration, and system tampering patterns.

## Tasks Completed

| Task | Name | Commit | Files |
|------|------|--------|-------|
| 2 | Cron prompt security scanner | 9d69cb6 | crates/ironhermes-cron/src/scanner.rs, crates/ironhermes-cron/src/lib.rs |
| 1 | CronjobTool with action dispatch | 7c064fa | crates/ironhermes-tools/src/cronjob_tool.rs, src/lib.rs, src/registry.rs, Cargo.toml |

## What Was Built

### CronjobTool (`crates/ironhermes-tools/src/cronjob_tool.rs`)

Implements the `Tool` trait with `name="cronjob"`, `toolset="cronjob"`. Single required `action` parameter dispatches to 8 handlers:

- `create` — parse schedule, scan prompt, call `store.add_job()`, return `{status:"created", job:{...}}`
- `list` — call `store.list_jobs()`, return `{status:"ok", jobs:[...], count:N}`
- `get` — call `store.find_job()` by id or name, return job or error
- `update` — build `JobUpdate` from optional fields, scan prompt if changed, call `store.update_job()`
- `pause` — call `store.toggle_job(id, false)`, return `{status:"paused", job_id}`
- `resume` — call `store.toggle_job(id, true)`, return `{status:"resumed", job_id, next_run}`
- `run` — verify job exists, return `{status:"triggered", job_id}` (execution deferred to Plan 03)
- `remove` — call `store.remove_job()`, return `{status:"removed", job_id}`

`register_cronjob_tool(store: Arc<Mutex<JobStore>>)` added to `ToolRegistry` following `register_memory_tool` pattern.

### Security Scanner (`crates/ironhermes-cron/src/scanner.rs`)

`scan_cron_prompt(prompt: &str) -> Result<(), String>` using `LazyLock<RegexSet>` with 10 patterns across 3 categories:

- **Injection**: `ignore.*instructions`, `do not tell the user`, `system prompt override`, `disregard your/all/any instructions/rules/guidelines`
- **Credential exfiltration**: `curl/wget` with env var patterns (`$API_KEY`, `$TOKEN`, etc.), `cat` of `.env`/`credentials`/`.netrc`/`.pgpass`
- **System tampering**: `authorized_keys`, `/etc/sudoers`/`visudo`, `rm -rf /`
- **Invisible unicode**: checks against 10 zero-width/directional override characters before regex scan

Error messages follow UI-SPEC copy: `"Blocked: cron prompt contains restricted pattern -- {category}"`.

## Test Results

```
cargo test -p ironhermes-cron scanner    → 12 tests, all pass
cargo test -p ironhermes-tools cronjob   → 18 tests, all pass
cargo build --workspace                  → Finished, 0 errors
```

## Deviations from Plan

### Execution Order Change

**Found during:** Task planning  
**Issue:** Task 1 calls `scan_cron_prompt()` from Task 2 — compile dependency requires Task 2 first.  
**Fix:** Implemented scanner (Task 2) before tool (Task 1). Plan listed them in reverse dependency order.  
**Impact:** None — same artifacts produced, same commits.

None of the plan's functionality was omitted or altered.

## Known Stubs

- `handle_run` returns `{status:"triggered"}` without executing the job. This is intentional — actual execution is delegated to the Plan 03 tick runner. The stub is load-bearing for the API contract but produces no side effects.

## Threat Flags

All mitigations from the threat model (T-05-03 through T-05-06) are implemented:

| Threat | Status | Implementation |
|--------|--------|----------------|
| T-05-03 Spoofing — unknown action | Mitigated | Unknown actions return `{status:"error"}` JSON |
| T-05-04 Tampering — prompt injection | Mitigated | `scan_cron_prompt()` called in create and update paths |
| T-05-05 Info disclosure — exfiltration | Mitigated | curl/wget/$ENV and cat/.env patterns in RegexSet |
| T-05-06 Privilege escalation — invisible unicode | Mitigated | 10 invisible unicode chars checked before regex scan |

## Self-Check: PASSED

- `crates/ironhermes-tools/src/cronjob_tool.rs` exists and contains `impl Tool for CronjobTool`
- `crates/ironhermes-cron/src/scanner.rs` exists and contains `pub fn scan_cron_prompt`
- Commit `9d69cb6` exists (scanner)
- Commit `7c064fa` exists (tool)
- `cargo test -p ironhermes-tools cronjob` exits 0 (18 tests)
- `cargo test -p ironhermes-cron scanner` exits 0 (12 tests)
- `cargo build --workspace` exits 0
