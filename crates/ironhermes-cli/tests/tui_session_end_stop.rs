#![cfg(feature = "test-support")]
//! Phase 49.8 Plan 23 (D-10/D-16/D-09, gap G-49.8-21): exit-path matrix
//! proving every ordinary TUI exit, plus a panic or `Err` in the loop,
//! stops a live walk on the registry turns actually drive — the real
//! production composition (`event_loop::run_tui_session`), driven over a
//! `TestBackend` and a channel-backed event stream instead of a real
//! terminal, never a hand-rolled substitute for the teardown under test.
//!
//! Correction to the debug diagnosis (`.planning/debug/tui-exit-mid-walk-no-stop.md`):
//! the UAT failure was NOT the `ctrl_c` re-poll panic that doc blames (that
//! bug is real but reachable only via an external SIGINT, fixed separately
//! in 49.8-24) — it was the TUI stopping the wrong registry. See the
//! 49.8-23-PLAN.md "Correction to the debug diagnosis" section.

#[path = "../../ironhermes-tools/tests/support/mock_rosbridge.rs"]
mod mock_rosbridge;
#[path = "support/tui_ros2_fixture.rs"]
mod tui_ros2_fixture;

use std::sync::Arc;
use std::time::{Duration, Instant};

use ironhermes_cli::tui_rata::event_loop::{run_tui_session, session_end_registry, with_tui_session_end_stop};
use mock_rosbridge::MockRosbridge;
use tui_ros2_fixture::*;

/// `WALK_SECS` is long enough that every exit race (double ctrl+c at
/// ~300ms, `/quit`, input-closed, panic, Err) is unambiguously BEFORE the
/// walk's own natural end, and short enough that a hung test fails fast
/// under the harness's own timeout rather than the suite's.
const WALK_SECS: f64 = 5.0;

/// Shared post-exit assertion for every test below: within 1s of the
/// teardown completing, the mock holds a gait-0 rest stop on
/// `/controller/traveling` recorded after `marker`, and earlier than
/// `walk_start + 4.0s` — proving the STOP interrupted the walk rather than
/// the walk's own 5.0s end firing it.
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
         — a later stop means the walk's OWN end fired the stop, not the exit teardown"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_exit_mid_walk_double_ctrl_c_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, mut app, mut terminal) = build_fixture().await;

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    // spawn_turn leaves exactly this behind for an in-flight turn.
    app.cancel_child = Some(tokio_util::sync::CancellationToken::new());

    let (tx, events) = channel_event_stream();
    tx.send(Ok(ctrl_c_key_event())).expect("send first ctrl+c");
    let tx2 = tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let _ = tx2.send(Ok(ctrl_c_key_event()));
    });

    let marker = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(
        app.should_quit,
        "app.should_quit must be true after a double ctrl+c exit"
    );

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2: `/quit` is an exact palette match for the registered `quit`
/// command, so a single `Enter` after typing it already calls
/// `dispatch_or_submit` -> `SlashOutcome::Quit` -> `should_quit = true`
/// (see `app.rs::palette_enter`'s "exact match" branch). A second `Enter`
/// is sent anyway (harmless no-op on the now-empty textarea via
/// `submit()`'s empty-text guard) to match the documented exit sequence.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_exit_mid_walk_quit_command_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, mut app, mut terminal) = build_fixture().await;

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    let (tx, events) = channel_event_stream();
    for c in "/quit".chars() {
        tx.send(Ok(char_key_event(c))).expect("send /quit char");
    }
    tx.send(Ok(enter_key_event())).expect("send first enter");
    tx.send(Ok(enter_key_event())).expect("send second enter");

    let marker = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(app.should_quit, "app.should_quit must be true after /quit");

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2: dropping the event sender ends the stream; `run_event_loop`'s
/// `events.next() => ... None => { app.should_quit = true; }` arm fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_exit_mid_walk_terminal_input_closed_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, mut app, mut terminal) = build_fixture().await;

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    let (tx, events) = channel_event_stream();
    drop(tx); // closes the stream — events.next() yields None on the next poll

    let marker = Instant::now();
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        run_tui_session(&mut terminal, &mut app, events),
    )
    .await
    .expect("run_tui_session must return within 10s");

    assert!(result.is_ok(), "run_tui_session returned Err: {result:?}");
    assert!(
        app.should_quit,
        "app.should_quit must be true after the terminal input stream ends"
    );

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2 (D-10/D-16): a panic inside the wrapped session future still
/// runs the hooks + the awaited stop BEFORE the panic resumes —
/// `with_tui_session_end_stop` catches it via `catch_unwind`, stops the
/// robot, then calls `std::panic::resume_unwind`, so the panic reaches the
/// spawned task's join as a genuine panic (`JoinError::is_panic()`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_session_panic_still_stops_the_robot_then_resumes() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, app, _terminal) = build_fixture().await;

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    let registry = session_end_registry(&app);
    let marker = Instant::now();
    let join = tokio::spawn(async move {
        with_tui_session_end_stop(registry, async { panic!("simulated TUI loop panic") }).await
    });

    let join_result = join.await;
    let join_err = join_result.expect_err("the spawned task must panic, not return");
    assert!(
        join_err.is_panic(),
        "JoinError must be a panic, not a cancellation: {join_err:?}"
    );

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2 (D-10/D-16): an `Err` from the wrapped session future still runs
/// the hooks + the awaited stop, and the ORIGINAL error message is
/// returned unchanged (no stop-outcome swallowing the real failure).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tui_session_err_still_stops_the_robot() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (mock, app, _terminal) = build_fixture().await;

    let (walk_handle, walk_start) = launch_walk(&app, &mock, WALK_SECS).await;

    let registry = session_end_registry(&app);
    let marker = Instant::now();
    let result =
        with_tui_session_end_stop(registry, async { Err(anyhow::anyhow!("simulated draw failure")) }).await;

    let err = result.expect_err("with_tui_session_end_stop must propagate the session's Err");
    assert!(
        err.to_string().contains("simulated draw failure"),
        "got: {err}"
    );

    assert_walk_was_interrupted_by_the_stop(&mock, marker, walk_start).await;

    let walk_result = walk_handle.await.expect("walk task must not panic");
    assert!(
        walk_result.is_ok(),
        "walk dispatch must not error: {walk_result:?}"
    );
}

/// Task 2 (G-49.8-21 regression guard): `session_end_registry` must return
/// the SAME `Arc` as `agent_runtime.registry()` — the registry every turn's
/// tool call actually executes on. As a fixture-topology precondition,
/// `app.registry` must be a DIFFERENT `Arc`, mirroring production's
/// `build_app_deps` (the TUI-local registry is never the runtime's).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_end_registry_is_the_runtime_registry() {
    let _guard = serial_guard().await;
    let _home = hermetic_env();
    let (_mock, app, _terminal) = build_fixture().await;

    assert!(
        Arc::ptr_eq(&session_end_registry(&app), app.agent_runtime.registry()),
        "session_end_registry must return the SAME Arc as agent_runtime.registry() (G-49.8-21)"
    );
    assert!(
        !Arc::ptr_eq(&app.registry, app.agent_runtime.registry()),
        "fixture-topology precondition: app.registry must be a DIFFERENT Arc from the runtime \
         registry, mirroring production build_app_deps"
    );
}
