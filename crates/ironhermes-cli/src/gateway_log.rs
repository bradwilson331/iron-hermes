//! Gateway log file (Phase 54.8 Plan 04, D-08).
//!
//! # Why this exists
//!
//! Only supervised installs (the container entrypoint, systemd, launchd, the
//! cron watchdog) captured gateway output, because each of them redirects the
//! process's stdout to the canonical `$IRONHERMES_HOME/logs/gateway.log`
//! (Phase 48.3 D-06). A plain `ironhermes gateway` in a terminal logged to the
//! terminal only, which is why the Telegram stall was undiagnosable outside the
//! container. D-08 gives every install a persistent log.
//!
//! # Relationship to the 48.3 canonical file
//!
//! The process writes `logs/gateway.log.YYYY-MM-DD` (UTC date, the same naming
//! `tracing_appender::rolling::daily` gives `web.log` / `tui.log`) only when its
//! stdout is NOT already the canonical `logs/gateway.log`
//! ([`should_install_file_layer`]). Supervised installs already have exactly one
//! log at the canonical path, and the operator docs (`tail -f logs/gateway.log`)
//! stay true for them. `ironhermes doctor` reports whichever of the two is
//! newest.
//!
//! # Why a small custom writer
//!
//! `tracing_appender::rolling::daily` cannot set a file mode, and gateway output
//! can quote tokens in error paths (the reason `docker/web-entrypoint.sh`
//! already chmods its file 600). This writer keeps the appender's file naming
//! and its `non_blocking` worker, and adds `OpenOptionsExt::mode(0o600)` plus
//! retention. A process-wide umask change was rejected: it would also change the
//! mode of agent-created workspace files.
//!
//! # Symlinks and the accepted check-then-open window
//!
//! `libc` is not an `ironhermes-cli` dependency and no new dependency is
//! allowed, so there is no `O_NOFOLLOW`. Instead the writer runs
//! `std::fs::symlink_metadata` on the day file before opening it and refuses
//! anything that is not a regular file (a symlink, a directory, a FIFO). The
//! remaining check-then-open window is accepted at L1: a logs directory this
//! writer creates is 0700 and owner-only, so only the owner can plant a file
//! there.
//!
//! # Retention
//!
//! The process keeps the [`GATEWAY_LOG_KEEP`] most recent dated files. That
//! bounds the NUMBER of dated files, not the bytes written within one day, and
//! the supervised canonical file is rotated by its supervisor, outside this
//! process (T-54.8-46, accepted). The canonical `gateway.log` is never touched.

use std::fs::{DirBuilder, File, OpenOptions};
use std::io;
use std::os::fd::AsFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::NaiveDate;

/// File-name prefix shared by the canonical file (`gateway.log`) and the dated
/// files (`gateway.log.YYYY-MM-DD`).
pub const GATEWAY_LOG_PREFIX: &str = "gateway.log";

/// How many dated files the process keeps (about two weeks).
pub const GATEWAY_LOG_KEEP: usize = 14;

type Clock = Box<dyn Fn() -> NaiveDate + Send + Sync>;

struct State {
    date: NaiveDate,
    file: File,
}

/// A daily-rolling, owner-only log file writer.
///
/// Every file it creates is mode 0600 (an existing file is tightened on open)
/// and a logs directory it creates is 0700. A day file that is a symlink or not
/// a regular file is refused.
pub struct DailyRollingFile {
    dir: PathBuf,
    clock: Clock,
    state: Mutex<State>,
}

fn day_file_name(date: NaiveDate) -> String {
    format!("{GATEWAY_LOG_PREFIX}.{}", date.format("%Y-%m-%d"))
}

