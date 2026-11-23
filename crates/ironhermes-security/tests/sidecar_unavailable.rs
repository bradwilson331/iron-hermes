//! The unavailable matrix (Phase 57 plan 03, D-02 / D-12): every way the sidecar can
//! fail is a typed `Unavailable`, never a clean verdict, and the client recovers by
//! itself. Driven against the MIT fake sidecar (`ironhermes-tirith-fake`).
//!
//! Run single-threaded (`-- --test-threads=1`): `child_env_is_scrubbed` mutates the
//! process environment.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ironhermes_core::config::DefconLevel;
use ironhermes_security::policy::Verdict;
use ironhermes_security::service::{ScreenSubject, SecurityService, VerdictSource};
use ironhermes_security::{
    SidecarClient, SidecarOptions, SidecarState, SubjectKind, Unavailable, UnavailableReason,
};
use tempfile::TempDir;

fn fake_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_ironhermes-tirith-fake"))
}

/// A client over the fake in `mode`, with short budgets so the matrix is quick.
fn fake_client(mode: &str, tweak: impl FnOnce(&mut SidecarOptions)) -> (SidecarClient, TempDir) {
    let home = tempfile::tempdir().unwrap();
    let mut opts = SidecarOptions::for_home(home.path());
    opts.binary = Some(fake_bin());
    opts.extra_args = vec!["--mode".to_string(), mode.to_string()];
    opts.first_timeout = Duration::from_millis(300);
    opts.steady_timeout = Duration::from_millis(100);
    opts.backoff_min = Duration::from_millis(300);
    opts.backoff_max = Duration::from_secs(5);
    tweak(&mut opts);
    (SidecarClient::spawn(opts), home)
}

async fn exec(
    client: &SidecarClient,
    text: &str,
) -> Result<Vec<ironhermes_security::WireFinding>, Unavailable> {
    client.check(SubjectKind::Exec, text, None).await
}

fn titles(findings: &[ironhermes_security::WireFinding]) -> Vec<String> {
    findings.iter().map(|f| f.title.clone()).collect()
}

fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn assert_reason<T: std::fmt::Debug>(result: Result<T, Unavailable>, want: UnavailableReason) {
    match result {
        Err(e) => assert_eq!(e.reason, want, "wrong reason: {e}"),
        Ok(v) => panic!("expected Unavailable({want}), got Ok({v:?})"),
    }
}

#[tokio::test]
async fn missing_binary_is_missing_fast() {
    let home = tempfile::tempdir().unwrap();
    let mut opts = SidecarOptions::for_home(home.path());
    opts.binary = Some(home.path().join("no-such-sidecar"));
    let client = SidecarClient::spawn(opts);

    let started = Instant::now();
    let result = exec(&client, "echo hi").await;
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "must fail fast"
    );
    assert_reason(result, UnavailableReason::Missing);
    assert_eq!(client.health().state, SidecarState::Missing);
}

#[tokio::test]
async fn silent_child_is_timeout() {
    let (client, _home) = fake_client("sleep", |_| {});
    let started = Instant::now();
    let result = exec(&client, "echo hi").await;
    let elapsed = started.elapsed();
    assert_reason(result, UnavailableReason::Timeout);
    assert!(
        elapsed >= Duration::from_millis(250),
        "first request gets the first-request budget, took {elapsed:?}"
    );
    assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
    assert_eq!(client.health().consecutive_timeouts, 1);
    assert_eq!(
        client.health().last_unavailable,
        Some(UnavailableReason::Timeout)
    );
}

#[tokio::test]
async fn garbage_reply_is_protocol_error() {
    let (client, _home) = fake_client("garbage", |_| {});
    assert_reason(
        exec(&client, "echo hi").await,
        UnavailableReason::ProtocolError,
    );
}

#[tokio::test]
async fn oversized_subject_is_too_large_without_io() {
    let (client, _home) = fake_client("count", |o| o.max_subject_bytes = 1024);
    let big = "x".repeat(2000);
    assert_reason(exec(&client, &big).await, UnavailableReason::TooLarge);

    // The refused subject was never written: the fake has seen no check yet, so
    // the next one is its first.
    let findings = exec(&client, "echo ok").await.expect("fake answers");
    assert_eq!(titles(&findings), vec!["count:1".to_string()]);
}

