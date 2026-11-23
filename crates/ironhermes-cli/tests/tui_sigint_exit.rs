#![cfg(all(unix, feature = "test-support"))]
//! Phase 49.8 Plan 24 (gap G-49.8-21, D-10/D-16): real-signal regression
//! tests for the TUI event loop's SIGINT re-arm and the SIGTERM/SIGHUP
//! graceful-quit path, driven through the REAL production composition
//! (`event_loop::run_tui_session`) over the shared hermetic ros2 fixture
//! (`tui_ros2_fixture.rs`, 49.8-23) — real signals delivered with POSIX
//! `kill` to this test process, not a hand-rolled substitute for the OS
//! signal plumbing under test.
//!
//! RED evidence (Task 1, before the ctrl_c re-arm fix): running
//! `tui_double_sigint_mid_walk_stops_the_robot` against the pre-fix
//! `run_event_loop` (a single `ctrl_c` future created once and reused by
//! reference on every `select!` poll) panics on the very next event-loop
//! tick after the FIRST delivered SIGINT, before the second delivery ever
//! fires:
//!
//!   thread 'tui_double_sigint_mid_walk_stops_the_robot' panicked at
//!   .../tokio-1.52.3/src/signal/ctrl_c.rs:59:41:
//!   `async fn` resumed after completion
//!
//! This matches `.planning/debug/tui-exit-mid-walk-no-stop.md`'s Evidence
//! 21:40 repro exactly. `with_tui_session_end_stop`'s `catch_unwind` still
//! runs the D-10/D-16 stop before `resume_unwind`, so the mock DOES record
//! a rest stop even on this RED run — the panic itself, not a missing stop,
//! is the RED signal this test proves and the GREEN fix below removes.

#[path = "../../ironhermes-tools/tests/support/mock_rosbridge.rs"]
mod mock_rosbridge;
#[path = "support/tui_ros2_fixture.rs"]
mod tui_ros2_fixture;

use std::time::{Duration, Instant};

use ironhermes_cli::tui_rata::event_loop::{run_event_loop, run_tui_session};
use mock_rosbridge::MockRosbridge;
use tui_ros2_fixture::*;

/// Long enough that every signal race below (double SIGINT at ~300ms+400ms,
/// a third idle SIGINT at ~350ms spacing, a single SIGTERM/SIGHUP) is
/// unambiguously BEFORE the walk's own natural end.
const WALK_SECS: f64 = 5.0;

/// Delivers a real signal to THIS process via POSIX `kill` — avoids a
/// `libc`/`nix` dev-dependency. `signal_name` is e.g. `"INT"`, `"TERM"`,
/// `"HUP"` (no `SIG` prefix, matching `kill -s`'s own argument form).
fn deliver(signal_name: &str) {
    let pid = std::process::id().to_string();
    let status = std::process::Command::new("kill")
        .args(["-s", signal_name, &pid])
        .status()
        .unwrap_or_else(|e| panic!("tui_sigint_exit: failed to spawn `kill -s {signal_name} {pid}`: {e}"));
    assert!(
        status.success(),
        "tui_sigint_exit: `kill -s {signal_name} {pid}` did not exit successfully: {status:?}"
    );
}

/// Shared post-exit assertion (mirrors `tui_session_end_stop.rs`'s helper of
/// the same shape): within 1s of the teardown completing, the mock holds a
/// gait-0 rest stop on `/controller/traveling` recorded after `marker`, and
/// earlier than `walk_start + 4.0s` — proving the signal-driven exit
/// interrupted the walk rather than the walk's own 5.0s end firing it.
async fn assert_walk_was_interrupted_by_the_stop(mock: &MockRosbridge, marker: Instant, walk_start: Instant) {
    wait_until(
        || rest_stop_after(mock, marker).is_some(),
        Duration::from_secs(1),
    )
    .await;
    let stop_at = rest_stop_after(mock, marker).unwrap_or_else(|| {
        panic!(
            "expected a gait-0 rest stop on /controller/traveling after the exit marker; \
             published traveling frames: {:?}",
            mock.published_on("/controller/traveling")
        )
    });
    let walk_deadline = walk_start + Duration::from_millis(4000);
    assert!(
        stop_at < walk_deadline,
        "rest stop at {stop_at:?} must be earlier than walk_start + 4.0s ({walk_deadline:?}) \
         — a later stop means the walk's OWN end fired the stop, not the signal-driven exit"
    );
}

