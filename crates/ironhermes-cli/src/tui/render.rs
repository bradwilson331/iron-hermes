//! Render task + TuiHandle — the ONLY I/O surface in the tui module.
//!
//! Per D-15/D-16: one tokio::task ticks every 100ms (D-07 frame rate), reads two
//! watch channels (activity + status), and writes to stderr via crossterm absolute
//! cursor positioning. Per D-17: non-tty stderr causes the loop to exit immediately
//! so piped output / CI / ssh-over-slow-link degrade gracefully.
//!
//! Flicker mitigation per RESEARCH §Pitfall 3: Hide/Show wraps every frame.
//! SIGWINCH tolerance per RESEARCH §Pitfall 4: re-query size() each tick.
//!
//! Phase 22.1 Plan 02: Extension widget slots, TuiEvent mpsc channel, dynamic
//! DECSTBM, merged style overrides applied in redraw_with_extensions.

use crate::tui::activity::ActivityState;
use crate::tui::extension::{LayoutSlot, StyleOverrides, TuiEvent, TuiExtension, Widget, MAX_WIDGET_HEIGHT};
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
use std::collections::HashMap;
use std::io::{stderr, Write};
use std::sync::atomic::{AtomicU16, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Frame period for the render loop (D-07: ~10 fps).
pub const FRAME_PERIOD: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// reserved_rows helper
// ---------------------------------------------------------------------------

/// Calculate the total reserved rows at the bottom of the terminal.
/// Base = 3 (prompt + scanner + status). Extensions add their widget heights
/// from AboveStatus and BelowScanner slots (StatusRight is inline, no extra rows).
fn reserved_rows(extension_widgets: &HashMap<String, (LayoutSlot, Widget)>) -> u16 {
    let extra: u16 = extension_widgets
        .values()
        .filter(|(slot, _)| *slot != LayoutSlot::StatusRight)
        .map(|(_, w)| w.height.min(MAX_WIDGET_HEIGHT))
        .sum();
    3 + extra
}

// ---------------------------------------------------------------------------
// merge_style_overrides helper
// ---------------------------------------------------------------------------

/// Merge StyleOverrides from all registered extensions into a single HashMap.
/// Extensions registered later have higher priority (last-wins on key conflict).
/// Logs a tracing::debug when a style key is overridden by a later extension.
fn merge_style_overrides(extensions: &[Box<dyn TuiExtension>]) -> StyleOverrides {
    let mut merged = StyleOverrides::new();
    for ext in extensions {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ext.style_overrides())) {
            Ok(overrides) => {
                for (key, value) in overrides {
                    if let Some(prev) = merged.insert(key.clone(), value) {
                        tracing::debug!(
                            "tui: extension '{}' overrides style slot '{}' (was '{}')",
                            ext.name(),
                            key,
                            prev
                        );
                    }
                }
            }
            Err(_) => {
                tracing::warn!(
                    "tui: extension '{}' panicked in style_overrides() -- skipping",
                    ext.name()
                );
            }
        }
    }
    merged
}

// ---------------------------------------------------------------------------
// TuiHandle
// ---------------------------------------------------------------------------

/// Public handle returned by `TuiHandle::new`. Holds the watch senders +
/// shutdown token + render task handle + optional extension event sender.
/// All setter methods are non-blocking and best-effort: a send on a closed
/// channel is silently dropped.
pub struct TuiHandle {
    activity_tx: watch::Sender<ActivityState>,
    status_tx: watch::Sender<StatusLineState>,
    event_tx: Option<mpsc::UnboundedSender<TuiEvent>>,
    shutdown: CancellationToken,
    task: Option<JoinHandle<()>>,
    extensions: Vec<Box<dyn TuiExtension>>,
    /// Shared reserved row count, updated by the render loop when widgets
    /// are added/removed at runtime (WR-01 fix: AtomicU16 replaces stale cache).
    reserved: Arc<AtomicU16>,
}

impl TuiHandle {
    /// Spawn the render task and return a handle. Always succeeds; if stderr
    /// is not a TTY the loop exits on first iteration (non-tty fallback D-17).
    /// Delegates to `new_with_extensions` with an empty extension list, preserving
    /// Phase 21 zero-extension behavior exactly.
    pub fn new(initial_status: StatusLineState) -> Self {
        Self::new_with_extensions(initial_status, Vec::new())
    }

