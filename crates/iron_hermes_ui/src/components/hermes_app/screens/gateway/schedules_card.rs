//! Gateway schedules status card (D-10) — a compact summary of the
//! selected scope's scheduled jobs: active job count, soonest next-run
//! time, and a red recent-failure badge. The standalone Schedules screen
//! (`screens/schedules.rs`) is unchanged and remains the only place to
//! create/edit/delete jobs — this card only summarizes and links through.
//!
//! Mounted as a sibling card in `gateway/mod.rs`'s existing `.grid.wide`,
//! following `ApiServerCard`'s prop signature
//! (`scope: ReadSignal<ConfigScope>, refresh_tick: Signal<u32>`) and its
//! resource-plus-refresh-tick idiom (Plan 01's established contract) — the
//! card refetches whenever the Gateway screen's scope selector changes.
//!
//! `schedules_card_summary` is a pure, clock-free function (the reference
//! instant is a parameter, never read internally) so it is directly unit
//! tested against every behavior bullet without a `JobStore` fixture.
//!
//! # `last_run_failed`/`last_run_at_raw`/`next_run_at_raw` (deviation, Rule 2)
//!
//! `ScheduleRow` did not previously expose whether a job's last run errored,
//! nor the RAW last-run/next-run instants (only locale-formatted display
//! strings) — all three are required for this card's "red badge when a job
//! failed recently" and "soonest next-run" to be truthful rather than
//! fabricated or lexically-sorted-wrong. `schedules_api.rs`'s `ScheduleRow`
//! and `build_schedule_row` were extended with `last_run_failed:
//! Option<bool>`, `last_run_at_raw: Option<String>`, and `next_run_at_raw:
//! Option<String>` to supply this (see this plan's SUMMARY, deviation
//! section) — the existing `create_schedule`/`update_schedule` server fn
//! SIGNATURES are unchanged, and no new server fn was added.
//!
//! # Recent-failure disclosure + full-width placement (2026-09-01)
//!
//! Operator request: the `N failed recently` badge should reveal WHICH jobs
//! failed when selected, and the card should span the top of the Gateway
//! card grid.
//!
//! The badge is now a real `button` toggling `failures_open`, and
//! `SchedulesCardSummary` carries `recent_failures: Vec<RecentFailure>`
//! with `recent_failure_count` derived from its length — so the badge and
//! the list it opens cannot disagree. Both the badge and the disclosure
//! `stop_propagation`, because the whole card is `role="button"` and would
//! otherwise navigate to the Schedules screen on the same click.
//!
//! Supplying the failure REASON required one more `ScheduleRow` field,
//! `last_error: Option<String>` — a passthrough of the already-persisted
//! `CronJob::last_error`, gated on the same "last run failed" predicate as
//! `last_run_failed` because `JobStore::mark_job_run` does not clear the
//! error text on a later success. Same widening precedent as the three
//! fields above; no server fn signature changed.

use crate::server::schedules_api::{get_schedules, ScheduleRow};
use crate::server::tools_config_api::ConfigScope;
use crate::state::Screen;
use dioxus::prelude::*;

/// Recent-failure lookback window (Claude's Discretion, per the
/// Copywriting Contract row: "Claude's Discretion: 24h").
const RECENT_FAILURE_LOOKBACK_SECS: i64 = 24 * 60 * 60;

/// Visible-row cap before the card links to the standalone screen for the
/// remainder (D-10/E8 overflow).
const VISIBLE_ROW_CAP: usize = 5;

/// Current instant, safe on wasm32 — `chrono::Utc::now()` calls
/// `SystemTime::now()` internally, which panics on
/// `wasm32-unknown-unknown` without the `wasmbind` feature (not enabled in
/// this crate). Mirrors `screens/schedules.rs`'s `now_rfc3339` /
/// `kanban/card.rs`'s `current_unix_time` cfg split.
fn current_instant() -> chrono::DateTime<chrono::Utc> {
    #[cfg(target_arch = "wasm32")]
    {
        let ms = js_sys::Date::now();
        let secs = (ms / 1000.0).floor() as i64;
        let millis_part = (ms - (secs as f64) * 1000.0).max(0.0) as u32;
        let nanos = millis_part.saturating_mul(1_000_000);
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nanos)
            .unwrap_or_else(|| chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        chrono::Utc::now()
    }
}

