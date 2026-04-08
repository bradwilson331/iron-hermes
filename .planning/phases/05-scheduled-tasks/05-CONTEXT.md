# Phase 5: Scheduled Tasks - Context

**Gathered:** 2026-04-08
**Status:** Ready for planning

<domain>
## Phase Boundary

Extend the existing `ironhermes-cron` crate so users can create, manage, and run scheduled tasks with a rule-based schedule parser, a single `cronjob` agent tool, CLI subcommands, and multi-platform output delivery. Skills are stored as string references for future wiring in Phase 7.

</domain>

<decisions>
## Implementation Decisions

### Schedule Parsing
- **D-01:** Rule-based schedule parser — port Python's `parse_schedule()` approach. Support duration (`30m`), intervals (`every 2h`), cron expressions (`0 9 * * *`), and ISO timestamps. No LLM dependency for schedule interpretation.
- **D-02:** Support all three schedule kinds — `once` (run at a specific time or after a delay), `interval` (recurring every X minutes/hours/days), and `cron` (standard 5-field cron expressions). The `CronJob` struct needs a `ScheduleKind` enum to replace the current plain `schedule: String` field.

### Agent Tool API
- **D-03:** Single `cronjob` tool with `action` parameter — actions: create, list, get, update, pause, resume, run, remove. Matches Python's compressed tool pattern to minimize LLM context/schema bloat.

### CLI Interface
- **D-04:** CLI subcommands for cron management — `ironhermes cron [list|create|edit|pause|resume|run|remove|status|tick]` matching Python's pattern. Allows managing jobs without chatting with the agent.

### Task Editing
- **D-05:** Full field editing — update any field individually (schedule, prompt/agent_input, name, deliver, skills). Partial updates only change specified fields. Satisfies SCHED-02 (edit without delete+recreate).

### Skill Attachment
- **D-06:** Skills stored as `Vec<String>` name references now, wired to SkillRegistry in Phase 7. The cronjob tool accepts `skills` parameter. No validation against a skill catalog until Phase 7 delivers SkillRegistry.

### Output Delivery
- **D-07:** Match Python delivery pattern — targets: `local` (file output), `origin` (platform+chat_id captured at job creation), `platform:<chat_id>` (explicit target), `webhook:<url>`. Output is always saved to file (`~/.ironhermes/cron/output/{job_id}/{timestamp}.md`). Support `[SILENT]` marker to suppress platform delivery when agent has nothing interesting to report.

### Tick Runner
- **D-08:** Both gateway-integrated ticking AND manual tick command. Gateway spawns a tokio task that ticks every 60s checking for due jobs. `ironhermes cron tick` also available for manual/external triggering (systemd timer, system cron). File-based tick lock (already implemented) prevents overlapping execution.

### Cron Security
- **D-09:** Port cron prompt security scanning from Python — regex-based threat detection for prompt injection, exfiltration patterns, and invisible unicode characters. Cron prompts run in fresh sessions with full tool access, making them a higher-risk surface. Extend or reuse the existing context scanner from Phase 3.

### Claude's Discretion
- Internal architecture decisions (how ScheduleKind maps to next_run computation, JobStore migration strategy from current format)
- Output file format and naming convention
- Error message wording and tool response JSON structure

</decisions>

<canonical_refs>
## Canonical References

**Downstream agents MUST read these before planning or implementing.**

### Python Reference Implementation
- `~/code/hermes-agent/cron/jobs.py` — Job storage, `parse_schedule()`, schedule kinds (once/interval/cron)
- `~/code/hermes-agent/cron/scheduler.py` — Tick execution, delivery target resolution, `[SILENT]` marker
- `~/code/hermes-agent/tools/cronjob_tools.py` — Single `cronjob()` tool with action parameter, cron prompt scanning
- `~/code/hermes-agent/hermes_cli/cron.py` — CLI subcommands (list/create/edit/pause/resume/run/remove/status/tick)
- `~/code/hermes-agent/gateway/delivery.py` — Delivery routing for cron outputs to platforms

### Existing Rust Codebase
- `crates/ironhermes-cron/src/lib.rs` — Current CronJob struct, JobStore, compute_next_run(), tick lock
- `crates/ironhermes-core/src/config.rs` — CronConfig (currently only `wrap_response`)
- `crates/ironhermes-tools/src/registry.rs` — Tool trait and ToolRegistry for registering the cronjob tool
- `crates/ironhermes-gateway/src/adapter.rs` — PlatformAdapter::send_message() for delivery
- `crates/ironhermes-gateway/src/runner.rs` — GatewayRunner where tick task would be spawned
- `crates/ironhermes-cli/src/main.rs` — CLI entry point where cron subcommands would be added

</canonical_refs>

<code_context>
## Existing Code Insights

### Reusable Assets
- `CronJob` struct with JSON persistence, atomic saves, and enable/disable toggle
- `compute_next_run()` with 5/6-field cron normalization (uses `cron` crate)
- `acquire_tick_lock()` / `LockGuard` for exclusive tick execution
- `JobStore` with add/remove/get/list/toggle/mark_job_run operations
- Context file security scanner in `ironhermes-core` (Phase 3) — extensible for cron prompt scanning
- `PlatformAdapter::send_message()` in gateway — ready for delivery routing

### Established Patterns
- Tool trait in `ironhermes-tools` — implement `Tool` for `CronjobTool`, register in `register_defaults()`
- Atomic file I/O via temp-file-and-rename (already used in JobStore and memory subsystem)
- `clap` derive for CLI subcommands in `ironhermes-cli`
- Config sections with `Default` impl in `ironhermes-core`

### Integration Points
- `ironhermes-cron` currently only depends on `ironhermes-core` — will need to stay isolated or gain minimal new deps
- Gateway runner needs a spawned tokio task for periodic ticking
- CLI binary needs a new `cron` subcommand group added to clap
- `ToolRegistry::register_defaults()` needs the new `CronjobTool` added

</code_context>

<specifics>
## Specific Ideas

- Match the Python hermes-agent's UX patterns closely — single compressed tool, CLI subcommands, same delivery model
- The `parse_schedule()` port should handle the same input formats as Python (duration strings, "every X" pattern, cron expressions, ISO timestamps)
- Origin tracking: capture platform + chat_id at job creation time so `deliver: "origin"` works for jobs created via Telegram

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope

</deferred>

---

*Phase: 05-scheduled-tasks*
*Context gathered: 2026-04-08*