    /// Spawn the render task with extension support. Extensions supply widgets,
    /// keybindings, style overrides, and command handlers.
    ///
    /// - Initial widgets from all extensions are collected and stored in a
    ///   `HashMap<String, (LayoutSlot, Widget)>` keyed by prefixed ID.
    /// - Style overrides from all extensions are merged (last-wins) and passed
    ///   to the render loop.
    /// - An mpsc event channel is created so extensions can push `TuiEvent`s
    ///   at runtime to update widget state.
    ///
    /// The zero-extension case (empty `extensions` vec) produces `reserved=3`
    /// and empty `StyleOverrides`, identical to Phase 21 behavior.
    pub fn new_with_extensions(
        initial_status: StatusLineState,
        extensions: Vec<Box<dyn TuiExtension>>,
    ) -> Self {
        let (activity_tx, activity_rx) = watch::channel(ActivityState::Idle);
        let (status_tx, status_rx) = watch::channel(initial_status);
        let shutdown = CancellationToken::new();
        let task_shutdown = shutdown.clone();

        // Collect initial widgets from all extensions.
        // Widget IDs are prefixed "{ext.name()}:{widget.id}" to prevent collisions (Pitfall 4).
        let mut widgets: HashMap<String, (LayoutSlot, Widget)> = HashMap::new();
        for ext in &extensions {
            let ext_widgets = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                ext.widgets()
            })) {
                Ok(w) => w,
                Err(_) => {
                    tracing::warn!(
                        "tui: extension '{}' panicked in widgets() during construction -- skipping",
                        ext.name()
                    );
                    continue;
                }
            };
            for (slot, widget) in ext_widgets {
                let prefixed_id = format!("{}:{}", ext.name(), widget.id);
                if widgets.contains_key(&prefixed_id) {
                    tracing::warn!(
                        "tui: widget id '{}' conflicts with existing widget -- ignoring",
                        prefixed_id
                    );
                } else {
                    let mut w = widget;
                    w.id = prefixed_id.clone();
                    widgets.insert(prefixed_id, (slot, w));
                }
            }
        }

        // Merge style overrides from all extensions.
        let merged_styles = merge_style_overrides(&extensions);

        // Calculate reserved rows and store in shared atomic (WR-01).
        let initial_reserved = reserved_rows(&widgets);
        let reserved = Arc::new(AtomicU16::new(initial_reserved));
        let reserved_atomic = reserved.clone(); // clone for the render loop

        // DECSTBM: set scroll region using dynamic reserved row count.
        if stderr().is_tty() && let Ok((_cols, rows)) = size() {
            let scroll_end = rows.saturating_sub(initial_reserved);
            if scroll_end > 0 {
                let mut out = stderr();
                let _ = write!(out, "\x1b[1;{}r", scroll_end);
                let _ = out.flush();
            }
        }

        // Create mpsc event channel for runtime widget updates.
        let (event_tx, event_rx) = mpsc::unbounded_channel::<TuiEvent>();

        let task = tokio::spawn(async move {
            render_loop(
                activity_rx,
                status_rx,
                event_rx,
                widgets,
                merged_styles,
                reserved_atomic,
                task_shutdown,
            )
            .await;
        });

        Self {
            activity_tx,
            status_tx,
            event_tx: Some(event_tx),
            shutdown,
            task: Some(task),
            extensions,
            reserved,
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

    /// Plan 21.7-07 (D-04 / Pitfall 8): clone of the status-line watch sender
    /// so off-render-path tasks can `send_modify` a single field (e.g. the
    /// subagent pill) without clobbering unrelated state like token counts.
    /// `send_modify` is sync — it never awaits — so the caller can invoke it
    /// from any task without leaking an `.await` into the render path.
    pub fn status_tx_handle(&self) -> watch::Sender<StatusLineState> {
        self.status_tx.clone()
    }

    /// Plan 21.7-07 (D-04): snapshot of the current status state. Used when
    /// a caller needs to reconstruct the full `StatusLineState` on a
    /// full-write path (e.g. `set_status` after a turn) and wants to preserve
    /// fields updated out-of-band by the pill-refresh path.
    pub fn status_snapshot(&self) -> StatusLineState {
        self.status_tx.borrow().clone()
    }

    /// Return a clone of the event sender, if available. Extensions or callers
    /// use this to push `TuiEvent`s to the render loop at runtime.
    pub fn event_sender(&self) -> Option<mpsc::UnboundedSender<TuiEvent>> {
        self.event_tx.clone()
    }

    /// Return the current reserved row count. Updated atomically by the
    /// render loop when widgets are added/removed at runtime (WR-01 fix).
    pub fn reserved_row_count(&self) -> u16 {
        self.reserved.load(AtomicOrdering::Relaxed)
    }

    /// Return a reference to the registered extensions slice.
    pub fn extensions(&self) -> &[Box<dyn TuiExtension>] {
        &self.extensions
    }

    /// Best-effort terminal cleanup for exit paths that cannot consume self
    /// (e.g. ExitCleanly via process::exit while Arc clones exist). Cancels
    /// the render loop, clears bottom rows, and resets the DECSTBM scroll region.
    pub fn cleanup_on_exit(&self) {
        self.shutdown.cancel();
        reset_scroll_region_with_reserve(self.reserved.load(AtomicOrdering::Relaxed));
    }

    /// Shut down the render task cooperatively. Safe to call once; consumes
    /// self so the JoinHandle is awaited exactly once.
    pub async fn shutdown(mut self) {
        self.shutdown.cancel();
        if let Some(h) = self.task.take() {
            let _ = h.await;
        }
        reset_scroll_region_with_reserve(self.reserved.load(AtomicOrdering::Relaxed));
    }
}

