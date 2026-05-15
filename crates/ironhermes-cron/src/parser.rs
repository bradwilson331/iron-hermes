use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Duration, NaiveDateTime, TimeZone, Utc};
use cron::Schedule;
use regex::Regex;
use std::str::FromStr;

use crate::job::ScheduleParsed;

// ---------------------------------------------------------------------------
// Grace window constants
// ---------------------------------------------------------------------------

/// The grace window (in seconds) for one-shot jobs: a `Once` job whose
/// `run_at` was up to this many seconds ago and has never run is still
/// considered due.
pub const ONESHOT_GRACE_SECONDS: i64 = 120;

// ---------------------------------------------------------------------------
// parse_duration
// ---------------------------------------------------------------------------

/// Parse a duration string like "30m", "2h", "1d" into minutes.
pub fn parse_duration(input: &str) -> Result<u32> {
    let re =
        Regex::new(r"(?i)^(\d+)\s*(m|min|mins|minute|minutes|h|hr|hrs|hour|hours|d|day|days)$")
            .expect("static regex");

    let caps = re
        .captures(input.trim())
        .ok_or_else(|| anyhow!("invalid duration: {:?}", input))?;

    let amount: u32 = caps[1].parse().context("duration amount overflow")?;
    let unit = caps[2].to_lowercase();

    let minutes = match unit.as_str() {
        "m" | "min" | "mins" | "minute" | "minutes" => amount,
        "h" | "hr" | "hrs" | "hour" | "hours" => amount * 60,
        "d" | "day" | "days" => amount * 1440,
        _ => unreachable!("regex guarantees valid unit"),
    };

    Ok(minutes)
}

// ---------------------------------------------------------------------------
// parse_schedule
// ---------------------------------------------------------------------------

/// Parse a user-supplied schedule string into a `ScheduleParsed` variant.
///
/// Rules (in order):
/// 1. Starts with "every " → parse remainder as duration → Interval
/// 2. 5+ whitespace-separated cron fields (all `[\d*\-,/]+`) → Cron
/// 3. Contains 'T' or looks like ISO date (starts with 4 digits, len >= 10) → Once (timestamp)
/// 4. Bare duration string (e.g. "30m") → Once with run_at = now + duration
pub fn parse_schedule(input: &str) -> Result<ScheduleParsed> {
    let s = input.trim();

    // Rule 1: "every X" → Interval
    if let Some(rest) = s.strip_prefix("every ") {
        let minutes =
            parse_duration(rest).with_context(|| format!("invalid interval in {:?}", s))?;
        let display = format!("every {}m", minutes);
        return Ok(ScheduleParsed::Interval { minutes, display });
    }

    // Rule 2: cron expression (5 or 6 whitespace-separated fields)
    {
        let fields: Vec<&str> = s.split_whitespace().collect();
        if fields.len() >= 5 {
            let all_cron = fields.iter().all(|f| {
                f.chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, '*' | '-' | ',' | '/' | '?'))
            });
            if all_cron {
                // Validate with the cron crate (normalise 5-field to 6-field)
                let normalised = if fields.len() == 5 {
                    format!("0 {}", s)
                } else {
                    s.to_owned()
                };
                Schedule::from_str(&normalised)
                    .with_context(|| format!("invalid cron expression: {:?}", s))?;
                let display = s.to_owned();
                return Ok(ScheduleParsed::Cron {
                    expr: s.to_owned(),
                    display,
                });
            }
        }
    }

    // Rule 3: ISO timestamp
    if s.contains('T') || (s.len() >= 10 && s.chars().take(4).all(|c| c.is_ascii_digit())) {
        // Try RFC3339 first
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            let run_at = dt.with_timezone(&Utc);
            let display = format!("once at {}", s);
            return Ok(ScheduleParsed::Once { run_at, display });
        }
        // Fallback: NaiveDateTime without timezone
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            let run_at = Utc.from_utc_datetime(&ndt);
            let display = format!("once at {}", s);
            return Ok(ScheduleParsed::Once { run_at, display });
        }
        // Try date-only
        if let Ok(nd) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            let ndt = nd.and_hms_opt(0, 0, 0).unwrap();
            let run_at = Utc.from_utc_datetime(&ndt);
            let display = format!("once at {}", s);
            return Ok(ScheduleParsed::Once { run_at, display });
        }
    }

    // Rule 4: bare duration → Once
    let minutes =
        parse_duration(s).with_context(|| format!("unrecognised schedule: {:?}", input))?;
    let run_at = Utc::now() + Duration::minutes(minutes as i64);
    let display = format!("once in {}m", minutes);
    Ok(ScheduleParsed::Once { run_at, display })
}

// ---------------------------------------------------------------------------
// compute_grace_seconds
// ---------------------------------------------------------------------------

