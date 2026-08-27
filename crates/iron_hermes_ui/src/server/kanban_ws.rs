//! Phase 36.3.7.11 Plan 01 (D-08 / D-15) — kanban dashboard WebSocket handler
//! and dashboard-side tail consumer.
//!
//! Two surface elements live here:
//!
//! 1. **`ws_kanban` route at `/api/ws/kanban`** — accepts a WS upgrade,
//!    subscribes to `AppState::kanban_tail_broadcast`, and forwards every
//!    new `KanbanWsEvent::TaskEventBatch` to the client. Lifecycle mirrors
//!    `server/ws.rs` byte-for-byte (5s keepalive Ping + close-frame on
//!    teardown + RAII receiver drop on exit).
//!
//! 2. **`run_kanban_tail_loop`** — polls `KanbanStore::list_all_events_after`
//!    at `tail_interval_ms` cadence (D-17 default 250 ms), advances a
//!    watermark, serializes a `KanbanWsEvent::TaskEventBatch`, and broadcasts
//!    the JSON string. Independent of the gateway notifier (D-15 — no
//!    `use ironhermes_kanban::notifier` import, no shared primitive).

use dioxus::prelude::*;
#[cfg(feature = "server")]
use dioxus_fullstack::{body::Bytes, CloseCode, Message, TypedWebsocket};
use dioxus_fullstack::{WebSocketOptions, Websocket};
#[cfg(feature = "server")]
use std::time::Duration;
#[cfg(feature = "server")]
use tracing::{info, warn};

#[cfg(feature = "server")]
use ironhermes_kanban::store::KanbanStore;

// ---------------------------------------------------------------------------
// D-14 websocket-security (T-49.1-08-05): CSWSH defense-in-depth
// ---------------------------------------------------------------------------
//
// `WebSocketOptions::from_request` (dioxus-fullstack 0.7.7,
// payloads/websocket.rs) extracts `axum::extract::ws::WebSocketUpgrade` and
// discards the original request's headers — the `ws: WebSocketOptions`
// parameter this module's `ws_kanban` receives has no accessor for the
// `Origin` header the upgrade request carried. Per this plan's own
// documented contingency, the Origin check therefore lives as router-wide
// middleware "layered over the WS route path" rather than inside the
// handler body — and since `auth::require_auth` (auth.rs) ALREADY wraps
// every route in the router, including both WS upgrade paths (main.rs's own
// comment: "wraps server fns, the WS upgrade, artifacts, attachments"), the
// check is added there. This module owns the pure, dependency-free
// predicate + path list (no axum/dioxus types below — safe to leave
// ungated so it also compiles on the wasm client target); `require_auth`
// calls into it after the session is already proven valid.
//
// Threat register T-49.1-08-05 (medium, mitigate): `ws_kanban` and
// `ws_chat` (server/ws.rs — the other binary-frame dispatcher D-14 names)
// are already session-gated, and the session cookie is `HttpOnly;
// SameSite=Strict` (auth.rs `session_cookie`, :388) — SameSite=Strict
// already prevents the cookie from riding along on a cross-site top-level
// navigation to an attacker page that then tries to open a cross-origin WS
// connection back here, since the cookie simply won't be attached to that
// request. This Origin check is defense-in-depth on top of that mitigation,
// closing the gap for any browser/cookie-attribute edge case where
// SameSite handling regresses or is bypassed — not "closing an open hole".

/// D-14 (T-49.1-08-05): every WebSocket upgrade endpoint the Origin check
/// below applies to. `ws_kanban` (this module) and `ws_chat`
/// (`server/ws.rs`, `#[get("/api/ws/chat")]`) are the only two — confirmed
/// by enumerating every `WebSocketOptions`/`Websocket<` usage under
/// `src/server/`; `audio_route.rs`'s `serve_audio`/`serve_audio_axum` are
/// plain HTTP byte-serving routes, not WebSocket upgrades, despite the
/// "binary-frame dispatcher" framing in the phase context doc.
pub(crate) const WS_ORIGIN_CHECKED_PATHS: &[&str] = &["/api/ws/kanban", "/api/ws/chat"];

/// Derive this deployment's expected `Origin` from the incoming request's
/// own `Host` header — never a hardcoded scheme+host (a request-derived
/// value works identically on 127.0.0.1 local probing, a LAN bind, and the
/// production `iron-hermes.sliplane.app` deployment behind Caddy, with zero
/// per-environment configuration). `secure` selects the scheme; callers
/// pass `AuthState`'s own `cookie_secure` flag, matching the same signal
/// that already decides whether the session cookie itself carries `Secure`.
pub(crate) fn expected_ws_origin(host: &str, secure: bool) -> String {
    let scheme = if secure { "https" } else { "http" };
    format!("{scheme}://{host}")
}