// ---------------------------------------------------------------------------
// Scroll region reset helpers
// ---------------------------------------------------------------------------

/// Clear reserved bottom rows and reset DECSTBM scroll region to full terminal.
/// Uses the given `reserved` row count to clear the correct number of rows.
fn reset_scroll_region_with_reserve(reserved: u16) {
    let mut out = stderr();
    if out.is_tty() {
        let Ok((_cols, rows)) = size() else { return };
        let _ = queue!(out, SavePosition, Hide);
        for i in 0..reserved {
            let _ = queue!(
                out,
                MoveTo(0, rows.saturating_sub(reserved - i)),
                Clear(ClearType::CurrentLine),
            );
        }
        let _ = queue!(out, Show, RestorePosition);
        let _ = write!(out, "\x1b[r");
        let _ = out.flush();
    }
}

/// Clear reserved bottom rows and reset DECSTBM scroll region to full terminal.
/// Backward-compatibility wrapper: clears exactly 3 rows (zero-extension case).
#[allow(dead_code)]
fn reset_scroll_region() {
    reset_scroll_region_with_reserve(3);
}

// ---------------------------------------------------------------------------
// Prompt positioning helpers
// ---------------------------------------------------------------------------

/// Plan 21.7-12 (GAP-21.7-02): pure helper returning the ANSI byte
/// sequence that positions the cursor at the fixed prompt row for the
/// given `(rows, reserved)` geometry. Same inputs always produce the
/// same bytes (deterministic; no scroll state, no TTY dependency), so
/// this function is testable without a live terminal.
///
/// Returns `None` when positioning is not meaningful:
/// - `rows < 4`: terminal too tiny (same guard as `redraw_with_extensions`).
/// - `reserved >= rows`: reserved area overflows the terminal.
///
/// The returned bytes are:
/// - `\x1b[{row};1H` (CUP — Cursor Position, 1-based row, column 1)
/// - `\x1b[2K` (Erase-Line entire line — safer than CurrentLine because
///   the worker does not know what may have been painted there since the
///   last frame).
///
/// The worker thread writes these bytes directly to `stderr` and flushes
/// immediately before calling `rl.readline(&prefix)`, closing the
/// cross-thread positioning race Plan 11 opened.
pub fn prompt_position_ansi(rows: u16, reserved: u16) -> Option<Vec<u8>> {
    if rows < 4 || reserved >= rows {
        return None;
    }
    let prompt_row = rows.saturating_sub(reserved);
    // crossterm's MoveTo is 0-based; CUP is 1-based — so we emit
    // `prompt_row + 1` to match the same row `prepare_prompt_with_reserve`
    // paints when it calls `MoveTo(0, prompt_row)`.
    let mut out = Vec::with_capacity(16);
    use std::io::Write as _;
    let _ = write!(&mut out, "\x1b[{};1H\x1b[2K", prompt_row + 1);
    Some(out)
}

