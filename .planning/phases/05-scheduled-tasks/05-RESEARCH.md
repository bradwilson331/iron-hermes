# Phase 5: Scheduled Tasks - Research

**Researched:** 2026-04-08
**Domain:** Rust cron scheduling, tool trait implementation, async tick runner, CLI subcommands
**Confidence:** HIGH

## Summary

Phase 5 extends the existing `ironhermes-cron` crate from a thin cron-only scheduler into a full scheduled-task engine matching the Python hermes-agent reference implementation. The gap between current Rust and the Python reference is substantial but well-bounded: the Rust codebase already has `CronJob`, `JobStore`, `compute_next_run()`, `acquire_tick_lock()`, and atomic JSON persistence. What is missing is (1) the `ScheduleKind` enum and `parse_schedule()` that make the struct support once/interval/cron in parallel with Python, (2) the `CronjobTool` that exposes these operations to the agent via a single action-dispatching `Tool` impl, (3) the CLI `cron` subcommand group wired into `clap`, (4) the gateway tick task spawned in `GatewayRunner`, (5) the delivery routing layer that calls `PlatformAdapter::send_message()`, and (6) the cron prompt security scanner (an extension of the Phase 3 `context_scanner.rs` in `ironhermes-core`).

The Python reference in `~/code/hermes-agent/` is comprehensive and has been fully read. Every data structure, state machine, edge case (grace windows, at-most-once semantics for recurring jobs, `[SILENT]` suppression, origin capture), and security pattern is documented with exact field names. The Rust port should mirror the Python field names and JSON shape closely so the on-disk `jobs.json` format is compatible across both agents.

The existing Rust infrastructure is ready: `cron = "0.13"` and `chrono` are already in `ironhermes-cron/Cargo.toml`, the `Tool` trait is in `ironhermes-tools::registry`, the `PlatformAdapter::send_message()` signature is stable, and `GatewayRunner` uses a `JoinSet` where the tick task can be spawned cleanly. No new external crates are required beyond `regex` (already a workspace dependency) for the cron prompt scanner.

**Primary recommendation:** Port the Python implementation field-for-field into Rust, keeping JSON field names identical so `jobs.json` files produced by either agent are interchangeable. Implement in this order: data model migration → parse_schedule() → JobStore operations → CronjobTool → security scanner → gateway tick task → CLI subcommands → delivery routing.

---

<user_constraints>
## User Constraints (from CONTEXT.md)

### Locked Decisions

- **D-01:** Rule-based schedule parser — port Python's `parse_schedule()` approach. Support duration (`30m`), intervals (`every 2h`), cron expressions (`0 9 * * *`), and ISO timestamps. No LLM dependency for schedule interpretation.
- **D-02:** Support all three schedule kinds — `once` (run at a specific time or after a delay), `interval` (recurring every X minutes/hours/days), and `cron` (standard 5-field cron expressions). The `CronJob` struct needs a `ScheduleKind` enum to replace the current plain `schedule: String` field.
- **D-03:** Single `cronjob` tool with `action` parameter — actions: create, list, get, update, pause, resume, run, remove. Matches Python's compressed tool pattern to minimize LLM context/schema bloat.
- **D-04:** CLI subcommands for cron management — `ironhermes cron [list|create|edit|pause|resume|run|remove|status|tick]` matching Python's pattern.
- **D-05:** Full field editing — update any field individually (schedule, prompt/agent_input, name, deliver, skills). Partial updates only change specified fields.
- **D-06:** Skills stored as `Vec<String>` name references now, wired to SkillRegistry in Phase 7. No validation against a skill catalog until Phase 7.
- **D-07:** Match Python delivery pattern — targets: `local`, `origin`, `platform:<chat_id>`, `webhook:<url>`. Output always saved to file (`~/.ironhermes/cron/output/{job_id}/{timestamp}.md`). Support `[SILENT]` marker.
- **D-08:** Both gateway-integrated ticking AND manual tick command. Gateway spawns a tokio task that ticks every 60s. `ironhermes cron tick` available for manual/external triggering.
- **D-09:** Port cron prompt security scanning from Python — regex-based threat detection. Extend or reuse the existing context scanner from Phase 3.

### Claude's Discretion

- Internal architecture decisions (how ScheduleKind maps to next_run computation, JobStore migration strategy from current format)
- Output file format and naming convention
- Error message wording and tool response JSON structure

### Deferred Ideas (OUT OF SCOPE)

None — discussion stayed within phase scope
</user_constraints>

<phase_requirements>
## Phase Requirements

| ID | Description | Research Support |
|----|-------------|------------------|
| SCHED-01 | User can create scheduled tasks using natural language ("every morning at 9am") which the agent interprets to cron expressions | parse_schedule() port handles "every 2h", "0 9 * * *", "30m", ISO timestamps; natural language like "every morning at 9am" maps to cron "0 9 * * *" via rule-based detection |
| SCHED-02 | User can pause, resume, and edit existing scheduled tasks without delete+recreate | CronjobTool update/pause/resume actions on JobStore; partial updates preserve unmodified fields |
| SCHED-03 | User can attach named skills to scheduled tasks for reliable, inspectable recurring jobs | Vec<String> skills field on CronJob; _build_job_prompt() pattern loads skills into prompt prefix before agent run |
| SCHED-04 | Scheduled task output routes to configured platform (Telegram, CLI, or webhook) | Delivery routing calls PlatformAdapter::send_message(); [SILENT] suppression; local file always written |
</phase_requirements>