#[tokio::test]
async fn unsupported_subject_reply_is_unsupported() {
    let (refused, _h1) = fake_client("err=unsupported_subject", |_| {});
    assert_reason(exec(&refused, "x").await, UnavailableReason::Unsupported);

    let (internal, _h2) = fake_client("err=internal", |_| {});
    assert_reason(exec(&internal, "x").await, UnavailableReason::ProtocolError);

    let (too_large, _h3) = fake_client("err=too_large", |_| {});
    assert_reason(exec(&too_large, "x").await, UnavailableReason::TooLarge);
}

/// Options for the budget tests: the child answers each check after 250 ms (or
/// 150 ms), a warm check has 100 ms plus 10 ms per KiB, capped at 2 s.
fn scaled_budget(o: &mut SidecarOptions) {
    o.first_timeout = Duration::from_secs(2);
    o.steady_timeout = Duration::from_millis(100);
    o.budget_per_kib = Duration::from_millis(10);
    o.budget_ceiling = Duration::from_secs(2);
}

#[tokio::test]
async fn budget_scales_with_subject_size() {
    let (client, _home) = fake_client("delay-ms=250", scaled_budget);

    // Warm the child: the cold first check has the 2 s first-request budget.
    exec(&client, "warm-up").await.expect("warm-up is answered");

    // 1 byte: 100 ms budget against a 250 ms answer.
    assert_reason(exec(&client, "x").await, UnavailableReason::Timeout);
    // Let that late reply drain so it cannot delay the next answer.
    tokio::time::sleep(Duration::from_millis(400)).await;

    // 30 KiB: 100 ms + 30 * 10 ms = 400 ms against a 250 ms answer.
    let big = "y".repeat(30 * 1024);
    let findings = exec(&client, &big)
        .await
        .expect("a large subject gets a budget that matches its size");
    assert_eq!(findings.len(), 1);
}

#[tokio::test]
async fn queued_request_budget_counts_bytes_ahead() {
    let (client, _home) = fake_client("delay-ms=150", scaled_budget);
    exec(&client, "warm-up").await.expect("warm-up is answered");

    // 40 KiB goes first: 100 ms + 400 ms budget, answered at about 150 ms.
    let big_client = client.clone();
    let big = "z".repeat(40 * 1024);
    let first = tokio::spawn(async move { exec(&big_client, &big).await });
    tokio::time::sleep(Duration::from_millis(20)).await;

    // One byte queued behind it: the child answers it at about 300 ms. Its own
    // size would give it 100 ms; the 40 KiB queued ahead give it 500 ms.
    let second = exec(&client, "q").await;
    assert!(second.is_ok(), "queued behind a large check: {second:?}");
    assert!(first.await.expect("join").is_ok(), "the large check itself");
}

#[tokio::test]
async fn late_reply_never_reaches_next_request() {
    let (client, _home) = fake_client("delay-first-ms=400", |o| {
        o.first_timeout = Duration::from_millis(150);
        o.steady_timeout = Duration::from_millis(150);
    });

    assert_reason(exec(&client, "A").await, UnavailableReason::Timeout);

    let b = exec(&client, "B").await.expect("B is answered promptly");
    assert_eq!(
        titles(&b),
        vec!["B".to_string()],
        "B must get only its own reply"
    );

    // A's reply lands after its request gave up; it must be dropped and counted.
    let deadline = Instant::now() + Duration::from_secs(3);
    while client.health().late_replies < 1 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(client.health().late_replies, 1);

    let c = exec(&client, "C").await.expect("C is answered");
    assert_eq!(titles(&c), vec!["C".to_string()]);
}

#[tokio::test]
async fn crash_then_backoff_then_respawn() {
    let (client, _home) = fake_client("crash-after=1", |o| {
        o.backoff_min = Duration::from_millis(400);
    });

    let first = exec(&client, "one").await.expect("request 1 is answered");
    assert_eq!(titles(&first), vec!["one".to_string()]);

    assert_reason(exec(&client, "two").await, UnavailableReason::Crash);

    // Inside the backoff window: immediate Crash, nothing is spawned.
    let started = Instant::now();
    assert_reason(exec(&client, "three").await, UnavailableReason::Crash);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "backoff must not queue"
    );
    assert_eq!(client.health().respawns, 0);
    assert_eq!(client.health().state, SidecarState::Backoff);

    // After the window the next request respawns and succeeds.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let fourth = exec(&client, "four")
        .await
        .expect("respawned child answers");
    assert_eq!(titles(&fourth), vec!["four".to_string()]);
    assert_eq!(client.health().respawns, 1);
}