/// The no-`Origin` rule (documented here, pinned by Test 3 in
/// `origin_check_tests` below): a MISSING `Origin` header is ACCEPTED, not
/// rejected. Browsers always send `Origin` on a WebSocket upgrade — same-
/// origin or cross-origin — so only non-browser clients (CLI tools, curl,
/// another backend service) legitimately omit it; rejecting a missing
/// header would break those without closing any browser-driven CSWSH
/// vector (which always presents an `Origin`).
///
/// Returns `true` (reject) only when `path` is one of
/// [`WS_ORIGIN_CHECKED_PATHS`] AND a present `origin` does not equal
/// `expected`.
pub(crate) fn is_cross_origin_ws_upgrade(path: &str, origin: Option<&str>, expected: &str) -> bool {
    if !WS_ORIGIN_CHECKED_PATHS.contains(&path) {
        return false;
    }
    match origin {
        None => false,
        Some(o) => o != expected,
    }
}

/// Phase 36.3.7.11 (D-08): kanban WebSocket application-level keepalive
/// interval. Identical to `server::ws::WS_KEEPALIVE_INTERVAL` — 5 s — well
/// under the ~9 s `dx serve` proxy idle-close threshold.
#[cfg(feature = "server")]
const WS_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Phase 36.3.7.11 (D-08): best-effort close-frame emit before dropping the
/// socket. Mirrors `server::ws::send_close_frame` so every teardown branch
/// completes the WebSocket close handshake (no raw transport resets visible
/// to upstream proxies).
#[cfg(feature = "server")]
async fn send_close_frame(
    socket: &mut TypedWebsocket<String, String>,
    code: CloseCode,
    reason: &str,
) {
    let _ = socket
        .send_raw(Message::Close {
            code,
            reason: reason.to_string(),
        })
        .await;
}

