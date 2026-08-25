//! Webhook route configuration surface for the adapter (D-09).
//!
//! The serde-facing route SCHEMA ([`WebhookRoute`] and its nested enums)
//! lives in `ironhermes_core::webhook_route` — not here — because
//! `ironhermes_core::config::PlatformGatewayConfig` must hold a
//! `Vec<WebhookRoute>` field, and this crate depends on `ironhermes-core`
//! (the reverse dependency would be a cycle). This module re-exports those
//! types so the natural import path for adapter code is
//! `ironhermes_restgw::webhook::route_config::{WebhookRoute, ...}`, and adds
//! [`WebhookRoutesConfig`] — the RUNTIME input `WebhookAdapter::new` takes,
//! assembled by the caller (`GatewayRunner::start()` in Plan 03, or a test
//! harness) from the listener-level fields plus the route list.

pub use ironhermes_core::webhook_route::{
    DeliverTarget, OutboundAuth, RouteRails, SessionMode, SignatureKind, WebhookRoute,
};

/// Runtime input to [`crate::webhook::WebhookAdapter::new`]: the listener
/// configuration (bind host/port, whether the operator explicitly opted
/// into a non-loopback bind, and the externally-visible base URL used to
/// reconstruct the exact request URL a sender addressed — see
/// `crate::webhook::mod`'s per-request handler doc) plus the route table
/// itself.
///
/// Not serde — `PlatformGatewayConfig` (in `ironhermes-core`) is the
/// serde-facing config; this struct is assembled from it at the
/// construction call site.
#[derive(Debug, Clone, Default)]
pub struct WebhookRoutesConfig {
    /// Bind host, e.g. `"127.0.0.1"` or `"0.0.0.0"`.
    pub host: String,
    /// Bind port.
    pub port: u16,
    /// Whether the operator explicitly opted into exposing this listener
    /// publicly. Declared for parity with the REST API server adapter's own
    /// D-07 gate (which needs this flag); the webhook adapter's own D-10
    /// gate is purely route-shaped (does any route select `signature:
    /// none`) and does not consume this field directly.
    pub public_opt_in: bool,
    /// The externally-visible base URL (scheme + host, no trailing slash)
    /// for this listener, when it sits behind a reverse proxy. When
    /// `None`, the per-request handler reconstructs the base from the
    /// inbound `Host` header. A route using `signature: twilio` (Plan 02)
    /// behind a proxy MUST set this — Twilio signs the externally visible
    /// URL byte for byte.
    pub external_base_url: Option<String>,
    /// The configured routes. Read once at construction; owned immutably
    /// thereafter (D-17).
    pub routes: Vec<WebhookRoute>,
}
