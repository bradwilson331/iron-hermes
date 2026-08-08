//! Pure frame-render function for the tui_rata REPL (Phase 22.4).
//!
//! Template: /Users/you/code/tmon/src/main.rs `ui()` fn (lines 564–624).
//! 4-chunk vertical layout per CONTEXT §specifics:
//! - Min(5) transcript (Paragraph — per RESEARCH Open Question §4)
//! - Length(1) knight-rider row (rendered only when in-flight)
//! - Length(1) status pills row (D-10)
//! - Length(3) tui-textarea input (D-05)
//!
//! Takes `&App` (not `&mut`) so plan 22.4-10's TestBackend snapshot tests
//! render deterministically.

use ansi_to_tui::IntoText;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::tui_rata::app::{App, word_wrapped_line_count};
use crate::tui_rata::knight_rider;
use crate::tui_rata::status_line::render_status_line_ratatui;

/// Pure render function for the ratatui REPL frame.
///
/// Splits `frame.area()` into 4 vertical chunks and renders each:
/// - chunks[0]: Transcript (Paragraph — WARNING-07 lock)
/// - chunks[1]: Knight-rider animation row (blank when idle)
/// - chunks[2]: Status pills row
/// - chunks[3]: tui-textarea input
///
/// No side effects; no mutation of `app`; no stdout writes.
pub fn ui(frame: &mut Frame, app: &App) {
    // Phase 36.6.2 Plan 02 (D-01): expanded thinking pane prepends a 5th
    // chunk. The `else` branch below is BYTE-IDENTICAL to the pre-phase
    // 4-chunk layout — every existing render_* call keeps its exact
    // relative order and constraints when `thinking_expanded` is false.
    if app.thinking_expanded {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(8), // thinking pane (D-01)
                Constraint::Min(5),    // transcript
                Constraint::Length(1), // knight-rider (blank when idle)
                Constraint::Length(1), // status pills
                Constraint::Length(3), // tui-textarea input
            ])
            .split(frame.area());

        render_thinking_panel(frame, app, chunks[0]);
        render_transcript(frame, app, chunks[1]);
        render_knight_rider(frame, app, chunks[2]);
        render_status(frame, app, chunks[3]);
        render_input(frame, app, chunks[4]);
        render_cursor(frame, app, chunks[4]);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // transcript
            Constraint::Length(1), // knight-rider (blank when idle)
            Constraint::Length(1), // status pills
            Constraint::Length(3), // tui-textarea input
        ])
        .split(frame.area());

    render_transcript(frame, app, chunks[0]);
    render_knight_rider(frame, app, chunks[1]);
    render_status(frame, app, chunks[2]);
    render_input(frame, app, chunks[3]);
    render_cursor(frame, app, chunks[3]);
}

/// Expanded thinking pane (Phase 36.6.2 Plan 02, D-01/D-02 refinement).
/// `Block::bordered()` (Plain border, matches transcript/input) with two
/// titles: left `" Thinking "`, right `" Ctrl+T "`. Renders the idle DIM
/// placeholder when `thinking_lines` is empty, otherwise the buffered
/// activity feed auto-scrolled so the newest line is the last visible row.
fn render_thinking_panel(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(" Thinking ").left_aligned())
        .title(Line::from(" Ctrl+T ").right_aligned());

    if app.thinking_lines.is_empty() {
        let placeholder = Line::from(Span::styled(
            "No active reasoning yet.",
            Style::default().add_modifier(Modifier::DIM),
        ));
        let paragraph = Paragraph::new(placeholder)
            .block(block)
            .wrap(Wrap { trim: false });
        frame.render_widget(paragraph, area);
        return;
    }

    // memory `feedback_scroll_width_inner` / RESEARCH: wrap math MUST use the
    // INNER width (border excluded), recomputed every render, never cached —
    // mirrors `transcript_max_scroll`'s discipline in app.rs.
    let inner_width = area.width.saturating_sub(2) as usize;
    let wrapped_line_count: usize = app
        .thinking_lines
        .iter()
        .map(|line| word_wrapped_line_count(line, inner_width))
        .sum();
    // Auto-scroll: newest line is always the last visible content row.
    //
    // WR-01: visible rows MUST be derived from the actual inner height
    // (`area.height - 2`, border excluded), not a hardcoded `6`. At the
    // normal `Length(8)` pane this is byte-identical (8 - 2 = 6); it only
    // changes behavior when ratatui squeezes the pane below 8 rows at small
    // terminal heights, where the hardcoded constant under-scrolled and
    // clipped the newest line below the viewport.
    let visible_rows = (area.height.saturating_sub(2) as usize).max(1);
    let scroll = wrapped_line_count.saturating_sub(visible_rows).min(u16::MAX as usize) as u16;

    let text = app.thinking_lines.join("\n");
    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, area);
}

