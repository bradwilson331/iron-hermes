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
#[cfg(feature = "server")]
pub mod state;
pub mod ws;
// Phase 36.3.7.11 Plan 01 — kanban dashboard read surface + WS.
// `kanban_api` is unconditional (the `#[server]` macros handle the
// client/server split). `kanban_ws` is unconditional too: like `ws`, the
// route fn returns a server-side handler, but the WASM build still needs
// the route stub for client-side URL generation; `#[cfg(feature = "server")]`
// gates the internals.
pub mod kanban_api;
pub mod kanban_ws;
// Phase 36.17.7 D-02-a: web-runtime AudioDispatcher impl.
// Like `ws` and `kanban_ws`, the module is gated internally with
// `#![cfg(feature = "server")]` so the WASM client never sees it.
pub mod web_audio_dispatcher;
// Phase 36.17.7 D-02-c: GET /audio/:uuid replay route.
// Like `ws` and `kanban_ws`, exposes a `#[get]` server function;
// the route body is gated `#[cfg(feature = "server")]`.
pub mod audio_route;
// Phase 36.17.7 D-02-d: audio cache GC (startup sweep + periodic loop).
// Gated internally with `#![cfg(feature = "server")]`.
#[cfg(feature = "server")]
pub mod audio_cache;
