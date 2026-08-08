use std::fs;
use std::path::PathBuf;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &str) -> String {
    fs::read_to_string(crate_root().join(path)).expect("failed to read source file")
}

#[test]
fn server_ws_runs_turn_in_spawned_task_and_streams_concurrently() {
    let ws = read("src/server/ws.rs");
    assert!(
        ws.contains("#[cfg(feature = \"server\")]\nuse tokio::sync::mpsc;")
            && ws.contains("#[cfg(feature = \"server\")]\nuse tokio::task::JoinHandle;")
            && (ws.contains("#[cfg(feature = \"server\")]\nuse tracing::warn;")
                || ws.contains("#[cfg(feature = \"server\")]\nuse tracing::{info, warn};")),
        "server-only websocket runtime imports must remain cfg-gated"
    );
    assert!(
        ws.contains("#[cfg(feature = \"server\")]\n    let app_state =")
            && ws.contains("#[cfg(feature = \"server\")]\n                {")
            && ws.contains("#[cfg(not(feature = \"server\"))]"),
        "ws_chat must keep explicit feature-boundary branches"
    );
    assert!(
        ws.contains("tokio::spawn"),
        "ws_chat must spawn the turn execution task"
    );
    // Phase 39.1 Plan 02 (R39.1-01/R39.1-06) replaced the single-turn
    // `Option<InFlightTurn>` + `maybe_event = async { turn.rx.recv().await }`
    // select arm with a `HashMap<TurnId, InFlightTurn>` drained concurrently
    // via a `StreamMap` — multiple turns per session can stream at once, up
    // to `concurrency.session_turn_cap`. Lock the StreamMap plumbing that
    // replaced it: the map is constructed once, each spawned turn's receiver
    // is inserted keyed by its `TurnId`, and the select loop drains all of
    // them concurrently via `stream_map.next()`.
    assert!(
        ws.contains("StreamMap::new()"),
        "ws_chat must construct a StreamMap to drain multiple in-flight turns concurrently"
    );
    assert!(
        ws.contains("stream_map.insert("),
        "ws_chat must register each spawned turn's receiver into the StreamMap, keyed by TurnId"
    );
    assert!(
        ws.contains("stream_map.next()"),
        "ws_chat must forward events by draining the StreamMap in the select! loop"
    );
}

#[test]
fn malformed_request_path_is_recoverable_and_send_failures_abort_turn() {
    let ws = read("src/server/ws.rs");
    assert!(
        ws.contains("#[get(\"/api/ws/chat\")]"),
        "ws route annotation must remain /api/ws/chat"
    );
    assert!(
        ws.contains("Invalid request:"),
        "ws_chat must emit protocol errors for malformed JSON"
    );
    // WR-03: anchor `continue;` to the malformed-request branch (within 500 chars
    // after the first `Invalid request:` marker) instead of a global file match.
    // The window is 500 bytes to accommodate the send_raw call between the error
    // format and the continue; statement.
    let inv_pos = ws
        .find("Invalid request:")
        .expect("ws_chat must emit `Invalid request:` for malformed JSON");
    let window_end = (inv_pos + 500).min(ws.len());
    assert!(
        ws[inv_pos..window_end].contains("continue;"),
        "ws_chat malformed request branch must `continue;` within 500 chars after the `Invalid request:` error send"
    );

    // WR-03: anchor abort assertion to the verbatim call site + adjacent log
    // message rather than any `abort()` substring elsewhere in the file.
    assert!(
        ws.contains("turn.handle.abort();"),
        "ws_chat must call `turn.handle.abort();` on socket send failure"
    );
    // Phase 39.1 Plan 02 (R39.1-01/R39.1-06): the singular `Option<InFlightTurn>`
    // became a `HashMap<TurnId, InFlightTurn>` (multiple concurrent turns per
    // session), so the send-failure log message pluralized from "aborting
    // in-flight turn" to "aborting all in-flight turns" (it now drains every
    // entry in the map, not a single Option).
    assert!(
        ws.contains("websocket send failed; aborting all in-flight turns"),
        "ws_chat must log the send-failure abort message near the abort call site"
    );
}

