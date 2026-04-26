//! Pure-text formatters for cron job output (Phase 22.4.2.1 Plan 01, D-06).
//!
//! These functions return plain `String` without any ANSI colour codes so
//! they are safe to use in the slash-command handler (which may run inside
//! the Telegram gateway where ANSI codes are noise).
//!
//! The CLI `cron.rs` helpers delegate to these formatters and layer ANSI
//! colour on top as appropriate.

use std::fmt::Write as FmtWrite;

use crate::job::{CronJob, JobState};

// ---------------------------------------------------------------------------
// format_job_list
// ---------------------------------------------------------------------------

/// Format a slice of cron jobs as a plain-text table.
///
/// When `all` is false only Scheduled and Paused jobs are shown (matching
/// the CLI `cron list` default filter). Returns a multi-line `String`.
pub fn format_job_list(jobs: &[CronJob], all: bool) -> String {
    let visible: Vec<&CronJob> = jobs
        .iter()
        .filter(|j| {
            if all {
                true
            } else {
                matches!(j.state, JobState::Scheduled | JobState::Paused)
            }
        })
        .collect();

    let mut out = String::new();

    let _ = writeln!(out, "Scheduled Jobs");
    let _ = writeln!(out, "{}", "-".repeat(70));

    if visible.is_empty() {
        let _ = writeln!(out, "  No scheduled jobs.");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  Use `ironhermes cron create --name <name> --schedule <expr> --prompt <text>` to create one."
        );
        return out;
    }

    let _ = writeln!(
        out,
        "  {:<20} {:<20} {:<12} {}",
        "NAME", "SCHEDULE", "STATUS", "NEXT RUN"
    );

    for job in &visible {
        let status_str = match job.state {
            JobState::Scheduled => {
                if job.enabled {
                    "scheduled".to_string()
                } else {
                    "disabled".to_string()
                }
            }
            JobState::Paused => "paused".to_string(),
            JobState::Completed => "completed".to_string(),
        };

        let next_run_str = job
            .next_run_at
            .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "---".to_string());

        let _ = writeln!(
            out,
            "  {:<20} {:<20} {:<12} {}",
            job.name, job.schedule_display, status_str, next_run_str
        );
    }

    let _ = writeln!(out, "{}", "-".repeat(70));
    let _ = writeln!(out, "  {} job(s) total", visible.len());

    out
}

// ---------------------------------------------------------------------------
// format_job_detail
// ---------------------------------------------------------------------------

/// Format a single cron job as a plain-text detail view.
///
/// Returns a multi-line `String` with all job fields labelled.
pub fn format_job_detail(job: &CronJob) -> String {
    let mut out = String::new();

    let _ = writeln!(out, "Cron Job");
    let _ = writeln!(out, "{}", "-".repeat(50));

    // Core identity
    let _ = writeln!(out, "  {:<14} {}", "Name:", job.name);
    let _ = writeln!(out, "  {:<14} {}", "ID:", job.id);

    // Schedule
    let _ = writeln!(out, "  {:<14} {}", "Schedule:", job.schedule_display);

    // Prompt (may be multi-line; rendered as-is)
    let _ = writeln!(out, "  {:<14} {}", "Prompt:", job.prompt);

    // Delivery target
    let _ = writeln!(out, "  {:<14} {}", "Deliver:", job.deliver);

    // Skills
    let skills_str = if job.skills.is_empty() {
        "none".to_string()
    } else {
        job.skills.join(", ")
    };
    let _ = writeln!(out, "  {:<14} {}", "Skills:", skills_str);

    // State
    let state_str = match job.state {
        JobState::Scheduled => {
            if job.enabled {
                "scheduled".to_string()
            } else {
                "disabled".to_string()
            }
        }
        JobState::Paused => "paused".to_string(),
        JobState::Completed => "completed".to_string(),
    };
    let _ = writeln!(out, "  {:<14} {}", "State:", state_str);
    let _ = writeln!(out, "  {:<14} {}", "Enabled:", job.enabled);

    // Timestamps
    let created_str = job.created_at.format("%Y-%m-%d %H:%M UTC").to_string();
    let _ = writeln!(out, "  {:<14} {}", "Created:", created_str);

    let next_run_str = job
        .next_run_at
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "never".to_string());
    let _ = writeln!(out, "  {:<14} {}", "Next run:", next_run_str);

    let last_run_str = job
        .last_run_at
        .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| "never".to_string());
    let _ = writeln!(out, "  {:<14} {}", "Last run:", last_run_str);

    // Optional status/error tail
    if let Some(ref status) = job.last_status {
        let _ = writeln!(out, "  {:<14} {}", "Last status:", status);
    }
    if let Some(ref err) = job.last_error {
        let _ = writeln!(out, "  {:<14} {}", "Last error:", err);
    }

    out
}

