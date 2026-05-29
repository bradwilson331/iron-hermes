//! Formatters for kanban CLI output — table + JSON views.
//!
//! All formatters are pure functions; they do not open the DB. The CLI verbs
//! call these after fetching data from `KanbanStore`.

use ironhermes_kanban::dispatcher::{StrandedReport, StrandedSeverity};
use ironhermes_kanban::types::{Task, TaskComment, TaskRun};
use ironhermes_kanban::events::KanbanEvent;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Task table (for `kanban list`)
// ---------------------------------------------------------------------------

/// Render a list of tasks as a plain-text table with columns:
/// ID | STATUS | ASSIGNEE | PRIORITY | TITLE
pub fn format_task_table(tasks: &[Task]) -> String {
    if tasks.is_empty() {
        return "No tasks found.\n".to_string();
    }

    let id_w = tasks.iter().map(|t| t.id.len()).max().unwrap_or(8).max(8);
    let status_w = tasks
        .iter()
        .map(|t| t.status.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let assignee_w = tasks
        .iter()
        .map(|t| t.assignee.len())
        .max()
        .unwrap_or(8)
        .max(8);

    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "{:<id_w$}  {:<status_w$}  {:<assignee_w$}  PRI  TITLE\n",
        "ID",
        "STATUS",
        "ASSIGNEE",
        id_w = id_w,
        status_w = status_w,
        assignee_w = assignee_w,
    ));
    out.push_str(&format!(
        "{:-<id_w$}  {:-<status_w$}  {:-<assignee_w$}  ---  {:-<40}\n",
        "",
        "",
        "",
        "",
        id_w = id_w,
        status_w = status_w,
        assignee_w = assignee_w,
    ));

    for t in tasks {
        // Truncate long titles
        let title = if t.title.len() > 60 {
            format!("{}…", &t.title[..59])
        } else {
            t.title.clone()
        };
        out.push_str(&format!(
            "{:<id_w$}  {:<status_w$}  {:<assignee_w$}  {:<3}  {}\n",
            t.id,
            t.status,
            t.assignee,
            t.priority,
            title,
            id_w = id_w,
            status_w = status_w,
            assignee_w = assignee_w,
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Task JSON (for `kanban show --json`)
// ---------------------------------------------------------------------------

/// Render a full task view as a JSON value including runs, events, comments.
pub fn format_task_json(
    task: &Task,
    runs: &[TaskRun],
    events: &[KanbanEvent],
    comments: &[TaskComment],
) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "body": task.body,
        "assignee": task.assignee,
        "status": task.status,
        "priority": task.priority,
        "tenant": task.tenant,
        "workspace": task.workspace,
        "skills": task.skills,
        "idempotency_key": task.idempotency_key,
        "claim_lock": task.claim_lock,
        "claim_expires": task.claim_expires,
        "current_run_id": task.current_run_id,
        "consecutive_failures": task.consecutive_failures,
        "max_retries": task.max_retries,
        "max_runtime_seconds": task.max_runtime_seconds,
        "scheduled_at": task.scheduled_at,
        "created_by": task.created_by,
        "created_at": task.created_at,
        "started_at": task.started_at,
        "ended_at": task.ended_at,
        "runs": runs.iter().map(|r| json!({
            "id": r.id,
            "claim_lock": r.claim_lock,
            "claim_pid": r.claim_pid,
            "started_at": r.started_at,
            "ended_at": r.ended_at,
            "outcome": r.outcome,
            "summary": r.summary,
            "metadata": r.metadata,
            "error": r.error,
            "log_path": r.log_path,
        })).collect::<Vec<_>>(),
        "events": events.iter().map(|e| json!({
            "id": e.id,
            "run_id": e.run_id,
            "kind": e.kind,
            "payload": e.payload,
            "created_at": e.created_at,
        })).collect::<Vec<_>>(),
        "comments": comments.iter().map(|c| json!({
            "id": c.id,
            "author": c.author,
            "body": c.body,
            "created_at": c.created_at,
        })).collect::<Vec<_>>(),
    })
}

/// Human-readable multi-section view for `kanban show` (plain text).
pub fn format_task_detail(
    task: &Task,
    runs: &[TaskRun],
    events: &[KanbanEvent],
    comments: &[TaskComment],
) -> String {
    let mut out = String::new();

    out.push_str(&format!("=== TASK: {} ===\n", task.id));
    out.push_str(&format!("TITLE:    {}\n", task.title));
    out.push_str(&format!("STATUS:   {}\n", task.status));
    out.push_str(&format!("ASSIGNEE: {}\n", task.assignee));
    out.push_str(&format!("PRIORITY: {}\n", task.priority));
    if let Some(ref tenant) = task.tenant {
        out.push_str(&format!("TENANT:   {}\n", tenant));
    }
    if let Some(ref ws) = task.workspace {
        out.push_str(&format!("WORKSPACE: {}\n", ws));
    }
    if task.consecutive_failures > 0 {
        out.push_str(&format!("FAILURES: {}\n", task.consecutive_failures));
    }
    if let Some(ref body) = task.body {
        out.push_str("\n--- BODY ---\n");
        out.push_str(body);
        out.push('\n');
    }

    if !comments.is_empty() {
        out.push_str("\n--- COMMENTS ---\n");
        for c in comments {
            out.push_str(&format!("[{}] {}: {}\n", format_ts(c.created_at), c.author, c.body));
        }
    }

    if !runs.is_empty() {
        out.push_str("\n--- RUNS ---\n");
        out.push_str(&format_runs_table(runs));
    }

    if !events.is_empty() {
        out.push_str("\n--- EVENTS (last 10) ---\n");
        for e in events.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
            let payload_str = e.payload.as_deref().unwrap_or("");
            out.push_str(&format!(
                "[{}] {} {}\n",
                format_ts(e.created_at),
                e.kind,
                payload_str
            ));
        }
    }

    out
}