#[test]
fn server_ws_disconnect_teardown_distinguishes_clean_recv_from_broken_send() {
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("websocket recv closed; exiting connection")
            || ws.contains("websocket recv closed cleanly; exiting connection")
            || ws.contains("websocket close frame received; exiting connection"),
        "clean websocket recv closure should log a clean-exit warning"
    );

    assert!(
        ws.contains("websocket recv failed; closing connection")
            || ws.contains("websocket recv failed; aborting connection"),
        "recv error path should remain explicitly classified"
    );

    // Phase 39.1 Plan 02: pluralized from "aborting in-flight turn" to
    // "aborting all in-flight turns" when the singular Option<InFlightTurn>
    // became a HashMap<TurnId, InFlightTurn> drained on send failure.
    assert!(
        ws.contains("websocket send failed; aborting all in-flight turns"),
        "send failure path must stay classified as transport-broken and abort in-flight turns"
    );

    assert!(
        ws.contains("session_id = %") && ws.contains("reason = %") && ws.contains("in_flight ="),
        "disconnect telemetry must include session_id, reason, and in_flight fields"
    );
}

#[test]
fn client_ws_receiver_retries_after_disconnect_and_resets_transient_state() {
    let ui = read("src/components/warp_hermes.rs");
    assert!(
        ui.contains("crate::protocol::ChatRequest")
            && ui.contains("crate::protocol::ChatStreamEvent"),
        "client websocket protocol types must come from crate::protocol"
    );
    assert!(
        !ui.contains("crate::server::ws::ChatRequest")
            && !ui.contains("crate::server::ws::ChatStreamEvent"),
        "client websocket code must not depend on server::ws protocol paths"
    );
    assert!(
        ui.contains("with_automatic_reconnect()"),
        "client websocket initialization must keep automatic reconnect enabled"
    );
    assert!(
        ui.contains("loop {") && ui.contains("let state = ws.connect().await"),
        "client receiver must use an outer reconnect cycle"
    );
    assert!(
        ui.contains("Err(err) => {")
            && ui.contains("scanner_active.set(false);")
            && ui.contains("streaming_block_id.set(None);")
            && ui.contains("continue;"),
        "disconnect/error path must reset transient streaming UI state"
    );
    assert!(
        ui.contains("let _ = ws.send_raw("),
        "submit/rerun websocket sends must remain non-panicking"
    );
}

#[test]
fn client_ws_disconnect_resets_streaming_state_and_reconnects() {
    // Retargeted from legacy warp_hermes.rs to the active HermesApp websocket
    // client (hoisted to the root component, src/components/hermes_app/mod.rs).
    // The legacy user-facing disconnect notice (push_disconnect_notice + the
    // "Connection interrupted..." transcript copy) was intentionally dropped in
    // the wheel-driven shell: on disconnect it silently clears the streaming
    // indicator and reconnects, letting the user retry. This test locks that
    // close/error-boundary contract instead of the removed notice copy.
    let app = read("src/components/hermes_app/mod.rs");

    assert!(
        app.contains("with_automatic_reconnect()"),
        "client websocket must keep automatic reconnect enabled"
    );
    assert!(
        app.contains("loop {") && app.contains("ws.connect().await"),
        "client receiver must use an outer reconnect cycle"
    );
    assert!(
        app.contains("Ok(dioxus_fullstack::Message::Close { .. }) => {")
            && app.contains("streaming_id.set(None);")
            && app.contains("is_ws_connected.set(false);")
            && app.contains("break;"),
        "a close frame must clear the streaming indicator, mark ws disconnected, and break to reconnect once"
    );
    assert!(
        app.contains("Err(_) => {"),
        "the recv error path must be explicitly handled (break + reconnect), not panic"
    );
}

#[test]
fn server_ws_emits_close_frame_on_every_teardown_branch() {
    // HUMAN-UAT Gap 3 regression lock: the server must send a WebSocket
    // close frame before dropping the socket on every teardown branch so
    // proxies/clients never observe `Connection reset without closing
    // handshake`. The close-frame send is best-effort (errors ignored) so
    // it does not block teardown on broken-send paths (D-06 intent).
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("fn send_close_frame("),
        "server teardown must funnel close-frame emission through a single helper"
    );

    assert!(
        ws.contains("CloseCode") && ws.contains("Message"),
        "close-frame helper must reference CloseCode and Message types from dioxus_fullstack"
    );

    assert!(
        ws.contains("Message::Close {"),
        "send_close_frame must emit a WebSocket Close variant"
    );

    assert!(
        ws.contains("CloseCode::Normal") && ws.contains("CloseCode::Away"),
        "teardown branches must classify close codes (Normal for clean, Away for failure)"
    );

    // Every break; that exits the ws_chat loop must be preceded by a
    // send_close_frame(...) call. Count invocations and breaks to keep
    // the invariant regression-locked without getting tripped by
    // unrelated formatting changes.
    let close_frame_calls = ws.matches("send_close_frame(").count();
    // Definition + 4 call sites (clean recv, broken recv, broken send,
    // keepalive ping failure).
    assert!(
        close_frame_calls >= 5,
        "expected send_close_frame to be invoked at every teardown branch \
         (clean recv close, broken recv, broken send, keepalive failure); \
         found {close_frame_calls} occurrence(s)"
    );

    assert!(
        ws.contains("\"recv closed cleanly\"")
            && ws.contains("\"recv failed\"")
            && ws.contains("\"send failed\"")
            && ws.contains("\"keepalive failed\""),
        "each teardown branch must carry a distinct close-frame reason string for telemetry parity"
    );
}

