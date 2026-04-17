//! Render task + TuiHandle — the ONLY I/O surface in the tui module.
//!
//! Per D-15/D-16: one tokio::task ticks every 100ms (D-07 frame rate), reads two
//! watch channels (activity + status), and writes to stderr via crossterm absolute
//! cursor positioning. Per D-17: non-tty stderr causes the loop to exit immediately
//! so piped output / CI / ssh-over-slow-link degrade gracefully.
//!
//! Flicker mitigation per RESEARCH §Pitfall 3: Hide/Show wraps every frame.
//! SIGWINCH tolerance per RESEARCH §Pitfall 4: re-query size() each tick.

use crate::tui::activity::ActivityState;
use crate::tui::knight_rider;
use crate::tui::status_line::{render_status_line, StatusLineState};
use colored::Colorize;
use crossterm::{
    cursor::{Hide, MoveTo, RestorePosition, SavePosition, Show},
    queue,
    style::Print,
    terminal::{size, Clear, ClearType},
    tty::IsTty,
};
use std::io::{stderr, Write};
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Frame period for the render loop (D-07: ~10 fps).
pub const FRAME_PERIOD: Duration = Duration::from_millis(100);

/// Public handle returned by `TuiHandle::new`. Holds the watch senders +
/// shutdown token + render task handle. All setter methods are non-blocking
/// and best-effort: a send on a closed channel is silently dropped.
pub struct TuiHandle {
    activity_tx: watch::Sender<ActivityState>,
    status_tx: watch::Sender<StatusLineState>,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl TuiHandle {
    /// Spawn the render task and return a handle. Always succeeds; if stderr
    /// is not a TTY the loop exits on first iteration (non-tty fallback D-17).
    pub fn new(initial_status: StatusLineState) -> Self {
        let (activity_tx, activity_rx) = watch::channel(ActivityState::Idle);
        let (status_tx, status_rx) = watch::channel(initial_status);
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();

        // DECSTBM: reserve bottom 3 rows (prompt + scanner + status bar) by
        // setting a scroll region BEFORE any streaming output starts. Normal
        // stdout/stderr output scrolls only within rows 1..rows-3, keeping the
        // prompt, scanner, and status bar rows fixed outside the scroll region.
        if stderr().is_tty() && let Ok((_cols, rows)) = size() {
            let scroll_end = rows.saturating_sub(3);
            if scroll_end > 0 {
                let mut out = stderr();
                let _ = write!(out, "\x1b[1;{}r", scroll_end);
                let _ = out.flush();
            }
        }

        let task = tokio::spawn(async move {
            render_loop(activity_rx, status_rx, task_shutdown).await;
        });

        Self {
            activity_tx,
            status_tx,
            shutdown,
            task: Some(task),
        }
    }

    /// Test-only constructor: forces the non-tty path by never writing to
    /// a real terminal. The render task still spawns but exits on its first
    /// IsTty check. Used by unit tests without a real terminal.
    #[cfg(test)]
    pub fn new_for_tests() -> Self {
        Self::new(StatusLineState::default())
    }

    /// Publish a new activity state. Non-blocking; errors (closed channel)
    /// are ignored per D-09 "best-effort" semantics.
    pub fn set_activity(&self, state: ActivityState) {
        let _ = self.activity_tx.send(state);
    }

    /// Publish a new status-line snapshot. Non-blocking.
    pub fn set_status(&self, state: StatusLineState) {
        let _ = self.status_tx.send(state);
    }

    /// Best-effort terminal cleanup for exit paths that cannot consume self
    /// (e.g. ExitCleanly via process::exit while Arc clones exist). Cancels
    /// the render loop, clears bottom rows, and resets the DECSTBM scroll region.
    pub fn cleanup_on_exit(&self) {
        self.shutdown.cancel();
        reset_scroll_region();
    }

    /// Shut down the render task cooperatively. Safe to call once; consumes
    /// self so the JoinHandle is awaited exactly once.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(h) = self.task.take() {
            let _ = h.await;
        }
        reset_scroll_region();
    }
}

/// Clear reserved bottom rows and reset DECSTBM scroll region to full terminal.
fn reset_scroll_region() {
    let mut out = stderr();
    if out.is_tty() {
        let Ok((_cols, rows)) = size() else { return };
        let _ = queue!(
            out,
            SavePosition,
            Hide,
            MoveTo(0, rows.saturating_sub(3)),
            Clear(ClearType::CurrentLine),
            MoveTo(0, rows.saturating_sub(2)),
            Clear(ClearType::CurrentLine),
            MoveTo(0, rows.saturating_sub(1)),
            Clear(ClearType::CurrentLine),
            Show,
            RestorePosition,
        );
        let _ = write!(out, "\x1b[r");
        let _ = out.flush();
    }
}

/// Position the terminal cursor at the fixed prompt row (row rows-3, outside
/// the scroll region). Call before `rl.readline()` so user input appears at
/// a stable position above the scanner and status bar.
pub fn prepare_prompt() {
    let mut out = stderr();
    if !out.is_tty() {
        return;
    }
    let Ok((_cols, rows)) = size() else { return };
    let prompt_row = rows.saturating_sub(3);
    let _ = queue!(
        out,
        MoveTo(0, prompt_row),
        Clear(ClearType::CurrentLine),
    );
    let _ = out.flush();
}

