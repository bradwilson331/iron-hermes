//! Phase 54.10 Plan 05 (D-05/D-07/D-10/D-11): the data every native Dashboard tile reads.
//!
//! Everything here is read-only (D-05): a tile never calls an agent tool or starts a turn. The
//! Usage, Sessions and Reports tiles get new server fns; the Kanban, Cron and Gateway tiles fold
//! the result of server fns that already exist, with the pure functions below. DTOs and folds
//! are plain serde/std code outside the `feature = "server"` helpers so wasm and native share
//! them and the logic is natively testable.

use crate::server::dashboard_api::UsageWindow;
use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// One model row of the Usage tile.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UsageRowView {
    pub provider: String,
    pub model: String,
    pub cost_micros: i64,
    pub tokens: i64,
}

/// The Usage tile: totals over a window, the top 5 models and the failed-call count.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct UsageTileView {
    pub window: UsageWindow,
    pub total_cost_micros: i64,
    pub total_tokens: i64,
    pub rows: Vec<UsageRowView>,
    pub failed_calls: i64,
    /// False when every cost is 0: the rows are then ranked by tokens (UI-SPEC cost fallback).
    pub cost_recorded: bool,
}

/// Seconds covered by a Usage window.
#[cfg_attr(not(feature = "server"), allow(dead_code))] // only the server fns call it
pub fn window_seconds(w: UsageWindow) -> i64 {
    match w {
        UsageWindow::Day => 86_400,
        UsageWindow::Week => 7 * 86_400,
        UsageWindow::Month => 30 * 86_400,
    }
}

/// Fold `(provider, model, cost_micros, in+out tokens)` rollups into the tile. Integer math
/// only; totals cover every row, the list is the top 5 by cost (by tokens when no cost exists).
#[cfg_attr(not(feature = "server"), allow(dead_code))] // only the server fns call it
pub fn fold_usage(
    rows: Vec<(String, String, i64, i64)>,
    failed_calls: i64,
    window: UsageWindow,
) -> UsageTileView {
    let total_cost_micros: i64 = rows.iter().map(|r| r.2).sum();
    let total_tokens: i64 = rows.iter().map(|r| r.3).sum();
    let cost_recorded = total_cost_micros > 0;
    let mut rows: Vec<UsageRowView> = rows
        .into_iter()
        .map(|(provider, model, cost_micros, tokens)| UsageRowView {
            provider,
            model,
            cost_micros,
            tokens,
        })
        .collect();
    if cost_recorded {
        rows.sort_by_key(|r| std::cmp::Reverse(r.cost_micros));
    } else {
        rows.sort_by_key(|r| std::cmp::Reverse(r.tokens));
    }
    rows.truncate(5);
    UsageTileView {
        window,
        total_cost_micros,
        total_tokens,
        rows,
        failed_calls,
        cost_recorded,
    }
}

/// Usage over a window from the server runtime's state.db (operator resolution 2).
#[server]
pub async fn dashboard_usage(window: UsageWindow) -> Result<UsageTileView, ServerFnError> {
    let store = crate::server::state::global_app_state().state_store.clone();
    tokio::task::spawn_blocking(move || {
        let store = store
            .lock()
            .map_err(|_| ServerFnError::new("State store mutex poisoned"))?;
        let secs = window_seconds(window);
        let filter = ironhermes_state::UsageFilter {
            since_seconds: Some(secs),
            ..Default::default()
        };
        let rollups = store
            .query_usage_events(&filter)
            .map_err(|e| ServerFnError::new(format!("Usage query failed: {e}")))?;
        let failed = store
            .count_usage_errors(Some(secs))
            .map_err(|e| ServerFnError::new(format!("Usage query failed: {e}")))?;
        let rows = rollups
            .into_iter()
            .map(|r| (r.provider, r.model, r.cost_usd_micros, r.in_tok + r.out_tok))
            .collect();
        Ok(fold_usage(rows, failed, window))
    })
    .await
    .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

/// One recent web session.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DashSessionRow {
    pub id: String,
    pub title: Option<String>,
    pub started_at: f64,
    /// `ended_at IS NULL` (RESEARCH A5).
    pub active: bool,
}

