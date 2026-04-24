//! Phase 22.4 D-19 Layer 2 — ratatui `TestBackend` + `insta` snapshot tests
//! for canonical frames of the tui_rata REPL.
//!
//! Requires the `test-support` feature to access `App::new_test_empty()`
//! and `App::new_test_with_messages(...)` constructors (plan 22.4-05).
//!
//! Knight-rider frames seed `app.knight_rider_tick = 5` for determinism.
//! In-flight frames additionally seed `app.pending_rx = Some(rx)` so the
//! ui.rs `pending_rx.is_some()` guard fires (WARNING-01).
//!
//! Snapshot acceptance workflow: `cargo insta review` after first run.

#![cfg(feature = "test-support")]

use ironhermes_cli::tui_rata::{App, StreamEvent};
use ironhermes_cli::tui_rata::ui::ui;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn render(app: &App) -> String {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| ui(f, app)).unwrap();
    format!("{}", terminal.backend())
}

/// Helper — seed a throwaway unbounded channel so `pending_rx.is_some()`
/// is true, which gates knight-rider rendering in ui.rs (WARNING-01).
fn seed_in_flight(app: &mut App) {
    let (_tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    app.pending_rx = Some(rx);
}

// ——————————————————————————————————————————————————————————————————————
// Frame 1: Empty transcript + empty input
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_empty_transcript() {
    let app = App::new_test_empty();
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 2: Two-message conversation (User + Hermes)
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_two_message_conversation() {
    let app = App::new_test_with_messages(vec![
        ("user", "hello"),
        ("assistant", "hi there"),
    ]);
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 3: In-flight streaming — partial delta in assistant_buffer.
// pending_rx seeded so knight-rider row is active (WARNING-01 fix).
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_in_flight_streaming_partial_delta() {
    let mut app = App::new_test_with_messages(vec![("user", "hello")]);
    app.handle_stream_event(StreamEvent::Started);
    app.handle_stream_event(StreamEvent::Delta("partial resp".into()));
    app.knight_rider_tick = 5;
    seed_in_flight(&mut app); // WARNING-01: activate knight-rider row
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 4: Tool-call activity row.
// pending_rx seeded so knight-rider row is active (WARNING-01 fix).
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_tool_call_activity_row() {
    let mut app = App::new_test_with_messages(vec![("user", "run bash")]);
    app.handle_stream_event(StreamEvent::Started);
    app.handle_stream_event(StreamEvent::ToolCall { name: "bash".into() });
    app.knight_rider_tick = 5;
    seed_in_flight(&mut app); // WARNING-01
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 5: Scroll-active indicator.
// reconcile_scroll clamps transcript_scroll to a legal range so the
// snapshot cannot lock an over-scrolled state (WARNING-05 fix).
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_scroll_active_indicator() {
    let lines: Vec<(&str, &str)> = (0..30).map(|i| {
        if i % 2 == 0 { ("user", "Lorem ipsum dolor sit amet") }
        else          { ("assistant", "consectetur adipiscing elit") }
    }).collect();
    let mut app = App::new_test_with_messages(lines);
    app.auto_follow = false;
    app.transcript_scroll = 20;

    // WARNING-05: clamp to transcript_max_scroll so the indicator shows a
    // real in-range "scroll N/M" state. Transcript chunk on 80x24 is
    // height = HEIGHT - kr(1) - status(1) - input(3) = 19.
    let transcript_area = Rect::new(0, 0, WIDTH, HEIGHT - 5);
    app.reconcile_scroll(transcript_area);

    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 6: Double-Ctrl-C pending-exit warning.
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_double_ctrl_c_pending_exit_warning() {
    let mut app = App::new_test_empty();
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
    let key = KeyEvent {
        code: KeyCode::Char('c'),
        modifiers: KeyModifiers::CONTROL,
        kind: KeyEventKind::Press,
        state: crossterm::event::KeyEventState::empty(),
    };
    app.handle_event(Event::Key(key), Rect::new(0, 0, WIDTH, HEIGHT - 5));
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 7: Error banner (status hint shows "error: …").
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_error_banner() {
    let mut app = App::new_test_with_messages(vec![("user", "do something")]);
    app.handle_stream_event(StreamEvent::Started);
    app.handle_stream_event(StreamEvent::Error("api error".into()));
    insta::assert_snapshot!(render(&app));
}

// ——————————————————————————————————————————————————————————————————————
// Frame 8: 3-line multi-line input in textarea.
// ——————————————————————————————————————————————————————————————————————
#[test]
fn snap_three_line_multiline_input() {
    let mut app = App::new_test_empty();
    app.load_history_entry("line one\nline two\nline three");
    insta::assert_snapshot!(render(&app));
}
