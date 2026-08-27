//! Tick-loop heartbeat persistence (Phase 49.2 Plan 01, D-03/D-04).
//!
//! A small sidecar JSON file (`tick_state.json`) lives next to `jobs.json` in
//! the same cron directory both the gateway (writer) and the CLI (reader)
//! already share via `get_hermes_home().join("cron")`. Before this module,
//! there was no way for a separate CLI process invocation (`cron status`,
//! `cron run`) to tell whether the gateway's tick loop was alive, wedged, or
//! simply idle — a healthy zero-due tick produced no observable trace at all.
//!
//! `TickState` is intentionally forward/backward compatible: it derives
//! `#[serde(default)]` at the struct level so that a later plan (49.2 Plan 02)
//! can add fields (`last_boot_at`, `backlog`) without breaking a reader built
//! against this earlier version, and so that an older on-disk file missing
//! those fields still deserializes cleanly once they're added.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use ironhermes_core::get_hermes_home;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::warn;

/// Filename of the tick heartbeat sidecar, sibling of `jobs.json`.
pub const TICK_STATE_FILE: &str = "tick_state.json";

/// A tick is considered stale once its `last_tick_at` is this many seconds
/// old — 1.5x the 60s tick interval that D-06 keeps unchanged.
pub const TICK_STALE_SECONDS: i64 = 90;

/// Maximum number of runtime grace-skip events retained in
/// [`TickState::recent_skips`]. Every [`record_grace_skips_at`] write
/// truncates from the FRONT (oldest first) once this bound is exceeded, so
/// `tick_state.json` cannot grow without bound on a long-running or
/// frequently-skipping install (T-49.2-04-01).
pub const GRACE_SKIP_HISTORY_MAX: usize = 20;

// ---------------------------------------------------------------------------
// TickSummary — the tick-owned fields a caller reports each cycle
// ---------------------------------------------------------------------------

/// Per-tick summary passed by the tick-check call site to [`record_tick`] /
/// [`record_tick_at`]. Deliberately narrow: only counts, timestamps (added by
/// the recorder itself), and an optional error string. Never job prompt text.
#[derive(Debug, Clone, Default)]
pub struct TickSummary {
    pub jobs_checked: usize,
    pub jobs_due: usize,
    pub jobs_idle: usize,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// TickState — the on-disk sidecar record
// ---------------------------------------------------------------------------

/// Persisted tick-loop heartbeat. `#[serde(default)]` on the container is
/// load-bearing: Plan 02 adds `last_boot_at` and `backlog` to this same
/// struct, and older/newer files must still round-trip.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TickState {
    pub last_tick_at: Option<DateTime<Utc>>,
    pub last_tick_pid: Option<u32>,
    pub jobs_checked: usize,
    pub jobs_due: usize,
    pub jobs_idle: usize,
    pub last_tick_error: Option<String>,
    /// Set by [`record_backlog_at`]: when the last startup backlog pass ran.
    pub last_boot_at: Option<DateTime<Utc>>,
    /// Set by [`record_backlog_at`]: events from the most recent pass only (replaced, not appended).
    pub backlog: Vec<BacklogEvent>,
    /// Set by [`record_grace_skips_at`]: bounded, APPENDED runtime grace-skip history.
    pub recent_skips: Vec<BacklogEvent>,
}

// `recent_skips` (Plan 04): a bounded, APPENDED history of runtime
// grace-skip events from `JobStore::get_due_jobs`'s own independent stale
// fast-forward — distinct from `backlog`, which is the per-boot
// `fast_forward_backlog` record set by `record_backlog_at`. Capped at
// `GRACE_SKIP_HISTORY_MAX` entries (oldest dropped first, newest last) by
// `record_grace_skips_at`. Kept in its own field so a per-tick runtime skip
// can never clobber the per-boot backlog record and vice versa.

// ---------------------------------------------------------------------------
// Backlog surface (Phase 49.2 Plan 02, D-05 revised / D-03)
// ---------------------------------------------------------------------------

