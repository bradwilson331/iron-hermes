//! Pure-text formatters for cron job output (Phase 22.4.2.1 Plan 01, D-06).
//!
//! These functions return plain `String` without any ANSI colour codes so
//! they are safe to use in the slash-command handler (which may run inside
//! the Telegram gateway where ANSI codes are noise).
//!
//! The CLI `cron.rs` helpers delegate to these formatters and layer ANSI
//! colour on top as appropriate.

use std::fmt::Write as FmtWrite;

use crate::heartbeat::{BacklogAction, BacklogEvent, GRACE_SKIP_HISTORY_MAX, TickState};
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
        "  {:<20} {:<20} {:<12} NEXT RUN",
        "NAME", "SCHEDULE", "STATUS"
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
/// Delegates to [`format_cron_status_with_tick`] with `tick = None`. Kept as
/// a thin 1-arg wrapper so existing call sites (e.g. the tui_rata slash
/// `/cron status` handler) keep compiling unchanged.
pub fn format_cron_status(jobs: &[CronJob]) -> String {
    format_cron_status_with_tick(jobs, None)
}

/// Format aggregate cron status information as plain text, including the
/// tick-loop heartbeat (D-03).
///
/// Mirrors the body of CLI `cmd_status` with ANSI stripped.
pub fn format_cron_status_with_tick(jobs: &[CronJob], tick: Option<&TickState>) -> String {
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
        let _ = writeln!(
            out,
            "  {:<14} {} in {}",
            "Next due:", job.name, duration_str
        );
    } else {
        let _ = writeln!(out, "  {:<14} none", "Next due:");
    }

    match tick.and_then(|t| t.last_tick_at.map(|at| (t, at))) {
        Some((state, last_tick_at)) => {
            let age = now - last_tick_at;
            let age_secs = age.num_seconds().max(0);
            let age_str = if age_secs < 60 {
                format!("{}s ago", age_secs)
            } else {
                format!("{}m ago", age_secs / 60)
            };
            let _ = writeln!(
                out,
                "  {:<14} {} ({}) — checked {}, due {}, idle {}",
                "Last tick:",
                last_tick_at.format("%Y-%m-%d %H:%M UTC"),
                age_str,
                state.jobs_checked,
                state.jobs_due,
                state.jobs_idle,
            );
            if let Some(ref err) = state.last_tick_error {
                let _ = writeln!(out, "  {:<14} {}", "Tick error:", err);
            }
        }
        None => {
            let _ = writeln!(
                out,
                "  {:<14} no tick observed yet — is `ironhermes gateway` running?",
                "Last tick:"
            );
        }
    }

    // D-03: aggregate per-job visibility — the most recent run and the most
    // recent error, so the operator doesn't need `cron get <id>` to answer
    // "did anything run, and did it fail?" from `cron status` alone.
    let last_run_value = match most_recently_run_job(jobs) {
        Some(job) => {
            let last_run_str = job
                .last_run_at
                .map(|t| t.format("%Y-%m-%d %H:%M UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let status_str = job.last_status.as_deref().unwrap_or("unknown");
            format!("{} at {} ({})", job.name, last_run_str, status_str)
        }
        None => "no job has run yet".to_string(),
    };
    let _ = writeln!(out, "  {:<14} {}", LAST_RUN_LABEL, last_run_value);

    if let Some((job, message, is_delivery)) = most_recent_job_error(jobs) {
        let truncated = truncate_error(message, 120);
        let kind = if is_delivery { "delivery failed" } else { "error" };
        let _ = writeln!(
            out,
            "  {:<14} {} ({}): {}",
            LAST_ERROR_LABEL, job.name, kind, truncated
        );
    }

    // D-03/D-05 (Plan 04): the backlog view. `backlog` is the most recent
    // startup pass only (fast_forward_backlog, Plan 02); `recent_skips` is
    // the separate, accumulating runtime grace-skip history (Plan 04, this
    // plan's Task 1). Neither line is emitted when its underlying set is
    // empty, so a healthy install's status stays short.
    if let Some(tick) = tick {
        if let Some(last_boot_at) = tick.last_boot_at {
            let age = now - last_boot_at;
            let age_secs = age.num_seconds().max(0);
            let age_str = if age_secs < 60 {
                format!("{}s ago", age_secs)
            } else {
                format!("{}m ago", age_secs / 60)
            };
            let _ = writeln!(
                out,
                "  {:<14} {} ({})",
                LAST_BOOT_LABEL,
                last_boot_at.format("%Y-%m-%d %H:%M UTC"),
                age_str,
            );
        }

        let caught_up: Vec<&BacklogEvent> = tick
            .backlog
            .iter()
            .filter(|e| e.action == BacklogAction::CaughtUp)
            .collect();
        if !caught_up.is_empty() {
            let names: Vec<String> = caught_up.iter().map(|e| e.job_name.clone()).collect();
            let _ = writeln!(
                out,
                "  {:<14} {}",
                CAUGHT_UP_LABEL,
                format_count_and_names(caught_up.len(), &names)
            );
        }

        let skipped: Vec<&BacklogEvent> = tick
            .backlog
            .iter()
            .filter(|e| matches!(e.action, BacklogAction::Skipped | BacklogAction::Dropped))
            .collect();
        if !skipped.is_empty() {
            let names: Vec<String> = skipped
                .iter()
                .map(|e| match e.action {
                    BacklogAction::Dropped => format!("{} (dropped)", e.job_name),
                    _ => e.job_name.clone(),
                })
                .collect();
            let _ = writeln!(
                out,
                "  {:<14} {}",
                SKIPPED_LABEL,
                format_count_and_names(skipped.len(), &names)
            );
        }

        // Runtime grace-skip history (Task 1's get_due_jobs site) — distinct
        // from the per-boot backlog above.
        if !tick.recent_skips.is_empty() {
            let most_recent = tick
                .recent_skips
                .last()
                .expect("non-empty checked above");
            let name = format!(
                "{} missed {}",
                most_recent.job_name,
                most_recent.missed_at.format("%Y-%m-%d %H:%M UTC")
            );
            let _ = writeln!(
                out,
                "  {:<14} {} (capped at {})",
                RECENT_SKIPS_LABEL,
                format_count_and_names(tick.recent_skips.len(), &[name]),
                GRACE_SKIP_HISTORY_MAX
            );
        }
    }

    out
}

/// Render a "`<count>` (name1, name2, name3, +N more)" summary shared by the
/// `Caught up:`, `Skipped:`, and `Recent skips:` lines so their shapes cannot
/// drift apart. At most 3 of `names` are listed; any remainder (`count` minus
/// the number actually named) is folded into a trailing "+N more".
fn format_count_and_names(count: usize, names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(3).map(String::as_str).collect();
    if shown.is_empty() {
        return count.to_string();
    }
    let remainder = count.saturating_sub(shown.len());
    if remainder > 0 {
        format!("{} ({}, +{} more)", count, shown.join(", "), remainder)
    } else {
        format!("{} ({})", count, shown.join(", "))
    }
}

/// Label constants for the D-03 aggregate per-job lines. Defined once and
/// referenced (not re-literalled) by both the formatter and its tests, so
/// the label text stays a single source of truth alongside `format_job_detail`'s
/// pre-existing hardcoded `"Last run:"`/`"Last error:"` labels.
const LAST_RUN_LABEL: &str = "Last run:";
const LAST_ERROR_LABEL: &str = "Last error:";

/// Label constants for the Plan 04 backlog-view lines. Defined once,
/// referenced (not re-literalled) by both the formatter and its tests, so
/// each label appears exactly once as a string literal in this file's
/// non-test code — matching the plan's mechanical acceptance grep.
const LAST_BOOT_LABEL: &str = "Last boot:";
const CAUGHT_UP_LABEL: &str = "Caught up:";
const SKIPPED_LABEL: &str = "Skipped:";
const RECENT_SKIPS_LABEL: &str = "Recent skips:";

/// The job with the greatest `last_run_at`, if any job has run.
fn most_recently_run_job(jobs: &[CronJob]) -> Option<&CronJob> {
    jobs.iter()
        .filter(|j| j.last_run_at.is_some())
        .max_by_key(|j| j.last_run_at)
}

/// The most-recently-run job that carries a `last_error` or
/// `last_delivery_error`, along with the error text and whether it was a
/// delivery failure (as opposed to an agent-run failure) — these are kept
/// distinct because a job can fail its agent run and its delivery
/// independently, and conflating them hides one of the two.
fn most_recent_job_error(jobs: &[CronJob]) -> Option<(&CronJob, &str, bool)> {
    jobs.iter()
        .filter(|j| j.last_error.is_some() || j.last_delivery_error.is_some())
        .max_by_key(|j| j.last_run_at)
        .map(|j| {
            if let Some(ref err) = j.last_error {
                (j, err.as_str(), false)
            } else {
                (
                    j,
                    j.last_delivery_error.as_deref().unwrap_or(""),
                    true,
                )
            }
        })
}

/// Truncate an error string to at most `max_len` characters, appending an
/// ellipsis when truncated.
fn truncate_error(err: &str, max_len: usize) -> String {
    if err.chars().count() <= max_len {
        return err.to_string();
    }
    let truncated: String = err.chars().take(max_len.saturating_sub(1)).collect();
    format!("{}…", truncated)
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
            model: None,
            provider: None,
            base_url: None,
            script: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            workdir: None,
            last_delivery_error: None,
            continuity: false,
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
        assert!(
            !result.is_empty(),
            "format_cron_status must return non-empty string"
        );
    }

    #[test]
    fn format_cron_status_with_tick_renders_last_tick_line() {
        use crate::heartbeat::TickState;
        let state = TickState {
            last_tick_at: Some(Utc::now()),
            jobs_checked: 3,
            jobs_due: 1,
            jobs_idle: 2,
            ..Default::default()
        };
        let result = format_cron_status_with_tick(&[], Some(&state));
        assert!(result.contains("Last tick:"), "missing 'Last tick:' label");
        assert!(result.contains('3'), "missing checked count");
        assert!(result.contains('1'), "missing due count");
        assert!(result.contains('2'), "missing idle count");
    }

    #[test]
    fn format_cron_status_delegates_with_none() {
        let result = format_cron_status(&[]);
        assert!(!result.is_empty());
        assert!(
            result.contains("Last tick:"),
            "1-arg format_cron_status should still render the never-observed Last tick line"
        );
        assert!(
            result.contains("no tick observed"),
            "expected never-observed wording"
        );
    }

    #[test]
    fn format_cron_status_with_tick_renders_last_run_and_last_error() {
        let mut job_a = make_job("job-a");
        job_a.last_run_at = Some(Utc::now() - chrono::Duration::minutes(10));
        job_a.last_error = Some("boom".to_string());

        let mut job_b = make_job("job-b");
        job_b.last_run_at = Some(Utc::now());
        job_b.last_status = Some("ok".to_string());

        let result = format_cron_status_with_tick(&[job_a, job_b], None);
        assert!(
            result.contains(LAST_RUN_LABEL),
            "missing '{}' label",
            LAST_RUN_LABEL
        );
        assert!(
            result.contains("job-b"),
            "Last run: should name the more-recently-run job: {}",
            result
        );
        assert!(
            result.contains(LAST_ERROR_LABEL),
            "missing '{}' label",
            LAST_ERROR_LABEL
        );
        assert!(
            result.contains("job-a"),
            "Last error: should name the job carrying the error: {}",
            result
        );
        assert!(result.contains("boom"), "missing error text: {}", result);
    }

    #[test]
    fn format_cron_status_reports_tick_never_observed() {
        let result = format_cron_status_with_tick(&[], None);
        assert!(result.contains("Last tick:"), "missing 'Last tick:' label");
        assert!(
            result.contains("no tick observed"),
            "expected never-observed wording: {}",
            result
        );
        assert!(
            !result.contains(LAST_ERROR_LABEL),
            "no job carries an error; '{}' should be absent: {}",
            LAST_ERROR_LABEL,
            result
        );
    }

    #[test]
    fn format_cron_status_truncates_long_last_error() {
        let mut job = make_job("job-c");
        job.last_run_at = Some(Utc::now());
        job.last_error = Some("x".repeat(500));

        let result = format_cron_status_with_tick(&[job], None);
        let error_line = result
            .lines()
            .find(|l| l.contains(LAST_ERROR_LABEL))
            .expect("expected a Last error: line");
        assert!(
            error_line.len() <= 200,
            "Last error: line should be truncated to at most 200 chars, got {} chars: {}",
            error_line.len(),
            error_line
        );
    }

    // =========================================================================
    // Phase 49.2 Plan 04: backlog + recent_skips rendering
    // =========================================================================

    fn backlog_event(name: &str, action: BacklogAction) -> BacklogEvent {
        BacklogEvent {
            job_id: format!("id-{name}"),
            job_name: name.to_string(),
            missed_at: Utc::now() - chrono::Duration::minutes(30),
            action,
            rescheduled_to: Some(Utc::now()),
        }
    }

    #[test]
    fn format_cron_status_renders_caught_up_and_skipped() {
        let state = TickState {
            last_boot_at: Some(Utc::now() - chrono::Duration::minutes(2)),
            backlog: vec![
                backlog_event("Daily Weather Briefing", BacklogAction::CaughtUp),
                backlog_event("Job Two", BacklogAction::Skipped),
                backlog_event("Job Three", BacklogAction::Skipped),
            ],
            ..Default::default()
        };

        let result = format_cron_status_with_tick(&[], Some(&state));

        assert!(
            result.contains(LAST_BOOT_LABEL),
            "missing '{}' label: {}",
            LAST_BOOT_LABEL,
            result
        );

        let caught_up_line = result
            .lines()
            .find(|l| l.contains(CAUGHT_UP_LABEL))
            .unwrap_or_else(|| panic!("expected a {} line", CAUGHT_UP_LABEL));
        assert!(caught_up_line.contains('1'), "expected count 1: {}", caught_up_line);
        assert!(
            caught_up_line.contains("Daily Weather Briefing"),
            "expected job name: {}",
            caught_up_line
        );

        let skipped_line = result
            .lines()
            .find(|l| l.contains(SKIPPED_LABEL))
            .unwrap_or_else(|| panic!("expected a {} line", SKIPPED_LABEL));
        assert!(skipped_line.contains('2'), "expected count 2: {}", skipped_line);
    }

    #[test]
    fn format_cron_status_renders_recent_skips() {
        let state = TickState {
            recent_skips: vec![
                backlog_event("Old Skip", BacklogAction::Skipped),
                backlog_event("Middle Skip", BacklogAction::Skipped),
                backlog_event("Newest Skip", BacklogAction::Skipped),
            ],
            ..Default::default()
        };

        let result = format_cron_status_with_tick(&[], Some(&state));
        let line = result
            .lines()
            .find(|l| l.contains(RECENT_SKIPS_LABEL))
            .unwrap_or_else(|| panic!("expected a {} line", RECENT_SKIPS_LABEL));
        assert!(line.contains('3'), "expected count 3: {}", line);
        assert!(
            line.contains("Newest Skip"),
            "{} should name the most recently recorded entry: {}",
            RECENT_SKIPS_LABEL,
            line
        );
    }

    #[test]
    fn format_cron_status_empty_backlog_and_skips_emits_no_backlog_lines() {
        let state = TickState {
            last_boot_at: None,
            backlog: vec![],
            recent_skips: vec![],
            ..Default::default()
        };

        let result = format_cron_status_with_tick(&[], Some(&state));
        assert!(
            !result.contains(LAST_BOOT_LABEL),
            "unexpected '{}' line: {}",
            LAST_BOOT_LABEL,
            result
        );
        assert!(
            !result.contains(CAUGHT_UP_LABEL),
            "unexpected '{}' line: {}",
            CAUGHT_UP_LABEL,
            result
        );
        assert!(
            !result.contains(SKIPPED_LABEL),
            "unexpected '{}' line: {}",
            SKIPPED_LABEL,
            result
        );
        assert!(
            !result.contains(RECENT_SKIPS_LABEL),
            "unexpected '{}' line: {}",
            RECENT_SKIPS_LABEL,
            result
        );
    }

    #[test]
    fn format_cron_status_caps_named_jobs_at_three_with_remainder() {
        let state = TickState {
            backlog: vec![
                backlog_event("Job A", BacklogAction::CaughtUp),
                backlog_event("Job B", BacklogAction::CaughtUp),
                backlog_event("Job C", BacklogAction::CaughtUp),
                backlog_event("Job D", BacklogAction::CaughtUp),
                backlog_event("Job E", BacklogAction::CaughtUp),
            ],
            ..Default::default()
        };

        let result = format_cron_status_with_tick(&[], Some(&state));
        let line = result
            .lines()
            .find(|l| l.contains(CAUGHT_UP_LABEL))
            .unwrap_or_else(|| panic!("expected a {} line", CAUGHT_UP_LABEL));
        assert!(line.contains('5'), "expected total count 5: {}", line);
        assert!(line.contains("Job A"), "{}", line);
        assert!(line.contains("Job B"), "{}", line);
        assert!(line.contains("Job C"), "{}", line);
        assert!(!line.contains("Job D"), "should not name the 4th job: {}", line);
        assert!(!line.contains("Job E"), "should not name the 5th job: {}", line);
        assert!(
            line.contains("more"),
            "expected a remainder count for the 2 unnamed jobs: {}",
            line
        );
    }

    #[test]
    fn format_cron_status_1arg_delegation_survives_plan04() {
        let result = format_cron_status(&[]);
        assert!(!result.is_empty());
        assert!(
            result.contains("Last tick:"),
            "1-arg format_cron_status should still render the never-observed Last tick line"
        );
        assert!(
            !result.contains(LAST_BOOT_LABEL),
            "no TickState supplied — {} must not appear",
            LAST_BOOT_LABEL
        );
    }
}
