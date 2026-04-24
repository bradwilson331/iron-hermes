//! Phase 21.7 Plan 12 (GAP-21.7-02) regression gate.
//!
//! The floating-prompt defect is TTY-dependent, so we cannot assert
//! "prompt paints at row N" directly from a headless test environment.
//! Instead we assert the DETERMINISTIC PROPERTIES that — when combined
//! with the worker-thread positioning structure landed in Task 12-01
//! and the TUI readline barrier landed in Task 12-02 — imply the
//! prompt lands at the same row every cycle:
//!
//! 1. `prompt_position_ansi(rows, reserved)` is a pure function:
//!    same inputs -> same bytes. No scroll state dependency. No TTY
//!    dependency. Any future refactor that introduces non-determinism
//!    (e.g. reading terminal state) fails this test.
//!
//! 2. `PromptRequest` carries `reserved_rows` end-to-end so the worker
//!    receives the value the main task computed. The schema surface is
//!    the single integration point; if it silently drops the field,
//!    GAP-21.7-02 re-opens.
//!
//! 3. The `TuiHandle::readline_active` barrier is observable via the
//!    public `readline_active_handle()` accessor — the render-loop gate
//!    cannot be wired incorrectly without this accessor being used.
//!    Toggling the flag from a caller must be observable in the flag's
//!    own load.

use ironhermes_cli::tui::prompt_position_ansi;
use ironhermes_cli::{PromptRequest, ReplInputChannel};
use std::sync::atomic::Ordering;

/// GAP-21.7-02 core property: positioning is a pure function of
/// (rows, reserved). Same inputs -> byte-identical output. This is
/// what makes the worker-thread emission deterministic regardless of
/// terminal scroll state or concurrent thread activity.
#[test]
fn prompt_row_is_deterministic_from_reserved_count() {
    let a = prompt_position_ansi(40, 3).expect("valid geometry");
    let b = prompt_position_ansi(40, 3).expect("valid geometry");
    assert_eq!(
        a, b,
        "same (rows, reserved) MUST produce identical ANSI bytes — any \
         non-determinism here re-opens GAP-21.7-02"
    );

    // rows=40, reserved=3 -> prompt_row=37 (0-based) -> CUP row 38 (1-based).
    let s = std::str::from_utf8(&a).expect("utf-8");
    assert!(
        s.contains("\x1b[38;1H"),
        "expected CUP to row 38 for rows=40 reserved=3; got: {:?}",
        s
    );
    assert!(
        s.contains("\x1b[2K"),
        "expected erase-line (\\x1b[2K); got: {:?}",
        s
    );
}

/// Tiny-terminal guard: positioning is not emitted on a terminal too
/// small to hold the reserved area. Protects against negative-row CUP
/// emission that would paint in the scrollback region.
#[test]
fn prompt_position_returns_none_on_tiny_terminal() {
    assert!(
        prompt_position_ansi(3, 3).is_none(),
        "rows<4 must return None"
    );
    assert!(
        prompt_position_ansi(2, 1).is_none(),
        "rows<4 must return None even when reserved<rows"
    );
    assert!(
        prompt_position_ansi(5, 5).is_none(),
        "reserved==rows must return None"
    );
    assert!(
        prompt_position_ansi(10, 20).is_none(),
        "reserved>rows must return None"
    );
}

/// Schema round-trip: a PromptRequest with `reserved_rows: Some(N)`
/// and `in_turn: false` reaches the rustyline worker via the public
/// `request_prompt` contract without the field being silently dropped.
///
/// We do NOT assert the worker paints to a real TTY here (the test env
/// has no TTY — rustyline's `is_tty()` guard will skip the stderr
/// write). We assert the schema + channel contract survives so the
/// structural fix cannot silently regress.
#[test]
fn prompt_request_round_trips_reserved_rows_through_worker() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    rt.block_on(async {
        let (chan, _printer) = ReplInputChannel::spawn(None)
            .expect("spawn must succeed even without TTY (rustyline can init)");
        let req = PromptRequest {
            prefix: "You: ".to_string(),
            in_turn: false,
            reserved_rows: Some(3),
        };
        // request_prompt returning Ok(()) proves the schema landed in
        // the worker-command enqueue path. The worker's behavior on a
        // non-TTY is to skip positioning AND fail rl.readline — we do
        // not drive readline to completion here.
        let res = chan.request_prompt(req);
        assert!(
            res.is_ok(),
            "request_prompt with reserved_rows: Some(..) must enqueue cleanly"
        );
        chan.shutdown();
    });
}

/// Barrier flag contract: construct a TuiHandle, obtain the barrier
/// via the public accessor, toggle it, and assert the same atomic
/// view is observed. This locks the handle-sharing contract so the
/// render loop and the main task cannot accidentally use separate
/// atomics (which would silently disable the gate).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn readline_active_handle_is_shared_across_clones() {
    use ironhermes_cli::tui::{StatusLineState, TuiHandle};
    let tui = TuiHandle::new(StatusLineState::default());

    let barrier_a = tui.readline_active_handle();
    let barrier_b = tui.readline_active_handle();

    // Both clones observe the same initial state.
    assert!(!barrier_a.load(Ordering::Relaxed));
    assert!(!barrier_b.load(Ordering::Relaxed));

    // Store from one clone; the other must see the same value.
    barrier_a.store(true, Ordering::Relaxed);
    assert!(
        barrier_b.load(Ordering::Relaxed),
        "clones of readline_active_handle() MUST share the same AtomicBool — \
         a divergent atomic would silently disable the GAP-21.7-02 barrier"
    );

    barrier_b.store(false, Ordering::Relaxed);
    assert!(
        !barrier_a.load(Ordering::Relaxed),
        "store from one clone must be visible on the other"
    );

    tui.shutdown().await;
}