/// Clear the prompt row after `rl.readline()` returns and reposition the cursor
/// at the bottom of the scroll region so subsequent `println!()` output flows
/// naturally inside the scrollable content area.
pub fn finish_prompt() {
    use std::io::stdout;
    let _ = stdout().flush();
    let mut out = stderr();
    if !out.is_tty() {
        return;
    }
    let Ok((_cols, rows)) = size() else { return };
    let prompt_row = rows.saturating_sub(3);
    let scroll_bottom = rows.saturating_sub(4);
    let _ = queue!(
        out,
        MoveTo(0, prompt_row),
        Clear(ClearType::CurrentLine),
        MoveTo(0, scroll_bottom),
    );
    let _ = out.flush();
}

/// Main render loop. Per D-17 exits immediately if stderr is not a TTY.
async fn render_loop(
    activity_rx: watch::Receiver<ActivityState>,
    status_rx: watch::Receiver<StatusLineState>,
    shutdown: CancellationToken,
) {
    // D-17 graceful degradation: non-tty stderr (pipe/ssh/CI) → no-op loop.
    if !stderr().is_tty() {
        tracing::debug!("tui: stderr is not a tty, render loop is a no-op");
        shutdown.cancelled().await;
        return;
    }

    let mut ticker = tokio::time::interval(FRAME_PERIOD);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tick: u64 = 0;
    let mut prev_rows: u16 = size().map(|(_, r)| r).unwrap_or(0);

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                // DECSTBM resize tracking: update scroll region when terminal
                // height changes so the reserved area stays at the new bottom.
                if let Ok((_cols, rows)) = size() && rows != prev_rows && rows >= 4 {
                    let scroll_end = rows.saturating_sub(3);
                    let mut out = stderr();
                    let _ = write!(out, "\x1b[1;{}r", scroll_end);
                    let _ = out.flush();
                    prev_rows = rows;
                }

                let activity = activity_rx.borrow().clone();
                let status = status_rx.borrow().clone();
                if let Err(e) = redraw(tick, &activity, &status) {
                    // Re-query size on next tick; log and continue (T-21-08).
                    tracing::debug!(error = %e, "tui: redraw failed — continuing");
                }
                tick = tick.wrapping_add(1);
            }
        }
    }
}

/// Single-frame redraw. Per RESEARCH §Pitfall 4: re-query size() each tick
/// to handle SIGWINCH without subscribing to the signal. Per §Pitfall 3:
/// Hide/Show wraps the frame to prevent cursor flicker.
fn redraw(tick: u64, activity: &ActivityState, status: &StatusLineState) -> std::io::Result<()> {
    let (_cols, rows) = size()?;
    // Need at least 3 rows: prompt + scanner + status. On tiny terminals (<3 rows)
    // skip rendering to avoid colliding with the prompt.
    if rows < 4 {
        return Ok(());
    }
    let bottom = rows.saturating_sub(1);
    let scanner_row = rows.saturating_sub(2);

    let status_str = render_status_line(status);

    // Per D-08: scanner visible iff in-flight.
    let scanner_str: Option<String> = match activity {
        ActivityState::Idle => None,
        ActivityState::Streaming => Some(format!(
            "{} {}",
            knight_rider::frame(tick),
            "Streaming".dimmed()
        )),
        ActivityState::ToolCall { name } => Some(format!(
            "{} {} {}",
            knight_rider::frame(tick),
            "Running:".dimmed(),
            name.yellow()
        )),
    };

    let mut out = stderr();
    queue!(out, SavePosition, Hide)?;
    // Scanner row: either write the scanner string or clear it so the line
    // doesn't retain stale glyphs when activity returns to Idle.
    queue!(out, MoveTo(0, scanner_row), Clear(ClearType::CurrentLine))?;
    if let Some(ref s) = scanner_str {
        queue!(out, Print(s))?;
    }
    // Status row:
    queue!(
        out,
        MoveTo(0, bottom),
        Clear(ClearType::CurrentLine),
        Print(&status_str),
        Show,
        RestorePosition
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test uses #[tokio::test] with the default single-threaded runtime.
    // Non-tty path (stderr in `cargo test` is typically captured → not a tty)
    // means the render_loop exits immediately on `shutdown.cancelled()`.

    #[tokio::test]
    async fn construct_and_shutdown_no_panic() {
        let tui = TuiHandle::new_for_tests();
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn set_activity_published_to_receiver() {
        let tui = TuiHandle::new_for_tests();
        // Subscribe to the sender's channel to observe published values.
        let rx = tui.activity_tx.subscribe();
        tui.set_activity(ActivityState::Streaming);
        // Allow the update to propagate.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*rx.borrow(), ActivityState::Streaming);
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn set_status_published_to_receiver() {
        let tui = TuiHandle::new_for_tests();
        let rx = tui.status_tx.subscribe();
        tui.set_status(StatusLineState {
            mode: "Agent".to_string(),
            model_short: "m".to_string(),
            provider: "p".to_string(),
            tokens_used: 50,
            tokens_limit: 100,
            hint: String::new(),
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(rx.borrow().tokens_used, 50);
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_after_set_does_not_panic() {
        let tui = TuiHandle::new_for_tests();
        tui.set_activity(ActivityState::Streaming);
        // Shutdown — task join must complete cleanly.
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn double_construct_shutdown_is_safe() {
        // Two sequential TuiHandles must not deadlock or panic.
        let a = TuiHandle::new_for_tests();
        a.shutdown().await;
        let b = TuiHandle::new_for_tests();
        b.shutdown().await;
    }
}
