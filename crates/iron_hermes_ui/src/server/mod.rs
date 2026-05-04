//! Server-side modules for the Dioxus UI backend.
//!
//! `api` is compiled on BOTH client and server — the `#[get]`/`#[post]` macros
//! generate HTTP-call stubs on the client and API endpoints on the server.
//! Only `ws` and `state` (pure server-side logic) stay behind `#[cfg(feature = "server")]`.

pub mod api;
#[cfg(feature = "server")]
pub mod ws;
#[cfg(feature = "server")]
pub mod state;
