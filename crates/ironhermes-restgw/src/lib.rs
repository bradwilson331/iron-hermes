//! `ironhermes-restgw` — inbound REST gateway adapters for IronHermes
//! (Phase 36.7.1).
//!
//! Two adapters share this crate:
//! - [`webhook`]: the INBOUND webhook adapter (`WebhookAdapter`). Not to be
//!   confused with `ironhermes_hooks::webhook`, which is the OUTBOUND
//!   hook-event delivery module — the direction is opposite and the two
//!   modules are unrelated.
//! - [`api_server`]: the HTTP REST API server adapter (`ApiServerAdapter`) —
//!   the OpenAI-compatible-shaped surface, on its own listener/port.
//!
//! Both adapters share [`bind_guard`], the fail-closed D-07 predicate that
//! must be evaluated before either adapter opens a `TcpListener`.

pub mod api_server;
pub mod bind_guard;
pub mod webhook;
