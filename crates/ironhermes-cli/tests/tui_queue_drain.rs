//! Phase 36.17.3 D-04/D-05/D-11/D-12 — FIFO drain + negative control regression.
//!
//! This file is a Wave 2 (Plan 03) skeleton: all `#[test]` stubs are marked
//! `#[ignore = "Plan 06 fills body"]` and exercise the shared `make_queue` +
//! `make_key` helpers only. Plan 06 replaces each body with the real
//! D-11 / D-12 assertions per VALIDATION.md §"Per-Task Verification Map".

#![cfg(feature = "test-support")]

use ironhermes_cli::tui_rata::app::App;
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

#[test]
#[ignore = "Plan 06 fills body"]
fn tui_queue_drain_fifo() {
    let _ = make_queue();
    let _ = make_key();
    let _ = std::any::type_name::<App>();
    let _ = std::any::type_name::<StreamEvent>();
    let _ = std::any::type_name::<dyn MessageQueue<SessionKey>>();
    let _ = Ordering::SeqCst;
    // Plan 06 will fill in the real assertions per D-11 / D-12.
}

#[test]
#[ignore = "Plan 06 fills body"]
fn tui_queue_regression_d11() {
    let _ = make_queue();
    let _ = make_key();
    // Plan 06 will fill in the real assertions per D-11 / D-12.
}

#[test]
#[ignore = "Plan 06 fills body"]
fn tui_queue_regression_negative_control() {
    let _ = make_queue();
    let _ = make_key();
    // Plan 06 will fill in the real assertions per D-11 / D-12.
}