/// Parse `gateway.log.YYYY-MM-DD` into its date. Anything else — the canonical
/// `gateway.log`, `gateway.log.pre-48.3`, a non-padded date — is `None`.
pub fn parse_dated_log_name(name: &str) -> Option<NaiveDate> {
    let suffix = name.strip_prefix(GATEWAY_LOG_PREFIX)?.strip_prefix('.')?;
    let date = NaiveDate::parse_from_str(suffix, "%Y-%m-%d").ok()?;
    // Round-trip so only the canonical zero-padded spelling counts.
    (date.format("%Y-%m-%d").to_string() == suffix).then_some(date)
}

/// Open (creating if needed) the owner-only file for `date` under `dir`.
fn open_day_file(dir: &Path, date: NaiveDate) -> io::Result<File> {
    // Create the directory 0700 if absent; never chmod an existing one.
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let name = day_file_name(date);
    let path = dir.join(&name);
    // Never follow or write through anything that is not a regular file.
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing non-regular gateway log file {name}"),
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)?;
    // A pre-existing wider file is tightened.
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Best-effort retention: keep the `keep` newest `gateway.log.YYYY-MM-DD`
/// regular files, remove the rest. Never touches the canonical `gateway.log`
/// or any entry whose suffix is not a date.
fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut dated: Vec<(NaiveDate, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let date = parse_dated_log_name(&name)?;
            let is_regular = std::fs::symlink_metadata(entry.path())
                .map(|m| m.file_type().is_file())
                .unwrap_or(false);
            is_regular.then(|| (date, entry.path()))
        })
        .collect();
    dated.sort_by_key(|(date, _)| std::cmp::Reverse(*date));
    for (_, path) in dated.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

impl DailyRollingFile {
    /// Writer for `log_dir` using the UTC clock.
    pub fn new(log_dir: &Path) -> io::Result<Self> {
        Self::with_clock(log_dir, Box::new(|| chrono::Utc::now().date_naive()))
    }

    /// Writer with an injected clock (tests drive the midnight rollover).
    pub fn with_clock(log_dir: &Path, clock: Clock) -> io::Result<Self> {
        let date = clock();
        let file = open_day_file(log_dir, date)?;
        // A gateway that is restarted daily never sees a midnight rollover, so
        // retention also runs at open.
        prune(log_dir, GATEWAY_LOG_KEEP);
        Ok(Self {
            dir: log_dir.to_path_buf(),
            clock,
            state: Mutex::new(State { date, file }),
        })
    }
}

impl std::io::Write for DailyRollingFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let today = (self.clock)();
        if today != st.date {
            // Open the new day's file first. On failure the state stays on the
            // old date, so the next write retries; the writer never panics.
            let file = open_day_file(&self.dir, today)?;
            st.file = file;
            st.date = today;
            prune(&self.dir, GATEWAY_LOG_KEEP);
        }
        st.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut st = self.state.lock().unwrap_or_else(|e| e.into_inner());
        st.file.flush()
    }
}

/// Build the non-blocking file writer for `home`'s `logs/` directory.
///
/// The returned guard must be held until process exit: dropping it flushes the
/// worker thread's buffered lines.
pub fn build_file_writer(
    home: &Path,
) -> io::Result<(
    tracing_appender::non_blocking::NonBlocking,
    tracing_appender::non_blocking::WorkerGuard,
)> {
    Ok(tracing_appender::non_blocking(DailyRollingFile::new(
        &home.join("logs"),
    )?))
}

/// Identity of an open file or a path: the (device, inode) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
}

impl FileIdentity {
    /// Identity of an already-open file.
    pub fn of_file(f: &File) -> io::Result<Self> {
        let m = f.metadata()?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }

    /// Identity of the file a path resolves to (follows symlinks).
    pub fn of_path(p: &Path) -> io::Result<Self> {
        let m = std::fs::metadata(p)?;
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
        })
    }
}

