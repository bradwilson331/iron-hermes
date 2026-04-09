use anyhow::{Context, Result, anyhow};
use clap::Subcommand;
use colored::Colorize;
use ironhermes_cron::{
    parse_schedule, run_tick_check, scan_cron_prompt, JobStore, JobUpdate, ScheduleParsed,
};
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

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
        /// Delivery target (local, origin, platform:chat_id, webhook:url)
        #[arg(long, default_value = "local")]
        deliver: String,
        /// Skills to attach (repeatable)
        #[arg(long = "skill")]
        skills: Vec<String>,
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
    /// Manually trigger a tick check
    Tick,
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
    }
}

// ---------------------------------------------------------------------------
// cmd_list
// ---------------------------------------------------------------------------

fn cmd_list(all: bool) -> Result<()> {
    let store = open_store()?;
    let jobs = store.list_jobs();

    let visible: Vec<_> = jobs
        .iter()
        .filter(|j| {
            if all {
                true
            } else {
                matches!(j.state, ironhermes_cron::JobState::Scheduled | ironhermes_cron::JobState::Paused)
            }
        })
        .collect();

    println!("{}", "Scheduled Jobs".bold().cyan());
    println!("{}", "─".repeat(70));

    if visible.is_empty() {
        println!("  {}", "No scheduled jobs.".dimmed());
        println!();
        println!(
            "  {}",
            "Use `ironhermes cron create --name <name> --schedule <expr> --prompt <text>` to create one.".dimmed()
        );
        return Ok(());
    }

    println!(
        "  {:<20} {:<20} {:<12} {}",
        "NAME".bold(),
        "SCHEDULE".bold(),
        "STATUS".bold(),
        "NEXT RUN".bold()
    );

    for job in &visible {
        let status_str = match job.state {
            ironhermes_cron::JobState::Scheduled => {
                if job.enabled {
                    "scheduled".green().to_string()
                } else {
                    "disabled".yellow().to_string()
                }
            }
            ironhermes_cron::JobState::Paused => "paused".yellow().to_string(),
            ironhermes_cron::JobState::Completed => "completed".dimmed().to_string(),
        };

        let next_run_str = job
            .next_run_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "---".to_string());

        println!(
            "  {:<20} {:<20} {:<20} {}",
            job.name.yellow().to_string(),
            job.schedule_display,
            status_str,
            next_run_str.dimmed()
        );
    }

    println!("{}", "─".repeat(70));
    println!(
        "  {}",
        format!("{} job(s) total", visible.len()).dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_create
// ---------------------------------------------------------------------------

fn cmd_create(
    name: String,
    schedule: String,
    prompt: String,
    deliver: String,
    skills: Vec<String>,
) -> Result<()> {
    // Security scan on prompt
    if let Err(e) = scan_cron_prompt(&prompt) {
        eprintln!("{}: {}", "Error".red().bold(), e);
        return Err(anyhow!("Prompt blocked by security scanner"));
    }

    // Parse schedule
    let parsed = parse_schedule(&schedule)
        .with_context(|| format!("Invalid schedule: {:?}", schedule))?;

    let schedule_display = match &parsed {
        ScheduleParsed::Once { display, .. } => display.clone(),
        ScheduleParsed::Interval { display, .. } => display.clone(),
        ScheduleParsed::Cron { display, .. } => display.clone(),
    };

    let mut store = open_store()?;
    let job = store.add_job(
        name,
        prompt,
        parsed,
        schedule_display.clone(),
        deliver,
        skills,
        None,
    )?;

    println!("{}: {} ({})", "Job created".bold().cyan(), job.name.bold(), job.id.dimmed());
    println!(
        "  {:<12} {}",
        "Schedule:".dimmed(),
        schedule_display
    );
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
    if let Some(ref p) = prompt {
        if let Err(e) = scan_cron_prompt(p) {
            eprintln!("{}: {}", "Error".red().bold(), e);
            return Err(anyhow!("Prompt blocked by security scanner"));
        }
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

    let skills_opt = if skills.is_empty() && schedule.is_none() && prompt.is_none() && name.is_none() && deliver.is_none() {
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

fn cmd_run(job_id: String) -> Result<()> {
    let store = open_store()?;

    let job = store
        .find_job(&job_id)
        .ok_or_else(|| anyhow!("Job not found: {}", job_id))?;
    let name = job.name.clone();

    println!("{}", format!("Running job: {}", name).dimmed());
    println!(
        "{}",
        format!(
            "Job triggered: {} (use gateway for full agent execution)",
            name
        )
        .yellow()
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

    let total = jobs.len();
    let enabled = jobs
        .iter()
        .filter(|j| j.enabled && matches!(j.state, ironhermes_cron::JobState::Scheduled))
        .count();
    let paused = jobs
        .iter()
        .filter(|j| matches!(j.state, ironhermes_cron::JobState::Paused))
        .count();

    // Find next due job
    let now = chrono::Utc::now();
    let next_due = jobs
        .iter()
        .filter(|j| j.enabled && j.next_run_at.is_some())
        .filter_map(|j| j.next_run_at.map(|t| (j, t)))
        .filter(|(_, t)| *t >= now)
        .min_by_key(|(_, t)| *t);

    // Check tick lock status
    let cron_dir = ironhermes_core::get_hermes_home().join("cron");
    let lock_path = cron_dir.join(".tick.lock");
    let lock_status = if lock_path.exists() { "held" } else { "free" };

    // Output dir
    let output_dir = ironhermes_core::get_hermes_home().join("cron").join("output");

    println!("{}", "Cron Status".bold().cyan());
    println!("{}", "─".repeat(50));
    println!(
        "  {:<14} {} total, {} enabled, {} paused",
        "Jobs:".dimmed(),
        total,
        enabled,
        paused
    );

    if let Some((job, next_t)) = next_due {
        let diff = next_t - now;
        let mins = diff.num_minutes();
        let duration_str = if mins < 60 {
            format!("{}m", mins)
        } else if mins < 1440 {
            format!("{}h {}m", mins / 60, mins % 60)
        } else {
            format!("{}d {}h", mins / 1440, (mins % 1440) / 60)
        };
        println!(
            "  {:<14} {} in {}",
            "Next due:".dimmed(),
            job.name.yellow(),
            duration_str
        );
    } else {
        println!("  {:<14} {}", "Next due:".dimmed(), "none".dimmed());
    }

    println!("  {:<14} {}", "Tick lock:".dimmed(), lock_status);
    println!(
        "  {:<14} {}",
        "Output dir:".dimmed(),
        output_dir.display().to_string().dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// cmd_tick
// ---------------------------------------------------------------------------

async fn cmd_tick() -> Result<()> {
    let store = Arc::new(Mutex::new(open_store()?));

    let total = store.lock().unwrap().list_jobs().len();
    println!("{}", format!("Tick: checking {} jobs...", total).dimmed());

    let (due_jobs, result, _lock_guard) = run_tick_check(&store).await?;

    for job in &due_jobs {
        println!("  {}", format!("Running job: {}", job.name).yellow());

        // Complete the run with placeholder (full agent execution via gateway)
        match ironhermes_cron::complete_job_run(
            &store,
            job,
            "[CLI tick: agent execution runs via gateway]",
            true,
        )
        .await
        {
            Ok(delivery_target) => {
                let target_str = delivery_target
                    .map(|t| format!("{} ({})", t.platform, t.chat_id))
                    .unwrap_or_else(|| "local file".to_string());
                println!(
                    "  {}",
                    format!("Job complete: {} --- delivered to {}", job.name, target_str).dimmed()
                );
            }
            Err(e) => {
                eprintln!("  {}: {} — {}", "Error".red(), job.name, e);
            }
        }
    }

    println!(
        "{}",
        format!(
            "Tick complete. {} job(s) ran, {} skipped.",
            result.jobs_run, result.jobs_skipped
        )
        .dimmed()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_store() -> Result<JobStore> {
    JobStore::new().context("Failed to open cron job store")
}