#[test]
fn server_ws_emits_application_level_keepalive_ping() {
    // HUMAN-UAT Gap 3 follow-up regression lock: the server must emit
    // application-level WebSocket Ping frames on a periodic interval
    // while otherwise idle, so `dx serve` / hyper (and any other
    // intermediate proxy) does not idle-close the connection after ~9s
    // and surface the drop to the browser as
    // `WebsocketError::ConnectionClosed` with no server-side teardown
    // trace. The keepalive interval must be well under common proxy
    // idle thresholds (10s), and Ping failure must be classified as a
    // send-path failure per D-05 (abort in-flight turn, close frame,
    // break).
    let ws = read("src/server/ws.rs");

    assert!(
        ws.contains("WS_KEEPALIVE_INTERVAL"),
        "keepalive interval must be a named constant for observability"
    );

    assert!(
        ws.contains("Duration::from_secs(5)"),
        "keepalive interval must default to 5 seconds (< 10s proxy idle threshold)"
    );

    assert!(
        ws.contains("tokio::time::interval(WS_KEEPALIVE_INTERVAL)"),
        "keepalive must drive the ping cadence via tokio::time::interval"
    );

    assert!(
        ws.contains("MissedTickBehavior::Skip"),
        "keepalive must skip missed ticks rather than bursting Pings after wake-up"
    );

    assert!(
        ws.contains("keepalive.tick().await")
            && ws.contains("_ = keepalive.tick() =>"),
        "keepalive must participate in the tokio::select! loop and consume the first immediate tick"
    );

    assert!(
        ws.contains("Message::Ping(Bytes::new())"),
        "keepalive must emit a WebSocket Ping frame (browsers auto-pong at protocol level)"
    );

    assert!(
        ws.contains("websocket keepalive ping failed; closing connection"),
        "failed keepalive Ping must classify as a transport-broken send failure"
    );
}

#[test]
fn busy_gate_opportunistically_clears_finished_turn() {
    let ws = read("src/server/ws.rs");

    // Phase 39.1 Plan 02 (R39.1-01/R39.1-03/R39.1-09) replaced the WR-02
    // mechanism wholesale: the Phase 36.1 singular `Option<InFlightTurn>`
    // busy flag (which needed an explicit "opportunistic clear" step run
    // BEFORE the busy check, or a finished turn would wrongly read as still
    // busy) is gone. Concurrency is now gated by a semaphore
    // (`ConcurrencyLayer::try_acquire`), and each turn's permits are moved
    // into its own spawned task and held there for the task's full lifetime
    // (RAII) — so capacity is restored automatically the instant a turn's
    // task completes. There is no longer a manual "clear the flag" step to
    // order against the busy check; the old ordering invariant is replaced
    // by a structural one (permits can only ever release via task-drop).
    // Confirm the old singular flag is fully gone.
    assert!(
        !ws.contains("in_flight_turn.is_some()"),
        "WR-02 evolved (Phase 39.1 Plan 02): ws.rs must NOT reference the old singular \
         `in_flight_turn.is_some()` busy flag — it was replaced by the concurrency semaphore"
    );
    assert!(
        !ws.contains("in_flight_turn = None;"),
        "WR-02 evolved (Phase 39.1 Plan 02): ws.rs must NOT manually clear a singular \
         `in_flight_turn` flag — permit release is now RAII-driven, not manual"
    );

    // New busy-gate: non-slash messages are queued instead of rejected once
    // the semaphore has no capacity.
    assert!(
        ws.contains(
            "app_state.concurrency.try_acquire().is_none() && !message.starts_with('/')"
        ),
        "ws_chat must gate new turns on the concurrency semaphore (Phase 39.1 replacement \
         for the Phase 36.1 `in_flight_turn.is_some()` busy-gate)"
    );

    // New "opportunistic clear" equivalent: every turn-spawning call site
    // (primary message turn, STT-transcript turn, queue-drain turn) moves
    // its permits into the spawned task with `let _per = ...; let _global =
    // ...;` so they can only be released when that task finishes — the
    // structural replacement for the old manual clear-before-check.
    let raii_permit_sites = ws.matches("let _per = ").count();
    assert!(
        raii_permit_sites >= 3,
        "WR-02 replacement: every turn-spawning call site (primary, STT, queue-drain) must \
         hold its semaphore permits for the spawned task's lifetime via RAII, not a manually \
         cleared flag; found {raii_permit_sites} site(s), expected >= 3"
    );

    // The in_flight_turns HashMap bookkeeping (TurnEnded emission + queue
    // drain trigger) still opportunistically removes a turn's entry once its
    // JoinHandle reports finished — this is the direct descendant of the
    // original "opportunistically clears finished turn" behavior, now scoped
    // to telemetry/bookkeeping rather than concurrency gating.
    let finished_pos = ws
        .find("turn.handle.is_finished()")
        .expect("ws_chat must check turn.handle.is_finished() to opportunistically clear finished turn entries from in_flight_turns (bookkeeping descendant of WR-02)");
    let remove_pos = ws
        .find("in_flight_turns.remove(&done_turn_id)")
        .expect("ws_chat must remove the finished turn's entry from in_flight_turns once its handle reports finished");
    assert!(
        finished_pos < remove_pos,
        "the is_finished() check ({finished_pos}) must precede removing the entry ({remove_pos})"
    );
}