/// Fold `(id, title, started_at, ended_at, message_count)` into rows, newest first as given.
/// Sessions with no messages are dropped (parity with the Sessions screen) and at most `limit`
/// rows are kept.
#[cfg_attr(not(feature = "server"), allow(dead_code))] // only the server fns call it
pub fn fold_sessions(
    sessions: impl IntoIterator<Item = (String, Option<String>, f64, Option<f64>, i64)>,
    limit: usize,
) -> Vec<DashSessionRow> {
    sessions
        .into_iter()
        .filter(|s| s.4 > 0)
        .take(limit)
        .map(|(id, title, started_at, ended_at, _)| DashSessionRow {
            id,
            title,
            started_at,
            active: ended_at.is_none(),
        })
        .collect()
}

/// Recent web sessions of the server runtime's state.db (operator resolution 2).
#[server]
pub async fn dashboard_sessions(limit: usize) -> Result<Vec<DashSessionRow>, ServerFnError> {
    let limit = limit.clamp(1, 20);
    let store = crate::server::state::global_app_state().state_store.clone();
    tokio::task::spawn_blocking(move || {
        let web = ironhermes_core::types::Platform::Web.to_string();
        let sessions = store
            .lock()
            .map_err(|_| ServerFnError::new("State store mutex poisoned"))?
            .list_sessions(Some(&web), 100)
            .map_err(|e| ServerFnError::new(format!("StateStore list sessions failed: {e}")))?;
        Ok(fold_sessions(
            sessions
                .into_iter()
                .map(|s| (s.id, s.title, s.started_at, s.ended_at, s.message_count)),
            limit,
        ))
    })
    .await
    .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
}

// ---------------------------------------------------------------------------
// Kanban, Cron, Gateway: pure folds over server fns the app already has
// ---------------------------------------------------------------------------

use crate::protocol::TaskRow;
use crate::server::dashboard_api::KanbanColumn;
use crate::server::gateway_platform_status_api::PlatformStatusMap;
use crate::server::schedules_api::ScheduleRow;

/// The Kanban tile: counts for the selected columns and the blocked tasks needing attention.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct KanbanTileView {
    /// Tasks in the selected columns that are not `done` (archived tasks are never fetched).
    pub open_total: usize,
    pub columns: Vec<(KanbanColumn, usize)>,
    /// Titles of every `blocked` task (operator resolution 5: there is no `failed` state).
    pub blocked: Vec<String>,
}

fn column_status(c: KanbanColumn) -> &'static str {
    match c {
        KanbanColumn::Triage => "triage",
        KanbanColumn::Todo => "todo",
        KanbanColumn::Ready => "ready",
        KanbanColumn::Running => "running",
        KanbanColumn::Blocked => "blocked",
        KanbanColumn::Done => "done",
    }
}

/// Count tasks per selected column; empty `columns` means all six.
pub fn kanban_summary(rows: &[TaskRow], columns: &[KanbanColumn]) -> KanbanTileView {
    use KanbanColumn::*;
    let all = [Triage, Todo, Ready, Running, Blocked, Done];
    let selected: &[KanbanColumn] = if columns.is_empty() { &all } else { columns };
    let columns: Vec<(KanbanColumn, usize)> = selected
        .iter()
        .map(|c| {
            let n = rows
                .iter()
                .filter(|r| r.status == column_status(*c))
                .count();
            (*c, n)
        })
        .collect();
    let open_total = columns
        .iter()
        .filter(|(c, _)| *c != Done)
        .map(|(_, n)| n)
        .sum();
    let blocked = rows
        .iter()
        .filter(|r| r.status == "blocked")
        .map(|r| r.title.clone())
        .collect();
    KanbanTileView {
        open_total,
        columns,
        blocked,
    }
}

/// The most recent run across all jobs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CronLastRun {
    pub name: String,
    pub at_raw: String,
    pub at_display: Option<String>,
    pub failed: Option<bool>,
    /// Only set when `failed == Some(true)`.
    pub error: Option<String>,
}