---

## Standard Stack

### Core (all already in workspace)
| Library | Version | Purpose | Why Standard |
|---------|---------|---------|--------------|
| cron | 0.13.0 | Parse 5/6/7-field cron expressions, compute next occurrence | Already in ironhermes-cron/Cargo.toml; `Schedule::from_str` + `.after().next()` works correctly [VERIFIED: cargo tree] |
| chrono | 0.4.44 | DateTime<Utc>, timezone awareness, duration arithmetic | Already in workspace; used throughout [VERIFIED: cargo tree] |
| regex | 1.x | Cron prompt security scanner (RegexSet for threat patterns) | Already workspace dep; `context_scanner.rs` uses same pattern [VERIFIED: Cargo.toml] |
| serde/serde_json | 1.x | CronJob serialization, tool response JSON | Already in workspace [VERIFIED: Cargo.toml] |
| tokio | 1.x | Async tick task in gateway (interval + select!) | Already in workspace [VERIFIED: Cargo.toml] |
| uuid | 1.x | Job ID generation (v4) | Already in workspace [VERIFIED: Cargo.toml] |
| clap | 4.x (derive) | CLI subcommand group `ironhermes cron` | Already in workspace; main.rs uses derive pattern [VERIFIED: Cargo.toml] |

### No New Dependencies Required
All necessary crates are already workspace dependencies. The only addition needed is adding `regex` as a direct dependency to `ironhermes-cron/Cargo.toml` if the cron scanner lives there (it is already available workspace-wide but not listed as a direct dep in the cron crate's Cargo.toml). [VERIFIED: crates/ironhermes-cron/Cargo.toml]

**Version verification:** All versions confirmed via `cargo tree -p ironhermes-cron`. [VERIFIED: cargo tree]

## Architecture Patterns

### Recommended Project Structure

The `ironhermes-cron` crate grows to:
```
crates/ironhermes-cron/src/
├── lib.rs           # re-exports, pub mod declarations
├── job.rs           # CronJob struct, ScheduleKind enum, ScheduleParsed
├── store.rs         # JobStore (replaces current lib.rs store code)
├── parser.rs        # parse_schedule(), parse_duration() port from Python
├── scanner.rs       # scan_cron_prompt() — extends context_scanner patterns
└── tick.rs          # tick() function, advance_next_run(), get_due_jobs() logic

crates/ironhermes-tools/src/
└── cronjob_tool.rs  # CronjobTool: impl Tool, action dispatch, _format_job()

crates/ironhermes-cli/src/
└── cron.rs          # cron_command() handler, CronArgs clap structs
```

The alternative — keeping everything in a single `lib.rs` — is acceptable for a smaller implementation but makes the file unwieldy given the volume of logic being ported.

### Pattern 1: ScheduleKind Enum and CronJob Migration

The current `CronJob.schedule: String` holds only a cron expression. It must become a structured type. The Python `parse_schedule()` output has this exact shape:

```rust
// Source: Python cron/jobs.py parse_schedule() return value
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduleParsed {
    Once {
        run_at: DateTime<Utc>,
        display: String,
    },
    Interval {
        minutes: u32,
        display: String,
    },
    Cron {
        expr: String,
        display: String,
    },
}
```

The `CronJob` struct gains these fields to match Python:

```rust
// Source: Python cron/jobs.py create_job() job dict shape
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub prompt: String,           // renamed from agent_input (matches Python)
    pub skills: Vec<String>,      // D-06: name references, no validation
    pub schedule: ScheduleParsed, // replaces plain String
    pub schedule_display: String, // human-readable schedule label
    pub repeat: RepeatConfig,     // times: Option<u32>, completed: u32
    pub enabled: bool,
    pub state: JobState,          // "scheduled" | "paused" | "completed"
    pub paused_at: Option<DateTime<Utc>>,
    pub paused_reason: Option<String>,
    pub deliver: String,          // "local" | "origin" | "platform:chat_id" | "webhook:url"
    pub origin: Option<JobOrigin>, // platform + chat_id captured at creation
    pub created_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_status: Option<String>, // "ok" | "error"
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobOrigin {
    pub platform: String,
    pub chat_id: String,
    pub chat_name: Option<String>,
    pub thread_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobState { Scheduled, Paused, Completed }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepeatConfig {
    pub times: Option<u32>,    // None = forever
    pub completed: u32,
}
```

**Migration note:** The existing `jobs.json` stores jobs with `schedule: String` and no `skills`/`state`/`repeat` fields. `JobStore::open()` must handle both old and new formats. Use `#[serde(default)]` on new fields and a migration path (serde's untagged or a version field) to deserialize legacy entries without crashing. The simplest approach: try to deserialize as new format; on failure, attempt legacy deserialization and migrate.

### Pattern 2: parse_schedule() Port

The Python logic maps directly to Rust match arms:

```rust
// Source: Python cron/jobs.py parse_schedule()
pub fn parse_schedule(input: &str) -> Result<ScheduleParsed> {
    let s = input.trim();
    let lower = s.to_lowercase();

    // 1. "every X" → interval
    if let Some(rest) = lower.strip_prefix("every ") {
        let minutes = parse_duration(rest.trim())?;
        return Ok(ScheduleParsed::Interval {
            minutes,
            display: format!("every {}m", minutes),
        });
    }

    // 2. 5-field cron: all parts match [\d\*\-,/]+
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() >= 5 && parts[..5].iter().all(|p| {
        p.chars().all(|c| c.is_ascii_digit() || "*-,/".contains(c))
    }) {
        // validate with cron crate
        let normalised = if parts.len() == 5 {
            format!("0 {}", s)
        } else { s.to_string() };
        Schedule::from_str(&normalised)
            .with_context(|| format!("Invalid cron expression: {s:?}"))?;
        return Ok(ScheduleParsed::Cron {
            expr: s.to_string(),
            display: s.to_string(),
        });
    }

    // 3. ISO timestamp
    if s.contains('T') || s.len() >= 10 && s[..4].parse::<u32>().is_ok() {
        let dt = DateTime::parse_from_rfc3339(s)
            .or_else(|_| /* try naive ISO */ ...)
            .context("Invalid ISO timestamp")?;
        return Ok(ScheduleParsed::Once {
            run_at: dt.with_timezone(&Utc),
            display: format!("once at {}", dt.format("%Y-%m-%d %H:%M")),
        });
    }

    // 4. Plain duration "30m", "2h", "1d" → one-shot from now
    let minutes = parse_duration(s)?;
    let run_at = Utc::now() + chrono::Duration::minutes(minutes as i64);
    Ok(ScheduleParsed::Once {
        run_at,
        display: format!("once in {}", s),
    })
}
```

`parse_duration()` maps `30m`/`2h`/`1d` to minutes using the same regex as Python: `^(\d+)\s*(m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)$`.

### Pattern 3: Tool Implementation (CronjobTool)

Follows the exact `MemoryTool` pattern already in `ironhermes-tools`:

```rust
// Source: crates/ironhermes-tools/src/memory_tool.rs (established pattern)
pub struct CronjobTool {
    store: Arc<Mutex<JobStore>>,
}

#[async_trait]
impl Tool for CronjobTool {
    fn name(&self) -> &str { "cronjob" }
    fn toolset(&self) -> &str { "cronjob" }
    fn description(&self) -> &str { CRONJOB_DESCRIPTION }
    fn schema(&self) -> ToolSchema { /* see Python CRONJOB_SCHEMA */ }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let action = args["action"].as_str().unwrap_or("");
        match action {
            "create" => handle_create(&self.store, &args),
            "list"   => handle_list(&self.store, &args),
            "update" => handle_update(&self.store, &args),
            "pause"  => handle_pause(&self.store, &args),
            "resume" => handle_resume(&self.store, &args),
            "run"    => handle_run(&self.store, &args),
            "remove" => handle_remove(&self.store, &args),
            _        => Ok(json!({"success":false,"error":format!("Unknown action '{action}'")}).to_string()),
        }
    }
}
```

Registration in `ToolRegistry::register_defaults()` adds:
```rust
// New: register CronjobTool with a shared Arc<Mutex<JobStore>>
pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
    use crate::cronjob_tool::CronjobTool;
    self.register(Box::new(CronjobTool::new(store)));
}
```

This follows the `register_memory_tool()` pattern so the store can be shared between the tool and the tick task.

### Pattern 4: Gateway Tick Task

Spawned in `GatewayRunner::start()` alongside the session cleanup task (step 9 in runner.rs). The tick task uses `tokio::time::interval` and cooperates with the `CancellationToken`:

```rust
// Source: crates/ironhermes-gateway/src/runner.rs step 9 pattern
let tick_cancel = self.cancel.clone();
let job_store_tick = job_store.clone(); // Arc<Mutex<JobStore>>
let adapter_tick = adapter.clone();
join_set.spawn(async move {
    let mut interval = tokio::time::interval(
        tokio::time::Duration::from_secs(60)
    );
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = tick_cancel.cancelled() => break,
            _ = interval.tick() => {
                if let Err(e) = run_tick(&job_store_tick, &adapter_tick).await {
                    tracing::error!("Tick error: {}", e);
                }
            }
        }
    }
});
```

`MissedTickBehavior::Skip` is important: if a tick falls behind (slow job execution), subsequent ticks are dropped rather than burst-fired. [VERIFIED: tokio docs pattern]

### Pattern 5: CLI Subcommand Group

Added to `ironhermes-cli/src/main.rs` `Commands` enum using clap derive. Create `crates/ironhermes-cli/src/cron.rs` for the handler functions (mirrors Python `hermes_cli/cron.py`):

```rust
// Source: Python hermes_cli/cron.py cron_command() dispatch pattern
#[derive(Subcommand)]
pub enum CronCommands {
    List {
        #[arg(long, short = 'a')]
        all: bool,
    },
    Create {
        #[arg(long)] schedule: String,
        #[arg(long)] prompt: Option<String>,
        #[arg(long)] name: Option<String>,
        #[arg(long)] deliver: Option<String>,
        #[arg(long)] repeat: Option<u32>,
        #[arg(long = "skill")] skills: Vec<String>,
    },
    Edit {
        job_id: String,
        #[arg(long)] schedule: Option<String>,
        #[arg(long)] prompt: Option<String>,
        #[arg(long)] name: Option<String>,
        #[arg(long)] deliver: Option<String>,
        #[arg(long = "skill")] skills: Vec<String>,
    },
    Pause { job_id: String },
    Resume { job_id: String },
    Run { job_id: String },
    Remove { job_id: String },
    Status,
    Tick,
}
```

### Pattern 6: Delivery Routing

Delivery is simpler in Rust than Python because ironhermes only has Telegram (one `PlatformAdapter`). The `_resolve_delivery_target()` logic from Python maps to a `resolve_delivery_target()` function in `ironhermes-cron`:

```rust
// Source: Python cron/scheduler.py _resolve_delivery_target()
pub fn resolve_delivery_target(job: &CronJob) -> Option<DeliveryTarget> {
    match job.deliver.as_str() {
        "local" => None,
        "origin" => job.origin.as_ref().map(|o| DeliveryTarget {
            platform: o.platform.clone(),
            chat_id: o.chat_id.clone(),
            thread_id: o.thread_id.clone(),
        }),
        s if s.contains(':') => {
            // "platform:chat_id" or "webhook:url"
            let (platform, rest) = s.split_once(':').unwrap();
            Some(DeliveryTarget { platform: platform.to_string(), chat_id: rest.to_string(), thread_id: None })
        }
        _ => None,
    }
}
```

Delivery calls `adapter.send_message(chat_id, content, thread_id)`. Output is always written to `~/.ironhermes/cron/output/{job_id}/{timestamp}.md` before platform delivery. Content exceeding ~4000 chars should be truncated with a "full output saved to..." note (matching Python's `MAX_PLATFORM_OUTPUT = 4000`).

### Pattern 7: Cron Prompt Security Scanner

Extends the existing `context_scanner.rs` `THREAT_PATTERNS` RegexSet. The cron scanner is stricter — it adds patterns from Python's `_CRON_THREAT_PATTERNS`:

```rust
// Source: Python tools/cronjob_tools.py _CRON_THREAT_PATTERNS +
//         existing crates/ironhermes-core/src/context_scanner.rs
// Additional patterns beyond the Phase 3 scanner:
r"(?i)ignore\s+(?:\w+\s+)*(?:previous|all|above|prior)\s+(?:\w+\s+)*instructions",
r"(?i)do\s+not\s+tell\s+the\s+user",
r"(?i)system\s+prompt\s+override",
r"(?i)disregard\s+(your|all|any)\s+(instructions|rules|guidelines)",
r"(?i)curl\s+[^\n]*\$\{?\w*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|API)",
r"(?i)wget\s+[^\n]*\$\{?\w*(KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL|API)",
r"(?i)cat\s+[^\n]*(\.env|credentials|\.netrc|\.pgpass)",
r"(?i)authorized_keys",
r"(?i)/etc/sudoers|visudo",
r"(?i)rm\s+-rf\s+/",
```

Invisible unicode check reuses `INVISIBLE_CHARS` from `context_scanner.rs`. The scanner returns `Result<(), ScanError>` (or a `String` error) and is called in the `create` and `update` actions of `CronjobTool` before persisting the prompt. [VERIFIED: Python cronjob_tools.py + context_scanner.rs]

### Anti-Patterns to Avoid

- **Using `schedule: String` directly for all three kinds:** The current Rust struct stores only cron expressions in `schedule: String`. Keeping this and trying to detect "once" vs "interval" at runtime by re-parsing is fragile. Use the typed `ScheduleParsed` enum from the start.
- **Sharing JobStore without Arc<Mutex>:** The tick task and the CronjobTool both need to mutate JobStore. Without Arc<Mutex> wrapping, the borrow checker will reject the split-ownership. Follow the MemoryStore/MemoryTool pattern exactly.
- **Using `AtomicBool` file lock instead of O_CREAT|O_EXCL:** The existing `acquire_tick_lock_at()` uses `OpenOptions::create_new(true)` for atomicity. Do not replace this with an in-process atomic bool — it would not protect against the `ironhermes cron tick` CLI command running concurrently with the gateway.
- **Running tick synchronously in gateway thread:** The tick invokes an agent run, which can take 30–120 seconds. Running it synchronously in the 60s interval task would block the gateway. Spawn each job execution into a separate `tokio::spawn` inside `run_tick()`.
- **Advancing next_run_at after execution:** Python's `advance_next_run()` advances the timestamp BEFORE execution so crash mid-run does not cause duplicate fires on restart. This at-most-once semantic is critical for recurring jobs and must be replicated. [VERIFIED: Python cron/jobs.py advance_next_run()]
- **Ignoring grace window on restart:** `get_due_jobs()` must implement the same stale-run fast-forward logic as Python: if `now - next_run_dt > grace_seconds`, skip the run and advance to the next future occurrence. This prevents burst execution after gateway downtime. [VERIFIED: Python cron/jobs.py get_due_jobs()]

## Don't Hand-Roll

| Problem | Don't Build | Use Instead | Why |
|---------|-------------|-------------|-----|
| Cron expression parsing | Custom regex parser | `cron` crate 0.13 (already dep) | 5/6/7-field normalization, all cron syntax handled |
| Datetime arithmetic | Manual timestamp math | `chrono::Duration` + `chrono::DateTime` | DST, leap seconds, timezone-aware |
| Atomic file write | `File::create` + write | Existing `JobStore::save()` temp-rename pattern | Prevents partial-write corruption |
| File-based lock | `std::sync::Mutex` | Existing `acquire_tick_lock_at()` | Cross-process safety (CLI + gateway) |
| Platform message send | Custom HTTP | `PlatformAdapter::send_message()` | Already implemented, handles Telegram API |

**Key insight:** The hardest part of this phase is not any single algorithm — it is the state machine correctness (advance-before-execute, grace windows, one-shot auto-repeat=1, resume computing next future run). All of this is already solved in Python and must be ported faithfully, not re-invented.

## Common Pitfalls

### Pitfall 1: JobStore Deserialization After CronJob Schema Change
**What goes wrong:** Existing `jobs.json` has `schedule: "0 9 * * *"` (plain string) and no `state`/`skills`/`repeat` fields. After the schema change, `serde_json::from_str::<Vec<CronJob>>` panics or returns an error, causing all existing jobs to be lost.
**Why it happens:** Serde's default behavior on missing fields is to fail unless `#[serde(default)]` is set.
**How to avoid:** Add `#[serde(default)]` to all new fields. Write a `LegacyCronJob` struct for the old format and implement a one-time migration in `JobStore::open()`: try new format first, fall back to legacy, migrate and save.
**Warning signs:** `JobStore::open()` returning an error for a non-empty `jobs.json` during integration testing.

### Pitfall 2: Origin Capture Requires Gateway Context
**What goes wrong:** `CronjobTool` is called from the agent during a Telegram conversation. The `origin` (platform + chat_id) must be injected from the gateway's message context — but the tool has no direct access to the current session's chat_id.
**Why it happens:** Tools receive only `serde_json::Value` args; they have no session context.
**How to avoid:** Follow the Python pattern: inject `HERMES_SESSION_PLATFORM` and `HERMES_SESSION_CHAT_ID` as environment variables (or equivalent context) before the agent run, then read them in `CronjobTool::execute()` when action is `create`. The gateway sets these env vars before dispatching (or passes them via a context struct). An alternative is to pass origin as explicit args in the tool schema.
**Warning signs:** All jobs created via Telegram having `deliver: "local"` instead of `deliver: "origin"`.

### Pitfall 3: Tick Task Blocking the Gateway
**What goes wrong:** `run_tick()` is called synchronously inside the 60-second interval task. An agent run takes 45 seconds, so the tick slot is consumed and subsequent ticks pile up.
**Why it happens:** `tokio::time::interval` with `MissedTickBehavior::Delay` (the default) will fire immediately after a long tick.
**How to avoid:** Use `MissedTickBehavior::Skip`. Spawn each job execution into its own `tokio::spawn` so the interval loop stays responsive. Add a check: if a tick lock cannot be acquired, log and skip (the previous tick is still running).
**Warning signs:** Multiple jobs running concurrently when only one was due; gateway becoming unresponsive after a long cron job.

### Pitfall 4: SILENT Marker Detection
**What goes wrong:** Agent returns `"[SILENT] I have nothing to report."` — both the marker and extra text. The detection code checks `starts_with("[SILENT]")` but the content is still delivered because the case doesn't match or there's leading whitespace.
**Why it happens:** The Python reference checks `deliver_content.strip().upper().startswith(SILENT_MARKER)`. Rust port might forget the `.trim()` or case normalization.
**How to avoid:** Normalize: `response.trim().to_uppercase().starts_with("[SILENT]")`. Always save output to file regardless of `[SILENT]` — suppress only the platform delivery.
**Warning signs:** Telegram receiving `[SILENT]` messages; or silent jobs whose output was not saved.

### Pitfall 5: Delivering to Telegram Before Saving Output
**What goes wrong:** Delivery to Telegram succeeds but `save_job_output()` panics. The run appears successful to the user but no local audit trail exists.
**Why it happens:** Delivery happens before file save.
**How to avoid:** Always write the output file FIRST, then attempt platform delivery. Delivery failure should be logged but should not prevent `mark_job_run()` from being called.
**Warning signs:** Missing files in `~/.ironhermes/cron/output/`.

### Pitfall 6: repeat=1 Not Auto-Set for Once-Kind Jobs
**What goes wrong:** A `once` job (e.g., "run in 30m") runs once, then `compute_next_run()` returns `None`, but the job is not marked `completed` and keeps attempting to run.
**Why it happens:** The `repeat.times = None` (infinite) was the default and one-shot detection was missed.
**How to avoid:** In `create_job()`, auto-set `repeat.times = Some(1)` when `ScheduleParsed::Once` is detected and repeat was not explicitly provided. This mirrors Python's auto-set logic: `if parsed_schedule["kind"] == "once" and repeat is None: repeat = 1`.
**Warning signs:** One-shot jobs appearing in `cron list` indefinitely after their run time.

## Code Examples

Verified patterns from existing codebase:

### Tool Registration Pattern (from MemoryTool)
```rust
// Source: crates/ironhermes-tools/src/registry.rs
pub fn register_cronjob_tool(&mut self, store: Arc<Mutex<JobStore>>) {
    use crate::cronjob_tool::CronjobTool;
    self.register(Box::new(CronjobTool::new(store)));
}
```

### Tick Lock Acquisition (existing, reuse as-is)
```rust
// Source: crates/ironhermes-cron/src/lib.rs acquire_tick_lock_at()
// Uses OpenOptions::create_new(true) — atomic, cross-process safe
match acquire_tick_lock() {
    Ok(Some(_guard)) => { /* run tick, guard drops lock on exit */ }
    Ok(None) => { tracing::debug!("Tick skipped — lock held"); return Ok(()); }
    Err(e) => { tracing::error!("Lock error: {}", e); return Err(e); }
}
```

### Gateway Interval Task (from session cleanup step 9)
```rust
// Source: crates/ironhermes-gateway/src/runner.rs (step 9 pattern)
let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
loop {
    tokio::select! {
        _ = cancel.cancelled() => break,
        _ = interval.tick() => { /* spawn job executions */ }
    }
}
```

### JSON Response Pattern (from CronjobTool, mirrors Python)
```rust
// Source: Python tools/cronjob_tools.py _format_job()
fn format_job_response(job: &CronJob) -> serde_json::Value {
    json!({
        "job_id": job.id,
        "name": job.name,
        "skills": job.skills,
        "schedule": job.schedule_display,
        "repeat": format_repeat(&job.repeat),
        "deliver": job.deliver,
        "next_run_at": job.next_run_at.map(|t| t.to_rfc3339()),
        "last_run_at": job.last_run_at.map(|t| t.to_rfc3339()),
        "last_status": job.last_status,
        "enabled": job.enabled,
        "state": format!("{:?}", job.state).to_lowercase(),
        "paused_at": job.paused_at.map(|t| t.to_rfc3339()),
        "paused_reason": job.paused_reason,
    })
}
```

### compute_next_run for ScheduleKind (port from Python)
```rust
// Source: Python cron/jobs.py compute_next_run()
pub fn compute_next_run(schedule: &ScheduleParsed, last_run_at: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match schedule {
        ScheduleParsed::Once { run_at, .. } => {
            if last_run_at.is_some() { return None; } // already ran
            let grace = Utc::now() - chrono::Duration::seconds(120);
            if *run_at >= grace { Some(*run_at) } else { None }
        }
        ScheduleParsed::Interval { minutes, .. } => {
            let base = last_run_at.unwrap_or_else(Utc::now);
            Some(base + chrono::Duration::minutes(*minutes as i64))
        }
        ScheduleParsed::Cron { expr, .. } => {
            let normalised = if expr.split_whitespace().count() == 5 {
                format!("0 {}", expr)
            } else { expr.clone() };
            let sched = Schedule::from_str(&normalised).ok()?;
            sched.after(&Utc::now()).next()
        }
    }
}
```

## State of the Art

| Old Approach | Current Approach | When Changed | Impact |
|--------------|------------------|--------------|--------|
| `schedule: String` (cron-only) | `ScheduleParsed` enum (once/interval/cron) | Phase 5 | Enables natural language scheduling |
| No tool — agent has no cron access | `CronjobTool` registered in `ToolRegistry` | Phase 5 | Agent can create/manage jobs in conversation |
| No CLI cron commands | `ironhermes cron [subcommands]` | Phase 5 | Operator can manage without chatting |
| No tick runner | Gateway tick task + `cron tick` CLI | Phase 5 | Jobs actually fire |
| No skills field | `Vec<String>` name references | Phase 5 (wired Phase 7) | Enables skill-driven recurring jobs |
| No delivery routing | Local + platform delivery | Phase 5 | Output reaches Telegram/CLI |

**Deprecated/outdated:**
- `CronJob.agent_input: String` field name: Python uses `prompt`, Rust should align to avoid JSON shape divergence on the disk format.
- `CronJob.schedule: String` (current): being replaced by `schedule: ScheduleParsed` + `schedule_display: String`.
- `JobStore.save()` serializing `Vec<CronJob>` directly as array: Python wraps in `{"jobs": [...], "updated_at": "..."}`. Decision: keep Rust's flat array format for now (simpler) OR align with Python. This is a Claude's Discretion item — the plan should pick one and document it.

## Assumptions Log

| # | Claim | Section | Risk if Wrong |
|---|-------|---------|---------------|
| A1 | `ironhermes-cron` will remain a separate crate and not be merged into `ironhermes-tools` | Architecture Patterns | If merged, the crate boundary changes affect import paths in gateway and CLI |
| A2 | Origin injection (for `deliver: "origin"`) will use an environment variable or a context struct passed through GatewayMessageHandler — exact mechanism is Claude's Discretion | Pattern 3/Common Pitfalls | If gateway doesn't provide origin context, all Telegram-created jobs default to "local" delivery |
| A3 | `jobs.json` format will remain file-based (no SQLite migration) | Standard Stack | If SQLite is chosen for jobs storage, the entire JobStore implementation changes |
| A4 | Job execution during cron tick runs the agent loop inline (not as a subprocess) | Architecture Patterns | Rust has no equivalent of Python's `AIAgent` subprocess isolation — if job execution needs isolation, this is out of scope for Phase 5 |

**If this table is empty:** Not empty — four assumptions requiring planner awareness.

## Open Questions

1. **Origin injection mechanism**
   - What we know: Python uses `os.environ["HERMES_SESSION_PLATFORM"]` and `HERMES_SESSION_CHAT_ID`; these are set by the gateway before agent dispatch.
   - What's unclear: The Rust gateway dispatches via `GatewayMessageHandler::handle_with_multimodal()`. The tool has no access to the session's `chat_id` unless it is injected somehow. Three options: (a) env vars (Python pattern), (b) thread-local storage, (c) explicit `origin` parameter in the tool schema that the agent fills from system context.
   - Recommendation: Use option (c) — add `origin_platform` and `origin_chat_id` as optional tool parameters populated by the agent from its session context. This avoids env var mutation in an async context, which is unsafe when multiple agents run concurrently.

2. **jobs.json format: flat array vs Python's `{"jobs": [...]}` wrapper**
   - What we know: Current Rust saves `Vec<CronJob>` as a bare JSON array. Python wraps in `{"jobs": [...], "updated_at": "..."}`.
   - What's unclear: Should these be interchangeable?
   - Recommendation: Align with Python's wrapper format now, since the planner may want file compatibility between the two agents. Migration: detect bare array on load, wrap on save.

3. **Job execution: how does the cron tick invoke the agent?**
   - What we know: Python calls `AIAgent(...).run_conversation(prompt)`. Rust has `AgentLoop::run()` in `ironhermes-agent`.
   - What's unclear: `AgentLoop` requires a `LlmClient` which requires API key resolution. The tick task needs access to the same client config as the gateway.
   - Recommendation: Pass a `LlmClient` or `Config` to the tick runner at construction time, the same way `GatewayRunner` holds `Config`. The tick creates a fresh `AgentLoop` per job execution.

## Environment Availability

| Dependency | Required By | Available | Version | Fallback |
|------------|------------|-----------|---------|----------|
| Rust / cargo | Build | Yes | detected | — |
| `cron` crate 0.13 | parse_schedule cron branch | Yes | 0.13.0 | — |
| `chrono` 0.4 | DateTime arithmetic | Yes | 0.4.44 | — |
| `regex` 1.x | Cron prompt scanner | Yes (workspace) | 1.x | — |
| `tokio` 1.x | Async tick task | Yes | 1.x | — |
| `clap` 4.x | CLI subcommands | Yes | 4.x | — |

**Missing dependencies with no fallback:** None.

**Note:** `regex` is in `[workspace.dependencies]` but not yet in `ironhermes-cron/Cargo.toml`. It must be added there if `scanner.rs` lives in the cron crate. Alternatively, the scanner can live in `ironhermes-core` (where `regex` is already a direct dep via `context_scanner.rs`). [VERIFIED: Cargo.toml]

## Validation Architecture

### Test Framework
| Property | Value |
|----------|-------|
| Framework | Rust built-in `#[test]` + `#[tokio::test]` |
| Config file | None (Cargo test runner) |
| Quick run command | `cargo test --package ironhermes-cron` |
| Full suite command | `cargo test --workspace` |

### Phase Requirements → Test Map
| Req ID | Behavior | Test Type | Automated Command | File Exists? |
|--------|----------|-----------|-------------------|-------------|
| SCHED-01 | `parse_schedule("every 2h")` returns Interval{minutes:120} | unit | `cargo test --package ironhermes-cron test_parse_schedule` | Wave 0 |
| SCHED-01 | `parse_schedule("0 9 * * *")` returns Cron{expr} | unit | `cargo test --package ironhermes-cron test_parse_schedule_cron` | Wave 0 |
| SCHED-01 | `parse_schedule("30m")` returns Once{run_at} | unit | `cargo test --package ironhermes-cron test_parse_schedule_once` | Wave 0 |
| SCHED-01 | `parse_schedule("every morning at 9am")` fails with clear error | unit | `cargo test --package ironhermes-cron test_parse_schedule_invalid` | Wave 0 |
| SCHED-02 | `update_job()` with only `schedule` changes schedule, preserves prompt | unit | `cargo test --package ironhermes-cron test_update_job_partial` | Wave 0 |
| SCHED-02 | `pause_job()` sets state=paused, enabled=false, paused_at | unit | `cargo test --package ironhermes-cron test_pause_job` | Wave 0 |
| SCHED-02 | `resume_job()` sets state=scheduled, enabled=true, next_run_at in future | unit | `cargo test --package ironhermes-cron test_resume_job` | Wave 0 |
| SCHED-03 | CronJob created with skills=["morning-brief"] stores name reference | unit | `cargo test --package ironhermes-cron test_skills_stored` | Wave 0 |
| SCHED-04 | `[SILENT]` response suppresses Telegram delivery, still writes file | unit | `cargo test --package ironhermes-cron test_silent_marker` | Wave 0 |
| SCHED-04 | `deliver: "origin"` with origin present routes to origin platform | unit | `cargo test --package ironhermes-cron test_delivery_routing` | Wave 0 |
| SCHED-04 | `deliver: "local"` writes file, returns no delivery target | unit | `cargo test --package ironhermes-cron test_delivery_local` | Wave 0 |
| D-09 | Prompt with "ignore previous instructions" blocked by scanner | unit | `cargo test --package ironhermes-cron test_cron_scanner_blocks_injection` | Wave 0 |
| D-09 | Invisible unicode in prompt blocked by scanner | unit | `cargo test --package ironhermes-cron test_cron_scanner_invisible_unicode` | Wave 0 |
| D-01 | `get_due_jobs()` fast-forwards stale recurring job past grace window | unit | `cargo test --package ironhermes-cron test_stale_job_fast_forward` | Wave 0 |

### Sampling Rate
- **Per task commit:** `cargo test --package ironhermes-cron`
- **Per wave merge:** `cargo test --workspace`
- **Phase gate:** Full suite green before `/gsd-verify-work`

### Wave 0 Gaps
All test functions listed above need to be created. The existing 9 tests in `lib.rs` cover:
- `test_compute_next_run_5field` ✅
- `test_compute_next_run_6field` ✅
- `test_invalid_schedule` ✅ (cron-only; parse_schedule() tests are new)
- `test_job_store_roundtrip` ✅
- `test_remove_job` ✅
- `test_toggle_job` ✅
- `test_get_due_jobs` ✅
- `test_mark_job_run` ✅
- `test_tick_lock` ✅

New tests needed in Wave 0:
- [ ] `tests/test_parser.rs` — covers SCHED-01 (parse_schedule variants)
- [ ] `tests/test_store.rs` — covers SCHED-02 (update/pause/resume partial)
- [ ] `tests/test_scanner.rs` — covers D-09 (cron prompt scanner)
- [ ] `tests/test_delivery.rs` — covers SCHED-04 (delivery routing, SILENT marker)
- [ ] `tests/test_job_state.rs` — covers SCHED-03 (skills stored), stale fast-forward

## Security Domain

### Applicable ASVS Categories

| ASVS Category | Applies | Standard Control |
|---------------|---------|-----------------|
| V2 Authentication | no | n/a — cron runs in-process with same identity |
| V3 Session Management | no | cron sessions are ephemeral, no user session |
| V4 Access Control | yes | cron tool excluded from cron-session ToolRegistry (prevent recursive scheduling, mirrors Python `disabled_toolsets=["cronjob"]`) |
| V5 Input Validation | yes | `parse_schedule()` validates all formats with error on invalid; cron prompt scanner blocks injection |
| V6 Cryptography | no | no new crypto; file permissions 0600 on output files (matches Python `_secure_file()`) |

### Known Threat Patterns for Cron/Scheduled Tasks

| Pattern | STRIDE | Standard Mitigation |
|---------|--------|---------------------|
| Prompt injection via cron prompt field | Tampering | `scan_cron_prompt()` — blocks injection patterns, invisible unicode; called on create and update |
| Recursive cron scheduling (cron job creates more cron jobs) | Elevation of Privilege | Exclude `cronjob` from ToolRegistry in cron-session agent runs (D-03 note; Python uses `disabled_toolsets=["cronjob"]`) |
| Exfiltration via cron prompt (curl $API_KEY) | Information Disclosure | `scan_cron_prompt()` exfil patterns: `exfil_curl`, `exfil_wget`, `read_secrets` |
| Stale gateway → burst job execution on restart | Denial of Service | Grace window + stale fast-forward in `get_due_jobs()` |
| Concurrent tick execution | Denial of Service | File-based tick lock (`acquire_tick_lock()`) — cross-process safe |
| Output file world-readable | Information Disclosure | `0o600` permissions on output files + `0o700` on output directories |

### Security Note on Origin Injection
Cron job execution must NOT inherit the gateway's active session environment variables. Python cleans up all `HERMES_SESSION_*` and `HERMES_CRON_*` env vars in a `finally` block. The Rust tick runner must similarly scope any context injection to the single job run and clean up afterward — or use an isolated struct rather than env vars.

## Sources

### Primary (HIGH confidence)
- `~/code/hermes-agent/cron/jobs.py` — Complete parse_schedule(), CronJob schema, all CRUD operations, grace window logic [VERIFIED: read in full]
- `~/code/hermes-agent/cron/scheduler.py` — tick(), run_job(), _deliver_result(), SILENT_MARKER, advance_next_run() [VERIFIED: read in full]
- `~/code/hermes-agent/tools/cronjob_tools.py` — CRONJOB_SCHEMA, action dispatch, _scan_cron_prompt(), _origin_from_env() [VERIFIED: read in full]
- `~/code/hermes-agent/hermes_cli/cron.py` — CLI subcommand dispatch, cron_create/edit/pause/resume/run/remove [VERIFIED: read in full]
- `~/code/hermes-agent/gateway/delivery.py` — DeliveryTarget, DeliveryRouter, platform routing logic [VERIFIED: read in full]
- `crates/ironhermes-cron/src/lib.rs` — Existing CronJob, JobStore, compute_next_run(), tick lock [VERIFIED: read in full]
- `crates/ironhermes-tools/src/registry.rs` — Tool trait, ToolRegistry, register pattern [VERIFIED: read in full]
- `crates/ironhermes-tools/src/memory_tool.rs` — Canonical Tool impl pattern to follow [VERIFIED: read in full]
- `crates/ironhermes-core/src/context_scanner.rs` — THREAT_PATTERNS, INVISIBLE_CHARS, scan pattern [VERIFIED: read in full]
- `crates/ironhermes-gateway/src/runner.rs` — JoinSet, tick task spawn point, CancellationToken pattern [VERIFIED: read in full]
- `crates/ironhermes-gateway/src/adapter.rs` — PlatformAdapter::send_message() signature [VERIFIED: read in full]
- `crates/ironhermes-cli/src/main.rs` — Clap derive pattern, Commands enum, build_registry() [VERIFIED: read in full]
- `crates/ironhermes-core/src/config.rs` — Config struct, CronConfig (wrap_response) [VERIFIED: read in full]
- `Cargo.toml` (workspace) — All dependency versions [VERIFIED: read in full]
- `crates/ironhermes-cron/Cargo.toml` — Cron crate direct deps [VERIFIED: read in full]
- `cargo test --package ironhermes-cron` — 9 existing tests pass [VERIFIED: executed]
- `cargo tree -p ironhermes-cron` — cron 0.13.0, chrono 0.4.44 confirmed [VERIFIED: executed]

### Secondary (MEDIUM confidence)
- `~/code/hermes-agent/gateway/delivery.py` — Python delivery.py is a fuller implementation with multi-platform support than IronHermes will need in Phase 5; Rust port only needs Telegram + local + webhook [VERIFIED: read in full, scope confirmed]

### Tertiary (LOW confidence)
- None

## Metadata

**Confidence breakdown:**
- Standard stack: HIGH — all versions verified via cargo tree; no new deps needed
- Architecture: HIGH — Python reference read in full; Rust patterns verified from existing code
- Pitfalls: HIGH — all derived from direct reading of Python source and existing Rust codebase; none are assumed

**Research date:** 2026-04-08
**Valid until:** 2026-05-08 (stable Rust ecosystem; Python reference unlikely to change)
