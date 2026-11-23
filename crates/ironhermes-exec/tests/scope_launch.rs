//! Phase 58.1 Plan 01 (D-04/D-11/D-12) — detached terminal commands are
//! launched through the systemd scope launcher.
//!
//! Drives `LocalEnvironment` through `backend::execute` (the one spawn point
//! every agent surface shares) with a fake launcher standing in for
//! `systemd-run`, so the contract is checked on any unix host, including
//! macOS where systemd does not exist.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use ironhermes_exec::backend::local::LocalEnvironment;
use ironhermes_exec::backend::{Session, execute};

/// Serialises tests: they all mutate process-wide env vars.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const DETACH_CMD: &str = "nohup sleep 1 >/dev/null 2>&1 & disown; echo launched";

/// Writes a fake `systemd-run`: logs every argv element on its own line to a
/// path baked into the script text (the scoped child gets a scrubbed env, so
/// the path cannot travel by env var), drops args up to and including the
/// first `--`, then execs the remainder.
fn write_fake_launcher(dir: &Path) -> (PathBuf, PathBuf) {
    let script = dir.join("fake-systemd-run");
    let log = dir.join("fake-systemd-run.log");
    let body = format!(
        "#!/usr/bin/env bash\n\
         for a in \"$@\"; do printf '%s\\n' \"$a\" >> '{}'; done\n\
         while [ \"$#\" -gt 0 ]; do\n\
           if [ \"$1\" = \"--\" ]; then shift; break; fi\n\
           shift\n\
         done\n\
         exec \"$@\"\n",
        log.display()
    );
    std::fs::write(&script, body).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, log)
}

async fn run(cmd: &str, cwd: &str) -> ironhermes_exec::backend::ExecResult {
    let t = Duration::from_secs(5);
    let env = LocalEnvironment::new(cwd, t, vec![]);
    let mut sess = Session::new(cwd);
    execute(&env, &mut sess, cmd, cwd, t, None)
        .await
        .expect("execute should succeed")
}

fn read_log(log: &Path) -> String {
    std::fs::read_to_string(log).unwrap_or_default()
}

#[tokio::test]
async fn detached_launch_routes_through_launcher() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, log) = write_fake_launcher(tmp.path());
    // SAFETY: serialised by ENV_LOCK; every test sets what it needs first.
    unsafe {
        std::env::set_var("IRONHERMES_SYSTEMD_RUN", &script);
        std::env::remove_var("IRONHERMES_JOB_SCOPE");
    }

    let result = run(DETACH_CMD, tmp.path().to_str().unwrap()).await;

    assert_eq!(result.returncode, 0, "output: {:?}", result.output);
    assert!(result.output.contains("launched"), "{:?}", result.output);
    assert!(
        result.output.starts_with("[ironhermes] detached into"),
        "hint must lead the output so truncation keeps it: {:?}",
        result.output
    );
    assert!(
        result
            .output
            .contains("systemctl --user stop ironhermes-job-"),
        "scope hint missing: {:?}",
        result.output
    );
    let log = read_log(&log);
    let lines: Vec<&str> = log.lines().collect();
    for want in [
        "--scope",
        "--collect",
        "--slice=ironhermes-jobs.slice",
        "--",
    ] {
        assert!(lines.contains(&want), "argv lacks {want}: {lines:?}");
    }
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("--unit=ironhermes-job-")),
        "argv lacks --unit=ironhermes-job-*: {lines:?}"
    );
}

#[tokio::test]
async fn plain_command_bypasses_launcher() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, log) = write_fake_launcher(tmp.path());
    // SAFETY: serialised by ENV_LOCK.
    unsafe {
        std::env::set_var("IRONHERMES_SYSTEMD_RUN", &script);
        std::env::remove_var("IRONHERMES_JOB_SCOPE");
    }

    let result = run("echo hi", tmp.path().to_str().unwrap()).await;

    assert!(result.output.contains("hi"), "{:?}", result.output);
    assert!(
        !result.output.contains("ironhermes-job-"),
        "plain command must not be scoped: {:?}",
        result.output
    );
    assert!(
        !read_log(&log).lines().any(|l| l.starts_with("--unit=")),
        "launcher must not be invoked for a plain command"
    );
}

fn set_launcher(path: &Path) {
    // SAFETY: callers hold ENV_LOCK.
    unsafe {
        std::env::set_var("IRONHERMES_SYSTEMD_RUN", path);
        std::env::remove_var("IRONHERMES_JOB_SCOPE");
    }
}

