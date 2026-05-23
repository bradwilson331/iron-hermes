//! Pure frame-render function for the tui_rata REPL (Phase 22.4).
//!
//! Template: /Users/twilson/code/tmon/src/main.rs `ui()` fn (lines 564–624).
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
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use crate::tui_rata::app::App;
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

fn render_transcript(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!("Chat [{}]", app.scroll_indicator(area));
    let block = Block::default().borders(Borders::ALL).title(title);
    let text = app.transcript_text();
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
    let inner_width = area.width.saturating_sub(2) as usize;
    let visible_rows = area.height.saturating_sub(2) as usize;
    let total = app.transcript_line_count(inner_width);
    let mut scrollbar_state = ScrollbarState::new(total)
        .position(app.transcript_scroll as usize)
        .viewport_content_length(visible_rows);
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin { vertical: 1, horizontal: 1 }),
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
    let line = render_status_line_ratatui(&app.status);
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
        let app = App::new_test_with_messages(vec![("assistant", Box::leak(body.into_boxed_str()))]);
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
            (1u16..17).map(|r| buf.cell((78, r)).map(|c| c.symbol().to_string())).collect::<Vec<_>>()
        );
    }
}