/// One recently-failed job, as disclosed by the card's expandable failure
/// list. Carries only what the disclosure renders — the card never links
/// per-row, so no id-keyed navigation target is needed beyond `id` as the
/// list key.
#[derive(Debug, Clone, PartialEq)]
pub struct RecentFailure {
    pub id: String,
    pub name: String,
    /// Formatted last-run timestamp (`ScheduleRow::last_run_at`), already
    /// in the operator's resolved display timezone. `None` is not possible
    /// in practice for a failure (the failure predicate requires a raw
    /// last-run instant) but is carried as `Option` rather than unwrapped
    /// so a store that ever writes one without the other degrades to "—"
    /// instead of panicking.
    pub last_run_at: Option<String>,
    /// Why it failed, from `ScheduleRow::last_error`. `None` when the store
    /// recorded a failure with no message — rendered as an explicit
    /// "no error message recorded" rather than an empty line, so the
    /// operator can tell "no message" apart from "not loaded yet".
    pub last_error: Option<String>,
}

/// Pure summary over a scope's schedule rows. See this module's tests for
/// every behavior bullet.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SchedulesCardSummary {
    pub active_count: usize,
    pub soonest_next_run: Option<String>,
    pub recent_failure_count: usize,
    /// The jobs behind `recent_failure_count`, in `rows` order. Always
    /// `recent_failure_count == recent_failures.len()` — the count is
    /// derived from this list, never counted separately, so the badge and
    /// the disclosure it opens can never disagree.
    pub recent_failures: Vec<RecentFailure>,
    pub overflow_count: usize,
}

/// Summarize `rows` as of `now` — never reads a clock internally, so tests
/// are fully deterministic.
///
/// - `active_count`: enabled jobs only.
/// - `soonest_next_run`: the earliest `next_run_at` (the FORMATTED display
///   string, for direct rendering) among enabled jobs that have one,
///   chosen by comparing the corresponding `next_run_at_raw` instants
///   (never a lexical string sort — see `schedules_api.rs` doc comment on
///   `next_run_at_raw`).
/// - `recent_failure_count`: jobs whose last run ended in error
///   (`last_run_failed == Some(true)`) AND whose `last_run_at_raw` falls
///   within `RECENT_FAILURE_LOOKBACK_SECS` of `now`.
/// - `overflow_count`: jobs beyond `VISIBLE_ROW_CAP` (over the full `rows`
///   slice, not just enabled ones — the card renders whichever rows it
///   shows and links out for the rest).
pub fn schedules_card_summary(
    rows: &[ScheduleRow],
    now: chrono::DateTime<chrono::Utc>,
) -> SchedulesCardSummary {
    let active_count = rows.iter().filter(|r| r.enabled).count();

    let soonest_next_run = rows
        .iter()
        .filter(|r| r.enabled)
        .filter_map(|r| {
            let raw = r.next_run_at_raw.as_deref()?;
            let instant = chrono::DateTime::parse_from_rfc3339(raw)
                .ok()?
                .with_timezone(&chrono::Utc);
            Some((instant, r.next_run_at.clone()))
        })
        .min_by_key(|(instant, _)| *instant)
        .and_then(|(_, display)| display);

    // Collect the failing rows rather than counting them: the badge's
    // disclosure needs WHICH jobs failed, and deriving the count from this
    // list (below) makes it impossible for the badge and the list it opens
    // to disagree. The predicate itself is unchanged.
    let recent_failures: Vec<RecentFailure> = rows
        .iter()
        .filter(|r| {
            if r.last_run_failed != Some(true) {
                return false;
            }
            let Some(raw) = r.last_run_at_raw.as_deref() else {
                return false;
            };
            let Ok(instant) = chrono::DateTime::parse_from_rfc3339(raw) else {
                return false;
            };
            let instant = instant.with_timezone(&chrono::Utc);
            let age_secs = (now - instant).num_seconds();
            (0..=RECENT_FAILURE_LOOKBACK_SECS).contains(&age_secs)
        })
        .map(|r| RecentFailure {
            id: r.id.clone(),
            name: r.name.clone(),
            last_run_at: r.last_run_at.clone(),
            last_error: r.last_error.clone(),
        })
        .collect();
    let recent_failure_count = recent_failures.len();

    let overflow_count = rows.len().saturating_sub(VISIBLE_ROW_CAP);

    SchedulesCardSummary {
        active_count,
        soonest_next_run,
        recent_failure_count,
        recent_failures,
        overflow_count,
    }
}