/// Return the grace window in seconds for a given schedule:
///
/// - `Interval { minutes }`: `(minutes * 60) / 2`, clamped to `[120, 7200]`
/// - `Cron`: `3600` (one hour)
/// - `Once`: `ONESHOT_GRACE_SECONDS` (120)
pub fn compute_grace_seconds(schedule: &ScheduleParsed) -> i64 {
    match schedule {
        ScheduleParsed::Interval { minutes, .. } => {
            ((*minutes as i64) * 60 / 2).clamp(120, 7200)
        }
        ScheduleParsed::Cron { .. } => 3600,
        ScheduleParsed::Once { .. } => ONESHOT_GRACE_SECONDS,
    }
}

// ---------------------------------------------------------------------------
// compute_next_run / compute_next_run_from
// ---------------------------------------------------------------------------

/// Compute the next run time after `after` for a parsed schedule, optionally
/// anchored at `last_run_at` instead of `after` for recurring schedules.
///
/// - `Once`:    returns `Some(run_at)` if `run_at > after`, else `None`.
///              `last_run_at` is ignored for Once.
/// - `Interval`: when `last_run_at` is `Some(t)`, returns `Some(t + period)`;
///               when `None`, returns `Some(after + period)`. This prevents
///               drift across crashes by anchoring at the last known run.
/// - `Cron`:    finds the first tick strictly after `last_run_at.unwrap_or(after)`.
pub fn compute_next_run_from(
    schedule: &ScheduleParsed,
    after: DateTime<Utc>,
    last_run_at: Option<DateTime<Utc>>,
) -> Result<Option<DateTime<Utc>>> {
    match schedule {
        ScheduleParsed::Once { run_at, .. } => {
            if *run_at > after {
                Ok(Some(*run_at))
            } else {
                Ok(None)
            }
        }
        ScheduleParsed::Interval { minutes, .. } => {
            let anchor = last_run_at.unwrap_or(after);
            Ok(Some(anchor + Duration::minutes(*minutes as i64)))
        }
        ScheduleParsed::Cron { expr, .. } => {
            let anchor = last_run_at.unwrap_or(after);
            let normalised = if expr.split_whitespace().count() == 5 {
                format!("0 {}", expr)
            } else {
                expr.clone()
            };
            let sched = Schedule::from_str(&normalised)
                .with_context(|| format!("invalid cron expression: {:?}", expr))?;
            Ok(sched.after(&anchor).next())
        }
    }
}