/// The Cron tile: the next three enabled runs and the latest run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CronTileView {
    /// `(job name, next_run_at_raw)`, soonest first.
    pub next: Vec<(String, String)>,
    pub last: Option<CronLastRun>,
}

fn parse_rfc3339(raw: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    chrono::DateTime::parse_from_rfc3339(raw).ok()
}

/// Fold schedule rows. RFC3339 strings are compared as instants, never lexically.
pub fn cron_summary(rows: &[ScheduleRow]) -> CronTileView {
    let mut next: Vec<_> = rows
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| {
            let raw = r.next_run_at_raw.as_ref()?;
            Some((parse_rfc3339(raw)?, r.name.clone(), raw.clone()))
        })
        .collect();
    next.sort_by_key(|(at, _, _)| *at);
    let next = next
        .into_iter()
        .take(3)
        .map(|(_, n, raw)| (n, raw))
        .collect();
    let last = rows
        .iter()
        .filter_map(|r| Some((parse_rfc3339(r.last_run_at_raw.as_ref()?)?, r)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, r)| CronLastRun {
            name: r.name.clone(),
            at_raw: r.last_run_at_raw.clone().unwrap_or_default(),
            at_display: r.last_run_at.clone(),
            failed: r.last_run_failed,
            error: if r.last_run_failed == Some(true) {
                r.last_error.clone()
            } else {
                None
            },
        });
    CronTileView { next, last }
}

/// One gateway platform: CONNECTED or DISCONNECTED only (operator resolution 3).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GatewayRow {
    pub platform: String,
    pub connected: bool,
}