fn render_transcript(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!("Chat [{}]", app.scroll_indicator(area));
    let block = Block::default().borders(Borders::ALL).title(title);
    // Phase 46.7 Plan 07 (D-17): rebuild the chip hit-test map for THIS
    // area/scroll before rendering — handle_mouse (app.rs) consults it on
    // the next input event using the same coordinate space `compute_transcript_area`
    // gives `handle_event` (event_loop.rs), one frame later.
    app.rebuild_chip_hit_test(area);
    let text = app.transcript_render_text();
    // RESEARCH Open Question §4 commits to Paragraph for v1. If UAT
    // surfaces lag on >1000-line transcripts, follow-up phase can swap
    // to tui-scrollview. INV-22.4-style acceptance grep locks this choice.
    let paragraph = Paragraph::new(text)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((app.transcript_scroll, 0));
    frame.render_widget(paragraph, area);

    // D-01..D-05: Scrollbar always visible, inside border, right edge, default style.
    // ScrollbarState built per-render from authoritative App fields — no cached state.
    // area.inner(Margin{vertical:1, horizontal:1}) trims all four border cells so the
    // track renders at column width-2 (inside the right border) not on the border char.
    //
    // Use inner_width (border excluded) so the line count matches what ratatui's
    // Paragraph actually wraps at. viewport_content_length makes the thumb reach
    // the bottom of the track when the view is at the bottom of the content.
    //
    // Phase 46.7 Plan 07: shared with `transcript_max_scroll`/
    // `rebuild_chip_hit_test` via `inner_transcript_width` so this never
    // re-derives `area.width - 2` independently (memory
    // `feedback_scroll_width_inner`).
    let inner_width = crate::tui_rata::app::inner_transcript_width(area);
    let visible_rows = area.height.saturating_sub(2) as usize;
    // Phase 46.7 Plan 07: total_line_count (not the base-only line_count) so
    // the scrollbar accounts for the Plan 07 chip rows too.
    let total = app.transcript_total_line_count(inner_width);
    let mut scrollbar_state = ScrollbarState::new(total)
        .position(app.transcript_scroll as usize)
        .viewport_content_length(visible_rows);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        }),
        &mut scrollbar_state,
    );
}

fn render_knight_rider(frame: &mut Frame, app: &App, area: Rect) {
    if app.pending_rx.is_none() {
        frame.render_widget(Paragraph::new(""), area);
        return;
    }
    let ansi_string = knight_rider::frame(app.knight_rider_tick);
    let text = ansi_string.as_bytes().into_text().unwrap_or_default();
    frame.render_widget(Paragraph::new(text), area);
}

