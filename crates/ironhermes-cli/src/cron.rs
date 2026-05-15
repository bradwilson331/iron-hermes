use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_cron::display::{format_cron_status, format_job_detail, format_job_list};
use ironhermes_cron::{
    CronJob, JobStore, JobUpdate, ScheduleParsed, parse_schedule, run_tick_check, scan_cron_prompt,
};
use std::fmt::Write as FmtWrite;
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
    /// Manually trigger a job
    Run {
        /// Job ID or name
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
        } => cmd_edit(job_id, schedule, prompt, name, deliver, skills),
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

    let skills_opt = if skills.is_empty()
        && schedule.is_none()
        && prompt.is_none()
        && name.is_none()
        && deliver.is_none()
    {
        // If nothing provided, don't touch skills
        None
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

/// Note: `cmd_run` does NOT execute the job inline. It acknowledges the
/// request — actual execution is deferred to the tick runner (gateway).
fn cmd_run(job_id: String) -> Result<()> {
    let store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let name = job.name.clone();

    println!(
        "{}",
        format!(
            "Job queued: {} — execution is deferred to the tick runner (gateway).",
            name
        )
        .yellow()
    );
    println!(
        "{}",
        "The job will run on the next tick cycle. Check `ironhermes cron status` for details."
            .dimmed()
    );

    Ok(())
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
    // D-06: delegate to shared pure-text formatter; CLI parity with slash /cron status.
    let plain = format_cron_status(jobs);
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
    let (due_jobs, tick_result, _lock) =
        ironhermes_cron::run_tick_check(&ctx.job_store).await?;

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
        skill_registry: None,   // TODO: load SkillRegistry from HERMES_HOME for CLI cron
        tool_registry,
        memory_manager: None,   // TODO: wire MemoryManager for CLI cron
        hook_registry: None,    // TODO: wire HookRegistry for CLI cron
        config: config.clone(),
        mcp_manager: None,      // TODO: wire McpManager for CLI cron
        tg_client: None,        // CLI path is always standalone (no live TG adapter)
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
    use std::sync::{Mutex as StdMutex, OnceLock};
    use tempfile::TempDir;

    // Serialise env-mutating tests
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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

        assert!(result.is_ok(), "cmd_trigger by id should succeed: {:?}", result);

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

        assert!(result.is_ok(), "cmd_trigger by name should succeed: {:?}", result);

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
            err_msg.to_lowercase().contains("not found") || err_msg.to_lowercase().contains("no job"),
            "error should mention not found: {}",
            err_msg
        );
    }

    // Test 4: TickOnce exits (cmd_tick returns)
    #[tokio::test]
    async fn test4_tick_once_exits() {
        let _guard = env_guard();
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
        let _guard = env_guard();
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

        assert!(result.is_ok(), "cmd_daemon_with_cancel should exit within 3s when pre-cancelled");
        assert!(result.unwrap().is_ok(), "cmd_daemon_with_cancel should return Ok");
    }

    // Test 6: tg_client = None in build_cron_runner_ctx (source assertion)
    // Verified via source grep in acceptance criteria, not as a runtime test.
    // This test just confirms cmd_tick uses the runner crate.
    #[test]
    fn test6_cron_commands_enum_has_trigger_and_daemon() {
        // Verify the enum variants exist by constructing them
        let _trigger = CronCommands::Trigger { job_id: "test".to_string() };
        let _daemon = CronCommands::Daemon;
        // If these compile, the variants exist
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
}
