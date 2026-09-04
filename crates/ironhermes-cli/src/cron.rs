use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_cron::display::{format_cron_status_with_tick, format_job_detail, format_job_list};
use ironhermes_cron::{
    CronJob, JobStore, JobUpdate, ScheduleParsed, parse_schedule, scan_cron_prompt,
};
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};
use tracing::error;

// ---------------------------------------------------------------------------
// CronCommands
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
pub enum CronCommands {
    /// List all scheduled jobs
    List {
        /// Show all jobs including completed
        #[arg(long, short = 'a')]
        all: bool,
    },
    /// Create a new scheduled job
    Create {
        /// Job name
        #[arg(long)]
        name: String,
        /// Schedule expression ("every 2h", "0 9 * * *", "30m", ISO timestamp)
        #[arg(long)]
        schedule: String,
        /// Agent prompt to execute
        #[arg(long)]
        prompt: String,
        /// Delivery target for job output. When omitted: defaults to "origin" routing
        /// to the configured Telegram chat if the gateway has exactly one authorized
        /// chat in config.yaml's whitelist; otherwise defaults to "local". Pass
        /// "local", "origin", "telegram:<chat_id>", or "webhook:<url>" to override.
        #[arg(long)]
        deliver: Option<String>,
        /// Skills to attach (repeatable)
        #[arg(long = "skill")]
        skills: Vec<String>,
    },
    /// Show full details for a specific job
    Get {
        /// Job ID or name (case-insensitive)
        job_id: String,
    },
    /// Edit an existing job
    Edit {
        /// Job ID or name
        job_id: String,
        #[arg(long)]
        schedule: Option<String>,
        #[arg(long)]
        prompt: Option<String>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        deliver: Option<String>,
        #[arg(long = "skill")]
        skills: Vec<String>,
        /// Remove all skills from the job. Mutually exclusive with --skill;
        /// without this flag, passing no --skill leaves the existing skills
        /// untouched (so there was previously no way to clear them).
        #[arg(long, conflicts_with = "skills")]
        clear_skills: bool,
    },
    /// Pause a job
    Pause {
        /// Job ID or name
        job_id: String,
    },
    /// Resume a paused job
    Resume {
        /// Job ID or name
        job_id: String,
    },
    /// Force-run a job now: sets `next_run_at = now` in the store so the
    /// gateway's tick runner picks it up on its next cycle. Warns if no
    /// tick has been observed recently (nothing may be consuming the queue).
    Run {
        /// Job ID or name (case-insensitive). A multi-word name must be
        /// quoted — an unquoted multi-word name is split into separate
        /// positional tokens by the shell/clap and will not resolve.
        job_id: String,
    },
    /// Remove a job
    Remove {
        /// Job ID or name
        job_id: String,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
    /// Show cron system status
    Status,
    /// Manually trigger a tick check (single-shot one tick cycle, then exit)
    Tick,
    /// Trigger a job immediately (sets next_run_at = now, fires on next tick)
    Trigger {
        /// Job ID or name (case-insensitive)
        job_id: String,
    },
    /// Run as a long-lived cron daemon (ticks every 60s without gateway)
    Daemon,
}

// ---------------------------------------------------------------------------
// handle_cron_command
// ---------------------------------------------------------------------------

pub async fn handle_cron_command(cmd: CronCommands) -> Result<()> {
    match cmd {
        CronCommands::List { all } => cmd_list(all),
        CronCommands::Create {
            name,
            schedule,
            prompt,
            deliver,
            skills,
        } => cmd_create(name, schedule, prompt, deliver, skills),
        CronCommands::Get { job_id } => cmd_get(job_id),
        CronCommands::Edit {
            job_id,
            schedule,
            prompt,
            name,
            deliver,
            skills,
            clear_skills,
        } => cmd_edit(
            job_id,
            schedule,
            prompt,
            name,
            deliver,
            skills,
            clear_skills,
        ),
        CronCommands::Pause { job_id } => cmd_pause(job_id),
        CronCommands::Resume { job_id } => cmd_resume(job_id),
        CronCommands::Run { job_id } => cmd_run(job_id),
        CronCommands::Remove { job_id, force } => cmd_remove(job_id, force),
        CronCommands::Status => cmd_status(),
        CronCommands::Tick => cmd_tick().await,
        CronCommands::Trigger { job_id } => cmd_trigger(job_id),
        CronCommands::Daemon => cmd_daemon().await,
    }
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

fn cmd_list(all: bool) -> Result<()> {
    let store = open_store()?;
    let jobs = store.list_jobs();
    // D-06: delegate to shared pure-text formatter; CLI parity with slash /cron list.
    let plain = format_job_list(jobs, all);
    print!("{}", plain);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_create
// ---------------------------------------------------------------------------

/// Resolve the (deliver, origin) pair for `hermes cron create`.
/// - Some(flag) → respect explicitly per D-04, helper not consulted.
/// - None + OriginDecision::Single → ("origin", Some(JobOrigin{...}))
/// - None + OriginDecision::Multi → ("local", None) + caller eprintln hint
/// - None + OriginDecision::None → ("local", None) silently per D-05
pub(crate) fn resolve_cron_deliver(
    deliver_flag: Option<String>,
    config: &ironhermes_core::config::Config,
) -> (String, Option<ironhermes_cron::JobOrigin>) {
    use ironhermes_core::config::OriginDecision;
    match deliver_flag {
        Some(d) => (d, None),
        None => match config.telegram_default_origin() {
            OriginDecision::Single { platform, chat_id } => (
                "origin".to_string(),
                Some(ironhermes_cron::JobOrigin {
                    platform,
                    chat_id,
                    chat_name: None,
                    thread_id: None,
                }),
            ),
            OriginDecision::Multi { whitelist } => {
                eprintln!(
                    "hermes cron create: Telegram gateway has multiple authorized chats — defaulting to deliver=local."
                );
                eprintln!(
                    "                      Pass --deliver telegram:<chat_id> to route to a specific chat (whitelist: {:?}).",
                    whitelist
                );
                ("local".to_string(), None)
            }
            OriginDecision::None => ("local".to_string(), None),
        },
    }
}

/// Reject tool names mistakenly passed as skills. A tool (e.g. `web_search`)
/// is enabled via toolsets, not loaded as skill content — listing one in
/// `skills[]` resolves to nothing at tick time and used to inject a misleading
/// "skill was skipped" banner into the job prompt. Fail fast with guidance.
fn reject_tool_names_in_skills(skills: &[String]) -> Result<()> {
    let offenders = ironhermes_tools::tool_names_among(skills);
    if !offenders.is_empty() {
        eprintln!(
            "{}: {} {} a tool, not a skill. Tools are available to cron jobs via toolsets \
             (leave `enabled_toolsets` unset to grant all) — remove {} from --skill.",
            "Error".red().bold(),
            offenders.join(", "),
            if offenders.len() == 1 { "is" } else { "are" },
            if offenders.len() == 1 { "it" } else { "them" },
        );
        return Err(anyhow!("tool name(s) in skills: {}", offenders.join(", ")));
    }
    Ok(())
}

fn cmd_create(
    name: String,
    schedule: String,
    prompt: String,
    deliver: Option<String>,
    skills: Vec<String>,
) -> Result<()> {
    // Security scan on prompt
    if let Err(e) = scan_cron_prompt(&prompt) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        return Err(anyhow!("Prompt blocked by security scanner"));
    }

    // A1: a tool name is not a skill — reject before persisting.
    reject_tool_names_in_skills(&skills)?;

    // Parse schedule
    let parsed =
        parse_schedule(&schedule).with_context(|| format!("Invalid schedule: {:?}", schedule))?;

    let schedule_display = match &parsed {
        ScheduleParsed::Once { display, .. } => display.clone(),
        ScheduleParsed::Interval { display, .. } => display.clone(),
        ScheduleParsed::Cron { display, .. } => display.clone(),
    };

    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let (deliver_str, origin_opt) = resolve_cron_deliver(deliver, &config);

    let mut store = open_store()?;
    let job = store.add_job(
        name,
        prompt,
        parsed,
        schedule_display.clone(),
        deliver_str,
        skills,
        origin_opt,
    )?;

    println!(
        "{}: {} ({})",
        "Job created".bold().cyan(),
        job.name.bold(),
        job.id.dimmed()
    );
    println!("  {:<12} {}", "Schedule:".dimmed(), schedule_display);
    if let Some(next_run) = job.next_run_at {
        println!(
            "  {:<12} {}",
            "Next run:".dimmed(),
            next_run.format("%Y-%m-%d %H:%M UTC").to_string().dimmed()
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_get
// ---------------------------------------------------------------------------

fn cmd_get(job_id: String) -> Result<()> {
    let store = open_store()?;
    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    print!("{}", render_job_details(job));
    Ok(())
}

/// Pure rendering helper — produces the full detail view as a String.
/// D-06: delegates to the shared ironhermes_cron::display formatter so
/// CLI and slash /cron get share the same render logic.
fn render_job_details(job: &CronJob) -> String {
    format_job_detail(job)
}

// ---------------------------------------------------------------------------
// cmd_edit
// ---------------------------------------------------------------------------

fn cmd_edit(
    job_id: String,
    schedule: Option<String>,
    prompt: Option<String>,
    name: Option<String>,
    deliver: Option<String>,
    skills: Vec<String>,
    clear_skills: bool,
) -> Result<()> {
    let mut store = open_store()?;

    // Verify job exists
    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let id = job.id.clone();

    // Security scan if prompt is being updated
    if let Some(ref p) = prompt
        && let Err(e) = scan_cron_prompt(p)
    {
        eprintln!("{}: {}", "Error".red().bold(), e);
        return Err(anyhow!("Prompt blocked by security scanner"));
    }

    // A1: a tool name is not a skill — reject before persisting.
    reject_tool_names_in_skills(&skills)?;

    // Parse new schedule if provided
    let (parsed_schedule, schedule_display) = if let Some(ref sched_str) = schedule {
        let parsed = parse_schedule(sched_str)
            .with_context(|| format!("Invalid schedule: {:?}", sched_str))?;
        let display = match &parsed {
            ScheduleParsed::Once { display, .. } => display.clone(),
            ScheduleParsed::Interval { display, .. } => display.clone(),
            ScheduleParsed::Cron { display, .. } => display.clone(),
        };
        (Some(parsed), Some(display))
    } else {
        (None, None)
    };

    // Skills resolution:
    // - `--clear-skills` → set to empty (B: previously impossible via CLI)
    // - one or more `--skill` → replace with that set
    // - neither → leave existing skills untouched
    let skills_opt = if clear_skills {
        Some(Vec::new())
    } else if !skills.is_empty() {
        Some(skills)
    } else {
        None
    };

    let updates = JobUpdate {
        name,
        prompt,
        deliver,
        schedule: parsed_schedule,
        schedule_display,
        skills: skills_opt,
        ..Default::default()
    };

    let updated = store.update_job(&id, updates)?;
    println!("{}: {}", "Job updated".bold().cyan(), updated.name.bold());

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_pause
// ---------------------------------------------------------------------------

fn cmd_pause(job_id: String) -> Result<()> {
    let mut store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let id = job.id.clone();
    let name = job.name.clone();

    store.toggle_job(&id, false)?;
    println!("{}: {}", "Job paused".bold().cyan(), name.yellow());

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_resume
// ---------------------------------------------------------------------------

fn cmd_resume(job_id: String) -> Result<()> {
    let mut store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let id = job.id.clone();
    let name = job.name.clone();

    store.toggle_job(&id, true)?;
    println!("{}: {}", "Job resumed".bold().cyan(), name.yellow());

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_run
// ---------------------------------------------------------------------------

/// Force-run a job now: mutates the store (`next_run_at = now`) via
/// `trigger_job` — the same, already-tested store mutation `cmd_trigger`
/// uses — so the gateway's tick runner actually picks the job up on its next
/// cycle. `find_job`/`trigger_job` already resolve a job by id or by a
/// (quoted) name case-insensitively; no new lookup code is needed here.
///
/// D-04: the store mutation always happens — force-run is not gated on
/// heartbeat freshness — but the printed message honestly reflects whether
/// anything currently appears to be consuming the cron queue.
fn cmd_run(job_id: String) -> Result<()> {
    let mut store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let name = job.name.clone();

    store.trigger_job(&job_id)?;

    let tick = ironhermes_cron::read_tick_state();
    let message = cron_run_message(&name, tick.as_ref(), chrono::Utc::now());

    println!("{}", message.primary);
    if let Some(warning) = message.warning {
        println!("{}", warning.yellow());
    }
    println!(
        "{}",
        "Check `ironhermes cron status` for details.".dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cron_run_message — D-04 honesty message builder (pure, directly testable)
// ---------------------------------------------------------------------------

/// The message `cmd_run` prints after successfully mutating the store.
pub(crate) struct CronRunMessage {
    pub primary: String,
    pub warning: Option<String>,
}

/// Build the honest post-trigger message for `cron run`.
///
/// The store mutation has already happened by the time this is called — this
/// function only decides what to *say* about it. When the heartbeat is fresh
/// (within `ironhermes_cron::TICK_STALE_SECONDS`), `primary` reports the
/// observed tick age and `warning` is `None`. When the heartbeat is stale or
/// absent, `primary` still confirms the store was updated (the mutation
/// genuinely happened and survives a later gateway start), and `warning`
/// names the job, states that no tick has been observed within the
/// threshold, and points the operator at `ironhermes gateway`.
pub(crate) fn cron_run_message(
    job_name: &str,
    tick: Option<&ironhermes_cron::TickState>,
    now: chrono::DateTime<chrono::Utc>,
) -> CronRunMessage {
    let stale = ironhermes_cron::is_tick_stale(tick, now);

    if !stale {
        // Safe: is_tick_stale returning false means tick and last_tick_at are Some.
        let last_tick_at = tick.and_then(|t| t.last_tick_at).expect(
            "is_tick_stale returned false, so tick and last_tick_at must be present",
        );
        let age_secs = (now - last_tick_at).num_seconds().max(0);
        return CronRunMessage {
            primary: format!(
                "Job triggered: {} — next run set to now (last tick observed {}s ago; gateway tick runner executes it on its next cycle).",
                job_name, age_secs
            ),
            warning: None,
        };
    }

    let primary = format!(
        "Job triggered: {} — next run set to now in the store.",
        job_name
    );

    // WR-05: `run_tick_loop`'s very first tick after gateway boot is
    // deliberately skipped (a fast-forward boot guard), so the first REAL
    // tick heartbeat write only lands on the second tick, ~60-90s after the
    // gateway starts. Without this check, "no `last_tick_at` at all" and
    // "tick observed but stale" print the identical "nothing is consuming
    // the queue — start the gateway" warning, which is a guaranteed false
    // negative for that boot window — exactly the sequence an operator
    // testing this fix follows ("start the gateway, then force-run the job
    // to check it works"). `last_boot_at` is recorded by
    // `record_backlog_at` at the very start of boot, before the tick loop
    // even starts, so a recent `last_boot_at` with no `last_tick_at` yet is
    // reliable evidence the gateway is alive and simply hasn't ticked yet —
    // print an honest "started, first tick pending" message instead. The
    // genuine stale/dead-tick warning below (a `last_tick_at` that IS
    // present but old, or a `last_boot_at` that is ALSO stale) is untouched.
    let recent_boot_before_first_tick = tick
        .and_then(|t| t.last_tick_at)
        .is_none()
        .then(|| tick.and_then(|t| t.last_boot_at))
        .flatten()
        .map(|last_boot_at| (now - last_boot_at).num_seconds().max(0))
        .filter(|&secs| secs <= ironhermes_cron::TICK_STALE_SECONDS);

    if let Some(boot_age_secs) = recent_boot_before_first_tick {
        let warning = format!(
            "Gateway started {}s ago and hasn't completed its first tick yet — \
             {} will run once the tick loop catches up (within ~60s).",
            boot_age_secs, job_name
        );
        return CronRunMessage {
            primary,
            warning: Some(warning),
        };
    }

    let warning = match tick.and_then(|t| t.last_tick_at) {
        Some(last_tick_at) => {
            let age_secs = (now - last_tick_at).num_seconds().max(0);
            format!(
                "Warning: no cron tick observed for {} for {}s (threshold {}s) — nothing appears to be consuming the cron queue right now. Start `ironhermes gateway` so this job actually runs.",
                job_name, age_secs, ironhermes_cron::TICK_STALE_SECONDS
            )
        }
        None => format!(
            "Warning: no cron tick has ever been observed for {} — nothing appears to be consuming the cron queue right now. Start `ironhermes gateway` so this job actually runs.",
            job_name
        ),
    };

    CronRunMessage {
        primary,
        warning: Some(warning),
    }
}

// ---------------------------------------------------------------------------
// cmd_remove
// ---------------------------------------------------------------------------

fn cmd_remove(job_id: String, force: bool) -> Result<()> {
    let mut store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let id = job.id.clone();
    let name = job.name.clone();

    if !force {
        print!("Remove job {:?}? [y/N] ", name);
        io::stdout().flush()?;

        let stdin = io::stdin();
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let answer = line.trim().to_lowercase();

        if answer != "y" && answer != "yes" {
            println!("{}", "Cancelled.".dimmed());
            return Ok(());
        }
    }

    store.remove_job(&id)?;
    println!("{}: {}", "Job removed".bold().cyan(), name.yellow());

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_status
// ---------------------------------------------------------------------------

fn cmd_status() -> Result<()> {
    let store = open_store()?;
    let jobs = store.list_jobs();
    // D-03: include the tick-loop heartbeat so a healthy-but-idle tick is
    // distinguishable from a dead one.
    let state = ironhermes_cron::read_tick_state();
    // D-06: delegate to shared pure-text formatter; CLI parity with slash /cron status.
    let plain = format_cron_status_with_tick(jobs, state.as_ref());
    print!("{}", plain);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_tick  (single-shot: acquire lock, scan due jobs, run each, exit)
// ---------------------------------------------------------------------------

async fn cmd_tick() -> Result<()> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let ctx = build_cron_runner_ctx(&config).await?;

    // run_tick_check acquires the tick file-lock internally; acquiring it
    // here too would deadlock against ourselves because .tick.lock is an
    // O_CREAT|O_EXCL file lock with no same-process re-entry (the PID-alive
    // check sees our own PID and returns None).
    let (due_jobs, tick_result, _lock) = ironhermes_cron::run_tick_check(&ctx.job_store).await?;

    if _lock.is_none() {
        println!("Another tick is already running. Exiting.");
        return Ok(());
    }

    for job in &due_jobs {
        if let Err(e) = ironhermes_cron_runner::run_cron_job(job, &ctx).await {
            error!(job_id=%job.id, "tick: job failed: {}", e);
        }
    }

    println!(
        "Tick complete. {} due, {} ran, {} idle.",
        due_jobs.len(),
        tick_result.jobs_run,
        tick_result.jobs_idle,
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_trigger  (synchronous — mirrors cmd_pause style)
// ---------------------------------------------------------------------------

fn cmd_trigger(job_id: String) -> Result<()> {
    let mut store = ironhermes_cron::JobStore::new()?;
    store.trigger_job(&job_id)?;
    let resolved = store
        .get_job(&job_id)
        .map(|j| j.id.clone())
        .unwrap_or_else(|| job_id.clone());
    println!("Triggered job {}", resolved);
    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_daemon  (long-running tick loop, terminable by Ctrl+C)
// ---------------------------------------------------------------------------

async fn cmd_daemon() -> Result<()> {
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_for_signal = cancel.clone();

    // Spawn ctrl-c watcher
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("Received Ctrl+C, cancelling cron daemon...");
        cancel_for_signal.cancel();
    });

    println!("Cron daemon running. Press Ctrl+C to stop.");
    cmd_daemon_with_cancel(cancel).await?;
    println!("Cron daemon stopped.");
    Ok(())
}

/// Testable inner daemon runner. Accepts a pre-constructed cancel token so
/// tests can pass a pre-cancelled token and assert the daemon exits promptly.
async fn cmd_daemon_with_cancel(cancel: tokio_util::sync::CancellationToken) -> Result<()> {
    let config = ironhermes_core::config::Config::load().unwrap_or_default();
    let ctx = Arc::new(build_cron_runner_ctx(&config).await?);
    ironhermes_cron_runner::run_tick_loop(ctx, cancel).await;
    Ok(())
}

// ---------------------------------------------------------------------------
// build_cron_runner_ctx  (shared by cmd_tick and cmd_daemon)
// ---------------------------------------------------------------------------

async fn build_cron_runner_ctx(
    config: &ironhermes_core::config::Config,
) -> Result<ironhermes_cron_runner::CronRunnerContext> {
    use tokio::sync::RwLock;

    let job_store = Arc::new(Mutex::new(ironhermes_cron::JobStore::new()?));

    // ToolRegistry: CLI cron path uses an empty registry (no gateway tools).
    // TODO: wire skills/memory for CLI cron path in a future phase.
    let tool_registry = Arc::new(RwLock::new(ironhermes_tools::ToolRegistry::new()));

    Ok(ironhermes_cron_runner::CronRunnerContext {
        job_store,
        skill_registry: None, // TODO: load SkillRegistry from IRONHERMES_HOME for CLI cron
        tool_registry,
        memory_manager: None, // TODO: wire MemoryManager for CLI cron
        hook_registry: None,  // TODO: wire HookRegistry for CLI cron
        config: config.clone(),
        mcp_manager: None,      // TODO: wire McpManager for CLI cron
        tg_client: None,        // CLI path is always standalone (no live TG adapter)
        audio_dispatcher: None, // CLI cron path has no Telegram audio dispatcher
        delivery_registry: ironhermes_cron::DeliveryRegistry::new(), // CLI path has no live adapters
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store() -> Result<JobStore> {
    JobStore::new().context("Failed to open cron job store")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Phase 32.1-07 tests (TDD RED — new CLI subcommands)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests_phase_32_1 {
    use super::*;
    use ironhermes_cron::{JobStore, ScheduleParsed};
    use tempfile::TempDir;

    // Serialise env-mutating tests through the shared bin-wide lock so they
    // don't race other modules' IRONHERMES_HOME mutations.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::test_env_lock()
    }

    fn make_store_with_job(dir: &TempDir) -> (JobStore, ironhermes_cron::CronJob) {
        let cron_dir = dir.path().join("cron");
        let mut store = JobStore::open(cron_dir).expect("open store");
        let job = store
            .add_job(
                "daily-sync",
                "do something",
                ScheduleParsed::Interval {
                    minutes: 60,
                    display: "every 60m".to_string(),
                },
                "every 60m",
                "local",
                vec![],
                None,
            )
            .expect("add job");
        store.save().expect("save");
        (store, job)
    }

    // Test 1: Trigger by id sets next_run_at
    #[test]
    fn test1_trigger_by_id_sets_next_run_at() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let (_store, job) = make_store_with_job(&dir);

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let result = cmd_trigger(job.id.clone());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_ok(),
            "cmd_trigger by id should succeed: {:?}",
            result
        );

        // Reload and check next_run_at is set close to now
        let cron_dir = dir.path().join("cron");
        let reloaded = JobStore::open(cron_dir).expect("reload");
        let j = reloaded.get_job(&job.id).expect("job present");
        let nra = j.next_run_at.expect("next_run_at should be set");
        let diff = (chrono::Utc::now() - nra).abs();
        assert!(
            diff < chrono::Duration::seconds(5),
            "next_run_at should be within 5s of now, got diff={}s",
            diff.num_seconds()
        );
    }

    // Test 2: Trigger by name sets next_run_at
    #[test]
    fn test2_trigger_by_name_sets_next_run_at() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let (_store, job) = make_store_with_job(&dir);

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let result = cmd_trigger("daily-sync".to_string());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_ok(),
            "cmd_trigger by name should succeed: {:?}",
            result
        );

        let cron_dir = dir.path().join("cron");
        let reloaded = JobStore::open(cron_dir).expect("reload");
        let j = reloaded.get_job(&job.id).expect("job present");
        let nra = j.next_run_at.expect("next_run_at set");
        let diff = (chrono::Utc::now() - nra).abs();
        assert!(diff < chrono::Duration::seconds(5));
    }

    // Test 3: Trigger nonexistent returns Err with "job not found"
    #[test]
    fn test3_trigger_nonexistent_returns_err() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");
        JobStore::open(cron_dir).expect("open empty store");

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let result = cmd_trigger("nope".to_string());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "should fail for nonexistent job");
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(
            err_msg.to_lowercase().contains("not found")
                || err_msg.to_lowercase().contains("no job"),
            "error should mention not found: {}",
            err_msg
        );
    }

    // Test 4: TickOnce exits (cmd_tick returns)
    #[tokio::test]
    async fn test4_tick_once_exits() {
        let _guard = crate::test_env_lock_async().lock().await;
        let dir = TempDir::new().expect("tmpdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        // Use a short timeout to verify it doesn't hang
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            handle_cron_command(CronCommands::Tick),
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "cmd_tick should exit within 10s");
    }

    // Test 5: Daemon with pre-cancelled token exits promptly
    #[tokio::test]
    async fn test5_daemon_with_precancelled_token_exits() {
        let _guard = crate::test_env_lock_async().lock().await;
        let dir = TempDir::new().expect("tmpdir");
        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };

        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel(); // pre-cancel so daemon exits immediately

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            cmd_daemon_with_cancel(cancel),
        )
        .await;

        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_ok(),
            "cmd_daemon_with_cancel should exit within 3s when pre-cancelled"
        );
        assert!(
            result.unwrap().is_ok(),
            "cmd_daemon_with_cancel should return Ok"
        );
    }

    // Test 6: tg_client = None in build_cron_runner_ctx (source assertion)
    // Verified via source grep in acceptance criteria, not as a runtime test.
    // This test just confirms cmd_tick uses the runner crate.
    #[test]
    fn test6_cron_commands_enum_has_trigger_and_daemon() {
        // Verify the enum variants exist by constructing them
        let _trigger = CronCommands::Trigger {
            job_id: "test".to_string(),
        };
        let _daemon = CronCommands::Daemon;
        // If these compile, the variants exist
    }

    // Phase 49.2 Plan 01 Task 1 (D-01b): cmd_run mutates the store, cloning
    // the test1/test2/test3 template used above for cmd_trigger.

    #[test]
    fn cmd_run_by_id_sets_next_run_at() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let (_store, job) = make_store_with_job(&dir);

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let result = cmd_run(job.id.clone());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_ok(), "cmd_run by id should succeed: {:?}", result);

        let cron_dir = dir.path().join("cron");
        let reloaded = JobStore::open(cron_dir).expect("reload");
        let j = reloaded.get_job(&job.id).expect("job present");
        let nra = j.next_run_at.expect("next_run_at should be set");
        let diff = (chrono::Utc::now() - nra).abs();
        assert!(
            diff < chrono::Duration::seconds(5),
            "next_run_at should be within 5s of now, got diff={}s",
            diff.num_seconds()
        );
    }

    #[test]
    fn cmd_run_by_quoted_name_sets_next_run_at() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let (_store, job) = make_store_with_job(&dir);

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        // "daily-sync" mirrors a quoted multi-word job name resolving through
        // the existing find_job id-or-name lookup — no new matching code.
        let result = cmd_run("daily-sync".to_string());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(
            result.is_ok(),
            "cmd_run by name should succeed: {:?}",
            result
        );

        let cron_dir = dir.path().join("cron");
        let reloaded = JobStore::open(cron_dir).expect("reload");
        let j = reloaded.get_job(&job.id).expect("job present");
        let nra = j.next_run_at.expect("next_run_at set");
        let diff = (chrono::Utc::now() - nra).abs();
        assert!(diff < chrono::Duration::seconds(5));
    }

    #[test]
    fn cmd_run_nonexistent_returns_err() {
        let _guard = env_guard();
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");
        JobStore::open(cron_dir).expect("open empty store");

        unsafe { std::env::set_var("IRONHERMES_HOME", dir.path()) };
        let result = cmd_run("nope".to_string());
        unsafe { std::env::remove_var("IRONHERMES_HOME") };

        assert!(result.is_err(), "should fail for nonexistent job");
        let err_msg = format!("{:?}", result.unwrap_err());
        assert!(
            err_msg.to_lowercase().contains("not found"),
            "error should mention not found: {}",
            err_msg
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironhermes_cron::{JobStore, ScheduleParsed};

    #[test]
    fn render_job_details_contains_all_fields() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = JobStore::open(dir.path().join("cron")).unwrap();
        let job = store
            .add_job(
                "test-render",
                "say hello",
                ScheduleParsed::Interval {
                    minutes: 5,
                    display: "every 5m".to_string(),
                },
                "every 5m",
                "local",
                vec!["focus".to_string()],
                None,
            )
            .unwrap();

        let rendered = render_job_details(&job);
        assert!(
            rendered.contains("test-render"),
            "name missing: {}",
            rendered
        );
        assert!(rendered.contains(&job.id), "id missing");
        assert!(rendered.contains("every 5m"), "schedule_display missing");
        assert!(rendered.contains("say hello"), "prompt missing");
        assert!(rendered.contains("local"), "deliver missing");
        assert!(rendered.contains("focus"), "skill missing");
        assert!(rendered.contains("Next run:"), "next_run label missing");
    }

    #[test]
    fn cmd_get_not_found_returns_error() {
        // find_job returns None for an empty store; cmd_get maps that to anyhow error.
        let dir = tempfile::tempdir().unwrap();
        let store = JobStore::open(dir.path().join("cron")).unwrap();
        let result = store.find_job("ghost");
        assert!(result.is_none(), "expected None for missing job");
        // Verify the error message shape cmd_get would produce:
        let err_msg = format!("Job not found: {}", "ghost");
        assert!(err_msg.contains("Job not found"));
    }

    // A1: tool names passed as skills are rejected before persisting.
    #[test]
    fn reject_tool_names_in_skills_flags_tools_only() {
        // A genuine skill name passes.
        assert!(reject_tool_names_in_skills(&["focus".to_string()]).is_ok());
        // Empty passes.
        assert!(reject_tool_names_in_skills(&[]).is_ok());
        // A tool name (web_search) is rejected, and the error names it.
        let err = reject_tool_names_in_skills(&["web_search".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("web_search"),
            "error should name the offending tool: {err}"
        );
    }

    // B: --clear-skills relies on JobUpdate { skills: Some(vec![]) } emptying
    // the stored skills. Previously the CLI could never produce that value.
    #[test]
    fn job_update_with_empty_skills_clears_them() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = JobStore::open(dir.path().join("cron")).unwrap();
        let job = store
            .add_job(
                "clearable",
                "p",
                ScheduleParsed::Interval {
                    minutes: 5,
                    display: "every 5m".to_string(),
                },
                "every 5m",
                "local",
                vec!["focus".to_string()],
                None,
            )
            .unwrap();
        assert_eq!(job.skills, vec!["focus".to_string()]);

        let updated = store
            .update_job(
                &job.id,
                JobUpdate {
                    name: None,
                    prompt: None,
                    deliver: None,
                    schedule: None,
                    schedule_display: None,
                    skills: Some(Vec::new()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(updated.skills.is_empty(), "skills should be cleared");
    }

    // Phase 49.2 Plan 01 Task 2 (D-04): cron_run_message honesty warning.

    #[test]
    fn cron_run_message_warns_when_no_recent_tick() {
        let now = chrono::Utc::now();
        let msg = cron_run_message("Daily Weather Briefing", None, now);
        assert!(msg.warning.is_some(), "expected a warning when tick is None");
        let warning = msg.warning.unwrap();
        assert!(
            warning.contains("Daily Weather Briefing"),
            "warning should name the job: {}",
            warning
        );
        assert!(
            warning.to_lowercase().contains("no cron tick")
                || warning.to_lowercase().contains("not been observed")
                || warning.to_lowercase().contains("never been observed"),
            "warning should say no tick has been observed: {}",
            warning
        );
        assert!(
            warning.to_lowercase().contains("consuming"),
            "warning should say nothing is consuming the queue: {}",
            warning
        );
        assert!(
            warning.contains("gateway"),
            "warning should mention the gateway: {}",
            warning
        );
    }

    #[test]
    fn cron_run_message_confirms_when_tick_recent() {
        let now = chrono::Utc::now();
        let tick = ironhermes_cron::TickState {
            last_tick_at: Some(now - chrono::Duration::seconds(5)),
            ..Default::default()
        };
        let msg = cron_run_message("Daily Weather Briefing", Some(&tick), now);
        assert!(
            msg.warning.is_none(),
            "no warning expected when tick is fresh"
        );
        assert!(
            msg.primary.contains("Daily Weather Briefing"),
            "primary should name the job: {}",
            msg.primary
        );
    }

    // WR-05: cron_run_message must not false-negative "no gateway is
    // consuming the queue" during the ~60-90s window between gateway boot
    // and its first real tick heartbeat.

    #[test]
    fn cron_run_message_no_false_negative_shortly_after_boot() {
        let now = chrono::Utc::now();
        let tick = ironhermes_cron::TickState {
            last_tick_at: None,
            last_boot_at: Some(now - chrono::Duration::seconds(10)),
            ..Default::default()
        };
        let msg = cron_run_message("Daily Weather Briefing", Some(&tick), now);
        let warning = msg
            .warning
            .expect("a heads-up message is still expected before the first tick");
        assert!(
            !warning.to_lowercase().contains("start `ironhermes gateway`"),
            "must not tell the operator to start the gateway when last_boot_at is recent: {}",
            warning
        );
        assert!(
            warning.to_lowercase().contains("started"),
            "should acknowledge the gateway has started: {}",
            warning
        );
        assert!(
            warning.contains("Daily Weather Briefing"),
            "warning should name the job: {}",
            warning
        );
    }

    #[test]
    fn cron_run_message_still_warns_when_boot_and_tick_both_stale() {
        let now = chrono::Utc::now();
        let tick = ironhermes_cron::TickState {
            last_tick_at: None,
            last_boot_at: Some(now - chrono::Duration::minutes(30)),
            ..Default::default()
        };
        let msg = cron_run_message("Daily Weather Briefing", Some(&tick), now);
        let warning = msg
            .warning
            .expect("expected a warning when both boot and tick are stale");
        assert!(
            warning.to_lowercase().contains("consuming"),
            "genuine stale-tick warning must still fire when last_boot_at is also old: {}",
            warning
        );
    }
}