/// What happened to a single job's missed occurrence during a startup
/// backlog pass ([`ironhermes_gateway`]'s `fast_forward_backlog`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BacklogAction {
    /// The missed occurrence was outside the configured lookback window;
    /// `next_run_at` was rolled forward to the next regular occurrence, same
    /// as pre-phase behavior.
    Skipped,
    /// The missed occurrence fell within the configured lookback window and
    /// will fire once on the next tick (`next_run_at` was set to "now").
    CaughtUp,
    /// A past-due `Once` job whose `next_run_at` was cleared because it has
    /// no next occurrence to roll forward to.
    Dropped,
}

/// One job's outcome from a single backlog pass. Carries only identifiers
/// and timestamps — never prompt text, job output, or delivery targets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacklogEvent {
    pub job_id: String,
    pub job_name: String,
    pub missed_at: DateTime<Utc>,
    pub action: BacklogAction,
    pub rescheduled_to: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

/// Resolve the sidecar's path within a given cron directory.
pub fn tick_state_path(dir: &Path) -> PathBuf {
    dir.join(TICK_STATE_FILE)
}

// ---------------------------------------------------------------------------
// Read side
// ---------------------------------------------------------------------------

/// Read and parse the tick heartbeat at a caller-specified cron directory.
///
/// Never propagates an error: a missing file, an unreadable file, or
/// malformed JSON all resolve to `None`. An unreadable heartbeat must never
/// break `cron status` or `cron run`.
pub fn read_tick_state_at(dir: &Path) -> Option<TickState> {
    let path = tick_state_path(dir);
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// [`read_tick_state_at`] rooted at the default hermes home's `cron` directory.
pub fn read_tick_state() -> Option<TickState> {
    read_tick_state_at(&get_hermes_home().join("cron"))
}

// ---------------------------------------------------------------------------
// Write side
// ---------------------------------------------------------------------------

/// Record a tick's outcome to the sidecar at a caller-specified cron directory.
///
/// Read-modify-write: the existing state (defaulting to `TickState::default()`
/// when absent/unreadable) is loaded first, only the tick-owned fields are
/// overwritten (`last_tick_at`, `last_tick_pid`, the three counts,
/// `last_tick_error`), and every other field is left untouched. This shape is
/// required because Plan 02 writes a different field set (`last_boot_at`,
/// `backlog`) to the same file via a separate recorder.
///
/// Persists using the exact durability idiom `JobStore::save` uses:
/// `create_dir_all` the directory, serialize pretty, write a `.tmp` sibling,
/// flush, `sync_all`, `rename` into place, then (Unix) restrict to `0o600`.
pub fn record_tick_at(dir: &Path, summary: TickSummary) -> Result<()> {
    let mut state = read_tick_state_at(dir).unwrap_or_default();

    state.last_tick_at = Some(Utc::now());
    state.last_tick_pid = Some(std::process::id());
    state.jobs_checked = summary.jobs_checked;
    state.jobs_due = summary.jobs_due;
    state.jobs_idle = summary.jobs_idle;
    state.last_tick_error = summary.error;

    persist_tick_state(dir, &state)
}

/// [`record_tick_at`] rooted at the default hermes home's `cron` directory.
pub fn record_tick(summary: TickSummary) -> Result<()> {
    record_tick_at(&get_hermes_home().join("cron"), summary)
}

/// Record a startup backlog pass's outcome to the sidecar at a
/// caller-specified cron directory.
///
/// Read-modify-write, same shape as [`record_tick_at`]: the existing state is
/// loaded first, `last_boot_at` is set to now, `backlog` is REPLACED
/// wholesale with `events` (never appended — the record always describes the
/// most recent backlog pass only), and every tick-owned field
/// (`last_tick_at`, `last_tick_pid`, the counts, `last_tick_error`) is left
/// untouched.
pub fn record_backlog_at(dir: &Path, events: Vec<BacklogEvent>) -> Result<()> {
    let mut state = read_tick_state_at(dir).unwrap_or_default();

    state.last_boot_at = Some(Utc::now());
    state.backlog = events;

    persist_tick_state(dir, &state)
}

/// [`record_backlog_at`] rooted at the default hermes home's `cron` directory.
pub fn record_backlog(events: Vec<BacklogEvent>) -> Result<()> {
    record_backlog_at(&get_hermes_home().join("cron"), events)
}

/// Record runtime grace-skip events (Plan 04) to the sidecar at a
/// caller-specified cron directory.
///
/// Read-modify-write, same durability idiom as [`record_tick_at`] and
/// [`record_backlog_at`], but a different write shape: `events` are
/// APPENDED to `recent_skips` (never replaced — this is a rolling history,
/// not a per-pass snapshot like `backlog`), then the front is truncated so
/// at most [`GRACE_SKIP_HISTORY_MAX`] entries remain with the newest last.
/// `backlog` and every tick-owned field are left untouched.
///
/// A no-op (no read, no write) when `events` is empty, so the common
/// zero-skip tick performs no extra IO.
pub fn record_grace_skips_at(dir: &Path, events: Vec<BacklogEvent>) -> Result<()> {
    if events.is_empty() {
        return Ok(());
    }

    let mut state = read_tick_state_at(dir).unwrap_or_default();

    state.recent_skips.extend(events);
    if state.recent_skips.len() > GRACE_SKIP_HISTORY_MAX {
        let excess = state.recent_skips.len() - GRACE_SKIP_HISTORY_MAX;
        state.recent_skips.drain(0..excess);
    }

    persist_tick_state(dir, &state)
}

/// [`record_grace_skips_at`] rooted at the default hermes home's `cron` directory.
pub fn record_grace_skips(events: Vec<BacklogEvent>) -> Result<()> {
    record_grace_skips_at(&get_hermes_home().join("cron"), events)
}

/// Shared atomic tmp+rename+`0o600` persist body used by both
/// [`record_tick_at`] and [`record_backlog_at`] so there is exactly one place
/// that knows the sidecar's durability idiom (matches `JobStore::save`).
fn persist_tick_state(dir: &Path, state: &TickState) -> Result<()> {
    fs::create_dir_all(dir)
        .with_context(|| format!("failed to create cron dir: {}", dir.display()))?;

    let path = tick_state_path(dir);
    // IN-01: PID-qualified tmp filename so a concurrent writer's
    // `File::create` on the same path cannot truncate this write's
    // already-open descriptor mid-write. Each writer still targets its own
    // unique tmp file; only the final `rename` into `path` is a point of
    // cross-process contention, and that step is atomic on POSIX.
    let tmp_path = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let json = serde_json::to_string_pretty(state).context("failed to serialise tick state")?;
    {
        use std::io::Write;
        let mut f = fs::File::create(&tmp_path)
            .with_context(|| format!("failed to create temp file: {}", tmp_path.display()))?;
        f.write_all(json.as_bytes())
            .with_context(|| format!("failed to write temp file: {}", tmp_path.display()))?;
        f.flush()?;
        f.sync_all()?;
    }
    fs::rename(&tmp_path, &path).with_context(|| {
        format!(
            "failed to rename {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Staleness check
// ---------------------------------------------------------------------------

/// `true` when `state` is `None`, when `last_tick_at` is `None`, or when
/// `now - last_tick_at` exceeds [`TICK_STALE_SECONDS`]; `false` otherwise.
pub fn is_tick_stale(state: Option<&TickState>, now: DateTime<Utc>) -> bool {
    let Some(state) = state else {
        return true;
    };
    let Some(last_tick_at) = state.last_tick_at else {
        return true;
    };
    (now - last_tick_at).num_seconds() > TICK_STALE_SECONDS
}

/// Best-effort heartbeat write helper for tick-loop call sites: logs a
/// warning and swallows any error so a full/read-only disk can never fail
/// the tick loop itself.
pub fn record_tick_best_effort(dir: &Path, summary: TickSummary) {
    if let Err(e) = record_tick_at(dir, summary) {
        warn!("failed to record tick heartbeat: {}", e);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use tempfile::TempDir;

    #[test]
    fn heartbeat_record_tick_round_trips_tick_state() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        record_tick_at(
            &cron_dir,
            TickSummary {
                jobs_checked: 3,
                jobs_due: 1,
                jobs_idle: 2,
                error: None,
            },
        )
        .expect("record tick");

        let state = read_tick_state_at(&cron_dir).expect("state present");
        assert_eq!(state.jobs_checked, 3);
        assert_eq!(state.jobs_due, 1);
        assert_eq!(state.jobs_idle, 2);
        let last_tick_at = state.last_tick_at.expect("last_tick_at set");
        let diff = (Utc::now() - last_tick_at).abs();
        assert!(
            diff < Duration::seconds(5),
            "last_tick_at should be within 5s of now, got diff={}s",
            diff.num_seconds()
        );
    }

    #[test]
    fn heartbeat_read_missing_tick_state_returns_none() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");
        assert!(read_tick_state_at(&cron_dir).is_none());

        // Malformed JSON also resolves to None, not an error.
        fs::create_dir_all(&cron_dir).expect("create cron dir");
        fs::write(tick_state_path(&cron_dir), "{ not valid json").expect("write garbage");
        assert!(read_tick_state_at(&cron_dir).is_none());
    }

    #[test]
    fn is_tick_stale_true_when_older_than_threshold() {
        let old = TickState {
            last_tick_at: Some(Utc::now() - Duration::minutes(10)),
            ..Default::default()
        };
        assert!(is_tick_stale(Some(&old), Utc::now()));
        assert!(is_tick_stale(None, Utc::now()));

        let never_ticked = TickState {
            last_tick_at: None,
            ..Default::default()
        };
        assert!(is_tick_stale(Some(&never_ticked), Utc::now()));
    }

    #[test]
    fn is_tick_stale_false_when_recent() {
        let fresh = TickState {
            last_tick_at: Some(Utc::now() - Duration::seconds(5)),
            ..Default::default()
        };
        assert!(!is_tick_stale(Some(&fresh), Utc::now()));
    }

    // =========================================================================
    // Phase 49.2 Plan 02: record_backlog_at / BacklogEvent / BacklogAction
    // =========================================================================

    #[test]
    fn heartbeat_record_backlog_preserves_tick_fields() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        record_tick_at(
            &cron_dir,
            TickSummary {
                jobs_checked: 5,
                jobs_due: 1,
                jobs_idle: 4,
                error: None,
            },
        )
        .expect("record tick");

        record_backlog_at(
            &cron_dir,
            vec![BacklogEvent {
                job_id: "job-1".to_string(),
                job_name: "Daily Briefing".to_string(),
                missed_at: Utc::now() - Duration::minutes(30),
                action: BacklogAction::CaughtUp,
                rescheduled_to: Some(Utc::now()),
            }],
        )
        .expect("record backlog");

        let state = read_tick_state_at(&cron_dir).expect("state present");
        assert_eq!(state.jobs_checked, 5, "tick-owned fields must survive a backlog write");
        assert!(state.last_tick_at.is_some(), "last_tick_at must survive a backlog write");
        assert!(state.last_boot_at.is_some(), "last_boot_at must be set by record_backlog_at");
        assert_eq!(state.backlog.len(), 1);
        assert_eq!(state.backlog[0].action, BacklogAction::CaughtUp);
    }

    #[test]
    fn record_backlog_at_replaces_rather_than_appends() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        record_backlog_at(
            &cron_dir,
            vec![BacklogEvent {
                job_id: "job-1".to_string(),
                job_name: "First Pass".to_string(),
                missed_at: Utc::now(),
                action: BacklogAction::Skipped,
                rescheduled_to: Some(Utc::now()),
            }],
        )
        .expect("first backlog record");

        record_backlog_at(
            &cron_dir,
            vec![BacklogEvent {
                job_id: "job-2".to_string(),
                job_name: "Second Pass".to_string(),
                missed_at: Utc::now(),
                action: BacklogAction::CaughtUp,
                rescheduled_to: Some(Utc::now()),
            }],
        )
        .expect("second backlog record");

        let state = read_tick_state_at(&cron_dir).expect("state present");
        assert_eq!(
            state.backlog.len(),
            1,
            "the second record_backlog_at call must replace, not append"
        );
        assert_eq!(state.backlog[0].job_id, "job-2");
    }

    #[test]
    fn read_tick_state_at_tolerates_pre_plan_02_file() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");
        fs::create_dir_all(&cron_dir).expect("create cron dir");

        // A tick_state.json written before last_boot_at/backlog existed
        // (Plan 01 shape only).
        let legacy = r#"{
            "last_tick_at": "2026-08-26T12:00:00Z",
            "last_tick_pid": 1234,
            "jobs_checked": 3,
            "jobs_due": 0,
            "jobs_idle": 3,
            "last_tick_error": null
        }"#;
        fs::write(tick_state_path(&cron_dir), legacy).expect("write legacy file");

        let state = read_tick_state_at(&cron_dir).expect("legacy file still parses");
        assert_eq!(state.jobs_checked, 3);
        assert!(state.backlog.is_empty());
        assert!(state.last_boot_at.is_none());
    }

    // =========================================================================
    // Phase 49.2 Plan 04: record_grace_skips_at / TickState::recent_skips
    // =========================================================================

    fn skip_event(job_id: &str, job_name: &str) -> BacklogEvent {
        BacklogEvent {
            job_id: job_id.to_string(),
            job_name: job_name.to_string(),
            missed_at: Utc::now(),
            action: BacklogAction::Skipped,
            rescheduled_to: Some(Utc::now()),
        }
    }

    #[test]
    fn record_grace_skips_appends_and_caps() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        for i in 0..25 {
            record_grace_skips_at(&cron_dir, vec![skip_event(&format!("job-{i}"), &format!("Job {i}"))])
                .expect("record grace skip");
        }

        let state = read_tick_state_at(&cron_dir).expect("state present");
        assert_eq!(
            state.recent_skips.len(),
            GRACE_SKIP_HISTORY_MAX,
            "recent_skips must be capped at GRACE_SKIP_HISTORY_MAX"
        );
        assert_eq!(
            state.recent_skips.last().expect("non-empty").job_id,
            "job-24",
            "the last entry must be the most recently recorded one"
        );
        assert_eq!(
            state.recent_skips.first().expect("non-empty").job_id,
            "job-5",
            "oldest entries must be dropped from the front"
        );
    }

    #[test]
    fn record_grace_skips_does_not_clobber_boot_backlog() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        record_backlog_at(
            &cron_dir,
            vec![BacklogEvent {
                job_id: "boot-job".to_string(),
                job_name: "Boot Caught Up".to_string(),
                missed_at: Utc::now(),
                action: BacklogAction::CaughtUp,
                rescheduled_to: Some(Utc::now()),
            }],
        )
        .expect("record backlog");

        record_grace_skips_at(&cron_dir, vec![skip_event("runtime-job", "Runtime Skip")])
            .expect("record grace skip");

        let state = read_tick_state_at(&cron_dir).expect("state present");
        assert_eq!(state.backlog.len(), 1, "boot backlog must survive a grace-skip write");
        assert_eq!(state.backlog[0].job_id, "boot-job");
        assert_eq!(state.recent_skips.len(), 1);
        assert_eq!(state.recent_skips[0].job_id, "runtime-job");
    }

    #[test]
    fn record_grace_skips_at_empty_is_noop() {
        let dir = TempDir::new().expect("tmpdir");
        let cron_dir = dir.path().join("cron");

        record_grace_skips_at(&cron_dir, vec![]).expect("no-op on empty events");

        assert!(
            !tick_state_path(&cron_dir).exists(),
            "an empty events vec must neither create nor rewrite the file"
        );
    }
}