#[test]
fn session_select_switches_id_and_clears_transcript() {
    // Retargeted from legacy warp_hermes.rs on_tab_click to the active HermesApp
    // Sessions + Chat screens. Selecting a session row switches session_id and
    // routes to the Chat screen (sessions.rs); the Chat screen clears the prior
    // transcript + streaming indicator on every session_id change (chat.rs).
    // The legacy mid-stream guard (`scanner_active()` + early `return;`) was
    // intentionally dropped — the wheel-driven shell allows switching during an
    // in-flight turn (the orphaned turn is simply discarded).
    let sessions = read("src/components/hermes_app/screens/sessions.rs");
    assert!(
        sessions.contains("let on_select = move |sid: String|")
            && sessions.contains("session_id.set(sid)")
            && sessions.contains("active_screen.set(crate::state::Screen::Chat)"),
        "row select must set session_id and route to the Chat screen (D-09)"
    );

    let chat = read("src/components/hermes_app/screens/chat.rs");
    assert!(
        chat.contains("bubbles.write().clear()"),
        "Chat screen must clear the transcript on session_id change (D-08: no stale history)"
    );
    assert!(
        chat.contains("streaming_id.set(None)"),
        "Chat screen must cancel the stale streaming indicator on session switch (D-01)"
    );
}

#[test]
fn tab_new_calls_create_session_and_appends_tab() {
    let ui = read("src/components/warp_hermes.rs");
    assert!(
        ui.contains("let on_tab_new = move |_: ()|"),
        "WarpHermes must define on_tab_new closure (D-09)"
    );
    assert!(
        ui.contains("create_session().await"),
        "on_tab_new must call the create_session server function (D-03)"
    );
    assert!(
        ui.contains("\"New Session\".to_string()"),
        "new tab must use \"New Session\" placeholder label (D-04)"
    );
    assert!(
        ui.contains("tabs.write().push"),
        "on_tab_new must push the new Tab onto the tabs signal (D-03)"
    );
}

#[test]
fn session_delete_button_uses_stop_propagation() {
    // Retargeted from the legacy shell_legacy/title_bar.rs tab-close button to the
    // active HermesApp Sessions screen. The wheel-driven shell replaced the tab
    // strip + TitleBar with a sessions list; the per-row delete button must still
    // call evt.stop_propagation() so the click does not bubble to the row-select
    // handler. The legacy TitleBar prop contract (on_tab_click/on_tab_close/
    // on_tab_new EventHandlers) and the streaming `disabled: bool` +
    // `pointer-events: none; opacity: 0.5` gate were intentionally dropped in
    // hermes_app (the UI stays interactive during streaming), so those assertions
    // are not carried over.
    let sessions = read("src/components/hermes_app/screens/sessions.rs");
    assert!(
        sessions.contains("evt.stop_propagation()"),
        "session delete button must call evt.stop_propagation() so the click does not bubble to row select"
    );
    assert!(
        sessions.contains("on_delete.call("),
        "delete button must invoke the on_delete handler with the session id"
    );
}