/// Task 1 (G-49.8-21, D-10/D-16): two real SIGINTs delivered to a TUI with a
/// walk in flight cancel the turn, then end the session. The loop must
/// never panic re-polling the exhausted `ctrl_c` future (RED proof above).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_double_sigint_mid_walk_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, mut app, mut terminal) = build_fixture().await;

    // Hold a listener for the WHOLE test body so no delivery can ever take
    // the default action (process termination) even if the fix under test
    // is absent or broken.
    let _sigint_listener =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("tui_sigint_exit: failed to install SIGINT listener");

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    // spawn_turn leaves exactly this behind for an in-flight turn. Keep our
    // own clone so we can assert cancellation after the token is taken by
    // the double-ctrl-c state machine's first decision.
    let token = tokio_util::sync::CancellationToken::new();
    app.cancel_child = Some(token.clone());

    // The event sender stays alive for the whole test — the exit can only
    // come from the two SIGINTs below, never from the input stream ending.
    let (_tx, events) = channel_event_stream();

    let deliverer = tokio::spawn(async move {
        // The loop has polled its ctrl_c arm at least once by 300ms.
        tokio::time::sleep(Duration::from_millis(300)).await;
        deliver("INT");
        // At least 3 ticks (~100ms each); the old pinned future panicked on
        // the very first tick after a delivery, well before this elapses.
        tokio::time::sleep(Duration::from_millis(400)).await;
        deliver("INT");
    });

    let marker = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    deliverer.await.expect("deliverer task must not panic");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(
        token.is_cancelled(),
        "the first SIGINT must cancel the in-flight turn's token"
    );
    assert!(
        app.should_quit,
        "the second SIGINT must set app.should_quit (double-ctrl-c exit)"
    );

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2 (G-49.8-21): any number of SIGINTs at idle (no in-flight turn)
/// never panics. Three deliveries, each at least 3 ticks (~350ms) apart —
/// well past the RE-ARM fix's window — then the event sender is dropped to
/// end the loop cleanly. Each idle SIGINT shows the same "Ctrl+C again to
/// quit" hint (D-14); none of them is a "second press within the window"
/// because `DoubleCtrlCState` only starts that window on an in-flight
/// cancellation (see `double_ctrl_c.rs::on_ctrl_c`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_repeated_sigint_at_idle_never_panics() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (_mock, mut app, mut terminal) = build_fixture().await;

    let _sigint_listener =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .expect("tui_sigint_exit: failed to install SIGINT listener");

    let (tx, events) = channel_event_stream();

    let deliverer = tokio::spawn(async move {
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(350)).await;
            deliver("INT");
        }
        // Give the loop a final tick to process the third SIGINT, then end
        // the input stream to let run_event_loop return cleanly.
        tokio::time::sleep(Duration::from_millis(350)).await;
        drop(tx);
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_event_loop(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_event_loop must return within 10s");

    deliverer.await.expect("deliverer task must not panic");

    assert!(result.is_ok(), "run_event_loop returned Err: {result:?}");
    assert_eq!(
        app.status.hint, "Ctrl+C again to quit",
        "every idle SIGINT must show the double-ctrl-c hint, never panic or exit"
    );
}

/// Task 2 (G-49.8-21, D-10/D-16): SIGTERM (kill, a supervisor) ends the TUI
/// through the same graceful `should_quit` path as `/quit`, stopping a walk
/// in flight — never the default immediate termination.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_sigterm_mid_walk_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, mut app, mut terminal) = build_fixture().await;

    // Held for the whole test body so a bare SIGTERM can never take the
    // default action (process termination) even pre-fix.
    let _sigterm_listener =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("tui_sigint_exit: failed to install SIGTERM listener");

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    // Event sender kept alive — the exit can only come from the SIGTERM.
    let (_tx, events) = channel_event_stream();

    let deliverer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        deliver("TERM");
    });

    let marker = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    deliverer.await.expect("deliverer task must not panic");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(app.should_quit, "SIGTERM must set app.should_quit");

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2 (G-49.8-21, D-10/D-16): SIGHUP (a closed terminal window) ends the
/// TUI gracefully — never the default immediate termination, which would
/// kill this very test process.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_sighup_ends_the_session_gracefully() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (_mock, mut app, mut terminal) = build_fixture().await;

    // Held for the whole test body so a bare SIGHUP can never take the
    // default action (process termination) even pre-fix.
    let _sighup_listener =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .expect("tui_sigint_exit: failed to install SIGHUP listener");

    // Event sender kept alive — the exit can only come from the SIGHUP.
    let (_tx, events) = channel_event_stream();

    let deliverer = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        deliver("HUP");
    });

    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    deliverer.await.expect("deliverer task must not panic");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(app.should_quit, "SIGHUP must set app.should_quit");
    // The test reaching this line at all proves the process survived the
    // default action was never taken.
}