/// Phase 36.3.7.11 (D-08 / D-22): kanban dashboard WebSocket endpoint.
///
/// Auth inherited from the same session model that protects `/api/ws/chat`
/// (D-22 — no kanban-specific token). Subscribes to the dashboard tail
/// broadcaster and forwards JSON-serialized `KanbanWsEvent` frames to the
/// client. RAII `Receiver` drop on exit removes the client from the
/// broadcast fan-out (no explicit unsubscribe needed).
#[get("/api/ws/kanban")]
pub async fn ws_kanban(ws: WebSocketOptions) -> Result<Websocket<String, String>> {
    #[cfg(feature = "server")]
    let app_state = crate::server::state::global_app_state().clone();

    Ok(ws.on_upgrade(
        move |mut socket: dioxus_fullstack::TypedWebsocket<String, String>| {
            #[cfg(feature = "server")]
            let app_state = app_state.clone();
            async move {
                #[cfg(feature = "server")]
                {
                    info!("websocket kanban connection established");
                    let mut broadcast_rx = app_state.kanban_tail_broadcast.subscribe();

                    let mut keepalive = tokio::time::interval(WS_KEEPALIVE_INTERVAL);
                    keepalive.set_missed_tick_behavior(
                        tokio::time::MissedTickBehavior::Skip,
                    );
                    keepalive.tick().await;

                    loop {
                        tokio::select! {
                            // ── Incoming frames from the client ──────────
                            raw = socket.recv_raw() => {
                                match raw {
                                    Ok(Message::Text(_)) => {
                                        // Dashboard WS is server→client only; ignore
                                        // any text the client sends.
                                        continue;
                                    }
                                    Ok(Message::Close { code, reason }) => {
                                        warn!(
                                            code = ?code,
                                            reason = %reason,
                                            "websocket kanban close frame received; exiting connection"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Normal,
                                            "recv closed cleanly",
                                        )
                                        .await;
                                        break;
                                    }
                                    Ok(_) => continue,
                                    Err(err) => {
                                        warn!(
                                            reason = %err,
                                            "websocket kanban recv failed; closing connection"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "recv failed",
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                            // ── Tail broadcast → client ──────────────────
                            maybe_event = broadcast_rx.recv() => {
                                match maybe_event {
                                    Ok(json) => {
                                        if let Err(err) = socket
                                            .send_raw(Message::Text(json))
                                            .await
                                        {
                                            warn!(
                                                reason = %err,
                                                "websocket kanban send failed; closing connection"
                                            );
                                            send_close_frame(
                                                &mut socket,
                                                CloseCode::Away,
                                                "send failed",
                                            )
                                            .await;
                                            break;
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        // Client missed N events — log and continue.
                                        // Plan 02 reconnect logic will replay from
                                        // last_event_id cursor (D-08).
                                        warn!(
                                            lagged = n,
                                            "websocket kanban receiver lagged; events skipped"
                                        );
                                        continue;
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                        // Tail loop dropped the sender — exit.
                                        warn!(
                                            "websocket kanban broadcast channel closed; exiting"
                                        );
                                        send_close_frame(
                                            &mut socket,
                                            CloseCode::Away,
                                            "broadcast closed",
                                        )
                                        .await;
                                        break;
                                    }
                                }
                            }
                            // ── Keepalive Ping ──────────────────────────
                            _ = keepalive.tick() => {
                                if let Err(err) = socket
                                    .send_raw(Message::Ping(Bytes::new()))
                                    .await
                                {
                                    warn!(
                                        reason = %err,
                                        "websocket kanban keepalive ping failed; closing connection"
                                    );
                                    send_close_frame(
                                        &mut socket,
                                        CloseCode::Away,
                                        "keepalive failed",
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                    }
                }

                #[cfg(not(feature = "server"))]
                {
                    let unavailable = crate::protocol::KanbanWsEvent::Error {
                        message: "Kanban websocket unavailable without `server` feature"
                            .to_string(),
                    };
                    let _ = socket
                        .send_raw(Message::Text(
                            serde_json::to_string(&unavailable).unwrap_or_default(),
                        ))
                        .await;
                }
            }
        },
    ))
}

// ---------------------------------------------------------------------------
// run_kanban_tail_loop — D-15 dashboard tail consumer
// ---------------------------------------------------------------------------

/// Phase 36.3.7.11 (D-15 / D-17): dashboard tail consumer loop.
///
/// Polls `KanbanStore::list_all_events_after(watermark)` every
/// `interval_ms` milliseconds. On new events:
/// 1. Advance the watermark to `max(id)`.
/// 2. Build a `KanbanWsEvent::TaskEventBatch { events, last_event_id }`.
/// 3. Serialize to JSON and broadcast via `tx.send(json)`.
///
/// **Independent of the gateway notifier (D-15):** no
/// `use ironhermes_kanban::notifier` import; this loop only depends on
/// `KanbanStore::list_all_events_after` (Wave 0 helper).
///
/// Cancel-safety: every `tokio::select!` arm releases the `MutexGuard`
/// before any `.await` boundary that could be cancelled — preserves the
/// rusqlite `!Send` discipline (Pattern G in PATTERNS.md).
#[cfg(feature = "server")]
pub async fn run_kanban_tail_loop(
    tx: tokio::sync::broadcast::Sender<String>,
    cancel: tokio_util::sync::CancellationToken,
    interval_ms: u64,
) {
    use std::sync::Arc;
    use tokio::sync::Mutex as TokioMutex;

    // Open the dashboard's own KanbanStore connection (D-16: cross-process
    // WAL boundary — gateway notifier holds its own connection to the same
    // file). Q8: persistent per-loop connection avoids per-tick open cost.
    // Phase 36.3.7.13 D-A1: env-bridged open so the dashboard WS tailer
    // honors IRONHERMES_KANBAN_DB when run under a non-default profile
    // (legacy HERMES_KANBAN_DB also accepted during deprecation window).
    let store = match KanbanStore::open_from_env() {
        Ok(s) => Arc::new(TokioMutex::new(s)),
        Err(e) => {
            warn!(error = %e, "kanban tail: failed to open default board; tail disabled");
            return;
        }
    };

    // Initialize watermark via the lock-scope-before-anything-else pattern
    // (notifier.rs lines 426-442).
    let mut watermark: i64 = {
        let s = store.lock().await;
        match s.max_event_id() {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "kanban tail: max_event_id failed; tail disabled");
                return;
            }
        }
    };

    info!(interval_ms, watermark, "kanban tail consumer started");

    let poll_interval = Duration::from_millis(interval_ms);

    loop {
        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {
                // Read events under lock; release lock BEFORE broadcast send
                // (Pattern G — rusqlite !Send discipline).
                let read_result = {
                    let s = store.lock().await;
                    s.list_all_events_after(watermark)
                };
                let events = match read_result {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "kanban tail: list_all_events_after failed; continuing");
                        continue;
                    }
                };
                if events.is_empty() {
                    continue;
                }
                // Advance watermark BEFORE building the payload (events
                // already sorted id ASC by SQL).
                if let Some(last) = events.last() {
                    watermark = last.id;
                }
                let rows: Vec<crate::protocol::KanbanEventRow> = events
                    .into_iter()
                    .map(|e| crate::protocol::KanbanEventRow {
                        id: e.id,
                        task_id: e.task_id,
                        kind: e.kind,
                        payload: e.payload,
                        created_at: e.created_at,
                    })
                    .collect();
                let batch = crate::protocol::KanbanWsEvent::TaskEventBatch {
                    events: rows,
                    last_event_id: watermark,
                };
                let json = match serde_json::to_string(&batch) {
                    Ok(j) => j,
                    Err(e) => {
                        warn!(error = %e, "kanban tail: serialize TaskEventBatch failed; skipping");
                        continue;
                    }
                };
                // Silently discard SendError when no receivers (Q1).
                let _ = tx.send(json);
            }
            _ = cancel.cancelled() => {
                info!("kanban tail consumer cancelled; exiting");
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// D-14 websocket-security (T-49.1-08-05) unit tests — pure predicate table
// ---------------------------------------------------------------------------
#[cfg(test)]
mod origin_check_unit_tests {
    use super::*;

    #[test]
    fn expected_origin_derives_scheme_from_secure_flag() {
        assert_eq!(expected_ws_origin("127.0.0.1:8109", false), "http://127.0.0.1:8109");
        assert_eq!(
            expected_ws_origin("iron-hermes.sliplane.app", true),
            "https://iron-hermes.sliplane.app"
        );
    }

    #[test]
    fn checked_paths_are_exactly_the_two_ws_endpoints() {
        assert_eq!(WS_ORIGIN_CHECKED_PATHS, ["/api/ws/kanban", "/api/ws/chat"]);
    }

    #[test]
    fn unrelated_path_is_never_flagged_regardless_of_origin() {
        assert!(!is_cross_origin_ws_upgrade(
            "/api/some/other/route",
            Some("https://evil.example"),
            "http://127.0.0.1:8109"
        ));
    }

    #[test]
    fn same_origin_is_not_flagged() {
        for path in WS_ORIGIN_CHECKED_PATHS {
            assert!(!is_cross_origin_ws_upgrade(path, Some("http://127.0.0.1:8109"), "http://127.0.0.1:8109"));
        }
    }

    #[test]
    fn foreign_origin_is_flagged() {
        for path in WS_ORIGIN_CHECKED_PATHS {
            assert!(is_cross_origin_ws_upgrade(path, Some("https://evil.example"), "http://127.0.0.1:8109"));
        }
    }

    /// The pinned no-Origin rule: a MISSING Origin is never flagged, on
    /// either checked path.
    #[test]
    fn missing_origin_is_not_flagged() {
        for path in WS_ORIGIN_CHECKED_PATHS {
            assert!(!is_cross_origin_ws_upgrade(path, None, "http://127.0.0.1:8109"));
        }
    }
}

// ---------------------------------------------------------------------------
// D-14 websocket-security (T-49.1-08-05) live-wiring tests — the four
// `<behavior>` tests, driven through the REAL `auth::require_auth`
// middleware over a small router (same style as auth.rs's own
// `sec_fetch_dest_rejects_iframe_originated_api_request`), proving the
// Origin check actually rejects/accepts at the middleware boundary rather
// than only in the pure predicate above (project precedent: tests must not
// verify only their own assumptions).
// ---------------------------------------------------------------------------
#[cfg(all(test, feature = "server"))]
mod origin_check_tests {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{header, Request, StatusCode};
    use std::net::SocketAddr;
    use std::sync::{Arc, OnceLock};
    use tower::ServiceExt as _;

    use crate::server::auth::{AuthConfig, AuthState};

    /// Isolated `IRONHERMES_HOME` for every `AuthState` this test module
    /// constructs (D-06 — never touch the operator's real home; mirrors
    /// `auth.rs`'s and `login_page.rs`'s own `OnceLock<TempDir>` pattern).
    /// nextest runs each test function in its own process, so a single
    /// process-wide `OnceLock` set once here is sufficient — no restore
    /// needed.
    fn ensure_home_env() {
        static HOME: OnceLock<tempfile::TempDir> = OnceLock::new();
        let dir = HOME.get_or_init(|| tempfile::tempdir().expect("tempdir for IRONHERMES_HOME"));
        // SAFETY: test-only; set exactly once (OnceLock) before any test
        // constructs an AuthState, never mutated again.
        unsafe {
            std::env::set_var("IRONHERMES_HOME", dir.path());
        }
    }

    fn hash_password(pw: &str) -> String {
        use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
        use argon2::Argon2;
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default().hash_password(pw.as_bytes(), &salt).unwrap().to_string()
    }

    async fn dummy_ws_handler() -> StatusCode {
        StatusCode::OK
    }

    /// A minimal router carrying just the WS route path plus the raw
    /// `/auth/login` route (needed to mint a real session via the actual
    /// `login()` handler, not a private `mint_session()` call this module
    /// has no access to), wrapped by the real `require_auth` middleware —
    /// the exact wrapping order main.rs uses.
    fn test_router(auth_state: Arc<AuthState>) -> axum::Router {
        let auth_routes = axum::Router::new()
            .route("/auth/login", axum::routing::post(crate::server::auth::login))
            .with_state(auth_state.clone());

        axum::Router::new()
            .route("/api/ws/kanban", axum::routing::get(dummy_ws_handler))
            .merge(auth_routes)
            .layer(axum::middleware::from_fn_with_state(
                auth_state,
                crate::server::auth::require_auth,
            ))
    }

    /// Drives the real `login()` handler (raw JSON body — no struct
    /// literal, since `LoginRequest`'s field is private outside auth.rs)
    /// and returns the `name=value` cookie pair from `Set-Cookie`.
    async fn login_and_get_cookie(router: &axum::Router, password: &str) -> String {
        let mut req = Request::builder()
            .method("POST")
            .uri("/auth/login")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!("{{\"password\":\"{password}\"}}")))
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 47100))));
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT, "login must succeed with the correct password");
        let set_cookie = resp
            .headers()
            .get(header::SET_COOKIE)
            .expect("login response must carry Set-Cookie")
            .to_str()
            .unwrap()
            .to_string();
        set_cookie.split(';').next().unwrap().to_string()
    }

    fn ws_upgrade_request(cookie: Option<&str>, origin: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder()
            .method("GET")
            .uri("/api/ws/kanban")
            .header(header::HOST, "127.0.0.1:8109")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header("sec-websocket-version", "13")
            .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==");
        if let Some(c) = cookie {
            builder = builder.header(header::COOKIE, c);
        }
        if let Some(o) = origin {
            builder = builder.header(header::ORIGIN, o);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn fresh_auth() -> Arc<AuthState> {
        ensure_home_env();
        AuthState::new(AuthConfig {
            password_hash: Some(hash_password("hunter2")),
            ..Default::default()
        })
        .unwrap()
    }

    /// Test 1: a WebSocket upgrade carrying a valid session cookie and a
    /// same-origin `Origin` header succeeds (reaches the handler).
    #[tokio::test]
    async fn test1_same_origin_with_valid_session_succeeds() {
        let auth = fresh_auth();
        let router = test_router(auth.clone());
        let cookie = login_and_get_cookie(&router, "hunter2").await;

        let req = ws_upgrade_request(Some(&cookie), Some("http://127.0.0.1:8109"));
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "same-origin upgrade with a valid session must reach the handler"
        );
    }

    /// Test 2: a WebSocket upgrade carrying a valid session cookie and a
    /// foreign `Origin` header is REJECTED — a status that does not
    /// complete the upgrade.
    #[tokio::test]
    async fn test2_foreign_origin_with_valid_session_rejected() {
        let auth = fresh_auth();
        let router = test_router(auth.clone());
        let cookie = login_and_get_cookie(&router, "hunter2").await;

        let req = ws_upgrade_request(Some(&cookie), Some("https://evil.example"));
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::FORBIDDEN,
            "cross-origin upgrade with a valid session must be rejected before completing"
        );
    }

    /// Test 3: a WebSocket upgrade carrying a valid session cookie and NO
    /// `Origin` header is handled by the pinned rule — accepted (non-
    /// browser clients).
    #[tokio::test]
    async fn test3_no_origin_with_valid_session_pinned_rule_accepts() {
        let auth = fresh_auth();
        let router = test_router(auth.clone());
        let cookie = login_and_get_cookie(&router, "hunter2").await;

        let req = ws_upgrade_request(Some(&cookie), None);
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "a missing Origin must be accepted per the pinned no-Origin rule"
        );
    }

    /// Test 4: a WebSocket upgrade with NO session cookie is rejected,
    /// regardless of `Origin` — the pre-existing session gate, re-asserted
    /// so a later refactor cannot drop it silently.
    #[tokio::test]
    async fn test4_no_session_rejected_regardless_of_origin() {
        let auth = fresh_auth();
        let router = test_router(auth);

        let req = ws_upgrade_request(None, Some("http://127.0.0.1:8109"));
        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "no session cookie must be rejected regardless of Origin"
        );
    }
}