// ---------------------------------------------------------------------------
// format_cron_status
// ---------------------------------------------------------------------------

/// Format aggregate cron status information as plain text.
///
/// Mirrors the body of CLI `cmd_status` with ANSI stripped.
pub fn format_cron_status(jobs: &[CronJob]) -> String {
    let mut out = String::new();

    let total = jobs.len();
    let enabled = jobs
        .iter()
        .filter(|j| j.enabled && matches!(j.state, JobState::Scheduled))
        .count();
    let paused = jobs
        .iter()
        .filter(|j| matches!(j.state, JobState::Paused))
        .count();

    // Find next due job
    let now = chrono::Utc::now();
    let next_due = jobs
        .iter()
        .filter(|j| j.enabled && j.next_run_at.is_some())
        .filter_map(|j| j.next_run_at.map(|t| (j, t)))
        .filter(|(_, t)| *t >= now)
        .min_by_key(|(_, t)| *t);

    let _ = writeln!(out, "Cron Status");
    let _ = writeln!(out, "{}", "-".repeat(50));
    let _ = writeln!(
        out,
        "  {:<14} {} total, {} enabled, {} paused",
        "Jobs:", total, enabled, paused
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
        let _ = writeln!(out, "  {:<14} {} in {}", "Next due:", job.name, duration_str);
    } else {
        let _ = writeln!(out, "  {:<14} none", "Next due:");
    }

    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_job(name: &str) -> CronJob {
        use crate::job::{RepeatConfig, ScheduleParsed};
        CronJob {
            id: format!("id-{}", name),
            name: name.to_string(),
            prompt: format!("prompt for {}", name),
            skills: vec![],
            schedule: ScheduleParsed::Interval {
                minutes: 60,
                display: "every 1h".to_string(),
            },
            schedule_display: "every 1h".to_string(),
            repeat: RepeatConfig::default(),
            enabled: true,
            state: JobState::Scheduled,
            paused_at: None,
            paused_reason: None,
            deliver: "local".to_string(),
            origin: None,
            created_at: Utc::now(),
            next_run_at: None,
            last_run_at: None,
            last_status: None,
            last_error: None,
        }
    }

    #[test]
    fn format_job_list_empty_returns_no_scheduled_jobs() {
        let result = format_job_list(&[], false);
        assert!(
            result.contains("No scheduled jobs."),
            "expected 'No scheduled jobs.' in output; got: {}",
            result
        );
    }

    #[test]
    fn format_job_list_one_job_contains_name() {
        let job = make_job("foo");
        let result = format_job_list(&[job], false);
        assert!(
            result.contains("foo"),
            "expected 'foo' in output; got: {}",
            result
        );
    }

    #[test]
    fn format_job_detail_contains_required_fields() {
        let job = make_job("bar");
        let result = format_job_detail(&job);
        assert!(result.contains("Name:"), "missing 'Name:' label");
        assert!(result.contains("ID:"), "missing 'ID:' label");
        assert!(result.contains("Schedule:"), "missing 'Schedule:' label");
        assert!(result.contains("Deliver:"), "missing 'Deliver:' label");
    }

    #[test]
    fn format_cron_status_compiles_and_returns_string() {
        let result = format_cron_status(&[]);
        assert!(!result.is_empty(), "format_cron_status must return non-empty string");
    }
}