#[cfg(unix)]
#[tokio::test]
async fn killed_child_mid_request_is_crash() {
    let (client, _home) = fake_client("sleep", |o| {
        o.first_timeout = Duration::from_secs(5);
    });
    let pid = client.child_pid_for_tests().expect("child is running");

    let worker = client.clone();
    let handle = std::thread::spawn(move || {
        let started = Instant::now();
        let result = worker.check_blocking(SubjectKind::Exec, "echo hi", None);
        (result, started.elapsed())
    });
    std::thread::sleep(Duration::from_millis(200));
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .expect("run kill");
    assert!(status.success());

    let (result, elapsed) = handle.join().expect("worker thread");
    assert_reason(result, UnavailableReason::Crash);
    assert!(
        elapsed < Duration::from_secs(2),
        "in-flight request must resolve on child death, not at its budget (took {elapsed:?})"
    );
    assert!(!process_alive(pid), "killed child must be reaped");
}

#[cfg(unix)]
#[tokio::test]
async fn repeated_timeouts_kill_and_respawn() {
    let (client, _home) = fake_client("sleep", |o| {
        o.first_timeout = Duration::from_millis(150);
        o.timeout_kill_threshold = 2;
        o.backoff_min = Duration::from_millis(200);
    });
    let first_pid = client.child_pid_for_tests().expect("child is running");

    assert_reason(exec(&client, "a").await, UnavailableReason::Timeout);
    assert_reason(exec(&client, "b").await, UnavailableReason::Timeout);

    assert_eq!(
        client.health().state,
        SidecarState::Backoff,
        "second timeout kills the child"
    );
    assert!(!process_alive(first_pid), "the hung child must be killed");
    assert_eq!(
        client.health().consecutive_timeouts,
        0,
        "kill resets the streak"
    );

    tokio::time::sleep(Duration::from_millis(300)).await;
    // The respawned child is silent too; what matters is that it was respawned.
    assert_reason(exec(&client, "c").await, UnavailableReason::Timeout);
    assert_eq!(client.health().respawns, 1);
    let second_pid = client.child_pid_for_tests().expect("respawned child");
    assert_ne!(first_pid, second_pid);
}

fn level(n: u8) -> DefconLevel {
    DefconLevel::try_from(n).expect("level 1..=5")
}

fn paste_subject() -> ScreenSubject {
    ScreenSubject {
        kind: SubjectKind::Paste,
        text: "hello".to_string(),
        name: None,
    }
}

/// A kill for repeated timeouts is a not-screened reason, not an outage: every
/// request until the respawn fails closed, a non-Warn-class subject included.
#[cfg(unix)]
#[tokio::test]
async fn timeout_kill_backoff_is_not_an_outage() {
    let (client, _home) = fake_client("sleep", |o| {
        o.first_timeout = Duration::from_millis(150);
        o.timeout_kill_threshold = 2;
        o.backoff_min = Duration::from_secs(2);
    });

    assert_reason(exec(&client, "a").await, UnavailableReason::Timeout);
    assert_reason(exec(&client, "b").await, UnavailableReason::Timeout);

    let started = Instant::now();
    let during = exec(&client, "c").await;
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "the post-kill backoff must not queue"
    );
    let err = during.expect_err("the child was killed");
    assert_eq!(err.reason, UnavailableReason::KilledForTimeouts, "{err}");
    assert!(!err.reason.is_outage());
    let health = client.health();
    assert_eq!(health.state, SidecarState::Backoff);
    assert_eq!(
        health.last_unavailable,
        Some(UnavailableReason::KilledForTimeouts)
    );

    let svc = SecurityService::new(Arc::new(client.clone()));
    let three = svc.screen(&paste_subject(), level(3), false).await;
    assert_eq!(three.verdict, Verdict::Block, "DEFCON 3 must not Allow");
    assert_eq!(
        three.unavailable,
        Some(UnavailableReason::KilledForTimeouts)
    );
    assert_eq!(three.source, VerdictSource::Unavailable);
    let five = svc.screen(&paste_subject(), level(5), false).await;
    assert_eq!(five.verdict, Verdict::Warn, "DEFCON 5 warns, never silent");
    assert_eq!(five.unavailable, Some(UnavailableReason::KilledForTimeouts));
    assert_eq!(five.source, VerdictSource::Unavailable);
}

