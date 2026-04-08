# Phase 5: Scheduled Tasks - Discussion Log

> **Audit trail only.** Do not use as input to planning, research, or execution agents.
> Decisions are captured in CONTEXT.md — this log preserves the alternatives considered.

**Date:** 2026-04-08
**Phase:** 05-scheduled-tasks
**Areas discussed:** NL Parsing, Task Management, Skill Attachment, Output Delivery

---

## NL Parsing

| Option | Description | Selected |
|--------|-------------|----------|
| Rule-based parser (Recommended) | Port Python's parse_schedule() approach: support 'every 30m', 'every 2h', cron expressions, and ISO timestamps. Fast, deterministic, testable. | ✓ |
| LLM-assisted parsing | Send NL to LLM to interpret as cron expression. More flexible but adds latency and non-determinism. | |
| Hybrid | Try rule-based first; fall back to LLM call. Best of both but more complexity. | |

**User's choice:** Rule-based parser
**Notes:** None

### Follow-up: Schedule Kinds

| Option | Description | Selected |
|--------|-------------|----------|
| All three (Recommended) | once + interval + cron — matches Python parity | ✓ |
| Cron + interval only | Skip one-shot schedules. Simplify with 'repeat: 1' for one-shots. | |
| Cron only | Keep existing model. Convert intervals to cron at parse time. | |

**User's choice:** All three
**Notes:** None

---

## Task Management

### Agent Tool API

| Option | Description | Selected |
|--------|-------------|----------|
| Single tool (Recommended) | One 'cronjob' tool with action parameter — matches Python pattern. Low context usage. | ✓ |
| Multiple tools | Separate tools: schedule_task, list_tasks, edit_task, etc. Clearer but more schema. | |
| You decide | Claude's discretion on tool API design. | |

**User's choice:** Single tool
**Notes:** None

### CLI Subcommands

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, CLI subcommands (Recommended) | 'ironhermes cron [list|create|edit|...]' matching Python. | ✓ |
| Agent tool only | Users manage jobs through conversation only. | |
| You decide | Claude's discretion. | |

**User's choice:** Yes, CLI subcommands
**Notes:** None

### Edit Scope

| Option | Description | Selected |
|--------|-------------|----------|
| Full field editing (Recommended) | Update any field individually. Partial updates. | ✓ |
| Replace whole job | Edit replaces entire job definition. | |
| You decide | Claude's discretion on edit granularity. | |

**User's choice:** Full field editing
**Notes:** None

---

## Skill Attachment

| Option | Description | Selected |
|--------|-------------|----------|
| String field now, wire later (Recommended) | Add 'skills: Vec<String>' to CronJob. Validation deferred to Phase 7. | ✓ |
| Skip entirely | No skill field until Phase 7. | |
| You decide | Claude's discretion on forward dependency. | |

**User's choice:** String field now, wire later
**Notes:** None

---

## Output Delivery

### Delivery Routing

| Option | Description | Selected |
|--------|-------------|----------|
| Match Python pattern (Recommended) | local/origin/platform:<chat_id>/webhook:<url>. Store origin at creation. [SILENT] marker support. | ✓ |
| Simplified local + telegram | Only local file output and Telegram delivery. | |
| You decide | Claude's discretion on delivery architecture. | |

**User's choice:** Match Python pattern
**Notes:** None

### Tick Runner

| Option | Description | Selected |
|--------|-------------|----------|
| Gateway-integrated (Recommended) | Gateway spawns tokio task ticking every 60s. Single process. | |
| Separate tick command | 'ironhermes cron tick' triggered by system cron. More Unix-y. | |
| Both | Gateway ticks automatically AND manual tick command exists. | ✓ |

**User's choice:** Both
**Notes:** None

### Cron Security

| Option | Description | Selected |
|--------|-------------|----------|
| Yes, port it (Recommended) | Port regex-based threat scanner for cron prompts. | ✓ |
| Reuse existing scanner | Apply Phase 3 context scanner to cron prompts without cron-specific patterns. | |
| You decide | Claude's discretion on cron prompt security. | |

**User's choice:** Yes, port it
**Notes:** None

---

## Claude's Discretion

- Internal architecture (ScheduleKind mapping, JobStore migration)
- Output file format and naming
- Error message wording and tool response structure

## Deferred Ideas

None — discussion stayed within phase scope