/// Position the terminal cursor at the fixed prompt row (row rows-reserved,
/// outside the scroll region). Call before `rl.readline()` so user input
/// appears at a stable position above the scanner and status bar.
pub fn prepare_prompt_with_reserve(reserved: u16) {
    let mut out = stderr();
    if !out.is_tty() {
        return;
    }
    let Ok((_cols, rows)) = size() else { return };
    let prompt_row = rows.saturating_sub(reserved);
    let _ = queue!(
        out,
        MoveTo(0, prompt_row),
        Clear(ClearType::CurrentLine),
    );
    let _ = out.flush();
}

/// Position the terminal cursor at the fixed prompt row (row rows-3, outside
/// the scroll region). Call before `rl.readline()` so user input appears at
/// a stable position above the scanner and status bar.
/// Backward-compatibility wrapper: calls `prepare_prompt_with_reserve(3)`.
pub fn prepare_prompt() {
    prepare_prompt_with_reserve(3);
}

/// Clear the prompt row after `rl.readline()` returns and reposition the cursor
/// at the bottom of the scroll region so subsequent `println!()` output flows
/// naturally inside the scrollable content area.
pub fn finish_prompt_with_reserve(reserved: u16) {
    use std::io::stdout;
    let _ = stdout().flush();
    let mut out = stderr();
    if !out.is_tty() {
        return;
    }
    let Ok((_cols, rows)) = size() else { return };
    let prompt_row = rows.saturating_sub(reserved);
    let scroll_bottom = rows.saturating_sub(reserved + 1);
    let _ = queue!(
        out,
        MoveTo(0, prompt_row),
        Clear(ClearType::CurrentLine),
        MoveTo(0, scroll_bottom),
    );
    let _ = out.flush();
}

/// Clear the prompt row after `rl.readline()` returns and reposition the cursor
/// at the bottom of the scroll region so subsequent `println!()` output flows
/// naturally inside the scrollable content area.
/// Backward-compatibility wrapper: calls `finish_prompt_with_reserve(3)`.
pub fn finish_prompt() {
    finish_prompt_with_reserve(3);
}

// ---------------------------------------------------------------------------
// Render loop
// ---------------------------------------------------------------------------