/// Identity of fd 1, but only when it is a regular file (a supervisor's
/// redirect). A TTY, a pipe or a socket is `None`.
pub fn stdout_identity() -> Option<FileIdentity> {
    let owned = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
    let file = File::from(owned);
    let meta = file.metadata().ok()?;
    meta.is_file().then(|| FileIdentity {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

/// The Phase 48.3 canonical gateway log: `<home>/logs/gateway.log`. The systemd
/// unit, the launchd plist, `gateway-watchdog.sh` and `docker/web-entrypoint.sh`
/// all redirect stdout there.
pub fn canonical_gateway_log(home: &Path) -> PathBuf {
    home.join("logs").join(GATEWAY_LOG_PREFIX)
}

/// Today's (UTC) dated gateway log path: `<home>/logs/gateway.log.YYYY-MM-DD`.
/// What the startup line names when the process writes the daily file.
pub fn current_day_log_path(home: &Path) -> PathBuf {
    home.join("logs")
        .join(day_file_name(chrono::Utc::now().date_naive()))
}

/// Whether the process should add its own daily file layer.
///
/// False only when stdout is already the canonical log (a supervisor redirect):
/// writing the daily file as well would double-log. An unreadable canonical
/// path means install.
pub fn should_install_file_layer(stdout: Option<FileIdentity>, canonical: &Path) -> bool {
    match stdout {
        None => true,
        Some(out) => match FileIdentity::of_path(canonical) {
            Ok(canon) => canon != out,
            Err(_) => true,
        },
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex as StdMutex};

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// A clock the test can move.
    fn movable_clock(start: NaiveDate) -> (Arc<StdMutex<NaiveDate>>, Clock) {
        let cell = Arc::new(StdMutex::new(start));
        let handle = cell.clone();
        (cell, Box::new(move || *handle.lock().unwrap()))
    }

    fn mode_of(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn rotates_when_date_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        let (cell, clock) = movable_clock(date(2026, 10, 17));
        let mut w = DailyRollingFile::with_clock(&logs, clock).unwrap();
        w.write_all(b"day-one\n").unwrap();
        *cell.lock().unwrap() = date(2026, 10, 18);
        w.write_all(b"day-two\n").unwrap();
        w.flush().unwrap();

        let one = std::fs::read_to_string(logs.join("gateway.log.2026-10-17")).unwrap();
        let two = std::fs::read_to_string(logs.join("gateway.log.2026-10-18")).unwrap();
        assert_eq!(one, "day-one\n");
        assert_eq!(two, "day-two\n");
    }

    #[test]
    fn keeps_only_newest_14() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        for day in 1..=16 {
            std::fs::write(logs.join(format!("gateway.log.2026-09-{day:02}")), b"old\n").unwrap();
        }
        std::fs::write(logs.join("gateway.log"), b"canonical\n").unwrap();
        std::fs::write(logs.join("gateway.log.pre-48.3"), b"keep\n").unwrap();

        let (cell, clock) = movable_clock(date(2026, 10, 17));
        let mut w = DailyRollingFile::with_clock(&logs, clock).unwrap();
        *cell.lock().unwrap() = date(2026, 10, 18);
        w.write_all(b"rotate\n").unwrap();
        w.flush().unwrap();

        let mut dated: Vec<NaiveDate> = std::fs::read_dir(&logs)
            .unwrap()
            .filter_map(|e| parse_dated_log_name(&e.unwrap().file_name().to_string_lossy()))
            .collect();
        dated.sort();
        assert_eq!(dated.len(), GATEWAY_LOG_KEEP, "dated files: {dated:?}");
        // The two new days plus the 12 newest old ones (09-05 ..= 09-16).
        assert_eq!(dated.last(), Some(&date(2026, 10, 18)));
        assert_eq!(dated.first(), Some(&date(2026, 9, 5)));
        assert_eq!(
            std::fs::read_to_string(logs.join("gateway.log")).unwrap(),
            "canonical\n",
            "the canonical file is never pruned"
        );
        assert!(
            logs.join("gateway.log.pre-48.3").exists(),
            "a non-date suffix is never pruned"
        );
    }

    #[test]
    fn reopen_tightens_existing_file_to_0600() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let day = logs.join("gateway.log.2026-10-17");
        std::fs::write(&day, b"pre\n").unwrap();
        std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode_of(&day), 0o644);

        let (_cell, clock) = movable_clock(date(2026, 10, 17));
        let _w = DailyRollingFile::with_clock(&logs, clock).unwrap();
        assert_eq!(mode_of(&day), 0o600);
        // An existing directory is never chmod'ed.
        assert_ne!(mode_of(&logs), 0o600);
    }

    #[test]
    fn refuses_symlinked_day_file() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, b"outside\n").unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::os::unix::fs::symlink(&outside, logs.join("gateway.log.2026-10-17")).unwrap();

        let (_cell, clock) = movable_clock(date(2026, 10, 17));
        let res = DailyRollingFile::with_clock(&logs, clock);
        assert!(res.is_err(), "a symlinked day file must be refused");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "outside\n");
        assert_eq!(mode_of(&outside), 0o644);
    }

    #[test]
    fn refuses_non_regular_day_file() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        std::fs::create_dir_all(logs.join("gateway.log.2026-10-17")).unwrap();

        let (_cell, clock) = movable_clock(date(2026, 10, 17));
        assert!(DailyRollingFile::with_clock(&logs, clock).is_err());
    }

    #[test]
    fn rotation_into_unwritable_dir_returns_err_without_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let logs = tmp.path().join("logs");
        let (cell, clock) = movable_clock(date(2026, 10, 17));
        let mut w = DailyRollingFile::with_clock(&logs, clock).unwrap();
        w.write_all(b"day-one\n").unwrap();

        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o500)).unwrap();
        // A superuser ignores directory modes; the failure path cannot be
        // driven there.
        let probe = logs.join(".probe");
        let is_superuser = std::fs::write(&probe, b"x").is_ok();
        let _ = std::fs::remove_file(&probe);

        *cell.lock().unwrap() = date(2026, 10, 18);
        let res = w.write(b"day-two\n");
        if !is_superuser {
            assert!(res.is_err(), "rotation into a read-only dir must be an Err");
        }

        std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
        w.write_all(b"day-two\n").unwrap();
        w.flush().unwrap();
        let two = std::fs::read_to_string(logs.join("gateway.log.2026-10-18")).unwrap();
        assert!(two.ends_with("day-two\n"), "day-two file: {two:?}");
    }

    #[test]
    fn skip_when_stdout_is_the_canonical_log() {
        let tmp = tempfile::tempdir().unwrap();
        let canonical = canonical_gateway_log(tmp.path());
        std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
        let f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&canonical)
            .unwrap();
        let canon_id = FileIdentity::of_file(&f).unwrap();
        assert!(!should_install_file_layer(Some(canon_id), &canonical));

        let other = tempfile::NamedTempFile::new().unwrap();
        let other_id = FileIdentity::of_file(other.as_file()).unwrap();
        assert!(should_install_file_layer(Some(other_id), &canonical));

        // stdout is a TTY or a pipe.
        assert!(should_install_file_layer(None, &canonical));

        // An unreadable canonical path means install.
        let missing = tmp.path().join("logs").join("absent.log");
        assert!(should_install_file_layer(Some(canon_id), &missing));
    }

    #[test]
    fn parse_dated_log_name_accepts_only_padded_dates() {
        assert_eq!(
            parse_dated_log_name("gateway.log.2026-10-17"),
            Some(date(2026, 10, 17))
        );
        assert_eq!(parse_dated_log_name("gateway.log"), None);
        assert_eq!(parse_dated_log_name("gateway.log.pre-48.3"), None);
        assert_eq!(parse_dated_log_name("gateway.log.2026-1-7"), None);
        assert_eq!(parse_dated_log_name("web.log.2026-10-17"), None);
    }
}