/// A check still waiting on the child when it is killed for timeouts was not
/// screened because of somebody else's subject: it is `KilledForTimeouts` too.
#[cfg(unix)]
#[tokio::test]
async fn queued_check_at_timeout_kill_is_not_an_outage() {
    let (client, _home) = fake_client("delay-ms=500", |o| {
        scaled_budget(o);
        o.timeout_kill_threshold = 1;
        o.backoff_min = Duration::from_secs(2);
    });
    exec(&client, "warm-up").await.expect("warm-up is answered");

    // A: 1 byte, 100 ms budget against a 500 ms answer, so it times out and its
    // timeout kills the child (threshold 1).
    let a_client = client.clone();
    let a = tokio::spawn(async move { exec(&a_client, "a").await });
    tokio::time::sleep(Duration::from_millis(10)).await;

    // B: 100 KiB behind A, about 1.1 s of budget, so it is still waiting at the kill.
    let big = "b".repeat(100 * 1024);
    let sent = Instant::now();
    let b = exec(&client, &big).await;
    let waited = sent.elapsed();

    assert_reason(a.await.expect("join"), UnavailableReason::Timeout);
    assert_reason(b, UnavailableReason::KilledForTimeouts);
    assert!(
        waited < Duration::from_millis(600),
        "B resolves at the kill, not at its own budget (took {waited:?})"
    );
}

/// GC-WR-01: a check whose budget expired, but which the child has not answered, is
/// still on the child's queue. Its bytes keep counting toward the next check's budget
/// until its late reply arrives, and that reply is never delivered to anyone else.
#[tokio::test]
async fn abandoned_check_bytes_count_toward_next_budget() {
    let (client, _home) = fake_client("delay-ms=800", scaled_budget);
    exec(&client, "warm-up").await.expect("warm-up is answered");

    // A: 50 KiB, 100 ms + 500 ms budget against an 800 ms answer.
    let a_text = "a".repeat(50 * 1024);
    assert_reason(exec(&client, &a_text).await, UnavailableReason::Timeout);

    // B: 60 KiB, sent at once. The child is still on A, so it answers B about 1 s
    // after B was sent. B's own size gives 700 ms; counting A's 50 KiB gives 1.2 s.
    let b_text = "b".repeat(60 * 1024);
    let b = exec(&client, &b_text)
        .await
        .expect("B's budget counts the abandoned check queued ahead of it");
    assert_eq!(b.len(), 1, "B gets exactly its own reply");

    let health = client.health();
    assert_eq!(
        health.late_replies, 1,
        "A's reply was counted and dropped, never delivered"
    );
    assert_eq!(
        health.consecutive_timeouts, 0,
        "B's answer resets the streak"
    );
}

/// After a kill for timeouts the backoff stays at the minimum (an author must not
/// be able to grow the fail-closed window by repeating the trigger).
#[cfg(unix)]
#[tokio::test]
async fn timeout_kill_backoff_does_not_double() {
    let (client, _home) = fake_client("sleep", |o| {
        o.first_timeout = Duration::from_millis(100);
        o.timeout_kill_threshold = 1;
        o.backoff_min = Duration::from_millis(300);
        o.backoff_max = Duration::from_secs(5);
    });

    assert_reason(exec(&client, "one").await, UnavailableReason::Timeout);
    tokio::time::sleep(Duration::from_millis(450)).await;

    assert_reason(exec(&client, "two").await, UnavailableReason::Timeout);
    assert_eq!(client.health().respawns, 1);
    tokio::time::sleep(Duration::from_millis(450)).await;

    // A doubled backoff (600 ms) would still be running and give KilledForTimeouts.
    assert_reason(exec(&client, "three").await, UnavailableReason::Timeout);
    assert_eq!(client.health().respawns, 2);
}