fn format_ts(ts: f64) -> String {
    // Format a Unix epoch float as a readable datetime
    let secs = ts as u64;
    let dt = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    match dt.duration_since(std::time::UNIX_EPOCH) {
        Ok(_) => {
            // Simple ISO-like format without chrono dependency
            // Use the raw seconds as fallback for now
            format!("T+{}s", secs)
        }
        Err(_) => format!("{:.0}", ts),
    }
}

// ---------------------------------------------------------------------------
// Runs table (for `kanban runs <id>`)
// ---------------------------------------------------------------------------

/// Render attempt history as a plain-text table.
pub fn format_runs_table(runs: &[TaskRun]) -> String {
    if runs.is_empty() {
        return "No runs.\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!(
        "{:<20}  {:<12}  {:<10}  SUMMARY\n",
        "RUN_ID", "OUTCOME", "DURATION"
    ));
    out.push_str(&format!("{:-<20}  {:-<12}  {:-<10}  {:-<40}\n", "", "", "", ""));

    for r in runs {
        let outcome = r.outcome.as_deref().unwrap_or("running");
        let duration = match (r.started_at, r.ended_at) {
            (s, Some(e)) => {
                let secs = (e - s) as u64;
                if secs < 60 {
                    format!("{}s", secs)
                } else {
                    format!("{}m{}s", secs / 60, secs % 60)
                }
            }
            _ => "ongoing".to_string(),
        };
        let summary = r.summary.as_deref().unwrap_or("").chars().take(40).collect::<String>();
        // Use last 20 chars of run id for brevity
        let run_id_short = if r.id.len() > 20 {
            &r.id[r.id.len() - 20..]
        } else {
            &r.id
        };
        out.push_str(&format!(
            "{:<20}  {:<12}  {:<10}  {}\n",
            run_id_short, outcome, duration, summary
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Diagnostics formatter (for `kanban diagnostics`)
// ---------------------------------------------------------------------------

/// Render stranded-task diagnostic report with severity color hints.
pub fn format_diagnostics(reports: &[StrandedReport]) -> String {
    if reports.is_empty() {
        return "No stranded tasks detected.\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("{} stranded task(s) detected:\n\n", reports.len()));

    for r in reports {
        let severity_label = match r.severity {
            StrandedSeverity::Warn => "WARN    ",
            StrandedSeverity::Error => "ERROR   ",
            StrandedSeverity::Critical => "CRITICAL",
        };
        out.push_str(&format!(
            "[{}] {} — assignee: {} — age {}s\n",
            severity_label, r.task_id, r.assignee, r.age_seconds as u64
        ));
    }

    out
}

// ---------------------------------------------------------------------------
// Stats formatter (for `kanban stats`)
// ---------------------------------------------------------------------------

/// Render status counts as plain text.
pub fn format_stats(status_counts: &[(String, usize)]) -> String {
    if status_counts.is_empty() {
        return "No tasks in board.\n".to_string();
    }

    let mut out = String::new();
    out.push_str("STATUS COUNTS:\n");
    for (status, count) in status_counts {
        out.push_str(&format!("  {:<12} {}\n", status, count));
    }
    out
}

// ---------------------------------------------------------------------------
// Assignees table (for `kanban assignees`)
// ---------------------------------------------------------------------------

pub fn format_assignees(rows: &[(String, usize)]) -> String {
    if rows.is_empty() {
        return "No assignees found.\n".to_string();
    }

    let mut out = String::new();
    out.push_str(&format!("{:<30}  COUNT\n", "ASSIGNEE"));
    out.push_str(&format!("{:-<30}  -----\n", ""));
    for (assignee, count) in rows {
        out.push_str(&format!("{:<30}  {}\n", assignee, count));
    }
    out
}

// ---------------------------------------------------------------------------
// Event list formatter (for `kanban events <id>` or `tail`)
// ---------------------------------------------------------------------------

pub fn format_event_line(event: &KanbanEvent) -> String {
    let payload_str = event.payload.as_deref().unwrap_or("");
    format!("[{}] {}\n", event.kind, payload_str)
}