fn render_status(frame: &mut Frame, app: &App, area: Rect) {
    // Phase 36.17.3 (D-09 / RESEARCH Pitfall 5): read queue depth + paused
    // state LIVE per render pass — never cached in `app.status` because the
    // cached value would drift by one tick. The clone is cheap (StatusLineState
    // is Clone) and isolates the per-frame override from the persistent
    // app.status fields owned by the rest of the system.
    let mut state = app.status.clone();
    let queue_paused_now = app.queue_paused.load(std::sync::atomic::Ordering::Relaxed);
    state.queue_depth = app.queue.len(&app.queue_key);
    state.queue_paused = queue_paused_now;
    // Phase 36.17.8 (D-08): voice pill — populated live per frame from VoiceState
    // so it never lags by one tick (same discipline as queue_depth above).
    state.voice_phase = if app.voice.is_enabled() {
        use crate::tui_rata::status_line::VoicePillPhase;
        use crate::tui_rata::voice_state::RecordPhase;
        Some(match app.voice.current_phase() {
            RecordPhase::Idle => VoicePillPhase::Ready,
            RecordPhase::Listening => VoicePillPhase::Listening,
            RecordPhase::Transcribing => VoicePillPhase::Transcribing,
        })
    } else {
        None
    };
    let line = render_status_line_ratatui(&state);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_input(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(&app.textarea, area);
}

fn render_cursor(frame: &mut Frame, app: &App, area: Rect) {
    let (row, col) = app.textarea.cursor();
    // UAT Gap 1 (Phase 22.4 Plan 22.4-14): the textarea now wears
    // Block::default().borders(Borders::ALL).title("Prompt"). The borders
    // consume row 0 + column 0 of the chunk, so the typeable interior
    // starts at (area.y + 1, area.x + 1). Bump both offsets by +1 so the
    // visible caret lands inside the bordered region.
    let cursor_x = area.x.saturating_add(col as u16).saturating_add(1);
    let cursor_y = area.y.saturating_add(row as u16).saturating_add(1);
    frame.set_cursor_position(Position::new(cursor_x, cursor_y));
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn ui_renders_four_chunks_in_80x24() {
        let app = App::new_test_empty();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
    }

    #[test]
    fn ui_idle_knight_rider_chunk_is_blank() {
        let app = App::new_test_empty();
        assert!(app.pending_rx.is_none());
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
    }

    #[test]
    fn scrollbar_renders_in_right_column_when_content_overflows() {
        // Seed enough short lines to overflow a 24-row viewport.
        // Each line <= 7 chars; "Hermes: " prefix uses 8 cols; remaining ~65 cols are spaces.
        // Column 78 is therefore a space PRE-fix (no Scrollbar yet) and a thumb char POST-fix.
        let body = (1..=25)
            .map(|i| format!("ln{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let app =
            App::new_test_with_messages(vec![("assistant", Box::leak(body.into_boxed_str()))]);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        // The Scrollbar (D-01..D-05) renders at column 78 — area.inner(Margin{vertical:1, horizontal:1})
        // trims all four border rows/cols. Right border at col 79; inner right edge = col 78.
        // Track occupies column 78 in the content rows (rows 1..17 are safe, away from border noise).
        // Column 78 rows 1..17 are the transcript CONTENT rows (well inside the block,
        // away from any border chars that appear at rows 17+ from adjacent blocks).
        // Pre-fix: all spaces. Post-fix: Scrollbar track/thumb chars appear here.
        let has_scrollbar = (1u16..17).any(|row| {
            buf.cell((78, row))
                .map(|c| c.symbol() != " ")
                .unwrap_or(false)
        });
        assert!(
            has_scrollbar,
            "expected scrollbar thumb in column 78 rows 1..17 (transcript content area) when \
             content overflows; got all-space. Buffer dump for col 78 rows 1..17: {:?}",
            (1u16..17)
                .map(|r| buf.cell((78, r)).map(|c| c.symbol().to_string()))
                .collect::<Vec<_>>()
        );
    }

    /// D-03 — end-to-end auto-scroll integration test.
    ///
    /// Proves that after `StreamEvent::Finished` (simulated via `scroll_to_bottom`
    /// and `reconcile_scroll`), the transcript scrolls to the true visual bottom and
    /// the last in-viewport content row is non-blank when rendered to an 80x24
    /// TestBackend.
    #[test]
    fn auto_scroll_lands_at_true_bottom_after_stream_finished() {
        use crate::tui_rata::app::App;
        use crate::tui_rata::event_loop::compute_transcript_area;

        // Build a body long enough that max_scroll > 0 in a 24-row terminal.
        // 25 lines each ~50 chars — creates ~25 wrapped rows; viewport = 22 (24 - 2 border).
        let long_body: &'static str = Box::leak(
            (1..=25)
                .map(|i| format!("Line {:02}: some assistant content here.", i))
                .collect::<Vec<_>>()
                .join("\n")
                .into_boxed_str(),
        );
        let mut app = App::new_test_with_messages(vec![("assistant", long_body)]);

        let size = ratatui::prelude::Size {
            width: 80,
            height: 24,
        };
        let transcript_area = compute_transcript_area(size);

        // Simulate StreamEvent::Finished: mirrors handle_stream_event's scroll_to_bottom call
        // (sets auto_follow = true, transcript_scroll = 0).
        app.scroll_to_bottom();

        // reconcile_scroll sets transcript_scroll = transcript_max_scroll(area) when auto_follow.
        app.reconcile_scroll(transcript_area);

        let max = app.transcript_max_scroll(transcript_area);
        assert!(
            max > 0,
            "test setup: expected max_scroll > 0, got 0 — adjust body length or terminal height"
        );
        assert_eq!(
            app.transcript_scroll, max,
            "auto-scroll must land at true bottom: scroll={}, max={}",
            app.transcript_scroll, max
        );

        // Visual assertion: render at scroll=max and confirm the last content row is non-blank.
        //
        // Layout for 80x24 terminal with [Min(5), Length(1), Length(1), Length(3)]:
        //   chunks[0] (transcript block) = 24 - 1 - 1 - 3 = 19 rows  (y=0..18 in buffer)
        //   chunks[1] (knight-rider)     = 1 row  (y=19)
        //   chunks[2] (status)           = 1 row  (y=20)
        //   chunks[3] (input)            = 3 rows (y=21..23)
        //
        // Transcript block border: top border at y=0, bottom border at y=18.
        // Content rows: y=1..17 (inner height = 19 - 2 = 17 rows).
        // Last content row = y=17.
        //
        // Col 78 is the scrollbar track; col 79 is the right border. Test cols 1..78.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();
        // transcript_area.height = 19; last content row = 19 - 2 = 17 (0-indexed in buffer)
        let last_visible_row = transcript_area.height - 2; // = 17
        let last_row_non_blank = (1u16..78u16).any(|col| {
            buf.cell((col, last_visible_row))
                .map(|c| c.symbol() != " ")
                .unwrap_or(false)
        });
        assert!(
            last_row_non_blank,
            "last visible row (row {}) must be non-blank after auto-scroll to bottom — \
             got all blanks, content is below viewport. transcript_area={:?}, scroll={}, max={}",
            last_visible_row, transcript_area, app.transcript_scroll, max
        );
    }

    // — Phase 36.6.2 Plan 02: thinking panel render tests ───────────────────

    /// Read row `row` of an 80-wide `TestBackend` buffer as a plain string,
    /// for substring assertions (never re-derives layout math, per D-10).
    fn row_string(buf: &ratatui::buffer::Buffer, row: u16) -> String {
        (0..80u16)
            .map(|col| {
                buf.cell((col, row))
                    .map(|c| c.symbol().to_string())
                    .unwrap_or_else(|| " ".to_string())
            })
            .collect()
    }

    #[test]
    fn thinking_panel_collapsed_renders_unchanged_4chunk_layout() {
        let app = App::new_test_empty();
        assert!(!app.thinking_expanded, "test setup: must start collapsed");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        // The transcript's top border/title must be at row 0 — unchanged
        // from the pre-phase 4-chunk layout (no thinking pane chunk inserted).
        let row0 = row_string(buf, 0);
        assert!(
            row0.contains("Chat ["),
            "row 0 must be the transcript's top border/title when collapsed; got: {row0:?}"
        );
        for row in 0..24u16 {
            let line = row_string(buf, row);
            assert!(
                !line.contains("Thinking"),
                "collapsed layout must never render the thinking pane title; found at row {row}: {line:?}"
            );
        }
    }

    #[test]
    fn thinking_panel_expanded_renders_5th_chunk_above_transcript() {
        let mut app = App::new_test_empty();
        app.thinking_expanded = true;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        let row0 = row_string(buf, 0);
        assert!(
            row0.contains("Thinking"),
            "row 0 must show the bordered Thinking pane title when expanded; got: {row0:?}"
        );
        // Length(8) pane occupies rows 0..7 — transcript's top border is pushed to row 8.
        let row8 = row_string(buf, 8);
        assert!(
            row8.contains("Chat ["),
            "transcript top border must be pushed to row 8 above the Length(8) thinking pane; got: {row8:?}"
        );
    }

    #[test]
    fn thinking_panel_idle_shows_placeholder_text() {
        let mut app = App::new_test_empty();
        app.thinking_expanded = true;
        assert!(app.thinking_lines.is_empty(), "test setup: no activity buffered");
        assert!(app.pending_rx.is_none(), "test setup: no turn in flight");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        let found = (0..8u16).any(|row| row_string(buf, row).contains("No active reasoning yet."));
        assert!(found, "idle expanded pane must show the DIM placeholder text");
    }

    #[test]
    fn thinking_panel_autoscroll_keeps_newest_visible() {
        let mut app = App::new_test_empty();
        app.thinking_expanded = true;
        // 10 short one-row lines — well over the 6 visible content rows.
        for i in 0..10 {
            app.thinking_lines.push(format!("line {i}"));
        }
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| ui(f, &app)).unwrap();
        let buf = terminal.backend().buffer();

        // Pane rows 0..7 (Length(8)); border rows 0 and 7; content rows 1..6.
        let last_content_row = row_string(buf, 6);
        assert!(
            last_content_row.contains("line 9"),
            "the newest line must be the last visible content row; got: {last_content_row:?}"
        );
        for row in 0..8u16 {
            assert!(
                !row_string(buf, row).contains("line 0"),
                "the oldest line must have scrolled off the top; found \"line 0\" at row {row}"
            );
        }
    }

    /// The off-by-one auto-scroll contract (must_haves, non-backstop): at
    /// exactly 6 wrapped lines everything fits (scroll=0); at 7 lines the
    /// oldest scrolls off (scroll=1).
    #[test]
    fn thinking_panel_autoscroll_off_by_one_boundary() {
        let mut app6 = App::new_test_empty();
        app6.thinking_expanded = true;
        for i in 0..6 {
            app6.thinking_lines.push(format!("line {i}"));
        }
        let backend6 = TestBackend::new(80, 24);
        let mut terminal6 = Terminal::new(backend6).unwrap();
        terminal6.draw(|f| ui(f, &app6)).unwrap();
        let buf6 = terminal6.backend().buffer();
        assert!(
            row_string(buf6, 1).contains("line 0"),
            "at exactly 6 lines, scroll must be 0 (oldest line still visible); row1={:?}",
            row_string(buf6, 1)
        );
        assert!(
            row_string(buf6, 6).contains("line 5"),
            "newest line must be the last content row; row6={:?}",
            row_string(buf6, 6)
        );

        let mut app7 = App::new_test_empty();
        app7.thinking_expanded = true;
        for i in 0..7 {
            app7.thinking_lines.push(format!("line {i}"));
        }
        let backend7 = TestBackend::new(80, 24);
        let mut terminal7 = Terminal::new(backend7).unwrap();
        terminal7.draw(|f| ui(f, &app7)).unwrap();
        let buf7 = terminal7.backend().buffer();
        assert!(
            !row_string(buf7, 1).contains("line 0"),
            "at 7 lines, the oldest line must have scrolled off; row1={:?}",
            row_string(buf7, 1)
        );
        assert!(
            row_string(buf7, 1).contains("line 1"),
            "at 7 lines with scroll=1, row1 must show line 1; row1={:?}",
            row_string(buf7, 1)
        );
        assert!(
            row_string(buf7, 6).contains("line 6"),
            "newest line must be the last content row; row6={:?}",
            row_string(buf7, 6)
        );
    }

    /// WR-01 regression: at a reduced pane height (fewer than 6 inner rows),
    /// the newest `thinking_lines` entry must still be the last visible
    /// content row. Calls `render_thinking_panel` directly against a real
    /// `TestBackend` frame with a squeezed `Rect` (height 5 -> inner height
    /// 3), bypassing the full 5-chunk `ui()` layout so the pane height is
    /// controlled directly rather than depending on how ratatui's solver
    /// squeezes the `Length(8)` constraint. Before the fix, the hardcoded
    /// `- 6` under-scrolled here (visible=3, but subtracted 6), clipping the
    /// newest line below the viewport.
    #[test]
    fn thinking_panel_autoscroll_at_reduced_height_keeps_newest_visible() {
        let mut app = App::new_test_empty();
        app.thinking_expanded = true;
        for i in 0..10 {
            app.thinking_lines.push(format!("line {i}"));
        }

        let backend = TestBackend::new(80, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        let area = Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 5,
        };
        terminal
            .draw(|f| render_thinking_panel(f, &app, area))
            .unwrap();
        let buf = terminal.backend().buffer();

        // height=5 -> inner height = 3 -> content rows 1..3, last = row 3.
        let last_content_row = row_string(buf, 3);
        assert!(
            last_content_row.contains("line 9"),
            "at a squeezed pane height (inner=3 rows), the newest line must still be \
             the last visible content row; got: {last_content_row:?}"
        );
        assert!(
            !row_string(buf, 1).contains("line 0") && !row_string(buf, 2).contains("line 0"),
            "older lines must have scrolled off at the reduced height"
        );
    }
}