/// D-12 unchanged for a real outage: a child that exits on its own is `Crash`, and
/// during that backoff a non-Warn-class subject keeps its regex floor.
#[tokio::test]
async fn crash_backoff_stays_an_outage() {
    let (client, _home) = fake_client("crash-after=1", |o| {
        o.backoff_min = Duration::from_secs(2);
    });

    exec(&client, "one").await.expect("request 1 is answered");
    assert_reason(exec(&client, "two").await, UnavailableReason::Crash);

    let started = Instant::now();
    assert_reason(exec(&client, "three").await, UnavailableReason::Crash);
    assert!(
        started.elapsed() < Duration::from_millis(50),
        "backoff must not queue"
    );

    let svc = SecurityService::new(Arc::new(client.clone()));
    let floor = svc.screen(&paste_subject(), level(3), false).await;
    assert_eq!(
        floor.verdict,
        Verdict::Allow,
        "an outage keeps the regex floor for a non-Warn-class subject"
    );
    assert_eq!(floor.unavailable, Some(UnavailableReason::Crash));
    let warn_class = svc.screen(&paste_subject(), level(3), true).await;
    assert_eq!(warn_class.verdict, Verdict::Block);
    assert_eq!(warn_class.unavailable, Some(UnavailableReason::Crash));
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_client_kills_child() {
    let (client, _home) = fake_client("sleep", |_| {});
    let pid = client.child_pid_for_tests().expect("child is running");
    assert!(process_alive(pid));
    drop(client);
    assert!(
        !process_alive(pid),
        "no orphan sidecar after the client is dropped"
    );
}

#[test]
fn backoff_doubles_to_cap_and_resets() {
    let min = Duration::from_millis(500);
    let max = Duration::from_secs(30);
    let mut b = min;
    let mut seen = vec![b];
    for _ in 0..8 {
        b = ironhermes_security::next_backoff(b, min, max);
        seen.push(b);
    }
    assert_eq!(
        seen[..4],
        [
            Duration::from_millis(500),
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4)
        ]
    );
    assert_eq!(*seen.last().unwrap(), max, "capped at the maximum");
    assert_eq!(ironhermes_security::next_backoff(max, min, max), max);

    let reset = Duration::from_secs(60);
    assert_eq!(
        ironhermes_security::backoff_after_healthy(
            Duration::from_secs(61),
            reset,
            Duration::from_secs(8),
            min,
            max
        ),
        min,
        "a child healthy for the reset window starts over"
    );
    assert_eq!(
        ironhermes_security::backoff_after_healthy(
            Duration::from_secs(5),
            reset,
            Duration::from_secs(8),
            min,
            max
        ),
        Duration::from_secs(16),
        "a child that died young keeps doubling"
    );
}

/// Sets environment variables for the test process and restores them on drop.
/// Sound only under `--test-threads=1`.
struct ScopedEnv {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl ScopedEnv {
    fn set(vars: &[(&'static str, &str)]) -> Self {
        let mut saved = Vec::new();
        for (k, v) in vars {
            saved.push((*k, std::env::var_os(k)));
            // SAFETY: single-threaded test run (see the module doc).
            unsafe { std::env::set_var(k, v) };
        }
        Self { saved }
    }
}

impl Drop for ScopedEnv {
    fn drop(&mut self) {
        for (k, v) in self.saved.drain(..).rev() {
            // SAFETY: as in `ScopedEnv::set`.
            unsafe {
                match v {
                    Some(v) => std::env::set_var(k, v),
                    None => std::env::remove_var(k),
                }
            }
        }
    }
}

#[tokio::test]
async fn child_env_is_scrubbed() {
    let _env = ScopedEnv::set(&[
        ("TIRITH", "0"),
        ("TIRITH_SERVER_URL", "http://127.0.0.1:9"),
        ("TIRITH_API_KEY", "x"),
        ("SECRET_TOKEN", "hunter2"),
    ]);
    let (client, _home) = fake_client("echo-env", |o| {
        o.first_timeout = Duration::from_secs(3);
    });
    let findings = exec(&client, "ignored").await.expect("fake answers");
    let mut names = titles(&findings);
    names.sort();
    let mut want: Vec<String> = [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "TIRITH_LOG",
        "TIRITH_POLICY_ROOT",
        "PATH",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    want.sort();
    assert_eq!(
        names, want,
        "child must see exactly the allowlisted variables"
    );
}

#[tokio::test]
async fn no_error_path_returns_clean() {
    // Every failure mode must be an Err; none may be an Ok (clean) result.
    let home = tempfile::tempdir().unwrap();
    let mut missing = SidecarOptions::for_home(home.path());
    missing.binary = Some(home.path().join("nope"));
    let missing = SidecarClient::spawn(missing);
    assert!(exec(&missing, "x").await.is_err(), "missing");

    let (silent, _h1) = fake_client("sleep", |_| {});
    assert!(exec(&silent, "x").await.is_err(), "timeout");

    let (garbage, _h2) = fake_client("garbage", |_| {});
    assert!(exec(&garbage, "x").await.is_err(), "garbage");

    let (small, _h3) = fake_client("ok", |o| o.max_subject_bytes = 8);
    assert!(
        exec(&small, "this is longer than eight bytes")
            .await
            .is_err(),
        "too large"
    );

    let (crashy, _h4) = fake_client("crash-after=1", |_| {});
    let _ = exec(&crashy, "first").await;
    assert!(exec(&crashy, "second").await.is_err(), "crash");
    assert!(exec(&crashy, "third").await.is_err(), "backoff");
}
