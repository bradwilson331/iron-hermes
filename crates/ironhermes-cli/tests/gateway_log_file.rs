//! Phase 54.8 Plan 04 (D-08): the gateway log file.
//!
//! `gateway_event_reaches_daily_file` drives the writer + `tracing_appender`
//! non-blocking worker + an fmt layer end to end. The real-binary cases are
//! added alongside in the same file.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ironhermes_cli::gateway_log;
use tracing_subscriber::prelude::*;

const STARTUP_LINE: &str = "gateway logging configured";

/// A config the preflight accepts (an empty file and a bare `provider:` block are
/// reported invalid) that names a provider whose key is absent from the cleared
/// environment, so the gateway stops in `build_client` after logging is
/// configured. Same shape `preflight_provider_key_env.rs` uses.
fn write_minimal_config(home: &std::path::Path) {
    let cfg = "model:\n  provider: groq\n  default: groq/llama-3.3-70b\nproviders:\n  groq:\n    api_key_env: GROQ_API_KEY\n";
    std::fs::write(home.join("config.yaml"), cfg).expect("minimal config");
}

/// Spawn `ironhermes gateway --non-interactive` against `home` with a cleared
/// environment (no developer provider key leaks in, so the run is deterministic:
/// `build_client` finds no provider and the process returns on its own — after
/// the subscriber is installed and the startup line is emitted).
fn spawn_gateway(home: &std::path::Path, stdout: Stdio, stderr: Stdio) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ironhermes"))
        .args(["gateway", "--non-interactive"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("IRONHERMES_HOME", home)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .expect("spawn ironhermes gateway")
}

fn wait_for(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let start = Instant::now();
    while start.elapsed() < limit {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// Wait for a normal exit (which is what proves the guard flushed). After 30 s,
/// send SIGTERM through `kill` and wait again. Never SIGKILL, which would
/// discard buffered lines.
fn wait_for_exit(child: &mut Child) {
    if wait_for(child, Duration::from_secs(30)).is_some() {
        return;
    }
    let _ = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status();
    if wait_for(child, Duration::from_secs(30)).is_none() {
        // Last-resort cleanup so a failed run does not leak a process.
        let _ = child.kill();
        panic!("ironhermes gateway did not exit after SIGTERM");
    }
}

fn dated_logs(logs: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out: Vec<_> = std::fs::read_dir(logs)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .and_then(gateway_log::parse_dated_log_name)
                        .is_some()
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

#[test]
fn unsupervised_gateway_writes_dated_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();
    write_minimal_config(home);

    let before = utc_today();
    let mut child = spawn_gateway(home, Stdio::piped(), Stdio::piped());
    let mut stdout = child.stdout.take().expect("stdout pipe");
    let mut stderr = child.stderr.take().expect("stderr pipe");
    let out_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    wait_for_exit(&mut child);
    let out = out_reader.join().unwrap();
    let err = err_reader.join().unwrap();
    let after = utc_today();

    let dated = dated_logs(&home.join("logs"));
    assert_eq!(
        dated.len(),
        1,
        "exactly one dated gateway log: {dated:?}\nstdout:\n{out}\nstderr:\n{err}"
    );
    let name = dated[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name == format!("gateway.log.{before}") || name == format!("gateway.log.{after}"),
        "unexpected dated file {name}"
    );
    assert_eq!(
        std::fs::metadata(&dated[0]).unwrap().permissions().mode() & 0o777,
        0o600,
        "the live dated log must be owner-only"
    );
    let body = std::fs::read_to_string(&dated[0]).unwrap();
    assert_eq!(
        body.matches(STARTUP_LINE).count(),
        1,
        "file must hold the startup line exactly once:\n{body}"
    );
    assert!(
        out.contains(STARTUP_LINE),
        "stdout is kept alongside the file:\nstdout:\n{out}\nstderr:\n{err}"
    );
}

#[test]
fn supervised_gateway_does_not_double_log() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();
    write_minimal_config(home);
    let logs = home.join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    std::fs::set_permissions(&logs, std::fs::Permissions::from_mode(0o700)).unwrap();
    let canonical = logs.join("gateway.log");
    let out_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&canonical)
        .unwrap();
    let err_file = out_file.try_clone().unwrap();

    let mut child = spawn_gateway(home, Stdio::from(out_file), Stdio::from(err_file));
    wait_for_exit(&mut child);

    let dated = dated_logs(&logs);
    assert!(
        dated.is_empty(),
        "a supervised gateway must not write a daily file: {dated:?}"
    );
    let body = std::fs::read_to_string(&canonical).unwrap();
    assert_eq!(
        body.matches(STARTUP_LINE).count(),
        1,
        "the canonical file must hold the startup line exactly once:\n{body}"
    );
}

fn utc_today() -> String {
    chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

#[test]
fn gateway_event_reaches_daily_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();

    let before = utc_today();
    let (nb, guard) = gateway_log::build_file_writer(home).expect("build_file_writer");
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_writer(nb)
            .with_ansi(false),
    );
    tracing::subscriber::with_default(subscriber, || {
        tracing::info!("gateway-log-probe-548");
    });
    // Dropping the guard flushes the non-blocking worker.
    drop(guard);
    let after = utc_today();

    let logs = home.join("logs");
    let mut dated: Vec<_> = std::fs::read_dir(&logs)
        .expect("logs dir exists")
        .map(|e| e.expect("entry").path())
        .collect();
    dated.sort();
    assert_eq!(dated.len(), 1, "exactly one file under logs/: {dated:?}");
    let name = dated[0].file_name().unwrap().to_string_lossy().to_string();
    let ok = name == format!("gateway.log.{before}") || name == format!("gateway.log.{after}");
    assert!(ok, "unexpected file name {name}");

    let body = std::fs::read_to_string(&dated[0]).expect("read log");
    assert!(body.contains("gateway-log-probe-548"), "body: {body}");

    let file_mode = std::fs::metadata(&dated[0]).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "log file mode");
    let dir_mode = std::fs::metadata(&logs).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "logs dir created by the writer");
}
