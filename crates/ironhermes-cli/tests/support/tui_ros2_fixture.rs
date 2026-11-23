#![allow(dead_code)]
//! Shared hermetic fixture for the TUI exit-path tests (Phase 49.8 Plan 23,
//! gap G-49.8-21). Proves the real production composition
//! (`event_loop::run_tui_session`) stops a live walk on the RUNTIME
//! registry, never the TUI-local `app.registry`.
//!
//! Every including test binary must declare `mod mock_rosbridge` with
//! `#[path = "../../ironhermes-tools/tests/support/mock_rosbridge.rs"]`
//! BEFORE declaring this module — this file refers to it as
//! `crate::mock_rosbridge`.

use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use ironhermes_cli::tui_rata::app::App;
use ironhermes_core::Config;
use ironhermes_tools::ros2::Ros2ConfigSource;
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;

use crate::mock_rosbridge::{MockConfig, MockRosbridge};

/// Process-wide serial guard. These tests share process env
/// (`IRONHERMES_HOME`, `IRONHERMES_ROS2_BRIDGE_URL`) via `hermetic_env`, so
/// only one test may run at a time. Hold the returned guard for the test's
/// WHOLE body.
static SERIAL_LOCK: LazyLock<TokioMutex<()>> = LazyLock::new(|| TokioMutex::new(()));

pub async fn serial_guard() -> tokio::sync::MutexGuard<'static, ()> {
    SERIAL_LOCK.lock().await
}

/// Points `IRONHERMES_HOME` at a fresh temp dir and removes
/// `IRONHERMES_ROS2_BRIDGE_URL` from the process env, so no test can ever
/// reach the developer's real `~/.ironhermes/config.yaml` (which would point
/// at the live ROSPider, 127.0.0.1). MUST run under `serial_guard()`,
/// BEFORE `App` construction. The caller must keep the returned `TempDir`
/// alive for the whole test — dropping it early deletes the directory.
///
/// SAFETY: serialized via `serial_guard()`; only test code ever calls this.
pub fn hermetic_env() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tui_ros2_fixture: tempdir");
    unsafe {
        std::env::set_var("IRONHERMES_HOME", dir.path());
        std::env::remove_var(ironhermes_tools::ros2::BRIDGE_URL_ENV);
    }
    dir
}

/// Starts the mock rosbridge, builds an `App::new_test_empty()` with the
/// ros2 toolset registered on `app.agent_runtime.registry()` (the registry
/// every TUI tool call actually executes on — NOT `app.registry`, per
/// G-49.8-21), and a 120x40 `TestBackend` terminal. Call under
/// `serial_guard()`, after `hermetic_env()`.
pub async fn build_fixture() -> (
    MockRosbridge,
    App,
    ratatui::Terminal<ratatui::backend::TestBackend>,
) {
    let mock = MockRosbridge::start_with(MockConfig::default()).await;

    let app = App::new_test_empty();
    let mut cfg = Config::default();
    cfg.ros2.bridge_url = Some(mock.url.clone());
    app.agent_runtime
        .registry()
        .write()
        .await
        .register_ros2_tools(Ros2ConfigSource::Fixed(Arc::new(cfg)), None);

    let backend = ratatui::backend::TestBackend::new(120, 40);
    let terminal = ratatui::Terminal::new(backend).expect("tui_ros2_fixture: TestBackend terminal");

    (mock, app, terminal)
}

/// Dispatches a `rospider` walk forward of `duration_secs` on the RUNTIME
/// registry (`app.agent_runtime.registry()`), in a spawned task, mirroring
/// exactly how a real turn's tool call would run it (`agent_loop.rs`'s
/// `self.registry.read().await.execute_tool(...).await` shape — the read
/// guard is held for the whole call). Waits until the mock records the
/// first `/controller/cmd_vel` publish before returning, so the caller's
/// subsequent exit-trigger genuinely races a motion already on the wire.
/// Returns the walk's `JoinHandle` and `walk_start` (recorded before the
/// spawn, so a caller computing "earlier than walk_start + N" has a safe
/// lower bound).
pub async fn launch_walk(
    app: &App,
    mock: &MockRosbridge,
    duration_secs: f64,
) -> (tokio::task::JoinHandle<anyhow::Result<String>>, Instant) {
    let registry = app.agent_runtime.registry().clone();
    let walk_start = Instant::now();
    let handle = tokio::spawn(async move {
        registry
            .read()
            .await
            .dispatch(
                "rospider",
                serde_json::json!({
                    "action": "walk",
                    "direction": "forward",
                    "duration_secs": duration_secs,
                }),
            )
            .await
    });

    wait_until(
        || !mock.published_on("/controller/cmd_vel").is_empty(),
        Duration::from_secs(2),
    )
    .await;

    (handle, walk_start)
}

/// A channel-backed terminal-event stream: an unbounded sender the test
/// drives directly, paired with the `Stream` `run_event_loop`/`run_tui_session`
/// need (`tokio_stream::Stream<Item = io::Result<Event>> + Unpin`).
pub fn channel_event_stream() -> (
    tokio::sync::mpsc::UnboundedSender<std::io::Result<crossterm::event::Event>>,
    tokio_stream::wrappers::UnboundedReceiverStream<std::io::Result<crossterm::event::Event>>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (tx, tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
}

/// A `ctrl+c` key-press `Event`, ready to inject through `channel_event_stream`'s sender.
pub fn ctrl_c_key_event() -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char('c'),
        crossterm::event::KeyModifiers::CONTROL,
    ))
}

/// A plain character key-press `Event` (no modifiers) — for typing `/quit`.
pub fn char_key_event(c: char) -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char(c),
        crossterm::event::KeyModifiers::NONE,
    ))
}

/// An `Enter` key-press `Event`.
pub fn enter_key_event() -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Enter,
        crossterm::event::KeyModifiers::NONE,
    ))
}

/// A `Tab` key-press `Event` — completes the palette's highlighted command
/// without submitting (app.rs `handle_palette_key`).
pub fn tab_key_event() -> crossterm::event::Event {
    crossterm::event::Event::Key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Tab,
        crossterm::event::KeyModifiers::NONE,
    ))
}

/// The gait-0 rest stop: `gait == 0` and `time == 1.0`
/// (`ROSPIDER_REST_STOP_GAIT`/`ROSPIDER_REST_STOP_POSE_SECS` in
/// `rospider_tool.rs`) — mirrors `ros2_rospider_rest_stop.rs`'s own helper.
pub fn is_rest_stop(msg: &Value) -> bool {
    msg.get("gait").and_then(Value::as_i64) == Some(0) && msg.get("time").and_then(Value::as_f64) == Some(1.0)
}

/// Polls `cond` every 20ms until it returns `true` or `deadline` elapses.
/// Fire-and-forget rosbridge ops (`advertise`/`publish`) need this instead
/// of an immediate assertion right after dispatch.
pub async fn wait_until(mut cond: impl FnMut() -> bool, deadline: Duration) {
    let start = Instant::now();
    loop {
        if cond() || start.elapsed() > deadline {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The `at` timestamp of the first `/controller/traveling` gait-0 rest-stop
/// publish recorded on `mock` strictly after `marker`, or `None` if none has
/// arrived yet.
pub fn rest_stop_after(mock: &MockRosbridge, marker: Instant) -> Option<Instant> {
    mock.frames_with_op("publish")
        .into_iter()
        .filter(|f| f.at > marker && f.value.get("topic").and_then(Value::as_str) == Some("/controller/traveling"))
        .find(|f| is_rest_stop(f.value.get("msg").unwrap_or(&Value::Null)))
        .map(|f| f.at)
}
