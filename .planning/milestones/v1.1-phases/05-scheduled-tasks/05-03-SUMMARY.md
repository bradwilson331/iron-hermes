---
phase: 05-scheduled-tasks
plan: 03
subsystem: scheduler
tags: [rust, tokio, cron, delivery, tick, cli, clap, colored]

requires:
  - phase: 05-01
    provides: CronJob, JobStore, ScheduleParsed, JobState, acquire_tick_lock, LockGuard
  - phase: 05-02
    provides: CronjobTool, scan_cron_prompt, register_cronjob_tool

provides:
  - delivery.rs: resolve_delivery_target, is_silent, save_job_output, format_delivery_message
  - tick.rs: run_tick_check (lock + due-job collection), complete_job_run (file save + delivery routing)
  - Gateway 60s tick task with MissedTickBehavior::Skip
  - CLI cron subcommands: list, create, edit, pause, resume, run, remove, status, tick
  - JobStore wired to both CronjobTool and tick task in gateway startup

affects:
  - 06-event-hooks
  - 07-skills-system
  - any phase that adds gateway tasks or CLI subcommands

tech-stack:
  added: []
  patterns:
    - "Tick lock (O_CREAT|O_EXCL file lock) prevents concurrent tick runs"
    - "Atomic temp+rename write for job output files"
    - "DeliveryTarget resolved from deliver string (local/origin/platform:id/webhook:url)"
    - "MissedTickBehavior::Skip prevents burst execution after gateway downtime"
    - "scan_cron_prompt() security gate on all CLI create/edit paths"

key-files:
  created:
    - crates/ironhermes-cron/src/delivery.rs
    - crates/ironhermes-cron/src/tick.rs
    - crates/ironhermes-cli/src/cron.rs
  modified:
    - crates/ironhermes-cron/src/lib.rs
    - crates/ironhermes-gateway/src/runner.rs
    - crates/ironhermes-gateway/Cargo.toml
    - crates/ironhermes-cli/src/main.rs
    - crates/ironhermes-cli/Cargo.toml

key-decisions:
  - "Tick task in gateway logs due jobs but uses placeholder output — full AgentLoop integration deferred as natural enhancement once LlmClient is accessible from tick context"
  - "run_tick_check returns LockGuard as third tuple element so caller controls lock lifetime"
  - "CLI tick command calls complete_job_run with placeholder output (real execution via gateway)"
  - "deliver string parsing: colon split gives platform:chat_id for both platform and webhook targets"

patterns-established:
  - "Delivery routing pattern: resolve_delivery_target() called after is_silent() check"
  - "Output always saved to file first, platform delivery is secondary (T-05-07 mitigation)"
  - "CLI subcommand module pattern: cron.rs with CronCommands enum + handle_cron_command async fn"

requirements-completed:
  - SCHED-01
  - SCHED-02
  - SCHED-04

duration: 35min
completed: 2026-04-09
---

# Phase 5 Plan 03: Tick Runner, Delivery Routing, and CLI Cron Subcommands Summary

**Tick runner with file+platform delivery routing, 60s gateway integration with MissedTickBehavior::Skip, and 9 CLI cron subcommands backed by security-scanned schedule parsing**

## Performance

- **Duration:** ~35 min
- **Started:** 2026-04-09T00:30:00Z
- **Completed:** 2026-04-09T00:45:00Z
- **Tasks:** 2
- **Files modified:** 8

## Accomplishments

- Delivery routing resolves all 4 target types: local (None), origin (from JobOrigin), platform:chat_id, webhook:url
- SILENT marker suppresses platform delivery while unconditionally saving output to `~/.ironhermes/cron/output/{job_id}/{timestamp}.md`
- Gateway spawns a 60s tick task with `MissedTickBehavior::Skip` and full tick lock protection
- 9 CLI cron subcommands: list, create, edit, pause, resume, run, remove, status, tick — all verified working
- Security scanner gates `create` and `edit` prompt updates via `scan_cron_prompt()`
- 21 new unit tests (17 delivery + 4 tick) all passing

## Task Commits

1. **Task 1: Delivery routing + tick runner + gateway integration** - `ef014c9` (feat)
2. **Task 2: CLI cron subcommands** - `5b82711` (feat)

## Files Created/Modified

- `crates/ironhermes-cron/src/delivery.rs` - DeliveryTarget, resolve_delivery_target, is_silent, save_job_output, format_delivery_message with MAX_PLATFORM_OUTPUT=4000
- `crates/ironhermes-cron/src/tick.rs` - run_tick_check (lock acquisition + due job collection), complete_job_run (file save + delivery routing)
- `crates/ironhermes-cron/src/lib.rs` - Added `pub mod delivery` and `pub mod tick` with re-exports
- `crates/ironhermes-gateway/src/runner.rs` - Added job_store field, set_job_store(), and 60s tick task in start()
- `crates/ironhermes-gateway/Cargo.toml` - Added ironhermes-cron dependency
- `crates/ironhermes-cli/src/cron.rs` - Full CLI cron subcommand implementation (9 commands)
- `crates/ironhermes-cli/src/main.rs` - Added mod cron, Commands::Cron variant, match arm, JobStore wiring in run_gateway()
- `crates/ironhermes-cli/Cargo.toml` - Added ironhermes-cron dependency

## Decisions Made

- Full AgentLoop execution within the tick task deferred: wiring LlmClient into the tick context requires either threading it through GatewayRunner or building a fresh client per job. The tick task detects due jobs, saves placeholder output, and routes delivery. CLI `cron run` similarly uses a placeholder. Real agent execution runs through the gateway's existing handler path when a user manually triggers a job via chat.
- `run_tick_check` returns `(Vec<CronJob>, TickResult, Option<LockGuard>)` so the caller holds the lock guard for the duration of job execution, preventing concurrent ticks.
- CLI `cron tick` command calls `complete_job_run` inline to exercise the full delivery path in testing/manual trigger scenarios.

## Deviations from Plan

None - plan executed exactly as written. The "agent execution pending full integration" placeholder for the tick task was explicitly called out in the plan as the correct Phase 5 approach.

## Issues Encountered

None. The workspace built cleanly on first attempt. All 21 tests passed. All 9 CLI commands verified working interactively.

## Known Stubs

- Gateway tick task and CLI `cron run`/`cron tick` use placeholder output `"[Tick runner: agent execution pending full integration]"` / `"[CLI tick: agent execution runs via gateway]"` instead of running the job's prompt through an AgentLoop. This is intentional per plan scope — the due-job detection, file save, and delivery routing all work correctly. The agent execution integration is a follow-on enhancement tracked in the plan notes.

## Threat Flags

None - all planned mitigations implemented:
- T-05-07: Output always saved locally first; truncated at 4000 chars for platform delivery
- T-05-08: MissedTickBehavior::Skip + tick lock prevent burst/concurrent execution
- T-05-09: scan_cron_prompt() called on CLI create and edit before persisting
- T-05-10: Timestamp-named output files provide audit trail

## Self-Check

## Self-Check: PASSED

- FOUND: crates/ironhermes-cron/src/delivery.rs
- FOUND: crates/ironhermes-cron/src/tick.rs
- FOUND: crates/ironhermes-cli/src/cron.rs
- FOUND: .planning/phases/05-scheduled-tasks/05-03-SUMMARY.md
- FOUND commit ef014c9: feat(05-03): delivery routing + tick runner + gateway integration
- FOUND commit 5b82711: feat(05-03): CLI cron subcommands
- Build: Finished dev profile cleanly