/// One row per platform, in key order.
pub fn gateway_rows(map: &PlatformStatusMap) -> Vec<GatewayRow> {
    map.iter()
        .map(|(platform, s)| GatewayRow {
            platform: platform.clone(),
            connected: s.connected,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Reports (D-10, D-11)
// ---------------------------------------------------------------------------

/// One published report version without its body (the tile does not render it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReportVersionView {
    pub id: String,
    pub name: String,
    pub title: String,
    pub summary: String,
    /// critical, high, medium, low, info.
    pub counts: [u32; 5],
    /// `(severity, title)`.
    pub findings: Vec<(String, String)>,
    pub published_at_ms: i64,
}

/// Merge two newest-first lists of versions (producer home + active-profile home): newest
/// first, duplicates by id removed (operator resolution 1).
#[cfg_attr(not(feature = "server"), allow(dead_code))] // only the server fns call it
pub fn merge_report_versions(
    a: Vec<ReportVersionView>,
    b: Vec<ReportVersionView>,
) -> Vec<ReportVersionView> {
    let mut all = a;
    all.extend(b);
    all.sort_by(|x, y| {
        y.published_at_ms
            .cmp(&x.published_at_ms)
            .then_with(|| y.id.cmp(&x.id))
    });
    let mut seen = std::collections::HashSet::new();
    all.retain(|v| seen.insert(v.id.clone()));
    all
}

/// Per-severity change of the latest version against the previous one. `versions` is newest
/// first; a single version has no delta (D-11).
pub fn report_delta(versions: &[ReportVersionView]) -> Option<[i64; 5]> {
    let (latest, previous) = (versions.first()?, versions.get(1)?);
    let mut d = [0i64; 5];
    for (i, slot) in d.iter_mut().enumerate() {
        *slot = i64::from(latest.counts[i]) - i64::from(previous.counts[i]);
    }
    Some(d)
}

#[cfg(feature = "server")]
mod reports {
    use super::*;
    use ironhermes_tools::report::{
        ReportVersion, Severity, list_report_names_at, list_versions_at, reports_root,
    };
    use std::path::PathBuf;

    fn view(v: ReportVersion) -> ReportVersionView {
        let c = &v.counts;
        let counts = [c.critical, c.high, c.medium, c.low, c.info];
        let findings = v
            .findings
            .iter()
            .map(|f| {
                let sev = match f.severity {
                    Severity::Critical => "critical",
                    Severity::High => "high",
                    Severity::Medium => "medium",
                    Severity::Low => "low",
                    Severity::Info => "info",
                };
                (sev.to_string(), f.title.clone())
            })
            .collect();
        ReportVersionView {
            id: v.id,
            name: v.name,
            title: v.title,
            summary: v.summary,
            counts,
            findings,
            published_at_ms: v.published_at_ms,
        }
    }

    /// Homes with duplicates removed so a shared path is read once.
    fn unique(homes: &[PathBuf]) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = Vec::new();
        for h in homes {
            let key = std::fs::canonicalize(h).unwrap_or_else(|_| h.clone());
            if !out
                .iter()
                .any(|o| std::fs::canonicalize(o).unwrap_or_else(|_| o.clone()) == key)
            {
                out.push(h.clone());
            }
        }
        out
    }

    /// Report names present under any of `homes`, sorted, each once.
    pub(super) fn report_names_from_homes(homes: &[PathBuf]) -> Vec<String> {
        let mut names: Vec<String> = unique(homes)
            .iter()
            .flat_map(|h| list_report_names_at(&reports_root(h)))
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Versions of `name` merged across `homes`, newest first. The name is validated before
    /// any path is built (T-54.10-15).
    pub(super) fn report_versions_from_homes(
        homes: &[PathBuf],
        name: &str,
    ) -> Result<Vec<ReportVersionView>, String> {
        if !crate::server::dashboard_api::valid_report_name(name) {
            return Err("Invalid report name".to_string());
        }
        Ok(unique(homes).iter().fold(Vec::new(), |acc, h| {
            let mine = list_versions_at(&reports_root(h), name)
                .into_iter()
                .map(view)
                .collect();
            merge_report_versions(acc, mine)
        }))
    }

    /// The producer (server) home and the active profile's home.
    pub(super) fn report_homes() -> Result<Vec<PathBuf>, String> {
        Ok(vec![
            ironhermes_core::constants::get_hermes_home(),
            crate::server::openui_views_api::active_profile_home()?,
        ])
    }
}

#[cfg(feature = "server")]
use reports::{report_homes, report_names_from_homes, report_versions_from_homes};

/// Report names from the union of the producer home and the active-profile home.
#[server]
pub async fn list_dashboard_reports() -> Result<Vec<String>, ServerFnError> {
    tokio::task::spawn_blocking(|| report_homes().map(|h| report_names_from_homes(&h)))
        .await
        .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
        .map_err(ServerFnError::new)
}

/// Versions of one report (newest first) from both homes.
#[server]
pub async fn dashboard_report_versions(
    name: String,
) -> Result<Vec<ReportVersionView>, ServerFnError> {
    if !crate::server::dashboard_api::valid_report_name(&name) {
        return Err(ServerFnError::new("Invalid report name"));
    }
    tokio::task::spawn_blocking(move || {
        report_homes().and_then(|h| report_versions_from_homes(&h, &name))
    })
    .await
    .map_err(|e| ServerFnError::new(format!("join error: {e}")))?
    .map_err(ServerFnError::new)
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;

    fn row(model: &str, cost: i64, tokens: i64) -> (String, String, i64, i64) {
        ("p".to_string(), model.to_string(), cost, tokens)
    }

    #[test]
    fn usage_window_maps_to_seconds() {
        assert_eq!(window_seconds(UsageWindow::Day), 86_400);
        assert_eq!(window_seconds(UsageWindow::Week), 604_800);
        assert_eq!(window_seconds(UsageWindow::Month), 2_592_000);
    }

    #[test]
    fn fold_usage_top5_and_cost_fallback() {
        let rows: Vec<_> = (1..=7)
            .map(|i| row(&format!("m{i}"), i * 10, i * 100))
            .collect();
        let v = fold_usage(rows, 3, UsageWindow::Week);
        assert_eq!(v.total_cost_micros, 280);
        assert_eq!(v.total_tokens, 2800);
        assert_eq!(v.failed_calls, 3);
        assert!(v.cost_recorded);
        let models: Vec<_> = v.rows.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["m7", "m6", "m5", "m4", "m3"]);

        // No cost anywhere: rank by tokens and flag it.
        let rows = vec![row("a", 0, 5), row("b", 0, 50), row("c", 0, 20)];
        let v = fold_usage(rows, 0, UsageWindow::Day);
        assert!(!v.cost_recorded);
        let models: Vec<_> = v.rows.iter().map(|r| r.model.as_str()).collect();
        assert_eq!(models, ["b", "c", "a"]);
    }

    #[test]
    fn dashboard_sessions_mark_active_by_ended_at() {
        let s = |id: &str, ended: Option<f64>, msgs: i64| {
            (id.to_string(), Some(format!("t-{id}")), 1.0, ended, msgs)
        };
        let rows = fold_sessions(
            [
                s("a", None, 3),
                s("b", Some(9.0), 2),
                s("empty", None, 0),
                s("c", None, 1),
            ],
            3,
        );
        let got: Vec<_> = rows.iter().map(|r| (r.id.as_str(), r.active)).collect();
        assert_eq!(got, [("a", true), ("b", false), ("c", true)]);
        assert_eq!(
            fold_sessions([s("a", None, 3), s("b", None, 3)], 1).len(),
            1
        );
    }

    fn task(title: &str, status: &str) -> TaskRow {
        TaskRow {
            id: title.to_string(),
            title: title.to_string(),
            body: None,
            assignee: String::new(),
            status: status.to_string(),
            priority: 0,
            tenant: None,
            workspace: None,
            created_at: 0.0,
            started_at: None,
            ended_at: None,
            output_path: None,
        }
    }

    #[test]
    fn kanban_summary_counts_by_status_and_lists_blocked() {
        let rows = [
            task("t1", "todo"),
            task("t2", "todo"),
            task("t3", "running"),
            task("stuck", "blocked"),
            task("d1", "done"),
            task("a1", "archived"),
        ];
        let v = kanban_summary(&rows, &[]);
        assert_eq!(v.columns.len(), 6);
        let count = |c| v.columns.iter().find(|(k, _)| *k == c).unwrap().1;
        assert_eq!(count(KanbanColumn::Todo), 2);
        assert_eq!(count(KanbanColumn::Done), 1);
        // open = todo 2 + running 1 + blocked 1; done and archived are excluded.
        assert_eq!(v.open_total, 4);
        assert_eq!(v.blocked, ["stuck"]);

        let v = kanban_summary(&rows, &[KanbanColumn::Todo, KanbanColumn::Running]);
        assert_eq!(v.columns.len(), 2);
        assert_eq!(v.open_total, 3);
    }

    fn sched(name: &str, enabled: bool, next: Option<&str>, last: Option<&str>) -> ScheduleRow {
        ScheduleRow {
            id: name.to_string(),
            name: name.to_string(),
            schedule_display: String::new(),
            schedule_raw: String::new(),
            prompt: String::new(),
            deliver: String::new(),
            last_run_at: last.map(|l| format!("shown {l}")),
            next_run_at: None,
            enabled,
            is_valid: true,
            last_run_at_raw: last.map(str::to_string),
            last_run_failed: last.map(|_| false),
            last_error: None,
            next_run_at_raw: next.map(str::to_string),
            skills: vec![],
            provider: None,
            model: None,
            base_url: None,
            script: None,
            workdir: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            continuity: false,
            profile: "default".to_string(),
        }
    }

    #[test]
    fn cron_summary_picks_next_three_and_latest_run() {
        let mut failed = sched(
            "f",
            true,
            Some("2026-01-01T05:00:00Z"),
            Some("2026-01-01T01:00:00Z"),
        );
        failed.last_run_failed = Some(true);
        failed.last_error = Some("boom".into());
        let rows = [
            sched(
                "a",
                true,
                Some("2026-01-01T09:00:00Z"),
                Some("2026-01-01T00:30:00Z"),
            ),
            sched("b", true, Some("2026-01-01T03:00:00+00:00"), None),
            sched("off", false, Some("2026-01-01T01:00:00Z"), None),
            sched("none", true, None, None),
            sched("c", true, Some("2026-01-01T04:00:00Z"), None),
            failed,
        ];
        let v = cron_summary(&rows);
        let names: Vec<_> = v.next.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["b", "c", "f"]);
        let last = v.last.unwrap();
        assert_eq!(last.name, "f");
        assert_eq!(last.failed, Some(true));
        assert_eq!(last.error.as_deref(), Some("boom"));

        // A later run in a different offset wins as an instant, not as a string.
        let rows = [
            sched("x", true, None, Some("2026-01-01T10:00:00+05:00")),
            sched("y", true, None, Some("2026-01-01T06:00:00Z")),
        ];
        assert_eq!(cron_summary(&rows).last.unwrap().name, "y");
        assert_eq!(cron_summary(&[]).last, None);
    }

    #[test]
    fn gateway_rows_connected_or_disconnected_only() {
        use crate::server::gateway_platform_status_api::PlatformStatusView;
        let mut map = PlatformStatusMap::new();
        let view = |connected| PlatformStatusView {
            connected,
            session_count: Some(4),
        };
        map.insert("telegram".into(), view(true));
        map.insert("slack".into(), view(false));
        let rows = gateway_rows(&map);
        let got: Vec<_> = rows
            .iter()
            .map(|r| (r.platform.as_str(), r.connected))
            .collect();
        assert_eq!(got, [("slack", false), ("telegram", true)]);
    }

    fn publish(home: &std::path::Path, name: &str, id: &str, at: i64, high: u32) {
        use ironhermes_tools::report::{
            REPORT_SCHEMA_VERSION, ReportVersion, SeverityCounts, publish_at, reports_root,
        };
        publish_at(
            &reports_root(home),
            ReportVersion {
                schema_version: REPORT_SCHEMA_VERSION,
                id: id.to_string(),
                name: name.to_string(),
                title: format!("{name} {id}"),
                summary: String::new(),
                counts: SeverityCounts {
                    high,
                    ..Default::default()
                },
                findings: vec![],
                body: String::new(),
                published_at_ms: at,
            },
        )
        .unwrap();
    }

    #[test]
    fn report_union_merges_two_homes_newest_wins() {
        let (producer, profile) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        publish(producer.path(), "redteam", "p1", 1_000, 2);
        publish(profile.path(), "redteam", "q2", 3_000, 3);
        publish(profile.path(), "redteam", "q1", 2_000, 2);
        publish(producer.path(), "only-producer", "z1", 5, 0);
        let homes = [producer.path().to_path_buf(), profile.path().to_path_buf()];
        let versions = reports::report_versions_from_homes(&homes, "redteam").unwrap();
        let ids: Vec<_> = versions.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["q2", "q1", "p1"]);
        assert_eq!(
            reports::report_names_from_homes(&homes),
            ["only-producer", "redteam"]
        );
        // The same version present in both homes appears once.
        publish(profile.path(), "redteam", "p1", 1_000, 2);
        let versions = reports::report_versions_from_homes(&homes, "redteam").unwrap();
        assert_eq!(versions.len(), 3);
        // One path twice is read once.
        let same = [producer.path().to_path_buf(), producer.path().to_path_buf()];
        assert_eq!(
            reports::report_versions_from_homes(&same, "redteam")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn report_delta_latest_vs_previous() {
        let v = |high: u32, info: u32| ReportVersionView {
            id: String::new(),
            name: "r".into(),
            title: String::new(),
            summary: String::new(),
            counts: [0, high, 0, 0, info],
            findings: vec![],
            published_at_ms: 0,
        };
        assert_eq!(report_delta(&[v(2, 0)]), None);
        assert_eq!(report_delta(&[v(3, 0), v(2, 4)]), Some([0, 1, 0, 0, -4]));
        assert_eq!(report_delta(&[v(2, 1), v(2, 1)]), Some([0; 5]));
    }

    #[test]
    fn dashboard_report_versions_rejects_bad_names() {
        let missing = [std::path::PathBuf::from("/nonexistent/54.10-home")];
        for bad in ["../x", "", "A", "a/b", "a..b"] {
            assert!(reports::report_versions_from_homes(&missing, bad).is_err());
        }
    }
}