fn assert_plain_launch(result: &ironhermes_exec::backend::ExecResult) {
    assert_eq!(result.returncode, 0, "output: {:?}", result.output);
    assert!(result.output.contains("launched"), "{:?}", result.output);
    assert!(
        !result.output.contains("ironhermes-job-"),
        "must degrade to plain bash: {:?}",
        result.output
    );
}

#[tokio::test]
async fn missing_launcher_degrades_to_plain_bash() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    set_launcher(&tmp.path().join("does-not-exist"));

    let result = run(DETACH_CMD, tmp.path().to_str().unwrap()).await;

    assert_plain_launch(&result);
}

#[tokio::test]
async fn failing_probe_degrades_to_plain_bash() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let failing = tmp.path().join("failing-systemd-run");
    std::fs::write(&failing, "#!/usr/bin/env bash\nexit 1\n").unwrap();
    std::fs::set_permissions(&failing, std::fs::Permissions::from_mode(0o755)).unwrap();
    set_launcher(&failing);

    let result = run(DETACH_CMD, tmp.path().to_str().unwrap()).await;

    assert_plain_launch(&result);
}

#[tokio::test]
async fn job_scope_off_bypasses_launcher() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, log) = write_fake_launcher(tmp.path());
    set_launcher(&script);
    // SAFETY: serialised by ENV_LOCK.
    unsafe { std::env::set_var("IRONHERMES_JOB_SCOPE", "off") };

    let result = run(DETACH_CMD, tmp.path().to_str().unwrap()).await;

    // SAFETY: cleanup.
    unsafe { std::env::remove_var("IRONHERMES_JOB_SCOPE") };
    assert_plain_launch(&result);
    assert!(
        read_log(&log).is_empty(),
        "kill switch must skip the probe too: {:?}",
        read_log(&log)
    );
}

#[tokio::test]
async fn scoped_launch_does_not_leak_parent_secret() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, log) = write_fake_launcher(tmp.path());
    set_launcher(&script);
    let secret = "scope-secret-placeholder-xyz";
    // SAFETY: serialised by ENV_LOCK; unique var name.
    unsafe { std::env::set_var("EXEC_SCOPE_TEST_SECRET", secret) };

    let result = run(
        "printenv EXEC_SCOPE_TEST_SECRET; printenv PATH >/dev/null && echo path-present; nohup true >/dev/null 2>&1 &",
        tmp.path().to_str().unwrap(),
    )
    .await;

    // SAFETY: cleanup.
    unsafe { std::env::remove_var("EXEC_SCOPE_TEST_SECRET") };
    assert!(!result.output.contains(secret), "leak: {:?}", result.output);
    assert!(
        result.output.contains("path-present"),
        "{:?}",
        result.output
    );
    assert!(
        read_log(&log)
            .lines()
            .any(|l| l.starts_with("--unit=ironhermes-job-")),
        "scoped branch must have run"
    );
}

#[tokio::test]
async fn timed_out_scoped_command_says_grandchildren_survive() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let (script, _log) = write_fake_launcher(tmp.path());
    set_launcher(&script);

    let cwd = tmp.path().to_str().unwrap();
    let env = LocalEnvironment::new(cwd, Duration::from_millis(300), vec![]);
    let mut sess = Session::new(cwd);
    let result = execute(
        &env,
        &mut sess,
        "nohup sleep 2 >/dev/null 2>&1 & sleep 2",
        cwd,
        Duration::from_millis(300),
        None,
    )
    .await
    .expect("execute should succeed");

    assert_eq!(result.returncode, 124, "output: {:?}", result.output);
    assert!(
        result.output.contains("processes it started stay running"),
        "{:?}",
        result.output
    );
}

/// WR-08: a launcher that passes the probe but fails on the real launch never
/// ran the command, so the output must not claim a scope.
#[tokio::test]
async fn launcher_failure_after_probe_gets_no_scope_hint() {
    let _g = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let flaky = tmp.path().join("probe-only-systemd-run");
    std::fs::write(
        &flaky,
        "#!/usr/bin/env bash\n\
         for a in \"$@\"; do [ \"$a\" = true ] && exit 0; done\n\
         echo 'Failed to connect to bus' >&2\nexit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&flaky, std::fs::Permissions::from_mode(0o755)).unwrap();
    set_launcher(&flaky);

    let result = run(DETACH_CMD, tmp.path().to_str().unwrap()).await;

    assert_eq!(result.returncode, 1, "output: {:?}", result.output);
    assert!(
        result.output.contains("Failed to connect to bus"),
        "{:?}",
        result.output
    );
    assert!(
        !result.output.contains("ironhermes-job-"),
        "must not claim a scope: {:?}",
        result.output
    );
}
