//! Phase 36.3.7.11 Plan 01 Wave 0 stub — replaced in Wave 1 (Task 2) with
//! source-string assertions over `src/server/kanban_ws.rs`.
//!
//! Locks: D-08 (WS lifecycle parity + `WS_KEEPALIVE_INTERVAL = 5s` + close
//! frame on teardown), D-15 (tail consumer independence — no
//! `use ironhermes_kanban::notifier` import). Source-read assertion pattern
//! mirrors tests/websocket_lifecycle_parity.rs.

#[test]
fn placeholder_will_be_replaced_in_wave_1() {
    // TODO Plan 01 Task 3 — Wave 1 fills in the source-string assertions.
}