#[component]
pub fn GatewaySchedulesCard(scope: ReadSignal<ConfigScope>, refresh_tick: Signal<u32>) -> Element {
    // ALL hooks register unconditionally on every render (Pattern E from
    // PATTERNS.md — agents.rs UAT-2 hotfix discipline).
    let mut active_screen = use_context::<Signal<Screen>>();

    // Whether the recent-failure disclosure is expanded. Local to the card
    // and deliberately NOT persisted — it is a transient "what broke?" peek,
    // not a preference, and a remembered-open panel would re-open stale on
    // a later visit after the failures had aged out of the lookback window.
    let mut failures_open = use_signal(|| false);

    // `get_schedules` returns the WHOLE job list (no scope filter server
    // side — schedules are not currently scope-partitioned in
    // `ironhermes_cron::JobStore`). `scope` is read in the resource's sync
    // prefix so a scope change re-triggers this card's own fetch (matching
    // `ApiServerCard`'s idiom) even though the returned rows are the same
    // set for every scope today — this keeps the prop contract identical
    // to every sibling card and is forward-compatible if jobs ever gain a
    // scope dimension.
    let schedules_resource = use_resource(move || {
        let _scope_value = scope();
        let _tick = refresh_tick();
        // Phase 49.6 Plan 02 (D-04): aggregate scope (`None`) — preserves
        // this card's pre-existing "whole job list" contract now that the
        // list can span multiple profile stores, not just root.
        async move { get_schedules(None).await.map(|view| view.rows) }
    });

    // Extract data BEFORE rsx! — signal-borrow discipline
    // (iron_hermes_ui/clippy.toml: no GenerationalRef held across RSX).
    let is_loading = schedules_resource().is_none();
    let load_error: Option<String> = match schedules_resource() {
        Some(Err(ref e)) => Some(e.to_string()),
        _ => None,
    };
    let rows: Vec<ScheduleRow> = match schedules_resource() {
        Some(Ok(rows)) => rows,
        _ => Vec::new(),
    };

    let now = current_instant();
    let summary = schedules_card_summary(&rows, now);
    let visible_rows: Vec<&ScheduleRow> = rows.iter().take(VISIBLE_ROW_CAP).collect();

    let go_to_schedules = move |_| {
        active_screen.set(Screen::Schedules);
    };

    rsx! {
        div {
            // `--full` spans every column of the Gateway screen's
            // `.grid.wide`, placing this card as one banner across the top
            // of the card scroll (operator request, 2026-09-01).
            class: "plat-card plat-card--full",
            role: "button",
            tabindex: "0",
            "aria-label": "Open the Schedules screen",
            onclick: go_to_schedules,
            div { class: "plat-head",
                div { class: "plat-glyph", "◷" }
                div { style: "flex:1",
                    div { class: "plat-name", "Schedules" }
                    div { class: "plat-state", "{summary.active_count} ACTIVE" }
                }
                if summary.recent_failure_count > 0 {
                    button {
                        class: "pill red sched-fail-toggle",
                        "aria-expanded": if failures_open() { "true" } else { "false" },
                        "aria-label": if failures_open() {
                            "Hide which jobs failed recently".to_string()
                        } else {
                            format!("Show which {} job(s) failed recently", summary.recent_failure_count)
                        },
                        // The whole card is role="button" and navigates to
                        // the Schedules screen; without this the disclosure
                        // would open and immediately be replaced by that
                        // navigation. Same guard the overflow button uses.
                        onclick: move |evt: Event<MouseData>| {
                            evt.stop_propagation();
                            let cur = failures_open();
                            failures_open.set(!cur);
                        },
                        "{summary.recent_failure_count} failed recently"
                        span { class: "sched-fail-caret", "aria-hidden": "true",
                            if failures_open() { " ▾" } else { " ▸" }
                        }
                    }
                }
            }
            if failures_open() && summary.recent_failure_count > 0 {
                div {
                    class: "sched-fail-list",
                    role: "region",
                    "aria-label": "Jobs that failed recently",
                    // Clicks inside the disclosure must not navigate away
                    // either — an operator selecting error text to copy it
                    // would otherwise be thrown to the Schedules screen.
                    onclick: move |evt: Event<MouseData>| evt.stop_propagation(),
                    for failure in summary.recent_failures.iter() {
                        div { class: "sched-fail-item", key: "{failure.id}",
                            div { class: "sched-fail-item-head",
                                span { class: "sched-fail-item-name", "{failure.name}" }
                                span { class: "sched-fail-item-when",
                                    "{failure.last_run_at.clone().unwrap_or_else(|| \"—\".to_string())}"
                                }
                            }
                            if let Some(err) = failure.last_error.clone() {
                                p { class: "sched-fail-item-err", "{err}" }
                            } else {
                                p { class: "sched-fail-item-err sched-fail-item-err--none",
                                    "No error message recorded."
                                }
                            }
                        }
                    }
                }
            }
            if is_loading {
                div { class: "plat-card--ghost", "aria-hidden": "true",
                    dl { class: "kv", dt { "Next run" } dd { "···" } }
                }
            } else if let Some(reason) = load_error {
                p { class: "plat-card-help", "Could not load schedules for this scope — {reason}." }
            } else if rows.is_empty() {
                p { class: "plat-card-help", "No active jobs for this scope." }
            } else {
                div { class: "sched-card-rows",
                    dl { class: "kv",
                        dt { "Next run" }
                        dd { "{summary.soonest_next_run.clone().unwrap_or_else(|| \"—\".to_string())}" }
                    }
                    for row in visible_rows.iter() {
                        div { class: "sched-card-row", key: "{row.id}",
                            span { class: "sched-card-row-name", "{row.name}" }
                            span { class: "sched-card-row-next", "{row.next_run_at.clone().unwrap_or_else(|| \"—\".to_string())}" }
                        }
                    }
                    if summary.overflow_count > 0 {
                        button {
                            class: "btn btn--ghost btn--sm",
                            "aria-label": "See all scheduled jobs on the Schedules screen",
                            onclick: move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                active_screen.set(Screen::Schedules);
                            },
                            "+{summary.overflow_count} more — SCHEDULES"
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod schedules_card_summary_tests {
    use super::*;

    fn row(
        id: &str,
        enabled: bool,
        next_run_at_raw: Option<&str>,
        last_run_at_raw: Option<&str>,
        last_run_failed: Option<bool>,
    ) -> ScheduleRow {
        ScheduleRow {
            id: id.to_string(),
            name: id.to_string(),
            schedule_display: "daily 09:00".to_string(),
            schedule_raw: "0 9 * * *".to_string(),
            prompt: "do something".to_string(),
            deliver: "local".to_string(),
            last_run_at: last_run_at_raw.map(|s| s.to_string()),
            next_run_at: next_run_at_raw.map(|s| s.to_string()),
            enabled,
            is_valid: true,
            last_run_at_raw: last_run_at_raw.map(|s| s.to_string()),
            last_run_failed,
            // Fixture default: a failure with no recorded message. The
            // tests that assert on error text set this explicitly via
            // `row_with_error` below.
            last_error: None,
            next_run_at_raw: next_run_at_raw.map(|s| s.to_string()),
            // Phase 49.5 Plan 06 (D-15/D-16): this card never reads the
            // advanced fields — only `schedules_card_summary`'s own
            // enabled/next-run/last-run logic under test here — so the
            // fixture uses each field's zero value.
            skills: Vec::new(),
            provider: None,
            model: None,
            base_url: None,
            script: None,
            workdir: None,
            no_agent: false,
            context_from: None,
            enabled_toolsets: None,
            continuity: false,
            // Phase 49.6 Plan 02 (D-01): profile is irrelevant to this
            // card's summary math — root sentinel is the neutral fixture
            // value.
            profile: "default".to_string(),
        }
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-29T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn empty_slice_reports_all_zero() {
        let s = schedules_card_summary(&[], now());
        assert_eq!(s.active_count, 0);
        assert_eq!(s.soonest_next_run, None);
        assert_eq!(s.recent_failure_count, 0);
        assert_eq!(s.overflow_count, 0);
    }

    #[test]
    fn three_enabled_jobs_reports_three_active_and_earliest_next_run() {
        let rows = vec![
            row("a", true, Some("2026-08-30T09:00:00Z"), None, None),
            row("b", true, Some("2026-08-29T15:00:00Z"), None, None),
            row("c", true, Some("2026-09-01T09:00:00Z"), None, None),
        ];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.active_count, 3);
        assert_eq!(s.soonest_next_run, Some("2026-08-29T15:00:00Z".to_string()));
    }

    #[test]
    fn disabled_job_excluded_from_active_count_and_soonest() {
        let rows = vec![
            row("a", false, Some("2026-08-29T13:00:00Z"), None, None),
            row("b", true, Some("2026-08-30T09:00:00Z"), None, None),
        ];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.active_count, 1);
        assert_eq!(s.soonest_next_run, Some("2026-08-30T09:00:00Z".to_string()));
    }

    #[test]
    fn last_run_failed_within_lookback_increments_recent_failure_count() {
        let rows = vec![row(
            "a",
            true,
            None,
            Some("2026-08-29T06:00:00Z"), // 6h before `now()`
            Some(true),
        )];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.recent_failure_count, 1);
    }

    /// Same fixture as `row`, plus recorded error text.
    fn row_with_error(
        id: &str,
        last_run_at_raw: Option<&str>,
        last_run_failed: Option<bool>,
        last_error: Option<&str>,
    ) -> ScheduleRow {
        ScheduleRow {
            last_error: last_error.map(|s| s.to_string()),
            ..row(id, true, None, last_run_at_raw, last_run_failed)
        }
    }

    #[test]
    fn recent_failures_list_matches_the_count_it_is_derived_from() {
        let rows = vec![
            row_with_error("a", Some("2026-08-29T06:00:00Z"), Some(true), Some("boom")),
            row("b", true, None, Some("2026-08-29T07:00:00Z"), Some(false)),
            row_with_error("c", Some("2026-08-29T08:00:00Z"), Some(true), None),
        ];
        let s = schedules_card_summary(&rows, now());
        // The badge count and the list it opens can never disagree.
        assert_eq!(s.recent_failure_count, s.recent_failures.len());
        assert_eq!(s.recent_failure_count, 2);
        let ids: Vec<&str> = s.recent_failures.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "c"], "only the failed jobs, in rows order");
    }

    #[test]
    fn recent_failure_carries_error_text_and_distinguishes_absent_message() {
        let rows = vec![
            row_with_error(
                "a",
                Some("2026-08-29T06:00:00Z"),
                Some(true),
                Some("provider timeout after 30s"),
            ),
            row_with_error("b", Some("2026-08-29T06:00:00Z"), Some(true), None),
        ];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(
            s.recent_failures[0].last_error.as_deref(),
            Some("provider timeout after 30s")
        );
        // `None` is a distinct, renderable state ("no error message
        // recorded") — not an empty string, so the card can tell it apart.
        assert_eq!(s.recent_failures[1].last_error, None);
    }

    #[test]
    fn a_failure_outside_the_lookback_is_absent_from_the_list_not_just_the_count() {
        let rows = vec![row_with_error(
            "a",
            Some("2026-08-27T06:00:00Z"), // > 24h before `now()`
            Some(true),
            Some("stale error text still on disk"),
        )];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.recent_failure_count, 0);
        assert!(
            s.recent_failures.is_empty(),
            "an aged-out failure must not leak into the disclosure list"
        );
    }

    #[test]
    fn last_run_failed_outside_lookback_does_not_increment() {
        let rows = vec![row(
            "a",
            true,
            None,
            Some("2026-08-27T06:00:00Z"), // > 24h before `now()`
            Some(true),
        )];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.recent_failure_count, 0);
    }

    #[test]
    fn job_that_never_ran_does_not_increment_failure_count() {
        let rows = vec![row("a", true, None, None, None)];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.recent_failure_count, 0);
    }

    #[test]
    fn last_run_ok_within_lookback_does_not_increment() {
        let rows = vec![row(
            "a",
            true,
            None,
            Some("2026-08-29T06:00:00Z"),
            Some(false),
        )];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.recent_failure_count, 0);
    }

    #[test]
    fn eight_jobs_reports_five_visible_and_three_overflow() {
        let rows: Vec<ScheduleRow> = (0..8)
            .map(|i| row(&format!("job-{i}"), true, None, None, None))
            .collect();
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.overflow_count, 3);
    }

    #[test]
    fn enabled_job_with_no_next_run_counts_as_active_but_never_soonest() {
        let rows = vec![
            row("a", true, None, None, None),
            row("b", true, Some("2026-08-30T09:00:00Z"), None, None),
        ];
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.active_count, 2);
        assert_eq!(s.soonest_next_run, Some("2026-08-30T09:00:00Z".to_string()));
    }

    #[test]
    fn five_or_fewer_jobs_reports_zero_overflow() {
        let rows: Vec<ScheduleRow> = (0..5)
            .map(|i| row(&format!("job-{i}"), true, None, None, None))
            .collect();
        let s = schedules_card_summary(&rows, now());
        assert_eq!(s.overflow_count, 0);
    }
}