/// Compute the next run time after `after` for a parsed schedule.
///
/// This is a thin wrapper around [`compute_next_run_from`] with
/// `last_run_at = None`, preserving backward compatibility for all
/// existing call sites.
///
/// - `Once`:    returns `Some(run_at)` if `run_at > after`, else `None`
/// - `Interval`: returns `Some(after + minutes)`
/// - `Cron`:    normalise to 6-field, find next occurrence via cron crate
pub fn compute_next_run(
    schedule: &ScheduleParsed,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>> {
    compute_next_run_from(schedule, after, None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod parser_tests_phase_32_1 {
    use super::*;
    use chrono::TimeZone;

    fn t(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    fn interval(minutes: u32) -> ScheduleParsed {
        ScheduleParsed::Interval {
            minutes,
            display: format!("every {}m", minutes),
        }
    }

    fn cron_sched(expr: &str) -> ScheduleParsed {
        ScheduleParsed::Cron {
            expr: expr.to_string(),
            display: expr.to_string(),
        }
    }

    fn once_sched(run_at: DateTime<Utc>) -> ScheduleParsed {
        ScheduleParsed::Once {
            run_at,
            display: "once".to_string(),
        }
    }

    // Test 1: ONESHOT_GRACE_SECONDS is 120
    #[test]
    fn test1_oneshot_grace_seconds_is_120() {
        assert_eq!(ONESHOT_GRACE_SECONDS, 120i64);
        // Also verify it is accessible at the crate root level via re-export
        assert_eq!(crate::ONESHOT_GRACE_SECONDS, 120i64);
    }

    // Test 2: compute_grace_seconds for Interval 10min = 300
    #[test]
    fn test2_grace_interval_10min_returns_300() {
        let sched = interval(10);
        assert_eq!(compute_grace_seconds(&sched), 300);
    }

    // Test 3: compute_grace_seconds for Interval 1min clamps up to 120
    #[test]
    fn test3_grace_interval_1min_clamps_to_120() {
        let sched = interval(1);
        assert_eq!(compute_grace_seconds(&sched), 120);
    }

    // Test 4: compute_grace_seconds for Interval 600min clamps down to 7200
    #[test]
    fn test4_grace_interval_600min_clamps_to_7200() {
        let sched = interval(600);
        assert_eq!(compute_grace_seconds(&sched), 7200);
    }

    // Test 5: compute_grace_seconds for Cron returns 3600
    #[test]
    fn test5_grace_cron_returns_3600() {
        let sched = cron_sched("0 9 * * *");
        assert_eq!(compute_grace_seconds(&sched), 3600);
    }

    // Test 6: compute_grace_seconds for Once returns 120 (== ONESHOT_GRACE_SECONDS)
    #[test]
    fn test6_grace_once_returns_120() {
        let sched = once_sched(t(2026, 1, 1, 12, 0, 0));
        assert_eq!(compute_grace_seconds(&sched), ONESHOT_GRACE_SECONDS);
    }

    // Test 7: compute_next_run_from with None matches compute_next_run (backward compat)
    #[test]
    fn test7_compute_next_run_from_none_matches_compute_next_run() {
        let t0 = t(2026, 1, 1, 12, 0, 0);
        let sched = interval(10);
        let expected = compute_next_run(&sched, t0).unwrap();
        let actual = compute_next_run_from(&sched, t0, None).unwrap();
        assert_eq!(expected, actual);
    }

    // Test 8: compute_next_run_from with Some(t_last) anchors at t_last + interval
    #[test]
    fn test8_compute_next_run_from_anchors_at_last_run() {
        let t_last = t(2026, 1, 1, 10, 0, 0);
        let t_now = t(2026, 1, 1, 12, 0, 0); // t_now differs from t_last
        let sched = interval(10);
        let result = compute_next_run_from(&sched, t_now, Some(t_last)).unwrap();
        let expected = Some(t_last + Duration::minutes(10));
        assert_eq!(result, expected);
    }

    // Test 9: compute_next_run_from for Cron anchors at t_last (first tick after t_last, not t_now)
    #[test]
    fn test9_compute_next_run_from_cron_anchors_at_last_run() {
        // "0 9 * * *" fires at 09:00 UTC every day
        let sched = cron_sched("0 9 * * *");
        let t_last = t(2026, 1, 1, 8, 0, 0); // 08:00 Jan 1
        let t_now = t(2026, 1, 2, 10, 0, 0); // 10:00 Jan 2 — if we anchor at now, next would be Jan 3
        let result = compute_next_run_from(&sched, t_now, Some(t_last)).unwrap();
        // First tick strictly after t_last (2026-01-01 08:00) should be 2026-01-01 09:00
        let expected = Some(t(2026, 1, 1, 9, 0, 0));
        assert_eq!(result, expected);
    }

    // Test 10: compute_next_run (old signature) still compiles and equals compute_next_run_from(..., None)
    #[test]
    fn test10_compute_next_run_wrapper_equals_from_none() {
        let t0 = t(2026, 6, 15, 12, 0, 0);

        // Interval
        let sched_interval = interval(30);
        let old = compute_next_run(&sched_interval, t0).unwrap();
        let new = compute_next_run_from(&sched_interval, t0, None).unwrap();
        assert_eq!(old, new, "Interval: old and new must agree");

        // Cron
        let sched_cron = cron_sched("*/5 * * * *");
        let old_cron = compute_next_run(&sched_cron, t0).unwrap();
        let new_cron = compute_next_run_from(&sched_cron, t0, None).unwrap();
        assert_eq!(old_cron, new_cron, "Cron: old and new must agree");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, Utc};

    // --- parse_duration ---

    #[test]
    fn parse_duration_30m() {
        assert_eq!(parse_duration("30m").unwrap(), 30);
    }

    #[test]
    fn parse_duration_2h() {
        assert_eq!(parse_duration("2h").unwrap(), 120);
    }

    #[test]
    fn parse_duration_1d() {
        assert_eq!(parse_duration("1d").unwrap(), 1440);
    }

    #[test]
    fn parse_duration_bad() {
        assert!(parse_duration("bad").is_err());
    }

    #[test]
    fn parse_duration_minutes_word() {
        assert_eq!(parse_duration("45 minutes").unwrap(), 45);
    }

    #[test]
    fn parse_duration_hours_word() {
        assert_eq!(parse_duration("3 hours").unwrap(), 180);
    }

    // --- parse_schedule: Interval ---

    #[test]
    fn parse_schedule_every_2h() {
        let result = parse_schedule("every 2h").unwrap();
        assert_eq!(
            result,
            ScheduleParsed::Interval {
                minutes: 120,
                display: "every 120m".to_string()
            }
        );
    }

    #[test]
    fn parse_schedule_every_30m() {
        let result = parse_schedule("every 30m").unwrap();
        assert_eq!(
            result,
            ScheduleParsed::Interval {
                minutes: 30,
                display: "every 30m".to_string()
            }
        );
    }

    #[test]
    fn parse_schedule_every_1d() {
        let result = parse_schedule("every 1d").unwrap();
        assert_eq!(
            result,
            ScheduleParsed::Interval {
                minutes: 1440,
                display: "every 1440m".to_string()
            }
        );
    }

    // --- parse_schedule: Cron ---

    #[test]
    fn parse_schedule_cron_5field() {
        let result = parse_schedule("0 9 * * *").unwrap();
        match result {
            ScheduleParsed::Cron { expr, display } => {
                assert_eq!(expr, "0 9 * * *");
                assert_eq!(display, "0 9 * * *");
            }
            other => panic!("expected Cron, got {:?}", other),
        }
    }

    #[test]
    fn parse_schedule_cron_wildcard() {
        let result = parse_schedule("*/5 * * * *").unwrap();
        match result {
            ScheduleParsed::Cron { expr, .. } => {
                assert_eq!(expr, "*/5 * * * *");
            }
            other => panic!("expected Cron, got {:?}", other),
        }
    }

    // --- parse_schedule: Once (duration) ---

    #[test]
    fn parse_schedule_once_duration() {
        let before = Utc::now();
        let result = parse_schedule("30m").unwrap();
        let after = Utc::now();
        match result {
            ScheduleParsed::Once { run_at, display } => {
                // run_at should be ~30 minutes from now
                let lower = before + Duration::minutes(29);
                let upper = after + Duration::minutes(31);
                assert!(
                    run_at >= lower && run_at <= upper,
                    "run_at {} not in expected window",
                    run_at
                );
                assert!(display.contains("30m"), "display: {}", display);
            }
            other => panic!("expected Once, got {:?}", other),
        }
    }

    // --- parse_schedule: Once (ISO timestamp) ---

    #[test]
    fn parse_schedule_once_iso_timestamp() {
        let result = parse_schedule("2026-04-10T09:00:00Z").unwrap();
        match result {
            ScheduleParsed::Once { run_at, .. } => {
                assert_eq!(run_at.year(), 2026);
                assert_eq!(run_at.month(), 4);
                assert_eq!(run_at.day(), 10);
            }
            other => panic!("expected Once, got {:?}", other),
        }
    }

    // --- parse_schedule: error ---

    #[test]
    fn parse_schedule_invalid() {
        assert!(parse_schedule("not valid gibberish").is_err());
    }

    // --- ScheduleParsed serde roundtrip ---

    #[test]
    fn schedule_parsed_serde_roundtrip_interval() {
        let orig = ScheduleParsed::Interval {
            minutes: 60,
            display: "every 60m".to_string(),
        };
        let json = serde_json::to_string(&orig).unwrap();
        let back: ScheduleParsed = serde_json::from_str(&json).unwrap();
        assert_eq!(orig, back);
    }

    #[test]
    fn schedule_parsed_serde_roundtrip_cron() {
        let orig = ScheduleParsed::Cron {
            expr: "0 9 * * *".to_string(),
            display: "0 9 * * *".to_string(),
        };
        let json = serde_json::to_string(&orig).unwrap();
        let back: ScheduleParsed = serde_json::from_str(&json).unwrap();
        assert_eq!(orig, back);
    }

    #[test]
    fn schedule_parsed_serde_roundtrip_once() {
        let ts = Utc::now();
        let orig = ScheduleParsed::Once {
            run_at: ts,
            display: "once in 30m".to_string(),
        };
        let json = serde_json::to_string(&orig).unwrap();
        let back: ScheduleParsed = serde_json::from_str(&json).unwrap();
        assert_eq!(orig, back);
    }

    // --- compute_next_run ---

    #[test]
    fn compute_next_run_once_future() {
        let future = Utc::now() + Duration::hours(1);
        let sched = ScheduleParsed::Once {
            run_at: future,
            display: "once".to_string(),
        };
        let result = compute_next_run(&sched, Utc::now()).unwrap();
        assert_eq!(result, Some(future));
    }

    #[test]
    fn compute_next_run_once_past() {
        let past = Utc::now() - Duration::hours(1);
        let sched = ScheduleParsed::Once {
            run_at: past,
            display: "once".to_string(),
        };
        let result = compute_next_run(&sched, Utc::now()).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn compute_next_run_interval() {
        let now = Utc::now();
        let sched = ScheduleParsed::Interval {
            minutes: 30,
            display: "every 30m".to_string(),
        };
        let result = compute_next_run(&sched, now).unwrap();
        let expected = now + Duration::minutes(30);
        // Allow 1 second slack
        assert!(result.is_some());
        let diff = (result.unwrap() - expected).num_seconds().abs();
        assert!(diff <= 1);
    }

    #[test]
    fn compute_next_run_cron() {
        let now = Utc::now();
        let sched = ScheduleParsed::Cron {
            expr: "0 9 * * *".to_string(),
            display: "0 9 * * *".to_string(),
        };
        let result = compute_next_run(&sched, now).unwrap();
        assert!(result.is_some());
        assert!(result.unwrap() > now);
    }
}
