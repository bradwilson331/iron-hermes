//! Server-side modules for the Dioxus UI backend.
//!
//! `api` is compiled on BOTH client and server — the `#[get]`/`#[post]` macros
//! generate HTTP-call stubs on the client and API endpoints on the server.
//! `protocol` types (ChatRequest, ChatStreamEvent) live in `src/protocol.rs` and
//! are compiled unconditionally on both targets.
//! `ws` and `state` (pure server-side logic) stay behind `#[cfg(feature = "server")]`.

pub mod api;
#[cfg(feature = "server")]
pub mod logging;
/// Test-only support shared across this crate's server test modules.
///
/// `IRONHERMES_HOME` is process-global, but the test runner is parallel and
/// scoped per test fn — a module-local mutex (the pre-46.9 convention) only
/// serializes tests *within* one module, so `state::plan_46_7_04_tests` and
/// `chat_attachments_api::chat_attachment_upload_tests` raced each other and
/// failed intermittently. Every test that mutates `IRONHERMES_HOME` (or other
/// process-global env) must hold THIS lock instead of a module-local one.
#[cfg(all(test, feature = "server"))]
pub(crate) mod test_support {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Poison is swallowed deliberately: a panicking test must not cascade
    /// `PoisonError`s into unrelated modules' tests.
    pub(crate) fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
#[cfg(feature = "server")]
pub mod state;
// Phase 36.3.12 D-08 (GAP 2): fail-closed WebApprovalGate for run_web_turn's
// terminal/execute_code intercepts. Server-only like `state`, its sole caller.
#[cfg(feature = "server")]
pub mod web_approval_gate;
pub mod ws;
// Phase 36.3.7.11 Plan 01 — kanban dashboard read surface + WS.
// `kanban_api` is unconditional (the `#[server]` macros handle the
// client/server split). `kanban_ws` is unconditional too: like `ws`, the
// route fn returns a server-side handler, but the WASM build still needs
// the route stub for client-side URL generation; `#[cfg(feature = "server")]`
// gates the internals.
pub mod kanban_api;
pub mod kanban_ws;
// Phase 46.9 Plan 01 (D-01/D-02/D-10): Providers config read/write server
// fns. Unconditional like `kanban_api` — the `#[server]` macro handles the
// client/server split, and the internal helper fns are gated with
// `#[cfg(not(target_arch = "wasm32"))]`.
pub mod provider_config_api;
// Phase 46.9 Plan 06 (D-03/D-04): provider secret set/rotate/clear over
// ironhermes-vault::SecretStore — the phase's isolated security-sensitive
// surface. Unconditional like `provider_config_api`; internal helper fns
// are gated with `#[cfg(not(target_arch = "wasm32"))]`.
pub mod provider_secrets_api;
// Phase 46.9 Plan 04 (D-06): Schedules CRUD server fns over ironhermes-cron's
// JobStore. Unconditional like `provider_config_api` — the `#[server]`
// macro handles the client/server split, internal helper fns are gated with
// `#[cfg(not(target_arch = "wasm32"))]`.
pub mod schedules_api;
// Phase 46.7 Plan 04 (D-09): web-chat attachment upload transport. Like
// `kanban_api`, unconditional — the `#[server]` macro handles the
// client/server split, and the impl fns/tests are internally gated with
// `#[cfg(feature = "server")]`.
pub mod chat_attachments_api;
// Phase 36.17.7 D-02-a: web-runtime AudioDispatcher impl.
// Like `ws` and `kanban_ws`, the module is gated internally with
// `#![cfg(feature = "server")]` so the WASM client never sees it.
pub mod web_audio_dispatcher;
// Phase 01-04 (DLV-03 web): web-runtime image dispatcher (mirrors
// web_audio_dispatcher). Gated internally with `#![cfg(feature = "server")]`
// so the WASM client never sees it.
pub mod web_image_dispatcher;
// Phase 36.3.3 (D-08 web): web-runtime video dispatcher (mirrors
// web_image_dispatcher). Gated internally with `#![cfg(feature = "server")]`
// so the WASM client never sees it. Cap: 50MB (D-07), not 20MB.
pub mod web_video_dispatcher;
// Phase 36.17.7 D-02-c: GET /audio/:uuid replay route.
// Like `ws` and `kanban_ws`, exposes a `#[get]` server function;
// the route body is gated `#[cfg(feature = "server")]`.
pub mod audio_route;
// Phase 46.6 (D-02): GET /artifacts/{id} sandboxed artifact serving route.
// A raw axum handler mounted explicitly in main.rs (`.route(...)`), NOT a
// `#[get]` server fn — the server-fn codec serialized the Vec<u8> return as a
// JSON number array, breaking the iframe viewer. Body is gated
// `#[cfg(feature = "server")]`; the module is otherwise empty on wasm.
pub mod artifact_route;
// Phase 46.7 Plan 05 (D-06): GET /chat-attachments/{session_id}/{id} raw
// session-attachment serving route. Same raw-axum-handler pattern as
// artifact_route above (Vec<u8>-via-server-fn-codec would break both the
// sent-bubble <img src> thumbnail and the download link). Body is gated
// `#[cfg(feature = "server")]`; the module is otherwise empty on wasm.
pub mod chat_attachment_route;
// Phase 36.17.7 D-02-d: audio cache GC (startup sweep + periodic loop).
// Gated internally with `#![cfg(feature = "server")]`.
#[cfg(feature = "server")]
pub mod audio_cache;
// Phase 46.9 Plan 11 (GAP-4/GAP-5): read-only footer display/timezone
// resolution (config.display.timezone/extra_timezones -> primary + capped
// extras). Unconditional like `provider_config_api` — the `#[server]` macro
// handles the client/server split, and the internal helper fn is gated with
// `#[cfg(not(target_arch = "wasm32"))]`.
pub mod display_tz_api;
// Phase 47.3 Plan 01 (D-06/D-07): operator authentication — AuthState,
// SessionStore, rate limiter, require_auth deny-by-default middleware, and
// the /auth/* raw axum routes. Body gated `#[cfg(feature = "server")]`
// internally (native-only crypto + session state), same convention as
// `artifact_route` above.
#[cfg(feature = "server")]
pub mod auth;
// Phase 47.3 Plan 01 (D-07): server-rendered login page — root_handler
// (the GET / auth split), render_login_page, and the asset!()-derived
// pre-auth allowlist (public_asset_paths). Body gated
// `#[cfg(feature = "server")]` internally, same convention as `auth` above.
#[cfg(feature = "server")]
pub mod login_page;
// Phase 47.4 Plan 01 (D-08): kanban profile enumeration — `list_profiles`
// scans `$IRONHERMES_HOME/profiles/*` and classifies each per the D-11
// health rule. Unconditional like `provider_config_api` — the `#[server]`
// macro handles the client/server split, and internal helper fns are gated
// with `#[cfg(feature = "server")]`.
pub mod profile_api;
// Phase 47.4 Plan 06 (D-09/D-11/D-14): the real, profile-scoped judge probe
// — `verify_profile`. Unconditional like `profile_api` — the `#[server]`
// macro handles the client/server split, and internal helper fns are gated
// with `#[cfg(feature = "server")]`. Registered through
// `register_server_functions()` (main.rs), so `require_auth` wraps it same
// as every other `#[server]` fn.
pub mod profile_verify_api;
