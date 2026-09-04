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
///
/// Gated on `not(target_arch = "wasm32")` rather than `feature = "server"`
/// (49.4-02 fix, folded todo 2026-08-28): the body is pure `std::sync`, with
/// no dependency on the `server` feature's `tokio/full`/`dep:axum`, and a
/// dozen `server/*.rs` test modules across the crate call
/// `crate::server::test_support::env_lock()` from a
/// `#[cfg(all(test, not(target_arch = "wasm32")))]` `mod tests` that never
/// required the `server` feature either — so gating this module on
/// `feature = "server"` broke `cargo test --workspace` (no consumer in that
/// invocation requests the `server` feature for this crate, which is
/// excluded from workspace `default-members`). `state` (below) genuinely
/// needs `tokio::sync::RwLock`, gated behind `tokio/full` (only enabled by
/// `feature = "server"`) — this module has no such requirement.
#[cfg(all(test, not(target_arch = "wasm32")))]
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
// Phase 49.5 Plan 01 (D-02/D-04/D-05): Blueprints tab server surface —
// `list_blueprints`/`create_schedule_from_blueprint` over the compiled-in
// `ironhermes_cron::blueprint` catalog, writing through the SAME `JobStore`
// path `schedules_api::create_schedule` uses. Unconditional like
// `schedules_api` — the `#[server]` macro handles the client/server split,
// internal helper fns are gated with `#[cfg(not(target_arch = "wasm32"))]`.
pub mod blueprints_api;
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
// Phase 49.4 Plan 08 (D-14): persisted profile activation-with-scope —
// `activate_profile`/`get_active_profile`/`resolve_active_profile_for`.
// Unconditional like `profile_api` — the `#[server]` macro handles the
// client/server split, and internal helper fns are gated with
// `#[cfg(feature = "server")]` (same convention as `profile_api`'s own
// `create_profile_impl`/`check_profile_write_gate` — real `Config`
// read/write logic, not the narrower widened-pure-fn class like
// `profile_dir_for`).
pub mod profile_activation_api;
// Phase 47.4 Plan 06 (D-09/D-11/D-14): the real, profile-scoped judge probe
// — `verify_profile`. Unconditional like `profile_api` — the `#[server]`
// macro handles the client/server split, and internal helper fns are gated
// with `#[cfg(feature = "server")]`. Registered through
// `register_server_functions()` (main.rs), so `require_auth` wraps it same
// as every other `#[server]` fn.
pub mod profile_verify_api;
// Phase 50.1 Plan 01 (D-08/D-11/D-13/D-23): UI-owned bot display-metadata
// store — `list_bot_meta`/`save_bot_meta`/`delete_bot_meta`/
// `live_profile_name`. Unconditional like `profile_api` — the `#[server]`
// macro handles the client/server split, and internal helper fns are gated
// with `#[cfg(feature = "server")]`. Its own sibling store/mutex, isolated
// from `state_store` (module doc has the full persistence-shape record).
pub mod bot_meta_api;
// Phase 49.4 Plan 12 (D-16): the profile-to-bot binding store —
// `list_bot_bindings`/`set_bot_binding`/`clear_bot_binding` plus the
// internal `resolve_profile_for_bot`/`revert_bindings_for_archived_profile`
// primitives. Unconditional like `bot_meta_api` — the `#[server]` macro
// handles the client/server split, and internal helper fns are gated with
// `#[cfg(feature = "server")]`. Its own sibling store/mutex
// (`bot-bindings.json` at the home root), isolated from `state_store`,
// `BOT_META_LOCK`, and every other module's own lock (module doc has the
// full persistence-shape record and the honest "known gap" section on
// production gateway-spawn wiring).
pub mod bot_binding_api;
// Phase 50.1 Plan 03 (D-04/D-05/D-06/D-20): process-isolated bot message
// dispatch — `dispatch_bot_message` spawns an `ironhermes` subprocess for a
// non-live bot, reusing the kanban worker path's `.env_clear()` + allowlist
// isolation model. Unconditional like `profile_api` — the `#[server]` macro
// handles the client/server split, and internal helper fns are gated with
// `#[cfg(feature = "server")]`.
pub mod cli_handoff;
// Phase 50.2 Plan 16 (G-50.2-2c): the subprocess transport's own next-turn
// steering primitive — the accept-and-queue analogue of the LIVE profile's
// embedded-runtime mid-turn queue (`ws.rs:931-957`, R39.1-06). Server-only
// like `state` — its only caller is `cli_handoff` (and, transitively,
// `group_chat_api`'s round driver), never reached from wasm.
#[cfg(feature = "server")]
pub mod handoff_steering;
// Phase 50.1 Plan 04 (D-11/D-12): avatar upload and AI-portrait generation
// server paths — `upload_bot_avatar`/`generate_bot_avatar`. Unconditional
// like `profile_api` — the `#[server]` macro handles the client/server
// split, and internal helper fns are gated with `#[cfg(feature = "server")]`.
pub mod bot_avatar_api;
// Phase 50.1 Plan 04 (D-06 analog): GET /bot-avatars/{name}/{image_id} raw
// avatar-serving route. Same raw-axum-handler pattern as
// `chat_attachment_route` above (Vec<u8>-via-server-fn-codec would break
// the <img src> render). Body is gated `#[cfg(feature = "server")]`; the
// module is otherwise empty on wasm.
pub mod bot_avatar_route;
// Phase 50.1 Plan 07 (D-02/D-14): profile-scoped, read-only public identity
// key (npub) derivation — `fetch_bot_npub`. Unconditional like
// `profile_api` — the `#[server]` macro handles the client/server split,
// and the buzz-dependent derivation is further gated with
// `#[cfg(feature = "buzz")]` inside the module (see its own doc comment).
pub mod buzz_npub_api;
// Phase 50.2 Plan 01 (D-13/D-23/D-05/D-20): the UI-owned group-chat room
// store — `create_room_impl`/`list_rooms_impl`/`load_room_impl`/
// `append_room_messages_impl`/`delete_room_impl`/`set_room_needs_you_impl`.
// Unconditional like `bot_meta_api` — no `#[server]` fns of its own (those
// live in `group_chat_api`), and internal items are gated with
// `#[cfg(feature = "server")]`. Its own sibling store/mutex, isolated from
// `state_store` and `BOT_META_LOCK` (module doc has the full
// persistence-shape record).
pub mod group_chat_store;
// Phase 50.2 Plan 01 (D-20/D-21/D-05/D-04/D-06): the group-chat round
// driver — `dispatch_group_round`/`create_group_room`/`list_group_rooms`/
// `load_group_room`/`delete_group_room`. Unconditional like `bot_meta_api`
// — the `#[server]` macro handles the client/server split, and internal
// helper fns (`dispatch_member_turn`, `build_group_turn_prompt`,
// `dispatch_group_round_impl`) are gated with
// `#[cfg(feature = "server")]`. A NEW caller of `cli_handoff::run_bot_handoff`
// — defines no second env-scrub allowlist, sanitizer, or argv builder.
pub mod group_chat_api;
// Phase 50.2 Plan 05 (D-21/D-10/D-11): the persisted, app-wide Group Chat
// Settings record — `load_group_chat_settings`/`save_group_chat_settings`.
// Unconditional like `bot_meta_api` — the `#[server]` macro handles the
// client/server split, and internal helper fns (`load_group_settings_impl`,
// `save_group_settings_impl`, `clamp_group_settings`) are gated with
// `#[cfg(feature = "server")]`. Its own sibling store/mutex, isolated from
// `state_store`, `BOT_META_LOCK` and `GROUP_CHAT_LOCK` (module doc has the
// full reasoning).
pub mod group_settings_api;
// Phase 50.2 Plan 15 (G-50.2-2a): the room-membership-update `#[server]`
// surface — `update_group_room_members`. Unconditional like
// `group_settings_api` — the `#[server]` macro handles the client/server
// split. Defines no second member validator and no second persistence
// path: it calls straight through to
// `group_chat_store::update_room_members_impl`, which itself calls the
// single `group_chat_store::validate_room_members` this plan extracted
// from `create_room_impl`. No UI change lands in this plan — plan 50.2-17
// owns the header's `Edit members` control.
pub mod group_members_api;
// Phase 50.2 Plan 07 (D-20/D-21): the @mention handoff middleware —
// `dispatch_mention_handoff`. Unconditional like `group_chat_api` — the
// `#[server]` macro handles the client/server split, and internal helper
// fns (`resolve_roster_mentions`, `resolve_roster_mentions_client`) are
// gated per-item (`resolve_roster_mentions` requires `feature = "server"`
// for its regex dependency; `resolve_roster_mentions_client` compiles
// unconditionally as its wasm-reachable isomorphic duplicate — module doc
// has the full reasoning). Deterministic host-orchestrated middleware, NOT
// an agent-callable tool (RESEARCH Pitfall 1 / Open Question 2, locked by
// this plan) — a NEW caller of `group_chat_api::dispatch_member_turn`,
// defining no second env-scrub allowlist, argv builder, or reply
// sanitizer.
pub mod mention_handoff_api;
// Phase 50.2 Plan 08 (D-13/D-23/D-11/D-04/D-21): the UI-owned per-bot Bot
// Chat thread store — `load_bot_chat_thread`/`append_bot_chat_entry`. The
// OPERATOR half of "every bot has a persistent Bot Chat" (plan 02 gave the
// BOT half). Unconditional like `bot_meta_api` — the `#[server]` macro
// handles the client/server split, and internal helper fns are gated with
// `#[cfg(feature = "server")]`. Its own sibling store/mutex, isolated from
// `state_store`, `BOT_META_LOCK` and `GROUP_CHAT_LOCK` (module doc has the
// full persistence-shape record). Never reads a profile's own `state.db`
// (D-13/D-23) — the operator's view is served from this UI-owned file.
pub mod bot_chat_store;
// Phase 48.2 Plan 01 (D-05/D-08/D-10/D-11/D-12/D-13/D-14/D-16/D-20): the
// Tools page's config-editing server surface — `get_tools_page_state`/
// `toggle_toolset`. Unconditional like `provider_config_api` — the
// `#[server]` macro handles the client/server split, and internal helper
// fns are gated with `#[cfg(not(target_arch = "wasm32"))]`.
pub mod tools_config_api;
// Phase 48.2 Plan 02 (D-01/D-02/D-03/D-04/D-12/D-13/D-22): MCP server admin
// surface — snippet parsing, probe-before-commit, honest status, non-blocking
// OAuth CONNECT, and live apply. Unconditional like `tools_config_api` — the
// `#[server]` macro handles the client/server split, internal helper fns are
// gated with `#[cfg(not(target_arch = "wasm32"))]`.
pub mod mcp_admin_api;
// Phase 48.2 Plan 09 (D-03/T-48.2-09-01): GET /oauth/mcp/callback — the
// public, unauthenticated raw axum route that finishes a web MCP OAuth
// authorization. Same raw-axum-handler pattern as `bot_avatar_route` above
// (an authorization server redirects a *browser* here with an ordinary GET,
// which the server-fn codec cannot serve as HTML). Body is gated
// `#[cfg(feature = "server")]`; the module is otherwise empty on wasm.
pub mod mcp_oauth_callback_route;
// Phase 48.2 Plan 07 (D-05/D-06/D-07/D-09/D-16/D-22): write-only,
// masked-presence tool-credential API — root scope (vault-first, honest
// plaintext fallback) and profile scope (the existing hardened profile-key
// `.env` write, reused unmodified). Unconditional like `tools_config_api` —
// the `#[server]` macro handles the client/server split, internal helper
// fns are gated with `#[cfg(not(target_arch = "wasm32"))]`.
pub mod tools_credentials_api;
// Phase 48.2 Plan 11 (D-03/D-14/D-16, G-48.2-6 slice a): read-only,
// scope-aware gateway liveness for the Tools page RUNTIME section —
// `get_gateway_runtime_status`. Unconditional like `tools_config_api` — the
// `#[server]` macro handles the client/server split, internal helper fns
// are gated with `#[cfg(not(target_arch = "wasm32"))]`. Never calls
// `check_tools_write_gate` — this is a read, not a write (module doc).
pub mod gateway_status_api;
// Phase 48.2 Plan 13 (G-48.2-6 slice b): start/stop/restart the
// `ironhermes gateway` process from the Tools page RUNTIME section —
// `stop_gateway`/`start_gateway`/`restart_gateway`. Unconditional like
// `gateway_status_api` above — the `#[server]` macro handles the
// client/server split, internal helper fns are gated with
// `#[cfg(not(target_arch = "wasm32"))]`. Full security posture (module doc):
// two independent config gates, SIGTERM-only stop, env-scrubbed detached
// spawn, audited every attempt including refusals.
pub mod gateway_control_api;
// Phase 48.2 Plan 12 (D-06/D-08/D-09/D-10/D-11/D-12/D-14, G-48.2-7): the
// Buzz platform's read/write surface over `gateway.platforms["buzz"]` — an
// explicit-allowlist DTO (never a passthrough of the shared
// `PlatformGatewayConfig`) plus the gated scope-resolved write path.
// Unconditional like `tools_config_api` — the `#[server]` macro handles the
// client/server split, internal helper fns are gated with
// `#[cfg(not(target_arch = "wasm32"))]`. Never touches `BUZZ_NSEC` — that
// stays entirely in `tools_credentials_api`/`buzz_npub_api` (module doc).
// Phase 49.3 Plan 01 extends this SAME file with the Telegram tracer slice
// (`TelegramPlatformView`/`get_telegram_platform_view`/
// `set_telegram_enabled`) — Discord/Slack land in later 49.3 plans.
pub mod platform_config_api;
// Phase 49.3 Plan 01: sibling server-fn module skeleton for the Gateway
// screen's expansion plans (02/04/05/06), so each owns a disjoint file and
// none of them re-touches this `mod.rs` concurrently. Each module is an
// empty compiling stub as of this plan — filled by its owning plan:
// `gateway_env_secret_api` (plan 02, root/profile `.env` writer for bot
// tokens + the REST API key), `webhook_route_api` (plan 04, webhook route
// CRUD), `api_server_config_api` (plan 05, REST API server host/port/
// public_opt_in), `gateway_platform_status_api` (plan 06, D-08 heartbeat
// status reader). Unconditional like `platform_config_api` — the
// `#[server]` macro (once each module has fns) handles the client/server
// split.
pub mod gateway_env_secret_api;
pub mod webhook_route_api;
pub mod api_server_config_api;
pub mod gateway_platform_status_api;
// Phase 49.4 Plan 05 (D-05..D-09): the Skills IMPORT / NEW SKILL server
// surface — `preview_skill_import`/`install_previewed_skill`/`create_skill`/
// `fork_bundled_skill`, plus the two new `HubSource` adapters (URL fetch,
// pasted content) that close the gaps `ironhermes-hub` doesn't cover.
// Unconditional like `tools_config_api` — the `#[server]` macro handles the
// client/server split; internal helper fns and both adapters are gated with
// `#[cfg(feature = "server")]` (they depend on `ironhermes-hub`, `reqwest`
// and `tokio`, none of which the wasm client build should see). The pure
// classifier (`classify_import_source`/`normalize_github_identifier`) stays
// ungated so it compiles on both targets and the client can reuse it.
pub mod skills_import_api;