/// Main render loop. Per D-17 exits immediately if stderr is not a TTY.
/// Accepts extension widget state and TuiEvent channel for runtime widget updates.
async fn render_loop(
    activity_rx: watch::Receiver<ActivityState>,
    status_rx: watch::Receiver<StatusLineState>,
    mut event_rx: mpsc::UnboundedReceiver<TuiEvent>,
    mut widgets: HashMap<String, (LayoutSlot, Widget)>,
    style_overrides: StyleOverrides,
    reserved_atomic: Arc<AtomicU16>,
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
    // FlashHint state: (message, remaining_ticks)
    let mut flash_hint: Option<(String, u16)> = None;

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => {
                // DECSTBM resize tracking: update scroll region when terminal
                // height changes so the reserved area stays at the new bottom.
                // WR-01: also publish to AtomicU16 so callers get current value.
                let reserved = reserved_rows(&widgets);
                reserved_atomic.store(reserved, AtomicOrdering::Relaxed);
                if let Ok((_cols, rows)) = size() && rows != prev_rows && rows >= 4 {
                    let scroll_end = rows.saturating_sub(reserved);
                    let mut out = stderr();
                    let _ = write!(out, "\x1b[1;{}r", scroll_end);
                    let _ = out.flush();
                    prev_rows = rows;
                }

                // Drain all pending TuiEvents from the mpsc channel (non-blocking).
                loop {
                    match event_rx.try_recv() {
                        Ok(TuiEvent::UpdateWidget { id, content, height }) => {
                            if let Some((_, widget)) = widgets.get_mut(&id) {
                                widget.content = content;
                                widget.height = height.min(MAX_WIDGET_HEIGHT);
                            } else {
                                tracing::debug!(
                                    "tui: UpdateWidget for unknown id '{}' -- ignoring",
                                    id
                                );
                            }
                        }
                        Ok(TuiEvent::RemoveWidget { id }) => {
                            if widgets.remove(&id).is_some() {
                                // Recalculate reserved rows and update DECSTBM.
                                let new_reserved = reserved_rows(&widgets);
                                if let Ok((_cols, rows)) = size() {
                                    let scroll_end = rows.saturating_sub(new_reserved);
                                    if scroll_end > 0 {
                                        let mut out = stderr();
                                        let _ = write!(out, "\x1b[1;{}r", scroll_end);
                                        let _ = out.flush();
                                    }
                                }
                            } else {
                                tracing::debug!(
                                    "tui: RemoveWidget for unknown id '{}' -- ignoring",
                                    id
                                );
                            }
                        }
                        Ok(TuiEvent::FlashHint { message, duration_ticks }) => {
                            flash_hint = Some((message, duration_ticks));
                        }
                        Err(mpsc::error::TryRecvError::Empty) => break,
                        Err(mpsc::error::TryRecvError::Disconnected) => break,
                    }
                }

                // Decrement flash hint counter.
                if let Some((_, ref mut ticks)) = flash_hint {
                    if *ticks == 0 {
                        flash_hint = None;
                    } else {
                        *ticks -= 1;
                    }
                }

                let activity = activity_rx.borrow().clone();
                let status = status_rx.borrow().clone();

                if let Err(e) = redraw_with_extensions(tick, &activity, &status, &widgets, &style_overrides, &flash_hint) {
                    // Re-query size on next tick; log and continue (T-21-08).
                    tracing::debug!(error = %e, "tui: redraw failed — continuing");
                }
                tick = tick.wrapping_add(1);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Redraw functions
// ---------------------------------------------------------------------------

/// Single-frame redraw with extension widget compositing and style override support.
/// Per RESEARCH §Pitfall 4: re-query size() each tick to handle SIGWINCH.
/// Per §Pitfall 3: Hide/Show wraps the frame to prevent cursor flicker.
///
/// Rendering order (bottom to top):
/// 1. Status bar (rows-1): same as before, with optional StatusRight widget content
///    appended and status.separator override applied.
/// 2. BelowScanner widgets: rendered between scanner and status rows.
/// 3. Scanner row (rows-2 when no BelowScanner widgets): knight rider or clear.
/// 4. AboveStatus widgets: rendered above scanner row.
fn redraw_with_extensions(
    tick: u64,
    activity: &ActivityState,
    status: &StatusLineState,
    widgets: &HashMap<String, (LayoutSlot, Widget)>,
    style_overrides: &StyleOverrides,
    flash_hint: &Option<(String, u16)>,
) -> std::io::Result<()> {
    let (_cols, rows) = size()?;
    // Need at least 3 rows: prompt + scanner + status. On tiny terminals (<4 rows)
    // skip rendering to avoid colliding with the prompt.
    if rows < 4 {
        return Ok(());
    }

    // Extract style override values with defaults.
    let scanner_lit_color = style_overrides
        .get("scanner.lit")
        .map(|s| s.as_str())
        .unwrap_or("bright cyan");
    let scanner_trail_color = style_overrides
        .get("scanner.trail")
        .map(|s| s.as_str())
        .unwrap_or("cyan");
    let _scanner_bg_color = style_overrides
        .get("scanner.bg")
        .map(|s| s.as_str())
        .unwrap_or("");
    let _status_separator_color = style_overrides
        .get("status.separator")
        .map(|s| s.as_str())
        .unwrap_or("");
    let hint_override = style_overrides.get("status.hint").map(|s| s.as_str());

    // Collect widgets by slot.
    let mut above_status: Vec<&Widget> = widgets
        .values()
        .filter(|(slot, _)| *slot == LayoutSlot::AboveStatus)
        .map(|(_, w)| w)
        .collect();
    above_status.sort_by(|a, b| a.id.cmp(&b.id));

    let mut below_scanner: Vec<&Widget> = widgets
        .values()
        .filter(|(slot, _)| *slot == LayoutSlot::BelowScanner)
        .map(|(_, w)| w)
        .collect();
    below_scanner.sort_by(|a, b| a.id.cmp(&b.id));

    let status_right: Vec<&Widget> = widgets
        .values()
        .filter(|(slot, _)| *slot == LayoutSlot::StatusRight)
        .map(|(_, w)| w)
        .collect();

    // Build effective status string: base status + optional flash hint + StatusRight widgets.
    let mut effective_status = status.clone();
    if let Some((msg, _)) = flash_hint {
        effective_status.hint = msg.clone();
    }
    if let Some(color) = hint_override {
        // WR-04 fix: "status.hint" is not yet implemented as a render-time color
        // override. Log at debug level so extension authors get feedback instead
        // of silently dropping the value.
        tracing::debug!(
            "tui: style slot 'status.hint' override '{}' accepted but not yet applied \
             -- hint color rendering is a future enhancement",
            color
        );
    }
    let mut status_str = render_status_line(&effective_status);

    // Append StatusRight widget content inline after status pills.
    for w in &status_right {
        let dot_sep = format!(" {} ", "·".dimmed());
        status_str.push_str(&dot_sep);
        // Truncate to avoid oversized inline content.
        let truncated: String = w.content.chars().take(_cols as usize).collect();
        status_str.push_str(&truncated);
    }

    // Per D-08: scanner visible iff in-flight.
    // Apply style overrides to scanner colors.
    let scanner_str: Option<String> = match activity {
        ActivityState::Idle => None,
        ActivityState::Streaming => Some({
            let frame = build_scanner_frame(tick, scanner_lit_color, scanner_trail_color);
            format!("{} {}", frame, "Streaming".dimmed())
        }),
        ActivityState::ToolCall { name } => Some({
            let frame = build_scanner_frame(tick, scanner_lit_color, scanner_trail_color);
            format!("{} {} {}", frame, "Running:".dimmed(), name.yellow())
        }),
    };

    let mut out = stderr();
    queue!(out, SavePosition, Hide)?;

    // Calculate row positions from bottom up.
    let bottom = rows.saturating_sub(1); // status bar row

    // BelowScanner widgets occupy rows between scanner and status bar.
    let below_scanner_total_height: u16 = below_scanner
        .iter()
        .map(|w| w.height.min(MAX_WIDGET_HEIGHT))
        .sum();
    let scanner_row = rows
        .saturating_sub(2)
        .saturating_sub(below_scanner_total_height);

    // AboveStatus widgets occupy rows above scanner.
    // (No explicit rendering row needed — rendered upward from scanner_row.)

    // Render BelowScanner widgets (between scanner and status).
    let mut cur_row = scanner_row + 1;
    for w in &below_scanner {
        let h = w.height.min(MAX_WIDGET_HEIGHT);
        for (i, line) in w.content.lines().enumerate() {
            if i >= h as usize {
                break;
            }
            let truncated: String = line.chars().take(_cols as usize).collect();
            queue!(
                out,
                MoveTo(0, cur_row),
                Clear(ClearType::CurrentLine),
                Print(&truncated)
            )?;
            cur_row += 1;
        }
        // Clear any remaining lines within height allocation.
        for _ in w.content.lines().count()..h as usize {
            queue!(out, MoveTo(0, cur_row), Clear(ClearType::CurrentLine))?;
            cur_row += 1;
        }
    }

    // Scanner row.
    queue!(out, MoveTo(0, scanner_row), Clear(ClearType::CurrentLine))?;
    if let Some(ref s) = scanner_str {
        queue!(out, Print(s))?;
    }

    // AboveStatus widgets (rendered in rows above scanner_row, going upward).
    // WR-03 fix: stop rendering when row_cursor would enter the scroll region
    // (row 0) to prevent overwriting scrollable content above the TUI area.
    if !above_status.is_empty() {
        let mut row_cursor = scanner_row.saturating_sub(1);
        for w in above_status.iter().rev() {
            if row_cursor == 0 {
                break; // WR-03: no more room above -- stop to avoid overwriting content
            }
            let h = w.height.min(MAX_WIDGET_HEIGHT);
            let lines: Vec<&str> = w.content.lines().collect();
            let line_count = lines.len().min(h as usize);
            for i in (0..line_count).rev() {
                if row_cursor == 0 {
                    break; // WR-03: stop before overwriting scroll region
                }
                let truncated: String = lines[i].chars().take(_cols as usize).collect();
                queue!(
                    out,
                    MoveTo(0, row_cursor),
                    Clear(ClearType::CurrentLine),
                    Print(&truncated)
                )?;
                row_cursor -= 1;
            }
        }
    }

    // Status row.
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

/// Build a knight-rider scanner frame with style override colors.
/// When colors match defaults, produces the same output as Phase 21.
fn build_scanner_frame(tick: u64, lit_color: &str, trail_color: &str) -> String {
    // If defaults are requested, delegate directly to knight_rider::frame for
    // exact Phase 21 output fidelity.
    if lit_color == "bright cyan" && trail_color == "cyan" {
        return knight_rider::frame(tick);
    }

    // Build custom-colored frame using the same triangle-wave algorithm.
    let track_width = knight_rider::TRACK_WIDTH;
    let period = (track_width as u64 - 1) * 2;
    let phase = tick % period;
    let lit = if phase < track_width as u64 {
        phase as usize
    } else {
        (period - phase) as usize
    };

    (0..track_width)
        .map(|i| {
            let distance = (i as i32 - lit as i32).unsigned_abs() as usize;
            match distance {
                0 => "█".color(lit_color).to_string(),
                1 => "▓".color(trail_color).to_string(),
                2 => "▒".color(trail_color).dimmed().to_string(),
                _ => "░".dimmed().to_string(),
            }
        })
        .collect::<String>()
}

/// Single-frame redraw (zero-extension fast path). Per RESEARCH §Pitfall 4:
/// re-query size() each tick to handle SIGWINCH without subscribing to the signal.
/// Per §Pitfall 3: Hide/Show wraps the frame to prevent cursor flicker.
#[allow(dead_code)]
fn redraw(tick: u64, activity: &ActivityState, status: &StatusLineState) -> std::io::Result<()> {
    redraw_with_extensions(tick, activity, status, &HashMap::new(), &StyleOverrides::new(), &None)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::extension::{CommandResult, LayoutSlot, TuiExtension, Widget};

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
            active_subagents: 0,
            max_subagents: 0,
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

    // --- Phase 22.1 Plan 02 new tests ---

    #[tokio::test]
    async fn construct_with_empty_extensions_and_shutdown() {
        let tui = TuiHandle::new_with_extensions(StatusLineState::default(), Vec::new());
        // reserved should be 3 for empty extensions
        assert_eq!(tui.reserved_row_count(), 3);
        tui.shutdown().await;
    }

    #[tokio::test]
    async fn event_sender_returns_some() {
        struct NoOpExt;
        impl TuiExtension for NoOpExt {
            fn name(&self) -> &str { "noop" }
        }
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(NoOpExt)];
        let tui = TuiHandle::new_with_extensions(StatusLineState::default(), exts);
        assert!(tui.event_sender().is_some(), "event_sender should return Some after construction with extensions");
        tui.shutdown().await;
    }

    #[test]
    fn reserved_rows_empty_is_three() {
        let widgets: HashMap<String, (LayoutSlot, Widget)> = HashMap::new();
        assert_eq!(reserved_rows(&widgets), 3);
    }

    #[test]
    fn reserved_rows_with_above_status_widget() {
        let mut widgets = HashMap::new();
        let w = Widget::new("id1", "content", 2);
        widgets.insert("ext:id1".to_string(), (LayoutSlot::AboveStatus, w));
        // base 3 + height 2 = 5
        assert_eq!(reserved_rows(&widgets), 5);
    }

    #[test]
    fn reserved_rows_status_right_not_counted() {
        let mut widgets = HashMap::new();
        let w = Widget::new("id2", "content", 3);
        widgets.insert("ext:id2".to_string(), (LayoutSlot::StatusRight, w));
        // StatusRight is inline, no extra rows
        assert_eq!(reserved_rows(&widgets), 3);
    }

    #[test]
    fn merge_style_overrides_empty_extensions() {
        let exts: Vec<Box<dyn TuiExtension>> = vec![];
        let merged = merge_style_overrides(&exts);
        assert!(merged.is_empty());
    }

    #[test]
    fn merge_style_overrides_single_extension() {
        struct StyleExt;
        impl TuiExtension for StyleExt {
            fn name(&self) -> &str { "style_ext" }
            fn style_overrides(&self) -> StyleOverrides {
                let mut m = StyleOverrides::new();
                m.insert("scanner.lit".to_string(), "green".to_string());
                m
            }
        }
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(StyleExt)];
        let merged = merge_style_overrides(&exts);
        assert_eq!(merged.get("scanner.lit").map(|s| s.as_str()), Some("green"));
    }

    #[test]
    fn merge_style_overrides_later_wins() {
        struct EarlyExt;
        impl TuiExtension for EarlyExt {
            fn name(&self) -> &str { "early" }
            fn style_overrides(&self) -> StyleOverrides {
                let mut m = StyleOverrides::new();
                m.insert("scanner.lit".to_string(), "red".to_string());
                m
            }
        }
        struct LateExt;
        impl TuiExtension for LateExt {
            fn name(&self) -> &str { "late" }
            fn style_overrides(&self) -> StyleOverrides {
                let mut m = StyleOverrides::new();
                m.insert("scanner.lit".to_string(), "blue".to_string());
                m
            }
        }
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(EarlyExt), Box::new(LateExt)];
        let merged = merge_style_overrides(&exts);
        // Later extension (LateExt) wins
        assert_eq!(merged.get("scanner.lit").map(|s| s.as_str()), Some("blue"));
    }

    #[test]
    fn merge_style_overrides_panic_extension_skipped() {
        struct PanicStyleExt;
        impl TuiExtension for PanicStyleExt {
            fn name(&self) -> &str { "panic_style" }
            fn style_overrides(&self) -> StyleOverrides {
                panic!("intentional panic in style_overrides");
            }
        }
        // Should not propagate panic
        let exts: Vec<Box<dyn TuiExtension>> = vec![Box::new(PanicStyleExt)];
        let merged = merge_style_overrides(&exts);
        assert!(merged.is_empty());
    }

    // Suppress unused import warning for CommandResult in tests
    #[allow(dead_code)]
    fn _use_command_result() -> CommandResult {
        CommandResult::Silent
    }

    // ---- Plan 21.7-12 prompt_position_ansi tests ----

    /// Plan 21.7-12: pure helper is deterministic — same inputs produce
    /// identical bytes on every call. This is the structural property
    /// that, combined with worker-thread emission, gives GAP-21.7-02 its
    /// fix (floating prompt can no longer happen because the positioning
    /// bytes are (a) deterministic, (b) emitted on the same thread as
    /// `rl.readline`).
    #[test]
    fn prompt_position_ansi_is_deterministic() {
        let a = prompt_position_ansi(30, 3).expect("valid geometry");
        let b = prompt_position_ansi(30, 3).expect("valid geometry");
        assert_eq!(
            a, b,
            "prompt_position_ansi must be pure: same (rows, reserved) MUST produce identical bytes"
        );
        // rows=30, reserved=3 -> prompt_row=27 (0-based) -> CUP row 28 (1-based).
        let s = std::str::from_utf8(&a).expect("utf-8");
        assert!(
            s.contains("\x1b[28;1H"),
            "expected CUP to row 28 for rows=30 reserved=3; got: {:?}",
            s
        );
        assert!(
            s.contains("\x1b[2K"),
            "expected erase-line (\\x1b[2K); got: {:?}",
            s
        );
    }

    /// Plan 21.7-12: refuses invalid geometry so the worker skips the
    /// write entirely rather than emitting a broken ANSI sequence.
    #[test]
    fn prompt_position_ansi_refuses_invalid_geometry() {
        assert_eq!(
            prompt_position_ansi(3, 3),
            None,
            "rows<4 must return None (tiny-terminal guard)"
        );
        assert_eq!(
            prompt_position_ansi(2, 1),
            None,
            "rows<4 must return None even when reserved<rows"
        );
        assert_eq!(
            prompt_position_ansi(5, 5),
            None,
            "reserved==rows must return None (reserved area overflows)"
        );
        assert_eq!(
            prompt_position_ansi(5, 6),
            None,
            "reserved>rows must return None"
        );
        // Sanity: just-barely-valid geometry returns Some.
        assert!(
            prompt_position_ansi(4, 3).is_some(),
            "rows=4 reserved=3 is valid (row 1 / CUP row 2)"
        );
    }
}
