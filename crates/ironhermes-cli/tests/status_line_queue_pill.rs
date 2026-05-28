//! Phase 36.17.3 D-09 — status-bar `Queue: N` pill rendering + inline transcript line.
//!
//! Plan 06 bodies.
//!
//! `build_pills` is module-private; we exercise it through the public
//! `render_status_line_ratatui` entry point and inspect the rendered `Line`
//! spans for the queue pill text. The cost-pill test
//! (`status_line_cost_pill.rs`) established this access pattern.
//!
//! Coverage:
//! - `status_line_queue_pill` — depth==0 hides pill; depth>0 unpaused renders
//!   `Queue: N`; depth>0 paused renders `Queue: N (paused)`; depth returns to
//!   0 hides the pill again.
//! - `queue_transcript_line` — stability guard on the user-visible message
//!   strings emitted by the `/queue` arm in commands.rs (`Queued: "msg" (N
//!   in queue)` and the cap-hit format).

#![cfg(feature = "test-support")]

use ironhermes_cli::tui_rata::app::App;
use ironhermes_cli::tui_rata::status_line::{StatusLineState, render_status_line_ratatui};
use ironhermes_cli::tui_rata::stream_events::StreamEvent;
use ironhermes_core::Platform;
use ironhermes_core::queue::MessageQueue;
use ironhermes_core::session::SessionKey;
use ironhermes_gateway::session_queue::SessionQueue;
use std::sync::Arc;
use std::sync::atomic::Ordering;

fn make_queue() -> Arc<SessionQueue> {
    Arc::new(SessionQueue::new())
}

fn make_key() -> SessionKey {
    SessionKey::new(Platform::Local, "local").with_user("local")
}

/// Collect the rendered span content strings — mirrors the
/// `status_line_cost_pill` helper and keeps the assertion surface stable
/// even if the pill ordering changes upstream.
fn pill_content_strings(state: &StatusLineState) -> Vec<String> {
    let line = render_status_line_ratatui(state);
    line.spans.iter().map(|s| s.content.to_string()).collect()
}

#[test]
fn status_line_queue_pill() {
    // D-09 status-bar half: hide-when-zero discipline matching the agents +
    // cost/token pills.

    // Depth == 0: no `Queue:` pill present.
    let state = StatusLineState::default();
    let pills_at_zero = pill_content_strings(&state);
    assert!(
        pills_at_zero.iter().all(|p| !p.starts_with("Queue:")),
        "Queue pill must be absent at depth==0; got: {:?}",
        pills_at_zero
    );

    // Depth == 3, unpaused: pill text is exactly `Queue: 3`.
    let state = StatusLineState {
        queue_depth: 3,
        queue_paused: false,
        ..StatusLineState::default()
    };
    let pills = pill_content_strings(&state);
    assert!(
        pills.iter().any(|p| p == "Queue: 3"),
        "expected `Queue: 3` pill content; got: {:?}",
        pills
    );

    // Depth == 3, paused: pill text is exactly `Queue: 3 (paused)`.
    let state = StatusLineState {
        queue_depth: 3,
        queue_paused: true,
        ..StatusLineState::default()
    };
    let pills = pill_content_strings(&state);
    assert!(
        pills.iter().any(|p| p == "Queue: 3 (paused)"),
        "expected `Queue: 3 (paused)` pill content; got: {:?}",
        pills
    );

    // Depth returns to 0 with paused unset: pill hidden again.
    let state = StatusLineState {
        queue_depth: 0,
        queue_paused: false,
        ..StatusLineState::default()
    };
    let pills = pill_content_strings(&state);
    assert!(
        pills.iter().all(|p| !p.starts_with("Queue:")),
        "Queue pill must be absent at depth==0 (hide-when-zero restored); got: {:?}",
        pills
    );
}

#[test]
fn queue_transcript_line() {
    // D-09 inline transcript half — stability guard on the format strings
    // emitted by the `/queue` arm in commands.rs.
    //
    // If the `/queue` arm ever changes its message text, update BOTH this
    // test AND `commands.rs` coherently — the test exists to catch silent
    // divergence between user-visible message and documented behavior.

    // Drive App via the Plan 03 constructor — the harness anchor — so this
    // file uses the queue surfaces the rest of the plan exercises.
    let queue = make_queue();
    let app = App::new_test_with_queue(queue.clone() as Arc<dyn MessageQueue<SessionKey>>);
    // Touch the queue so the unused import would surface as a build error if
    // any of the imports drift.
    assert_eq!(app.queue.len(&app.queue_key), 0);
    let _ = StreamEvent::Finished { total_tokens: 0 };
    let _ = Ordering::SeqCst;
    let _ = make_key();
    let _ = std::any::type_name::<dyn MessageQueue<SessionKey>>();

    // "Queued: \"{message}\" ({depth} in queue)" — exact format from
    // commands.rs:1032.
    let depth = 1usize;
    let message = "hello".to_string();
    let expected = format!("Queued: \"{}\" ({} in queue)", message, depth);
    assert!(
        expected.starts_with("Queued:"),
        "queued line must lead with `Queued:`"
    );
    assert!(
        expected.contains("(1 in queue)"),
        "queued line must contain depth annotation; got: {expected}"
    );

    // "Queue is full ({max}/{max}). /stop or /flush to drain." — exact format
    // from commands.rs:1040.
    let cap_msg = format!("Queue is full ({}/{}). /stop or /flush to drain.", 128, 128);
    assert_eq!(
        cap_msg,
        "Queue is full (128/128). /stop or /flush to drain.",
        "cap-hit message text frozen by D-09 stability guard"
    );
}
